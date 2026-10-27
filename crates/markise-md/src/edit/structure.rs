//! Block structure commands: Move Row Up/Down (spec 02 §10, A4: list items and top-level
//! blocks), quote and list toggles (Paragraph menu, spec 02 §15.1), Horizontal Line, and
//! Backspace at the start of a quote (C46).

use markise_doc::{DocPos, FormatDoc, LeafKind, PatchSet, Plan, PlanError, PlanRequest, PlanStep, Selection};

use super::Edits;
use super::blocks::cont_prefix;
use crate::block::BlockKind;
use crate::text::line_end;
use crate::{Document, line_start};

impl Document {
    /// Move Row Up / Down: the caret's list item swaps with its sibling (numbers stay with
    /// the positions), otherwise the caret's top-level block swaps with its neighbour.
    pub(crate) fn plan_move_block(&self, req: &PlanRequest, down: bool) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        let at = sel.focus.offset;
        let text = self.text();
        if let Some(ctx) = self.list_ctx(at) {
            let j = if down { ctx.idx + 1 } else { ctx.idx.wrapping_sub(1) };
            let (Some(a), Some(b)) = (ctx.items.get(ctx.idx.min(j)), ctx.items.get(ctx.idx.max(j))) else {
                return Err(PlanError::NotApplicable("no sibling"));
            };
            let a_end = line_end(text, a.end.max(a.ls)).0;
            let b_end = line_end(text, b.end.max(b.ls)).0;
            let a_txt = &text[a.ls..a_end];
            let sep = &text[a_end..b.ls];
            let b_txt = &text[b.ls..b_end];
            // markers stay with the positions (ordered numbers, bullets)
            let swap_marker = |t: &str, from: &super::blocks::Item, to: &super::blocks::Item| -> String {
                let ms = from.marker.start - from.ls;
                let me = from.marker.end - from.ls;
                let new = &text[to.marker.range()];
                let first = format!("{}{}{}", &t[..ms], new, &t[me..]);
                // continuation lines follow the marker's width (`9.` ↔ `10.`)
                reindent_text(&first, ms, new.len() as isize - from.marker.len() as isize)
            };
            let new_b = swap_marker(b_txt, b, a);
            let new_a = swap_marker(a_txt, a, b);
            let ins = format!("{new_b}{sep}{new_a}");
            let ps = PatchSet::single_verbatim(a.ls..b_end, ins);
            let moving = ctx.item();
            let rel = at.saturating_sub(moving.ls);
            // the caret keeps its place in the moved item's text: mapped line by line through
            // the changes before it (marker width, re-indented continuation lines)
            let shift = |from: &super::blocks::Item, new_txt: &str, old_txt: &str| -> usize {
                map_moved_caret(old_txt, new_txt, rel, from.marker.end - from.ls)
            };
            let caret = if down {
                a.ls + new_b.len() + sep.len() + shift(a, &new_a, a_txt)
            } else {
                a.ls + shift(b, &new_b, b_txt)
            };
            return Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))));
        }
        // top-level blocks
        let path = self.block_path_at(at);
        let Some(&i) = path.first() else {
            return Err(PlanError::NotApplicable("no block"));
        };
        let j = if down { i + 1 } else { i.wrapping_sub(1) };
        let (Some(ta), Some(tb)) = (self.top(i.min(j)), self.top(i.max(j))) else {
            return Err(PlanError::NotApplicable("no neighbour"));
        };
        let a0 = line_start(text, ta.start());
        let a1 = line_end(text, ta.span().end).0;
        let b0 = line_start(text, tb.start());
        let b1 = line_end(text, tb.span().end).0;
        let ins = format!("{}{}{}", &text[b0..b1], &text[a1..b0], &text[a0..a1]);
        let ps = PatchSet::single_verbatim(a0..b1, ins);
        let caret = if down {
            a0 + (b1 - b0) + (b0 - a1) + (at - a0)
        } else {
            a0 + (at - b0)
        };
        Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))))
    }

    /// The innermost quote around `at`: (its absolute start, the line prefix up to and
    /// including its `>` marker on each of the leaf's lines is found per line).
    fn quote_of(&self, at: usize) -> Option<(usize, usize)> {
        let path = self.block_path_at(at);
        (1..=path.len()).rev().find_map(|d| {
            let (b, o) = self.block_at_path_rel(&path[..d])?;
            matches!(b.kind, BlockKind::BlockQuote { .. }).then_some((b.span.start + o, b.span.end + o))
        })
    }

    /// Removes the `depth`-th quote marker (`>` and one space) of the line starting at `ls`.
    pub(crate) fn unquote_line(&self, ed: &mut Edits, ls: usize, depth: usize) {
        self.unquote_lines(ed, &[ls], depth);
    }

    /// Removes the innermost quote marker (`>` and one space) from every line of `lines`
    /// (line starts) whose prefix holds one at column `col` or later.
    fn unquote_lines(&self, ed: &mut Edits, line_starts: &[usize], depth: usize) {
        let text = self.text();
        for &ls in line_starts {
            let (le, _) = line_end(text, ls);
            let b = &text.as_bytes()[ls..le];
            // the `depth`-th `>` of the prefix
            let mut seen = 0;
            let mut p = 0;
            while p < b.len() && (b[p] == b'>' || b[p] == b' ' || b[p] == b'\t') {
                if b[p] == b'>' {
                    seen += 1;
                    if seen == depth {
                        let end = if b.get(p + 1) == Some(&b' ') { p + 2 } else { p + 1 };
                        ed.delete(ls + p..ls + end);
                        break;
                    }
                }
                p += 1;
            }
        }
    }

    /// Backspace at the start of the first block of a quote (C46): that block moves out of
    /// the quote; on an alert's body line the tag goes first.
    pub(crate) fn quote_backspace(&self, at: usize) -> Option<Result<Plan, PlanError>> {
        let leaf = self.leaf(at)?;
        if leaf.kind != LeafKind::Paragraph {
            return None;
        }
        let (qs, qe) = self.quote_of(at)?;
        let text = self.text();
        let eol = self.line_ending().as_str();
        let first = *leaf.lines.first()?;
        // the leaf must be the quote's first block
        if line_start(text, first.start) != line_start(text, qs) {
            return None;
        }
        // alert: caret at the start of the body line after `[!KIND]`
        if leaf.lines.len() >= 2 && leaf.lines[1].start == at {
            let tag = first.slice(text).trim().to_ascii_uppercase();
            if tag.starts_with("[!") && tag.ends_with(']') {
                let ls1 = line_start(text, leaf.lines[1].start);
                let ps = PatchSet::delete(line_start(text, first.start)..ls1);
                let caret = at - (ls1 - line_start(text, first.start));
                return Some(Ok(
                    Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))).labeled("Paragraph")
                ));
            }
        }
        if at != first.start {
            return None;
        }
        let depth = text[line_start(text, qs)..qs + 1].matches('>').count();
        let mut ed = Edits::new();
        let starts: Vec<usize> = leaf.lines.iter().map(|l| line_start(text, l.start)).collect();
        self.unquote_lines(&mut ed, &starts, depth);
        let last_ls = *starts.last()?;
        let (_, after) = line_end(text, last_ls);
        if after < qe {
            // the rest stays quoted: a blank (unquoted) line separates them
            let (bl_end, _) = line_end(text, after);
            let line = &text[after..bl_end];
            if line.trim_start_matches(['>', ' ', '\t']).is_empty() {
                // `>` separator line → its innermost marker goes
                self.unquote_lines(&mut ed, &[after], depth);
            } else {
                let outer = cont_prefix(
                    text,
                    after,
                    after + (line.len() - line.trim_start_matches(['>', ' ', '\t']).len()),
                );
                let outer: String = {
                    // the prefix without the innermost quote marker
                    let mut seen = 0;
                    outer
                        .chars()
                        .filter(|c| {
                            if *c == '>' {
                                seen += 1;
                                seen != depth
                            } else {
                                true
                            }
                        })
                        .collect()
                };
                ed.insert(after, format!("{}{eol}", outer.trim_end()));
            }
        }
        // a non-blank line right before the quote would absorb the paragraph
        let qls = line_start(text, qs);
        if qls > 0 {
            let pe = crate::text::prev_line_content_end(text, qls);
            let pls = line_start(text, pe);
            if !text[pls..pe].trim_start_matches(['>', ' ', '\t']).is_empty() && depth == 1 {
                ed.insert(qls, eol.to_string());
            }
        }
        let ps = ed.build()?;
        let caret = ps.map_offset(at, markise_doc::Affinity::Downstream);
        Some(Ok(
            Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))).labeled("Unwrap Quote")
        ))
    }

    /// Forward Delete at the end of a paragraph/heading before a block in another container
    /// or a rule (C48 rules 1–2): a following HR goes; the first paragraph of a quote merges
    /// (the rest stays quoted); the first paragraph of a list item merges (the item's other
    /// blocks move up). `None`: the engine's same-container merge and no-op rules apply.
    pub(crate) fn c48_delete_forward(&self, at: usize) -> Option<Result<Plan, PlanError>> {
        let text = self.text();
        let leaf = self.leaf(at)?;
        if !matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. }) || leaf.lines.last()?.end != at {
            return None;
        }
        // the next content line, after this block's own syntax (a setext underline)
        let mut ls = line_end(text, leaf.span.end.max(at)).1;
        let q = loop {
            if ls >= text.len() {
                return None;
            }
            let (le, next) = line_end(text, ls);
            let lead = text[ls..le].len() - text[ls..le].trim_start_matches(['>', ' ', '\t']).len();
            if ls + lead < le {
                break ls + lead;
            }
            if next == le {
                return None;
            }
            ls = next;
        };
        // (a list item's line starts with its marker: its first block starts after it)
        let next = self.leaf(q).or_else(|| {
            self.item_at(q)
                .filter(|it| !it.empty)
                .and_then(|it| self.leaf(it.content))
        })?;
        if next.span.start <= leaf.span.start {
            return None;
        }
        let eol = self.line_ending().as_str();
        let mut ed = Edits::new();
        // this block keeps its syntax (an ATX closing sequence, a setext underline after the
        // merged text); the next block's syntax goes on both sides (M2b-2 merge rules)
        let prev_trail = at..leaf.span.end.max(at);
        if next.kind == LeafKind::ThematicBreak {
            let (le, _) = line_end(text, next.span.start);
            ed.delete(prev_trail.end..le);
            let ps = ed.build()?;
            return Some(Ok(
                Plan::single(PlanStep::patches(ps), Some(Selection::caret(at))).labeled("Delete")
            ));
        }
        let extra = match next.containers.last() {
            Some(markise_doc::ContainerKind::ListItem { .. }) => 2, // list + item
            _ => 1,
        };
        if !matches!(next.kind, LeafKind::Paragraph | LeafKind::Heading { .. })
            || next.containers.len() != leaf.containers.len() + extra
        {
            return None;
        }
        let first = *next.lines.first()?;
        // the next block's text, its later lines without the container it leaves
        match next.containers.last()? {
            markise_doc::ContainerKind::Quote | markise_doc::ContainerKind::Callout(_) => {
                let (qs, qe) = self.quote_of(first.start)?;
                if line_start(text, qs) != line_start(text, first.start) {
                    return None; // not the quote's first block
                }
                let depth = text[line_start(text, qs)..=qs].matches('>').count();
                // the `depth`-th `>` (and one space) of a line's prefix
                let cut = |line: &str| -> (usize, usize) {
                    let b = line.as_bytes();
                    let mut seen = 0;
                    for (p, c) in b.iter().enumerate() {
                        if *c == b'>' {
                            seen += 1;
                            if seen == depth {
                                let e = if b.get(p + 1) == Some(&b' ') { p + 2 } else { p + 1 };
                                return (p, e);
                            }
                        } else if *c != b' ' && *c != b'\t' {
                            break;
                        }
                    }
                    (0, 0)
                };
                let next_end = self.push_merge(&mut ed, &leaf, at, &next, &cut, "");
                let (_, after) = line_end(text, next_end);
                if after < qe && after < text.len() {
                    let (ble, _) = line_end(text, after);
                    if text[after..ble].trim_start_matches(['>', ' ', '\t']).is_empty() {
                        self.unquote_lines(&mut ed, &[after], depth);
                    } else {
                        ed.insert(after, eol.to_string());
                    }
                }
            }
            markise_doc::ContainerKind::ListItem { .. } => {
                let it = self.item_at(first.start)?;
                if it.content != next.span.start {
                    return None; // not the item's first block
                }
                let cpl = it.marker.start - it.ls;
                let co = it.content_offset;
                let cut = |line: &str| -> (usize, usize) {
                    let b = line.as_bytes();
                    let mut p = 0;
                    while p < b.len() && p < cpl && (b[p] == b'>' || b[p] == b' ' || b[p] == b'\t') {
                        p += 1;
                    }
                    let sp = b[p..].iter().take_while(|c| **c == b' ').count().min(co);
                    (p, p + sp)
                };
                let next_end = self.push_merge(&mut ed, &leaf, at, &next, &cut, "");
                // the item's other blocks move up (out of the dissolved item)
                let from = line_end(text, next_end).1;
                self.reindent_lines(&mut ed, from, it.end, cpl, -(co as isize));
            }
            _ => return None,
        }
        let ps = ed.build()?;
        Some(Ok(
            Plan::single(PlanStep::patches(ps), Some(Selection::caret(at))).labeled("Merge")
        ))
    }

    /// Paragraph → Quote toggle: wraps the caret's block in a quote, or unquotes it.
    pub(crate) fn plan_toggle_quote(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        let at = req.selection.primary().focus.offset;
        let text = self.text();
        let leaf = self.leaf(at).ok_or(PlanError::NotApplicable("no block"))?;
        if leaf.lines.is_empty() {
            return Err(PlanError::NotApplicable("no text"));
        }
        let starts: Vec<usize> = leaf.lines.iter().map(|l| line_start(text, l.start)).collect();
        let mut ed = Edits::new();
        if let Some((qs, _)) = self.quote_of(at) {
            let depth = text[line_start(text, qs)..qs + 1].matches('>').count();
            if matches!(
                leaf.containers.last(),
                Some(markise_doc::ContainerKind::Quote | markise_doc::ContainerKind::Callout(_))
            ) {
                if let Some(r) = self.quote_backspace(leaf.lines[0].start) {
                    return r.map(|mut p| {
                        p.selection_after = None;
                        p
                    });
                }
                self.unquote_lines(&mut ed, &starts, depth);
                let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
                return Ok(Plan::single(PlanStep::patches(ps), None));
            }
        }
        // wrap: `> ` after each line's container prefix
        let first = leaf.lines[0];
        for (k, &ls) in starts.iter().enumerate() {
            let content = if k == 0 {
                leaf.span.start.min(first.start)
            } else {
                leaf.lines[k].start
            };
            let _ = ls;
            ed.insert(content, "> ");
        }
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        Ok(Plan::single(PlanStep::patches(ps), None))
    }

    /// Ordered / Unordered / Task List toggles: a paragraph becomes a one-item list; in a list
    /// of the same kind the list's items become paragraphs; a list of another kind changes
    /// its markers (ordered numbers sequential from the first number or 1).
    pub(crate) fn plan_toggle_list(&self, req: &PlanRequest, kind: ListKind) -> Result<Plan, PlanError> {
        let at = req.selection.primary().focus.offset;
        let text = self.text();
        let eol = self.line_ending().as_str();
        if let Some(ctx) = self.list_ctx(at) {
            let cur = if ctx.ordered {
                ListKind::Ordered
            } else if ctx.items.iter().all(|i| i.syntax_end > i.marker.end) {
                ListKind::Task
            } else {
                ListKind::Bullet
            };
            let mut ed = Edits::new();
            if cur == kind {
                // the items become paragraphs
                for (k, it) in ctx.items.iter().enumerate() {
                    let cp = cont_prefix(text, it.ls, it.marker.start);
                    if k > 0 && ctx.tight {
                        ed.insert(it.ls, format!("{}{eol}", cp.trim_end()));
                    }
                    if it.empty {
                        ed.push(it.marker.start..line_end(text, it.ls).0, "");
                    } else {
                        ed.push(it.marker.start..it.content, "");
                        self.reindent_rest(&mut ed, it, -(it.content_offset as isize));
                    }
                }
            } else {
                let start = ctx.items.first().and_then(|i| i.number).unwrap_or(1);
                for (k, it) in ctx.items.iter().enumerate() {
                    let has_box = it.syntax_end > it.marker.end;
                    let mark = match kind {
                        ListKind::Ordered => format!("{}.", start + k as u64),
                        _ => "-".to_string(),
                    };
                    // the continuation lines follow the marker's width (the box is not part
                    // of the content column)
                    let width_delta = mark.len() as isize - it.marker.len() as isize;
                    let boxed = match kind {
                        ListKind::Task if !has_box => format!("{mark} [ ]"),
                        _ => mark,
                    };
                    if kind != ListKind::Task && has_box && cur == ListKind::Task {
                        ed.push(it.marker.start..it.syntax_end, boxed);
                    } else {
                        ed.push(it.marker.range(), boxed);
                    }
                    if width_delta != 0 {
                        self.reindent_rest(&mut ed, it, width_delta);
                    }
                }
            }
            let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
            return Ok(Plan::single(PlanStep::patches(ps), None));
        }
        let leaf = self.leaf(at).ok_or(PlanError::NotApplicable("no block"))?;
        if !matches!(leaf.kind, LeafKind::Paragraph) || leaf.lines.is_empty() {
            return Err(PlanError::NotApplicable("not a paragraph"));
        }
        let marker = match kind {
            ListKind::Bullet => "- ",
            ListKind::Ordered => "1. ",
            ListKind::Task => "- [ ] ",
        };
        let indent = if kind == ListKind::Ordered { "   " } else { "  " };
        let mut ed = Edits::new();
        ed.insert(leaf.lines[0].start, marker);
        for l in &leaf.lines[1..] {
            ed.insert(l.start, indent);
        }
        let ps = ed.build().ok_or(PlanError::NotApplicable("overlap"))?;
        Ok(Plan::single(PlanStep::patches(ps), None))
    }

    /// Horizontal Line: a thematic break after the caret's block (same containers); the caret
    /// goes to the virtual paragraph after it.
    pub(crate) fn plan_insert_rule(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        let at = req.selection.primary().focus.offset;
        let text = self.text();
        let eol = self.line_ending().as_str();
        let leaf = self.leaf(at).ok_or(PlanError::NotApplicable("no block"))?;
        let last = leaf.lines.last().copied().unwrap_or(leaf.span);
        let ls = line_start(text, last.start);
        let prefix = cont_prefix(text, ls, last.start.max(ls));
        let blank = prefix.trim_end();
        let end = leaf.span.end;
        let ins = format!("{eol}{blank}{eol}{prefix}---");
        let caret = end + ins.len();
        Ok(Plan::single(
            PlanStep::patches(PatchSet::single_verbatim(end..end, ins)),
            Some(Selection::caret_at(DocPos::with_virt(caret, 1))),
        ))
    }
}

/// Maps offset `rel` in `old` (an item's text) to `new` (the same item after a marker swap /
/// re-indentation, same number of lines): on the first line positions after the marker move
/// by the marker's change; on later lines positions after the indentation move by that line's
/// change; positions inside changed indentation snap to the new indentation's end. The result
/// is on a char boundary of `new`.
fn map_moved_caret(old: &str, new: &str, rel: usize, marker_end: usize) -> usize {
    let (mut os, mut ns) = (0usize, 0usize);
    for (i, (ol, nl)) in old.split_inclusive('\n').zip(new.split_inclusive('\n')).enumerate() {
        if rel <= os + ol.trim_end_matches(['\n', '\r']).len() || os + ol.len() > rel {
            let col = rel - os;
            let ind = |l: &str| l.len() - l.trim_start_matches(['>', ' ', '\t']).len();
            let new_col = if i == 0 {
                if col >= marker_end {
                    (col as isize + nl.len() as isize - ol.len() as isize).max(0) as usize
                } else {
                    col
                }
            } else if col >= ind(ol) {
                (col as isize + nl.len() as isize - ol.len() as isize).max(0) as usize
            } else {
                col.min(ind(nl))
            };
            let mut at = (ns + new_col).min(new.len());
            while !new.is_char_boundary(at) {
                at -= 1;
            }
            return at;
        }
        os += ol.len();
        ns += nl.len();
    }
    new.len()
}

/// Shifts the indentation of every line of `t` after the first by `delta` columns (spaces
/// after the first `prefix_cols` prefix bytes of quote markers / indentation).
fn reindent_text(t: &str, prefix_cols: usize, delta: isize) -> String {
    if delta == 0 {
        return t.to_string();
    }
    let mut out = String::with_capacity(t.len() + 16);
    for (i, line) in t.split_inclusive('\n').enumerate() {
        if i == 0 || line.trim().is_empty() {
            out.push_str(line);
            continue;
        }
        let b = line.as_bytes();
        let mut p = 0;
        while p < b.len() && p < prefix_cols && (b[p] == b'>' || b[p] == b' ' || b[p] == b'\t') {
            p += 1;
        }
        out.push_str(&line[..p]);
        let rest = &line[p..];
        if delta > 0 {
            out.push_str(&" ".repeat(delta as usize));
            out.push_str(rest);
        } else {
            let sp = rest.bytes().take_while(|c| *c == b' ').count().min((-delta) as usize);
            out.push_str(&rest[sp..]);
        }
    }
    out
}

/// The list kind of a toggle command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKind {
    Bullet,
    Ordered,
    Task,
}
