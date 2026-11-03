//! Format commands (spec 02 §3.3, C49) as Markdown planners: bytes and selection after.
//! Notation: `‸` caret, `⟦…⟧` selection (anchor at `⟦`).

use markise_doc::{EditBehavior, FormatDoc, PlanRequest, PlannerId, Selection};
use markise_md::{Document, ParseOptions};

fn parse_sel(src: &str) -> (String, Selection) {
    let mut out = String::new();
    let (mut a, mut b) = (None, None);
    for c in src.chars() {
        match c {
            '‸' => {
                a = Some(out.len());
                b = Some(out.len());
            }
            '⟦' => a = Some(out.len()),
            '⟧' => b = Some(out.len()),
            c => out.push(c),
        }
    }
    let (a, b) = (a.expect("selection"), b.expect("selection"));
    (out, Selection::range(a, b))
}

fn show(text: &str, sel: &Selection) -> String {
    let r = sel.primary();
    let (a, f) = (r.anchor.offset, r.focus.offset);
    let mut s = text.to_string();
    if a == f {
        s.insert(a, '‸');
    } else {
        let (x, y) = (a.min(f), a.max(f));
        s.insert(y, '⟧');
        s.insert(x, '⟦');
    }
    s
}

pub fn run(src: &str, id: &'static str) -> String {
    try_run(src, id).unwrap_or_else(|| panic!("{id} not applicable on {src:?}"))
}

pub fn try_run(src: &str, id: &'static str) -> Option<String> {
    let (text, sel) = parse_sel(src);
    let mut d = Document::parse(text, ParseOptions::default());
    let req = PlanRequest::at(sel.clone());
    let plan = d.plan(PlannerId(id), &req).ok()?;
    let mut sel = sel;
    for st in &plan.steps {
        let ps = d.canonicalize(&st.patches);
        FormatDoc::apply(&mut d, &ps);
        sel = sel.transform(&ps);
    }
    let sel = plan.selection_after.unwrap_or(sel);
    Some(show(d.text(), &sel))
}

fn checked(src: &str, id: &'static str) -> bool {
    let (text, sel) = parse_sel(src);
    let d = Document::parse(text, ParseOptions::default());
    d.plan_checked(PlannerId(id), &PlanRequest::at(sel))
}

#[test]
fn spec02_3_3_wrap_and_toggle_strong() {
    assert_eq!(run("a ⟦bold⟧ c", "format.strong"), "a **⟦bold⟧** c");
    assert_eq!(run("a **⟦bold⟧** c", "format.strong"), "a ⟦bold⟧ c");
    // selection including the markers toggles off too
    assert_eq!(run("a ⟦**bold**⟧ c", "format.strong"), "a ⟦bold⟧ c");
    assert!(checked("a **⟦bold⟧** c", "format.strong"));
    assert!(!checked("a ⟦bold⟧ c", "format.strong"));
    assert!(checked("a **bo‸ld** c", "format.strong"));
}

#[test]
fn spec02_3_3_collapsed_caret_inserts_pair() {
    assert_eq!(run("ab‸", "format.strong"), "ab**‸**");
    assert_eq!(run("ab**‸**", "format.strong"), "ab‸");
    assert_eq!(run("a‸b", "format.emph"), "a*‸*b");
    assert_eq!(run("a‸", "format.underline"), "a<u>‸</u>");
    assert_eq!(run("a‸", "format.code"), "a`‸`");
    assert_eq!(run("a‸", "format.link"), "a[‸]()");
}

#[test]
fn c49_trims_whitespace() {
    assert_eq!(run("foo⟦ bar ⟧baz", "format.strong"), "foo **⟦bar⟧** baz");
    // only whitespace: acts as a caret at the selection start
    assert_eq!(run("foo⟦ ⟧bar", "format.strong"), "foo**‸** bar");
}

#[test]
fn c49_multi_block() {
    assert_eq!(run("⟦a\n\nb⟧", "format.emph"), "*⟦a*\n\n*b⟧*");
    assert_eq!(run("⟦*a*\n\n*b*⟧", "format.emph"), "⟦a\n\nb⟧");
    assert_eq!(
        run("⟦x\n\n```\ncode\n```\n\ny⟧", "format.strong"),
        "**⟦x**\n\n```\ncode\n```\n\n**y⟧**"
    );
}

#[test]
fn partial_toggle_off_splits_span() {
    assert_eq!(run("**ab⟦cd⟧ef**", "format.strong"), "**ab**⟦cd⟧**ef**");
    assert_eq!(run("**ab ⟦cd⟧ ef**", "format.strong"), "**ab** ⟦cd⟧ **ef**");
    assert_eq!(run("**⟦ab⟧cd**", "format.strong"), "⟦ab⟧**cd**");
}

#[test]
fn wrapping_merges_inner_and_widens_cut_spans() {
    assert_eq!(run("⟦a **b** c⟧", "format.strong"), "**⟦a b c⟧**");
    // a selection cutting an emphasis span is widened to the whole span
    assert_eq!(run("x *a⟦b* c⟧", "format.strong"), "x **⟦*ab* c⟧**");
    assert_eq!(run("**a**⟦b⟧", "format.strong"), "**a⟦b⟧**");
}

#[test]
fn code_with_backticks_uses_longer_fence() {
    assert_eq!(run("⟦a`b⟧", "format.code"), "``⟦a`b⟧``");
    assert_eq!(run("⟦`a⟧", "format.code"), "`` ⟦`a⟧ ``");
}

#[test]
fn other_markers() {
    assert_eq!(run("⟦x⟧", "format.strike"), "~~⟦x⟧~~");
    assert_eq!(run("⟦x⟧", "format.underline"), "<u>⟦x⟧</u>");
    assert_eq!(run("<u>⟦x⟧</u>", "format.underline"), "⟦x⟧");
    assert_eq!(run("⟦x⟧", "format.inlineMath"), "$⟦x⟧$");
    assert_eq!(run("⟦x⟧", "format.comment"), "<!--⟦x⟧-->");
}

#[test]
fn link_and_image() {
    assert_eq!(run("see ⟦docs⟧ here", "format.link"), "see [docs](‸) here");
    assert_eq!(run("see [do⟦c⟧s](u) here", "format.link"), "see ⟦docs⟧ here");
    assert_eq!(run("⟦alt⟧", "format.image"), "![alt](‸)");
}

#[test]
fn clear_format() {
    assert_eq!(run("⟦a **b** *c* ~~d~~ `e`⟧", "format.clear"), "⟦a b c d e⟧");
    assert_eq!(run("**ab⟦cd⟧ef**", "format.clear"), "**ab**⟦cd⟧**ef**");
    assert!(try_run("a‸b", "format.clear").is_none());
}

#[test]
fn formats_inside_containers_with_crlf() {
    assert_eq!(run("> a ⟦b⟧\r\n", "format.strong"), "> a **⟦b⟧**\r\n");
    assert_eq!(run("- ⟦x\r\n  y⟧\r\n", "format.emph"), "- *⟦x\r\n  y⟧*\r\n");
}

#[test]
fn spec02_15_1_heading_level_commands() {
    assert_eq!(run("para‸", "block.headingUp"), "###### para‸");
    assert_eq!(run("## h‸", "block.headingUp"), "# h‸");
    assert!(try_run("# h‸", "block.headingUp").is_none());
    assert_eq!(run("## h‸", "block.headingDown"), "### h‸");
    assert_eq!(run("###### h‸", "block.headingDown"), "h‸");
    assert!(try_run("p‸", "block.headingDown").is_none());
    // setext H1/H2 keep their style; other levels convert to ATX
    assert_eq!(run("T‸\n===\n", "block.headingDown"), "T‸\n---\n");
    assert_eq!(run("T‸\n---\n", "block.headingUp"), "T‸\n===\n");
    assert_eq!(run("T‸\n---\n", "block.headingDown"), "### T‸\n");
    assert_eq!(run("T‸\n---\n", "block.paragraph"), "T‸\n");
}

#[test]
fn clear_format_keeps_literal_code_content() {
    // Codex review of PR #35: `<u>` inside a code span is text, not underline
    assert_eq!(run("a ⟦`<u>x</u>`⟧ b", "format.clear"), "a ⟦<u>x</u>⟧ b");
    assert_eq!(run("a ⟦<u>x</u>⟧ b", "format.clear"), "a ⟦x⟧ b");
    assert_eq!(run("a <u>⟦x⟧</u> b", "format.clear"), "a ⟦x⟧ b");
    assert_eq!(run("⟦$<u>$ **b**⟧", "format.clear"), "⟦$<u>$ b⟧");
}

const FORMAT_IDS: [&str; 11] = [
    "format.strong",
    "format.emph",
    "format.underline",
    "format.code",
    "format.strike",
    "format.highlight",
    "format.inlineMath",
    "format.comment",
    "format.link",
    "format.image",
    "format.clear",
];

/// Issue #133: above `EXACT_QUERY_LIMIT` the format commands decide `can_plan` without
/// planning; the answer agrees with the planner (Clear Format: enabled whenever it would act).
#[test]
fn large_selections_decide_format_enablement_without_planning() {
    let limit = markise_md::EXACT_QUERY_LIMIT;
    let prose = markise_corpus::generate(3 * limit, 7);
    let mut code = String::from("```\n");
    while code.len() < 2 * limit {
        code.push_str("let x = **not markup**;\n");
    }
    code.push_str("```\n");
    let blank = format!("Text.\n{}\nMore.\n", "\n".repeat(2 * limit));
    for text in [prose, code, blank] {
        let d = Document::parse(text.clone(), ParseOptions::default());
        let n = text.len();
        let mut ranges = vec![(0, n), (n / 3, n), (7, limit + 9)];
        if let Some(i) = text.get(1..).and_then(|t| t.find("\n\n")) {
            ranges.push((i + 1, n)); // starting in whitespace between blocks
        }
        for (a, b) in ranges {
            assert!(b - a > limit);
            for id in FORMAT_IDS {
                let req = PlanRequest::at(Selection::range(a, b));
                let planned = d.plan(PlannerId(id), &req).is_ok();
                let can = d.can_plan(PlannerId(id), &req);
                if id == "format.clear" {
                    assert!(can || !planned, "{id} {a}..{b}: enabled whenever it acts");
                } else {
                    let head: String = text.chars().take(20).collect();
                    assert_eq!(can, planned, "{id} {a}..{b} of {head:?}…");
                }
                // check marks are not computed for large selections
                assert!(!d.plan_checked(PlannerId(id), &req));
            }
        }
    }
}

/// Issue #133: the format toggles and Clear Format refuse selections over
/// `FORMAT_SELECTION_LIMIT` (bounded work on the UI thread); Hyperlink acts on the first block
/// only and stays available.
#[test]
fn format_toggles_refuse_selections_over_the_limit() {
    let para = "Some words, and a **bold** one.\n\n";
    let text = para.repeat(markise_md::FORMAT_SELECTION_LIMIT / para.len() + 2);
    let d = Document::parse(text.clone(), ParseOptions::default());
    let req = PlanRequest::at(Selection::range(0, text.len()));
    for id in FORMAT_IDS {
        let can = d.can_plan(PlannerId(id), &req);
        let plan = d.plan(PlannerId(id), &req);
        if matches!(id, "format.link" | "format.image") {
            assert!(can && plan.is_ok(), "{id}");
        } else {
            assert!(!can, "{id} disabled");
            assert!(
                matches!(plan, Err(markise_doc::PlanError::NotApplicable(m)) if m.contains("too large")),
                "{id}: {plan:?}"
            );
        }
    }
    // the link wraps the first block's text, as with a selection of that block alone
    let first = run(&format!("⟦{}⟧", para.trim_end()), "format.link");
    let plan = d.plan(PlannerId("format.link"), &req).unwrap();
    let mut d2 = d.clone();
    for st in &plan.steps {
        let ps = d2.canonicalize(&st.patches);
        FormatDoc::apply(&mut d2, &ps);
    }
    let caret = plan.selection_after.unwrap().primary().focus.offset;
    let mut shown = d2.text()[..para.len() + 3].to_string();
    shown.insert(caret, '‸');
    assert_eq!(shown.trim_end(), first);
}
