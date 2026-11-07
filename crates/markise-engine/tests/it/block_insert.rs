//! Inserting a block (ADR 0022), through the editing surface: where it goes, the bytes around it, its
//! ids, one undo step, how it shows and where the caret lands. The Markdown comes from a package, so
//! what is not exactly its block is refused.

use markise_doc::{DocPos, Selection};
use markise_engine::editor::InsertRefused;
use markise_layout::BlockViewMode;
use markise_layout::source::SourceParams;

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

use super::block_inplace::surface;

const TIP: &str = ":::tip\n\nHello\n\n:::\n";
const POLL: &str = ":::poll\n\n## Which? {#q}\n\n- [ ] One {#a1}\n- [ ] Two {#a2}\n\n:::\n";

/// `‸` marks the caret, `⟨n⟩` a virtual caret of slot n there.
fn at(marked: &str) -> (String, DocPos) {
    let virt = marked.find('⟨').map(|i| {
        let tail = &marked[i + '⟨'.len_utf8()..];
        (i, tail.split('⟩').next().unwrap().to_string())
    });
    match virt {
        Some((i, n)) => (
            marked.replace(&format!("⟨{n}⟩"), ""),
            DocPos::with_virt(i, n.parse().unwrap()),
        ),
        None => {
            let i = marked.find('‸').unwrap();
            (marked.replace('‸', ""), DocPos::new(i))
        }
    }
}

fn inserted(marked: &str, preset: &str, block: &str) -> String {
    let (text, pos) = at(marked);
    let mut s = surface(&text);
    s.ed.session.set_selection(Selection::caret_at(pos));
    s.ed.insert_block(block, preset, false).unwrap();
    s.text()
}

#[test]
fn it_goes_to_the_blank_line_or_after_what_holds_the_caret() {
    for (marked, want) in [
        // a blank line: there, with what is there used as separation
        ("a⟨1⟩\n\nb\n", "a\n\n:::tip\n\nHello\n\n:::\n\nb\n"),
        ("a\n\n‸\n\nb\n", "a\n\n:::tip\n\nHello\n\n:::\n\n\nb\n"),
        ("a\n‸\nb\n", "a\n\n:::tip\n\nHello\n\n:::\n\nb\n"),
        // an empty document
        ("‸", ":::tip\n\nHello\n\n:::\n"),
        // inside a paragraph: after it, never splitting it
        ("o‸ne\ntwo\n\nb\n", "one\ntwo\n\n:::tip\n\nHello\n\n:::\n\nb\n"),
        // inside a list or a quote: after all of it
        ("- o‸ne\n- two\n\nb\n", "- one\n- two\n\n:::tip\n\nHello\n\n:::\n\nb\n"),
        ("> o‸ne\n> two\n", "> one\n> two\n\n:::tip\n\nHello\n\n:::\n"),
        // code: after the fence
        (
            "```\nc‸ode\n```\nb\n",
            "```\ncode\n```\n\n:::tip\n\nHello\n\n:::\n\nb\n",
        ),
        // inside a block: after it, also from the virtual paragraph in an empty one
        (
            "a\n\n:::x\n\nb‸ody\n\n:::\n\nc\n",
            "a\n\n:::x\n\nbody\n\n:::\n\n:::tip\n\nHello\n\n:::\n\nc\n",
        ),
        (":::x⟨1⟩\n:::\n", ":::x\n:::\n\n:::tip\n\nHello\n\n:::\n"),
        // nested: after the outermost
        (
            "::::x\n\n:::y\n\nb‸ody\n\n:::\n\n::::\n",
            "::::x\n\n:::y\n\nbody\n\n:::\n\n::::\n\n:::tip\n\nHello\n\n:::\n",
        ),
        // after a block's closer: outside it
        (
            ":::x\n\nb\n\n:::⟨1⟩\n\nc\n",
            ":::x\n\nb\n\n:::\n\n:::tip\n\nHello\n\n:::\n\nc\n",
        ),
        // the end of a document, with and without a line ending: it stays as it was
        ("on‸e\n", "one\n\n:::tip\n\nHello\n\n:::\n"),
        ("on‸e", "one\n\n:::tip\n\nHello\n\n:::"),
        ("one\n\n‸", "one\n\n:::tip\n\nHello\n\n:::\n"),
        // front matter: after it
        (
            "---\nt‸: 1\n---\n\nb\n",
            "---\nt: 1\n---\n\n:::tip\n\nHello\n\n:::\n\nb\n",
        ),
    ] {
        assert_eq!(inserted(marked, TIP, "tip"), want, "{marked:?}");
    }
}

#[test]
fn line_endings_follow_the_document() {
    let want = "one\r\n\r\n:::tip\r\n\r\nHello\r\n\r\n:::\r\n\r\ntwo\r\n";
    assert_eq!(inserted("o‸ne\r\n\r\ntwo\r\n", TIP, "tip"), want);
    assert_eq!(
        inserted("o‸ne\n", ":::tip\r\n\r\nx\r\n\r\n:::\r\n", "tip"),
        "one\n\n:::tip\n\nx\n\n:::\n"
    );
}

#[test]
fn a_preset_keeps_its_ids_and_only_a_heading_s_is_the_page_s() {
    // nothing in the way: as the package wrote them
    assert_eq!(inserted("o‸ne\n", POLL, "poll"), format!("one\n\n{POLL}"));
    // the same block again: answers are per field and stay; a heading's id is the page's and gets a new number
    let doc = format!("{POLL}\nz‸\n");
    let out = inserted(&doc, POLL, "poll");
    let tail = out.rsplit("z\n\n").next().unwrap();
    assert_eq!(
        tail,
        ":::poll\n\n## Which? {#q-2}\n\n- [ ] One {#a1}\n- [ ] Two {#a2}\n\n:::\n"
    );
    // an id elsewhere on the page is no reason for an answer, and is for a heading
    let out = inserted("## H {#a1}\n\n## I {#a9}\n\nz‸\n", POLL, "poll");
    assert!(out.ends_with("- [ ] One {#a1}\n- [ ] Two {#a2}\n\n:::\n"), "{out}");
    let heading = ":::poll\n\n## Q {#a1}\n\n- [ ] A {#a1}\n\n:::\n";
    let out = inserted("x {#a1}\n\nz‸\n", heading, "poll");
    assert!(
        out.ends_with(":::poll\n\n## Q {#a1-2}\n\n- [ ] A {#a1}\n\n:::\n"),
        "{out}"
    );
    // a name can mean something: it stays, whatever is on the page
    let exclusive = ":::poll\n\n## Q\n\n- [ ] A {#a1}\n- [ ] None {#none}\n\n:::\n";
    let doc = format!("{exclusive}\nz‸\n");
    let out = inserted(&doc, exclusive, "poll");
    assert_eq!(out.matches("- [ ] None {#none}\n").count(), 2, "{out}");
    // a package repeating an id in one field: the next one in that field for the repeat only, never a name
    let twice = ":::poll\n\n- [ ] C {#a2}\n- [ ] A {#a1}\n- [ ] B {#a1}\n\n:::\n";
    let out = inserted("z‸\n", twice, "poll");
    assert!(out.contains("- [ ] C {#a2}\n- [ ] A {#a1}\n- [ ] B {#a3}\n"), "{out}");
    let repeated = ":::poll\n\n- [ ] A {#none}\n- [ ] B {#none}\n\n:::\n";
    assert!(inserted("z‸\n", repeated, "poll").contains("- [ ] A {#none}\n- [ ] B {#a1}\n"));
    // an id later in the field that is no repeat stays: the replacement goes past it
    let later = ":::poll\n\n- [ ] A {#none}\n- [ ] B {#none}\n- [ ] C {#a1}\n\n:::\n";
    let out = inserted("z‸\n", later, "poll");
    assert!(out.contains("- [ ] A {#none}\n- [ ] B {#a2}\n- [ ] C {#a1}\n"), "{out}");
    // and two replacements do not meet
    let two_repeats = ":::poll\n\n- [ ] A {#x}\n- [ ] B {#x}\n- [ ] C {#x}\n- [ ] D {#a1}\n\n:::\n";
    let out = inserted("z‸\n", two_repeats, "poll");
    assert!(
        out.contains("- [ ] A {#x}\n- [ ] B {#a2}\n- [ ] C {#a3}\n- [ ] D {#a1}\n"),
        "{out}"
    );
    // two fields may each have the same one
    let two = ":::poll\n\n- [ ] A {#a1}\n\ntext\n\n- [ ] B {#a1}\n\n:::\n";
    assert_eq!(inserted("z‸\n", two, "poll"), format!("z\n\n{two}"));
    // what only looks like an id in the preset is text
    let code = ":::tip\n\n```\nx {#a1}\n```\n\nand `y {#a1}`\n\n:::\n";
    assert_eq!(
        inserted("a {#a1}\n\nz‸\n", code, "tip"),
        format!("a {{#a1}}\n\nz\n\n{code}")
    );
}

#[test]
fn a_preset_that_is_not_exactly_its_block_is_refused_and_nothing_changes() {
    for (block, markdown) in [
        ("tip", "plain text\n"),
        ("tip", ":::tip\n\nnever closed\n"),
        ("tip", ":::tip\n\nx\n\n:::\n\nmore\n"),
        ("tip", ":::tip\n\nx\n\n:::\n\n:::tip\n\ny\n\n:::\n"),
        ("tip", "\n:::tip\n\nx\n\n:::\n"),
        ("tip", ":::poll\n\nx\n\n:::\n"),
        ("tip", ":::tip\n\n:::\n\n:::\n"),
        ("tip", ""),
    ] {
        let mut s = surface("one\n");
        s.caret(1);
        let rev = s.ed.session.history().undo_len();
        assert_eq!(
            s.ed.insert_block(block, markdown, false),
            Err(InsertRefused::NotItsBlock),
            "{markdown:?}"
        );
        assert_eq!(s.text(), "one\n");
        assert_eq!(s.ed.session.history().undo_len(), rev);
    }
}

#[test]
fn a_block_that_would_not_stand_there_is_refused() {
    // an unclosed fence holds everything to the end of the file
    let doc = "```\ncode\n";
    let mut s = surface(doc);
    s.caret(5);
    let history = s.ed.session.history().undo_len();
    assert_eq!(s.ed.insert_block("tip", TIP, false), Err(InsertRefused::WouldNotStand));
    assert_eq!(s.text(), doc);
    assert_eq!(s.ed.session.history().undo_len(), history);
    assert_eq!(s.offset(), 5);
}

#[test]
fn stray_markers_around_it_pair_as_they_did() {
    for doc in [
        ":::s\n\nz\n",
        "::::::s\n\nz\n\n:::\n",
        "z\n\n:::\n",
        ":::s\n\nz\n\n::::b\n\n::::\n\n:::\n",
    ] {
        let mut s = surface(doc);
        s.caret(doc.find('z').unwrap());
        assert!(s.ed.insert_block("tip", TIP, false).is_ok(), "{doc:?}");
        assert!(s.text().contains(":::tip"));
    }
}

#[test]
fn it_is_one_undo_step_back_to_the_exact_source_and_caret() {
    let doc = "one\n\n\n\ntwo\n";
    for caret in [DocPos::new(1), DocPos::with_virt(3, 1), DocPos::new(5)] {
        let mut s = surface(doc);
        s.ed.session.set_selection(Selection::caret_at(caret));
        let before = s.ed.session.history().undo_len();
        s.ed.insert_block("poll", POLL, false).unwrap();
        assert_eq!(s.ed.session.history().undo_len(), before + 1);
        assert_ne!(s.text(), doc);
        s.undo();
        assert_eq!(s.text(), doc);
        assert_eq!(s.ed.session.selection().primary().focus, caret);
        s.key("edit.redo");
        assert!(s.text().contains(":::poll"));
    }
}

#[test]
fn the_caret_lands_in_the_first_field_and_it_shows_natively() {
    // a heading's end, before its hidden id
    let mut s = surface("one\n");
    s.caret(0);
    s.ed.insert_block("poll", POLL, false).unwrap();
    s.frame();
    let text = s.text();
    assert_eq!(s.offset(), text.find("Which?").unwrap() + "Which?".len());
    assert!(s.ed.view.native_block(s.offset()).is_some());

    // no heading in the preset: the text's end
    let mut s = surface("one\n");
    s.caret(0);
    s.ed.insert_block("tip", TIP, false).unwrap();
    s.frame();
    assert_eq!(s.offset(), s.text().find("Hello").unwrap() + 5);
    s.typed("!");
    assert!(s.text().contains("Hello!"), "typing goes in the field: {}", s.text());
    assert!(s.ed.view.native_block(s.offset()).is_some());
}

#[test]
fn an_empty_marked_answer_stays_native_while_it_is_edited() {
    // no answer marked yet: only not filled in
    let preset = ":::poll\n\n## Q\n\n- [ ] A {#a1}\n- [ ] B {#a2}\n\n:::\n";
    let mut s = surface("one\n\nlast\n");
    s.caret(0);
    s.ed.insert_block("poll", preset, false).unwrap();
    s.frame();
    assert!(
        s.ed.view.native_block(s.offset()).is_some(),
        "native while the caret is in it"
    );
    let inside = s.offset();
    s.caret(s.text().find("last").unwrap());
    s.frame();
    assert!(
        s.ed.view.native_block(inside).is_some(),
        "and once it is left, as ADR 0021 says"
    );
    let i = s.ed.view.model().item_at_offset(s.text().find(":::poll").unwrap());
    assert!(s.ed.view.block_warnings(i).is_some(), "with its reason");
}

#[test]
fn a_preset_of_a_broken_structure_is_native_only_while_it_is_edited() {
    // an answer with no id is more than something yet to be filled in
    let preset = ":::poll\n\n## Q\n\n- [x] A\n- [ ] B {#a2}\n\n:::\n";
    let mut s = surface("one\n\nlast\n");
    s.caret(0);
    s.ed.insert_block("poll", preset, false).unwrap();
    s.frame();
    assert!(s.ed.view.native_block(s.offset()).is_some());
    let inside = s.offset();
    s.caret(s.text().find("last").unwrap());
    s.frame();
    assert!(s.ed.view.native_block(inside).is_none(), "a card once it is left");
}

#[test]
fn it_starts_native_also_when_the_document_shows_markdown() {
    let mut s = surface("one\n");
    s.ed.switch_all_block_views();
    assert_eq!(s.ed.view.block_mode_of(0), BlockViewMode::Markdown);
    s.caret(0);
    let opener = s.ed.insert_block("tip", TIP, false).unwrap();
    s.frame();
    assert_eq!(s.ed.view.block_mode_of(opener), BlockViewMode::Native);
    assert!(s.ed.view.native_block(s.offset()).is_some());
}

#[test]
fn a_block_with_no_view_gets_the_caret_on_its_first_content() {
    let mut s = surface("one\n");
    s.caret(0);
    s.ed.insert_block("unknown", ":::unknown\n\nAbc\n\n:::\n", false)
        .unwrap();
    assert_eq!(s.offset(), s.text().find("Abc").unwrap() + 3);
    let mut s = surface("one\n");
    s.caret(0);
    s.ed.insert_block("unknown", ":::unknown\n:::\n", false).unwrap();
    let opener_end = s.text().find(":::unknown").unwrap() + ":::unknown".len();
    assert_eq!(
        s.ed.session.selection().primary().focus,
        DocPos::with_virt(opener_end, 1)
    );
}

#[test]
fn in_source_mode_the_caret_is_the_panes_and_undo_works() {
    let doc = "one\n\ntwo\n";
    let mut s = surface(doc);
    s.ed.open_source(SourceParams::default(), 500.0, 1.0);
    s.ed.source_set_selection(Selection::caret(4)); // the blank line
    s.ed.session.begin_source_session();
    s.ed.insert_block("tip", TIP, true).unwrap();
    assert_eq!(s.text(), "one\n\n:::tip\n\nHello\n\n:::\n\ntwo\n");
    let pane = s.ed.source.as_ref().unwrap().sel.primary().focus.offset;
    assert_eq!(
        pane,
        s.text().find("Hello").unwrap() + 5,
        "the pane's caret in the field"
    );
    assert!(s.ed.edit(|e| e.undo()));
    assert_eq!(s.text(), doc);
}

#[test]
fn read_only_refuses() {
    let mut s = surface("one\n");
    s.ed.session.set_read_only(Some("a preview"));
    assert!(matches!(
        s.ed.insert_block("tip", TIP, false),
        Err(InsertRefused::Edit(_))
    ));
    assert_eq!(s.text(), "one\n");
}

#[test]
fn an_id_with_blanks_after_it_is_an_id() {
    let preset = ":::tip\n\n## New {#q}  \n\nBody\n\n:::\n";
    let out = inserted("## Existing {#q}\n\nz‸\n", preset, "tip");
    assert!(out.contains("## New {#q-2}  \n"), "{out}");
    // a free one keeps its bytes, blanks included
    let out = inserted("z‸\n", preset, "tip");
    assert!(out.contains("## New {#q}  \n"), "{out}");
}

#[test]
fn inserting_twice_leaves_the_second_being_edited() {
    // no answer marked: outside its schema, and native only while it is being edited
    let preset = ":::poll\n\n## Q\n\n- [ ] A {#a1}\n- [ ] B {#a2}\n\n:::\n";
    let mut s = surface("one\n");
    s.caret(0);
    s.ed.insert_block("poll", preset, false).unwrap();
    s.frame();
    assert!(s.ed.view.native_block(s.offset()).is_some(), "the first");
    s.ed.insert_block("poll", preset, false).unwrap();
    s.frame();
    assert!(
        s.ed.view.native_block(s.offset()).is_some(),
        "the second, as the first was"
    );
    let first = s.text().find("## Q").unwrap();
    assert!(
        s.ed.view.native_block(first).is_some(),
        "the first is left and only not filled in: ADR 0021"
    );
}

#[test]
fn only_what_the_parser_calls_blank_is_blank() {
    // a no-break space is text: a paragraph, and no separation
    assert_eq!(
        inserted("o‸ne\n\u{00a0}\n\ntwo\n", TIP, "tip"),
        "one\n\u{00a0}\n\n:::tip\n\nHello\n\n:::\n\ntwo\n"
    );
    assert_eq!(
        inserted("‸\n\u{00a0}\n", TIP, "tip"),
        "\n:::tip\n\nHello\n\n:::\n\n\u{00a0}\n".replacen('\n', "", 1)
    );
}

/// The trailing ids of the headings of a text, as a reader finds them: these are unique on a page. An answer's is
/// its field's.
fn ids(text: &str) -> Vec<&str> {
    (text.lines())
        .filter(|l| l.starts_with('#'))
        .filter_map(|l| l.trim_end().strip_suffix('}')?.rsplit_once("{#").map(|x| x.1))
        .collect()
}

const PIECES: [&str; 9] = [
    "a paragraph\nof two lines\n",
    "## A heading {#h1}\n",
    "- one\n- two\n",
    "> a quote\n",
    "```\ncode {#c}\n```\n",
    ":::tip\n\nBody\n\n:::\n",
    ":::poll\n\n## Q {#q}\n\n- [x] A {#a1}\n- [ ] B {#a2}\n\n:::\n",
    "::::plain\n\n:::tip\n\nInner\n\n:::\n\n::::\n",
    "| x |\n|---|\n| y |\n",
];

fn document(pieces: &[usize], gaps: &[usize]) -> String {
    let mut out = String::new();
    for (i, p) in pieces.iter().enumerate() {
        out.push_str(PIECES[*p]);
        out.push_str(&"\n".repeat(gaps[i % gaps.len()]));
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/block_insert.txt",
        ))),
        ..ProptestConfig::default()
    })]

    /// Insertions add bytes and take none, a heading's id is free, an answer's as written, and undo gives the exact
    /// source back.
    #[test]
    fn insertions_keep_the_rest_and_undo_restores_it(
        pieces in prop::collection::vec(0..PIECES.len(), 0..6),
        gaps in prop::collection::vec(1..3usize, 1..4),
        ops in prop::collection::vec((any::<prop::sample::Index>(), 0..3usize), 1..6),
    ) {
        let doc = document(&pieces, &gaps);
        let mut s = surface(&doc);
        let presets = [(TIP, "tip"), (POLL, "poll"), (":::plain\n\n## X {#a1}\n\n:::\n", "plain")];
        let start = s.ed.session.history().undo_len();
        for (caret, preset) in ops {
            let before = s.text();
            let at = caret.index(before.len().max(1));
            let at = (0..=at).rev().find(|i| before.is_char_boundary(*i)).unwrap_or(0);
            s.ed.session.set_selection(Selection::caret(at));
            let (markdown, block) = presets[preset];
            let Ok(opener) = s.ed.insert_block(block, markdown, false) else {
                prop_assert_eq!(s.text(), before);
                continue;
            };
            let after = s.text();
            // what was there is still there in order: the insertion is one run of new bytes
            let tail = before.len() - before.as_bytes().iter().rev().zip(after.as_bytes().iter().rev()).take_while(|(a, b)| a == b).count();
            let head = before.bytes().zip(after.bytes()).take_while(|(a, b)| a == b).count();
            prop_assert!(head + (before.len() - tail) >= before.len(), "{:?} to {:?}", before, after);
            prop_assert!(s.ed.session.doc().named_containers_at(opener).last().is_some_and(|c| c.opener.start == opener));
            // a heading's id never repeats what was there, nor another
            let mut trailing = ids(&after);
            let n = trailing.len();
            trailing.sort_unstable();
            trailing.dedup();
            let repeats_before = { let mut b = ids(&before); let m = b.len(); b.sort_unstable(); b.dedup(); m - b.len() };
            prop_assert_eq!(n - trailing.len(), repeats_before, "{}", after);
            // an answer's is as written
            if preset == 1 {
                prop_assert!(after.contains("- [ ] One {#a1}\n- [ ] Two {#a2}\n"), "{}", after);
            }
        }
        while s.ed.session.history().undo_len() > start {
            s.undo();
        }
        prop_assert_eq!(s.text(), doc);
    }
}
