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
