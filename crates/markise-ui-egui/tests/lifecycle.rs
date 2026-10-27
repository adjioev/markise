//! Second review: document lifecycle races, made deterministic with worker hooks (no sleeps
//! to "win" a race: the test holds a worker at a hook until it has done its part).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::jobs::JobHooks;
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-life-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

/// A gate a worker hook blocks on: `entered` reports arrival, `release` lets it continue.
struct Gate {
    entered: Receiver<u64>,
    release: Sender<()>,
}

fn gate_hook() -> (Gate, impl Fn(u64) + Send + Sync + 'static) {
    let (etx, entered) = channel();
    let (release, rrx) = channel::<()>();
    let (etx, rrx) = (Mutex::new(etx), Mutex::new(rrx));
    let hook = move |rev: u64| {
        let _ = etx.lock().unwrap().send(rev);
        let _ = rrx.lock().unwrap().recv();
    };
    (Gate { entered, release }, hook)
}

fn harness(open: Vec<PathBuf>, hooks: JobHooks) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        hooks: Arc::new(hooks),
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
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}

/// Runs frames until the gated worker has arrived at its hook.
fn wait_entered(h: &mut Harness<'static, MarkiseApp>, g: &Gate) {
    let t = Instant::now();
    loop {
        if g.entered.try_recv().is_ok() {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "worker never reached the hook");
        h.step();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn loaded(a: &MarkiseApp) -> bool {
    a.model().is_some() && !a.is_busy()
}

fn type_text(h: &mut Harness<'static, MarkiseApp>, from: &str, to: &str) {
    if !h.state().in_source_mode() {
        h.state_mut().toggle_source();
        h.step();
    }
    let b = h.state_mut().source_buffer_mut().unwrap();
    assert!(b.contains(from), "{from:?} not in buffer");
    *b = b.replacen(from, to, 1);
    h.step();
}

#[test]
fn save_then_close_asks_again_when_typing_during_the_save() {
    // item 2
    let d = tmpdir("close");
    let a = d.join("a.md");
    std::fs::write(&a, "one\n").unwrap();
    let (g, hook) = gate_hook();
    let mut h = harness(
        vec![a.clone()],
        JobHooks {
            before_save: Some(Box::new(hook)),
            ..Default::default()
        },
    );
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "one", "two");
    assert!(!h.state_mut().request_close());
    h.run_steps(2); // the modal takes a frame to become interactive
    h.get_by_label("Save").click();
    wait_entered(&mut h, &g); // the save job holds revision "two"
    type_text(&mut h, "two", "three"); // typed while saving
    g.release.send(()).unwrap();
    wait_until(&mut h, "save done", |s| !s.is_saving());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "two\n");
    assert!(h.state().is_dirty());
    assert!(h.state().is_asking(), "asked again instead of closing");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn save_then_open_asks_again_when_typing_during_the_save() {
    // item 2 (install / open continuation)
    let d = tmpdir("install");
    let (a, b) = (d.join("a.md"), d.join("b.md"));
    std::fs::write(&a, "alpha\n").unwrap();
    std::fs::write(&b, "beta\n").unwrap();
    let (g, hook) = gate_hook();
    let mut h = harness(
        vec![a.clone()],
        JobHooks {
            before_save: Some(Box::new(hook)),
            ..Default::default()
        },
    );
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "alpha", "ALPHA");
    h.state_mut().open(b.clone());
    h.run_steps(2);
    h.get_by_label("Save").click();
    wait_entered(&mut h, &g);
    type_text(&mut h, "ALPHA", "ALPHA!");
    g.release.send(()).unwrap();
    wait_until(&mut h, "save done", |s| !s.is_busy());
    assert_eq!(
        h.state().document_path(),
        Some(a.as_path()),
        "b not opened over newer edits"
    );
    assert!(h.state().is_asking());
    assert_eq!(h.state().document_text().unwrap(), "ALPHA!\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn reentering_source_mode_during_a_rebuild_keeps_the_text() {
    // item 3
    let d = tmpdir("reenter");
    let a = d.join("a.md");
    std::fs::write(&a, "first\n").unwrap();
    let (g, hook) = gate_hook();
    let mut h = harness(
        vec![a.clone()],
        JobHooks {
            before_rebuild: Some(Box::new(move |rev| {
                hook(rev);
                Ok(())
            })),
            ..Default::default()
        },
    );
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "first", "second");
    h.state_mut().toggle_source(); // preview: starts the rebuild
    wait_entered(&mut h, &g); // rebuild is held
    h.state_mut().toggle_source(); // back to source before it finishes
    h.step();
    assert_eq!(
        h.state().document_text().unwrap(),
        "second\n",
        "not restored from the old model"
    );
    type_text(&mut h, "second", "third");
    g.release.send(()).unwrap();
    h.state_mut().save();
    wait_until(&mut h, "save", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "third\n");
    let _ = std::fs::remove_dir_all(&d);
}

fn failing_rebuild(panic: bool) {
    let d = tmpdir(if panic { "panic" } else { "fail" });
    let a = d.join("a.md");
    std::fs::write(&a, "old text\n").unwrap();
    let mut h = harness(
        vec![a.clone()],
        JobHooks {
            before_rebuild: Some(Box::new(move |_| {
                if panic {
                    panic!("injected rebuild crash");
                }
                Err("injected failure".into())
            })),
            ..Default::default()
        },
    );
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "old text", "new text");
    h.state_mut().toggle_source(); // rebuild fails
    wait_until(&mut h, "failure reported", |s| {
        s.banner_text()
            .is_some_and(|b| b.contains("preview could not be updated"))
    });
    assert_eq!(h.state().document_text().unwrap(), "new text\n", "submitted text kept");
    assert!(h.state().is_dirty());
    // saving writes the authoritative text, not the stale model
    h.state_mut().save();
    wait_until(&mut h, "save", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "new text\n");
    // re-entering source mode shows the kept text
    h.state_mut().toggle_source();
    h.step();
    assert_eq!(h.state().document_text().unwrap(), "new text\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn failed_rebuild_keeps_the_submitted_text() {
    // item 4
    failing_rebuild(false);
}

#[test]
fn panicking_rebuild_keeps_the_submitted_text() {
    // item 4 (worker crash)
    failing_rebuild(true);
}

#[test]
fn discard_then_failed_open_keeps_the_edited_document_dirty() {
    // item 9
    let d = tmpdir("discard");
    let (a, b) = (d.join("a.md"), d.join("b.md"));
    std::fs::write(&a, "alpha\n").unwrap();
    std::fs::write(&b, "beta\n").unwrap();
    let mut h = harness(vec![a.clone()], JobHooks::default());
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "alpha", "edited");
    h.state_mut().open(b.clone());
    h.run_steps(2);
    std::fs::remove_file(&b).unwrap(); // the open will fail
    h.get_by_label("Discard").click();
    wait_until(&mut h, "open attempt fails", |s| {
        !s.is_busy() && s.banner_text().is_some_and(|b| b.starts_with("error"))
    });
    assert_eq!(h.state().document_path(), Some(a.as_path()));
    assert!(h.state().is_dirty(), "edits not falsely marked clean");
    assert_eq!(h.state().document_text().unwrap(), "edited\n");
    h.state_mut().save();
    wait_until(&mut h, "save", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "edited\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn overwrite_authorization_survives_further_edits() {
    // item 11
    let d = tmpdir("overwrite");
    let a = d.join("a.md");
    std::fs::write(&a, "base\n").unwrap();
    let mut h = harness(vec![a.clone()], JobHooks::default());
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "base", "mine");
    std::fs::write(&a, "theirs\n").unwrap();
    h.state_mut().save();
    wait_until(&mut h, "conflict", |s| !s.is_busy() && s.banner_text().is_some());
    if h.state().banner_text().as_deref() != Some("save-conflict") {
        // the watcher was faster: it reported the external change; saving now conflicts
        h.state_mut().save();
        wait_until(&mut h, "conflict 2", |s| {
            !s.is_busy() && s.banner_text().as_deref() == Some("save-conflict")
        });
    }
    type_text(&mut h, "mine", "mine, edited again");
    h.state_mut().resolve_save_conflict_overwrite();
    wait_until(&mut h, "overwrite", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "mine, edited again\n");
    assert!(h.state().banner_text().is_none(), "{:?}", h.state().banner_text());
    let _ = std::fs::remove_dir_all(&d);
}

/// Codex review of PR #21 (P1): a save of a source-mode buffer marks the session state of
/// that text saved whichever worker finishes first, and that state stays reachable by undo.
fn source_save_keeps_its_boundary(rebuild_first: bool) {
    let d = tmpdir(if rebuild_first { "srcsave-a" } else { "srcsave-b" });
    let a = d.join("a.md");
    std::fs::write(&a, "first\n").unwrap();
    let (g, hook) = gate_hook();
    let hooks = if rebuild_first {
        JobHooks {
            before_save: Some(Box::new(hook)),
            ..Default::default()
        }
    } else {
        JobHooks {
            before_rebuild: Some(Box::new(move |rev| {
                hook(rev);
                Ok(())
            })),
            ..Default::default()
        }
    };
    let mut h = harness(vec![a.clone()], hooks);
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "first", "second");
    if rebuild_first {
        h.state_mut().save(); // held at the hook
        wait_entered(&mut h, &g);
        h.state_mut().toggle_source(); // rebuild runs and is installed first
        wait_until(&mut h, "rebuild", |s| s.viewed_text() == Some("second\n"));
        assert!(h.state().is_saving());
        g.release.send(()).unwrap();
    } else {
        h.state_mut().toggle_source(); // rebuild held at the hook
        wait_entered(&mut h, &g);
        h.state_mut().save(); // the buffer is ahead: saves the buffer text
        wait_until(&mut h, "save", |s| !s.is_saving());
        g.release.send(()).unwrap();
    }
    wait_until(&mut h, "both workers", |s| {
        !s.is_busy() && s.viewed_text() == Some("second\n")
    });
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "second\n");
    assert!(!h.state().is_dirty(), "the session matches the disk");
    // more source edits, then undo back to the saved state: clean again
    type_text(&mut h, "second", "third");
    h.state_mut().toggle_source();
    if !rebuild_first {
        wait_entered(&mut h, &g);
        g.release.send(()).unwrap();
    }
    wait_until(&mut h, "rebuild 2", |s| {
        !s.is_busy() && s.viewed_text() == Some("third\n")
    });
    assert!(h.state().is_dirty());
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    h.step();
    assert_eq!(h.state().document_text().unwrap(), "second\n");
    assert!(!h.state().is_dirty(), "the saved state is a transaction boundary");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn source_save_marks_saved_when_the_rebuild_lands_first() {
    source_save_keeps_its_boundary(true);
}

#[test]
fn source_save_marks_saved_when_the_save_lands_first() {
    source_save_keeps_its_boundary(false);
}

/// Codex review round 2 of PR #21 (P1): a save inside a source-mode session whose text never
/// became the preview stays an undo boundary: A → (source) B, save, C, preview, undo → B clean.
#[test]
fn source_session_saves_split_the_source_edit() {
    let d = tmpdir("srcsplit");
    let a = d.join("a.md");
    std::fs::write(&a, "alpha\n").unwrap();
    let mut h = harness(vec![a.clone()], JobHooks::default());
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "alpha", "bravo");
    h.state_mut().save();
    wait_until(&mut h, "save B", |s| !s.is_saving());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "bravo\n");
    type_text(&mut h, "bravo", "charlie");
    h.state_mut().toggle_source(); // preview: C is rebuilt and installed
    wait_until(&mut h, "rebuild", |s| {
        !s.is_busy() && s.viewed_text() == Some("charlie\n")
    });
    assert!(h.state().is_dirty());
    let undo = |h: &mut Harness<'static, MarkiseApp>| {
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        h.step();
    };
    undo(&mut h);
    assert_eq!(h.state().document_text().unwrap(), "bravo\n");
    assert!(!h.state().is_dirty(), "B is what is on disk");
    undo(&mut h);
    assert_eq!(h.state().document_text().unwrap(), "alpha\n");
    assert!(h.state().is_dirty(), "A is not on disk");
    // and saving A really writes it
    h.state_mut().save();
    wait_until(&mut h, "save A", |s| !s.is_busy() && !s.is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "alpha\n");
    let _ = std::fs::remove_dir_all(&d);
}

/// Two saves in one source session: both saved states are boundaries; the last one is clean.
#[test]
fn two_saves_in_one_source_session() {
    let d = tmpdir("srcsplit2");
    let a = d.join("a.md");
    std::fs::write(&a, "one\n").unwrap();
    let mut h = harness(vec![a.clone()], JobHooks::default());
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "one", "two");
    h.state_mut().save();
    wait_until(&mut h, "save 2", |s| !s.is_saving());
    type_text(&mut h, "two", "three");
    h.state_mut().save();
    wait_until(&mut h, "save 3", |s| !s.is_saving());
    type_text(&mut h, "three", "four");
    h.state_mut().toggle_source();
    wait_until(&mut h, "rebuild", |s| !s.is_busy() && s.viewed_text() == Some("four\n"));
    let mut seen = Vec::new();
    for _ in 0..3 {
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        h.step();
        seen.push((h.state().document_text().unwrap(), h.state().is_dirty()));
    }
    assert_eq!(
        seen,
        [
            ("three\n".to_string(), false),
            ("two\n".to_string(), true),
            ("one\n".to_string(), true)
        ]
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Codex review round 3 of PR #21: B saved and installed, then C saved in a second source
/// session before its install — B must stay an undo boundary.
#[test]
fn earlier_saved_source_state_stays_a_boundary() {
    let d = tmpdir("srcsplit3");
    let a = d.join("a.md");
    std::fs::write(&a, "alpha\n").unwrap();
    let mut h = harness(vec![a.clone()], JobHooks::default());
    wait_until(&mut h, "load", loaded);
    type_text(&mut h, "alpha", "bravo");
    h.state_mut().save();
    wait_until(&mut h, "save B", |s| !s.is_saving());
    h.state_mut().toggle_source(); // preview: B installed
    wait_until(&mut h, "rebuild B", |s| {
        !s.is_busy() && s.viewed_text() == Some("bravo\n")
    });
    type_text(&mut h, "bravo", "charlie"); // back in source mode
    h.state_mut().save();
    wait_until(&mut h, "save C", |s| !s.is_saving());
    h.state_mut().toggle_source();
    wait_until(&mut h, "rebuild C", |s| {
        !s.is_busy() && s.viewed_text() == Some("charlie\n")
    });
    assert!(!h.state().is_dirty());
    let mut seen = Vec::new();
    for _ in 0..2 {
        h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
        h.step();
        seen.push((h.state().document_text().unwrap(), h.state().is_dirty()));
    }
    assert_eq!(seen, [("bravo\n".to_string(), true), ("alpha\n".to_string(), true)]);
    let _ = std::fs::remove_dir_all(&d);
}
