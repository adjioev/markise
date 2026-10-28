//! M2b interaction tests (egui_kittest): the real app, keyboard routing through the engine
//! keymap, typing, undo, dirty state, save and source-mode round trip in the rich view; a
//! typing smoke pass with platform-like key events (issue #31).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use markise_ui_egui::{AppOptions, MarkiseApp};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-edit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
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

fn wait_until(h: &mut Harness<'static, MarkiseApp>, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
    let t = Instant::now();
    while !f(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
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

fn typ(h: &mut Harness<'static, MarkiseApp>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
    }
}

fn text(h: &Harness<'static, MarkiseApp>) -> String {
    h.state().document_text().unwrap()
}

/// Clicks into the document surface (focus) and puts the caret at the document start.
fn focus_doc(h: &mut Harness<'static, MarkiseApp>) {
    let p = egui::pos2(600.0, 200.0);
    h.hover_at(p);
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
    // Jump to Top (⌘↑ on macOS, Ctrl+Home elsewhere)
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowUp);
    } else {
        h.key_press_modifiers(cmd(), Key::Home);
    }
    h.step();
}

#[test]
fn typing_undo_save_in_the_rich_view() {
    let dir = tmpdir("type");
    let p = dir.join("t.md");
    std::fs::write(&p, "Hello world\n\nSecond **bold** paragraph\n").unwrap();
    let mut h = harness(vec![p.clone()]);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    focus_doc(&mut h);
    typ(&mut h, "Hi ");
    assert_eq!(text(&h), "Hi Hello world\n\nSecond **bold** paragraph\n");
    assert!(h.state().is_dirty());
    // Undo through the keymap; back to the opened state is clean again
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "Hello world\n\nSecond **bold** paragraph\n");
    assert!(!h.state().is_dirty(), "undo to the opened state is clean");
    // arrows + Enter split the paragraph
    for _ in 0..5 {
        h.key_press(Key::ArrowRight);
        h.step();
    }
    h.key_press(Key::Enter);
    h.step();
    assert_eq!(text(&h), "Hello\n\nworld\n\nSecond **bold** paragraph\n");
    // Backspace at the start of `world` merges the paragraphs again
    h.key_press(Key::Backspace);
    h.step();
    assert_eq!(text(&h), "Helloworld\n\nSecond **bold** paragraph\n");
    // save writes the session's text
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(&mut h, "save", |a| !a.is_busy() && !a.is_dirty());
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "Helloworld\n\nSecond **bold** paragraph\n"
    );
    // source mode shows the edited text and edits made there come back to the rich view
    h.state_mut().toggle_source();
    h.step();
    h.state_mut().edit_source_text(|t| t.push_str("\nAdded in source.\n"));
    h.step();
    h.state_mut().toggle_source();
    h.step();
    assert!(text(&h).ends_with("Added in source.\n"));
    // the source edit is one undoable transaction in the rich view
    focus_doc(&mut h);
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), "Helloworld\n\nSecond **bold** paragraph\n");
    assert!(!h.state().is_dirty(), "back at the saved state");
}

#[test]
fn heading_command_from_the_keymap() {
    let dir = tmpdir("heading");
    let p = dir.join("h.md");
    std::fs::write(&p, "Title text\n").unwrap();
    let mut h = harness(vec![p]);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    focus_doc(&mut h);
    h.key_press_modifiers(cmd(), Key::Num2);
    h.step();
    assert_eq!(text(&h), "## Title text\n");
    h.key_press_modifiers(cmd(), Key::Num0);
    h.step();
    assert_eq!(text(&h), "Title text\n");
}

#[test]
fn shift_click_extends_from_the_retained_anchor() {
    // Codex review PR #21 P2: the press must not collapse the shift-click selection
    let dir = tmpdir("shift");
    let p = dir.join("s.md");
    let body: String = (0..12)
        .map(|i| format!("Paragraph number {i} with some words.\n\n"))
        .collect();
    std::fs::write(&p, &body).unwrap();
    let mut h = harness(vec![p]);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    focus_doc(&mut h); // caret at the document start
    let q = egui::pos2(500.0, 300.0);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    h.event(Event::ModifiersChanged(shift));
    h.hover_at(q);
    h.event(Event::PointerButton {
        pos: q,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: shift,
    });
    h.step();
    h.step(); // pointer still down: the drag update must keep the anchor
    h.event(Event::PointerButton {
        pos: q,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: shift,
    });
    h.event(Event::ModifiersChanged(Modifiers::default()));
    h.step();
    h.step();
    let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
    assert_eq!(sel.anchor.offset, 0, "anchor retained");
    assert!(sel.focus.offset > 40, "focus at the clicked point: {sel:?}");
    // copying the selection (no deadlock with the shaper's context lock)
    h.event(Event::Copy);
    h.step();
    let copied = h
        .output()
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t.starts_with("Paragraph number 0")));
    assert!(copied, "selection copied as source text");
}

/// Types like the platform does (egui-winit): a key press, the text it produced, the release.
fn type_keys(h: &mut Harness<'static, MarkiseApp>, s: &str) {
    for c in s.chars() {
        let key = if c == ' ' {
            Some(Key::Space)
        } else {
            Key::from_name(&c.to_string())
        };
        let ev = |k: Key, pressed: bool| Event::Key {
            key: k,
            physical_key: Some(k),
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        if let Some(k) = key {
            h.event(ev(k, true));
        }
        h.event(Event::Text(c.to_string()));
        if let Some(k) = key {
            h.event(ev(k, false));
        }
        h.step();
    }
}

fn key(h: &mut Harness<'static, MarkiseApp>, k: Key) {
    h.key_press(k);
    h.step();
}

fn caret(h: &mut Harness<'static, MarkiseApp>) -> usize {
    let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
    assert!(sel.is_caret(), "a caret: {sel:?}");
    sel.focus.offset
}

fn open_with(name: &str, src: &str, backend: markise_ui_egui::TextBackend) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let dir = tmpdir(name);
    let p = dir.join("t.md");
    std::fs::write(&p, src).unwrap();
    let app = MarkiseApp::new(AppOptions {
        open: vec![p.clone()],
        text_backend: backend,
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
    focus_doc(&mut h);
    (h, p)
}

fn open(name: &str, src: &str) -> (Harness<'static, MarkiseApp>, PathBuf) {
    open_with(name, src, Default::default())
}

fn save(h: &mut Harness<'static, MarkiseApp>, p: &PathBuf) -> String {
    h.key_press_modifiers(cmd(), Key::S);
    wait_until(h, "save", |a| !a.is_busy() && !a.is_dirty());
    std::fs::read_to_string(p).unwrap()
}

const SENTENCE: &str = "Foo bar, «цитата» \"q\" 'x' 123!";

#[test]
fn space_at_the_end_of_a_paragraph_issue_31() {
    // the reported case: typing a sentence at the end of a paragraph (and into an empty
    // document) — the trailing space must keep a caret stop after it
    for backend in [
        markise_ui_egui::TextBackend::Epaint,
        markise_ui_egui::TextBackend::Parley,
    ] {
        let (mut h, p) = open_with("smoke-space-end", "Hello\n", backend);
        key(&mut h, Key::End);
        type_keys(&mut h, " Foo bar ");
        assert_eq!(text(&h), "Hello Foo bar \n", "{backend:?}");
        assert_eq!(caret(&mut h), 14);
        type_keys(&mut h, "baz");
        assert_eq!(save(&mut h, &p), "Hello Foo bar baz\n");
        let (mut h, p) = open_with("smoke-space-empty", "", backend);
        type_keys(&mut h, "Foo bar");
        assert_eq!(save(&mut h, &p), "Foo bar", "{backend:?}");
    }
}

#[test]
fn typing_basics_in_a_paragraph() {
    let (mut h, p) = open("smoke-para", "Hello\n\nNext\n");
    key(&mut h, Key::End);
    type_keys(&mut h, " ");
    type_keys(&mut h, SENTENCE);
    assert_eq!(text(&h), format!("Hello {SENTENCE}\n\nNext\n"));
    assert_eq!(caret(&mut h), 6 + SENTENCE.len());
    // Enter splits at the end: a new paragraph
    key(&mut h, Key::Enter);
    type_keys(&mut h, "Two words");
    assert_eq!(text(&h), format!("Hello {SENTENCE}\n\nTwo words\n\nNext\n"));
    // Backspace deletes one character, Delete the one after the caret
    key(&mut h, Key::Backspace);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::Delete);
    assert_eq!(text(&h), format!("Hello {SENTENCE}\n\nTwo wod\n\nNext\n"));
    // undo / redo
    h.key_press_modifiers(cmd(), Key::Z);
    h.step();
    assert_eq!(text(&h), format!("Hello {SENTENCE}\n\nTwo word\n\nNext\n"));
    let shift_cmd = Modifiers { shift: true, ..cmd() };
    h.key_press_modifiers(shift_cmd, Key::Z);
    h.step();
    assert_eq!(text(&h), format!("Hello {SENTENCE}\n\nTwo wod\n\nNext\n"));
    assert_eq!(save(&mut h, &p), format!("Hello {SENTENCE}\n\nTwo wod\n\nNext\n"));
}

/// Opens `src`, moves the caret with `nav` keys, types `typed` and expects `want` on disk.
fn type_in(name: &str, src: &str, nav: &[Key], typed: &str, want: &str) {
    let (mut h, p) = open(name, src);
    for k in nav {
        key(&mut h, *k);
    }
    type_keys(&mut h, typed);
    assert_eq!(text(&h), want, "{name}");
    assert_eq!(save(&mut h, &p), want, "{name}");
}

#[test]
fn typing_with_spaces_in_blocks() {
    type_in(
        "blk-heading",
        "# Title\n",
        &[Key::End],
        " and more",
        "# Title and more\n",
    );
    type_in("blk-heading-mid", "# Title\n", &[], "A b ", "# A b Title\n");
    type_in(
        "blk-list",
        "- item\n- two\n",
        &[Key::End],
        " one, «ок» 1",
        "- item one, «ок» 1\n- two\n",
    );
    type_in(
        "blk-quote",
        "> quoted\n",
        &[Key::End],
        " text here",
        "> quoted text here\n",
    );
}

#[test]
fn typing_with_spaces_in_a_table_cell() {
    // the space typed at the end of a cell stays in that cell (it was snapped into the next one)
    type_in(
        "blk-table",
        "| a | b |\n|---|---|\n| x | y |\n",
        &[Key::End],
        " q r",
        "| a q r | b |\n|---|---|\n| x | y |\n",
    );
}

fn doc_end(h: &mut Harness<'static, MarkiseApp>) {
    if cfg!(target_os = "macos") {
        h.key_press_modifiers(cmd(), Key::ArrowDown);
    } else {
        h.key_press_modifiers(cmd(), Key::End);
    }
    h.step();
}

#[test]
fn trailing_space_at_the_end_of_a_segmented_paragraph() {
    // > 128 line pieces: the paragraph is split into segments; only the last one ends it
    let body: String = (0..300).map(|i| format!("line {i} of a long paragraph\n")).collect();
    let src = body.trim_end().to_string();
    let (mut h, p) = open("seg-para", &src);
    doc_end(&mut h);
    key(&mut h, Key::End);
    assert_eq!(caret(&mut h), src.len());
    type_keys(&mut h, " Foo bar");
    let want = format!("{src} Foo bar");
    assert_eq!(text(&h), want);
    assert_eq!(caret(&mut h), want.len());
    assert_eq!(save(&mut h, &p), want);
}

#[test]
fn typing_spaces_in_headings_closed_atx_and_setext() {
    type_in(
        "hd-closed",
        "# Title #\n",
        &[Key::End],
        " and more",
        "# Title and more #\n",
    );
    type_in(
        "hd-closed2",
        "## Title ##  \n",
        &[Key::End],
        " and more",
        "## Title and more ##  \n",
    );
    type_in(
        "hd-setext",
        "Title\n=====\n",
        &[Key::End],
        " and more",
        "Title and more\n=====\n",
    );
}

#[test]
fn trailing_space_edge_cases() {
    // two consecutive trailing spaces stay where they were typed
    type_in("ws-two", "Hello\n", &[Key::End], "  x", "Hello  x\n");
    // CRLF document
    type_in(
        "ws-crlf",
        "Hello\r\n\r\nNext\r\n",
        &[Key::End],
        " Foo bar",
        "Hello Foo bar\r\n\r\nNext\r\n",
    );
    // a paragraph inside a nested list and quote
    type_in(
        "ws-nested",
        "> - item\n>   - inner\n",
        &[Key::End],
        " a b",
        "> - item a b\n>   - inner\n",
    );
    type_in(
        "ws-nested2",
        "- outer\n  > quoted\n",
        &[Key::ArrowDown, Key::End],
        " a b",
        "- outer\n  > quoted a b\n",
    );
}

#[test]
fn trailing_space_at_soft_wrap_boundaries() {
    // a growing paragraph wraps many times: every typed space at a row end keeps its stop
    let (mut h, p) = open("ws-wrap", "Start\n");
    key(&mut h, Key::End);
    let mut want = String::from("Start");
    for i in 0..60 {
        let w = if i % 3 == 0 { " wrapping" } else { " ab" };
        type_keys(&mut h, w);
        want.push_str(w);
        assert_eq!(text(&h), format!("{want}\n"), "after {i} words");
        assert_eq!(caret(&mut h), want.len());
    }
    assert_eq!(save(&mut h, &p), format!("{want}\n"));
}
