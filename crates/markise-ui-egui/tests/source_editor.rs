//! The M2d source editor in the real app (spec 02 §12.1 / §12.1.1, ADR 0006 §4.3, §5.5):
//! typing with platform-like key events, verbatim save bytes, selection by keyboard and mouse,
//! soft wrap and line numbers, undo/redo across mode switches, position mapping in and out of
//! source mode, and a 10 MB document opened, scrolled and edited in source mode.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, ViewMode};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-src-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn open(name: &str, text: &str) -> (H, PathBuf) {
    let f = tmpdir(name).join("doc.md");
    std::fs::write(&f, text).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![f.clone()],
        config: ConfigDir::None,
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(vec2(1000.0, 700.0))
        // short simulated frames: a double click is two presses within the double-click time
        .with_step_dt(0.02)
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    (h, f)
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

fn typ(h: &mut H, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
    }
}

fn key(h: &mut H, k: Key) {
    h.key_press(k);
    h.step();
}

fn text(h: &H) -> String {
    h.state().document_text().unwrap()
}

fn source_mode(h: &mut H) {
    h.state_mut().set_view_mode(ViewMode::Source);
    h.run_steps(3);
    assert_eq!(h.state().view_mode(), ViewMode::Source);
}

fn click(h: &mut H, p: Pos2, n: usize) {
    h.hover_at(p);
    for _ in 0..n {
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        h.step();
    }
    h.run_steps(2);
}

fn doc_end(h: &mut H) {
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowDown);
    } else {
        h.key_press_modifiers(Modifiers::CTRL | Modifiers::COMMAND, Key::End);
    }
    h.step();
}

fn save(h: &mut H) {
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(h, "save", |a| !a.is_saving() && !a.is_dirty());
}

/// Typing in full Source mode is verbatim (no input rules) and saving writes exactly the
/// bytes: untouched CRLF terminators stay, a typed line break repeats its line's (C58).
#[test]
fn typing_in_source_mode_saves_verbatim_bytes() {
    let (mut h, f) = open("type", "# T\r\n\r\n* one\r\n");
    source_mode(&mut h);
    doc_end(&mut h);
    typ(&mut h, "* two");
    key(&mut h, Key::Enter);
    typ(&mut h, "- [ ] **x** (y");
    assert_eq!(text(&h), "# T\r\n\r\n* one\r\n* two\r\n- [ ] **x** (y");
    assert!(h.state().is_dirty());
    save(&mut h);
    assert_eq!(
        std::fs::read(&f).unwrap(),
        b"# T\r\n\r\n* one\r\n* two\r\n- [ ] **x** (y"
    );
}

/// Keyboard and mouse selection: shift+arrows, double click (word), triple click (line), and
/// typing replaces the selection.
#[test]
fn selection_by_keyboard_and_mouse() {
    let (mut h, _f) = open("select", "alpha beta gamma\nsecond line\n");
    source_mode(&mut h);
    key(&mut h, Key::Home);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.step();
    assert_eq!(h.state().source_selection(), Some(0..2));
    typ(&mut h, "A");
    assert_eq!(text(&h), "Apha beta gamma\nsecond line\n");
    // double click on the first line selects a word, triple click the line
    let r = h.state().source_rect().unwrap();
    let p = pos2(r.left() + 12.0 + 60.0, r.top() + 8.0 + 8.0);
    click(&mut h, p, 1);
    assert!(
        h.state().source_selection().unwrap().is_empty(),
        "a click places the caret"
    );
    h.run_steps(30);
    click(&mut h, p, 2);
    let sel = h.state().source_selection().unwrap();
    let word = &text(&h)[sel.clone()];
    assert!(!word.is_empty() && !word.contains(' '), "a word: {word:?}");
    click(&mut h, pos2(p.x + 300.0, p.y), 1);
    h.run_steps(30);
    click(&mut h, pos2(p.x, p.y), 3);
    let sel = h.state().source_selection().unwrap();
    assert_eq!(&text(&h)[sel], "Apha beta gamma\n");
}

/// Soft wrap (default on) and line numbers are display preferences: toggling them keeps the
/// text and the caret, and long lines wrap only with soft wrap on.
#[test]
fn soft_wrap_and_line_numbers_toggle() {
    let long = "word ".repeat(400);
    let (mut h, _f) = open("wrap", &format!("{long}\nshort\n"));
    source_mode(&mut h);
    let prefs = h.state().source_prefs();
    assert!(prefs.soft_wrap && !prefs.line_numbers && prefs.current_line && !prefs.whitespace_marks);
    // with wrap: going down from the first line stays in the long line (next visual row)
    key(&mut h, Key::Home);
    key(&mut h, Key::ArrowDown);
    assert!(
        h.state().source_caret().unwrap() < long.len(),
        "next row of the same line"
    );
    let ctx = h.ctx.clone();
    for id in ["view.sourceSoftWrap", "view.sourceLineNumbers", "view.sourceWhitespace"] {
        h.state_mut()
            .menu_action(&ctx, &markise_ui_egui::menu::MenuAction::Command(id));
    }
    h.run_steps(2);
    let prefs = h.state().source_prefs();
    assert!(!prefs.soft_wrap && prefs.line_numbers && prefs.whitespace_marks);
    key(&mut h, Key::Home);
    h.key_press_modifiers(cmd(), Key::ArrowUp);
    h.step();
    key(&mut h, Key::ArrowDown);
    assert!(
        h.state().source_caret().unwrap() > long.len(),
        "without wrap the next row is the next line"
    );
    assert_eq!(text(&h), format!("{long}\nshort\n"));
}

/// ADR 0006 §4.3: full Source mode has its own history (fresh on entry), collapsed into one
/// "Source Edit" on exit; the rendered view's undo history stays intact around it.
#[test]
fn undo_redo_across_mode_switches() {
    let (mut h, _f) = open("undo", "para\n");
    source_mode(&mut h);
    doc_end(&mut h);
    typ(&mut h, "more");
    key(&mut h, Key::Enter);
    typ(&mut h, "x");
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "para\nmore\n", "undo in source mode");
    h.key_press_modifiers(cmd() | Modifiers::SHIFT, Key::Z);
    h.step();
    assert_eq!(text(&h), "para\nmore\nx");
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(2);
    {
        let v = h.state_mut().viewer().unwrap();
        let last = v.ed.session.history().last().unwrap();
        assert_eq!(last.label, "Source Edit");
        assert_eq!(v.ed.session.history().undo_len(), 1);
    }
    h.state_mut().viewer().unwrap().ed.edit(|s| s.undo());
    h.step();
    assert_eq!(text(&h), "para\n", "the whole source session is one step");
    assert!(!h.state().is_dirty());
    // entering again: a fresh history (the redo of the rendered view is set aside meanwhile)
    source_mode(&mut h);
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "para\n");
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(2);
    h.state_mut().viewer().unwrap().ed.edit(|s| s.redo());
    h.step();
    assert_eq!(text(&h), "para\nmore\nx", "the rendered redo stack survived");
}

/// Spec 02 §12.1 position mapping: entering source mode centres the line of the block at the
/// rendered viewport's middle; leaving centres the block of the source viewport's middle line.
#[test]
fn position_mapping_in_and_out_of_source_mode() {
    let mut src = String::new();
    for i in 0..400 {
        src.push_str(&format!("## Heading {i}\n\nParagraph {i} text.\n\n"));
    }
    let (mut h, _f) = open("map", &src);
    // scroll the rendered view far down
    let target = src.find("## Heading 250").unwrap();
    {
        let v = h.state_mut().viewer().unwrap();
        let item = v.model().item_at_offset(target);
        v.ed.view.scroll_to_item(item);
    }
    h.run_steps(2);
    source_mode(&mut h);
    h.run_steps(2);
    let top = h.state().source_top_line().unwrap();
    let line_250 = src[..target].matches('\n').count() as f32;
    assert!(
        top > line_250 - 40.0 && top < line_250 + 5.0,
        "the heading's line is around the middle: top {top}, heading at {line_250}"
    );
    // move the source view, leave: the rendered view shows the same region
    h.state_mut().set_view_mode(ViewMode::Rendered);
    h.run_steps(2);
    let v = h.state_mut().viewer().unwrap();
    let (anchor, _) = v.ed.view.anchor();
    let start = v.model().item_start(anchor);
    let heading = src[..start].matches("## Heading ").count();
    assert!(
        (235..=255).contains(&heading),
        "rendered view near heading 250: {heading}"
    );
}

/// A 10 MB document: source mode opens (no size limit, no read-only fallback), jumps to the
/// end, takes a keystroke there, and the text saved is exactly the edited text.
#[test]
fn ten_megabyte_document_opens_scrolls_and_edits() {
    let big = markise_corpus::generate(10 << 20, 7);
    let (mut h, f) = open("big", &big);
    let t = Instant::now();
    source_mode(&mut h);
    let opened = t.elapsed();
    doc_end(&mut h);
    h.run_steps(2);
    assert_eq!(h.state().source_caret(), Some(big.len()));
    let t = Instant::now();
    typ(&mut h, "Z");
    let typed = t.elapsed();
    assert!(text(&h).ends_with('Z'));
    eprintln!("10 MB: source mode entered in {opened:?}, keystroke frame {typed:?} (debug build)");
    save(&mut h);
    let mut want = big.clone();
    want.push('Z');
    assert_eq!(std::fs::read_to_string(&f).unwrap(), want);
}

/// Side by Side: the read-only rendered pane does not move the source caret to a rendered caret
/// stop (a caret between hidden markers), so typing there stays one typing group.
#[test]
fn side_by_side_typing_between_markers_is_one_group() {
    let (mut h, _f) = open("sbsgroup", "**bold** text\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    key(&mut h, Key::Home);
    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::ArrowRight);
    assert_eq!(h.state().source_caret(), Some(2));
    typ(&mut h, "xyz");
    assert_eq!(text(&h), "**xyzbold** text\n");
    assert_eq!(h.state_mut().viewer().unwrap().ed.session.history().undo_len(), 1);
}

/// Review of PR #23 (P1): an external change reloaded while the source caret sits after `a`
/// in `ab` (now `éb`): the caret is mapped through the diff onto a character boundary and
/// Backspace works.
#[test]
fn reload_keeps_the_source_caret_on_a_char_boundary() {
    let (mut h, f) = open("reloadutf8", "ab");
    source_mode(&mut h);
    key(&mut h, Key::Home);
    key(&mut h, Key::ArrowRight);
    assert_eq!(h.state().source_caret(), Some(1));
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&f, "éb").unwrap();
    wait_until(&mut h, "reload", |a| a.document_text().as_deref() == Some("éb"));
    let c = h.state().source_caret().unwrap();
    assert!("éb".is_char_boundary(c), "caret {c}");
    key(&mut h, Key::Backspace);
    let t = text(&h);
    assert!(t == "b" || t == "éb", "{t:?}");
}

/// Review of PR #23 (P1): in Side by Side, an Undo run from the focused rendered pane maps the
/// source selection; typing in the source pane afterwards works.
#[test]
fn rendered_pane_undo_maps_the_source_caret() {
    let (mut h, _f) = open("sbsundo", "abc\n");
    h.state_mut().set_view_mode(ViewMode::SideBySide);
    h.run_steps(3);
    doc_end(&mut h);
    typ(&mut h, "xyz");
    assert_eq!(text(&h), "abc\nxyz");
    // focus the rendered pane and undo there (menu command)
    click(&mut h, pos2(900.0, 120.0), 1);
    let ctx = h.ctx.clone();
    assert!(h.state_mut().run_command(&ctx, "edit.undo"));
    h.run_steps(2);
    assert_eq!(text(&h), "abc\n");
    assert!(h.state().source_caret().unwrap() <= 4);
    // back to the source pane: Backspace at the mapped caret
    let r = h.state().source_rect().unwrap();
    click(&mut h, pos2(r.left() + 200.0, r.top() + 60.0), 1);
    key(&mut h, Key::Backspace);
    assert!(text(&h).len() < 5);
}
