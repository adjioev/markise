//! Storage of the top-level blocks with **relative offsets** (ADR 0006 §7.6, gate G1 remedy).
//!
//! An edit near the start of a large document must not touch every following block. Each
//! top-level block is therefore stored with its spans relative to its own *origin* (its first
//! line start when it is a clean synchronisation block, else its first byte), and origins are
//! stored relative to the base of a chunk of at most 256 blocks (`markise_doc::chunked`). Shifting the tail of the
//! document after an edit adds the delta to the origins after the edit inside one chunk and to
//! the bases of the later chunks — O(`CHUNK` + blocks / `CHUNK`) integer additions, independent
//! of the size of the blocks. Absolute offsets are computed on demand ([`Top`]).

use std::ops::Range;

use markise_doc::chunked::{RelChunks, Relative};

use crate::block::Block;
use crate::text::Span;

#[cfg(test)]
use markise_doc::chunked::CHUNK;

impl Relative for Block {
    /// The clean line start when it precedes the block's first byte, else the first byte.
    fn origin(&self) -> usize {
        self.clean_line.map_or(self.span.start, |l| l.min(self.span.start))
    }
    fn shift(&mut self, delta: isize) {
        Block::shift(self, delta);
    }
}

/// The top-level blocks of a document (see the module docs).
#[derive(Clone, Debug, Default)]
pub(crate) struct TopBlocks(RelChunks<Block>);

/// A top-level block with its absolute origin. `block`'s spans are relative to `origin`.
#[derive(Clone, Copy, Debug)]
pub struct Top<'a> {
    pub origin: usize,
    pub block: &'a Block,
}

impl Top<'_> {
    /// Absolute span of the block.
    #[inline]
    pub fn span(&self) -> Span {
        Span::new(self.origin + self.block.span.start, self.origin + self.block.span.end)
    }
    #[inline]
    pub fn start(&self) -> usize {
        self.origin + self.block.span.start
    }
    #[inline]
    pub fn end(&self) -> usize {
        self.origin + self.block.span.end
    }
    /// Absolute line start of a clean (synchronisation) block.
    #[inline]
    pub fn clean_line(&self) -> Option<usize> {
        self.block.clean_line.map(|l| self.origin + l)
    }
    /// An owned copy with absolute spans.
    pub fn to_abs(&self) -> Block {
        let mut b = self.block.clone();
        b.shift(self.origin as isize);
        b
    }
    /// Absolute form of a span of this block (e.g. one of `block.lines`).
    #[inline]
    pub fn abs(&self, s: Span) -> Span {
        Span::new(self.origin + s.start, self.origin + s.end)
    }
}

fn top(r: markise_doc::chunked::Rel<'_, Block>) -> Top<'_> {
    Top {
        origin: r.origin,
        block: r.item,
    }
}

impl TopBlocks {
    pub(crate) fn from_abs(blocks: Vec<Block>) -> Self {
        TopBlocks(RelChunks::from_abs_vec(blocks))
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn get(&self, i: usize) -> Option<Top<'_>> {
        self.0.get(i).map(top)
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = Top<'_>> + '_ {
        self.0.iter().map(top)
    }

    pub(crate) fn range(&self, r: Range<usize>) -> impl Iterator<Item = Top<'_>> + '_ {
        self.0.range(r).map(top)
    }

    /// Index of the first block for which `pred` is false (`pred` must be monotone).
    pub(crate) fn partition_point(&self, mut pred: impl FnMut(&Top<'_>) -> bool) -> usize {
        self.0.partition_point(|r| pred(&top(*r)))
    }

    /// Like [`TopBlocks::splice`]; `new` holds `(absolute origin, block relative to it)`.
    pub(crate) fn splice_entries(&mut self, range: Range<usize>, new: Vec<(usize, Block)>, delta: isize) {
        self.0.splice_entries(range, new, delta);
    }

    /// Block `i` with spans relative to its origin, mutable (the origin cannot change).
    pub(crate) fn get_mut(&mut self, i: usize) -> Option<&mut Block> {
        self.0.get_mut(i)
    }

    /// Shifts every block after `i` by `delta` (block `i` changed length in place).
    pub(crate) fn shift_after(&mut self, i: usize, delta: isize) {
        self.0.shift_after(i, delta);
    }

    /// All blocks as owned absolute copies (O(document)).
    pub(crate) fn to_abs_vec(&self) -> Vec<Block> {
        self.0.to_abs_vec()
    }

    /// Replaces the blocks `range` with `new` (absolute spans in post-edit coordinates) and
    /// shifts every block after the range by `delta`.
    pub(crate) fn splice(&mut self, range: Range<usize>, new: Vec<Block>, delta: isize) {
        self.0.splice(range, new, delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockKind;
    use crate::text::Span;

    fn blk(start: usize, end: usize) -> Block {
        let mut b = Block::new(BlockKind::Paragraph, Span::new(start, end));
        b.lines.push(Span::new(start, end));
        b.clean_line = Some(start);
        b
    }

    fn spans(t: &TopBlocks) -> Vec<(usize, usize)> {
        t.iter().map(|x| (x.start(), x.end())).collect()
    }

    #[test]
    fn splice_shifts_tail_across_chunks() {
        let n = CHUNK * 3 + 7;
        let mut t = TopBlocks::from_abs((0..n).map(|i| blk(i * 10, i * 10 + 5)).collect());
        assert_eq!(t.len(), n);
        // replace block 3 with two blocks, text grew by 4
        t.splice(3..4, vec![blk(30, 32), blk(33, 39)], 4);
        assert_eq!(t.len(), n + 1);
        let s = spans(&t);
        assert_eq!(s[3], (30, 32));
        assert_eq!(s[4], (33, 39));
        assert_eq!(s[5], (44, 49));
        assert_eq!(*s.last().unwrap(), ((n - 1) * 10 + 4, (n - 1) * 10 + 9));
        // lines shift with their block
        let last = t.get(t.len() - 1).unwrap();
        assert_eq!(last.abs(last.block.lines[0]), last.span());
        assert_eq!(last.clean_line(), Some(last.start()));
        // index lookups
        for (i, sp) in s.iter().enumerate() {
            assert_eq!(t.get(i).unwrap().start(), sp.0);
        }
        let p = t.partition_point(|x| x.start() <= 2000);
        assert_eq!(p, s.iter().filter(|x| x.0 <= 2000).count());
        // delete a range spanning chunks
        t.splice(10..(CHUNK * 2), Vec::new(), -100);
        assert_eq!(t.len(), n + 1 - (CHUNK * 2 - 10));
        let s2 = spans(&t);
        assert_eq!(s2[10].0, s[CHUNK * 2].0 - 100);
        assert_eq!(t.to_abs_vec().len(), t.len());
        // insert at the end and at the start
        let end = s2.last().unwrap().1;
        t.splice(t.len()..t.len(), vec![blk(end + 2, end + 3)], 0);
        assert_eq!(spans(&t).last().unwrap().0, end + 2);
        t.splice(0..0, vec![blk(0, 1)], 2);
        assert_eq!(spans(&t)[0], (0, 1));
        assert_eq!(spans(&t)[1], (2, 7));
    }

    #[test]
    fn empty_and_singletons() {
        let mut t = TopBlocks::from_abs(Vec::new());
        assert_eq!(t.len(), 0);
        assert!(t.get(0).is_none());
        assert_eq!(t.partition_point(|_| true), 0);
        t.splice(0..0, vec![blk(0, 3)], 3);
        assert_eq!(spans(&t), vec![(0, 3)]);
        t.splice(0..1, Vec::new(), -3);
        assert_eq!(t.len(), 0);
    }
}
