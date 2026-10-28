//! Whole-window zoom (issue #43): View → Zoom In / Zoom Out / Actual Size on ⇧⌘= / ⇧⌘- / ⇧⌘0
//! (Ctrl+Shift elsewhere) set egui's zoom factor in 10 % steps within 50–300 %, persisted;
//! ⌘= / ⌘- stay the heading level commands; the document's caret and hit testing stay right
//! when zoomed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-zoom-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(open: Vec<PathBuf>, config: Option<&Path>) -> H {
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open,
                config: config.map_or(ConfigDir::None, |c| ConfigDir::Dir(c.to_path_buf())),
                ..Default::default()
            }),
        )
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

fn loaded(a: &MarkiseApp) -> bool {
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

fn shift_cmd() -> Modifiers {
    Modifiers { shift: true, ..cmd() }
}

fn press(h: &mut H, m: Modifiers, k: Key) {
    h.key_press_modifiers(m, k);
    h.run_steps(2);
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn zoom_is(h: &H, z: f32) {
    assert!(close(h.state().zoom(), z), "zoom {} != {z}", h.state().zoom());
    assert!(
        close(h.ctx.zoom_factor(), z),
        "egui zoom factor {} != {z}",
        h.ctx.zoom_factor()
    );
}

#[test]
fn shortcuts_and_menu_items_zoom_the_window_in_steps_within_bounds() {
    let d = tmpdir("keys");
    let f = d.join("t.md");
    std::fs::write(&f, "Title\n").unwrap();
    let mut h = harness(vec![f], None);
    wait_until(&mut h, "load", loaded);
    zoom_is(&h, 1.0);

    // ⇧⌘= (and ⇧= arriving as "+") zoom in, ⇧⌘- out, ⇧⌘0 back to 100 %
    press(&mut h, shift_cmd(), Key::Equals);
    zoom_is(&h, 1.1);
    press(&mut h, shift_cmd(), Key::Plus);
    zoom_is(&h, 1.2);
    assert!(
        h.query_by_label("Zoom 120 %").is_some(),
        "the status bar shows the zoom"
    );
    press(&mut h, shift_cmd(), Key::Minus);
    zoom_is(&h, 1.1);
    press(&mut h, shift_cmd(), Key::Num0);
    zoom_is(&h, 1.0);

    // the View menu items do the same
    let ctx = h.ctx.clone();
    for _ in 0..3 {
        assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("view.zoomIn")));
    }
    h.run_steps(2);
    zoom_is(&h, 1.3);
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("view.zoomOut")));
    h.run_steps(2);
    zoom_is(&h, 1.2);
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("view.actualSize")));
    h.run_steps(2);
    zoom_is(&h, 1.0);
    let bar = h.state().menu_model();
    let view = bar.menu("View").expect("View menu");
    for key in ["view.zoomIn", "view.zoomOut", "view.actualSize"] {
        assert!(view.items().iter().any(|i| i.key == key), "View → {key}");
    }

    // bounds: 50–300 %
    h.state_mut().set_zoom(2.95);
    for _ in 0..3 {
        press(&mut h, shift_cmd(), Key::Equals);
    }
    zoom_is(&h, 3.0);
    h.state_mut().set_zoom(0.55);
    for _ in 0..3 {
        press(&mut h, shift_cmd(), Key::Minus);
    }
    zoom_is(&h, 0.5);
    h.state_mut().set_zoom(1.0);
    h.run_steps(2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn heading_shortcuts_stay_heading_level_and_do_not_zoom() {
    let d = tmpdir("headings");
    let f = d.join("t.md");
    std::fs::write(&f, "Title\n").unwrap();
    let mut h = harness(vec![f], None);
    wait_until(&mut h, "load", loaded);
    h.state_mut().set_zoom(1.5);
    h.run_steps(2);
    // caret into the paragraph
    let p = egui::pos2(400.0, 140.0);
    h.hover_at(p);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.run_steps(2);
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_selection(markise_doc::Selection::caret(1));
    h.run_steps(2);
    press(&mut h, cmd(), Key::Equals);
    assert_eq!(h.state().document_text().as_deref(), Some("###### Title\n"));
    zoom_is(&h, 1.5);
    press(&mut h, cmd(), Key::Minus);
    assert_eq!(h.state().document_text().as_deref(), Some("Title\n"));
    zoom_is(&h, 1.5);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_zoom_persists_across_a_restart() {
    let d = tmpdir("persist");
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    {
        let mut h = harness(vec![], Some(&cfg));
        h.run_steps(3);
        h.state_mut().set_zoom(1.5);
        h.run_steps(2);
        h.state_mut().on_exit(); // writes the settings now
    }
    let file = cfg.join("markise").join("settings.json");
    let t = Instant::now();
    while !std::fs::read_to_string(&file).is_ok_and(|s| s.contains("\"zoom\": 1.5")) {
        assert!(t.elapsed() < Duration::from_secs(10), "settings written with the zoom");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut h = harness(vec![], Some(&cfg));
    wait_until(&mut h, "zoom restored", |a| close(a.zoom(), 1.5));
    h.run_steps(2);
    zoom_is(&h, 1.5);
    // Actual Size leaves no `zoom` key behind
    let ctx = h.ctx.clone();
    assert!(h.state_mut().menu_action(&ctx, &MenuAction::Command("view.actualSize")));
    h.run_steps(2);
    h.state_mut().on_exit();
    let t = Instant::now();
    while std::fs::read_to_string(&file).is_ok_and(|s| s.contains("\"zoom\"")) {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "settings written without the zoom"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(std::fs::read_to_string(&file).unwrap().contains("\"version\""));
    let _ = std::fs::remove_dir_all(&d);
}

/// The caret rectangle the app reports to the platform (IME cursor area) this frame.
fn caret_screen(h: &mut H) -> egui::Rect {
    h.step();
    h.output().platform_output.ime.expect("a caret is shown").cursor_rect
}

fn caret(h: &mut H) -> usize {
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .session
        .selection()
        .primary()
        .focus
        .offset
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
    h.run_steps(2);
}

#[test]
fn caret_and_hit_testing_are_right_at_150_percent() {
    let d = tmpdir("hit");
    let f = d.join("t.md");
    let text = "# Harbour\n\nThe lamp was trimmed at dusk and the log written at dawn.\n";
    std::fs::write(&f, text).unwrap();
    let mut h = harness(vec![f], None);
    wait_until(&mut h, "load", loaded);
    h.state_mut().set_zoom(1.5);
    h.run_steps(3);
    zoom_is(&h, 1.5);
    assert!(close(h.ctx.pixels_per_point(), 1.5), "ppp {}", h.ctx.pixels_per_point());
    // focus the document
    let para = text.find("lamp").unwrap();
    let r = h.state().viewer_item_rect(para).expect("paragraph shown");
    click(&mut h, r.center());
    // for several offsets: the caret shown there, clicked, puts the caret back there
    for word in ["lamp", "trimmed", "log", "dawn"] {
        let at = text.find(word).unwrap() + 1;
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .set_selection(markise_doc::Selection::caret(at));
        h.run_steps(2);
        let c = caret_screen(&mut h);
        let surface = h.state().viewer_item_rect(para).unwrap();
        assert!(
            surface.contains(c.center()),
            "caret {c:?} inside its paragraph {surface:?}"
        );
        // move the caret away, then click where it was drawn
        h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .set_selection(markise_doc::Selection::caret(0));
        h.run_steps(2);
        click(&mut h, c.center());
        assert_eq!(caret(&mut h), at, "click on the caret drawn at {word:?}");
    }
    // typing lands where the caret is
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_selection(markise_doc::Selection::caret(text.find("lamp").unwrap()));
    h.run_steps(2);
    h.event(egui::Event::Text("old ".into()));
    h.run_steps(2);
    assert!(h.state().document_text().unwrap().contains("The old lamp"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_first_rendered_frame_has_the_saved_zoom() {
    // Codex review of PR #47: no frame at 100 % before the restored zoom applies
    let d = tmpdir("first-frame");
    let cfg = d.join("config");
    std::fs::create_dir_all(cfg.join("markise")).unwrap();
    std::fs::write(
        cfg.join("markise").join("settings.json"),
        r#"{"version": 2, "zoom": 1.5}"#,
    )
    .unwrap();
    let f = d.join("t.md");
    std::fs::write(&f, "# Title\n\nText\n").unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u64, f32)>::new()));
    let rec = seen.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(
            move |ui, app: &mut MarkiseApp| {
                app.ui(ui);
                rec.lock().unwrap().push((app.frames, ui.ctx().pixels_per_point()));
            },
            MarkiseApp::new(AppOptions {
                open: vec![f],
                config: ConfigDir::Dir(cfg.clone()),
                ..Default::default()
            }),
        );
    wait_until(&mut h, "load", loaded);
    let seen = seen.lock().unwrap();
    // every pass that laid out the UI (frames > 0) ran at the saved scale
    let content: Vec<_> = seen.iter().filter(|(f, _)| *f > 0).collect();
    assert!(!content.is_empty());
    for (f, ppp) in content {
        assert!(close(*ppp, 1.5), "pass {f} ran at pixels_per_point {ppp}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
