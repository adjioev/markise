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

    let doc = ":::goals\n\n02. a\n03. b\n\n:::\n";
    let mut s = surface(doc);
    s.caret_at("b", 1);
    s.typed("!");
    assert_eq!(s.text(), doc.replace("03. b", "03. b!"));

    let doc = ":::goals\n\n- a\n\tb\n- c\n\n:::\n";
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
    "Top.\n\n:::poll\n\n## Which? {#q}\n\n- [ ] One {#a1}\n- [x] Two {#a2}\n\n> Because.\n> Yes.\n\n:::\n\nMiddle \\# x.\n\n:::tip\n\n### Title\n\nHello there\nsecond line\n\nAnother {#p}\n\n:::\n\n:::goals\n\n1. First\n2. Second\n\n:::\n\nEnd.\n".to_string()
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
            let p = s.offset();
            // the lines of the field the caret is in: nothing else may change
            let span = field_lines(&t, p);
            match op {
                0 | 1 => { let _ = s.run("edit.insertText", &Params::text("text", *what.get(TYPED))); }
                2 => { for _ in 0..n { s.key("edit.deleteBackward"); } }
                3 => { for _ in 0..n { s.key("edit.deleteForward"); } }
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
                    prop_assert!(now.ends_with(t2), "after the field: {} -> {}", t, now);
                }
            }
            prop_assert_eq!(outside_blocks(&now), around.clone(), "around the blocks: {}", now);
            prop_assert_eq!(shape_of(&now), names.clone(), "the same blocks: {}", now);
            // answers keep their ids, whatever was typed
            let ids = now.matches("{#a1}").count() + now.matches("{#a2}").count();
            prop_assert!(ids >= 2, "{}", now);
        }
        for _ in 0..200 {
            if s.text() == doc { break; }
            s.undo();
        }
        prop_assert_eq!(s.text(), doc);
    }
}
