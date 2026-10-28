//! Selection and delete commands (spec 02 §10, C8): word, line, styled scope, block.
//!
//! - Select Word: the word (UAX #29) at the caret; repeating does nothing (C8).
//! - Select Line: the source line of the block text holding the caret; repeating extends to
//!   the end of the next line (C8).
//! - Select Styled Scope: the innermost styled span's content, then the whole span with its
//!   markers, then the enclosing span …, then the block (C8).
//! - Select Block: the block's text.
//! - Delete Word / Line / Styled Scope / Block: delete what the selection command would
//!   select at a collapsed caret (Delete Block removes the whole block with its separator).

use markise_doc::{
    FormatDoc, InlineNode, InlineSource, LeafKind, LeafView, PatchSet, Plan, PlanError, PlanRequest, PlanStep,
    Selection,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::text::line_end;
use crate::{Document, line_start};

/// Which unit a selection / delete command works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Word,
    Line,
    Scope,
    Block,
}

fn text_leaf(l: &LeafView) -> bool {
    matches!(l.kind, LeafKind::Paragraph | LeafKind::Heading { .. })
}

impl Document {
    /// The word range at `at` inside the block's content line, if any.
    fn word_at(&self, leaf: &LeafView, at: usize) -> Option<(usize, usize)> {
        let line = leaf.lines.iter().find(|l| l.start <= at && at <= l.end)?;
        let s = &self.text()[line.start..line.end];
        let rel = at - line.start;
        let mut best = None;
        for (i, w) in s.split_word_bound_indices() {
            let (a, b) = (i, i + w.len());
            if a <= rel && rel <= b && w.chars().any(char::is_alphanumeric) {
                best = Some((line.start + a, line.start + b));
                if rel < b {
                    break;
                }
            }
        }
        if best.is_none() {
            // the caret next to hidden markers (`|**word**`): the word they enclose
            let skip = s[rel..].len()
                - s[rel..]
                    .trim_start_matches(|c: char| !c.is_alphanumeric() && !c.is_whitespace())
                    .len();
            if skip > 0 {
                return self.word_at(leaf, at + skip);
            }
        }
        best
    }

    fn scope_ranges(&self, leaf: &LeafView) -> Vec<(usize, usize)> {
        let rich = self.inline(InlineSource::Lines(&leaf.lines), 0);
        let mut out = Vec::new();
        fn walk(ns: &[InlineNode], out: &mut Vec<(usize, usize)>) {
            for n in ns {
                if !n.markers.is_empty() {
                    out.push((n.content.start as usize, n.content.end as usize));
                    out.push((n.source.start as usize, n.source.end as usize));
                }
                walk(&n.children, out);
            }
        }
        walk(&rich.nodes, &mut out);
        out
    }

    /// The range a unit command selects for the current selection (`None`: nothing).
    pub(crate) fn unit_range(&self, unit: Unit, req: &PlanRequest) -> Option<(usize, usize)> {
        let sel = req.selection.primary();
        let r = sel.range();
        let leaf = self.leaf(sel.focus.offset)?;
        let (first, last) = (*leaf.lines.first()?, *leaf.lines.last()?);
        match unit {
            Unit::Word => {
                if !text_leaf(&leaf) {
                    return None;
                }
                if !sel.is_caret() {
                    return Some((r.start, r.end)); // C8: repeat is a no-op
                }
                self.word_at(&leaf, sel.focus.offset)
            }
            Unit::Line => {
                let i = leaf.lines.iter().position(|l| l.start <= r.start && r.start <= l.end)?;
                let line = leaf.lines[i];
                if r.start == line.start && r.end == line.end || (r.start == line.start && r.end > line.end) {
                    // extend to the end of the next line (C8)
                    let j = leaf
                        .lines
                        .iter()
                        .position(|l| l.end >= r.end)
                        .unwrap_or(leaf.lines.len() - 1);
                    if j + 1 < leaf.lines.len() {
                        return Some((r.start, leaf.lines[j + 1].end));
                    }
                    // the next block's first line
                    let next = self
                        .leaves_in(last.end + 1, last.end + 1)
                        .into_iter()
                        .find(|l| l.lines.first().is_some_and(|f| f.start > last.end));
                    return next
                        .and_then(|n| n.lines.first().map(|f| (r.start, f.end)))
                        .or(Some((r.start, r.end)));
                }
                Some((line.start, line.end))
            }
            Unit::Scope => {
                let mut cands: Vec<(usize, usize)> = if text_leaf(&leaf) {
                    self.scope_ranges(&leaf)
                } else {
                    Vec::new()
                };
                cands.push((first.start, last.end));
                cands
                    .into_iter()
                    .filter(|&(a, b)| a <= r.start && r.end <= b && (a, b) != (r.start, r.end) && b > a)
                    .min_by_key(|&(a, b)| b - a)
            }
            Unit::Block => Some((first.start, last.end)),
        }
    }

    pub(crate) fn plan_select(&self, unit: Unit, req: &PlanRequest) -> Result<Plan, PlanError> {
        let (a, b) = self
            .unit_range(unit, req)
            .ok_or(PlanError::NotApplicable("nothing to select"))?;
        Ok(Plan::select(Selection::range(a, b)))
    }

    pub(crate) fn plan_delete_unit(&self, unit: Unit, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        if !sel.is_caret() {
            let r = sel.range();
            return Ok(Plan::single(
                PlanStep::patches(PatchSet::delete(r.clone())),
                Some(Selection::caret(r.start)),
            ));
        }
        if unit == Unit::Block {
            return self.plan_delete_block(sel.focus.offset);
        }
        let (mut a, mut b) = self
            .unit_range(unit, req)
            .ok_or(PlanError::NotApplicable("nothing to delete"))?;
        if a == b {
            return Err(PlanError::NotApplicable("empty"));
        }
        if unit == Unit::Line
            && let Some(leaf) = self.leaf(sel.focus.offset)
            && leaf.lines.len() > 1
            && let Some(i) = leaf.lines.iter().position(|l| l.start == a && l.end == b)
        {
            // a whole soft line goes with its line break (an empty soft line cannot stay)
            if i + 1 < leaf.lines.len() {
                b = leaf.lines[i + 1].start;
            } else {
                a = leaf.lines[i - 1].end;
            }
        }
        Ok(Plan::single(
            PlanStep::patches(PatchSet::delete(a..b)),
            Some(Selection::caret(a)),
        ))
    }

    /// Delete Paragraph / Block: the leaf's lines and one separator go.
    fn plan_delete_block(&self, at: usize) -> Result<Plan, PlanError> {
        let text = self.text();
        let leaf = self.leaf(at).ok_or(PlanError::NotApplicable("no block"))?;
        let ls = line_start(text, leaf.span.start);
        let (le, next) = line_end(text, leaf.span.end);
        // the block is the first content of a list item or quote line: delete its text only
        let prefix = &text[ls..leaf.span.start];
        if !prefix.trim().is_empty() && !prefix.trim_start().starts_with('>') || leaf.containers.len() > 1 {
            let first = leaf.lines.first().map_or(leaf.span.start, |l| l.start);
            let last = leaf.lines.last().map_or(leaf.span.end, |l| l.end);
            return Ok(Plan::single(
                PlanStep::patches(PatchSet::delete(first..last)),
                Some(Selection::caret(first)),
            ));
        }
        // whole lines plus the blank lines after them; the last block takes the separator
        // before it instead
        let mut end = next;
        while end < text.len() {
            let (e2, n2) = line_end(text, end);
            if !text[end..e2].trim().is_empty() {
                break;
            }
            end = if n2 == e2 { e2 } else { n2 };
            if n2 == e2 {
                break;
            }
        }
        let (start, end) = if end >= text.len() {
            // the separator after the previous block from the structure — up to its end,
            // never into it (an empty quote before the block stays; ADR 0011)
            match self.adjacent_block(leaf.span.start, false) {
                Some(p) => (p.block.end, le),
                None => (ls, le),
            }
        } else {
            (ls, end)
        };
        Ok(Plan::single(
            PlanStep::patches(PatchSet::delete(start..end)),
            Some(Selection::caret(start)),
        ))
    }
}
