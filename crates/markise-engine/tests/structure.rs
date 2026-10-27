//! M2c structure and selection commands on the headless editing surface: Move Row (A4),
//! quote Backspace (C46), quote/list toggles, Horizontal Line, selection and delete
//! commands (spec 02 §10, C8). Notation: `‸` caret, `⟦…⟧` selection.

mod common;
use common::T;

fn script(src: &str, steps: &[(&str, &str)]) {
    let mut t = T::at(src);
    for (k, want) in steps {
        if let Some(s) = k.strip_prefix("type:") {
            t.typ(s);
        } else {
            t.run(k);
        }
        assert_eq!(t.shown(), *want, "after {k} (from {src:?})");
        assert!(
            t.selection_on_stops(),
            "selection off the stops after {k}: {}",
            t.shown()
        );
        t.check_model();
    }
}

#[test]
fn a4_move_row_moves_items_and_blocks() {
    script(
        "- a\n- b‸\n- c\n",
        &[
            ("edit.moveBlockUp", "- b‸\n- a\n- c\n"),
            ("edit.moveBlockDown", "- a\n- b‸\n- c\n"),
            ("edit.moveBlockDown", "- a\n- c\n- b‸\n"),
            ("edit.undo", "- a\n- b‸\n- c\n"),
        ],
    );
    // ordered: the numbers stay with the positions
    script("1. a\n2. b‸\n", &[("edit.moveBlockUp", "1. b‸\n2. a\n")]);
    // the last item cannot move down: the command is disabled
    let t = T::at("- a\n- b‸\n");
    assert!(t.ed.registry.enabled(&t.ed.session, "edit.moveBlockDown").is_err());
    script("one\n\ntw‸o\n", &[("edit.moveBlockUp", "tw‸o\n\none\n")]);
}

#[test]
fn c46_backspace_at_the_start_of_a_quote() {
    script("> ‸a\n> b\n", &[("edit.deleteBackward", "‸a\nb\n")]);
    script("> ‸a\n>\n> b\n", &[("edit.deleteBackward", "‸a\n\n> b\n")]);
    script(
        "> [!NOTE]\n> ‸text\n",
        &[("edit.deleteBackward", "> ‸text\n"), ("edit.deleteBackward", "‸text\n")],
    );
}

#[test]
fn quote_and_list_toggles() {
    script("a‸\n", &[("block.quote", "> a‸\n"), ("block.quote", "a‸\n")]);
    script(
        "a‸\n",
        &[
            ("block.bulletList", "- a‸\n"),
            ("block.orderedList", "1. a‸\n"),
            ("block.taskList", "- [ ] a‸\n"),
            ("block.bulletList", "- a‸\n"),
            ("block.bulletList", "a‸\n"),
        ],
    );
    script("- a\n- b‸\n", &[("block.orderedList", "1. a\n2. b‸\n")]);
    script("x‸\n", &[("block.insertRule", "x\n\n---(v)‸\n")]);
}

#[test]
fn spec02_10_selection_commands() {
    script(
        "one tw‸o three",
        &[("select.word", "one ⟦two⟧ three"), ("select.word", "one ⟦two⟧ three")],
    );
    script(
        "a **b‸old** c",
        &[
            ("select.scope", "a **⟦bold⟧** c"),
            ("select.scope", "a ⟦**bold**⟧ c"),
            ("select.scope", "⟦a **bold** c⟧"),
        ],
    );
    script(
        "ab‸c\nde\n\nfg",
        &[("select.line", "⟦abc⟧\nde\n\nfg"), ("select.line", "⟦abc\nde⟧\n\nfg")],
    );
    script("x\n\nab‸c\nd", &[("select.block", "x\n\n⟦abc\nd⟧")]);
}

#[test]
fn spec02_10_delete_commands() {
    script("one tw‸o three", &[("delete.word", "one ‸ three")]);
    script("a **b‸old** c", &[("delete.scope", "a **‸** c")]);
    script("ab‸c\nde", &[("delete.line", "‸de")]);
    script("abc\nd‸e", &[("delete.line", "abc‸")]);
    script("x\n\nab‸c\n\ny\n", &[("delete.block", "x\n\n‸y\n")]);
    script("x\n\nab‸c\n", &[("delete.block", "x‸\n")]);
}

#[test]
fn c45_leaving_the_middle_of_a_quote_splits_it() {
    script(
        "> a‸\n>\n> c\n",
        &[
            ("edit.newline", "> a(v)‸\n>\n> c\n"),
            ("edit.newline", "> a(v)‸\n\n> c\n"),
            ("type:x", "> a\n\nx‸\n\n> c\n"),
        ],
    );
}

#[test]
fn c48_delete_merges_across_containers_and_removes_rules() {
    script("a‸\n\n> b\n> c\n", &[("edit.deleteForward", "a‸b\nc\n")]);
    script("a‸\n\n> b\n>\n> c\n", &[("edit.deleteForward", "a‸b\n\n> c\n")]);
    script("a‸\n\n---\n\nb\n", &[("edit.deleteForward", "a‸\n\nb\n")]);
    script("a‸\n\n- b\n  more\n", &[("edit.deleteForward", "a‸b\nmore\n")]);
}

/// Codex review round 2: Move Row maps a caret on a continuation line through the changes
/// before it only (Unicode text, marker width changes both ways).
#[test]
fn move_row_caret_on_continuation_lines_with_unicode() {
    script(
        "9. a\n   é‸éé\n   z\n10. b\n",
        &[
            ("edit.moveBlockDown", "9. b\n10. a\n    é‸éé\n    z\n"),
            ("edit.moveBlockUp", "9. a\n   é‸éé\n   z\n10. b\n"),
            ("edit.undo", "9. b\n10. a\n    é‸éé\n    z\n"),
        ],
    );
    script(
        "9. b\n10. ‸aé\n    éé\n",
        &[("edit.moveBlockUp", "9. ‸aé\n   éé\n10. b\n")],
    );
    script("- x\n- ü‸ö\n  äö\n", &[("edit.moveBlockUp", "- ü‸ö\n  äö\n- x\n")]);
}

/// Codex review round 2: C48 merges drop the next block's heading syntax and keep this
/// block's (setext underline, ATX closing), also inside quotes and list items; undo restores.
#[test]
fn c48_delete_with_heading_syntax_on_both_sides() {
    let cases: &[(&str, &str)] = &[
        ("a‸\n\n> ## B\n", "a‸B\n"),
        ("a‸\n\n> ## B ##\n", "a‸B\n"),
        ("a‸\n\n> B\n> ---\n", "a‸B\n"),
        ("a‸\n\n- ## B\n", "a‸B\n"),
        ("A‸\n===\n\n---\n\nb\n", "A‸\n===\n\nb\n"),
        ("A‸\n===\n\n> b\n", "A‸b\n===\n"),
        ("# A‸ ##\n\n> b\n", "# A‸b ##\n"),
    ];
    for (src, want) in cases {
        let mut t = T::at(src);
        t.run("edit.deleteForward");
        assert_eq!(t.shown(), *want, "Delete in {src:?}");
        t.check_model();
        t.run("edit.undo");
        assert_eq!(t.shown(), *src, "undo of Delete in {src:?}");
        t.run("edit.redo");
        assert_eq!(t.shown(), *want, "redo of Delete in {src:?}");
    }
}

/// Codex review round 3: C48 between sibling list items shares the heading-aware merge;
/// Backspace mirrors (rule 4 first turns a heading into a paragraph; S-5.4 keeps the
/// previous item's setext heading).
#[test]
fn c48_between_list_items_with_headings_and_backspace_mirrors() {
    let cases: &[(&str, &str, &str)] = &[
        ("- a‸\n- ## B\n", "edit.deleteForward", "- a‸B\n"),
        ("- A‸\n  ===\n- b\n", "edit.deleteForward", "- A‸b\n  ===\n"),
        ("- A‸\n  ===\n- ## B ##\n", "edit.deleteForward", "- A‸B\n  ===\n"),
        ("- a‸\n- b\n  c\n", "edit.deleteForward", "- a‸b\n  c\n"),
        ("- a\n- ## ‸B\n", "edit.deleteBackward", "- a\n- ‸B\n"),
        ("- A\n  ===\n- ‸b\n", "edit.deleteBackward", "- A\n  ===\n\n  ‸b\n"),
    ];
    for (src, key, want) in cases {
        let mut t = T::at(src);
        t.run(key);
        assert_eq!(t.shown(), *want, "{key} in {src:?}");
        assert!(t.selection_on_stops(), "{key} in {src:?}: caret off the stops");
        t.check_model();
        t.run("edit.undo");
        assert_eq!(t.shown(), *src, "undo of {key} in {src:?}");
        t.run("edit.redo");
        assert_eq!(t.shown(), *want, "redo of {key} in {src:?}");
    }
}

/// Codex review round 4: Backspace joining a later paragraph into a setext heading in the
/// same item keeps the underline (the shared heading-aware merge).
#[test]
fn backspace_twice_into_a_setext_heading_in_an_item() {
    script(
        "- A\n  ===\n- ‸b\n",
        &[
            ("edit.deleteBackward", "- A\n  ===\n\n  ‸b\n"),
            ("edit.deleteBackward", "- A‸b\n  ===\n"),
            ("edit.undo", "- A\n  ===\n\n  ‸b\n"),
            ("edit.undo", "- A\n  ===\n- ‸b\n"),
            ("edit.redo", "- A\n  ===\n\n  ‸b\n"),
            ("edit.redo", "- A‸b\n  ===\n"),
        ],
    );
    script("- # A #\n\n  ‸b\n", &[("edit.deleteBackward", "- # A‸b #\n")]);
}
