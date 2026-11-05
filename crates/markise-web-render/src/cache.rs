//! A bounded cache of what took a render to get: least recently used goes first. In memory only:
//! a picture is worth less than the disk it would take to keep it, and it can come again.

use std::collections::HashMap;

struct Entry<T> {
    value: T,
    cost: usize,
    used: u64,
}

pub struct ShotCache<T> {
    entries: HashMap<u64, Entry<T>>,
    limit: usize,
    total: usize,
    clock: u64,
}

impl<T> ShotCache<T> {
    /// `limit`: what the costs may add up to.
    pub fn new(limit: usize) -> ShotCache<T> {
        ShotCache {
            entries: HashMap::new(),
            limit,
            total: 0,
            clock: 0,
        }
    }

    pub fn contains(&self, key: u64) -> bool {
        self.entries.contains_key(&key)
    }

    /// The value, now the most recently used.
    pub fn get(&mut self, key: u64) -> Option<&T> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(&key).map(|e| {
            e.used = clock;
            &e.value
        })
    }

    /// The value, with no change to what goes first.
    pub fn peek(&self, key: u64) -> Option<&T> {
        self.entries.get(&key).map(|e| &e.value)
    }

    /// Keeps it, unless it alone is over the limit, and makes room. Returns what went.
    pub fn insert(&mut self, key: u64, value: T, cost: usize) -> Vec<u64> {
        let mut gone = Vec::new();
        if let Some(old) = self.entries.remove(&key) {
            self.total -= old.cost;
        }
        if cost > self.limit {
            gone.push(key);
            return gone;
        }
        self.clock += 1;
        self.total += cost;
        let used = self.clock;
        self.entries.insert(key, Entry { value, cost, used });
        while self.total > self.limit {
            let Some(oldest) = (self.entries.iter()).min_by_key(|(_, e)| e.used).map(|(k, _)| *k) else {
                break;
            };
            if let Some(e) = self.entries.remove(&oldest) {
                self.total -= e.cost;
            }
            gone.push(oldest);
        }
        gone
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total = 0;
    }

    /// What the costs add up to.
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
