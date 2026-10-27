//! A sequence of position-carrying elements stored with **relative offsets** (ADR 0006 §7.6,
//! gate G1 remedy: "top-level blocks and items in chunks of ≤ 256 with lazy base deltas").
//!
//! Each element is stored relative to its own *origin* ([`Relative::origin`]); origins are
//! stored relative to the base of a chunk of at most [`CHUNK`] elements. Replacing a range and
//! shifting everything after it by the edit's length delta ([`RelChunks::splice`]) rebuilds the
//! touched chunks and adds the delta to the bases of the later chunks: O(`CHUNK` + replaced +
//! len / `CHUNK`), independent of how many offsets the following elements hold. Absolute
//! positions are computed on demand ([`Rel`]).
//!
//! Used by `markise-md` for its top-level blocks and by `markise-layout` for its items.

use std::ops::Range;

use crate::text::Span;

/// Maximum number of elements per chunk.
pub const CHUNK: usize = 256;

/// An element with byte offsets.
pub trait Relative {
    /// The element's origin in its current coordinates (absolute before insertion). Every
    /// offset of the element must be `>=` its origin.
    fn origin(&self) -> usize;
    /// Adds `delta` to every offset of the element.
    fn shift(&mut self, delta: isize);
}

#[derive(Clone, Debug)]
struct Chunk<T> {
    /// Absolute origin of the chunk's first element.
    base: usize,
    /// Origin of each element relative to `base` (ascending; the first is 0).
    rel: Vec<usize>,
    /// Elements with every offset relative to their own origin.
    items: Vec<T>,
}

/// See the module docs.
#[derive(Clone, Debug)]
pub struct RelChunks<T> {
    chunks: Vec<Chunk<T>>,
    /// Global index of the first element of each chunk.
    firsts: Vec<usize>,
    len: usize,
}

impl<T> Default for RelChunks<T> {
    fn default() -> Self {
        RelChunks {
            chunks: Vec::new(),
            firsts: Vec::new(),
            len: 0,
        }
    }
}

/// An element with its absolute origin: absolute offset = `origin` + relative offset.
#[derive(Debug)]
pub struct Rel<'a, T> {
    pub origin: usize,
    pub item: &'a T,
}

impl<T> Clone for Rel<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Rel<'_, T> {}

impl<T: Clone + Relative> Rel<'_, T> {
    /// An owned copy with absolute offsets.
    pub fn to_abs(&self) -> T {
        let mut t = self.item.clone();
        t.shift(self.origin as isize);
        t
    }
}

impl<T> Rel<'_, T> {
    /// Absolute form of a span of this element.
    #[inline]
    pub fn abs(&self, s: Span) -> Span {
        Span::new(self.origin + s.start, self.origin + s.end)
    }
}

impl<T: Relative> RelChunks<T> {
    /// From elements with absolute offsets, in ascending origin order.
    pub fn from_abs(items: Vec<T>) -> Self {
        let n = items.len();
        let mut t = RelChunks {
            chunks: chunk_entries(items.into_iter().map(to_rel), n),
            ..Default::default()
        };
        t.reindex(0);
        t
    }

    /// Appends elements with absolute offsets (ascending, after every element held) without
    /// building an intermediate list: chunks fill up to [`CHUNK`].
    pub fn extend_abs(&mut self, items: impl IntoIterator<Item = T>) {
        let from = self.chunks.len().saturating_sub(1);
        for t in items {
            let (o, b) = to_rel(t);
            match self.chunks.last_mut() {
                Some(c) if c.items.len() < CHUNK => {
                    c.rel.push(o - c.base);
                    c.items.push(b);
                }
                _ => {
                    let mut rel = Vec::with_capacity(CHUNK);
                    rel.push(0);
                    let mut items = Vec::with_capacity(CHUNK);
                    items.push(b);
                    self.chunks.push(Chunk { base: o, rel, items });
                }
            }
        }
        self.reindex(from.min(self.chunks.len()));
    }

    /// From a vector of absolute elements, consumed from the back so that its memory is
    /// released while the chunks are built (no full-size intermediate copy).
    pub fn from_abs_vec(mut v: Vec<T>) -> Self {
        let mut rev: Vec<Chunk<T>> = Vec::with_capacity(v.len().div_ceil(CHUNK));
        while !v.is_empty() {
            let k = v.len().saturating_sub(CHUNK);
            let n = v.len() - k;
            rev.extend(chunk_entries(v.drain(k..).map(to_rel), n));
            if v.capacity() > 4 * CHUNK && v.len() < v.capacity() / 2 {
                v.shrink_to_fit();
            }
        }
        rev.reverse();
        let mut t = RelChunks {
            chunks: rev,
            ..Default::default()
        };
        t.reindex(0);
        t
    }

    /// Replaces the elements `range` with `new` (absolute offsets in post-edit coordinates)
    /// and shifts every element after the range by `delta`.
    ///
    /// # Panics
    /// When `range` is out of bounds.
    pub fn splice(&mut self, range: Range<usize>, new: Vec<T>, delta: isize) {
        self.splice_entries(range, new.into_iter().map(to_rel).collect(), delta);
    }

    /// Like [`RelChunks::splice`] with the new elements given as `(absolute origin, element
    /// relative to it)` (an element already in relative form is not shifted twice).
    pub fn splice_entries(&mut self, range: Range<usize>, new: Vec<(usize, T)>, delta: isize) {
        assert!(
            range.start <= range.end && range.end <= self.len,
            "splice out of bounds"
        );
        if self.chunks.is_empty() {
            let n = new.len();
            self.chunks = chunk_entries(new.into_iter(), n);
            self.reindex(0);
            return;
        }
        // the group of chunks to rebuild: from the chunk holding `range.start` to the chunk
        // holding the last replaced element (or the element after the range for an insertion)
        let last = self.chunks.len() - 1;
        let pos_chunk = |i: usize| -> usize { if i >= self.len { last } else { self.locate(i).0 } };
        let ka = pos_chunk(range.start);
        let mut kb = pos_chunk(range.end.saturating_sub(1).max(range.start)).max(ka);
        // absorb a neighbour when the rebuilt group would be small (no tiny chunks)
        let group: usize = (ka..=kb).map(|k| self.chunks[k].items.len()).sum();
        if group + new.len() - range.len() < CHUNK / 4 && kb < last {
            kb += 1;
        }
        let first = self.firsts[ka];
        let (lo, hi) = (range.start - first, range.end - first);
        let old: Vec<Chunk<T>> = self.chunks.drain(ka..=kb).collect();
        let total = old.iter().map(|c| c.items.len()).sum::<usize>() - range.len() + new.len();
        let mut entries: Vec<(usize, T)> = Vec::with_capacity(total);
        let mut idx = 0usize;
        let mut new = Some(new);
        for c in old {
            for (r, b) in c.rel.into_iter().zip(c.items) {
                let anchor = c.base + r;
                if idx == lo
                    && let Some(n) = new.take()
                {
                    entries.extend(n);
                }
                if idx < lo {
                    entries.push((anchor, b));
                } else if idx >= hi {
                    entries.push(((anchor as isize + delta) as usize, b));
                }
                idx += 1;
            }
        }
        if let Some(n) = new.take() {
            entries.extend(n);
        }
        let n = entries.len();
        let rebuilt = chunk_entries(entries.into_iter(), n);
        let count = rebuilt.len();
        self.chunks.splice(ka..ka, rebuilt);
        if delta != 0 {
            for c in &mut self.chunks[ka + count..] {
                c.base = (c.base as isize + delta) as usize;
            }
        }
        self.reindex(ka);
    }
}

impl<T> RelChunks<T> {
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Recomputes `firsts` and `len` from chunk `from` on.
    fn reindex(&mut self, from: usize) {
        self.firsts.truncate(from);
        let mut n = if from == 0 {
            0
        } else {
            self.firsts[from - 1] + self.chunks[from - 1].items.len()
        };
        for c in &self.chunks[from..] {
            self.firsts.push(n);
            n += c.items.len();
        }
        self.len = n;
    }

    /// `(chunk, index in chunk)` of global index `i < len`.
    #[inline]
    fn locate(&self, i: usize) -> (usize, usize) {
        let k = self.firsts.partition_point(|&f| f <= i) - 1;
        (k, i - self.firsts[k])
    }

    /// Element `i` with its absolute origin. O(log(len / CHUNK)).
    pub fn get(&self, i: usize) -> Option<Rel<'_, T>> {
        if i >= self.len {
            return None;
        }
        let (k, j) = self.locate(i);
        let c = &self.chunks[k];
        Some(Rel {
            origin: c.base + c.rel[j],
            item: &c.items[j],
        })
    }

    /// Mutable access to element `i` (relative offsets; the origin cannot change).
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i >= self.len {
            return None;
        }
        let (k, j) = self.locate(i);
        Some(&mut self.chunks[k].items[j])
    }

    /// Shifts every element after `i` by `delta` (element `i` grew or shrank by `delta`
    /// without moving its origin). O(`CHUNK` + len / `CHUNK`).
    pub fn shift_after(&mut self, i: usize, delta: isize) {
        if delta == 0 || i >= self.len {
            return;
        }
        let (k, j) = self.locate(i);
        for r in &mut self.chunks[k].rel[j + 1..] {
            *r = (*r as isize + delta) as usize;
        }
        for c in &mut self.chunks[k + 1..] {
            c.base = (c.base as isize + delta) as usize;
        }
    }

    /// Absolute origin of element `i`.
    pub fn origin(&self, i: usize) -> Option<usize> {
        self.get(i).map(|r| r.origin)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Rel<'_, T>> + '_ {
        self.chunks.iter().flat_map(|c| {
            c.rel.iter().zip(&c.items).map(move |(&r, b)| Rel {
                origin: c.base + r,
                item: b,
            })
        })
    }

    /// Elements `range` in order (O(log) to start, then sequential).
    pub fn range(&self, range: Range<usize>) -> impl Iterator<Item = Rel<'_, T>> + '_ {
        let n = range.end.min(self.len).saturating_sub(range.start);
        let start = if range.start < self.len {
            self.locate(range.start)
        } else {
            (self.chunks.len(), 0)
        };
        self.chunks[start.0.min(self.chunks.len())..]
            .iter()
            .enumerate()
            .flat_map(move |(ci, c)| {
                let skip = if ci == 0 { start.1 } else { 0 };
                c.rel[skip..].iter().zip(&c.items[skip..]).map(move |(&r, b)| Rel {
                    origin: c.base + r,
                    item: b,
                })
            })
            .take(n)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.chunks.iter_mut().flat_map(|c| c.items.iter_mut())
    }

    /// Index of the first element for which `pred` is false; `pred` must be true for a
    /// prefix of the elements (like `slice::partition_point`).
    pub fn partition_point(&self, mut pred: impl FnMut(&Rel<'_, T>) -> bool) -> usize {
        let k = self.chunks.partition_point(|c| {
            pred(&Rel {
                origin: c.base + c.rel[0],
                item: &c.items[0],
            })
        });
        if k == 0 {
            return 0;
        }
        let c = &self.chunks[k - 1];
        let (mut lo, mut hi) = (1, c.items.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if pred(&Rel {
                origin: c.base + c.rel[mid],
                item: &c.items[mid],
            }) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        self.firsts[k - 1] + lo
    }

    /// All elements as owned absolute copies (O(len)).
    pub fn to_abs_vec(&self) -> Vec<T>
    where
        T: Clone + Relative,
    {
        self.iter().map(|r| r.to_abs()).collect()
    }
}

impl<T> std::ops::Index<usize> for RelChunks<T> {
    type Output = T;
    /// The element with **relative** offsets (see [`RelChunks::get`] for its origin).
    fn index(&self, i: usize) -> &T {
        self.get(i).expect("index out of bounds").item
    }
}

fn to_rel<T: Relative>(mut b: T) -> (usize, T) {
    let o = b.origin();
    b.shift(-(o as isize));
    (o, b)
}

/// Splits `(absolute origin, relative element)` entries into chunks of about equal size, at
/// most [`CHUNK`] each.
fn chunk_entries<T>(entries: impl Iterator<Item = (usize, T)>, n: usize) -> Vec<Chunk<T>> {
    if n == 0 {
        return Vec::new();
    }
    let pieces = n.div_ceil(CHUNK);
    let per = n.div_ceil(pieces);
    let mut out: Vec<Chunk<T>> = Vec::with_capacity(pieces);
    for (o, b) in entries {
        match out.last_mut() {
            Some(c) if c.items.len() < per => {
                c.rel.push(o - c.base);
                c.items.push(b);
            }
            _ => {
                let mut rel = Vec::with_capacity(per);
                rel.push(0);
                let mut items = Vec::with_capacity(per);
                items.push(b);
                out.push(Chunk { base: o, rel, items });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct E(Span, Vec<Span>);
    impl Relative for E {
        fn origin(&self) -> usize {
            self.0.start
        }
        fn shift(&mut self, d: isize) {
            self.0.shift(d);
            for s in &mut self.1 {
                s.shift(d);
            }
        }
    }
    fn e(start: usize, end: usize) -> E {
        E(Span::new(start, end), vec![Span::new(start + 1, end)])
    }
    fn spans(t: &RelChunks<E>) -> Vec<(usize, usize)> {
        t.iter().map(|x| (x.origin, x.origin + x.item.0.end)).collect()
    }

    #[test]
    fn splice_shifts_tail_across_chunks() {
        let n = CHUNK * 3 + 7;
        let mut t = RelChunks::from_abs((0..n).map(|i| e(i * 10, i * 10 + 5)).collect());
        assert_eq!(t.len(), n);
        t.splice(3..4, vec![e(30, 32), e(33, 39)], 4);
        assert_eq!(t.len(), n + 1);
        let s = spans(&t);
        assert_eq!(s[3], (30, 32));
        assert_eq!(s[4], (33, 39));
        assert_eq!(s[5], (44, 49));
        assert_eq!(*s.last().unwrap(), ((n - 1) * 10 + 4, (n - 1) * 10 + 9));
        let last = t.get(t.len() - 1).unwrap();
        assert_eq!(last.to_abs().1[0].start, last.origin + 1);
        for (i, sp) in s.iter().enumerate() {
            assert_eq!(t.get(i).unwrap().origin, sp.0);
        }
        assert_eq!(
            t.partition_point(|x| x.origin <= 2000),
            s.iter().filter(|x| x.0 <= 2000).count()
        );
        let r: Vec<usize> = t.range(250..260).map(|x| x.origin).collect();
        assert_eq!(r, s[250..260].iter().map(|x| x.0).collect::<Vec<_>>());
        t.splice(10..(CHUNK * 2), Vec::new(), -100);
        assert_eq!(t.len(), n + 1 - (CHUNK * 2 - 10));
        let s2 = spans(&t);
        assert_eq!(s2[10].0, s[CHUNK * 2].0 - 100);
        assert_eq!(t.to_abs_vec().len(), t.len());
        let end = s2.last().unwrap().1;
        t.splice(t.len()..t.len(), vec![e(end + 2, end + 3)], 0);
        assert_eq!(spans(&t).last().unwrap().0, end + 2);
        t.splice(0..0, vec![e(0, 1)], 2);
        assert_eq!(spans(&t)[0], (0, 1));
        assert_eq!(spans(&t)[1], (2, 7));
        assert_eq!(t[1].0, Span::new(0, 5), "Index gives relative offsets");
        let before = spans(&t);
        t.shift_after(3, 7);
        let after = spans(&t);
        assert_eq!(after[..4], before[..4]);
        assert!(after[4..].iter().zip(&before[4..]).all(|(a, b)| a.0 == b.0 + 7));
    }

    #[test]
    fn incremental_constructors_match() {
        let all: Vec<E> = (0..1000).map(|i| e(i * 3, i * 3 + 2)).collect();
        let a = RelChunks::from_abs(all.clone());
        let b = RelChunks::from_abs_vec(all.clone());
        let mut c = RelChunks::default();
        c.extend_abs(all[..300].to_vec());
        c.extend_abs(all[300..].to_vec());
        assert_eq!(a.to_abs_vec(), all);
        assert_eq!(b.to_abs_vec(), all);
        assert_eq!(c.to_abs_vec(), all);
        assert_eq!(c.get(999).unwrap().origin, 2997);
    }

    #[test]
    fn empty_and_singletons() {
        let mut t: RelChunks<E> = RelChunks::from_abs(Vec::new());
        assert!(t.is_empty());
        assert!(t.get(0).is_none());
        assert_eq!(t.partition_point(|_| true), 0);
        assert_eq!(t.range(0..5).count(), 0);
        t.splice(0..0, vec![e(0, 3)], 3);
        assert_eq!(spans(&t), vec![(0, 3)]);
        t.splice(0..1, Vec::new(), -3);
        assert!(t.is_empty());
    }

    #[test]
    fn many_random_splices_match_a_flat_vector() {
        // deterministic LCG
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |m: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % m.max(1)
        };
        let mut flat: Vec<E> = (0..2000).map(|i| e(i * 4, i * 4 + 3)).collect();
        let mut t = RelChunks::from_abs(flat.clone());
        for _ in 0..300 {
            let a = rnd(flat.len() + 1);
            let b = (a + rnd(40)).min(flat.len());
            let at = if a < flat.len() {
                flat[a].0.start
            } else {
                flat.last().map_or(0, |x| x.0.end + 1)
            };
            let k = rnd(5);
            let new: Vec<E> = (0..k).map(|i| e(at + i, at + i + 1)).collect();
            let old_end = if b < flat.len() { flat[b].0.start } else { at };
            let new_end = at + k + 1;
            let delta = new_end as isize - old_end.max(at) as isize;
            let mut tail: Vec<E> = flat.drain(a..).skip(b - a).collect();
            for x in &mut tail {
                x.shift(delta);
            }
            flat.extend(new.clone());
            flat.extend(tail);
            t.splice(a..b, new, delta);
            assert_eq!(t.to_abs_vec(), flat);
        }
    }
}
