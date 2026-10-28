//! Cross-block risks fixed by the block-ownership query (ADR 0011): every join, deletion and
//! virtual-paragraph placement takes its neighbour from the parsed structure and never reaches
//! past a block without text (an empty quote `>`, an empty list item). Headless surface;
//! notation: `‸` caret, `(v)` virtual paragraph.

mod common;
use common::T;

fn script(src: &str, steps: &[&str], want: &str) {
    let mut t = T::at(src);
    for k in steps {
        if let Some(s) = k.strip_prefix("type:") {
            t.typ(s);
        } else {
            t.run(k);
        }
        t.check_model();
    }
    assert_eq!(t.shown(), want, "{src:?} after {steps:?}");
    assert!(t.selection_on_stops(), "selection off the stops: {}", t.shown());
}

#[test]
fn hinted_empty_heading_enter_keeps_an_empty_quote_before_it() {
    // `## ` typed as paragraph text (a literal hint), emptied, Enter (C43): the empty quote
    // before it used to be deleted together with the line
    script(
        "Before\n\n>\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline", "type:x"],
        "Before\n\n>\n\nx‸",
    );
    // C43 in place: the markers go, the empty paragraph is the virtual one after the quote
    script(
        "Before\n\n>\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline"],
        "Before\n\n>(v)‸",
    );
}

#[test]
fn hinted_empty_heading_enter_after_a_quote_leaves_the_quote() {
    // the virtual paragraph after the quote's last paragraph used to open inside the quote
    script(
        "> q\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline", "type:x"],
        "> q\n\nx‸",
    );
    script(
        "- q\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline", "type:x"],
        "- q\n\nx‸",
    );
}

#[test]
fn list_item_paragraph_join_stops_at_an_empty_block() {
    // Backspace at the start of an item's later paragraph used to merge with the paragraph
    // before an empty quote / empty nested item, deleting it
    script("- a\n\n  >\n\n  ‸b\n", &["edit.deleteBackward"], "- a\n\n  >\n\n  ‸b\n");
    script(
        "- a\n\n  - \n\n  ‸b\n",
        &["edit.deleteBackward"],
        "- a\n\n  - \n\n  ‸b\n",
    );
    // a plain join still works
    script("- a\n\n  ‸b\n", &["edit.deleteBackward"], "- a‸b\n");
}

#[test]
fn deleting_the_final_block_keeps_the_block_before_it() {
    // Delete Block on the last block takes the separator before it — up to the previous
    // block's end, never into an empty quote
    script("x\n\n>\n\nab‸c\n", &["delete.block"], "x\n\n>‸\n");
    script("x\n\nab‸c\n", &["delete.block"], "x‸\n");
}

#[test]
fn virtual_paragraph_never_skips_a_block() {
    // Enter at the start of a heading after an empty quote: the virtual paragraph "above"
    // used to open after the paragraph before the quote
    script(
        "Before\n\n>\n\n## ‸Title\n",
        &["edit.newline"],
        "Before\n\n>\n\n## ‸Title\n",
    );
    script("Before\n\n## ‸Title\n", &["edit.newline"], "Before(v)‸\n\n## Title\n");
    // Enter on an empty first list item after a quote: the paragraph goes before the list,
    // outside the quote (it used to open inside it)
    script("> q\n\n- ‸\n- b\n", &["edit.newline", "type:x"], "> q\n\nx‸\n\n- b\n");
}
