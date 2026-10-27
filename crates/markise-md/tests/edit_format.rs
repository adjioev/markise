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
