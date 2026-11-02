//! Fenwick (binary indexed) tree over block heights: O(log n) height updates, prefix sums and
//! "which block is at y" queries. Heights are `f64` so positions stay exact in very tall
//! documents (a 10 MB file is several million points tall; `f32` would lose sub-pixel precision).

/// Values per leaf of the two-level tree.
const LEAF: usize = 1024;

/// A flat Fenwick tree (one leaf of [`Fenwick`], and its level over leaf totals).
#[derive(Clone, Debug, Default)]
struct Flat {
    tree: Vec<f64>,
    values: Vec<f64>,
}

impl Flat {
    /// Builds the tree from `values` in O(n).
    fn from_values(values: Vec<f64>) -> Self {
        let n = values.len();
        let mut tree = vec![0.0; n + 1];
        for (i, v) in values.iter().enumerate() {
            tree[i + 1] += v;
            let j = (i + 1) + ((i + 1) & (!(i + 1) + 1));
            if j <= n {
                let t = tree[i + 1];
                tree[j] += t;
            }
        }
        Flat { tree, values }
    }

    fn set(&mut self, i: usize, v: f64) {
        let delta = v - self.values[i];
        if delta == 0.0 {
            return;
        }
        self.values[i] = v;
        let mut k = i + 1;
        while k < self.tree.len() {
            self.tree[k] += delta;
            k += k & (!k + 1);
        }
    }

    fn prefix(&self, i: usize) -> f64 {
        let mut k = i.min(self.values.len());
        let mut s = 0.0;
        while k > 0 {
            s += self.tree[k];
            k &= k - 1;
        }
        s
    }

    /// The largest `i` with `prefix(i) <= y`, clamped to the last index (0 when empty).
    fn index_at(&self, y: f64) -> usize {
        let n = self.values.len();
        if n == 0 {
            return 0;
        }
        let mut pos = 0;
        let mut rem = y;
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= rem {
                pos = next;
                rem -= self.tree[next];
            }
            step >>= 1;
        }
        pos.min(n - 1)
    }
}

/// Fenwick tree over item heights, in two levels: leaves of up to 1,024 values (each a
/// flat Fenwick tree) and a Fenwick tree over the leaf totals. Updates, prefix sums and
/// lookups are O(log n); a splice that changes the number of values rebuilds only the touched
/// leaves and the small top level — O(`LEAF` + n / `LEAF`), not O(n) (issue #29: Enter /
/// Backspace across blocks in a 2 M-item document).
#[derive(Clone, Debug, Default)]
pub struct Fenwick {
    leaves: Vec<Flat>,
    /// Global index of each leaf's first value.
    firsts: Vec<usize>,
    /// Fenwick tree over the leaf totals.
    top: Flat,
    len: usize,
}

impl Fenwick {
    /// Builds the tree from `values` in O(n).
    pub fn from_values(values: Vec<f64>) -> Self {
        let mut f = Fenwick {
            leaves: values.chunks(LEAF).map(|c| Flat::from_values(c.to_vec())).collect(),
            ..Default::default()
        };
        f.reindex(0);
        f
    }

    /// Recomputes `firsts` (from leaf `from` on), `len` and the top level (O(leaves)).
    fn reindex(&mut self, from: usize) {
        self.firsts.truncate(from);
        let mut n = if from == 0 {
            0
        } else {
            self.firsts[from - 1] + self.leaves[from - 1].values.len()
        };
        for l in &self.leaves[from..] {
            self.firsts.push(n);
            n += l.values.len();
        }
        self.len = n;
        self.top = Flat::from_values(self.leaves.iter().map(|l| l.prefix(l.values.len())).collect());
    }

    /// `(leaf, index in leaf)` of global index `i < len`.
    fn locate(&self, i: usize) -> (usize, usize) {
        let k = self.firsts.partition_point(|&f| f <= i) - 1;
        (k, i - self.firsts[k])
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Replaces values `range` with `new`. Same length: O(k log n) updates; otherwise the
    /// leaves holding the range are rebuilt (with a small neighbour, so leaves do not shrink
    /// to nothing) and the top level is recomputed: O(`LEAF` + k + n / `LEAF`).
    pub fn splice(&mut self, range: std::ops::Range<usize>, new: Vec<f64>) {
        assert!(
            range.start <= range.end && range.end <= self.len,
            "splice out of bounds"
        );
        if range.len() == new.len() {
            for (k, v) in new.into_iter().enumerate() {
                self.set(range.start + k, v);
            }
            return;
        }
        if self.leaves.is_empty() {
            *self = Fenwick::from_values(new);
            return;
        }
        let last = self.leaves.len() - 1;
        let leaf_of = |i: usize| if i >= self.len { last } else { self.locate(i).0 };
        let ka = leaf_of(range.start);
        let mut kb = leaf_of(range.end.saturating_sub(1).max(range.start)).max(ka);
        let group: usize = (ka..=kb).map(|k| self.leaves[k].values.len()).sum();
        if group + new.len() - range.len() < LEAF / 4 && kb < last {
            kb += 1;
        }
        let first = self.firsts[ka];
        let mut values: Vec<f64> = Vec::with_capacity(group + new.len());
        for l in self.leaves.drain(ka..=kb) {
            values.extend(l.values);
        }
        values.splice(range.start - first..range.end - first, new);
        let rebuilt: Vec<Flat> = values.chunks(LEAF).map(|c| Flat::from_values(c.to_vec())).collect();
        self.leaves.splice(ka..ka, rebuilt);
        self.reindex(ka);
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// All values in order.
    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.leaves.iter().flat_map(|l| l.values.iter().copied())
    }

    pub fn get(&self, i: usize) -> f64 {
        let (k, j) = self.locate(i);
        self.leaves[k].values[j]
    }

    /// Sets value `i`.
    pub fn set(&mut self, i: usize, v: f64) {
        let (k, j) = self.locate(i);
        let leaf = &mut self.leaves[k];
        let delta = v - leaf.values[j];
        if delta == 0.0 {
            return;
        }
        leaf.set(j, v);
        let t = self.top.values[k] + delta;
        self.top.set(k, t);
    }

    /// Sum of values `0..i`.
    pub fn prefix(&self, i: usize) -> f64 {
        if i >= self.len {
            return self.total();
        }
        let (k, j) = self.locate(i);
        self.top.prefix(k) + self.leaves[k].prefix(j)
    }

    pub fn total(&self) -> f64 {
        self.top.prefix(self.leaves.len())
    }

    /// Index of the value containing position `y` (the largest `i` with `prefix(i) <= y`),
    /// clamped to the last index. Returns 0 for an empty tree.
    pub fn index_at(&self, y: f64) -> usize {
        if self.len == 0 {
            return 0;
        }
        let k = self.top.index_at(y);
        let rem = y - self.top.prefix(k);
        self.firsts[k] + self.leaves[k].index_at(rem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_and_lookup_match_naive() {
        let vals: Vec<f64> = (0..37).map(|i| ((i * 7) % 5) as f64 * 3.5).collect();
        let mut f = Fenwick::from_values(vals.clone());
        for i in 0..=vals.len() {
            let naive: f64 = vals[..i].iter().sum();
            assert!((f.prefix(i) - naive).abs() < 1e-9);
        }
        f.set(10, 100.0);
        let mut v2 = vals.clone();
        v2[10] = 100.0;
        for y in [0.0, 1.0, 50.0, 120.0, 400.0, 1e9] {
            let mut acc = 0.0;
            let mut want = v2.len() - 1;
            for (i, h) in v2.iter().enumerate() {
                if acc + h > y {
                    want = i;
                    break;
                }
                acc += h;
            }
            assert_eq!(f.index_at(y), want, "y={y}");
        }
    }

    fn naive_index(v: &[f64], y: f64) -> usize {
        // the largest `i` with `sum(v[..i]) <= y`, clamped to the last index
        let mut acc = 0.0;
        let mut best = 0;
        for i in 0..=v.len() {
            if acc <= y {
                best = i;
            }
            if i < v.len() {
                acc += v[i];
            }
        }
        best.min(v.len().saturating_sub(1))
    }

    /// Issue #29: splices that change the number of values rebuild only the touched leaves;
    /// the tree must still agree with a plain list across leaf boundaries (zero heights too).
    #[test]
    fn splices_across_leaves_match_naive() {
        let mut seed = 7u64;
        let mut rnd = |m: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % m.max(1)
        };
        // multiples of 0.5: sums stay exact, so comparisons can be exact
        let val = |r: &mut dyn FnMut(usize) -> usize| if r(5) == 0 { 0.0 } else { r(40) as f64 * 0.5 };
        let mut v: Vec<f64> = (0..5000).map(|_| val(&mut rnd)).collect();
        let mut f = Fenwick::from_values(v.clone());
        for round in 0..300 {
            let a = rnd(v.len() + 1);
            let b = (a + rnd(if round % 7 == 0 { 3000 } else { 4 })).min(v.len());
            let n = if round % 11 == 0 { rnd(2500) } else { rnd(4) };
            let new: Vec<f64> = (0..n).map(|_| val(&mut rnd)).collect();
            v.splice(a..b, new.clone());
            f.splice(a..b, new);
            if round % 5 == 0 && !v.is_empty() {
                let i = rnd(v.len());
                let x = val(&mut rnd);
                v[i] = x;
                f.set(i, x);
            }
            assert_eq!(f.len(), v.len());
            for _ in 0..20 {
                let i = rnd(v.len() + 1);
                assert_eq!(f.prefix(i), v[..i].iter().sum::<f64>(), "prefix({i}) round {round}");
                if i < v.len() {
                    assert_eq!(f.get(i), v[i]);
                }
                let y = rnd((v.iter().sum::<f64>() as usize + 10) * 2) as f64 * 0.5;
                assert_eq!(f.index_at(y), naive_index(&v, y), "index_at({y}) round {round}");
            }
            assert_eq!(f.total(), v.iter().sum::<f64>());
        }
        // shrinking to nothing and growing again
        let n = f.len();
        f.splice(0..n, Vec::new());
        assert!(f.is_empty());
        assert_eq!(f.index_at(5.0), 0);
        f.splice(0..0, vec![1.0, 2.0]);
        assert_eq!((f.len(), f.total(), f.index_at(1.5)), (2, 3.0, 1));
    }
}
