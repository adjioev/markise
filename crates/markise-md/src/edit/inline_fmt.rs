//! Format commands (spec 02 §3.3, C49): strong, emphasis, underline, code, strike, highlight,
//! inline math, comment toggles; hyperlink and image insertion; Clear Format.
//!
//! - Collapsed caret: an empty marker pair is inserted with the caret between (inside an empty
//!   pair of the same style, the pair is removed again).
//! - Selection: trimmed to its non-whitespace span (C49); a selection over several blocks is
//!   formatted block by block (code, math, HTML, front matter, tables skipped). When every
//!   part already has the style it is removed (a part inside a styled span splits the span),
//!   otherwise each part is wrapped (same-style spans inside it are merged into the new one).
//! - Selection boundaries that cut through a styled span are widened to the whole span so the
//!   result stays well nested; boundaries inside markers snap to the content.

use markise_doc::{
    FormatDoc, InlineKind as NK, InlineNode, InlineSource, LeafKind, Plan, PlanError, PlanRequest, PlanStep,
    RichInline, Selection,
};

use super::{Edits, is_sp};
use crate::Document;

/// An inline style a format command toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Strong,
    Emph,
    Underline,
    Code,
    Strike,
    Highlight,
    Math,
    Comment,
}

impl Style {
    pub(crate) fn markers(self) -> (&'static str, &'static str) {
        match self {
            Style::Strong => ("**", "**"),
            Style::Emph => ("*", "*"),
            Style::Underline => ("<u>", "</u>"),
            Style::Code => ("`", "`"),
            Style::Strike => ("~~", "~~"),
            Style::Highlight => ("==", "=="),
            Style::Math => ("$", "$"),
            Style::Comment => ("<!--", "-->"),
        }
    }
    /// Styles represented by parsed inline nodes (the others are raw HTML tags).
    fn kind_matches(self, k: &NK) -> bool {
        matches!(
            (self, k),
            (Style::Strong, NK::Strong)
                | (Style::Emph, NK::Emph)
                | (Style::Code, NK::Code)
                | (Style::Strike, NK::Strike)
                | (Style::Highlight, NK::Highlight)
                | (Style::Math, NK::Math { display: false })
        )
    }
    fn textual(self) -> bool {
        matches!(self, Style::Underline | Style::Comment)
    }
}

/// Nodes whose content is literal (no inline markup inside).
fn atomic(k: &NK) -> bool {
    matches!(
        k,
        NK::Code | NK::Math { .. } | NK::Image { .. } | NK::Atom | NK::RawInline | NK::FootnoteRef { .. }
    )
}

/// All nodes with markers (or atomic), depth-first, with their depth.
fn flat_nodes(nodes: &[InlineNode], depth: usize, out: &mut Vec<(usize, InlineNode)>) {
    for n in nodes {
        if !n.markers.is_empty() || atomic(&n.kind) {
            let mut c = n.clone();
            c.children = Vec::new();
            out.push((depth, c));
        }
        flat_nodes(&n.children, depth + 1, out);
    }
}

/// One selected part of a text leaf.
#[derive(Clone, Debug)]
struct Part {
    s: usize,
    e: usize,
    rich: RichInline,
}

fn r(n: &InlineNode) -> (usize, usize, usize, usize) {
    (
        n.source.start as usize,
        n.content.start as usize,
        n.content.end as usize,
        n.source.end as usize,
    )
}

impl Document {
    fn text_leaf_kind(k: &LeafKind) -> bool {
        matches!(k, LeafKind::Paragraph | LeafKind::Heading { .. })
    }

    /// The trimmed, normalised selected parts of the text leaves in `a..b`.
    fn format_parts(&self, a: usize, b: usize, st: Option<Style>) -> Vec<Part> {
        let text = self.text().as_bytes();
        let mut parts = Vec::new();
        for leaf in self.leaves_in(a, b) {
            if !Self::text_leaf_kind(&leaf.kind) || leaf.lines.is_empty() {
                continue;
            }
            let (first, last) = (leaf.lines[0], *leaf.lines.last().unwrap());
            let mut s = a.max(first.start);
            let mut e = b.min(last.end);
            if s > e {
                continue;
            }
            // trim whitespace, soft breaks and container prefixes (C49)
            loop {
                if s < e && is_sp(text[s]) {
                    s += 1;
                    continue;
                }
                if let Some(i) = leaf.lines.iter().position(|l| l.end == s)
                    && i + 1 < leaf.lines.len()
                    && s < e
                {
                    s = leaf.lines[i + 1].start.min(e);
                    continue;
                }
                if !leaf.lines.iter().any(|l| l.start <= s && s <= l.end) {
                    // inside a prefix between lines
                    match leaf.lines.iter().find(|l| l.start > s) {
                        Some(l) if l.start <= e => {
                            s = l.start;
                            continue;
                        }
                        _ => s = e,
                    }
                }
                break;
            }
            loop {
                if e > s && is_sp(text[e - 1]) {
                    e -= 1;
                    continue;
                }
                if let Some(i) = leaf.lines.iter().position(|l| l.start == e)
                    && i > 0
                    && e > s
                {
                    e = leaf.lines[i - 1].end.max(s);
                    continue;
                }
                if !leaf.lines.iter().any(|l| l.start <= e && e <= l.end) {
                    match leaf.lines.iter().rev().find(|l| l.end < e) {
                        Some(l) if l.end >= s => {
                            e = l.end;
                            continue;
                        }
                        _ => e = s,
                    }
                }
                break;
            }
            if s >= e {
                continue;
            }
            let rich = self.inline(InlineSource::Lines(&leaf.lines), 0);
            let (s, e) = normalize(&rich, s, e, st);
            parts.push(Part { s, e, rich });
        }
        parts
    }

    /// The node of style `st` containing part `p` (innermost), if any.
    fn styled_node(&self, p: &Part, st: Style) -> Option<(usize, usize, usize, usize)> {
        if st.textual() {
            let t = self.text();
            let (o, c) = st.markers();
            if t[..p.s].ends_with(o) && t[p.e..].starts_with(c) {
                return Some((p.s - o.len(), p.s, p.e, p.e + c.len()));
            }
            if t[p.s..p.e].starts_with(o) && t[p.s..p.e].ends_with(c) && p.e - p.s >= o.len() + c.len() {
                return Some((p.s, p.s + o.len(), p.e - c.len(), p.e));
            }
            return None;
        }
        let mut v = Vec::new();
        flat_nodes(&p.rich.nodes, 0, &mut v);
        v.into_iter()
            .filter(|(_, n)| st.kind_matches(&n.kind))
            .map(|(d, n)| (d, r(&n)))
            .filter(|(_, (os, cs, ce, oe))| (*cs <= p.s && p.e <= *ce) || (p.s == *os && p.e == *oe))
            .max_by_key(|(d, _)| *d)
            .map(|(_, x)| x)
    }

    /// Format toggle `st` (spec 02 §3.3, C49).
    pub(crate) fn plan_format(&self, st: Style, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        let text = self.text();
        let (open, close) = st.markers();
        if sel.is_caret() {
            let a = sel.focus.offset;
            if sel.focus.virt > 0 {
                return Err(PlanError::NotApplicable("virtual paragraph"));
            }
            // in whitespace typed after a heading's text: at its end for the checks (#16)
            let t = self.heading_end_before_ws(a).unwrap_or(a);
            let leaf = self.leaf(t).ok_or(PlanError::NotApplicable("no block"))?;
            if !Self::text_leaf_kind(&leaf.kind) || !leaf.lines.iter().any(|l| l.start <= t && t <= l.end) {
                return Err(PlanError::NotApplicable("not in text"));
            }
            if text[..a].ends_with(open) && text[a..].starts_with(close) {
                let mut ed = Edits::new();
                ed.delete(a - open.len()..a);
                ed.delete(a..a + close.len());
                let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
                return Ok(Plan::single(
                    PlanStep::patches(ps),
                    Some(Selection::caret(a - open.len())),
                ));
            }
            let ps = markise_doc::PatchSet::single_verbatim(a..a, format!("{open}{close}"));
            return Ok(Plan::single(
                PlanStep::patches(ps),
                Some(Selection::caret(a + open.len())),
            ));
        }
        let r0 = sel.range();
        let parts = self.format_parts(r0.start, r0.end, Some(st));
        if parts.is_empty() {
            // only whitespace selected: act as with no selection at its start (C49)
            let leaf = self.leaf(r0.start).ok_or(PlanError::NotApplicable("no block"))?;
            if !Self::text_leaf_kind(&leaf.kind) {
                return Err(PlanError::NotApplicable("not in text"));
            }
            let ps = markise_doc::PatchSet::single_verbatim(r0.start..r0.start, format!("{open}{close}"));
            return Ok(Plan::single(
                PlanStep::patches(ps),
                Some(Selection::caret(r0.start + open.len())),
            ));
        }
        let all_styled = parts.iter().all(|p| self.styled_node(p, st).is_some());
        let mut ed = Edits::new();
        let mut bounds = Vec::new();
        for p in &parts {
            if all_styled {
                let (os, cs, ce, oe) = self.styled_node(p, st).unwrap();
                let (s, e) = if p.s == os && p.e == oe { (cs, ce) } else { (p.s, p.e) };
                remove_style(text, &mut ed, (os, cs, ce, oe), s, e, open, close, 0);
                bounds.push((s, e));
            } else {
                self.wrap_part(&mut ed, p, st);
                bounds.push((p.s, p.e));
            }
        }
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlapping edits"))?;
        let s = ps.map_offset(bounds[0].0, markise_doc::Affinity::Downstream);
        let e = ps.map_offset(bounds.last().unwrap().1, markise_doc::Affinity::Upstream);
        let after = if sel.anchor.offset <= sel.focus.offset {
            Selection::range(s, e.max(s))
        } else {
            Selection::range(e.max(s), s)
        };
        Ok(Plan::single(PlanStep::patches(ps), Some(after)))
    }

    fn wrap_part(&self, ed: &mut Edits, p: &Part, st: Style) {
        let text = self.text();
        let (mut open, mut close) = (st.markers().0.to_string(), st.markers().1.to_string());
        let mut nodes = Vec::new();
        flat_nodes(&p.rich.nodes, 0, &mut nodes);
        if st == Style::Code {
            // a fence longer than any backtick run inside; pad when the content touches one
            let inner = &text[p.s..p.e];
            let mut run = 0;
            let mut best = 0;
            for c in inner.bytes() {
                if c == b'`' {
                    run += 1;
                    best = best.max(run);
                } else {
                    run = 0;
                }
            }
            let fence = "`".repeat(best + 1);
            let pad = if inner.starts_with('`') || inner.ends_with('`') {
                " "
            } else {
                ""
            };
            open = format!("{fence}{pad}");
            close = format!("{pad}{fence}");
        }
        // same-style spans inside the part merge into the new one
        let mut opened = false;
        let mut closed = false;
        for (_, n) in &nodes {
            if !st.kind_matches(&n.kind) {
                continue;
            }
            let (os, cs, ce, oe) = r(n);
            if os >= p.s && oe <= p.e {
                ed.delete(os..cs);
                ed.delete(ce..oe);
            } else if st != Style::Code && oe == p.s && !opened {
                // `**a**[b]` → `**ab**`
                ed.delete(ce..oe);
                opened = true;
            } else if st != Style::Code && os == p.e && !closed {
                ed.delete(os..cs);
                closed = true;
            }
        }
        if !opened {
            ed.push_ranked(p.s..p.s, 1, open);
        }
        if !closed {
            ed.push_ranked(p.e..p.e, -1, close);
        }
    }

    /// Whether style `st` is active at the selection (menu check marks, toolbar state; spec
    /// 04 §3.2): the whole trimmed selection has it, or the caret is inside it.
    pub(crate) fn format_active(&self, st: Style, req: &PlanRequest) -> bool {
        let sel = req.selection.primary();
        let r0 = sel.range();
        if sel.is_caret() {
            let a = self.heading_end_before_ws(r0.start).unwrap_or(r0.start);
            let Some(leaf) = self.leaf(a) else { return false };
            if !Self::text_leaf_kind(&leaf.kind) || leaf.lines.is_empty() {
                return false;
            }
            if st.textual() {
                return false;
            }
            let rich = self.inline(InlineSource::Lines(&leaf.lines), 0);
            let mut v = Vec::new();
            flat_nodes(&rich.nodes, 0, &mut v);
            return v.iter().any(|(_, n)| {
                st.kind_matches(&n.kind) && n.content.start as usize <= a && a <= n.content.end as usize
            });
        }
        let parts = self.format_parts(r0.start, r0.end, Some(st));
        !parts.is_empty() && parts.iter().all(|p| self.styled_node(p, st).is_some())
    }

    /// Hyperlink / image (spec 02 §3.3): wraps the selection as `[sel](url)` / `![sel](url)`
    /// (caret in the destination), inserts `[]()` at a caret (caret in the text), and removes
    /// a link when the selection lies inside one. `url` (optional): destination prefill.
    pub(crate) fn plan_link(&self, image: bool, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        let url = req.params.get_text("url").unwrap_or("").trim().to_string();
        let lead = if image { "![" } else { "[" };
        if sel.is_caret() {
            let a = sel.focus.offset;
            let t = self.heading_end_before_ws(a).unwrap_or(a);
            let leaf = self.leaf(t).ok_or(PlanError::NotApplicable("no block"))?;
            if sel.focus.virt > 0
                || !Self::text_leaf_kind(&leaf.kind)
                || !leaf.lines.iter().any(|l| l.start <= t && t <= l.end)
            {
                return Err(PlanError::NotApplicable("not in text"));
            }
            let ins = format!("{lead}]({url})");
            return Ok(Plan::single(
                PlanStep::patches(markise_doc::PatchSet::single_verbatim(a..a, ins)),
                Some(Selection::caret(a + lead.len())),
            ));
        }
        let r0 = sel.range();
        let parts = self.format_parts(r0.start, r0.end, None);
        let p = parts.first().ok_or(PlanError::NotApplicable("nothing selected"))?;
        if !image {
            let mut v = Vec::new();
            flat_nodes(&p.rich.nodes, 0, &mut v);
            let link = v
                .iter()
                .filter(|(_, n)| matches!(n.kind, NK::Link { .. }) && n.markers.len() == 2)
                .map(|(_, n)| r(n))
                .find(|(os, cs, ce, oe)| (*cs <= p.s && p.e <= *ce) || (p.s <= *os && *oe <= p.e));
            if let Some((os, cs, ce, oe)) = link
                && self.text()[os..].starts_with('[')
            {
                let mut ed = Edits::new();
                ed.delete(os..cs);
                ed.delete(ce..oe);
                let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
                let s = ps.map_offset(cs, markise_doc::Affinity::Downstream);
                let e = ps.map_offset(ce, markise_doc::Affinity::Upstream);
                return Ok(Plan::single(PlanStep::patches(ps), Some(Selection::range(s, e))));
            }
        }
        let mut ed = Edits::new();
        ed.insert(p.s, lead);
        ed.insert(p.e, format!("]({url})"));
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        let caret = p.e + lead.len() + 2 + url.len();
        Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))))
    }

    /// Clear Format (spec 02 §3.3): removes emphasis, strong, underline, strike, highlight,
    /// sub/superscript and code spans from the selected text (spans partly selected are split).
    pub(crate) fn plan_clear_format(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        if sel.is_caret() {
            return Err(PlanError::NotApplicable("no selection"));
        }
        let text = self.text();
        let r0 = sel.range();
        let parts = self.format_parts(r0.start, r0.end, None);
        let mut ed = Edits::new();
        for p in &parts {
            let mut v = Vec::new();
            flat_nodes(&p.rich.nodes, 0, &mut v);
            for (d, n) in &v {
                let clearable = matches!(
                    n.kind,
                    NK::Emph | NK::Strong | NK::Strike | NK::Highlight | NK::Sub | NK::Sup | NK::Code
                );
                if !clearable || n.markers.len() != 2 {
                    continue;
                }
                let (os, cs, ce, oe) = r(n);
                if oe <= p.s || os >= p.e {
                    continue;
                }
                let s = p.s.max(cs);
                let e = p.e.min(ce);
                let o = &text[os..cs];
                let c = &text[ce..oe];
                remove_style(text, &mut ed, (os, cs, ce, oe), s, e, o, c, *d as i32);
            }
            // underline tags: parsed raw-HTML nodes inside the part or right around it (never
            // text inside code spans, math or other literal content)
            for (_, n) in &v {
                if n.kind != NK::RawInline {
                    continue;
                }
                let (os, oe) = (n.source.start as usize, n.source.end as usize);
                let raw = &text[os..oe];
                let open = raw.eq_ignore_ascii_case("<u>");
                let close = raw.eq_ignore_ascii_case("</u>");
                let inside = p.s <= os && oe <= p.e;
                let around = (open && oe == p.s) || (close && os == p.e);
                if (open || close) && (inside || around) {
                    ed.delete(os..oe);
                }
            }
        }
        if ed.is_empty() {
            return Err(PlanError::NotApplicable("nothing to clear"));
        }
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlapping edits"))?;
        let s = ps.map_offset(r0.start, markise_doc::Affinity::Downstream);
        let e = ps.map_offset(r0.end, markise_doc::Affinity::Upstream);
        Ok(Plan::single(PlanStep::patches(ps), Some(Selection::range(s, e.max(s)))))
    }
}

/// Snaps part `s..e` out of markers into content and widens it over spans it cuts through
/// (atomic spans as a whole unless styling them as code from inside).
fn normalize(rich: &RichInline, mut s: usize, mut e: usize, st: Option<Style>) -> (usize, usize) {
    let mut v = Vec::new();
    flat_nodes(&rich.nodes, 0, &mut v);
    for _ in 0..8 {
        let (s0, e0) = (s, e);
        for (_, n) in &v {
            let (os, cs, ce, oe) = r(n);
            if oe <= s || os >= e {
                continue;
            }
            let inside = os <= s && e <= oe;
            if inside {
                if atomic(&n.kind) {
                    let own = st.is_some_and(|st| st.kind_matches(&n.kind));
                    if !own && !(cs <= s && e <= ce && st.is_none()) {
                        s = os;
                        e = oe;
                    } else {
                        s = s.clamp(cs, ce);
                        e = e.clamp(cs, ce);
                    }
                } else if !(s == os && e == oe) {
                    // snap out of markers into the content
                    if s < cs {
                        s = cs.min(e);
                    }
                    if e > ce {
                        e = ce.max(s);
                    }
                }
                continue;
            }
            if s <= os && oe <= e {
                continue; // whole node inside
            }
            // partial overlap: widen to the whole node
            s = s.min(os);
            e = e.max(oe);
        }
        if (s, e) == (s0, e0) {
            break;
        }
    }
    (s, e)
}

/// Removes style node `(os, cs, ce, oe)` from its content part `s..e`: whole content → both
/// markers deleted; otherwise the span is split around the part (whitespace at the cut stays
/// outside the markers).
#[allow(clippy::too_many_arguments)]
fn remove_style(
    text: &str,
    ed: &mut Edits,
    (os, cs, ce, oe): (usize, usize, usize, usize),
    s: usize,
    e: usize,
    open: &str,
    close: &str,
    depth: i32,
) {
    let b = text.as_bytes();
    // left piece cs..s
    let mut sl = s;
    while sl > cs && is_sp(b[sl - 1]) {
        sl -= 1;
    }
    if sl <= cs {
        ed.delete(os..cs);
    } else {
        // closers of inner spans come first at the same point
        ed.push_ranked(sl..sl, -depth, close);
    }
    let mut er = e;
    while er < ce && is_sp(b[er]) {
        er += 1;
    }
    if er >= ce {
        ed.delete(ce..oe);
    } else {
        ed.push_ranked(er..er, depth, open);
    }
}
