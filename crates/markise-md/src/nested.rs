//! Nested restart checkpoints (ADR 0006 §7.2.3): an edit inside a large top-level container
//! (list, block quote, footnote definition, nested lists) re-parses from a child boundary of
//! the deepest container on the path to the edit — with the container stack reconstructed —
//! and re-synchronises at the first old sibling after the edit, instead of re-parsing the whole
//! top-level block. The result is spliced into the top-level block in place (relative
//! coordinates), so the cost is bounded by the re-parsed children plus a shift of the
//! container's tail.
//!
//! Semantics never change: a restart is taken only where the reconstructed state equals the
//! state a full parse has at that line (see `pick` and `parser::parse_nested`), and the
//! property tests compare every edit with a fresh parse.

use std::ops::Range;
use std::sync::OnceLock;

use markise_doc::{DepChange, HintSet, PatchSet};

use crate::block::Block;
use crate::parser::{self, Nested, NestedPlan};
use crate::text::{count_terminators, line_end};
use crate::{Document, EditResult, definition_labels, line_start};

/// A chosen restart: top-level block index, path indices (child index on the path at each
/// level, the last one is the first re-parsed child) and the restart line.
struct Pick {
    top: usize,
    kept: Vec<usize>,
    restart: usize,
}

impl Document {
    /// The deepest restart point for an edit at `anchor..range.end` inside one top-level
    /// container, if any.
    fn pick(&self, range: &Range<usize>, anchor: usize) -> Option<Pick> {
        let text = &self.text;
        let t = self.top.partition_point(|b| b.start() <= anchor).checked_sub(1)?;
        let tb = self.top.get(t)?;
        if anchor < tb.start() || range.end > tb.end() || !tb.block.is_container() {
            return None;
        }
        let o = tb.origin;
        let a = anchor - o;
        let anchor_line = line_start(text, anchor);
        // the containers on the path and the child index containing the anchor at each level
        let mut path: Vec<&Block> = vec![tb.block];
        let mut idx: Vec<usize> = Vec::new();
        loop {
            let x = *path.last().unwrap();
            let c = x.children.partition_point(|ch| ch.span.start <= a);
            if c == 0 {
                break;
            }
            idx.push(c - 1);
            let ch = &x.children[c - 1];
            if ch.is_container() && a <= ch.span.end {
                path.push(ch);
            } else {
                break;
            }
        }
        // deepest level with a child that starts on a line after the container's first line,
        // whose first line is entirely before the edit
        for level in (0..idx.len()).rev() {
            let x = path[level];
            let xl = line_start(text, x.span.start + o);
            let ci = idx[level];
            for r in [ci, ci.wrapping_sub(1)] {
                let Some(ch) = x.children.get(r) else { continue };
                let cl = line_start(text, ch.span.start + o);
                if cl <= xl {
                    continue;
                }
                let (_, next) = line_end(text, cl);
                if next > anchor_line || next > range.start {
                    continue;
                }
                // outside a list, the restart line could continue an open paragraph or table of
                // the previous sibling (lazy continuation) when it no longer starts a block —
                // which may depend on look-ahead into the edited text (math/front-matter
                // closers): restart only where that sibling is certainly closed
                // (in a list a marker line after an item always starts a sibling item; a
                // paragraph directly in a list only comes from a literal-line hint)
                if r > 0
                    && let Some(prev) = x.children.get(r - 1)
                    && !(matches!(x.kind, crate::block::BlockKind::List { .. })
                        && matches!(prev.kind, crate::block::BlockKind::ListItem { .. }))
                    && can_absorb(prev)
                    && count_terminators(&text[prev.span.end + o..ch.span.start + o]) < 2
                {
                    continue;
                }
                let mut kept = idx[..level].to_vec();
                kept.push(r);
                return Some(Pick {
                    top: t,
                    kept,
                    restart: cl,
                });
            }
        }
        None
    }

    /// Applies an edit through a nested restart when one exists; `None` leaves the document
    /// untouched (the caller re-parses from a top-level block).
    pub(crate) fn edit_nested(
        &mut self,
        range: &Range<usize>,
        insert: &str,
        anchor: usize,
        hint_ps: Option<&PatchSet>,
        final_hints: Option<&HintSet>,
    ) -> Option<EditResult> {
        if range.start <= crate::bom_len(&self.text) {
            return None;
        }
        let pick = self.pick(range, anchor)?;
        let t = pick.top;
        let tb = self.top.get(t)?;
        let o = tb.origin;
        // path blocks (old, relative)
        let mut path: Vec<&Block> = vec![tb.block];
        for &k in &pick.kept[..pick.kept.len() - 1] {
            path.push(&path.last().unwrap().children[k]);
        }
        let plan = NestedPlan {
            origin: o,
            path: path.clone(),
            kept: pick.kept.clone(),
            restart: pick.restart,
        };
        let prep = parser::prepare_nested(&self.text, &self.opts, &plan)?;
        let lv = pick.kept.len() - 1;
        // the first re-parsed child's own start (after its line's container prefixes): where
        // the changed leaves begin
        let region_start = path[lv].children[pick.kept[lv]].span.start + o;
        // definition labels of the old children the edit overlaps (read before the change)
        let mut labels: Vec<u64> = Vec::new();
        for ch in &path[lv].children[pick.kept[lv]..] {
            if ch.span.start + o > range.end {
                break;
            }
            definition_labels(&self.text, ch, o as isize, &mut labels);
        }
        // relative starts of the old path containers (identity of their reconstructions)
        let path_starts: Vec<usize> = path.iter().map(|b| b.span.start).collect();
        drop(path);

        // a line starting where the edit ends is unchanged only if the edit ended at a line
        // start of the old text (else the edit changed the start of that line)
        let end_at_line_start = line_start(&self.text, range.end) == range.end;

        // the text change (as in `edit_core`)
        self.abs = OnceLock::new();
        let delta = insert.len() as isize - range.len() as isize;
        let new_edit_end = range.start + insert.len();
        let old_hints = self.apply_text_change(range, insert, hint_ps, final_hints);

        let old_edit_end = range.end;
        let text = &self.text;
        let hints = &self.hints;
        let blocks = &self.top;
        let kept = &pick.kept;
        // old (relative) container at `level` on the path
        let old_at = |level: usize| -> &Block {
            let mut b = blocks.get(t).unwrap().block;
            for &k in &kept[..level] {
                b = &b.children[k];
            }
            b
        };
        // `pos` (new coordinates) and its whole line lie after the edit; the old position
        let after_edit = |pos: usize| -> Option<usize> {
            let ls = line_start(text, pos);
            if ls < new_edit_end || (ls == new_edit_end && !end_at_line_start) {
                return None;
            }
            let old_ls = (ls as isize - delta) as usize;
            if old_ls < old_edit_end || hints.get(ls).map(|h| h.col) != old_hints.get(old_ls).map(|h| h.col) {
                return None;
            }
            Some((pos as isize - delta) as usize)
        };
        // the child started on the line is the old sibling: same start, same kind of block
        let nested_sync = |level: usize, pos: usize, kind: &crate::block::BlockKind| -> bool {
            let Some(old_pos) = after_edit(pos) else { return false };
            let x = old_at(level);
            let rel = old_pos - o;
            let j = x.children.partition_point(|c| c.span.start < rel);
            if !(j > kept[level]
                && x.children
                    .get(j)
                    .is_some_and(|c| c.span.start == rel && same_start(&c.kind, kind)))
            {
                return false;
            }
            // a block directly after a link reference definition is the rest of the paragraph
            // the definition was extracted from (it opened on an earlier line, so its fate —
            // e.g. a table or setext heading — differs from a block opened here): not an
            // independent sibling. Its old gap is known only when it lies after the edit.
            let prev = &x.children[j - 1];
            if matches!(prev.kind, crate::block::BlockKind::LinkRefDef { .. }) {
                let gap_old = prev.span.end + o..rel + o;
                if gap_old.start < old_edit_end {
                    return false;
                }
                let gap_new = (gap_old.start as isize + delta) as usize..pos;
                if count_terminators(&text[gap_new]) <= 1 {
                    return false;
                }
            }
            true
        };
        let find_old = |pos: usize| -> Option<usize> {
            let old_pos = after_edit(pos)?;
            let j = blocks.partition_point(|b| b.start() < old_pos);
            let b = blocks.get(j)?;
            (j > t && b.clean_line() == Some(old_pos)).then_some(j)
        };
        let top_sync = |pos: usize| find_old(pos).is_some();
        let out = parser::parse_nested(text, &self.opts, prep, &top_sync, &nested_sync, hints);

        let restart = pick.restart;
        let region_start_ok = kept[lv] > 0;
        match out {
            Nested::Synced {
                level,
                chain,
                sibling,
                failed_openers,
            } => {
                // old children replaced, read after the change (they lie after the edit except
                // the overlapping ones read above); new children
                let sib_rel = (sibling as isize - delta) as usize - o;
                {
                    let x = old_at(level);
                    let j = x.children.partition_point(|c| c.span.start < sib_rel);
                    for (d, &k) in kept.iter().enumerate().skip(level) {
                        let xd = old_at(d);
                        let lo = if d == lv { k } else { k + 1 };
                        let hi = if d == level { j } else { xd.children.len() };
                        for ch in &xd.children[lo.min(hi)..hi] {
                            if ch.span.start + o > range.end {
                                definition_labels(text, ch, o as isize + delta, &mut labels);
                            }
                        }
                    }
                    let _ = x;
                }
                for b in &chain {
                    for ch in &b.children {
                        definition_labels(text, ch, 0, &mut labels);
                    }
                }
                let flips = self.assemble_synced(t, o, kept, &path_starts, level, chain, sib_rel, delta);
                self.top.shift_after(t, delta);
                // failed openers: before the restart, new ones, old ones after the sibling
                let old_sib = (sibling as isize - delta) as usize;
                let mut fo: Vec<usize> = self.failed_openers.iter().copied().filter(|&f| f < restart).collect();
                fo.extend(failed_openers.iter().copied().filter(|&f| f < sibling));
                fo.extend(
                    self.failed_openers
                        .iter()
                        .filter(|&&f| f >= old_sib)
                        .map(|&f| (f as isize + delta) as usize),
                );
                fo.sort_unstable();
                fo.dedup();
                self.failed_openers = fo;
                // with a region: it ends at the re-synchronised sibling; without (a container's
                // computed state changed): the whole top-level block
                let nested = (region_start_ok && !flips).then_some(region_start);
                let end = match nested {
                    Some(_) => sibling,
                    None => self.top.get(t + 1).map_or(self.text.len(), |b| b.origin),
                };
                Some(self.finish_nested(t..t + 1, 1, o, end, labels, nested))
            }
            Nested::Top(out) => {
                let (resume_idx, reparsed_end) = match out.stopped_at {
                    Some(pos) => (find_old(pos).unwrap(), pos),
                    None => (self.top.len(), self.text.len()),
                };
                // old children after the kept/path ones at every level and old top blocks up
                // to the resync point are replaced
                for (d, &k) in kept.iter().enumerate() {
                    let xd = old_at(d);
                    let lo = if d == lv { k } else { k + 1 };
                    for ch in &xd.children[lo.min(xd.children.len())..] {
                        if ch.span.start + o > range.end {
                            definition_labels(text, ch, o as isize + delta, &mut labels);
                        }
                    }
                }
                for b in self.top.range(t + 1..resume_idx) {
                    definition_labels(text, b.block, b.origin as isize + delta, &mut labels);
                }
                for b in &out.blocks {
                    definition_labels(text, b, 0, &mut labels);
                }
                let mut blocks_new = out.blocks;
                if blocks_new.is_empty() {
                    // cannot happen: the reconstructed container is always emitted
                    return None;
                }
                let mut first = blocks_new.remove(0);
                first.shift(-(o as isize));
                let old = self.top.get_mut(t).expect("block");
                let flips = fill(
                    &self.text,
                    &self.opts,
                    &self.hints,
                    &mut first,
                    old,
                    kept,
                    &path_starts,
                    0,
                    o,
                );
                let mut entries: Vec<(usize, Block)> = vec![(o, first)];
                for mut b in blocks_new {
                    use markise_doc::chunked::Relative;
                    let org = b.origin();
                    b.shift(-(org as isize));
                    entries.push((org, b));
                }
                let n_new = entries.len();
                let replaced = resume_idx - t;
                self.top.splice_entries(t..resume_idx, entries, delta);
                let old_resume = (reparsed_end as isize - delta) as usize;
                let mut fo: Vec<usize> = self.failed_openers.iter().copied().filter(|&f| f < restart).collect();
                fo.extend(out.failed_openers.iter().copied().filter(|&f| f < reparsed_end));
                if out.stopped_at.is_some() {
                    fo.extend(
                        self.failed_openers
                            .iter()
                            .filter(|&&f| f >= old_resume)
                            .map(|&f| (f as isize + delta) as usize),
                    );
                }
                fo.sort_unstable();
                fo.dedup();
                self.failed_openers = fo;
                Some(self.finish_nested(
                    t..t + n_new,
                    replaced,
                    o,
                    reparsed_end,
                    labels,
                    (region_start_ok && !flips).then_some(region_start),
                ))
            }
        }
    }

    /// Splices a nested re-sync at `level` into top-level block `t` (relative to `o`).
    /// Returns whether a container's computed state (list tightness, callout) changed.
    #[allow(clippy::too_many_arguments)]
    fn assemble_synced(
        &mut self,
        t: usize,
        o: usize,
        kept: &[usize],
        path_starts: &[usize],
        level: usize,
        mut chain: Vec<Block>,
        sib_rel: usize,
        delta: isize,
    ) -> bool {
        let text = &self.text;
        let opts = &self.opts;
        let hints = &self.hints;
        let lv = kept.len() - 1;
        let top = self.top.get_mut(t).expect("block");
        let mut flips = false;
        // descend to the old container at `level`, shifting tails and ends on the way
        fn descend<'b>(
            b: &'b mut Block,
            kept: &[usize],
            level: usize,
            d: usize,
            delta: isize,
            visit: &mut dyn FnMut(&mut Block, usize),
        ) -> &'b mut Block {
            if d == level {
                return b;
            }
            b.span.end = (b.span.end as isize + delta) as usize;
            for c in &mut b.children[kept[d] + 1..] {
                c.shift(delta);
            }
            visit(b, d);
            let k = kept[d];
            descend(&mut b.children[k], kept, level, d + 1, delta, visit)
        }
        let mut ancestors: Vec<usize> = Vec::new();
        let x = descend(top, kept, level, 0, delta, &mut |_, d| ancestors.push(d));
        // the new children of the container at `level`
        let new_parent = chain.swap_remove(level);
        let mut newc = new_parent.children;
        if kept[level] > 0 && !newc.is_empty() {
            newc.remove(0); // the placeholder of the kept children (they stay in place)
        }
        for c in &mut newc {
            c.shift(-(o as isize));
        }
        // a deeper reconstructed container closed during the re-parse: give it back its kept
        // children from the old one (which is replaced)
        if level < lv {
            let pc = kept[level];
            let ps = path_starts[level + 1];
            if let Some(nx) = newc.iter_mut().find(|c| c.span.start == ps) {
                flips |= fill(
                    text,
                    opts,
                    hints,
                    nx,
                    &mut x.children[pc],
                    kept,
                    path_starts,
                    level + 1,
                    o,
                );
            }
        }
        let j = x.children.partition_point(|c| c.span.start < sib_rel);
        let from = kept[level];
        let n_new = newc.len();
        x.children.splice(from..j, newc);
        for c in &mut x.children[from + n_new..] {
            c.shift(delta);
        }
        x.span.end = (x.span.end as isize + delta) as usize;
        let before = state_of(x);
        // only the spliced children changed (the others shifted)
        parser::refinish_spliced(text, opts, hints, x, o, from..from + n_new);
        flips |= before != state_of(x);
        // ancestors: recompute their states (tightness may depend on the spliced item, the
        // only changed child on the path)
        let top = self.top.get_mut(t).expect("block");
        let mut b: &mut Block = top;
        for &k in &kept[..level] {
            let before = state_of(b);
            parser::refinish_spliced(text, opts, hints, b, o, k..k + 1);
            flips |= before != state_of(b);
            b = &mut b.children[k];
        }
        flips
    }

    fn finish_nested(
        &mut self,
        new_blocks: Range<usize>,
        replaced: usize,
        reparsed_start: usize,
        reparsed_end: usize,
        mut labels: Vec<u64>,
        nested_from: Option<usize>,
    ) -> EditResult {
        let mut deps = DepChange::default();
        if !labels.is_empty() {
            labels.sort_unstable();
            labels.dedup();
            deps.labels = labels;
            self.ctx = OnceLock::new();
        }
        EditResult {
            new_blocks,
            replaced,
            reparsed: reparsed_start..reparsed_end,
            deps,
            inline_leaf: None,
            nested_from,
        }
    }
}

/// Whether a line after `b` may continue it lazily: its deepest last block is a paragraph, a
/// table or a link reference definition (extracted from a paragraph).
fn can_absorb(b: &Block) -> bool {
    let mut b = b;
    loop {
        match &b.kind {
            // definitions are extracted from a paragraph when it closes: the paragraph may still
            // be open at the next line
            crate::block::BlockKind::Paragraph
            | crate::block::BlockKind::Table { .. }
            | crate::block::BlockKind::LinkRefDef { .. } => return true,
            _ => match b.children.last() {
                Some(c) => b = c,
                None => return false,
            },
        }
    }
}

/// Whether a block started on a line (`new`, just opened) is the same kind of block as the old
/// one that started there: same variant, and for list items the same marker shape and content
/// column (spans compared by length, they are in different coordinates).
fn same_start(old: &crate::block::BlockKind, new: &crate::block::BlockKind) -> bool {
    use crate::block::BlockKind as K;
    if std::mem::discriminant(old) != std::mem::discriminant(new) {
        return false;
    }
    match (old, new) {
        (
            K::ListItem {
                marker: a,
                number: na,
                task: ta,
                content_offset: ca,
            },
            K::ListItem {
                marker: b,
                number: nb,
                task: tb,
                content_offset: cb,
            },
        ) => a.len() == b.len() && na == nb && ta.map(|t| t.0) == tb.map(|t| t.0) && ca == cb,
        (
            K::List {
                ordered: oa,
                marker: ma,
                ..
            },
            K::List {
                ordered: ob,
                marker: mb,
                ..
            },
        ) => oa == ob && ma == mb,
        _ => true,
    }
}

/// Computed state of a container (list tightness, callout kind).
fn state_of(b: &Block) -> Option<crate::block::BlockKind> {
    match &b.kind {
        k @ (crate::block::BlockKind::List { .. } | crate::block::BlockKind::BlockQuote { .. }) => Some(k.clone()),
        _ => None,
    }
}

/// Gives the reconstructed container `new` (relative, level `d`) its kept children from the
/// old container `old` (moved), recursively for the path below; recomputes container state.
/// Returns whether a computed state changed.
#[allow(clippy::too_many_arguments)]
fn fill(
    text: &str,
    opts: &crate::ParseOptions,
    hints: &HintSet,
    new: &mut Block,
    old: &mut Block,
    kept: &[usize],
    path_starts: &[usize],
    d: usize,
    o: usize,
) -> bool {
    let mut flips = false;
    let lv = kept.len() - 1;
    if d < lv {
        let pc = kept[d];
        let ps = path_starts[d + 1];
        if let (Some(nx), Some(ox)) = (
            new.children.iter_mut().find(|c| c.span.start == ps),
            old.children.get_mut(pc),
        ) {
            flips |= fill(text, opts, hints, nx, ox, kept, path_starts, d + 1, o);
        }
    }
    if kept[d] > 0 && !new.children.is_empty() {
        let k: Vec<Block> = old.children.drain(..kept[d]).collect();
        new.children.splice(0..1, k);
    }
    let before = state_of(old);
    parser::refinish(text, opts, hints, new, o);
    flips |= before != state_of(new);
    flips
}
