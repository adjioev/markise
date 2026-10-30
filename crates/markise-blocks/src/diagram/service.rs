//! Background diagram rendering: one worker thread, a bounded LIFO queue (the most recently
//! requested — i.e. visible — diagram goes first), a byte-bounded LRU result cache.
//!
//! Results are keyed by [`diagram_key`] (language, source, font size, text metrics). They hold
//! color roles, not colors, so a color-scheme change reuses them; a width change only rescales
//! at paint time. Errors are cached too (a broken diagram is not re-parsed every frame), except
//! cancellations. [`DiagramService::set_generation`] drops queued jobs of older documents and
//! cancels the running one.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Cancel, DiagramError, DiagramLang, Limits, RenderCtx, Scene, TextMeasure, render};

pub type DiagramKey = u64;
pub type DiagramResult = Arc<Result<Scene, DiagramError>>;

/// Cache key of one diagram.
pub fn diagram_key(lang: DiagramLang, source: &str, font_size: f32, measure_id: u64) -> DiagramKey {
    let mut h = DefaultHasher::new();
    lang.hash(&mut h);
    source.hash(&mut h);
    font_size.to_bits().hash(&mut h);
    measure_id.hash(&mut h);
    h.finish()
}

#[derive(Clone, Debug)]
pub enum DiagramLookup {
    Ready(DiagramResult),
    /// Queued or rendering.
    Pending,
    /// Never requested, dropped or evicted.
    Missing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiagramStats {
    pub requests: u64,
    pub rendered: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub evicted: u64,
    pub dropped: u64,
    pub max_render: Duration,
}

/// Jobs waiting for the worker; beyond this the oldest is dropped (re-requested when shown).
pub const MAX_QUEUED: usize = 64;
pub const DEFAULT_CACHE_BYTES: usize = 32 << 20;

struct Job {
    key: DiagramKey,
    lang: DiagramLang,
    source: Box<dyn FnOnce() -> String + Send>,
    font_size: f32,
    generation: u64,
    /// Label metrics for this job (`None` = the service's default measurer).
    measure: Option<Arc<dyn TextMeasure>>,
}

struct Entry {
    result: DiagramResult,
    bytes: usize,
    last_used: u64,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    /// Queued or running keys → generation of their newest request.
    pending: HashMap<DiagramKey, u64>,
    entries: HashMap<DiagramKey, Entry>,
    bytes: usize,
    max_bytes: usize,
    tick: u64,
    generation: u64,
    running: Option<(DiagramKey, u64, Cancel)>,
    shutdown: bool,
    stats: DiagramStats,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct DiagramService {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    limits: Limits,
}

impl DiagramService {
    /// Starts the worker. `wake` is called (from the worker) when a result is ready.
    pub fn new(measure: Arc<dyn TextMeasure>, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Self::with_limits(measure, wake, Limits::default(), DEFAULT_CACHE_BYTES)
    }

    pub fn with_limits(
        measure: Arc<dyn TextMeasure>,
        wake: Arc<dyn Fn() + Send + Sync>,
        limits: Limits,
        cache_bytes: usize,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                max_bytes: cache_bytes,
                ..State::default()
            }),
            cv: Condvar::new(),
        });
        let s2 = shared.clone();
        let worker = std::thread::Builder::new()
            .name("markise-diagrams".into())
            .spawn(move || worker(s2, measure, wake, limits))
            .map_err(|e| format!("cannot start diagram worker: {e}"))?;
        Ok(DiagramService {
            shared,
            worker: Some(worker),
            limits,
        })
    }

    /// The cached result, or queues a render (copying `source`) and returns `Pending`.
    pub fn request(&self, key: DiagramKey, lang: DiagramLang, source: &str, font_size: f32) -> DiagramLookup {
        let owned = source.to_string();
        self.request_with(key, lang, source.len(), Box::new(move || owned), font_size, None)
    }

    /// Like [`DiagramService::request`], but the source (of `len` bytes) is built by `source` on
    /// the worker thread, so the caller does not copy the block (O(1) on the UI thread).
    /// `measure` overrides the service's label metrics for this job (its id must be part of
    /// `key`).
    pub fn request_with(
        &self,
        key: DiagramKey,
        lang: DiagramLang,
        len: usize,
        source: Box<dyn FnOnce() -> String + Send>,
        font_size: f32,
        measure: Option<Arc<dyn TextMeasure>>,
    ) -> DiagramLookup {
        let mut st = self.shared.lock();
        st.tick += 1;
        let tick = st.tick;
        if let Some(e) = st.entries.get_mut(&key) {
            e.last_used = tick;
            return DiagramLookup::Ready(e.result.clone());
        }
        match st.pending.get(&key) {
            Some(&g) if g >= st.generation => {
                // move it to the front: it is visible now
                if let Some(i) = st.queue.iter().position(|j| j.key == key)
                    && let Some(job) = st.queue.remove(i)
                {
                    st.queue.push_back(job);
                }
                return DiagramLookup::Pending;
            }
            Some(_) => {
                // a request of an older document (queued, or running and being cancelled):
                // replace it with a current one so the result is not lost with the old job
                st.queue.retain(|j| j.key != key);
                st.pending.remove(&key);
            }
            None => {}
        }
        st.stats.requests += 1;
        if len > self.limits.max_source_bytes {
            let r: DiagramResult = Arc::new(Err(DiagramError::TooLarge(format!("{} KB source", len / 1024))));
            insert(&mut st, key, r.clone(), 64);
            return DiagramLookup::Ready(r);
        }
        if st.queue.len() >= MAX_QUEUED
            && let Some(old) = st.queue.pop_front()
        {
            if st.pending.get(&old.key) == Some(&old.generation) {
                st.pending.remove(&old.key);
            }
            st.stats.dropped += 1;
        }
        let generation = st.generation;
        st.queue.push_back(Job {
            key,
            lang,
            source,
            font_size,
            generation,
            measure,
        });
        st.pending.insert(key, generation);
        drop(st);
        self.shared.cv.notify_one();
        DiagramLookup::Pending
    }

    pub fn lookup(&self, key: DiagramKey) -> DiagramLookup {
        let mut st = self.shared.lock();
        st.tick += 1;
        let tick = st.tick;
        if let Some(e) = st.entries.get_mut(&key) {
            e.last_used = tick;
            return DiagramLookup::Ready(e.result.clone());
        }
        if st.pending.contains_key(&key) {
            DiagramLookup::Pending
        } else {
            DiagramLookup::Missing
        }
    }

    /// Drops queued jobs of older generations and cancels an older running job.
    pub fn set_generation(&self, generation: u64) {
        let mut st = self.shared.lock();
        st.generation = generation;
        let (keep, drop): (VecDeque<Job>, VecDeque<Job>) = st.queue.drain(..).partition(|j| j.generation >= generation);
        st.queue = keep;
        for j in drop {
            if st.pending.get(&j.key) == Some(&j.generation) {
                st.pending.remove(&j.key);
            }
            st.stats.dropped += 1;
        }
        if let Some((_, g, c)) = &st.running
            && *g < generation
        {
            c.cancel();
        }
    }

    pub fn stats(&self) -> DiagramStats {
        self.shared.lock().stats
    }

    pub fn cache_bytes(&self) -> usize {
        self.shared.lock().bytes
    }

    /// Whether jobs are queued or running.
    pub fn busy(&self) -> bool {
        let st = self.shared.lock();
        !st.queue.is_empty() || st.running.is_some()
    }
}

impl Drop for DiagramService {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            if let Some((_, _, c)) = &st.running {
                c.cancel();
            }
        }
        self.shared.cv.notify_all();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn insert(st: &mut State, key: DiagramKey, result: DiagramResult, bytes: usize) {
    st.tick += 1;
    let tick = st.tick;
    // evict least recently used until it fits
    while st.bytes + bytes > st.max_bytes && !st.entries.is_empty() {
        let Some((&k, _)) = st.entries.iter().min_by_key(|(_, e)| e.last_used) else {
            break;
        };
        if let Some(e) = st.entries.remove(&k) {
            st.bytes -= e.bytes;
            st.stats.evicted += 1;
        }
    }
    if let Some(old) = st.entries.insert(
        key,
        Entry {
            result,
            bytes,
            last_used: tick,
        },
    ) {
        st.bytes -= old.bytes;
    }
    st.bytes += bytes;
}

fn worker(shared: Arc<Shared>, measure: Arc<dyn TextMeasure>, wake: Arc<dyn Fn() + Send + Sync>, limits: Limits) {
    loop {
        let (job, cancel) = {
            let mut st = shared.lock();
            loop {
                if st.shutdown {
                    return;
                }
                if let Some(job) = st.queue.pop_back() {
                    let c = Cancel::new();
                    st.running = Some((job.key, job.generation, c.clone()));
                    break (job, c);
                }
                st = shared.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        let t = Instant::now();
        let m: &dyn TextMeasure = job.measure.as_deref().unwrap_or(measure.as_ref());
        let ctx = RenderCtx::new(m, job.font_size)
            .with_limits(limits)
            .with_cancel(&cancel);
        let text = (job.source)();
        let result = render(job.lang, &text, &ctx);
        let dt = t.elapsed();
        // the UI may see this job as finished from here on: count it until its wake returned
        let published = crate::wake_track::publishing();
        let mut st = shared.lock();
        st.running = None;
        // a newer request for the same key (current document) keeps its own pending entry
        let superseded = st.pending.get(&job.key).is_some_and(|&g| g != job.generation);
        if !superseded {
            st.pending.remove(&job.key);
        }
        st.stats.max_render = st.stats.max_render.max(dt);
        match &result {
            Err(DiagramError::Cancelled) => {
                st.stats.cancelled += 1;
                drop(st);
                // whoever still shows this diagram asks again on the next frame
                wake();
                drop(published);
                continue;
            }
            Ok(_) => st.stats.rendered += 1,
            Err(_) => st.stats.failed += 1,
        }
        if superseded {
            // the result is a function of the key: the newer request is answered too
            st.queue.retain(|j| j.key != job.key);
            st.pending.remove(&job.key);
        }
        let bytes = result.as_ref().map_or(64, |s| s.heap_size() + 64);
        insert(&mut st, job.key, Arc::new(result), bytes);
        drop(st);
        wake();
        drop(published);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::ApproxMeasure;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};

    fn wait_ready(s: &DiagramService, key: DiagramKey) -> DiagramResult {
        let t = Instant::now();
        loop {
            if let DiagramLookup::Ready(r) = s.lookup(key) {
                return r;
            }
            assert!(t.elapsed() < crate::TEST_HANG_GUARD, "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn renders_in_background_and_caches() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w2 = wakes.clone();
        let s = DiagramService::new(
            Arc::new(ApproxMeasure),
            Arc::new(move || {
                w2.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        let src = "graph TD\nA-->B";
        let key = diagram_key(DiagramLang::Mermaid, src, 14.0, ApproxMeasure.id());
        assert!(matches!(s.lookup(key), DiagramLookup::Missing));
        let _ = s.request(key, DiagramLang::Mermaid, src, 14.0);
        let r = wait_ready(&s, key);
        assert!(r.is_ok());
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        assert!(matches!(
            s.request(key, DiagramLang::Mermaid, src, 14.0),
            DiagramLookup::Ready(_)
        ));
        assert_eq!(s.stats().rendered, 1);
        // errors are cached as results
        let bad = "graph TD\nA-->";
        let k2 = diagram_key(DiagramLang::Mermaid, bad, 14.0, ApproxMeasure.id());
        let _ = s.request(k2, DiagramLang::Mermaid, bad, 14.0);
        assert!(wait_ready(&s, k2).is_err());
    }

    /// Codex P2: a reload re-requests a diagram whose previous-generation render is being
    /// cancelled; the request must not be swallowed and the UI must be woken.
    #[test]
    fn rerequest_during_cancellation_is_not_lost() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w2 = wakes.clone();
        let s = DiagramService::new(
            Arc::new(ApproxMeasure),
            Arc::new(move || {
                w2.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        // a slow job: the source builder blocks until released
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g2 = gate.clone();
        let src = "graph TD\nA-->B\n".to_string();
        let src2 = src.clone();
        let key = 42;
        let _ = s.request_with(
            key,
            DiagramLang::Mermaid,
            src.len(),
            Box::new(move || {
                let (m, cv) = &*g2;
                let mut open = m.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                src2
            }),
            14.0,
            None,
        );
        // wait until the worker runs it
        let t = Instant::now();
        while !s.shared.lock().running.is_some() {
            assert!(t.elapsed() < crate::TEST_HANG_GUARD);
            std::thread::sleep(Duration::from_millis(1));
        }
        // document reload: cancel, then the same diagram is requested again
        s.set_generation(1);
        assert!(matches!(
            s.request(key, DiagramLang::Mermaid, &src, 14.0),
            DiagramLookup::Pending
        ));
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        let r = wait_ready(&s, key);
        assert!(r.is_ok(), "re-requested diagram rendered: {r:?}");
        assert!(wakes.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn cancellation_wakes_the_ui() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w2 = wakes.clone();
        let s = DiagramService::new(
            Arc::new(ApproxMeasure),
            Arc::new(move || {
                w2.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
        let src = format!(
            "graph TD\n{}",
            (0..900)
                .map(|j| format!("n{j}-->n{}\n", (j * 7 + 3) % 900))
                .collect::<String>()
        );
        let _ = s.request(7, DiagramLang::Mermaid, &src, 14.0);
        let t = Instant::now();
        while !s.shared.lock().running.is_some() {
            assert!(t.elapsed() < crate::TEST_HANG_GUARD);
            std::thread::sleep(Duration::from_millis(1));
        }
        s.set_generation(1);
        while s.busy() {
            assert!(t.elapsed() < crate::TEST_HANG_GUARD);
            std::thread::sleep(Duration::from_millis(1));
        }
        let st = s.stats();
        if st.cancelled == 1 {
            assert!(wakes.load(Ordering::Relaxed) >= 1, "cancellation must wake the UI");
        }
    }

    #[test]
    fn cache_is_bounded() {
        let s =
            DiagramService::with_limits(Arc::new(ApproxMeasure), Arc::new(|| {}), Limits::default(), 20_000).unwrap();
        let mut keys = Vec::new();
        for i in 0..20 {
            let src = format!("graph LR\nA{i}-->B{i}-->C{i}");
            let k = diagram_key(DiagramLang::Mermaid, &src, 14.0, 1);
            let _ = s.request(k, DiagramLang::Mermaid, &src, 14.0);
            wait_ready(&s, k);
            keys.push(k);
        }
        assert!(s.cache_bytes() <= 20_000);
        assert!(s.stats().evicted > 0);
    }

    #[test]
    fn generation_drops_queued_jobs() {
        let s = DiagramService::new(Arc::new(ApproxMeasure), Arc::new(|| {})).unwrap();
        for i in 0..10 {
            let src = format!(
                "graph TD\n{}",
                (0..300)
                    .map(|j| format!("n{j}-->n{}\n", (j * 7 + i) % 300))
                    .collect::<String>()
            );
            let k = diagram_key(DiagramLang::Mermaid, &src, 14.0, 1);
            let _ = s.request(k, DiagramLang::Mermaid, &src, 14.0);
        }
        s.set_generation(1);
        let t = Instant::now();
        while s.busy() {
            assert!(t.elapsed() < crate::TEST_HANG_GUARD);
            std::thread::sleep(Duration::from_millis(2));
        }
        let st = s.stats();
        assert!(st.dropped + st.cancelled >= 8, "{st:?}");
    }
}
