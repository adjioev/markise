//! Index of backtick runs, so "find the next run of exactly `n` backticks" is a binary search
//! instead of a scan (keeps inputs with many distinct run lengths linear-ish).

use std::collections::HashMap;

/// Maximal backtick runs of a byte range, grouped by length.
#[derive(Default)]
pub(crate) struct TickRuns {
    by_len: HashMap<usize, Vec<usize>>,
}

impl TickRuns {
    pub(crate) fn build(b: &[u8], start: usize, end: usize) -> Self {
        let mut by_len: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut i = start;
        while i < end {
            match memchr::memchr(b'`', &b[i..end]) {
                None => break,
                Some(off) => {
                    let s = i + off;
                    let mut t = s;
                    while t < end && b[t] == b'`' {
                        t += 1;
                    }
                    by_len.entry(t - s).or_default().push(s);
                    i = t;
                }
            }
        }
        TickRuns { by_len }
    }

    /// Start of the first run of exactly `len` backticks starting at or after `from`.
    pub(crate) fn next_run(&self, len: usize, from: usize) -> Option<usize> {
        let v = self.by_len.get(&len)?;
        let i = v.partition_point(|&p| p < from);
        v.get(i).copied()
    }
}

/// Start of the first run of exactly `n` backticks in `b[from..end]`. A bounded plain scan
/// handles the common case; past `budget` bytes the index over `b[start..end]` is built once
/// (in `idx`) and used, so repeated searches stay near-linear.
pub(crate) fn find_run(
    b: &[u8],
    start: usize,
    from: usize,
    end: usize,
    n: usize,
    budget: usize,
    idx: &mut Option<TickRuns>,
) -> Option<usize> {
    let limit = from.saturating_add(budget).min(end);
    let mut r = from;
    while r < limit {
        if b[r] == b'`' {
            let mut t = r;
            while t < end && b[t] == b'`' {
                t += 1;
            }
            if t - r == n {
                return Some(r);
            }
            r = t;
        } else {
            r += 1;
        }
    }
    if limit >= end {
        return None;
    }
    let t = idx.get_or_insert_with(|| TickRuns::build(b, start, end));
    t.next_run(n, from).filter(|&r| r + n <= end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs() {
        let s = b"a `` b ` c `` d ```";
        let t = TickRuns::build(s, 0, s.len());
        assert_eq!(t.next_run(2, 0), Some(2));
        assert_eq!(t.next_run(2, 4), Some(11));
        assert_eq!(t.next_run(1, 0), Some(7));
        assert_eq!(t.next_run(3, 0), Some(16));
        assert_eq!(t.next_run(4, 0), None);
    }
}
