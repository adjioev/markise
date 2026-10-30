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

/// Which document state an answer is for: (relevant edits, replacements) of the session.
type Key = (u64, u64);

fn key_of(r: &ReviewEdits) -> Key {
    (r.relevant, r.replaced)
}

/// A chunked scan in progress.
#[derive(Debug, Clone, Copy)]
struct Scan {
    key: Key,
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
    /// The answer for the document `doc` whose session reports `review` at time `now`, and,
    /// when more work is due, the seconds until the next frame should run.
    pub fn poll(&mut self, doc: &dyn FormatDoc, review: ReviewEdits, now: f64) -> (bool, Option<f64>) {
        let key = key_of(&review);
        self.wanted = Some(key);
        if let Some(s) = self.scan {
            if s.key == key {
                return self.step(doc, s);
            }
            // the text changed under the scan: its position means nothing now
            self.scan = None;
        }
        if self.seen == Some(key) {
            return (self.has, None);
        }
        let wait = RECHECK - (now - self.at);
        if self.seen.is_some() && wait > 0.0 {
            return (self.has, Some(wait));
        }
        self.at = now;
        let replaced = self.seen.is_none_or(|(_, r)| r != review.replaced);
        if !replaced {
            // an edit that could add or remove a comment: its own leaf first
            let at = review.last_at.min(doc.text().len());
            let at = (0..=at).rev().find(|&i| doc.text().is_char_boundary(i)).unwrap_or(0);
            if marks_near(doc, at).iter().any(|m| m.kind == CriticKind::Comment) {
                self.has = true;
                self.seen = Some(key);
                return (true, None);
            }
        }
        self.scans += 1;
        self.step(doc, Scan { key, pos: 0 })
    }

    /// An evaluation is still due or running (tests).
    pub fn pending(&self) -> bool {
        self.scan.is_some() || self.wanted != self.seen
    }

    fn step(&mut self, doc: &dyn FormatDoc, s: Scan) -> (bool, Option<f64>) {
        match scan_chunk(doc, s.pos, &mut self.scanned) {
            Step::Found => {
                self.has = true;
                self.seen = Some(s.key);
                self.scan = None;
                (true, None)
            }
            Step::None => {
                self.has = false;
                self.seen = Some(s.key);
                self.scan = None;
                (false, None)
            }
            Step::More(pos) => {
                self.scan = Some(Scan { pos, ..s });
                (self.has, Some(0.0))
            }
        }
    }
}

/// Scans `doc`'s text in place from `pos` for a comment, within one frame's budget.
fn scan_chunk(doc: &dyn FormatDoc, pos: usize, scanned: &mut u64) -> Step {
    let text = doc.text();
    let started = Instant::now();
    let mut end = (pos + CHUNK_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    // openers starting before `end` (the window reaches two bytes past it)
    let mut win_end = (end + 2).min(text.len());
    while !text.is_char_boundary(win_end) {
        win_end += 1;
    }
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
        from = next.min(text.len());
        while !text.is_char_boundary(from) {
            from += 1;
        }
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

    /// Polls until the answer is final (frames in between).
    fn settle(p: &mut CommentPresence, d: &dyn FormatDoc, r: ReviewEdits, mut now: f64) -> bool {
        for _ in 0..10_000 {
            let (has, more) = p.poll(d, r, now);
            match more {
                None => return has,
                Some(w) => now += w.max(0.016),
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
            ("a {++insert++} b {==hl==}\n", false),
            ("x\n\n```\ncode {>>no<<}\n```\n\ny {>>yes<<}\n", true),
        ] {
            let d = doc(src);
            let mut p = CommentPresence::default();
            assert_eq!(settle(&mut p, d.as_ref(), rev(0, 0, 0), 0.0), want, "{src:?}");
        }
    }

    #[test]
    fn large_documents_are_scanned_in_chunks() {
        let para = "Some ordinary words in a paragraph of the document.\n\n";
        let mut src = para.repeat(3 * CHUNK_BYTES / para.len());
        src.push_str("End {>>last<<}.\n");
        let d = doc(&src);
        let mut p = CommentPresence::default();
        let (has, more) = p.poll(d.as_ref(), rev(0, 0, 0), 0.0);
        assert!(!has && more.is_some(), "not all in one frame");
        assert!(p.scanned as usize <= CHUNK_BYTES + 8);
        assert!(settle(&mut p, d.as_ref(), rev(0, 0, 0), 0.0));
        assert_eq!(p.scans, 1);
    }

    #[test]
    fn edits_reevaluate_only_when_relevant_and_debounced() {
        let d = doc("a {>>note<<} b\n");
        let mut p = CommentPresence::default();
        assert!(settle(&mut p, d.as_ref(), rev(0, 0, 0), 0.0));
        assert_eq!(p.scans, 1);
        // irrelevant edits keep the counters: no work at all
        for t in 1..50 {
            assert_eq!(p.poll(d.as_ref(), rev(0, 0, 0), t as f64), (true, None));
        }
        assert_eq!(p.scans, 1);
        // a relevant edit inside the comment's leaf: answered by the local check, no scan
        assert_eq!(p.poll(d.as_ref(), rev(1, 0, 5), 100.0), (true, None));
        assert_eq!(p.scans, 1);
        // the comment deleted: the local check misses, a scan confirms none is left
        let d2 = doc("a b\n");
        let (_, wait) = p.poll(d2.as_ref(), rev(2, 0, 2), 100.1);
        assert!(wait.is_some(), "debounced");
        assert!(!settle(&mut p, d2.as_ref(), rev(2, 0, 2), 100.6));
        assert_eq!(p.scans, 2);
        // a replaced document: scanned again
        assert!(settle(&mut p, d.as_ref(), rev(2, 1, 0), 200.0));
        assert_eq!(p.scans, 3);
    }
}
