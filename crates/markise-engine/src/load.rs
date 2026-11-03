//! Background document loading: read → decode → parse → build the view model.
//!
//! **Progressive open** (issue #126, ADR 0017): an open of a large file
//! ([`ProgressiveOpts::min_bytes`]) parses and models only the first part of the text first
//! (about [`ProgressiveOpts::prefix_bytes`], cut at the start of a top-level block:
//! [`markise_md::progressive_prefix`]) and returns it at once as [`Outcome::Partial`]; the whole
//! text is parsed and modelled on a thread of its own and delivered through the partial's
//! [`Rest`]. The loader worker is free for the next request meanwhile (opening another file
//! does not wait for the rest; dropping the [`Rest`] cancels it at its next stage).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use markise_layout::DocModel;
use markise_md::{Document, ParseOptions};

use crate::Wake;

/// Durations of the loading phases (for the timing log / NFR-2/3 measurements).
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadTimings {
    pub read: Duration,
    pub parse: Duration,
    pub model: Duration,
}

/// A loaded document.
#[derive(Debug)]
pub struct Loaded {
    pub path: PathBuf,
    /// The parsed document (shared, immutable snapshot) …
    pub doc: Arc<Document>,
    /// … and its view model (built through the format seam; it does not own the document).
    pub model: Arc<DocModel>,
    pub timings: LoadTimings,
    pub mtime: Option<SystemTime>,
    /// Content identity of what was read (for self-write detection and save conflicts).
    pub version: crate::save::DiskVersion,
    /// The file was not valid UTF-8 and was decoded lossily; saving would change it.
    pub lossy: bool,
    pub bytes: usize,
}

/// Progressive open settings (issue #126).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressiveOpts {
    /// Files with at least this many bytes (after decoding) open progressively.
    pub min_bytes: usize,
    /// About this much of the text is parsed and shown first.
    pub prefix_bytes: usize,
}

impl Default for ProgressiveOpts {
    fn default() -> Self {
        ProgressiveOpts {
            min_bytes: 4 << 20,
            prefix_bytes: 200 << 10,
        }
    }
}

impl ProgressiveOpts {
    /// The default, or `None` when `MARKISE_PROGRESSIVE_OPEN=0` switches progressive open off
    /// (comparison runs).
    pub fn from_env() -> Option<ProgressiveOpts> {
        match std::env::var("MARKISE_PROGRESSIVE_OPEN") {
            Ok(v) if v == "0" => None,
            _ => Some(ProgressiveOpts::default()),
        }
    }
}

/// The rest of a progressive open: the whole document, parsed and modelled on its own thread
/// (see the module docs). Dropping it cancels that work at its next stage.
pub struct Rest {
    rx: Receiver<Result<Loaded, String>>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for Rest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rest")
            .field("cancelled", &self.cancel.load(Ordering::Relaxed))
            .finish()
    }
}

impl Rest {
    /// The whole document once it is ready (`Some(Err)` when the thread stopped without one).
    pub fn try_recv(&self) -> Option<Result<Loaded, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("loading the rest of the file stopped unexpectedly".into())),
        }
    }

    /// Stops the work at its next stage (as dropping does); [`Rest::try_recv`] then reports
    /// that it stopped.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Waits for the whole document (tests, benchmarks).
    pub fn wait(&self) -> Result<Loaded, String> {
        self.rx
            .recv()
            .unwrap_or_else(|_| Err("loading the rest of the file stopped unexpectedly".into()))
    }
}

impl Drop for Rest {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// The first part of a progressively opened file ([`Outcome::Partial`]).
#[derive(Debug)]
pub struct Partial {
    /// The first `cut` bytes of the text, parsed and modelled. `version`, `mtime`, `lossy` and
    /// `bytes` describe the whole file as read.
    pub loaded: Loaded,
    /// Length of the part (bytes of the decoded text).
    pub cut: usize,
    /// The whole document, still being parsed.
    pub rest: Rest,
}

/// What a load request produced.
#[derive(Debug)]
pub enum Outcome {
    Loaded(Loaded),
    /// Progressive open ([`Loader::request_open`]): the first part of a large file, with the
    /// whole document to follow through [`Partial::rest`].
    Partial(Box<Partial>),
    /// A reload check found the file's content equal to the version the caller already has
    /// ([`Loader::request_unless`]; e.g. the watcher's event for our own save): nothing was
    /// decoded, parsed or built.
    Unchanged {
        path: PathBuf,
        /// What is on disk now (the same content; its modification time may differ).
        version: crate::save::DiskVersion,
        read: Duration,
    },
}

impl Outcome {
    /// The loaded document (`None` for [`Outcome::Unchanged`]).
    pub fn into_loaded(self) -> Option<Loaded> {
        match self {
            Outcome::Loaded(l) => Some(l),
            Outcome::Partial(_) | Outcome::Unchanged { .. } => None,
        }
    }
}

/// Loading stages (cancellation points, test hooks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Parse,
    Model,
}

/// Reads and parses `path` on the calling thread.
pub fn load_file(path: &Path, opts: ParseOptions) -> Result<Loaded, String> {
    load_staged(path, opts, &mut |_| true).map(|o| o.expect("never cancelled"))
}

/// Like [`load_file`]; `go(stage)` runs before each stage and returns `false` to cancel
/// (then `Ok(None)`).
pub fn load_staged(
    path: &Path,
    opts: ParseOptions,
    go: &mut dyn FnMut(Stage) -> bool,
) -> Result<Option<Loaded>, String> {
    // without a version to compare with, every completed load is `Outcome::Loaded`
    Ok(load_staged_unless(path, opts, None, go)?.and_then(Outcome::into_loaded))
}

/// Like [`load_staged`]; when the bytes read have the same content as `unless`, stops after
/// reading with [`Outcome::Unchanged`] (no decode, parse or model build).
pub fn load_staged_unless(
    path: &Path,
    opts: ParseOptions,
    unless: Option<crate::save::DiskVersion>,
    go: &mut dyn FnMut(Stage) -> bool,
) -> Result<Option<Outcome>, String> {
    let read = match read_file(path, unless, go)? {
        Some(Ok(r)) => r,
        Some(Err(o)) => return Ok(Some(o)),
        None => return Ok(None),
    };
    Ok(build(path, opts, read, go).map(Outcome::Loaded))
}

/// A file read and decoded.
struct Read {
    text: String,
    lossy: bool,
    mtime: Option<SystemTime>,
    version: crate::save::DiskVersion,
    bytes: usize,
    read: Duration,
}

/// Reads and decodes `path` (`Err(Unchanged)` when it has the content of `unless`; `None`:
/// cancelled).
fn read_file(
    path: &Path,
    unless: Option<crate::save::DiskVersion>,
    go: &mut dyn FnMut(Stage) -> bool,
) -> Result<Option<Result<Read, Outcome>>, String> {
    if !go(Stage::Read) {
        return Ok(None);
    }
    let t0 = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let n = bytes.len();
    let version = crate::save::DiskVersion::of_bytes(&bytes, mtime);
    if unless.is_some_and(|k| k.same_content(&version)) {
        return Ok(Some(Err(Outcome::Unchanged {
            path: path.to_path_buf(),
            version,
            read: t0.elapsed(),
        })));
    }
    let (text, lossy) = match String::from_utf8(bytes) {
        Ok(s) => (s, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    Ok(Some(Ok(Read {
        text,
        lossy,
        mtime,
        version,
        bytes: n,
        read: t0.elapsed(),
    })))
}

/// Parses and models a file read (`None`: cancelled).
fn build(path: &Path, opts: ParseOptions, r: Read, go: &mut dyn FnMut(Stage) -> bool) -> Option<Loaded> {
    if !go(Stage::Parse) {
        return None;
    }
    let t1 = Instant::now();
    let doc = Document::parse(r.text, opts);
    let t2 = Instant::now();
    if !go(Stage::Model) {
        return None;
    }
    let model = DocModel::build(&doc, Some(path));
    let t3 = Instant::now();
    Some(Loaded {
        path: path.to_path_buf(),
        doc: Arc::new(doc),
        model,
        timings: LoadTimings {
            read: r.read,
            parse: t2 - t1,
            model: t3 - t2,
        },
        mtime: r.mtime,
        version: r.version,
        lossy: r.lossy,
        bytes: r.bytes,
    })
}

/// Opens `path` progressively when it is large enough ([`ProgressiveOpts`]): the first part
/// comes back as [`Outcome::Partial`] while a new thread parses the whole text (calling
/// `rest_hook` before its stages, waking `wake` when done); otherwise like
/// [`load_staged_unless`] without a version.
pub fn load_progressive(
    path: &Path,
    opts: ParseOptions,
    progressive: ProgressiveOpts,
    go: &mut dyn FnMut(Stage) -> bool,
    rest_hook: Option<Box<dyn Fn(Stage) + Send>>,
    wake: crate::Wake,
) -> Result<Option<Outcome>, String> {
    let read = match read_file(path, None, go)? {
        Some(Ok(r)) => r,
        Some(Err(o)) => return Ok(Some(o)),
        None => return Ok(None),
    };
    if read.text.len() < progressive.min_bytes {
        return Ok(build(path, opts, read, go).map(Outcome::Loaded));
    }
    if !go(Stage::Parse) {
        return Ok(None);
    }
    let t1 = Instant::now();
    let Some((cut, doc)) = markise_md::progressive_prefix(&read.text, progressive.prefix_bytes, &opts) else {
        return Ok(build(path, opts, read, &mut |st| st == Stage::Parse || go(st)).map(Outcome::Loaded));
    };
    let t2 = Instant::now();
    let model = DocModel::build(&doc, Some(path));
    let t3 = Instant::now();
    let loaded = Loaded {
        path: path.to_path_buf(),
        doc: Arc::new(doc),
        model,
        timings: LoadTimings {
            read: read.read,
            parse: t2 - t1,
            model: t3 - t2,
        },
        mtime: read.mtime,
        version: read.version,
        lossy: read.lossy,
        bytes: read.bytes,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let c2 = cancel.clone();
    let p2 = path.to_path_buf();
    std::thread::Builder::new()
        .name("markise-load-rest".into())
        .spawn(move || {
            let mut go = |st: Stage| {
                if let Some(h) = &rest_hook {
                    h(st);
                }
                !c2.load(Ordering::Acquire)
            };
            if let Some(full) = build(&p2, opts, read, &mut go) {
                let published = crate::wake_track::publishing();
                if tx.send(Ok(full)).is_ok() {
                    wake();
                }
                drop(published);
            }
        })
        .map_err(|e| format!("cannot start a loader thread: {e}"))?;
    Ok(Some(Outcome::Partial(Box::new(Partial {
        loaded,
        cut,
        rest: Rest { rx, cancel },
    }))))
}

/// Hook called by the loader worker before each stage of request `generation` (tests use it
/// to hold the worker at a stage deterministically).
pub type StageHook = Arc<dyn Fn(Stage, u64) + Send + Sync>;

/// A queued request.
struct Job {
    path: PathBuf,
    generation: u64,
    /// The version to compare with ([`Loader::request_unless`]).
    unless: Option<crate::save::DiskVersion>,
    /// May open progressively ([`Loader::request_open`]).
    progressive: bool,
}
type Slot = Arc<(Mutex<Option<Job>>, Condvar)>;

/// A single loader worker with a latest-request slot: a new request replaces a queued one and
/// cancels the running one at its next stage boundary (read → parse → model), so superseded
/// loads stop consuming CPU and memory. Results carry the request generation.
pub struct Loader {
    slot: Slot,
    current: Arc<AtomicU64>,
    rx: Receiver<(u64, Result<Outcome, String>)>,
}

impl Loader {
    pub fn new(opts: ParseOptions, wake: Wake, hook: Option<StageHook>) -> Result<Loader, String> {
        Self::with_progressive(opts, wake, hook, None)
    }

    /// Like [`Loader::new`]; [`Loader::request_open`] opens large files progressively with
    /// `progressive` (`None`: never).
    pub fn with_progressive(
        opts: ParseOptions,
        wake: Wake,
        hook: Option<StageHook>,
        progressive: Option<ProgressiveOpts>,
    ) -> Result<Loader, String> {
        let slot: Slot = Arc::new((Mutex::new(None), Condvar::new()));
        let current = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let (s2, c2) = (slot.clone(), current.clone());
        std::thread::Builder::new()
            .name("markise-load".into())
            .spawn(move || {
                loop {
                    let Job {
                        path,
                        generation,
                        unless,
                        progressive: may_progress,
                    } = {
                        let (m, cv) = &*s2;
                        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(job) = g.take() {
                                break job;
                            }
                            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let mut go = |st: Stage| {
                        if let Some(h) = &hook {
                            h(st, generation);
                        }
                        c2.load(Ordering::Acquire) == generation
                    };
                    let result = match progressive.filter(|_| may_progress && unless.is_none()) {
                        Some(p) => {
                            let rest_hook = hook
                                .clone()
                                .map(|h| -> Box<dyn Fn(Stage) + Send> { Box::new(move |st| h(st, generation)) });
                            load_progressive(&path, opts.clone(), p, &mut go, rest_hook, wake.clone())
                        }
                        None => load_staged_unless(&path, opts.clone(), unless, &mut go),
                    };
                    match result {
                        Ok(None) => {} // cancelled
                        r => {
                            let r = r.map(|o| o.expect("not cancelled"));
                            let published = crate::wake_track::publishing();
                            if tx.send((generation, r)).is_err() {
                                drop(published);
                                break;
                            }
                            wake();
                            drop(published);
                        }
                    }
                }
            })
            .map_err(|e| format!("cannot start loader thread: {e}"))?;
        Ok(Loader { slot, current, rx })
    }

    /// Requests `path`, superseding every earlier request; returns its generation.
    pub fn request(&self, path: PathBuf) -> u64 {
        self.request_job(path, None, false)
    }

    /// Like [`Loader::request`] for opening a document: a large file comes back first as
    /// [`Outcome::Partial`] (progressive open, if configured; see the module docs).
    pub fn request_open(&self, path: PathBuf) -> u64 {
        self.request_job(path, None, true)
    }

    /// Like [`Loader::request`] for a reload check: if the file's content equals `known` (the
    /// version the caller has, e.g. what its own save wrote), the result is
    /// [`Outcome::Unchanged`] right after reading — no second document is parsed and built.
    pub fn request_unless(&self, path: PathBuf, known: crate::save::DiskVersion) -> u64 {
        self.request_job(path, Some(known), false)
    }

    fn request_job(&self, path: PathBuf, unless: Option<crate::save::DiskVersion>, progressive: bool) -> u64 {
        let generation = self.current.fetch_add(1, Ordering::AcqRel) + 1;
        let (m, cv) = &*self.slot;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = Some(Job {
            path,
            generation,
            unless,
            progressive,
        });
        cv.notify_one();
        generation
    }

    /// Cancels the queued / running request.
    pub fn cancel(&self) {
        self.current.fetch_add(1, Ordering::AcqRel);
        *self.slot.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn try_recv(&self) -> Option<(u64, Result<Outcome, String>)> {
        self.rx.try_recv().ok()
    }
}

/// Builds a model from text already in memory (source-mode edits) — same work as the loader
/// minus I/O; cheap for edits because markise-md re-parses incrementally.
pub fn rebuild(doc: &Document, path: Option<&Path>) -> Arc<DocModel> {
    DocModel::build(doc, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_loads_are_cancelled_at_the_next_stage() {
        // second review, item 12 — deterministic: the hook holds the worker after reading
        // request 1 until request 2 has been made
        let dir = std::env::temp_dir().join(format!("markise-engine-loader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        std::fs::write(&a, "# A\n").unwrap();
        std::fs::write(&b, "# B\n").unwrap();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let seen: Arc<Mutex<Vec<(Stage, u64)>>> = Default::default();
        let seen2 = seen.clone();
        let hook: StageHook = Arc::new(move |st, generation| {
            seen2.lock().unwrap().push((st, generation));
            if generation == 1 && st == Stage::Parse {
                let _ = gate_rx.lock().unwrap().recv(); // hold until released
            }
        });
        let l = Loader::new(ParseOptions::default(), Arc::new(|| {}), Some(hook)).unwrap();
        assert_eq!(l.request(a.clone()), 1);
        while !seen.lock().unwrap().contains(&(Stage::Parse, 1)) {
            std::thread::yield_now();
        }
        assert_eq!(l.request(b.clone()), 2);
        gate_tx.send(()).unwrap();
        let (generation, r) = loop {
            if let Some(x) = l.try_recv() {
                break x;
            }
            std::thread::yield_now();
        };
        assert_eq!(generation, 2);
        assert_eq!(r.unwrap().into_loaded().unwrap().doc.text(), "# B\n");
        let stages = seen.lock().unwrap().clone();
        assert!(
            !stages.contains(&(Stage::Model, 1)),
            "request 1 stopped before building: {stages:?}"
        );
        assert!(l.try_recv().is_none(), "no result for the superseded request");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_check_of_an_unchanged_file_stops_after_reading() {
        // issue #29: the watcher event of our own save must not parse a second document
        let dir = std::env::temp_dir().join(format!("markise-engine-unless-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.md");
        std::fs::write(&p, "# Saved\n").unwrap();
        let known = crate::save::disk_version(&p).unwrap().unwrap();
        let stages: Arc<Mutex<Vec<Stage>>> = Default::default();
        let s2 = stages.clone();
        let hook: StageHook = Arc::new(move |st, _| s2.lock().unwrap().push(st));
        let l = Loader::new(ParseOptions::default(), Arc::new(|| {}), Some(hook)).unwrap();
        let next = || loop {
            if let Some((_, r)) = l.try_recv() {
                break r.unwrap();
            }
            std::thread::yield_now();
        };
        l.request_unless(p.clone(), known);
        match next() {
            Outcome::Unchanged { path, version, .. } => {
                assert_eq!(path, p);
                assert!(version.same_content(&known));
            }
            o => panic!("expected Unchanged, got {o:?}"),
        }
        assert_eq!(*stages.lock().unwrap(), vec![Stage::Read], "no parse, no model");
        // a different content (same length included) is loaded in full
        std::fs::write(&p, "# Other\n").unwrap();
        l.request_unless(p.clone(), known);
        let loaded = next().into_loaded().expect("changed content is loaded");
        assert_eq!(loaded.doc.text(), "# Other\n");
        assert_eq!(
            *stages.lock().unwrap(),
            vec![Stage::Read, Stage::Read, Stage::Parse, Stage::Model]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn small_progressive() -> ProgressiveOpts {
        ProgressiveOpts {
            min_bytes: 16 << 10,
            prefix_bytes: 4 << 10,
        }
    }

    fn next(l: &Loader) -> (u64, Result<Outcome, String>) {
        loop {
            if let Some(x) = l.try_recv() {
                break x;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn progressive_open_returns_the_first_part_then_the_whole_document() {
        // issue #126
        let dir = std::env::temp_dir().join(format!("markise-engine-progressive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.md");
        // a BOM, CRLF line endings and one invalid byte (decoded lossily)
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(markise_corpus::generate_profile(64 << 10, 3, markise_corpus::Profile::Mixed, true).into_bytes());
        bytes.extend(b"\r\nbad \xff byte\r\n");
        std::fs::write(&p, &bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let l = Loader::with_progressive(
            ParseOptions::default(),
            Arc::new(|| {}),
            None,
            Some(small_progressive()),
        )
        .unwrap();
        let generation = l.request_open(p.clone());
        let (g, r) = next(&l);
        assert_eq!(g, generation);
        let Outcome::Partial(part) = r.unwrap() else {
            panic!("a partial outcome")
        };
        let Partial { loaded, cut, rest } = *part;
        assert!(cut > 2 << 10 && cut < 16 << 10, "{cut}");
        assert_eq!(loaded.doc.text(), &text[..cut]);
        assert!(loaded.doc.has_bom());
        assert!(loaded.lossy, "the whole file's decoding");
        assert_eq!(loaded.bytes, bytes.len());
        let on_disk = crate::save::disk_version(&p).unwrap().unwrap();
        assert!(loaded.version.same_content(&on_disk), "the whole file's version");
        let full = rest.wait().unwrap();
        assert_eq!(full.doc.text(), text);
        assert!(full.lossy && full.version.same_content(&on_disk));
        assert!(full.model.check_against_full(&*full.doc).is_ok());
        // only opens are progressive; small files open in one step
        l.request(p.clone());
        assert!(matches!(next(&l).1, Ok(Outcome::Loaded(_))));
        let small = dir.join("small.md");
        std::fs::write(&small, "# Small\n\n".repeat(100)).unwrap();
        l.request_open(small);
        assert!(matches!(next(&l).1, Ok(Outcome::Loaded(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_rest_stops_before_the_model() {
        let dir = std::env::temp_dir().join(format!("markise-engine-rest-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.md");
        std::fs::write(&p, markise_corpus::generate(64 << 10, 9)).unwrap();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let seen: Arc<Mutex<Vec<(Stage, std::thread::ThreadId)>>> = Default::default();
        let s2 = seen.clone();
        let hook: StageHook = Arc::new(move |st, _| {
            let me = std::thread::current();
            s2.lock().unwrap().push((st, me.id()));
            if me.name() == Some("markise-load-rest") && st == Stage::Parse {
                let _ = gate_rx.lock().unwrap().recv();
            }
        });
        let l = Loader::with_progressive(
            ParseOptions::default(),
            Arc::new(|| {}),
            Some(hook),
            Some(small_progressive()),
        )
        .unwrap();
        l.request_open(p.clone());
        let Ok(Outcome::Partial(part)) = next(&l).1 else {
            panic!("a partial outcome")
        };
        part.rest.cancel();
        gate_tx.send(()).unwrap();
        let r = loop {
            if let Some(r) = part.rest.try_recv() {
                break r;
            }
            std::thread::yield_now();
        };
        assert!(r.is_err(), "stopped without a document");
        // (the prefix is modelled without a stage: only the rest has a model stage)
        let stages: Vec<Stage> = seen.lock().unwrap().iter().map(|x| x.0).collect();
        assert!(!stages.contains(&Stage::Model), "no model built: {stages:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_in_background_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("markise-engine-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.md");
        let text = markise_corpus::generate(64 * 1024, 7);
        std::fs::write(&p, &text).unwrap();
        let l = Loader::new(ParseOptions::default(), Arc::new(|| {}), None).unwrap();
        l.request(p.clone());
        let loaded = loop {
            if let Some((_, r)) = l.try_recv() {
                break r.unwrap().into_loaded().unwrap();
            }
            std::thread::yield_now();
        };
        assert_eq!(loaded.doc.serialize(), text);
        assert!(!loaded.lossy);
        assert!(!loaded.model.items.is_empty());
        std::fs::write(&p, b"bad \xff byte").unwrap();
        assert!(load_file(&p, ParseOptions::default()).unwrap().lossy);
        assert!(load_file(&dir.join("missing.md"), ParseOptions::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
