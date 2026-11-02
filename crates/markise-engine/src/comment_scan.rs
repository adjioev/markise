//! "Does the document contain a review comment?" (the comment gutter, ADR 0012, issues #91
//! and #138): the scan itself, and a thread that runs it off the UI thread.
//!
//! [`scan_step`] reads the text in place from a byte position, at most about [`STEP_BYTES`]
//! or [`STEP_TIME`] at a time, and stops at the first comment. A [`CommentScanner`] runs the
//! steps on its own thread: the owner lends it the document of one scan *job* through a
//! [`DocLease`] (the session takes the loan back before every edit, so the scan never makes an
//! edit copy the document, and an edit waits at most for one step) and asks for the scan from
//! a position ([`CommentScanner::scan`]). The answer is published by job
//! ([`CommentScanner::report`]); the scanner wakes the owner once when a job is answered,
//! never while it scans. A job whose loan was taken back is abandoned: its document no longer
//! exists, and the owner starts a new job for the edited one when it wants an answer.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use markise_doc::review::marks_near;
use markise_doc::{CriticKind, FormatDoc};

use crate::Wake;
use crate::lease::DocLease;

/// Most text bytes one step reads (searching; a leaf with a comment opener is read whole).
pub const STEP_BYTES: usize = 256 * 1024;
/// Most time one step spends (checked after each comment opener).
pub const STEP_TIME: Duration = Duration::from_millis(2);

/// What one [`scan_step`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A comment.
    Found,
    /// No comment from the start position to the end of the text.
    None,
    /// No comment before this byte; continue from it.
    More(usize),
}

/// The char boundary at or before `i` (clamped to the text).
pub fn floor(text: &str, i: usize) -> usize {
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The char boundary at or after `i` (clamped to the text).
fn ceil(text: &str, i: usize) -> usize {
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Scans `doc`'s text in place from `pos` for a comment, reading about `max_bytes` and
/// spending about `max_time` at most; adds the bytes looked at to `scanned`.
pub fn scan_step(doc: &dyn FormatDoc, pos: usize, max_bytes: usize, max_time: Duration, scanned: &mut u64) -> Step {
    let text = doc.text();
    let started = Instant::now();
    let pos = floor(text, pos);
    let end = ceil(text, pos.saturating_add(max_bytes));
    // openers starting before `end` (the window reaches two bytes past it)
    let win_end = ceil(text, end.saturating_add(2));
    let mut from = pos;
    while from < end {
        let Some(rel) = text[from..win_end].find("{>>") else {
            break;
        };
        let at = from + rel;
        if at >= end {
            break;
        }
        if marks_near(doc, at).iter().any(|m| m.kind == CriticKind::Comment) {
            *scanned += (at - pos) as u64;
            return Step::Found;
        }
        // the rest of that leaf has been looked at
        let next = doc.leaf(at).map_or(at + 3, |l| l.span.end.max(at + 3));
        from = ceil(text, next);
        if started.elapsed() >= max_time && from < end {
            *scanned += (from - pos) as u64;
            return Step::More(from);
        }
    }
    let done = end.max(from).min(text.len());
    *scanned += (done - pos) as u64;
    if done >= text.len() {
        Step::None
    } else {
        Step::More(done)
    }
}

/// Whether `doc` contains a comment: the whole scan at once (tests, small documents).
pub fn has_comment(doc: &dyn FormatDoc) -> bool {
    let mut n = 0;
    let mut pos = 0;
    loop {
        match scan_step(doc, pos, usize::MAX, Duration::MAX, &mut n) {
            Step::Found => return true,
            Step::None => return false,
            Step::More(p) => pos = p,
        }
    }
}

/// What the scanner has published.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The job the answer below is for (0: none yet).
    pub job: u64,
    /// Whether that job's document has a comment from the job's start position on.
    pub found: bool,
    /// Text bytes that job read.
    pub scanned: u64,
    /// Steps run, over all jobs (tests).
    pub steps: u64,
    /// The most bytes one step read (tests: every step under the lease is bounded).
    pub max_step_bytes: u64,
}

/// The scanning thread and its channel (see the module docs). Dropping it takes the document
/// back and stops the thread.
pub struct CommentScanner {
    tx: Option<Sender<(u64, usize)>>,
    lease: Arc<DocLease>,
    out: Arc<Mutex<Report>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for CommentScanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommentScanner")
            .field("report", &self.report())
            .finish()
    }
}

impl CommentScanner {
    /// Starts the thread; `wake` is called (from the scanner) when a job is answered.
    pub fn spawn(wake: Wake) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        let lease = Arc::new(DocLease::new());
        let out = Arc::new(Mutex::new(Report::default()));
        let (l, o) = (lease.clone(), out.clone());
        let thread = std::thread::Builder::new()
            .name("markise-comments".into())
            .spawn(move || run(rx, &l, &o, &wake))?;
        Ok(CommentScanner {
            tx: Some(tx),
            lease,
            out,
            thread: Some(thread),
        })
    }

    /// The lease the session revokes before each edit
    /// ([`crate::editor::EditorSession::add_doc_lease`]).
    pub fn lease(&self) -> &Arc<DocLease> {
        &self.lease
    }

    /// Scans `doc` from byte `from` as job `job` (jobs grow); `doc` is lent until the
    /// session's next edit. Replaces a job in progress.
    pub fn scan(&self, job: u64, from: usize, doc: Arc<dyn FormatDoc>) {
        self.lease.lend(job, doc);
        if let Some(tx) = &self.tx {
            // a scanner that died (panicked) leaves the last answer standing
            let _ = tx.send((job, from));
        }
    }

    /// What the scanner has published so far.
    pub fn report(&self) -> Report {
        *self.out.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for CommentScanner {
    fn drop(&mut self) {
        self.lease.revoke();
        self.tx = None; // the thread's `recv` sees the disconnect
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(rx: Receiver<(u64, usize)>, lease: &DocLease, out: &Mutex<Report>, wake: &Wake) {
    // the job in progress: (job, next byte, bytes read so far)
    let mut current: Option<(u64, usize, u64)> = None;
    let (mut steps, mut max_step) = (0u64, 0u64);
    loop {
        let msg = if current.is_some() {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        if let Some((job, from)) = msg {
            current = Some((job, from, 0));
            continue; // the newest job wins
        }
        let Some((job, pos, read)) = current else {
            continue;
        };
        let step = lease.with(job, |doc| {
            let mut n = 0;
            let s = scan_step(doc, pos, STEP_BYTES, STEP_TIME, &mut n);
            (s, n)
        });
        let Some((step, n)) = step else {
            // the loan was taken back (an edit): the job's document is gone
            current = None;
            continue;
        };
        steps += 1;
        max_step = max_step.max(n);
        let read = read + n;
        let found = match step {
            Step::More(p) => {
                current = Some((job, p, read));
                continue;
            }
            Step::Found => true,
            Step::None => false,
        };
        current = None;
        // a result the owner waits for: counted until its wake-up has returned (tests)
        let guard = crate::wake_track::publishing();
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Report {
            job,
            found,
            scanned: read,
            steps,
            max_step_bytes: max_step,
        };
        wake();
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_doc::{DocumentFormat, FormatOptions, Patch, PatchSet};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn parse(text: &str) -> Arc<dyn FormatDoc> {
        let opts = markise_md::ParseOptions {
            critic_markup: true,
            ..Default::default()
        };
        Arc::from(markise_md::MarkdownFormat { opts }.parse(text.to_string(), &FormatOptions::default()))
    }

    fn scanner() -> (CommentScanner, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let s = CommentScanner::spawn(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        (s, wakes)
    }

    fn answer(s: &CommentScanner, job: u64) -> Report {
        let t = Instant::now();
        loop {
            let r = s.report();
            if r.job == job {
                return r;
            }
            assert!(t.elapsed() < Duration::from_secs(60), "no answer for job {job}: {r:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn finds_comments_and_ignores_lookalikes() {
        for (src, want) in [
            ("plain text\n", false),
            ("a {>>note<<} b\n", true),
            ("`{>>code<<}`\n", false),
            ("```\n{>>in a fence<<}\n```\n", false),
            ("    {>>indented code<<}\n", false),
            ("a {++insert++} b {==hl==}\n", false),
            ("x\n\n```\ncode {>>no<<}\n```\n\ny {>>yes<<}\n", true),
        ] {
            assert_eq!(has_comment(&*parse(src)), want, "{src:?}");
        }
    }

    /// Codex round 3 of #94: any start position is clamped to a character boundary.
    #[test]
    fn positions_inside_characters_are_safe() {
        let d = parse("Обычный абзац {>>заметка<<}.\n");
        let mut n = 0;
        for pos in 0..64 {
            let _ = scan_step(&*d, pos, 8, STEP_TIME, &mut n);
        }
    }

    /// Issue #138: the scanner's answers equal a full scan, after random edits that add,
    /// remove, split and hide comments, made while it scans (the owner takes the document back
    /// before each edit, as the session does), with jobs that start mid-document.
    #[test]
    fn answers_equal_a_full_scan_after_random_edits() {
        let para = "An ordinary paragraph with `code` and *emphasis* in it.\n\n";
        let mut src = para.repeat((3 * STEP_BYTES) / para.len());
        src.push_str("Tail {>>the only comment<<} here.\n");
        let mut doc = parse(&src);
        let (s, wakes) = scanner();
        s.scan(1, 0, doc.clone());
        assert!(answer(&s, 1).found);
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "one wake per answer");
        let inserts = [
            "{>>x<<}",
            "{>>",
            "<<}",
            "`",
            "\n```\n",
            "\n\n",
            "    ",
            " plain ",
            "{++add++}",
            "\n~~~\n",
        ];
        let mut x = 23u64;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) as usize
        };
        let mut job = 1;
        let (mut yes, mut no) = (0, 0);
        for round in 0..80 {
            for _ in 0..1 + next() % 3 {
                s.lease().revoke();
                let d = Arc::get_mut(&mut doc).expect("the scanner holds no reference after a revoke");
                let text = d.text();
                // edits near the comments and anywhere
                let at = match next() % 3 {
                    0 => text.find("{>>").unwrap_or(0),
                    1 => text.rfind("<<}").unwrap_or(text.len()),
                    _ => next() % (text.len() + 1),
                };
                let at = floor(text, at);
                let end = floor(text, (at + next() % 12).min(text.len()));
                let ins = inserts[next() % inserts.len()];
                let range = if next() % 3 == 0 { at..end } else { at..at };
                d.apply(&PatchSet::new(vec![Patch::verbatim(range, ins)]).unwrap());
            }
            job += 1;
            // sometimes from the start, sometimes after the first part (as when the owner's
            // first step found nothing)
            let from = if round % 2 == 0 {
                0
            } else {
                floor(doc.text(), STEP_BYTES / 2)
            };
            s.scan(job, from, doc.clone());
            if round % 3 == 0 {
                // sometimes answered before the next edit, sometimes abandoned by it
                let want = {
                    let mut n = 0;
                    let mut pos = from;
                    loop {
                        match scan_step(&*doc, pos, usize::MAX, Duration::MAX, &mut n) {
                            Step::Found => break true,
                            Step::None => break false,
                            Step::More(p) => pos = p,
                        }
                    }
                };
                let r = answer(&s, job);
                assert_eq!(r.found, want, "round {round}");
                if want { yes += 1 } else { no += 1 }
            }
        }
        assert!(yes > 0 && no > 0, "both answers were exercised: {yes} yes, {no} no");
        job += 1;
        s.scan(job, 0, doc.clone());
        assert_eq!(answer(&s, job).found, has_comment(&*doc));
    }

    /// Issue #138: a scan of a large document runs in bounded steps; taking the document back
    /// for an edit waits for one step, not for the whole scan.
    #[test]
    fn edits_during_a_scan_wait_one_step() {
        // many comment openers that are not comments (code spans): a slow scan
        let line = "Words `{>>not a comment<<}` and more words around it.\n\n";
        let src = line.repeat((4 << 20) / line.len());
        let doc = parse(&src);
        let t = Instant::now();
        assert!(!has_comment(&*doc));
        let whole = t.elapsed();

        let (s, wakes) = scanner();
        let mut longest = Duration::ZERO;
        let mut x = 7u64;
        for job in 1..=20 {
            s.scan(job, 0, doc.clone());
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            std::thread::sleep(Duration::from_micros(500 + (x >> 33) % 3000));
            let t = Instant::now();
            s.lease().revoke();
            longest = longest.max(t.elapsed());
        }
        assert_eq!(Arc::strong_count(&doc), 1, "nothing holds the document after a revoke");
        assert_eq!(s.report().job, 0, "every job was abandoned by its edit");
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "no wake for an abandoned job");
        s.scan(21, 0, doc.clone());
        let r = answer(&s, 21);
        assert!(!r.found);
        assert_eq!(r.scanned, src.len() as u64);
        assert!(r.steps > 1);
        assert!(
            r.max_step_bytes <= (STEP_BYTES + 256) as u64,
            "every step under the lease stays within the byte budget: {r:?}"
        );
        assert!(
            longest * 4 < whole,
            "an edit waited {longest:?}; the whole scan takes {whole:?}"
        );
    }

    /// Dropping the scanner takes the document back and stops the thread.
    #[test]
    fn drop_releases_the_document() {
        let doc = parse("a {>>b<<}\n");
        let (s, _) = scanner();
        s.scan(1, 0, doc.clone());
        assert!(answer(&s, 1).found);
        assert_eq!(Arc::strong_count(&doc), 2, "lent while the document is unchanged");
        drop(s);
        assert_eq!(Arc::strong_count(&doc), 1);
    }
}
