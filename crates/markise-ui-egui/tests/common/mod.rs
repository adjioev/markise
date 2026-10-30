//! Shared waiting helpers for the UI tests (issue #42: no wall-clock flakes under load).
//!
//! Tests wait for *events* — a state the app reports, the app's "background settled" signal —
//! never for a fixed time. The only time bound is [`HANG_GUARD`], which turns a hang into a
//! failure; it is deliberately generous because a loaded machine (CI, the whole suite in
//! parallel) can be many times slower than an idle one, and a passing run never waits for it.

#![allow(dead_code)] // each test binary uses a different subset

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use markise_ui_egui::MarkiseApp;

/// Upper bound for any wait: a hang guard, not a latency expectation.
pub const HANG_GUARD: Duration = Duration::from_secs(180);

/// Steps until `f` holds (then one more frame), failing only after [`HANG_GUARD`].
pub fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(
            t.elapsed() < HANG_GUARD,
            "timed out waiting for {what} (banner: {:?})",
            h.state().banner_text()
        );
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}

/// Blocks until the app's watchers applied every request (their catch-up events queued for
/// the next frame), failing — instead of blocking — once [`HANG_GUARD`] since `started` is
/// spent. A watch that is not in effect is still applied (not an error here).
pub fn sync_watches(app: &MarkiseApp, started: Instant) {
    let left = HANG_GUARD.saturating_sub(started.elapsed());
    assert!(
        app.sync_background_watches(left).is_some(),
        "a file watcher did not answer within the hang guard"
    );
}

/// Steps until the app's background work has settled ([`MarkiseApp::background_settled`]):
/// every watch request is applied (its catch-up events queued before the next frame polls
/// them) and no worker has anything outstanding. After this, nothing wakes the UI on its own.
pub fn settle(h: &mut Harness<'static, MarkiseApp>) {
    let t = Instant::now();
    loop {
        sync_watches(h.state(), t);
        h.step();
        if h.state().background_settled() {
            return;
        }
        assert!(t.elapsed() < HANG_GUARD, "background work never settled");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// `Harness::run` once the background has settled: the remaining frames are egui's own
/// (layout passes, animations), so `max_steps` is not exhausted by late worker wake-ups.
pub fn run(h: &mut Harness<'static, MarkiseApp>) {
    settle(h);
    h.run();
}
