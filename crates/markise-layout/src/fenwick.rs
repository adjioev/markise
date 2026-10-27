//! Fenwick (binary indexed) tree over block heights: O(log n) height updates, prefix sums and
//! "which block is at y" queries. Heights are `f64` so positions stay exact in very tall
//! documents (a 10 MB file is several million points tall; `f32` would lose sub-pixel precision).

#[derive(Clone, Debug, Default)]
pub struct Fenwick {
    tree: Vec<f64>,
    values: Vec<f64>,
}

impl Fenwick {
    /// Builds the tree from `values` in O(n).
    pub fn from_values(values: Vec<f64>) -> Self {
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
        Fenwick { tree, values }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Replaces values `range` with `new`. Same length: O(k log n) updates; otherwise the
    /// values are spliced and the tree rebuilt in O(n) (a linear pass over `f64`s, ADR 0006
    /// §7.3 "allowed per keystroke").
    pub fn splice(&mut self, range: std::ops::Range<usize>, new: Vec<f64>) {
        if range.len() == new.len() {
            for (k, v) in new.into_iter().enumerate() {
                self.set(range.start + k, v);
            }
            return;
        }
        let mut values = std::mem::take(&mut self.values);
        values.splice(range, new);
        *self = Fenwick::from_values(values);
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, i: usize) -> f64 {
        self.values[i]
    }

    /// Sets value `i`.
    pub fn set(&mut self, i: usize, v: f64) {
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

    /// Sum of values `0..i`.
    pub fn prefix(&self, i: usize) -> f64 {
        let mut k = i.min(self.values.len());
        let mut s = 0.0;
        while k > 0 {
            s += self.tree[k];
            k &= k - 1;
        }
        s
    }

    pub fn total(&self) -> f64 {
        self.prefix(self.values.len())
    }

    /// Index of the value containing position `y` (the largest `i` with `prefix(i) <= y`),
    /// clamped to the last index. Returns 0 for an empty tree.
    pub fn index_at(&self, y: f64) -> usize {
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
        // skip zero-height entries so that `y` maps to a visible block where possible
        pos.min(n - 1)
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
}
