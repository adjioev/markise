//! Theme UI (ADR 0004): selecting a built-in theme and appearance, persistence in the settings
//! file, "Import theme…" and live reload of a user theme — through the real `MarkiseApp`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_theme::Overrides;
use markise_theme::Rgba;
use markise_theme::catalog::{Selection, StyleId, ThemeId};
use markise_ui_egui::{AppOptions, Appearance, ConfigDir, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-themes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(config: &Path) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        config: ConfigDir::Dir(config.to_path_buf()),
        ..Default::default()
    });
    Harness::builder()
        .with_size(egui::vec2(900.0, 600.0))
        // the menu's ⌘/⇧ shortcut hints are not in egui's bundled fonts: tofu, as in the app
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what} (theme: {}, banner: {:?})",
            h.state().theme().name,
            h.state().banner_text()
        );
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Opens View → Theme in the egui menu bar.
fn open_view_theme_menu(h: &mut Harness<'static, MarkiseApp>) {
    h.get_by_label("View").click();
    h.run_steps(2);
    h.get_by_label("Theme").click();
    h.run_steps(2);
}

#[test]
fn builtin_theme_and_appearance_are_selected_and_persisted() {
    let cfg = tmpdir("persist");
    {
        let mut h = harness(&cfg);
        h.state_mut().set_theme_choice(Appearance::Light);
        h.step();
        h.step();
        assert_eq!(h.state().theme().name, "Markise Light");
        // View → Theme lists the complete themes; clicking one selects it (issue #50)
        open_view_theme_menu(&mut h);
        h.get_by_label("Phycat");
        h.get_by_label("Drake").click();
        wait_until(&mut h, "Drake Light", |a| a.theme().name == "Drake Light");
        // palette themes are native: selected immediately, light/dark pairs follow appearance
        h.state_mut().select_theme(ThemeId::Palette("catppuccin".into()));
        wait_until(&mut h, "Catppuccin Latte", |a| a.theme().name == "Catppuccin Latte");
        h.state_mut().set_theme_choice(Appearance::Dark);
        wait_until(&mut h, "Catppuccin Mocha", |a| a.theme().name == "Catppuccin Mocha");
        h.state_mut().set_theme_choice(Appearance::Light);
        h.state_mut().select_theme(ThemeId::Palette("nord".into()));
        wait_until(&mut h, "Nord (dark only)", |a| a.theme().name == "Nord");
        h.state_mut().set_theme_choice(Appearance::Dark);
        h.state_mut().select_theme(ThemeId::Builtin("phycat".into()));
        wait_until(&mut h, "Phycat Abyss", |a| a.theme().name == "Phycat Abyss");
        assert!(h.state().theme().dark);
        assert_eq!(h.state().theme().page_bg, Rgba::hex(0x0f111a));
        h.state_mut().set_theme_choice(Appearance::Light);
        wait_until(&mut h, "Phycat Sky", |a| a.theme().name == "Phycat Sky");
        let settings = cfg.join("markise").join("settings.json");
        wait_until(&mut h, "settings written", |_| {
            std::fs::read_to_string(&settings).is_ok_and(|s| s.contains("builtin:phycat") && s.contains("\"light\""))
        });
    }
    // a new session starts with the saved theme on its first frame (no default flash)
    let mut h = harness(&cfg);
    h.step();
    assert_eq!(h.state().theme().name, "Phycat Sky");
    let _ = std::fs::remove_dir_all(&cfg);
}

#[test]
fn import_theme_copies_selects_and_live_reloads() {
    let cfg = tmpdir("import");
    let src = tmpdir("import-src");
    std::fs::create_dir_all(src.join("harbour")).unwrap();
    std::fs::write(src.join("harbour").join("bg.png"), [0u8; 4]).unwrap();
    std::fs::write(
        src.join("harbour.css"),
        "body { background: #f3efe2; color: #222 } #write a { color: #8a2be2 }",
    )
    .unwrap();
    let mut h = harness(&cfg);
    h.step();
    h.state_mut().import_theme(src.join("harbour.css"));
    wait_until(&mut h, "imported theme selected", |a| a.theme().name == "Harbour");
    let user = cfg.join("markise").join("themes");
    assert!(user.join("harbour.css").is_file(), "stub");
    let pkg = user.join("harbour.markise-theme");
    assert!(
        pkg.join("harbour").join("bg.png").is_file(),
        "asset folder copied into the package"
    );
    assert_eq!(h.state().theme().link, Rgba::hex(0x8a2be2));
    assert_eq!(
        h.state().theme_manager().selection(),
        &Selection::complete(ThemeId::User("harbour".into()))
    );

    // live reload: editing the installed file re-imports and re-renders
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(
        pkg.join("harbour.css"),
        "body { background: #102030; color: #ddd } #write a { color: #8a2be2 }",
    )
    .unwrap();
    wait_until(&mut h, "reloaded theme", |a| a.theme().page_bg == Rgba::hex(0x102030));
    assert!(h.state().theme().dark);

    // a user layer that did not exist before also reloads the theme
    std::fs::write(user.join("harbour.user.css"), "#write a { color: #00aa00 }").unwrap();
    wait_until(&mut h, "user layer applied", |a| a.theme().link == Rgba::hex(0x00aa00));

    // importing it again asks before replacing; Replace swaps the package
    std::fs::write(
        src.join("harbour.css"),
        "body { background: #fafafa } #write a { color: #0000cc }",
    )
    .unwrap();
    h.state_mut().import_theme(src.join("harbour.css"));
    wait_until(&mut h, "replace question", |a| a.is_asking());
    h.run_steps(2); // the modal takes a frame to become visible (egui 0.37)
    h.get_by_label("Replace"); // the question lists the action
    h.state_mut().confirm_replace_theme();
    wait_until(&mut h, "replaced theme", |a| a.theme().page_bg == Rgba::hex(0xfafafa));
    let _ = std::fs::remove_dir_all(&cfg);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn fallback_fonts_are_requested_only_for_documents_that_need_them() {
    let dir = tmpdir("fallback");
    let latin = dir.join("latin.md");
    let cjk = dir.join("cjk.md");
    std::fs::write(&latin, "# Hello\n\nPlain text only.\n").unwrap();
    std::fs::write(&cjk, "# 你好\n\nText with 中文.\n").unwrap();
    for (doc, wants) in [(latin, false), (cjk, true)] {
        let app = MarkiseApp::new(AppOptions {
            open: vec![doc],
            ..Default::default()
        });
        let mut h = Harness::builder()
            .with_size(egui::vec2(900.0, 600.0))
            // the menu's ⌘/⇧ shortcut hints are not in egui's bundled fonts: tofu, as in the app
            .allow_missing_glyphs()
            .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
        wait_until(&mut h, "document", |a| a.model().is_some() && !a.is_loading());
        h.run_steps(3);
        assert_eq!(h.state().theme_manager().fallback_requested(), wants);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR 0007: style and colors are picked independently (also from complete themes), user
/// overrides apply on top, everything persists, and a version-1 settings file migrates.
#[test]
fn style_colors_and_overrides_are_independent_and_persisted() {
    let cfg = tmpdir("split");
    let settings = cfg.join("markise").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"version": 1, "theme": "builtin:drake", "appearance": "light", "future": 1}"#,
    )
    .unwrap();
    {
        let mut h = harness(&cfg);
        wait_until(&mut h, "migrated complete theme", |a| a.theme().name == "Drake Light");
        let drake_bg = h.state().theme().page_bg;
        // Drake's colors with the Classic style
        h.state_mut().select_style(StyleId::Builtin("classic".into()));
        wait_until(&mut h, "Drake colors, Classic style", |a| {
            a.theme().name == "Drake Light · Classic"
        });
        assert_eq!(h.state().theme().page_bg, drake_bg);
        assert_eq!(
            h.state().theme().fonts.body.first().map(String::as_str),
            Some("Iowan Old Style")
        );
        // Drake's style with Nord colors
        h.state_mut()
            .select_style(StyleId::Theme(ThemeId::Builtin("drake".into())));
        h.state_mut().select_colors(ThemeId::Palette("nord".into()));
        wait_until(&mut h, "Nord colors, Drake style", |a| a.theme().name == "Nord · Drake");
        assert_eq!(h.state().theme().page_bg, Rgba::hex(0x2e3440));
        assert_eq!(
            h.state().theme().fonts.body.first().map(String::as_str),
            Some("Iosevka Curly")
        );
        // overrides
        h.state_mut().set_overrides(Overrides {
            line_height: Some(2.0),
            content_width: Some(900.0),
            ..Default::default()
        });
        wait_until(&mut h, "overrides applied", |a| {
            a.theme().line_height == 2.0 && a.theme().column_max_width == 900.0
        });
        // Preferences → Appearance offers both parts, complete themes, light/dark and the
        // overrides (issue #50: they left the Themes menu); View → Theme → Theme Settings…
        open_view_theme_menu(&mut h);
        h.get_by_label("Theme Settings…").click();
        h.run_steps(2);
        assert!(h.state().is_preferences_open());
        h.get_by_label("Style");
        h.get_by_label("Colors");
        h.get_by_label("Complete theme");
        h.get_by_label("Follow system");
        h.get_by_role_and_label(egui::Role::Slider, "Line height");
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(!h.state().is_preferences_open());
        // the Font & size window itself still opens by its command
        let ctx = h.ctx.clone();
        h.state_mut()
            .menu_action(&ctx, &markise_ui_egui::menu::MenuAction::Command("theme.fontAndSize"));
        h.run_steps(2);
        assert!(h.query_all_by_label("Font & size").count() >= 1);
        h.get_by_role_and_label(egui::Role::Slider, "Line height");
        wait_until(&mut h, "settings written", |_| {
            std::fs::read_to_string(&settings).is_ok_and(|s| {
                s.contains("\"version\": 2")
                    && s.contains("\"style\": \"theme:builtin:drake\"")
                    && s.contains("\"colors\": \"palette:nord\"")
                    && s.contains("\"line_height\": 2.0")
                    && s.contains("\"future\": 1")
                    && s.contains("\"theme\": \"palette:nord\"") // for older builds
            })
        });
    }
    // a new session restores the composition
    let mut h = harness(&cfg);
    h.step();
    assert_eq!(h.state().theme().name, "Nord · Drake");
    assert_eq!(h.state().theme().line_height, 2.0);
    let _ = std::fs::remove_dir_all(&cfg);
}

/// A complete theme picked in the menu marks both parts as chosen, even when it equals the
/// current selection (so a late settings file cannot replace the explicit choice).
#[test]
fn complete_theme_menu_click_marks_style_and_colors_chosen() {
    let cfg = tmpdir("complete-menu");
    let mut h = harness(&cfg);
    h.step();
    assert_eq!(h.state().theme_manager().chosen_parts(), (false, false));
    open_view_theme_menu(&mut h);
    h.get_by_label("Markise").click(); // the complete theme, already the current selection
    h.run_steps(2);
    assert_eq!(h.state().theme_manager().chosen_parts(), (true, true));
    assert_eq!(
        h.state().theme_manager().selection(),
        &Selection::complete(ThemeId::Markise)
    );
    let _ = std::fs::remove_dir_all(&cfg);
}
