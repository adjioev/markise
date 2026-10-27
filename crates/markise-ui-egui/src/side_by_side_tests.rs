//! Side by Side tests that need crate-internal test hooks (`#[cfg(test)]` only): injected
//! incremental-update failures and the colouring pass counter.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Modifiers, pos2, vec2};
use egui_kittest::Harness;

use crate::{AppOptions, MarkiseApp, ViewMode};

fn open(name: &str, text: &str) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let d = std::env::temp_dir().join(format!("markise-sbs-unit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let d = std::fs::canonicalize(&d).unwrap();
    let f = d.join("doc.md");
    std::fs::write(&f, text).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![f.clone()],
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    (h, f)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn click(h: &mut Harness<'static, MarkiseApp>, p: egui::Pos2) {
    h.hover_at(p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run_steps(2);
}

fn text(h: &mut Harness<'static, MarkiseApp>) -> String {
    h.state().document_text().unwrap()
}

/// Review round 2: a failed incremental update falls back to the rebuild, which applies the
/// editor text verbatim; saving then writes exactly the editor's bytes (mixed endings).
#[test]
fn failure_rebuild_and_save_keep_the_editor_bytes() {
    let (mut h, file) = open("verbatim", "a\r\nmiddle\nz\r\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    crate::side_by_side::fail_incremental_updates_for_tests(true);
    let b = h.state_mut().source_buffer_mut().unwrap();
    b.insert_str(3, "new\n");
    h.run_steps(2);
    wait_until(&mut h, "rebuild", |a| {
        a.viewed_text().is_some_and(|t| t.contains("new")) && !a.is_busy()
    });
    crate::side_by_side::fail_incremental_updates_for_tests(false);
    assert_eq!(
        h.state().viewed_text().unwrap(),
        "a\r\nnew\nmiddle\nz\r\n",
        "rebuild == editor text"
    );
    assert_eq!(h.state().banner_text(), None, "the rebuild succeeded: no error");
    h.state_mut().save();
    wait_until(&mut h, "save", |a| !a.is_saving() && !a.is_dirty());
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"a\r\nnew\nmiddle\nz\r\n",
        "file == editor bytes"
    );
    // the next edit uses the incremental path again: the preview has it in the same frame
    // (`is_busy` is not checked: the watcher may be re-checking the file after our save)
    h.state_mut().source_buffer_mut().unwrap().push_str("psi\n");
    h.step();
    assert!(h.state().viewed_text().unwrap().ends_with("psi\n"));
}

/// Review P2: a paste that takes the text over the colouring limit is not tokenised.
#[test]
fn a_large_paste_is_not_coloured() {
    let (mut h, _f) = open("paste", "# A\n\nalpha\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    click(&mut h, pos2(420.0, 50.0));
    h.event(Event::Text("Q".into()));
    h.run_steps(2);
    let before = crate::source_highlight::passes();
    assert!(before > 0, "small documents are coloured");
    let big = "word *em* `code`\n".repeat(crate::source_highlight::highlight_limit() / 16);
    h.event(Event::Paste(big));
    h.run_steps(3);
    assert!(text(&mut h).len() > crate::source_highlight::highlight_limit());
    assert_eq!(
        crate::source_highlight::passes(),
        before,
        "no tokenising pass over the limit"
    );
}
