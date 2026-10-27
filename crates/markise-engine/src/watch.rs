//! File watching (spec 04 §11 external changes, §4.1 tree auto-refresh) on top of `notify`.
//! The open file is watched through its parent directory (editors and our own atomic save
//! replace the file, which would end a watch on the file itself).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::Wake;

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

pub struct FileWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<WatchEvent>,
    targets: Targets,
    /// Directory watched for the open file (non-recursive).
    file_dir: Option<PathBuf>,
    folder: Option<PathBuf>,
    themes: Option<PathBuf>,
}

fn same(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

impl FileWatcher {
    pub fn new(wake: Wake) -> notify::Result<FileWatcher> {
        let (tx, rx) = channel();
        let targets: Targets = Default::default();
        let shared = targets.clone();
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
        Ok(FileWatcher {
            watcher,
            rx,
            targets,
            file_dir: None,
            folder: None,
            themes: None,
        })
    }

    /// Watches the open document (replacing the previous one).
    pub fn watch_file(&mut self, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(d) = self.file_dir.take() {
            let _ = self.watcher.unwatch(&d);
        }
        if let Some(dir) = path.parent()
            && self.watcher.watch(dir, RecursiveMode::NonRecursive).is_ok()
        {
            self.file_dir = Some(dir.to_path_buf());
        }
        if let Ok(mut t) = self.targets.lock() {
            t.file = Some(path);
        }
    }

    /// Watches the mounted folder recursively (replacing the previous one).
    pub fn watch_folder(&mut self, dir: &Path) {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Some(d) = self.folder.take() {
            let _ = self.watcher.unwatch(&d);
        }
        if self.watcher.watch(&dir, RecursiveMode::Recursive).is_ok() {
            self.folder = Some(dir.clone());
        }
        if let Ok(mut t) = self.targets.lock() {
            t.folder = Some(dir);
        }
    }

    /// Watches the user theme folder recursively (replacing the previous one); events arrive as
    /// [`WatchEvent::ThemeChanged`]. The folder must exist.
    pub fn watch_themes(&mut self, dir: &Path) {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if self.themes.as_ref() == Some(&dir) {
            return;
        }
        if let Some(d) = self.themes.take() {
            let _ = self.watcher.unwatch(&d);
        }
        if self.watcher.watch(&dir, RecursiveMode::Recursive).is_ok() {
            self.themes = Some(dir.clone());
        }
        if let Ok(mut t) = self.targets.lock() {
            t.themes = Some(dir);
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
        w.watch_file(&p);
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
}
