//! Editing a block's fields where it stands (ADR 0021), through the editing surface with the CPU
//! test fonts: what a keystroke writes, where the caret goes, what cannot happen.

use std::sync::Arc;

use markise_doc::{DocPos, DocumentFormat, Params, Selection};
use markise_engine::block_fields::{BlockFields, FieldDecl, FieldKind, FieldValue, ItemData};
use markise_engine::block_view::PackageViews;
use markise_engine::editor::EditorView;
use markise_layout::block::NoImages;
use markise_layout::testing;
use markise_md::{MarkdownFormat, ParseOptions};
use markise_theme::Theme;
use proptest::prelude::*;

use super::block_views::index;

const VH: f32 = 900.0;

pub(crate) struct Surface {
    pub ed: EditorView,
    fonts: epaint::text::Fonts,
}

pub(crate) fn surface(text: &str) -> Surface {
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    let mut ed = EditorView::open(format, text, Theme::light(), 700.0);
    let views = Arc::new(PackageViews::new(index(), "en"));
    ed.view.set_block_views(Some(views.clone()));
    ed.set_block_fields(Some(views));
    let mut s = Surface {
        ed,
        fonts: testing::fonts(),
    };
    s.frame();
    s
}

impl Surface {
    pub fn frame(&mut self) {
        let mut f = self.fonts.with_pixels_per_point(1.0);
        self.ed.view.frame(VH * 4.0, 0.0, &mut f, &NoImages);
    }

    pub fn text(&self) -> String {
        self.ed.session.text().to_string()
    }

    pub fn caret(&mut self, at: usize) {
        self.ed.set_selection(Selection::caret(at));
        self.ed.refresh_focus();
        self.frame();
    }

    pub fn caret_at(&mut self, needle: &str, extra: usize) {
        let at = self.text().find(needle).unwrap_or_else(|| panic!("{needle}")) + extra;
        self.caret(at);
    }

    pub fn offset(&self) -> usize {
        self.ed.session.selection().primary().focus.offset
    }

    pub fn run(&mut self, id: &str, params: &Params) -> Result<(), String> {
        let r = {
            let mut f = self.fonts.with_pixels_per_point(1.0);
            self.ed.execute(id, params, &mut f, &NoImages, VH)
        };
        self.ed.refresh_focus();
        self.frame();
        r.map_err(|e| e.to_string())
    }

    pub fn key(&mut self, id: &str) {
        let _ = self.run(id, &Params::new());
    }

    pub fn typed(&mut self, text: &str) {
        for c in text.chars() {
            let _ = self.run("edit.insertText", &Params::text("text", c.to_string()));
        }
    }

    pub fn undo(&mut self) {
        self.key("edit.undo");
    }
}

pub(crate) const DOC: &str = "Before.\n\n:::poll\n\n## Which? {#q}\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> Because.\n\n:::\n\nBetween.\n\n:::tip\n\n### A title\n\nHello there\n\nSecond part\n\n:::\n\nAfter.\n";

#[test]
fn typing_writes_only_what_was_typed() {
    let mut s = surface(DOC);
    for (needle, extra, want) in [
        ("Which", 0, "## XWhich? {#q}"),
        ("Which", 6, "## Which?X {#q}"),
        ("One", 3, "- [ ] OneX {#a1}"),
        ("Two", 1, "- [x] TXwo {#a2}"),
        ("Because", 7, "> Because.".replace("Because.", "BecauseX.").leak()),
        ("A title", 7, "### A titleX"),
        ("Hello", 5, "HelloX there"),
        ("Second", 0, "XSecond part"),
    ] {
        let mut s2 = surface(DOC);
        s2.caret_at(needle, extra);
        s2.typed("X");
        let t = s2.text();
        assert!(t.contains(want), "{needle}: {t}");
        assert_eq!(t.len(), DOC.len() + 1, "{needle}: one byte: {t}");
        let at = s2.offset();
        assert_eq!(&t[at - 1..at], "X", "{needle}: the caret is after it");
    }
    s.caret_at("Between", 0);
    s.typed("Y");
    assert!(s.text().contains("YBetween"), "outside blocks as ever");
}

#[test]
fn markup_at_the_start_of_a_field_stays_text() {
    let mut s = surface(DOC);
    s.caret_at("Hello", 0);
    s.typed("# ");
    assert!(s.text().contains("\\# Hello there"), "{}", s.text());
    assert_eq!(
        s.ed.view.native_block(s.offset()).map(|_| ()),
        Some(()),
        "still its view"
    );

    let mut s = surface(DOC);
    s.caret_at("One", 0);
    s.typed("- x");
    assert!(s.text().contains("- [ ] \\- xOne {#a1}"), "{}", s.text());

    let mut s = surface(DOC);
    s.caret_at("Second", 0);
    s.typed(":::end");
    assert!(s.text().contains("\\:::endSecond part"), "{}", s.text());

    // a typed id at the end of an answer is its text, and its own id stays
    let mut s = surface(DOC);
    s.caret_at("One", 3);
    s.typed(" {#a9}");
    let t = s.text();
    assert!(t.contains("{#a1}"), "{t}");
    let m = decode(&t, 0);
    assert_eq!(
        answers(&m),
        [("One \\{#a9}".replace('\\', ""), false), ("Two".to_string(), true)]
    );
}

fn decls() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("question", FieldKind::Heading { level: Some(2) }),
        FieldDecl::new("answers", FieldKind::Choices),
        FieldDecl::new("why", FieldKind::Quote),
    ]
}

fn decode(doc: &str, first: usize) -> BlockFields {
    let d = markise_md::parse(doc);
    let dirs = d.directives();
    let b = &dirs[first];
    let r = b.opener.start..b.closer.end;
    BlockFields::decode(&doc[r.clone()], r.start, &decls(), &ParseOptions::default()).expect("a block")
}

fn answers(m: &BlockFields) -> Vec<(String, bool)> {
    let FieldValue::Items(items) = &m.field("answers").unwrap().value else {
        panic!()
    };
    (items.iter())
        .map(|i| match &i.data {
            ItemData::Choice { text, correct, .. } => (text.clone(), *correct),
            _ => panic!(),
        })
        .collect()
}

#[test]
fn the_caret_keeps_to_text_and_arrows_go_field_to_field() {
    let mut s = surface(DOC);
    // from the end of the first answer to the start of the second: no stop on marks or the id
    s.caret_at("One", 3);
    s.key("caret.right");
    assert_eq!(s.offset(), DOC.find("Two").unwrap());
    s.key("caret.left");
    assert_eq!(s.offset(), DOC.find("One").unwrap() + 3);
    s.key("caret.down");
    let at = s.offset();
    assert!(
        (DOC.find("Two").unwrap()..=DOC.find("Two").unwrap() + 3).contains(&at),
        "{at}"
    );
    // all the way through the block, in order, and out of it
    s.caret_at("Which", 0);
    let (start, end) = (DOC.find(":::poll").unwrap(), DOC.find("Between").unwrap());
    for _ in 0..80 {
        s.key("caret.right");
        let p = s.offset();
        if p >= end {
            break;
        }
        let line_start = DOC[..p].rfind('\n').map_or(0, |i| i + 1);
        let line = &DOC[line_start..DOC[p..].find('\n').map_or(DOC.len(), |i| p + i)];
        assert!(p > start, "{p}");
        if line.starts_with("- [") {
            assert!(p >= line_start + 6, "not in a marker: {line} at {}", p - line_start);
            assert!(!DOC[line_start..p].contains('{'), "not in an id: {line}");
        }
        assert_ne!(line, ":::poll");
    }
    assert!(s.offset() >= end, "out of the block on the far side");
}

#[test]
fn a_selection_stays_in_its_field() {
    let mut s = surface(DOC);
    s.caret_at("One", 1);
    for _ in 0..20 {
        s.key("select.right");
    }
    let sel = s.ed.session.selection().primary();
    assert_eq!(
        sel.range(),
        DOC.find("One").unwrap() + 1..DOC.find("One").unwrap() + 3,
        "to its end"
    );
    // what a drag asks for: as far as the field goes
    let two = DOC.find("Two").unwrap();
    s.ed.set_selection(Selection::single(
        DocPos::new(two + 1),
        DocPos::new(DOC.find("Between").unwrap()),
    ));
    assert_eq!(s.ed.session.selection().primary().range(), two + 1..two + 3);
    // backspace on it stays there
    s.key("edit.deleteBackward");
    assert!(s.text().contains("- [x] T {#a2}"), "{}", s.text());
    assert_eq!(s.text().len(), DOC.len() - 2);
}

#[test]
fn a_selection_from_outside_holds_the_block_whole() {
    let mut s = surface(DOC);
    let before = DOC.find("Before").unwrap();
    s.ed.set_selection(Selection::single(
        DocPos::new(before),
        DocPos::new(DOC.find("One").unwrap()),
    ));
    let block_end = DOC.find(":::\n\nBetween").unwrap() + 3;
    assert_eq!(s.ed.session.selection().primary().range(), before..block_end);
    assert!(!s.ed.in_native_block());
    s.frame();
    {
        let mut f = s.fonts.with_pixels_per_point(1.0);
        assert!(!s.ed.fix_selection(&mut f, &NoImages), "and that is where it stays");
    }
    s.typed("x");
    assert!(s.text().starts_with("x\n\nBetween."), "{}", s.text());
    s.undo();
    s.undo();
    assert_eq!(s.text(), DOC);
}

#[test]
fn backspace_and_delete_inside_a_field_and_at_its_edges() {
    let mut s = surface(DOC);
    s.caret_at("One", 3);
    s.key("edit.deleteBackward");
    assert!(s.text().contains("- [ ] On {#a1}"), "{}", s.text());
    s.caret_at("Two", 0);
    s.key("edit.deleteForward");
    assert!(s.text().contains("- [x] wo {#a2}"), "{}", s.text());
    // a word and to the line's start stop at the field's edge
    let mut s = surface(DOC);
    s.caret_at("there", 5);
    s.key("edit.deleteWordBackward");
    assert!(s.text().contains("Hello \n"), "{}", s.text());
    let mut s = surface(DOC);
    s.caret_at("Which", 3);
    s.key("delete.toLineStart");
    assert!(s.text().contains("## ch? {#q}"), "{}", s.text());
    // an edge is the end of the line here: a field does not run into the next
    let mut s = surface(DOC);
    s.caret_at("Hello", 0);
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), DOC);
    s.caret_at("Which", 6);
    s.key("edit.deleteForward");
    assert_eq!(s.text(), DOC);
}

#[test]
fn undo_is_the_documents() {
    let mut s = surface(DOC);
    s.caret_at("Hello", 5);
    let before = s.offset();
    s.typed("abc");
    assert!(s.text().contains("Helloabc there"));
    s.undo();
    assert_eq!(s.text(), DOC, "a group, as typing is anywhere");
    assert_eq!(s.offset(), before, "and the caret");
    s.key("edit.redo");
    assert!(s.text().contains("Helloabc there"));
    assert_eq!(s.offset(), before + 3);
    // a blank starts another group, as it does elsewhere
    s.undo();
    s.typed("ab cd");
    s.undo();
    assert!(s.text().contains("Helloab there"), "{}", s.text());
}

#[test]
fn every_other_change_is_refused_in_a_field() {
    let mut s = surface(DOC);
    s.caret_at("Hello", 2);
    for id in ["block.heading1", "block.bulletList", "block.quote", "edit.moveBlockUp"] {
        let _ = s.run(id, &Params::new());
        assert_eq!(s.text(), DOC, "{id}");
    }
    // marks around text in one field are text
    let hello = DOC.find("Hello").unwrap();
    s.ed.set_selection(Selection::range(hello, hello + 5));
    s.frame();
    s.key("format.strong");
    assert!(s.text().contains("**Hello** there"), "{}", s.text());
}

#[test]
fn bytes_nobody_edited_stay() {
    // CRLF and LF mixed, trailing blanks, a blank line of spaces, a tab-indented line, a number
    let doc = "Top\r\n\r\n:::tip\r\n\r\nFirst line  \nsecond\r\n   \r\nThird\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("second", 6);
    s.typed("!");
    assert_eq!(s.text(), doc.replace("second", "second!"));

    let doc = ":::steps\n\n02. a\n03. b\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("b", 1);
    s.typed("!");
    assert_eq!(s.text(), doc.replace("03. b", "03. b!"));

    let doc = ":::steps\n\n- a\n\tb\n- c\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("b", 1);
    s.typed("!");
    assert_eq!(s.text(), doc.replace("\tb", "\tb!"), "a line's prefix stays");
}

#[test]
fn a_reference_definition_cannot_form_over_two_lines() {
    let doc = ":::tip\n\n[r]x:\n/url\n\n:::\n\nA [r] link.\n";
    let mut s = surface(doc);
    s.caret_at("x:", 1);
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), doc, "refused: it would change a link elsewhere");
}

#[test]
fn what_a_text_field_took_as_it_stands_is_read_only() {
    let doc = ":::tip\n\nBefore\n\n```\n- code\n```\n\nAfter\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("code", 2);
    s.typed("X");
    assert_eq!(s.text(), doc);
    // and a selection over it does not take it away
    let (a, b) = (doc.find("Before").unwrap(), doc.find("After").unwrap() + 5);
    s.ed.set_selection(Selection::range(a, b));
    s.frame();
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), doc);
    // the paragraphs around it are text
    s.caret_at("After", 5);
    s.typed("!");
    assert_eq!(s.text(), doc.replace("After", "After!"));
}

#[test]
fn an_emptied_heading_keeps_its_place_and_its_id() {
    let mut s = surface(DOC);
    s.caret_at("Which?", 6);
    for _ in 0..6 {
        s.key("edit.deleteBackward");
    }
    assert!(s.text().contains("\n## {#q}\n"), "{}", s.text());
    assert!(
        s.ed.view.native_block(s.offset()).is_some(),
        "native while it is being edited"
    );
    assert!(s.ed.view.block_warnings(s.ed.view.model().item_at_offset(2)).is_none());
    s.typed("New");
    assert!(s.text().contains("\n## New {#q}\n"), "{}", s.text());
    s.undo();
    s.undo();
    assert_eq!(s.text(), DOC);
}

#[test]
fn a_block_being_edited_stays_native_until_the_caret_leaves() {
    let mut s = surface(DOC);
    s.caret_at("Which?", 6);
    for _ in 0..6 {
        s.key("edit.deleteBackward");
    }
    let opener = s.text().find(":::poll").unwrap();
    assert!(s.ed.view.native_block(opener).is_some());
    // the block just below it with the same text is not it
    s.caret_at("Between", 0);
    assert!(s.ed.view.native_block(opener).is_none(), "outside its schema: a card");
    let i = s.ed.view.model().item_at_offset(opener);
    assert!(s.ed.view.block_warnings(i).is_some());
    // it edits as a card now, and the next fit brings the view back
    s.caret_at("## {#q}", 3);
    s.typed("Back");
    assert!(s.ed.view.native_block(opener).is_some(), "{}", s.text());
}

#[test]
fn an_emptied_text_field_keeps_a_place_for_the_caret() {
    let doc = ":::tip\n\nHi\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("Hi", 2);
    s.key("edit.deleteBackward");
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), ":::tip\n\n:::\n");
    let p = s.ed.session.selection().primary().focus;
    assert!(p.virt > 0, "{p:?}");
    s.typed("Yo");
    assert_eq!(s.text(), ":::tip\n\nYo\n\n:::\n");
    assert_eq!(s.offset(), ":::tip\n\nYo".len());
    s.undo();
    assert_eq!(s.text(), ":::tip\n\n:::\n");
    s.undo();
    assert_eq!(s.text(), doc);

    // after a field that stays
    let doc = ":::tip\n\n### Title\n\nHi\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("Hi", 2);
    s.key("edit.deleteBackward");
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), ":::tip\n\n### Title\n\n:::\n");
    s.typed("Yo");
    assert_eq!(s.text(), ":::tip\n\n### Title\n\nYo\n\n:::\n");
}

#[test]
fn enter_in_a_list_makes_an_item() {
    let doc = ":::steps\n\n- First\n- Second\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("First", 5);
    s.key("edit.newline");
    assert_eq!(s.text(), ":::steps\n\n- First\n- \n- Second\n\n:::\n");
    s.typed("New");
    assert_eq!(s.text(), ":::steps\n\n- First\n- New\n- Second\n\n:::\n");
    // in the middle it splits, and markup stays text
    s.caret_at("New", 1);
    s.key("edit.newline");
    assert_eq!(s.text(), ":::steps\n\n- First\n- N\n- ew\n- Second\n\n:::\n");
    s.typed("# ");
    assert!(s.text().contains("- \\# ew\n"), "{}", s.text());
}

#[test]
fn enter_on_an_answer_makes_an_answer_with_a_new_id() {
    let mut s = surface(DOC);
    s.caret_at("One", 3);
    s.key("edit.newline");
    let t = s.text();
    assert!(t.contains("- [ ] One {#a1}\n- [ ] {#a3}\n- [x] Two {#a2}\n"), "{t}");
    s.typed("Three");
    assert!(s.text().contains("- [ ] Three {#a3}\n"), "{}", s.text());
    // one undo for the words, one for the answer
    s.undo();
    assert_eq!(s.text(), t, "the words go, the answer stays");
    s.undo();
    assert_eq!(s.text(), DOC);
    // in the middle the rest goes along, without the mark
    s.caret_at("Two", 1);
    s.key("edit.newline");
    assert!(s.text().contains("- [x] T {#a2}\n- [ ] wo {#a3}\n"), "{}", s.text());
}

#[test]
fn a_removed_answer_does_not_give_its_id_back() {
    let mut s = surface(DOC);
    s.caret_at("Two", 3);
    s.key("edit.newline");
    assert!(s.text().contains("{#a3}"));
    s.key("edit.deleteBackward");
    assert!(!s.text().contains("{#a3}"), "{}", s.text());
    s.key("edit.newline");
    assert!(
        s.text().contains("{#a4}"),
        "the third one is taken while this goes on: {}",
        s.text()
    );
}

#[test]
fn enter_on_an_empty_last_item_leaves_the_list() {
    let mut s = surface(DOC);
    s.caret_at("Two", 3);
    s.key("edit.newline");
    s.key("edit.newline");
    assert_eq!(s.text(), DOC, "gone again");
    assert_eq!(s.offset(), DOC.find("Two").unwrap() + 3);
    // an empty one in the middle just stays
    s.caret_at("One", 3);
    s.key("edit.newline");
    let t = s.text();
    s.key("edit.newline");
    assert_eq!(s.text(), t);
}

/// A list with an empty second item made in place: a block like this only stays native while
/// it is being edited.
fn with_an_empty_item() -> Surface {
    let mut s = surface(":::steps\n\n- First\n- Third\n\n:::\n");
    s.caret_at("First", 5);
    s.key("edit.newline");
    assert_eq!(s.text(), EMPTY_ITEM);
    s
}

const EMPTY_ITEM: &str = ":::steps\n\n- First\n- \n- Third\n\n:::\n";

#[test]
fn backspace_and_delete_remove_an_empty_item_and_nothing_else() {
    let mut s = with_an_empty_item();
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), ":::steps\n\n- First\n- Third\n\n:::\n");
    assert_eq!(
        s.offset(),
        EMPTY_ITEM.find("First").unwrap() + 5,
        "the end of the one before"
    );
    s.undo();
    assert_eq!(s.text(), EMPTY_ITEM);

    let mut s = with_an_empty_item();
    s.key("edit.deleteForward");
    assert_eq!(s.text(), ":::steps\n\n- First\n- Third\n\n:::\n");
    assert_eq!(
        &s.text()[s.offset()..s.offset() + 5],
        "Third",
        "the start of the one after"
    );

    // an item with text is not joined to its neighbour: nothing happens at its edge
    let doc = ":::steps\n\n- First\n- Third\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("Third", 0);
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), doc);
    s.caret_at("First", 5);
    s.key("edit.deleteForward");
    assert_eq!(s.text(), doc);

    // the only item stays
    let doc = ":::steps\n\n- a\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("- a", 3);
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), ":::steps\n\n- \n\n:::\n");
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), ":::steps\n\n- \n\n:::\n");
}

#[test]
fn enter_in_a_heading_goes_on_and_enter_in_text_makes_a_paragraph() {
    let mut s = surface(DOC);
    s.caret_at("Which", 2);
    s.key("edit.newline");
    assert_eq!(s.text(), DOC);
    assert_eq!(s.offset(), DOC.find("One").unwrap());

    let mut s = surface(DOC);
    s.caret_at("Hello", 5);
    s.key("edit.newline");
    assert!(s.text().contains("Hello\n\nthere\n\nSecond part"), "{}", s.text());
    assert_eq!(&s.text()[s.offset()..s.offset() + 5], "there");
    // Backspace at its start joins them again
    s.key("edit.deleteBackward");
    assert!(
        s.text().contains("Hello there\n") || s.text().contains("Hellothere\n"),
        "{}",
        s.text()
    );

    // at the end of the text: a place for a new paragraph, nothing in the file until it has text
    let mut s = surface(DOC);
    s.caret_at("Second part", 11);
    s.key("edit.newline");
    assert_eq!(s.text(), DOC);
    assert!(s.ed.session.selection().primary().focus.virt > 0);
    s.typed("# Third");
    assert!(s.text().contains("Second part\n\n\\# Third\n"), "{}", s.text());
    // leaving it without text changes nothing
    let mut s = surface(DOC);
    s.caret_at("Second part", 11);
    s.key("edit.newline");
    s.key("edit.deleteBackward");
    assert_eq!(s.text(), DOC);
    assert_eq!(s.ed.session.selection().primary().focus.virt, 0);
}

#[test]
fn enter_in_a_quote_makes_a_paragraph_of_the_quote() {
    let mut s = surface(DOC);
    s.caret_at("Because.", 8);
    s.key("edit.newline");
    s.typed("More");
    assert!(s.text().contains("> Because.\n>\n> More\n"), "{}", s.text());
}

#[test]
fn a_press_on_a_mark_marks_by_the_blocks_rule() {
    // one answer: the press moves the mark, as one undo step, and the caret stays
    let mut s = surface(DOC);
    s.caret_at("Between", 0);
    let caret = s.offset();
    assert!(s.ed.press_answer_mark(DOC.find("One").unwrap()));
    assert!(s.text().contains("- [x] One {#a1}\n- [ ] Two {#a2}"), "{}", s.text());
    assert_eq!(s.offset(), caret);
    assert!(
        !s.ed.press_answer_mark(DOC.find("One").unwrap()),
        "a press on the marked one"
    );
    s.undo();
    assert_eq!(s.text(), DOC);

    // several: each its own, and nothing else in the file moves
    let doc = DOC.replace(":::poll\n", ":::poll many\n");
    let mut s = surface(&doc);
    assert!(s.ed.press_answer_mark(doc.find("One").unwrap()));
    assert!(s.text().contains("- [x] One {#a1}\n- [x] Two {#a2}"), "{}", s.text());
    assert!(s.ed.press_answer_mark(doc.find("Two").unwrap()));
    assert!(s.text().contains("- [x] One {#a1}\n- [ ] Two {#a2}"), "{}", s.text());
    assert_eq!(s.text().len(), doc.len());
    // not a mark's business anywhere else
    assert!(!s.ed.press_answer_mark(doc.find("Because").unwrap()));
}

#[test]
fn a_paste_keeps_markup_as_text_and_a_one_line_field_one_line() {
    let mut s = surface(DOC);
    s.caret_at("Hello", 0);
    let _ = s.run("edit.insertText", &Params::text("text", "# a\n- b\n\n:::c\n"));
    let t = s.text();
    assert!(t.contains("\\# a\n\\- b\n\n\\:::c\nHello there"), "{t}");
    assert_eq!(s.ed.view.native_block(s.offset()).map(|_| ()), Some(()));

    let mut s = surface(DOC);
    s.caret_at("Which", 0);
    let _ = s.run("edit.insertText", &Params::text("text", "# a\r\n\r\nb\n"));
    assert!(s.text().contains("## # a bWhich? {#q}"), "{}", s.text());

    let mut s = surface(DOC);
    s.caret_at("One", 3);
    let _ = s.run("edit.insertText", &Params::text("text", "\n- [x] x {#a2}\n"));
    let t = s.text();
    assert!(
        t.contains("- [ ] One - [x] x \\{#a2} {#a1}\n") || t.contains("- [ ] One - [x] x {#a2} {#a1}\n"),
        "{t}"
    );
    assert_eq!(t.matches("\n- [").count(), 2, "{t}");
}

#[test]
fn tab_goes_from_field_to_field() {
    let mut s = surface(DOC);
    s.caret_at("Which", 2);
    s.key("edit.indentOrNextCell");
    assert_eq!(s.offset(), DOC.find("One").unwrap());
    s.key("edit.indentOrNextCell");
    assert_eq!(s.offset(), DOC.find("Two").unwrap());
    s.key("edit.outdentOrPrevCell");
    assert_eq!(s.offset(), DOC.find("One").unwrap() + 3);
    assert_eq!(s.text(), DOC);
}

#[test]
fn marks_that_would_lose_the_block_are_taken_back() {
    // clearing the bold makes a closing delimiter of the text
    let doc = ":::tip\n\n**:::**\n\n:::\n";
    let mut s = surface(doc);
    let at = doc.find("**").unwrap();
    s.ed.set_selection(Selection::range(at, at + 7));
    s.frame();
    s.key("format.clear");
    assert_eq!(s.text(), doc);
    assert!(s.ed.view.native_block(at).is_some());
    s.key("edit.redo");
    assert_eq!(s.text(), doc, "and it is not there to redo");

    // with text after it the rest of the block reads the same, and it is still not the block
    let doc = ":::tip\n\nA\n\n**:::**\n\nB\n\n:::\n";
    let mut s = surface(doc);
    let at = doc.find("**").unwrap();
    s.ed.set_selection(Selection::range(at, at + 7));
    s.frame();
    s.key("format.clear");
    assert_eq!(s.text(), doc);

    // marks that keep it are marks
    let doc = ":::tip\n\n**Hello**\n\n:::\n";
    let mut s = surface(doc);
    let at = doc.find("**").unwrap();
    s.ed.set_selection(Selection::range(at, at + 9));
    s.frame();
    s.key("format.clear");
    assert_eq!(s.text(), ":::tip\n\nHello\n\n:::\n");
}

#[test]
fn undo_back_to_a_place_with_no_text_keeps_which_field_it_is() {
    let doc = ":::tip\n\n### Title\n\nBody\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("Body", 4);
    for _ in 0..4 {
        s.key("edit.deleteBackward");
    }
    assert_eq!(s.text(), ":::tip\n\n### Title\n\n:::\n");
    s.typed("X");
    assert_eq!(s.text(), ":::tip\n\n### Title\n\nX\n\n:::\n");
    s.undo();
    assert_eq!(s.text(), ":::tip\n\n### Title\n\n:::\n");
    s.typed("Y");
    assert_eq!(s.text(), ":::tip\n\n### Title\n\nY\n\n:::\n", "the body, not the title");
    // redo over it too
    s.undo();
    s.key("edit.redo");
    assert_eq!(s.text(), ":::tip\n\n### Title\n\nY\n\n:::\n");
}

#[test]
fn a_place_nobody_can_name_writes_nowhere() {
    let doc = ":::tip\n\n### Title\n\nBody\n\n:::\n";
    let mut s = surface(doc);
    let end = doc.find("Title").unwrap() + 5;
    // what a stale selection could be: no edit put the caret there
    s.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(end, 1)));
    s.frame();
    s.typed("Y");
    assert_eq!(s.text(), doc);
}

#[test]
fn the_caret_after_a_paste_is_on_a_character() {
    for (doc, needle, want) in [
        (":::tip\n\nю  word\n\n:::\n", "ю", "юé\nword"),
        (":::tip\n\n### ю  word\n\nBody\n\n:::\n", "ю", "юé  word"),
    ] {
        let mut s = surface(doc);
        s.caret_at(needle, "ю".len());
        s.run("edit.insertText", &Params::text("text", "é\n")).unwrap();
        let t = s.text();
        assert!(t.contains(want), "{t:?}");
        let at = s.offset();
        assert!(t.is_char_boundary(at));
        assert!(
            t[..at].trim_end().ends_with('é'),
            "right after what was pasted: {t:?} at {at}"
        );
    }
}

#[test]
fn a_replacement_from_outside_a_field_goes_through_its_checks() {
    let mut s = surface(DOC);
    let at = DOC.find("Hello").unwrap();
    assert!(s.ed.field_replace(at..at + 5, "[[Other]]").is_some_and(|r| r.is_ok()));
    assert!(s.text().contains("[[Other]] there"), "{}", s.text());
    s.undo();
    assert_eq!(s.text(), DOC);
    // over two fields: no
    let (a, b) = (DOC.find("One").unwrap(), DOC.find("Two").unwrap() + 1);
    assert!(s.ed.field_replace(a..b, "x").is_some_and(|r| r.is_err()));
    assert_eq!(s.text(), DOC);
    // outside a block it is not for this
    assert!(s.ed.field_replace(0..1, "x").is_none());
}

#[test]
fn typing_on_the_place_after_a_block_is_not_in_a_field() {
    let doc = ":::tip\n\nBody\n\n:::\n";
    let mut s = surface(doc);
    let end = doc.rfind(":::").unwrap() + 3;
    s.ed.set_selection(Selection::caret_at(DocPos::with_virt(end, 1)));
    s.frame();
    s.typed("After");
    assert!(s.text().starts_with(doc), "{}", s.text());
    assert!(s.text().contains("After"), "{}", s.text());
}

#[test]
fn a_line_break_at_the_end_of_text_is_a_place_for_the_next_line() {
    let mut s = surface(DOC);
    s.caret_at("Second part", 11);
    s.key("edit.softBreak");
    assert_eq!(s.text(), DOC, "nothing in the file yet");
    assert!(s.ed.session.selection().primary().focus.virt > 0);
    s.typed("line");
    assert!(s.text().contains("Second part\nline\n"), "{}", s.text());
    s.undo();
    assert_eq!(s.text(), DOC);
    s.typed("y");
    assert!(
        s.text().contains("Second part\ny\n"),
        "undo keeps which place it is: {}",
        s.text()
    );

    let mut s = surface(DOC);
    s.caret_at("Because.", 8);
    s.key("edit.softBreak");
    s.typed("More");
    assert!(s.text().contains("> Because.\n> More\n"), "{}", s.text());

    // over a selection to the end, the selection goes and the place is there
    let mut s = surface(DOC);
    let at = DOC.find("Second part").unwrap();
    s.ed.set_selection(Selection::range(at + 6, at + 11));
    s.frame();
    s.key("edit.softBreak");
    s.typed("z");
    assert!(s.text().contains("Second\nz\n"), "{}", s.text());

    // nothing of the kind in a field of one line
    let mut s = surface(DOC);
    s.caret_at("One", 3);
    s.key("edit.softBreak");
    assert_eq!(s.text(), DOC);
    assert_eq!(s.ed.session.selection().primary().focus.virt, 0);
}

#[test]
fn enter_at_the_end_of_a_paragraph_before_another_goes_on_to_it() {
    let mut s = surface(DOC);
    s.caret_at("Hello there", 11);
    s.key("edit.newline");
    assert_eq!(s.text(), DOC, "no blank lines pile up");
    assert_eq!(s.offset(), DOC.find("Second").unwrap());
}

#[test]
fn tab_and_enter_reach_the_text_of_a_field_that_holds_more() {
    let doc = ":::tip\n\n### Title\n\n```\ncode\n```\n\nAfter\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("Title", 2);
    s.key("edit.indentOrNextCell");
    assert_eq!(s.offset(), doc.find("After").unwrap());
    s.key("edit.outdentOrPrevCell");
    assert_eq!(s.offset(), doc.find("Title").unwrap() + 5);
    s.key("edit.newline");
    assert_eq!(s.offset(), doc.find("After").unwrap());
    assert_eq!(s.text(), doc);
}

#[test]
fn a_blocks_state_goes_with_it_when_it_is_deleted() {
    use markise_doc::PatchSet;
    use markise_layout::BlockViewMode as Mode;
    let doc = ":::tip\n\nFirst\n\n:::\n\n:::tip\n\nSecond\n\n:::\n";
    let second = doc.rfind(":::tip").unwrap();
    let mut s = surface(doc);
    s.ed.switch_block_view(0);
    assert_eq!(s.ed.view.block_mode_of(0), Mode::Markdown);
    assert_eq!(s.ed.view.block_mode_of(second), Mode::Native);
    // the first block goes, as an edit in Source mode can make it go
    let cut = PatchSet::single_verbatim(0..second, "");
    s.ed.edit(|e| e.apply_external(&cut, "Edit", false)).unwrap();
    s.frame();
    assert_eq!(
        s.ed.view.block_mode_of(0),
        Mode::Native,
        "what follows does not inherit it"
    );
    assert!(s.ed.view.native_block(2).is_some());
    assert!(s.ed.block_view_state().1.is_empty());

    // two states, the first one's block goes: the other's stays where it is now
    let mut s = surface(doc);
    s.ed.switch_block_view(0);
    s.ed.switch_block_view(second);
    let cut = PatchSet::single_verbatim(0..second, "");
    s.ed.edit(|e| e.apply_external(&cut, "Edit", false)).unwrap();
    s.frame();
    assert_eq!(s.ed.block_view_state().1, vec![(0, Mode::Markdown)]);

    // an edit above only moves it, and a rename makes another block of it
    let mut s = surface(doc);
    s.ed.switch_block_view(second);
    let above = PatchSet::single_verbatim(0..0, "Top\n\n");
    s.ed.edit(|e| e.apply_external(&above, "Edit", false)).unwrap();
    assert_eq!(s.ed.block_view_state().1, vec![(second + 5, Mode::Markdown)]);
    let name = second + 5 + 3;
    let rename = PatchSet::single_verbatim(name..name + 3, "poll");
    s.ed.edit(|e| e.apply_external(&rename, "Edit", false)).unwrap();
    assert!(s.ed.block_view_state().1.is_empty());
}

#[test]
fn marks_that_would_change_what_a_quote_holds_are_taken_back() {
    let doc = ":::poll\n\n## Which?\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> `# Heading`\n\n:::\n";
    let mut s = surface(doc);
    let at = doc.find('`').unwrap();
    s.ed.set_selection(Selection::range(at, at + "`# Heading`".len()));
    s.frame();
    s.key("format.clear");
    assert_eq!(s.text(), doc, "a heading in the quote would make it no text any more");

    // and in an answer: below an item is no place for a new kind of block either
    let doc = ":::poll\n\n## Which?\n\n- [ ] `# One` {#a1}\n- [x] Two {#a2}\n\n:::\n";
    let mut s = surface(doc);
    let at = doc.find('`').unwrap();
    s.ed.set_selection(Selection::range(at, at + "`# One`".len()));
    s.frame();
    s.key("format.clear");
    assert_eq!(s.text(), doc);

    // marks that keep it are marks
    let doc = ":::poll\n\n## Which?\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> `Plain`\n\n:::\n";
    let mut s = surface(doc);
    let at = doc.find('`').unwrap();
    s.ed.set_selection(Selection::range(at, at + "`Plain`".len()));
    s.frame();
    s.key("format.clear");
    assert!(s.text().contains("\n> Plain\n"), "{}", s.text());
}

#[test]
fn tab_stops_at_a_field_with_no_text_yet() {
    // an empty item
    let mut s = surface(":::steps\n\n- First\n- Third\n\n:::\n");
    s.caret_at("First", 5);
    s.key("edit.newline");
    let empty = s.offset();
    s.key("edit.indentOrNextCell");
    assert_eq!(&s.text()[s.offset()..s.offset() + 5], "Third");
    s.key("edit.outdentOrPrevCell");
    assert_eq!(s.offset(), empty, "not past the empty one");
    s.key("edit.outdentOrPrevCell");
    assert_eq!(&s.text()[s.offset() - 5..s.offset()], "First");
    s.key("edit.indentOrNextCell");
    assert_eq!(s.offset(), empty);
    // an empty answer
    let mut s = surface(DOC);
    s.caret_at("One", 3);
    s.key("edit.newline");
    let empty = s.offset();
    s.key("edit.indentOrNextCell");
    s.key("edit.outdentOrPrevCell");
    assert_eq!(s.offset(), empty);
    // a cleared heading
    let doc = ":::poll\n\n## Hi {#q}\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("Hi", 2);
    s.key("edit.deleteBackward");
    s.key("edit.deleteBackward");
    let empty = s.offset();
    assert!(s.text().contains("## {#q}"), "{}", s.text());
    s.key("edit.indentOrNextCell");
    assert_eq!(&s.text()[s.offset()..s.offset() + 3], "One");
    s.key("edit.outdentOrPrevCell");
    assert_eq!(s.offset(), empty);
}

// ---- random edits

const TYPED: &[&str] = &[
    "x",
    " ",
    "# ",
    "- ",
    "1. ",
    "> ",
    ":::",
    ":::a",
    "---",
    "===",
    "| a |",
    "|-|",
    "$$",
    "```",
    "<div>",
    "<details>",
    "{#a1}",
    " {#z}",
    "\\",
    "*",
    "**",
    "`",
    "[r]: /u",
    "[^1]: n",
    "    ",
    "\t",
    "é",
    "日本",
    "\n",
    "a\nb",
    "\r\n",
    "[",
    "]",
];

fn doc_for_props() -> String {
    "Top.\n\n:::poll\n\n## Which? {#q}\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> Because.\n> Yes.\n\n:::\n\nMiddle \\# x.\n\n:::tip\n\n### Title\n\nHello there\nsecond line\n\nAnother {#p}\n\n:::\n\n:::steps\n\n1. First\n2. Second\n\n:::\n\nEnd.\n".to_string()
}

fn outside_blocks(doc: &str) -> Vec<String> {
    let d = markise_md::parse(doc);
    let mut out = Vec::new();
    let mut at = 0;
    for b in d.directives() {
        out.push(doc[at..b.opener.start].to_string());
        at = b.closer.end;
    }
    out.push(doc[at..].to_string());
    out
}

fn shape_of(doc: &str) -> Vec<(String, String)> {
    let d = markise_md::parse(doc);
    (d.directives().iter())
        .map(|b| (b.name.slice(doc).to_string(), b.args.slice(doc).to_string()))
        .collect()
}

/// The text with the numbers of ordered list items taken out.
fn unnumbered(text: &str) -> String {
    (text.split_inclusive('\n'))
        .map(|l| {
            let d = l.bytes().take_while(u8::is_ascii_digit).count();
            if d > 0 && l[d..].starts_with('.') { &l[d..] } else { l }
        })
        .collect()
}

/// What stands before and after the lines of the field at `at`.
fn field_lines(doc: &str, at: usize) -> Option<(String, String)> {
    let d = markise_md::parse(doc);
    let d: &dyn markise_doc::FormatDoc = &d;
    let c = d.named_containers_at(at).pop()?;
    let views = PackageViews::new(index(), "en");
    let (a, b) = (c.opener.start, c.closer.end);
    let model = views.fields_of(&c.name, &c.args, &doc[a..b], a)?;
    let r = model.slot_at(at - a)?.range();
    let (r0, r1) = (r.start + a, r.end + a);
    let head = doc[..r0].rfind('\n').map_or(0, |i| i + 1);
    let tail = doc[r1..].find('\n').map_or(doc.len(), |i| r1 + i);
    Some((doc[..head].to_string(), doc[tail..].to_string()))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Random in-place edits never change a byte outside the blocks, the blocks stay the same
    /// blocks, and undoing them all gives the document back.
    #[test]
    fn random_edits_keep_the_blocks_and_everything_around_them(
        ops in prop::collection::vec((any::<prop::sample::Index>(), 0..5u8, any::<prop::sample::Index>(), 0..4usize), 1..16)
    ) {
        let doc = doc_for_props();
        let mut s = surface(&doc);
        let around = outside_blocks(&doc);
        let names = shape_of(&doc);
        for (at, op, what, n) in ops {
            let t = s.text();
            let offsets: Vec<usize> = t.char_indices().map(|(i, _)| i).collect();
            // somewhere in a block
            let inside: Vec<usize> = offsets.iter().copied().filter(|&o| s.ed.view.native_block(o).is_some()).collect();
            if inside.is_empty() {
                break;
            }
            let p = *at.get(&inside);
            s.caret(p);
            for _ in 0..if matches!(op, 2 | 3) { n } else { 1 } {
                let t = s.text();
                // the lines of the field the caret is in: nothing else may change
                let span = field_lines(&t, s.offset());
                match op {
                    0 => { let _ = s.run("edit.insertText", &Params::text("text", *what.get(TYPED))); }
                    1 => s.key("edit.newline"),
                    2 => s.key("edit.deleteBackward"),
                    3 => s.key("edit.deleteForward"),
                    _ => {
                        s.key("select.right");
                        let _ = s.run("edit.insertText", &Params::text("text", *what.get(TYPED)));
                    }
                }
                let now = s.text();
                match span {
                    None => prop_assert_eq!(&now, &t, "the caret is in no field"),
                    Some((head, tail)) => {
                        let (h, t2) = (head.trim_end_matches('\n'), tail.trim_start_matches('\n'));
                        prop_assert!(now.starts_with(h), "before the field: {} -> {}", t, now);
                        // (an item coming or going renumbers the ones after it in an ordered list)
                        let (a, b) = (unnumbered(&now), unnumbered(t2));
                        prop_assert!(a.ends_with(&b), "after the field: {} -> {}", t, now);
                    }
                }
                prop_assert_eq!(outside_blocks(&now), around.clone(), "around the blocks: {}", now);
                prop_assert_eq!(shape_of(&now), names.clone(), "the same blocks: {}", now);
            }
        }
        for _ in 0..200 {
            if s.text() == doc { break; }
            s.undo();
        }
        prop_assert_eq!(s.text(), doc);
    }
}
