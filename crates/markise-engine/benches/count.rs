//! `count-1mb` / `count-10mb` (issue #53): word counting on the UI thread in per-frame budgets
//! ([`FRAME_BYTES`] and 2 ms, as the status bar uses them) — the first count of the corpus
//! (calls, longest call) and the work after a keystroke ([`DocCounter::reconcile`] against
//! the edited model + counting the changed block).
//!
//! Run: `cargo bench -p markise-engine --bench count`.

use std::time::{Duration, Instant};

use markise_corpus::generate;
use markise_engine::count::{Budget, DocCounter, FRAME_BYTES};
use markise_layout::DocModel;
use markise_md::{Document, ParseOptions};

/// CPU time of the calling thread (separates scheduling noise on a loaded machine from work).
fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec; CLOCK_THREAD_CPUTIME_ID is supported on macOS/Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

fn main() {
    for mb in [1usize, 10] {
        let src = generate(mb << 20, 7);
        let doc = Document::parse(src.as_str(), ParseOptions::default());
        let model = DocModel::build_owned(&doc, None);

        let mut c = DocCounter::new();
        c.reconcile(&model);
        let (mut calls, mut longest, mut longest_cpu, mut cpu_total) =
            (0, Duration::ZERO, Duration::ZERO, Duration::ZERO);
        loop {
            let (t, cpu) = (Instant::now(), thread_cpu());
            let budget = Budget {
                bytes: FRAME_BYTES,
                until: Some(t + Duration::from_millis(2)),
            };
            let done = c.fill(&doc, &model, budget);
            let (e, ec) = (t.elapsed(), thread_cpu() - cpu);
            calls += 1;
            longest = longest.max(e);
            longest_cpu = longest_cpu.max(ec);
            cpu_total += ec;
            if done {
                break;
            }
        }
        let words = c.total().unwrap().words;

        // a keystroke in the middle of the document
        let mid = src.len() / 2;
        let at = mid + src[mid..].find(". ").unwrap_or(0) + 1;
        let mut edited = src.clone();
        edited.insert(at, 'x');
        let doc2 = Document::parse(edited.as_str(), ParseOptions::default());
        let model2 = DocModel::build_owned(&doc2, None);
        let before = c.stats();
        let (t, cpu) = (Instant::now(), thread_cpu());
        c.reconcile(&model2);
        let reconciled = (t.elapsed(), thread_cpu() - cpu);
        let (t, cpu) = (Instant::now(), thread_cpu());
        assert!(c.fill(&doc2, &model2, FRAME_BYTES));
        let filled = (t.elapsed(), thread_cpu() - cpu);
        println!(
            "count-{mb}mb: {} items, {words} words; first count: {calls} frames, CPU {cpu_total:?} in all, \
             longest frame {longest:?} (CPU {longest_cpu:?}); after a keystroke: reconcile {:?} (CPU {:?}), \
             {} block(s) {:?} (CPU {:?})",
            model.items.len(),
            reconciled.0,
            reconciled.1,
            c.stats().blocks_counted - before.blocks_counted,
            filled.0,
            filled.1,
        );
    }
}
