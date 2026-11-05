//! What the document says about the caret's place, for a host offering blocks (ADR 0022).

use markise_doc::{DocPos, Selection};

use crate::common::T;

fn free_at(text: &str, pos: DocPos) -> bool {
    let mut t = T::new(text);
    t.ed.session.set_selection(Selection::caret_at(pos));
    t.ed.on_free_line()
}

#[test]
fn a_free_line_is_empty_at_the_documents_level_and_in_no_block() {
    let text = ":::a\n\n:::\n\npara\n\n- item\n";
    let virt = |needle: &str| DocPos::with_virt(text.find(needle).unwrap() + needle.len(), 1);
    assert!(free_at("", DocPos::new(0)), "an empty document");
    assert!(free_at(text, virt("para")), "after a paragraph");
    assert!(
        free_at(text, DocPos::with_virt(":::a\n\n:::".len(), 1)),
        "after a closer"
    );
    assert!(!free_at(text, virt(":::a")), "inside a block");
    assert!(!free_at(text, virt("- item")), "inside a list item");
    assert!(free_at(text, DocPos::with_virt(text.len() - 1, 3)), "out of the list");
    assert!(
        !free_at(text, DocPos::new(text.find("para").unwrap())),
        "a line with text"
    );
    assert!(!free_at("```\n\n```\n", DocPos::new(4)), "an empty line of code");
    assert!(!free_at("- \n", DocPos::new(2)), "an empty item");
}

#[test]
fn a_selection_or_a_read_only_document_has_no_free_line() {
    let mut t = T::new("a\n");
    t.ed.session
        .set_selection(Selection::single(DocPos::new(0), DocPos::new(1)));
    assert!(!t.ed.on_free_line());
    t.ed.session.set_selection(Selection::caret_at(DocPos::with_virt(1, 1)));
    assert!(t.ed.on_free_line());
    t.ed.session.set_read_only(Some("a preview"));
    assert!(!t.ed.on_free_line());
}
