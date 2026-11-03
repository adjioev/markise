//! The caret between two adjacent blocks on the real app (issue #166): a click in the gap
//! between two tables (or rules) puts the caret on a virtual empty line there, ↓ / ↑ reach it
//! too, typing writes a paragraph between the blocks into the file, leaving without typing
//! leaves the file byte-identical; also in Side by Side.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use markise_doc::DocPos;
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode};

const TABLES: &str = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n| c | d |\n|---|---|\n| 3 | 4 |\n\nAfter\n";

fn open(name: &str, src: &str) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let d = std::env::temp_dir().join(format!("markise-gaps-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("t.md");
    std::fs::write(&p, src).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p.clone()],
        ..common::no_watch()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    let t = Instant::now();
    while h.state().model().is_none() || h.state().is_loading() {
        assert!(t.elapsed() < common::HANG_GUARD, "timed out waiting for the load");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    (h, p)
}

fn text(h: &Harness<'static, MarkiseApp>) -> String {
    h.state().document_text().unwrap()
}

fn caret(h: &mut Harness<'static, MarkiseApp>) -> DocPos {
    let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
    assert!(sel.is_caret(), "a caret: {sel:?}");
    sel.focus
}

fn typ(h: &mut Harness<'static, MarkiseApp>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
    }
    h.run_steps(1);
}

fn key(h: &mut Harness<'static, MarkiseApp>, k: Key) {
    h.key_press_modifiers(Modifiers::NONE, k);
    h.run_steps(2);
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

fn click(h: &mut Harness<'static, MarkiseApp>, p: egui::Pos2) {
    h.hover_at(p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run_steps(2);
}

/// A point in the gap above the block starting at `offset` (the space between it and the
/// block before it), at `x`.
fn gap_above(h: &mut Harness<'static, MarkiseApp>, offset: usize, x: f32) -> egui::Pos2 {
    let r = h
        .state_mut()
        .viewer()
        .unwrap()
        .item_screen_rect(offset)
        .expect("the block is shown");
    egui::pos2(x, r.min.y + 2.0)
}

fn save(h: &mut Harness<'static, MarkiseApp>) {
    h.state_mut().save();
    common::wait_until(h, "save", |a| !a.is_saving() && !a.is_dirty());
}

/// Owner report (issue #166): a click between two adjacent tables puts the caret on an empty
/// line between them (nothing written); typing writes a paragraph there, saved to the file
/// with blank lines around it; undo restores the file and the caret.
#[test]
fn click_between_two_tables_and_type_issue_166() {
    let (mut h, path) = open("click", TABLES);
    let second = TABLES.find("| c |").unwrap();
    let gap = DocPos::with_virt(TABLES.find("2 |").unwrap() + 3, 1);
    let p = gap_above(&mut h, second, 300.0);
    click(&mut h, p);
    assert_eq!(caret(&mut h), gap, "the caret in the gap");
    assert_eq!(text(&h), TABLES, "nothing written");
    assert!(!h.state().is_dirty(), "a click does not dirty the document");
    typ(&mut h, "Between");
    let want = TABLES.replace("| 1 | 2 |\n\n", "| 1 | 2 |\n\nBetween\n\n");
    assert_eq!(text(&h), want);
    save(&mut h);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), want, "the file");
    // undo (the first character materialised the paragraph: its own step)
    h.key_press_modifiers(cmd(), Key::Z);
    h.key_press_modifiers(cmd(), Key::Z);
    h.run_steps(2);
    assert_eq!(text(&h), TABLES, "undo");
    assert_eq!(caret(&mut h), gap, "undo restores the virtual caret");
}

/// ↓ from the first table's last row and ↑ from the second's first row reach the gap;
/// leaving it with the arrows (or a click elsewhere) leaves the file byte-identical.
#[test]
fn arrows_reach_the_gap_and_leaving_writes_nothing_issue_166() {
    let (mut h, path) = open("arrows", TABLES);
    let gap = DocPos::with_virt(TABLES.find("2 |").unwrap() + 3, 1);
    // focus the document, then the caret in the first table's last row
    click(&mut h, egui::pos2(600.0, 300.0));
    let at = TABLES.find("1 |").unwrap();
    h.state_mut().viewer().unwrap().ed.set_caret(DocPos::new(at), false);
    h.run_steps(2);
    key(&mut h, Key::ArrowDown);
    assert_eq!(caret(&mut h), gap, "Down into the gap");
    key(&mut h, Key::ArrowDown);
    let c = caret(&mut h);
    assert_eq!(c.virt, 0, "Down into the second table");
    assert!(
        c.offset > gap.offset && c.offset < TABLES.find("After").unwrap(),
        "{c:?}"
    );
    key(&mut h, Key::ArrowUp);
    assert_eq!(caret(&mut h), gap, "Up from the second table's header");
    key(&mut h, Key::ArrowUp);
    assert_eq!(caret(&mut h).virt, 0, "Up into the first table");
    // through the gap and away: nothing written
    key(&mut h, Key::ArrowDown);
    assert_eq!(caret(&mut h), gap);
    let after = TABLES.find("After").unwrap();
    let p = h
        .state_mut()
        .viewer()
        .unwrap()
        .item_screen_rect(after)
        .unwrap()
        .center();
    click(&mut h, p);
    assert_eq!(caret(&mut h).virt, 0);
    assert_eq!(text(&h), TABLES);
    assert!(!h.state().is_dirty());
    h.state_mut().save();
    h.run_steps(3);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), TABLES, "byte-identical");
}

/// Two rules (no caret stops of their own) between tables: the gaps around them are reached
/// by clicks; typing in the middle one keeps both rules rules.
#[test]
fn clicks_between_rules_issue_166() {
    let src = "| a |\n|---|\n| 1 |\n\n***\n\n***\n\n| b |\n|---|\n| 2 |\n";
    let (mut h, _path) = open("rules", src);
    let second = src.rfind("***").unwrap();
    let p = gap_above(&mut h, second, 300.0);
    click(&mut h, p);
    let first = src.find("***").unwrap();
    assert_eq!(caret(&mut h), DocPos::with_virt(first + 3, 1), "between the rules");
    typ(&mut h, "x");
    assert_eq!(
        text(&h),
        "| a |\n|---|\n| 1 |\n\n***\n\nx\n\n***\n\n| b |\n|---|\n| 2 |\n"
    );
}

/// Side by Side (the rendered pane takes the caret and commands, typing goes to the source
/// pane): a click in the gap of the rendered pane puts the caret there and the arrows move
/// through it without changing either pane; back in the rendered view, typing in the gap
/// writes the paragraph, which Side by Side then shows in both panes.
#[test]
fn gap_in_side_by_side_issue_166() {
    let (mut h, _path) = open("sbs", TABLES);
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    let second = TABLES.find("| c |").unwrap();
    let gap = DocPos::with_virt(TABLES.find("2 |").unwrap() + 3, 1);
    let r = h.state_mut().viewer().unwrap().item_screen_rect(second).unwrap();
    click(&mut h, egui::pos2(r.min.x + 40.0, r.min.y + 2.0));
    assert_eq!(caret(&mut h), gap, "the caret in the gap");
    key(&mut h, Key::ArrowDown);
    assert_eq!(caret(&mut h).virt, 0, "Down into the second table");
    key(&mut h, Key::ArrowUp);
    assert_eq!(caret(&mut h), gap, "Up back into the gap");
    h.run_steps(3);
    assert_eq!(caret(&mut h), gap, "the caret stays in the gap over the next frames");
    assert_eq!(h.state().source_buffer(), Some(TABLES), "the source pane unchanged");
    assert_eq!(text(&h), TABLES);
    assert!(!h.state().is_dirty());
    // the rendered view: typing in the gap, then both panes of Side by Side show it
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(3);
    let r = h.state_mut().viewer().unwrap().item_screen_rect(second).unwrap();
    click(&mut h, egui::pos2(r.min.x + 40.0, r.min.y + 2.0));
    assert_eq!(caret(&mut h), gap);
    typ(&mut h, "s");
    let want = TABLES.replace("| 1 | 2 |\n\n", "| 1 | 2 |\n\ns\n\n");
    assert_eq!(text(&h), want);
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    assert_eq!(h.state().source_buffer(), Some(want.as_str()), "both panes");
}
