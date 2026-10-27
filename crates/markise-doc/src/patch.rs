//! Text patches (ADR 0006 §2.1, §4.1).
//!
//! A [`PatchSet`] is a list of replacements in **pre-edit coordinates**, sorted and
//! non-overlapping. Applied to a text it yields the new text plus the verbatim bytes each patch
//! removed; [`PatchSet::inverse`] turns those removed bytes into the exact inverse patch set (in
//! post-edit coordinates). A *canonical* patch set has every insert marked `verbatim`: its bytes
//! are final (line endings already converted), so redo reproduces them exactly and mixed-ending
//! files (C23) survive undo/redo.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use crate::pos::Affinity;
use crate::text::{LineEnding, has_breaks, normalize_breaks};

/// One replacement: `range` (pre-edit byte offsets) is replaced by `insert`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Patch {
    pub range: Range<usize>,
    pub insert: String,
    /// `true`: `insert` is final bytes (canonical). `false`: newly typed text whose line breaks
    /// still have to be converted to the document's line ending (see `canonicalize`).
    pub verbatim: bool,
}

impl Patch {
    pub fn new(range: Range<usize>, insert: impl Into<String>) -> Self {
        Patch {
            range,
            insert: insert.into(),
            verbatim: false,
        }
    }
    pub fn verbatim(range: Range<usize>, insert: impl Into<String>) -> Self {
        Patch {
            range,
            insert: insert.into(),
            verbatim: true,
        }
    }
    #[inline]
    pub fn is_insertion(&self) -> bool {
        self.range.is_empty()
    }
    #[inline]
    pub fn is_noop(&self) -> bool {
        self.range.is_empty() && self.insert.is_empty()
    }
    #[inline]
    pub fn delta(&self) -> isize {
        self.insert.len() as isize - self.range.len() as isize
    }
}

impl fmt::Debug for Patch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}..{}{}{:?}",
            self.range.start,
            self.range.end,
            if self.verbatim { "=" } else { "~" },
            self.insert
        )
    }
}

/// Why a patch set was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchError {
    /// Patch `index` overlaps (or is an insertion at the same offset as) its predecessor.
    Overlap { index: usize },
    /// Patch `index` has `start > end` or ends past the text.
    OutOfBounds { index: usize },
    /// Patch `index` starts or ends inside a UTF-8 sequence.
    NotCharBoundary { index: usize, offset: usize },
    /// The removed-bytes list does not match the patch set (inverse construction).
    RemovedMismatch,
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatchError::Overlap { index } => write!(f, "patch {index} overlaps its predecessor"),
            PatchError::OutOfBounds { index } => write!(f, "patch {index} is out of bounds"),
            PatchError::NotCharBoundary { index, offset } => {
                write!(f, "patch {index}: offset {offset} is not a char boundary")
            }
            PatchError::RemovedMismatch => write!(f, "removed bytes do not match the patch set"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Sorted, non-overlapping patches in pre-edit coordinates. No-op patches are dropped.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct PatchSet {
    patches: Vec<Patch>,
}

impl fmt::Debug for PatchSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.patches).finish()
    }
}

impl PatchSet {
    pub const fn empty() -> Self {
        PatchSet { patches: Vec::new() }
    }

    /// Builds a set from patches in any order. Fails when two patches overlap or two
    /// insertions share an offset (their order would be ambiguous).
    pub fn new(mut patches: Vec<Patch>) -> Result<Self, PatchError> {
        patches.retain(|p| !p.is_noop());
        for (i, p) in patches.iter().enumerate() {
            if p.range.start > p.range.end {
                return Err(PatchError::OutOfBounds { index: i });
            }
        }
        patches.sort_by_key(|p| (p.range.start, p.range.end));
        for i in 1..patches.len() {
            let (a, b) = (&patches[i - 1], &patches[i]);
            let same_point = a.is_insertion() && b.is_insertion() && a.range.start == b.range.start;
            if a.range.end > b.range.start || same_point {
                return Err(PatchError::Overlap { index: i });
            }
        }
        Ok(PatchSet { patches })
    }

    /// One replacement (newly typed text: not verbatim).
    pub fn single(range: Range<usize>, insert: impl Into<String>) -> Self {
        let p = Patch::new(range, insert);
        PatchSet {
            patches: if p.is_noop() { Vec::new() } else { vec![p] },
        }
    }

    /// One replacement whose bytes are final.
    pub fn single_verbatim(range: Range<usize>, insert: impl Into<String>) -> Self {
        let p = Patch::verbatim(range, insert);
        PatchSet {
            patches: if p.is_noop() { Vec::new() } else { vec![p] },
        }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::single(at..at, text)
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self::single_verbatim(range, "")
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.patches.len()
    }
    #[inline]
    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }
    /// Consumes the set, returning its patches (rebuild with [`PatchSet::new`], which
    /// re-checks the invariants).
    pub fn into_patches(self) -> Vec<Patch> {
        self.patches
    }

    /// Net length change.
    pub fn delta(&self) -> isize {
        self.patches.iter().map(Patch::delta).sum()
    }

    /// Smallest pre-edit range covering every patch.
    pub fn covering_range(&self) -> Option<Range<usize>> {
        Some(self.patches.first()?.range.start..self.patches.last()?.range.end)
    }

    /// Checks bounds and `char` boundaries against `text` (the pre-edit text).
    pub fn validate(&self, text: &str) -> Result<(), PatchError> {
        for (i, p) in self.patches.iter().enumerate() {
            if p.range.start > p.range.end || p.range.end > text.len() {
                return Err(PatchError::OutOfBounds { index: i });
            }
            for off in [p.range.start, p.range.end] {
                if !text.is_char_boundary(off) {
                    return Err(PatchError::NotCharBoundary { index: i, offset: off });
                }
            }
        }
        Ok(())
    }

    /// The bytes each patch would remove from `text` (validated set).
    pub fn removed_from(&self, text: &str) -> Vec<Arc<str>> {
        self.patches.iter().map(|p| Arc::from(&text[p.range.clone()])).collect()
    }

    /// Applies the set to `text` in place and returns the removed bytes per patch.
    ///
    /// # Panics
    /// When the set is not valid for `text` (see [`PatchSet::validate`]).
    pub fn apply(&self, text: &mut String) -> Vec<Arc<str>> {
        let removed = self.removed_from(text);
        match self.patches.as_slice() {
            [] => {}
            [p] => text.replace_range(p.range.clone(), &p.insert),
            _ => *text = self.apply_to(text),
        }
        removed
    }

    /// The text after applying the set (one pass).
    ///
    /// # Panics
    /// When the set is not valid for `text`.
    pub fn apply_to(&self, text: &str) -> String {
        let mut out = String::with_capacity((text.len() as isize + self.delta()).max(0) as usize);
        let mut pos = 0;
        for p in &self.patches {
            out.push_str(&text[pos..p.range.start]);
            out.push_str(&p.insert);
            pos = p.range.end;
        }
        out.push_str(&text[pos..]);
        out
    }

    /// The inverse set in post-edit coordinates: each inserted range is replaced by the
    /// verbatim bytes the patch removed.
    pub fn inverse(&self, removed: &[Arc<str>]) -> Result<PatchSet, PatchError> {
        if removed.len() != self.patches.len() {
            return Err(PatchError::RemovedMismatch);
        }
        let mut delta: isize = 0;
        let mut out: Vec<Patch> = Vec::with_capacity(self.patches.len());
        for (p, r) in self.patches.iter().zip(removed) {
            if r.len() != p.range.len() {
                return Err(PatchError::RemovedMismatch);
            }
            let start = (p.range.start as isize + delta) as usize;
            let range = start..start + p.insert.len();
            // Adjacent deletions invert to insertions at one offset: merge them (in order) so
            // the inverse keeps the "one insertion per offset" invariant.
            match out.last_mut() {
                Some(last) if last.range.is_empty() && range.is_empty() && last.range.start == start => {
                    last.insert.push_str(r);
                }
                _ => out.push(Patch::verbatim(range, r.to_string())),
            }
            delta += p.delta();
        }
        out.retain(|p| !p.is_noop());
        Ok(PatchSet { patches: out })
    }

    /// Converts the line breaks of every non-verbatim insert to `eol` and marks all patches
    /// verbatim (the canonical forward patch of ADR 0006 §2.1).
    pub fn canonical_eol(&self, eol: LineEnding) -> PatchSet {
        self.canonical_with(|s| normalize_breaks(s, eol), has_breaks)
    }

    /// Like [`PatchSet::canonical_eol`] with a format-supplied break conversion.
    pub fn canonical_with(&self, convert: impl Fn(&str) -> String, needs: impl Fn(&str) -> bool) -> PatchSet {
        PatchSet {
            patches: self
                .patches
                .iter()
                .map(|p| {
                    let insert = if !p.verbatim && needs(&p.insert) {
                        convert(&p.insert)
                    } else {
                        p.insert.clone()
                    };
                    Patch::verbatim(p.range.clone(), insert)
                })
                .collect(),
        }
    }

    /// Maps a pre-edit offset to post-edit coordinates (live-mark transform, ADR 0006 §2.2).
    ///
    /// Patches strictly before `pos` shift it. At a shared boundary every patch touching `pos`
    /// is considered (the whole cluster, in order): a replacement **ending** at `pos` lies
    /// before it (`pos` goes after its insert); an **insertion** at `pos` is passed by a
    /// `Downstream` position and not by an `Upstream` one; a replacement **starting** at `pos`
    /// lies after it. Strictly inside a replaced range: `Upstream` → patch start,
    /// `Downstream` → end of the insert.
    pub fn map_offset(&self, pos: usize, affinity: Affinity) -> usize {
        let mut delta: isize = 0;
        let sh = |p: usize, d: isize| (p as isize + d) as usize;
        for p in &self.patches {
            let (s, e) = (p.range.start, p.range.end);
            let ins = p.insert.len() as isize;
            if s > pos {
                break;
            }
            if s == e {
                // insertion at or before pos
                if s < pos || affinity == Affinity::Downstream {
                    delta += ins;
                }
                continue;
            }
            if s == pos {
                break; // replacement starting at pos lies after it
            }
            if pos < e {
                return match affinity {
                    Affinity::Upstream => sh(s, delta),
                    Affinity::Downstream => sh(s, delta + ins),
                };
            }
            // e <= pos: replacement before (or ending at) pos
            delta += p.delta();
        }
        sh(pos, delta)
    }

    /// `true` when `pos` lies strictly inside a replaced (non-empty) range.
    pub fn swallows(&self, pos: usize) -> bool {
        self.patches.iter().any(|p| p.range.start < pos && pos < p.range.end)
    }

    /// `true` when a patch touches `pos` (inside or at either end of its range).
    pub fn touches(&self, pos: usize) -> bool {
        self.patches.iter().any(|p| p.range.start <= pos && pos <= p.range.end)
    }

    /// Heap bytes retained by the set (history budget accounting): insert capacities and the
    /// patch vector.
    pub fn byte_cost(&self) -> usize {
        self.patches
            .iter()
            .map(|p| p.insert.capacity() + std::mem::size_of::<Patch>())
            .sum::<usize>()
            + (self.patches.capacity() - self.patches.len()) * std::mem::size_of::<Patch>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: Vec<(Range<usize>, &str)>) -> PatchSet {
        PatchSet::new(v.into_iter().map(|(r, s)| Patch::verbatim(r, s)).collect()).unwrap()
    }

    #[test]
    fn apply_and_inverse_roundtrip() {
        let mut t = String::from("hello wide world");
        let ps = set(vec![(0..5, "HELLO"), (6..10, ""), (16..16, "!\r\n")]);
        ps.validate(&t).unwrap();
        let orig = t.clone();
        let removed = ps.apply(&mut t);
        assert_eq!(t, "HELLO  world!\r\n");
        let inv = ps.inverse(&removed).unwrap();
        inv.validate(&t).unwrap();
        inv.apply(&mut t);
        assert_eq!(t, orig);
    }

    #[test]
    fn rejects_overlaps_and_ambiguous_insertions() {
        assert!(PatchSet::new(vec![Patch::new(0..3, "a"), Patch::new(2..4, "b")]).is_err());
        assert!(PatchSet::new(vec![Patch::new(2..2, "a"), Patch::new(2..2, "b")]).is_err());
        // insertion then replacement at the same offset is fine and ordered
        let ps = PatchSet::new(vec![Patch::new(2..4, "R"), Patch::new(2..2, "I")]).unwrap();
        assert_eq!(ps.apply_to("abcdef"), "abIRef");
    }

    #[test]
    fn validates_char_boundaries() {
        let ps = PatchSet::single(1..2, "x");
        assert!(matches!(ps.validate("é"), Err(PatchError::NotCharBoundary { .. })));
        assert!(matches!(
            PatchSet::single(0..9, "").validate("ab"),
            Err(PatchError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn canonical_converts_only_typed_text() {
        let ps = PatchSet::new(vec![Patch::new(0..0, "a\nb"), Patch::verbatim(1..1, "c\nd")]).unwrap();
        let c = ps.canonical_eol(LineEnding::CrLf);
        assert_eq!(c.patches()[0].insert, "a\r\nb");
        assert_eq!(c.patches()[1].insert, "c\nd");
        assert!(c.patches().iter().all(|p| p.verbatim));
    }

    #[test]
    fn shared_boundary_cluster_is_mapped_whole() {
        use Affinity::*;
        // "ab": delete(0..1) + insert(1, "é") → "éb"
        let ps = set(vec![(0..1, ""), (1..1, "é")]);
        assert_eq!(ps.apply_to("ab"), "éb");
        assert_eq!(ps.map_offset(1, Downstream), 2, "after the inserted é");
        assert_eq!(ps.map_offset(1, Upstream), 0, "before it");
        assert_eq!(ps.map_offset(2, Upstream), 3);
        // replacement ending at pos, insertion at pos, replacement starting at pos
        let ps = set(vec![(0..2, "日"), (2..2, "é"), (2..3, "XY")]);
        assert_eq!(ps.apply_to("abcd"), "日éXYd");
        assert_eq!(ps.map_offset(2, Upstream), 3, "after 日, before é");
        assert_eq!(ps.map_offset(2, Downstream), 5, "after é, before XY");
        assert_eq!(ps.map_offset(3, Downstream), 7, "end of the replaced c");
        assert_eq!(ps.map_offset(4, Downstream), 8);
    }

    #[test]
    fn inverse_of_adjacent_deletions_merges_insertions() {
        use Affinity::*;
        let mut t = String::from("abcd");
        let ps = set(vec![(0..1, ""), (1..2, ""), (3..4, "é")]);
        let removed = ps.apply(&mut t);
        assert_eq!(t, "cé");
        let inv = ps.inverse(&removed).unwrap();
        assert_eq!(inv.len(), 2, "two coincident insertions merged: {inv:?}");
        assert!(
            PatchSet::new(inv.patches().to_vec()).is_ok(),
            "inverse satisfies the invariants"
        );
        assert_eq!(inv.map_offset(0, Downstream), 2, "after both restored bytes");
        assert_eq!(inv.map_offset(0, Upstream), 0);
        assert_eq!(inv.map_offset(3, Upstream), 4, "end of the replaced é");
        inv.apply(&mut t);
        assert_eq!(t, "abcd");
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)]
    fn reversed_ranges_are_rejected() {
        assert!(matches!(
            PatchSet::single(2..1, "x").validate("abc"),
            Err(PatchError::OutOfBounds { .. })
        ));
        assert!(PatchSet::new(vec![Patch::new(2..1, "x")]).is_err());
    }

    #[test]
    fn map_offset_rules() {
        let ps = set(vec![(2..2, "XX"), (4..6, "Y")]);
        use Affinity::*;
        assert_eq!(ps.map_offset(1, Upstream), 1);
        assert_eq!(ps.map_offset(2, Upstream), 2);
        assert_eq!(ps.map_offset(2, Downstream), 4);
        assert_eq!(ps.map_offset(3, Downstream), 5);
        assert_eq!(
            ps.map_offset(4, Downstream),
            6,
            "start of a replaced range stays before it"
        );
        assert_eq!(ps.map_offset(5, Upstream), 6);
        assert_eq!(ps.map_offset(5, Downstream), 7);
        assert_eq!(
            ps.map_offset(6, Upstream),
            7,
            "end of a replaced range goes after the insert"
        );
        assert_eq!(ps.map_offset(9, Upstream), 10);
    }
}
