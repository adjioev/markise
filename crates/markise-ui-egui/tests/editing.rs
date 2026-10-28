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

/// The caret rectangle the app reports to the platform (IME cursor area) this frame.
fn caret_screen(h: &mut Harness<'static, MarkiseApp>) -> egui::Rect {
    h.step();
    h.output().platform_output.ime.expect("a caret is shown").cursor_rect
}

/// Heading variants for issue #16: (source, navigation to the end of the heading text, the
/// caret offset there, the bytes after typing " x" there, the bytes after Enter and "x").
#[allow(clippy::type_complexity)]
const HEADINGS: &[(&str, &[Key], usize, &str, &str)] = &[
    (
        "# Title\n\nPara\n",
        &[Key::End],
        7,
        "# Title x\n\nPara\n",
        "# Title\n\nx\n\nPara\n",
    ),
    (
        "# Title\nPara\n",
        &[Key::End],
        7,
        "# Title x\nPara\n",
        "# Title\n\nx\nPara\n",
    ),
    (
        "## Title\n\nPara\n",
        &[Key::End],
        8,
        "## Title x\n\nPara\n",
        "## Title\n\nx\n\nPara\n",
    ),
    (
        "### Title\n\nPara\n",
        &[Key::End],
        9,
        "### Title x\n\nPara\n",
        "### Title\n\nx\n\nPara\n",
    ),
    (
        "#### Title\n\nPara\n",
        &[Key::End],
        10,
        "#### Title x\n\nPara\n",
        "#### Title\n\nx\n\nPara\n",
    ),
    (
        "##### Title\n\nPara\n",
        &[Key::End],
        11,
        "##### Title x\n\nPara\n",
        "##### Title\n\nx\n\nPara\n",
    ),
    (
        "###### Title\n\nPara\n",
        &[Key::End],
        12,
        "###### Title x\n\nPara\n",
        "###### Title\n\nx\n\nPara\n",
    ),
    (
        "# Title #\n\nPara\n",
        &[Key::End],
        7,
        "# Title x #\n\nPara\n",
        "# Title #\n\nx\n\nPara\n",
    ),
    (
        "Title\n=====\n\nPara\n",
        &[Key::End],
        5,
        "Title x\n=====\n\nPara\n",
        "Title\n=====\n\nx\n\nPara\n",
    ),
    (
        "Title\n-----\nPara\n",
        &[Key::End],
        5,
        "Title x\n-----\nPara\n",
        "Title\n-----\n\nx\nPara\n",
    ),
    (
        "Para\n\n# Title\n",
        &[Key::ArrowDown, Key::End],
        13,
        "Para\n\n# Title x\n",
        "Para\n\n# Title\n\nx\n",
    ),
    ("# Title", &[Key::End], 7, "# Title x", "# Title\n\nx"),
    (
        "- # Title\n\nPara\n",
        &[Key::End],
        9,
        "- # Title x\n\nPara\n",
        "- # Title\n\n  x\n\nPara\n",
    ),
    (
        "> # Title\n\nPara\n",
        &[Key::End],
        9,
        "> # Title x\n\nPara\n",
        "> # Title\n>\n> x\n\nPara\n",
    ),
];

fn open_toolbar(name: &str, src: &str, toolbar: bool) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let (mut h, p) = open(name, src);
    h.state_mut().set_show_toolbar(toolbar);
    h.step();
    (h, p)
}

#[test]
fn space_at_the_end_of_a_heading_issue_16() {
    // the space used to move the caret to the next block: the heading's span stops at its
    // text, so the caret in the whitespace after it was mapped to the following item
    for toolbar in [false, true] {
        for (src, nav, at, want, _) in HEADINGS {
            let (mut h, p) = open_toolbar("hd16-space", src, toolbar);
            for k in *nav {
                key(&mut h, *k);
            }
            assert_eq!(caret(&mut h), *at, "{src:?}");
            let before = caret_screen(&mut h);
            type_keys(&mut h, " ");
            assert_eq!(
                caret(&mut h),
                at + 1,
                "caret stays after the space: {src:?} toolbar={toolbar}"
            );
            let after = caret_screen(&mut h);
            assert_eq!(
                after.center().y,
                before.center().y,
                "same line: {src:?} toolbar={toolbar}"
            );
            assert!(after.min.x > before.min.x, "moved right: {src:?} toolbar={toolbar}");
            type_keys(&mut h, "x");
            assert_eq!(caret(&mut h), at + 2, "{src:?}");
            assert_eq!(save(&mut h, &p), *want, "{src:?} toolbar={toolbar}");
        }
    }
}

#[test]
fn enter_at_the_end_of_a_heading_issue_16() {
    // M2b-1 / C7: Enter at the end of a heading opens the (virtual) empty paragraph below,
    // also right after a typed trailing space (it used to do nothing there)
    for toolbar in [false, true] {
        for space in [false, true] {
            for (src, nav, _, _, want) in HEADINGS {
                let (mut h, p) = open_toolbar("hd16-enter", src, toolbar);
                for k in *nav {
                    key(&mut h, *k);
                }
                if space {
                    type_keys(&mut h, " ");
                }
                let before = caret_screen(&mut h);
                key(&mut h, Key::Enter);
                let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
                assert!(
                    sel.is_caret() && sel.focus.virt > 0,
                    "virtual paragraph: {src:?} {sel:?}"
                );
                let after = caret_screen(&mut h);
                assert!(
                    after.min.y > before.max.y,
                    "caret on the new line below: {src:?} space={space}"
                );
                type_keys(&mut h, "x");
                // a typed trailing space stays on the heading line
                let want = if space {
                    want.replacen("Title", "Title ", 1)
                } else {
                    want.to_string()
                };
                assert_eq!(save(&mut h, &p), want, "{src:?} toolbar={toolbar} space={space}");
            }
        }
    }
}

#[test]
fn space_and_enter_at_other_block_ends_issue_16() {
    // the same checks at the end of paragraphs, list items, quotes and table cells
    let space: &[(&str, usize, &str)] = &[
        ("Hello\n\nPara\n", 5, "Hello x\n\nPara\n"),
        ("Hello\n", 5, "Hello x\n"),
        ("- item\n- two\n", 6, "- item x\n- two\n"),
        ("> quoted\n\nPara\n", 8, "> quoted x\n\nPara\n"),
        (
            "| a | b |\n|---|---|\n| x | y |\n",
            3,
            "| a x | b |\n|---|---|\n| x | y |\n",
        ),
    ];
    for (src, at, want) in space {
        let (mut h, p) = open("blk16-space", src);
        key(&mut h, Key::End);
        assert_eq!(caret(&mut h), *at, "{src:?}");
        type_keys(&mut h, " ");
        assert_eq!(caret(&mut h), at + 1, "{src:?}");
        type_keys(&mut h, "x");
        assert_eq!(save(&mut h, &p), *want, "{src:?}");
    }
    let enter: &[(&str, &str)] = &[
        ("Hello\n\nPara\n", "Hello\n\nx\n\nPara\n"),
        ("Hello\n", "Hello\n\nx\n"),
        ("- item\n- two\n", "- item\n- x\n- two\n"),
        ("> quoted\n\nPara\n", "> quoted\n>\n> x\n\nPara\n"),
    ];
    for (src, want) in enter {
        let (mut h, p) = open("blk16-enter", src);
        key(&mut h, Key::End);
        let before = caret_screen(&mut h);
        key(&mut h, Key::Enter);
        let after = caret_screen(&mut h);
        assert!(after.min.y > before.max.y, "caret on the new line below: {src:?}");
        type_keys(&mut h, "x");
        assert_eq!(save(&mut h, &p), *want, "{src:?}");
    }
}

#[test]
fn spaces_after_a_heading_are_drawn_issue_16() {
    // the caret is drawn one space further after each Space (the whitespace after an ATX
    // heading lies outside its item's source: its layout must still be refreshed)
    for backend in [
        markise_ui_egui::TextBackend::Epaint,
        markise_ui_egui::TextBackend::Parley,
    ] {
        for (src, want) in [
            ("# Title\n\nPara\n", "# Title   x\n\nPara\n"),
            ("# Title\nPara\n", "# Title   x\nPara\n"),
            ("# Title\n", "# Title   x\n"),
        ] {
            let (mut h, p) = open_with("hd16-drawn", src, backend);
            key(&mut h, Key::End);
            let mut x = caret_screen(&mut h);
            let mut step: Option<f32> = None;
            for n in 1..=3 {
                type_keys(&mut h, " ");
                assert_eq!(caret(&mut h), 7 + n, "{backend:?} {src:?}");
                let r = caret_screen(&mut h);
                assert_eq!(r.center().y, x.center().y, "same line: {backend:?} {src:?} space {n}");
                let dx = r.min.x - x.min.x;
                assert!(dx > 2.0, "advanced by a space: {backend:?} {src:?} space {n}: {dx}");
                if let Some(s) = step {
                    assert!(
                        (dx - s).abs() < 0.5,
                        "one space width each: {backend:?} {src:?} {dx} vs {s}"
                    );
                }
                step = Some(dx);
                x = r;
            }
            type_keys(&mut h, "x");
            assert_eq!(save(&mut h, &p), want, "{backend:?}");
        }
    }
}

#[test]
fn toolbar_after_a_space_at_a_heading_end_issue_16() {
    // block style and inline commands still act on the heading after a trailing space
    let (mut h, p) = open_toolbar("hd16-toolbar", "# Title\n\nPara\n", true);
    key(&mut h, Key::End);
    type_keys(&mut h, " ");
    let m = h.state().toolbar_state().unwrap();
    assert_eq!(m.block_label(), "Heading 1");
    for id in [
        "block.paragraph",
        "block.heading2",
        "format.strong",
        "format.emph",
        "format.link",
        "block.quote",
    ] {
        assert!(m.get(id).unwrap().enabled, "{id} enabled after the space");
    }
    // Bold inserts its markers at the caret (after the space), in the heading
    h.key_press_modifiers(cmd(), Key::B);
    h.step();
    type_keys(&mut h, "b");
    assert_eq!(text(&h), "# Title **b**\n\nPara\n");
    assert_eq!(h.state().toolbar_state().unwrap().block_label(), "Heading 1");
    assert_eq!(save(&mut h, &p), "# Title **b**\n\nPara\n");
    // a block command keeps the caret after the whitespace
    let (mut h, p) = open_toolbar("hd16-toolbar2", "# Title\n\nPara\n", true);
    key(&mut h, Key::End);
    type_keys(&mut h, " ");
    h.key_press_modifiers(cmd(), Key::Num2);
    h.step();
    assert_eq!(text(&h), "## Title \n\nPara\n");
    assert_eq!(h.state().toolbar_state().unwrap().block_label(), "Heading 2");
    assert_eq!(caret(&mut h), 9);
    type_keys(&mut h, "x");
    assert_eq!(save(&mut h, &p), "## Title x\n\nPara\n");
}

#[test]
fn moving_a_heading_keeps_the_caret_in_its_trailing_spaces_issue_16() {
    // Move Row (Alt+↓ / Alt+↑) with the caret inside spaces typed after the heading text
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    let (mut h, p) = open("hd16-move", "# Title\n\nPara\n");
    key(&mut h, Key::End);
    type_keys(&mut h, "  ");
    h.key_press_modifiers(alt, Key::ArrowDown);
    h.step();
    assert_eq!(text(&h), "Para\n\n# Title  \n");
    assert_eq!(caret(&mut h), 15, "after both spaces");
    h.key_press_modifiers(alt, Key::ArrowUp);
    h.step();
    assert_eq!(text(&h), "# Title  \n\nPara\n");
    assert_eq!(caret(&mut h), 9);
    // one space in: the caret stays between the two spaces
    key(&mut h, Key::ArrowLeft);
    assert_eq!(caret(&mut h), 8);
    h.key_press_modifiers(alt, Key::ArrowDown);
    h.step();
    assert_eq!(caret(&mut h), 14);
    type_keys(&mut h, "x");
    assert_eq!(save(&mut h, &p), "Para\n\n# Title x \n");
}

/// Caret offset, drawn caret rectangle and toolbar block label after the last key.
fn caret_state(h: &mut Harness<'static, MarkiseApp>) -> (usize, egui::Rect, &'static str) {
    let c = caret(h);
    let r = caret_screen(h);
    let label = h.state().toolbar_state().map_or("-", |m| m.block_label());
    (c, r, label)
}

fn open_toolbar_with(
    name: &str,
    src: &str,
    backend: markise_ui_egui::TextBackend,
) -> (Harness<'static, MarkiseApp>, PathBuf) {
    let (mut h, p) = open_with(name, src, backend);
    h.state_mut().set_show_toolbar(true);
    h.step();
    (h, p)
}

#[test]
fn backspace_at_the_end_of_a_heading_issue_16() {
    // Backspace removes typed trailing spaces one by one, then letters: the caret is drawn
    // one step further left each time on the heading line, the toolbar stays on the heading
    // (spaces after an ATX heading's text lie after its span: Backspace did nothing there)
    let variants: &[(&str, &[Key], &str)] = &[
        ("# Title\n\nPara\n", &[Key::End], "Heading 1"),
        ("# Title\nPara\n", &[Key::End], "Heading 1"),
        ("# Title\n", &[Key::End], "Heading 1"),
        ("### Title\n\nPara\n", &[Key::End], "Heading 3"),
        ("# Title #\n\nPara\n", &[Key::End], "Heading 1"),
        ("Title\n=====\n\nPara\n", &[Key::End], "Heading 1"),
        ("Title\n-----\nPara\n", &[Key::End], "Heading 2"),
        ("Para\n\n# Title\n", &[Key::ArrowDown, Key::End], "Heading 1"),
        ("- # Title\n\nPara\n", &[Key::End], "Heading 1"),
        ("> # Title\n\nPara\n", &[Key::End], "Heading 1"),
    ];
    for backend in [
        markise_ui_egui::TextBackend::Epaint,
        markise_ui_egui::TextBackend::Parley,
    ] {
        for (src, nav, label) in variants {
            for spaces in [0, 1, 3] {
                let (mut h, p) = open_toolbar_with("hd16-bs", src, backend);
                for k in *nav {
                    key(&mut h, *k);
                }
                type_keys(&mut h, &" ".repeat(spaces));
                let (mut c, mut r, l) = caret_state(&mut h);
                assert_eq!(l, *label, "{backend:?} {src:?}");
                for n in 0..spaces + 2 {
                    key(&mut h, Key::Backspace);
                    let (c2, r2, l2) = caret_state(&mut h);
                    let what = format!("{backend:?} {src:?} spaces={spaces} backspace {}", n + 1);
                    assert_eq!(c2 + 1, c, "{what}");
                    assert_eq!(r2.center().y, r.center().y, "same line: {what}");
                    assert!(r2.min.x < r.min.x - 1.0, "drawn further left: {what}");
                    assert_eq!(l2, *label, "{what}");
                    (c, r) = (c2, r2);
                }
                assert_eq!(
                    save(&mut h, &p),
                    src.replacen("Title", "Tit", 1),
                    "{backend:?} {spaces}"
                );
            }
        }
    }
}

#[test]
fn backspace_in_and_at_the_start_of_a_heading_issue_16() {
    // inside the text: one character, caret drawn left, still the heading
    let (mut h, p) = open_toolbar("hd16-bs-mid", "Para\n\n# Title\n", true);
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::End);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::ArrowLeft);
    let (c, r, _) = caret_state(&mut h);
    key(&mut h, Key::Backspace);
    let (c2, r2, l) = caret_state(&mut h);
    assert_eq!((c2, l), (c - 1, "Heading 1"));
    assert!(r2.min.x < r.min.x && r2.center().y == r.center().y);
    assert_eq!(save(&mut h, &p), "Para\n\n# Tile\n");
    // at the start of the text: the heading becomes a paragraph, then merges with the
    // paragraph above (spec 02 §5.2 rules 4 and 5); below a list, with the list's last paragraph
    for (src, nav, want1, want2) in [
        (
            "Para\n\n# Title\n",
            &[Key::ArrowDown, Key::Home][..],
            "Para\n\nTitle\n",
            "ParaTitle\n",
        ),
        (
            "Para\n# Title\n",
            &[Key::ArrowDown, Key::Home][..],
            "Para\nTitle\n",
            "ParaTitle\n",
        ),
        (
            "- one\n- two\n\n# Title\n",
            &[Key::ArrowDown, Key::ArrowDown, Key::Home][..],
            "- one\n- two\n\nTitle\n",
            "- one\n- twoTitle\n",
        ),
    ] {
        let (mut h, p) = open_toolbar("hd16-bs-start", src, true);
        for k in nav {
            key(&mut h, *k);
        }
        key(&mut h, Key::Backspace);
        assert_eq!(text(&h), want1, "{src:?}");
        assert_eq!(caret_state(&mut h).2, "Paragraph", "{src:?}");
        key(&mut h, Key::Backspace);
        assert_eq!(save(&mut h, &p), want2, "{src:?}");
    }
}

#[test]
fn delete_in_the_trailing_spaces_of_a_heading_issue_16() {
    // between typed spaces: Delete removes the next one (it did nothing)
    let (mut h, p) = open_toolbar("hd16-del", "# Title\n\nPara\n", true);
    key(&mut h, Key::End);
    type_keys(&mut h, "  ");
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::Delete);
    assert_eq!(text(&h), "# Title \n\nPara\n");
    let (c, _, l) = caret_state(&mut h);
    assert_eq!((c, l), (8, "Heading 1"));
    // at the end of the line: the next block's text is merged (C48) and the spaces stay
    key(&mut h, Key::Delete);
    assert_eq!(text(&h), "# Title Para\n");
    assert_eq!(caret_state(&mut h).0, 8);
    assert_eq!(save(&mut h, &p), "# Title Para\n");
    let (mut h, p) = open_toolbar("hd16-del2", "# Title\n\n# Next\n", true);
    key(&mut h, Key::End);
    type_keys(&mut h, " ");
    key(&mut h, Key::Delete);
    assert_eq!(save(&mut h, &p), "# Title Next\n");
}

#[test]
fn selection_from_trailing_spaces_stays_in_the_block_issue_16() {
    // Shift+← from inside trailing spaces: the anchor stayed in whitespace that is laid out
    // only while the focus is in it, so it was "fixed" to the next block and Backspace
    // deleted across blocks (`# TitPara`, or a table cell's pipe)
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    for (src, typed, n, want) in [
        ("# Title\n\nPara\n", "  ", 4, "# Tit\n\nPara\n"),
        ("# Title #\n\nPara\n", "  ", 4, "# Tit #\n\nPara\n"),
        ("# Title\n", " ", 2, "# Titl\n"),
        ("Hello\n\nPara\n", " ", 3, "Hel\n\nPara\n"),
        (
            "| ab | c |\n|---|---|\n| x | y |\n",
            " ",
            2,
            "| a | c |\n|---|---|\n| x | y |\n",
        ),
    ] {
        let (mut h, p) = open_toolbar("hd16-sel", src, true);
        key(&mut h, Key::End);
        type_keys(&mut h, typed);
        let at = caret(&mut h);
        for _ in 0..n {
            h.key_press_modifiers(shift, Key::ArrowLeft);
            h.step();
        }
        let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
        assert_eq!(sel.anchor.offset, at, "anchor kept: {src:?}");
        assert_eq!(sel.focus.offset, at - n, "{src:?}");
        key(&mut h, Key::Backspace);
        assert_eq!(caret(&mut h), at - n, "{src:?}");
        assert_eq!(save(&mut h, &p), want, "{src:?}");
    }
}

#[derive(Clone, Copy)]
enum St {
    K(Key),
    C(Key),
    S(Key),
    T(&'static str),
    /// Cmd/Ctrl + Alt + Shift + key
    Cas(Key),
}

/// `src` with `‸` (caret) or `⟦`…`⟧` (anchor…focus): the plain text and the selection.
fn parse_marked(src: &str) -> (String, usize, usize) {
    let mut t = String::new();
    let (mut a, mut f) = (None, None);
    for ch in src.chars() {
        match ch {
            '‸' => {
                a = Some(t.len());
                f = Some(t.len());
            }
            '⟦' => a = Some(t.len()),
            '⟧' => f = Some(t.len()),
            c => t.push(c),
        }
    }
    (t, a.unwrap(), f.unwrap())
}

fn run_marked(name: &str, src: &str, steps: &[St]) -> String {
    let (t, a, f) = parse_marked(src);
    let (mut h, _p) = open(name, &t);
    {
        let ed = &mut h.state_mut().viewer().unwrap().ed;
        ed.set_selection(markise_doc::Selection::single(
            markise_doc::DocPos::new(a),
            markise_doc::DocPos::new(f),
        ));
    }
    h.step();
    h.step();
    for s in steps {
        match *s {
            St::K(k) => key(&mut h, k),
            St::C(k) => {
                h.key_press_modifiers(cmd(), k);
                h.step();
            }
            St::S(k) => {
                h.key_press_modifiers(
                    Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                    k,
                );
                h.step();
            }
            St::T(s) => type_keys(&mut h, s),
            St::Cas(k) => {
                h.key_press_modifiers(cmd().plus(Modifiers::ALT).plus(Modifiers::SHIFT), k);
                h.step();
            }
        }
    }
    h.step();
    let sel = h.state_mut().viewer().unwrap().ed.session.selection().primary();
    let mut out = text(&h);
    if sel.is_caret() {
        let mark = if sel.focus.virt > 0 { "‸v" } else { "‸" };
        out.insert_str(sel.focus.offset, mark);
    } else {
        let (s, e) = (sel.range().start, sel.range().end);
        out.insert(e, '⟧');
        out.insert(s, '⟦');
    }
    out
}

/// Heading behaviour of the block-edge specification (T-HE cases; Markise decisions C7, C43,
/// C44, M2b-1 and owner decisions BE-1…BE-4 where they differ). `‸` is the caret (`‸v`: on the
/// virtual paragraph), `⟦`…`⟧` a selection. Not covered here (follow-ups): Enter at the start
/// of a heading that opens the document (an empty paragraph above the first block, T-HE.7),
/// Backspace at a heading's start after a rule or inside a quote / list item (T-HE.20–23),
/// Cmd+N on a heading of level N (open question F.6: today it toggles to a paragraph) and
/// Cmd+N on one line of a soft-broken paragraph (T-HE.37).
#[test]
fn heading_edges_t_he() {
    use St::*;
    let cases: &[(&str, &str, &[St], &str)] = &[
        ("1", "## Title‸\n", &[T("  ")], "## Title  ‸\n"),
        ("3", "## Title‸\n", &[K(Key::Enter), T("x")], "## Title\n\nx‸\n"),
        ("4", "## Title  ‸\n", &[K(Key::Enter), T("x")], "## Title  \n\nx‸\n"),
        // BE-2: only whitespace after the caret is "at the end"
        ("5", "## Title‸  \n", &[K(Key::Enter), T("x")], "## Title  \n\nx‸\n"),
        ("6", "## Ti‸tle\n", &[K(Key::Enter)], "## Ti\n\n‸tle\n"),
        (
            "7",
            "Intro\n\n## ‸Title\n",
            &[K(Key::Enter), T("x")],
            "Intro\n\nx‸\n\n## Title\n",
        ),
        ("8", "## ‸\n", &[K(Key::Enter)], "‸\n"),
        ("8b", "Intro\n\n## ‸\n", &[K(Key::Enter), T("x")], "Intro\n\nx‸\n"),
        ("10", "## Ti‸tle\n", &[S(Key::Enter)], "## Ti<br/>‸tle\n"),
        ("11", "Title‸\n===\n", &[K(Key::Enter), T("x")], "Title\n===\n\nx‸\n"),
        ("12", "Ti‸tle\n===\n", &[K(Key::Enter)], "Ti\n===\n\n‸tle\n"),
        ("13", "# Ti‸tle #\n", &[K(Key::Enter)], "# Ti #\n\n‸tle\n"),
        ("15", "## ‸Title\n", &[K(Key::Backspace)], "‸Title\n"),
        ("16", "## ‸Title ##\n", &[K(Key::Backspace)], "‸Title\n"),
        ("17", "‸Title\n---\n", &[K(Key::Backspace)], "‸Title\n"),
        (
            "18",
            "Intro\n\n## ‸Title\n",
            &[K(Key::Backspace), K(Key::Backspace)],
            "Intro‸Title\n",
        ),
        (
            "21b",
            "> ## ‸Title\n",
            &[K(Key::Backspace), K(Key::Backspace)],
            "‸Title\n",
        ),
        ("24", "## T‸\n", &[K(Key::Backspace)], "## ‸\n"),
        ("25", "## ‸\n", &[K(Key::Backspace)], "‸\n"),
        ("25b", "Intro\n\n## ‸\n", &[K(Key::Backspace), T("x")], "Intro\n\nx‸\n"),
        // BE-3: Delete removes the empty heading; the next block keeps its type
        ("26", "## ‸\n\nnext\n", &[K(Key::Delete)], "‸next\n"),
        ("26b", "## ‸\n\n### B\n", &[K(Key::Delete)], "### ‸B\n"),
        ("27", "## ‸\n", &[K(Key::Delete)], "‸\n"),
        ("28", "## Title‸\n\npara\n", &[K(Key::Delete)], "## Title‸para\n"),
        ("29", "## A‸\n\n### B\n", &[K(Key::Delete)], "## A‸B\n"),
        ("30", "## A‸\n\n- b\n", &[K(Key::Delete)], "## A‸b\n"),
        ("31", "## Ti‸tle\n", &[C(Key::Num3)], "### Ti‸tle\n"),
        // E18: a closed ATX heading's closing hashes follow the level, or go with it
        ("32", "## Ti‸tle ##\n", &[C(Key::Num3)], "### Ti‸tle ###\n"),
        ("32b", "## Ti‸tle ##\n", &[C(Key::Num0)], "Ti‸tle\n"),
        ("33", "Ti‸tle\n===\n", &[C(Key::Num2)], "Ti‸tle\n---\n"),
        ("34", "Ti‸tle\n===\n", &[C(Key::Num3)], "### Ti‸tle\n"),
        ("36", "## Ti‸tle\n", &[C(Key::Num0)], "Ti‸tle\n"),
        ("38", "# T‸\n", &[C(Key::Equals)], "# T‸\n"),
        ("39", "###### T‸\n", &[C(Key::Minus)], "T‸\n"),
        ("40", "T‸\n", &[C(Key::Equals)], "###### T‸\n"),
        ("41", "## Ti⟦tle\n\npa⟧ra\n", &[K(Key::Backspace)], "## Ti‸ra\n"),
        // BE-4: from the heading's text start the remainder keeps the later block's type
        ("42", "## ⟦Title\n\npa⟧ra\n", &[K(Key::Backspace)], "‸ra\n"),
        ("42b", "## ⟦Title\n\npa⟧ra\n", &[T("x")], "x‸ra\n"),
        ("42c", "## ⟦Title\n\npa⟧ra\n", &[K(Key::Delete)], "‸ra\n"),
        ("43", "## Ti⟦tle\n\npa⟧ra\n", &[T("x")], "## Tix‸ra\n"),
        ("44", "te⟦xt\n\n## Ti⟧tle\n", &[K(Key::Backspace)], "te‸tle\n"),
        ("46", "‸", &[T("## Plan"), K(Key::Enter), T("x")], "## Plan\n\nx‸"),
        ("49", "## Title‸\n", &[K(Key::Tab)], "## Title\t‸\n"),
        ("50", "## Title‸\n", &[T("xyz"), C(Key::Z)], "## Title‸\n"),
    ];
    let mut failed = Vec::new();
    for (id, src, steps, want) in cases {
        let got = run_marked(&format!("the{id}"), src, steps);
        if got != *want {
            failed.push(format!("T-HE.{id}: {src:?} -> {got:?}, want {want:?}"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn delete_after_heading_spaces_never_skips_a_block_issue_16() {
    // Codex review: Delete after a heading's trailing spaces declined in the Markdown rules and
    // the engine's merge fallback scanned past a list (`# Title  After`, the list gone). It
    // now follows the rules at the text end (the spaces stay) and never merges past a block.
    use St::*;
    let cases: &[(&str, &str)] = &[
        // the first block of a list item / quote merges into the heading (C48)
        ("- keep\n- two\n", "keep\n- two\n"),
        ("> keep\n", "keep\n"),
        // a rule after the heading is removed
        ("---\n", "\n"),
        // tables, code, indented code, link definitions: nothing happens
        (
            "| a | b |\n|---|---|\n| x | y |\n",
            "\n\n| a | b |\n|---|---|\n| x | y |\n",
        ),
        ("```\ncode\n```\n", "\n\n```\ncode\n```\n"),
        ("    indented\n", "\n\n    indented\n"),
        ("[ref]: http://x\n", "\n\n[ref]: http://x\n"),
    ];
    for (next, rest) in cases {
        for sp in ["", "  "] {
            let src = format!("# Title{sp}‸\n\n{next}\nAfter\n");
            let rest = if rest.is_empty() {
                "\n\n".to_string()
            } else {
                rest.to_string()
            };
            let want = format!("# Title{sp}‸{rest}\nAfter\n");
            assert_eq!(run_marked("hd16-next", &src, &[K(Key::Delete)]), want, "{src:?}");
        }
    }
    // Backspace mirrors at the start of the next block: its own rules, the heading untouched
    for (src, want) in [
        (
            "# Title  \n\n- ‸keep\n- two\n\nAfter\n",
            "# Title  \n\n‸keep\n\n- two\n\nAfter\n",
        ),
        ("# Title  \n\n> ‸keep\n\nAfter\n", "# Title  \n\n‸keep\n\nAfter\n"),
    ] {
        assert_eq!(run_marked("hd16-next-bs", src, &[K(Key::Backspace)]), want, "{src:?}");
    }
}

#[test]
fn joins_stop_at_the_neighbouring_block_issue_16() {
    // Codex review round 4: line scans skipped a standalone empty quote `>` (and could skip
    // other blocks without a text leaf), so Delete / Backspace / the empty-heading rule joined
    // or deleted across it. Neighbours now come from the parsed block structure: a block with
    // no text leaf at its edge stops the join, nothing past the neighbour is touched.
    use St::*;
    for mid in [">\n", "-\n", "- \n", "<!-- c -->\n"] {
        // Delete at a heading end (with and without trailing spaces), Backspace at the start
        // of the paragraph after the block: no change
        for src in [
            format!("# Title‸\n\n{mid}\nAfter\n"),
            format!("# Title  ‸\n\n{mid}\nAfter\n"),
            format!("# Title\n\n{mid}\n‸After\n"),
        ] {
            let k = if src.contains("‸After") {
                Key::Backspace
            } else {
                Key::Delete
            };
            assert_eq!(run_marked("nb-join", &src, &[K(k)]), src, "{k:?} {src:?}");
        }
        // an empty heading next to the block: Backspace / Delete leave both alone, Enter only
        // opens the virtual paragraph after the heading
        let html = mid.starts_with('<');
        for (src, enter, back, del) in [
            (
                format!("Before\n\n{mid}\n## ‸\n"),
                if html {
                    format!("Before\n\n{}‸v\n", mid.trim_end())
                } else {
                    format!("Before\n\n{mid}\n##‸v \n")
                },
                if html {
                    format!("Before\n\n{}‸v\n", mid.trim_end())
                } else {
                    format!("Before\n\n{mid}\n## ‸\n")
                },
                if html {
                    format!("Before\n\n{}‸v\n", mid.trim_end())
                } else {
                    format!("Before\n\n{mid}\n## ‸\n")
                },
            ),
            (
                format!("## ‸\n\n{mid}\nAfter\n"),
                format!("##‸v \n\n{mid}\nAfter\n"),
                format!("## ‸\n\n{mid}\nAfter\n"),
                if html {
                    format!("‸{mid}\nAfter\n")
                } else {
                    format!("## ‸\n\n{mid}\nAfter\n")
                },
            ),
        ] {
            assert_eq!(run_marked("nb-e", &src, &[K(Key::Enter)]), enter, "Enter {src:?}");
            assert_eq!(
                run_marked("nb-b", &src, &[K(Key::Backspace)]),
                back,
                "Backspace {src:?}"
            );
            assert_eq!(run_marked("nb-d", &src, &[K(Key::Delete)]), del, "Delete {src:?}");
        }
    }
    // front matter before an empty heading (C43 keeps it), and before a heading's start
    let fm = "---\ntitle: x\n---\n\n## ‸\n";
    for k in [Key::Enter, Key::Backspace, Key::Delete] {
        assert_eq!(run_marked("nb-fm", fm, &[K(k)]), "---\ntitle: x\n---‸v\n", "{k:?}");
    }
    assert_eq!(
        run_marked(
            "nb-fm2",
            "---\ntitle: x\n---\n\n# ‸Title\n",
            &[K(Key::Backspace), K(Key::Backspace)]
        ),
        "---\ntitle: x\n---\n\n‸Title\n"
    );
}

/// Asserts that each of `n` Spaces typed at the caret moves the drawn caret right by one space
/// width on the same line, with the caret offset advancing from `at`.
fn assert_spaces_drawn(h: &mut Harness<'static, MarkiseApp>, at: usize, n: usize, what: &str) {
    let mut x = caret_screen(h);
    let mut step: Option<f32> = None;
    for i in 1..=n {
        type_keys(h, " ");
        assert_eq!(caret(h), at + i, "{what}: offset after space {i}");
        let r = caret_screen(h);
        assert_eq!(r.center().y, x.center().y, "{what}: same line after space {i}");
        let dx = r.min.x - x.min.x;
        assert!(dx > 2.0, "{what}: advanced by a space after space {i}: {dx}");
        if let Some(s) = step {
            assert!((dx - s).abs() < 0.5, "{what}: one space width each: {dx} vs {s}");
        }
        step = Some(dx);
        x = r;
    }
}

#[test]
fn spaces_after_a_final_heading_are_drawn() {
    // owner report: a heading that is the last block — without a final newline, and with one
    for backend in [
        markise_ui_egui::TextBackend::Epaint,
        markise_ui_egui::TextBackend::Parley,
    ] {
        for ((src, want), toolbar) in [("# Title", "# Title   x"), ("# Title\n", "# Title   x\n")]
            .into_iter()
            .flat_map(|c| [(c, false), (c, true)])
        {
            let (mut h, p) = open_with("final-heading-ws", src, backend);
            h.state_mut().set_show_toolbar(toolbar);
            h.step();
            key(&mut h, Key::End);
            assert_spaces_drawn(&mut h, 7, 3, &format!("{backend:?} {src:?} toolbar={toolbar}"));
            type_keys(&mut h, "x");
            assert_eq!(save(&mut h, &p), want, "{backend:?}");
        }
    }
}

/// The owner's settings on top of built-in style `style`: font size override 17.5, dark
/// appearance, toolbar at the bottom.
fn owner_look(h: &mut Harness<'static, MarkiseApp>, style: &str) {
    let st = h.state_mut();
    st.select_style(markise_theme::catalog::StyleId::Builtin(style.into()));
    st.set_overrides(markise_theme::Overrides {
        font_size: Some(17.5),
        ..Default::default()
    });
    st.set_theme_choice(markise_ui_egui::Appearance::Dark);
    st.set_show_toolbar(true);
    st.set_toolbar_position(markise_engine::settings::ToolbarPosition::Bottom);
    for _ in 0..3 {
        h.step();
    }
}

/// Opens an untitled document (File → New) with `style`, and types `src` into it.
fn untitled_with(backend: markise_ui_egui::TextBackend, style: &str, src: &str) -> Harness<'static, MarkiseApp> {
    let app = MarkiseApp::new(AppOptions {
        text_backend: backend,
        ..Default::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .allow_missing_glyphs()
        .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
    h.step();
    h.key_press_modifiers(cmd(), Key::N);
    h.step();
    assert!(h.state().is_untitled());
    owner_look(&mut h, style);
    wait_until(&mut h, "new document", |a| a.model().is_some() && !a.is_loading());
    focus_doc(&mut h);
    type_keys(&mut h, src);
    h
}

#[test]
fn spaces_after_a_heading_move_the_caret_in_every_style_and_level() {
    // owner reports: Space at the end of an H1 changed the text but not the screen.
    // - Classic centres H1: epaint aligned the centred row without its trailing whitespace and
    //   put the caret at the text end;
    // - the built-in CSS theme Drake (the owner's theme) centres H1 too.
    // The caret's pixel x must grow with each Space, and a letter typed after them must appear
    // after the gap; for every built-in style (the registry, incl. justified text) and every
    // built-in CSS theme, every heading level, both text backends, a saved file and (styles)
    // an untitled document, with a font size override, dark appearance and the toolbar at
    // the bottom.
    use markise_ui_egui::TextBackend::{Epaint, Parley};
    let styles: Vec<&str> = markise_theme::style::BUILTIN_STYLES.iter().map(|s| s.id).collect();
    assert!(styles.len() >= 5, "{styles:?}");
    let mut fails = Vec::new();
    for backend in [Epaint, Parley] {
        for style in &styles {
            for level in 1..=6 {
                let hashes = "#".repeat(level);
                let what = format!("{backend:?} {style} h{level}");
                // a saved file (heading, an empty line, a paragraph)
                let src = format!("{hashes} Title\n\nPara\n");
                let (mut h, p) = open_with("ws-styles", &src, backend);
                owner_look(&mut h, style);
                key(&mut h, Key::End);
                let at = caret(&mut h);
                assert_eq!(at, level + 6, "{what}");
                if let Err(e) = spaces_and_letter(&mut h, at, &what) {
                    fails.push(e);
                } else {
                    assert_eq!(save(&mut h, &p), format!("{hashes} Title   x\n\nPara\n"), "{what}");
                }
                // an untitled document
                let mut h = untitled_with(backend, style, &format!("{hashes} Title"));
                let at = caret(&mut h);
                if let Err(e) = spaces_and_letter(&mut h, at, &format!("{what} untitled")) {
                    fails.push(e);
                } else {
                    assert!(text(&h).starts_with(&format!("{hashes} Title   x")), "{what} untitled");
                }
            }
        }
    }
    // the built-in CSS themes (complete themes: their own heading alignment — Drake centres
    // H1, the owner's case), dark, opened from a file
    for backend in [Epaint, Parley] {
        for theme in markise_theme::builtin::BUILTIN_THEMES.iter().map(|t| t.id) {
            for level in 1..=6 {
                let hashes = "#".repeat(level);
                let what = format!("{backend:?} builtin:{theme} h{level}");
                let src = format!("{hashes} Title\n\nPara\n");
                let (mut h, p) = open_with("ws-themes", &src, backend);
                let before = h.state().theme().name.clone();
                h.state_mut()
                    .select_theme(markise_theme::catalog::ThemeId::Builtin(theme.into()));
                h.state_mut().set_theme_choice(markise_ui_egui::Appearance::Dark);
                wait_until(&mut h, "theme imported", |a| a.theme().name != before);
                h.step();
                key(&mut h, Key::End);
                let at = caret(&mut h);
                assert_eq!(at, level + 6, "{what}");
                if let Err(e) = spaces_and_letter(&mut h, at, &what) {
                    fails.push(e);
                } else {
                    assert_eq!(save(&mut h, &p), format!("{hashes} Title   x\n\nPara\n"), "{what}");
                }
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// Three Spaces move the drawn caret right each time; a letter after them lands after the gap.
fn spaces_and_letter(h: &mut Harness<'static, MarkiseApp>, at: usize, what: &str) -> Result<(), String> {
    let x0 = caret_screen(h);
    let mut x = x0;
    for i in 1..=3 {
        type_keys(h, " ");
        if caret(h) != at + i {
            return Err(format!("{what}: offset after space {i}: {}", caret(h)));
        }
        let r = caret_screen(h);
        if r.center().y != x.center().y {
            return Err(format!("{what}: caret left the line after space {i}"));
        }
        if r.min.x <= x.min.x + 1.0 {
            return Err(format!("{what}: caret x after space {i}: {} -> {}", x.min.x, r.min.x));
        }
        x = r;
    }
    type_keys(h, "x");
    let after = caret_screen(h);
    if after.min.x <= x.min.x + 2.0 || x.min.x - x0.min.x <= 3.0 {
        return Err(format!("{what}: no visible gap before the letter"));
    }
    Ok(())
}

#[test]
fn cross_block_risks_block_ownership() {
    // ADR 0011: joins, deletions and virtual-paragraph placement take the neighbouring block
    // from the parsed structure and stop at a block without text (an empty quote `>`, an empty
    // list item). Each case failed on main before the ownership query (Codex review of #18).
    use St::*;
    let cases: &[(&str, &str, &[St], &str)] = &[
        // hinted-heading Enter (C43 on `## ` typed as paragraph text)
        (
            "hinted-quote",
            "Before\n\n>\n\n‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            "Before\n\n>\n\nx‸\n",
        ),
        // … after an empty nested container: stays in its own container
        (
            "hinted-nested-quote",
            "> >\n>\n> ‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            "> >\n>\n> x‸\n",
        ),
        (
            "hinted-nested-item",
            "> - a\n>   -\n>\n> ‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            "> - a\n>   -\n>\n> x‸\n",
        ),
        (
            "hinted-quote-in-item",
            "- a\n\n  >\n\n  ‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            "- a\n\n  >\n\n  x‸\n",
        ),
        // … when the empty block is outside the line's own container
        (
            "hinted-item-after-quote",
            ">\n\n- ‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            ">\n\n- x‸\n",
        ),
        (
            "hinted-item-after-quote-item",
            "- >\n- ‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            "- >\n- x‸\n",
        ),
        (
            "hinted-after-quote",
            "> q\n\n‸y\n",
            &[T("## "), K(Key::Delete), K(Key::Enter), T("x")],
            "> q\n\nx‸\n",
        ),
        // list-item paragraph join
        (
            "item-join",
            "- a\n\n  >\n\n  ‸b\n",
            &[K(Key::Backspace)],
            "- a\n\n  >\n\n  ‸b\n",
        ),
        ("item-join-plain", "- a\n\n  ‸b\n", &[K(Key::Backspace)], "- a‸b\n"),
        // final-block deletion (Delete Paragraph / Block)
        ("final-block", "x\n\n>\n\nab‸c\n", &[Cas(Key::P)], "x\n\n>‸\n"),
        // virtual-caret placement
        (
            "virtual-above",
            "Before\n\n>\n\n## ‸Title\n",
            &[K(Key::Enter)],
            "Before\n\n>\n\n## ‸Title\n",
        ),
        (
            "virtual-first-item",
            "> q\n\n- ‸\n- b\n",
            &[K(Key::Enter), T("x")],
            "> q\n\nx‸\n\n- b\n",
        ),
    ];
    let mut fails = Vec::new();
    for (name, src, steps, want) in cases {
        let got = run_marked(&format!("own-{name}"), src, steps);
        if got != *want {
            fails.push(format!("{name}: {src:?} -> {got:?}, want {want:?}"));
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}
