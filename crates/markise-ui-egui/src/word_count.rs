//! The status bar's word count (issue #53): words, characters or characters without spaces
//! (click to cycle, persisted as `statusBar.count`) and the reading time of the document, or
//! "12 of 3,456 words" for a non-empty selection. Counting rules: [`markise_engine::count`].
//! Counts that include an estimate (a block larger than
//! [`markise_engine::count::BIG_BLOCK`], a very large selection) are shown with `~`.
//!
//! Everything runs on the UI thread, reading the session's text in place (no copy): a frame
//! without a change only checks the session's change log. After an edit the counts are
//! brought up to date once the document has been quiet for [`DEBOUNCE`]: the [`DocCounter`]
//! keeps one count per top-level block of the document, applies the edits' summaries (no text
//! read) and counts only the changed blocks; it does not depend on the view model (whose items
//! a paged model materializes only near the viewport, issue #125). A document up to [`SMALL_DOC`] is counted in its first frame; a larger
//! one about [`FRAME_BYTES`] of source per frame (each block is at most
//! [`markise_engine::count::BIG_BLOCK`]), showing the last value (or "Counting…") until done.
//! A selection reads at most its two partly selected blocks.

use std::ops::Range;
use std::time::{Duration, Instant};

use markise_engine::count::{CounterStats, Counts, DocCounter, FRAME_BYTES};
use markise_engine::editor::{ChangeLog, EditorSession};
use markise_engine::settings::CountMode;

/// Quiet time after an edit before the counts are updated.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Documents up to this size are counted in one frame (opening one leaves no counting work
/// for later frames).
pub const SMALL_DOC: usize = 256 * 1024;

/// What the status bar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Not counted yet.
    Counting,
    Document(Counts),
    Selection {
        selection: Counts,
        document: Counts,
    },
    /// A selection while the document is still being counted.
    SelectionPending {
        document: Option<Counts>,
    },
}

/// Formats `n` with thousands separators (`3,456`).
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn value(c: &Counts, mode: CountMode) -> u64 {
    match mode {
        CountMode::Words => c.words,
        CountMode::Characters => c.chars,
        CountMode::CharactersNoSpaces => c.chars_no_spaces,
    }
}

/// The number shown for `c`: `~` marks an estimate.
fn number(c: &Counts, mode: CountMode) -> String {
    let n = thousands(value(c, mode));
    if c.is_approx() { format!("~{n}") } else { n }
}

fn unit(mode: CountMode, n: u64) -> &'static str {
    match (mode, n == 1) {
        (CountMode::Words, true) => "word",
        (CountMode::Words, false) => "words",
        (CountMode::Characters, true) => "character",
        (CountMode::Characters, false) => "characters",
        (CountMode::CharactersNoSpaces, true) => "character without spaces",
        (CountMode::CharactersNoSpaces, false) => "characters without spaces",
    }
}

/// The status bar label: `1,234 words · 7 min read`, `12 of 3,456 words`, `~5,000 words`,
/// `Counting…`.
pub fn label(shown: &Shown, mode: CountMode) -> String {
    match shown {
        Shown::Counting => "Counting…".into(),
        Shown::Document(c) => {
            let mut s = format!("{} {}", number(c, mode), unit(mode, value(c, mode)));
            if c.words > 0 {
                s.push_str(&format!(" · {} min read", c.reading_minutes()));
            }
            s
        }
        Shown::SelectionPending { document } => match document {
            Some(d) => format!("… of {} {}", number(d, mode), unit(mode, value(d, mode))),
            None => "Counting…".into(),
        },
        Shown::Selection { selection, document } => format!(
            "{} of {} {}",
            number(selection, mode),
            number(document, mode),
            unit(mode, value(document, mode))
        ),
    }
}

/// The label's tooltip: every count.
pub fn tooltip(shown: &Shown) -> String {
    let all = |c: &Counts| {
        let mut s = format!(
            "Words: {}\nCharacters: {}\nCharacters without spaces: {}",
            number(c, CountMode::Words),
            number(c, CountMode::Characters),
            number(c, CountMode::CharactersNoSpaces)
        );
        if c.is_approx() {
            s.push_str("\n~ estimated: very large blocks and selections are estimated from their size");
        }
        s
    };
    match shown {
        Shown::Counting | Shown::SelectionPending { .. } => "Counting words…".into(),
        Shown::Document(c) => format!(
            "{}\nReading time: {} min (at 200 words per minute)\n\nClick to switch between words and characters",
            all(c),
            c.reading_minutes()
        ),
        Shown::Selection { selection, .. } => format!(
            "Selection\n{}\n\nClick to switch between words and characters",
            all(selection)
        ),
    }
}

/// Word-count state of the open document.
#[derive(Default)]
pub struct WordCount {
    counter: DocCounter,
    /// The session whose document is counted ([`EditorSession::id`]).
    session: Option<u64>,
    /// Edits not applied to the counter yet (debounce).
    pending: ChangeLog,
    /// The counter is in step with the document.
    synced: bool,
    /// When the document last changed (debounce).
    changed: Option<Instant>,
    /// Last complete document counts (shown, stale, while an update is pending).
    last: Option<Counts>,
}

impl WordCount {
    /// Counting statistics (tests: typing does not recount the document; per-frame reads stay
    /// within the byte budget).
    pub fn stats(&self) -> CounterStats {
        self.counter.stats()
    }

    /// Test hook ("background settled", issue #42): the counts are up to date — no debounce
    /// or per-frame counting is pending (as of the last frame).
    pub fn settled(&self) -> bool {
        self.synced && self.counter.missing() == 0
    }

    /// Brings the counts up to date as far as this frame's budget allows and returns what to
    /// show. Takes the session's document changes ([`EditorSession::take_log`]); `sel` is the
    /// selected source range.
    pub fn update(&mut self, ctx: &egui::Context, session: &mut EditorSession, sel: Range<usize>) -> Shown {
        let now = Instant::now();
        let log = session.take_log();
        let doc = session.doc();
        if self.session != Some(session.id()) {
            // the document just opened: counted at once (the owner starts a new `WordCount`
            // for another document)
            self.session = Some(session.id());
            self.counter.reset(doc);
            self.pending = ChangeLog::default();
            self.synced = true;
            self.changed = None;
        } else if !log.is_empty() {
            // a change: applied once the document has been quiet for a moment
            self.pending.append(log);
            self.synced = false;
            self.changed = Some(now);
        }
        if !self.synced {
            match self.changed.map(|t| now.saturating_duration_since(t)) {
                Some(quiet) if quiet < DEBOUNCE => ctx.request_repaint_after(DEBOUNCE - quiet),
                _ => {
                    let p = std::mem::take(&mut self.pending);
                    let applied = !p.reset && p.edits.iter().all(|e| self.counter.apply(e));
                    if !applied || self.counter.len() != doc.top_count() {
                        self.counter.reset(doc);
                    }
                    self.synced = true;
                }
            }
        }
        if self.synced {
            if self.counter.missing() > 0 {
                let budget = if doc.text().len() <= SMALL_DOC {
                    usize::MAX
                } else {
                    FRAME_BYTES
                };
                if !self.counter.fill(doc, budget) {
                    ctx.request_repaint();
                }
            }
            if let Some(t) = self.counter.total() {
                self.last = Some(t);
            }
        }
        if sel.is_empty() {
            return match self.last {
                Some(d) => Shown::Document(d),
                None => Shown::Counting,
            };
        }
        // the selection is counted once the document is
        let selection = (self.synced && self.counter.missing() == 0)
            .then(|| self.counter.selection(doc, sel))
            .flatten();
        match (selection, self.last) {
            (Some(selection), Some(document)) => Shown::Selection { selection, document },
            (_, document) => Shown::SelectionPending { document },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(3456), "3,456");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn labels() {
        let c = Counts {
            words: 1234,
            chars: 6000,
            chars_no_spaces: 5000,
            approx: 0,
        };
        assert_eq!(label(&Shown::Document(c), CountMode::Words), "1,234 words · 7 min read");
        assert_eq!(
            label(&Shown::Document(c), CountMode::Characters),
            "6,000 characters · 7 min read"
        );
        assert_eq!(
            label(&Shown::Document(c), CountMode::CharactersNoSpaces),
            "5,000 characters without spaces · 7 min read"
        );
        assert_eq!(label(&Shown::Document(Counts::default()), CountMode::Words), "0 words");
        let s = Counts {
            words: 12,
            chars: 1,
            chars_no_spaces: 1,
            approx: 0,
        };
        assert_eq!(
            label(
                &Shown::Selection {
                    selection: s,
                    document: c
                },
                CountMode::Words
            ),
            "12 of 1,234 words"
        );
        // estimates are marked
        let e = Counts { approx: 1, ..c };
        assert_eq!(
            label(&Shown::Document(e), CountMode::Words),
            "~1,234 words · 7 min read"
        );
        assert_eq!(
            label(
                &Shown::Selection {
                    selection: Counts { approx: 1, ..s },
                    document: e
                },
                CountMode::Words
            ),
            "~12 of ~1,234 words"
        );
        assert!(tooltip(&Shown::Document(e)).contains("~ estimated"));
        assert_eq!(
            label(&Shown::SelectionPending { document: Some(c) }, CountMode::Words),
            "… of 1,234 words"
        );
    }
}
