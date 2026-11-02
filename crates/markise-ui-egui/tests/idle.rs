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

/// Runs frames until the document is loaded and every background job (theme worker, vault
/// scan, loader, image decodes, word count, comment-presence scans, watch catch-ups) has
/// finished — the app's signal, not a time (issue #42); then a few frames for egui's own
/// layout passes, returning their repaint delays.
fn settle_and_measure(ctx: &Context, app: &mut MarkiseApp) -> Vec<Duration> {
    let t = Instant::now();
    loop {
        common::sync_watches(app, t);
        frame(ctx, app);
        if app.model().is_some() && !app.is_loading() && app.background_settled() {
            break;
        }
        assert!(
            t.elapsed() < common::HANG_GUARD,
            "document load / background never settled"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    (0..6).map(|_| frame(ctx, app)).collect()
}

fn assert_idle(delays: &[Duration], when: &str) {
    assert_eq!(
        *delays.last().unwrap(),
        Duration::MAX,
        "idle frames must not schedule repaints ({when}): {delays:?}"
    );
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
    assert_idle(&settle_and_measure(&ctx, &mut app), "after the open");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The status bar's word count (counted over several frames for a large document, debounced
/// after edits) and the margin's comment-presence tracking (debounced, chunked, with an idle
/// rescan 2 s after the last edit) are background work too: settling waits for them, after a
/// large-document open and after an edit, and the window is idle afterwards.
#[test]
fn idle_after_large_open_and_edit_with_word_count_and_comment_margin() {
    let dir = std::env::temp_dir().join(format!("markise-ui-idle-large-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("big.md");
    // larger than the word count's one-frame size: counted over several frames
    let mut text = markise_corpus::generate(1024 * 1024, 5);
    text.push_str("\nA {>>remark<<} for the margin.\n");
    std::fs::write(&p, text).unwrap();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    app.set_show_word_count(true);
    app.set_comment_display(markise_engine::settings::CommentDisplay::Margin);
    assert_idle(&settle_and_measure(&ctx, &mut app), "after a large open");
    let counted = app.word_count_stats().expect("word count shown");
    // an edit: word count debounce and recount, comment-presence recheck and idle rescan
    let scans = app.viewer().unwrap().comment_scans().0;
    let at = app.document_text().unwrap().rfind("A {>>remark").unwrap();
    let session = &mut app.viewer().unwrap().ed.session;
    session.set_selection(markise_doc::Selection::single(
        markise_doc::DocPos::new(at),
        markise_doc::DocPos::new(at),
    ));
    session.insert_text("more words ").unwrap();
    assert_idle(&settle_and_measure(&ctx, &mut app), "after an edit");
    assert!(
        app.viewer().unwrap().comment_scans().0 > scans,
        "settling waited for the comment-presence rescan of the edited document (its idle \
         safety net runs 2 s of egui time after the last edit)"
    );
    let after = app.word_count_stats().unwrap();
    assert!(
        after.reconciles > counted.reconciles,
        "the edit was counted: {counted:?} → {after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #132: while the window is occluded or minimized the app asks for no repaint, even
/// with work that would otherwise schedule frames (the word count's debounce after an edit);
/// visible again, it resumes that work.
#[test]
fn hidden_window_requests_no_repaint_and_resumes_when_shown() {
    let dir = std::env::temp_dir().join(format!("markise-ui-idle-hidden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.md");
    std::fs::write(&p, markise_corpus::generate(200 * 1024, 3)).unwrap();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    app.set_show_word_count(true);
    assert_idle(&settle_and_measure(&ctx, &mut app), "after the open");
    app.viewer().unwrap().ed.session.insert_text("more words ").unwrap();
    let hidden_frame = |ctx: &Context, app: &mut MarkiseApp, occluded: bool, minimized: bool| {
        let mut raw = input();
        raw.viewports.insert(
            ViewportId::ROOT,
            egui::ViewportInfo {
                occluded: Some(occluded),
                minimized: Some(minimized),
                ..Default::default()
            },
        );
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        out.textures_delta.clear();
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        out.viewport_output[&ViewportId::ROOT].repaint_delay
    };
    for (occluded, minimized) in [(true, false), (false, true), (true, true)] {
        for _ in 0..3 {
            assert_eq!(
                hidden_frame(&ctx, &mut app, occluded, minimized),
                Duration::MAX,
                "no repaint while hidden (occluded {occluded}, minimized {minimized})"
            );
        }
    }
    // shown again: the pending word-count update schedules its frame, then everything settles
    let delay = hidden_frame(&ctx, &mut app, false, false);
    assert!(delay < Duration::from_secs(1), "work resumes once visible: {delay:?}");
    assert_idle(&settle_and_measure(&ctx, &mut app), "after showing it again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #86: a wheel notch over the document scrolls it smoothly (a frame per display refresh
/// while it moves, by exactly the notch's distance in total) and the window is idle again once
/// the motion and its short tail are over.
#[test]
fn wheel_scroll_moves_smoothly_then_goes_idle() {
    let dir = std::env::temp_dir().join(format!("markise-ui-idle-wheel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.md");
    std::fs::write(&p, markise_corpus::generate(200 * 1024, 3)).unwrap();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    assert_idle(&settle_and_measure(&ctx, &mut app), "after the open");
    // frames at 120 Hz from here on, with the pointer over the document
    let center = egui::pos2(700.0, 400.0);
    let mut t = ctx.input(|i| i.time) + 1.0;
    let mut timed = |ctx: &Context, app: &mut MarkiseApp, events: Vec<egui::Event>| {
        t += 1.0 / 120.0;
        let mut raw = input();
        raw.time = Some(t);
        raw.predicted_dt = 1.0 / 120.0;
        raw.events = events;
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        out.textures_delta.clear();
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        out.viewport_output[&ViewportId::ROOT].repaint_delay
    };
    timed(&ctx, &mut app, vec![egui::Event::PointerMoved(center)]);
    timed(&ctx, &mut app, vec![]);
    let y0 = app.viewer().unwrap().ed.view.scroll_y();
    let notch = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -1.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    };
    let mut ys = Vec::new();
    let mut delays = Vec::new();
    delays.push(timed(&ctx, &mut app, vec![notch]));
    ys.push(app.viewer().unwrap().ed.view.scroll_y());
    for _ in 0..60 {
        delays.push(timed(&ctx, &mut app, vec![]));
        ys.push(app.viewer().unwrap().ed.view.scroll_y());
    }
    let moved = ys.last().unwrap() - y0;
    assert!(moved > 0.0, "the notch scrolled down");
    // ~90 % of the notch within ~120 ms: not all of it in the first frame, most by frame 15
    assert!(ys[0] - y0 < moved * 0.5, "spread over frames: {:?}", &ys[..4]);
    assert!(
        ys[15] - y0 > moved * 0.85,
        "mostly there after 125 ms: {} of {moved}",
        ys[15] - y0
    );
    // while moving, the next frame is wanted right away
    assert!(delays[..10].iter().all(|d| *d == Duration::ZERO), "{delays:?}");
    // idle again after the motion and its tail
    let rest: Vec<Duration> = (0..4).map(|_| timed(&ctx, &mut app, vec![])).collect();
    assert_idle(&rest, "after a wheel notch");
    assert_eq!(app.viewer().unwrap().ed.view.scroll_y(), *ys.last().unwrap(), "at rest");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #86 (review): a jump while a wheel notch is still moving the document (outline click,
/// anchor link, find) wins: the pending motion is dropped and the document stays where the
/// jump put it.
#[test]
fn jump_during_wheel_motion_stays_put() {
    let dir = std::env::temp_dir().join(format!("markise-ui-idle-jump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.md");
    std::fs::write(&p, markise_corpus::generate(200 * 1024, 3)).unwrap();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![p],
        ..Default::default()
    });
    assert_idle(&settle_and_measure(&ctx, &mut app), "after the open");
    let mut t = ctx.input(|i| i.time) + 1.0;
    let mut timed = |ctx: &Context, app: &mut MarkiseApp, events: Vec<egui::Event>| {
        t += 1.0 / 120.0;
        let mut raw = input();
        raw.time = Some(t);
        raw.predicted_dt = 1.0 / 120.0;
        raw.events = events;
        let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
        out.textures_delta.clear();
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    };
    timed(
        &ctx,
        &mut app,
        vec![egui::Event::PointerMoved(egui::pos2(700.0, 400.0))],
    );
    let notch = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -4.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    };
    timed(&ctx, &mut app, vec![notch]);
    timed(&ctx, &mut app, vec![]);
    let v = &mut app.viewer().unwrap().ed.view;
    assert!(v.scroll_y() > 0.0, "the notch started moving");
    // the jump (as the outline does) while the notch is still under way
    v.scroll_to_item(40);
    for _ in 0..30 {
        timed(&ctx, &mut app, vec![]);
    }
    // (the anchor: the absolute offset changes as estimated heights above become real)
    let anchor = app.viewer().unwrap().ed.view.anchor();
    assert_eq!(anchor, (40, 0.0), "the document stayed where the jump put it");
    let _ = std::fs::remove_dir_all(&dir);
}
