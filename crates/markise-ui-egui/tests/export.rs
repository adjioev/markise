//! File → Export wiring: the menu entries exist, an export runs off the UI thread, the status
//! bar reports the result, and "Export and Overwrite with Previous" repeats it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::export::{ExportKind, ExportRequest};
use markise_theme::Theme;
use markise_ui_egui::export_ui::{ExportAction, ExportMessage};
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(open: Vec<PathBuf>) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        ..Default::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

#[test]
fn export_menu_runs_jobs_and_previous() {
    let dir = tmpdir("menu");
    let doc = dir.join("note.md");
    std::fs::write(&doc, "# Note\n\nHello **export**.\n").unwrap();
    let mut h = harness(vec![doc.clone()]);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());

    // the submenu is reachable
    h.get_by_label("File").click();
    h.run();
    h.get_by_label("Export").hover();
    h.run();
    assert!(h.query_by_label("PDF").is_some());
    assert!(h.query_by_label("HTML (without styles)").is_some());
    assert!(h.query_by_label("Other Formats via Pandoc").is_some());
    h.key_press(egui::Key::Escape);
    h.run();

    let out = dir.join("note.html");
    let req = ExportRequest::new(
        ExportKind::Html,
        h.state().document_text().unwrap(),
        Some(doc.clone()),
        out.clone(),
        Theme::light(),
    );
    h.state_mut().start_export(req);
    wait_until(&mut h, "export", |a| !a.export_state().is_running());
    assert!(
        matches!(h.state().export_state().message(), Some(ExportMessage::Done { text, .. }) if text.contains("note.html")),
        "{:?}",
        h.state().export_state().message()
    );
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(html.contains("<strong>export</strong>"));
    assert!(h.state().export_state().has_previous());

    // overwrite with previous after an edit of the file on disk is not needed: re-export the
    // same snapshot to the same path
    std::fs::remove_file(&out).unwrap();
    // the first overwrite asks once; cancel writes nothing
    h.state_mut().export_action_for_test(ExportAction::OverwritePrevious);
    assert!(h.state().export_state().is_asking_overwrite());
    h.state_mut().answer_overwrite_for_test(false);
    h.run();
    assert!(!out.exists());
    h.state_mut().export_action_for_test(ExportAction::OverwritePrevious);
    h.state_mut().answer_overwrite_for_test(true);
    wait_until(&mut h, "second export", |a| !a.export_state().is_running());
    assert!(out.exists());
    // confirmed for this document: no question the next time
    h.state_mut().export_action_for_test(ExportAction::OverwritePrevious);
    assert!(!h.state().export_state().is_asking_overwrite());
    wait_until(&mut h, "third export", |a| !a.export_state().is_running());

    // Codex re-review P2: the approval is bound to the destination; a different previous
    // export (another file) asks again
    let other = dir.join("other.html");
    let req = ExportRequest::new(
        ExportKind::Html,
        h.state().document_text().unwrap(),
        Some(doc.clone()),
        other.clone(),
        Theme::light(),
    );
    h.state_mut().start_export(req);
    wait_until(&mut h, "export to another file", |a| !a.export_state().is_running());
    h.state_mut().export_action_for_test(ExportAction::OverwritePrevious);
    assert!(
        h.state().export_state().is_asking_overwrite(),
        "new destination must be confirmed"
    );
    h.state_mut().answer_overwrite_for_test(false);

    // a failing export reports its error in the status bar
    let bad = ExportRequest::new(
        ExportKind::Html,
        "# x\n".into(),
        None,
        dir.join("no-such-folder").join("x.html"),
        Theme::light(),
    );
    h.state_mut().start_export(bad);
    wait_until(&mut h, "failed export", |a| !a.export_state().is_running());
    assert!(matches!(
        h.state().export_state().message(),
        Some(ExportMessage::Failed(_))
    ));
}
