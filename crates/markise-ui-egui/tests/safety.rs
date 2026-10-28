//! Review follow-up: no silent loss of edits, conflict-aware saving, watcher races.
//! Drives the real `MarkiseApp` headlessly (egui_kittest, no window).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-safety-{name}-{}", std::process::id()));
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
        // the menu's ⌘/⇧ shortcut hints are not in egui's bundled fonts: tofu, as in the app
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

fn idle(a: &MarkiseApp) -> bool {
    a.model().is_some() && !a.is_busy()
}

fn edit(h: &mut Harness<'static, MarkiseApp>, from: &str, to: &str) {
    if !h.state().in_source_mode() {
        h.state_mut().toggle_source();
        h.run_steps(1);
    }
    h.state_mut().edit_source_text(|t| *t = t.replace(from, to));
    h.run_steps(1);
}

#[test]
fn opening_another_file_asks_and_cancel_keeps_edits() {
    // review item 1
    let d = tmpdir("gate");
    let a = d.join("a.md");
    let b = d.join("b.md");
    std::fs::write(&a, "# A\n\nalpha\n").unwrap();
    std::fs::write(&b, "# B\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "alpha", "ALPHA");
    h.state_mut().open(b.clone());
    h.run_steps(2);
    assert!(h.state().is_asking(), "unsaved edits: ask before replacing");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(!h.state().is_asking());
    assert_eq!(h.state().document_path(), Some(a.as_path()));
    assert!(h.state().is_dirty());
    // Save in the dialog writes, then opens
    h.state_mut().open(b.clone());
    h.run_steps(2);
    h.get_by_label("Save").click();
    wait_until(&mut h, "save then open", |s| {
        s.document_path() == Some(b.as_path()) && !s.is_busy()
    });
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "# A\n\nALPHA\n");
    // Discard drops edits and opens
    edit(&mut h, "# B", "# Bee");
    h.state_mut().open(a.clone());
    h.run_steps(2);
    h.get_by_label("Discard").click();
    wait_until(&mut h, "discard then open", |s| {
        s.document_path() == Some(a.as_path()) && !s.is_busy()
    });
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "# B\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn close_with_unsaved_edits_is_intercepted() {
    let d = tmpdir("close");
    let a = d.join("a.md");
    std::fs::write(&a, "text\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    assert!(h.state_mut().request_close(), "clean document closes");
    edit(&mut h, "text", "changed");
    assert!(!h.state_mut().request_close(), "dirty document does not close");
    h.run_steps(2);
    assert!(h.state().is_asking());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn edits_made_while_an_open_is_pending_are_not_dropped() {
    let d = tmpdir("pending");
    let a = d.join("a.md");
    let big = d.join("big.md");
    std::fs::write(&a, "alpha\n").unwrap();
    std::fs::write(&big, markise_corpus::generate(2 << 20, 9)).unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    h.state_mut().toggle_source();
    h.run_steps(1);
    h.state_mut().open(big.clone()); // clean: starts loading
    // edit before the load completes
    h.state_mut().edit_source_text(|t| t.push_str("late edit\n"));
    wait_until(&mut h, "load finished", |s| !s.is_loading());
    assert!(h.state().is_asking(), "the finished load waits for a decision");
    assert_eq!(h.state().document_path(), Some(a.as_path()));
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn failed_save_keeps_the_document_dirty_and_retry_works() {
    // review item 2
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir("retry");
    let a = d.join("a.md");
    std::fs::write(&a, "one\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "one", "two");
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    h.state_mut().save();
    wait_until(&mut h, "save attempt", |s| !s.is_busy());
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        h.state()
            .banner_text()
            .unwrap_or_default()
            .starts_with("error: Save failed")
    );
    assert!(h.state().is_dirty(), "still unsaved after the failure");
    assert!(h.state().title().contains('•'));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\n");
    h.state_mut().save();
    wait_until(&mut h, "retry", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "two\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn save_refuses_to_overwrite_an_external_change() {
    // review item 3
    let d = tmpdir("conflict");
    let a = d.join("a.md");
    std::fs::write(&a, "base\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "base", "mine");
    std::fs::write(&a, "theirs\n").unwrap(); // before the watcher delivers anything
    h.state_mut().save();
    wait_until(&mut h, "save attempt", |s| !s.is_busy() && s.banner_text().is_some());
    let b = h.state().banner_text().unwrap();
    assert!(b == "save-conflict" || b.starts_with("conflict"), "{b}");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "theirs\n", "external change kept");
    assert!(h.state().is_dirty());
    // explicit overwrite of the observed version
    if b == "save-conflict" {
        h.state_mut().resolve_save_conflict_overwrite();
    } else {
        h.state_mut().save();
        wait_until(&mut h, "save attempt 2", |s| !s.is_busy());
        h.state_mut().resolve_save_conflict_overwrite();
    }
    wait_until(&mut h, "overwrite", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "mine\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn own_save_is_not_a_conflict_but_a_quick_external_write_is_seen() {
    // review item 4: no time-based suppression
    let d = tmpdir("selfwrite");
    let a = d.join("a.md");
    std::fs::write(&a, "v0\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "v0", "v1");
    h.state_mut().save();
    wait_until(&mut h, "save", |s| !s.is_busy() && !s.is_dirty());
    // our own write: watcher event → reload check → same content → nothing happens
    std::thread::sleep(Duration::from_millis(300));
    h.run_steps(5);
    wait_until(&mut h, "settle", |s| !s.is_busy());
    assert!(h.state().banner_text().is_none(), "{:?}", h.state().banner_text());
    // leave source mode (clean), then an external write right after our save is picked up
    h.state_mut().toggle_source();
    std::fs::write(&a, "external\n").unwrap();
    wait_until(&mut h, "external reload", |s| {
        s.viewed_text().is_some_and(|t| t == "external\n") && !s.is_busy()
    });
    // bursts: the last write wins (events during a load are queued, not dropped)
    for i in 0..5 {
        std::fs::write(&a, format!("burst {i}\n")).unwrap();
        std::thread::sleep(Duration::from_millis(15));
        h.step();
    }
    wait_until(&mut h, "last burst", |s| {
        s.viewed_text().is_some_and(|t| t == "burst 4\n") && !s.is_busy()
    });
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn edits_made_during_a_save_stay_unsaved() {
    // review item 7: background save; a stale completion must not clear newer edits
    let d = tmpdir("stale");
    let a = d.join("a.md");
    std::fs::write(&a, "one\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "one", "two");
    h.state_mut().save();
    // run frames until the save job is running (its result is polled on a later frame),
    // then edit again before that completion is processed
    let t = Instant::now();
    while !h.state().is_saving() {
        assert!(t.elapsed() < Duration::from_secs(10));
        h.step();
    }
    h.state_mut().edit_source_text(|t| *t = t.replace("two", "three"));
    wait_until(&mut h, "jobs", |s| !s.is_busy());
    assert!(h.state().is_dirty(), "the newer edit is still unsaved");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "two\n");
    h.state_mut().save();
    wait_until(&mut h, "second save", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "three\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn reload_from_disk_after_preview_edits_replaces_the_document() {
    // review item 11
    let d = tmpdir("accept");
    let a = d.join("a.md");
    std::fs::write(&a, "local base\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "local base", "local edit");
    h.state_mut().toggle_source(); // applied to the preview, unsaved
    wait_until(&mut h, "apply", |s| !s.is_busy());
    assert!(h.state().is_dirty());
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&a, "from disk\n").unwrap();
    wait_until(&mut h, "conflict", |s| {
        s.banner_text().is_some_and(|b| b.starts_with("conflict"))
    });
    h.state_mut().resolve_conflict_reload();
    h.run_steps(2);
    assert!(!h.state().is_dirty());
    assert_eq!(h.state().viewed_text().unwrap(), "from disk\n");
    assert!(h.state().banner_text().is_none());
    let _ = std::fs::remove_dir_all(&d);
}
