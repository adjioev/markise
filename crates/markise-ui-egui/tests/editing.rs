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
    let b = h.state_mut().source_buffer_mut().unwrap();
    b.push_str("\nAdded in source.\n");
    h.step();
    h.state_mut().toggle_source();
    wait_until(&mut h, "rebuild", |a| {
        !a.is_busy() && a.viewed_text().is_some_and(|t| t.contains("Added"))
    });
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
    // paragraph above (spec 02 §5.2 rules 4 and 5); below a list only the first happens
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
            "- one\n- two\n\nTitle\n",
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
