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
