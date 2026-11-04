//! Editing around the marker lines of `:::name` blocks (ADR 0018): the keyboard never merges
//! text into a marker, never splits one and never leaves half a pair behind; everything inside
//! edits as usual; undo restores the source. Notation: `‸` caret, `⟦…⟧` selection.

use crate::common;
use common::T;
use markise_doc::{DocPos, Selection};
use proptest::prelude::*;

/// Opens `src` with `‸` marking the caret or `⟦…⟧` the selection.
fn open(src: &str) -> T {
    if src.contains('‸') {
        return T::at(src);
    }
    let a = src.find('⟦').expect("selection");
    let z = src.find('⟧').expect("selection") - '⟦'.len_utf8();
    let mut t = T::new(&src.replace(['⟦', '⟧'], ""));
    t.ed.session.set_selection(Selection::range(a, z));
    t
}

fn script(src: &str, steps: &[(&str, &str)]) {
    let mut t = open(src);
    for (k, want) in steps {
        if let Some(s) = k.strip_prefix("type:") {
            t.typ(s);
        } else {
            t.run(k);
        }
        assert_eq!(t.shown(), *want, "after {k} (from {src:?})");
    }
}

#[test]
fn backspace_at_the_first_line_inside_does_not_merge_into_the_opener() {
    script(
        ":::note\n\n‸text\n\n:::\n",
        &[("edit.deleteBackward", ":::note\n\n‸text\n\n:::\n")],
    );
    // without blank lines too
    script(
        ":::note\n‸text\n:::\n",
        &[("edit.deleteBackward", ":::note\n‸text\n:::\n")],
    );
}

#[test]
fn delete_at_the_last_line_inside_does_not_merge_the_closer() {
    script(
        ":::note\n\ntext‸\n\n:::\n",
        &[("edit.deleteForward", ":::note\n\ntext‸\n\n:::\n")],
    );
    script(
        ":::note\ntext‸\n:::\n",
        &[("edit.deleteForward", ":::note\ntext‸\n:::\n")],
    );
}

#[test]
fn backspace_and_delete_next_to_a_block_from_outside() {
    script(
        ":::note\nx\n:::\n\n‸after\n",
        &[("edit.deleteBackward", ":::note\nx\n:::\n\n‸after\n")],
    );
    script(
        "before‸\n\n:::note\nx\n:::\n",
        &[("edit.deleteForward", "before‸\n\n:::note\nx\n:::\n")],
    );
}

#[test]
fn typing_and_deleting_inside_edit_only_the_content() {
    script(
        ":::note\n\nte‸xt\n\n:::\n",
        &[
            ("type:!", ":::note\n\nte!‸xt\n\n:::\n"),
            ("edit.deleteBackward", ":::note\n\nte‸xt\n\n:::\n"),
            ("edit.undo", ":::note\n\nte!‸xt\n\n:::\n"),
        ],
    );
}

#[test]
fn enter_inside_splits_the_paragraph_and_stays_inside() {
    let mut t = T::at(":::note\n\nab‸cd\n\n:::\n");
    t.run("edit.newline");
    assert_eq!(t.text(), ":::note\n\nab\n\ncd\n\n:::\n");
    assert_eq!(t.ed.session.doc().named_containers_at(t.caret().offset).len(), 1);
}

#[test]
fn enter_at_the_end_of_the_last_line_then_typing_stays_inside() {
    let mut t = T::at(":::note\n\ntext‸\n\n:::\n");
    t.run("edit.newline");
    t.typ("x");
    assert_eq!(t.text(), ":::note\n\ntext\n\nx\n\n:::\n");
    t.run("edit.undo");
    assert_eq!(t.text(), ":::note\n\ntext\n\n:::\n");
    // no blank line before the closer: it stays closed
    let mut t = T::at(":::note\ntext‸\n:::\n");
    t.run("edit.newline");
    t.typ("x");
    assert_eq!(t.text(), ":::note\ntext\n\nx\n\n:::\n");
    assert_eq!(t.ed.session.doc().named_containers_at(t.caret().offset).len(), 1);
}

#[test]
fn an_empty_block_takes_typing_on_its_virtual_paragraph() {
    let src = ":::note\n:::\n";
    let mut t = T::new(src);
    let gap = ":::note".len();
    assert_eq!(t.ed.session.doc().gap_virt(gap), Some(1));
    t.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(gap, 1)));
    t.typ("x");
    assert_eq!(t.text(), ":::note\n\nx\n\n:::\n");
    t.run("edit.undo");
    assert_eq!(t.text(), src);
}

#[test]
fn a_paragraph_typed_after_a_block_stands_outside() {
    let src = ":::note\nx\n:::\n\n```\ncode\n```\n";
    let mut t = T::new(src);
    let gap = ":::note\nx\n:::".len();
    assert_eq!(t.ed.session.doc().gap_virt(gap), Some(1));
    t.ed.session
        .set_selection(Selection::caret_at(DocPos::with_virt(gap, 1)));
    t.typ("y");
    assert_eq!(t.text(), ":::note\nx\n:::\n\ny\n\n```\ncode\n```\n");
    assert!(t.ed.session.doc().named_containers_at(t.caret().offset).is_empty());
}

#[test]
fn marker_lines_are_not_edited_by_the_keyboard() {
    for src in [":::no‸te\nx\n:::\n", ":::note\nx\n:‸::\n"] {
        let mut t = T::at(src);
        let plain = t.text().to_string();
        t.typ("z");
        assert_eq!(t.text(), plain, "typing in {src:?}");
        t.run("edit.deleteBackward");
        assert_eq!(t.text(), plain, "Backspace in {src:?}");
        t.run("edit.deleteForward");
        assert_eq!(t.text(), plain, "Delete in {src:?}");
    }
    // Enter does not split a marker: it goes to the virtual paragraph after the line
    let mut t = T::at(":::no‸te\nx\n:::\n");
    t.run("edit.newline");
    assert_eq!(t.text(), ":::note\nx\n:::\n");
    assert_eq!(t.caret(), DocPos::with_virt(":::note".len(), 1));
}

#[test]
fn block_commands_leave_marker_lines_alone() {
    for id in [
        "block.heading1",
        "block.paragraph",
        "block.quote",
        "block.bulletList",
        "block.orderedList",
        "block.taskList",
        "block.insertRule",
        "edit.moveBlockUp",
        "edit.moveBlockDown",
    ] {
        // with the caret on a marker, and with a selection running over one
        let t = T::at("a\n\n:::no‸te\nx\n:::\n\nb\n");
        assert!(t.ed.registry.enabled(&t.ed.session, id).is_err(), "{id} on a marker");
        let t = open("⟦a\n\n:::note\nx⟧\n:::\n\nb\n");
        assert!(t.ed.registry.enabled(&t.ed.session, id).is_err(), "{id} over a marker");
    }
    // a command on the content inside works as anywhere
    script(
        ":::note\n\nte‸xt\n\n:::\n",
        &[("block.heading1", ":::note\n\n# te‸xt\n\n:::\n")],
    );
    // a block moved past a marker leaves the pair intact
    let mut t = T::at(":::note\n\nte‸xt\n\n:::\n");
    t.run("edit.moveBlockDown");
    assert_eq!(markers(&t), (1, 0));
}

#[test]
fn a_selection_cutting_a_block_keeps_the_markers_and_deletes_the_rest() {
    // from before the block into it: the opener stays
    script(
        "one ⟦two\n\n:::note\n\nthree⟧ four\n\n:::\n",
        &[("edit.deleteBackward", "one ‸\n:::note\n four\n\n:::\n")],
    );
    // from inside the block past it: the closer stays
    script(
        ":::note\n\none ⟦two\n\n:::\n\nthree⟧ four\n",
        &[("edit.deleteForward", ":::note\n\none ‸\n:::\n four\n")],
    );
    // from one block into the next: both of the markers in between stay
    script(
        ":::a\nx⟦x\n:::\n\nmid\n\n:::b\ny⟧y\n:::\n",
        &[("edit.deleteBackward", ":::a\nx‸\n:::\n\n:::b\ny\n:::\n")],
    );
    // typing over it: the text goes where the selection started
    script(
        "one ⟦two\n\n:::note\n\nthree⟧ four\n\n:::\n",
        &[
            ("type:Z", "one Z‸\n:::note\n four\n\n:::\n"),
            ("edit.undo", "one ‸\n:::note\n four\n\n:::\n"),
        ],
    );
}

#[test]
fn a_selection_holding_a_whole_block_deletes_it() {
    script(
        "one⟦\n\n:::note\n\nx\n\n:::\n\n⟧two\n",
        &[("edit.deleteBackward", "one‸two\n")],
    );
    script("⟦:::note\nx\n:::⟧\n\ntwo\n", &[("edit.deleteBackward", "‸\n\ntwo\n")]);
}

#[test]
fn enter_after_a_typed_opener_writes_the_closer() {
    let mut t = T::new("");
    t.typ(":::note x");
    assert_eq!(t.text(), ":::note x", "as typed");
    assert!(t.ed.session.doc().named_containers_at(0).is_empty());
    t.run("edit.newline");
    assert_eq!(t.text(), ":::note x\n\n:::");
    t.typ("y");
    assert_eq!(t.text(), ":::note x\n\ny\n\n:::");
    let d = t.ed.session.doc();
    assert_eq!(d.named_containers_at(t.caret().offset).len(), 1);
}

#[test]
fn a_longer_fence_is_kept_by_its_closer() {
    let mut t = T::new("");
    t.typ("::::outer");
    t.run("edit.newline");
    assert_eq!(t.text(), "::::outer\n\n::::");
}

#[test]
fn leaving_a_typed_opener_without_enter_keeps_it_text() {
    let mut t = T::new("first\n\nsecond\n");
    let end = t.text().len();
    t.set(end);
    t.run("edit.newline");
    t.typ(":::note");
    let typed_at = t.text().find(":::note").unwrap();
    // the caret leaves the line
    t.set(2);
    t.ed.session.expire_hints_left_behind();
    assert!(t.text().contains("\\:::note"), "{:?}", t.text());
    assert!(t.ed.session.doc().named_containers_at(typed_at).is_empty());
}

#[test]
fn in_a_quote_the_same_text_is_plain_text_with_no_hint() {
    let mut t = T::at("> ‸\n");
    t.typ(":::a");
    t.set(0);
    t.ed.session.expire_hints_left_behind();
    assert!(t.text().contains(":::a"));
    assert!(!t.text().contains('\\'));
}

#[test]
fn inline_styles_over_a_block_skip_the_markers() {
    let mut t = open("⟦one\n\n:::note\n\ntwo⟧\n\n:::\n");
    t.run("format.strong");
    let d = t.text().to_string();
    assert!(d.contains("\n:::note\n"), "{d:?}");
    assert!(d.contains("\n:::\n"), "{d:?}");
    assert!(d.contains("**"), "{d:?}");
}

// ---- properties ----------------------------------------------------------------------------

/// `(pairs, marker lines that pair with nothing)`.
fn markers(t: &T) -> (usize, usize) {
    let d = markise_md::parse(t.text());
    let stray = d
        .tops()
        .filter(|x| matches!(x.block.kind, markise_md::BlockKind::Paragraph))
        .filter(|x| {
            let s = x.span().slice(t.text());
            s.starts_with(":::") && s.lines().count() == 1
        })
        .count();
    (d.directives().len(), stray)
}

fn doc() -> String {
    "a text\n\n:::::one x\n\nb text\n\n- item\n- two\n\n::::two\n\n:::three\n\nc text\n\n:::\n\nd text\n\n::::\n\n:::::\n\ne text\n\n:::four\n:::\n\nf text\n".into()
}

const CMDS: &[&str] = &["edit.deleteBackward", "edit.deleteForward", "edit.newline"];

proptest! {
    /// Random carets, selections and keys in the rendered editor: a caret key never changes
    /// the pairs, a selection removes only pairs it holds whole, no marker is left behind, and
    /// undo gets back to the source.
    #[test]
    fn the_keyboard_does_not_break_a_block(
        ops in prop::collection::vec((any::<prop::sample::Index>(), 0usize..30, 0usize..CMDS.len()), 1..12)
    ) {
        let src = doc();
        let mut t = T::new(&src);
        prop_assert_eq!(markers(&t), (4, 0));
        for (at, len, cmd) in ops {
            let n = t.text().len();
            let a = at.index(n + 1);
            let z = (a + len).min(n);
            if !t.text().is_char_boundary(a) || !t.text().is_char_boundary(z) {
                continue;
            }
            let caret = len % 3 == 0;
            t.ed.session.set_selection(if caret { Selection::caret(a) } else { Selection::range(a, z) });
            let before = t.text().to_string();
            let (pairs, _) = markers(&t);
            t.run(CMDS[cmd]);
            let (now, stray) = markers(&t);
            prop_assert_eq!(stray, 0, "{} at {}..{} on {:?}: {:?}", CMDS[cmd], a, z, before, t.text());
            if caret {
                prop_assert_eq!(now, pairs, "{} at {} on {:?}", CMDS[cmd], a, before);
            }
            t.check_model();
        }
        while t.ed.session.undo() {}
        prop_assert_eq!(t.text(), src.as_str());
    }
}
