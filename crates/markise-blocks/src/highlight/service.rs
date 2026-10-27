//! Background highlighting: one worker thread, a bounded job queue, a byte-bounded result cache.
//!
//! - Results are keyed by a [`BlockKey`] — a hash of the block's language and content computed
//!   by the caller — so identical blocks share one result and a reload reuses it. Results are
//!   token kinds, not colors: they stay valid across color-scheme changes.
//! - Work is bounded per step: the worker highlights one *chunk* ([`CHUNK_LINES`] lines,
//!   [`CHUNK_BYTES`] bytes or [`CHUNK_TIME`], whichever comes first) of a block, publishes it, then moves on to the next job
//!   (round-robin; newly requested blocks go first). A huge block is thus highlighted
//!   incrementally with syntect's line state carried between chunks, and the visible part of
//!   other blocks does not wait for it.
//! - Cancellation: every request carries a generation; [`HighlightService::set_generation`]
//!   (document change) cancels unfinished jobs of older generations at their next chunk, and
//!   drops their partial results. A newer request for the same block adopts the running job.
//! - The wake callback (repaint) is throttled to one call per [`WAKE_INTERVAL`] while chunks
//!   of a long job arrive, plus one when a block completes or a job is cancelled.
//! - Memory is bounded: block content is built *on the worker* from a caller-supplied source
//!   (the UI thread never concatenates a block), and the source bytes of queued and running jobs
//!   are reserved against [`MAX_SOURCE_BYTES`] before a job is admitted; results — pending ones
//!   included — count against the cache budget. A block whose content exceeds
//!   [`MAX_BLOCK_SOURCE_BYTES`], or whose runs would exceed a quarter of the cache budget (or
//!   not fit at all), gets a bounded *terminal* result: the runs so far, plain for the rest. It
//!   is never re-highlighted in a loop; fresh results are kept until the UI has read them.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use markise_theme::TokenRun;

use super::{Highlighter, MAX_LINE_BYTES, resolve};

/// Hash of (language, block content), computed by the caller.
pub type BlockKey = u64;

/// A [`BlockKey`] from the fence language and the content lines (std hasher; callers with their
/// own content hashes may use those instead).
pub fn block_key<'a>(lang: &str, lines: impl IntoIterator<Item = &'a str>) -> BlockKey {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lang.to_lowercase().hash(&mut h);
    for l in lines {
        l.hash(&mut h);
    }
    h.finish()
}

/// Lines per worker step.
pub const CHUNK_LINES: usize = 64;
/// Bytes per worker step (soft: the step ends after the line that crosses it).
pub const CHUNK_BYTES: usize = 16 * 1024;
/// Time budget per worker step (soft: checked after each line).
pub const CHUNK_TIME: Duration = Duration::from_millis(8);
/// Queued requests not yet taken by the worker.
pub const MAX_QUEUED: usize = 256;
/// Unfinished jobs the worker keeps; beyond this the oldest is cancelled (re-requested when
/// it is shown again).
pub const MAX_ACTIVE: usize = 32;
/// Default cache size (estimated bytes of runs).
pub const DEFAULT_CACHE_BYTES: usize = 32 << 20;
/// Source bytes of all queued and running jobs together.
pub const MAX_SOURCE_BYTES: usize = 64 << 20;
/// Blocks with more content than this are not highlighted (terminal plain result).
pub const MAX_BLOCK_SOURCE_BYTES: usize = 16 << 20;
/// A completed result nobody has read yet is not evicted for this long.
pub const FRESH_FOR: Duration = Duration::from_secs(2);
pub const WAKE_INTERVAL: Duration = Duration::from_millis(30);

/// Result of a cache lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The range is highlighted (empty for a plain language).
    Ready(Vec<TokenRun>),
    /// Requested, not (fully) highlighted yet.
    Pending,
    /// Never requested, cancelled or evicted.
    Missing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServiceStats {
    pub requests: u64,
    pub chunks: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub evicted: u64,
    /// Blocks given a truncated terminal result (too large for the budgets).
    pub truncated: u64,
    /// Requests refused because the source-byte budget was exhausted (asked again later).
    pub refused: u64,
    /// Bytes of code highlighted.
    pub bytes: u64,
    /// Longest single worker step.
    pub max_step: Duration,
}

struct Chunk {
    start: u32,
    end: u32,
    runs: Box<[TokenRun]>,
}

#[derive(Default)]
struct Entry {
    chunks: Vec<Chunk>,
    /// Content bytes highlighted so far (from the start).
    done: u32,
    complete: bool,
    /// Terminal result covering only a prefix (the rest is plain).
    truncated: bool,
    /// Read by `ready`/`lookup` since it completed.
    consumed: bool,
    completed_at: Option<Instant>,
    last_used: u64,
    bytes: usize,
}

#[derive(Default)]
struct State {
    entries: HashMap<BlockKey, Entry>,
    /// Unfinished requests → their newest generation.
    pending: HashMap<BlockKey, u64>,
    bytes: usize,
    max_bytes: usize,
    /// Source bytes reserved by queued and running jobs.
    source_bytes: usize,
    max_source_bytes: usize,
    /// See [`FRESH_FOR`].
    fresh_for: Duration,
    tick: u64,
    stats: ServiceStats,
}

impl State {
    /// Evicts completed results (least recently used first; unread fresh results never) until
    /// `incoming` more bytes fit under the budget, or down to 3/4 of it when over budget.
    fn evict(&mut self, incoming: usize) {
        if self.bytes + incoming <= self.max_bytes {
            return;
        }
        let now = Instant::now();
        let mut done: Vec<(u64, BlockKey, usize)> = self
            .entries
            .iter()
            .filter(|(k, e)| {
                e.complete
                    && !self.pending.contains_key(k)
                    && (e.consumed || e.completed_at.is_none_or(|t| now.duration_since(t) >= self.fresh_for))
            })
            .map(|(k, e)| (e.last_used, *k, e.bytes))
            .collect();
        done.sort_unstable();
        let target = (self.max_bytes / 4 * 3).min(self.max_bytes.saturating_sub(incoming));
        for (_, k, b) in done {
            if self.bytes <= target {
                break;
            }
            self.entries.remove(&k);
            self.bytes -= b;
            self.stats.evicted += 1;
        }
    }

    fn cancel(&mut self, key: BlockKey, source_len: usize) {
        self.source_bytes -= source_len;
        self.pending.remove(&key);
        if let Some(e) = self.entries.remove(&key) {
            self.bytes -= e.bytes;
        }
        self.stats.cancelled += 1;
    }
}

struct Shared {
    state: Mutex<State>,
    generation: AtomicU64,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // a panicking worker must not take the UI down with a poisoned lock
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Builds a block's content (lines each followed by `\n`) on the worker thread.
pub type BlockSource = Box<dyn FnOnce() -> String + Send>;

struct Job {
    key: BlockKey,
    lang: String,
    /// Content length announced by the caller (reserved against the source budget).
    len: usize,
    source: Option<BlockSource>,
}

/// Background highlighter (see the module docs).
pub struct HighlightService {
    shared: Arc<Shared>,
    tx: SyncSender<Job>,
}

impl HighlightService {
    /// Starts the worker thread. `wake` is called (throttled) when results arrive.
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Self::with_cache_bytes(wake, DEFAULT_CACHE_BYTES)
    }

    pub fn with_cache_bytes(wake: Arc<dyn Fn() + Send + Sync>, max_bytes: usize) -> Result<Self, String> {
        Self::with_limits(wake, max_bytes, MAX_SOURCE_BYTES)
    }

    /// `max_bytes`: result cache budget; `max_source_bytes`: source bytes of queued and running
    /// jobs together.
    pub fn with_limits(
        wake: Arc<dyn Fn() + Send + Sync>,
        max_bytes: usize,
        max_source_bytes: usize,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                max_bytes,
                max_source_bytes,
                fresh_for: FRESH_FOR,
                ..Default::default()
            }),
            generation: AtomicU64::new(0),
            wake,
        });
        let (tx, rx) = sync_channel(MAX_QUEUED);
        let s = shared.clone();
        std::thread::Builder::new()
            .name("markise-highlight".into())
            .spawn(move || worker(s, rx))
            .map_err(|e| format!("cannot start the highlight worker: {e}"))?;
        Ok(HighlightService { shared, tx })
    }

    /// Current generation (requests made with an older one are cancelled).
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Cancels unfinished jobs requested with an older generation (document change).
    pub fn set_generation(&self, generation: u64) {
        self.shared.generation.store(generation, Ordering::Release);
    }

    /// Requests highlighting of block `key` (language `lang`, `len` content bytes) for the
    /// current generation. `source` builds the content (lines each followed by `\n`) and runs on
    /// the worker thread. No-op when the block is cached or already requested; `false` when the
    /// queue or the source-byte budget is full (ask again later). A block above
    /// [`MAX_BLOCK_SOURCE_BYTES`] gets a terminal plain result at once.
    pub fn request(&self, key: BlockKey, lang: &str, len: usize, source: BlockSource) -> bool {
        let generation = self.generation();
        {
            let mut st = self.shared.lock();
            if let Some(g) = st.pending.get_mut(&key) {
                *g = (*g).max(generation); // adopt the running job
                return true;
            }
            if st.entries.contains_key(&key) {
                return true;
            }
            st.stats.requests += 1;
            if len > MAX_BLOCK_SOURCE_BYTES {
                st.stats.truncated += 1;
                let tick = st.tick;
                st.entries.insert(
                    key,
                    Entry {
                        done: len.min(u32::MAX as usize) as u32,
                        complete: true,
                        truncated: true,
                        completed_at: Some(Instant::now()),
                        last_used: tick,
                        ..Default::default()
                    },
                );
                return true;
            }
            if st.source_bytes + len > st.max_source_bytes {
                st.stats.refused += 1;
                return false;
            }
            st.source_bytes += len;
            st.pending.insert(key, generation);
        }
        let job = Job {
            key,
            lang: lang.to_string(),
            len,
            source: Some(source),
        };
        match self.tx.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                let mut st = self.shared.lock();
                st.pending.remove(&key);
                st.source_bytes -= len;
                st.stats.refused += 1;
                false
            }
        }
    }

    /// [`request`](Self::request) with content already in hand (tests, tools).
    pub fn request_text(&self, key: BlockKey, lang: &str, text: String) -> bool {
        let len = text.len();
        self.request(key, lang, len, Box::new(move || text))
    }

    /// Whether block `key` got a truncated terminal result.
    pub fn truncated(&self, key: BlockKey) -> bool {
        self.shared.lock().entries.get(&key).is_some_and(|e| e.truncated)
    }

    /// Source bytes reserved by queued and running jobs.
    pub fn source_bytes(&self) -> usize {
        self.shared.lock().source_bytes
    }

    /// Whether `range` (content bytes) of block `key` is highlighted. No side effects besides
    /// recency.
    pub fn ready(&self, key: BlockKey, range: Range<usize>) -> bool {
        let mut st = self.shared.lock();
        st.tick += 1;
        let tick = st.tick;
        match st.entries.get_mut(&key) {
            Some(e) if e.complete || e.done as usize >= range.end => {
                e.last_used = tick;
                e.consumed |= e.complete;
                true
            }
            _ => false,
        }
    }

    /// The runs of block `key` that intersect `range` (content bytes), clipped to it.
    pub fn lookup(&self, key: BlockKey, range: Range<usize>) -> Lookup {
        let mut st = self.shared.lock();
        st.tick += 1;
        let tick = st.tick;
        let pending = st.pending.contains_key(&key);
        let Some(e) = st.entries.get_mut(&key) else {
            return if pending { Lookup::Pending } else { Lookup::Missing };
        };
        e.last_used = tick;
        if !(e.complete || e.done as usize >= range.end) {
            return Lookup::Pending;
        }
        e.consumed |= e.complete;
        let (a, b) = (range.start as u32, range.end as u32);
        let first = e.chunks.partition_point(|c| c.end <= a);
        let mut out = Vec::new();
        for c in &e.chunks[first..] {
            if c.start >= b {
                break;
            }
            for r in c.runs.iter() {
                if r.end > a && r.start < b {
                    out.push(TokenRun {
                        start: r.start.max(a),
                        end: r.end.min(b),
                        kind: r.kind,
                    });
                }
            }
        }
        Lookup::Ready(out)
    }

    pub fn stats(&self) -> ServiceStats {
        self.shared.lock().stats
    }

    /// Estimated bytes of cached runs.
    pub fn cache_bytes(&self) -> usize {
        self.shared.lock().bytes
    }

    /// Whether the worker has unfinished requests.
    pub fn busy(&self) -> bool {
        !self.shared.lock().pending.is_empty()
    }
}

struct Active {
    job: Job,
    text: Arc<str>,
    hl: Option<Highlighter>,
    started: bool,
}

fn worker(shared: Arc<Shared>, rx: Receiver<Job>) {
    let mut active: VecDeque<Active> = VecDeque::new();
    let mut last_wake = Instant::now() - WAKE_INTERVAL;
    let mut runs = Vec::new();
    loop {
        if active.is_empty() {
            match rx.recv() {
                Ok(j) => active.push_front(new_active(j)),
                Err(_) => return,
            }
        }
        loop {
            match rx.try_recv() {
                Ok(j) => active.push_front(new_active(j)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        while active.len() > MAX_ACTIVE {
            if let Some(a) = active.pop_back() {
                shared.lock().cancel(a.job.key, a.job.len);
                (shared.wake)();
            }
        }
        let Some(mut a) = active.pop_front() else { continue };
        let generation = shared.generation.load(Ordering::Acquire);
        {
            let mut st = shared.lock();
            match st.pending.get(&a.job.key) {
                Some(&g) if g >= generation => {}
                Some(_) => {
                    st.cancel(a.job.key, a.job.len);
                    drop(st);
                    // the UI may show a partial result of this block: let it re-check
                    (shared.wake)();
                    continue;
                }
                None => continue, // already cancelled
            }
        }
        if !a.started {
            a.started = true;
            if let Some(src) = a.job.source.take() {
                let mut t = src();
                // the announced length is what was reserved; never trust more
                if t.len() > a.job.len {
                    let mut cut = a.job.len;
                    while !t.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    t.truncate(cut);
                }
                a.text = Arc::from(t);
            }
            a.hl = Highlighter::new(resolve(&a.job.lang));
        }
        let text = a.text.clone();
        let start = a.hl.as_ref().map_or(text.len(), |h| h.pos());
        let mut end = start;
        runs.clear();
        let step = Instant::now();
        if let Some(h) = a.hl.as_mut() {
            let mut lines = 0;
            for line in text[start..].split_inclusive('\n') {
                h.line(line, &mut runs);
                lines += 1;
                if lines >= CHUNK_LINES || h.pos() - start >= CHUNK_BYTES || step.elapsed() >= CHUNK_TIME {
                    break;
                }
                // a single over-long line counts as a whole chunk
                if line.len() > MAX_LINE_BYTES {
                    break;
                }
            }
            end = h.pos();
            if h.degraded() {
                end = text.len(); // the rest is plain: done
            }
        }
        let mut complete = end >= text.len();
        {
            let mut st = shared.lock();
            st.stats.chunks += 1;
            st.stats.max_step = st.stats.max_step.max(step.elapsed());
            st.stats.bytes += (end - start) as u64;
            let bytes = runs.len() * std::mem::size_of::<TokenRun>() + std::mem::size_of::<Chunk>();
            let tick = st.tick;
            let max_block = st.max_bytes / 4;
            st.evict(bytes);
            let over_budget = st.bytes + bytes > st.max_bytes;
            let e = st.entries.entry(a.job.key).or_default();
            let mut added = 0;
            if e.bytes + bytes > max_block || over_budget {
                // bounded terminal result: what fits so far, plain for the rest
                e.truncated = true;
                e.done = text.len() as u32;
                complete = true;
            } else {
                if end > start || e.chunks.is_empty() {
                    e.chunks.push(Chunk {
                        start: start as u32,
                        end: end as u32,
                        runs: runs.as_slice().into(),
                    });
                    e.bytes += bytes;
                    added = bytes;
                }
                e.done = end as u32;
            }
            e.last_used = tick;
            e.complete = complete;
            let truncated = e.truncated;
            if complete {
                e.completed_at = Some(Instant::now());
            }
            st.bytes += added;
            if complete {
                st.pending.remove(&a.job.key);
                st.source_bytes -= a.job.len;
                st.stats.completed += 1;
                if truncated {
                    st.stats.truncated += 1;
                }
            }
        }
        if complete || last_wake.elapsed() >= WAKE_INTERVAL {
            last_wake = Instant::now();
            (shared.wake)();
        }
        if !complete {
            active.push_back(a);
        }
    }
}

fn new_active(job: Job) -> Active {
    Active {
        job,
        text: Arc::from(""),
        hl: None,
        started: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_theme::TokenKind;
    use std::sync::atomic::AtomicUsize;

    fn service() -> (HighlightService, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let s = HighlightService::new(Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }))
        .unwrap();
        (s, wakes)
    }

    fn wait(s: &HighlightService, key: BlockKey, range: Range<usize>) -> Vec<TokenRun> {
        let t = Instant::now();
        loop {
            if let Lookup::Ready(v) = s.lookup(key, range.clone()) {
                return v;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn highlights_off_thread_and_caches() {
        let (s, wakes) = service();
        let src = "fn main() {}\n";
        let key = block_key("rust", [src]);
        assert_eq!(s.lookup(key, 0..src.len()), Lookup::Missing);
        assert!(s.request_text(key, "rust", src.to_string()));
        let runs = wait(&s, key, 0..src.len());
        assert!(runs.iter().any(|r| r.kind == TokenKind::Keyword));
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        // second request: cache hit, no new job
        let before = s.stats();
        assert!(s.request(
            key,
            "rust",
            1,
            Box::new(|| panic!("text must not be built for a cached block"))
        ));
        assert_eq!(s.stats().requests, before.requests);
        // clipped lookup
        let part = wait(&s, key, 3..7);
        assert!(part.iter().all(|r| r.start >= 3 && r.end <= 7), "{part:?}");
    }

    #[test]
    fn plain_language_completes_empty() {
        let (s, _) = service();
        let key = block_key("cobol", ["x"]);
        s.request_text(key, "cobol", "x\n".to_string());
        assert_eq!(wait(&s, key, 0..2), Vec::new());
        assert!(s.ready(key, 0..2));
    }

    #[test]
    fn cancellation_on_generation_change() {
        let (s, _) = service();
        // a block large enough that it cannot finish before the generation changes
        let line = "let x = \"abc\"; // comment\n";
        let big: String = line.repeat(100_000);
        let key = block_key("rust", [big.as_str()]);
        s.request_text(key, "rust", big.clone());
        s.set_generation(1);
        let t = Instant::now();
        while s.busy() {
            assert!(t.elapsed() < Duration::from_secs(10), "cancelled job must stop");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(s.lookup(key, 0..10), Lookup::Missing, "partial result dropped");
        assert!(s.stats().cancelled >= 1);
        assert!(s.stats().bytes < big.len() as u64, "stopped early");
        // re-request in the new generation works
        let small = "fn a() {}\n";
        let k2 = block_key("rust", [small]);
        s.request_text(k2, "rust", small.to_string());
        assert!(!wait(&s, k2, 0..small.len()).is_empty());
    }

    #[test]
    fn newer_request_adopts_running_job() {
        let (s, _) = service();
        let big = "fn a() {}\n".repeat(5_000);
        let key = block_key("rust", [big.as_str()]);
        s.request_text(key, "rust", big.clone());
        s.set_generation(1);
        s.request(key, "rust", big.len(), Box::new(|| unreachable!("already running")));
        let runs = wait(&s, key, 0..big.len());
        let last = runs.last().expect("runs");
        assert!(last.end as usize > big.len() - 10, "highlighted to the end");
    }

    /// Huge block: highlighted incrementally; the first segment is ready long before the end;
    /// bounded time overall.
    #[test]
    fn huge_block_incremental_and_bounded() {
        let (s, _) = service();
        let unit = "pub fn f(x: u32) -> u32 { x * 2 + 1 } // c\n";
        let big = unit.repeat(5_000); // ~215 KB
        let key = block_key("rust", [big.as_str()]);
        let t = Instant::now();
        s.request_text(key, "rust", big.clone());
        wait(&s, key, 0..unit.len() * 64);
        let first = t.elapsed();
        let all = wait(&s, key, 0..big.len());
        let total = t.elapsed();
        assert!(!all.is_empty());
        assert!(
            first * 4 < total || total < Duration::from_millis(200),
            "first {first:?} total {total:?}"
        );
        // generous bound for debug builds on slow CI; release throughput is in benches/
        assert!(total < Duration::from_secs(60), "{total:?}");
        // bounded work per step (short lines: the time budget plus one line)
        let max = s.stats().max_step;
        assert!(max < Duration::from_millis(250), "max step {max:?}");
        assert!(s.stats().chunks >= 5_000 / CHUNK_LINES as u64, "{:?}", s.stats());
    }

    #[test]
    fn small_block_not_starved_by_huge_one() {
        let (s, _) = service();
        let big = "let a = 1; // x\n".repeat(100_000);
        let kb = block_key("rust", [big.as_str()]);
        s.request_text(kb, "rust", big.clone());
        std::thread::sleep(Duration::from_millis(20));
        let small = "def f(): pass\n";
        let ks = block_key("python", [small]);
        s.request_text(ks, "python", small.to_string());
        wait(&s, ks, 0..small.len());
        assert!(!s.ready(kb, 0..big.len()), "small block finished first");
    }

    #[test]
    fn cache_is_byte_bounded() {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let s = HighlightService::with_cache_bytes(wake, 64 * 1024).unwrap();
        let mut keys = Vec::new();
        for i in 0..40 {
            let src = format!("fn f{i}() {{ let x = {i}; }}\n").repeat(100);
            let k = block_key("rust", [src.as_str()]);
            s.request_text(k, "rust", src.clone());
            wait(&s, k, 0..src.len());
            keys.push(k);
        }
        assert!(s.cache_bytes() <= 64 * 1024, "{}", s.cache_bytes());
        assert!(s.stats().evicted > 0);
        assert_eq!(s.lookup(keys[0], 0..1), Lookup::Missing, "oldest evicted");
    }

    /// Review P1: a block whose runs exceed the per-block share of the budget gets a bounded
    /// terminal result (prefix highlighted, rest plain), stays within the budget and is not
    /// re-highlighted when requested again.
    #[test]
    fn oversized_result_is_terminal_and_bounded() {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let s = HighlightService::with_cache_bytes(wake, 64 * 1024).unwrap();
        let big = "let a = (1, 2, 3); // c\n".repeat(5_000);
        let key = block_key("rust", [big.as_str()]);
        s.request_text(key, "rust", big.clone());
        let runs = wait(&s, key, 0..big.len());
        assert!(s.truncated(key));
        assert!(!runs.is_empty(), "prefix highlighted");
        assert!(runs.last().unwrap().end < (big.len() / 2) as u32, "rest plain");
        assert!(s.cache_bytes() <= 64 * 1024, "{}", s.cache_bytes());
        assert_eq!(s.source_bytes(), 0, "reservation released");
        let before = s.stats();
        for _ in 0..3 {
            s.request(
                key,
                "rust",
                big.len(),
                Box::new(|| unreachable!("terminal result is cached")),
            );
        }
        assert_eq!(s.stats().requests, before.requests);
        assert!(!s.busy());
    }

    /// Blocks above MAX_BLOCK_SOURCE_BYTES: terminal plain result at once, content never built.
    #[test]
    fn huge_source_is_plain_without_building_it() {
        let (s, _) = service();
        let key = 42;
        assert!(s.request(
            key,
            "rust",
            MAX_BLOCK_SOURCE_BYTES + 1,
            Box::new(|| unreachable!("not built"))
        ));
        assert!(s.ready(key, 0..10));
        assert!(s.truncated(key));
        assert_eq!(s.lookup(key, 0..10), Lookup::Ready(Vec::new()));
        assert_eq!(s.source_bytes(), 0);
    }

    /// Review P1/P2: source bytes of queued and running jobs are reserved before admission;
    /// content is built on the worker thread.
    #[test]
    fn source_budget_and_worker_side_content() {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let s = HighlightService::with_limits(wake, DEFAULT_CACHE_BYTES, 1000).unwrap();
        let built_on = Arc::new(Mutex::new(String::new()));
        let b = built_on.clone();
        assert!(s.request(
            1,
            "rust",
            800,
            Box::new(move || {
                *b.lock().unwrap() = std::thread::current().name().unwrap_or("").to_string();
                std::thread::sleep(Duration::from_millis(300));
                "fn a() {}\n".repeat(80)
            })
        ));
        assert_eq!(s.source_bytes(), 800);
        assert!(
            !s.request_text(2, "rust", "x\n".repeat(150)),
            "over the source budget: refused"
        );
        assert!(s.stats().refused >= 1);
        assert!(s.request_text(3, "rust", "fn b() {}\n".to_string()), "fits");
        wait(&s, 1, 0..800);
        wait(&s, 3, 0..10);
        assert_eq!(&*built_on.lock().unwrap(), "markise-highlight");
        assert_eq!(s.source_bytes(), 0);
        assert!(
            s.request_text(2, "rust", "x\n".repeat(150)),
            "admitted once the budget is free"
        );
    }

    /// A source that returns more than it announced is cut to the announced length.
    #[test]
    fn source_longer_than_announced_is_cut() {
        let (s, _) = service();
        s.request(7, "rust", 10, Box::new(|| "fn a() {}\nfn b() {}\n".repeat(1000)));
        let runs = wait(&s, 7, 0..10);
        assert!(runs.iter().all(|r| r.end <= 10));
        assert_eq!(s.source_bytes(), 0);
    }

    /// Cancellation wakes the UI (it may show a partial result of the cancelled block).
    #[test]
    fn cancellation_wakes() {
        let (s, wakes) = service();
        let big = "let x = 1; // c\n".repeat(200_000);
        s.request_text(9, "rust", big);
        std::thread::sleep(Duration::from_millis(30));
        let before = wakes.load(Ordering::Relaxed);
        s.set_generation(5);
        let t = Instant::now();
        while s.busy() {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(20));
        assert!(wakes.load(Ordering::Relaxed) > before);
        assert_eq!(s.source_bytes(), 0);
    }

    /// Unread fresh results survive eviction pressure; the budget still holds.
    #[test]
    fn fresh_results_survive_until_read() {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let s = HighlightService::with_cache_bytes(wake, 64 * 1024).unwrap();
        // a long freshness window: the test must not depend on machine speed
        s.shared.lock().fresh_for = Duration::from_secs(600);
        let first = "fn first() { let x = 1; }\n".repeat(100);
        let k0 = block_key("rust", [first.as_str()]);
        s.request_text(k0, "rust", first.clone());
        while !s.shared.lock().entries.get(&k0).is_some_and(|e| e.complete) {
            std::thread::sleep(Duration::from_millis(2));
        }
        for i in 0..20 {
            let src = format!("fn f{i}() {{ let x = {i}; }}\n").repeat(100);
            let k = block_key("rust", [src.as_str()]);
            s.request_text(k, "rust", src.clone());
            wait(&s, k, 0..src.len());
        }
        assert!(s.cache_bytes() <= 64 * 1024);
        assert!(matches!(s.lookup(k0, 0..first.len()), Lookup::Ready(v) if !v.is_empty()));
    }
}
