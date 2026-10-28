//! Background math rendering: worker thread(s), a bounded LIFO job queue, a small metrics map
//! and a byte-bounded bitmap cache.
//!
//! Geometry is kept apart from pixels (review of PR #15):
//! - **Metrics** — a formula's natural box (width / height / depth in em) or its error, keyed by
//!   (TeX, mode). Tiny, never evicted together with bitmaps (bounded by entry count only).
//! - **Bitmaps** — rasterised only at the size they are shown: a [`MathRequest`] carries the
//!   column width as `max_width`; when the natural width at `px_per_em` fits, the bitmap is the
//!   natural one and its key does not depend on the column at all; when it does not fit, the
//!   bitmap is rendered at the column width rounded down to [`WIDTH_BUCKET`], so resizing only
//!   misses the cache when a bucket boundary is crossed. A full-size bitmap of a formula that is
//!   shown narrower is never rendered.
//!
//! A request whose metrics are unknown queues one job; the worker lays the formula out (~30 µs),
//! records the metrics and rasterises the request's effective size in the same job.
//!
//! - The queue is served newest-first: the formulas requested last are the ones on screen.
//!   It is bounded ([`MAX_QUEUED`]); the oldest requests fall off and are asked again when they
//!   become visible again. [`MathService::clear_queue`] drops all queued work (document switch).
//! - Errors live in the metrics map, so invalid TeX is not re-parsed every frame.
//! - The wake callback (repaint) runs after every finished job.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::raster::{MathImage, fit, rasterize};
use super::{MathError, MathLayout, MathMode, Rgba8, layout};

/// Cache key of a rendered bitmap.
pub type MathKey = u64;

/// Requests waiting for a worker.
pub const MAX_QUEUED: usize = 512;
/// Default bitmap-cache budget (bytes).
pub const DEFAULT_CACHE_BYTES: usize = 64 << 20;
/// Formulas whose metrics are kept; beyond this the map is cleared (metrics are cheap to redo).
pub const MAX_METRICS: usize = 100_000;
/// Width buckets (pixels) for formulas rendered narrower than their natural size.
pub const WIDTH_BUCKET: f32 = 32.0;

/// One formula to show.
#[derive(Clone, Debug, PartialEq)]
pub struct MathRequest {
    pub tex: String,
    pub mode: MathMode,
    /// Pixels per math em (text size × math scale × display scale).
    pub px_per_em: f32,
    /// Color of items without an explicit TeX color.
    pub color: Rgba8,
    /// The widest the formula may be shown, in pixels (its column or table cell); a wider
    /// formula is rendered scaled down to it (bucketed, see the module docs).
    pub max_width: Option<f32>,
}

/// The widest bucketed width not above `limit` pixels (at least 1).
pub fn width_bucket(limit: f32) -> f32 {
    if limit >= 2.0 * WIDTH_BUCKET {
        (limit / WIDTH_BUCKET).floor() * WIDTH_BUCKET
    } else {
        limit.floor().max(1.0)
    }
}

/// The width a formula of natural width `width_em` is rendered at for `req`: `None` when it
/// fits (natural size), else the bucketed column width.
fn effective_width(req: &MathRequest, width_em: f64) -> Option<f32> {
    let limit = req.max_width.filter(|m| m.is_finite() && *m > 0.0)?;
    let natural = width_em * req.px_per_em as f64;
    (natural > limit as f64 + 0.5).then(|| width_bucket(limit))
}

/// The key of a bitmap: the request fields plus the *effective* width (`None` when natural).
pub fn key_of(tex: &str, mode: MathMode, px_per_em: f32, color: Rgba8, width: Option<f32>) -> MathKey {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tex.hash(&mut h);
    mode.hash(&mut h);
    px_per_em.to_bits().hash(&mut h);
    color.hash(&mut h);
    width.map(f32::to_bits).hash(&mut h);
    h.finish()
}

fn metrics_key(tex: &str, mode: MathMode) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "metrics".hash(&mut h);
    tex.hash(&mut h);
    mode.hash(&mut h);
    h.finish()
}

/// Result of [`MathService::measure`]: the shown size in pixels, no bitmap.
#[derive(Clone, Debug)]
pub enum MathMeasure {
    Ready { width: f32, ascent: f32, descent: f32 },
    Empty,
    Failed(Arc<MathError>),
    Pending,
}

/// Result of [`MathService::get`].
#[derive(Clone, Debug)]
pub enum MathImageLookup {
    /// The bitmap; [`MathImage::key`] names it.
    Ready(Arc<MathImage>),
    /// The formula is empty (nothing to draw).
    Empty,
    /// Invalid TeX (or too large to draw): show the source with an error style.
    Failed(Arc<MathError>),
    /// Queued or rendering; ask again after the next wake.
    Pending,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MathServiceStats {
    /// Jobs queued.
    pub requests: u64,
    pub rendered: u64,
    pub failed: u64,
    pub dropped: u64,
    pub evicted: u64,
    /// Longest single job.
    pub max_job: Duration,
    /// Jobs that panicked (reported as errors).
    pub panicked: u64,
    /// Worker threads restarted after dying.
    pub respawned: u64,
}

/// A formula's geometry (or why it has none).
#[derive(Clone, Debug)]
enum Metrics {
    Box { width: f64, height: f64, depth: f64 },
    Empty,
    Failed(Arc<MathError>),
}

#[derive(Clone)]
enum Outcome {
    Image(Arc<MathImage>),
    Failed(Arc<MathError>),
}

impl Outcome {
    fn bytes(&self) -> usize {
        match self {
            Outcome::Image(i) => i.approx_bytes(),
            Outcome::Failed(e) => 64 + e.message.len(),
        }
    }
}

struct Entry {
    outcome: Outcome,
    last_used: u64,
}

struct Job {
    /// Queue identity: the bitmap key, or the metrics key while the metrics are unknown.
    id: u64,
    req: MathRequest,
    /// Rasterise too (`false`: metrics only, for measuring).
    pixels: bool,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    queued: HashSet<u64>,
    running: HashSet<u64>,
    metrics: HashMap<u64, Metrics>,
    entries: HashMap<MathKey, Entry>,
    bytes: usize,
    max_bytes: usize,
    tick: u64,
    stats: MathServiceStats,
    shutdown: bool,
}

impl State {
    fn evict(&mut self, incoming: usize) {
        if self.bytes + incoming <= self.max_bytes {
            return;
        }
        let target = (self.max_bytes * 3 / 4).saturating_sub(incoming);
        let mut by_age: Vec<(u64, MathKey)> = self.entries.iter().map(|(k, e)| (e.last_used, *k)).collect();
        by_age.sort_unstable();
        for (_, k) in by_age {
            if self.bytes <= target {
                break;
            }
            if let Some(e) = self.entries.remove(&k) {
                self.bytes -= e.outcome.bytes();
                self.stats.evicted += 1;
            }
        }
    }

    fn insert(&mut self, key: MathKey, outcome: Outcome) {
        let b = outcome.bytes();
        self.evict(b);
        self.tick += 1;
        let tick = self.tick;
        self.bytes += b;
        if let Some(old) = self.entries.insert(
            key,
            Entry {
                outcome,
                last_used: tick,
            },
        ) {
            self.bytes -= old.outcome.bytes();
        }
    }

    fn set_metrics(&mut self, key: u64, m: Metrics) {
        if self.metrics.len() >= MAX_METRICS {
            self.metrics.clear();
        }
        self.metrics.insert(key, m);
    }

    /// Queues `req` under `id` (once; a queued job moves to the back: it is visible now).
    fn enqueue(&mut self, id: u64, req: &MathRequest, pixels: bool) -> bool {
        if self.running.contains(&id) {
            return false;
        }
        if self.queued.contains(&id) {
            if let Some(pos) = self.queue.iter().position(|j| j.id == id) {
                let mut job = self.queue.remove(pos).expect("position is valid");
                job.pixels |= pixels;
                if pixels {
                    job.req = req.clone();
                }
                self.queue.push_back(job);
            }
            return false;
        }
        self.stats.requests += 1;
        if self.queue.len() >= MAX_QUEUED
            && let Some(old) = self.queue.pop_front()
        {
            self.queued.remove(&old.id);
            self.stats.dropped += 1;
        }
        self.queue.push_back(Job {
            id,
            req: req.clone(),
            pixels,
        });
        self.queued.insert(id);
        true
    }
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Renders formulas off the UI thread.
pub struct MathService {
    shared: Arc<Shared>,
}

impl MathService {
    /// One worker thread and the default cache budget.
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Self::with_limits(wake, 1, DEFAULT_CACHE_BYTES)
    }

    pub fn with_limits(wake: Arc<dyn Fn() + Send + Sync>, threads: usize, max_bytes: usize) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                max_bytes,
                ..Default::default()
            }),
            cv: Condvar::new(),
            wake,
        });
        for _ in 0..threads.max(1) {
            spawn_worker(shared.clone())?;
        }
        Ok(MathService { shared })
    }

    /// The bitmap for `req`, or `Pending` after queueing the work it needs (once).
    pub fn get(&self, req: &MathRequest) -> MathImageLookup {
        let mk = metrics_key(&req.tex, req.mode);
        let mut st = self.shared.lock();
        st.tick += 1;
        let tick = st.tick;
        let width = match st.metrics.get(&mk) {
            Some(Metrics::Empty) => return MathImageLookup::Empty,
            Some(Metrics::Failed(e)) => return MathImageLookup::Failed(e.clone()),
            Some(Metrics::Box { width, .. }) => *width,
            None => {
                let queued = st.enqueue(mk, req, true);
                drop(st);
                if queued {
                    self.shared.cv.notify_one();
                }
                return MathImageLookup::Pending;
            }
        };
        let key = key_of(
            &req.tex,
            req.mode,
            req.px_per_em,
            req.color,
            effective_width(req, width),
        );
        if let Some(e) = st.entries.get_mut(&key) {
            e.last_used = tick;
            return match &e.outcome {
                Outcome::Image(i) => MathImageLookup::Ready(i.clone()),
                Outcome::Failed(f) => MathImageLookup::Failed(f.clone()),
            };
        }
        let queued = st.enqueue(key, req, true);
        drop(st);
        if queued {
            self.shared.cv.notify_one();
        }
        MathImageLookup::Pending
    }

    /// The size `req` would be shown at, without rendering pixels (natural-width measuring,
    /// e.g. of table columns); queues a layout-only job when the metrics are unknown.
    pub fn measure(&self, req: &MathRequest) -> MathMeasure {
        let mk = metrics_key(&req.tex, req.mode);
        let mut st = self.shared.lock();
        match st.metrics.get(&mk) {
            Some(Metrics::Empty) => MathMeasure::Empty,
            Some(Metrics::Failed(e)) => MathMeasure::Failed(e.clone()),
            Some(Metrics::Box { width, height, depth }) => {
                // the rasteriser's own sizing (column width, max side, pixel area)
                match fit(*width, *height, *depth, req.px_per_em, effective_width(req, *width)) {
                    Some(f) => {
                        let ppe = f.px_per_em as f64;
                        MathMeasure::Ready {
                            width: (width.max(0.0) * ppe) as f32,
                            ascent: (height.max(0.0) * ppe) as f32,
                            descent: (depth.max(0.0) * ppe) as f32,
                        }
                    }
                    None => MathMeasure::Failed(Arc::new(MathError {
                        message: "formula too large to draw".into(),
                        offset: None,
                    })),
                }
            }
            None => {
                let queued = st.enqueue(mk, req, false);
                drop(st);
                if queued {
                    self.shared.cv.notify_one();
                }
                MathMeasure::Pending
            }
        }
    }

    /// Only the bitmap cache, never queues (tests, diagnostics).
    pub fn peek(&self, key: MathKey) -> Option<MathImageLookup> {
        let st = self.shared.lock();
        st.entries.get(&key).map(|e| match &e.outcome {
            Outcome::Image(i) => MathImageLookup::Ready(i.clone()),
            Outcome::Failed(f) => MathImageLookup::Failed(f.clone()),
        })
    }

    /// Drops all queued (not yet running) requests.
    pub fn clear_queue(&self) {
        let mut st = self.shared.lock();
        let n = st.queue.len() as u64;
        st.queue.clear();
        st.queued.clear();
        st.stats.dropped += n;
    }

    /// Whether work is queued or running.
    pub fn busy(&self) -> bool {
        let st = self.shared.lock();
        !st.queue.is_empty() || !st.running.is_empty()
    }

    pub fn stats(&self) -> MathServiceStats {
        self.shared.lock().stats
    }

    /// Bytes of cached bitmaps.
    pub fn cache_bytes(&self) -> usize {
        self.shared.lock().bytes
    }

    /// Formulas with known metrics.
    pub fn metrics_len(&self) -> usize {
        self.shared.lock().metrics.len()
    }
}

impl Drop for MathService {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

/// Test hooks: a job with this TeX panics inside rendering / kills its worker thread.
#[cfg(test)]
const PANIC_IN_RENDER: &str = "\u{0}panic-in-render";
#[cfg(test)]
const KILL_WORKER: &str = "\u{0}kill-worker";

fn internal_error() -> Arc<MathError> {
    Arc::new(MathError {
        message: "internal error while rendering this formula".into(),
        offset: None,
    })
}

/// What one job produces: the metrics, and the bitmap (key, outcome) when there is one.
type JobResult = (Metrics, Option<(MathKey, Outcome)>);

fn render(req: &MathRequest, pixels: bool) -> JobResult {
    // layout guards its own panics; rasterisation and anything else of the job is guarded here,
    // so one bad formula never takes the worker down
    let job = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        if req.tex == PANIC_IN_RENDER {
            panic!("test: panic in render");
        }
        let l: MathLayout = match layout(&req.tex, req.mode) {
            Err(e) => return (Metrics::Failed(Arc::new(e)), None),
            Ok(l) => l,
        };
        if l.items.is_empty() {
            return (Metrics::Empty, None);
        }
        let metrics = Metrics::Box {
            width: l.width,
            height: l.height,
            depth: l.depth,
        };
        if !pixels {
            return (metrics, None);
        }
        let width = effective_width(req, l.width);
        let key = key_of(&req.tex, req.mode, req.px_per_em, req.color, width);
        let outcome = match rasterize(&l, req.px_per_em, req.color, width) {
            Some(mut img) => {
                img.key = key;
                Outcome::Image(Arc::new(img))
            }
            None => Outcome::Failed(Arc::new(MathError {
                message: "formula too large to draw".into(),
                offset: None,
            })),
        };
        (metrics, Some((key, outcome)))
    }));
    job.unwrap_or_else(|_| (Metrics::Failed(internal_error()), None))
}

fn spawn_worker(s: Arc<Shared>) -> Result<(), String> {
    std::thread::Builder::new()
        .name("markise-math".into())
        .spawn(move || worker(s))
        .map(|_| ())
        .map_err(|e| format!("cannot start the math worker: {e}"))
}

/// Self-healing: if a worker thread unwinds anyway, its running job is reported as failed, the
/// UI is woken and a replacement worker is started, so later formulas never stay pending.
struct WorkerGuard {
    shared: Arc<Shared>,
    job: Option<(u64, u64)>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let shutdown = {
            let mut st = self.shared.lock();
            if let Some((id, mk)) = self.job.take() {
                st.running.remove(&id);
                st.set_metrics(mk, Metrics::Failed(internal_error()));
            }
            st.stats.respawned += 1;
            st.shutdown
        };
        if !shutdown {
            let _ = spawn_worker(self.shared.clone());
        }
        (self.shared.wake)();
    }
}

fn worker(s: Arc<Shared>) {
    let mut guard = WorkerGuard {
        shared: s.clone(),
        job: None,
    };
    loop {
        let job = {
            let mut st = s.lock();
            loop {
                if st.shutdown {
                    return;
                }
                if let Some(job) = st.queue.pop_back() {
                    st.queued.remove(&job.id);
                    st.running.insert(job.id);
                    break job;
                }
                st = s.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        let mk = metrics_key(&job.req.tex, job.req.mode);
        guard.job = Some((job.id, mk));
        #[cfg(test)]
        if job.req.tex == KILL_WORKER {
            panic!("test: worker dies");
        }
        let t = Instant::now();
        let (metrics, bitmap) = render(&job.req, job.pixels);
        let took = t.elapsed();
        {
            let mut st = s.lock();
            st.running.remove(&job.id);
            st.stats.max_job = st.stats.max_job.max(took);
            match &metrics {
                Metrics::Failed(e) => {
                    st.stats.failed += 1;
                    if e.message.starts_with("internal error") {
                        st.stats.panicked += 1;
                    }
                }
                _ => st.stats.rendered += 1,
            }
            st.set_metrics(mk, metrics);
            if let Some((key, outcome)) = bitmap {
                st.insert(key, outcome);
            }
        }
        guard.job = None;
        (s.wake)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req(tex: &str) -> MathRequest {
        MathRequest {
            tex: tex.into(),
            mode: MathMode::Inline,
            px_per_em: 20.0,
            color: [0, 0, 0, 255],
            max_width: None,
        }
    }

    fn wait(svc: &MathService, r: &MathRequest) -> MathImageLookup {
        let t = Instant::now();
        loop {
            match svc.get(r) {
                MathImageLookup::Pending if t.elapsed() < Duration::from_secs(20) => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                other => return other,
            }
        }
    }

    #[test]
    fn renders_caches_and_wakes() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let svc = MathService::new(Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }))
        .unwrap();
        let r = req("x^2");
        assert!(matches!(
            svc.get(&r),
            MathImageLookup::Pending | MathImageLookup::Ready(_)
        ));
        let MathImageLookup::Ready(img) = wait(&svc, &r) else {
            panic!("not ready")
        };
        assert!(img.size[0] > 0);
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        // a second lookup is a cache hit; one job did metrics and pixels
        assert!(matches!(svc.get(&r), MathImageLookup::Ready(_)));
        assert_eq!(svc.stats().rendered, 1);
        assert_eq!(svc.stats().requests, 1);
        assert!(svc.peek(img.key).is_some());
    }

    #[test]
    fn errors_and_empty_are_cached() {
        let svc = MathService::new(Arc::new(|| {})).unwrap();
        assert!(matches!(wait(&svc, &req(r"\frac{1}{")), MathImageLookup::Failed(_)));
        assert!(matches!(wait(&svc, &req("")), MathImageLookup::Empty));
        assert_eq!(svc.stats().failed, 1);
        let n = svc.stats().requests;
        assert!(matches!(svc.get(&req(r"\frac{1}{")), MathImageLookup::Failed(_)));
        assert_eq!(svc.stats().requests, n, "errors are not re-queued");
    }

    #[test]
    fn cache_is_bounded() {
        let svc = MathService::with_limits(Arc::new(|| {}), 1, 64 * 1024).unwrap();
        for i in 0..40 {
            let _ = wait(&svc, &req(&format!(r"\sum_{{i={i}}}^n x_i^{{{i}}}")));
        }
        assert!(svc.cache_bytes() <= 64 * 1024 + 32 * 1024, "{}", svc.cache_bytes());
        assert!(svc.stats().evicted > 0);
        // metrics survive bitmap eviction
        assert_eq!(svc.metrics_len(), 40);
    }

    #[test]
    fn huge_formula_fails_and_the_worker_survives() {
        // review P2 #1: used to overflow in the rasteriser and kill the only worker
        let svc = MathService::new(Arc::new(|| {})).unwrap();
        let big = req(r"\rule{100000000000000000000em}{1em}");
        assert!(matches!(wait(&svc, &big), MathImageLookup::Failed(_)));
        assert!(matches!(wait(&svc, &req("x+1")), MathImageLookup::Ready(_)));
    }

    #[test]
    fn panicking_job_is_an_error_and_the_service_continues() {
        let svc = MathService::new(Arc::new(|| {})).unwrap();
        let MathImageLookup::Failed(e) = wait(&svc, &req(PANIC_IN_RENDER)) else {
            panic!("not failed")
        };
        assert!(e.message.contains("internal error"));
        assert_eq!(svc.stats().panicked, 1);
        assert!(matches!(wait(&svc, &req("y")), MathImageLookup::Ready(_)));
    }

    #[test]
    fn dead_worker_is_replaced_and_its_job_fails() {
        // Issue #33: the dying worker publishes the failure (under the lock) *before* it starts
        // the replacement and calls `wake`, so polling `get` until `Failed` and then asserting
        // that a wake already happened raced. Instead, synchronise on the wake itself: every
        // finished (or killed) job sends exactly one message, and after it the job's result is
        // guaranteed to be visible. The timeout only turns a hang into a failure.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let svc = MathService::new(Arc::new(move || {
            let _ = tx.send(());
        }))
        .unwrap();
        let woken = || {
            rx.recv_timeout(Duration::from_secs(120))
                .expect("the UI is woken after the job ends")
        };
        let kill = req(KILL_WORKER);
        assert!(matches!(svc.get(&kill), MathImageLookup::Pending));
        woken();
        assert!(matches!(svc.get(&kill), MathImageLookup::Failed(_)));
        assert_eq!(svc.stats().respawned, 1);
        // the replacement worker serves later formulas
        let z = req("z^2");
        assert!(matches!(svc.get(&z), MathImageLookup::Pending));
        woken();
        assert!(matches!(svc.get(&z), MathImageLookup::Ready(_)));
        assert!(!svc.busy());
        assert!(rx.try_recv().is_err(), "one wake per job");
    }

    #[test]
    fn key_depends_on_every_field() {
        let k = |tex: &str, mode, ppe, color, w| key_of(tex, mode, ppe, color, w);
        let c = [0, 0, 0, 255];
        let keys: HashSet<_> = [
            k("x", MathMode::Inline, 20.0, c, None),
            k("x", MathMode::Display, 20.0, c, None),
            k("x", MathMode::Inline, 21.0, c, None),
            k("x", MathMode::Inline, 20.0, [1, 0, 0, 255], None),
            k("x", MathMode::Inline, 20.0, c, Some(300.0)),
        ]
        .into_iter()
        .collect();
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn fitting_formula_ignores_the_column_and_wide_one_uses_buckets() {
        let svc = MathService::new(Arc::new(|| {})).unwrap();
        let at = |tex: &str, col: f32| {
            let mut r = req(tex);
            r.max_width = Some(col);
            r
        };
        // fits: the same bitmap whatever the column width
        let MathImageLookup::Ready(a) = wait(&svc, &at("x+1", 600.0)) else {
            panic!()
        };
        let MathImageLookup::Ready(b) = wait(&svc, &at("x+1", 601.0)) else {
            panic!()
        };
        assert_eq!(a.key, b.key);
        // wider than the column: rendered at the bucket, shared by nearby widths
        let wide = r"\rule{100em}{1em}";
        let MathImageLookup::Ready(c) = wait(&svc, &at(wide, 600.0)) else {
            panic!()
        };
        let MathImageLookup::Ready(d) = wait(&svc, &at(wide, 601.0)) else {
            panic!()
        };
        assert_eq!(c.key, d.key);
        assert!(c.width <= 600.0 && c.width >= 600.0 - WIDTH_BUCKET - 1.0, "{}", c.width);
        let MathImageLookup::Ready(e) = wait(&svc, &at(wide, 700.0)) else {
            panic!()
        };
        assert_ne!(c.key, e.key);
        assert_eq!(width_bucket(40.0), 40.0);
        assert_eq!(width_bucket(601.0), 576.0);
    }

    #[test]
    fn measure_renders_no_pixels() {
        let svc = MathService::new(Arc::new(|| {})).unwrap();
        let mut r = req(r"\rule{100em}{2em}");
        r.max_width = Some(400.0);
        let t = Instant::now();
        let (w, a) = loop {
            match svc.measure(&r) {
                MathMeasure::Ready { width, ascent, .. } => break (width, ascent),
                MathMeasure::Pending => {
                    assert!(t.elapsed() < Duration::from_secs(20));
                    std::thread::sleep(Duration::from_millis(2));
                }
                other => panic!("{other:?}"),
            }
        };
        assert!(w <= 400.0 && w > 300.0, "shown width {w}");
        assert!(a > 0.0);
        assert_eq!(svc.cache_bytes(), 0, "no bitmap rendered");
        // a later get renders only the shown size
        let MathImageLookup::Ready(img) = wait(&svc, &r) else {
            panic!()
        };
        assert!((img.width - w).abs() < 1.0);
    }

    /// Review round 4: measuring applies the same caps as rasterising (column width, max side,
    /// pixel area), so a measured size is the drawn size.
    #[test]
    fn measure_equals_the_rendered_size_under_every_cap() {
        let svc = MathService::new(Arc::new(|| {})).unwrap();
        let cases = [
            // fits
            (r"x^2+y^2", 20.0, Some(600.0)),
            // column-width capped
            (r"\rule{100em}{2em}", 20.0, Some(400.0)),
            // height capped by the max bitmap side (40 000 px tall at 20 px/em)
            (r"\rule{10em}{2000em}", 20.0, Some(600.0)),
            // area capped (6000 × 6000 px at 40 px/em, no column limit)
            (r"\rule{150em}{150em}", 40.0, None),
        ];
        for (tex, ppe, col) in cases {
            let r = MathRequest {
                tex: tex.into(),
                mode: MathMode::Display,
                px_per_em: ppe,
                color: [0, 0, 0, 255],
                max_width: col,
            };
            let t = Instant::now();
            let (w, a, d) = loop {
                match svc.measure(&r) {
                    MathMeasure::Ready { width, ascent, descent } => break (width, ascent, descent),
                    MathMeasure::Pending => {
                        assert!(t.elapsed() < Duration::from_secs(20));
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    other => panic!("{tex}: {other:?}"),
                }
            };
            let MathImageLookup::Ready(img) = wait(&svc, &r) else {
                panic!("{tex}")
            };
            assert_eq!((w, a, d), (img.width, img.ascent, img.descent), "{tex}");
            assert!(img.size[0] <= super::super::raster::MAX_SIDE && img.size[1] <= super::super::raster::MAX_SIDE);
        }
    }

    /// Review round 3: formulas whose *natural* bitmaps would overflow the cache (but whose
    /// shown sizes fit easily) settle — later frames queue no work.
    #[test]
    fn cache_pressure_settles_without_requeueing() {
        // 2 MiB cache; natural bitmaps of these would be ~4096² × 4 bytes each
        let svc = MathService::with_limits(Arc::new(|| {}), 1, 2 << 20).unwrap();
        let reqs: Vec<MathRequest> = (0..3)
            .map(|i| MathRequest {
                tex: format!(r"\rule{{{}em}}{{200em}}", 200 + i),
                mode: MathMode::Inline,
                px_per_em: 20.0,
                color: [0, 0, 0, 255],
                max_width: Some(200.0),
            })
            .collect();
        let t = Instant::now();
        loop {
            let ready = reqs
                .iter()
                .filter(|r| matches!(svc.get(r), MathImageLookup::Ready(_)))
                .count();
            if ready == reqs.len() {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(30), "never settled");
            std::thread::sleep(Duration::from_millis(2));
        }
        let before = svc.stats();
        for _frame in 0..50 {
            for r in &reqs {
                let MathImageLookup::Ready(img) = svc.get(r) else {
                    panic!("evicted or pending")
                };
                assert!(img.size[0] <= 200 + 2 * img.pad);
            }
        }
        assert_eq!(svc.stats().requests, before.requests, "later frames queue no work");
        assert_eq!(svc.stats().evicted, 0);
        assert!(!svc.busy());
        assert!(svc.cache_bytes() < 2 << 20);
    }
}
