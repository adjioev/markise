//! `count-10mb` (issue #53): word counting of the 10 MB corpus — the worker's whole-document
//! count (parse + model + [`count_all`]) and the UI thread's work after a keystroke
//! ([`DocCounter::reconcile`] against the edited model + counting the changed items).
//!
//! Run: `cargo bench -p markise-engine --bench count`.

use std::time::{Duration, Instant};

use markise_corpus::generate;
use markise_engine::count::{DocCounter, count_all};
use markise_layout::DocModel;
use markise_md::{Document, ParseOptions};

/// CPU time of the calling thread (separates scheduling noise on a loaded machine from work).
fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec; CLOCK_THREAD_CPUTIME_ID is supported on Linux/macOS
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

fn main() {
    for mb in [1usize, 10] {
        let src = generate(mb << 20, 7);
        let t = Instant::now();
        let doc = Document::parse(src.as_str(), ParseOptions::default());
        let model = DocModel::build_owned(&doc, None);
        let parsed = t.elapsed();
        let (t, cpu) = (Instant::now(), thread_cpu());
        let all = count_all(&doc, &model);
        let (counted, counted_cpu) = (t.elapsed(), thread_cpu() - cpu);
        let words: u64 = all.iter().map(|(_, c)| c.words).sum();

        let mut c = DocCounter::new();
        c.reconcile(&model);
        c.absorb(&all);
        // a keystroke in the middle of the document
        let mid = src.len() / 2;
        let at = mid + src[mid..].find(". ").unwrap_or(0) + 1;
        let mut edited = src.clone();
        edited.insert(at, 'x');
        let doc2 = Document::parse(edited.as_str(), ParseOptions::default());
        let model2 = DocModel::build_owned(&doc2, None);
        let t = Instant::now();
        c.reconcile(&model2);
        let reconciled = t.elapsed();
        let t = Instant::now();
        assert!(c.fill(&doc2, &model2, Duration::MAX));
        let filled = t.elapsed();
        println!(
            "count-{mb}mb: {} items, {words} words; worker: parse+model {parsed:?}, count {counted:?} (CPU {counted_cpu:?}); \
             after a keystroke (UI thread): reconcile {reconciled:?}, count {} item(s) {filled:?}",
            model.items.len(),
            c.stats().items_counted,
        );
    }
}
