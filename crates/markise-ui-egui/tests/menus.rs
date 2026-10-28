//! Menus built from the command registry (spec 04 §2, ADR 0006 §3.1): labels, per-platform
//! accelerators, enabled/checked state, dispatch through `menu_action` / `run_command`, and a
//! native menu host replacing the egui menu bar without breaking idle (NFR-10).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Context, RawInput, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_engine::editor::keymap::Platform;
use markise_ui_egui::menu::{
    Check, EGUI_NATIVE_KEYS, Entry, MenuAction, MenuBar, MenuKind, Trigger, app_command, app_shortcuts,
};
use markise_ui_egui::{AppOptions, MarkiseApp, MenuHost, ViewMode};

fn tmpdoc(name: &str, text: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-menus-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = std::fs::canonicalize(&d).unwrap().join("note.md");
    std::fs::write(&p, text).unwrap();
    p
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

fn wait_loaded(h: &mut Harness<'static, MarkiseApp>) {
    let t = Instant::now();
    while h.state().model().is_none() || h.state().is_busy() {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out loading");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn titles(bar: &MenuBar) -> Vec<&str> {
    bar.menus.iter().map(|m| m.title.as_str()).collect()
}

#[test]
fn mac_menu_bar_from_registry_without_document() {
    let mut app = MarkiseApp::new(AppOptions::default());
    app.set_menu_platform(Platform::Mac);
    let bar = app.menu_model();
    assert_eq!(
        titles(&bar),
        [
            "Markise",
            "File",
            "Edit",
            "Paragraph",
            // the Format menu appears now that the registry has format commands (M2c)
            "Format",
            "View",
            // Back / Forward between opened notes (spec 06 §2.7)
            "Go",
            "Themes",
            "Window",
            "Help"
        ]
    );
    // ⌘[ is Outdent: Back is ⌥⌘←, Forward ⌥⌘→; disabled with no history
    let back = bar.item("go.back").unwrap();
    assert!(
        back.chord
            .as_ref()
            .is_some_and(|c| c.meta && c.alt && !c.ctrl && c.key == "Left")
    );
    assert!(!back.enabled);
    let fwd = bar.item("go.forward").unwrap();
    assert!(
        fwd.chord
            .as_ref()
            .is_some_and(|c| c.meta && c.alt && !c.ctrl && c.key == "Right")
    );
    assert_eq!(bar.menus[0].kind, MenuKind::App);
    assert_eq!(bar.menus[0].items()[0].label, "About Markise");
    assert_eq!(bar.menu("Window").unwrap().kind, MenuKind::Window);

    let folder = bar.item("file.openFolder").unwrap();
    assert_eq!(folder.label, "Open Folder…");
    let c = folder.chord.as_ref().unwrap();
    assert!(c.meta && c.shift && !c.ctrl && c.key == "O");
    // registry labels and chords; disabled without a document
    let h1 = bar.item("block.heading1").unwrap();
    assert_eq!(h1.label, "Heading 1");
    assert!(h1.chord.as_ref().is_some_and(|c| c.meta && c.key == "1"));
    assert!(!h1.enabled);
    let undo = bar.item("edit.undo").unwrap();
    assert!(undo.chord.as_ref().is_some_and(|c| c.meta && !c.shift && c.key == "Z"));
    assert!(!undo.enabled);
    let sbs = bar.item("view.sideBySide").unwrap();
    assert!(sbs.chord.as_ref().is_some_and(|c| c.meta && c.ctrl && c.key == "/"));
    assert_eq!(sbs.check, Check::Radio(false));
    // Quit is the app's own item (closing asks to save), not the OS terminate action
    assert!(bar.item("app.quit").is_some() && bar.item("app.exit").is_none());
    // caret / typing commands never appear in menus
    assert!(bar.item("caret.left").is_none() && bar.item("edit.insertText").is_none());
}

#[test]
fn other_platform_menus_use_ctrl_chords_and_have_no_app_menu() {
    let mut app = MarkiseApp::new(AppOptions::default());
    app.set_menu_platform(Platform::Other);
    let bar = app.menu_model();
    assert_eq!(
        titles(&bar),
        ["File", "Edit", "Paragraph", "Format", "View", "Go", "Themes", "Help"]
    );
    let back = bar.item("go.back").unwrap();
    assert!(
        back.chord.as_ref().is_some_and(|c| c.alt && !c.ctrl && c.key == "Left"),
        "Alt+Left"
    );
    assert_eq!(bar.menu("Help").unwrap().items()[0].key, "app.about");
    let sbs = bar.item("view.sideBySide").unwrap();
    let c = sbs.chord.as_ref().unwrap();
    assert!(c.ctrl && c.alt && !c.meta && c.key == "/", "spec 04 §3.3 Ctrl+Alt+/");
    let src = bar.item("view.sourceMode").unwrap();
    assert!(src.chord.as_ref().is_some_and(|c| c.ctrl && !c.alt && c.key == "/"));
    assert!(bar.item("app.exit").is_some() && bar.item("app.quit").is_none());
    let redo = bar.item("edit.redo").unwrap();
    // the shortest of its chords (Ctrl+Y, the Windows convention; Ctrl+Shift+Z also works)
    assert!(redo.chord.as_ref().is_some_and(|c| c.ctrl && !c.shift && c.key == "Y"));
}

#[test]
fn menu_actions_run_registry_commands_and_update_state() {
    let doc = tmpdoc("dispatch", "Hello menus\n");
    let mut h = harness(vec![doc]);
    wait_loaded(&mut h);
    let bar = h.state().menu_model();
    assert!(bar.item("block.heading1").unwrap().enabled);
    assert_eq!(bar.item("view.rendered").unwrap().check, Check::Radio(true));
    let export = bar.menu("File").unwrap().entries.iter().find_map(|e| match e {
        Entry::Submenu(m) if m.title == "Export" => Some(m.clone()),
        _ => None,
    });
    assert!(export.is_some_and(|m| m.enabled));

    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("block.heading1")));
    h.step();
    assert_eq!(h.state().document_text().unwrap(), "# Hello menus\n");
    let bar = h.state().menu_model();
    assert!(bar.item("edit.undo").unwrap().enabled, "undo enabled after an edit");
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("edit.undo")));
    h.step();
    assert_eq!(h.state().document_text().unwrap(), "Hello menus\n");

    // view modes: the radio group follows, Side by Side keeps both panes equal
    h.state_mut().menu_action(&ctx, &MenuAction::Command("view.sideBySide"));
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
    let bar = h.state().menu_model();
    assert_eq!(bar.item("view.sideBySide").unwrap().check, Check::Radio(true));
    assert_eq!(bar.item("view.rendered").unwrap().check, Check::Radio(false));
    // the source pane has the focus on entering: format and block commands are not available
    // there yet (M2d slice 2), Undo is
    assert!(!h.state().menu_model().item("block.heading2").unwrap().enabled);
    assert!(!h.state_mut().menu_action(&ctx, &MenuAction::Command("block.heading2")));
    assert!(h.state().menu_model().item("edit.redo").unwrap().enabled);
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("edit.redo")));
    h.step();
    assert_eq!(h.state().document_text().unwrap(), "# Hello menus\n");
    assert_eq!(
        h.state().source_buffer(),
        Some("# Hello menus\n"),
        "both panes show the session"
    );
    h.state_mut().menu_action(&ctx, &MenuAction::Command("view.rendered"));
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::Rendered);

    // toggles
    let before = h.state().highlight_code();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.highlightCode"));
    assert_eq!(h.state().highlight_code(), !before);
    let bar = h.state().menu_model();
    assert_eq!(bar.item("view.highlightCode").unwrap().check, Check::Toggle(!before));

    // source mode: format and block commands are disabled in the source editor (M2d slice 2)
    h.state_mut().menu_action(&ctx, &MenuAction::Command("view.sourceMode"));
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::Source);
    let bar = h.state().menu_model();
    assert!(!bar.item("block.heading1").unwrap().enabled);
    assert!(bar.item("edit.paste").unwrap().enabled);
}

/// A native menu host stand-in: queued actions, recorded syncs.
#[derive(Clone, Default)]
struct FakeHost(Arc<Mutex<HostState>>);

#[derive(Default)]
struct HostState {
    queued: Vec<MenuAction>,
    syncs: usize,
    rebuilds: usize,
    structure: String,
    last: Option<MenuBar>,
}

impl MenuHost for FakeHost {
    fn take_actions(&mut self) -> Vec<(MenuAction, Trigger)> {
        std::mem::take(&mut self.0.lock().unwrap().queued)
            .into_iter()
            .map(|a| (a, Trigger::Click))
            .collect()
    }

    fn sync(&mut self, bar: &MenuBar) {
        let mut s = self.0.lock().unwrap();
        s.syncs += 1;
        let st = bar.structure();
        if st != s.structure {
            s.rebuilds += 1;
            s.structure = st;
        }
        s.last = Some(bar.clone());
    }
}

#[test]
fn native_host_replaces_egui_menu_bar_and_receives_actions() {
    let doc = tmpdoc("host", "Text\n");
    let host = FakeHost::default();
    let mut h = harness(vec![doc]);
    h.state_mut().set_menu_host(Box::new(host.clone()));
    wait_loaded(&mut h);
    assert!(h.state().has_native_menu());
    assert!(h.query_by_label("Paragraph").is_none(), "no egui menu bar");
    assert!(host.0.lock().unwrap().syncs > 0);

    host.0
        .lock()
        .unwrap()
        .queued
        .push(MenuAction::Command("block.heading3"));
    h.step();
    h.step();
    assert_eq!(h.state().document_text().unwrap(), "### Text\n");
    let s = host.0.lock().unwrap();
    let undo = s.last.as_ref().unwrap().item("edit.undo").unwrap().clone();
    assert!(undo.enabled, "state change reached the host");
}

fn input() -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
        ..Default::default()
    }
}

fn frame(ctx: &Context, app: &mut MarkiseApp) -> Duration {
    let mut out = ctx.run_ui(input(), |ui| app.ui(ui));
    out.textures_delta.clear();
    let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    out.viewport_output[&ViewportId::ROOT].repaint_delay
}

/// NFR-10 with a native menu host: idle frames schedule no repaint and do not rebuild menus.
#[test]
fn idle_with_native_menu_requests_no_repaint_and_no_rebuild() {
    // plain paragraphs: no image/diagram/math workers whose late results repaint (tests/idle.rs
    // covers a full corpus document)
    let doc = tmpdoc("idle", &"Idle paragraph with some words.\n\n".repeat(2000));
    let host = FakeHost::default();
    let ctx = Context::default();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![doc],
        ..Default::default()
    });
    app.set_menu_host(Box::new(host.clone()));
    let t = Instant::now();
    while (app.model().is_none() || app.is_loading()) && t.elapsed() < Duration::from_secs(20) {
        frame(&ctx, &mut app);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.model().is_some());
    // settle: egui's first-frame layout passes and any background result
    let mut delays = Vec::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        let d = frame(&ctx, &mut app);
        delays.push(d);
        if d == Duration::MAX {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..3 {
        delays.push(frame(&ctx, &mut app));
    }
    assert!(delays.iter().rev().take(4).all(|d| *d == Duration::MAX), "{delays:?}");
    let rebuilds = host.0.lock().unwrap().rebuilds;
    for _ in 0..3 {
        frame(&ctx, &mut app);
    }
    assert_eq!(host.0.lock().unwrap().rebuilds, rebuilds, "no menu rebuild while idle");
}

/// Spec 04 §3.3: choosing a View-menu item sets the mode; ⌘/ and ⌃⌘/ toggle (Codex review of
/// PR #34 — the native macOS menu reports which one chose the item).
#[test]
fn view_mode_click_sets_and_shortcut_toggles() {
    let doc = tmpdoc("modes", "Mode test\n");
    let mut h = harness(vec![doc]);
    wait_loaded(&mut h);
    let ctx = h.ctx.clone();
    let src = MenuAction::Command("view.sourceMode");
    let sbs = MenuAction::Command("view.sideBySide");
    h.state_mut().menu_action(&ctx, &src);
    h.step();
    h.state_mut().menu_action(&ctx, &src);
    h.step();
    assert_eq!(
        h.state().view_mode(),
        ViewMode::Source,
        "a click on the checked mode keeps it"
    );
    h.state_mut().menu_action_by(&ctx, &src, Trigger::Shortcut);
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::Rendered, "the shortcut toggles");

    h.state_mut().menu_action(&ctx, &sbs);
    h.step();
    h.state_mut().menu_action(&ctx, &sbs);
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
    h.state_mut().menu_action_by(&ctx, &sbs, Trigger::Shortcut);
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::Rendered, "back to the remembered mode");
}

/// Registry host commands handled by the app's fixed shortcuts (app.rs `SC_*`).
const HOST_KEYS: [&str; 6] = [
    "file.new",
    "file.open",
    "file.save",
    "file.saveAs",
    "view.sourceMode",
    "view.toggleSidebar",
];

/// Every shortcut a menu shows on Windows/Linux (where menus only display them) and on macOS
/// without the native menu has a key handler (Codex review of PR #34).
#[test]
fn every_advertised_shortcut_has_a_handler() {
    for p in [Platform::Other, Platform::Mac] {
        let mut app = MarkiseApp::new(AppOptions::default());
        app.set_menu_platform(p);
        let handled: Vec<&str> = app_shortcuts(p).into_iter().map(|(id, _)| id).collect();
        let registry = markise_engine::editor::command::Registry::for_format(&markise_md::MarkdownFormat::default());
        let keymap = markise_engine::editor::keymap::Keymap::new(&registry, p);
        for i in app.menu_model().items() {
            let (Some(c), MenuAction::Command(id)) = (&i.chord, &i.action) else {
                continue;
            };
            let ok = if app_command(id).is_some() {
                handled.contains(id) || EGUI_NATIVE_KEYS.contains(id)
            } else {
                // registry commands: the editor routes its keymap; host commands: SC_* shortcuts
                keymap.lookup(c).contains(id) || HOST_KEYS.contains(id)
            };
            assert!(ok, "{p:?}: {id} shows {c:?} but nothing handles it");
        }
    }
}

/// The application commands' shortcuts work through the egui key path (here Side by Side's
/// Ctrl+Alt+/ of Windows/Linux, which toggles).
#[test]
fn app_shortcut_keys_dispatch_on_windows_and_linux() {
    let doc = tmpdoc("keys", "Keys\n");
    let mut h = harness(vec![doc]);
    h.state_mut().set_menu_platform(Platform::Other);
    wait_loaded(&mut h);
    let ctrl_alt = egui::Modifiers {
        ctrl: true,
        alt: true,
        command: true,
        ..Default::default()
    };
    h.key_press_modifiers(ctrl_alt, egui::Key::Slash);
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
    h.key_press_modifiers(ctrl_alt, egui::Key::Slash);
    h.step();
    assert_eq!(h.state().view_mode(), ViewMode::Rendered);
}

/// About Markise: in the application menu (macOS) and the Help menu (Windows/Linux); the
/// command opens the window, Escape closes it.
#[test]
fn about_command_opens_the_about_window() {
    let mut h = harness(Vec::new());
    h.state_mut().set_menu_platform(Platform::Other);
    h.run();
    // the Linux egui menu bar: Help → About Markise
    h.get_by_label("Help").click();
    h.run();
    h.get_by_label("About Markise").click();
    h.run();
    assert!(h.state().is_about_open());
    assert!(h.query_by_label_contains("The marquise of Markdown editors").is_some());
    assert!(h.query_by_label_contains(env!("CARGO_PKG_VERSION")).is_some());
    assert!(h.query_by_label("© The Markise contributors").is_some());
    assert!(h.query_by_label("Website").is_some() && h.query_by_label("GitHub").is_some());
    h.get_by_label("Licences & Third-Party Notices…").click();
    h.run();
    assert!(h.query_by_label_contains("Document").is_some(), "licence viewer open");
    h.key_press(egui::Key::Escape);
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().is_about_open());

    // macOS: the application menu's item runs the same command
    let ctx = h.ctx.clone();
    h.state_mut().set_menu_platform(Platform::Mac);
    let bar = h.state().menu_model();
    let item = bar.menus[0].items()[0].clone();
    assert_eq!(item.key, "app.about");
    assert!(h.state_mut().menu_action(&ctx, &item.action));
    h.run();
    assert!(h.state().is_about_open());
}

/// The egui fallback menu (Linux) shows shortcuts on radio and check items too (Codex review
/// round 2 of PR #34: Source Code Mode and Side by Side lost theirs).
#[test]
fn egui_fallback_shows_shortcuts_on_radio_items() {
    let doc = tmpdoc("fallback", "Fallback\n");
    let mut h = harness(vec![doc]);
    h.state_mut().set_menu_platform(Platform::Other);
    wait_loaded(&mut h);
    h.get_by_label("View").click();
    h.run();
    assert!(
        h.query_by_label("Source Code Mode Ctrl+/").is_some(),
        "radio item with its shortcut"
    );
    assert!(h.query_by_label("Side by Side Ctrl+Alt+/").is_some());
    assert!(h.query_by_label("Rendered").is_some(), "no shortcut, no trailing text");
    // plain items keep theirs
    assert!(h.query_by_label_contains("Ctrl+Shift+L").is_some());
}

/// Codex review of PR #35, round 5: each chord appears on exactly one menu item (native
/// key equivalents never collide), on both platforms.
#[test]
fn no_chord_on_two_menu_items() {
    fn walk(es: &[markise_ui_egui::menu::Entry], out: &mut Vec<(String, markise_engine::editor::keymap::Chord)>) {
        for e in es {
            match e {
                markise_ui_egui::menu::Entry::Item(it) => {
                    if let Some(c) = &it.chord {
                        out.push((it.label.clone(), c.clone()));
                    }
                }
                markise_ui_egui::menu::Entry::Submenu(m) => walk(&m.entries, out),
                _ => {}
            }
        }
    }
    // standard role items (Hide, Minimize…) carry native accelerators too
    fn roles(
        es: &[markise_ui_egui::menu::Entry],
        p: Platform,
        out: &mut Vec<(String, markise_engine::editor::keymap::Chord)>,
    ) {
        for e in es {
            match e {
                markise_ui_egui::menu::Entry::Role(r) => {
                    if let Some(c) = markise_ui_egui::menu::role_chord(*r, p) {
                        out.push((format!("{r:?}"), c));
                    }
                }
                markise_ui_egui::menu::Entry::Submenu(m) => roles(&m.entries, p, out),
                _ => {}
            }
        }
    }
    for p in [Platform::Mac, Platform::Other] {
        let mut app = MarkiseApp::new(AppOptions::default());
        app.set_menu_platform(p);
        let mut v = Vec::new();
        for m in &app.menu_model().menus {
            walk(&m.entries, &mut v);
            roles(&m.entries, p, &mut v);
        }
        for (i, (l, c)) in v.iter().enumerate() {
            assert!(
                v[i + 1..].iter().all(|(_, d)| d != c),
                "{p:?}: {l} shares {c:?} with another item"
            );
        }
    }
}

/// Codex review of PR #35, round 6: the standard role accelerators are reserved chords.
#[test]
fn role_accelerators_are_reserved_for_key_bindings() {
    for r in [
        markise_ui_egui::menu::Role::Hide,
        markise_ui_egui::menu::Role::HideOthers,
        markise_ui_egui::menu::Role::Minimize,
    ] {
        let c = markise_ui_egui::menu::role_chord(r, Platform::Mac).expect("macOS role chord");
        assert!(
            markise_engine::editor::keymap::reserved_chords(Platform::Mac).contains(&c),
            "{r:?}"
        );
    }
}
