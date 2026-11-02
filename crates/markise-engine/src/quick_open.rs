//! Bounded, cancellable filename indexing and fuzzy matching; never runs on the UI thread.
use crate::{Wake, tree::is_markdown};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};
pub struct Query {
    pub generation: u64,
    pub text: String,
}
pub struct Results {
    pub generation: u64,
    pub recent: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub limited: bool,
    pub unreadable: bool,
}
pub struct Search {
    pub tx: Sender<Query>,
    pub rx: Receiver<Results>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Search {
    pub fn new(roots: Vec<PathBuf>, recent: Vec<PathBuf>, wake: Wake) -> std::io::Result<Self> {
        let (tx, queries) = mpsc::channel::<Query>();
        let (results, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::Builder::new()
            .name("markise-quick-open".into())
            .spawn(move || {
                let mut pending = roots;
                let mut seen = std::collections::HashSet::new();
                let mut files = Vec::new();
                let mut visited = 0;
                let mut limited = false;
                let mut unreadable = false;
                while let Some(dir) = pending.pop() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if !seen.insert(dir.clone()) {
                        continue;
                    }
                    let Ok(entries) = std::fs::read_dir(dir) else {
                        unreadable = true;
                        continue;
                    };
                    for entry in entries {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        visited += 1;
                        if visited > 100_000 {
                            limited = true;
                            break;
                        }
                        let Ok(e) = entry else {
                            unreadable = true;
                            continue;
                        };
                        if e.file_name().to_string_lossy().starts_with('.') {
                            continue;
                        }
                        let Ok(kind) = e.file_type() else {
                            continue;
                        };
                        if kind.is_dir() {
                            pending.push(e.path());
                        } else if kind.is_file() && is_markdown(&e.path()) {
                            files.push(e.path());
                        }
                    }
                    if limited {
                        break;
                    }
                }
                files.sort();
                files.dedup();
                let recent: Vec<_> = recent.into_iter().filter(|p| p.is_file() && is_markdown(p)).collect();
                while let Ok(mut q) = queries.recv() {
                    for next in queries.try_iter() {
                        q = next;
                    }
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let recent: Vec<_> = recent
                        .iter()
                        .filter(|p| {
                            p.file_name()
                                .is_some_and(|n| score(&n.to_string_lossy(), &q.text).is_some())
                        })
                        .take(10)
                        .cloned()
                        .collect();
                    let mut found = rank(&files, &q.text, 60);
                    found.retain(|p| !recent.contains(p));
                    let _ = results.send(Results {
                        generation: q.generation,
                        recent,
                        files: found,
                        limited,
                        unreadable,
                    });
                    wake();
                }
            })?;
        Ok(Self { tx, rx, cancel })
    }
}
/// Subsequence score: contiguous runs and early matches win; exact names sort first.
fn score(name: &str, query: &str) -> Option<usize> {
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    if name == query {
        return Some(0);
    }
    let mut chars = name.chars().enumerate();
    let mut cost = 1;
    let mut previous = None;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let (at, _) = chars.find(|(_, c)| *c == wanted)?;
        cost += match previous {
            Some(p) => at - p - 1,
            None => at * 2,
        };
        previous = Some(at);
    }
    Some(cost * 1000 + name.chars().count())
}
fn rank(paths: &[PathBuf], q: &str, limit: usize) -> Vec<PathBuf> {
    let mut matched: Vec<_> = paths
        .iter()
        .filter_map(|p| score(&p.file_name()?.to_string_lossy(), q).map(|s| (s, p)))
        .collect();
    matched.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    matched.truncate(limit);
    matched.into_iter().map(|(_, p)| p.clone()).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filename_subsequence_unicode_and_order() {
        assert!(score("ქართული.md", "ქრთ").is_some());
        assert!(score("note.md", "xyz").is_none());
        let paths = vec![
            PathBuf::from("z/note.md"),
            PathBuf::from("a/notebook.md"),
            PathBuf::from("note.md/unrelated.md"),
        ];
        assert_eq!(rank(&paths, "note.md", 10)[0], PathBuf::from("z/note.md"));
        assert!(!rank(&paths, "note.md", 10).contains(&paths[2]));
        assert!(score("notes.md", "note").unwrap() < score("north-east.md", "note").unwrap());
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    #[test]
    fn scans_nested_files_but_not_hidden_or_symlink_trees() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("nested")).unwrap();
        std::fs::create_dir(d.path().join(".hidden")).unwrap();
        for p in ["nested/report.md", ".hidden/report.md", "report.png"] {
            std::fs::write(d.path().join(p), "").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path(), d.path().join("loop")).unwrap();
        let s = Search::new(vec![d.path().to_owned()], Vec::new(), Arc::new(|| {})).unwrap();
        s.tx.send(Query {
            generation: 7,
            text: "rpt".into(),
        })
        .unwrap();
        let r = s.rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(r.generation, 7);
        assert_eq!(r.files, vec![d.path().join("nested/report.md")]);
        assert!(!r.limited);
    }
}
