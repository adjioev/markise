//! Review planners (ADR 0012, issue #69): Add Comment, Accept / Reject one and all, Delete
//! Comment and the clean copy, as patches over the text.

use markise_doc::{EditBehavior, ParamValue, Params, PlanRequest, PlannerId, Selection, planners};
use markise_md::*;

fn run(src: &str, id: PlannerId, sel: Selection, params: Params) -> Result<(String, Option<Selection>), String> {
    let doc = Document::parse(src, ParseOptions::default());
    let req = PlanRequest {
        selection: sel,
        params,
        ..Default::default()
    };
    let plan = doc.plan(id, &req).map_err(|e| format!("{e:?}"))?;
    let mut t = src.to_string();
    for s in plan.steps {
        t = s.patches.apply_to(&t);
    }
    Ok((t, plan.selection_after))
}

/// Runs `id` with the caret at the `|` in `src` (removed first).
fn at_caret(src: &str, id: PlannerId) -> Result<(String, usize), String> {
    let at = src.find('|').expect("caret mark");
    let src = src.replacen('|', "", 1);
    let (t, sel) = run(&src, id, Selection::caret(at), Params::new())?;
    Ok((t, sel.map_or(usize::MAX, |s| s.primary().focus.offset)))
}

/// Runs `id` on the selection between the two `|` in `src`.
fn on_selection(src: &str, id: PlannerId) -> Result<(String, usize), String> {
    let a = src.find('|').unwrap();
    let b = src[a + 1..].find('|').unwrap() + a;
    let src = src.replace('|', "");
    let (t, sel) = run(&src, id, Selection::range(a, b), Params::new())?;
    Ok((t, sel.map_or(usize::MAX, |s| s.primary().focus.offset)))
}

fn with_caret(t: &str, at: usize) -> String {
    format!("{}|{}", &t[..at], &t[at..])
}

#[test]
fn add_comment_wraps_the_selection_and_puts_the_caret_in_the_comment() {
    let (t, c) = on_selection("The |quick fox| jumps.\n", planners::REVIEW_ADD_COMMENT).unwrap();
    assert_eq!(with_caret(&t, c), "The {==quick fox==}{>>|<<} jumps.\n");
    // surrounding spaces are not part of the highlight (C49)
    let (t, _) = on_selection("The| quick |fox.\n", planners::REVIEW_ADD_COMMENT).unwrap();
    assert_eq!(t, "The {==quick==}{>><<} fox.\n");
    // a collapsed caret gets an empty comment
    let (t, c) = at_caret("Done.| Next.\n", planners::REVIEW_ADD_COMMENT).unwrap();
    assert_eq!(with_caret(&t, c), "Done.{>>|<<} Next.\n");
}

#[test]
fn add_comment_widens_through_styled_spans() {
    let (t, _) = on_selection("a **bo|ld** te|xt\n", planners::REVIEW_ADD_COMMENT).unwrap();
    assert_eq!(t, "a {==**bold** te==}{>><<}xt\n");
    // inside one styled span: stays inside
    let (t, _) = on_selection("a **b|ol|d** c\n", planners::REVIEW_ADD_COMMENT).unwrap();
    assert_eq!(t, "a **b{==ol==}{>><<}d** c\n");
}

#[test]
fn add_comment_refusals() {
    // over two paragraphs
    assert!(on_selection("on|e\n\ntw|o\n", planners::REVIEW_ADD_COMMENT).is_err());
    // inside a code span, a code block, an existing mark
    assert!(on_selection("a `co|d|e` b\n", planners::REVIEW_ADD_COMMENT).is_err());
    assert!(at_caret("```\nco|de\n```\n", planners::REVIEW_ADD_COMMENT).is_err());
    assert!(at_caret("a {++in|s++} b\n", planners::REVIEW_ADD_COMMENT).is_err());
    assert!(on_selection("|a {--x--} b|\n", planners::REVIEW_ADD_COMMENT).is_err());
    // right after a mark is fine: the comment attaches to it
    let (t, _) = at_caret("a {++ins++}| b\n", planners::REVIEW_ADD_COMMENT).unwrap();
    assert_eq!(t, "a {++ins++}{>><<} b\n");
    // off: refused
    let doc = Document::parse(
        "a b\n",
        ParseOptions {
            critic_markup: false,
            ..Default::default()
        },
    );
    assert!(!doc.can_plan(planners::REVIEW_ADD_COMMENT, &PlanRequest::at(Selection::caret(1))));
}

#[test]
fn accept_and_reject_one_suggestion() {
    let cases = [
        // (source with caret, accept, reject)
        ("Lorem {++ip|sum ++}dolor\n", "Lorem ipsum dolor\n", "Lorem dolor\n"),
        ("Lorem {--ip|sum--} dolor\n", "Lorem dolor\n", "Lorem ipsum dolor\n"),
        (
            "Lorem {~~hip|sum~>ipsum~~} dolor\n",
            "Lorem ipsum dolor\n",
            "Lorem hipsum dolor\n",
        ),
        // the attached comment goes with the suggestion
        (
            "It {~~is~>was|~~}{>>@ai: tense<<} late.\n",
            "It was late.\n",
            "It is late.\n",
        ),
        // the caret on the comment of a suggestion counts
        ("It {++very ++}{>>|why?<<}good.\n", "It very good.\n", "It good.\n"),
    ];
    for (src, acc, rej) in cases {
        assert_eq!(at_caret(src, planners::REVIEW_ACCEPT).unwrap().0, acc, "accept {src}");
        assert_eq!(at_caret(src, planners::REVIEW_REJECT).unwrap().0, rej, "reject {src}");
    }
    // caret after the accepted text
    let (t, c) = at_caret("A {++new++}| word\n", planners::REVIEW_ACCEPT).unwrap();
    assert_eq!(with_caret(&t, c), "A new| word\n");
    // the `at` parameter instead of the caret
    let src = "x {--y--} z {++w++}\n";
    let at = src.find("{++").unwrap() + 3;
    let (t, _) = run(
        src,
        planners::REVIEW_ACCEPT,
        Selection::caret(0),
        Params::new().with("at", ParamValue::Int(at as i64)),
    )
    .unwrap();
    assert_eq!(t, "x {--y--} z w\n");
    // no suggestion at the caret
    assert!(at_caret("plain| text\n", planners::REVIEW_ACCEPT).is_err());
    assert!(at_caret("a {==hi|gh==} b\n", planners::REVIEW_ACCEPT).is_err());
}

#[test]
fn removing_a_mark_leaves_tidy_spaces() {
    let cases = [
        ("one {--two|--} three\n", "one three\n"),
        ("one {--two|--}.\n", "one.\n"),
        ("{--Two|--} three\n", "three\n"),
        ("one {--two|--}\n", "one\n"),
        ("one{-- two|--} three\n", "one three\n"),
    ];
    for (src, want) in cases {
        assert_eq!(at_caret(src, planners::REVIEW_ACCEPT).unwrap().0, want, "{src}");
    }
}

#[test]
fn accept_all_and_reject_all() {
    let src = "A {--b--} {--c--} d {++e++} {~~f~>g~~}{>>why<<} {==keep==}{>>me<<}.\n";
    let (t, _) = run(src, planners::REVIEW_ACCEPT_ALL, Selection::caret(0), Params::new()).unwrap();
    assert_eq!(t, "A d e g {==keep==}{>>me<<}.\n");
    let (t, _) = run(src, planners::REVIEW_REJECT_ALL, Selection::caret(0), Params::new()).unwrap();
    assert_eq!(t, "A b c d f {==keep==}{>>me<<}.\n");
    // nothing to do
    assert!(
        run(
            "a {>>c<<}\n",
            planners::REVIEW_ACCEPT_ALL,
            Selection::caret(0),
            Params::new()
        )
        .is_err()
    );
    // marks in quotes, lists and tables
    let src = "> q {++a++}\n\n- i {--b--}\n\n| {~~c~>d~~} | e |\n|---|---|\n";
    let (t, _) = run(src, planners::REVIEW_ACCEPT_ALL, Selection::caret(0), Params::new()).unwrap();
    assert_eq!(t, "> q a\n\n- i\n\n| d | e |\n|---|---|\n");
}

#[test]
fn delete_comment() {
    // a standalone comment
    let (t, _) = at_caret("Done. {>>no|te<<} Next.\n", planners::REVIEW_DELETE_COMMENT).unwrap();
    assert_eq!(t, "Done. Next.\n");
    // a highlight with its comment: the text stays
    for src in [
        "A {==pass|age==}{>>vague<<} here.\n",
        "A {==passage==}{>>va|gue<<} here.\n",
    ] {
        let (t, c) = at_caret(src, planners::REVIEW_DELETE_COMMENT).unwrap();
        assert_eq!(with_caret(&t, c), "A passage| here.\n", "{src}");
    }
    // the comment of a suggestion: only the comment
    let (t, _) = at_caret("A {++b++}{>>c|<<} d\n", planners::REVIEW_DELETE_COMMENT).unwrap();
    assert_eq!(t, "A {++b++} d\n");
    assert!(at_caret("A {++b|++} d\n", planners::REVIEW_DELETE_COMMENT).is_err());
}

#[test]
fn clean_copy_patches() {
    let src = "# T {>>title?<<}\n\nA {==b==}{>>c<<} {++d++}{>>e<<} {--f--} g.\n\n`{++code++}`\n";
    let p = |reject: bool| Params::new().with("reject", ParamValue::Bool(reject));
    let (t, _) = run(src, planners::REVIEW_CLEAN_COPY, Selection::caret(0), p(false)).unwrap();
    assert_eq!(t, "# T\n\nA b d g.\n\n`{++code++}`\n");
    let (t, _) = run(src, planners::REVIEW_CLEAN_COPY, Selection::caret(0), p(true)).unwrap();
    assert_eq!(t, "# T\n\nA b f g.\n\n`{++code++}`\n");
}

/// Review P2-4: the exact cases where resolving all at once used to differ from resolving one
/// by one.
#[test]
fn adjacent_removals_resolve_like_one_by_one() {
    let accept_all = |src: &str| {
        run(src, planners::REVIEW_ACCEPT_ALL, Selection::caret(0), Params::new())
            .unwrap()
            .0
    };
    assert_eq!(accept_all("A {--b--}{--c--} D\n"), "A D\n");
    assert_eq!(accept_all("A {--b--} {--c--}.\n"), "A.\n");
    assert_eq!(accept_all("{--a--} {--b--} c\n"), "c\n");
    assert_eq!(
        accept_all("x {++a ++}{--b--}.\n"),
        sequential("x {++a ++}{--b--}.\n", Some(true))
    );
}

/// Resolves every mark one at a time in document order with the one-mark planners
/// (`accept`: `Some(true)` accept / `Some(false)` reject suggestions only; `None`: clean copy,
/// accepting and deleting comments too).
fn sequential(src: &str, accept: Option<bool>) -> String {
    let mut t = src.to_string();
    loop {
        let doc = Document::parse(t.as_str(), ParseOptions::default());
        let marks = markise_doc::review::all_marks(&doc);
        let next = marks.iter().find(|m| accept.is_none() || m.is_suggestion());
        let Some(m) = next else { return t };
        let id = match (m.is_suggestion(), accept) {
            (true, Some(false)) => planners::REVIEW_REJECT,
            (true, _) => planners::REVIEW_ACCEPT,
            (false, _) => planners::REVIEW_DELETE_COMMENT,
        };
        let at = Params::new().with("at", ParamValue::Int(m.content.start as i64));
        t = run(&t, id, Selection::caret(0), at).unwrap().0;
    }
}

mod bulk_equals_sequential {
    use super::*;
    use proptest::prelude::*;

    fn piece() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("a".to_string()),
            Just("bc".to_string()),
            Just(" ".to_string()),
            Just("  ".to_string()),
            Just(".".to_string()),
            Just(",".to_string()),
            Just("\n".to_string()),
            Just("{--x--}".to_string()),
            Just("{-- y--}".to_string()),
            Just("{++z++}".to_string()),
            Just("{++w ++}".to_string()),
            Just("{~~o~>n~~}".to_string()),
            Just("{~~o~>~~}".to_string()),
            Just("{>>c<<}".to_string()),
            Just("{==h==}".to_string()),
            // review round 3: multi-byte text next to marks
            Just("é".to_string()),
            Just("日本".to_string()),
            Just("🙂".to_string()),
            Just("{++é++}".to_string()),
            Just("{--日--}".to_string()),
            Just("{~~🙂~>é~~}".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]
        #[test]
        fn accept_reject_and_clean_copy(parts in proptest::collection::vec(piece(), 0..14)) {
            let src = format!("{}\n", parts.concat());
            let all = |id, p: Params| {
                run(&src, id, Selection::caret(0), p).map(|r| r.0).unwrap_or_else(|_| src.clone())
            };
            prop_assert_eq!(all(planners::REVIEW_ACCEPT_ALL, Params::new()), sequential(&src, Some(true)));
            prop_assert_eq!(all(planners::REVIEW_REJECT_ALL, Params::new()), sequential(&src, Some(false)));
            let clean = Params::new().with("reject", ParamValue::Bool(false));
            prop_assert_eq!(all(planners::REVIEW_CLEAN_COPY, clean), sequential(&src, None));
        }
    }
}

/// Owner decision (ADR 0012): Enter inside a comment adds one line break inside it, keeping
/// the paragraph's container prefix; a second Enter adds nothing; the result round-trips and
/// is still one comment.
#[test]
fn enter_inside_a_comment_breaks_the_line_and_keeps_the_paragraph() {
    let cases = [
        ("A {>>one|two<<} b\n", "A {>>one\n|two<<} b\n"),
        ("> A {>>one|two<<} b\n", "> A {>>one\n> |two<<} b\n"),
        ("- A {>>one|two<<} b\n", "- A {>>one\n  |two<<} b\n"),
        ("1. A {>>one|two<<} b\n", "1. A {>>one\n   |two<<} b\n"),
        // review round 2: a task item continues at its content indent (not past the box)
        ("- [ ] A {>>one|two<<} b\n", "- [ ] A {>>one\n  |two<<} b\n"),
        ("> - [x] A {>>one|two<<}\n", "> - [x] A {>>one\n>   |two<<}\n"),
        // a continuation line repeats its own prefix
        ("- A\n  B {>>one|two<<}\n", "- A\n  B {>>one\n  |two<<}\n"),
    ];
    for (src, want) in cases {
        let (t, c) = at_caret(src, planners::NEWLINE).unwrap();
        assert_eq!(with_caret(&t, c), want, "{src}");
        // a second Enter: nothing
        let (t2, _) = run(&t, planners::NEWLINE, Selection::caret(c), Params::new()).unwrap();
        assert_eq!(t2, t, "{src}: no blank line");
        // still one comment over two lines, byte-identical round-trip
        let doc = Document::parse(t.as_str(), ParseOptions::default());
        assert_eq!(doc.serialize(), t);
        let marks = markise_doc::review::all_marks(&doc);
        assert_eq!(marks.len(), 1, "{t:?}");
        assert_eq!(marks[0].kind, markise_doc::CriticKind::Comment);
    }
    // single-line blocks and whitespace-only results: Enter does nothing
    for src in [
        "# Title {>>one|two<<}\n",
        "Title {>>one|two<<}\n===\n",
        "| a {>>one|two<<} | b |\n|---|---|\n",
        "A {>>one|  \ntwo<<} b\n",
        "A {>>one\n  |two<<} b\n",
        "A {>>one  |<<} b\n",
        // review round 3: block syntax after the caret would start a new block
        "A {>>one|# two<<} b\n",
        "A {>>one|> two<<} b\n",
        "A {>>one|- two<<} b\n",
        "A {>>one|1. two<<} b\n",
        "A {>>one|```two<<} b\n",
        // a line of only dashes would turn the first line into a setext heading
        "A {>>one|---\nmore<<} b\n",
        "> A {>>one|# two<<}\n",
        "- A {>>one|- two<<}\n",
    ] {
        let at = src.find('|').unwrap();
        let clean = src.replacen('|', "", 1);
        let (t, _) = run(&clean, planners::NEWLINE, Selection::caret(at), Params::new()).unwrap();
        assert_eq!(t, clean, "{src:?}: ignored");
    }
    // outside a comment Enter splits the paragraph as before
    let (t, _) = at_caret("A {>>x<<} b|c\n", planners::NEWLINE).unwrap();
    assert!(t.contains("\n\n"), "{t:?}");
}

/// Edit Comment (issue #87) with the comment at the `|` in `src` (removed first) and `new` as
/// its text.
fn edit_comment(src: &str, new: &str) -> Result<String, String> {
    let at = src.find('|').expect("caret mark");
    let src = src.replacen('|', "", 1);
    let params = Params::new()
        .with("at", ParamValue::Int(at as i64))
        .with("text", ParamValue::Text(new.into()));
    run(&src, planners::REVIEW_EDIT_COMMENT, Selection::caret(0), params).map(|(t, _)| t)
}

#[test]
fn edit_comment_replaces_the_text() {
    assert_eq!(
        edit_comment("a {==b==}{>>|old<<} c\n", "new text").unwrap(),
        "a {==b==}{>>new text<<} c\n"
    );
    // the author prefix is part of the text the card writes back
    assert_eq!(
        edit_comment("a{>>|@ai: old<<} c\n", "@ai: new").unwrap(),
        "a{>>@ai: new<<} c\n"
    );
    // an empty comment (as Add Comment makes it) gets its text
    assert_eq!(edit_comment("a{>>|<<} c\n", "hi").unwrap(), "a{>>hi<<} c\n");
    // unchanged: no patch
    assert_eq!(edit_comment("a{>>|same<<} c\n", "same").unwrap(), "a{>>same<<} c\n");
    // no escapes in CriticMarkup: a closer in the text is kept apart
    assert_eq!(
        edit_comment("a{>>|x<<} c\n", "use <<} here").unwrap(),
        "a{>>use << } here<<} c\n"
    );
    // not on a comment
    assert!(edit_comment("a {++|x++} c\n", "t").is_err());
}

#[test]
fn edit_comment_empty_deletes_the_comment_and_its_highlight() {
    assert_eq!(edit_comment("a {==b==}{>>|x<<} c\n", "  \n ").unwrap(), "a b c\n");
    assert_eq!(edit_comment("a {>>|x<<} c\n", "").unwrap(), "a c\n");
}

#[test]
fn edit_comment_lines_follow_the_paragraph() {
    // a paragraph: line breaks, blank lines dropped, lines trimmed
    assert_eq!(
        edit_comment("a{>>|x<<} b\n", "one\n\n  two  \nthree").unwrap(),
        "a{>>one\ntwo\nthree<<} b\n"
    );
    // CRLF documents keep their line ending
    assert_eq!(
        edit_comment("a{>>|x<<} b\r\n", "one\ntwo").unwrap(),
        "a{>>one\r\ntwo<<} b\r\n"
    );
    // quotes and list items: the continuation prefix
    assert_eq!(
        edit_comment("> a{>>|x<<} b\n", "one\ntwo").unwrap(),
        "> a{>>one\n> two<<} b\n"
    );
    assert_eq!(
        edit_comment("- a{>>|x<<} b\n- c\n", "one\ntwo").unwrap(),
        "- a{>>one\n  two<<} b\n- c\n"
    );
    assert_eq!(
        edit_comment("> 1. a{>>|x<<} b\n", "one\ntwo").unwrap(),
        "> 1. a{>>one\n>    two<<} b\n"
    );
    // a continuation line repeats its own prefix
    assert_eq!(
        edit_comment("- a\n  b{>>|x<<}\n", "one\ntwo").unwrap(),
        "- a\n  b{>>one\n  two<<}\n"
    );
    // single-line blocks: joined with spaces
    assert_eq!(edit_comment("# T{>>|x<<}\n", "one\ntwo").unwrap(), "# T{>>one two<<}\n");
    // a line that would start block syntax ends the paragraph: joined instead
    assert_eq!(
        edit_comment("a{>>|x<<} b\n", "one\n# two").unwrap(),
        "a{>>one # two<<} b\n"
    );
    assert_eq!(
        edit_comment("a{>>|x<<} b\n", "one\n- two").unwrap(),
        "a{>>one - two<<} b\n"
    );
}

#[test]
fn edit_comment_multi_line_comment_is_replaced_whole() {
    assert_eq!(
        edit_comment("> a{>>|one\n> two<<} b\n", "just one").unwrap(),
        "> a{>>just one<<} b\n"
    );
}

/// Edit Comment on the comment at byte `at` of `src` (tables: `|` cannot mark the caret).
fn edit_comment_at(src: &str, at: usize, new: &str) -> Result<String, String> {
    let params = Params::new()
        .with("at", ParamValue::Int(at as i64))
        .with("text", ParamValue::Text(new.into()));
    run(src, planners::REVIEW_EDIT_COMMENT, Selection::caret(0), params).map(|(t, _)| t)
}

/// Codex review of #158: a `|` in a table cell's comment would add a column (the table
/// becomes a paragraph while the comment survives): it is written `\|`, which the comment
/// shows as `|`; the table keeps its shape.
#[test]
fn edit_comment_in_a_table_cell_escapes_pipes() {
    let src = "| A{>>old<<} | B |\n| --- | --- |\n";
    let at = src.find("old").unwrap();
    let t = edit_comment_at(src, at, "x|y").unwrap();
    assert_eq!(t, "| A{>>x\\|y<<} | B |\n| --- | --- |\n");
    let doc = Document::parse(t.as_str(), ParseOptions::default());
    let marks = markise_doc::review::all_marks(&doc);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].text, "x|y");
    assert!(matches!(
        markise_doc::FormatDoc::leaf(&doc, 0).unwrap().kind,
        markise_doc::LeafKind::Table { .. }
    ));
    // lines are joined in a table row
    assert_eq!(
        edit_comment_at(src, at, "a\nb").unwrap(),
        "| A{>>a b<<} | B |\n| --- | --- |\n"
    );
}

const TABLE: &str = "| Head one | Head two |\n| --- | --- |\n| cell text | a\\|b |\n";

fn sel_in(src: &str, needle: &str) -> Selection {
    let a = src.find(needle).unwrap();
    Selection::range(a, a + needle.len())
}

/// Issue #90: Add Comment in a table cell — a selection inside one cell (header or body) is
/// highlighted, a caret gets an empty comment; escaped pipes stay; selections across cells or
/// rows, and the delimiter row, are refused with a reason.
#[test]
fn add_comment_in_table_cells() {
    let add = |sel: Selection| {
        run(TABLE, planners::REVIEW_ADD_COMMENT, sel, Params::new())
            .map(|(t, s)| (t, s.unwrap().primary().focus.offset))
    };
    let (t, c) = add(sel_in(TABLE, "cell")).unwrap();
    assert_eq!(
        t,
        "| Head one | Head two |\n| --- | --- |\n| {==cell==}{>><<} text | a\\|b |\n"
    );
    assert_eq!(&t[c - 3..c + 3], "{>><<}", "the caret in the comment");
    let (t, _) = add(sel_in(TABLE, "Head two")).unwrap();
    assert!(t.starts_with("| Head one | {==Head two==}{>><<} |\n"), "{t}");
    let (t, _) = add(sel_in(TABLE, "a\\|b")).unwrap();
    assert!(t.ends_with("| cell text | {==a\\|b==}{>><<} |\n"), "{t}");
    let at = TABLE.find("text").unwrap() + 4;
    let (t, _) = add(Selection::caret(at)).unwrap();
    assert!(t.ends_with("| cell text{>><<} | a\\|b |\n"), "{t}");
    // the result is still a table with the same cells and the comment in its cell
    let doc = Document::parse(t.as_str(), ParseOptions::default());
    let marks = markise_doc::review::all_marks(&doc);
    assert_eq!(marks.len(), 1);
    // refused: across cells, across rows, in the delimiter row
    let a = TABLE.find("text").unwrap();
    let b = TABLE.find("a\\|b").unwrap() + 1;
    let err = add(Selection::range(a, b)).unwrap_err();
    assert!(err.contains("one table cell"), "{err}");
    let a = TABLE.find("one").unwrap();
    assert!(add(Selection::range(a, TABLE.find("cell").unwrap() + 2)).is_err());
    let d = TABLE.find("---").unwrap() + 1;
    let err = add(Selection::caret(d)).unwrap_err();
    assert!(err.contains("delimiter"), "{err}");
}

/// Issue #90: accept / reject / delete inside cells keep the table, and unchanged documents
/// round-trip.
#[test]
fn resolve_marks_in_table_cells() {
    let src = "| A | B |\n| --- | --- |\n| {++new++} x | {--old--} y |\n| {~~p~>q~~}{>>why<<} | {==h==}{>>c<<} z |\n";
    let doc = Document::parse(src, ParseOptions::default());
    assert_eq!(markise_doc::FormatDoc::text(&doc), src, "round trip");
    assert_eq!(markise_doc::review::all_marks(&doc).len(), 6);
    let at = |n: &str| Selection::caret(src.find(n).unwrap() + 3);
    let (t, _) = run(src, planners::REVIEW_ACCEPT, at("{++new"), Params::new()).unwrap();
    assert!(t.contains("| new x | {--old--} y |"), "{t}");
    let (t, _) = run(src, planners::REVIEW_REJECT, at("{--old"), Params::new()).unwrap();
    assert!(t.contains("| {++new++} x | old y |"), "{t}");
    let (t, _) = run(src, planners::REVIEW_ACCEPT, at("{~~p"), Params::new()).unwrap();
    assert!(t.contains("| q | {==h==}{>>c<<} z |"), "{t}");
    let (t, _) = run(src, planners::REVIEW_DELETE_COMMENT, at("{==h"), Params::new()).unwrap();
    assert!(t.contains("| h z |"), "{t}");
    let (t, _) = run(src, planners::REVIEW_ACCEPT_ALL, Selection::caret(0), Params::new()).unwrap();
    assert_eq!(
        t,
        "| A | B |\n| --- | --- |\n| new x | y |\n| q{>>why<<} | {==h==}{>>c<<} z |\n".replace("q{>>why<<}", "q")
    );
    // still a table with two columns in every row
    let d = Document::parse(t.as_str(), ParseOptions::default());
    let tv = markise_doc::FormatDoc::table(&d, 0).expect("a table");
    for r in &tv.rows {
        assert_eq!(markise_doc::FormatDoc::table_row_cells(&d, *r).len(), 2, "{t}");
    }
}

/// Codex review of #161: rows are told apart by their place in the table (a body row of `-`
/// and `:` is text, an empty cell takes a comment), and a backslash right before the
/// selection — which would escape the opener and leave the marks as plain text — is refused,
/// in cells and in paragraphs.
#[test]
fn add_comment_table_rows_by_index_and_backslashes() {
    let add = |src: &str, sel: Selection| run(src, planners::REVIEW_ADD_COMMENT, sel, Params::new()).map(|(t, _)| t);
    let src = "| A | B |\n| --- | --- |\n| - | : |\n|  |  |\n";
    let colon = src.find(" : ").unwrap() + 1;
    let t = add(src, Selection::range(colon, colon + 1)).unwrap();
    assert!(t.contains("| - | {==:==}{>><<} |"), "{t}");
    let dash = src.find("| - ").unwrap() + 2;
    assert!(add(src, Selection::range(dash, dash + 1)).is_ok());
    let empty = src.find("|  |  |").unwrap() + 2;
    let t = add(src, Selection::caret(empty)).unwrap();
    assert!(t.ends_with("| {>><<} |  |\n"), "{t}");
    // the delimiter row (by its index) is still refused
    let d = src.find("---").unwrap() + 1;
    assert!(add(src, Selection::caret(d)).unwrap_err().contains("delimiter"));
    // a backslash right before: refused with a reason (cells and paragraphs)
    for src in [
        "| a\\word | x |\n| --- | --- |\n| b | c |\n",
        "| H | I |\n| --- | --- |\n| a\\word | x |\n",
        "a\\word here\n",
        "# a\\word\n",
    ] {
        let w = src.find("word").unwrap();
        let err = add(src, Selection::range(w, w + 4)).unwrap_err();
        assert!(err.contains("backslash"), "{src:?}: {err}");
        assert!(add(src, Selection::caret(w)).is_err(), "{src:?}: caret");
    }
    // an escaped backslash is just text
    let src = "a\\\\word here\n";
    let w = src.find("word").unwrap();
    assert_eq!(
        add(src, Selection::range(w, w + 4)).unwrap(),
        "a\\\\{==word==}{>><<} here\n"
    );
}
