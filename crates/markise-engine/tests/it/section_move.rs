//! Moving and deleting a section as one unit (ADR 0026): what it goes past, the bytes around it, one undo
//! step, and that nothing moves if anything would pair or divide differently.

use markise_doc::{DocPos, Selection};
use markise_engine::editor::MoveRefused;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

use super::block_inplace::surface;

/// The document with the caret at `‸`, moved, and undone to the same bytes.
fn moved(marked: &str, up: bool) -> String {
    let at = marked.find('‸').unwrap();
    let text = marked.replace('‸', "");
    let mut s = surface(&text);
    s.caret(at);
    let k = s.ed.section_at_caret();
    assert!(s.ed.can_move_section(k, up), "{marked:?}");
    assert_eq!(s.ed.move_section(k, up, false), Ok(true), "{marked:?}");
    let after = s.text();
    s.undo();
    assert_eq!(s.text(), text, "one undo step: {marked:?}");
    after
}

fn deleted(marked: &str) -> String {
    let at = marked.find('‸').unwrap();
    let text = marked.replace('‸', "");
    let mut s = surface(&text);
    s.caret(at);
    let k = s.ed.section_at_caret();
    assert_eq!(s.ed.delete_section(k, false), Ok(true), "{marked:?}");
    let after = s.text();
    s.undo();
    assert_eq!(s.text(), text, "one undo step: {marked:?}");
    after
}

#[test]
fn it_goes_past_one_neighbour_with_its_bytes() {
    for (marked, up, want) in [
        ("A\n\n---\n\nB‸\n\n---\n\nC\n", true, "B\n\n---\n\nA\n\n---\n\nC\n"),
        ("A\n\n---\n\nB‸\n\n---\n\nC\n", false, "A\n\n---\n\nC\n\n---\n\nB\n"),
        ("A\n\n---\n\nB\n\n---\n\nC‸\n", true, "A\n\n---\n\nC\n\n---\n\nB\n"),
        // lines of its own, blanks at a line's end, what is in it
        (
            "A\n\n---\n\nB  \n\n  x‸\n\n:::y\n\n---\n\n:::\n\n---\n\nC\n",
            true,
            "B  \n\n  x\n\n:::y\n\n---\n\n:::\n\n---\n\nA\n\n---\n\nC\n",
        ),
        // the last section's end stays the file's
        ("A\n\n---\n\nB‸", true, "B\n\n---\n\nA"),
        ("A\n\n---\n\nB‸\n\n\n", true, "B\n\n---\n\nA\n\n\n"),
    ] {
        assert_eq!(moved(marked, up), want, "{marked:?}");
    }
}

#[test]
fn the_first_section_has_no_boundary_whoever_is_first() {
    // the boundary stays between them
    assert_eq!(moved("First‸\n\n---\n\nB\n", false), "B\n\n---\n\nFirst\n");
    assert_eq!(moved("First\n\n---\n\nB‸\n", true), "B\n\n---\n\nFirst\n");
    // front matter stays first
    assert_eq!(
        moved("---\nlang: en\n---\n\nFirst\n\n---\n\nB‸\n", true),
        "---\nlang: en\n---\n\nB\n\n---\n\nFirst\n"
    );
    // with an empty first section, the next one has a boundary and goes nowhere above it
    let doc = "---\nlang: en\n---\n\n---\n\nB\n\n---\n\nC\n";
    let mut s = surface(doc);
    s.caret(doc.find('B').unwrap());
    let k = s.ed.section_at_caret();
    assert_eq!(k, 1);
    assert!(!s.ed.can_move_section(k, true));
    assert_eq!(s.ed.move_section(k, true, false), Ok(false));
    assert!(s.ed.can_move_section(k, false));
    assert_eq!(s.text(), doc);
    s.caret(doc.find('C').unwrap());
    assert_eq!(s.ed.move_section(2, true, false), Ok(true));
    assert_eq!(s.text(), "---\nlang: en\n---\n\n---\n\nC\n\n---\n\nB\n");
}

#[test]
fn there_is_nothing_to_go_past_at_the_ends() {
    let doc = "A\n\n---\n\nB\n";
    let mut s = surface(doc);
    assert!(!s.ed.can_move_section(0, true));
    assert!(!s.ed.can_move_section(1, false));
    assert_eq!(s.ed.move_section(0, true, false), Ok(false));
    assert_eq!(s.ed.move_section(1, false, false), Ok(false));
    assert_eq!(s.text(), doc);
    let one = surface("Only one.\n");
    assert!(!one.ed.can_move_section(0, true) && !one.ed.can_move_section(0, false));
}

#[test]
fn metadata_stays_first_in_its_section() {
    let meta = ":::section-metadata\n\nid: b\n\n:::";
    let doc = format!("A\n\n---\n\n{meta}\n\nB‸\n");
    assert_eq!(moved(&doc, true), format!("{meta}\n\nB\n\n---\n\nA\n"));
    let at = doc.replace('‸', "");
    let mut s = surface(&at);
    s.caret(0);
    assert_eq!(s.ed.move_section(0, false, false), Ok(true));
    let doc = s.ed.session.doc();
    let first = doc.section_metadata(0).expect("first in its section");
    assert_eq!(first.get("id"), Some("b"));
}

#[test]
fn blank_lines_come_out_normal_where_a_boundary_had_none() {
    // no blank line under a heading: a paragraph coming there would make the rule a heading's underline
    assert_eq!(
        moved("# H\n---\nB‸\n\n---\n\nC\n", true),
        "B\n\n---\n\n# H\n\n---\n\nC\n"
    );
    assert_eq!(
        moved("A\n\n---\n\n# H\n---\nC‸\n", true),
        "A\n\n---\n\nC\n\n---\n\n# H\n"
    );
    // too many of them around what goes
    assert_eq!(
        moved("A\n\n\n\n---\n\n\nB‸\n\n\n---\n\nC\n", true),
        "B\n\n---\n\nA\n\n\n\n---\n\nC\n"
    );
}

#[test]
fn the_caret_stays_in_what_it_was_in() {
    let doc = "Aaa\n\n---\n\nBbb\n\n---\n\nC\n";
    let mut s = surface(doc);
    s.caret(doc.find("bb").unwrap());
    assert_eq!(s.ed.move_section(1, true, false), Ok(true));
    assert_eq!(s.text().find("bb").unwrap(), s.offset());
    // a section going down by the other one coming up: still in it
    let mut s = surface(doc);
    s.caret(doc.find("aa").unwrap());
    assert_eq!(s.ed.move_section(0, false, false), Ok(true));
    assert_eq!(s.text(), "Bbb\n\n---\n\nAaa\n\n---\n\nC\n");
    assert_eq!(s.text().find("aa").unwrap(), s.offset());
    // on the boundary: the start of the section it opens
    let mut s = surface(doc);
    s.ed.set_selection(Selection::caret_at(DocPos::new(doc.find("---").unwrap())));
    assert_eq!(s.ed.move_section(1, true, false), Ok(true));
    assert_eq!(s.offset(), 0);
}

#[test]
fn what_would_change_how_things_pair_or_divide_is_refused() {
    // a stray closer in one and a stray opener in another would come to pair
    let doc = "A\n\n:::\n\n---\n\n:::x\n\n---\n\nC\n";
    let mut s = surface(doc);
    s.caret(doc.find('A').unwrap());
    assert_eq!(s.ed.move_section(0, false, false), Err(MoveRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
    // an unclosed fence takes what comes to stand after it
    let doc = "A\n\n---\n\n```\ncode\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.move_section(1, true, false), Err(MoveRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
}

#[test]
fn a_section_can_be_deleted_as_one_undo_step() {
    for (marked, want) in [
        ("A‸\n\n---\n\nB\n\n---\n\nC\n", "B\n\n---\n\nC\n"),
        ("A\n\n---\n\nB‸\n\n---\n\nC\n", "A\n\n---\n\nC\n"),
        ("A\n\n---\n\nB\n\n---\n\nC‸\n", "A\n\n---\n\nB\n"),
        ("A\n\n---\n\nB‸", "A"),
        // front matter stays
        ("---\nlang: en\n---\n\nA‸\n\n---\n\nB\n", "---\nlang: en\n---\n\nB\n"),
        // metadata goes with its section
        (
            "A\n\n---\n\n:::section-metadata\n\nid: b\n\n:::\n\nB‸\n\n---\n\nC\n",
            "A\n\n---\n\nC\n",
        ),
    ] {
        assert_eq!(deleted(marked), want, "{marked:?}");
    }
}

#[test]
fn the_only_section_is_not_deleted() {
    let mut s = surface("Only one.\n");
    assert!(!s.ed.can_delete_section(0));
    assert_eq!(s.ed.delete_section(0, false), Ok(false));
    assert_eq!(s.text(), "Only one.\n");
}

#[test]
fn deleting_refuses_what_would_pair_otherwise() {
    let doc = ":::x\n\n---\n\nB\n\n---\n\n:::\n\nC\n";
    let mut s = surface(doc);
    // a rule in a pair is no boundary: one section, nothing to delete
    assert_eq!(s.ed.session.doc().section_count(), 1);
    assert_eq!(s.ed.delete_section(0, false), Ok(false));
    let doc = "A\n\n---\n\n:::x\n\n---\n\nB\n\n:::\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.session.doc().section_count(), 2);
    assert_eq!(s.ed.delete_section(1, false), Ok(true));
    assert_eq!(s.text(), "A\n");
}

/// A section's body: ends in something other than a blank, and is no boundary.
fn body() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("text".to_string()),
        Just("two words  ".to_string()),
        Just("# head {#h}".to_string()),
        Just("- list\n- more".to_string()),
        Just(":::x\n\nin it\n\n:::".to_string()),
        Just(":::section-metadata\n\nid: m\n\n:::".to_string()),
        Just("```\n---\n```".to_string()),
        Just("***".to_string()),
        Just("> quote".to_string()),
    ];
    prop::collection::vec(piece, 1..4).prop_map(|p| p.join("\n\n"))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/section_move.txt",
        ))),
        ..ProptestConfig::default()
    })]

    /// With any line ending, the two swap, every section's bytes stay exactly and undo restores the source.
    #[test]
    fn sections_keep_their_bytes_and_undo_restores_them(
        bodies in prop::collection::vec(body(), 2..6),
        pick in 0usize..16,
        up in any::<bool>(),
        eol in prop_oneof![Just("\n"), Just("\r\n"), Just("\r")],
        front in any::<bool>(),
        tail in prop_oneof![Just(""), Just("\n"), Just("\n\n")],
        extra_blanks in 0usize..3,
    ) {
        let bodies: Vec<String> = bodies.iter().map(|b| b.replace('\n', eol)).collect();
        let sep = format!("{eol}{eol}{}---{eol}{eol}", eol.repeat(extra_blanks));
        let head = if front { format!("---{eol}lang: en{eol}---{eol}{eol}") } else { String::new() };
        let text = format!("{head}{}{}", bodies.join(&sep), tail.replace('\n', eol));
        let mut s = surface(&text);
        prop_assert_eq!(s.ed.session.doc().section_count(), bodies.len());
        let k = pick % bodies.len();
        let has = if up { k > 0 } else { k + 1 < bodies.len() };
        prop_assert_eq!(s.ed.can_move_section(k, up), has);
        match s.ed.move_section(k, up, false) {
            Ok(true) => {
                let other = if up { k - 1 } else { k + 1 };
                let mut want = bodies.clone();
                want.swap(k, other);
                let after = s.text();
                prop_assert_eq!(s.ed.session.doc().section_count(), bodies.len());
                let mut from = head.len();
                for b in &want {
                    let at = after[from..].find(b.as_str());
                    prop_assert!(at.is_some(), "{:?} in {:?}", b, after);
                    from += at.unwrap() + b.len();
                }
                prop_assert!(after.starts_with(&head));
                prop_assert!(after.ends_with(&tail.replace('\n', eol)) || tail.is_empty());
                s.undo();
                prop_assert_eq!(s.text(), text);
            }
            Ok(false) => prop_assert!(!has),
            // what would pair otherwise: nothing changed
            Err(_) => prop_assert_eq!(s.text(), text),
        }
    }
}

const META: &str = ":::section-metadata\n\nid: m\n\n:::";

#[test]
fn a_section_with_nothing_or_only_metadata_does_not_move_and_nothing_moves_past_it() {
    for doc in [
        format!("A\n\n---\n\n{META}\n\n---\n\nC\n"),
        "A\n\n---\n\n---\n\nC\n".to_string(),
    ] {
        let mut s = surface(&doc);
        for (k, up) in [(1, true), (1, false), (0, false), (2, true)] {
            // with a first section next to one with nothing in it the commands are off already
            let done = s.ed.move_section(k, up, false);
            assert!(
                matches!(done, Err(MoveRefused::WouldLeaveEmpty) | Ok(false)),
                "{doc:?} {k}: {done:?}"
            );
            assert_eq!(s.text(), doc);
        }
    }
    // elsewhere in the document it is no reason
    let doc = format!("A\n\n---\n\nB\n\n---\n\nC\n\n---\n\n{META}\n");
    let mut s = surface(&doc);
    assert_eq!(s.ed.move_section(1, true, false), Ok(true));
}

#[test]
fn a_delete_creates_no_section_with_only_metadata_and_can_take_one_away() {
    let doc = format!("A\n\n---\n\n{META}\n\n---\n\nC\n");
    let mut s = surface(&doc);
    assert_eq!(s.ed.delete_section(1, false), Ok(true));
    assert_eq!(s.text(), "A\n\n---\n\nC\n");
    // metadata goes with what it is in: nothing is left of it behind
    let doc = format!("A\n\n---\n\n{META}\n\nB\n\n---\n\nC\n");
    let mut s = surface(&doc);
    assert_eq!(s.ed.delete_section(1, false), Ok(true));
    assert_eq!(s.text(), "A\n\n---\n\nC\n");
    let doc = format!("{META}\n\nA\n\n---\n\nB\n");
    let mut s = surface(&doc);
    assert_eq!(s.ed.delete_section(0, false), Ok(true));
    assert_eq!(s.text(), "B\n");
}

/// The document with section `k` duplicated, and undone to the same bytes.
fn duplicated(text: &str, k: usize) -> String {
    let mut s = surface(text);
    assert!(s.ed.can_duplicate_section(k), "{text:?}");
    assert_eq!(s.ed.duplicate_section(k, false), Ok(true), "{text:?}");
    let after = s.text();
    assert_eq!(s.ed.session.doc().section_count(), section_count(text) + 1, "{after:?}");
    s.undo();
    assert_eq!(s.text(), text, "one undo step: {text:?}");
    after
}

fn section_count(text: &str) -> usize {
    surface(text).ed.session.doc().section_count()
}

#[test]
fn a_copy_comes_after_it_with_its_bytes_and_new_ids() {
    for (doc, k, want) in [
        // ids get a new number, with the words they had
        (
            "A\n\n---\n\n## B {#b}\n\nText  \n\n---\n\nC\n",
            1,
            "A\n\n---\n\n## B {#b}\n\nText  \n\n---\n\n## B {#b-2}\n\nText  \n\n---\n\nC\n",
        ),
        // a first section has no boundary and keeps none, the copy has one; the last one's end stays the file's
        (
            "# T\n\nA\n\n---\n\nB",
            0,
            "# T\n\nA\n\n---\n\n# T {#t-2}\n\nA\n\n---\n\nB",
        ),
        ("A\n\n---\n\nB", 1, "A\n\n---\n\nB\n\n---\n\nB"),
        ("A\n\n---\n\nB\n\n\n", 1, "A\n\n---\n\nB\n\n---\n\nB\n\n\n"),
        // a heading with no id gets one that is not what its original goes by; only the first heading
        (
            "## Two words\n\n## More\n\n---\n\nB\n",
            0,
            "## Two words\n\n## More\n\n---\n\n## Two words {#two-words-2}\n\n## More\n\n---\n\nB\n",
        ),
        // no id for what cannot take one: a setext heading, a closing `#`
        ("B\n===\n\n---\n\nC\n", 0, "B\n===\n\n---\n\nB\n===\n\n---\n\nC\n"),
        ("## B ##\n\n---\n\nC\n", 0, "## B ##\n\n---\n\n## B ##\n\n---\n\nC\n"),
        // taken anywhere on the page, also in code, and every id in it gets its own
        (
            "x {#b-2}\n\n```\n{#b-3}\n```\n\n---\n\n## B {#b}\n\n:::y\n\n- [ ] One {#a1}\n\n:::\n",
            1,
            "x {#b-2}\n\n```\n{#b-3}\n```\n\n---\n\n## B {#b}\n\n:::y\n\n- [ ] One {#a1}\n\n:::\n\n---\n\n## B {#b-4}\n\n:::y\n\n- [ ] One {#a2}\n\n:::\n",
        ),
        // a block's answers are numbered as its fields number them, past every number on the page
        (
            ":::y\n\n- [ ] One {#a7}\n- [ ] Two {#none}\n\n:::\n",
            0,
            ":::y\n\n- [ ] One {#a7}\n- [ ] Two {#none}\n\n:::\n\n---\n\n:::y\n\n- [ ] One {#a8}\n- [ ] Two {#a9}\n\n:::\n",
        ),
        // in code it is no id: the bytes stay
        (
            "A\n\n---\n\n## B {#b}\n\n```\ntext {#b}\n```\n",
            1,
            "A\n\n---\n\n## B {#b}\n\n```\ntext {#b}\n```\n\n---\n\n## B {#b-2}\n\n```\ntext {#b}\n```\n",
        ),
        // metadata's id: only its value
        (
            "A\n\n---\n\n:::section-metadata\n\nkind: x\nid:   top  \nid: second\n\n:::\n\n## B {#top}\n",
            1,
            "A\n\n---\n\n:::section-metadata\n\nkind: x\nid:   top  \nid: second\n\n:::\n\n## B {#top}\n\n---\n\n:::section-metadata\n\nkind: x\nid:   top-2  \nid: second\n\n:::\n\n## B {#top-3}\n",
        ),
        // what is in it pairs in the copy too, its rule included
        (
            "A\n\n---\n\n:::y\n\n---\n\n:::\n",
            1,
            "A\n\n---\n\n:::y\n\n---\n\n:::\n\n---\n\n:::y\n\n---\n\n:::\n",
        ),
    ] {
        assert_eq!(duplicated(doc, k), want, "{doc:?}");
    }
}

#[test]
fn a_copy_keeps_the_line_endings_and_the_caret_is_in_it() {
    let doc = "A\r\n\r\n---\r\n\r\n## B {#b}\r\n";
    assert_eq!(
        duplicated(doc, 1),
        "A\r\n\r\n---\r\n\r\n## B {#b}\r\n\r\n---\r\n\r\n## B {#b-2}\r\n"
    );
    let meta = "A\n\n---\n\n:::section-metadata\n\nid: m\n\n:::\n\nB\n";
    let mut s = surface(meta);
    s.caret(0);
    assert_eq!(s.ed.duplicate_section(1, false), Ok(true));
    let p = s.ed.session.selection().primary();
    assert!(p.is_caret());
    assert_eq!(s.ed.section_at_caret(), 2);
    let after = s.text();
    assert_eq!(&after[p.focus.offset..], "B\n", "past the plaque, in {after:?}");
}

#[test]
fn nothing_to_copy_or_what_would_pair_differently_is_no_copy() {
    for (doc, k) in [
        (format!("A\n\n---\n\n{META}\n\n---\n\nC\n"), 1),
        ("A\n\n---\n\n---\n\nC\n".to_string(), 1),
        ("---\n\nA\n\n---\n\nC\n".to_string(), 5),
    ] {
        let mut s = surface(&doc);
        assert!(!s.ed.can_duplicate_section(k));
        assert_eq!(s.ed.duplicate_section(k, false), Ok(false));
        assert_eq!(s.text(), doc);
    }
    // a stray closer in the copy pairs with the stray opener before it and takes the boundary in
    let doc = ":::\n\nA\n\n:::y\n\n---\n\nC\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.duplicate_section(0, false), Err(MoveRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
    // an unclosed fence would hold what is put after it
    let doc = "A\n\n---\n\nB\n\n```\ncode\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.duplicate_section(1, false), Err(MoveRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
}

#[test]
fn the_only_title_is_a_level_one_heading_outside_a_block() {
    for (doc, want) in [
        ("# T\n\nA\n\n---\n\nB\n", Some(0)),
        ("A\n\n---\n\nT\n===\n", Some(1)),
        ("# T\n\n---\n\n# U\n", None),
        ("## T\n\n---\n\nB\n", None),
        (":::y\n\n# T\n\n:::\n\n---\n\nB\n", None),
        (":::y\n\n# T\n\n:::\n\n---\n\n# U\n", Some(1)),
    ] {
        assert_eq!(surface(doc).ed.only_title_section(), want, "{doc:?}");
    }
}

#[test]
fn a_new_section_goes_at_the_end_of_the_one_named_and_the_caret_stays_if_refused() {
    use markise_engine::editor::NewSection;
    let new = NewSection {
        title: "N".into(),
        id: "n".into(),
        metadata: vec![],
    };
    let doc = "A\n\n---\n\nB\n\n- list\n\n---\n\nC\n";
    let mut s = surface(doc);
    s.caret(0);
    assert!(s.ed.insert_section_after(1, &new, false).is_ok());
    assert_eq!(s.text(), "A\n\n---\n\nB\n\n- list\n\n---\n\n## N {#n}\n\n---\n\nC\n");
    s.undo();
    assert_eq!(s.text(), doc);
    // the last one, with no line ending
    let mut s = surface("A\n\n---\n\nB");
    assert!(s.ed.insert_section_after(1, &new, false).is_ok());
    assert_eq!(s.text(), "A\n\n---\n\nB\n\n---\n\n## N {#n}");
    // into one with nothing in it: no boundary, it is filled in
    let mut s = surface("A\n\n---\n\n---\n\nC\n");
    assert!(s.ed.insert_section_after(1, &new, false).is_ok());
    assert_eq!(s.text(), "A\n\n---\n\n## N {#n}\n\n---\n\nC\n");
    // a first section with nothing in it has no place
    let first_empty = "---\nlang: en\n---\n\n---\n\nB\n";
    let mut s = surface(first_empty);
    if s.ed.session.doc().section_count() == 2 && !s.ed.can_insert_after_section(0) {
        assert!(s.ed.insert_section_after(0, &new, false).is_err());
        assert_eq!(s.text(), first_empty);
    }
    // refused: the caret stays where it was
    let hollow = format!("A\n\n---\n\n{META}\n\n---\n\nC\n");
    let mut s = surface(&hollow);
    s.caret(0);
    let with = NewSection {
        metadata: vec![("kind".into(), "x".into())],
        ..new.clone()
    };
    assert!(s.ed.insert_section_after(1, &with, false).is_err());
    assert_eq!(s.text(), hollow);
    assert_eq!(s.ed.session.selection().primary().focus.offset, 0);
}
