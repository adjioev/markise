//! Markdown editing planners for M2c (ADR 0006 §5, spec 02 §2–§5, §10): inline format
//! commands, input rules, per-block keys, selection commands and block moves. Every planner
//! is pure (document in, [`markise_doc::Plan`] out); the engine applies and records it.

pub(crate) mod blocks;
pub(crate) mod inline_fmt;
pub(crate) mod review;
pub(crate) mod rules;
pub(crate) mod select;
pub(crate) mod structure;

use std::ops::Range;

use markise_doc::{FlatEvent, FormatDoc, LeafKind, Patch, PatchSet, Span};

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
        let lo = self.top_partition_point(|t| t.end() < a);
        let hi = self.top_partition_point(|t| t.start() <= b).max(lo);
        let mut out = Vec::new();
        self.flatten(lo..hi, &mut |e| {
            if let FlatEvent::Leaf(l) = e {
                let s = l.abs_span();
                if s.end >= a && s.start <= b {
                    out.push(Leaf {
                        kind: l.kind.clone(),
                        lines: l.abs_lines().collect(),
                    });
                }
            }
        });
        out
    }
}

/// `true` for ASCII space or tab.
pub(crate) fn is_sp(c: u8) -> bool {
    c == b' ' || c == b'\t'
}
