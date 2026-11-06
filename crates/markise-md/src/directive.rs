//! Fenced directives (`:::name` … `:::`, ADR 0018): the document-wide pairing of marker lines.
//!
//! The parser never asks whether a marker pairs: a marker line is always a block of its own and
//! parsing the lines around it does not depend on the answer. So the top-level block list stays
//! flat (markers and the content between them are siblings) and pairing is a pure function of
//! the sequence of marker lines ([`pair`]). This module keeps that sequence in a chunked
//! relative-offset list, updates it from each edit's re-parsed region, labels the marker blocks
//! ([`BlockKind::DirectiveOpen`], [`BlockKind::DirectiveClose`], or a plain paragraph when
//! unpaired) and folds the flat list into the tree view ([`Document::tree`]).
//!
//! An edit that leaves the region's marker sequence alone costs a shift of the later markers,
//! O(chunks); an edit that changes it costs O(markers), with no parse beyond the edit's own
//! region, however far apart an opener and its closer are.

use std::ops::Range;

use markise_doc::chunked::{RelChunks, Relative};

use crate::block::{Block, BlockKind};
use crate::scan;
use crate::text::{Span, line_end};
use crate::{Document, EditResult};

/// Deepest stack of openers still waiting for a closer. An opener beyond it is ordinary text;
/// this bounds the depth of the tree view (recursive traversal, `Drop`).
pub const MAX_DIRECTIVE_DEPTH: usize = 32;

const NONE: u32 = u32::MAX;

/// A marker line as the parser sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawMarker {
    /// First colon of the marker (absolute).
    pub at: usize,
    pub fence: usize,
    pub close: bool,
    /// Bytes from `at` to the end of the line's content.
    pub line_len: usize,
}

#[derive(Clone, Copy, Debug)]
struct Marker {
    /// First colon (absolute before insertion, 0 once stored relative to the list's origin).
    at: usize,
    fence: u32,
    close: bool,
    /// Bytes from `at` to the end of the line's content, so that "is this position on the
    /// marker's line" does not read the line (it may hold megabytes of whitespace). Any edit of
    /// the line re-parses the marker.
    line_len: usize,
    /// Index of the other marker of the pair.
    partner: u32,
    /// Index of the opener of the innermost directive around this marker (around both markers
    /// of a pair, not the pair itself).
    parent: u32,
}

impl Relative for Marker {
    fn origin(&self) -> usize {
        self.at
    }
    fn shift(&mut self, delta: isize) {
        self.at = (self.at as isize + delta) as usize;
    }
}

/// See the module docs.
#[derive(Clone, Debug, Default)]
pub(crate) struct DirectiveIndex {
    markers: RelChunks<Marker>,
    /// Number of paired directives.
    paired: usize,
}

/// A directive of the document (absolute spans).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectiveView {
    /// Number of colons of the opener.
    pub fence_len: usize,
    /// The name after the colons.
    pub name: Span,
    /// The trimmed arguments after the name (empty right after the name when there are none).
    pub args: Span,
    /// The opener line from its first colon, line terminator excluded.
    pub opener: Span,
    /// The closer line from its first colon, line terminator excluded.
    pub closer: Span,
}

impl DirectiveView {
    /// From the first colon of the opener to the end of the closer line.
    pub fn span(&self) -> Span {
        Span::new(self.opener.start, self.closer.end)
    }
}

/// Partner of each marker; `seq` holds `(fence, close)`. The rule (ADR 0018): a new opener ends
/// every waiting opener of the same or a shorter fence, which stay text; a closer closes the
/// outermost waiting opener whose fence is not longer than its own, and every opener inside
/// that one stays text. A closer with no such opener is text.
pub(crate) fn pair(seq: &[(u32, bool)]) -> Vec<u32> {
    let mut partner = vec![NONE; seq.len()];
    let mut waiting: Vec<usize> = Vec::new();
    for (i, &(fence, close)) in seq.iter().enumerate() {
        if close {
            if let Some(k) = waiting.iter().position(|&o| seq[o].0 <= fence) {
                partner[waiting[k]] = i as u32;
                partner[i] = waiting[k] as u32;
                waiting.truncate(k);
            }
        } else {
            while waiting.last().is_some_and(|&o| seq[o].0 <= fence) {
                waiting.pop();
            }
            if waiting.len() < MAX_DIRECTIVE_DEPTH {
                waiting.push(i);
            }
        }
    }
    partner
}

impl DirectiveIndex {
    pub(crate) fn from_raw(raw: Vec<RawMarker>) -> Self {
        let mut x = DirectiveIndex {
            markers: RelChunks::from_abs_vec(raw.into_iter().map(to_marker).collect()),
            paired: 0,
        };
        x.pair_all();
        x
    }

    /// Recomputes partners and parents from the marker sequence.
    fn pair_all(&mut self) {
        let seq: Vec<(u32, bool)> = self.markers.iter().map(|m| (m.item.fence, m.item.close)).collect();
        let partner = pair(&seq);
        let mut open: Vec<u32> = Vec::new();
        let mut paired = 0;
        for (i, m) in self.markers.iter_mut().enumerate() {
            let p = partner[i];
            m.partner = p;
            if p != NONE && m.close {
                open.pop();
            }
            m.parent = open.last().copied().unwrap_or(NONE);
            if p != NONE && !m.close {
                open.push(i as u32);
                paired += 1;
            }
        }
        self.paired = paired;
    }

    fn at(&self, i: u32) -> usize {
        self.markers.origin(i as usize).expect("marker index")
    }

    /// Replaces the markers in `old` (old coordinates, end exclusive) with `new` (new
    /// coordinates) and shifts the ones after by `delta`. Returns the indices of the markers
    /// whose block must be labelled again, and the byte range (new coordinates) where the
    /// structure around blocks changed because a pair changed, if any.
    pub(crate) fn splice(
        &mut self,
        old: Range<usize>,
        new: Vec<RawMarker>,
        delta: isize,
    ) -> (Vec<usize>, Option<Range<usize>>) {
        if self.markers.is_empty() && new.is_empty() {
            return (Vec::new(), None);
        }
        let i0 = self.markers.partition_point(|m| m.origin < old.start);
        let i1 = i0
            + self
                .markers
                .range(i0..self.markers.len())
                .take_while(|m| m.origin < old.end)
                .count();
        let same = i1 - i0 == new.len()
            && self
                .markers
                .range(i0..i1)
                .zip(&new)
                .all(|(a, b)| a.item.fence as usize == b.fence && a.item.close == b.close);
        if same {
            // same sequence: same pairs; keep them, and the indices
            let kept: Vec<(u32, u32)> = self
                .markers
                .range(i0..i1)
                .map(|m| (m.item.partner, m.item.parent))
                .collect();
            let n = new.len();
            let items = new
                .into_iter()
                .zip(kept)
                .map(|(r, (partner, parent))| Marker {
                    partner,
                    parent,
                    ..to_marker(r)
                })
                .collect();
            self.markers.splice(i0..i1, items, delta);
            return ((i0..i0 + n).collect(), None);
        }
        // the old pairs in new coordinates (`None`: inside the replaced region)
        let to_new = |i: usize, at: usize| -> Option<usize> {
            if i < i0 {
                Some(at)
            } else if i >= i1 {
                Some((at as isize + delta) as usize)
            } else {
                None
            }
        };
        let old_ats: Vec<usize> = self.markers.iter().map(|m| m.origin).collect();
        let old_partner: Vec<u32> = self.markers.iter().map(|m| m.item.partner).collect();
        let n_new = new.len();
        self.markers
            .splice(i0..i1, new.into_iter().map(to_marker).collect(), delta);
        self.pair_all();
        let mut label: Vec<usize> = (i0..i0 + n_new).collect();
        let mut region: Option<Range<usize>> = None;
        let mut widen = |p: usize| {
            region = Some(match &region {
                Some(r) => r.start.min(p)..r.end.max(p),
                None => p..p,
            });
        };
        let new_index = |i: usize| if i < i0 { i } else { i - (i1 - i0) + n_new };
        let cur: Vec<u32> = self.markers.iter().map(|m| m.item.partner).collect();
        let cur_ats: Vec<usize> = self.markers.iter().map(|m| m.origin).collect();
        for (i, &oat) in old_ats.iter().enumerate() {
            if (i0..i1).contains(&i) {
                continue;
            }
            let ni = new_index(i);
            let at = to_new(i, oat).unwrap();
            let old_p = old_partner[i];
            let old_at = (old_p != NONE)
                .then(|| to_new(old_p as usize, old_ats[old_p as usize]))
                .map(|p| p.ok_or(()));
            let new_at = (cur[ni] != NONE).then(|| cur_ats[cur[ni] as usize]);
            let same_pair = match (&old_at, new_at) {
                (None, None) => true,
                (Some(Ok(a)), Some(b)) => *a == b,
                // the old partner was re-parsed: the same pair if it is still at the new
                // partner's place; the markers themselves stay labelled below
                _ => false,
            };
            if same_pair {
                continue;
            }
            label.push(ni);
            widen(at);
            if let Some(Ok(a)) = old_at {
                widen(a);
            }
            if let Some(b) = new_at {
                widen(b);
            }
        }
        // a marker of the region whose pair changed widens to its partner too
        for ni in i0..i0 + n_new {
            if cur[ni] != NONE {
                widen(cur_ats[ni]);
                widen(cur_ats[cur[ni] as usize]);
            }
        }
        label.sort_unstable();
        (label, region)
    }

    /// Shifts the markers at or after `pos` by `delta` (an edit before them that touched none).
    pub(crate) fn shift_from(&mut self, pos: usize, delta: isize) {
        if delta == 0 || self.markers.is_empty() {
            return;
        }
        let i = self.markers.partition_point(|m| m.origin < pos);
        if i == 0 {
            // every marker moves (`shift_after` starts after an index)
            self.markers.splice(0..0, Vec::new(), delta);
        } else {
            self.markers.shift_after(i - 1, delta);
        }
    }
}

fn to_marker(r: RawMarker) -> Marker {
    Marker {
        at: r.at,
        fence: r.fence as u32,
        close: r.close,
        line_len: r.line_len,
        partner: NONE,
        parent: NONE,
    }
}

impl Document {
    /// The directive whose opener is marker `i`.
    fn directive_view(&self, i: u32) -> Option<DirectiveView> {
        let x = &self.directives;
        let m = x.markers.get(i as usize)?;
        if m.item.close || m.item.partner == NONE {
            return None;
        }
        let text = self.text();
        let at = m.origin;
        let (le, _) = line_end(text, at);
        let l = scan::directive_marker(text, at, le)?;
        let cat = x.at(m.item.partner);
        let (cle, _) = line_end(text, cat);
        Some(DirectiveView {
            fence_len: l.fence,
            name: l.name,
            args: l.args,
            opener: Span::new(at, scan::trim_end_ws(text.as_bytes(), at, le)),
            closer: Span::new(cat, scan::trim_end_ws(text.as_bytes(), cat, cle)),
        })
    }

    /// Indices of the paired markers whose line overlaps `a..=b`. O(log markers + found).
    fn paired_markers_in(&self, a: usize, b: usize) -> Vec<usize> {
        let x = &self.directives;
        if x.paired == 0 {
            return Vec::new();
        }
        // a marker before `a` may still reach it on its line
        let first = x.markers.partition_point(|m| m.origin < a).saturating_sub(1);
        let mut out = Vec::new();
        for (k, m) in x.markers.range(first..x.markers.len()).enumerate() {
            // the line starts before the first colon (at most three columns of indentation)
            if crate::line_start(self.text(), m.origin) > b {
                break;
            }
            if m.item.partner != NONE && m.origin + m.item.line_len >= a {
                out.push(first + k);
            }
        }
        out
    }

    /// `true` when `a..=b` overlaps the line of a paired directive marker.
    pub(crate) fn touches_marker(&self, a: usize, b: usize) -> bool {
        !self.paired_markers_in(a, b).is_empty()
    }

    /// The virtual paragraph at `gap` stands at the document level (or in directives only).
    pub(crate) fn gap_at_document_level(&self, gap: usize, virt: u16) -> bool {
        use markise_doc::FormatDoc;
        let probe = gap.saturating_sub(1);
        self.containers_at(probe).len() < virt as usize
    }

    /// See `FormatDoc::selection_deletion`: `None` when no marker is cut; otherwise the patches
    /// deleting `range` except the lines of markers whose pair is not wholly inside it. Each
    /// kept line keeps its own line terminators, so it stays a line of its own.
    pub(crate) fn directive_safe_deletion(&self, range: Range<usize>) -> Option<markise_doc::PatchSet> {
        let text = self.text();
        let x = &self.directives;
        let whole = |at: usize| {
            let start = crate::line_start(text, at);
            let (end, _) = line_end(text, at);
            range.start <= start.min(at) && end <= range.end
        };
        let mut keep: Vec<(usize, usize)> = Vec::new();
        for i in self.paired_markers_in(range.start, range.end) {
            let m = x.markers.get(i)?;
            let partner = x.at(m.item.partner);
            if !(whole(m.origin) && whole(partner)) {
                keep.push((crate::line_start(text, m.origin), line_end(text, m.origin).1));
            }
        }
        if keep.is_empty() {
            return None;
        }
        let mut patches = Vec::new();
        let mut pos = range.start;
        for (ls, next) in keep {
            // up to the end of the line before the kept one (its terminator stays)
            let stop = if ls == 0 {
                0
            } else {
                crate::text::prev_line_content_end(text, ls)
            };
            if pos < stop {
                patches.push(markise_doc::Patch::verbatim(pos..stop, ""));
            }
            pos = pos.max(next);
        }
        if pos < range.end {
            patches.push(markise_doc::Patch::verbatim(pos..range.end, ""));
        }
        markise_doc::PatchSet::new(patches).ok()
    }

    /// Bytes from the opener at `at` to the end of its closer's line, from the index alone: no line
    /// is read, however long. `None` when no paired opener stands at `at`.
    pub(crate) fn paired_extent(&self, at: usize) -> Option<usize> {
        let x = &self.directives;
        let i = x.markers.partition_point(|m| m.origin < at);
        let m = x
            .markers
            .get(i)
            .filter(|m| m.origin == at && !m.item.close && m.item.partner != NONE)?;
        let closer = x.markers.get(m.item.partner as usize)?;
        Some(closer.origin + closer.item.line_len - at)
    }

    /// The closer's first colon of the paired opener at `at`.
    pub(crate) fn directive_closer_at(&self, at: usize) -> Option<usize> {
        let x = &self.directives;
        let i = x.markers.partition_point(|m| m.origin < at);
        let m = x
            .markers
            .get(i)
            .filter(|m| m.origin == at && !m.item.close && m.item.partner != NONE)?;
        x.markers.origin(m.item.partner as usize)
    }

    /// `true` when the document holds at least one paired directive.
    pub fn has_directives(&self) -> bool {
        self.directives.paired > 0
    }

    /// Every directive, in order of their openers. O(markers).
    pub fn directives(&self) -> Vec<DirectiveView> {
        let n = self.directives.markers.len() as u32;
        (0..n).filter_map(|i| self.directive_view(i)).collect()
    }

    /// The directives around `pos`, outermost first; both marker lines belong to their
    /// directive. O(log markers + depth).
    pub fn directives_at(&self, pos: usize) -> Vec<DirectiveView> {
        let x = &self.directives;
        if x.paired == 0 {
            return Vec::new();
        }
        let k = x.markers.partition_point(|m| m.origin <= pos);
        let Some(k) = k.checked_sub(1) else {
            return Vec::new();
        };
        let m = x.markers.get(k).expect("marker");
        let mut cur = match (m.item.partner != NONE, m.item.close) {
            (true, false) => k as u32,
            (true, true) if pos <= m.origin + m.item.line_len => m.item.partner,
            _ => m.item.parent,
        };
        let mut out = Vec::new();
        while cur != NONE {
            out.extend(self.directive_view(cur));
            cur = x.markers.get(cur as usize).map_or(NONE, |m| m.item.parent);
        }
        out.reverse();
        out
    }

    /// Number of directives around `pos`, as [`Document::directives_at`] counts them, without
    /// reading their text. O(log markers + depth); O(1) in a document without directives.
    pub(crate) fn directive_depth_at(&self, pos: usize) -> usize {
        let x = &self.directives;
        if x.paired == 0 {
            return 0;
        }
        let k = x.markers.partition_point(|m| m.origin <= pos);
        let Some(k) = k.checked_sub(1) else { return 0 };
        let m = x.markers.get(k).expect("marker");
        let mut cur = match (m.item.partner != NONE, m.item.close) {
            (true, false) => k as u32,
            (true, true) if pos <= m.origin + m.item.line_len => m.item.partner,
            _ => m.item.parent,
        };
        let mut depth = 0;
        while cur != NONE {
            depth += 1;
            cur = x.markers.get(cur as usize).map_or(NONE, |m| m.item.parent);
        }
        depth
    }

    /// The block tree with directives as nodes: [`BlockKind::Directive`] holds the blocks
    /// between its marker lines as children (absolute spans). The stored top-level list
    /// ([`Document::tops`], [`Document::blocks`]) is flat; this view folds it, O(document).
    pub fn tree(&self) -> Vec<Block> {
        let mut root: Vec<Block> = Vec::new();
        // the open directives: the opener's block and the children so far
        let mut open: Vec<(Block, Vec<Block>)> = Vec::new();
        for t in self.tops() {
            let b = t.to_abs();
            match &b.kind {
                BlockKind::DirectiveOpen { .. } => open.push((b, Vec::new())),
                BlockKind::DirectiveClose { .. } => {
                    let Some((op, children)) = open.pop() else {
                        debug_assert!(false, "a closer is labelled only when paired");
                        continue;
                    };
                    let BlockKind::DirectiveOpen { fence_len, name, args } = op.kind else {
                        unreachable!("only openers are pushed")
                    };
                    let node = Block {
                        kind: BlockKind::Directive {
                            fence_len,
                            name,
                            args,
                            opener: op.span,
                            closer: b.span,
                        },
                        span: Span::new(op.span.start, b.span.end),
                        lines: Vec::new(),
                        children,
                        clean_line: op.clean_line,
                    };
                    open.last_mut().map_or(&mut root, |o| &mut o.1).push(node);
                }
                _ => open.last_mut().map_or(&mut root, |o| &mut o.1).push(b),
            }
        }
        debug_assert!(open.is_empty(), "an opener is labelled only when paired");
        root
    }

    /// Takes over the markers of a re-parsed region and labels the marker blocks; the blocks
    /// of the region are already in place. Widens `r` to everything whose surrounding
    /// directives changed. `old` is the replaced byte range in old coordinates.
    pub(crate) fn splice_directives(
        &mut self,
        old: Range<usize>,
        new: Vec<RawMarker>,
        rules: Vec<usize>,
        delta: isize,
        r: &mut EditResult,
    ) {
        let (label, region) = self.directives.splice(old.clone(), new, delta);
        for i in label {
            self.label_marker(i);
        }
        self.splice_sections(old, rules, delta, region.clone());
        let Some(region) = region else { return };
        let first = self.top.partition_point(|t| t.end() < region.start);
        let last = self.top.partition_point(|t| t.start() <= region.end);
        let (a, z) = (r.new_blocks.start, r.new_blocks.end);
        let (na, nz) = (a.min(first), z.max(last));
        r.replaced += (a - na) + (nz - z);
        r.new_blocks = na..nz;
        if na < a {
            r.reparsed.start = r.reparsed.start.min(self.top.get(na).map_or(0, |t| t.origin));
        }
        if nz > z {
            let end = self.top.get(nz).map_or(self.text().len(), |t| t.origin);
            r.reparsed.end = r.reparsed.end.max(end);
        }
        // the block list around the edit is the same, but not its meaning
        r.inline_leaf = None;
        r.nested_from = None;
    }

    /// Sets the kind of the top-level block of marker `i` from the marker's pair.
    fn label_marker(&mut self, i: usize) {
        let Some(m) = self.directives.markers.get(i) else {
            return;
        };
        let (at, close, paired) = (m.origin, m.item.close, m.item.partner != NONE);
        let ti = self.top.partition_point(|t| t.start() < at);
        let Some(t) = self.top.get(ti) else { return };
        debug_assert_eq!(t.start(), at, "a marker is a top-level block of its own");
        let o = t.origin;
        let text = &self.text;
        let (le, _) = line_end(text, at);
        let kind = match scan::directive_marker(text, at, le) {
            Some(l) if paired && close => BlockKind::DirectiveClose { fence_len: l.fence },
            Some(l) if paired => BlockKind::DirectiveOpen {
                fence_len: l.fence,
                name: Span::new(l.name.start - o, l.name.end - o),
                args: Span::new(l.args.start - o, l.args.end - o),
            },
            _ => BlockKind::Paragraph,
        };
        if let Some(b) = self.top.get_mut(ti) {
            b.kind = kind;
        }
    }

    /// Labels every marker block after a whole-document parse.
    pub(crate) fn label_all_markers(&mut self) {
        let n = self.directives.markers.len();
        for i in 0..n {
            if self.directives.markers.get(i).is_some_and(|m| m.item.partner != NONE) {
                self.label_marker(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(seq: &[(u32, bool)]) -> Vec<(usize, usize)> {
        let p = pair(seq);
        (0..seq.len())
            .filter(|&i| !seq[i].1 && p[i] != NONE)
            .map(|i| (i, p[i] as usize))
            .collect()
    }

    #[test]
    fn pairing_rule() {
        let (o, c) = (false, true);
        assert_eq!(pairs(&[(3, o), (3, c)]), [(0, 1)]);
        // a longer fence holds a shorter one
        assert_eq!(pairs(&[(4, o), (3, o), (3, c), (4, c)]), [(0, 3), (1, 2)]);
        // an opener of the same or a longer fence ends the waiting one
        assert_eq!(pairs(&[(3, o), (3, o), (3, c), (3, c)]), [(1, 2)]);
        assert_eq!(pairs(&[(3, o), (4, o), (4, c), (3, c)]), [(1, 2)]);
        // a longer closer closes; the outermost that fits, inner openers stay text
        assert_eq!(pairs(&[(3, o), (5, c)]), [(0, 1)]);
        assert_eq!(pairs(&[(4, o), (3, o), (4, c)]), [(0, 2)]);
        assert_eq!(pairs(&[(5, o), (3, o), (4, c), (5, c)]), [(0, 3), (1, 2)]);
        // too short a closer, a stray closer, an unclosed opener
        assert_eq!(pairs(&[(4, o), (3, c)]), []);
        assert_eq!(pairs(&[(3, c), (3, o)]), []);
    }

    #[test]
    fn depth_is_bounded() {
        let n = MAX_DIRECTIVE_DEPTH as u32 + 10;
        let mut seq: Vec<(u32, bool)> = (0..n).map(|i| (3 + n - i, false)).collect();
        seq.extend((0..n).map(|i| (3 + i + 1, true)));
        let p = pair(&seq);
        let paired = p.iter().filter(|&&x| x != NONE).count() / 2;
        assert!(paired <= MAX_DIRECTIVE_DEPTH);
    }
}
