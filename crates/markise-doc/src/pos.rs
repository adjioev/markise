//! Positions, selections and live marks (ADR 0006 §2.2).

use std::cmp::Ordering;
use std::ops::Range;

use crate::patch::PatchSet;

/// Which side a position sticks to when text is inserted exactly at it (ADR 0005 §4.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Affinity {
    /// Stays before text inserted at the position.
    Upstream,
    /// Moves after text inserted at the position.
    #[default]
    Downstream,
}

/// A caret position: a byte offset into the source (on a `char` boundary), its affinity and a
/// format-neutral virtual-slot discriminator (`virt`: 0 for positions that exist in the source,
/// k > 0 for the k-th editor-only slot the format defines at that offset — missing table cells,
/// the virtual empty paragraph). Positions order by `(offset, virt)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DocPos {
    pub offset: usize,
    pub affinity: Affinity,
    pub virt: u16,
}

impl DocPos {
    pub const fn new(offset: usize) -> Self {
        DocPos {
            offset,
            affinity: Affinity::Downstream,
            virt: 0,
        }
    }
    pub const fn upstream(offset: usize) -> Self {
        DocPos {
            offset,
            affinity: Affinity::Upstream,
            virt: 0,
        }
    }
    pub const fn with_virt(offset: usize, virt: u16) -> Self {
        DocPos {
            offset,
            affinity: Affinity::Downstream,
            virt,
        }
    }
    /// Ordering key `(offset, virt)`; affinity does not order positions.
    #[inline]
    pub fn key(&self) -> (usize, u16) {
        (self.offset, self.virt)
    }
    #[inline]
    pub fn cmp_pos(&self, other: &DocPos) -> Ordering {
        self.key().cmp(&other.key())
    }

    /// Live-mark transform through an applied patch set. A virtual position whose offset a
    /// patch touches collapses to `virt = 0`; formats that define virtual slots re-resolve such
    /// marks from their [`VirtualSlot`] (`EditBehavior::resolve_virtual`, M2b/M2d).
    pub fn transform(&self, ps: &PatchSet) -> DocPos {
        let virt = if self.virt > 0 && ps.touches(self.offset) {
            0
        } else {
            self.virt
        };
        DocPos {
            offset: ps.map_offset(self.offset, self.affinity),
            affinity: self.affinity,
            virt,
        }
    }
}

/// The structural intent of a virtual position, stored with every live mark whose `virt > 0`
/// (ADR 0006 §2.2). Data only; resolution is format behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VirtualSlot {
    /// A missing (GFM-padded) table cell.
    TableCell { table_start: usize, row: u32, col: u32 },
    /// The editor-only empty paragraph at a source gap.
    EmptyParagraph { gap: usize },
}

/// One selection range: `anchor` is where it started, `focus` where the caret is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SelRange {
    pub anchor: DocPos,
    pub focus: DocPos,
}

impl SelRange {
    pub const fn caret(pos: DocPos) -> Self {
        SelRange {
            anchor: pos,
            focus: pos,
        }
    }
    pub fn is_caret(&self) -> bool {
        self.anchor.key() == self.focus.key()
    }
    pub fn start(&self) -> DocPos {
        if self.anchor.cmp_pos(&self.focus) == Ordering::Greater {
            self.focus
        } else {
            self.anchor
        }
    }
    pub fn end(&self) -> DocPos {
        if self.anchor.cmp_pos(&self.focus) == Ordering::Greater {
            self.anchor
        } else {
            self.focus
        }
    }
    /// Source byte range covered (virtual slots ignored).
    pub fn range(&self) -> Range<usize> {
        self.start().offset..self.end().offset
    }
    pub fn transform(&self, ps: &PatchSet) -> SelRange {
        SelRange {
            anchor: self.anchor.transform(ps),
            focus: self.focus.transform(ps),
        }
    }
}

/// A selection. Multi-range ready (a list of ranges plus the primary one); M2 uses exactly one
/// range (decision C8). `goal_x` is the remembered horizontal position for vertical moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    ranges: Vec<SelRange>,
    primary: usize,
    pub goal_x: Option<f32>,
}

impl Default for Selection {
    fn default() -> Self {
        Selection::caret(0)
    }
}

impl Selection {
    pub fn caret(offset: usize) -> Self {
        Self::caret_at(DocPos::new(offset))
    }
    pub fn caret_at(pos: DocPos) -> Self {
        Selection {
            ranges: vec![SelRange::caret(pos)],
            primary: 0,
            goal_x: None,
        }
    }
    /// A single range from `anchor` to `focus` (byte offsets).
    pub fn range(anchor: usize, focus: usize) -> Self {
        Self::single(DocPos::new(anchor), DocPos::new(focus))
    }
    pub fn single(anchor: DocPos, focus: DocPos) -> Self {
        Selection {
            ranges: vec![SelRange { anchor, focus }],
            primary: 0,
            goal_x: None,
        }
    }
    /// Several ranges (sorted by start); `primary` indexes into the sorted list.
    ///
    /// # Panics
    /// When `ranges` is empty or `primary` is out of range.
    pub fn from_ranges(mut ranges: Vec<SelRange>, primary: usize) -> Self {
        assert!(primary < ranges.len());
        let p = ranges[primary];
        ranges.sort_by(|a, b| a.start().cmp_pos(&b.start()));
        let primary = ranges.iter().position(|r| *r == p).unwrap_or(0);
        Selection {
            ranges,
            primary,
            goal_x: None,
        }
    }
    pub fn ranges(&self) -> &[SelRange] {
        &self.ranges
    }
    pub fn primary(&self) -> SelRange {
        self.ranges[self.primary]
    }
    pub fn is_caret(&self) -> bool {
        self.ranges.len() == 1 && self.ranges[0].is_caret()
    }
    /// Live-mark transform of every range.
    pub fn transform(&self, ps: &PatchSet) -> Selection {
        Selection {
            ranges: self.ranges.iter().map(|r| r.transform(ps)).collect(),
            primary: self.primary,
            goal_x: self.goal_x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_inside_deleted_text_collapses_by_affinity() {
        let ps = PatchSet::delete(2..8);
        let sel = Selection::single(DocPos::upstream(3), DocPos::new(6));
        let t = sel.transform(&ps);
        assert_eq!(t.primary().anchor.offset, 2);
        assert_eq!(t.primary().focus.offset, 2);
        let after = Selection::caret(10).transform(&ps);
        assert_eq!(after.primary().focus.offset, 4);
    }

    #[test]
    fn virtual_positions_collapse_when_touched() {
        let p = DocPos::with_virt(5, 2);
        assert_eq!(p.transform(&PatchSet::insert(1, "ab")), DocPos::with_virt(7, 2));
        assert_eq!(p.transform(&PatchSet::insert(5, "x")).virt, 0);
    }

    #[test]
    fn multi_range_ready() {
        let s = Selection::from_ranges(
            vec![SelRange::caret(DocPos::new(9)), SelRange::caret(DocPos::new(1))],
            0,
        );
        assert_eq!(s.ranges()[0].focus.offset, 1);
        assert_eq!(s.primary().focus.offset, 9);
    }
}
