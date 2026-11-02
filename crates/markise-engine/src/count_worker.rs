//! The status bar's word count off the UI thread (issue #132).
//!
//! A [`CountWorker`] owns a [`DocCounter`] on its own thread. The owner (the status bar) sends
//! it each document change as a revision ([`CountWorker::update`]: the edits' summaries, or
//! "start over") and lends it the document of that revision through a [`DocLease`]; the
//! session takes the loan back before every edit, so the worker never makes an edit copy the
//! document, and it reads the document only in steps of about [`FRAME_BYTES`], for the
//! document's count and for selections alike (an edit waits at most one step). Only changed
//! top-level blocks are counted again (reference-definition changes recount the blocks that
//! may use one, as [`DocCounter::apply`] does).
//!
//! Results are published by revision ([`CountWorker::report`]); the worker wakes the owner
//! once when a revision's document count is complete and once per selection result
//! ([`CountWorker::select`]), never while it is counting.

use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use markise_doc::EditSummary;

use crate::Wake;
use crate::count::{CounterStats, Counts, DocCounter, FRAME_BYTES};
use crate::lease::DocLease;

/// How the counter gets from its revision to the next one.
#[derive(Clone, Debug)]
pub enum Change {
    /// Count the document from scratch (it was opened or replaced).
    Reset,
    /// These edits, in the order they were made (each summary from the state after the one
    /// before it).
    Edits(Vec<EditSummary>),
}

/// What the worker has published.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The revision the counts below describe (0: nothing yet).
    pub rev: u64,
    /// The document's counts, once every block of revision `rev` is counted.
    pub total: Option<Counts>,
    /// Top-level blocks counted and in all (progress while `total` is `None`).
    pub counted: usize,
    pub blocks: usize,
    /// The last selection counted: its revision, range and counts.
    pub selection: Option<(u64, Range<usize>, Counts)>,
    pub stats: CounterStats,
}

enum Msg {
    Change(u64, Change),
    Select(u64, Range<usize>),
}

/// The counting thread and its channel (see the module docs). Dropping it takes the document
/// back and stops the thread.
pub struct CountWorker {
    tx: Option<Sender<Msg>>,
    lease: Arc<DocLease>,
    out: Arc<Mutex<Report>>,
    thread: Option<JoinHandle<()>>,
}

impl CountWorker {
    /// Starts the thread; `wake` is called (from the worker) when a result is published.
    pub fn spawn(wake: Wake) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        let lease = Arc::new(DocLease::new());
        let out = Arc::new(Mutex::new(Report::default()));
        let (l, o) = (lease.clone(), out.clone());
        let thread = std::thread::Builder::new()
            .name("markise-count".into())
            .spawn(move || run(rx, &l, &o, &wake))?;
        Ok(CountWorker {
            tx: Some(tx),
            lease,
            out,
            thread: Some(thread),
        })
    }

    /// The lease the session revokes before each edit ([`crate::editor::EditorSession::set_doc_lease`]).
    pub fn lease(&self) -> &Arc<DocLease> {
        &self.lease
    }

    /// The document is now revision `rev` (revisions grow): `change` brings the counter there,
    /// and `doc` is that revision's document, lent until the session's next edit.
    pub fn update(&self, rev: u64, change: Change, doc: Arc<dyn markise_doc::FormatDoc>) {
        self.lease.lend(rev, doc);
        self.send(Msg::Change(rev, change));
    }

    /// Asks for the counts of the source range `sel` of revision `rev` (published once the
    /// document's count is complete, if `rev` is still current).
    pub fn select(&self, rev: u64, sel: Range<usize>) {
        self.send(Msg::Select(rev, sel));
    }

    fn send(&self, m: Msg) {
        if let Some(tx) = &self.tx {
            // a worker that died (panicked) leaves the status bar showing "Counting…"
            let _ = tx.send(m);
        }
    }

    /// What the worker has published so far.
    pub fn report(&self) -> Report {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for CountWorker {
    fn drop(&mut self) {
        self.lease.revoke();
        self.tx = None; // the worker's `recv` sees the disconnect
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What one step under the lease did.
enum Step {
    Counted,
    Selected(Range<usize>, Option<Counts>),
    Idle,
}

fn run(rx: Receiver<Msg>, lease: &DocLease, out: &Mutex<Report>, wake: &Wake) {
    let mut counter = DocCounter::new();
    let mut rev = 0u64;
    let mut reset = true;
    let mut sel: Option<(u64, Range<usize>)> = None;
    // something may be done without a new message
    let mut runnable = false;
    // the revision whose complete count was published
    let mut published = 0u64;
    let publish = |f: &mut dyn FnMut(&mut Report)| {
        let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut o);
    };
    loop {
        let msg = if runnable {
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
        if let Some(m) = msg {
            match m {
                Msg::Change(r, c) => {
                    rev = r;
                    match c {
                        Change::Reset => reset = true,
                        Change::Edits(es) => {
                            if !reset {
                                reset = !es.iter().all(|e| counter.apply(e));
                            }
                        }
                    }
                }
                Msg::Select(r, range) => sel = Some((r, range)),
            }
            runnable = true;
            continue; // take every pending message first
        }
        let step = lease.with(rev, |doc| {
            if reset || counter.len() != doc.top_count() {
                counter.reset(doc);
                reset = false;
            }
            if counter.missing() > 0 {
                counter.fill(doc, FRAME_BYTES);
                return Step::Counted;
            }
            match sel.take() {
                // in steps too (review of #137): a selection of most of a huge list must not
                // hold the document for longer than one step
                Some((r, range)) if r == rev => match counter.selection_step(doc, range.clone(), FRAME_BYTES) {
                    Some(c) => Step::Selected(range, c),
                    None => {
                        sel = Some((r, range));
                        Step::Counted
                    }
                },
                _ => Step::Idle,
            }
        });
        let Some(step) = step else {
            // nothing lent for this revision (an edit took it back): wait for the next change
            runnable = false;
            continue;
        };
        let total = counter.total();
        let (counted, blocks, stats) = (counter.len() - counter.missing(), counter.len(), counter.stats());
        let done = total.is_some() && published != rev;
        let selected = match step {
            Step::Selected(range, Some(c)) => Some((rev, range, c)),
            Step::Idle => {
                runnable = false;
                None
            }
            _ => None,
        };
        let announce = done || selected.is_some();
        // a result the owner waits for: counted until its wake-up has returned (tests)
        let guard = announce.then(crate::wake_track::publishing);
        publish(&mut |o| {
            o.rev = rev;
            o.total = total;
            o.counted = counted;
            o.blocks = blocks;
            o.stats = stats;
            if let Some(s) = selected.clone() {
                o.selection = Some(s);
            }
        });
        if done {
            published = rev;
        }
        if announce {
            wake();
        }
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::count::count_document;
    use markise_doc::{FormatDoc, Patch, PatchSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn worker() -> (CountWorker, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let cw = CountWorker::spawn(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        (cw, wakes)
    }

    fn wait(cw: &CountWorker, what: &str, f: impl Fn(&Report) -> bool) -> Report {
        let t = Instant::now();
        loop {
            let r = cw.report();
            if f(&r) {
                return r;
            }
            assert!(
                t.elapsed() < Duration::from_secs(60),
                "timed out waiting for {what}: {r:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn final_count(cw: &CountWorker, rev: u64) -> Counts {
        wait(cw, "the count", |r| r.rev == rev && r.total.is_some())
            .total
            .unwrap()
    }

    /// Issue #132: the worker's results equal counting the whole document, after random edits
    /// applied while it counts (the owner takes the document back before each edit, as the
    /// session does), in batches, with reference definitions changing.
    #[test]
    fn results_equal_count_document_after_random_edits() {
        let mut src = String::from("[r]: /u\n\n");
        src.push_str(&markise_corpus::generate(300 * 1024, 11));
        let mut doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(&src));
        let (cw, wakes) = worker();
        let mut rev = 1;
        cw.update(rev, Change::Reset, doc.clone());
        assert_eq!(final_count(&cw, rev), count_document(&*doc));
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "one wake per completed count");
        let inserts = [
            "x",
            " two words ",
            "\n\n# New heading\n\n",
            "\n\n[r]: /v\n\n",
            "[r] ",
            "\n- item\n",
            "`",
            "\n```\n",
        ];
        let mut x = 17u64;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) as usize
        };
        for round in 0..60 {
            let mut edits = Vec::new();
            for _ in 0..1 + next() % 3 {
                cw.lease().revoke();
                let d = Arc::get_mut(&mut doc).expect("the worker holds no reference after a revoke");
                let text = d.text();
                let mut at = next() % (text.len() + 1);
                while !text.is_char_boundary(at) {
                    at -= 1;
                }
                let mut end = (at + next() % 40).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let ins = inserts[next() % inserts.len()];
                let range = if next() % 3 == 0 { at..end } else { at..at };
                edits.push(d.apply(&PatchSet::new(vec![Patch::verbatim(range, ins)]).unwrap()));
            }
            rev += 1;
            let change = if round % 17 == 16 {
                Change::Reset
            } else {
                Change::Edits(edits)
            };
            cw.update(rev, change, doc.clone());
            if round % 4 == 0 {
                // sometimes the count completes before the next edit, sometimes not
                assert_eq!(final_count(&cw, rev), count_document(&*doc), "round {round}");
            }
        }
        assert_eq!(final_count(&cw, rev), count_document(&*doc));
    }

    /// Selections are counted by the worker for the current revision only.
    #[test]
    fn selections_are_counted_for_the_current_revision() {
        let src = "Alpha beta gamma.\n\nDelta epsilon.\n\nZeta eta theta iota.\n";
        let doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(src));
        let (cw, wakes) = worker();
        cw.update(1, Change::Reset, doc.clone());
        cw.select(0, 0..5); // a stale revision: ignored
        cw.select(1, 0..src.len());
        let r = wait(&cw, "the selection", |r| r.selection.is_some());
        assert_eq!(r.selection, Some((1, 0..src.len(), count_document(&*doc))));
        assert_eq!(wakes.load(Ordering::SeqCst), 2, "the count, then the selection");
    }

    /// Review of #137: a selection of most of a huge list (one top-level block, partly
    /// selected) is counted in steps; taking the document back for an edit waits for one
    /// step, not for the whole selection count.
    #[test]
    fn edits_during_a_large_in_container_selection_wait_one_step() {
        let mut src = String::new();
        let mut k = 0;
        while src.len() < 2 << 20 {
            src.push_str(&format!("- item {k} with a few more words in it\n"));
            k += 1;
        }
        let doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(&src));
        assert_eq!(doc.top_count(), 1, "one list");
        let range = 3..src.len() - 3;
        // the whole selection read at once (the old behaviour)
        let mut c = DocCounter::new();
        c.reset(&*doc);
        assert!(c.fill(&*doc, usize::MAX));
        let t = Instant::now();
        let expected = c.selection(&*doc, range.clone()).unwrap();
        let whole = t.elapsed();

        let (cw, _) = worker();
        let mut rev = 1;
        cw.update(rev, Change::Reset, doc.clone());
        final_count(&cw, rev);
        let mut longest = Duration::ZERO;
        let mut x = 5u64;
        for _ in 0..20 {
            cw.select(rev, range.clone());
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            std::thread::sleep(Duration::from_micros(500 + (x >> 33) % 3000));
            let t = Instant::now();
            cw.lease().revoke();
            longest = longest.max(t.elapsed());
            // the document is unchanged: the next revision resumes the selection
            rev += 1;
            cw.update(rev, Change::Edits(Vec::new()), doc.clone());
        }
        cw.select(rev, range.clone());
        let r = wait(&cw, "the selection", |r| {
            r.selection.as_ref().is_some_and(|s| s.0 == rev)
        });
        assert_eq!(r.selection.unwrap().2, expected);
        assert!(
            r.stats.max_call_bytes <= (FRAME_BYTES + crate::count::BIG_BLOCK) as u64,
            "every step under the lease stays within the byte budget: {:?}",
            r.stats
        );
        assert!(
            longest * 4 < whole,
            "an edit waited {longest:?}; counting the whole selection takes {whole:?}"
        );
    }

    /// Dropping the worker takes the document back and stops the thread.
    #[test]
    fn drop_releases_the_document() {
        let doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse("a b\n"));
        let (cw, _) = worker();
        cw.update(1, Change::Reset, doc.clone());
        final_count(&cw, 1);
        assert_eq!(Arc::strong_count(&doc), 2, "lent while the document is unchanged");
        drop(cw);
        assert_eq!(Arc::strong_count(&doc), 1);
    }
}
