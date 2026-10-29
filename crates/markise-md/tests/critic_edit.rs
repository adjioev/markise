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
