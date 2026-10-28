//! File commands (issue #17; spec 04 §2 File menu, §4.1 sidebar file items, spec 05 default
//! extension): File → New, Save As…, Save of an untitled document, and the file tree's New
//! File / New Folder / Rename. The native save dialog is answered through a test hook.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::editor::keymap::Platform;
use markise_engine::jobs::JobHooks;
use markise_ui_egui::sidebar::TreeAction;
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(open: Vec<PathBuf>) -> Harness<'static, MarkiseApp> {
    harness_with(open, JobHooks::default())
}

fn harness_with(open: Vec<PathBuf>, hooks: JobHooks) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        hooks: Arc::new(hooks),
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
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    h.step();
}

fn idle(a: &MarkiseApp) -> bool {
    a.model().is_some() && !a.is_busy()
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        }
    }
}

/// Replaces the whole text through the source editor.
fn set_text(h: &mut Harness<'static, MarkiseApp>, text: &str) {
    if h.state().view_mode() == ViewMode::Rendered {
        h.state_mut().toggle_source();
        h.step();
    }
    *h.state_mut().source_buffer_mut().unwrap() = text.to_string();
    h.step();
}

#[test]
fn new_then_save_as_writes_the_file_and_adopts_it() {
    let d = tmpdir("saveas");
    let mut h = harness(Vec::new());
    h.step();
    h.key_press_modifiers(cmd(), Key::N);
    h.step();
    assert!(h.state().is_untitled(), "Cmd/Ctrl+N opens an untitled document");
    assert_eq!(h.state().document_path(), None);
    h.step();
    assert_eq!(h.state().title(), "Untitled — Markise");
    set_text(&mut h, "# Notes\n\nfirst line\n");
    assert!(h.state().is_dirty());
    h.step();
    assert_eq!(h.state().title(), "Untitled • — Markise");
    // no extension typed: `.md` is added (spec 05 default extension)
    h.state_mut().queue_save_dialog_answer(Some(d.join("notes")));
    h.key_press_modifiers(cmd().plus(Modifiers::SHIFT), Key::S);
    h.step();
    wait_until(&mut h, "save", |a| !a.is_saving());
    let file = d.join("notes.md");
    assert_eq!(std::fs::read(&file).unwrap(), b"# Notes\n\nfirst line\n");
    assert!(!h.state().is_untitled() && !h.state().is_dirty());
    assert_eq!(h.state().document_path(), Some(file.as_path()));
    h.step();
    assert_eq!(h.state().title(), "notes.md — Markise");
    // later saves go to the adopted file
    set_text(&mut h, "# Notes\n\nsecond\n");
    h.state_mut().save();
    wait_until(&mut h, "save", |a| !a.is_saving());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Notes\n\nsecond\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn save_of_an_untitled_document_goes_through_save_as() {
    let d = tmpdir("save-untitled");
    let mut h = harness(Vec::new());
    h.step();
    h.state_mut().new_document();
    h.step();
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
    set_text(&mut h, "side by side\n");
    // Cancel in the dialog: nothing is written, the document stays untitled and dirty
    h.state_mut().queue_save_dialog_answer(None);
    h.state_mut().save();
    h.step();
    assert!(h.state().is_untitled() && h.state().is_dirty() && !h.state().is_saving());
    h.state_mut().queue_save_dialog_answer(Some(d.join("sbs.md")));
    h.key_press_modifiers(cmd(), Key::S);
    h.step();
    wait_until(&mut h, "save", |a| !a.is_saving());
    assert_eq!(std::fs::read_to_string(d.join("sbs.md")).unwrap(), "side by side\n");
    assert!(!h.state().is_untitled() && !h.state().is_dirty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn new_with_unsaved_edits_asks_first() {
    let d = tmpdir("new-dirty");
    let a = d.join("a.md");
    std::fs::write(&a, "alpha\n").unwrap();
    let mut h = harness(vec![a.clone()]);
    wait_until(&mut h, "load", idle);
    set_text(&mut h, "alpha edited\n");
    h.state_mut().new_document();
    h.step();
    assert!(h.state().is_asking(), "Save / Discard / Cancel");
    assert!(!h.state().is_untitled());
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.step();
    assert!(!h.state().is_asking() && h.state().document_path() == Some(a.as_path()));
    h.state_mut().new_document();
    h.run_steps(2);
    h.get_by_label("Discard").click();
    h.step();
    assert!(h.state().is_untitled() && !h.state().is_dirty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "alpha\n", "discarded, not saved");
    // an empty untitled document is replaced without a question
    h.state_mut().open(a.clone());
    wait_until(&mut h, "load", |s| idle(s) && !s.is_untitled());
    assert!(!h.state().is_asking());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unsaved_untitled_document_saved_from_the_question_then_continues() {
    let d = tmpdir("ask-save");
    let b = d.join("b.md");
    std::fs::write(&b, "beta\n").unwrap();
    let mut h = harness(Vec::new());
    h.step();
    h.state_mut().new_document();
    h.step();
    set_text(&mut h, "draft\n");
    h.state_mut().open(b.clone());
    h.run_steps(2);
    assert!(h.state().is_asking());
    h.state_mut().queue_save_dialog_answer(Some(d.join("draft.md")));
    h.get_by_label("Save").click();
    wait_until(&mut h, "save and open", |s| {
        idle(s) && s.document_path() == Some(b.as_path())
    });
    assert_eq!(std::fs::read_to_string(d.join("draft.md")).unwrap(), "draft\n");
    let _ = std::fs::remove_dir_all(&d);
}

fn rows_contain(h: &mut Harness<'static, MarkiseApp>, name: &str) {
    let name = name.to_string();
    wait_until(h, &format!("row {name}"), move |s| s.file_tree_rows().contains(&name));
}

#[test]
fn sidebar_new_file_new_folder_and_rename() {
    let d = tmpdir("tree");
    std::fs::write(d.join("existing.md"), "x\n").unwrap();
    let mut h = harness(vec![d.clone()]);
    h.step();
    h.state_mut().show_files_tab(true);
    rows_contain(&mut h, "existing.md");

    // New File: Untitled.md, opened, its name offered for renaming
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::NewFile(d.clone()));
    let new = d.join("Untitled.md");
    assert!(new.is_file());
    wait_until(&mut h, "open", |s| idle(s) && s.document_path().is_some());
    assert_eq!(h.state().document_path(), Some(new.as_path()));
    assert_eq!(h.state().rename_state().map(|(_, n)| n), Some("Untitled.md"));
    rows_contain(&mut h, "Untitled.md");
    // a second one takes the next free name
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::NewFile(d.clone()));
    assert!(d.join("Untitled 2.md").is_file());
    wait_until(&mut h, "open", |s| {
        s.document_path() == Some(d.join("Untitled 2.md").as_path())
    });

    // Rename refuses an existing name and keeps the question open
    h.state_mut().set_rename_text("existing.md");
    h.state_mut().confirm_rename();
    assert!(h.state().rename_error().is_some_and(|e| e.contains("already exists")));
    assert_eq!(std::fs::read_to_string(d.join("existing.md")).unwrap(), "x\n");
    // typing and clicking Rename in the dialog renames; the open document follows
    h.state_mut().set_rename_text("renamed.md");
    h.run_steps(2);
    h.get_by_label("Rename").click();
    h.step();
    assert!(h.state().rename_state().is_none());
    let renamed = d.join("renamed.md");
    assert!(renamed.is_file() && !d.join("Untitled 2.md").exists());
    assert_eq!(h.state().document_path(), Some(renamed.as_path()));
    rows_contain(&mut h, "renamed.md");
    h.step();
    assert_eq!(h.state().title(), "renamed.md — Markise");

    // New Folder, renamed
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::NewFolder(d.clone()));
    assert!(d.join("Untitled").is_dir());
    h.state_mut().set_rename_text("notes");
    h.state_mut().confirm_rename();
    assert!(d.join("notes").is_dir() && !d.join("Untitled").exists());
    rows_contain(&mut h, "notes");
    // New File inside the new folder
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::NewFile(d.join("notes")));
    assert!(d.join("notes").join("Untitled.md").is_file());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn renaming_the_folder_of_the_open_document_moves_it() {
    let d = tmpdir("rename-folder");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    let f = d.join("sub").join("doc.md");
    std::fs::write(&f, "doc\n").unwrap();
    let mut h = harness(vec![d.clone(), f.clone()]);
    wait_until(&mut h, "load", |s| idle(s) && s.document_path().is_some());
    h.state_mut().start_rename(d.join("sub"));
    h.state_mut().set_rename_text("moved");
    h.state_mut().confirm_rename();
    let moved = d.join("moved").join("doc.md");
    assert_eq!(h.state().document_path(), Some(moved.as_path()));
    set_text(&mut h, "doc edited\n");
    h.state_mut().save();
    wait_until(&mut h, "save", |s| !s.is_saving());
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "doc edited\n");
    assert!(h.state().banner_text().is_none(), "{:?}", h.state().banner_text());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn file_menu_has_new_and_save_as_with_platform_shortcuts() {
    for (p, mods) in [(Platform::Mac, "meta"), (Platform::Other, "ctrl")] {
        let mut app = MarkiseApp::new(AppOptions::default());
        app.set_menu_platform(p);
        let bar = app.menu_model();
        let file = bar.menu("File").unwrap();
        let keys: Vec<&str> = file.items().iter().map(|i| i.key.as_str()).collect();
        for k in [
            "file.new",
            "file.open",
            "file.save",
            "file.saveAs",
            "file.rename",
            "file.reveal",
        ] {
            assert!(keys.contains(&k), "{p:?}: {k} in {keys:?}");
        }
        let new = bar.item("file.new").unwrap();
        assert!(new.enabled, "New works without a document");
        let c = new.chord.as_ref().unwrap();
        assert_eq!(c.key, "N");
        assert!(
            if mods == "meta" {
                c.meta && !c.ctrl
            } else {
                c.ctrl && !c.meta
            } && !c.shift
        );
        let save_as = bar.item("file.saveAs").unwrap();
        assert!(!save_as.enabled, "no document");
        let c = save_as.chord.as_ref().unwrap();
        assert!(c.key == "S" && c.shift && if mods == "meta" { c.meta } else { c.ctrl });
        assert!(!bar.item("file.rename").unwrap().enabled);
    }
}

#[test]
fn new_follows_key_binding_overrides() {
    let mut h = harness(Vec::new());
    h.state_mut()
        .set_key_bindings_override(Some(vec![("file.new".into(), "Cmd+Alt+N".into())]));
    h.step();
    h.key_press_modifiers(cmd(), Key::N);
    h.step();
    assert!(!h.state().is_untitled(), "the default chord was rebound");
    h.key_press_modifiers(cmd().plus(Modifiers::ALT), Key::N);
    h.step();
    assert!(h.state().is_untitled());
    let chord = h
        .state()
        .menu_model()
        .item("file.new")
        .and_then(|i| i.chord.clone())
        .unwrap();
    assert!(chord.alt && chord.key == "N");
}

#[test]
fn delete_asks_and_cancel_keeps_the_file() {
    // moving to the Trash itself is not exercised here (it would touch the user's Trash)
    let d = tmpdir("delete");
    let f = d.join("keep.md");
    std::fs::write(&f, "k\n").unwrap();
    let mut h = harness(vec![d.clone()]);
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Delete(f.clone()));
    h.run_steps(2);
    assert_eq!(h.state().delete_pending(), Some(f.as_path()));
    h.get_by_label("Cancel").click();
    h.step();
    assert!(h.state().delete_pending().is_none() && f.is_file());
    let _ = std::fs::remove_dir_all(&d);
}

/// A gate the save worker blocks on: `entered` reports arrival, `release` lets it continue.
struct Gate {
    entered: Receiver<u64>,
    release: Sender<()>,
}

fn gated_saves() -> (Gate, JobHooks) {
    let (etx, entered) = channel();
    let (release, rrx) = channel::<()>();
    let (etx, rrx) = (Mutex::new(etx), Mutex::new(rrx));
    let hooks = JobHooks {
        before_save: Some(Box::new(move |rev: u64| {
            let _ = etx.lock().unwrap().send(rev);
            let _ = rrx.lock().unwrap().recv();
        })),
        ..Default::default()
    };
    (Gate { entered, release }, hooks)
}

fn wait_entered(h: &mut Harness<'static, MarkiseApp>, g: &Gate) {
    let t = Instant::now();
    while g.entered.try_recv().is_err() {
        assert!(t.elapsed() < Duration::from_secs(10), "the save never started");
        h.step();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn save_as_never_replaces_a_file_the_dialog_did_not_confirm() {
    // review P1-1: `notes` was confirmed (free), `notes.md` exists → refused
    let d = tmpdir("unconfirmed");
    let existing = d.join("notes.md");
    std::fs::write(&existing, "keep me\n").unwrap();
    let mut h = harness(Vec::new());
    h.step();
    h.state_mut().new_document();
    h.step();
    set_text(&mut h, "draft\n");
    h.state_mut().queue_save_dialog_answer(Some(d.join("notes")));
    h.state_mut().save_as();
    h.step();
    assert!(!h.state().is_saving());
    assert!(h.state().banner_text().is_some_and(|b| b.contains("already exists")));
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep me\n");
    assert!(h.state().is_untitled() && h.state().is_dirty());
    // choosing the file itself replaces it (the dialog confirmed that name)
    h.state_mut().queue_save_dialog_answer(Some(existing.clone()));
    h.state_mut().save_as();
    wait_until(&mut h, "save", |a| !a.is_saving());
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "draft\n");
    assert!(!h.state().is_dirty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn save_as_over_a_file_changed_after_confirmation_is_a_conflict() {
    // review P1-2: the confirmed file's version is captured; a later change is not lost
    let d = tmpdir("as-conflict");
    let target = d.join("target.md");
    std::fs::write(&target, "old\n").unwrap();
    let (g, hooks) = gated_saves();
    let mut h = harness_with(Vec::new(), hooks);
    h.step();
    h.state_mut().new_document();
    h.step();
    set_text(&mut h, "mine\n");
    h.state_mut().queue_save_dialog_answer(Some(target.clone()));
    h.state_mut().save_as();
    wait_entered(&mut h, &g);
    std::fs::write(&target, "theirs, written after the dialog\n").unwrap();
    g.release.send(()).unwrap();
    wait_until(&mut h, "save", |a| !a.is_saving());
    assert_eq!(h.state().banner_text().as_deref(), Some("save-conflict"));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "theirs, written after the dialog\n"
    );
    assert!(h.state().is_untitled() && h.state().is_dirty(), "still unsaved");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rename_is_refused_while_a_save_is_running() {
    // review P1-5
    let d = tmpdir("rename-saving");
    let a = d.join("a.md");
    std::fs::write(&a, "one\n").unwrap();
    let (g, hooks) = gated_saves();
    let mut h = harness_with(vec![a.clone()], hooks);
    wait_until(&mut h, "load", idle);
    set_text(&mut h, "two\n");
    h.state_mut().save();
    wait_entered(&mut h, &g);
    h.state_mut().start_rename(a.clone());
    h.state_mut().set_rename_text("b.md");
    h.state_mut().confirm_rename();
    assert!(
        h.state()
            .rename_error()
            .is_some_and(|e| e.contains("save is in progress"))
    );
    assert!(a.exists() && !d.join("b.md").exists());
    g.release.send(()).unwrap();
    wait_until(&mut h, "save", |s| !s.is_saving());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "two\n");
    // after the save the rename goes through and the document follows, still clean
    h.state_mut().confirm_rename();
    let b = d.join("b.md");
    assert_eq!(h.state().document_path(), Some(b.as_path()));
    assert!(!h.state().is_dirty());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "two\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn overwrite_after_a_save_as_conflict_writes_the_save_as_destination() {
    // review round 2, P2: "Overwrite" retries the Save As target, never the old file
    let d = tmpdir("as-overwrite");
    let (a, b) = (d.join("a.md"), d.join("b.md"));
    std::fs::write(&a, "alpha\n").unwrap();
    std::fs::write(&b, "old b\n").unwrap();
    let (g, hooks) = gated_saves();
    let mut h = harness_with(vec![a.clone()], hooks);
    wait_until(&mut h, "load", idle);
    set_text(&mut h, "alpha edited\n");
    h.state_mut().queue_save_dialog_answer(Some(b.clone()));
    h.state_mut().save_as();
    wait_entered(&mut h, &g);
    std::fs::write(&b, "b changed by another program\n").unwrap();
    g.release.send(()).unwrap();
    wait_until(&mut h, "save", |s| !s.is_saving());
    assert_eq!(h.state().banner_text().as_deref(), Some("save-conflict"));
    h.state_mut().resolve_save_conflict_overwrite();
    wait_entered(&mut h, &g);
    g.release.send(()).unwrap();
    wait_until(&mut h, "overwrite", |s| !s.is_saving());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "alpha edited\n");
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "alpha\n",
        "the old file is untouched"
    );
    assert_eq!(h.state().document_path(), Some(b.as_path()));
    assert!(!h.state().is_dirty() && h.state().banner_text().is_none());
    let _ = std::fs::remove_dir_all(&d);
}
