//! Issue #24: selecting a small file in the file tree shows it at once. The real `MarkiseApp`
//! open path (sidebar click → loader worker → install → first document frame) is driven
//! headlessly with egui_kittest and timed from the click to the frame that paints the file.
//!
//! The file watcher's worker is stalled (as macOS FSEvents stream restarts did for up to
//! ~1.5 s in the measurements) to prove that no watch registration sits on the open path.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::jobs::JobHooks;
use markise_ui_egui::{AppOptions, MarkiseApp};

/// Click → first frame showing the document. The release-build target is ~50 ms (typically
/// 2–10 ms measured); the bound leaves room for unoptimised test builds and busy CI machines
/// while staying far below the stalled watcher.
const BOUND: Duration = Duration::from_millis(250);
const WATCH_STALL: Duration = Duration::from_millis(800);

fn tmpdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-open-latency-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn note(i: usize) -> String {
    let mut s = format!("# Note {i}\n\nSome *text* with `code` and a [link](https://example.com).\n\n");
    for j in 0..40 {
        s.push_str(&format!("- item {j} with a few more words to wrap\n"));
    }
    s.push_str("\n```rust\nfn main() {}\n```\n");
    s
}

#[test]
fn clicking_a_small_file_in_the_tree_shows_it_without_delay() {
    let dir = tmpdir();
    let names: Vec<String> = (1..=4).map(|i| format!("note{i}.md")).collect();
    for (i, n) in names.iter().enumerate() {
        std::fs::write(dir.join(n), note(i)).unwrap(); // ~2 KB each
    }
    let hooks = Arc::new(JobHooks {
        before_watch: Some(Box::new(|| std::thread::sleep(WATCH_STALL))),
        ..Default::default()
    });
    let app = MarkiseApp::new(AppOptions {
        open: vec![dir.clone()],
        hooks,
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    let t = Instant::now();
    while !names.iter().all(|n| h.state().file_tree_rows().contains(n)) {
        assert!(t.elapsed() < Duration::from_secs(10), "listing");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);

    let mut worst = Duration::ZERO;
    for n in names.iter().chain(names.iter().take(1)) {
        h.get_by_label(n.as_str()).click();
        let t0 = Instant::now();
        loop {
            h.step(); // the click frame, then frames until the loaded file is painted
            let a = h.state();
            if a.document_path().is_some_and(|p| p.ends_with(n)) && !a.is_loading() {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "{n} never opened");
            std::thread::sleep(Duration::from_millis(1));
        }
        let took = t0.elapsed();
        eprintln!("open-latency: {n} click→first frame {:.1} ms", took.as_secs_f64() * 1e3);
        worst = worst.max(took);
        h.run_steps(2);
    }
    assert!(worst < BOUND, "click→first frame took {worst:?} (bound {BOUND:?})");
    let _ = std::fs::remove_dir_all(&dir);
}
