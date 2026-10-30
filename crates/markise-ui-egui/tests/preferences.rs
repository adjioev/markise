//! The Preferences window (issue #50; spec 04 §2.1, spec 05 §4): opened with ⌘, / Ctrl+, and
//! from the menus; every section renders; a setting changed in the window changes the app,
//! settings.json and the menu's check mark, and a menu change shows in the window; the search
//! filters by label; Reset to Defaults asks through the confirm seam; Esc closes and Tab moves
//! between controls; the window fits at 150 % zoom and follows dark mode.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::accesskit::Toggled;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use markise_engine::editor::keymap::Platform;
use markise_engine::settings::EditingSettings;
use markise_ui_egui::confirm::{Choice, Confirm, Kind, Scripted};
use markise_ui_egui::menu::{Check, MenuAction};
use markise_ui_egui::prefs::Section;
use markise_ui_egui::{AppOptions, Appearance, ConfigDir, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-prefs-{name}-{}", std::process::id()));
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

/// A harness with a document and a settings folder.
fn harness(dir: &Path) -> H {
    let doc = dir.join("note.md");
    std::fs::write(&doc, "# Note\n\nSome text.\n").unwrap();
    let cfg = dir.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut h = harness_with(AppOptions {
        open: vec![doc],
        config: ConfigDir::Dir(cfg),
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    h
}

fn settings_file(dir: &Path) -> PathBuf {
    dir.join("config").join("markise").join("settings.json")
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn wait_for_file(h: &mut H, path: &Path, what: &str, f: impl Fn(&str) -> bool) {
    let path = path.to_path_buf();
    wait_until(h, what, move |_| std::fs::read_to_string(&path).is_ok_and(|s| f(&s)));
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

fn open_at(h: &mut H, s: Section) {
    let ctx = h.ctx.clone();
    if !h.state().is_preferences_open() {
        assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("app.preferences")));
    }
    h.run_steps(3);
    h.get_by_role_and_label(egui::Role::Button, s.label()).click();
    h.run_steps(2);
}

/// Activates a control that may be below the fold of the settings pane: keyboard focus and
/// Space, as a keyboard user would (a pointer click needs the row scrolled into view).
fn click(h: &mut H, role: egui::Role, label: &str) {
    h.get_by_role_and_label(role, label).focus();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
}

fn toggled(h: &H, label: &str) -> bool {
    h.get_by_role_and_label(egui::Role::Switch, label)
        .accesskit_node()
        .toggled()
        == Some(Toggled::True)
}

fn menu_check(h: &H, id: &str) -> Check {
    h.state().menu_model().item(id).map(|i| i.check).unwrap_or(Check::None)
}

#[test]
fn opens_with_cmd_comma_and_from_the_menus_and_esc_closes() {
    let mut h = harness_with(AppOptions::default());
    common::run(&mut h);
    assert!(!h.state().is_preferences_open());
    // ⌘, (Ctrl+, elsewhere): the app's shortcut path
    h.key_press_modifiers(cmd(), Key::Comma);
    h.run_steps(2);
    assert!(h.state().is_preferences_open());
    assert!(h.query_all_by_label("Preferences").count() >= 1);
    // the search box has the keyboard focus; Esc still closes the window
    assert!(h.get_by_label("Search settings").is_focused());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().is_preferences_open());

    // Windows/Linux: Edit → Preferences… in the egui menu bar, Ctrl+, shown
    h.state_mut().set_menu_platform(Platform::Other);
    common::run(&mut h);
    h.get_by_label("Edit").click();
    h.run_steps(2);
    h.get_by_label_contains("Preferences…").click();
    h.run_steps(2);
    assert!(h.state().is_preferences_open());
    // no Done button (changes apply live): the × closes
    assert!(h.query_by_label("Done").is_none());
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(!h.state().is_preferences_open());

    // macOS: the application menu's item, enabled, runs the same command
    let ctx = h.ctx.clone();
    h.state_mut().set_menu_platform(Platform::Mac);
    let bar = h.state().menu_model();
    let item = bar.menus[0]
        .items()
        .into_iter()
        .find(|i| i.key == "app.preferences")
        .expect("in the application menu")
        .clone();
    assert!(item.enabled);
    assert!(h.state_mut().menu_action(&ctx, &item.action));
    h.run_steps(2);
    assert!(h.state().is_preferences_open());
}

#[test]
fn every_section_renders_its_settings() {
    let d = tmpdir("sections");
    let mut h = harness(&d);
    let expect: [(Section, &[&str]); 7] = [
        (Section::General, &["Custom key bindings"]),
        (
            Section::Editor,
            &[
                "Auto-pair brackets and quotes",
                "Auto-pair Markdown syntax",
                "Smart punctuation (convert on input)",
                "Smart dashes: -- as em dash",
            ],
        ),
        (
            Section::Markdown,
            &[
                "Render diagrams",
                "Wikilinks [[Note]]",
                "Comments & suggestions (CriticMarkup)",
                // "Show comments" labels the segments (filtered as a label node)
                "In the margin",
                "As bubbles (hover to read)",
                "Keep comments in exports",
                "Highlight code (this session)",
            ],
        ),
        (
            Section::Appearance,
            &[
                "Markise",
                "Drake",
                "Style",
                "Colors",
                "System",
                "Body font",
                "Show toolbar",
                "Top",
                "Show recent files in the sidebar",
                "Zoom In",
                "Import Theme…",
                "Theme Report…",
            ],
        ),
        (
            Section::Source,
            &["Line numbers", "Soft wrap", "Highlight current line", "Show whitespace"],
        ),
        (Section::Files, &["Settings file", "Images"]),
        (Section::Export, &[]),
    ];
    for (s, labels) in expect {
        open_at(&mut h, s);
        assert!(
            h.query_all_by_label(s.label()).count() >= 2,
            "{s:?}: list entry and title"
        );
        for l in labels {
            assert!(h.query_all_by_label(l).count() >= 1, "{s:?}: {l}");
        }
    }
    assert!(h.query_by_label_contains("Pandoc is found automatically").is_some());
    let _ = std::fs::remove_dir_all(&d);
}

/// A toggle in the window changes what the app does, settings.json and the menu's check mark,
/// exactly like the menu item; a menu change shows in the window.
#[test]
fn window_and_menus_change_the_same_settings_both_ways() {
    let d = tmpdir("sync");
    let mut h = harness(&d);
    let settings = settings_file(&d);

    // Appearance → Show toolbar: the toolbar disappears, the file and View → Toolbar follow
    open_at(&mut h, Section::Appearance);
    assert!(h.state().toolbar_state().is_some());
    assert_eq!(menu_check(&h, "view.toolbar"), Check::Toggle(true));
    click(&mut h, egui::Role::Switch, "Show toolbar");
    assert!(h.state().toolbar_state().is_none(), "the toolbar is gone");
    assert_eq!(menu_check(&h, "view.toolbar"), Check::Toggle(false));
    wait_for_file(&mut h, &settings, "showToolbar written", |s| {
        s.contains("\"showToolbar\": false")
    });
    // and back from the menu: the window's box follows
    let ctx = h.ctx.clone();
    h.state_mut().menu_action(&ctx, &MenuAction::Command("view.toolbar"));
    h.run_steps(2);
    assert!(toggled(&h, "Show toolbar"));
    assert!(h.state().toolbar_state().is_some());

    // Editor → auto-pair brackets: the editor's preferences, the file
    open_at(&mut h, Section::Editor);
    assert!(toggled(&h, "Auto-pair brackets and quotes"));
    h.get_by_label("Auto-pair brackets and quotes").click();
    h.run_steps(2);
    assert!(!h.state().theme_manager().editing().auto_pair_brackets);
    wait_for_file(&mut h, &settings, "editing written", |s| {
        s.contains("\"auto_pair_brackets\": false")
    });
    // smart dashes depend on smart punctuation (as in the old Edit menu)
    assert!(!h.state().preference_values().editing.smart_punctuation);
    h.get_by_label("Smart punctuation (convert on input)").click();
    h.run_steps(2);
    h.get_by_label("Smart dashes: -- as em dash").click();
    h.run_steps(2);
    assert!(h.state().preference_values().editing.smart_dashes_em);

    // Markdown → Render diagrams: the view stops rendering them, View menu unaffected (moved)
    open_at(&mut h, Section::Markdown);
    h.get_by_label("Render diagrams").click();
    h.run_steps(2);
    assert!(!h.state().diagrams());
    wait_for_file(&mut h, &settings, "diagrams written", |s| {
        s.contains("\"diagrams\": false")
    });
    // Highlight Code stays a View menu toggle: both ways
    h.get_by_label("Highlight code (this session)").click();
    h.run_steps(2);
    assert_eq!(menu_check(&h, "view.highlightCode"), Check::Toggle(false));
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.highlightCode"));
    h.run_steps(2);
    assert!(toggled(&h, "Highlight code (this session)"));

    // Source Mode: a command that left the menu (still bound by id) shows in the window
    open_at(&mut h, Section::Source);
    assert!(!toggled(&h, "Line numbers"));
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("view.sourceLineNumbers"));
    h.run_steps(2);
    assert!(toggled(&h, "Line numbers"));
    h.get_by_label("Soft wrap").click();
    h.run_steps(2);
    assert!(!h.state().source_prefs().soft_wrap);
    wait_for_file(&mut h, &settings, "source written", |s| {
        s.contains("\"line_numbers\": true") && s.contains("\"soft_wrap\": false")
    });

    // Appearance → light/dark and the View → Theme switcher
    open_at(&mut h, Section::Appearance);
    h.get_by_role_and_label(egui::Role::RadioButton, "Dark").click();
    h.run_steps(2);
    assert_eq!(h.state().theme_manager().appearance(), Appearance::Dark);
    wait_until(&mut h, "dark theme", |a| a.theme().dark);
    h.state_mut().menu_action(
        &ctx,
        &MenuAction::Theme(markise_theme::catalog::ThemeId::Builtin("drake".into())),
    );
    h.run_steps(2);
    assert_eq!(
        h.state().preference_values().theme,
        Some(markise_theme::catalog::ThemeId::Builtin("drake".into()))
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn search_filters_settings_by_label() {
    let mut h = harness_with(AppOptions::default());
    common::run(&mut h);
    open_at(&mut h, Section::Editor);
    h.get_by_label("Search settings").focus();
    h.run_steps(1);
    h.get_by_label("Search settings").type_text("wrap");
    h.run_steps(2);
    assert!(h.query_by_label("Soft wrap").is_some());
    assert!(h.query_by_label("Auto-pair brackets and quotes").is_none());
    assert!(h.query_by_label("Line numbers").is_none());
    // the matching section's title heads the results
    assert!(h.query_all_by_label("Source Mode").count() >= 2);
    // search words: "dark" finds the light/dark control in Appearance
    h.get_by_label("Search settings").focus();
    h.key_press_modifiers(cmd(), Key::A);
    h.get_by_label("Search settings").type_text("dark");
    h.run_steps(2);
    assert!(h.query_by_label("System").is_some());
    assert!(h.query_by_label("Soft wrap").is_none());
    // nothing matches
    h.key_press_modifiers(cmd(), Key::A);
    h.get_by_label("Search settings").type_text("zzz");
    h.run_steps(2);
    assert!(h.query_by_label("No settings match.").is_some());
    // choosing a section clears the search
    h.get_by_role_and_label(egui::Role::Button, "Editor").click();
    h.run_steps(2);
    assert!(h.query_by_label("Auto-pair brackets and quotes").is_some());
    assert_eq!(h.state_mut().preferences().query, "");
}

#[test]
fn reset_to_defaults_asks_then_restores_the_section() {
    let d = tmpdir("reset");
    let s = Scripted::new();
    let doc = d.join("note.md");
    std::fs::write(&doc, "Text\n").unwrap();
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut h = harness_with(AppOptions {
        open: vec![doc],
        config: ConfigDir::Dir(cfg),
        confirm: s.clone() as Arc<dyn Confirm>,
        ..Default::default()
    });
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    let ctx = h.ctx.clone();
    for id in [
        "edit.autoPairMarkdown",
        "edit.smartPunctuation",
        "edit.autoPairBrackets",
    ] {
        h.state_mut().menu_action(&ctx, &MenuAction::Command(id));
    }
    let changed = h.state().preference_values().editing;
    assert_ne!(changed, EditingSettings::default());
    open_at(&mut h, Section::Editor);

    // Cancel keeps everything
    click(&mut h, egui::Role::Button, "Reset to Defaults");
    let q = s.last().expect("asked");
    assert_eq!(q.kind, Kind::ResetSettings);
    assert!(q.message.contains("Editor"));
    assert_eq!(q.default, Choice::Cancel);
    s.answer(Choice::Cancel);
    h.run_steps(2);
    assert_eq!(h.state().preference_values().editing, changed);

    // Reset restores the defaults, in the app and the file
    click(&mut h, egui::Role::Button, "Reset to Defaults");
    s.answer(Choice::Reset);
    h.run_steps(2);
    assert_eq!(h.state().preference_values().editing, EditingSettings::default());
    assert!(toggled(&h, "Auto-pair brackets and quotes"));
    wait_for_file(&mut h, &settings_file(&d), "defaults written", |t| {
        t.contains("\"auto_pair_brackets\": true") && t.contains("\"smart_punctuation\": false")
    });
    assert_eq!(s.asked().len(), 2);

    // Appearance: zoom, toolbar and appearance come back too; the window stays open
    h.state_mut().set_zoom(1.3);
    h.state_mut().set_theme_choice(Appearance::Dark);
    h.state_mut().set_show_toolbar(false);
    open_at(&mut h, Section::Appearance);
    click(&mut h, egui::Role::Button, "Reset to Defaults");
    s.answer(Choice::Reset);
    h.run_steps(3);
    assert!((h.state().zoom() - 1.0).abs() < 1e-4);
    assert!(h.state().show_toolbar());
    assert_eq!(h.state().theme_manager().appearance(), Appearance::System);
    assert!(h.state().is_preferences_open());
    let _ = std::fs::remove_dir_all(&d);
}

/// Tab moves the keyboard focus from the search box through the window's controls, which
/// carry their labels as accessible names.
#[test]
fn tab_moves_between_controls() {
    let mut h = harness_with(AppOptions::default());
    common::run(&mut h);
    open_at(&mut h, Section::Source);
    h.get_by_label("Search settings").focus();
    h.run_steps(2);
    let mut seen = Vec::new();
    for _ in 0..12 {
        h.key_press(Key::Tab);
        h.run_steps(1);
        let focused = h.ctx.memory(|m| m.focused());
        assert!(focused.is_some(), "something has the focus");
        seen.push(focused);
    }
    seen.dedup();
    assert!(seen.len() >= 6, "the focus moved through the controls: {seen:?}");
    // the source-mode check boxes are among them, reachable by name
    let line_numbers = h.get_by_role_and_label(egui::Role::Switch, "Line numbers");
    line_numbers.focus();
    h.run_steps(1);
    assert!(h.get_by_role_and_label(egui::Role::Switch, "Line numbers").is_focused());
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(h.state().source_prefs().line_numbers, "Space toggles the focused box");
    assert!(h.state().is_preferences_open());
}

/// At 150 % zoom the window still fits inside the app window (it scales with the zoom), and
/// it follows the dark appearance.
#[test]
fn survives_150_percent_zoom_and_dark_mode() {
    let d = tmpdir("zoom");
    let mut h = harness(&d);
    h.state_mut().set_zoom(1.5);
    h.state_mut().set_theme_choice(Appearance::Dark);
    wait_until(&mut h, "dark", |a| a.theme().dark);
    h.run_steps(2);
    let screen = h.ctx.content_rect();
    for s in markise_ui_egui::prefs::Section::ALL {
        open_at(&mut h, s);
        let close = h.get_by_label("Close").rect();
        let search = h.get_by_label("Search settings").rect();
        let export = h.get_by_role_and_label(egui::Role::Button, "Export").rect();
        assert!(
            screen.contains_rect(close) && screen.contains_rect(search) && screen.contains_rect(export),
            "{s:?}: {close:?} / {search:?} / {export:?} inside {screen:?}"
        );
    }
    assert!((h.ctx.zoom_factor() - 1.5).abs() < 1e-3);
    assert!(
        markise_ui_egui::dialog::Look::get(&h.ctx).dark,
        "dialog colours follow dark mode"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// The restyled controls: a switch row toggles from anywhere on the row, the segmented
/// control picks one segment, and a theme card applies that theme and writes settings.json.
#[test]
fn switches_segments_and_theme_cards() {
    let d = tmpdir("controls");
    let mut h = harness(&d);
    let settings = settings_file(&d);
    open_at(&mut h, Section::Source);
    // switch: role, state, toggled by a click on the row
    assert!(toggled(&h, "Soft wrap"));
    h.get_by_role_and_label(egui::Role::Switch, "Soft wrap").click();
    h.run_steps(2);
    assert!(!toggled(&h, "Soft wrap"));
    assert!(!h.state().source_prefs().soft_wrap);

    // segmented control: one segment selected; clicking another picks it
    open_at(&mut h, Section::Appearance);
    let selected = |h: &H, l: &str| {
        h.get_by_role_and_label(egui::Role::RadioButton, l)
            .accesskit_node()
            .toggled()
            == Some(Toggled::True)
    };
    assert!(selected(&h, "System") && !selected(&h, "Light") && !selected(&h, "Dark"));
    h.get_by_role_and_label(egui::Role::RadioButton, "Light").click();
    h.run_steps(2);
    assert_eq!(h.state().theme_manager().appearance(), Appearance::Light);
    assert!(selected(&h, "Light") && !selected(&h, "System"));

    // theme cards: Markise is selected; clicking Drake applies it and persists it
    assert!(selected(&h, "Markise"));
    h.get_by_role_and_label(egui::Role::RadioButton, "Drake").click();
    h.run_steps(2);
    wait_until(&mut h, "Drake applied", |a| a.theme().name.starts_with("Drake"));
    assert!(selected(&h, "Drake") && !selected(&h, "Markise"));
    wait_for_file(&mut h, &settings, "theme written", |s| {
        s.contains("\"colors\": \"builtin:drake\"") && s.contains("\"style\": \"theme:builtin:drake\"")
    });
    // the cards show the themes' own colours once imported
    wait_until(&mut h, "previews", |a| {
        let v = a.preference_values();
        v.themes.len() >= 4 && v.themes.iter().all(|t| t.swatch.is_some())
    });

    // a typography override shows ↺, which puts the style's value back
    h.state_mut().set_overrides(markise_theme::Overrides {
        line_height: Some(2.0),
        ..Default::default()
    });
    h.run_steps(2);
    click(&mut h, egui::Role::Button, "Use the style’s line height");
    assert!(h.state().theme_manager().overrides().line_height.is_none());
    assert!(h.query_by_label("Use the style’s line height").is_none());
    let _ = std::fs::remove_dir_all(&d);
}

/// Accessibility state (Codex round 2): a dependent switch is reported disabled while its
/// prerequisite is off, and the Font & size window's override boxes report checked or not
/// (never "mixed").
#[test]
fn accessible_states_of_dependent_switches_and_override_boxes() {
    let mut h = harness_with(AppOptions::default());
    common::run(&mut h);
    open_at(&mut h, Section::Editor);
    let dashes = "Smart dashes: -- as em dash";
    let disabled = |h: &H| {
        h.get_by_role_and_label(egui::Role::Switch, dashes)
            .accesskit_node()
            .is_disabled()
    };
    assert!(!h.state().preference_values().editing.smart_punctuation);
    assert!(disabled(&h), "announced as disabled while smart punctuation is off");
    h.get_by_role_and_label(egui::Role::Switch, dashes).click();
    h.run_steps(2);
    assert!(
        !h.state().preference_values().editing.smart_dashes_em,
        "no input while disabled"
    );
    h.get_by_role_and_label(egui::Role::Switch, "Smart punctuation (convert on input)")
        .click();
    h.run_steps(2);
    assert!(!disabled(&h));

    // Font & size window: the override box's toggled state before and after a click
    h.key_press(Key::Escape);
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut()
        .menu_action(&ctx, &MenuAction::Command("theme.fontAndSize"));
    h.run_steps(2);
    let state = |h: &H| {
        h.get_by_role_and_label(egui::Role::CheckBox, "Override line height")
            .accesskit_node()
            .toggled()
    };
    assert_eq!(state(&h), Some(Toggled::False));
    h.get_by_role_and_label(egui::Role::CheckBox, "Override line height")
        .click();
    h.run_steps(2);
    assert_eq!(state(&h), Some(Toggled::True));
    assert!(h.state().theme_manager().overrides().line_height.is_some());
}

/// Theme previews keep only a few colours per theme: opening Appearance imports the CSS
/// themes for their cards without keeping full imports (Codex round 2, memory budget).
#[test]
fn theme_previews_do_not_keep_full_imports() {
    let d = tmpdir("previews");
    let mut h = harness(&d);
    wait_until(&mut h, "Markise", |a| a.theme().name.starts_with("Markise"));
    let before = h.state().theme_manager().cached_imports();
    open_at(&mut h, Section::Appearance);
    wait_until(&mut h, "previews", |a| {
        let v = a.preference_values();
        v.themes.len() >= 4 && v.themes.iter().all(|t| t.swatch.is_some())
    });
    assert_eq!(h.state().theme_manager().cached_imports(), before);
    // the cards differ: they come from the themes' own colours
    let v = h.state().preference_values();
    let mut pages: Vec<_> = v.themes.iter().map(|t| t.swatch.unwrap().heading).collect();
    pages.dedup();
    assert!(pages.len() >= 2);
    let _ = std::fs::remove_dir_all(&d);
}
