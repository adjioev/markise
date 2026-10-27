//! M2c list editing on the headless editing surface (session + layout, CPU fonts): Enter
//! continuation, empty-item exit / outdent / split (S-5.1, S-5.2, C45), C11 renumbering,
//! Tab / Shift+Tab, Backspace at an item start (S-5.4, C46), Delete joins (C48), loose
//! lists (C52), lists in quotes, CRLF. Notation: `‸` caret, `(v)‸` virtual paragraph.
//! Every step also checks that the caret is drawable and the incremental model equals a
//! full rebuild.

mod common;
use common::T;

/// Runs `keys` from `src` and asserts the shown state after each key.
fn script(src: &str, steps: &[(&str, &str)]) {
    let mut t = T::at(src);
    for (k, want) in steps {
        if let Some(s) = k.strip_prefix("type:") {
            t.typ(s);
        } else {
            t.run(k);
        }
        assert_eq!(t.shown(), *want, "after {k} (from {src:?})");
        assert!(t.caret_visible(), "caret not drawable after {k}: {want:?}");
        assert!(
            t.selection_on_stops(),
            "caret off the caret stops after {k}: {want:?} → {}",
            t.shown()
        );
        t.check_model();
    }
}

#[test]
fn enter_continues_items_and_splits_mid_item() {
    script(
        "- a‸\n- b\n",
        &[("edit.newline", "- a\n- ‸\n- b\n"), ("type:x", "- a\n- x‸\n- b\n")],
    );
    script("- ab‸cd\n- e\n", &[("edit.newline", "- ab\n- ‸cd\n- e\n")]);
    script(
        "- [ ] a‸\n",
        &[
            ("edit.newline", "- [ ] a\n- [ ] ‸\n"),
            ("type:b", "- [ ] a\n- [ ] b‸\n"),
        ],
    );
    script(
        "> - a‸\n",
        &[("edit.newline", "> - a\n> - ‸\n"), ("type:x", "> - a\n> - x‸\n")],
    );
    script("- one\n  two‸\n", &[("edit.newline", "- one\n  two\n- ‸\n")]);
    script("* a‸\n", &[("edit.newline", "* a\n* ‸\n")]);
    script("1) a‸\n", &[("edit.newline", "1) a\n2) ‸\n")]);
}

#[test]
fn c11_renumbering_only_when_sequential() {
    script("1. a‸\n2. b\n3. c\n", &[("edit.newline", "1. a\n2. ‸\n3. b\n4. c\n")]);
    script("1. ab‸cd\n2. e\n", &[("edit.newline", "1. ab\n2. ‸cd\n3. e\n")]);
    // fixed numbers stay as written
    script("1. a‸\n1. b\n", &[("edit.newline", "1. a\n2. ‸\n1. b\n")]);
    // S-2.3: the start number is kept
    script("3. a‸\n", &[("edit.newline", "3. a\n4. ‸\n")]);
}

#[test]
fn s5_1_empty_item_leaves_the_list() {
    script(
        "- a‸\n",
        &[
            ("edit.newline", "- a\n- ‸\n"),
            ("edit.newline", "- a(v)‸\n"),
            ("type:y", "- a\n\ny‸\n"),
        ],
    );
    script("- ‸\n", &[("edit.newline", "‸\n")]);
}

#[test]
fn s5_2_nested_empty_item_outdents() {
    script(
        "- a\n  - b‸\n",
        &[
            ("edit.newline", "- a\n  - b\n  - ‸\n"),
            ("edit.newline", "- a\n  - b\n- ‸\n"),
            ("edit.newline", "- a\n  - b(v)‸\n"),
        ],
    );
    script(
        "1. a\n   1. b\n   2. ‸\n2. c\n",
        &[("edit.newline", "1. a\n   1. b\n2. ‸\n3. c\n")],
    );
}

#[test]
fn c45_empty_middle_item_splits_the_list() {
    script(
        "- a\n- ‸\n- c\n",
        &[
            ("edit.newline", "- a(v)‸\n\n\n- c\n"),
            ("type:x", "- a\n\nx‸\n\n- c\n"),
            ("edit.undo", "- a(v)‸\n\n\n- c\n"),
            ("edit.undo", "- a\n- ‸\n- c\n"),
        ],
    );
    script(
        "1. a\n2. ‸\n3. c\n4. d\n",
        &[
            ("edit.newline", "1. a(v)‸\n\n\n2. c\n3. d\n"),
            ("type:x", "1. a\n\nx‸\n\n2. c\n3. d\n"),
        ],
    );
    script("1. a\n1. ‸\n1. c\n", &[("edit.newline", "1. a(v)‸\n\n\n1. c\n")]);
    script(
        "p\n\n- ‸\n- b\n",
        &[("edit.newline", "p(v)‸\n\n- b\n"), ("type:x", "p\n\nx‸\n\n- b\n")],
    );
}

#[test]
fn c52_loose_lists_stay_loose() {
    script(
        "- a\n\n- b‸\n",
        &[
            ("edit.newline", "- a\n\n- b\n\n- ‸\n"),
            ("type:c", "- a\n\n- b\n\n- c‸\n"),
        ],
    );
}

#[test]
fn tab_and_shift_tab_nest_items() {
    script(
        "- a\n- b‸\n",
        &[
            ("edit.indentOrNextCell", "- a\n  - b‸\n"),
            ("edit.outdentOrPrevCell", "- a\n- b‸\n"),
        ],
    );
    script(
        "1. a\n2. b‸\n3. c\n",
        &[
            ("edit.indentOrNextCell", "1. a\n   1. b‸\n2. c\n"),
            ("edit.outdentOrPrevCell", "1. a\n2. b‸\n3. c\n"),
        ],
    );
    // nothing to nest under / already top level: no change, no tab
    script(
        "- a‸\n",
        &[
            ("edit.indentOrNextCell", "- a‸\n"),
            ("edit.outdentOrPrevCell", "- a‸\n"),
        ],
    );
    script("> - a\n> - b‸\n", &[("edit.indentOrNextCell", "> - a\n>   - b‸\n")]);
    // the keymap's Indent / Outdent commands do the same
    script(
        "- a\n- b‸\n",
        &[("block.indent", "- a\n  - b‸\n"), ("block.outdent", "- a\n- b‸\n")],
    );
}

#[test]
fn s5_4_backspace_joins_the_previous_item() {
    script("- a\n- ‸b\n", &[("edit.deleteBackward", "- a\n\n  ‸b\n")]);
    script(
        "1. a\n2. ‸b\n3. c\n",
        &[("edit.deleteBackward", "1. a\n\n   ‸b\n2. c\n")],
    );
    script("- a\n- ‸\n", &[("edit.deleteBackward", "- a‸\n")]);
    // a later paragraph of an item merges with the paragraph above (C52 rule 4)
    script("- a\n\n  ‸b\n", &[("edit.deleteBackward", "- a‸b\n")]);
}

#[test]
fn c46_backspace_unwraps_the_first_item() {
    script("- ‸a\n- b\n", &[("edit.deleteBackward", "‸a\n\n- b\n")]);
    script("- ‸a\n", &[("edit.deleteBackward", "‸a\n")]);
    script("1. ‸a\n2. b\n3. c\n", &[("edit.deleteBackward", "‸a\n\n1. b\n2. c\n")]);
    script("3. ‸a\n3. b\n", &[("edit.deleteBackward", "‸a\n\n3. b\n")]);
    script(
        "- x\n  - ‸a\n  - b\n",
        &[("edit.deleteBackward", "- x\n\n  ‸a\n\n  - b\n")],
    );
    script("- [ ] ‸a\n", &[("edit.deleteBackward", "‸a\n")]);
    script("text\n- ‸a\n", &[("edit.deleteBackward", "text\n\n‸a\n")]);
}

#[test]
fn c48_delete_joins_the_next_item() {
    script("- a‸\n- b\n", &[("edit.deleteForward", "- a‸b\n")]);
    script("- a‸\n- b\n  - c\n", &[("edit.deleteForward", "- a‸b\n  - c\n")]);
    script("1. a‸\n2. b\n3. c\n", &[("edit.deleteForward", "1. a‸b\n2. c\n")]);
}

#[test]
fn task_toggle_and_crlf() {
    script("- [ ] a‸\n", &[("block.toggleTask", "- [x] a‸\n")]);
    script(
        "- one\r\n  two‸\r\n",
        &[
            ("edit.newline", "- one\r\n  two\r\n- ‸\r\n"),
            ("type:z", "- one\r\n  two\r\n- z‸\r\n"),
            ("edit.newline", "- one\r\n  two\r\n- z\r\n- ‸\r\n"),
            ("edit.newline", "- one\r\n  two\r\n- z(v)‸\r\n"),
            ("type:p", "- one\r\n  two\r\n- z\r\n\r\np‸\r\n"),
        ],
    );
}

#[test]
fn empty_quote_paragraph_leaves_the_quote() {
    script(
        "> q‸\n",
        &[
            ("edit.newline", "> q(v)‸\n"),
            ("edit.newline", "> q(v)‸\n"),
            ("type:x", "> q\n\nx‸\n"),
        ],
    );
}

#[test]
fn caret_reaches_empty_items_and_empty_documents() {
    let mut t = T::at("- a‸\n- b\n");
    t.run("edit.newline");
    t.run("caret.down");
    t.run("caret.up");
    assert_eq!(t.shown(), "- a\n- ‸\n- b\n");
    let mut t = T::at("‸");
    assert!(t.caret_visible());
}

#[test]
fn list_at_document_end_without_newline() {
    script(
        "- a\n- b‸",
        &[
            ("edit.newline", "- a\n- b\n- ‸"),
            ("edit.indentOrNextCell", "- a\n- b\n  - ‸"),
            ("type:c", "- a\n- b\n  - c‸"),
            ("edit.newline", "- a\n- b\n  - c\n  - ‸"),
            ("edit.newline", "- a\n- b\n  - c\n- ‸"),
        ],
    );
}

#[test]
fn empty_items_without_spacing_and_nested_empty_items() {
    // an item written as a bare marker gets the space it needs when typed into
    script("- a\n-‸\n", &[("type:x", "- a\n- x‸\n")]);
    script("1.‸\n", &[("type:x", "1. x‸\n")]);
    script("- a\n- [ ]‸\n", &[("type:x", "- a\n- [ ] x‸\n")]);
    // Tab nests an empty item under a paragraph; Backspace removes it again
    script(
        "- a‸\n",
        &[
            ("edit.newline", "- a\n- ‸\n"),
            ("edit.indentOrNextCell", "- a\n  - ‸\n"),
            ("edit.deleteBackward", "- a‸\n"),
        ],
    );
    // an empty nested item left empty disappears when the caret leaves (ForceBlock expiry)
    let mut t = T::at("- a‸\n\nend\n");
    t.run("edit.newline");
    t.run("edit.indentOrNextCell");
    assert_eq!(t.text(), "- a\n  - \n\nend\n");
    t.set(t.text().len() - 2);
    t.ed.edit(|s| s.expire_hints_left_behind());
    assert_eq!(t.text(), "- a\n\nend\n");
}

#[test]
fn marker_width_changes_keep_children_inside() {
    // Codex review of PR #35: `9.` → `10.` must move the child with the content column
    script(
        "8. a‸\n9. b\n   - child\n",
        &[("edit.newline", "8. a\n9. ‸\n10. b\n    - child\n")],
    );
    let mut t = T::at("8. a‸\n9. b\n   - child\n");
    t.run("edit.newline");
    assert!(
        t.ed.session.doc().debug_tree().contains("para \"b\"\n    ul"),
        "{}",
        t.ed.session.doc().debug_tree()
    );
    // Move Row across the boundary and the list-kind toggle
    script(
        "9. a\n   - x\n10. b‸\n    - y\n",
        &[("edit.moveBlockUp", "9. b‸\n   - y\n10. a\n    - x\n")],
    );
    script("- a‸\n  - x\n", &[("block.orderedList", "1. a‸\n   - x\n")]);
}

#[test]
fn leaving_an_outer_list_from_a_nested_item_gives_a_top_level_paragraph() {
    script(
        "- a\n  - b\n- ‸\n",
        &[("edit.newline", "- a\n  - b(v)‸\n"), ("type:x", "- a\n  - b\n\nx‸\n")],
    );
}
