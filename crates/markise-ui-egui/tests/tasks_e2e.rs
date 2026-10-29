//! Task-list checkbox clicks in the rendered view (issue #73, spec 02 §9 "Task checkbox:
//! clicking toggles `[ ]`/`[x]`"): the real app, a real pointer press and release at the
//! centre of the painted checkbox (taken from the last frame's layout), under the window
//! layouts the owner uses (toolbar top/bottom, sidebar with the activity bar, 150 % zoom, the
//! Drake theme with a 17.5 font size). Each toggle is one undo step, leaves the caret where it
//! was and saves the exact bytes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use markise_engine::settings::ToolbarPosition;
use markise_layout::Prim;
use markise_theme::Overrides;
use markise_theme::catalog::ThemeId;
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::{AppOptions, MarkiseApp};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-tasks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn open(name: &str, text: &str) -> (H, PathBuf) {
    let dir = tmpdir(name);
    let p = dir.join("t.md");
    std::fs::write(&p, text).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p.clone()],
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_busy());
    (h, p)
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
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

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn caret(h: &mut H) -> (usize, usize) {
    let s = h.state_mut().viewer().unwrap().ed.session.selection().primary();
    (s.anchor.offset, s.focus.offset)
}

/// Screen centre (last frame) of the checkbox painted for the item holding `offset`, from the
/// layout geometry the viewer painted.
fn box_center(h: &mut H, offset: usize) -> egui::Pos2 {
    h.step();
    let v = h.state_mut().viewer().unwrap();
    let r = v.item_screen_rect(offset).expect("item visible");
    let i = v.ed.view.model().item_at_offset(offset);
    let l = v.ed.view.cache_peek(i).expect("item laid out");
    let b = l
        .prims
        .iter()
        .find_map(|p| match p {
            Prim::Checkbox { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("a checkbox is painted for the item");
    r.min + b.center().to_vec2()
}

/// A real press and release at `p` (no movement in between).
fn click_at(h: &mut H, p: egui::Pos2) {
    h.hover_at(p);
    h.step();
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
    h.step();
}

/// Clicks the checkbox of the item holding `offset`.
fn click_box(h: &mut H, offset: usize) {
    let p = box_center(h, offset);
    click_at(h, p);
}

/// Clicks at the end of the first visual line of the item holding `offset` (caret there).
fn click_line_end(h: &mut H, offset: usize) {
    let r = h
        .state_mut()
        .viewer()
        .unwrap()
        .item_screen_rect(offset)
        .expect("item visible");
    click_at(h, egui::pos2(r.right() - 4.0, r.top() + 8.0));
}

fn undo(h: &mut H) {
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
}

fn redo(h: &mut H) {
    h.key_press_modifiers(cmd() | Modifiers::SHIFT, Key::Z);
    h.step();
}

fn save_and_read(h: &mut H, p: &Path) -> String {
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(h, "save", |a| !a.is_busy() && !a.is_dirty());
    std::fs::read_to_string(p).unwrap()
}

/// Toggles the box of the item holding `offset` on and off by clicks, checking the bytes, the
/// caret (unchanged), one undo step per click, undo/redo and the saved file.
fn toggle_round_trip(h: &mut H, p: &Path, offset: usize, before: &str, after: &str) {
    toggle_round_trip_back(h, p, offset, before, after, before);
}

/// [`toggle_round_trip`] where the second click gives `back` (a `[X]` box comes back as `[x]`).
fn toggle_round_trip_back(h: &mut H, p: &Path, offset: usize, before: &str, after: &str, back: &str) {
    assert_eq!(text(h), before);
    let sel = caret(h);
    let undo_len = h.state_mut().viewer().unwrap().ed.session.history().undo_len();
    click_box(h, offset);
    assert_eq!(text(h), after, "a click on the box toggles it");
    assert_eq!(caret(h), sel, "the caret stays where it was");
    assert_eq!(
        h.state_mut().viewer().unwrap().ed.session.history().undo_len(),
        undo_len + 1,
        "one undo step"
    );
    undo(h);
    assert_eq!(text(h), before, "undo restores the exact bytes");
    redo(h);
    assert_eq!(text(h), after, "redo reapplies it");
    assert_eq!(save_and_read(h, p), after, "the saved file");
    // and back
    click_box(h, offset);
    assert_eq!(text(h), back, "a second click toggles it back");
    assert_eq!(save_and_read(h, p), back);
}

#[test]
fn plain_task_list() {
    let doc = "- [ ] milk\n- [x] eggs\n- [X] bread\n";
    let (mut h, p) = open("plain", doc);
    toggle_round_trip(&mut h, &p, 6, doc, "- [x] milk\n- [x] eggs\n- [X] bread\n");
    toggle_round_trip(&mut h, &p, 17, doc, "- [ ] milk\n- [ ] eggs\n- [X] bread\n");
    // `[X]` counts as checked; undo restores the capital X, checking again writes `[x]`
    toggle_round_trip_back(
        &mut h,
        &p,
        28,
        doc,
        "- [ ] milk\n- [x] eggs\n- [ ] bread\n",
        "- [ ] milk\n- [x] eggs\n- [x] bread\n",
    );
}

#[test]
fn nested_task_item() {
    let doc = "- [ ] outer\n  - [ ] inner\n  - [x] done\n";
    let (mut h, p) = open("nested", doc);
    toggle_round_trip(&mut h, &p, 20, doc, "- [ ] outer\n  - [x] inner\n  - [x] done\n");
    toggle_round_trip(&mut h, &p, 6, doc, "- [x] outer\n  - [ ] inner\n  - [x] done\n");
}

#[test]
fn task_inside_a_quote() {
    let doc = "> - [ ] quoted\n> - [x] again\n";
    let (mut h, p) = open("quote", doc);
    toggle_round_trip(&mut h, &p, 8, doc, "> - [x] quoted\n> - [x] again\n");
}

#[test]
fn caret_on_the_line_reveal_mode() {
    let doc = "intro\n\n- [ ] milk *em*\n- [ ] eggs\n";
    let (mut h, p) = open("reveal", doc);
    click_line_end(&mut h, 13);
    let (a, f) = caret(&mut h);
    assert_eq!(a, f);
    assert!((13..=22).contains(&f), "caret on the task line: {f}");
    toggle_round_trip(&mut h, &p, 13, doc, "intro\n\n- [x] milk *em*\n- [ ] eggs\n");
}

#[test]
fn toolbar_at_the_bottom_and_at_the_top() {
    let doc = "# T\n\n- [ ] milk\n";
    let (mut h, p) = open("toolbar", doc);
    h.state_mut().set_show_toolbar(true);
    for pos in [ToolbarPosition::Bottom, ToolbarPosition::Top] {
        h.state_mut().set_toolbar_position(pos);
        h.run_steps(3);
        toggle_round_trip(&mut h, &p, 11, doc, "# T\n\n- [x] milk\n");
    }
}

#[test]
fn sidebar_with_the_activity_bar_shown_and_hidden() {
    let doc = "- [ ] milk\n";
    let (mut h, p) = open("sidebar", doc);
    for files in [true, false] {
        h.state_mut().show_files_tab(files);
        h.run_steps(3);
        toggle_round_trip(&mut h, &p, 6, doc, "- [x] milk\n");
    }
    let ctx = h.ctx.clone();
    assert!(
        h.state_mut()
            .menu_action(&ctx, &MenuAction::Command("view.toggleSidebar"))
    );
    h.run_steps(3);
    toggle_round_trip(&mut h, &p, 6, doc, "- [x] milk\n");
}

#[test]
fn window_zoom_150() {
    let doc = "para\n\n- [ ] milk\n  - [ ] nested\n";
    let (mut h, p) = open("zoom", doc);
    h.state_mut().set_zoom(1.5);
    h.run_steps(4);
    toggle_round_trip(&mut h, &p, 12, doc, "para\n\n- [x] milk\n  - [ ] nested\n");
    toggle_round_trip(&mut h, &p, 25, doc, "para\n\n- [ ] milk\n  - [x] nested\n");
}

#[test]
fn owner_settings_drake_theme_font_17_5() {
    let doc = "# Todo\n\n- [ ] milk\n  - [x] nested\n> - [ ] quoted\n";
    let (mut h, p) = open("drake", doc);
    h.state_mut().select_theme(ThemeId::Builtin("drake".into()));
    wait_until(&mut h, "Drake", |a| a.theme().name.starts_with("Drake"));
    h.state_mut().set_overrides(Overrides {
        font_size: Some(17.5),
        ..Default::default()
    });
    h.run_steps(3);
    assert_eq!(h.state().theme().font_size, 17.5);
    toggle_round_trip(
        &mut h,
        &p,
        14,
        doc,
        "# Todo\n\n- [x] milk\n  - [x] nested\n> - [ ] quoted\n",
    );
    toggle_round_trip(
        &mut h,
        &p,
        27,
        doc,
        "# Todo\n\n- [ ] milk\n  - [ ] nested\n> - [ ] quoted\n",
    );
    toggle_round_trip(
        &mut h,
        &p,
        44,
        doc,
        "# Todo\n\n- [ ] milk\n  - [x] nested\n> - [x] quoted\n",
    );
}
