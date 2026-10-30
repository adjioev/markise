//! The status bar's word count (issue #53): words, characters or characters without spaces
//! (click to cycle, persisted as `statusBar.count`) and the reading time of the document, or
//! "12 of 3,456 words" for a non-empty selection. Counting rules: [`markise_engine::count`].
//!
//! Cost on the UI thread: a frame only compares the document's revision. After an edit the
//! counts are brought up to date once the document has been quiet for [`DEBOUNCE`]: the
//! [`DocCounter`] matches the view model's items by content hash (integer compares, no text)
//! and counts only the changed items. Counting many items (a newly opened large document, a
//! big paste) happens on a worker from a text snapshot, re-parsed there; its result is merged
//! by item key. Small documents are counted at once on the UI thread (bounded by
//! [`SYNC_BYTES`]), so opening one leaves no background work behind.

use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use markise_doc::{DocumentFormat, FormatDoc, FormatOptions};
use markise_engine::Wake;
use markise_engine::count::{CounterStats, Counts, DocCounter, ItemKey, count_all};
use markise_engine::settings::CountMode;
use markise_layout::DocModel;

/// Quiet time after an edit before the counts are updated.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Documents up to this size are counted at once on the UI thread.
pub const SYNC_BYTES: usize = 256 * 1024;
/// At most this many missing items are counted on the UI thread (in [`FILL_BUDGET`] slices);
/// more go to a worker.
const SYNC_ITEMS: usize = 256;
/// UI-thread time per frame for counting missing items.
const FILL_BUDGET: Duration = Duration::from_millis(3);

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

/// The status bar label: `1,234 words · 7 min read`, `12 of 3,456 words`, `Counting…`.
pub fn label(shown: &Shown, mode: CountMode) -> String {
    match shown {
        Shown::Counting => "Counting…".into(),
        Shown::Document(c) => {
            let n = value(c, mode);
            let mut s = format!("{} {}", thousands(n), unit(mode, n));
            if c.words > 0 {
                s.push_str(&format!(" · {} min read", c.reading_minutes()));
            }
            s
        }
        Shown::Selection { selection, document } => {
            let total = value(document, mode);
            format!(
                "{} of {} {}",
                thousands(value(selection, mode)),
                thousands(total),
                unit(mode, total)
            )
        }
    }
}

/// The label's tooltip: every count.
pub fn tooltip(shown: &Shown) -> String {
    let all = |c: &Counts| {
        format!(
            "Words: {}\nCharacters: {}\nCharacters without spaces: {}",
            thousands(c.words),
            thousands(c.chars),
            thousands(c.chars_no_spaces)
        )
    };
    match shown {
        Shown::Counting => "Counting words…".into(),
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

/// A worker counting a whole document snapshot.
struct Job {
    rx: Receiver<Vec<(ItemKey, Counts)>>,
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
    /// Selection counts by selection range and the reconcile they were made after.
    selection: Option<(Range<usize>, u64, Option<Counts>)>,
    job: Option<Job>,
    /// Worker jobs started (tests).
    jobs: u64,
}

impl WordCount {
    /// Counting statistics: UI-thread item counts, worker jobs (tests: typing does not recount
    /// the document on the UI thread).
    pub fn stats(&self) -> (CounterStats, u64) {
        (self.counter.stats(), self.jobs)
    }

    /// Brings the counts up to date as far as this frame allows and returns what to show.
    /// `rev` is the document's text revision, `sel` the selected source range.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        wake: Option<&Wake>,
        format: &Arc<dyn DocumentFormat>,
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
            self.poll_job();
            if self.counter.missing() > 0 {
                let small = doc.text().len() <= SYNC_BYTES;
                if small {
                    self.counter.fill(doc, model, Duration::MAX);
                } else if self.counter.missing() <= SYNC_ITEMS && self.job.is_none() {
                    if !self.counter.fill(doc, model, FILL_BUDGET) {
                        ctx.request_repaint();
                    }
                } else if self.job.is_none() {
                    self.spawn(ctx, wake, format.clone(), doc.snapshot());
                }
            }
            if let Some(t) = self.counter.total() {
                self.last = Some(t);
            }
        }
        let Some(document) = self.last else {
            return Shown::Counting;
        };
        if sel.is_empty() || !self.synced {
            self.selection = None;
            return Shown::Document(document);
        }
        let epoch = self.counter.stats().reconciles;
        let cached = self
            .selection
            .as_ref()
            .filter(|(r, e, c)| *r == sel && *e == epoch && c.is_some())
            .and_then(|(_, _, c)| *c);
        let selection = cached.or_else(|| {
            let c = self.counter.selection(doc, model, sel.clone());
            self.selection = Some((sel.clone(), epoch, c));
            c
        });
        match selection {
            Some(selection) => Shown::Selection { selection, document },
            None => Shown::Counting,
        }
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Ok(done) => {
                self.counter.absorb(&done);
                self.job = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.job = None,
        }
    }

    fn spawn(&mut self, ctx: &egui::Context, wake: Option<&Wake>, format: Arc<dyn DocumentFormat>, text: Arc<str>) {
        let (tx, rx) = channel();
        let wake: Wake = match wake {
            Some(w) => w.clone(),
            None => {
                let c = ctx.clone();
                Arc::new(move || c.request_repaint())
            }
        };
        let r = std::thread::Builder::new()
            .name("markise-word-count".into())
            .spawn(move || {
                let doc = format.parse(text.to_string(), &FormatOptions::default());
                drop(text);
                let model = DocModel::build_owned(&*doc, None);
                let _ = tx.send(count_all(&*doc, &model));
                wake();
            });
        if r.is_ok() {
            self.jobs += 1;
            self.job = Some(Job { rx });
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
    }
}
