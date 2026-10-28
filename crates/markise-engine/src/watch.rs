//! File watching (spec 04 §11 external changes, §4.1 tree auto-refresh) on top of `notify`.
//! The open file is watched through its parent directory (editors and our own atomic save
//! replace the file, which would end a watch on the file itself).
//!
//! Registering or removing a watch can block for a long, unpredictable time (on macOS every
//! change restarts the FSEvents stream; hundreds of milliseconds were measured, issue #24), so
//! the `notify` watcher lives on its own worker thread: [`FileWatcher`]'s methods only post the
//! wanted targets and return at once (ADR 0010). The worker coalesces bursts into one batched
//! update and skips the per-file watch when the mounted folder's recursive watch already covers
//! the file's directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::Wake;
use crate::jobs::Hooks;
use crate::save::DiskVersion;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// The open document changed (modified, replaced, created).
    FileChanged(PathBuf),
    /// The open document was removed or renamed away.
    FileRemoved(PathBuf),
    /// Something changed in this directory of the mounted folder.
    DirChanged(PathBuf),
    /// The mounted folder's `.obsidian/app.json` (or the `.obsidian/` folder itself) changed,
    /// also in place (spec 06 §14, E17); carries the mounted folder.
    VaultConfigChanged(PathBuf),
    /// A file in the user theme folder changed (live theme reload, ADR 0004). After the theme
    /// folder's watch (re)started, the folder itself is reported: anything beneath it may have
    /// changed.
    ThemeChanged(PathBuf),
    /// The watch of the mounted folder (re)started: changes made before it took effect have no
    /// events of their own, so directories listed earlier must be listed again.
    FolderWatched(PathBuf),
    /// Watching failed (the paths stay wanted; the next request retries).
    Error(String),
}

#[derive(Clone, Default)]
struct TargetSet {
    file: Option<PathBuf>,
    folder: Option<PathBuf>,
    themes: Option<PathBuf>,
}

type Targets = std::sync::Arc<std::sync::Mutex<TargetSet>>;

/// Requests to the watcher worker.
enum Ctl {
    /// The open file and the content identity of the version the caller has.
    File(PathBuf, Option<DiskVersion>),
    /// No open file on disk any more (a new, untitled document).
    NoFile,
    Folder(PathBuf),
    Themes(PathBuf),
    /// The caller's document is now this version on disk (its own save, an accepted reload);
    /// `None`: it knows the file is missing.
    Acknowledge(PathBuf, Option<DiskVersion>),
    /// Answered once every earlier request is applied: `Err` lists watches not in effect.
    Sync(Sender<Result<(), String>>),
}

/// Watches the open file, the mounted folder and the user theme folder. Cheap to call from the
/// UI thread: all file-system work happens on the watcher's worker thread.
pub struct FileWatcher {
    rx: Receiver<WatchEvent>,
    ctl: Sender<Ctl>,
}

fn same(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The watches wanted for `t`: path → recursive. The open file's directory needs no watch of
/// its own when a recursive watch already covers it.
fn wanted(t: &TargetSet) -> BTreeMap<PathBuf, bool> {
    let mut m = BTreeMap::new();
    for root in [&t.folder, &t.themes].into_iter().flatten() {
        m.insert(root.clone(), true);
    }
    if let Some(dir) = t.file.as_ref().and_then(|f| f.parent())
        && !m.iter().any(|(root, rec)| *rec && dir.starts_with(root))
    {
        m.insert(dir.to_path_buf(), false);
    }
    m
}

/// What the watcher worker needs besides the `notify` watcher.
struct Worker {
    targets: Targets,
    ctl: Receiver<Ctl>,
    hooks: Hooks,
    events: Sender<WatchEvent>,
    wake: Wake,
}

/// Compares the open file on disk with the version the caller has (content identity: length
/// and hash of the bytes, not the modification time, which can be paired with older bytes when
/// a replace races the caller's read, or be preserved by the replacing tool).
/// What the worker believes the caller's document is on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Baseline {
    /// No version given: nothing to compare with.
    Unknown,
    /// The file is known not to exist (deleted; saving or an external write recreates it).
    Missing,
    Version(DiskVersion),
}

impl Baseline {
    fn of(v: Option<DiskVersion>) -> Self {
        v.map_or(Baseline::Missing, Baseline::Version)
    }
}

/// Returns the event to report, if any, and the new baseline (what is on disk now).
fn reconcile(file: &Path, known: Baseline) -> (Option<WatchEvent>, Baseline) {
    if known == Baseline::Unknown {
        return (None, known);
    }
    let changed = || Some(WatchEvent::FileChanged(file.to_path_buf()));
    match crate::save::disk_version(file) {
        Ok(None) if known == Baseline::Missing => (None, known),
        Ok(None) => (Some(WatchEvent::FileRemoved(file.to_path_buf())), Baseline::Missing),
        Ok(Some(now)) => match known {
            Baseline::Version(k) if k.same_content(&now) => (None, Baseline::Version(now)),
            // changed, or recreated after a deletion
            _ => (changed(), Baseline::Version(now)),
        },
        Err(_) => (None, known),
    }
}

/// The worker: owns the `notify` watcher and applies the latest wanted targets.
///
/// After every change of the registration — on macOS the whole FSEvents stream is stopped and
/// restarted "since now", so nothing that happens meanwhile is reported — and whenever a new
/// file is opened, it catches up: the mounted folder's listings are refreshed
/// ([`WatchEvent::FolderWatched`]) and the open file is compared with the caller's version.
fn run_worker(mut watcher: RecommendedWatcher, w: Worker) {
    let Worker {
        targets,
        ctl,
        hooks,
        events,
        wake,
    } = w;
    let mut active: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut want = TargetSet::default();
    let mut known = Baseline::Unknown;
    let mut failure: Option<String> = None;
    while let Ok(first) = ctl.recv() {
        // coalesce a burst (fast clicks through the file tree) into one update
        let mut acks = Vec::new();
        // a new file or a new baseline: compare the open file with it
        let mut new_file = false;
        for c in std::iter::once(first).chain(ctl.try_iter()) {
            match c {
                Ctl::File(p, version) => {
                    want.file = Some(canonical(&p));
                    known = version.map_or(Baseline::Unknown, Baseline::Version);
                    new_file = true;
                }
                Ctl::Acknowledge(p, version) => {
                    if want.file.as_ref() == Some(&canonical(&p)) {
                        known = Baseline::of(version);
                        // the disk may have changed again since (e.g. while this request
                        // waited behind a registration change): compare now
                        new_file = true;
                    }
                }
                Ctl::NoFile => {
                    want.file = None;
                    known = Baseline::Unknown;
                }
                Ctl::Folder(p) => want.folder = Some(canonical(&p)),
                Ctl::Themes(p) => want.themes = Some(canonical(&p)),
                Ctl::Sync(tx) => acks.push(tx),
            }
        }
        let next = wanted(&want);
        let mut out = Vec::new();
        let restarted = next != active;
        if restarted {
            // one batched change: the backend restarts at most once (FSEvents restarts its
            // stream on every individual watch/unwatch)
            let mut paths = watcher.paths_mut();
            if let Some(h) = &hooks.before_watch {
                h(); // tests: inside the gap (FSEvents: the stream is stopped)
            }
            for (p, rec) in &active {
                if next.get(p) != Some(rec) {
                    let _ = paths.remove(p);
                }
            }
            let mut errors = Vec::new();
            let mut added = next.clone();
            for (p, rec) in &next {
                if active.get(p) != Some(rec) {
                    let mode = if *rec {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    };
                    if let Err(e) = paths.add(p, mode) {
                        errors.push(format!("{}: {e}", p.display()));
                        added.remove(p);
                    }
                }
            }
            match paths.commit() {
                Ok(()) => active = added,
                Err(e) => {
                    // nothing is known to be watched: every wanted path is registered again
                    // by the next request
                    errors.push(format!("cannot start watching: {e}"));
                    active.clear();
                }
            }
            failure = (!errors.is_empty()).then(|| errors.join("; "));
            if let Some(f) = &failure {
                out.push(WatchEvent::Error(f.clone()));
            }
        }
        if let Ok(mut t) = targets.lock() {
            *t = want.clone();
        }
        if restarted && let Some(root) = want.folder.as_ref().filter(|r| active.get(*r) == Some(&true)) {
            out.push(WatchEvent::FolderWatched(root.clone()));
        }
        if restarted && let Some(root) = want.themes.as_ref().filter(|r| active.get(*r) == Some(&true)) {
            // the same catch-up for the theme folder: "something beneath it may have changed"
            out.push(WatchEvent::ThemeChanged(root.clone()));
        }
        if (restarted || new_file)
            && let Some(f) = &want.file
        {
            let (e, now) = reconcile(f, known);
            // a difference is reported once; the caller re-reads and decides
            known = now;
            out.extend(e);
        }
        let mut sent = false;
        for e in out {
            sent |= events.send(e).is_ok();
        }
        if sent {
            wake();
        }
        for tx in acks {
            let _ = tx.send(failure.clone().map_or(Ok(()), Err));
        }
    }
}

impl FileWatcher {
    pub fn new(wake: Wake) -> notify::Result<FileWatcher> {
        Self::with_hooks(wake, Hooks::default())
    }

    /// Like [`FileWatcher::new`], with test hooks ([`crate::jobs::JobHooks::before_watch`]).
    pub fn with_hooks(wake: Wake, hooks: Hooks) -> notify::Result<FileWatcher> {
        let (tx, rx) = channel();
        let targets: Targets = Default::default();
        let shared = targets.clone();
        let (events, wake2) = (tx.clone(), wake.clone());
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, EventKind::Access(_)) {
                return;
            }
            let TargetSet { file, folder, themes } = shared.lock().map(|g| g.clone()).unwrap_or_default();
            let mut sent = false;
            for p in &ev.paths {
                if themes.as_ref().is_some_and(|root| p.starts_with(root)) {
                    sent |= tx.send(WatchEvent::ThemeChanged(p.clone())).is_ok();
                }
                if let Some(f) = &file
                    && p.file_name() == f.file_name()
                    && p.parent().zip(f.parent()).is_some_and(|(a, b)| same(a, b))
                {
                    let e = if f.exists() {
                        WatchEvent::FileChanged(f.clone())
                    } else {
                        WatchEvent::FileRemoved(f.clone())
                    };
                    sent |= tx.send(e).is_ok();
                }
                // `.obsidian/app.json` (the only config file spec 06 §14 reads), content edits
                // included — DirChanged covers names only; Obsidian's constant workspace and
                // plugin writes are not reported
                if let Some(root) = folder
                    .as_ref()
                    .filter(|root| *p == root.join(".obsidian").join("app.json") || *p == root.join(".obsidian"))
                {
                    sent |= tx.send(WatchEvent::VaultConfigChanged(root.clone())).is_ok();
                }
                if folder.as_ref().is_some_and(|root| p.starts_with(root))
                    && matches!(
                        ev.kind,
                        EventKind::Create(_)
                            | EventKind::Remove(_)
                            | EventKind::Modify(notify::event::ModifyKind::Name(_))
                    )
                    && let Some(dir) = p.parent()
                {
                    sent |= tx.send(WatchEvent::DirChanged(dir.to_path_buf())).is_ok();
                }
            }
            if sent {
                wake();
            }
        })?;
        let (ctl, ctl_rx) = channel();
        // detached: dropping the `FileWatcher` closes `ctl`, the worker then drops the watcher
        // (which may block for a while) off the caller's thread
        std::thread::Builder::new()
            .name("markise-watch".into())
            .spawn(move || {
                let w = Worker {
                    targets,
                    ctl: ctl_rx,
                    hooks,
                    events,
                    wake: wake2,
                };
                run_worker(watcher, w)
            })
            .map_err(|e| notify::Error::generic(&format!("watcher thread: {e}")))?;
        Ok(FileWatcher { rx, ctl })
    }

    /// Watches the open document (replacing the previous one). Returns at once. `version` is
    /// the content identity of what the caller read: if the file's content differs once the
    /// watch is in effect (or after any later restart of the watch), a
    /// [`WatchEvent::FileChanged`] (or `FileRemoved`) follows, so a change made in between is
    /// not lost.
    pub fn watch_file(&mut self, path: &Path, version: Option<DiskVersion>) {
        let _ = self.ctl.send(Ctl::File(path.to_path_buf(), version));
    }

    /// Watches the mounted folder recursively (replacing the previous one). Returns at once.
    pub fn watch_folder(&mut self, dir: &Path) {
        let _ = self.ctl.send(Ctl::Folder(dir.to_path_buf()));
    }

    /// Watches the user theme folder recursively (replacing the previous one); events arrive as
    /// [`WatchEvent::ThemeChanged`]. The folder must exist. Returns at once.
    pub fn watch_themes(&mut self, dir: &Path) {
        let _ = self.ctl.send(Ctl::Themes(dir.to_path_buf()));
    }

    /// Stops watching the open document (the editor now shows a document without a file).
    /// Returns at once.
    pub fn clear_file(&mut self) {
        let _ = self.ctl.send(Ctl::NoFile);
    }

    /// The open document `path` is now `version` on disk (after the caller's own save or an
    /// accepted reload; `None` when it knows the file is gone): later catch-ups compare with
    /// it. Returns at once.
    pub fn acknowledge(&self, path: &Path, version: Option<DiskVersion>) {
        let _ = self.ctl.send(Ctl::Acknowledge(path.to_path_buf(), version));
    }

    /// Blocks until every earlier `watch_*` request has been applied (tests, tools; never call
    /// it on the UI thread). `Err` names the watches that are not in effect.
    pub fn sync(&self) -> Result<(), String> {
        let (tx, rx) = channel();
        self.ctl
            .send(Ctl::Sync(tx))
            .map_err(|_| "watcher stopped".to_string())?;
        rx.recv().map_err(|_| "watcher stopped".to_string())?
    }

    pub fn try_recv(&self) -> Option<WatchEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime};

    #[test]
    fn reports_changes_of_the_open_file() {
        let dir = std::env::temp_dir().join(format!("markise-engine-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("w.md");
        std::fs::write(&p, "one").unwrap();
        let mut w = FileWatcher::new(Arc::new(|| {})).unwrap();
        w.watch_file(&p, None);
        w.sync().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        crate::save::save_atomic(&p, "two").unwrap();
        let t = Instant::now();
        let mut got = None;
        while t.elapsed() < Duration::from_secs(30) {
            if let Some(e) = w.try_recv() {
                got = Some(e);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let canon = std::fs::canonicalize(&p).unwrap();
        assert_eq!(got, Some(WatchEvent::FileChanged(canon)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_event(w: &FileWatcher, want: &WatchEvent) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(30) {
            while let Some(e) = w.try_recv() {
                if &e == want {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_file_inside_the_mounted_folder_needs_no_watch_of_its_own() {
        let t = TargetSet {
            file: Some(PathBuf::from("/v/notes/a.md")),
            folder: Some(PathBuf::from("/v")),
            themes: Some(PathBuf::from("/t")),
        };
        let w = wanted(&t);
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w.get(Path::new("/v")), Some(&true));
        // outside the mounted folder: its directory is watched (non-recursively)
        let t = TargetSet {
            file: Some(PathBuf::from("/elsewhere/b.md")),
            ..t
        };
        assert_eq!(wanted(&t).get(Path::new("/elsewhere")), Some(&false));
    }

    /// Issue #24: registering watches stalled the UI thread (FSEvents stream restarts took up
    /// to ~1.5 s). The requests must return at once even while the worker is stuck.
    #[test]
    fn watch_requests_return_at_once_while_the_worker_is_busy() {
        let dir = std::env::temp_dir().join(format!("markise-engine-watch-busy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let p = dir.join("sub").join("n.md");
        std::fs::write(&p, "one").unwrap();
        let hooks = Arc::new(crate::jobs::JobHooks {
            before_watch: Some(Box::new(|| std::thread::sleep(Duration::from_millis(400)))),
            ..Default::default()
        });
        let mut w = FileWatcher::with_hooks(Arc::new(|| {}), hooks).unwrap();
        let t = Instant::now();
        w.watch_folder(&dir);
        w.watch_file(&p, None);
        w.watch_file(&dir.join("sub").join("other.md"), None);
        w.watch_file(&p, None);
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "watch requests blocked: {:?}",
            t.elapsed()
        );
        // still effective: a change of the open file inside the mounted folder is reported
        w.sync().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        crate::save::save_atomic(&p, "two").unwrap();
        let canon = std::fs::canonicalize(&p).unwrap();
        assert!(wait_event(&w, &WatchEvent::FileChanged(canon)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("markise-engine-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    /// Holds the watcher worker inside its next registration change (FSEvents: stream stopped)
    /// while `held`; `entered` receives one message per registration change.
    struct Gate {
        held: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        entered: Receiver<()>,
    }

    impl Gate {
        fn new() -> (Gate, crate::jobs::Hooks) {
            let held = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let (tx, entered) = channel();
            let (h2, tx) = (held.clone(), std::sync::Mutex::new(tx));
            let hooks = Arc::new(crate::jobs::JobHooks {
                before_watch: Some(Box::new(move || {
                    let _ = tx.lock().unwrap().send(());
                    let (m, cv) = &*h2;
                    let mut g = m.lock().unwrap();
                    while *g {
                        g = cv.wait(g).unwrap();
                    }
                })),
                ..Default::default()
            });
            (Gate { held, entered }, hooks)
        }
        fn hold(&self) {
            while self.entered.try_recv().is_ok() {}
            *self.held.0.lock().unwrap() = true;
        }
        fn wait_entered(&self) {
            self.entered
                .recv_timeout(Duration::from_secs(30))
                .expect("registration started");
        }
        fn release(&self) {
            *self.held.0.lock().unwrap() = false;
            self.held.1.notify_all();
        }
    }

    /// Review of #27 (P1): the tree is listed while the folder watch is still being registered;
    /// a change in that gap must still refresh the listings.
    #[test]
    fn folder_activation_asks_to_refresh_the_listings() {
        let dir = tmp("activate");
        let (gate, hooks) = Gate::new();
        let mut w = FileWatcher::with_hooks(Arc::new(|| {}), hooks).unwrap();
        gate.hold();
        w.watch_folder(&dir);
        gate.wait_entered();
        std::fs::write(dir.join("created-in-the-gap.md"), "x").unwrap();
        gate.release();
        w.sync().unwrap();
        assert!(wait_event(&w, &WatchEvent::FolderWatched(dir.clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review of #27 (P1): a folder-only switch changes the watch covering the open document;
    /// a change to the document in that gap is still reported.
    #[test]
    fn a_folder_switch_reconciles_the_open_document() {
        let root = tmp("switch");
        std::fs::create_dir_all(root.join("a").join("x")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let doc = root.join("a").join("x").join("doc.md");
        std::fs::write(&doc, "loaded").unwrap();
        let (gate, hooks) = Gate::new();
        let mut w = FileWatcher::with_hooks(Arc::new(|| {}), hooks).unwrap();
        w.watch_folder(&root.join("a"));
        let v = crate::save::disk_version(&doc).unwrap();
        w.watch_file(&doc, v);
        w.sync().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        while w.try_recv().is_some() {}
        gate.hold();
        w.watch_folder(&root.join("b"));
        gate.wait_entered();
        std::fs::write(&doc, "changed in the gap").unwrap();
        gate.release();
        w.sync().unwrap();
        assert!(wait_event(&w, &WatchEvent::FileChanged(doc.clone())));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review of #27 (P1): the comparison uses content identity, not the modification time.
    #[test]
    fn reconciling_compares_content_not_modification_time() {
        let dir = tmp("identity");
        let p = dir.join("r.md");
        // a replace between the loader's read and its stat: old bytes paired with the new mtime
        std::fs::write(&p, "new content").unwrap();
        let new_mtime = std::fs::metadata(&p).unwrap().modified().ok();
        let raced = DiskVersion::of_bytes(b"old content", new_mtime);
        assert_eq!(
            reconcile(&p, Baseline::Version(raced)).0,
            Some(WatchEvent::FileChanged(p.clone()))
        );
        // a replace that preserves the modification time (same length, too)
        let v = crate::save::disk_version(&p).unwrap().unwrap();
        std::fs::write(&p, "NEW CONTENT").unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(v.mtime.unwrap()).unwrap();
        drop(f);
        assert_eq!(std::fs::metadata(&p).unwrap().modified().ok(), v.mtime);
        assert_eq!(
            reconcile(&p, Baseline::Version(v)).0,
            Some(WatchEvent::FileChanged(p.clone()))
        );
        // touching without a content change is not a change
        let v = crate::save::disk_version(&p).unwrap().unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
        drop(f);
        assert_eq!(reconcile(&p, Baseline::Version(v)).0, None);
        // removed
        std::fs::remove_file(&p).unwrap();
        assert_eq!(
            reconcile(&p, Baseline::Version(v)).0,
            Some(WatchEvent::FileRemoved(p.clone()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End to end: a change between the caller's read and the watch is reported.
    #[test]
    fn a_change_before_the_watch_takes_effect_is_reported() {
        let dir = tmp("gap");
        let p = dir.join("g.md");
        std::fs::write(&p, "read by the caller").unwrap();
        let v = crate::save::disk_version(&p).unwrap();
        std::fs::write(&p, "changed before the watch").unwrap();
        let mut w = FileWatcher::new(Arc::new(|| {})).unwrap();
        w.watch_file(&p, v);
        assert!(wait_event(&w, &WatchEvent::FileChanged(p.clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review of #27 (P2): a failed registration is reported, not recorded as in effect, and
    /// the next request retries it.
    #[test]
    fn a_failed_registration_is_reported_and_retried() {
        let root = tmp("fail");
        let missing = root.join("not-yet");
        let mut w = FileWatcher::new(Arc::new(|| {})).unwrap();
        w.watch_folder(&missing);
        assert!(w.sync().is_err());
        let mut saw_error = false;
        while let Some(e) = w.try_recv() {
            saw_error |= matches!(e, WatchEvent::Error(_));
        }
        assert!(saw_error);
        std::fs::create_dir_all(&missing).unwrap();
        w.watch_folder(&missing);
        w.sync().unwrap();
        let canon = std::fs::canonicalize(&missing).unwrap();
        assert!(wait_event(&w, &WatchEvent::FolderWatched(canon)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Lets pending events arrive, then drops them.
    fn settle(w: &FileWatcher) {
        w.sync().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        while w.try_recv().is_some() {}
    }

    /// A document in `root/a/x/doc.md` watched through the mounted folder `root/a`.
    fn doc_in_folder(name: &str, text: &str) -> (PathBuf, PathBuf, Gate, FileWatcher) {
        let root = tmp(name);
        std::fs::create_dir_all(root.join("a").join("x")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let doc = root.join("a").join("x").join("doc.md");
        std::fs::write(&doc, text).unwrap();
        let (gate, hooks) = Gate::new();
        let mut w = FileWatcher::with_hooks(Arc::new(|| {}), hooks).unwrap();
        w.watch_folder(&root.join("a"));
        w.watch_file(&doc, crate::save::disk_version(&doc).unwrap());
        settle(&w);
        (root, doc, gate, w)
    }

    /// Runs `change` while the worker is inside a registration change (folder switch).
    fn in_gap(w: &mut FileWatcher, gate: &Gate, folder: &Path, change: impl FnOnce(&FileWatcher)) {
        gate.hold();
        w.watch_folder(folder);
        gate.wait_entered();
        change(w);
        gate.release();
        w.sync().unwrap();
    }

    /// Review of #27, round 2 (P1): after the caller's own save the catch-up compares with
    /// the saved version, so an external restore of the opened version is noticed.
    #[test]
    fn catch_ups_compare_with_the_acknowledged_version() {
        let (root, doc, gate, mut w) = doc_in_folder("ack", "version A");
        std::fs::write(&doc, "version B").unwrap(); // our save
        w.acknowledge(&doc, crate::save::disk_version(&doc).unwrap());
        settle(&w);
        in_gap(&mut w, &gate, &root.join("b"), |_| {
            std::fs::write(&doc, "version A").unwrap()
        });
        assert!(wait_event(&w, &WatchEvent::FileChanged(doc.clone())));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review of #27, round 2 (P2): a detected deletion does not end later catch-ups; a
    /// recreation in another gap is reported.
    #[test]
    fn a_deletion_does_not_end_catch_ups() {
        let (root, doc, gate, mut w) = doc_in_folder("deleted", "here");
        in_gap(&mut w, &gate, &root.join("b"), |_| std::fs::remove_file(&doc).unwrap());
        assert!(wait_event(&w, &WatchEvent::FileRemoved(doc.clone())));
        settle(&w);
        in_gap(&mut w, &gate, &root.join("a"), |_| {
            std::fs::write(&doc, "recreated").unwrap()
        });
        assert!(wait_event(&w, &WatchEvent::FileChanged(doc.clone())));
        // still missing, known missing: nothing to report
        assert_eq!(reconcile(&root.join("gone.md"), Baseline::Missing).0, None);
        assert_eq!(reconcile(&root.join("gone.md"), Baseline::Unknown).0, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Spec 06 E17: editing `.obsidian/app.json` in place (no rename) is reported.
    #[test]
    fn obsidian_settings_edited_in_place_are_reported() {
        let root = tmp("obsidian");
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::write(root.join(".obsidian/app.json"), "{}").unwrap();
        let mut w = FileWatcher::new(Arc::new(|| {})).unwrap();
        w.watch_folder(&root);
        w.sync().unwrap();
        assert!(wait_event(&w, &WatchEvent::FolderWatched(root.clone())));
        // Obsidian's own state files are not settings: not reported
        std::fs::write(root.join(".obsidian/workspace.json"), "{\"x\": 1}").unwrap();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(1500) {
            while let Some(e) = w.try_recv() {
                assert_ne!(e, WatchEvent::VaultConfigChanged(root.clone()), "workspace.json");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(root.join(".obsidian/app.json"))
                .unwrap();
            f.write_all(br#"{"useMarkdownLinks": true}"#).unwrap();
        }
        assert!(wait_event(&w, &WatchEvent::VaultConfigChanged(root.clone())));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The theme folder gets the same catch-up: once its watch is in effect, the folder is
    /// reported as changed (a theme edited before that is re-imported).
    #[test]
    fn theme_folder_activation_reports_the_folder() {
        let dir = tmp("themes");
        let (gate, hooks) = Gate::new();
        let mut w = FileWatcher::with_hooks(Arc::new(|| {}), hooks).unwrap();
        gate.hold();
        w.watch_themes(&dir);
        gate.wait_entered();
        std::fs::write(dir.join("edited-in-the-gap.css"), "body {}").unwrap();
        gate.release();
        w.sync().unwrap();
        assert!(wait_event(&w, &WatchEvent::ThemeChanged(dir.clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review of #27, round 3: an acknowledgement queued while a registration change is in
    /// progress is compared with the disk once applied (save B is acknowledged during the gap,
    /// then the file is restored to A externally).
    #[test]
    fn an_acknowledgement_queued_in_a_gap_is_checked_against_the_disk() {
        let (root, doc, gate, mut w) = doc_in_folder("ack-gap", "version A");
        in_gap(&mut w, &gate, &root.join("b"), |w| {
            std::fs::write(&doc, "version B").unwrap(); // our save …
            w.acknowledge(&doc, crate::save::disk_version(&doc).unwrap()); // … acknowledged
            std::fs::write(&doc, "version A").unwrap(); // external restore
        });
        assert!(wait_event(&w, &WatchEvent::FileChanged(doc.clone())));
        let _ = std::fs::remove_dir_all(&root);
    }
}
