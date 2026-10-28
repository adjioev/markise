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
use std::time::SystemTime;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::Wake;
use crate::jobs::Hooks;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// The open document changed (modified, replaced, created).
    FileChanged(PathBuf),
    /// The open document was removed or renamed away.
    FileRemoved(PathBuf),
    /// Something changed in this directory of the mounted folder.
    DirChanged(PathBuf),
    /// A file in the user theme folder changed (live theme reload, ADR 0004).
    ThemeChanged(PathBuf),
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
    /// The open file and the modification time of the version the caller has.
    File(PathBuf, Option<SystemTime>),
    Folder(PathBuf),
    Themes(PathBuf),
    /// Answered once every earlier request is applied.
    Sync(Sender<()>),
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

/// The worker: owns the `notify` watcher and applies the latest wanted targets.
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
    while let Ok(first) = ctl.recv() {
        // coalesce a burst (fast clicks through the file tree) into one update
        let mut acks = Vec::new();
        let mut check = None;
        for c in std::iter::once(first).chain(ctl.try_iter()) {
            match c {
                Ctl::File(p, seen) => {
                    let p = canonical(&p);
                    check = Some((p.clone(), seen));
                    want.file = Some(p);
                }
                Ctl::Folder(p) => want.folder = Some(canonical(&p)),
                Ctl::Themes(p) => want.themes = Some(canonical(&p)),
                Ctl::Sync(tx) => acks.push(tx),
            }
        }
        let next = wanted(&want);
        if next != active {
            if let Some(h) = &hooks.before_watch {
                h();
            }
            // one batched change: the backend restarts at most once (FSEvents restarts its
            // stream on every individual watch/unwatch)
            let mut paths = watcher.paths_mut();
            for (p, rec) in &active {
                if next.get(p) != Some(rec) {
                    let _ = paths.remove(p);
                }
            }
            let mut failed = Vec::new();
            for (p, rec) in &next {
                if active.get(p) != Some(rec) {
                    let mode = if *rec {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    };
                    if paths.add(p, mode).is_err() {
                        failed.push(p.clone());
                    }
                }
            }
            let _ = paths.commit();
            active = next;
            for p in failed {
                active.remove(&p);
            }
        }
        if let Ok(mut t) = targets.lock() {
            *t = want.clone();
        }
        // a change between the caller's read and the watch taking effect has no event: compare
        // the file with the version the caller has
        if let Some((p, Some(seen))) = check {
            let e = match std::fs::metadata(&p).and_then(|m| m.modified()) {
                Ok(now) if now == seen => None,
                Ok(_) => Some(WatchEvent::FileChanged(p)),
                Err(_) if !p.exists() => Some(WatchEvent::FileRemoved(p)),
                Err(_) => None,
            };
            if let Some(e) = e
                && events.send(e).is_ok()
            {
                wake();
            }
        }
        for tx in acks {
            let _ = tx.send(());
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

    /// Watches the open document (replacing the previous one). Returns at once. `seen` is the
    /// modification time of the version the caller read: if the file differs once the watch is
    /// in effect, a [`WatchEvent::FileChanged`] (or `FileRemoved`) follows, so a change made
    /// in between is not lost.
    pub fn watch_file(&mut self, path: &Path, seen: Option<SystemTime>) {
        let _ = self.ctl.send(Ctl::File(path.to_path_buf(), seen));
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

    /// Blocks until every earlier `watch_*` request is in effect (tests, tools; never call it
    /// on the UI thread).
    pub fn sync(&self) {
        let (tx, rx) = channel();
        if self.ctl.send(Ctl::Sync(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    pub fn try_recv(&self) -> Option<WatchEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn reports_changes_of_the_open_file() {
        let dir = std::env::temp_dir().join(format!("markise-engine-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("w.md");
        std::fs::write(&p, "one").unwrap();
        let mut w = FileWatcher::new(Arc::new(|| {})).unwrap();
        w.watch_file(&p, None);
        w.sync();
        std::thread::sleep(Duration::from_millis(300));
        crate::save::save_atomic(&p, "two").unwrap();
        let t = Instant::now();
        let mut got = None;
        while t.elapsed() < Duration::from_secs(5) {
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
        while t.elapsed() < Duration::from_secs(5) {
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
        w.sync();
        std::thread::sleep(Duration::from_millis(300));
        crate::save::save_atomic(&p, "two").unwrap();
        let canon = std::fs::canonicalize(&p).unwrap();
        assert!(wait_event(&w, &WatchEvent::FileChanged(canon)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Watches take effect asynchronously: a change made after the caller read the file but
    /// before the watch is in effect is still reported.
    #[test]
    fn a_change_before_the_watch_takes_effect_is_reported() {
        let dir = std::env::temp_dir().join(format!("markise-engine-watch-gap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.md");
        std::fs::write(&p, "read by the caller").unwrap();
        let seen = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&p, "changed before the watch").unwrap();
        let mut w = FileWatcher::new(Arc::new(|| {})).unwrap();
        w.watch_file(&p, Some(seen));
        let canon = std::fs::canonicalize(&p).unwrap();
        assert!(wait_event(&w, &WatchEvent::FileChanged(canon)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
