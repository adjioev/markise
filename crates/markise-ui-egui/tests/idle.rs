//! NFR-10: with a document open and no input, the app schedules no repaint.

mod common;

use std::time::{Duration, Instant};

use egui::{Context, RawInput, ViewportId};
use markise_ui_egui::{AppOptions, MarkiseApp};

fn input() -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
        ..Default::default()
    }
}

/// Runs a frame; returns the repaint delay egui asks for.
fn frame(ctx: &Context, app: &mut MarkiseApp) -> Duration {
    let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
    out.textures_delta.clear(); // headless: no renderer to apply them to
    let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    out.viewport_output[&ViewportId::ROOT].repaint_delay
}

#[test]
fn idle_document_requests_no_repaint() {
    let dir = std::env::temp_dir().join(format!("markise-ui-idle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.md");
    std::fs::write(&p, markise_corpus::generate(200 * 1024, 3)).unwrap();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    // run until the document is loaded and every background job has finished (issue #42:
    // late one-shot completions — theme worker, vault scan, loader, image decodes — used to
    // land in the measured frames under load). Waits on the app's signal, not on time.
    let t = Instant::now();
    loop {
        let _ = app.sync_background_watches();
        frame(&ctx, &mut app);
        if app.model().is_some() && !app.is_loading() && app.background_settled() {
            break;
        }
        assert!(
            t.elapsed() < common::HANG_GUARD,
            "document load / background never settled"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    // a few frames for egui's own layout passes; the background can no longer wake us
    let mut delays = Vec::new();
    for _ in 0..6 {
        delays.push(frame(&ctx, &mut app));
    }
    assert_eq!(
        *delays.last().unwrap(),
        Duration::MAX,
        "idle frames must not schedule repaints: {delays:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
