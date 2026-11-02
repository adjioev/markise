//! The status bar's word count (issue #53): words, characters or characters without spaces
//! (click to cycle, persisted as `statusBar.count`) and the reading time of the document, or
//! "12 of 3,456 words" for a non-empty selection. Counting rules: [`markise_engine::count`].
//! Counts that include an estimate (a block larger than
//! [`markise_engine::count::BIG_BLOCK`], a very large selection) are shown with `~`.
//!
//! Counting runs off the UI thread (issue #132): a [`CountWorker`] keeps one count per
//! top-level block of the document and counts only changed blocks. The UI thread sends it the
//! session's edit summaries once the document has been quiet for [`DEBOUNCE`] (as revisions)
//! and lends it the document of that revision ([`EditorSession::set_doc_lease`]: the session
//! takes it back before its next edit, so counting never copies the document). The worker
//! wakes the UI once when a count is complete; the UI never repaints just to count. Until the
//! first count of a document (or after it was replaced) the status bar shows "Counting…" with
//! the progress as of the last frame; after an edit the previous count stays until the new
//! one arrives (a few blocks: milliseconds).
//!
//! A selection up to [`SMALL_SELECTION`] bytes is counted at once on the UI thread (reading
//! about that much); a larger one is counted by the worker from its cached block counts.

use std::ops::Range;
use std::time::{Duration, Instant};

use markise_engine::count::{CounterStats, Counts, count_range};
use markise_engine::count_worker::{Change, CountWorker, Report};
use markise_engine::editor::{ChangeLog, EditorSession};
use markise_engine::settings::CountMode;

/// Quiet time after an edit before the counts are updated.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Selections up to this many source bytes are counted on the UI thread, at once.
pub const SMALL_SELECTION: usize = 16 * 1024;

/// What the status bar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Not counted yet: the percentage of the document's blocks counted so far, if known.
    Counting(Option<u8>),
    Document(Counts),
    Selection {
        selection: Counts,
        document: Counts,
    },
    /// A selection while it or the document is still being counted.
    SelectionPending {
        selection: Option<Counts>,
        document: Option<Counts>,
    },
    /// The counting thread could not be started.
    Unavailable,
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
/// `Counting… 40%`, `… of 3,456 words` / `12 of … words` while a part is being counted.
pub fn label(shown: &Shown, mode: CountMode) -> String {
    let counting = |p: &Option<u8>| match p {
        Some(p) => format!("Counting… {p}%"),
        None => "Counting…".into(),
    };
    match shown {
        Shown::Counting(p) => counting(p),
        Shown::Document(c) => {
            let mut s = format!("{} {}", number(c, mode), unit(mode, value(c, mode)));
            if c.words > 0 {
                s.push_str(&format!(" · {} min read", c.reading_minutes()));
            }
            s
        }
        Shown::SelectionPending { selection, document } => match (selection, document) {
            (_, Some(d)) => format!("… of {} {}", number(d, mode), unit(mode, value(d, mode))),
            (Some(s), None) => format!("{} of … {}", number(s, mode), unit(mode, 2)),
            (None, None) => counting(&None),
        },
        Shown::Selection { selection, document } => format!(
            "{} of {} {}",
            number(selection, mode),
            number(document, mode),
            unit(mode, value(document, mode))
        ),
        Shown::Unavailable => "Word count unavailable".into(),
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
        Shown::Counting(_) | Shown::SelectionPending { .. } => "Counting words in the background…".into(),
        Shown::Unavailable => "The word count could not start a background thread".into(),
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

/// Word-count state of the open document (see the module docs).
#[derive(Default)]
pub struct WordCount {
    /// The counting thread (started at the first update; `Err` when it could not start).
    worker: Option<Result<CountWorker, ()>>,
    /// The session whose document is counted ([`EditorSession::id`]).
    session: Option<u64>,
    /// The revision last sent to the worker.
    rev: u64,
    /// Edits not sent to the worker yet (debounce).
    pending: ChangeLog,
    /// The worker has every change (no debounce pending).
    synced: bool,
    /// When the document last changed (debounce).
    changed: Option<Instant>,
    /// Bumped on every document change (keys the small-selection count).
    generation: u64,
    /// Last complete document counts (shown, stale, while an update is pending).
    last: Option<Counts>,
    /// The whole document is being counted again (opened or replaced): `last` does not
    /// describe it.
    recounting: bool,
    /// The last small selection counted on the UI thread: generation, range, counts.
    small: Option<(u64, Range<usize>, Counts)>,
    /// The large selection asked of the worker (revision, range).
    asked: Option<(u64, Range<usize>)>,
    /// A selection result is outstanding (as of the last frame).
    waiting: bool,
    /// The worker's report as of the last frame.
    report: Report,
}

impl WordCount {
    /// Counting statistics of the worker (tests: typing does not recount the document; every
    /// step stays within the byte budget).
    pub fn stats(&self) -> CounterStats {
        self.report.stats
    }

    /// Test hook ("background settled", issue #42): the counts are up to date — no debounce
    /// is pending and the worker has published the current revision's count (and the
    /// selection asked of it), as of the last frame.
    pub fn settled(&self) -> bool {
        let failed = matches!(self.worker, Some(Err(())));
        failed || (self.synced && self.current() && !self.waiting)
    }

    /// The worker's report describes the revision last sent, completely.
    fn current(&self) -> bool {
        self.report.rev == self.rev && self.report.total.is_some()
    }

    /// Brings the worker up to date with the session's document changes
    /// ([`EditorSession::take_log`]) and returns what to show; `sel` is the selected source
    /// range. Never requests a repaint for counting: the worker wakes `ctx` when a result
    /// arrives; only the debounce schedules one frame.
    pub fn update(&mut self, ctx: &egui::Context, session: &mut EditorSession, sel: Range<usize>) -> Shown {
        let now = Instant::now();
        let log = session.take_log();
        if self.worker.is_none() {
            let c = ctx.clone();
            self.worker = Some(CountWorker::spawn(std::sync::Arc::new(move || c.request_repaint())).map_err(|_| ()));
        }
        let Some(Ok(worker)) = self.worker.as_ref() else {
            return Shown::Unavailable;
        };
        if self.session != Some(session.id()) {
            // the document just opened (the owner starts a new `WordCount` for another
            // document): counted from scratch
            self.session = Some(session.id());
            session.set_doc_lease(Some(worker.lease().clone()));
            self.rev += 1;
            worker.update(self.rev, Change::Reset, session.doc_arc().clone());
            self.pending = ChangeLog::default();
            self.synced = true;
            self.changed = None;
            self.recounting = true;
            self.generation += 1;
        } else if !log.is_empty() {
            // a change: sent once the document has been quiet for a moment
            self.pending.append(log);
            self.synced = false;
            self.changed = Some(now);
            self.generation += 1;
        }
        if !self.synced {
            match self.changed.map(|t| now.saturating_duration_since(t)) {
                Some(quiet) if quiet < DEBOUNCE => ctx.request_repaint_after(DEBOUNCE - quiet),
                _ => {
                    let p = std::mem::take(&mut self.pending);
                    let change = if p.reset {
                        self.recounting = true;
                        Change::Reset
                    } else {
                        Change::Edits(p.edits)
                    };
                    self.rev += 1;
                    worker.update(self.rev, change, session.doc_arc().clone());
                    self.synced = true;
                }
            }
        }
        self.report = worker.report();
        if self.current() {
            self.last = self.report.total;
            self.recounting = false;
        }
        let document = if self.recounting { None } else { self.last };
        self.waiting = false;
        if sel.is_empty() {
            return match document {
                Some(d) => Shown::Document(d),
                None => {
                    let r = &self.report;
                    let progress =
                        (r.rev == self.rev && r.blocks > 0).then(|| (r.counted * 100 / r.blocks).min(99) as u8);
                    Shown::Counting(progress)
                }
            };
        }
        let selection = match &self.small {
            Some((g, r, c)) if *g == self.generation && *r == sel => Some(*c),
            _ if sel.len() <= SMALL_SELECTION => {
                let c = count_range(session.doc(), sel.clone());
                self.small = Some((self.generation, sel.clone(), c));
                Some(c)
            }
            _ if self.synced => match &self.report.selection {
                Some((r, range, c)) if *r == self.rev && *range == sel => Some(*c),
                _ => {
                    let ask = (self.rev, sel.clone());
                    if self.asked.as_ref() != Some(&ask) {
                        worker.select(ask.0, ask.1.clone());
                        self.asked = Some(ask);
                    }
                    self.waiting = true;
                    None
                }
            },
            _ => None,
        };
        match (selection, document) {
            (Some(selection), Some(document)) => Shown::Selection { selection, document },
            (selection, document) => Shown::SelectionPending { selection, document },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame that updates `wc`; returns what it shows and the repaint delay asked.
    fn frame(ctx: &egui::Context, wc: &mut WordCount, s: &mut EditorSession, sel: Range<usize>) -> (Shown, Duration) {
        let mut shown = Shown::Unavailable;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            shown = wc.update(ui.ctx(), s, sel.clone())
        });
        out.textures_delta.clear(); // headless
        (shown, out.viewport_output[&egui::ViewportId::ROOT].repaint_delay)
    }

    fn wait_settled(ctx: &egui::Context, wc: &mut WordCount, s: &mut EditorSession, sel: Range<usize>) -> Shown {
        let t = Instant::now();
        loop {
            let (shown, _) = frame(ctx, wc, s, sel.clone());
            if wc.settled() {
                return shown;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "never settled: {shown:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Issue #132: counting a large document runs on the worker: no frame asks for a repaint
    /// to count (the window stays idle), the worker wakes the UI once when the count is
    /// complete, typing never copies the document, and the edit is counted after the
    /// debounce; a large selection is counted by the worker, a small one at once.
    #[test]
    fn counting_requests_no_repaint_and_wakes_once() {
        let text = markise_corpus::generate(2 << 20, 5);
        let mut s = EditorSession::new(std::sync::Arc::new(markise_md::MarkdownFormat::default()), text.clone());
        let ctx = egui::Context::default();
        let wakes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = wakes.clone();
        ctx.set_request_repaint_callback(move |_| {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        // egui's own first passes ask for repaints
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
            out.textures_delta.clear();
        }
        let mut wc = WordCount::default();
        let (shown, delay) = frame(&ctx, &mut wc, &mut s, 0..0);
        assert!(matches!(shown, Shown::Counting(_)), "{shown:?}");
        assert_eq!(delay, Duration::MAX, "no repaint requested to count");
        wakes.store(0, std::sync::atomic::Ordering::SeqCst);
        // frames while the worker counts never ask for a repaint
        let mut frames = 0;
        while !wc.settled() {
            let woken = wakes.load(std::sync::atomic::Ordering::SeqCst) > 0;
            let (_, delay) = frame(&ctx, &mut wc, &mut s, 0..0);
            // (the worker's wake-up, when the count is complete, is the only repaint)
            assert!(
                delay == Duration::MAX || woken,
                "frame {frames} asked for a repaint while counting: {delay:?}"
            );
            frames += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        let shown = frame(&ctx, &mut wc, &mut s, 0..0).0;
        let total = markise_engine::count::count_document(s.doc());
        assert_eq!(shown, Shown::Document(total));
        // the worker's wake-up and the frame egui runs for it
        assert!(
            wakes.load(std::sync::atomic::Ordering::SeqCst) <= 2,
            "one wake when the count arrived: {}",
            wakes.load(std::sync::atomic::Ordering::SeqCst)
        );
        assert_eq!(frame(&ctx, &mut wc, &mut s, 0..0).1, Duration::MAX, "idle once counted");

        // typing: no document copy; counted after the debounce
        let copies = s.copies;
        s.set_selection(markise_doc::Selection::single(
            markise_doc::DocPos::new(text.len()),
            markise_doc::DocPos::new(text.len()),
        ));
        for w in ["\n\nzebra", " quokka", " okapi"] {
            s.insert_text(w).unwrap();
            let (_, delay) = frame(&ctx, &mut wc, &mut s, 0..0);
            assert!(delay <= DEBOUNCE, "the debounce schedules a frame: {delay:?}");
        }
        assert_eq!(s.copies, copies, "the lent document was taken back before each edit");
        std::thread::sleep(DEBOUNCE);
        let shown = wait_settled(&ctx, &mut wc, &mut s, 0..0);
        assert_eq!(shown, Shown::Document(markise_engine::count::count_document(s.doc())));
        assert_eq!(total.words + 3, markise_engine::count::count_document(s.doc()).words);

        // a small selection: counted at once; Select All: by the worker
        let len = s.doc().text().len();
        let (shown, _) = frame(&ctx, &mut wc, &mut s, len - 12..len);
        assert!(
            matches!(shown, Shown::Selection { selection, .. } if selection.words == 2),
            "{shown:?}"
        );
        let shown = wait_settled(&ctx, &mut wc, &mut s, 0..len);
        let doc = markise_engine::count::count_document(s.doc());
        assert_eq!(
            shown,
            Shown::Selection {
                selection: doc,
                document: doc
            }
        );
    }

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
            label(
                &Shown::SelectionPending {
                    selection: None,
                    document: Some(c)
                },
                CountMode::Words
            ),
            "… of 1,234 words"
        );
        assert_eq!(
            label(
                &Shown::SelectionPending {
                    selection: Some(s),
                    document: None
                },
                CountMode::Words
            ),
            "12 of … words"
        );
        assert_eq!(label(&Shown::Counting(None), CountMode::Words), "Counting…");
        assert_eq!(label(&Shown::Counting(Some(40)), CountMode::Words), "Counting… 40%");
    }
}
