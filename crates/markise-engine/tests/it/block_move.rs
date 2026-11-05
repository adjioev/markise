//! Moving a block among its siblings (ADR 0022): what it goes past, the bytes around it, one undo step,
//! and that its own bytes and ids stay as they are.

use markise_doc::{DocPos, Selection};
use markise_engine::editor::MoveRefused;
use markise_layout::{BlockViewMode, Paging};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

use super::block_inplace::surface;
use crate::common::T;

fn moved(marked: &str, up: bool) -> String {
    let at = marked.find('‸').unwrap();
    let text = marked.replace('‸', "");
    let mut s = surface(&text);
    s.caret(at);
    assert!(s.ed.can_move_block(up), "{marked:?}");
    assert_eq!(s.ed.move_block(up, false), Ok(true), "{marked:?}");
    s.text()
}

#[test]
fn it_goes_past_what_stands_next_to_it() {
    for (marked, up, want) in [
        // a paragraph
        ("a\n\n:::x\n\nb‸\n\n:::\n", true, ":::x\n\nb\n\n:::\n\na\n"),
        (":::x\n\nb‸\n\n:::\n\na\n", false, "a\n\n:::x\n\nb\n\n:::\n"),
        // another block, whole
        (
            ":::x\n\na\n\n:::\n\n:::y\n\nb‸\n\n:::\n",
            true,
            ":::y\n\nb\n\n:::\n\n:::x\n\na\n\n:::\n",
        ),
        (
            ":::x\n\na‸\n\n:::\n\n:::y\n\nb\n\n:::\n\nz\n",
            false,
            ":::y\n\nb\n\n:::\n\n:::x\n\na\n\n:::\n\nz\n",
        ),
        // a list or a table, whole
        (
            "- a\n- b\n\n:::x\n\nc‸\n\n:::\n",
            true,
            ":::x\n\nc\n\n:::\n\n- a\n- b\n",
        ),
        // around a neighbour on the other side, the rest stays
        (
            "p\n\n:::x\n\nb‸\n\n:::\n\nq\n\nr\n",
            false,
            "p\n\nq\n\n:::x\n\nb\n\n:::\n\nr\n",
        ),
        (
            "p\n\nq\n\n:::x\n\nb‸\n\n:::\n\nr\n",
            true,
            "p\n\n:::x\n\nb\n\n:::\n\nq\n\nr\n",
        ),
    ] {
        assert_eq!(moved(marked, up), want, "{marked:?}");
    }
}

#[test]
fn blank_lines_end_up_normal_and_nothing_runs_together() {
    for (marked, up, want) in [
        // none around it: what it held apart stays apart, and it gets its own
        ("p\n:::x\nb‸\n:::\nq\n", false, "p\n\nq\n\n:::x\nb\n:::\n"),
        ("p\n:::x\nb‸\n:::\nq\nr\n", true, ":::x\nb\n:::\n\np\n\nq\nr\n"),
        // many around it: they do not pile up
        (
            "p\n\n\n\n:::x\nb‸\n:::\n\n\n\nq\n",
            false,
            "p\n\n\n\nq\n\n:::x\nb\n:::\n",
        ),
        // the end of a file stays as it was
        ("p\n\n:::x\nb‸\n:::", true, ":::x\nb\n:::\n\np"),
        (":::x\nb‸\n:::\n\np", false, "p\n\n:::x\nb\n:::"),
        ("p\n\n:::x\nb‸\n:::\n", true, ":::x\nb\n:::\n\np\n"),
        // line endings
        ("p\r\n\r\n:::x\r\nb‸\r\n:::\r\n", true, ":::x\r\nb\r\n:::\r\n\r\np\r\n"),
    ] {
        assert_eq!(moved(marked, up), want, "{marked:?}");
    }
}

#[test]
fn its_bytes_stay_exactly_as_they_are() {
    // odd spacing, trailing blanks on the closer, ids, an indented opener
    let block = "  :::poll  odd   args\n\n\n## Q {#q}\n- [x] A {#a1}\n\n\n- [ ] B {#a2}\n:::   ";
    let doc = format!("p\n\n{block}\n\nq\n");
    let mut s = surface(&doc);
    s.caret(doc.find('Q').unwrap());
    s.ed.move_block(false, false).unwrap();
    assert_eq!(s.text(), format!("p\n\nq\n\n{block}\n"));
    s.ed.move_block(true, false).unwrap();
    s.ed.move_block(true, false).unwrap();
    assert_eq!(s.text(), format!("{block}\n\np\n\nq\n"));
}

#[test]
fn a_nested_block_moves_within_its_parent() {
    let a = ":::a\n\none\n\n:::";
    let b = ":::b\n\ntwo\n\n:::";
    let doc = format!("before\n\n::::p\n\n{a}\n\n{b}\n\n::::\n\nafter\n");
    let mut s = surface(&doc);
    s.caret(doc.find("two").unwrap());
    assert!(s.ed.can_move_block(true));
    assert!(!s.ed.can_move_block(false), "the parent's end is its limit");
    s.ed.move_block(true, false).unwrap();
    assert_eq!(s.text(), format!("before\n\n::::p\n\n{b}\n\n{a}\n\n::::\n\nafter\n"));
    assert!(!s.ed.can_move_block(true), "the parent's start too");
    // the parent moves whole, from its own lines
    let mut s = surface(&doc);
    s.caret(doc.find("::::p").unwrap());
    s.ed.move_block(false, false).unwrap();
    assert_eq!(s.text(), format!("before\n\nafter\n\n::::p\n\n{a}\n\n{b}\n\n::::\n"));
}

#[test]
fn front_matter_stays_first_and_definitions_are_gone_past() {
    let doc = "---\nt: 1\n---\n\n:::x\n\nb\n\n:::\n";
    let mut s = surface(doc);
    s.caret(doc.find('b').unwrap());
    assert!(!s.ed.can_move_block(true));
    assert_eq!(s.ed.move_block(true, false), Ok(false));
    assert_eq!(s.text(), doc);
    // a definition does not show: a move that only passed it would look like nothing
    let out = moved("p\n\n[r]: /url\n\n:::x\n\nb‸\n\n:::\n", true);
    assert_eq!(out, ":::x\n\nb\n\n:::\n\np\n\n[r]: /url\n");
}

#[test]
fn outside_a_block_or_at_an_end_there_is_nothing_to_move() {
    let doc = "p\n\n:::x\n\nb\n\n:::\n";
    let mut s = surface(doc);
    s.caret(0);
    assert!(!s.ed.can_move_block(true) && !s.ed.can_move_block(false));
    s.caret(doc.find('b').unwrap());
    assert!(!s.ed.can_move_block(false));
    assert_eq!(s.ed.move_block(false, false), Ok(false));
    // the virtual paragraph after a closer is outside it
    s.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(doc.len() - 1, 1)));
    assert!(!s.ed.can_move_block(true));
}

#[test]
fn it_is_one_undo_step_and_the_caret_stays_in_the_block() {
    let doc = "p\n\n:::x\n\nbody\n\n:::\n\nq\n";
    let mut s = surface(doc);
    let at = doc.find("body").unwrap() + 2;
    s.caret(at);
    let steps = s.ed.session.history().undo_len();
    s.ed.move_block(false, false).unwrap();
    assert_eq!(s.ed.session.history().undo_len(), steps + 1);
    assert_eq!(&s.text()[s.offset()..s.offset() + 2], "dy");
    s.undo();
    assert_eq!(s.text(), doc);
    assert_eq!(s.offset(), at);
    s.key("edit.redo");
    assert_eq!(s.text(), "p\n\nq\n\n:::x\n\nbody\n\n:::\n");
}

#[test]
fn a_move_that_would_change_how_blocks_pair_is_refused() {
    // `b` ends the waiting `s`; with `b` above it, `s` would pair with the last line
    let doc = ":::s\n\n::::b\n\nx\n\n::::\n\n:::\n";
    let mut s = surface(doc);
    s.caret(doc.find('x').unwrap());
    let steps = s.ed.session.history().undo_len();
    assert_eq!(s.ed.move_block(true, false), Err(MoveRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
    assert_eq!(s.ed.session.history().undo_len(), steps);
    assert_eq!(s.offset(), doc.find('x').unwrap());
}

#[test]
fn a_blocks_own_view_goes_with_it() {
    let doc = "p\n\n:::tip\n\nHello\n\n:::\n\nq\n";
    let mut s = surface(doc);
    let opener = doc.find(":::tip").unwrap();
    s.ed.switch_block_view(opener);
    assert_eq!(s.ed.view.block_mode_of(opener), BlockViewMode::Markdown);
    s.caret(doc.find("Hello").unwrap());
    s.ed.move_block(false, false).unwrap();
    let now = s.text().find(":::tip").unwrap();
    assert_eq!(s.ed.view.block_mode_of(now), BlockViewMode::Markdown);
}

#[test]
fn drop_lines_are_at_the_blocks_level_and_not_where_it_is() {
    let doc = "---\nt: 1\n---\n\np\n\n:::x\n\nb\n\n:::\n\nq\n\n::::y\n\n:::z\n\nw\n\n:::\n\n::::\n\nr\n";
    let s = surface(doc);
    let x = doc.find(":::x").unwrap();
    let line = |needle: &str| doc.find(needle).unwrap();
    assert_eq!(s.ed.drop_line(x, line("p\n")), Some(line("p\n")));
    assert_eq!(s.ed.drop_line(x, line("::::y")), Some(line("::::y")));
    assert_eq!(s.ed.drop_line(x, line("r\n")), Some(line("r\n")));
    assert_eq!(s.ed.drop_line(x, 0), None, "not before front matter");
    assert_eq!(s.ed.drop_line(x, x), None, "where it is");
    assert_eq!(
        s.ed.drop_line(x, line("q\n")),
        None,
        "right after it is where it is too"
    );
    assert_eq!(s.ed.drop_line(x, line("b\n")), None, "not inside itself");
    assert_eq!(s.ed.drop_line(x, line(":::z")), None, "not inside another block");
    assert_eq!(s.ed.drop_line(x, line("w\n")), None);
    assert!(s.ed.drop_line_at_end(x).is_some());
    // a nested block stays in its parent
    let z = line(":::z");
    assert_eq!(s.ed.drop_line(z, line("p\n")), None);
    assert_eq!(s.ed.drop_line(z, line("r\n")), None);
    assert_eq!(s.ed.drop_line_at_end(z), None, "it is last already");
}

#[test]
fn it_works_by_drop_line_to_the_end() {
    let doc = ":::x\n\nb\n\n:::\n\np\n\nq\n";
    let mut s = surface(doc);
    let to = s.ed.drop_line_at_end(0).unwrap();
    assert_eq!(s.ed.move_block_to(0, to), Ok(true));
    assert_eq!(s.text(), "p\n\nq\n\n:::x\n\nb\n\n:::\n");
    let opener = s.text().find(":::x").unwrap();
    assert_eq!(s.ed.drop_line_at_end(opener), None);
    let top = s.ed.drop_line(opener, 0).unwrap();
    assert_eq!(s.ed.move_block_to(opener, top), Ok(true));
    assert_eq!(s.text(), ":::x\n\nb\n\n:::\n\np\n\nq\n");
}

#[test]
fn it_works_in_a_large_paged_document() {
    let mut doc = String::new();
    for i in 0..4000 {
        doc.push_str(&format!("Paragraph {i}\n\n"));
        if i % 500 == 250 {
            doc.push_str(&format!(":::x\n\n## Block {i} {{#b{i}}}\n\n:::\n\n"));
        }
    }
    let mut t = T::with_paging(&doc, Paging::Paged { tops: 64 });
    let needle = "## Block 2250";
    let at = doc.find(needle).unwrap();
    t.ed.set_selection(Selection::caret(at + 5));
    t.reveal();
    assert_eq!(t.ed.move_block(true, false), Ok(true));
    t.frame();
    let text = t.ed.session.text().to_string();
    assert!(text.contains(":::x\n\n## Block 2250 {#b2250}\n\n:::\n\nParagraph 2250\n"));
    assert_eq!(text.len(), doc.len());
    assert_eq!(
        &text[t.ed.session.selection().primary().focus.offset - 5..][..5],
        "## Bl"
    );
    // far away, in one step, across pages that are not laid out
    let opener = text.find(":::x\n\n## Block 2250").unwrap();
    let last = text.rfind("Paragraph 3999").unwrap();
    assert_eq!(t.ed.move_block_to(opener, last), Ok(true));
    t.frame();
    let text = t.ed.session.text().to_string();
    assert!(
        text.ends_with(":::x\n\n## Block 2250 {#b2250}\n\n:::\n\nParagraph 3999\n\n"),
        "{}",
        &text[text.len() - 90..]
    );
    t.ed.edit(|s| s.undo());
    t.ed.edit(|s| s.undo());
    t.frame();
    assert_eq!(t.ed.session.text(), doc);
}

#[test]
fn a_line_that_is_no_longer_there_moves_nothing() {
    let doc = ":::x\n\nb\n\n:::\n\nsome text to go\n";
    let mut s = surface(doc);
    let line = s.ed.drop_line_at_end(0).unwrap();
    s.ed.edit(|e| e.source_edit("Cut", markise_doc::PatchSet::single_verbatim(12..doc.len(), ""), None))
        .unwrap();
    let now = s.text();
    assert_eq!(s.ed.move_block_to(0, line), Ok(false));
    assert_eq!(s.text(), now);
    // nor one that is not a boundary at the block's level
    let doc = ":::x\n\nb\n\n:::\n\none\ntwo\n\nthree\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.move_block_to(0, doc.find("two").unwrap()), Ok(false));
    assert_eq!(s.ed.move_block_to(0, doc.find("wo\n").unwrap()), Ok(false));
    assert_eq!(s.text(), doc);
}

#[test]
fn a_no_break_space_is_text_and_a_move_keeps_it() {
    let doc = ":::x\nb\n:::\n\n\u{00a0}\n\nq\n";
    let mut s = surface(doc);
    let line = s.ed.drop_line_at_end(0).unwrap();
    assert_eq!(s.ed.move_block_to(0, line), Ok(true));
    assert_eq!(s.text(), "\u{00a0}\n\nq\n\n:::x\nb\n:::\n");
}

#[test]
fn a_list_is_one_block_for_a_drop() {
    let doc = ":::x\n\nb\n\n:::\n\np\n\n- one\n- two\n- three\n\n> q1\n> q2\n\nz\n";
    let s = surface(doc);
    let line = |n: &str| doc.find(n).unwrap();
    assert_eq!(s.ed.drop_line(0, line("- one")), Some(line("- one")));
    assert_eq!(
        s.ed.drop_line(0, line("one")),
        Some(line("- one")),
        "the text of its first item is its first line"
    );
    for inside in ["two", "- two", "- three", "q2", "> q2"] {
        assert_eq!(s.ed.drop_line(0, line(inside)), None, "{inside}");
    }
}

const PIECES: [&str; 10] = [
    "\u{00a0}\n",
    "a paragraph\nof two lines\n",
    "## A heading {#h1}\n",
    "- one\n- two\n",
    "> a quote\n",
    "```\ncode\n```\n",
    ":::tip\n\nBody\n\n:::\n",
    ":::poll\n\n## Q {#q}\n\n- [x] A {#a1}\n- [ ] B {#a2}\n\n:::\n",
    "::::plain\n\n:::tip\n\nInner\n\n:::\n\n:::steps\n\n- Item\n\n:::\n\n::::\n",
    "| x |\n|---|\n| y |\n",
];
const PRESETS: [(&str, &str); 3] = [
    (":::tip\n\nNew\n\n:::\n", "tip"),
    (
        ":::poll\n\n## Which? {#q}\n\n- [ ] One {#a1}\n- [ ] Two {#a2}\n\n:::\n",
        "poll",
    ),
    (":::plain\n\n## X {#a1}\n\n:::\n", "plain"),
];

/// The lines with something on them, in no order: what must not be lost or doubled.
/// Lines by any ending: LF, CRLF or a lone CR.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\r', '\n'])
}

fn lines(text: &str) -> Vec<String> {
    let mut out: Vec<_> = split_lines(text)
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    out.sort();
    out
}

/// How many of the trailing ids repeat another.
fn repeats(text: &str) -> usize {
    let mut ids: Vec<&str> = split_lines(text)
        .filter_map(|l| l.trim_end().strip_suffix('}')?.rsplit_once("{#").map(|x| x.1))
        .collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    n - ids.len()
}

#[derive(Clone, Debug)]
enum Op {
    Insert(prop::sample::Index, usize),
    Move(prop::sample::Index, bool),
    Drop(prop::sample::Index, prop::sample::Index),
    Delete(prop::sample::Index),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        1 => (any::<prop::sample::Index>(), 0..PRESETS.len()).prop_map(|(a, b)| Op::Insert(a, b)),
        3 => (any::<prop::sample::Index>(), any::<bool>()).prop_map(|(a, b)| Op::Move(a, b)),
        2 => (any::<prop::sample::Index>(), any::<prop::sample::Index>()).prop_map(|(a, b)| Op::Drop(a, b)),
        2 => any::<prop::sample::Index>().prop_map(Op::Delete),
    ]
}

fn boundary(text: &str, at: prop::sample::Index) -> usize {
    let i = at.index(text.len().max(1));
    (0..=i).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0)
}

fn deleted(marked: &str) -> (String, usize) {
    let at = marked.find('‸').unwrap();
    let text = marked.replace('‸', "");
    let mut s = surface(&text);
    s.caret(at);
    let opener = s.ed.block_opener_at_caret().expect(marked);
    assert_eq!(s.ed.delete_block(opener, false), Ok(true), "{marked:?}");
    let (out, caret) = (s.text(), s.offset());
    assert!(s.ed.session.selection().primary().is_caret());
    s.undo();
    assert_eq!(s.text(), text, "one undo step, exact");
    (out, caret)
}

#[test]
fn deleting_takes_the_block_and_the_blank_lines_it_leaves_and_nothing_else() {
    for (marked, want, caret) in [
        // between text: a blank line stays, the caret at the end of the text before
        ("a\n\n:::x\n\nb‸\n\n:::\n\nc\n", "a\n\nc\n", 1),
        // first and last in the file
        (":::x\nb‸\n:::\n\nc\n", "c\n", 0),
        ("a\n\n:::x\nb‸\n:::\n", "a\n", 1),
        ("a\n\n:::x\nb‸\n:::", "a", 1),
        // the only thing there is
        (":::x\nb‸\n:::\n", "\n", 0),
        // no blank lines around it: what it held apart stays apart
        ("a\n:::x\nb‸\n:::\nc\n", "a\n\nc\n", 1),
        // many: they do not pile up, and spaces and tabs count
        ("a\n\n\n:::x\nb‸\n:::\n \t\n\n\nc\n", "a\n\n\nc\n", 1),
        // line endings
        ("a\r\n\r\n:::x\r\nb‸\r\n:::\r\n\r\nc\r\n", "a\r\n\r\nc\r\n", 1),
        // odd bytes of its own go with it
        ("a\n\n  :::x  \nb‸\n:::   \n\nc\n", "a\n\nc\n", 1),
    ] {
        let (out, at) = deleted(marked);
        assert_eq!(out, want, "{marked:?}");
        assert_eq!(at, caret, "{marked:?}");
    }
}

#[test]
fn after_a_block_before_it_the_caret_is_on_the_free_line_there() {
    use markise_doc::DocPos;
    for marked in [
        ":::y\nq\n:::\n\n:::x\nb‸\n:::\n\nc\n",
        ":::y\nq\n:::\n\n:::x\nb‸\n:::\n\n:::z\nr\n:::\n",
        ":::y\nq\n:::\n\n:::x\nb‸\n:::\n",
    ] {
        let at = marked.find('‸').unwrap();
        let mut s = surface(&marked.replace('‸', ""));
        s.caret(at);
        let opener = s.ed.block_opener_at_caret().unwrap();
        assert_eq!(s.ed.delete_block(opener, false), Ok(true));
        assert_eq!(
            s.ed.session.selection().primary().focus,
            DocPos::with_virt(10, 1),
            "{marked:?}"
        );
    }
}

#[test]
fn every_line_ending_keeps_what_stands_around_it_for_delete_move_and_insert() {
    // a lone CR
    assert_eq!(
        deleted("Before.\r\r:::x\rBo‸dy\r:::\r\rAfter.\r").0,
        "Before.\r\rAfter.\r"
    );
    assert_eq!(deleted("a\r:::x\rb‸\r:::\rc\r").0, "a\r\rc\r");
    assert_eq!(deleted("a\r\r:::x\rb‸\r:::\r").0, "a\r");
    // mixed
    assert_eq!(deleted("a\n\r\n:::x\rb‸\n:::\r\n\rc\n").0, "a\n\r\nc\n");
    assert_eq!(moved("a\r\r:::x\r\rb‸\r\r:::\r", true), ":::x\r\rb\r\r:::\r\ra\r");
    assert_eq!(moved(":::x\r\rb‸\r\r:::\r\ra\r", false), "a\r\r:::x\r\rb\r\r:::\r");
    assert_eq!(
        moved("a\n\r\n:::x\r\nb‸\r\n:::\r\n", true),
        ":::x\r\nb\r\n:::\r\n\r\na\r\n"
    );
    let mut s = surface("Before.\r\r:::x\rBody\r:::\r\rAfter.\r");
    s.caret(2);
    let at = s.text().find("After.").unwrap();
    s.caret(at);
    s.ed.insert_block("y", ":::y\nn\n:::\n", false).unwrap();
    assert_eq!(s.text(), "Before.\r\r:::x\rBody\r:::\r\rAfter.\r\r:::y\rn\r:::\r");
    s.undo();
    assert_eq!(s.text(), "Before.\r\r:::x\rBody\r:::\r\rAfter.\r");
}

#[test]
fn a_no_break_space_is_text_and_stays() {
    let (out, _) = deleted("a\n\n:::x\nb‸\n:::\n\u{a0}\n\nc\n");
    assert_eq!(out, "a\n\n\u{a0}\n\nc\n");
}

#[test]
fn a_nested_block_goes_and_its_parent_stays_a_parent_takes_what_is_inside() {
    let doc = "::::p\n\n:::x\nb‸\n:::\n\n:::y\nc\n:::\n\n::::\n";
    let (out, _) = deleted(doc);
    assert_eq!(out, "::::p\n\n:::y\nc\n:::\n\n::::\n");
    let (out, _) = deleted("::::p\n\nt\n\n:::x\nb‸\n:::\n\n::::\n");
    assert_eq!(out, "::::p\n\nt\n\n::::\n");
    // from the parent's own text, the parent whole
    let (out, _) = deleted("z\n\n::::p\n\nt‸\n\n:::x\nb\n:::\n\n::::\n\nq\n");
    assert_eq!(out, "z\n\nq\n");
}

#[test]
fn deleting_refuses_what_would_pair_differently_and_changes_nothing() {
    // stray markers around `b`: with it gone, the first line would pair with the last
    let text = ":::s\n\n::::b\n\n::::\n\n:::\n";
    let mut s = surface(text);
    let opener = text.find("::::b").unwrap();
    let undos = s.ed.session.history().undo_len();
    assert_eq!(s.ed.delete_block(opener, false), Err(MoveRefused::WouldNotStand));
    assert_eq!(s.text(), text);
    assert_eq!(s.ed.session.history().undo_len(), undos);
    // no block there
    assert_eq!(s.ed.delete_block(1, false), Ok(false));
}

#[test]
fn a_state_of_its_own_goes_with_the_block_and_the_next_inherits_nothing() {
    use markise_layout::BlockViewMode;
    let text = ":::x\nb\n:::\n\n:::y\nc\n:::\n";
    let mut s = surface(text);
    s.ed.restore_block_view_state(BlockViewMode::Native, &[(0, BlockViewMode::Markdown)]);
    assert_eq!(s.ed.block_view_state().1, vec![(0, BlockViewMode::Markdown)]);
    assert_eq!(s.ed.delete_block(0, false), Ok(true));
    assert_eq!(s.text(), ":::y\nc\n:::\n");
    assert_eq!(s.ed.block_view_state().1, Vec::new());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/block_move.txt",
        ))),
        ..ProptestConfig::default()
    })]

    /// Sequences of insertions and moves keep every other line of the document and the bytes of the
    /// block moved, never repeat an id that was not repeated, and undo gives back the exact source.
    #[test]
    fn inserts_and_moves_keep_every_other_block_and_undo_restores_the_source(
        pieces in prop::collection::vec(0..PIECES.len(), 1..7),
        gaps in prop::collection::vec(0..4usize, 1..4),
        ops in prop::collection::vec(op(), 1..10),
        endings in 0..4usize,
    ) {
        let mut doc = String::new();
        for (i, p) in pieces.iter().enumerate() {
            doc.push_str(PIECES[*p]);
            doc.push_str(&"\n".repeat(gaps[i % gaps.len()]));
        }
        // every line ending a file can have, mixed too
        let doc = match endings {
            0 => doc,
            1 => doc.replace('\n', "\r\n"),
            2 => doc.replace('\n', "\r"),
            _ => {
                let mut mixed = String::new();
                for (i, part) in doc.split('\n').enumerate() {
                    if i > 0 {
                        mixed.push_str(["\n", "\r\n", "\r"][i % 3]);
                    }
                    mixed.push_str(part);
                }
                mixed
            }
        };
        let mut s = surface(&doc);
        let start = s.ed.session.history().undo_len();
        let mut states = vec![doc.clone()];
        for op in ops {
            let before = s.text();
            match op {
                Op::Insert(at, preset) => {
                    s.ed.session.set_selection(Selection::caret(boundary(&before, at)));
                    let (markdown, block) = PRESETS[preset];
                    let Ok(_) = s.ed.insert_block(block, markdown, false) else {
                        prop_assert_eq!(s.text(), before);
                        continue;
                    };
                    let mut want = lines(&before);
                    want.extend(lines(markdown));
                    want.sort();
                    // only the numbers of ids differ
                    let strip = |l: Vec<String>| -> Vec<String> {
                        let mut out: Vec<_> = l.into_iter().map(|x| x.split(" {#").next().unwrap().to_string()).collect();
                        out.sort();
                        out
                    };
                    prop_assert_eq!(strip(lines(&s.text())), strip(want));
                }
                Op::Move(at, up) => {
                    s.ed.session.set_selection(Selection::caret(boundary(&before, at)));
                    let c = s.ed.session.doc().named_containers_at(s.offset()).pop();
                    let moved = s.ed.move_block(up, false);
                    if moved != Ok(true) {
                        prop_assert_eq!(s.text(), before);
                        continue;
                    }
                    let c = c.unwrap();
                    prop_assert!(s.text().contains(&before[c.opener.start..c.closer.end]));
                    prop_assert_eq!(lines(&s.text()), lines(&before));
                }
                Op::Drop(block, target) => {
                    let at = boundary(&before, block);
                    let Some(c) = s.ed.session.doc().named_containers_at(at).pop() else { continue };
                    let item = boundary(&before, target);
                    let top = s.ed.session.doc().top_at(item).and_then(|t| s.ed.session.doc().top_span(t));
                    let Some(top) = top else { continue };
                    let Some(line) = s.ed.drop_line(c.opener.start, top.start) else { continue };
                    if s.ed.move_block_to(c.opener.start, line) != Ok(true) {
                        prop_assert_eq!(s.text(), before);
                        continue;
                    }
                    prop_assert!(s.text().contains(&before[c.opener.start..c.closer.end]));
                    prop_assert_eq!(lines(&s.text()), lines(&before));
                }
                Op::Delete(block) => {
                    let at = boundary(&before, block);
                    let Some(c) = s.ed.session.doc().named_containers_at(at).pop() else { continue };
                    let undos = s.ed.session.history().undo_len();
                    if s.ed.delete_block(c.opener.start, false) != Ok(true) {
                        prop_assert_eq!(s.text(), before);
                        prop_assert_eq!(s.ed.session.history().undo_len(), undos);
                        continue;
                    }
                    prop_assert_eq!(s.ed.session.history().undo_len(), undos + 1);
                    // every other line is there, as it was; the block's lines, and blank ones, are not
                    let mut want = lines(&before);
                    for gone in lines(&before[c.opener.start..c.closer.end]) {
                        let i = want.iter().position(|l| *l == gone).unwrap();
                        want.remove(i);
                    }
                    prop_assert_eq!(lines(&s.text()), want);
                    prop_assert!(s.ed.session.selection().primary().focus.offset <= s.text().len());
                    states.push(s.text());
                    continue;
                }
            }
            prop_assert_eq!(repeats(&s.text()), repeats(&before), "{}", s.text());
            states.push(s.text());
        }
        // one undo step each, back through every state, to the exact source
        while s.ed.session.history().undo_len() > start {
            prop_assert_eq!(s.text(), states.pop().unwrap());
            s.undo();
        }
        prop_assert_eq!(s.text(), doc);
    }
}
