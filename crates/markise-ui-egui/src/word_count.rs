//! The status bar's word count (issue #53): words, characters or characters without spaces
//! (click to cycle, persisted as `statusBar.count`) and the reading time of the document, or
//! "12 of 3,456 words" for a non-empty selection. Counting rules: [`markise_engine::count`].
//! Counts that include an estimate (a block larger than
//! [`markise_engine::count::BIG_BLOCK`], a very large selection) are shown with `~`.
//!
//! Everything runs on the UI thread, reading the session's text in place (no copy): a frame
//! without a change only compares the document's revision. After an edit the counts are
//! brought up to date once the document has been quiet for [`DEBOUNCE`]: the [`DocCounter`]
//! matches the view model's items by content hash (integer compares, no text) and counts only
//! the changed blocks. A document up to [`SMALL_DOC`] is counted in its first frame; a larger
//! one about [`FRAME_BYTES`] of source per frame (each block is at most
//! [`markise_engine::count::BIG_BLOCK`]), showing the last value (or "Counting…") until done.
//! A selection reads at most its two partly selected blocks.

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_doc::FormatDoc;
use markise_engine::count::{CounterStats, Counts, DocCounter, FRAME_BYTES};
use markise_engine::settings::CountMode;
use markise_layout::DocModel;

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

/// The document whose counts are cached: identity of its model, text revision, item count.
type Seen = (usize, u64, usize);

/// Word-count state of the open document.
#[derive(Default)]
pub struct WordCount {
    counter: DocCounter,
    seen: Option<Seen>,
    /// The counter is reconciled with the model `seen` describes.
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
    /// show. `rev` is the document's text revision, `sel` the selected source range.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        doc: &dyn FormatDoc,
        model: &Arc<DocModel>,
        rev: u64,
        sel: Range<usize>,
    ) -> Shown {
        let now = Instant::now();
        let key = (Arc::as_ptr(model) as usize, rev, model.items.len());
        if self.seen != Some(key) {
            // the document just opened: at once; a change (an edit, a rebuilt model): after
            // the document has been quiet for a moment (the owner starts a new `WordCount`
            // for another document)
            let fresh = self.seen.is_none();
            self.seen = Some(key);
            self.synced = false;
            self.changed = if fresh { None } else { Some(now) };
        }
        if !self.synced {
            match self.changed.map(|t| now.saturating_duration_since(t)) {
                Some(quiet) if quiet < DEBOUNCE => ctx.request_repaint_after(DEBOUNCE - quiet),
                _ => {
                    self.counter.reconcile(model);
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
                if !self.counter.fill(doc, model, budget) {
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
            .then(|| self.counter.selection(doc, model, sel))
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
