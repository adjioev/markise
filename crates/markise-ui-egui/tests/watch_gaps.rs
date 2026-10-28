//! File watches are registered off the UI thread (ADR 0010), so the app keeps working while a
//! registration is pending. Changes made in that window must not be lost: the real
//! `MarkiseApp` is driven headlessly with the watcher worker held inside its registration
//! change (`JobHooks::before_watch`).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use markise_engine::jobs::JobHooks;
use markise_ui_egui::{AppOptions, MarkiseApp};

type Held = Arc<(Mutex<bool>, Condvar)>;

fn gate() -> (Held, Receiver<()>, Arc<JobHooks>) {
    let held: Held = Arc::new((Mutex::new(true), Condvar::new()));
    let (tx, entered) = channel();
    let (h2, tx) = (held.clone(), Mutex::new(tx));
    let hooks = Arc::new(JobHooks {
        before_watch: Some(Box::new(move || {
            let _ = tx.lock().unwrap().send(());
            let (m, cv) = &*h2;
            let mut g = m.lock().unwrap();
            while *g {
                g = cv.wait(g).unwrap();
            }
        })),
        ..Default::default()
    });
    (held, entered, hooks)
}

fn release(held: &Held) {
    *held.0.lock().unwrap() = false;
    held.1.notify_all();
}

fn harness(open: Vec<PathBuf>, hooks: Arc<JobHooks>) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        hooks,
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
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Review of #27 (P1): the tree is listed at once while the folder watch is still pending; a
/// file created in between appears once the watch is in effect.
#[test]
fn a_file_created_before_the_folder_watch_takes_effect_appears_in_the_tree() {
    let dir = std::env::temp_dir().join(format!("markise-watch-gaps-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("first.md"), "# First\n").unwrap();
    let (held, entered, hooks) = gate();
    let mut h = harness(vec![dir.clone()], hooks);
    wait_until(&mut h, "listing", |a| {
        a.file_tree_rows().iter().any(|r| r == "first.md")
    });
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("folder watch registration");
    std::fs::write(dir.join("second.md"), "# Second\n").unwrap();
    release(&held);
    wait_until(&mut h, "refreshed listing", |a| {
        a.file_tree_rows().iter().any(|r| r == "second.md")
    });
    let _ = std::fs::remove_dir_all(&dir);
}

fn hold(held: &Held, entered: &Receiver<()>) {
    while entered.try_recv().is_ok() {}
    *held.0.lock().unwrap() = true;
}

/// Review of #27, round 2 (P1): after the app's own save, a later registration gap compares
/// with the saved version, so restoring the originally opened text on disk is noticed.
#[test]
fn an_external_restore_after_our_save_is_noticed_across_a_folder_switch() {
    let root = std::env::temp_dir().join(format!("markise-watch-gaps-ack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let doc = root.join("a").join("doc.md");
    std::fs::write(&doc, "version A\n").unwrap();
    let (held, entered, hooks) = gate();
    release(&held);
    let mut h = harness(vec![doc.clone()], hooks);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    h.state_mut().toggle_source();
    h.run_steps(1);
    h.state_mut()
        .edit_source_text(|t| *t = t.replace("version A", "version B"));
    h.run_steps(1);
    h.state_mut().save();
    wait_until(&mut h, "save", |a| !a.is_busy() && !a.is_dirty());
    h.state_mut().toggle_source();
    std::thread::sleep(Duration::from_millis(300));
    wait_until(&mut h, "settle", |a| !a.is_busy());
    assert_eq!(std::fs::read_to_string(&doc).unwrap(), "version B\n");

    hold(&held, &entered);
    h.state_mut().open(root.join("b")); // mount another folder: the document's watch changes
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("registration change");
    std::fs::write(&doc, "version A\n").unwrap();
    release(&held);
    wait_until(&mut h, "restored text", |a| {
        a.viewed_text().is_some_and(|t| t == "version A\n") && !a.is_busy()
    });
    let _ = std::fs::remove_dir_all(&root);
}

/// Review of #27, round 3: a deletion seen through an ordinary watcher event is known to the
/// catch-up, so recreating the file with the same content inside a registration gap clears
/// the "deleted" state.
#[test]
fn recreating_a_deleted_file_with_the_same_content_in_a_gap_is_noticed() {
    let root = std::env::temp_dir().join(format!("markise-watch-gaps-del-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let doc = root.join("a").join("doc.md");
    std::fs::write(&doc, "same text\n").unwrap();
    let (held, entered, hooks) = gate();
    release(&held);
    let mut h = harness(vec![doc.clone()], hooks);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    // the deletion must arrive as an ordinary watcher event, not through a catch-up
    h.state().wait_for_watches().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    h.run_steps(3);
    std::fs::remove_file(&doc).unwrap();
    wait_until(&mut h, "deleted", |a| a.document_deleted());

    hold(&held, &entered);
    h.state_mut().open(root.join("b")); // the document's watch changes
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("registration change");
    std::fs::write(&doc, "same text\n").unwrap();
    release(&held);
    wait_until(&mut h, "recreated", |a| !a.document_deleted() && !a.is_busy());
    assert!(h.state().banner_text().is_none(), "{:?}", h.state().banner_text());
    let _ = std::fs::remove_dir_all(&root);
}

/// Review of #27, round 4: a reload that finds the editor's own text again is still the
/// watcher's new baseline. Sequence: the catch-up sees B (its baseline becomes B), the disk
/// returns to A before the UI's reload reads it (A = the editor's text: nothing to do), then
/// another registration gap changes the disk to B; that must be reported.
#[test]
fn a_same_content_reload_is_the_new_baseline() {
    let root = std::env::temp_dir().join(format!("markise-watch-gaps-same-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let doc = root.join("a").join("doc.md");
    std::fs::write(&doc, "version A\n").unwrap();
    let (held, entered, hooks) = gate();
    release(&held);
    // the loader can be held before it reads (armed by the test)
    let load_armed: Held = Arc::new((Mutex::new(false), Condvar::new()));
    let (load_tx, load_entered) = channel();
    let (armed2, load_tx) = (load_armed.clone(), Mutex::new(load_tx));
    let load_hook: markise_engine::load::StageHook = Arc::new(move |stage, _generation| {
        if stage != markise_engine::load::Stage::Read {
            return;
        }
        let (m, cv) = &*armed2;
        let mut g = m.lock().unwrap();
        if *g {
            let _ = load_tx.lock().unwrap().send(());
            while *g {
                g = cv.wait(g).unwrap();
            }
        }
    });
    let app = MarkiseApp::new(AppOptions {
        open: vec![doc.clone()],
        hooks,
        load_hook: Some(load_hook),
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    h.state().wait_for_watches().unwrap();

    // gap 1: the disk becomes B; the catch-up reports it (worker baseline B)
    *load_armed.0.lock().unwrap() = true;
    hold(&held, &entered);
    h.state_mut().open(root.join("b"));
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("registration change 1");
    std::fs::write(&doc, "version B\n").unwrap();
    release(&held);
    h.state().wait_for_watches().unwrap();
    // the UI's reload starts and is held before it reads; meanwhile the disk returns to A
    let t = Instant::now();
    while load_entered.try_recv().is_err() {
        assert!(t.elapsed() < Duration::from_secs(30), "reload never started");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(&doc, "version A\n").unwrap();
    *load_armed.0.lock().unwrap() = false;
    load_armed.1.notify_all();
    wait_until(&mut h, "reload done", |a| !a.is_busy());
    h.run_steps(3);
    h.state().wait_for_watches().unwrap();
    assert_eq!(h.state().viewed_text(), Some("version A\n"));

    // gap 2: the disk becomes B again
    hold(&held, &entered);
    h.state_mut().open(root.join("a"));
    entered
        .recv_timeout(Duration::from_secs(30))
        .expect("registration change 2");
    std::fs::write(&doc, "version B\n").unwrap();
    release(&held);
    wait_until(&mut h, "B shown", |a| {
        a.viewed_text().is_some_and(|t| t == "version B\n") && !a.is_busy()
    });
    let _ = std::fs::remove_dir_all(&root);
}

/// Save As (#22) moves the watch to the new file, with what was written as its baseline: a
/// change to the new file made while that watch is being registered is shown, and the old
/// file no longer drives the document.
#[test]
fn save_as_moves_the_watch_to_the_new_file() {
    let root = std::env::temp_dir().join(format!("markise-watch-gaps-saveas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("c")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let old = root.join("a").join("doc.md");
    let new = root.join("c").join("copy.md"); // outside the mounted folder: a new watch
    std::fs::write(&old, "original\n").unwrap();
    let (held, entered, hooks) = gate();
    release(&held);
    let mut h = harness(vec![old.clone()], hooks);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    h.state().wait_for_watches().unwrap();

    hold(&held, &entered);
    h.state_mut().queue_save_dialog_answer(Some(new.clone()));
    h.state_mut().save_as();
    let t = Instant::now();
    while entered.try_recv().is_err() {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "the new file's watch was never registered"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read_to_string(&new).unwrap(), "original\n");
    assert_eq!(h.state().document_path(), Some(new.as_path()));
    std::fs::write(&new, "changed by another program\n").unwrap();
    release(&held);
    wait_until(&mut h, "new file's change shown", |a| {
        a.viewed_text().is_some_and(|t| t == "changed by another program\n") && !a.is_busy()
    });

    // the old file is not the document any more
    std::fs::write(&old, "old file edited\n").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    h.run_steps(10);
    h.state().wait_for_watches().unwrap();
    h.run_steps(10);
    assert_eq!(h.state().viewed_text(), Some("changed by another program\n"));
    let _ = std::fs::remove_dir_all(&root);
}
