//! Crash backups and recent paths. One worker owns disk I/O; session locks exclude live
//! instances and keep two windows from offering the same abandoned drafts.
use crate::Wake;
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

pub enum Request {
    /// Write a draft backup: `text` with the escape patches that materialise its literal hints
    /// applied (on the worker; build it with [`Request::backup`]).
    Backup {
        id: u64,
        name: String,
        text: std::sync::Arc<str>,
        literal: Vec<markise_doc::Patch>,
    },
    Remove(u64),
    RemoveRecovered(PathBuf),
    Recent(PathBuf),
    ClearRecent,
}
impl Request {
    /// A backup of `doc` as the user sees it. Only a text snapshot and the (few) hint escape
    /// patches are taken on the caller's thread — no copy of the parsed document, which is
    /// tens of milliseconds on a 100 MiB document (issue #29); the patches are applied on the
    /// worker, so the document itself keeps its hints.
    pub fn backup(id: u64, name: String, doc: &dyn markise_doc::FormatDoc) -> Request {
        let literal = doc
            .hints()
            .iter()
            .filter_map(|h| doc.materialize_hint(h))
            .flat_map(|p| p.patches().to_vec())
            .collect();
        Request::Backup {
            id,
            name,
            text: doc.snapshot(),
            literal,
        }
    }
}

pub enum Event {
    Ready {
        drafts: Vec<PathBuf>,
        recent: Vec<PathBuf>,
        recent_files: Vec<PathBuf>,
    },
    BackedUp(u64),
    Error(String),
}
pub struct Worker {
    pub tx: Sender<Request>,
    pub rx: Receiver<Event>,
}
impl Worker {
    pub fn new(root: PathBuf, wake: Wake) -> std::io::Result<Self> {
        let (tx, requests) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("markise-recovery".into())
            .spawn(move || {
                let run = || -> std::io::Result<()> {
                    fs::create_dir_all(&root)?;
                    let mut locks = Vec::new();
                    let mut drafts = Vec::new();
                    for entry in fs::read_dir(&root)?.flatten() {
                        if !entry.file_type()?.is_dir() || !entry.file_name().to_string_lossy().starts_with("session-")
                        {
                            continue;
                        }
                        let lock = File::options()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(entry.path().join("lock"))?;
                        if lock.try_lock().is_err() {
                            continue;
                        }
                        for f in fs::read_dir(entry.path())?.flatten() {
                            if f.path().extension().is_some_and(|x| x == "md") {
                                drafts.push(f.path());
                            }
                        }
                        locks.push(lock);
                    }
                    drafts.sort();
                    let (session, lock) = claim_session(&root)?;
                    locks.push(lock);
                    let recent_file = root.join("recent.json");
                    let mut recent: Vec<PathBuf> = fs::read(&recent_file)
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default();
                    recent.truncate(30);
                    let recent_files = recent.iter().filter(|p| p.is_file()).cloned().collect();
                    let _ = events.send(Event::Ready {
                        drafts,
                        recent: recent.clone(),
                        recent_files,
                    });
                    wake();
                    let mut backups = std::collections::HashMap::new();
                    for req in requests {
                        let result = (|| -> std::io::Result<()> {
                            match req {
                                Request::Backup {
                                    id,
                                    name,
                                    text,
                                    literal,
                                } => {
                                    let text: std::borrow::Cow<str> = if literal.is_empty() {
                                        std::borrow::Cow::Borrowed(&text)
                                    } else {
                                        let patches =
                                            markise_doc::PatchSet::new(literal).map_err(std::io::Error::other)?;
                                        patches.validate(&text).map_err(std::io::Error::other)?;
                                        std::borrow::Cow::Owned(patches.apply_to(&text))
                                    };
                                    let path = backups.entry(id).or_insert_with(|| {
                                        let name = if name == "Untitled" {
                                            text.lines()
                                                .find(|s| !s.trim().is_empty())
                                                .unwrap_or("Untitled")
                                                .trim_start_matches('#')
                                                .trim()
                                        } else {
                                            Path::new(&name).file_stem().and_then(|s| s.to_str()).unwrap_or(&name)
                                        };
                                        let name: String = name
                                            .chars()
                                            .take(60)
                                            .map(|c| {
                                                if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                                                    c
                                                } else {
                                                    '_'
                                                }
                                            })
                                            .scan(0, |bytes, c| {
                                                *bytes += c.len_utf8();
                                                (*bytes <= 120).then_some(c)
                                            })
                                            .collect();
                                        session.join(format!(
                                            "{}-{id}-{name}.md",
                                            chrono::Local::now().format("%Y-%m-%d-%H%M%S")
                                        ))
                                    });
                                    atomic_write(path, text.as_bytes())?;
                                    let _ = events.send(Event::BackedUp(id));
                                }
                                Request::Remove(id) => {
                                    if let Some(p) = backups.remove(&id) {
                                        remove(&p)?;
                                    }
                                }
                                Request::RemoveRecovered(p) => {
                                    if p.starts_with(&root) {
                                        remove(&p)?;
                                    }
                                }
                                Request::Recent(p) => {
                                    // Merge under a lock so separate application windows do not lose entries.
                                    let l = File::options()
                                        .create(true)
                                        .truncate(false)
                                        .read(true)
                                        .write(true)
                                        .open(root.join("recent.lock"))?;
                                    l.lock()?;
                                    recent = fs::read(&recent_file)
                                        .ok()
                                        .and_then(|b| serde_json::from_slice(&b).ok())
                                        .unwrap_or_default();
                                    recent.retain(|v| v != &p);
                                    recent.insert(0, p);
                                    recent.truncate(30);
                                    atomic_write(&recent_file, &serde_json::to_vec(&recent)?)?;
                                }
                                Request::ClearRecent => {
                                    let l = File::options()
                                        .create(true)
                                        .truncate(false)
                                        .read(true)
                                        .write(true)
                                        .open(root.join("recent.lock"))?;
                                    l.lock()?;
                                    recent.clear();
                                    atomic_write(&recent_file, b"[]")?;
                                }
                            }
                            Ok(())
                        })();
                        if let Err(e) = result {
                            let _ = events.send(Event::Error(format!("Recovery storage: {e}")));
                        }
                        wake();
                    }
                    Ok(())
                };
                if let Err(e) = run() {
                    let _ = events.send(Event::Error(format!("Recovery storage: {e}")));
                    wake();
                }
            })?;
        Ok(Self { tx, rx })
    }
}
fn remove(p: &Path) -> std::io::Result<()> {
    match fs::remove_file(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}
/// Creates a fresh `session-*` directory and holds its lock. The directory is visible to other
/// instances before its lock is held, so one of them may claim it first (by creating the lock file
/// or winning the lock); then this worker leaves it to them and tries a new name instead of
/// blocking. The directory is never renamed: Windows refuses to rename a directory while a file
/// inside it (the held lock) is open.
fn claim_session(root: &Path) -> std::io::Result<(PathBuf, File)> {
    for _ in 0..16 {
        let session = tempfile::Builder::new().prefix("session-").tempdir_in(root)?.keep();
        let lock = match File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(session.join("lock"))
        {
            Ok(lock) => lock,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        match lock.try_lock() {
            Ok(()) => return Ok((session, lock)),
            Err(std::fs::TryLockError::WouldBlock) => continue,
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
    Err(std::io::Error::other("could not claim a session directory"))
}
fn atomic_write(p: &Path, b: &[u8]) -> std::io::Result<()> {
    let dir = p.parent().unwrap();
    let mut t = tempfile::NamedTempFile::new_in(dir)?;
    t.write_all(b)?;
    t.as_file().sync_all()?;
    t.persist(p).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};
    fn event(w: &Worker) -> Event {
        w.rx.recv_timeout(Duration::from_secs(5)).expect("worker event")
    }
    fn worker(root: &Path) -> Worker {
        Worker::new(root.to_owned(), Arc::new(|| {})).unwrap()
    }
    #[test]
    fn backup_materializes_literal_syntax_only_in_its_copy() {
        use markise_doc::{HintOrigin, HintSet, LiteralHint};
        let dir = tempfile::tempdir().unwrap();
        let w = worker(dir.path());
        event(&w);
        let mut doc = markise_md::Document::parse("# literal\n", Default::default());
        doc.set_hints(&HintSet::from_hints([LiteralHint::new(0, HintOrigin::EnterPending)]));
        w.tx.send(Request::backup(1, "Untitled".into(), &doc)).unwrap();
        assert!(matches!(event(&w), Event::BackedUp(1)));
        assert_eq!(doc.text(), "# literal\n");
        assert!(!doc.hints().is_empty());
        let path = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .flat_map(|e| fs::read_dir(e.path()).unwrap().flatten())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "md"))
            .unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "\\# literal\n");
    }

    #[test]
    fn crash_drafts_are_locked_atomic_and_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let a = worker(dir.path());
        assert!(matches!(event(&a),Event::Ready {drafts,..} if drafts.is_empty()));
        a.tx.send(Request::backup(
            1,
            "Untitled".into(),
            &markise_md::Document::parse("unsaved ✓", Default::default()),
        ))
        .unwrap();
        assert!(matches!(event(&a), Event::BackedUp(1)));
        let b = worker(dir.path());
        assert!(matches!(event(&b),Event::Ready {drafts,..} if drafts.is_empty()));
        let session = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.is_dir()
                    && fs::read_dir(p)
                        .unwrap()
                        .flatten()
                        .any(|e| e.path().extension().is_some_and(|x| x == "md"))
            })
            .unwrap();
        drop(a);
        // Wait for the first worker to release its session before simulating next launch.
        let lock = File::open(session.join("lock")).unwrap();
        lock.lock().unwrap();
        drop(lock);
        let c = worker(dir.path());
        let Event::Ready { drafts, .. } = event(&c) else {
            panic!("ready")
        };
        assert_eq!(drafts.len(), 1);
        assert_eq!(fs::read_to_string(&drafts[0]).unwrap(), "unsaved ✓");
        c.tx.send(Request::RemoveRecovered(drafts[0].clone())).unwrap();
        c.tx.send(Request::backup(
            2,
            "Untitled".into(),
            &markise_md::Document::parse("next", Default::default()),
        ))
        .unwrap();
        assert!(matches!(event(&c), Event::BackedUp(2)));
        assert!(!drafts[0].exists());
        drop(b);
    }
    #[test]
    fn recent_paths_survive_restart_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let w = worker(dir.path());
        event(&w);
        for p in ["first.md", "folder", "first.md"] {
            w.tx.send(Request::Recent(PathBuf::from(p))).unwrap();
        }
        w.tx.send(Request::backup(
            1,
            "Untitled".into(),
            &markise_md::Document::parse("", Default::default()),
        ))
        .unwrap();
        event(&w);
        let v: Vec<PathBuf> = serde_json::from_slice(&fs::read(dir.path().join("recent.json")).unwrap()).unwrap();
        assert_eq!(v, vec![PathBuf::from("first.md"), PathBuf::from("folder")]);
        w.tx.send(Request::ClearRecent).unwrap();
        w.tx.send(Request::backup(
            1,
            "Untitled".into(),
            &markise_md::Document::parse("", Default::default()),
        ))
        .unwrap();
        event(&w);
        assert_eq!(fs::read_to_string(dir.path().join("recent.json")).unwrap(), "[]");
    }
}
