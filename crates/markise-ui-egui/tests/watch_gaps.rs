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
