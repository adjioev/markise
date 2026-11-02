//! "Does the document contain a review comment?" for the comment gutter (ADR 0012, issue #91),
//! tracked incrementally and without copying the text.
//!
//! - When the document object is new (open, reload, re-parse) the text is scanned in place
//!   from the start until the first comment is found. The first step (about
//!   [`STEP_BYTES`] or [`STEP_TIME`]) runs at once on the UI thread, so a small document, or
//!   one with a comment near its start, is answered in the same frame; the rest of a larger
//!   document is scanned by a [`CommentScanner`] thread (issue #138), which borrows the
//!   session's document through a lease (the session takes it back before every edit, so the
//!   scan never copies the document and an edit waits at most one step) and wakes the UI once
//!   when it has the answer. No frame is requested while it scans.
//! - After an edit nothing is looked at unless the edit was review-relevant (the session's
//!   [`ReviewEdits`]: the edit's removed or inserted text, with a few bytes around it, holds a
//!   comment delimiter or block syntax that could turn a comment into code). Then the leaf of
//!   the edit is checked first (inserting a comment flips the answer to "yes" without a scan);
//!   only otherwise is the scan run again (e.g. after deleting a comment: was it the last
//!   one?).
//! - Evaluations start at most every [`RECHECK`] seconds, so the layout does not jump per
//!   keystroke; between them the last answer stands.
//! - Any edit drops a running scan (its document is gone; the edit took the scanner's loan
//!   back); it starts over from the beginning.
//! - Safety net for what the triggers miss (e.g. unindenting a code line that holds a
//!   comment): once the document has been idle for [`IDLE_RESCAN`] seconds and was edited
//!   since its last full scan, the scan runs again. Staleness is thus bounded to a few
//!   seconds in every case.
//! - If the scanner thread cannot be started, the rest of the scan runs here, one step per
//!   frame (the behaviour before #138).

use std::sync::Arc;

use markise_doc::review::marks_near;
use markise_doc::{CriticKind, FormatDoc};
use markise_engine::comment_scan::{CommentScanner, STEP_BYTES, STEP_TIME, Step, floor, scan_step};
use markise_engine::editor::{EditorSession, ReviewEdits};

/// Evaluations start at most this often (seconds).
pub const RECHECK: f64 = 0.4;
/// Safety net: after this long without edits (seconds), a document edited since its last full
/// scan is scanned again, so an edit the relevance triggers miss is caught within seconds.
pub const IDLE_RESCAN: f64 = 2.0;

/// Which document state an answer is for: (relevant edits, replacements) of the session.
type Key = (u64, u64);

fn key_of(r: &ReviewEdits) -> Key {
    (r.relevant, r.replaced)
}

/// Where the rest of a scan runs.
#[derive(Debug, Clone, Copy)]
enum Rest {
    /// On the scanner thread, as this job.
    Scanner(u64),
    /// Here, one step per frame, from this byte (the thread could not be started).
    Here(usize),
}

/// A scan in progress, valid only for the text revision it started on.
#[derive(Debug, Clone, Copy)]
struct Scan {
    key: Key,
    rev: u64,
    rest: Rest,
}

#[derive(Debug, Default)]
pub struct CommentPresence {
    /// The state the current answer is for.
    seen: Option<Key>,
    /// The state of the document at the last poll.
    wanted: Option<Key>,
    has: bool,
    /// When the last evaluation started (egui time).
    at: f64,
    scan: Option<Scan>,
    /// The text revision at the last poll, and when it last changed (egui time).
    rev: Option<u64>,
    last_edit: f64,
    /// The text revision the last full scan completed on.
    scanned_rev: Option<u64>,
    /// The scanner thread (started at the first scan that needs it; `Err` when it could not
    /// start).
    scanner: Option<Result<CommentScanner, ()>>,
    /// The last job given to the scanner.
    job: u64,
    /// Full scans started (test seam).
    pub scans: u64,
    /// Text bytes read by them (test seam; the scanner's share once it has answered).
    pub scanned: u64,
}

impl CommentPresence {
    /// The answer for the document of `session` at time `now`; and, when more work is due on
    /// this thread, the seconds until the next frame should run. A scan running on the
    /// scanner thread wakes `ctx` once when it has the answer.
    pub fn poll(&mut self, session: &mut EditorSession, now: f64, ctx: &egui::Context) -> (bool, Option<f64>) {
        let doc = session.doc_arc().clone();
        let out = self.poll_doc(&doc, session.review_edits(), session.revision().counter, now, ctx);
        drop(doc);
        if let Some(Ok(s)) = &self.scanner {
            // before the next edit: the session takes the scanner's loan back first
            session.add_doc_lease(s.lease().clone());
        }
        out
    }

    /// [`CommentPresence::poll`] for the document `doc` at text revision `rev`, whose session
    /// reports `review`.
    fn poll_doc(
        &mut self,
        doc: &Arc<dyn FormatDoc>,
        review: ReviewEdits,
        rev: u64,
        now: f64,
        ctx: &egui::Context,
    ) -> (bool, Option<f64>) {
        let key = key_of(&review);
        self.wanted = Some(key);
        if self.rev != Some(rev) {
            if self.rev.is_some() {
                self.last_edit = now;
            }
            self.rev = Some(rev);
        }
        if let Some(s) = self.scan {
            if s.rev == rev && s.key == key {
                return self.resume(doc.as_ref(), s);
            }
            // any edit, relevant or not, replaced the document under the scan (and took the
            // scanner's loan back): drop it; it starts over below (debounced), or, if it was
            // the idle safety net's, after the next idle period
            self.scan = None;
        }
        if self.seen != Some(key) {
            let wait = RECHECK - (now - self.at);
            if self.seen.is_some() && wait > 0.0 {
                return (self.has, Some(wait));
            }
            self.at = now;
            let replaced = self.seen.is_none_or(|(_, r)| r != review.replaced);
            if !replaced {
                // an edit that could add or remove a comment: its own leaf first
                let at = floor(doc.text(), review.last_at);
                if marks_near(doc.as_ref(), at)
                    .iter()
                    .any(|m| m.kind == CriticKind::Comment)
                {
                    self.has = true;
                    self.seen = Some(key);
                    return (true, None);
                }
            }
            return self.start(doc, key, rev, ctx);
        }
        // up to date as far as the triggers know; the safety net for what they miss
        if self.scanned_rev != Some(rev) {
            let idle = IDLE_RESCAN - (now - self.last_edit);
            if idle > 0.0 {
                return (self.has, Some(idle));
            }
            return self.start(doc, key, rev, ctx);
        }
        (self.has, None)
    }

    /// An evaluation is still due or running (tests; the idle safety net does not count).
    pub fn pending(&self) -> bool {
        self.scan.is_some() || self.wanted != self.seen
    }

    /// Test hook ("background settled", issue #42): nothing is due any more, the idle safety
    /// net's rescan of an edited document included (it fires [`IDLE_RESCAN`] after the last
    /// edit and asks for a frame then).
    pub fn settled(&self) -> bool {
        !self.pending() && self.scanned_rev == self.rev
    }

    /// Starts a full scan: its first step here, the rest on the scanner thread.
    fn start(&mut self, doc: &Arc<dyn FormatDoc>, key: Key, rev: u64, ctx: &egui::Context) -> (bool, Option<f64>) {
        self.scans += 1;
        let pos = match scan_step(doc.as_ref(), 0, STEP_BYTES, STEP_TIME, &mut self.scanned) {
            Step::Found => return self.finish(key, rev, true),
            Step::None => return self.finish(key, rev, false),
            Step::More(pos) => pos,
        };
        if self.scanner.is_none() {
            let c = ctx.clone();
            self.scanner = Some(CommentScanner::spawn(Arc::new(move || c.request_repaint())).map_err(|_| ()));
        }
        let rest = match &self.scanner {
            Some(Ok(s)) => {
                self.job += 1;
                s.scan(self.job, pos, doc.clone());
                Rest::Scanner(self.job)
            }
            _ => Rest::Here(pos),
        };
        self.scan = Some(Scan { key, rev, rest });
        (self.has, matches!(rest, Rest::Here(_)).then_some(0.0))
    }

    /// Continues the scan `s` of `doc`: picks up the scanner's answer, or runs a step here.
    fn resume(&mut self, doc: &dyn FormatDoc, s: Scan) -> (bool, Option<f64>) {
        match s.rest {
            Rest::Scanner(job) => {
                let r = match &self.scanner {
                    Some(Ok(w)) => w.report(),
                    _ => Default::default(),
                };
                if r.job != job {
                    // still scanning: the scanner wakes the UI when it has the answer
                    return (self.has, None);
                }
                self.scanned += r.scanned;
                self.finish(s.key, s.rev, r.found)
            }
            Rest::Here(pos) => match scan_step(doc, pos, STEP_BYTES, STEP_TIME, &mut self.scanned) {
                Step::Found => self.finish(s.key, s.rev, true),
                Step::None => self.finish(s.key, s.rev, false),
                Step::More(pos) => {
                    self.scan = Some(Scan {
                        rest: Rest::Here(pos),
                        ..s
                    });
                    (self.has, Some(0.0))
                }
            },
        }
    }

    fn finish(&mut self, key: Key, rev: u64, has: bool) -> (bool, Option<f64>) {
        self.has = has;
        self.seen = Some(key);
        self.scanned_rev = Some(rev);
        self.scan = None;
        (has, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_doc::{DocumentFormat, FormatOptions};
    use markise_engine::comment_scan::has_comment;
    use std::time::{Duration, Instant};

    fn format() -> markise_md::MarkdownFormat {
        markise_md::MarkdownFormat {
            opts: markise_md::ParseOptions {
                critic_markup: true,
                ..Default::default()
            },
        }
    }

    fn doc(text: &str) -> Arc<dyn FormatDoc> {
        Arc::from(format().parse(text.to_string(), &FormatOptions::default()))
    }

    fn rev(relevant: u64, replaced: u64, last_at: usize) -> ReviewEdits {
        ReviewEdits {
            relevant,
            last_at,
            replaced,
        }
    }

    /// Polls frame after frame (16 ms apart) until no work is due right now, waiting (in real
    /// time, egui time standing still) while the scanner thread works; returns the answer and
    /// the time reached. Idle-net waits (> 0.1 s) end the loop.
    fn run(
        p: &mut CommentPresence,
        d: &Arc<dyn FormatDoc>,
        r: ReviewEdits,
        text_rev: u64,
        mut now: f64,
    ) -> (bool, f64) {
        let ctx = egui::Context::default();
        let t = Instant::now();
        loop {
            let (has, more) = p.poll_doc(d, r, text_rev, now, &ctx);
            match more {
                Some(w) if w <= RECHECK + 1e-9 => now += w.max(0.016),
                None if p.scan.is_some() => std::thread::sleep(Duration::from_millis(1)),
                _ => return (has, now),
            }
            assert!(t.elapsed() < Duration::from_secs(60), "no answer");
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
            let d = doc(src);
            let mut p = CommentPresence::default();
            assert_eq!(run(&mut p, &d, rev(0, 0, 0), 0, 0.0).0, want, "{src:?}");
        }
    }

    /// Issue #138: the first step of a scan runs at once here; the rest of a large document is
    /// scanned by the scanner thread, with no frame requested meanwhile, and the scanner
    /// wakes the UI once when it has the answer.
    #[test]
    fn large_documents_are_scanned_off_the_ui_thread_without_repaints() {
        let para = "Some ordinary words in a paragraph of the document.\n\n";
        let mut src = para.repeat(12 * STEP_BYTES / para.len());
        src.push_str("End {>>last<<}.\n");
        let d = doc(&src);
        let ctx = egui::Context::default();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = wakes.clone();
        ctx.set_request_repaint_callback(move |_| {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let mut p = CommentPresence::default();
        assert_eq!(
            p.poll_doc(&d, rev(0, 0, 0), 0, 0.0, &ctx),
            (false, None),
            "no frame asked for"
        );
        assert!(p.pending());
        assert!(p.scanned as usize <= STEP_BYTES + 8, "one step here: {}", p.scanned);
        let t = Instant::now();
        let mut frames = 0;
        while p.pending() {
            // frames for other reasons (input) while the scanner works: none asks for another
            let (_, more) = p.poll_doc(&d, rev(0, 0, 0), 0, 0.0, &ctx);
            assert_eq!(more, None, "frame {frames} asked for a repaint while scanning");
            frames += 1;
            assert!(t.elapsed() < Duration::from_secs(60), "no answer");
            std::thread::sleep(Duration::from_micros(200));
        }
        assert!(p.poll_doc(&d, rev(0, 0, 0), 0, 0.0, &ctx).0);
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one wake: the answer"
        );
        assert_eq!(p.scans, 1);
        assert!(p.scanned as usize >= src.len() - 32, "{} of {}", p.scanned, src.len());
        assert_eq!(
            p.poll_doc(&d, rev(0, 0, 0), 0, 9.0, &ctx),
            (true, None),
            "idle afterwards"
        );
    }

    /// Codex round 3 of #94: an edit during a scan (here an ASCII insert before its position
    /// in Cyrillic text, which shifts every later byte by one) drops it; the scan starts over
    /// and still finds the comment.
    #[test]
    fn an_edit_during_a_scan_restarts_it_safely_in_cyrillic_text() {
        let para = "Обычный абзац русского текста, достаточно длинный для переноса строки.\n\n";
        let mut src = para.repeat(6 * STEP_BYTES / para.len());
        src.push_str("Конец {>>заметка<<}.\n");
        let d = doc(&src);
        let ctx = egui::Context::default();
        let mut p = CommentPresence::default();
        let (has, _) = p.poll_doc(&d, rev(0, 0, 0), 0, 0.0, &ctx);
        assert!(!has && p.pending(), "mid-scan");
        // an ordinary (not review-relevant) edit: one ASCII byte at the start
        let d2 = doc(&format!("x{src}"));
        for k in 0..4 {
            // a few frames right after the edit, then later: no panic, and the answer comes
            let _ = p.poll_doc(&d2, rev(0, 0, 0), 1, 0.016 * k as f64, &ctx);
        }
        assert!(run(&mut p, &d2, rev(0, 0, 0), 1, 1.0).0);
    }

    #[test]
    fn edits_reevaluate_only_when_relevant_and_debounced() {
        let ctx = egui::Context::default();
        let d = doc("a {>>note<<} b\n");
        let mut p = CommentPresence::default();
        assert!(run(&mut p, &d, rev(0, 0, 0), 0, 0.0).0);
        assert_eq!(p.scans, 1);
        // no edits: no work at all, ever
        for t in 1..50 {
            assert_eq!(p.poll_doc(&d, rev(0, 0, 0), 0, t as f64, &ctx), (true, None));
        }
        assert_eq!(p.scans, 1);
        // a relevant edit inside the comment's leaf: answered by the local check, no scan
        assert!(p.poll_doc(&d, rev(1, 0, 5), 1, 100.0, &ctx).0);
        assert_eq!(p.scans, 1);
        // the comment deleted: the local check misses, a scan confirms none is left
        let d2 = doc("a b\n");
        let (_, wait) = p.poll_doc(&d2, rev(2, 0, 2), 2, 100.1, &ctx);
        assert!(wait.is_some(), "debounced");
        assert!(!run(&mut p, &d2, rev(2, 0, 2), 2, 100.6).0);
        assert_eq!(p.scans, 2);
        // a replaced document: scanned again
        assert!(run(&mut p, &d, rev(2, 1, 0), 3, 200.0).0);
        assert_eq!(p.scans, 3);
    }

    /// Codex round 3: unindenting `    Here is {>>note<<}` exposes a comment without any
    /// relevance trigger; the idle safety net finds it a few seconds later.
    #[test]
    fn the_idle_rescan_catches_what_the_triggers_miss() {
        let ctx = egui::Context::default();
        let d = doc("    Here is {>>note<<}\n");
        let mut p = CommentPresence::default();
        assert!(!run(&mut p, &d, rev(0, 0, 0), 0, 0.0).0, "code: no comment");
        assert_eq!(p.scans, 1);
        // unindented (an edit the triggers do not see): nothing changes while it is recent
        let d2 = doc("Here is {>>note<<}\n");
        let (has, wait) = p.poll_doc(&d2, rev(0, 0, 0), 1, 10.0, &ctx);
        assert!(!has);
        assert!((wait.unwrap() - IDLE_RESCAN).abs() < 1e-9, "{wait:?}");
        assert!(!p.poll_doc(&d2, rev(0, 0, 0), 1, 11.0, &ctx).0);
        // idle long enough: rescanned, found
        assert!(p.poll_doc(&d2, rev(0, 0, 0), 1, 12.1, &ctx).0);
        assert_eq!(p.scans, 2);
        // and not again while nothing changes
        assert_eq!(p.poll_doc(&d2, rev(0, 0, 0), 1, 30.0, &ctx), (true, None));
        assert_eq!(p.scans, 2);
    }

    /// Issue #138: with a real session, after random edits (made while the scanner works on a
    /// large document), the settled answer equals a full scan, and no edit copied the
    /// document (the session took the scanner's loan back first).
    #[test]
    fn answers_equal_a_full_scan_after_random_session_edits() {
        let para = "An ordinary paragraph with `code` and *emphasis* in it.\n\n";
        let mut src = para.repeat(3 * STEP_BYTES / para.len());
        src.push_str("Tail {>>the only comment<<} here.\n");
        let mut s = EditorSession::new(Arc::new(format()), src);
        let ctx = egui::Context::default();
        let mut p = CommentPresence::default();
        let mut now = 0.0;
        let settle = |p: &mut CommentPresence, s: &mut EditorSession, now: &mut f64| {
            let t = Instant::now();
            loop {
                let (has, more) = p.poll(s, *now, &ctx);
                if p.settled() {
                    return has;
                }
                match more {
                    Some(w) => *now += w.max(0.016),
                    None => std::thread::sleep(Duration::from_millis(1)),
                }
                assert!(t.elapsed() < Duration::from_secs(60), "never settled");
            }
        };
        assert!(settle(&mut p, &mut s, &mut now));
        let inserts = [
            "{>>x<<}", "{>>", "<<}", "`", "\n```\n", "\n\n", "    ", " plain ", "\n~~~\n",
        ];
        let mut x = 41u64;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) as usize
        };
        let (mut yes, mut no) = (0, 0);
        for round in 0..40 {
            for _ in 0..1 + next() % 3 {
                let text = s.doc().text();
                let at = match next() % 3 {
                    0 => text.find("{>>").unwrap_or(0),
                    1 => text.rfind("<<}").unwrap_or(text.len()),
                    _ => next() % (text.len() + 1),
                };
                let at = floor(text, at);
                let end = floor(text, (at + next() % 12).min(text.len()));
                let ins = inserts[next() % inserts.len()];
                let range = if next() % 3 == 0 { at..end } else { at..at };
                // now and then a fresh comment at the start, before anything that could hide it
                let (range, ins) = if next() % 7 == 0 {
                    (0..0, "Fresh {>>note<<} text.\n\n")
                } else {
                    (range, ins)
                };
                let ps = markise_doc::PatchSet::new(vec![markise_doc::Patch::verbatim(range, ins)]).unwrap();
                s.apply_external(&ps, "edit", false).unwrap();
                // a frame or two between edits: scans start and are dropped
                now += 0.5;
                let _ = p.poll(&mut s, now, &ctx);
            }
            if round % 2 == 0 {
                let has = settle(&mut p, &mut s, &mut now);
                assert_eq!(has, has_comment(s.doc()), "round {round}");
                if has { yes += 1 } else { no += 1 }
            }
        }
        assert!(yes > 0 && no > 0, "both answers were exercised: {yes} yes, {no} no");
        assert_eq!(s.copies, 0, "no edit copied the document");
    }
}
