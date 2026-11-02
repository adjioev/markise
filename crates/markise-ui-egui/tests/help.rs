//! Help menu (issue #63): the welcome tutorial — once at the first launch, again from Help →
//! Tutorial, always as a clean untitled scratch copy — and the keyboard shortcut cheat sheet
//! (⌘? / Ctrl+?, Help → Keyboard Shortcuts) with the keymap in effect.

mod common;

use std::path::PathBuf;

use common::wait_until;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_ui_egui::menu::{MenuAction, Platform};
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-help-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(opts: AppOptions) -> H {
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), MarkiseApp::new(opts))
}

fn tutorial_open(a: &MarkiseApp) -> bool {
    a.is_untitled() && a.viewed_text() == Some(markise_ui_egui::tutorial::TEXT)
}

fn steps(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

#[test]
fn first_launch_opens_the_tutorial_once_as_a_clean_scratch_copy() {
    let dir = tmpdir("first");
    let cfg = dir.join("config");
    let settings = cfg.join("markise").join("settings.json");
    {
        let mut h = harness(AppOptions {
            config: ConfigDir::Dir(cfg.clone()),
            welcome: true,
            ..Default::default()
        });
        wait_until(&mut h, "tutorial", tutorial_open);
        assert!(!h.state().is_dirty(), "nothing to save");
        assert_eq!(h.state().document_path(), None, "no file behind it");
        wait_until(&mut h, "flag written", |_| {
            std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("\"tutorialShown\": true"))
        });
        // closing the untouched tutorial asks nothing
        assert!(h.state_mut().request_close());
    }
    // the next launch does not show it again
    let mut h = harness(AppOptions {
        config: ConfigDir::Dir(cfg.clone()),
        welcome: true,
        ..Default::default()
    });
    wait_until(&mut h, "settings", MarkiseApp::settings_loaded);
    steps(&mut h, 5);
    assert!(h.state().viewed_text().is_none(), "shown only once");
    // nothing but the settings file was written
    let written: Vec<_> = walk(&dir);
    assert!(
        written.iter().all(|p| p.starts_with(cfg.join("markise"))),
        "{written:?}"
    );
}

fn walk(d: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn a_launch_that_opens_a_file_skips_the_tutorial_and_keeps_the_flag_unset() {
    let dir = tmpdir("withfile");
    let cfg = dir.join("config");
    let doc = dir.join("note.md");
    std::fs::write(&doc, "Note\n").unwrap();
    let mut h = harness(AppOptions {
        config: ConfigDir::Dir(cfg),
        open: vec![doc.clone()],
        welcome: true,
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    common::settle(&mut h);
    steps(&mut h, 3);
    assert_eq!(h.state().document_path(), Some(doc.as_path()));
    assert!(!tutorial_open(h.state()));
}

#[test]
fn without_the_welcome_option_nothing_opens() {
    let mut h = harness(AppOptions::default());
    common::settle(&mut h);
    steps(&mut h, 3);
    assert!(h.state().viewed_text().is_none());
}

#[test]
fn help_menu_has_tutorial_and_shortcuts_on_both_platforms() {
    for p in [Platform::Mac, Platform::Other] {
        let mut app = MarkiseApp::new(AppOptions::default());
        app.set_menu_platform(p);
        let bar = app.menu_model();
        let keys: Vec<String> = bar
            .menu("Help")
            .unwrap()
            .items()
            .iter()
            .map(|i| i.key.clone())
            .collect();
        let want: &[&str] = match p {
            Platform::Mac => &["help.tutorial", "help.shortcuts"],
            Platform::Other => &["app.about", "app.checkUpdates", "help.tutorial", "help.shortcuts"],
        };
        assert_eq!(keys, want, "{p:?}");
        let sc = bar.item("help.shortcuts").unwrap();
        assert_eq!(sc.label, "Keyboard Shortcuts");
        let c = sc.chord.as_ref().unwrap();
        assert!(c.shift && c.key == "/", "{p:?}: {c:?}");
        assert_eq!(bar.item("help.tutorial").unwrap().label, "Tutorial");
    }
}

#[test]
fn help_tutorial_replaces_the_document_asking_first_when_it_has_edits() {
    let dir = tmpdir("menu");
    let doc = dir.join("note.md");
    std::fs::write(&doc, "Note\n").unwrap();
    let mut h = harness(AppOptions {
        open: vec![doc.clone()],
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    let ctx = h.ctx.clone();
    h.state_mut().viewer().unwrap().ed.session.insert_text("edit ").unwrap();
    assert!(h.state().is_dirty());
    h.state_mut().menu_action(&ctx, &MenuAction::Command("help.tutorial"));
    h.step();
    assert!(h.state().is_asking(), "unsaved edits: asked first");
    h.state_mut()
        .answer_question(markise_ui_egui::confirm::Choice::DontSave);
    h.step();
    assert!(tutorial_open(h.state()));
    assert_eq!(
        std::fs::read_to_string(&doc).unwrap(),
        "Note\n",
        "the user's file is untouched"
    );
    // an edit to the tutorial makes it ask on close (it is a document like any other)
    h.state_mut().viewer().unwrap().ed.session.insert_text("x").unwrap();
    assert!(h.state().is_dirty());
}

fn cmd_shift() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            mac_cmd: true,
            command: true,
            shift: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            command: true,
            shift: true,
            ..Default::default()
        }
    }
}

#[test]
fn cmd_question_mark_opens_the_cheat_sheet_and_escape_closes_it() {
    let dir = tmpdir("keys");
    let doc = dir.join("note.md");
    std::fs::write(&doc, "Note\n").unwrap();
    let mut h = harness(AppOptions {
        open: vec![doc],
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    // ⇧/ arrives as "?" on most layouts, as "/" with Shift on others
    for key in [Key::Questionmark, Key::Slash] {
        h.key_press_modifiers(cmd_shift(), key);
        steps(&mut h, 2);
        assert!(h.state().is_shortcuts_open(), "{key:?}");
        h.event(egui::Event::Text("bold".into()));
        steps(&mut h, 2);
        assert_eq!(
            h.state().document_text().unwrap(),
            "Note\n",
            "typing goes to the search field"
        );
        h.key_press(Key::Escape);
        steps(&mut h, 2);
        assert!(!h.state().is_shortcuts_open(), "{key:?}");
    }
    // the window lists the shortcuts (Strong's row is drawn for the search "bold")
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("help.shortcuts"));
    steps(&mut h, 2);
    h.state_mut().set_shortcuts_query("bold");
    steps(&mut h, 3);
    let label = egui::accesskit::Role::Label;
    assert!(h.query_by_role_and_label(label, "Strong").is_some());
    assert!(h.query_by_role_and_label(label, "Heading 1").is_none(), "filtered out");
}

#[test]
fn cheat_sheet_reflects_key_binding_overrides() {
    let dir = tmpdir("override");
    let doc = dir.join("note.md");
    std::fs::write(&doc, "Note\n").unwrap();
    let mut h = harness(AppOptions {
        open: vec![doc],
        ..Default::default()
    });
    h.state_mut()
        .set_key_bindings_override(Some(vec![("Strong".into(), "Ctrl+Shift+J".into())]));
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    steps(&mut h, 2);
    let groups = h.state().shortcut_groups();
    let strong = groups
        .iter()
        .flat_map(|g| g.rows.iter())
        .find(|r| r.id == "format.strong")
        .unwrap();
    assert_eq!(strong.chord, "Ctrl+Shift+J");
    let all: Vec<&str> = groups.iter().flat_map(|g| g.rows.iter().map(|r| r.id)).collect();
    assert!(all.contains(&"help.shortcuts") && all.contains(&"file.openFolder"));
}

/// Codex review of PR #120: the cheat sheet is modal for the keyboard. Over the find bar,
/// Escape closes the sheet (not the bar) and typing goes to the sheet; over Quick Open,
/// Enter does not open the selected file.
#[test]
fn cheat_sheet_keys_do_not_reach_the_find_bar_or_quick_open() {
    let dir = tmpdir("modal");
    let doc = dir.join("note.md");
    let other = dir.join("other.md");
    std::fs::write(&doc, "Note\n").unwrap();
    std::fs::write(&other, "Other\n").unwrap();
    let mut h = harness(AppOptions {
        open: vec![doc.clone()],
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("find.open"));
    steps(&mut h, 2);
    h.key_press_modifiers(cmd_shift(), Key::Questionmark);
    steps(&mut h, 2);
    assert!(h.state().is_shortcuts_open());
    h.event(egui::Event::Text("x".into()));
    steps(&mut h, 2);
    assert_eq!(h.state().document_text().unwrap(), "Note\n");
    h.key_press(Key::Escape);
    steps(&mut h, 2);
    assert!(!h.state().is_shortcuts_open());
    assert_eq!(
        h.state().find_and_quick_open(),
        (true, false),
        "Escape closed only the sheet"
    );
    h.event(egui::Event::Text("y".into()));
    steps(&mut h, 2);
    assert_eq!(
        h.state().document_text().unwrap(),
        "Note\n",
        "focus is back in the find bar"
    );

    h.state_mut().menu_action(&ctx, &MenuAction::Command("file.quickOpen"));
    steps(&mut h, 2);
    h.event(egui::Event::Text("other".into()));
    wait_until(&mut h, "results", |a| a.find_and_quick_open().1);
    steps(&mut h, 5);
    h.state_mut().menu_action(&ctx, &MenuAction::Command("help.shortcuts"));
    steps(&mut h, 2);
    h.key_press(Key::Enter);
    steps(&mut h, 3);
    assert_eq!(
        h.state().document_path(),
        Some(doc.as_path()),
        "Enter did not open a file"
    );
    assert!(h.state().find_and_quick_open().1 && h.state().is_shortcuts_open());
}

/// Codex review of PR #120, round 2: a command that opens another window (Preferences via
/// its shortcut) closes the cheat sheet, so the sheet no longer holds the keyboard focus.
#[test]
fn opening_preferences_closes_the_cheat_sheet() {
    let mut h = harness(AppOptions::default());
    steps(&mut h, 2);
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("help.shortcuts"));
    steps(&mut h, 2);
    assert!(h.state().is_shortcuts_open());
    let comma = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    };
    h.key_press_modifiers(comma, Key::Comma);
    steps(&mut h, 2);
    assert!(!h.state().is_shortcuts_open(), "Preferences took over");
    assert!(h.state().is_preferences_open());
}

/// Codex review of PR #120, round 3: the Edit menu's text-field commands (Paste, Undo…)
/// act on the cheat sheet's search field: the sheet stays open and the document is untouched.
#[test]
fn edit_commands_keep_the_cheat_sheet_open_and_leave_the_document_alone() {
    let dir = tmpdir("editcmds");
    let doc = dir.join("note.md");
    std::fs::write(&doc, "Note\n").unwrap();
    let mut h = harness(AppOptions {
        open: vec![doc],
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("help.shortcuts"));
    steps(&mut h, 2);
    for id in [
        "edit.paste",
        "edit.cut",
        "edit.copy",
        "edit.undo",
        "edit.redo",
        "select.all",
    ] {
        h.state_mut().menu_action(&ctx, &MenuAction::Command(id));
        steps(&mut h, 2);
        assert!(h.state().is_shortcuts_open(), "{id} closed the sheet");
    }
    h.event(egui::Event::Paste("pasted".into()));
    steps(&mut h, 2);
    assert_eq!(h.state().document_text().unwrap(), "Note\n");
    assert!(h.state().is_shortcuts_open());
}
