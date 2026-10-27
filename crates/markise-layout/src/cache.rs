//! Per-item layout cache.
//!
//! The key holds everything a [`BlockLayout`](crate::BlockLayout) depends on: the item's own
//! content hash (kind + source + containers + gap), the document context hash (link
//! definitions, footnotes, outline for `[toc]`), the column width, the theme fingerprint, the
//! pixels-per-point and an `extra` word (expanded state, image epoch). A change of any input
//! produces a different key, so stale layouts are never returned ("invalidated on change");
//! unused entries are evicted least-recently-used once the cache exceeds its capacity.
//! Identical blocks (same key) share one layout.

use std::collections::HashMap;
use std::sync::Arc;

use crate::block::BlockLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub item_hash: u64,
    pub ctx_hash: u64,
    /// Width in quarter points.
    pub width_q: u32,
    pub theme: u64,
    /// Pixels per point × 100.
    pub ppp_q: u32,
    pub extra: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evicted: u64,
}

struct Entry {
    layout: Arc<BlockLayout>,
    last_used: u64,
    bytes: usize,
}

pub struct LayoutCache {
    map: HashMap<CacheKey, Entry>,
    tick: u64,
    capacity: usize,
    max_bytes: usize,
    bytes: usize,
    pub stats: CacheStats,
}

/// Default limits: galleys carry their glyph meshes, so the cache is bounded by memory as
/// well as by entry count (NFR-6/7).
pub const DEFAULT_CAPACITY: usize = 4096;
pub const DEFAULT_MAX_BYTES: usize = 16 << 20;

impl Default for LayoutCache {
    fn default() -> Self {
        Self::with_limits(DEFAULT_CAPACITY, DEFAULT_MAX_BYTES)
    }
}

impl LayoutCache {
    /// `capacity`: number of entries kept before least-recently-used eviction.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_limits(capacity, usize::MAX)
    }

    /// Limits by entry count and by estimated bytes (see [`BlockLayout::cost`]).
    pub fn with_limits(capacity: usize, max_bytes: usize) -> Self {
        LayoutCache {
            map: HashMap::new(),
            tick: 0,
            capacity: capacity.max(16),
            max_bytes,
            bytes: 0,
            stats: CacheStats::default(),
        }
    }

    /// Estimated bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Advances the use clock (call once per frame).
    pub fn tick(&mut self) {
        self.tick += 1;
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<Arc<BlockLayout>> {
        match self.map.get_mut(key) {
            Some(e) => {
                e.last_used = self.tick;
                self.stats.hits += 1;
                Some(e.layout.clone())
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// Looks up without touching statistics or recency (height reuse after a reload).
    pub fn peek(&self, key: &CacheKey) -> Option<&Arc<BlockLayout>> {
        self.map.get(key).map(|e| &e.layout)
    }

    pub fn insert(&mut self, key: CacheKey, layout: Arc<BlockLayout>) {
        let bytes = layout.cost();
        self.bytes += bytes;
        if let Some(old) = self.map.insert(
            key,
            Entry {
                layout,
                last_used: self.tick,
                bytes,
            },
        ) {
            self.bytes -= old.bytes;
        }
        if self.map.len() > self.capacity || self.bytes > self.max_bytes {
            self.evict();
        }
    }

    /// Drops the least recently used entries down to 3/4 of the capacity; entries used in the
    /// current tick are never dropped.
    fn evict(&mut self) {
        let target_n = self.capacity * 3 / 4;
        let target_b = self.max_bytes / 4 * 3;
        let mut ages: Vec<(u64, usize)> = self.map.values().map(|e| (e.last_used, e.bytes)).collect();
        ages.sort_unstable();
        // drop the oldest entries until both targets hold (never the current tick's)
        let (mut n, mut b) = (self.map.len(), self.bytes);
        let mut cutoff = None;
        for &(age, bytes) in &ages {
            if (n <= target_n && b <= target_b) || age >= self.tick {
                break;
            }
            n -= 1;
            b -= bytes;
            cutoff = Some(age);
        }
        let Some(cutoff) = cutoff else { return };
        let before = self.map.len();
        let mut freed = 0;
        self.map.retain(|_, e| {
            let keep = e.last_used > cutoff;
            if !keep {
                freed += e.bytes;
            }
            keep
        });
        self.bytes -= freed;
        self.stats.evicted += (before - self.map.len()) as u64;
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: u64) -> CacheKey {
        CacheKey {
            item_hash: i,
            ctx_hash: 0,
            width_q: 2400,
            theme: 1,
            ppp_q: 100,
            extra: 0,
        }
    }

    #[test]
    fn hit_miss_and_lru_eviction() {
        let mut c = LayoutCache::with_capacity(16);
        for i in 0..16 {
            c.insert(key(i), Arc::new(BlockLayout::default()));
            c.tick();
        }
        assert!(c.get(&key(3)).is_some());
        assert!(c.get(&key(99)).is_none());
        assert_eq!((c.stats.hits, c.stats.misses), (1, 1));
        // key 3 was just used: it survives the eviction triggered by the 17th insert
        c.tick();
        c.insert(key(100), Arc::new(BlockLayout::default()));
        assert!(c.len() <= 13, "evicted down to 3/4 capacity, len = {}", c.len());
        assert!(c.peek(&key(3)).is_some());
        assert!(c.peek(&key(100)).is_some());
        assert!(c.peek(&key(0)).is_none(), "oldest entry evicted");
    }

    #[test]
    fn byte_limit_evicts() {
        let mut c = LayoutCache::with_limits(1000, 4000);
        for i in 0..10 {
            let l = BlockLayout {
                height: 1.0,
                ..Default::default()
            };
            // default layout costs its struct size; fake a heavier one through many links
            let mut l = l;
            l.links = (0..10)
                .map(|k| crate::block::LinkHit {
                    prim: k,
                    range: 0..1,
                    target: "x".repeat(20),
                })
                .collect();
            c.insert(key(i), Arc::new(l));
            c.tick();
        }
        assert!(
            c.bytes() <= 4000,
            "bytes {} len {} each {}",
            c.bytes(),
            c.len(),
            c.peek(&key(9)).unwrap().cost()
        );
        assert!(c.peek(&key(9)).is_some());
        assert!(c.peek(&key(0)).is_none());
    }

    #[test]
    fn every_key_component_separates_entries() {
        let mut c = LayoutCache::with_capacity(64);
        let base = key(1);
        c.insert(base, Arc::new(BlockLayout::default()));
        let variants = [
            CacheKey { item_hash: 2, ..base },
            CacheKey { ctx_hash: 9, ..base },
            CacheKey { width_q: 2000, ..base },
            CacheKey { theme: 7, ..base },
            CacheKey { ppp_q: 200, ..base },
            CacheKey { extra: 1, ..base },
        ];
        for v in variants {
            assert!(c.get(&v).is_none(), "{v:?} must miss");
        }
        assert!(c.get(&base).is_some());
    }
}
