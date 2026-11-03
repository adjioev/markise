//! Markdown editing planners for M2c (ADR 0006 §5, spec 02 §2–§5, §10): inline format
//! commands, input rules, per-block keys, selection commands and block moves. Every planner
//! is pure (document in, [`markise_doc::Plan`] out); the engine applies and records it.

pub(crate) mod blocks;
pub(crate) mod gap;
pub(crate) mod inline_fmt;
pub(crate) mod review;
pub(crate) mod rules;
pub(crate) mod select;
pub(crate) mod structure;
pub(crate) mod tables;

use std::ops::Range;

use markise_doc::{LeafKind, Patch, PatchSet, Span};

use crate::Document;

/// Collects edits in pre-edit coordinates and builds a [`PatchSet`]. Insertions at the same
/// offset are concatenated in `rank` order (then push order); a deletion and an insertion at
/// the same point stay separate patches.
#[derive(Default, Debug)]
pub(crate) struct Edits {
    items: Vec<(Range<usize>, i32, usize, String)>,
}

impl Edits {
    pub(crate) fn new() -> Self {
        Edits::default()
    }
    pub(crate) fn push(&mut self, range: Range<usize>, text: impl Into<String>) {
        self.push_ranked(range, 0, text);
    }
    pub(crate) fn insert(&mut self, at: usize, text: impl Into<String>) {
        self.push(at..at, text);
    }
    pub(crate) fn delete(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.push(range, "");
        }
    }
    pub(crate) fn push_ranked(&mut self, range: Range<usize>, rank: i32, text: impl Into<String>) {
        let n = self.items.len();
        self.items.push((range, rank, n, text.into()));
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// The patch set (`None` when edits overlap).
    pub(crate) fn build(mut self) -> Option<PatchSet> {
        self.items
            .sort_by_key(|(r, rank, n, _)| (r.start, !r.is_empty(), r.end, *rank, *n));
        let mut out: Vec<Patch> = Vec::new();
        for (r, _, _, t) in self.items {
            if let Some(last) = out.last_mut()
                && last.range.is_empty()
                && r.is_empty()
                && last.range.start == r.start
            {
                last.insert.push_str(&t);
                continue;
            }
            if r.is_empty() && t.is_empty() {
                continue;
            }
            out.push(Patch::verbatim(r, t));
        }
        PatchSet::new(out).ok()
    }
}

/// A text leaf (or any leaf) inside a range, with absolute spans.
#[derive(Clone, Debug)]
pub(crate) struct Leaf {
    pub kind: LeafKind,
    pub lines: Vec<Span>,
}

impl Document {
    /// The leaves overlapping `a..=b` (absolute), in document order.
    pub(crate) fn leaves_in(&self, a: usize, b: usize) -> Vec<Leaf> {
        let mut out = Vec::new();
        self.walk_leaves_in(a, b, &mut |l| {
            out.push(l);
            true
        });
        out
    }

    /// Calls `f` with the leaves overlapping `a..=b` in document order until it returns
    /// `false`. Containers are entered only where they overlap the range (children found by
    /// binary search) and the walk stops as soon as `f` declines, inside containers too: a
    /// query that needs only the first leaf of a huge range — or of a huge list or quote —
    /// costs that leaf (issue #133).
    pub(crate) fn walk_leaves_in(&self, a: usize, b: usize, f: &mut dyn FnMut(Leaf) -> bool) {
        let lo = self.top_partition_point(|t| t.end() < a);
        let hi = self.top_partition_point(|t| t.start() <= b).max(lo);
        for i in lo..hi {
            let Some(t) = self.top(i) else { break };
            if !self.walk_block(t.block, t.origin, a, b, f) {
                break;
            }
        }
    }

    /// [`Document::walk_leaves_in`] below one block (spans relative to `origin`); `false`
    /// when `f` stopped the walk.
    fn walk_block(
        &self,
        blk: &crate::Block,
        origin: usize,
        a: usize,
        b: usize,
        f: &mut dyn FnMut(Leaf) -> bool,
    ) -> bool {
        #[cfg(test)]
        tests::VISITS.with(|v| v.set(v.get() + 1));
        if blk.span.end + origin < a || blk.span.start + origin > b {
            return true;
        }
        if !blk.is_container() {
            return f(Leaf {
                kind: crate::format::leaf_kind_at(self.text(), blk, origin),
                lines: blk
                    .lines
                    .iter()
                    .map(|l| Span::new(l.start + origin, l.end + origin))
                    .collect(),
            });
        }
        let first = blk.children.partition_point(|c| c.span.end + origin < a);
        for c in &blk.children[first..] {
            if c.span.start + origin > b {
                break;
            }
            if !self.walk_block(c, origin, a, b, f) {
                return false;
            }
        }
        true
    }
}

/// `true` for ASCII space or tab.
pub(crate) fn is_sp(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use markise_doc::{EditBehavior, PlanRequest, Selection, planners};

    thread_local! {
        /// Blocks visited by `walk_block` on this thread.
        pub(super) static VISITS: Cell<usize> = const { Cell::new(0) };
        /// Text leaves inline-parsed by the format commands on this thread.
        pub(crate) static PARSES: Cell<usize> = const { Cell::new(0) };
    }

    fn parses(f: impl FnOnce()) -> usize {
        PARSES.with(|v| v.set(0));
        f();
        PARSES.with(|v| v.get())
    }

    /// Issue #133: enablement and check state over a huge paragraph never inline-parse it;
    /// Hyperlink parses only its (bounded) first block, and refuses one over the limit.
    #[test]
    fn large_selection_queries_do_not_parse_huge_paragraphs() {
        use super::inline_fmt::FORMAT_SELECTION_LIMIT;
        let ids = [
            planners::STRONG,
            planners::EMPH,
            planners::CODE,
            planners::COMMENT,
            planners::LINK,
            planners::IMAGE,
            planners::CLEAR_FORMAT,
        ];
        for (size, fits) in [(1 << 20, true), (FORMAT_SELECTION_LIMIT + 64, false)] {
            let mut text = String::new();
            while text.len() < size {
                text.push_str("some **words** and more words\n");
            }
            let d = crate::parse(&text);
            let req = PlanRequest::at(Selection::range(0, text.len()));
            for id in ids {
                let n = parses(|| {
                    assert_eq!(d.can_plan(id, &req), fits, "{id:?}");
                    assert!(!d.plan_checked(id, &req));
                });
                assert_eq!(n, 0, "{id:?} parsed {n} blocks");
            }
            let n = parses(|| assert_eq!(d.plan(planners::LINK, &req).is_ok(), fits));
            assert_eq!(n, usize::from(fits));
            let n = parses(|| assert_eq!(d.plan(planners::STRONG, &req).is_ok(), fits));
            assert_eq!(n, usize::from(fits));
        }
    }

    fn visits(f: impl FnOnce()) -> usize {
        VISITS.with(|v| v.set(0));
        f();
        VISITS.with(|v| v.get())
    }

    /// Issue #133: a query that needs only the first leaf stops there, also inside one huge
    /// list or quote.
    #[test]
    fn walking_leaves_stops_inside_containers() {
        let list: String = (0..20_000).map(|i| format!("- item {i}\n")).collect();
        let quote: String = (0..20_000).map(|i| format!("> line {i}\n>\n")).collect();
        let nested: String = (0..20_000).map(|i| format!("> - item {i}\n")).collect();
        for text in [list, quote, nested] {
            let d = crate::parse(&text);
            let n = text.len();
            let mut leaves = 0;
            let v = visits(|| {
                d.walk_leaves_in(0, n, &mut |_| {
                    leaves += 1;
                    false
                })
            });
            assert_eq!(leaves, 1);
            assert!(v < 10, "visited {v} blocks for the first leaf");
            // the full walk still sees every leaf, in order, as before
            assert_eq!(d.leaves_in(0, n).len(), 20_000);
            // starting in the middle: children are found by binary search
            let v = visits(|| d.walk_leaves_in(n / 2, n, &mut |_| false));
            assert!(v < 10, "visited {v} blocks from the middle");
            // enablement after Select All (above the exact limit) stays bounded
            let req = PlanRequest::at(Selection::range(0, n));
            for id in [planners::STRONG, planners::LINK, planners::CLEAR_FORMAT] {
                let v = visits(|| assert!(d.can_plan(id, &req), "{id:?}"));
                assert!(v < 10, "{id:?} visited {v} blocks");
            }
        }
    }
}
