//! "Does the document contain a review comment?" for the comment gutter (ADR 0012, issue #91),
//! tracked incrementally and without copying the text.
//!
//! - When the document object is new (open, reload, re-parse) the text is scanned in place, in
//!   chunks of at most [`CHUNK_BYTES`] or [`CHUNK_TIME`] per frame, from the start until the
//!   first comment is found.
//! - After an edit nothing is looked at unless the edit was review-relevant (the session's
//!   [`ReviewEdits`]: the edit's removed or inserted text, with a few bytes around it, holds a
//!   comment delimiter or block syntax that could turn a comment into code). Then the leaf of
//!   the edit is checked first (inserting a comment flips the answer to "yes" without a scan);
//!   only otherwise is the chunked scan run again (e.g. after deleting a comment: was it the
//!   last one?).
//! - Evaluations start at most every [`RECHECK`] seconds, so the layout does not jump per
//!   keystroke; between them the last answer stands.
//! - Any edit drops a running scan (its byte position means nothing in the new text); it
//!   starts over from the beginning.
//! - Safety net for what the triggers miss (e.g. unindenting a code line that holds a
//!   comment): once the document has been idle for [`IDLE_RESCAN`] seconds and was edited
//!   since its last full scan, the chunked scan runs again. Staleness is thus bounded to a few
//!   seconds in every case.

use std::time::{Duration, Instant};

use markise_doc::review::marks_near;
use markise_doc::{CriticKind, FormatDoc};
use markise_engine::editor::ReviewEdits;

/// Most text bytes scanned in one frame.
pub const CHUNK_BYTES: usize = 256 * 1024;
/// Most time spent scanning in one frame.
pub const CHUNK_TIME: Duration = Duration::from_millis(2);
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

/// A chunked scan in progress, valid only for the text revision it started on.
#[derive(Debug, Clone, Copy)]
struct Scan {
    key: Key,
    rev: u64,
    /// Next byte to look at.
    pos: usize,
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
    /// Full (chunked) scans started (test seam).
    pub scans: u64,
    /// Text bytes scanned by them (test seam).
    pub scanned: u64,
}

/// What one scan step found.
enum Step {
    Found,
    None,
    More(usize),
}

impl CommentPresence {
    /// The answer for the document `doc` at text revision `rev`, whose session reports
    /// `review`, at time `now`; and, when more work is due, the seconds until the next frame
    /// should run.
    pub fn poll(&mut self, doc: &dyn FormatDoc, review: ReviewEdits, rev: u64, now: f64) -> (bool, Option<f64>) {
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
                return self.step(doc, s);
            }
            // any edit, relevant or not, moves text under the scan's position: drop it; it
            // starts over below (debounced), or, if it was the idle safety net's, after the
            // next idle period
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
                if marks_near(doc, at).iter().any(|m| m.kind == CriticKind::Comment) {
                    self.has = true;
                    self.seen = Some(key);
                    return (true, None);
                }
            }
            self.scans += 1;
            return self.step(doc, Scan { key, rev, pos: 0 });
        }
        // up to date as far as the triggers know; the safety net for what they miss
        if self.scanned_rev != Some(rev) {
            let idle = IDLE_RESCAN - (now - self.last_edit);
            if idle > 0.0 {
                return (self.has, Some(idle));
            }
            self.scans += 1;
            return self.step(doc, Scan { key, rev, pos: 0 });
        }
        (self.has, None)
    }

    /// An evaluation is still due or running (tests; the idle safety net does not count).
    pub fn pending(&self) -> bool {
        self.scan.is_some() || self.wanted != self.seen
    }

    fn step(&mut self, doc: &dyn FormatDoc, s: Scan) -> (bool, Option<f64>) {
        match scan_chunk(doc, s.pos, &mut self.scanned) {
            Step::Found => self.finish(s, true),
            Step::None => self.finish(s, false),
            Step::More(pos) => {
                self.scan = Some(Scan { pos, ..s });
                (self.has, Some(0.0))
            }
        }
    }

    fn finish(&mut self, s: Scan, has: bool) -> (bool, Option<f64>) {
        self.has = has;
        self.seen = Some(s.key);
        self.scanned_rev = Some(s.rev);
        self.scan = None;
        (has, None)
    }
}

/// The char boundary at or before `i` (clamped to the text).
fn floor(text: &str, i: usize) -> usize {
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

/// Scans `doc`'s text in place from `pos` for a comment, within one frame's budget.
fn scan_chunk(doc: &dyn FormatDoc, pos: usize, scanned: &mut u64) -> Step {
    let text = doc.text();
    let started = Instant::now();
    let pos = floor(text, pos);
    let end = ceil(text, pos + CHUNK_BYTES);
    // openers starting before `end` (the window reaches two bytes past it)
    let win_end = ceil(text, end + 2);
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
        if started.elapsed() >= CHUNK_TIME && from < end {
            *scanned += (from - pos) as u64;
            return Step::More(from);
        }
    }
    let done = end.max(from);
    *scanned += (done.min(text.len()) - pos) as u64;
    if done >= text.len() {
        Step::None
    } else {
        Step::More(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_doc::{DocumentFormat, FormatOptions};

    fn doc(text: &str) -> Box<dyn FormatDoc> {
        let opts = markise_md::ParseOptions {
            critic_markup: true,
            ..Default::default()
        };
        markise_md::MarkdownFormat { opts }.parse(text.to_string(), &FormatOptions::default())
    }

    fn rev(relevant: u64, replaced: u64, last_at: usize) -> ReviewEdits {
        ReviewEdits {
            relevant,
            last_at,
            replaced,
        }
    }

    /// Polls frame after frame (16 ms apart) until no work is due right now; returns the
    /// answer and the time reached. Idle-net waits (> 0.1 s) end the loop.
    fn run(p: &mut CommentPresence, d: &dyn FormatDoc, r: ReviewEdits, text_rev: u64, mut now: f64) -> (bool, f64) {
        for _ in 0..100_000 {
            let (has, more) = p.poll(d, r, text_rev, now);
            match more {
                Some(w) if w <= RECHECK + 1e-9 => now += w.max(0.016),
                _ => return (has, now),
            }
        }
        panic!("no answer")
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
            assert_eq!(run(&mut p, d.as_ref(), rev(0, 0, 0), 0, 0.0).0, want, "{src:?}");
        }
    }

    #[test]
    fn large_documents_are_scanned_in_chunks() {
        let para = "Some ordinary words in a paragraph of the document.\n\n";
        let mut src = para.repeat(3 * CHUNK_BYTES / para.len());
        src.push_str("End {>>last<<}.\n");
        let d = doc(&src);
        let mut p = CommentPresence::default();
        let (has, more) = p.poll(d.as_ref(), rev(0, 0, 0), 0, 0.0);
        assert!(!has && more.is_some(), "not all in one frame");
        assert!(p.scanned as usize <= CHUNK_BYTES + 8);
        assert!(run(&mut p, d.as_ref(), rev(0, 0, 0), 0, 0.0).0);
        assert_eq!(p.scans, 1);
    }

    /// Codex round 3: an edit during a scan (here an ASCII insert before its position in
    /// Cyrillic text, which shifts every later byte by one) must not slice inside a character;
    /// the scan starts over and still finds the comment.
    #[test]
    fn an_edit_during_a_scan_restarts_it_safely_in_cyrillic_text() {
        let para = "Обычный абзац русского текста, достаточно длинный для переноса строки.\n\n";
        let mut src = para.repeat(3 * CHUNK_BYTES / para.len());
        src.push_str("Конец {>>заметка<<}.\n");
        let d = doc(&src);
        let mut p = CommentPresence::default();
        let (has, more) = p.poll(d.as_ref(), rev(0, 0, 0), 0, 0.0);
        assert!(!has && more.is_some(), "mid-scan");
        // an ordinary (not review-relevant) edit: one ASCII byte at the start
        let edited = format!("x{src}");
        let d2 = doc(&edited);
        for k in 0..4 {
            // a few frames right after the edit, then later: no panic, and the answer comes
            let _ = p.poll(d2.as_ref(), rev(0, 0, 0), 1, 0.016 * k as f64);
        }
        assert!(run(&mut p, d2.as_ref(), rev(0, 0, 0), 1, 1.0).0);
        // the scanner itself clamps any position to a character boundary
        let mut n = 0;
        for pos in 0..64 {
            let _ = scan_chunk(d2.as_ref(), pos, &mut n);
        }
    }

    #[test]
    fn edits_reevaluate_only_when_relevant_and_debounced() {
        let d = doc("a {>>note<<} b\n");
        let mut p = CommentPresence::default();
        assert!(run(&mut p, d.as_ref(), rev(0, 0, 0), 0, 0.0).0);
        assert_eq!(p.scans, 1);
        // no edits: no work at all, ever
        for t in 1..50 {
            assert_eq!(p.poll(d.as_ref(), rev(0, 0, 0), 0, t as f64), (true, None));
        }
        assert_eq!(p.scans, 1);
        // a relevant edit inside the comment's leaf: answered by the local check, no scan
        assert!(p.poll(d.as_ref(), rev(1, 0, 5), 1, 100.0).0);
        assert_eq!(p.scans, 1);
        // the comment deleted: the local check misses, a scan confirms none is left
        let d2 = doc("a b\n");
        let (_, wait) = p.poll(d2.as_ref(), rev(2, 0, 2), 2, 100.1);
        assert!(wait.is_some(), "debounced");
        assert!(!run(&mut p, d2.as_ref(), rev(2, 0, 2), 2, 100.6).0);
        assert_eq!(p.scans, 2);
        // a replaced document: scanned again
        assert!(run(&mut p, d.as_ref(), rev(2, 1, 0), 3, 200.0).0);
        assert_eq!(p.scans, 3);
    }

    /// Codex round 3: unindenting `    Here is {>>note<<}` exposes a comment without any
    /// relevance trigger; the idle safety net finds it a few seconds later.
    #[test]
    fn the_idle_rescan_catches_what_the_triggers_miss() {
        let d = doc("    Here is {>>note<<}\n");
        let mut p = CommentPresence::default();
        assert!(!run(&mut p, d.as_ref(), rev(0, 0, 0), 0, 0.0).0, "code: no comment");
        assert_eq!(p.scans, 1);
        // unindented (an edit the triggers do not see): nothing changes while it is recent
        let d2 = doc("Here is {>>note<<}\n");
        let (has, wait) = p.poll(d2.as_ref(), rev(0, 0, 0), 1, 10.0);
        assert!(!has);
        assert!((wait.unwrap() - IDLE_RESCAN).abs() < 1e-9, "{wait:?}");
        assert!(!p.poll(d2.as_ref(), rev(0, 0, 0), 1, 11.0).0);
        // idle long enough: rescanned, found
        assert!(p.poll(d2.as_ref(), rev(0, 0, 0), 1, 12.1).0);
        assert_eq!(p.scans, 2);
        // and not again while nothing changes
        assert_eq!(p.poll(d2.as_ref(), rev(0, 0, 0), 1, 30.0), (true, None));
        assert_eq!(p.scans, 2);
    }
}
