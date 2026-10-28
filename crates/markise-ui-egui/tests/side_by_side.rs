//! Side by Side view mode (spec 04 §3.3, interim delivery 2026-10-27): mode transitions,
//! divider persistence, edit → preview updates and scroll sync, on the real `MarkiseApp`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, pos2, vec2};
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-sbs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn harness(open: Vec<PathBuf>, config: ConfigDir) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        open,
        config,
        ..Default::default()
    });
    Harness::builder()
        .with_size(vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app)
}

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn loaded(a: &MarkiseApp) -> bool {
    a.model().is_some() && !a.is_loading()
}

fn long_doc() -> String {
    let mut s = String::new();
    for i in 0..300 {
        s.push_str(&format!(
            "## Heading {i}\n\nParagraph {i} with some text.\nSecond line {i}.\n\n"
        ));
    }
    s
}

fn open(name: &str, text: &str) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let dir = tmpdir(name);
    let f = dir.join("doc.md");
    std::fs::write(&f, text).unwrap();
    let mut h = harness(vec![f], ConfigDir::None);
    wait_until(&mut h, "load", loaded);
    (h, dir)
}

fn sbs_mods() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::CTRL
    } else {
        Modifiers::CTRL | Modifiers::ALT
    }
}

fn source_mods() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

fn press(h: &mut Harness<'static, MarkiseApp>, m: Modifiers) {
    let m = if m.mac_cmd || m.ctrl && !cfg!(target_os = "macos") {
        Modifiers { command: true, ..m }
    } else {
        m
    };
    h.key_press_modifiers(m, Key::Slash);
    h.run_steps(2);
}

#[test]
fn view_mode_transitions_follow_spec_04_3_3() {
    let (mut h, _d) = open("modes", "# A\n\ntext\n");
    assert_eq!(h.state().view_mode(), ViewMode::Rendered);
    // ⌃⌘/ from Rendered → Side by Side; again → back to Rendered
    press(&mut h, sbs_mods());
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
    press(&mut h, sbs_mods());
    assert_eq!(h.state().view_mode(), ViewMode::Rendered);
    // ⌘/ → Source; ⌃⌘/ → Side by Side remembering Source; again → Source
    press(&mut h, source_mods());
    assert_eq!(h.state().view_mode(), ViewMode::Source);
    press(&mut h, sbs_mods());
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
    press(&mut h, sbs_mods());
    assert_eq!(h.state().view_mode(), ViewMode::Source);
    // ⌘/ from Side by Side → Source (full width)
    press(&mut h, sbs_mods());
    press(&mut h, source_mods());
    assert_eq!(h.state().view_mode(), ViewMode::Source);
    press(&mut h, source_mods());
    assert_eq!(h.state().view_mode(), ViewMode::Rendered);
    // the View menu sets a mode directly; Rendered/Source become the remembered mode
    h.state_mut().set_view_mode(ViewMode::Source);
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    press(&mut h, sbs_mods());
    assert_eq!(h.state().view_mode(), ViewMode::Rendered, "remembered: Rendered (menu)");
}

#[test]
fn view_menu_offers_three_radio_items() {
    use egui_kittest::kittest::Queryable;
    let (mut h, _d) = open("menu", "# A\n");
    h.get_by_label("View").click();
    h.run_steps(2);
    h.get_by_label("Rendered");
    h.get_by_label_contains("Source Code Mode");
    h.get_by_label_contains("Side by Side").click();
    h.run_steps(2);
    assert_eq!(h.state().view_mode(), ViewMode::SideBySide);
}

#[test]
fn edits_on_the_left_update_the_preview_without_a_rebuild() {
    let (mut h, _d) = open("edit", &long_doc());
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    let items = h.state().model().unwrap().items.len();
    h.state_mut()
        .edit_source_text(|t| *t = t.replacen("Paragraph 3 with", "Paragraph 3 now with", 1));
    h.step();
    // applied synchronously in the same frame: no background rebuild
    assert!(!h.state().is_busy(), "no rebuild job");
    assert!(h.state().viewed_text().unwrap().contains("Paragraph 3 now with"));
    assert!(h.state().is_dirty());
    // a structural edit: a new heading adds items
    h.state_mut().edit_source_text(|t| t.insert_str(0, "# New\n\n"));
    h.step();
    assert_eq!(h.state().model().unwrap().items.len(), items + 1);
    assert!(h.state().viewed_text().unwrap().starts_with("# New\n\n"));
    // leaving the mode keeps the text
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(2);
    assert!(h.state().viewed_text().unwrap().starts_with("# New\n\n"));
}

fn wheel(h: &mut Harness<'static, MarkiseApp>, at: egui::Pos2, dy: f32) {
    h.hover_at(at);
    h.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(4);
}

#[test]
fn scrolling_either_pane_moves_the_other_without_feedback() {
    let (mut h, _d) = open("scroll", &long_doc());
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    let top = |h: &mut Harness<'static, MarkiseApp>| h.state_mut().viewer().unwrap().ed.view.anchor().0;
    assert_eq!(top(&mut h), 0);
    // scroll the source pane (left half) down: the rendered pane follows
    wheel(&mut h, pos2(420.0, 300.0), -2000.0);
    let src = h.state().source_scroll_offset().unwrap();
    assert!(src > 500.0, "source scrolled: {src}");
    let item = top(&mut h);
    assert!(item > 10, "rendered followed: item {item}");
    // the rendered pane's top item maps back to about the source pane's top line
    let model = h.state().model().unwrap().clone();
    let start = model.item_start(item);
    let text = h.state().viewed_text().unwrap().to_string();
    let line = text[..start].matches('\n').count() as f32;
    let pitch = src / line.max(1.0);
    assert!(
        pitch > 8.0 && pitch < 30.0,
        "consistent mapping: line {line}, offset {src}"
    );
    // idle frames: nothing moves (no feedback loop)
    h.run_steps(5);
    assert!((h.state().source_scroll_offset().unwrap() - src).abs() < 1.0);
    assert_eq!(top(&mut h), item);
    // scroll the rendered pane (right half) back up: the source pane follows
    wheel(&mut h, pos2(850.0, 300.0), 1500.0);
    let item2 = top(&mut h);
    assert!(item2 < item);
    let src2 = h.state().source_scroll_offset().unwrap();
    assert!(src2 < src, "source followed: {src2} < {src}");
    h.run_steps(5);
    assert_eq!(top(&mut h), item2, "no feedback");
}

#[test]
fn divider_position_is_persisted() {
    let cfg = tmpdir("split-cfg");
    let dir = tmpdir("split");
    let f = dir.join("doc.md");
    std::fs::write(&f, "# A\n").unwrap();
    {
        let mut h = harness(vec![f.clone()], ConfigDir::Dir(cfg.clone()));
        wait_until(&mut h, "load", loaded);
        h.state_mut().set_view_mode(ViewMode::SideBySide);
        h.run_steps(2);
        assert_eq!(h.state().side_by_side_split(), 0.5);
        h.state_mut().set_side_by_side_split(0.3);
        h.state_mut().on_exit();
    }
    let s = std::fs::read_to_string(settings(&cfg)).unwrap();
    assert!(s.contains("\"side_by_side_split\": 0.3"), "{s}");
    let mut h = harness(vec![f], ConfigDir::Dir(cfg));
    wait_until(&mut h, "load", loaded);
    assert_eq!(h.state().side_by_side_split(), 0.3);
}

fn settings(cfg: &Path) -> PathBuf {
    cfg.join("markise").join("settings.json")
}

#[test]
fn caret_moves_scroll_the_preview_only_when_the_block_is_out_of_view() {
    let (mut h, _d) = open("caret", &long_doc());
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    let top = |h: &mut Harness<'static, MarkiseApp>| h.state_mut().viewer().unwrap().ed.view.anchor().0;
    // click into the source pane near the top: the caret's block is visible, nothing moves
    let p = pos2(420.0, 120.0);
    h.hover_at(p);
    h.event(Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(3);
    assert_eq!(top(&mut h), 0, "a visible caret block does not scroll the preview");
    // jump to the end of the text: the caret's block is out of view, the preview follows
    let end = if cfg!(target_os = "macos") {
        (Modifiers::MAC_CMD | Modifiers::COMMAND, Key::ArrowDown)
    } else {
        (Modifiers::CTRL | Modifiers::COMMAND, Key::End)
    };
    h.key_press_modifiers(end.0, end.1);
    h.run_steps(4);
    let n = h.state().model().unwrap().items.len();
    assert!(top(&mut h) > n / 2, "preview follows the caret: {} of {n}", top(&mut h));
}

#[test]
fn scroll_sync_accounts_for_soft_wrapped_source_lines() {
    // long paragraph lines wrap in the narrow source pane
    let mut s = String::new();
    for i in 0..200 {
        s.push_str(&format!("## H{i}\n\n{}\n\n", format!("word{i} ").repeat(40)));
    }
    let (mut h, _d) = open("wrap", &s);
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    wheel(&mut h, pos2(420.0, 300.0), -3000.0);
    let line = h.state().source_top_line().unwrap();
    let item = h.state_mut().viewer().unwrap().ed.view.anchor().0;
    let model = h.state().model().unwrap().clone();
    let text = h.state().viewed_text().unwrap().to_string();
    let first = text[..model.item_start(item)].matches('\n').count() as f32;
    let last = text[..model.item_span(item).end].matches('\n').count() as f32 + 1.0;
    assert!(
        line >= first - 1.0 && line <= last + 1.0,
        "rendered top item spans lines {first}..{last}, source top line {line}"
    );
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

fn undo(h: &mut Harness<'static, MarkiseApp>) {
    let m = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    };
    h.key_press_modifiers(m, Key::Z);
    h.run_steps(2);
}

fn text(h: &mut Harness<'static, MarkiseApp>) -> String {
    h.state().document_text().unwrap()
}

/// Spec 02 §12.1.1 / ADR 0006 §4.3: Side by Side edits are transactions of the single live
/// history (undone from either pane or after leaving the mode); full Source mode starts a
/// fresh history; another document starts with a fresh one.
#[test]
fn undo_history_is_the_live_history_across_panes_and_documents() {
    let (mut h, dir) = open("undo", "# A\n\nalpha\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    click(&mut h, pos2(420.0, 50.0));
    h.event(Event::Text("Q".into()));
    h.run_steps(2);
    assert!(text(&mut h).contains('Q'), "typed: {:?}", text(&mut h));
    // undo in the source pane, redo through the session: the same history
    undo(&mut h);
    assert!(
        !text(&mut h).contains('Q'),
        "undone in the source pane: {:?}",
        text(&mut h)
    );
    h.state_mut().viewer().unwrap().ed.edit(|s| s.redo());
    h.run_steps(1);
    assert!(text(&mut h).contains('Q'));
    // full Source mode: a fresh history — the Side by Side edit is not undone there
    h.state_mut().set_view_mode(ViewMode::Source);
    h.run_steps(2);
    undo(&mut h);
    assert!(
        text(&mut h).contains('Q'),
        "not undone in Source mode: {:?}",
        text(&mut h)
    );
    // back in the rendered view the live history has it
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(2);
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    h.run_steps(1);
    assert!(
        !text(&mut h).contains('Q'),
        "undone in the rendered view: {:?}",
        text(&mut h)
    );
    // another document: Cmd+Z has nothing to undo
    let other = dir.join("other.md");
    std::fs::write(&other, "# B\n\nbeta\n").unwrap();
    h.state_mut().open(other);
    wait_until(&mut h, "second document", |a| {
        a.document_path().is_some_and(|p| p.ends_with("other.md")) && !a.is_loading()
    });
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    click(&mut h, pos2(420.0, 50.0));
    undo(&mut h);
    assert_eq!(text(&mut h), "# B\n\nbeta\n", "a fresh history");
}

/// Review P2: undoing back to the saved text reads clean (ADR 0006 §4.3 intent).
#[test]
fn returning_to_the_saved_text_is_clean() {
    let (mut h, _d) = open("clean", "# A\n\nalpha\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    h.state_mut().edit_source_text(|t| t.push_str("more\n"));
    h.run_steps(2);
    assert!(h.state().is_dirty());
    h.state_mut().edit_source_text(|t| t.truncate(t.len() - "more\n".len()));
    h.run_steps(2);
    assert!(!h.state().is_dirty(), "back at the saved content");
    // save, edit, edit back to the newly saved text: clean again
    h.state_mut().edit_source_text(|t| t.push_str("x\n"));
    h.run_steps(2);
    h.state_mut().save();
    wait_until(&mut h, "save", |a| !a.is_saving() && !a.is_dirty());
    h.state_mut().edit_source_text(|t| t.push_str("y\n"));
    h.run_steps(2);
    assert!(h.state().is_dirty());
    h.state_mut().edit_source_text(|t| t.truncate(t.len() - 2));
    h.run_steps(2);
    assert!(!h.state().is_dirty());
}

/// Typing in the source pane: verbatim keystrokes grouped like typing in the rendered view
/// (one "Typing" transaction), the right pane shows them, and the saved state stays an undo
/// boundary.
#[test]
fn left_edits_flow_into_the_editor_session() {
    let (mut h, _d) = open("session", "# A\n\nalpha\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    click(&mut h, pos2(420.0, 50.0));
    let end = if cfg!(target_os = "macos") {
        (Modifiers::MAC_CMD | Modifiers::COMMAND, Key::ArrowDown)
    } else {
        (Modifiers::CTRL | Modifiers::COMMAND, Key::End)
    };
    h.key_press_modifiers(end.0, end.1);
    h.run_steps(1);
    for c in ["o", "n", "e"] {
        h.event(Event::Text(c.into()));
        h.step();
    }
    h.run_steps(1);
    {
        let v = h.state_mut().viewer().unwrap();
        assert_eq!(v.ed.session.text(), "# A\n\nalpha\none");
        assert_eq!(v.ed.view.doc().text(), v.ed.session.text(), "the right pane shows it");
        let last = v.ed.session.history().last().unwrap();
        assert_eq!(last.label, "Typing");
        assert_eq!(v.ed.session.history().undo_len(), 1, "one typing group");
    }
    h.state_mut().save();
    wait_until(&mut h, "save", |a| !a.is_saving() && !a.is_dirty());
    h.event(Event::Text("!".into()));
    h.run_steps(2);
    assert!(h.state().is_dirty());
    // the saved state is a transaction boundary: the new edit is its own transaction
    let v = h.state_mut().viewer().unwrap();
    let last = v.ed.session.history().last().unwrap();
    assert_eq!(Some(last.state_before), v.ed.session.saved_state());
    assert_eq!(v.ed.session.history().undo_len(), 2);
}

/// Codex review round 4 of PR #21: Edit/Paragraph menu commands in Side by Side change the
/// session and the left buffer together; the next frames do not revert them.
#[test]
fn menu_commands_keep_both_panes_equal() {
    let (mut h, _d) = open("menucmds", "# A\n\nalpha\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(2);
    h.state_mut().edit_source_text(|t| t.push_str("more\n"));
    h.run_steps(2);
    let both = |h: &mut Harness<'static, MarkiseApp>| {
        let a = h.state().source_buffer().unwrap().to_string();
        let s = h.state_mut().viewer().unwrap().ed.session.text().to_string();
        (a, s)
    };
    let ctx = h.ctx.clone();
    assert!(h.state_mut().run_command(&ctx, "edit.undo"));
    h.run_steps(3);
    let (left, session) = both(&mut h);
    assert_eq!(session, "# A\n\nalpha\n");
    assert_eq!(left, session, "Undo: both panes");
    // with the source pane focused, block commands are not available there yet (slice 2)
    assert!(!h.state_mut().run_command(&ctx, "block.heading2"));
    // the rendered pane focused: a heading command at a caret in `alpha` acts on the session
    click(&mut h, pos2(900.0, 120.0));
    h.state_mut()
        .viewer()
        .unwrap()
        .ed
        .set_caret(markise_doc::DocPos::new(7), false);
    assert!(h.state_mut().run_command(&ctx, "block.heading2"));
    h.run_steps(3);
    let (left, session) = both(&mut h);
    assert_eq!(session, "# A\n\n## alpha\n");
    assert_eq!(left, session, "Heading: both panes");
}
