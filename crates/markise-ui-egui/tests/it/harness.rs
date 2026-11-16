//! The shared waiting helpers themselves (issue #226).

use crate::common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use egui_kittest::Harness;
use markise_ui_egui::MarkiseApp;

fn harness() -> Harness<'static, MarkiseApp> {
    Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(common::no_watch()),
        )
}

/// A worker's wake-up landing while `Harness::run` counts frames asks for more frames than
/// `max_steps` (Windows CI: "would have settled after 6 steps", the cause a worker's wake).
/// Here a stand-in worker wakes the UI from its own thread during each of the next 12 frames,
/// as late wake-ups on a loaded runner do; `common::run` waits them out instead of failing.
/// (With `Harness::run` in its place this test fails: "exceeded max_steps (4)".)
#[test]
fn run_waits_out_wake_ups_that_land_during_it() {
    let mut h = harness();
    common::settle(&mut h);
    let left = Arc::new(AtomicU32::new(12));
    let count = left.clone();
    h.ctx.on_end_pass(
        "a late wake-up",
        Arc::new(move |ui: &mut egui::Ui| {
            // (the passes run on the test thread, one at a time)
            let n = count.load(Ordering::SeqCst);
            if n > 0 {
                count.store(n - 1, Ordering::SeqCst);
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || ctx.request_repaint()).join().unwrap();
            }
        }),
    );
    common::run(&mut h);
    assert_eq!(left.load(Ordering::SeqCst), 0, "every wake-up landed during the run");
}

/// What `max_steps` is there for still fails: frames that keep asking for more frames from
/// the UI thread, with no worker involved.
#[test]
#[should_panic(expected = "no worker woke the UI")]
fn run_still_fails_a_repaint_loop_of_the_ui() {
    let mut h = harness();
    common::settle(&mut h);
    h.ctx.on_end_pass(
        "a repaint loop",
        Arc::new(|ui: &mut egui::Ui| ui.ctx().request_repaint()),
    );
    common::run(&mut h);
}
