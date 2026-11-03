//! In-document source search. See ADR 0013 for matching and snapshot semantics.
use crate::Wake;
use markise_doc::{Patch, PatchSet};
use regex::{Regex, RegexBuilder};
use std::{
    ops::Range,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

/// Maximum source copied by the UI during one frame; UTF-8 boundaries may shorten it.
pub const SNAPSHOT_CHUNK: usize = 64 * 1024;
/// Explicit resource limits: an excessive result fails rather than truncating replacement.
pub const MAX_MATCHES: usize = 100_000;
pub const MAX_REPLACEMENT_BYTES: usize = 64 * 1024 * 1024;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub document: u64,
    pub revision: u64,
    pub generation: u64,
}
#[derive(Debug)]
pub struct Results {
    pub key: Key,
    pub matches: Arc<Vec<Range<usize>>>,
    pub error: Option<String>,
}
impl Results {
    /// Wrap around the document, starting from the caret when there is no active result.
    pub fn next(&self, active: Option<usize>, caret: usize, backwards: bool) -> Option<usize> {
        let n = self.matches.len();
        if n == 0 {
            return None;
        }
        Some(match active {
            Some(i) if backwards => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
            None if backwards => self.matches.iter().rposition(|r| r.start < caret).unwrap_or(n - 1),
            None => self.matches.iter().position(|r| r.start >= caret).unwrap_or(0),
        })
    }
}
fn compile(q: &Query) -> Result<Regex, String> {
    let p = if q.regex {
        q.text.clone()
    } else {
        regex::escape(&q.text)
    };
    let p = if q.whole_word { format!(r"\b(?:{p})\b") } else { p };
    RegexBuilder::new(&p)
        .case_insensitive(!q.case_sensitive)
        .size_limit(4 << 20)
        .build()
        .map_err(|e| e.to_string())
}
/// Pure matching entry point for benchmarks and headless callers.
pub fn find(text: &str, q: &Query) -> Result<Vec<Range<usize>>, String> {
    find_cancellable(text, q, || false)
}
fn find_cancellable(text: &str, q: &Query, cancelled: impl Fn() -> bool) -> Result<Vec<Range<usize>>, String> {
    if q.text.is_empty() {
        return Ok(Vec::new());
    }
    let re = compile(q)?;
    let mut out = Vec::new();
    for m in re.find_iter(text) {
        if cancelled() {
            return Err("Search cancelled".into());
        }
        if out.len() == MAX_MATCHES {
            return Err(format!("More than {MAX_MATCHES} matches; narrow the search"));
        }
        out.push(m.range());
    }
    Ok(out)
}
/// Regex replacements expand `$1`/`${name}`/`$$`; literal mode preserves all bytes.
/// Nothing is returned on overflow, cancellation or an invalid expression.
pub fn replacements(text: &str, q: &Query, replacement: &str, only: Option<usize>) -> Result<PatchSet, String> {
    replacement_cancellable(text, q, replacement, only, || false)
}
fn replacement_cancellable(
    text: &str,
    q: &Query,
    replacement: &str,
    only: Option<usize>,
    cancelled: impl Fn() -> bool,
) -> Result<PatchSet, String> {
    if q.text.is_empty() {
        return Ok(PatchSet::default());
    }
    let re = compile(q)?;
    let mut patches = Vec::new();
    let mut bytes = 0usize;
    for (i, c) in re.captures_iter(text).enumerate() {
        if cancelled() {
            return Err("Search cancelled".into());
        }
        if i == MAX_MATCHES {
            return Err(format!("More than {MAX_MATCHES} matches; narrow the search"));
        }
        if only.is_some_and(|j| i != j) {
            continue;
        }
        let m = c.get(0).unwrap();
        let mut insert = String::new();
        if q.regex {
            expand_bounded(
                &c,
                replacement,
                &mut insert,
                MAX_REPLACEMENT_BYTES.saturating_sub(bytes).saturating_sub(m.len()),
            )?;
        } else {
            if replacement.len().saturating_add(bytes).saturating_add(m.len()) > MAX_REPLACEMENT_BYTES {
                return Err("Replacement exceeds the 64 MiB transaction limit".into());
            }
            insert.push_str(replacement);
        }
        bytes = bytes.saturating_add(insert.len()).saturating_add(m.len());
        if bytes > MAX_REPLACEMENT_BYTES {
            return Err("Replacement exceeds the 64 MiB transaction limit".into());
        }
        patches.push(Patch::verbatim(m.range(), insert));
        if only.is_some() {
            break;
        }
    }
    PatchSet::new(patches).map_err(|e| e.to_string())
}
// Expand without ever allocating beyond the transaction byte allowance.
fn expand_bounded(c: &regex::Captures<'_>, replacement: &str, out: &mut String, limit: usize) -> Result<(), String> {
    let append = |out: &mut String, part: &str| -> Result<(), String> {
        if out.len().saturating_add(part.len()) > limit {
            return Err("Replacement exceeds the 64 MiB transaction limit".into());
        }
        out.push_str(part);
        Ok(())
    };
    let mut rest = replacement;
    while let Some(i) = rest.find('$') {
        append(out, &rest[..i])?;
        rest = &rest[i + 1..];
        if let Some(tail) = rest.strip_prefix('$') {
            append(out, "$")?;
            rest = tail;
            continue;
        }
        let (name, tail) = if let Some(braced) = rest.strip_prefix('{') {
            if let Some(end) = braced.find('}') {
                (&braced[..end], &braced[end + 1..])
            } else {
                append(out, "$")?;
                continue;
            }
        } else {
            let n = rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if n == 0 {
                append(out, "$")?;
                continue;
            }
            (&rest[..n], &rest[n..])
        };
        let value = name
            .parse::<usize>()
            .ok()
            .and_then(|i| c.get(i))
            .or_else(|| c.name(name));
        if let Some(value) = value {
            append(out, value.as_str())?;
        }
        rest = tail;
    }
    append(out, rest)
}
struct Snapshot {
    key: Key,
    chunks: Vec<String>,
    query: Query,
}
enum Job {
    Find(Snapshot),
    Replace(Snapshot, String, Option<usize>),
}
#[derive(Debug)]
pub enum Completion {
    Found(Results),
    Replaced(Key, Result<PatchSet, String>),
}
struct Shared {
    job: Option<Job>,
    stop: bool,
}
/// One sleeping worker and one latest-job slot. Never retains a document Arc.
pub struct SearchWorker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    generation: Arc<AtomicU64>,
    rx: mpsc::Receiver<Completion>,
}
impl SearchWorker {
    pub fn new(wake: Wake) -> Self {
        let shared = Arc::new((Mutex::new(Shared { job: None, stop: false }), Condvar::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        let g = generation.clone();
        thread::Builder::new()
            .name("markise-search".into())
            .spawn(move || {
                loop {
                    let job = {
                        let (lock, cv) = &*s;
                        let mut st = lock.lock().unwrap();
                        while st.job.is_none() && !st.stop {
                            st = cv.wait(st).unwrap();
                        }
                        if st.stop {
                            break;
                        }
                        st.job.take().unwrap()
                    };
                    let key = match &job {
                        Job::Find(s) | Job::Replace(s, ..) => s.key,
                    };
                    let cancelled = || g.load(Ordering::Relaxed) != key.generation;
                    if cancelled() {
                        continue;
                    }
                    let result = match job {
                        Job::Find(s) => match find_cancellable(&s.chunks.concat(), &s.query, cancelled) {
                            Ok(matches) => Completion::Found(Results {
                                key,
                                matches: Arc::new(matches),
                                error: None,
                            }),
                            Err(error) => Completion::Found(Results {
                                key,
                                matches: Arc::new(Vec::new()),
                                error: Some(error),
                            }),
                        },
                        Job::Replace(s, replacement, only) => Completion::Replaced(
                            key,
                            replacement_cancellable(&s.chunks.concat(), &s.query, &replacement, only, cancelled),
                        ),
                    };
                    if !cancelled() {
                        if tx.send(result).is_err() {
                            break;
                        }
                        wake();
                    }
                }
            })
            .expect("search worker");
        Self { shared, generation, rx }
    }
    pub fn cancel(&self, generation: u64) {
        self.generation.store(generation, Ordering::Relaxed);
        self.shared.0.lock().unwrap().job = None;
    }
    pub fn submit(&self, key: Key, chunks: Vec<String>, query: Query, replacement: Option<(String, Option<usize>)>) {
        self.generation.store(key.generation, Ordering::Relaxed);
        let snap = Snapshot { key, chunks, query };
        let job = match replacement {
            Some((r, i)) => Job::Replace(snap, r, i),
            None => Job::Find(snap),
        };
        self.shared.0.lock().unwrap().job = Some(job);
        self.shared.1.notify_one();
    }
    pub fn poll(&self) -> Option<Completion> {
        self.rx.try_recv().ok()
    }
}
impl Drop for SearchWorker {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        let mut s = self.shared.0.lock().unwrap();
        s.stop = true;
        s.job = None;
        self.shared.1.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_words_regex_and_navigation() {
        let q = Query {
            text: "é".into(),
            whole_word: true,
            ..Query::default()
        };
        let matches = find("É été é", &q).unwrap();
        assert_eq!(matches, vec![0..2, 9..11]);
        let r = Results {
            key: Key {
                document: 1,
                revision: 0,
                generation: 1,
            },
            matches: Arc::new(matches),
            error: None,
        };
        assert_eq!(r.next(None, 3, false), Some(1));
        assert_eq!(r.next(Some(1), 0, false), Some(0));
        assert_eq!(r.next(Some(0), 0, true), Some(1));
        assert!(
            find(
                "a",
                &Query {
                    text: "[".into(),
                    regex: true,
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(
            find(
                "é",
                &Query {
                    text: "^|$".into(),
                    regex: true,
                    ..Default::default()
                }
            )
            .unwrap(),
            vec![0..0, 2..2]
        );
    }
    #[test]
    fn replacements_are_atomic_and_capture_aware() {
        let q = Query {
            text: "(a)(b)".into(),
            regex: true,
            ..Default::default()
        };
        let p = replacements("ab -- ab", &q, "$2$1", None).unwrap();
        let mut session =
            crate::editor::EditorSession::new(Arc::new(markise_md::MarkdownFormat::default()), "ab -- ab");
        session.apply_external(&p, "Replace All", false).unwrap();
        assert_eq!(session.text(), "ba -- ba");
        assert!(session.undo());
        assert_eq!(session.text(), "ab -- ab");
        assert_eq!(find("abc", &Query::default()).unwrap(), Vec::<Range<usize>>::new());
    }
    #[test]
    fn bounded_expansion_limits_and_worker_latest_request() {
        let re = Regex::new("(a+)").unwrap();
        let c = re.captures("aaaa").unwrap();
        let mut out = String::new();
        assert!(expand_bounded(&c, "$0$0$0", &mut out, 10).is_err());
        assert!(out.len() <= 10);
        assert!(
            find(
                &"a".repeat(MAX_MATCHES + 1),
                &Query {
                    text: "a".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            find_cancellable(
                "a",
                &Query {
                    text: "a".into(),
                    ..Default::default()
                },
                || true
            )
            .is_err()
        );
        let (tx, rx) = mpsc::channel();
        let worker = SearchWorker::new(Arc::new(move || {
            let _ = tx.send(());
        }));
        for generation in 1..=2 {
            worker.submit(
                Key {
                    document: 1,
                    revision: 0,
                    generation,
                },
                vec!["alpha".into()],
                Query {
                    text: if generation == 1 { "old" } else { "alpha" }.into(),
                    ..Default::default()
                },
                None,
            );
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(Completion::Found(r)) = worker.poll()
                && r.key.generation == 2
            {
                assert_eq!(*r.matches, vec![0..5]);
                break;
            }
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("worker completion");
        }
    }
    #[test]
    #[ignore = "heavy: multi-MB search workload"]
    fn heavy_suite_ten_megabyte_search() {
        let text = "αβ paragraph\n".repeat(800_000);
        assert!(
            find(
                &text,
                &Query {
                    text: "absent".into(),
                    ..Default::default()
                }
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            find(
                &text,
                &Query {
                    text: "αβ".into(),
                    ..Default::default()
                }
            )
            .is_err(),
            "excessive matches fail rather than truncate"
        );
    }
}
