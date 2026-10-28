//! QA sweep: the real app driven like a user (egui_kittest) across block types and keys.
//!
//! Every scenario opens a file, places the caret, sends platform-like key events (key press,
//! the text it produced, the release — the `editing.rs` helper), and checks the document text,
//! the caret and the saved bytes. After every step the incrementally updated layout model is
//! compared with a full rebuild of the same document.
//!
//! Scenarios that fail on the current code are `#[ignore = "bug: …"]` with the expected
//! behaviour in a comment; run them with `cargo test -p markise-ui-egui --test qa_sweep --
//! --ignored`. Spec references are short IDs from the editing specification (S-x.y, Cxx,
//! M2b-x, M2c-x, §x.y).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use markise_doc::{DocPos, Selection};
use markise_layout::{DocModel, ItemKind};
use markise_ui_egui::{AppOptions, MarkiseApp, ViewMode};

type H = Harness<'static, MarkiseApp>;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markise-qa-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_until(h: &mut H, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
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

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

fn shift_cmd() -> Modifiers {
    Modifiers { shift: true, ..cmd() }
}

/// An open document in the real app.
struct Ed {
    h: H,
    p: PathBuf,
    name: String,
}

impl Ed {
    fn open(name: &str, src: &str) -> Ed {
        Self::open_sized(name, src, 1000.0, 700.0)
    }

    fn open_sized(name: &str, src: &str, w: f32, ht: f32) -> Ed {
        let dir = tmpdir(name);
        let p = dir.join("t.md");
        std::fs::write(&p, src).unwrap();
        let app = MarkiseApp::new(AppOptions {
            open: vec![p.clone()],
            ..Default::default()
        });
        let mut h = Harness::builder()
            .with_size(egui::vec2(w, ht))
            .allow_missing_glyphs()
            .build_ui_state(|ui, app: &mut MarkiseApp| app.ui(ui), app);
        wait_until(&mut h, "load", |a| a.model().is_some() && !a.is_loading());
        // a click into the document surface takes the keyboard focus
        let at = egui::pos2(w * 0.6, 200.0_f32.min(ht * 0.5));
        h.hover_at(at);
        h.drag_at(at);
        h.step();
        h.drop_at(at);
        h.step();
        let mut e = Ed {
            h,
            p,
            name: name.to_string(),
        };
        e.check_model("open");
        e
    }

    fn text(&self) -> String {
        self.h.state().document_text().unwrap()
    }

    /// Puts a collapsed caret at byte `off` (like a click there).
    fn at(&mut self, off: usize) -> &mut Self {
        self.sel(off, off)
    }

    fn sel(&mut self, a: usize, b: usize) -> &mut Self {
        self.h
            .state_mut()
            .viewer()
            .unwrap()
            .ed
            .session
            .set_selection(Selection::single(DocPos::new(a), DocPos::new(b)));
        self.h.step();
        self.h.step();
        self
    }

    /// Clicks at the right end of the first visual line of the item holding `off`.
    fn click_line_end(&mut self, off: usize) -> &mut Self {
        let r = self
            .h
            .state_mut()
            .viewer()
            .unwrap()
            .item_screen_rect(off)
            .expect("item visible");
        let p = egui::pos2(r.right() - 4.0, r.top() + 8.0);
        self.click(p)
    }

    fn click(&mut self, p: egui::Pos2) -> &mut Self {
        self.h.hover_at(p);
        self.h.drag_at(p);
        self.h.step();
        self.h.drop_at(p);
        self.h.step();
        self.h.step();
        self.check_model("click");
        self
    }

    /// Types like the platform does (egui-winit): key press, text, key release.
    fn typ(&mut self, s: &str) -> &mut Self {
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
                self.h.event(ev(k, true));
            }
            self.h.event(Event::Text(c.to_string()));
            if let Some(k) = key {
                self.h.event(ev(k, false));
            }
            self.h.step();
            self.check_model(&format!("typing {c:?}"));
        }
        self
    }

    fn key(&mut self, k: Key) -> &mut Self {
        self.h.key_press(k);
        self.h.step();
        self.check_model(&format!("{k:?}"));
        self
    }

    fn chord(&mut self, m: Modifiers, k: Key) -> &mut Self {
        self.h.key_press_modifiers(m, k);
        self.h.step();
        self.h.step();
        self.check_model(&format!("{m:?}+{k:?}"));
        self
    }

    fn undo(&mut self) -> &mut Self {
        self.chord(cmd(), Key::Z)
    }

    fn redo(&mut self) -> &mut Self {
        self.chord(shift_cmd(), Key::Z)
    }

    /// The incrementally updated model equals a full rebuild of the session's document.
    fn check_model(&mut self, after: &str) {
        let path = self.h.state().document_path().map(|p| p.to_path_buf());
        let Some(v) = self.h.state_mut().viewer() else { return };
        let built = DocModel::build(v.ed.session.doc(), path.as_deref());
        if let Err(e) = v.ed.view.model().diff(&built) {
            panic!("{}: incremental model != rebuild after {after}: {e}", self.name);
        }
    }

    fn caret(&mut self) -> DocPos {
        let sel = self.h.state_mut().viewer().unwrap().ed.session.selection().primary();
        assert!(sel.is_caret(), "{}: expected a caret, got {sel:?}", self.name);
        sel.focus
    }

    fn selection(&mut self) -> (usize, usize) {
        let sel = self.h.state_mut().viewer().unwrap().ed.session.selection().primary();
        (
            sel.anchor.offset.min(sel.focus.offset),
            sel.anchor.offset.max(sel.focus.offset),
        )
    }

    /// The kind of the item holding the caret.
    fn caret_kind(&mut self) -> ItemKind {
        let off = self.caret().offset;
        let v = self.h.state_mut().viewer().unwrap();
        let m = v.ed.view.model();
        let i = m.item_at_offset(off);
        m.items.get(i).map(|it| it.kind.clone()).unwrap_or(ItemKind::Empty)
    }

    fn expect(&mut self, want: &str) -> &mut Self {
        assert_eq!(self.text(), want, "{}: document text", self.name);
        self
    }

    fn expect_caret(&mut self, off: usize, virt: u16) -> &mut Self {
        let c = self.caret();
        assert_eq!((c.offset, c.virt), (off, virt), "{}: caret (offset, virt)", self.name);
        // the caret's item is on screen
        let vis = self.h.state_mut().viewer().unwrap().item_screen_rect(off).is_some();
        assert!(vis, "{}: caret item visible", self.name);
        self
    }

    fn save(&mut self) -> String {
        self.h.key_press_modifiers(cmd(), Key::S);
        let t = Instant::now();
        while self.h.state().is_busy() || self.h.state().is_dirty() {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "{}: save did not finish (dirty={})",
                self.name,
                self.h.state().is_dirty()
            );
            self.h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.h.step();
        std::fs::read_to_string(&self.p).unwrap()
    }

    fn expect_saved(&mut self, want: &str) {
        self.expect(want);
        let got = self.save();
        assert_eq!(got, want, "{}: saved bytes", self.name);
    }

    fn mode(&self) -> ViewMode {
        self.h.state().view_mode()
    }
}

/// Opens `src`, puts the caret at `at`, types `typed`; expects `want` in the session and on disk.
fn type_at(name: &str, src: &str, at: usize, typed: &str, want: &str) {
    let mut e = Ed::open(name, src);
    e.at(at).typ(typed);
    assert_eq!(e.caret().offset, at + typed.len(), "{name}: caret after the typed text");
    e.expect_saved(want);
}

const S: &str = "Hi, мир 👋 ok"; // ASCII punctuation, Cyrillic, an emoji, spaces

// ---------------------------------------------------------------------------------------
// A. Typing a sentence at the start / middle / end of every block type
// ---------------------------------------------------------------------------------------

#[test]
fn a01_paragraph_start_middle_end() {
    let src = "Para text\n\nNext\n";
    type_at("a01s", src, 0, &format!("{S} "), &format!("{S} Para text\n\nNext\n"));
    type_at("a01m", src, 4, &format!(" {S}"), &format!("Para {S} text\n\nNext\n"));
    type_at("a01e", src, 9, &format!(" {S}"), &format!("Para text {S}\n\nNext\n"));
}

#[test]
fn a02_atx_heading_followed_by_paragraph_start() {
    let src = "# Title\n\nNext para\n";
    type_at("a02s", src, 2, &format!("{S} "), &format!("# {S} Title\n\nNext para\n"));
}

// Expected: the space stays in the heading and the caret right after it. Overlaps issue #16.
#[test]
#[ignore = "bug: #16 space at heading end moves the caret to the next block"]
fn a02b_atx_heading_followed_by_paragraph_end() {
    let src = "# Title\n\nNext para\n";
    type_at("a02e", src, 7, &format!(" {S}"), &format!("# Title {S}\n\nNext para\n"));
}

// Expected: the space stays in the heading and the caret right after it. Overlaps issue #16.
#[test]
#[ignore = "bug: #16 space at heading end moves the caret to the next block"]
fn a03_heading_without_blank_line_before_paragraph() {
    type_at(
        "a03",
        "## Title\nNext para\n",
        8,
        &format!(" {S}"),
        &format!("## Title {S}\nNext para\n"),
    );
}

// Expected: the space stays in the heading and the caret right after it. Overlaps issue #16.
#[test]
#[ignore = "bug: #16 space at heading end moves the caret to the next block"]
fn a04_heading_levels_1_to_6_end() {
    for lvl in 1..=6 {
        let hashes = "#".repeat(lvl);
        let src = format!("{hashes} Head\n\nBody\n");
        let end = lvl + 5;
        type_at(
            &format!("a04-{lvl}"),
            &src,
            end,
            " and more",
            &format!("{hashes} Head and more\n\nBody\n"),
        );
    }
}

#[test]
fn a05_heading_last_block_without_newline() {
    type_at("a05", "Intro\n\n## Title", 15, " end", "Intro\n\n## Title end");
}

// Expected: the space stays in the heading and the caret right after it. Overlaps issue #16.
#[test]
#[ignore = "bug: #16 space at heading end moves the caret to the next block"]
fn a06_space_at_heading_end_via_click_keeps_caret_in_heading() {
    // issue #16 scenario reached the way the owner does it: click at the end of the heading
    let mut e = Ed::open("a06", "# Title\n\nNext para\n");
    e.click_line_end(0);
    assert_eq!(e.caret().offset, 7, "click lands at the heading end");
    e.typ(" ");
    e.expect("# Title \n\nNext para\n");
    assert_eq!(e.caret().offset, 8, "caret after the space, still in the heading");
    assert!(matches!(e.caret_kind(), ItemKind::Heading { level: 1 }));
    e.typ("x");
    e.expect_saved("# Title x\n\nNext para\n");
}

#[test]
fn a07_setext_headings_end() {
    type_at(
        "a07a",
        "Title\n=====\n\nBody\n",
        5,
        " more",
        "Title more\n=====\n\nBody\n",
    );
    type_at(
        "a07b",
        "Title\n-----\n\nBody\n",
        5,
        " more",
        "Title more\n-----\n\nBody\n",
    );
}

#[test]
fn a08_list_items_start_end_nested_ordered_task() {
    type_at("a08s", "- milk\n- eggs\n", 2, "oat ", "- oat milk\n- eggs\n");
    type_at(
        "a08e",
        "- milk\n- eggs\n",
        13,
        &format!(" {S}"),
        &format!("- milk\n- eggs {S}\n"),
    );
    type_at("a08n", "- a\n  - inner\n- b\n", 13, " x y", "- a\n  - inner x y\n- b\n");
    type_at("a08o", "1. one\n2. two\n", 13, " и три", "1. one\n2. two и три\n");
    type_at(
        "a08t",
        "- [ ] task\n- [x] done\n",
        10,
        " now",
        "- [ ] task now\n- [x] done\n",
    );
}

#[test]
fn a09_quotes_nested_and_callout() {
    type_at("a09q", "> quoted\n", 2, "very ", "> very quoted\n");
    type_at("a09n", "> > deep\n", 8, " er", "> > deep er\n");
    type_at(
        "a09c",
        "> [!NOTE]\n> body text\n",
        21,
        " 👋 ok",
        "> [!NOTE]\n> body text 👋 ok\n",
    );
}

#[test]
fn a10_table_cells_header_and_body() {
    let src = "| a | b |\n|---|---|\n| x | y |\n";
    type_at("a10h", src, 7, " c", "| a | b c |\n|---|---|\n| x | y |\n");
    type_at("a10b", src, 27, " z", "| a | b |\n|---|---|\n| x | y z |\n");
}

#[test]
fn a11_code_block_content() {
    type_at(
        "a11",
        "Intro\n\n```\ncode\n```\n",
        15,
        " (x) = 1;",
        "Intro\n\n```\ncode (x) = 1;\n```\n",
    );
}

// Expected (§8, §5.4): arrowing or clicking into a math block opens its source for editing. Actual: the caret stays in the block above and typing lands there.
#[test]
#[ignore = "bug: [M2d] math block source cannot be reached by caret (arrows/click)"]
fn a12_math_block_content() {
    let mut e = Ed::open("a12", "Intro\n\n$$\nx\n$$\n");
    e.at(5).key(Key::ArrowDown);
    e.key(Key::End).typ(" + y");
    e.expect_saved("Intro\n\n$$\nx + y\n$$\n");
}

// Expected (§1.6, §5.4): front matter is editable in place. Actual: Up/click from below never enters it; typing lands in the paragraph.
#[test]
#[ignore = "bug: [M2d] front matter cannot be reached by caret (arrows/click)"]
fn a13_front_matter() {
    let mut e = Ed::open("a13", "---\ntitle: a\n---\n\nBody\n");
    e.at(18).key(Key::ArrowUp);
    e.key(Key::End).typ(" b");
    e.expect_saved("---\ntitle: a b\n---\n\nBody\n");
}

#[test]
fn a14_footnote_definition_text() {
    type_at(
        "a14",
        "Text[^1]\n\n[^1]: note\n",
        20,
        " more",
        "Text[^1]\n\n[^1]: note more\n",
    );
}

#[test]
fn a15_empty_document_sentence() {
    let mut e = Ed::open("a15", "");
    e.typ(S);
    e.expect_saved(S);
}

#[test]
fn a16_paragraph_after_hr_start() {
    type_at("a16", "a\n\n---\n\nb\n", 8, "x ", "a\n\n---\n\nx b\n");
}

// ---------------------------------------------------------------------------------------
// B. Enter / Shift+Enter
// ---------------------------------------------------------------------------------------

#[test]
fn b01_enter_at_paragraph_end_virtual_paragraph_then_type() {
    let mut e = Ed::open("b01", "Para\n\nNext\n");
    e.at(4).key(Key::Enter);
    e.expect("Para\n\nNext\n").expect_caret(4, 1);
    e.typ("x y");
    e.expect_saved("Para\n\nx y\n\nNext\n");
}

#[test]
fn b02_enter_at_heading_end_creates_paragraph() {
    // overlaps issue #16 (Enter at the end of a heading)
    let mut e = Ed::open("b02", "# Title\n\nNext\n");
    e.at(7).key(Key::Enter);
    e.expect("# Title\n\nNext\n").expect_caret(7, 1);
    e.typ("x");
    e.expect_saved("# Title\n\nx\n\nNext\n");
}

#[test]
fn b03_enter_at_heading_end_via_click() {
    // overlaps issue #16: the same reached with a click at the end of the heading
    let mut e = Ed::open("b03", "# Title\n\nNext\n");
    e.click_line_end(0).key(Key::Enter).typ("x");
    e.expect_saved("# Title\n\nx\n\nNext\n");
}

#[test]
fn b04_enter_at_end_of_last_heading_without_newline() {
    let mut e = Ed::open("b04", "Intro\n\n## Title");
    e.at(15).key(Key::Enter).typ("x");
    e.expect_saved("Intro\n\n## Title\n\nx");
}

#[test]
fn b05_enter_in_middle_of_heading_splits() {
    // C7: the part after the caret becomes a paragraph
    let mut e = Ed::open("b05", "# Title\n");
    e.at(4).key(Key::Enter);
    e.expect("# Ti\n\ntle\n");
    assert_eq!(e.caret_kind(), ItemKind::Paragraph);
    e.typ("x");
    e.expect_saved("# Ti\n\nxtle\n");
}

#[test]
fn b06_enter_at_start_of_paragraph_opens_paragraph_above() {
    // M2b-1
    let mut e = Ed::open("b06", "A\n\nB\n");
    e.at(3).key(Key::Enter).typ("x");
    e.expect_saved("A\n\nx\n\nB\n");
}

#[test]
fn b07_enter_at_document_start_does_nothing() {
    // M2b-1: at the document start Enter does nothing
    let mut e = Ed::open("b07", "A\n");
    e.at(0).key(Key::Enter);
    e.expect("A\n").expect_caret(0, 0);
}

#[test]
fn b08_enter_on_virtual_paragraph_does_nothing() {
    let mut e = Ed::open("b08", "A\n\nB\n");
    e.at(1).key(Key::Enter).key(Key::Enter);
    e.expect("A\n\nB\n").expect_caret(1, 1);
    e.typ("x");
    e.expect_saved("A\n\nx\n\nB\n");
}

// Expected (C43): `## ` + Enter turns the heading into an empty paragraph; typing `x` gives `x`. Actual: nothing happens, typing gives `## x`.
#[test]
fn b09_enter_on_empty_heading_becomes_paragraph() {
    // C43
    let mut e = Ed::open("b09", "Intro\n\n## \n");
    e.at(10).key(Key::Enter).typ("x");
    e.expect_saved("Intro\n\nx\n");
}

#[test]
fn b10_enter_in_quote_paragraph_end() {
    let mut e = Ed::open("b10", "> q\n");
    e.at(3).key(Key::Enter).typ("x");
    let t = e.text();
    assert!(
        t == "> q\n>\n> x\n" || t == "> q\n> \n> x\n",
        "new paragraph inside the quote: {t:?}"
    );
}

#[test]
fn b11_enter_in_code_block_newline() {
    let mut e = Ed::open("b11", "```\ncode\n```\n");
    e.at(8).key(Key::Enter).typ("x");
    e.expect_saved("```\ncode\nx\n```\n");
}

// Expected (§5.1, §7): paragraph after the block. Actual: no-op, typing goes into the code.
#[test]
#[ignore = "bug: [M2d] Cmd+Enter at the end of code does not insert a paragraph after"]
fn b12_cmd_enter_at_code_end_inserts_paragraph_after() {
    // §5.1 / §7: Cmd+Enter at the end of the last code line
    let mut e = Ed::open("b12", "```\ncode\n```\n");
    e.at(8).chord(cmd(), Key::Enter).typ("x");
    e.expect_saved("```\ncode\n```\n\nx\n");
}

// Expected (§5.1): move to the cell below, selecting it.
#[test]
#[ignore = "bug: [M2d] Enter in a table cell does nothing"]
fn b13_enter_in_table_cell_moves_down() {
    // §5.1: Enter moves to the cell below (selecting it); typing replaces it
    let mut e = Ed::open("b13", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(3).key(Key::Enter).typ("Z");
    e.expect_saved("| a | b |\n|---|---|\n| Z | y |\n");
}

// Expected (§5.1): Enter inside front matter inserts a newline there.
#[test]
#[ignore = "bug: [M2d] front matter cannot be reached by caret"]
fn b14_enter_in_front_matter_newline() {
    let mut e = Ed::open("b14", "---\ntitle: a\n---\n\nBody\n");
    e.at(18).key(Key::ArrowUp).key(Key::End).key(Key::Enter).typ("k: v");
    e.expect_saved("---\ntitle: a\nk: v\n---\n\nBody\n");
}

#[test]
fn b15_shift_enter_soft_break_in_paragraph() {
    let mut e = Ed::open("b15", "Paratext\n");
    e.at(4).chord(shift(), Key::Enter).typ("x");
    e.expect_saved("Para\nxtext\n");
}

#[test]
fn b16_shift_enter_in_atx_heading_inserts_br() {
    // C44
    let mut e = Ed::open("b16", "# Long title\n");
    e.at(6).chord(shift(), Key::Enter);
    e.expect("# Long<br/> title\n");
    assert!(matches!(e.caret_kind(), ItemKind::Heading { level: 1 }));
}

#[test]
fn b17_shift_enter_on_blank_soft_line_acts_as_enter() {
    // C42: `foo⏎|bar` + Shift+Enter → `foo⏎⏎bar`
    let mut e = Ed::open("b17", "foo\nbar\n");
    e.at(4).chord(shift(), Key::Enter);
    e.expect("foo\n\nbar\n").expect_caret(5, 0);
}

// Expected (§5.1, §6): inserts `<br/>` in the cell.
#[test]
#[ignore = "bug: [M2d] Shift+Enter in a table cell does nothing"]
fn b18_shift_enter_in_table_cell_inserts_br() {
    let mut e = Ed::open("b18", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(3).chord(shift(), Key::Enter).typ("c");
    e.expect_saved("| a<br/>c | b |\n|---|---|\n| x | y |\n");
}

#[test]
fn b19_enter_in_middle_of_bold_keeps_style() {
    // S-3.1
    let mut e = Ed::open("b19", "This is **very bold** text\n");
    e.at(14).key(Key::Enter);
    e.expect_saved("This is **very**\n\n**bold** text\n");
}

#[test]
fn b20_enter_in_list_item_then_empty_item_leaves_list() {
    // S-2.2 / S-5.1
    let mut e = Ed::open("b20", "- milk\n");
    e.at(6)
        .key(Key::Enter)
        .typ("eggs")
        .key(Key::Enter)
        .key(Key::Enter)
        .typ("after");
    e.expect_saved("- milk\n- eggs\n\nafter\n");
}

#[test]
fn b21_enter_on_empty_middle_list_item_splits_list() {
    // C45
    let mut e = Ed::open("b21", "- a\n- \n- c\n");
    e.at(6).key(Key::Enter);
    e.expect("- a\n\n\n- c\n");
    e.typ("x");
    e.expect_saved("- a\n\nx\n\n- c\n");
}

#[test]
fn b22_enter_on_nested_empty_item_outdents() {
    // S-5.2
    let mut e = Ed::open("b22", "- a\n");
    e.at(3).key(Key::Enter).key(Key::Tab);
    e.expect("- a\n  - \n");
    e.key(Key::Enter);
    e.expect("- a\n- \n");
}

#[test]
fn b23_enter_on_empty_quote_paragraph_leaves_quote() {
    let mut e = Ed::open("b23", "> q\n");
    e.at(3).key(Key::Enter).key(Key::Enter).typ("out");
    e.expect_saved("> q\n\nout\n");
}

#[test]
fn b24_enter_after_emoji_and_cyrillic() {
    let mut e = Ed::open("b24", "мир 👋\n");
    let end = "мир 👋".len();
    e.at(end).key(Key::Enter).typ("да");
    e.expect_saved("мир 👋\n\nда\n");
}

#[test]
fn b25_enter_at_end_of_setext_heading() {
    let mut e = Ed::open("b25", "Title\n=====\n\nBody\n");
    e.at(5).key(Key::Enter).typ("x");
    e.expect_saved("Title\n=====\n\nx\n\nBody\n");
}

#[test]
fn b26_enter_at_end_of_closed_atx_heading() {
    let mut e = Ed::open("b26", "# Title #\n\nBody\n");
    e.at(7).key(Key::Enter).typ("x");
    e.expect_saved("# Title #\n\nx\n\nBody\n");
}

// ---------------------------------------------------------------------------------------
// C. Backspace / Delete
// ---------------------------------------------------------------------------------------

#[test]
fn c01_backspace_at_heading_start_makes_paragraph() {
    // S-5.3
    let mut e = Ed::open("c01", "## Notes\n");
    e.at(3).key(Key::Backspace);
    e.expect_saved("Notes\n");
}

#[test]
fn c02_backspace_merges_paragraphs() {
    let mut e = Ed::open("c02", "A\n\nB\n");
    e.at(3).key(Key::Backspace);
    e.expect("AB\n").expect_caret(1, 0);
    e.typ("x");
    e.expect_saved("AxB\n");
}

#[test]
fn c03_backspace_merges_paragraph_into_heading() {
    let mut e = Ed::open("c03", "# A\n\nB\n");
    e.at(5).key(Key::Backspace);
    e.expect_saved("# AB\n");
}

// Expected (§5.2 rule 2): the HR is deleted. Forward Delete before an HR works (c12).
#[test]
fn c04_backspace_after_hr_deletes_hr() {
    // §5.2 rule 2
    let mut e = Ed::open("c04", "a\n\n---\n\nb\n");
    e.at(8).key(Key::Backspace);
    e.expect_saved("a\n\nb\n");
}

#[test]
fn c05_backspace_at_second_item_joins_previous_item() {
    // S-5.4
    let mut e = Ed::open("c05", "- a\n- b\n");
    e.at(6).key(Key::Backspace);
    e.expect_saved("- a\n\n  b\n");
}

#[test]
fn c06_backspace_at_first_item_unwraps() {
    // C46
    let mut e = Ed::open("c06", "- a\n- b\n");
    e.at(2).key(Key::Backspace);
    e.expect("a\n\n- b\n").expect_caret(0, 0);
}

#[test]
fn c07_backspace_at_quote_start_unwraps() {
    // C46
    let mut e = Ed::open("c07", "> q\n");
    e.at(2).key(Key::Backspace);
    e.expect_saved("q\n");
}

// Expected (§5.2): the code block becomes a paragraph.
#[test]
#[ignore = "bug: [M2d] Backspace at the start of a one-line code block does nothing"]
fn c08_backspace_at_single_line_code_start_makes_paragraph() {
    // §5.2 code-block rule
    let mut e = Ed::open("c08", "Intro\n\n```\ncode\n```\n");
    e.at(11).key(Key::Backspace);
    e.expect_saved("Intro\n\ncode\n");
}

#[test]
fn c09_backspace_in_first_table_cell_is_noop() {
    // §5.2 rule 1
    let src = "Intro\n\n| a | b |\n|---|---|\n| x | y |\n";
    let mut e = Ed::open("c09", src);
    e.at(9).key(Key::Backspace);
    e.expect(src);
}

#[test]
fn c10_delete_at_paragraph_end_merges() {
    // C48
    let mut e = Ed::open("c10", "a\n\nb\n");
    e.at(1).key(Key::Delete);
    e.expect("ab\n").expect_caret(1, 0);
}

#[test]
fn c11_delete_before_heading_merges_text() {
    let mut e = Ed::open("c11", "a\n\n## B\n");
    e.at(1).key(Key::Delete);
    e.expect_saved("aB\n");
}

#[test]
fn c12_delete_before_hr_deletes_it() {
    let mut e = Ed::open("c12", "a\n\n---\n\nb\n");
    e.at(1).key(Key::Delete);
    e.expect_saved("a\n\nb\n");
}

#[test]
fn c13_delete_before_code_is_noop() {
    let src = "a\n\n```\ncode\n```\n";
    let mut e = Ed::open("c13", src);
    e.at(1).key(Key::Delete);
    e.expect(src).expect_caret(1, 0);
}

#[test]
fn c14_backspace_on_virtual_paragraph_returns() {
    let mut e = Ed::open("c14", "Para\n\nNext\n");
    e.at(4).key(Key::Enter).key(Key::Backspace);
    e.expect("Para\n\nNext\n").expect_caret(4, 0);
    assert!(!e.h.state().is_dirty(), "no change");
}

#[test]
fn c15_backspace_deletes_whole_graphemes() {
    let fam = "👨\u{200d}👩\u{200d}👧";
    let src = format!("мир{fam}\n");
    let mut e = Ed::open("c15", &src);
    e.at(src.len() - 1).key(Key::Backspace);
    e.expect("мир\n").key(Key::Backspace);
    e.expect_saved("ми\n");
}

#[test]
fn c16_delete_forward_cyrillic_and_emoji() {
    let mut e = Ed::open("c16", "👋да\n");
    e.at(0).key(Key::Delete);
    e.expect("да\n").key(Key::Delete);
    e.expect_saved("а\n");
}

#[test]
fn c17_backspace_at_second_quote_paragraph_merges() {
    let mut e = Ed::open("c17", "> a\n>\n> b\n");
    e.at(8).key(Key::Backspace);
    e.expect_saved("> ab\n");
}

#[test]
fn c18_backspace_after_empty_paragraph_region_merges_heading() {
    // two headings: Backspace at the second converts it to a paragraph first (rule 4)
    let mut e = Ed::open("c18", "# A\n\n# B\n");
    e.at(7).key(Key::Backspace);
    e.expect("# A\n\nB\n");
    e.key(Key::Backspace);
    e.expect_saved("# AB\n");
}

#[test]
fn c19_delete_at_end_of_list_item_joins_next() {
    // C48 example
    let mut e = Ed::open("c19", "- a\n- b\n  - c\n");
    e.at(3).key(Key::Delete);
    e.expect_saved("- ab\n  - c\n");
}

#[test]
fn c20_backspace_in_middle_of_word_in_heading() {
    let mut e = Ed::open("c20", "# Title\n\nNext\n");
    e.at(5).key(Key::Backspace);
    e.expect("# Tile\n\nNext\n").expect_caret(4, 0);
}

// ---------------------------------------------------------------------------------------
// D. Tab / Shift+Tab
// ---------------------------------------------------------------------------------------

#[test]
fn d01_tab_nests_list_item_and_shift_tab_outdents() {
    let mut e = Ed::open("d01", "- a\n- b\n");
    e.at(7).key(Key::Tab);
    e.expect("- a\n  - b\n");
    e.chord(shift(), Key::Tab);
    e.expect_saved("- a\n- b\n");
}

// Expected (§5.3): next cell, contents selected. Actual: `| a\tZ |`.
#[test]
#[ignore = "bug: [M2d] Tab in a table cell inserts a literal tab into the cell"]
fn d02_tab_in_table_moves_to_next_cell_selecting_it() {
    // §5.3: Tab → next cell, selecting its contents; typing replaces it
    let mut e = Ed::open("d02", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(3).key(Key::Tab).typ("Z");
    e.expect_saved("| a | Z |\n|---|---|\n| x | y |\n");
}

// Expected (S-6.1): a new row, caret in its first cell.
#[test]
#[ignore = "bug: [M2d] Tab in the last table cell inserts a tab instead of a new row"]
fn d03_tab_in_last_cell_adds_row() {
    // S-6.1
    let mut e = Ed::open("d03", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(27).key(Key::Tab).typ("n");
    let t = e.text();
    assert_eq!(t.lines().count(), 4, "a new row: {t:?}");
    assert!(
        t.lines().nth(3).unwrap().starts_with("| n"),
        "caret in the new row's first cell: {t:?}"
    );
}

// Expected (§5.3): previous cell, contents selected.
#[test]
#[ignore = "bug: [M2d] Shift+Tab in a table cell does not move to the previous cell"]
fn d04_shift_tab_in_table_moves_back() {
    let mut e = Ed::open("d04", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(7).chord(shift(), Key::Tab).typ("Z");
    e.expect_saved("| Z | b |\n|---|---|\n| x | y |\n");
}

#[test]
fn d05_tab_in_paragraph_inserts_tab() {
    // §5.3 rule 5
    let mut e = Ed::open("d05", "Para\n");
    e.at(4).key(Key::Tab).typ("x");
    e.expect_saved("Para\tx\n");
}

// Expected (§5.3 rule 6): the line is indented.
#[test]
#[ignore = "bug: [M2d] Tab in a code block does nothing"]
fn d06_tab_in_code_block_indents() {
    let mut e = Ed::open("d06", "```\ncode\n```\n");
    e.at(4).key(Key::Tab);
    let t = e.text();
    assert!(
        t == "```\n    code\n```\n" || t == "```\n\tcode\n```\n",
        "code indented: {t:?}"
    );
}

#[test]
fn d07_tab_on_first_item_keeps_document_valid() {
    // the first item cannot nest; Tab must not corrupt the list
    let mut e = Ed::open("d07", "- a\n- b\n");
    e.at(3).key(Key::Tab);
    let t = e.text();
    assert!(t == "- a\n- b\n" || t == "- a\t\n- b\n", "{t:?}");
}

// ---------------------------------------------------------------------------------------
// E. Navigation across block boundaries
// ---------------------------------------------------------------------------------------

#[test]
fn e01_right_at_block_end_enters_next_block_start() {
    let mut e = Ed::open("e01", "# Title\n\nNext\n");
    e.at(7).key(Key::ArrowRight);
    e.expect_caret(9, 0);
    e.key(Key::ArrowLeft);
    e.expect_caret(7, 0);
}

#[test]
fn e02_down_and_up_between_paragraphs() {
    let mut e = Ed::open("e02", "First line\n\nSecond line\n");
    e.at(3).key(Key::ArrowDown);
    let c = e.caret().offset;
    assert!((12..=23).contains(&c), "caret in the second paragraph: {c}");
    e.key(Key::ArrowUp);
    let c = e.caret().offset;
    assert!(c <= 10, "back in the first: {c}");
}

#[test]
fn e03_home_end_in_heading_skip_markers() {
    let mut e = Ed::open("e03", "## Title\n");
    e.at(5).key(Key::Home);
    e.expect_caret(3, 0);
    e.key(Key::End);
    e.expect_caret(8, 0);
}

#[test]
fn e04_home_end_in_list_item_and_quote() {
    let mut e = Ed::open("e04", "- item\n\n> quote\n");
    e.at(4).key(Key::Home);
    e.expect_caret(2, 0);
    e.at(12).key(Key::End);
    e.expect_caret(15, 0);
    e.key(Key::Home);
    e.expect_caret(10, 0);
}

// Expected (C53): caret on a virtual paragraph after the last block; typing appends a new paragraph. Actual: the caret stays.
#[test]
#[ignore = "bug: Down at the last block does not open the virtual paragraph (C53)"]
fn e05_down_at_last_block_virtual_paragraph() {
    // C53
    let mut e = Ed::open("e05", "a\n");
    e.at(1).key(Key::ArrowDown);
    e.expect("a\n").expect_caret(1, 1);
    e.key(Key::ArrowUp);
    e.expect_caret(1, 0);
    e.key(Key::ArrowDown).typ("x");
    e.expect_saved("a\n\nx\n");
}

// Expected (C53): caret on the virtual paragraph (virt = 1).
#[test]
#[ignore = "bug: Right at the document end does not open the virtual paragraph (C53)"]
fn e06_right_at_document_end_virtual_paragraph() {
    // C53
    let mut e = Ed::open("e06", "a\n");
    e.at(1).key(Key::ArrowRight);
    e.expect_caret(1, 1);
}

// Expected (C53 example): virtual paragraph below the table; `x` gives `...\n\nx`. Actual: typed into the cell.
#[test]
#[ignore = "bug: Down from the last table row does not escape the table (C53)"]
fn e07_down_from_last_table_row_virtual_paragraph() {
    // C53 example
    let mut e = Ed::open("e07", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(22).key(Key::ArrowDown).typ("x");
    e.expect_saved("| a | b |\n|---|---|\n| x | y |\n\nx\n");
}

// Expected (C53, §5.4): virtual paragraph after the block. Actual: typed into the code.
#[test]
#[ignore = "bug: Down from the last code line does not escape the code block (C53)"]
fn e08_down_from_last_code_line_virtual_paragraph() {
    let mut e = Ed::open("e08", "```\ncode\n```\n");
    e.at(6).key(Key::ArrowDown).typ("x");
    e.expect_saved("```\ncode\n```\n\nx\n");
}

#[test]
fn e09_down_into_code_block_first_line_up_into_last() {
    let mut e = Ed::open("e09", "above\n\n```\none\ntwo\n```\n\nbelow\n");
    e.at(2).key(Key::ArrowDown);
    let c = e.caret().offset;
    assert!((11..=14).contains(&c), "first code line: {c}");
    e.at(29).key(Key::ArrowUp);
    let c = e.caret().offset;
    assert!((15..=18).contains(&c), "last code line: {c}");
}

#[test]
fn e10_left_at_code_start_exits_to_previous_block() {
    let mut e = Ed::open("e10", "above\n\n```\ncode\n```\n");
    e.at(11).key(Key::ArrowLeft);
    e.expect_caret(5, 0);
}

#[test]
fn e11_arrows_cross_table_rows() {
    let mut e = Ed::open("e11", "| a | b |\n|---|---|\n| x | y |\n");
    e.at(2).key(Key::ArrowDown);
    let c = e.caret().offset;
    assert!((20..=24).contains(&c), "first body row: {c}");
}

#[test]
fn e12_page_down_and_up_move_caret_and_keep_it_visible() {
    let body: String = (0..120).map(|i| format!("Paragraph {i} text.\n\n")).collect();
    let mut e = Ed::open("e12", &body);
    e.at(0).key(Key::PageDown);
    let c = e.caret().offset;
    assert!(c > 100, "page down moved the caret: {c}");
    let vis = e.h.state_mut().viewer().unwrap().item_screen_rect(c).is_some();
    assert!(vis, "caret visible after Page Down");
    e.key(Key::PageUp);
    assert!(e.caret().offset < c, "page up moved back");
    e.typ("x");
    let t = e.text();
    assert_eq!(t.len(), body.len() + 1);
}

#[test]
fn e13_cmd_down_then_type_appends() {
    let mut e = Ed::open("e13", "a\n\nb\n");
    e.at(0).chord(cmd(), Key::ArrowDown).typ("c");
    e.expect_saved("a\n\nbc\n");
}

#[test]
fn e14_left_right_step_over_emoji() {
    let mut e = Ed::open("e14", "a👋b\n");
    e.at(1).key(Key::ArrowRight);
    e.expect_caret(5, 0);
    e.key(Key::ArrowLeft);
    e.expect_caret(1, 0);
}

// Expected (C53): two Downs from the last paragraph end on its virtual paragraph.
#[test]
#[ignore = "bug: Down at the last block does not open the virtual paragraph (C53)"]
fn e15_up_from_first_line_and_down_from_virtual_stay_valid() {
    let mut e = Ed::open("e15", "a\n\nb\n");
    e.at(0).key(Key::ArrowUp);
    let c = e.caret();
    assert_eq!((c.offset, c.virt), (0, 0));
    e.at(4).key(Key::ArrowDown).key(Key::ArrowDown);
    e.expect_caret(4, 1);
}

#[test]
fn e16_right_from_heading_end_over_hidden_markers_into_list() {
    let mut e = Ed::open("e16", "# T\n\n- item\n");
    e.at(3).key(Key::ArrowRight);
    e.expect_caret(7, 0);
}

#[test]
fn e17_click_below_last_block_places_caret_at_end() {
    let mut e = Ed::open("e17", "a\n\nlast para\n");
    e.click(egui::pos2(500.0, 650.0));
    let c = e.caret();
    assert!(c.offset == 12, "caret at the end: {c:?}");
    e.typ("x");
    let t = e.text();
    assert!(t == "a\n\nlast parax\n" || t == "a\n\nlast para\n\nx\n", "{t:?}");
}

#[test]
fn e18_click_below_last_heading_then_type() {
    let mut e = Ed::open("e18", "a\n\n# Last\n");
    e.click(egui::pos2(500.0, 650.0)).typ("x");
    let t = e.text();
    assert!(t == "a\n\n# Lastx\n" || t == "a\n\n# Last\n\nx\n", "{t:?}");
}

// ---------------------------------------------------------------------------------------
// F. Selection + typing
// ---------------------------------------------------------------------------------------

#[test]
fn f01_shift_arrow_selection_replaced_by_typing() {
    let mut e = Ed::open("f01", "Hello world\n");
    e.at(0);
    for _ in 0..5 {
        e.chord(shift(), Key::ArrowRight);
    }
    assert_eq!(e.selection(), (0, 5));
    e.typ("Bye");
    e.expect_saved("Bye world\n");
}

#[test]
fn f02_selection_across_paragraphs_replaced() {
    let mut e = Ed::open("f02", "One two\n\nThree four\n");
    e.sel(4, 15).typ("X");
    e.expect_saved("One Xfour\n");
}

// Expected (M2b-3: deliberate selections are deleted as selected): `new`. Actual: `# new\n` (the first block's heading marker and the final newline survive).
#[test]
#[ignore = "bug: typing over Select All keeps the first heading's `# `"]
fn f03_select_all_then_type() {
    let mut e = Ed::open("f03", "# H\n\n- a\n- b\n\n> q\n");
    e.chord(cmd(), Key::A).typ("new");
    e.expect_saved("new");
}

#[test]
fn f04_select_all_backspace_then_type() {
    let mut e = Ed::open("f04", "a\n\nb\n");
    e.chord(cmd(), Key::A).key(Key::Backspace);
    let t = e.text();
    assert!(t.is_empty() || t == "\n", "{t:?}");
    e.typ("z");
    let t = e.text();
    assert!(t == "z" || t == "z\n", "{t:?}");
}

#[test]
fn f05_selection_in_heading_replaced_keeps_heading() {
    let mut e = Ed::open("f05", "## Old title\n\nBody\n");
    e.sel(3, 6).typ("New");
    e.expect_saved("## New title\n\nBody\n");
}

#[test]
fn f06_selection_from_heading_into_paragraph_replaced() {
    let mut e = Ed::open("f06", "# Head\n\nBody text\n");
    e.sel(4, 12).typ("-");
    e.expect_saved("# He- text\n");
}

#[test]
fn f07_shift_end_selection_and_delete_in_list() {
    let mut e = Ed::open("f07", "- one two\n- three\n");
    e.at(6).chord(shift(), Key::End).key(Key::Backspace);
    e.expect_saved("- one \n- three\n");
}

#[test]
fn f08_selection_replaced_with_space() {
    let mut e = Ed::open("f08", "# a-b\n\nnext\n");
    e.sel(3, 4).typ(" ");
    e.expect("# a b\n\nnext\n").expect_caret(4, 0);
}

#[test]
fn f09_cmd_shift_down_select_to_end_and_type() {
    let mut e = Ed::open("f09", "a\n\nb\n\nc\n");
    e.at(3).chord(shift_cmd(), Key::ArrowDown).typ("z");
    e.expect_saved("a\n\nz\n");
}

// ---------------------------------------------------------------------------------------
// G. Format commands, toolbar, undo / redo
// ---------------------------------------------------------------------------------------

#[test]
fn g01_cmd_b_toggles_strong_on_selection() {
    let mut e = Ed::open("g01", "make bold here\n");
    e.sel(5, 9).chord(cmd(), Key::B);
    e.expect("make **bold** here\n");
    e.chord(cmd(), Key::B);
    e.expect("make bold here\n");
    e.undo();
    e.expect("make **bold** here\n");
    e.undo();
    e.expect("make bold here\n");
    e.redo();
    e.expect_saved("make **bold** here\n");
}

#[test]
fn g02_cmd_b_without_selection_inserts_pair() {
    // §3.3
    let mut e = Ed::open("g02", "a\n");
    e.at(1).typ(" ").chord(cmd(), Key::B);
    e.expect("a ****\n").expect_caret(4, 0);
    e.typ("x y");
    e.expect_saved("a **x y**\n");
}

#[test]
fn g03_cmd_i_on_selection() {
    let mut e = Ed::open("g03", "some words\n");
    e.sel(0, 4).chord(cmd(), Key::I);
    e.expect_saved("*some* words\n");
}

// Expected (§3.3): `[]()` with the caret between the brackets. Actual: typing gives `see t[]()`.
#[test]
#[ignore = "bug: Cmd+K without selection leaves the caret before `[]()`"]
fn g04_cmd_k_without_selection_inserts_link() {
    // §3.3
    let mut e = Ed::open("g04", "see\n");
    e.at(3).typ(" ").chord(cmd(), Key::K);
    let t = e.text();
    assert!(t.starts_with("see []("), "{t:?}");
    e.typ("t");
    assert!(
        e.text().starts_with("see [t]("),
        "caret inside the brackets: {:?}",
        e.text()
    );
}

#[test]
fn g05_cmd_k_on_selection() {
    let mut e = Ed::open("g05", "a link b\n");
    e.sel(2, 6).chord(cmd(), Key::K);
    let t = e.text();
    assert!(t.starts_with("a [link]("), "{t:?}");
}

#[test]
fn g06_cmd_b_trims_selection_whitespace() {
    // C49
    let mut e = Ed::open("g06", "foo bar baz\n");
    e.sel(3, 8).chord(cmd(), Key::B);
    e.expect_saved("foo **bar** baz\n");
}

#[test]
fn g07_cmd_i_across_paragraphs() {
    // C49
    let mut e = Ed::open("g07", "a\n\nb\n");
    e.sel(0, 4).chord(cmd(), Key::I);
    e.expect("*a*\n\n*b*\n");
    e.chord(cmd(), Key::I);
    e.expect("a\n\nb\n");
}

#[test]
fn g08_cmd_b_in_heading_and_list() {
    let mut e = Ed::open("g08", "# Big title\n\n- item one\n");
    e.sel(6, 11).chord(cmd(), Key::B);
    e.expect("# Big **title**\n\n- item one\n");
    let s = e.text().find("one").unwrap();
    e.sel(s, s + 3).chord(cmd(), Key::B);
    e.expect_saved("# Big **title**\n\n- item **one**\n");
}

#[test]
fn g09_toolbar_strong_then_undo() {
    let mut e = Ed::open("g09", "plain word\n");
    e.h.state_mut().set_show_toolbar(true);
    e.h.step();
    e.sel(6, 10);
    e.h.get_by_label("Strong").click();
    e.h.step();
    e.h.step();
    e.expect("plain **word**\n");
    e.undo();
    e.expect_saved("plain word\n");
}

#[test]
fn g10_heading_shortcut_undo_redo_keeps_caret() {
    let mut e = Ed::open("g10", "Title\n");
    e.at(3).chord(cmd(), Key::Num2);
    e.expect("## Title\n");
    e.typ("x");
    e.expect("## Titxle\n");
    e.undo().undo();
    e.expect("Title\n").expect_caret(3, 0);
    e.redo();
    e.expect("## Title\n");
}

#[test]
fn g11_typing_after_bold_close_stays_outside() {
    let mut e = Ed::open("g11", "a **b** c\n");
    e.at(7).typ("x");
    let t = e.text();
    assert!(t == "a **b**x c\n" || t == "a **bx** c\n", "{t:?}");
}

// ---------------------------------------------------------------------------------------
// H. Input rules
// ---------------------------------------------------------------------------------------

#[test]
fn h01_hash_heading_on_enter() {
    // S-2.1
    let mut e = Ed::open("h01", "");
    e.typ("## Plan").key(Key::Enter);
    assert!(
        e.h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .view
            .model()
            .items
            .get(0)
            .is_some_and(|it| it.kind == ItemKind::Heading { level: 2 })
    );
    e.typ("next");
    e.expect_saved("## Plan\n\nnext");
}

#[test]
fn h02_bullet_list_flow() {
    // S-2.2
    let mut e = Ed::open("h02", "");
    e.typ("- milk")
        .key(Key::Enter)
        .typ("eggs")
        .key(Key::Enter)
        .key(Key::Enter)
        .typ("done");
    e.expect_saved("- milk\n- eggs\n\ndone");
}

#[test]
fn h03_ordered_list_start_number() {
    // S-2.3
    let mut e = Ed::open("h03", "");
    e.typ("3. a").key(Key::Enter).typ("b");
    e.expect_saved("3. a\n4. b");
}

#[test]
fn h04_quote_rule() {
    let mut e = Ed::open("h04", "");
    e.typ("> said it");
    e.expect_saved("> said it");
}

// Expected (§2.1, §4.2): `- [ ] buy`. Actual: `- [ ] ] buy` — the auto-paired `]` is consumed by the task rule, so the typed `]` is inserted instead of stepping over.
#[test]
fn h05_task_rule() {
    let mut e = Ed::open("h05", "");
    e.typ("- [ ] buy");
    e.expect_saved("- [ ] buy");
}

#[test]
fn h06_code_fence_on_enter() {
    // S-2.5
    let mut e = Ed::open("h06", "");
    e.typ("```python").key(Key::Enter);
    assert!(matches!(e.caret_kind(), ItemKind::Code { .. }), "caret in code");
    e.typ("x = 1");
    let t = e.text();
    assert!(t.starts_with("```python\nx = 1\n```"), "{t:?}");
}

// Expected (§2.2): math block in edit state. Actual: paragraph `$$` and a new paragraph.
#[test]
#[ignore = "bug: [M2d] `$$` + Enter does not create a math block"]
fn h07_math_block_on_enter() {
    let mut e = Ed::open("h07", "");
    e.typ("$$").key(Key::Enter).typ("x");
    let t = e.text();
    assert!(t.starts_with("$$\nx\n$$"), "{t:?}");
}

// Expected (S-2.4): front matter with the caret inside. Actual: a thematic break.
#[test]
fn h08_front_matter_at_doc_start() {
    // S-2.4
    let mut e = Ed::open("h08", "");
    e.typ("---").key(Key::Enter);
    assert_eq!(e.caret_kind(), ItemKind::FrontMatter);
}

#[test]
fn h09_hr_in_middle() {
    // S-2.4
    let mut e = Ed::open("h09", "a\n");
    e.at(1).key(Key::Enter).typ("---").key(Key::Enter);
    let t = e.text();
    assert!(t.starts_with("a\n\n---\n"), "{t:?}");
    assert!(
        e.h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .view
            .model()
            .items
            .iter()
            .any(|it| it.kind == ItemKind::Rule),
        "a rule"
    );
}

// Expected (S-2.6): 2-column table, caret in the first body cell. Actual: stays a paragraph.
#[test]
#[ignore = "bug: [M2d] `| A | B |` + Enter does not create a table"]
fn h10_table_on_enter() {
    // S-2.6
    let mut e = Ed::open("h10", "");
    e.typ("| Name | Qty |").key(Key::Enter);
    assert_eq!(e.caret_kind(), ItemKind::Table);
    e.typ("x");
    let t = e.text();
    assert_eq!(t.lines().count(), 3, "{t:?}");
    assert!(
        t.lines().nth(2).unwrap().starts_with("| x"),
        "caret in first body cell: {t:?}"
    );
}

// Expected (§2.1): `- [ ] go`. Actual: `- [ ] ] go`.
#[test]
fn h11_task_box_in_bullet_item() {
    let mut e = Ed::open("h11", "- \n");
    e.at(2).typ("[ ] go");
    e.expect_saved("- [ ] go\n");
}

#[test]
fn h12_undo_input_rule_restores_literal() {
    // §11
    let mut e = Ed::open("h12", "");
    e.typ("- ");
    e.undo();
    let t = e.text();
    assert_eq!(t, "- ", "undo leaves the literal text");
    assert!(
        e.h.state_mut()
            .viewer()
            .unwrap()
            .ed
            .view
            .model()
            .items
            .iter()
            .all(|it| it.containers.is_empty()),
        "no list after undo"
    );
}

// Expected (§2.1): alert with the caret in its body. Actual: after `E` the caret jumps to offset 2; typing `]` gives `> ][!NOTE]`, Enter + `body` gives `> ]\n>\n> body[!NOTE]`.
#[test]
fn h13_callout_tag() {
    let mut e = Ed::open("h13", "");
    e.typ("> [!NOTE]");
    // the caret must stay after the tag (it jumped to offset 2, before it)
    assert_eq!(e.caret().offset, 9, "caret after the typed tag: {:?}", e.text());
    e.key(Key::Enter).typ("body");
    let t = e.text();
    // Enter on the empty body keeps the caret in the callout; the body is its own paragraph
    assert!(
        t.starts_with("> [!NOTE]\n> body") || t.starts_with("> [!NOTE]\n>\n> body"),
        "{t:?}"
    );
}

#[test]
fn h14_heading_rule_inside_quote() {
    let mut e = Ed::open("h14", "> \n");
    e.at(2).typ("# T").key(Key::Enter).typ("x");
    let t = e.text();
    assert!(t.starts_with("> # T\n"), "{t:?}");
    assert!(t.contains("> x"), "{t:?}");
}

// ---------------------------------------------------------------------------------------
// I. Undo / redo
// ---------------------------------------------------------------------------------------

#[test]
fn i01_word_undo_groups() {
    // C56
    let mut e = Ed::open("i01", "");
    e.typ("foo bar");
    e.undo();
    e.expect("foo");
    e.undo();
    e.expect("");
}

#[test]
fn i02_undo_restores_caret() {
    let mut e = Ed::open("i02", "abc\n\ndef\n");
    e.at(8).typ("X");
    e.at(0);
    e.undo();
    e.expect("abc\n\ndef\n").expect_caret(8, 0);
}

#[test]
fn i03_undo_enter_materialised_paragraph() {
    let mut e = Ed::open("i03", "a\n\nb\n");
    e.at(1).key(Key::Enter).typ("x");
    e.expect("a\n\nx\n\nb\n");
    e.undo();
    e.expect("a\n\nb\n");
    e.redo();
    e.expect_saved("a\n\nx\n\nb\n");
}

#[test]
fn i04_undo_backspace_merge() {
    let mut e = Ed::open("i04", "# A\n\nB\n");
    e.at(5).key(Key::Backspace);
    e.undo();
    e.expect("# A\n\nB\n").expect_caret(5, 0);
}

#[test]
fn i05_undo_all_returns_clean() {
    let mut e = Ed::open("i05", "- a\n\n> q\n");
    e.at(3).key(Key::Enter).typ("b").key(Key::Tab);
    e.at(10).typ(" w");
    for _ in 0..12 {
        e.undo();
    }
    e.expect("- a\n\n> q\n");
    assert!(!e.h.state().is_dirty());
}

// ---------------------------------------------------------------------------------------
// J. Mode switches mid-edit
// ---------------------------------------------------------------------------------------

// Expected: typing continues in the document after toggling source mode off. Actual: no widget has focus; keys are dropped until the user clicks (l08 passes with a click).
#[test]
#[ignore = "bug: keyboard focus is lost after Cmd+/ back to the rendered view"]
fn j01_source_mode_round_trip_mid_edit() {
    let mut e = Ed::open("j01", "Para\n\n# H\n");
    e.at(4).typ(" one");
    e.chord(cmd(), Key::Slash);
    assert_eq!(e.mode(), ViewMode::Source);
    assert_eq!(e.h.state().source_buffer(), Some("Para one\n\n# H\n"));
    e.chord(cmd(), Key::Slash);
    wait_until(&mut e.h, "rich view", |a| {
        !a.is_busy() && a.view_mode() == ViewMode::Rendered
    });
    e.check_model("back from source");
    e.at(8).typ(" two");
    e.expect_saved("Para one two\n\n# H\n");
}

// Expected: typing continues in the rendered view after Ctrl+Cmd+/ twice. Actual: keys are dropped until a click.
#[test]
#[ignore = "bug: keyboard focus is lost after leaving Side by Side"]
fn j02_side_by_side_mid_edit() {
    let mut e = Ed::open("j02", "Para\n");
    e.at(4).typ(" x");
    let ctrl_cmd = if cfg!(target_os = "macos") {
        Modifiers { ctrl: true, ..cmd() }
    } else {
        Modifiers {
            ctrl: true,
            alt: true,
            command: true,
            ..Default::default()
        }
    };
    e.chord(ctrl_cmd, Key::Slash);
    assert_eq!(e.mode(), ViewMode::SideBySide);
    e.chord(ctrl_cmd, Key::Slash);
    wait_until(&mut e.h, "rich view", |a| {
        !a.is_busy() && a.view_mode() == ViewMode::Rendered
    });
    e.check_model("back from side by side");
    e.at(6).typ("y");
    e.expect_saved("Para xy\n");
}

#[test]
fn j03_source_mode_with_virtual_paragraph_open() {
    let mut e = Ed::open("j03", "a\n");
    e.at(1).key(Key::Enter);
    e.chord(cmd(), Key::Slash);
    assert_eq!(e.h.state().source_buffer(), Some("a\n"));
    e.chord(cmd(), Key::Slash);
    wait_until(&mut e.h, "rich view", |a| {
        !a.is_busy() && a.view_mode() == ViewMode::Rendered
    });
    assert!(!e.h.state().is_dirty());
}

// ---------------------------------------------------------------------------------------
// K. Virtual paragraph and save
// ---------------------------------------------------------------------------------------

#[test]
fn k01_virtual_paragraph_abandoned_by_arrow_up_no_change() {
    let mut e = Ed::open("k01", "a\n\nb\n");
    e.at(1).key(Key::Enter).key(Key::ArrowUp);
    e.expect("a\n\nb\n");
    assert!(!e.h.state().is_dirty(), "no change");
    // Up from the (x = 0) virtual paragraph lands on the line above at x = 0
    e.expect_caret(0, 0);
    e.typ("z");
    e.expect_saved("za\n\nb\n");
}

#[test]
fn k02_virtual_paragraph_in_quote_typing() {
    let mut e = Ed::open("k02", "> a\n\nb\n");
    e.at(3).key(Key::Enter).typ("c");
    let t = e.text();
    assert!(t == "> a\n>\n> c\n\nb\n" || t == "> a\n> \n> c\n\nb\n", "{t:?}");
}

#[test]
fn k03_virtual_paragraph_after_list_typing_space_first() {
    let mut e = Ed::open("k03", "- a\n\nb\n");
    e.at(3).key(Key::Enter).key(Key::Enter).typ(" x");
    let t = e.text();
    assert!(t.starts_with("- a\n\n"), "{t:?}");
    assert!(!t.contains("- a\n- "), "no stray item: {t:?}");
}

#[test]
fn k04_space_first_on_virtual_paragraph() {
    let mut e = Ed::open("k04", "a\n\nb\n");
    e.at(1).key(Key::Enter).typ(" ");
    let c = e.caret();
    let t = e.text();
    assert!(t.starts_with("a\n\n"), "{t:?}");
    e.typ("x");
    let t2 = e.text();
    assert!(t2.contains("x"), "{t2:?}");
    assert!(c.offset > 1 || c.virt > 0, "caret moved past the space: {c:?}");
}

#[test]
fn k05_enter_twice_at_heading_end_then_type() {
    let mut e = Ed::open("k05", "# H\n");
    e.at(3).key(Key::Enter).typ("p").key(Key::Enter).typ("q");
    e.expect_saved("# H\n\np\n\nq\n");
}

#[test]
fn k06_typing_into_empty_doc_then_enter_and_heading() {
    let mut e = Ed::open("k06", "");
    e.typ("Title")
        .chord(cmd(), Key::Num1)
        .key(Key::End)
        .key(Key::Enter)
        .typ("Body text.");
    e.expect_saved("# Title\n\nBody text.");
}

// Expected: the space stays in the heading and the caret right after it. Overlaps issue #16. Actual bytes `## Title \n- xa`.
#[test]
#[ignore = "bug: #16 space at heading end; next typing lands in the list item"]
fn k07_space_at_end_of_heading_with_following_list() {
    let mut e = Ed::open("k07", "## Title\n- a\n");
    e.at(8).typ(" x");
    e.expect_saved("## Title x\n- a\n");
}

#[test]
fn k08_space_at_end_of_list_item_followed_by_heading() {
    let mut e = Ed::open("k08", "- a\n\n# H\n");
    e.at(3).typ(" b");
    e.expect_saved("- a b\n\n# H\n");
}

#[test]
fn k09_space_at_end_of_table_cell_last_row_last_cell() {
    let mut e = Ed::open("k09", "| a | b |\n|---|---|\n| x | y |\n\nafter\n");
    e.at(27).typ(" z");
    e.expect_saved("| a | b |\n|---|---|\n| x | y z |\n\nafter\n");
}

#[test]
fn k10_space_at_end_of_quote_before_paragraph() {
    let mut e = Ed::open("k10", "> q\n\nafter\n");
    e.at(3).typ(" r");
    e.expect_saved("> q r\n\nafter\n");
}

#[test]
fn k11_space_at_end_of_code_line_before_paragraph() {
    let mut e = Ed::open("k11", "```\ncode\n```\n\nafter\n");
    e.at(8).typ(" r");
    e.expect_saved("```\ncode r\n```\n\nafter\n");
}

#[test]
fn k12_space_at_end_of_footnote_before_paragraph() {
    let mut e = Ed::open("k12", "x[^1]\n\n[^1]: note\n\nafter\n");
    e.at(17).typ(" r");
    e.expect_saved("x[^1]\n\n[^1]: note r\n\nafter\n");
}

// ---------------------------------------------------------------------------------------
// L. More flows: trailing space from the file, paste / cut, Enter at a heading start
// ---------------------------------------------------------------------------------------

// Expected: End puts the caret after the paragraph's last source character, typing gives `a x`. Actual: `ax ` (caret before the invisible space).
#[test]
#[ignore = "bug: End before a trailing space that came from the file"]
fn l01_end_after_trailing_space_from_file_then_type() {
    // a paragraph whose source already ends with a space (not typed in this session)
    let mut e = Ed::open("l01", "a \n\nnext\n");
    e.at(0).key(Key::End).typ("x");
    e.expect_saved("a x\n\nnext\n");
}

#[test]
fn l02_paste_multi_paragraph_text_in_middle() {
    // M2b-7: plain text through the typing pipeline, one undo step
    let mut e = Ed::open("l02", "ab\n");
    e.at(1);
    e.h.event(Event::Paste("X\n\nY".into()));
    e.h.step();
    e.check_model("paste");
    e.expect("aX\n\nYb\n");
    e.undo();
    e.expect_saved("ab\n");
}

#[test]
fn l03_paste_over_selection_in_heading() {
    let mut e = Ed::open("l03", "# Old title\n");
    e.sel(2, 5);
    e.h.event(Event::Paste("New".into()));
    e.h.step();
    e.check_model("paste");
    e.expect_saved("# New title\n");
}

#[test]
fn l04_cut_and_paste_moves_text() {
    let mut e = Ed::open("l04", "one two\n\nthree\n");
    e.sel(3, 7);
    e.h.event(Event::Cut);
    e.h.step();
    e.check_model("cut");
    e.expect("one\n\nthree\n");
    e.at(10);
    e.h.event(Event::Paste(" two".into()));
    e.h.step();
    e.expect_saved("one\n\nthree two\n");
}

#[test]
fn l05_enter_at_heading_start_inserts_paragraph_above() {
    // C7: at the start, an empty paragraph above; the heading keeps its level
    let mut e = Ed::open("l05", "Intro\n\n# Title\n");
    e.at(9).key(Key::Enter).typ("x");
    e.expect_saved("Intro\n\nx\n\n# Title\n");
}

#[test]
fn l06_enter_in_middle_of_list_item_splits() {
    let mut e = Ed::open("l06", "- abcd\n");
    e.at(4).key(Key::Enter);
    e.expect("- ab\n- cd\n").expect_caret(7, 0);
}

// Expected (§5.2 rule 5): merge into the last item's paragraph, `- ab`. Actual: no-op.
#[test]
fn l07_backspace_at_paragraph_after_list_merges_into_item() {
    // §5.2 rule 5: merge into the previous block's last paragraph
    let mut e = Ed::open("l07", "- a\n\nb\n");
    e.at(5).key(Key::Backspace);
    e.expect_saved("- ab\n");
}

#[test]
fn l08_source_mode_round_trip_then_click_and_type() {
    let mut e = Ed::open("l08", "Para\n");
    e.chord(cmd(), Key::Slash);
    e.chord(cmd(), Key::Slash);
    wait_until(&mut e.h, "rich view", |a| {
        !a.is_busy() && a.view_mode() == ViewMode::Rendered
    });
    e.click_line_end(0).typ("x");
    e.expect_saved("Parax\n");
}

#[test]
fn l09_typing_in_nested_quote_list_item() {
    type_at("l09", "> - a\n>   - b\n", 13, " мир", "> - a\n>   - b мир\n");
}

// Expected: the space stays in the heading and the caret right after it. Overlaps issue #16.
#[test]
#[ignore = "bug: #16 space then Enter at heading end puts the space in the new paragraph"]
fn l10_space_then_enter_at_heading_end() {
    // overlaps issue #16: both keys at the end of a heading followed by a paragraph
    let mut e = Ed::open("l10", "# Title\n\nNext\n");
    e.at(7).typ(" ").key(Key::Enter).typ("x");
    let t = e.text();
    assert!(t == "# Title \n\nx\n\nNext\n" || t == "# Title\n\nx\n\nNext\n", "{t:?}");
}

#[test]
fn l11_caret_after_document_shrinks_in_source_mode() {
    // the caret sat at the end of a longer text; the text shrinks in source mode
    let mut e = Ed::open("l11", "first paragraph\n\nsecond paragraph here\n");
    e.at(38).typ("!");
    e.chord(cmd(), Key::Slash);
    *e.h.state_mut().source_buffer_mut().unwrap() = "ab\n".into();
    e.h.step();
    e.chord(cmd(), Key::Slash);
    wait_until(&mut e.h, "rich view", |a| {
        !a.is_busy() && a.view_mode() == ViewMode::Rendered
    });
    e.check_model("after shrink");
    let c = e.caret();
    assert!(c.offset <= 3, "caret clamped into the new text: {c:?}");
    e.click_line_end(0).typ("c");
    e.expect_saved("abc\n");
}

#[test]
fn l12_caret_after_external_reload_shrinks_document() {
    // the file is rewritten shorter on disk while the caret is at the end (clean document)
    let mut e = Ed::open("l12", "first paragraph\n\nsecond paragraph here\n");
    e.at(38);
    std::fs::write(&e.p, "ab\n").unwrap();
    let t = Instant::now();
    while e.text() != "ab\n" {
        assert!(t.elapsed() < Duration::from_secs(10), "reload not picked up");
        e.h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    e.h.step();
    e.check_model("after reload");
    let c = e.caret();
    assert!(c.offset <= 3, "caret clamped: {c:?}");
    e.typ("c");
}

// ---------------------------------------------------------------------------------------
// M. Inline typing: markers, auto-pairing (§4), literal text
// ---------------------------------------------------------------------------------------

#[test]
fn m01_typing_bold_markers_then_space_continues() {
    let mut e = Ed::open("m01", "");
    e.typ("a **b** c");
    e.expect_saved("a **b** c");
}

#[test]
fn m02_paren_pairs_and_steps_over() {
    // §4.1/§4.2
    let mut e = Ed::open("m02", "");
    e.typ("f(x) y");
    e.expect_saved("f(x) y");
}

#[test]
fn m03_quotes_and_apostrophe() {
    // §4.1: no pairing after a letter (don't)
    let mut e = Ed::open("m03", "");
    e.typ("don't \"q\" ok");
    e.expect_saved("don't \"q\" ok");
}

#[test]
fn m04_backspace_after_opener_deletes_pair() {
    // §4.3
    let mut e = Ed::open("m04", "");
    e.typ("a [").key(Key::Backspace);
    e.expect_saved("a ");
}

#[test]
fn m05_number_dot_in_middle_is_text() {
    let mut e = Ed::open("m05", "Total\n");
    e.at(5).typ(" 1. done - ok > x");
    e.expect_saved("Total 1. done - ok > x\n");
}

#[test]
fn m06_brackets_typed_literally_in_paragraph() {
    let mut e = Ed::open("m06", "");
    e.typ("see [1] and [x]");
    e.expect_saved("see [1] and [x]");
}

#[test]
fn m07_wrap_selection_in_parens() {
    // §4.4
    let mut e = Ed::open("m07", "a word b\n");
    e.sel(2, 6).typ("(");
    e.expect_saved("a (word) b\n");
}

#[test]
fn m08_inline_code_typed() {
    let mut e = Ed::open("m08", "");
    e.typ("use `x` now");
    e.expect_saved("use `x` now");
}
