//! Issue #39: egui's built-in keyboard zoom is off, so ⌘= / ⌘- (Ctrl elsewhere) only step the
//! heading level and ⌘0 only makes a paragraph — none of them scales the UI.

mod common;

use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, MarkiseApp};

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

fn scale(h: &Harness<'static, MarkiseApp>) -> (f32, f32) {
    (h.ctx.pixels_per_point(), h.ctx.zoom_factor())
}

#[test]
fn heading_shortcuts_change_the_heading_and_never_zoom() {
    let d = std::env::temp_dir().join(format!("markise-kbzoom-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("t.md");
    std::fs::write(&f, "Title\n").unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut MarkiseApp| app.ui(ui),
            MarkiseApp::new(AppOptions {
                open: vec![f],
                ..Default::default()
            }),
        );
    let t = Instant::now();
    while h.state().model().is_none() || h.state().is_busy() {
        assert!(t.elapsed() < common::HANG_GUARD, "load");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    // caret into the paragraph (focus the surface)
    let p = egui::pos2(600.0, 200.0);
    h.hover_at(p);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.run_steps(3);
    {
        let ed = &mut h.state_mut().viewer().unwrap().ed;
        ed.set_selection(markise_doc::Selection::caret(1));
    }
    h.run_steps(2);
    let before = scale(&h);
    let press = |h: &mut Harness<'static, MarkiseApp>, m: Modifiers, k: Key| {
        h.key_press_modifiers(m, k);
        h.run_steps(2);
    };
    let text = |h: &Harness<'static, MarkiseApp>| h.state().document_text().unwrap();

    // ⌘= : paragraph → h6 (spec 04 Paragraph menu), no zoom
    press(&mut h, cmd(), Key::Equals);
    assert_eq!(text(&h), "###### Title\n");
    assert_eq!(scale(&h), before, "⌘= must not zoom");
    press(&mut h, cmd(), Key::Equals);
    assert_eq!(text(&h), "##### Title\n");
    // ⌘- : back down, no zoom
    press(&mut h, cmd(), Key::Minus);
    assert_eq!(text(&h), "###### Title\n");
    assert_eq!(scale(&h), before, "⌘- must not zoom");
    // ⌘+ (⇧= on a US keyboard arrives as Plus) and ⌘0 never zoom either
    press(&mut h, cmd(), Key::Plus);
    assert_eq!(scale(&h), before, "⌘+ must not zoom");
    press(&mut h, cmd(), Key::Num0);
    assert_eq!(text(&h), "Title\n", "⌘0 makes a paragraph");
    assert_eq!(scale(&h), before, "⌘0 must not reset a zoom");
    let _ = std::fs::remove_dir_all(&d);
}
