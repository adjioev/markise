//! Caret geometry on laid-out items (ADR 0006 §2.3, M2b): document positions ↔ text fragments
//! through the item-relative source maps, hit testing, caret and selection rectangles, and
//! caret movement (grapheme, word, visual line, line start/end, page, document).
//!
//! Positions are source offsets ([`DocPos`]); a caret *stop* is any source offset a rendered
//! grapheme boundary maps to, so hidden markers, container prefixes and atoms are skipped and
//! revealed markers are stepped through (spec 02 §3.2). Pre-T7 stepping uses grapheme
//! boundaries of the rendered text (`unicode-segmentation`); hit testing uses the backend's
//! `TextBlock::hit` *[ADR 0005]*.
//!
//! Everything here is format-neutral: it reads only [`BlockLayout`]s, [`View`] heights and the
//! `markise-doc` inline view (for the reveal set).

use std::ops::Range;
use std::sync::Arc;

use epaint::{Pos2, Rect, pos2};
use markise_doc::{Affinity as DocAffinity, DocPos, InlineSource, PieceKind, SourceMap};
use markise_text::{Affinity, LocalPos};
use unicode_segmentation::UnicodeSegmentation;

use crate::block::{BlockLayout, Images, ItemFocus, Prim, reveal_set};
use crate::model::ItemKind;
use crate::text::{TextBlock, TextShaper};
use crate::view::View;

/// A position inside one item: item-relative source offset, affinity, and whether it is the
/// item's virtual empty paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemPos {
    pub rel: u32,
    pub affinity: Affinity,
    pub virt: bool,
}

impl ItemPos {
    pub const fn new(rel: u32, affinity: Affinity) -> Self {
        ItemPos {
            rel,
            affinity,
            virt: false,
        }
    }
}

fn to_text_aff(a: DocAffinity) -> Affinity {
    match a {
        DocAffinity::Upstream => Affinity::Upstream,
        DocAffinity::Downstream => Affinity::Downstream,
    }
}

fn to_doc_aff(a: Affinity) -> DocAffinity {
    match a {
        Affinity::Upstream => DocAffinity::Upstream,
        Affinity::Downstream => DocAffinity::Downstream,
    }
}

/// One text fragment of a layout that maps to source.
struct Frag<'a> {
    pos: Pos2,
    text: &'a Arc<dyn TextBlock>,
    map: &'a SourceMap,
    /// Smallest and largest item-relative source offset the fragment maps.
    lo: u32,
    hi: u32,
}

fn frags(l: &BlockLayout) -> Vec<Frag<'_>> {
    let mut out = Vec::new();
    for p in &l.prims {
        let Prim::Text { pos, text, .. } = p else { continue };
        let Some(map) = text.source_map() else { continue };
        let mut lo = u32::MAX;
        let mut hi = 0;
        for pc in &map.pieces {
            if pc.kind == PieceKind::Synthetic {
                continue;
            }
            lo = lo.min(pc.source.start);
            hi = hi.max(pc.source.end);
        }
        if lo == u32::MAX {
            continue; // generated text only (TOC entries)
        }
        out.push(Frag {
            pos: *pos,
            text,
            map,
            lo,
            hi,
        });
    }
    out
}

/// Rendered byte `t` → source, biased to the piece after `t` (`next`) or before it at piece
/// boundaries; generated pieces only when nothing else maps there.
fn src_at(map: &SourceMap, t: u32, next: bool) -> Option<u32> {
    let ps = &map.pieces;
    if ps.is_empty() {
        return None;
    }
    let i = ps.partition_point(|p| p.text.end < t);
    let Some(p) = ps.get(i) else {
        // past the last piece (padding): its end
        let p = ps.last()?;
        return Some(if p.kind == PieceKind::Synthetic {
            p.source.start
        } else {
            p.source.end
        });
    };
    let end_val = |p: &markise_doc::MapPiece| match p.kind {
        PieceKind::Synthetic => p.source.start,
        _ => p.source.end,
    };
    if p.text.start < t && t < p.text.end {
        return Some(match p.kind {
            PieceKind::Visible | PieceKind::Marker => p.source.start + (t - p.text.start),
            _ => p.source.start,
        });
    }
    let (prev, nxt) = if p.text.end == t && p.text.start < t {
        (Some(p), ps.get(i + 1).filter(|q| q.text.start == t))
    } else if p.text.start >= t {
        (None, Some(p))
    } else {
        // empty piece at t
        (Some(p), ps.get(i + 1).filter(|q| q.text.start == t))
    };
    fn real(q: Option<&markise_doc::MapPiece>) -> Option<&markise_doc::MapPiece> {
        q.filter(|q| q.kind != PieceKind::Synthetic)
    }
    let pick_next = |q: &markise_doc::MapPiece| q.source.start;
    let r = if next {
        real(nxt)
            .map(pick_next)
            .or(real(prev).map(end_val))
            .or(nxt.map(pick_next))
            .or(prev.map(end_val))
    } else {
        real(prev)
            .map(end_val)
            .or(real(nxt).map(pick_next))
            .or(prev.map(end_val))
            .or(nxt.map(pick_next))
    };
    if p.text.start > t && r.is_none() {
        return None;
    }
    r
}

/// The fragment holding item position `p` (index into `fs`) and its rendered byte.
fn locate(fs: &[Frag<'_>], p: ItemPos) -> Option<(usize, usize)> {
    let mut best: Option<usize> = None;
    for (i, f) in fs.iter().enumerate() {
        if p.rel < f.lo || p.rel > f.hi {
            continue;
        }
        let edge_end = p.rel == f.hi && p.rel != f.lo;
        match best {
            None => best = Some(i),
            Some(b) => {
                // at a boundary between fragments: Downstream prefers the one starting here
                let b_end = p.rel == fs[b].hi && p.rel != fs[b].lo;
                if b_end && !edge_end && p.affinity == Affinity::Downstream {
                    best = Some(i);
                }
            }
        }
    }
    // a position between fragments (hidden source): the nearest following fragment
    let i = best
        .or_else(|| fs.iter().position(|f| f.lo > p.rel))
        .or(fs.len().checked_sub(1))?;
    let f = &fs[i];
    let rel = p.rel.clamp(f.lo, f.hi);
    let t = f.map.to_text(rel)? as usize;
    Some((i, t.min(f.text.text().len())))
}

/// Caret rectangle (item-local, zero width) of `p` in layout `l`.
pub fn caret_rect(l: &BlockLayout, p: ItemPos) -> Option<Rect> {
    if p.virt {
        return l.virtual_caret;
    }
    let fs = frags(l);
    match locate(&fs, p) {
        Some((i, t)) => {
            let f = &fs[i];
            Some(
                f.text
                    .caret_rect(LocalPos::new(t, p.affinity))
                    .translate(f.pos.to_vec2()),
            )
        }
        None => l.empty_anchor.map(|(r, _)| r),
    }
}

/// Whether layout `l` has any caret stop.
pub fn has_stops(l: &BlockLayout) -> bool {
    l.empty_anchor.is_some() || l.virtual_caret.is_some() || !frags(l).is_empty()
}

/// Nearest caret position to item-local point `pt`.
pub fn hit(l: &BlockLayout, pt: Pos2) -> Option<ItemPos> {
    if let Some(r) = l.virtual_caret
        && pt.y >= r.min.y - 2.0
    {
        return Some(ItemPos {
            rel: 0,
            affinity: Affinity::Downstream,
            virt: true,
        });
    }
    let fs = frags(l);
    let mut best: Option<(f32, usize)> = None;
    for (i, f) in fs.iter().enumerate() {
        let r = f.text.rect().translate(f.pos.to_vec2());
        let dx = if pt.x < r.min.x {
            r.min.x - pt.x
        } else if pt.x > r.max.x {
            pt.x - r.max.x
        } else {
            0.0
        };
        let dy = if pt.y < r.min.y {
            r.min.y - pt.y
        } else if pt.y > r.max.y {
            pt.y - r.max.y
        } else {
            0.0
        };
        // vertical distance dominates (rows), horizontal breaks ties (cells side by side)
        let d = dy * 4.0 + dx;
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    match best {
        Some((_, i)) => {
            let f = &fs[i];
            let local = pt - f.pos.to_vec2();
            let mut lp = f.text.hit(local);
            // backends hit `char` boundaries: snap into grapheme stops like the arrows do
            // (ZWJ sequences, flags, combining marks are one stop)
            let text = f.text.text();
            if !text.is_char_boundary(lp.byte) || !is_grapheme_boundary(text, lp.byte) {
                let (a, b) = grapheme_around(text, lp.byte);
                let xa = f.text.caret_rect(LocalPos::new(a, Affinity::Downstream)).min.x;
                let xb = f.text.caret_rect(LocalPos::new(b, Affinity::Upstream)).min.x;
                lp.byte = if (local.x - xa).abs() <= (xb - local.x).abs() {
                    a
                } else {
                    b
                };
            }
            let rel = src_at(f.map, lp.byte as u32, lp.affinity == Affinity::Downstream)?;
            Some(ItemPos::new(rel, lp.affinity))
        }
        None => l.empty_anchor.map(|(_, rel)| ItemPos::new(rel, Affinity::Downstream)),
    }
}

fn is_grapheme_boundary(s: &str, at: usize) -> bool {
    let (a, _) = grapheme_around(s, at);
    a == at
}

/// The grapheme cluster containing byte `at`: its start and end (`(at, at)`-like at the end).
fn grapheme_around(s: &str, at: usize) -> (usize, usize) {
    let mut start = 0;
    for (i, g) in s.grapheme_indices(true) {
        if at < i + g.len() {
            return (i, i + g.len());
        }
        start = i + g.len();
    }
    (start, start)
}

fn boundaries(s: &str, word: bool, forward: bool) -> Vec<usize> {
    let mut v: Vec<usize> = if word {
        // word ends moving right, word starts moving left (UAX #29); whitespace and
        // punctuation runs are skipped over
        let mut v = vec![0];
        for (i, w) in s.split_word_bound_indices() {
            if w.chars().any(char::is_alphanumeric) {
                v.push(if forward { i + w.len() } else { i });
            }
        }
        v
    } else {
        s.grapheme_indices(true).map(|(i, _)| i).collect()
    };
    v.push(s.len());
    v.sort_unstable();
    v.dedup();
    v
}

/// First (`last = false`) or last caret stop of layout `l`.
pub fn edge_stop(l: &BlockLayout, last: bool) -> Option<ItemPos> {
    let fs = frags(l);
    let f = if last { fs.last() } else { fs.first() };
    match f {
        Some(f) => {
            let n = f.text.text().len() as u32;
            let (t, next) = if last { (n, false) } else { (0, true) };
            let rel = src_at(f.map, t, next)?;
            Some(ItemPos::new(
                rel,
                if last { Affinity::Upstream } else { Affinity::Downstream },
            ))
        }
        None => l.empty_anchor.map(|(_, rel)| ItemPos::new(rel, Affinity::Downstream)),
    }
}

/// One grapheme (or word) step inside layout `l`; `None` at the item's edge.
pub fn step(l: &BlockLayout, p: ItemPos, forward: bool, word: bool) -> Option<ItemPos> {
    if p.virt {
        return None;
    }
    let fs = frags(l);
    let (mut fi, t) = locate(&fs, p)?;
    let mut t = Some(t);
    loop {
        let f = &fs[fi];
        let text = f.text.text();
        let b = boundaries(text, word, forward);
        let cands: Box<dyn Iterator<Item = &usize>> = match (forward, t) {
            (true, Some(t)) => Box::new(b.iter().filter(move |&&x| x > t)),
            (true, None) => Box::new(b.iter()),
            (false, Some(t)) => Box::new(b.iter().rev().filter(move |&&x| x < t)),
            (false, None) => Box::new(b.iter().rev()),
        };
        for &c in cands {
            // moving right lands after the skipped character (Upstream), left before it
            if let Some(rel) = src_at(f.map, c as u32, !forward)
                && rel != p.rel
            {
                let aff = if forward {
                    Affinity::Upstream
                } else {
                    Affinity::Downstream
                };
                return Some(ItemPos::new(rel, aff));
            }
        }
        // next fragment of the item
        if forward {
            fi += 1;
            if fi >= fs.len() {
                return None;
            }
        } else {
            if fi == 0 {
                return None;
            }
            fi -= 1;
        }
        t = None;
    }
}

/// A visual row of a layout (item-local rect) and how to hit a point on it.
#[derive(Clone, Copy, Debug)]
struct Row {
    rect: Rect,
    /// `None`: a text row of a fragment; else a fixed stop (empty anchor, virtual paragraph).
    fixed: Option<ItemPos>,
}

fn rows(l: &BlockLayout) -> Vec<Row> {
    let mut out = Vec::new();
    for f in frags(l) {
        for li in f.text.lines() {
            let r = Rect::from_min_size(li.origin, li.size).translate(f.pos.to_vec2());
            out.push(Row { rect: r, fixed: None });
        }
    }
    if out.is_empty()
        && let Some((r, rel)) = l.empty_anchor
    {
        out.push(Row {
            rect: r,
            fixed: Some(ItemPos::new(rel, Affinity::Downstream)),
        });
    }
    if let Some(r) = l.virtual_caret {
        out.push(Row {
            rect: r,
            fixed: Some(ItemPos {
                rel: 0,
                affinity: Affinity::Downstream,
                virt: true,
            }),
        });
    }
    out
}

fn hit_row(l: &BlockLayout, row: &Row, x: f32) -> Option<ItemPos> {
    if let Some(p) = row.fixed {
        return Some(p);
    }
    hit(
        l,
        pos2(
            x.clamp(row.rect.min.x, row.rect.max.x.max(row.rect.min.x)),
            row.rect.center().y,
        ),
    )
}

/// The row below (`down`) or above the caret rectangle `caret` inside `l`; `None` when the
/// caret is on the item's last (first) row.
fn adjacent_row(l: &BlockLayout, caret: Rect, x: f32, down: bool) -> Option<ItemPos> {
    let rs = rows(l);
    let cy = caret.center().y;
    let cand = rs.iter().filter(|r| {
        if down {
            r.rect.min.y >= cy - 0.5 && r.rect.center().y > cy + 0.5
        } else {
            r.rect.max.y <= cy + 0.5 && r.rect.center().y < cy - 0.5
        }
    });
    let best = cand.min_by(|a, b| {
        let ka = if down { a.rect.min.y } else { -a.rect.max.y };
        let kb = if down { b.rect.min.y } else { -b.rect.max.y };
        let da = dist_x(a.rect, x);
        let db = dist_x(b.rect, x);
        (ka.round(), da)
            .partial_cmp(&(kb.round(), db))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    hit_row(l, best, x)
}

fn dist_x(r: Rect, x: f32) -> f32 {
    if x < r.min.x {
        r.min.x - x
    } else if x > r.max.x {
        x - r.max.x
    } else {
        0.0
    }
}

/// First (topmost, `top = true`) or last row of `l`, hit at `x`.
fn edge_row(l: &BlockLayout, x: f32, top: bool) -> Option<ItemPos> {
    let rs = rows(l);
    let best = rs.iter().min_by(|a, b| {
        let ka = if top { a.rect.min.y } else { -a.rect.max.y };
        let kb = if top { b.rect.min.y } else { -b.rect.max.y };
        (ka.round(), dist_x(a.rect, x))
            .partial_cmp(&(kb.round(), dist_x(b.rect, x)))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    hit_row(l, best, x)
}

/// Start (`end = false`) or end of the visual row holding `p` (soft-wrap aware: the end of a
/// wrapped row is Upstream, the start of the next row Downstream).
pub fn row_edge(l: &BlockLayout, p: ItemPos, end: bool) -> Option<ItemPos> {
    if p.virt {
        return Some(p);
    }
    let fs = frags(l);
    let Some((fi, t)) = locate(&fs, p) else {
        return l.empty_anchor.map(|(_, rel)| ItemPos::new(rel, Affinity::Downstream));
    };
    let f = &fs[fi];
    let lines = f.text.lines();
    let caret = f.text.caret_rect(LocalPos::new(t, p.affinity));
    let li = lines
        .iter()
        .position(|li| caret.center().y >= li.origin.y - 0.5 && caret.center().y <= li.origin.y + li.size.y + 0.5)
        .unwrap_or(0);
    let line = lines.get(li)?;
    let (tb, aff) = if end {
        (line.range.end, Affinity::Upstream)
    } else {
        (line.range.start, Affinity::Downstream)
    };
    let rel = src_at(f.map, tb as u32, !end)?;
    Some(ItemPos::new(rel, aff))
}

/// Item-local selection rectangles for the item-relative source range `r` (start inclusive)
/// in layout `l`; `to_end` / `from_start`: the selection continues past the item.
pub fn selection_rects(l: &BlockLayout, r: Range<u32>) -> Vec<Rect> {
    let mut out = Vec::new();
    for f in frags(l) {
        let a = r.start.max(f.lo);
        let b = r.end.min(f.hi);
        if a >= b && !(r.start <= f.lo && r.end > f.hi) {
            continue;
        }
        let ta = f.map.to_text(a).unwrap_or(0) as usize;
        let tb = f.map.to_text(b).unwrap_or(f.text.text().len() as u32) as usize;
        if tb <= ta {
            continue;
        }
        for rc in f.text.selection_rects(
            LocalPos::new(ta, Affinity::Downstream),
            LocalPos::new(tb, Affinity::Upstream),
        ) {
            out.push(rc.translate(f.pos.to_vec2()));
        }
    }
    out
}

// ---- document level ------------------------------------------------------------------------

/// The item holding `pos` and the item position.
pub fn item_pos(view: &View, pos: DocPos) -> Option<(usize, ItemPos)> {
    let model = view.model();
    if model.items.is_empty() {
        return None;
    }
    let mut i = model.item_at_offset(pos.offset);
    // spaces just typed after a heading's text sit in the source gap after its item (the
    // heading's span stops at its text): they still belong to the heading line, not to the
    // next item (issue #16)
    if i > 0
        && pos.offset < model.item_start(i)
        && model
            .items
            .get(i - 1)
            .is_some_and(|it| matches!(it.kind, ItemKind::Paragraph | ItemKind::Heading { .. }))
        && view
            .doc()
            .text()
            .get(model.item_span(i - 1).end..pos.offset)
            .is_some_and(|gap| gap.bytes().all(|c| matches!(c, b' ' | b'\t')))
    {
        i -= 1;
    }
    let start = model.item_start(i);
    Some((
        i,
        ItemPos {
            rel: pos.offset.saturating_sub(start) as u32,
            affinity: to_text_aff(pos.affinity),
            virt: pos.virt > 0,
        },
    ))
}

/// The editing focus for a caret at `pos` (ADR 0006 §2.4): the reveal set of its item
/// (paragraphs and headings; `reveal = false` freezes it, e.g. during a pointer drag), the
/// virtual paragraph flag and the shown trailing whitespace (paragraphs, headings, table cells).
pub fn focus_for(view: &View, pos: DocPos, reveal: bool) -> Option<(usize, ItemFocus)> {
    let (i, p) = item_pos(view, pos)?;
    let model = view.model();
    let it = model.items.get(i)?;
    let mut f = ItemFocus {
        reveal: Vec::new(),
        virtual_after: p.virt,
        virtual_exit: pos.virt.saturating_sub(1),
        trailing_ws: None,
        trailing_len: 0,
    };
    if !reveal || p.virt {
        return Some((i, f));
    }
    let doc = view.doc();
    let start = model.item_start(i);
    // the caret inside whitespace that ends the rendered content at item-relative `end` (#31)
    let in_trailing = |end: usize, kind: crate::block::WsEnd| {
        let n = doc
            .text()
            .get(start + end..)
            .map_or(0, |after| crate::block::trailing_ws_len(after, kind));
        (n > 0 && (end as u32) < p.rel && p.rel as usize <= end + n).then_some((end as u32, n as u32))
    };
    match it.kind {
        ItemKind::Paragraph | ItemKind::Heading { .. } => {
            let lines = model.item_lines(&**doc, i);
            if !lines.is_empty() {
                let rich = doc.inline(InlineSource::Lines(&lines), start);
                f.reveal = reveal_set(&rich, p.rel);
                // a segmented paragraph: only its last segment ends the text
                if it.segment.as_ref().is_none_or(|s| s.last()) {
                    let kind = if matches!(it.kind, ItemKind::Heading { .. }) {
                        crate::block::WsEnd::Heading
                    } else {
                        crate::block::WsEnd::Line
                    };
                    if let Some((end, n)) = rich.nodes.last().and_then(|n| in_trailing(n.source.end as usize, kind)) {
                        f.trailing_ws = Some(end);
                        f.trailing_len = n;
                    }
                }
            }
        }
        ItemKind::Table => {
            let at = pos.offset;
            if let Some(t) = doc.table(start)
                && let Some(row) = t.rows.iter().find(|r| r.start <= at && at <= r.end)
                && let Some((end, n)) = doc
                    .table_row_cells(*row)
                    .iter()
                    .find_map(|c| in_trailing(c.end.saturating_sub(start), crate::block::WsEnd::Cell))
            {
                f.trailing_ws = Some(end);
                f.trailing_len = n;
            }
        }
        _ => {}
    }
    Some((i, f))
}

/// Caret geometry over a [`View`]: lays out the items it needs (cache hits normally).
pub struct Geometry<'a> {
    pub view: &'a mut View,
    pub shaper: &'a mut dyn TextShaper,
    pub images: &'a dyn Images,
}

impl Geometry<'_> {
    fn layout(&mut self, i: usize) -> Arc<BlockLayout> {
        self.view.layout(i, self.shaper, self.images)
    }

    /// The item holding `pos` and the item position.
    pub fn item_pos(&self, pos: DocPos) -> Option<(usize, ItemPos)> {
        item_pos(self.view, pos)
    }

    fn doc_pos(&self, i: usize, p: ItemPos) -> DocPos {
        let start = self.view.model().item_start(i);
        if p.virt {
            // the virtual paragraph sits at the end of its item's leaf (the source gap)
            let span = self.view.model().item_span(i);
            return DocPos::with_virt(span.end, 1);
        }
        DocPos {
            offset: start + p.rel as usize,
            affinity: to_doc_aff(p.affinity),
            virt: 0,
        }
    }

    /// Caret rectangle of `pos`: (item, item-local rect).
    pub fn caret_rect(&mut self, pos: DocPos) -> Option<(usize, Rect)> {
        let (i, p) = self.item_pos(pos)?;
        let l = self.layout(i);
        caret_rect(&l, p).map(|r| (i, r))
    }

    /// The document position at item-local point `pt` of item `i`.
    pub fn hit(&mut self, i: usize, pt: Pos2) -> Option<DocPos> {
        let l = self.layout(i);
        if let Some(p) = hit(&l, pt) {
            return Some(self.doc_pos(i, p));
        }
        // an item without stops (rule, image): the nearest item that has one
        self.nearest_stop(i, pt.x, pt.y > l.height * 0.5)
    }

    fn nearest_stop(&mut self, i: usize, x: f32, below: bool) -> Option<DocPos> {
        let n = self.view.len();
        let order: Vec<usize> = if below {
            (i + 1..n).chain((0..i).rev()).collect()
        } else {
            (0..i).rev().chain(i + 1..n).collect()
        };
        for j in order.into_iter().take(64) {
            let l = self.layout(j);
            if let Some(p) = edge_row(&l, x, j > i) {
                return Some(self.doc_pos(j, p));
            }
        }
        None
    }

    /// Normalises `pos` to a caret stop (e.g. after an edit or a reload).
    pub fn normalize(&mut self, pos: DocPos) -> DocPos {
        let Some((i, p)) = self.item_pos(pos) else { return pos };
        let l = self.layout(i);
        if p.virt {
            return if l.virtual_caret.is_some() || self.view.focus().is_some_and(|(fi, _)| fi == i) {
                pos
            } else {
                DocPos::new(pos.offset)
            };
        }
        let fs = frags(&l);
        if let Some((fi, t)) = locate(&fs, p)
            && let Some(rel) = src_at(fs[fi].map, t as u32, p.affinity == Affinity::Downstream)
        {
            return self.doc_pos(i, ItemPos::new(rel, p.affinity));
        }
        if let Some((_, rel)) = l.empty_anchor {
            return self.doc_pos(i, ItemPos::new(rel, Affinity::Downstream));
        }
        self.nearest_stop(i, 0.0, true).unwrap_or(pos)
    }

    /// One grapheme / word step left or right, crossing items.
    pub fn horizontal(&mut self, pos: DocPos, forward: bool, word: bool) -> DocPos {
        let Some((i, p)) = self.item_pos(pos) else { return pos };
        if p.virt {
            // leaving the virtual paragraph backwards returns to the end of its block
            return if forward {
                pos
            } else {
                self.edge_of(i, true).unwrap_or(pos)
            };
        }
        let l = self.layout(i);
        if let Some(q) = step(&l, p, forward, word) {
            return self.doc_pos(i, q);
        }
        let n = self.view.len();
        let mut j = i;
        loop {
            if forward {
                j += 1;
                if j >= n {
                    return pos;
                }
            } else {
                if j == 0 {
                    return pos;
                }
                j -= 1;
            }
            if let Some(d) = self.edge_of(j, !forward) {
                return d;
            }
        }
    }

    fn edge_of(&mut self, j: usize, last: bool) -> Option<DocPos> {
        let l = self.layout(j);
        edge_stop(&l, last).map(|q| self.doc_pos(j, q))
    }

    /// Start or end of the visual row of `pos`.
    pub fn row_edge(&mut self, pos: DocPos, end: bool) -> DocPos {
        let Some((i, p)) = self.item_pos(pos) else { return pos };
        let l = self.layout(i);
        row_edge(&l, p, end).map_or(pos, |q| self.doc_pos(i, q))
    }

    /// First or last caret stop of the document.
    pub fn doc_edge(&mut self, end: bool) -> DocPos {
        let n = self.view.len();
        let order: Vec<usize> = if end { (0..n).rev().collect() } else { (0..n).collect() };
        for j in order {
            if let Some(d) = self.edge_of(j, end) {
                return d;
            }
        }
        DocPos::new(0)
    }

    /// One visual row up or down. `goal_x` (item-local x) is kept across vertical moves.
    /// Returns the new position and the goal x.
    pub fn vertical(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32) {
        let Some((i, p)) = self.item_pos(pos) else {
            return (pos, 0.0);
        };
        let l = self.layout(i);
        let Some(caret) = caret_rect(&l, p) else {
            return (pos, goal_x.unwrap_or(0.0));
        };
        let x = goal_x.unwrap_or(caret.min.x);
        if let Some(q) = adjacent_row(&l, caret, x, down) {
            return (self.doc_pos(i, q), x);
        }
        let n = self.view.len();
        let mut j = i;
        loop {
            if down {
                j += 1;
                if j >= n {
                    // the end of the document: the last stop
                    return (self.edge_of(i, true).unwrap_or(pos), x);
                }
            } else {
                if j == 0 {
                    return (self.edge_of(0, false).unwrap_or(pos), x);
                }
                j -= 1;
            }
            let lj = self.layout(j);
            if let Some(q) = edge_row(&lj, x, down) {
                return (self.doc_pos(j, q), x);
            }
        }
    }

    /// Page up/down: the position about `page` points below/above `pos`, at `goal_x`.
    pub fn page(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool, page: f32) -> (DocPos, f32) {
        let Some((i, p)) = self.item_pos(pos) else {
            return (pos, 0.0);
        };
        let l = self.layout(i);
        let Some(caret) = caret_rect(&l, p) else {
            return (pos, 0.0);
        };
        let x = goal_x.unwrap_or(caret.min.x);
        let y = self.view.item_y(i) + caret.center().y as f64;
        let target = if down { y + page as f64 } else { y - page as f64 };
        if target <= 0.0 {
            return (self.doc_edge(false), x);
        }
        if target >= self.view.total_height() {
            return (self.doc_edge(true), x);
        }
        let j = self.view.item_at_y(target);
        let lj = self.layout(j);
        let local = (target - self.view.item_y(j)) as f32;
        match hit(&lj, pos2(x, local)) {
            Some(q) => (self.doc_pos(j, q), x),
            None => (self.nearest_stop(j, x, down).unwrap_or(pos), x),
        }
    }

    /// See [`focus_for`].
    pub fn focus_for(&self, pos: DocPos, reveal: bool) -> Option<(usize, ItemFocus)> {
        focus_for(self.view, pos, reveal)
    }

    /// Item-local selection rectangles of item `i` for the document range `r`.
    pub fn selection_rects(&mut self, i: usize, r: Range<usize>) -> Vec<Rect> {
        let start = self.view.model().item_start(i);
        let l = self.layout(i);
        let a = r.start.saturating_sub(start) as u32;
        let b = r.end.saturating_sub(start).min(u32::MAX as usize) as u32;
        selection_rects(&l, a..b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pieces: &[(Range<u32>, Range<u32>, PieceKind)]) -> SourceMap {
        let mut m = SourceMap::default();
        for (t, s, k) in pieces {
            m.push(t.clone(), s.clone(), *k);
        }
        m
    }

    #[test]
    fn src_at_biases_at_hidden_markers() {
        // `a**b**c` collapsed: "abc"
        let m = map(&[
            (0..1, 0..1, PieceKind::Visible),
            (1..2, 3..4, PieceKind::Visible),
            (2..3, 6..7, PieceKind::Visible),
        ]);
        assert_eq!(src_at(&m, 2, false), Some(4)); // after `b`, inside the span
        assert_eq!(src_at(&m, 2, true), Some(6)); // before `c`, after the closing marker
        assert_eq!(src_at(&m, 1, true), Some(3));
        assert_eq!(src_at(&m, 1, false), Some(1));
        assert_eq!(src_at(&m, 3, false), Some(7));
    }

    #[test]
    fn src_at_prefers_real_text_over_generated() {
        let m = map(&[(0..3, 0..0, PieceKind::Synthetic), (3..5, 0..2, PieceKind::Visible)]);
        assert_eq!(src_at(&m, 3, false), Some(0));
        assert_eq!(src_at(&m, 3, true), Some(0));
        assert_eq!(src_at(&m, 4, true), Some(1));
    }
}
