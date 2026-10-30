//! Background document loading: read → decode → parse → build the view model.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
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
    if !go(Stage::Read) {
        return Ok(None);
    }
    let t0 = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let n = bytes.len();
    let version = crate::save::DiskVersion::of_bytes(&bytes, mtime);
    let (text, lossy) = match String::from_utf8(bytes) {
        Ok(s) => (s, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let t1 = Instant::now();
    if !go(Stage::Parse) {
        return Ok(None);
    }
    let doc = Document::parse(text, opts);
    let t2 = Instant::now();
    if !go(Stage::Model) {
        return Ok(None);
    }
    let model = DocModel::build(&doc, Some(path));
    let t3 = Instant::now();
    Ok(Some(Loaded {
        path: path.to_path_buf(),
        doc: Arc::new(doc),
        model,
        timings: LoadTimings {
            read: t1 - t0,
            parse: t2 - t1,
            model: t3 - t2,
        },
        mtime,
        version,
        lossy,
        bytes: n,
    }))
}

/// Hook called by the loader worker before each stage of request `generation` (tests use it
/// to hold the worker at a stage deterministically).
pub type StageHook = Arc<dyn Fn(Stage, u64) + Send + Sync>;

type Slot = Arc<(Mutex<Option<(PathBuf, u64)>>, Condvar)>;

/// A single loader worker with a latest-request slot: a new request replaces a queued one and
/// cancels the running one at its next stage boundary (read → parse → model), so superseded
/// loads stop consuming CPU and memory. Results carry the request generation.
pub struct Loader {
    slot: Slot,
    current: Arc<AtomicU64>,
    rx: Receiver<(u64, Result<Loaded, String>)>,
}

impl Loader {
    pub fn new(opts: ParseOptions, wake: Wake, hook: Option<StageHook>) -> Result<Loader, String> {
        let slot: Slot = Arc::new((Mutex::new(None), Condvar::new()));
        let current = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let (s2, c2) = (slot.clone(), current.clone());
        std::thread::Builder::new()
            .name("markise-load".into())
            .spawn(move || {
                loop {
                    let (path, generation) = {
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
                    match load_staged(&path, opts.clone(), &mut go) {
                        Ok(None) => {} // cancelled
                        r => {
                            let r = r.map(|o| o.expect("not cancelled"));
                            crate::wake_track::publishing();
                            if tx.send((generation, r)).is_err() {
                                crate::wake_track::woken();
                                break;
                            }
                            wake();
                            crate::wake_track::woken();
                        }
                    }
                }
            })
            .map_err(|e| format!("cannot start loader thread: {e}"))?;
        Ok(Loader { slot, current, rx })
    }

    /// Requests `path`, superseding every earlier request; returns its generation.
    pub fn request(&self, path: PathBuf) -> u64 {
        let generation = self.current.fetch_add(1, Ordering::AcqRel) + 1;
        let (m, cv) = &*self.slot;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, generation));
        cv.notify_one();
        generation
    }

    /// Cancels the queued / running request.
    pub fn cancel(&self) {
        self.current.fetch_add(1, Ordering::AcqRel);
        *self.slot.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn try_recv(&self) -> Option<(u64, Result<Loaded, String>)> {
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
        assert_eq!(r.unwrap().doc.text(), "# B\n");
        let stages = seen.lock().unwrap().clone();
        assert!(
            !stages.contains(&(Stage::Model, 1)),
            "request 1 stopped before building: {stages:?}"
        );
        assert!(l.try_recv().is_none(), "no result for the superseded request");
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
                break r.unwrap();
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
