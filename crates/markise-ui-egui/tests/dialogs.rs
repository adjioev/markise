//! Dialogs (issue #35) and the pointer cursor over the window's regions (issue #36):
//!
//! - every yes/no question goes through the [`Confirm`] seam (answered here by [`Scripted`]
//!   instead of a native alert) with the right buttons, default and outcome;
//! - the in-app fallback dialog answers Return with the default button and Esc with Cancel;
//! - Rename, New File and New Folder edit the name inline in the file tree row (Enter, Esc,
//!   a click elsewhere, a refused name shown under the row);
//! - the I-beam cursor only over editable text.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{CursorIcon, Key};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::export::{ExportKind, ExportRequest};
use markise_theme::{Rgba, Theme};
use markise_ui_egui::confirm::{Choice, Confirm, Kind, Role, Scripted};
use markise_ui_egui::export_ui::ExportAction;
use markise_ui_egui::sidebar::TreeAction;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-dialogs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness_with(opts: AppOptions) -> H {
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
}

/// A harness whose questions are answered by `s`.
fn scripted(open: Vec<PathBuf>, s: &Arc<Scripted>) -> H {
    harness_with(AppOptions {
        open,
        confirm: s.clone() as Arc<dyn Confirm>,
        ..Default::default()
    })
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
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

fn rows_contain(h: &mut H, name: &str) {
    let name = name.to_string();
    wait_until(h, &format!("row {name}"), move |s| s.file_tree_rows().contains(&name));
}

fn edit(h: &mut H, text: &str) {
    if h.state().view_mode() == ViewMode::Rendered {
        h.state_mut().toggle_source();
        h.step();
    }
    let text = text.to_string();
    h.state_mut().edit_source_text(move |t| t.push_str(&text));
    h.step();
}

// ------------------------------------------------------------------ the seam, per question

#[test]
fn unsaved_changes_question_through_the_seam() {
    let d = tmpdir("unsaved");
    let (a, b) = (d.join("a.md"), d.join("b.md"));
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();
    let s = Scripted::new();
    let mut h = scripted(vec![a.clone()], &s);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "edited\n");

    // closing the window asks; Save is the default, Don't Save is destructive
    assert!(!h.state_mut().request_close());
    h.step();
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::UnsavedChanges);
    assert!(q.message.contains("a.md"));
    let labels: Vec<_> = q.buttons.iter().map(|b| (b.choice, b.role)).collect();
    assert_eq!(
        labels,
        [
            (Choice::Save, Role::Primary),
            (Choice::DontSave, Role::Destructive),
            (Choice::Cancel, Role::Cancel)
        ]
    );
    assert_eq!(q.default, Choice::Save);
    // unanswered: the question stays open
    h.run_steps(3);
    assert!(h.state().is_asking());
    // Cancel keeps the edits and the window
    s.answer(Choice::Cancel);
    h.step();
    assert!(!h.state().is_asking() && h.state().is_dirty());

    // opening another file asks; Save writes the edits, then opens it
    h.state_mut().open(b.clone());
    h.step();
    s.answer(Choice::Save);
    wait_until(&mut h, "saved and opened", |s| {
        idle(s) && s.document_path() == Some(b.as_path())
    });
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a\nedited\n");

    // Don't Save drops the edits and continues
    edit(&mut h, "dropped\n");
    assert!(!h.state_mut().request_close());
    h.step();
    s.answer(Choice::DontSave);
    h.step();
    assert!(h.state_mut().request_close(), "the window may close now");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "b\n");
    assert_eq!(s.asked().len(), 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn move_to_trash_question_through_the_seam() {
    // moving to the Trash itself is not exercised (it would touch the user's Trash)
    let d = tmpdir("trash");
    let f = d.join("2026-05-30.md");
    std::fs::write(&f, "k\n").unwrap();
    let s = Scripted::new();
    let mut h = scripted(vec![d.clone()], &s);
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Delete(f.clone()));
    h.step();
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::MoveToTrash);
    assert!(q.message.contains("“2026-05-30.md”"), "{}", q.message);
    assert_eq!(q.default, Choice::Cancel, "destructive: Cancel is the default");
    let action = q.button(Choice::MoveToTrash).expect("the action is labelled");
    assert!(action.label.starts_with("Move to "));
    assert_eq!(action.role, Role::Destructive);
    assert_eq!(h.state().delete_pending(), Some(f.as_path()));
    s.answer(Choice::Cancel);
    h.step();
    assert!(h.state().delete_pending().is_none() && f.is_file());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn open_link_question_through_the_seam() {
    let d = tmpdir("link");
    let f = d.join("a.md");
    std::fs::write(&f, "[x](vscode://file/a)\n").unwrap();
    let s = Scripted::new();
    let mut h = scripted(vec![f], &s);
    wait_until(&mut h, "load", idle);
    // web links open without asking
    h.state_mut().follow_link("https://example.org");
    h.step();
    assert!(s.asked().is_empty());
    h.state_mut().follow_link("vscode://file/a");
    h.step();
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::OpenLink);
    assert_eq!(q.detail, "vscode://file/a");
    assert_eq!(q.default, Choice::Cancel);
    s.answer(Choice::Open);
    h.step();
    assert!(!h.state().is_asking());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn export_overwrite_question_through_the_seam() {
    let d = tmpdir("export");
    let doc = d.join("note.md");
    std::fs::write(&doc, "# Note\n").unwrap();
    let s = Scripted::new();
    let mut h = scripted(vec![doc.clone()], &s);
    wait_until(&mut h, "load", idle);
    let out = d.join("note.html");
    let req = ExportRequest::new(
        ExportKind::Html,
        h.state().document_text().unwrap(),
        Some(doc.clone()),
        out.clone(),
        Theme::light(),
    );
    h.state_mut().start_export(req);
    wait_until(&mut h, "export", |a| !a.export_state().is_running());
    std::fs::remove_file(&out).unwrap();

    h.state_mut().export_action_for_test(ExportAction::OverwritePrevious);
    h.step();
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::ExportOverwrite);
    assert_eq!(q.default, Choice::Cancel);
    assert_eq!(q.button(Choice::Overwrite).map(|b| b.role), Some(Role::Destructive));
    s.answer(Choice::Cancel);
    h.step();
    assert!(!h.state().export_state().is_asking_overwrite() && !out.exists());

    h.state_mut().export_action_for_test(ExportAction::OverwritePrevious);
    h.step();
    s.answer(Choice::Overwrite);
    h.step();
    wait_until(&mut h, "second export", |a| !a.export_state().is_running());
    assert!(out.exists());
    assert_eq!(s.asked().len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn replace_theme_question_through_the_seam() {
    let cfg = tmpdir("theme-cfg");
    let src = tmpdir("theme-src");
    std::fs::write(src.join("harbour.css"), "body { background: #102030; color: #ddd }").unwrap();
    let s = Scripted::new();
    let mut h = harness_with(AppOptions {
        config: ConfigDir::Dir(cfg.clone()),
        confirm: s.clone() as Arc<dyn Confirm>,
        ..Default::default()
    });
    h.step();
    h.state_mut().import_theme(src.join("harbour.css"));
    wait_until(&mut h, "imported", |a| a.theme().page_bg == Rgba::hex(0x102030));
    std::fs::write(src.join("harbour.css"), "body { background: #fafafa }").unwrap();
    h.state_mut().import_theme(src.join("harbour.css"));
    wait_until(&mut h, "replace question", |a| a.is_asking());
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::ReplaceTheme);
    assert!(q.detail.contains("harbour.css"), "{}", q.detail);
    assert_eq!(q.default, Choice::Cancel);
    s.answer(Choice::Replace);
    wait_until(&mut h, "replaced", |a| a.theme().page_bg == Rgba::hex(0xfafafa));
    let _ = std::fs::remove_dir_all(&cfg);
    let _ = std::fs::remove_dir_all(&src);
}

// ------------------------------------------------------------------ the in-app fallback

#[test]
fn in_app_question_return_takes_the_default_and_esc_cancels() {
    let d = tmpdir("inapp");
    let f = d.join("keep.md");
    std::fs::write(&f, "k\n").unwrap();
    let mut h = harness_with(AppOptions {
        open: vec![d.clone()],
        ..Default::default()
    });
    h.step();
    let ctx = h.ctx.clone();
    // Return on Move to Trash chooses the default, Cancel: nothing is moved
    h.state_mut().tree_action(&ctx, TreeAction::Delete(f.clone()));
    h.run_steps(2);
    assert!(h.query_by_label_contains("Move “keep.md” to the").is_some());
    assert!(h.query_by_label("Cancel").is_some());
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert!(h.state().delete_pending().is_none() && f.is_file());
    // Esc cancels
    h.state_mut().tree_action(&ctx, TreeAction::Delete(f.clone()));
    h.run_steps(2);
    assert!(h.state().delete_pending().is_some());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().delete_pending().is_none() && f.is_file());
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ inline rename

/// A folder with `existing.md` and `a.md`, mounted, the Files tab shown.
fn tree_harness(name: &str) -> (H, PathBuf) {
    let d = tmpdir(name);
    std::fs::write(d.join("existing.md"), "x\n").unwrap();
    std::fs::write(d.join("a.md"), "a\n").unwrap();
    let mut h = harness_with(AppOptions {
        open: vec![d.clone()],
        ..Default::default()
    });
    h.step();
    h.state_mut().show_files_tab(true);
    rows_contain(&mut h, "existing.md");
    (h, d)
}

/// Waits until the inline field has the keyboard focus.
fn field_focused(h: &mut H) {
    let t = Instant::now();
    loop {
        h.step();
        let id = markise_ui_egui::sidebar::rename_field_id();
        if h.ctx.memory(|m| m.has_focus(id)) {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the rename field never got the focus"
        );
    }
    h.step();
}

fn type_text(h: &mut H, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.step();
}

#[test]
fn inline_rename_enter_selects_the_stem_and_renames() {
    let (mut h, d) = tree_harness("rename-enter");
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("a.md")));
    field_focused(&mut h);
    // the field shows the name; typing replaces the stem, the extension stays
    type_text(&mut h, "notes");
    assert_eq!(
        h.state().rename_state().map(|(_, n)| n.to_string()),
        Some("notes.md".into())
    );
    h.key_press(Key::Enter);
    h.step();
    assert!(h.state().rename_state().is_none());
    assert!(d.join("notes.md").is_file() && !d.join("a.md").exists());
    rows_contain(&mut h, "notes.md");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn inline_rename_esc_keeps_the_old_name() {
    let (mut h, d) = tree_harness("rename-esc");
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("a.md")));
    field_focused(&mut h);
    type_text(&mut h, "other");
    h.key_press(Key::Escape);
    h.step();
    assert!(h.state().rename_state().is_none());
    assert!(d.join("a.md").is_file() && !d.join("other.md").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn inline_rename_collision_shows_the_error_under_the_row_and_keeps_editing() {
    let (mut h, d) = tree_harness("rename-collision");
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("a.md")));
    field_focused(&mut h);
    type_text(&mut h, "existing");
    h.key_press(Key::Enter);
    h.run_steps(2);
    // still editing, with the reason; nothing replaced
    assert_eq!(
        h.state().rename_state().map(|(_, n)| n.to_string()),
        Some("existing.md".into())
    );
    let err = h.state().rename_error().expect("refused").to_string();
    assert!(err.contains("already exists"), "{err}");
    assert!(h.query_by_label(&err).is_some(), "the error is shown");
    assert_eq!(std::fs::read_to_string(d.join("existing.md")).unwrap(), "x\n");
    assert!(d.join("a.md").is_file());
    // the field has the focus again: a new name and Enter rename
    field_focused(&mut h);
    type_text(&mut h, "fresh");
    assert!(h.state().rename_error().is_none(), "typing clears the error");
    h.key_press(Key::Enter);
    h.step();
    assert!(d.join("fresh.md").is_file() && !d.join("a.md").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn inline_rename_click_elsewhere_commits_a_valid_name_and_drops_a_refused_one() {
    let (mut h, d) = tree_harness("rename-click");
    let ctx = h.ctx.clone();
    // a refused name: given up, the old name stays
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("a.md")));
    field_focused(&mut h);
    type_text(&mut h, "existing");
    h.get_by_label("Outline").click(); // somewhere else
    h.run_steps(2);
    assert!(h.state().rename_state().is_none());
    assert!(d.join("a.md").is_file());
    assert!(
        h.state().banner_text().is_some_and(|b| b.contains("Not renamed")),
        "{:?}",
        h.state().banner_text()
    );
    h.state_mut().show_files_tab(true);
    h.step();
    // a valid name: renamed
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("a.md")));
    field_focused(&mut h);
    type_text(&mut h, "clicked");
    h.get_by_label("existing.md").click();
    h.run_steps(2);
    assert!(h.state().rename_state().is_none());
    assert!(d.join("clicked.md").is_file() && !d.join("a.md").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn new_file_and_new_folder_are_named_inline() {
    let (mut h, d) = tree_harness("new-items");
    let ctx = h.ctx.clone();
    // New File: Untitled.md is created, opened and its name edited in its row
    h.state_mut().tree_action(&ctx, TreeAction::NewFile(d.clone()));
    assert!(d.join("Untitled.md").is_file());
    field_focused(&mut h);
    type_text(&mut h, "today");
    h.key_press(Key::Enter);
    wait_until(&mut h, "renamed and open", |s| {
        idle(s) && s.document_path() == Some(d.join("today.md").as_path())
    });
    assert!(!d.join("Untitled.md").exists());
    // New Folder: all of the name is selected
    h.state_mut().tree_action(&ctx, TreeAction::NewFolder(d.clone()));
    assert!(d.join("Untitled").is_dir());
    field_focused(&mut h);
    type_text(&mut h, "drafts");
    assert_eq!(
        h.state().rename_state().map(|(_, n)| n.to_string()),
        Some("drafts".into())
    );
    h.key_press(Key::Enter);
    h.step();
    assert!(d.join("drafts").is_dir() && !d.join("Untitled").exists());
    rows_contain(&mut h, "drafts");
    let _ = std::fs::remove_dir_all(&d);
}

// (refusal while a save runs: `files.rs`, rename_is_refused_while_a_save_is_running)

// ------------------------------------------------------------------ cursor (issue #36)

fn cursor_at(h: &mut H, p: egui::Pos2) -> CursorIcon {
    h.hover_at(p);
    h.step();
    h.step();
    h.output().platform_output.cursor_icon
}

#[test]
fn ibeam_only_over_editable_text() {
    let (mut h, d) = tree_harness("cursor");
    h.state_mut().open(d.join("a.md"));
    wait_until(&mut h, "load", idle);
    h.state_mut().set_show_toolbar(true);
    h.run_steps(3);
    // focus the document (the old bug: a focused editor set the I-beam everywhere)
    let doc = egui::pos2(620.0, 120.0);
    h.hover_at(doc);
    h.event(egui::Event::PointerButton {
        pos: doc,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.event(egui::Event::PointerButton {
        pos: doc,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.run_steps(2);
    assert_eq!(cursor_at(&mut h, doc), CursorIcon::Text, "editor surface");

    let row = h.get_by_label("existing.md").rect().center();
    assert_ne!(cursor_at(&mut h, row), CursorIcon::Text, "sidebar file row");
    let folder = h.get_by_label("Outline").rect().center();
    assert_ne!(cursor_at(&mut h, folder), CursorIcon::Text, "sidebar tab");
    let status = egui::pos2(500.0, 690.0);
    assert_ne!(cursor_at(&mut h, status), CursorIcon::Text, "status bar");
    let bar = h.state().toolbar().bar_rect().expect("toolbar shown");
    assert_ne!(
        cursor_at(&mut h, egui::pos2(bar.right() - 3.0, bar.center().y)),
        CursorIcon::Text,
        "toolbar background"
    );
    assert_ne!(
        cursor_at(&mut h, bar.left_center() + egui::vec2(14.0, 0.0)),
        CursorIcon::Text,
        "toolbar button"
    );

    // the inline rename field is editable text
    h.state_mut().start_rename(d.join("existing.md"));
    field_focused(&mut h);
    let id = markise_ui_egui::sidebar::rename_field_id();
    let field = h.ctx.read_response(id).expect("field shown").rect.center();
    assert_eq!(cursor_at(&mut h, field), CursorIcon::Text, "rename field");
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ questions block the app

fn cmd() -> egui::Modifiers {
    if cfg!(target_os = "macos") {
        egui::Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        }
    } else {
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        }
    }
}

#[test]
fn an_open_question_blocks_commands_and_typing() {
    // Codex review P2 of PR #38: ⌘N while the unsaved-close alert waits must not replace
    // the Close continuation the answer applies to
    let d = tmpdir("blocked");
    let a = d.join("a.md");
    std::fs::write(&a, "a\n").unwrap();
    let s = Scripted::new();
    let mut h = scripted(vec![a.clone()], &s);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "edited\n");
    let text = h.state().document_text();
    assert!(!h.state_mut().request_close());
    h.step();
    let q = s.last().expect("asked");
    // shortcuts (File → New, Save), the menu and typing do nothing while it is open
    h.key_press_modifiers(cmd(), Key::N);
    h.step();
    h.key_press_modifiers(cmd(), Key::S);
    h.step();
    let ctx = h.ctx.clone();
    assert!(
        !h.state_mut()
            .menu_action(&ctx, &markise_ui_egui::menu::MenuAction::Command("file.new"))
    );
    h.event(egui::Event::Text("zzz".into()));
    h.step();
    assert_eq!(h.state().document_text(), text, "nothing typed into the document");
    assert!(!h.state().is_untitled() && h.state().is_dirty() && !h.state().is_saving());
    assert_eq!(s.asked(), vec![q.clone()], "still the same question");
    // the answer applies to Close
    s.answer(Choice::DontSave);
    h.step();
    assert!(h.state_mut().request_close(), "Don't Save → the window may close");
    assert_eq!(h.state().document_path(), Some(a.as_path()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_replaced_question_gets_a_new_number() {
    // two questions with the same text are different questions: an answer is bound to the
    // number it was asked with
    let d = tmpdir("question-ids");
    let a = d.join("a.md");
    std::fs::write(&a, "a\n").unwrap();
    let s = Scripted::new();
    let mut h = scripted(vec![a], &s);
    wait_until(&mut h, "load", idle);
    edit(&mut h, "edited\n");
    assert!(!h.state_mut().request_close());
    h.step();
    let first = h.state().question().expect("open");
    // something replaces the continuation (here directly, as a background event could)
    h.state_mut().new_document();
    h.step();
    let second = h.state().question().expect("open");
    assert_eq!(first.message, second.message);
    assert_ne!(first.id, second.id);
    assert_eq!(
        s.asked().len(),
        2,
        "asked again: an alert for the first can't answer it"
    );
    s.answer(Choice::Cancel);
    h.step();
    assert!(!h.state().is_asking());
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ rename row scrolled away

#[test]
fn inline_rename_commits_when_its_row_is_scrolled_away_and_the_user_clicks_elsewhere() {
    // Codex review P2 of PR #38: blur handling must not depend on the row being drawn
    let d = tmpdir("rename-scrolled");
    for i in 0..80 {
        std::fs::write(d.join(format!("n{i:02}.md")), "x\n").unwrap();
    }
    let mut h = harness_with(AppOptions {
        open: vec![d.clone(), d.join("n40.md")],
        ..Default::default()
    });
    h.step();
    h.state_mut().show_files_tab(true);
    rows_contain(&mut h, "n79.md");
    wait_until(&mut h, "load", idle);
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("n00.md")));
    field_focused(&mut h);
    type_text(&mut h, "renamed");
    // scroll the tree far down: the edited row is not drawn any more
    let over_tree = egui::pos2(120.0, 300.0);
    h.hover_at(over_tree);
    for _ in 0..10 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            modifiers: Default::default(),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.step();
    }
    h.run_steps(2);
    assert!(h.query_by_label("n79.md").is_some(), "scrolled to the end");
    assert!(h.state().rename_state().is_some(), "wheel scrolling keeps editing");
    // back up: the field has the focus again; down again, and a click elsewhere
    let wheel = |h: &mut H, dy: f32| {
        h.hover_at(egui::pos2(120.0, 300.0));
        for _ in 0..10 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, dy),
                modifiers: Default::default(),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            });
            h.step();
        }
        h.run_steps(2);
    };
    wheel(&mut h, 400.0);
    field_focused(&mut h);
    wheel(&mut h, -400.0);
    assert!(h.state().rename_state().is_some(), "still editing");
    // a click in the document
    let doc = egui::pos2(620.0, 200.0);
    h.hover_at(doc);
    h.event(egui::Event::PointerButton {
        pos: doc,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.event(egui::Event::PointerButton {
        pos: doc,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.run_steps(2);
    assert!(h.state().rename_state().is_none(), "editing ended");
    assert!(
        d.join("renamed.md").is_file() && !d.join("n00.md").exists(),
        "committed"
    );
    let _ = std::fs::remove_dir_all(&d);
}

fn click(h: &mut H, p: egui::Pos2) {
    h.hover_at(p);
    h.event(egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.event(egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
}

#[test]
fn inline_rename_ends_on_a_click_on_the_tree_scroll_bar() {
    // owner rule (Codex review round 3 of PR #38): scrolling keeps editing, but any press
    // outside the field ends it — the scroll bar included, as in Finder
    let d = tmpdir("rename-scrollbar");
    for i in 0..80 {
        std::fs::write(d.join(format!("n{i:02}.md")), "x\n").unwrap();
    }
    let mut h = harness_with(AppOptions {
        open: vec![d.clone()],
        ..Default::default()
    });
    h.step();
    h.state_mut().show_files_tab(true);
    rows_contain(&mut h, "n79.md");
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("n02.md")));
    field_focused(&mut h);
    type_text(&mut h, "barred");
    // the scroll bar sits right of the rows
    let row = h.get_by_label("n05.md").rect();
    click(&mut h, egui::pos2(row.right() + 6.0, 400.0));
    h.run_steps(3);
    assert!(h.state().rename_state().is_none(), "editing ended");
    assert!(d.join("barred.md").is_file() && !d.join("n02.md").exists(), "committed");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn typing_works_right_after_cancelling_an_in_app_question() {
    // Codex review round 2 of PR #38: the focus comes back after the modal is gone
    let d = tmpdir("refocus");
    let a = d.join("a.md");
    std::fs::write(&a, "Hello\n").unwrap();
    let mut h = harness_with(AppOptions {
        open: vec![a.clone()],
        ..Default::default()
    });
    wait_until(&mut h, "load", idle);
    click(&mut h, egui::pos2(620.0, 120.0));
    h.run_steps(2);
    {
        let ed = &mut h.state_mut().viewer().unwrap().ed;
        ed.set_selection(markise_doc::Selection::caret(5));
    }
    h.run_steps(2);
    type_text(&mut h, "!");
    assert_eq!(h.state().document_text().as_deref(), Some("Hello!\n"));
    assert!(!h.state_mut().request_close());
    h.run_steps(2);
    assert!(h.query_by_label("Don’t Save").is_some(), "the in-app question");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().is_asking());
    type_text(&mut h, "?");
    h.run_steps(2);
    assert_eq!(
        h.state().document_text().as_deref(),
        Some("Hello!?\n"),
        "typing goes to the document again"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn inline_rename_never_steals_the_focus_back_from_another_widget() {
    // Codex review round 4 of PR #38: scroll the row away, Tab to another widget, scroll back
    let d = tmpdir("rename-tab-away");
    for i in 0..80 {
        std::fs::write(d.join(format!("n{i:02}.md")), "x\n").unwrap();
    }
    let mut h = harness_with(AppOptions {
        open: vec![d.clone()],
        ..Default::default()
    });
    h.step();
    h.state_mut().show_files_tab(true);
    rows_contain(&mut h, "n79.md");
    let ctx = h.ctx.clone();
    h.state_mut().tree_action(&ctx, TreeAction::Rename(d.join("n00.md")));
    field_focused(&mut h);
    type_text(&mut h, "tabbed");
    let wheel = |h: &mut H, dy: f32| {
        h.hover_at(egui::pos2(120.0, 300.0));
        for _ in 0..10 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, dy),
                modifiers: Default::default(),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            });
            h.step();
        }
        h.run_steps(2);
    };
    wheel(&mut h, -400.0);
    assert!(h.state().rename_state().is_some(), "scrolling keeps editing");
    let field = markise_ui_egui::sidebar::rename_field_id();
    // another widget takes the focus
    h.key_press(Key::Tab);
    h.run_steps(2);
    let other = h.ctx.memory(|m| m.focused());
    assert!(
        other.is_some_and(|f| f != field),
        "Tab focused another widget: {other:?}"
    );
    assert!(h.state().rename_state().is_none(), "the rename is resolved");
    assert!(d.join("tabbed.md").is_file() && !d.join("n00.md").exists(), "committed");
    wheel(&mut h, 400.0);
    assert_eq!(h.ctx.memory(|m| m.focused()), other, "the focus stays where it went");
    let _ = std::fs::remove_dir_all(&d);
}
