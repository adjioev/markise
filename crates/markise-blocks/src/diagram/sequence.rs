//! Sequence diagram model and layout, shared by Mermaid `sequenceDiagram` and
//! js-sequence-diagrams (`sequence`).

use std::collections::HashMap;

use markise_theme::Rgba;

use super::measure::label_size;
use super::scene::{Anchor, Canvas, Paint, Role, Scene, Seg, Stroke};
use super::text::label_lines;
use super::{DiagramError, Limits, RenderCtx};

#[derive(Clone, Debug, PartialEq)]
pub struct Participant {
    pub id: String,
    pub label: Vec<String>,
    /// Drawn as a stick figure (`actor`).
    pub actor: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArrowHead {
    #[default]
    None,
    Filled,
    Open,
    Cross,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePlace {
    LeftOf(usize),
    RightOf(usize),
    Over(usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Message {
        from: usize,
        to: usize,
        text: Vec<String>,
        dotted: bool,
        head: ArrowHead,
        /// Arrow head at `from` as well (`<<->>`).
        both: bool,
    },
    Note {
        place: NotePlace,
        text: Vec<String>,
    },
    Activate(usize),
    Deactivate(usize),
    /// `loop`, `alt`, `opt`, `par`, `critical`, `break`, or `rect` (background only).
    BlockStart {
        kind: String,
        label: Vec<String>,
        fill: Option<Rgba>,
    },
    /// `else`, `and`, `option`.
    BlockSection {
        label: Vec<String>,
    },
    BlockEnd,
}

/// A parsed sequence diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sequence {
    pub title: Option<Vec<String>>,
    pub participants: Vec<Participant>,
    pub events: Vec<Event>,
    pub autonumber: bool,
}

/// Incremental builder enforcing [`Limits`].
pub(crate) struct SequenceBuilder<'l> {
    pub seq: Sequence,
    index: HashMap<String, usize>,
    pub limits: &'l Limits,
    depth: usize,
}

impl<'l> SequenceBuilder<'l> {
    pub fn new(limits: &'l Limits) -> Self {
        SequenceBuilder {
            seq: Sequence::default(),
            index: HashMap::new(),
            limits,
            depth: 0,
        }
    }

    /// Declares or looks up a participant; a declaration may set its label and kind.
    pub fn participant(
        &mut self,
        id: &str,
        label: Option<&str>,
        actor: bool,
        backslash_n: bool,
    ) -> Result<usize, DiagramError> {
        let max = self.limits.max_label_chars;
        if let Some(&i) = self.index.get(id) {
            if let Some(l) = label {
                self.seq.participants[i].label = label_lines(l, backslash_n, max);
            }
            self.seq.participants[i].actor |= actor;
            return Ok(i);
        }
        if self.seq.participants.len() >= self.limits.max_nodes.min(200) {
            return Err(DiagramError::TooLarge(format!(
                "more than {} participants",
                self.limits.max_nodes.min(200)
            )));
        }
        let i = self.seq.participants.len();
        self.seq.participants.push(Participant {
            id: id.to_string(),
            label: label_lines(label.unwrap_or(id), backslash_n, max),
            actor,
        });
        self.index.insert(id.to_string(), i);
        Ok(i)
    }

    pub fn event(&mut self, e: Event) -> Result<(), DiagramError> {
        if self.seq.events.len() >= self.limits.max_edges * 2 {
            return Err(DiagramError::TooLarge(format!(
                "more than {} statements",
                self.limits.max_edges * 2
            )));
        }
        match &e {
            Event::BlockStart { .. } => {
                if self.depth >= self.limits.max_depth {
                    return Err(DiagramError::TooLarge(format!(
                        "blocks nested deeper than {}",
                        self.limits.max_depth
                    )));
                }
                self.depth += 1;
            }
            Event::BlockEnd => {
                if self.depth == 0 {
                    return Err(DiagramError::Syntax {
                        line: 0,
                        message: "`end` without a block".into(),
                    });
                }
                self.depth -= 1;
            }
            _ => {}
        }
        self.seq.events.push(e);
        Ok(())
    }

    pub fn finish(mut self) -> Sequence {
        // close unterminated blocks so layout always sees balanced frames
        for _ in 0..self.depth {
            self.seq.events.push(Event::BlockEnd);
        }
        self.seq
    }
}

// ------------------------------------------------------------------------------------ layout

struct Frame {
    kind: String,
    label: Vec<String>,
    fill: Option<Rgba>,
    top: f32,
    sections: Vec<(f32, Vec<String>)>,
    /// Horizontal extent of everything drawn inside (participants, labels, nested frames).
    lo: f32,
    hi: f32,
    bottom: f32,
    /// Final left/right edges (set when the frame closes).
    x0: f32,
    x1: f32,
}

/// Widens every open frame to cover `[a, b]`.
fn touch(frames: &mut [Frame], open: &[usize], a: f32, b: f32) {
    for &f in open {
        frames[f].lo = frames[f].lo.min(a);
        frames[f].hi = frames[f].hi.max(b);
    }
}

/// Lays out a sequence diagram.
pub fn layout(seq: &Sequence, ctx: &RenderCtx) -> Result<Scene, DiagramError> {
    let m = ctx.measure;
    let fs = ctx.font_size;
    let lh = m.line_height(fs);
    let pad = (fs * 0.7).round();
    let n = seq.participants.len();
    if n == 0 {
        return Err(DiagramError::Empty);
    }
    ctx.check()?;

    // participant boxes
    let boxes: Vec<(f32, f32)> = seq
        .participants
        .iter()
        .map(|p| {
            let (w, h) = label_size(m, &p.label, fs, false);
            let w = (w + 2.0 * pad).max(fs * 5.0);
            let h = if p.actor { h + fs * 3.2 } else { h + 2.0 * pad };
            (w, h)
        })
        .collect();
    let box_h = boxes.iter().map(|b| b.1).fold(0.0, f32::max);
    let gap = fs * 3.5;
    let act_w = fs * 0.7;
    // self messages: loop size and the label to its right
    let loop_w = fs * 2.2;
    let loop_h = fs * 1.8;
    let self_label_dx = act_w * 1.5 + loop_w + fs * 0.6;

    // horizontal constraints: need[k] = list of (i, distance) with i < k
    let mut need: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n + 1];
    let num_w = if seq.autonumber { fs * 1.6 } else { 0.0 };
    for (k, e) in seq.events.iter().enumerate() {
        if k % 256 == 0 {
            ctx.check()?;
        }
        match e {
            Event::Message { from, to, text, .. } => {
                let (w, _) = label_size(m, text, fs, false);
                let (a, b) = ((*from).min(*to), (*from).max(*to));
                if a == b {
                    need[a + 1].push((a, self_label_dx + w + fs * 1.5));
                } else {
                    need[b].push((a, w + 2.0 * pad + num_w + fs));
                }
            }
            Event::Note { place, text } => {
                let (w, _) = label_size(m, text, fs, false);
                let w = w + 2.0 * pad;
                match *place {
                    NotePlace::RightOf(p) => need[p + 1].push((p, w + fs * 2.0)),
                    NotePlace::LeftOf(p) => {
                        if p > 0 {
                            need[p].push((p - 1, w + fs * 2.0));
                        }
                    }
                    NotePlace::Over(a, b) => {
                        let (a, b) = (a.min(b), a.max(b));
                        if a != b {
                            need[b].push((a, w - fs * 2.0));
                        } else if a + 1 < n {
                            need[a + 1].push((a, w / 2.0 + boxes[a + 1].0 / 2.0 + fs));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut xs = vec![0.0f32; n];
    for k in 0..n {
        let mut x = if k == 0 {
            boxes[0].0 / 2.0
        } else {
            xs[k - 1] + (boxes[k - 1].0 + boxes[k].0) / 2.0 + gap
        };
        for &(i, d) in &need[k] {
            x = x.max(xs[i] + d);
        }
        xs[k] = x;
    }
    let span = |a: usize, b: usize| (xs[a] - boxes[a].0 / 2.0, xs[b] + boxes[b].0 / 2.0);

    let mut c = Canvas::new(m);
    let mut y = 0.0f32;
    if let Some(t) = &seq.title {
        let (_, h) = label_size(m, t, fs * 1.15, true);
        let cx = (xs[0] + xs[n - 1]) / 2.0;
        c.text_block([cx, y + h / 2.0], t, fs * 1.15, true, Role::Text.into());
        y += h + pad;
    }
    let top_y = y;
    y += box_h + fs * 1.8;

    // events
    let thin = Stroke::solid(1.0, Role::LifeLine);
    let mut active: Vec<Vec<f32>> = vec![Vec::new(); n];
    let mut acts: Vec<([f32; 4], usize)> = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut notes: Vec<([f32; 4], Vec<String>)> = Vec::new();
    let mut number = 0usize;
    let tab_h = lh + fs * 0.2;
    let tab_w = |kind: &str| m.width(kind, fs * 0.9, true) + fs * 1.4;
    for (k, e) in seq.events.iter().enumerate() {
        if k % 256 == 0 {
            ctx.check()?;
        }
        match e {
            Event::Message {
                from,
                to,
                text,
                dotted,
                head,
                both,
            } => {
                let (from, to) = (*from, *to);
                let (sa, sb) = span(from.min(to), from.max(to));
                touch(&mut frames, &open, sa, sb);
                let mut label = text.clone();
                if label.len() == 1 && label[0].is_empty() {
                    label.clear();
                }
                let (lw, th) = if label.is_empty() {
                    (0.0, 0.0)
                } else {
                    label_size(m, &label, fs, false)
                };
                let off = |p: usize, right: bool| -> f32 {
                    let d = active[p].len() as f32 * act_w / 2.0;
                    if right { d } else { -d }
                };
                let stroke = if *dotted {
                    Stroke::dashed(1.4, Role::EdgeStroke, [5.0, 4.0])
                } else {
                    Stroke::solid(1.4, Role::EdgeStroke)
                };
                if seq.autonumber {
                    number += 1;
                }
                if from == to {
                    let x = xs[from] + off(from, true);
                    // a label taller than the loop is centred on it: lower the loop so the
                    // label's first line stays below the previous message
                    let y0 = y + fs * 0.4 + ((th - loop_h) / 2.0).max(0.0);
                    let segs = vec![
                        Seg::MoveTo([x, y0]),
                        Seg::CubicTo(
                            [x + loop_w * 1.3, y0 - loop_h * 0.1],
                            [x + loop_w * 1.3, y0 + loop_h * 1.1],
                            [x + 2.0, y0 + loop_h],
                        ),
                    ];
                    c.path(segs, None, Some(stroke));
                    arrow_head(&mut c, [x + loop_w, y0 + loop_h], [x + 1.0, y0 + loop_h], *head);
                    if seq.autonumber {
                        number_badge(&mut c, [x, y0], number, fs);
                    }
                    // the label sits to the right of the loop, clear of the activation bar
                    let lx = xs[from] + self_label_dx;
                    let ly = y0 + loop_h / 2.0 - th / 2.0;
                    for (i, l) in label.iter().enumerate() {
                        c.text(
                            [lx, ly + lh * (i as f32 + 0.5)],
                            l,
                            fs,
                            Anchor::Start,
                            false,
                            Role::EdgeText.into(),
                        );
                    }
                    touch(&mut frames, &open, sa, (lx + lw).max(x + loop_w * 1.3));
                    y = (y0 + loop_h).max(y0 + loop_h / 2.0 + th / 2.0) + fs * 1.6;
                } else {
                    let right = to > from;
                    let x1 = xs[from] + off(from, right);
                    let x2 = xs[to] + off(to, !right);
                    let ly = y + th + fs * 0.5;
                    let cx = (x1 + x2) / 2.0;
                    for (i, l) in label.iter().enumerate() {
                        c.text(
                            [cx, y + lh * (i as f32 + 0.5)],
                            l,
                            fs,
                            Anchor::Middle,
                            false,
                            Role::EdgeText.into(),
                        );
                    }
                    touch(&mut frames, &open, cx - lw / 2.0, cx + lw / 2.0);
                    let dir = if right { 1.0 } else { -1.0 };
                    let hl = if *head == ArrowHead::None { 0.0 } else { fs * 0.6 };
                    let tl = if *both { fs * 0.6 } else { 0.0 };
                    c.line([x1 + dir * tl, ly], [x2 - dir * hl, ly], stroke);
                    arrow_head(&mut c, [x1, ly], [x2, ly], *head);
                    if *both {
                        arrow_head(&mut c, [x2, ly], [x1, ly], *head);
                    }
                    if seq.autonumber {
                        number_badge(&mut c, [x1, ly], number, fs);
                    }
                    y = ly + fs * 1.8;
                }
            }
            Event::Note { place, text } => {
                let (tw, th) = label_size(m, text, fs, false);
                let w = tw + 2.0 * pad;
                let h = th + 2.0 * pad * 0.7;
                let (x0, w) = match *place {
                    NotePlace::LeftOf(p) => (xs[p] - fs * 0.8 - w, w),
                    NotePlace::RightOf(p) => (xs[p] + fs * 0.8, w),
                    NotePlace::Over(a, b) => {
                        let (a, b) = (a.min(b), a.max(b));
                        if a == b {
                            (xs[a] - w / 2.0, w)
                        } else {
                            let w2 = w.max(xs[b] - xs[a] + fs * 2.0);
                            ((xs[a] + xs[b]) / 2.0 - w2 / 2.0, w2)
                        }
                    }
                };
                touch(&mut frames, &open, x0, x0 + w);
                notes.push(([x0, y, w, h], text.clone()));
                y += h + fs * 1.2;
            }
            Event::Activate(p) => {
                if active[*p].len() < 16 {
                    active[*p].push(y - fs * 0.6);
                }
            }
            Event::Deactivate(p) => {
                let depth = active[*p].len();
                if let Some(y0) = active[*p].pop() {
                    let x = xs[*p] - act_w / 2.0 + (depth as f32 - 1.0) * act_w / 2.0;
                    acts.push(([x, y0, act_w, (y - fs * 0.6 - y0).max(fs * 0.5)], depth));
                }
            }
            Event::BlockStart { kind, label, fill } => {
                frames.push(Frame {
                    kind: kind.clone(),
                    label: label.clone(),
                    fill: *fill,
                    top: y,
                    sections: Vec::new(),
                    lo: f32::INFINITY,
                    hi: f32::NEG_INFINITY,
                    bottom: y,
                    x0: 0.0,
                    x1: 0.0,
                });
                open.push(frames.len() - 1);
                let header = if kind == "rect" {
                    fs * 0.8
                } else {
                    tab_h.max(label_size(m, label, fs, false).1) + fs * 0.9
                };
                y += header;
            }
            Event::BlockSection { label } => {
                if let Some(&f) = open.last() {
                    y += fs * 0.3;
                    frames[f].sections.push((y, label.clone()));
                    y += label_size(m, label, fs, false).1 + fs * 0.9;
                }
            }
            Event::BlockEnd => {
                if let Some(f) = open.pop() {
                    y += fs * 0.5;
                    frames[f].bottom = y;
                    y += fs * 0.9;
                    let fr = &frames[f];
                    let (mut lo, mut hi) = if fr.lo.is_finite() {
                        (fr.lo, fr.hi)
                    } else {
                        span(0, n - 1)
                    };
                    lo -= fs * 0.8;
                    hi += fs * 0.8;
                    // wide enough for the tab and the condition, and for every section label
                    let mut min_w = if fr.kind == "rect" { 0.0 } else { tab_w(&fr.kind) };
                    if fr.label.iter().any(|l| !l.is_empty()) {
                        min_w += fs * 0.6 + label_size(m, &bracket(&fr.label), fs, false).0 + fs * 1.0;
                    }
                    for (_, l) in &fr.sections {
                        if l.iter().any(|l| !l.is_empty()) {
                            min_w = min_w.max(label_size(m, &bracket(l), fs, false).0 + fs * 2.0);
                        }
                    }
                    if hi - lo < min_w {
                        hi = lo + min_w;
                    }
                    frames[f].x0 = lo;
                    frames[f].x1 = hi;
                    // the parent encloses this frame with its own margin
                    touch(&mut frames, &open, lo, hi);
                }
            }
        }
    }
    // close activations still open
    for (p, stack) in active.iter_mut().enumerate() {
        while let Some(y0) = stack.pop() {
            let depth = stack.len() + 1;
            let x = xs[p] - act_w / 2.0 + (depth as f32 - 1.0) * act_w / 2.0;
            acts.push(([x, y0, act_w, (y - y0).max(fs * 0.5)], depth));
        }
    }
    let bottom_y = y + fs * 0.5;

    // painter's order: rect backgrounds, lifelines, activations, frames, then the rest
    let mut back = Canvas::new(m);
    for f in frames.iter().filter(|f| f.kind == "rect") {
        back.rect(
            [f.x0, f.top, f.x1 - f.x0, f.bottom - f.top],
            0.0,
            Some(f.fill.map_or(Role::ClusterFill.into(), Paint::Custom)),
            None,
        );
    }
    for &x in &xs {
        back.line(
            [x, top_y + box_h],
            [x, bottom_y],
            Stroke::dashed(1.0, Role::LifeLine, [3.0, 3.0]),
        );
    }
    acts.sort_by_key(|a| a.1);
    for (r, _) in &acts {
        back.rect(*r, 0.0, Some(Role::ActivationFill.into()), Some(thin));
    }
    for f in frames.iter().filter(|f| f.kind != "rect") {
        let (x0, x1) = (f.x0, f.x1);
        let fr = Stroke::solid(1.2, Role::FrameStroke);
        back.rect([x0, f.top, x1 - x0, f.bottom - f.top], 0.0, None, Some(fr));
        let tw = tab_w(&f.kind);
        back.polygon(
            &[
                [x0, f.top],
                [x0 + tw, f.top],
                [x0 + tw, f.top + tab_h * 0.7],
                [x0 + tw - fs * 0.5, f.top + tab_h],
                [x0, f.top + tab_h],
            ],
            Some(Role::FrameLabelFill.into()),
            Some(fr),
        );
        back.text(
            [x0 + fs * 0.5, f.top + tab_h / 2.0],
            &f.kind,
            fs * 0.9,
            Anchor::Start,
            true,
            Role::FrameText.into(),
        );
        if f.label.iter().any(|l| !l.is_empty()) {
            // the condition starts right of the tab, on the tab's first line
            for (i, l) in bracket(&f.label).iter().enumerate() {
                back.text(
                    [x0 + tw + fs * 0.6, f.top + tab_h / 2.0 + lh * i as f32],
                    l,
                    fs,
                    Anchor::Start,
                    false,
                    Role::FrameText.into(),
                );
            }
        }
        for (sy, label) in &f.sections {
            back.line([x0, *sy], [x1, *sy], Stroke::dashed(1.0, Role::FrameStroke, [4.0, 3.0]));
            if label.iter().any(|l| !l.is_empty()) {
                let lines = bracket(label);
                back.text_block(
                    [(x0 + x1) / 2.0, sy + lh * lines.len() as f32 / 2.0 + fs * 0.3],
                    &lines,
                    fs,
                    false,
                    Role::FrameText.into(),
                );
            }
        }
    }
    // participants top and bottom
    for (i, p) in seq.participants.iter().enumerate() {
        for top in [top_y, bottom_y] {
            participant(&mut c, p, xs[i], top, boxes[i], box_h, fs);
        }
    }
    for (rect, text) in &notes {
        note(&mut c, *rect, text, fs);
    }
    back.append(c);
    Ok(back.finish(fs))
}

fn bracket(lines: &[String]) -> Vec<String> {
    let mut v = lines.to_vec();
    if let Some(f) = v.first_mut() {
        f.insert(0, '[');
    }
    if let Some(l) = v.last_mut() {
        l.push(']');
    }
    v
}

fn participant(c: &mut Canvas, p: &Participant, x: f32, top: f32, (w, h): (f32, f32), box_h: f32, fs: f32) {
    if p.actor {
        let s = Stroke::solid(1.4, Role::ActorStroke);
        let hy = top + box_h - h + fs * 0.5;
        let r = fs * 0.45;
        c.ellipse([x, hy + r], [r, r], Some(Role::ActorFill.into()), Some(s));
        let neck = hy + 2.0 * r;
        let hip = neck + fs * 1.0;
        c.line([x, neck], [x, hip], s);
        c.line([x - fs * 0.8, neck + fs * 0.4], [x + fs * 0.8, neck + fs * 0.4], s);
        c.line([x, hip], [x - fs * 0.7, hip + fs * 0.9], s);
        c.line([x, hip], [x + fs * 0.7, hip + fs * 0.9], s);
        let lh = c.measure.line_height(fs);
        let ty = hip + fs * 1.0 + lh * p.label.len() as f32 / 2.0;
        c.text_block([x, ty], &p.label, fs, false, Role::ActorText.into());
    } else {
        let y0 = top + (box_h - h);
        c.rect(
            [x - w / 2.0, y0, w, h],
            3.0,
            Some(Role::ActorFill.into()),
            Some(Stroke::solid(1.2, Role::ActorStroke)),
        );
        c.text_block([x, y0 + h / 2.0], &p.label, fs, false, Role::ActorText.into());
    }
}

fn note(c: &mut Canvas, rect: [f32; 4], text: &[String], fs: f32) {
    let [x, y, w, h] = rect;
    let fold = (fs * 0.6).min(h / 2.0);
    c.polygon(
        &[[x, y], [x + w - fold, y], [x + w, y + fold], [x + w, y + h], [x, y + h]],
        Some(Role::NoteFill.into()),
        Some(Stroke::solid(1.0, Role::NoteStroke)),
    );
    c.text_block([x + w / 2.0, y + h / 2.0], text, fs, false, Role::NoteText.into());
}

fn number_badge(c: &mut Canvas, at: [f32; 2], n: usize, fs: f32) {
    let r = fs * 0.62;
    c.ellipse(at, [r, r], Some(Role::EdgeStroke.into()), None);
    c.text(
        at,
        &n.to_string(),
        fs * 0.7,
        Anchor::Middle,
        true,
        Role::Background.into(),
    );
}

/// Draws a head at `tip`, pointing from `from` towards `tip`.
pub(crate) fn arrow_head(c: &mut Canvas, from: [f32; 2], tip: [f32; 2], head: ArrowHead) {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 || !len.is_finite() {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let l = 9.0;
    let hw = 4.0;
    let base = [tip[0] - ux * l, tip[1] - uy * l];
    match head {
        ArrowHead::None => {}
        ArrowHead::Filled => c.polygon(
            &[
                tip,
                [base[0] + px * hw, base[1] + py * hw],
                [base[0] - px * hw, base[1] - py * hw],
            ],
            Some(Role::EdgeStroke.into()),
            None,
        ),
        ArrowHead::Open => {
            let s = Stroke::solid(1.4, Role::EdgeStroke);
            c.path(
                vec![
                    Seg::MoveTo([base[0] + px * hw, base[1] + py * hw]),
                    Seg::LineTo(tip),
                    Seg::LineTo([base[0] - px * hw, base[1] - py * hw]),
                ],
                None,
                Some(s),
            );
        }
        ArrowHead::Cross => {
            let s = Stroke::solid(1.6, Role::EdgeStroke);
            let k = 4.0;
            let cx = [tip[0] - ux * k, tip[1] - uy * k];
            c.line(
                [cx[0] - k * (ux + px), cx[1] - k * (uy + py)],
                [cx[0] + k * (ux + px), cx[1] + k * (uy + py)],
                s,
            );
            c.line(
                [cx[0] - k * (ux - px), cx[1] - k * (uy - py)],
                [cx[0] + k * (ux - px), cx[1] + k * (uy - py)],
                s,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::diagram::scene::{Item, Paint, Role};
    use crate::diagram::{ApproxMeasure, DiagramLang, RenderCtx, Scene, TextMeasure, render};

    const DEMO: &str = "sequenceDiagram
    autonumber
    actor User
    participant App as Markise
    participant Disk
    User->>App: Open file
    activate App
    App->>Disk: Read
    alt file is huge
        App->>App: lazy layout
    else small file
        App->>App: full layout
    end
    deactivate App
    loop Autosave for every edit of the document
        User->>App: Edit
    end
";

    fn text(s: &Scene, t: &str) -> (f32, f32, f32) {
        s.items
            .iter()
            .find_map(|i| match i {
                Item::Text { text, pos, width, .. } if text == t => Some((pos[0], pos[1], *width)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text {t:?}"))
    }

    /// Frame rectangles: stroked with the frame role, not filled.
    fn frames(s: &Scene) -> Vec<[f32; 4]> {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::Rect {
                    rect,
                    fill: None,
                    stroke: Some(st),
                    ..
                } if st.paint == Paint::Role(Role::FrameStroke) => Some(*rect),
                _ => None,
            })
            .collect()
    }

    fn inside(f: [f32; 4], x0: f32, x1: f32, y: f32) -> bool {
        x0 >= f[0] && x1 <= f[0] + f[2] && y >= f[1] && y <= f[1] + f[3]
    }

    /// Owner review: frames fit their labels, self-message labels sit right of the loop and
    /// inside the frame, a frame spanning the first participant stays inside the scene.
    #[test]
    fn frames_fit_labels_and_self_messages_clear_the_bar() {
        let m = ApproxMeasure;
        let s = render(DiagramLang::Mermaid, DEMO, &RenderCtx::new(&m, 14.0)).unwrap();
        assert!(s.is_well_formed());
        let fr = frames(&s);
        assert_eq!(fr.len(), 2);
        let all_left = fr.iter().map(|f| f[0]).fold(f32::INFINITY, f32::min);
        assert!(all_left >= 0.0, "frames start inside the scene");
        for label in ["[file is huge]", "[small file]", "lazy layout", "full layout"] {
            let (x, y, w) = text(&s, label);
            // condition labels are start-anchored (after the tab); section labels centered
            let (x0, x1) = if label == "[small file]" {
                (x - w / 2.0, x + w / 2.0)
            } else {
                (x, x + w)
            };
            assert!(
                fr.iter().any(|f| inside(*f, x0, x1, y)),
                "{label} inside its frame ({x0}..{x1})"
            );
        }
        // the condition starts after the "alt" tab
        let alt = fr
            .iter()
            .find(|f| {
                inside(
                    **f,
                    text(&s, "[file is huge]").0,
                    text(&s, "[file is huge]").0,
                    text(&s, "[file is huge]").1,
                )
            })
            .unwrap();
        let tab = m.width("alt", 14.0 * 0.9, true) + 14.0 * 1.4;
        assert!(text(&s, "[file is huge]").0 >= alt[0] + tab);
        // the self-message label starts right of the activation bar and the loop
        let (lx, _, _) = text(&s, "lazy layout");
        let markise_x = text(&s, "Markise").0;
        assert!(lx >= markise_x + 14.0 * 0.7 * 1.5 + 14.0 * 2.2, "{lx} vs {markise_x}");
        // the long loop condition fits its frame
        let (cx, cy, cw) = text(&s, "[Autosave for every edit of the document]");
        assert!(fr.iter().any(|f| inside(*f, cx, cx + cw, cy)));
    }

    /// Text boxes `(x0, y0, x1, y1)` of the scene (line-height tall), excluding the small bold
    /// autonumber badges that sit on their arrows by design.
    fn text_boxes(s: &Scene, m: &dyn TextMeasure) -> Vec<(String, [f32; 4])> {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::Text {
                    text,
                    pos,
                    width,
                    size,
                    anchor,
                    bold,
                    ..
                } if !(*bold && *size < 12.0) => {
                    let h = m.line_height(*size) / 2.0;
                    let x0 = match anchor {
                        crate::diagram::Anchor::Start => pos[0],
                        crate::diagram::Anchor::Middle => pos[0] - width / 2.0,
                        crate::diagram::Anchor::End => pos[0] - width,
                    };
                    Some((text.clone(), [x0, pos[1] - h, x0 + width, pos[1] + h]))
                }
                _ => None,
            })
            .collect()
    }

    /// Horizontal path segments (message arrows, frame borders, section separators).
    fn hlines(s: &Scene) -> Vec<(f32, f32, f32)> {
        use crate::diagram::Seg;
        let mut out = Vec::new();
        for it in &s.items {
            if let Item::Path { segs, .. } = it {
                for w in segs.windows(2) {
                    if let (Seg::MoveTo(a) | Seg::LineTo(a), Seg::LineTo(b)) = (w[0], w[1])
                        && (a[1] - b[1]).abs() < 0.01
                        && (a[0] - b[0]).abs() > 12.0
                    {
                        out.push((a[0].min(b[0]), a[0].max(b[0]), a[1]));
                    }
                }
            }
            if let Item::Rect {
                rect,
                fill: None,
                stroke: Some(_),
                ..
            } = it
            {
                out.push((rect[0], rect[0] + rect[2], rect[1]));
                out.push((rect[0], rect[0] + rect[2], rect[1] + rect[3]));
            }
        }
        out
    }

    /// Codex round 2: multiline labels (self messages, messages, notes, frame conditions and
    /// sections) never overlap other text or cross arrows and frame lines.
    #[test]
    fn multiline_labels_do_not_overlap() {
        let six = "one<br>two<br>three<br>four<br>five<br>six";
        let cases = [
            format!("sequenceDiagram\nparticipant A\nparticipant B\nA->>B: previous\nA->>A: {six}\nB->>A: after"),
            format!("sequenceDiagram\nA->>B: {six}\nB->>B: {six}\nB-->>A: {six}"),
            format!(
                "sequenceDiagram\nA->>B: x\nNote over A,B: {six}\nNote left of A: {six}\nNote right of B: {six}\nA->>B: y"
            ),
            format!(
                "sequenceDiagram\nA->>B: x\nalt {six}\nA->>A: {six}\nelse {six}\nB->>A: {six}\nend\nloop {six}\nA->>B: z\nend"
            ),
        ];
        let m = ApproxMeasure;
        for src in &cases {
            let s = render(DiagramLang::Mermaid, src, &RenderCtx::new(&m, 14.0)).unwrap();
            let boxes = text_boxes(&s, &m);
            let lines = hlines(&s);
            let eps = 0.5;
            for (i, (ta, a)) in boxes.iter().enumerate() {
                for (tb, b) in boxes.iter().skip(i + 1) {
                    let overlap = a[0] < b[2] - eps && b[0] < a[2] - eps && a[1] < b[3] - eps && b[1] < a[3] - eps;
                    // participant labels repeat at the bottom; the same text at the same place is one label
                    assert!(!overlap, "{ta:?} {a:?} overlaps {tb:?} {b:?} in\n{src}");
                }
                for &(x0, x1, y) in &lines {
                    let crosses = y > a[1] + eps && y < a[3] - eps && x0 < a[2] - eps && x1 > a[0] + eps;
                    assert!(
                        !crosses,
                        "{ta:?} {a:?} crossed by a line at y={y} ({x0}..{x1}) in\n{src}"
                    );
                }
            }
        }
    }

    /// The reported case: the first line of a six-line self label stays below the previous
    /// arrow.
    #[test]
    fn tall_self_label_stays_below_the_previous_arrow() {
        let m = ApproxMeasure;
        let src = "sequenceDiagram\nparticipant A\nparticipant B\nA->>B: previous\nA->>A: one<br>two<br>three<br>four<br>five<br>six";
        let s = render(DiagramLang::Mermaid, src, &RenderCtx::new(&m, 14.0)).unwrap();
        let arrow_y = hlines(&s).iter().map(|l| l.2).fold(f32::INFINITY, f32::min);
        let (_, y, _) = text(&s, "one");
        assert!(
            y - m.line_height(14.0) / 2.0 > arrow_y,
            "first line top {} vs arrow {arrow_y}",
            y
        );
    }
}
