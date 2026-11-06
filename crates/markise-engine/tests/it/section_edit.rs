//! Starting a section (ADR 0026): where it goes, the bytes around it, one undo step, and that it refuses
//! rather than changes what stands around it.

use markise_doc::{DocPos, Selection};
use markise_engine::editor::{InsertRefused, NewSection};

use super::block_inplace::surface;

fn new(title: &str) -> NewSection {
    NewSection {
        title: title.to_string(),
        id: "s9".to_string(),
        metadata: Vec::new(),
    }
}

fn inserted(marked: &str, section: &NewSection) -> String {
    let at = marked.find('‸').unwrap();
    let text = marked.replace('‸', "");
    let mut s = surface(&text);
    s.caret(at);
    s.ed.insert_section(section, false).unwrap();
    let after = s.text();
    s.undo();
    assert_eq!(s.text(), text, "one undo step: {marked:?}");
    after
}

#[test]
fn it_goes_at_the_document_level_with_blank_lines_around_it() {
    for (marked, want) in [
        // at the end of what the caret is in
        ("One‸.\n\nTwo.\n", "One.\n\n---\n\n## T {#s9}\n\nTwo.\n"),
        ("One.\n\nTwo‸.\n", "One.\n\nTwo.\n\n---\n\n## T {#s9}\n"),
        // on the blank line it is on
        ("One.\n\n‸\nTwo.\n", "One.\n\n---\n\n## T {#s9}\n\nTwo.\n"),
        // a file ending with no line ending stays so
        ("One‸.", "One.\n\n---\n\n## T {#s9}"),
        // right above a boundary with no blank line: a boundary of its own still
        ("# H‸\n---\n\nB\n", "# H\n\n---\n\n## T {#s9}\n\n---\n\nB\n"),
        // never inside a block or a list: after the outermost
        (
            "::::a\n\n:::b\n\nIn‸.\n\n:::\n\n::::\n\nNext.\n",
            "::::a\n\n:::b\n\nIn.\n\n:::\n\n::::\n\n---\n\n## T {#s9}\n\nNext.\n",
        ),
        ("- a‸\n- b\n\nNext.\n", "- a\n- b\n\n---\n\n## T {#s9}\n\nNext.\n"),
        ("> a‸\n> b\n", "> a\n> b\n\n---\n\n## T {#s9}\n"),
    ] {
        assert_eq!(inserted(marked, &new("T")), want, "{marked:?}");
    }
}

#[test]
fn nothing_before_it_needs_no_boundary_before_it() {
    for (marked, want) in [
        // an empty document, whose first line would be taken for front matter one day
        ("‸", "## T {#s9}\n"),
        // what follows stays a section of its own
        ("‸\nOne.\n", "## T {#s9}\n\n---\n\nOne.\n"),
        // after front matter
        (
            "---\nlang: en\n---\n\n‸\nOne.\n",
            "---\nlang: en\n---\n\n## T {#s9}\n\n---\n\nOne.\n",
        ),
        ("---\nlang: en\n---\n‸", "---\nlang: en\n---\n\n## T {#s9}\n"),
        // an empty section takes the heading, with the boundary after it there already
        ("A\n\n---\n\n‸\n---\n\nB\n", "A\n\n---\n\n## T {#s9}\n\n---\n\nB\n"),
    ] {
        assert_eq!(inserted(marked, &new("T")), want, "{marked:?}");
    }
}

#[test]
fn metadata_comes_first_and_only_when_there_is_some() {
    let with = NewSection {
        metadata: vec![
            ("style".to_string(), "wide".to_string()),
            ("kind".to_string(), "x y".to_string()),
        ],
        ..new("T")
    };
    assert_eq!(
        inserted("One‸.\n", &with),
        "One.\n\n---\n\n:::section-metadata\n\nstyle: wide\nkind: x y\n\n:::\n\n## T {#s9}\n"
    );
    let at = "‸Text.\n";
    assert!(inserted(at, &with).starts_with("Text.\n\n---\n\n:::section-metadata"));
}

#[test]
fn line_endings_follow_the_document_and_a_title_is_one_line() {
    assert_eq!(
        inserted("One‸.\r\n\r\nTwo.\r\n", &new("T")),
        "One.\r\n\r\n---\r\n\r\n## T {#s9}\r\n\r\nTwo.\r\n"
    );
    assert_eq!(
        inserted("One‸.\n", &new("  a \n\t b ")),
        "One.\n\n---\n\n## a b {#s9}\n"
    );
}

#[test]
fn the_caret_ends_at_the_end_of_the_title_and_the_id_is_not_text() {
    let mut s = surface("One.\n");
    s.caret(1);
    let at = s.ed.insert_section(&new("Title {#x}"), false).unwrap();
    assert_eq!(s.text(), "One.\n\n---\n\n## Title {#x} {#s9}\n");
    assert_eq!(at, "One.\n\n---\n\n".len());
    let caret = s.ed.session.selection().primary().focus;
    assert_eq!(caret, DocPos::new(at + "## Title {#x}".len()));
    let doc = s.ed.session.doc();
    assert_eq!(doc.section_count(), 2);
    assert_eq!(doc.section_at(at).index, 1);
}

#[test]
fn a_selection_counts_as_a_caret_at_its_end_and_is_not_replaced() {
    let mut s = surface("One.\n\nTwo.\n");
    s.ed.session.set_selection(Selection::range(1, "One.\n\nTw".len()));
    s.ed.insert_section(&new("T"), false).unwrap();
    assert_eq!(s.text(), "One.\n\nTwo.\n\n---\n\n## T {#s9}\n");
}

#[test]
fn what_would_change_around_it_is_refused_and_nothing_changes() {
    // an unclosed fence holds what is put after it
    let doc = "```\ncode\n";
    let mut s = surface(doc);
    s.caret(4);
    assert_eq!(s.ed.insert_section(&new("T"), false), Err(InsertRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
    // a first line rule and a second one would be front matter
    let doc = "---\n\nOne.\n";
    let mut s = surface(doc);
    s.caret(6);
    assert_eq!(s.ed.insert_section(&new("T"), false), Err(InsertRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
}
