//! Test cases T1–T43 of `docs/spec/01-markdown-dialect.md` §10.

use std::path::{Path, PathBuf};

use markise_md::inline::{InlineContext, InlineKind, parse_inlines, plain_text};
use markise_md::slug::SlugTracker;
use markise_md::*;

fn opts() -> ParseOptions {
    ParseOptions::default()
}

fn tree_with(text: &str, o: ParseOptions) -> String {
    debug_tree(&Document::parse(text, o))
}

fn tree(text: &str) -> String {
    tree_with(text, opts())
}

fn roundtrip(text: &str) {
    let d = parse(text);
    assert_eq!(d.serialize(), text, "round-trip changed bytes");
    d.validate().unwrap();
}

/// Inline node kinds of the first paragraph-like block (flattened, depth-first).
fn inline_kinds(text: &str, o: ParseOptions) -> Vec<String> {
    let d = Document::parse(text, o.clone());
    let b = &d.blocks()[0];
    let content = b.content(d.text());
    let ctx = InlineContext::default();
    let nodes = parse_inlines(&content, &o, &ctx);
    let mut out = Vec::new();
    fn go(s: &str, n: &[markise_md::inline::Inline], out: &mut Vec<String>) {
        for x in n {
            let k = match &x.kind {
                InlineKind::Text => format!("text:{}", x.span.slice(s)),
                InlineKind::Math { content, .. } => format!("math:{}", content.slice(s)),
                InlineKind::Escape => format!("escape:{}", x.span.slice(s)),
                k => format!("{k:?}").split([' ', '{', '(']).next().unwrap().to_lowercase(),
            };
            out.push(k);
            go(s, &x.children, out);
        }
    }
    go(&content, &nodes, &mut out);
    out
}

fn top_kinds(d: &Document) -> Vec<String> {
    debug_tree(d)
        .lines()
        .filter(|l| !l.starts_with(' '))
        .map(|l| l.split(' ').next().unwrap().to_string())
        .collect()
}

#[test]
fn t1_bold_em_roundtrip() {
    roundtrip("__bold__ and _em_\n");
    assert_eq!(
        inline_kinds("__bold__ and _em_\n", opts()),
        ["strong", "text:bold", "text: and ", "emph", "text:em"]
    );
}

#[test]
fn t2_heading_without_space_non_strict() {
    assert_eq!(tree("#Title\n"), "h1 \"Title\"\n");
}

#[test]
fn t3_heading_without_space_strict() {
    let o = ParseOptions { strict: true, ..opts() };
    assert_eq!(tree_with("#Title\n", o), "para \"#Title\"\n");
}

#[test]
fn t4_closed_atx() {
    assert_eq!(tree("## A ##\n"), "h2 \"A\"\n");
    roundtrip("## A ##\n");
}

fn new_doc_with(md: &str, w: &WriteOptions) -> String {
    let mut d = Document::new_empty(opts(), w);
    ops::insert_block_after(&mut d, None, md, w).unwrap();
    d.serialize()
}

#[test]
fn t5_new_h1_variable_setext() {
    let w = WriteOptions {
        heading_style: HeadingStyle::VariableSetext,
        prefer_final_newline: true,
        ..Default::default()
    };
    assert_eq!(new_doc_with(&write::heading("Hi", 1, &w, false), &w), "Hi\n==\n");
}

#[test]
fn t6_new_h2_variable_setext_wide() {
    let w = WriteOptions {
        heading_style: HeadingStyle::VariableSetext,
        prefer_final_newline: true,
        ..Default::default()
    };
    let out = new_doc_with(&write::heading("日本", 2, &w, false), &w);
    assert_eq!(out, "日本\n----\n");
    assert_eq!(tree(&out), "h2s \"日本\"\n");
}

#[test]
fn t7_add_item_keeps_bullet() {
    let mut d = parse("* a\n* b\n");
    ops::insert_list_item(&mut d, &[0], 2, "c", None).unwrap();
    assert_eq!(d.serialize(), "* a\n* b\n* c\n");
}

#[test]
fn t8_add_item_fixed_numbering() {
    let mut d = parse("3. a\n3. b\n");
    ops::insert_list_item(&mut d, &[0], 2, "c", None).unwrap();
    assert_eq!(d.serialize(), "3. a\n3. b\n3. c\n");
}

#[test]
fn t9_insert_item_renumbers() {
    let mut d = parse("1. a\n2. b\n");
    ops::insert_list_item(&mut d, &[0], 0, "z", None).unwrap();
    assert_eq!(d.serialize(), "1. z\n2. a\n3. b\n");
}

#[test]
fn t10_uncheck() {
    let mut d = parse("- [X] done\n");
    ops::toggle_task(&mut d, 3).unwrap();
    assert_eq!(d.serialize(), "- [ ] done\n");
}

#[test]
fn t11_check() {
    let mut d = parse("- [ ] todo\n");
    ops::toggle_task(&mut d, 3).unwrap();
    assert_eq!(d.serialize(), "- [x] todo\n");
}

#[test]
fn t12_new_fence_uses_tildes() {
    let md = write::fenced_code("", "a\n```\nb");
    assert_eq!(md, "~~~\na\n```\nb\n~~~");
    assert_eq!(tree(&md), "fence[] \"a\\n```\\nb\"\n");
}

#[test]
fn t13_fence_lengthened() {
    let mut d = parse("```\nold\n```\n");
    ops::set_code_content(&mut d, &[0], "x\n````\ny").unwrap();
    assert_eq!(d.serialize(), "`````\nx\n````\ny\n`````\n");
    assert_eq!(tree(d.text()), "fence[] \"x\\n````\\ny\"\n");
}

#[test]
fn t14_unclosed_fence_roundtrip() {
    let t = "text\n\n```js\nlet a = 1;\n";
    roundtrip(t);
    let d = parse(t);
    assert!(matches!(d.blocks()[1].kind, BlockKind::FencedCode { closer: None, .. }));
}

#[test]
fn t15_dollar_amounts_not_math() {
    assert_eq!(inline_kinds("costs $5 and $6\n", opts()), ["text:costs $5 and $6"]);
}

#[test]
fn t16_math_and_spaced_dollars() {
    assert_eq!(
        inline_kinds("$x^2$ and $ y $\n", opts()),
        ["math:x^2", "text: and $ y $"]
    );
}

#[test]
fn t17_latex_inline_math() {
    assert_eq!(inline_kinds("\\(a+b\\)\n", opts()), ["math:a+b"]);
}

#[test]
fn t18_alert() {
    assert_eq!(
        tree("> [!warning]\n> careful\n"),
        "alert-warning\n  para \"[!warning]\\ncareful\"\n"
    );
}

#[test]
fn t19_alert_disabled() {
    let o = ParseOptions {
        alerts: false,
        ..opts()
    };
    assert_eq!(
        tree_with("> [!warning]\n> careful\n", o),
        "quote\n  para \"[!warning]\\ncareful\"\n"
    );
}

#[test]
fn t20_front_matter() {
    assert_eq!(tree("---\ntitle: x\n---\n# H\n"), "fm \"title: x\"\nh1 \"H\"\n");
}

#[test]
fn t21_empty_front_matter_is_not() {
    assert_eq!(tree("---\n---\n"), "hr\nhr\n");
    roundtrip("---\n---\n");
}

#[test]
fn t22_setext_at_top() {
    assert_eq!(tree("text\n---\n"), "h2s \"text\"\n");
}

#[test]
fn t23_front_matter_zwsp() {
    let mut d = parse("---\ntitle: x\n---\nbody\n");
    ops::set_front_matter(&mut d, "title: x\n--- note").unwrap();
    assert_eq!(d.serialize(), "---\ntitle: x\n\u{200B}--- note\n---\nbody\n");
    assert_eq!(top_kinds(&d), ["fm", "para"]);
}

#[test]
fn t24_toc_markers() {
    for m in ["[toc]", "[TOC]", "{:toc}", "[[_TOC_]]"] {
        let t = format!("{m}\n");
        assert_eq!(tree(&t), "toc\n", "{m}");
        roundtrip(&t);
    }
}

#[test]
fn t25_irregular_table_roundtrip() {
    let t = "| a | b |\n|---|---|\n| 1 | 2 |";
    roundtrip(t);
    assert_eq!(tree(t), "table2 \"| a | b |\\n|---|---|\\n| 1 | 2 |\"\n");
    roundtrip("|a|   b |\n|:-|-:|\n|  1|2|\n");
}

#[test]
fn t26_new_table() {
    let md = write::new_table(
        &[Align::Left, Align::Right],
        &[vec!["a".into(), "b".into()], vec!["xx".into(), "yy".into()]],
    );
    assert_eq!(md, "| a    |    b |\n| :--- | ---: |\n| xx   |   yy |");
}

#[test]
fn t27_cell_pipe_escaped() {
    let mut d = parse("| a    | b    |\n| ---- | ---- |\n| x    | y    |\n");
    ops::set_table_cell(&mut d, &[0], 1, 0, "a|b").unwrap();
    assert_eq!(d.serialize(), "| a    | b    |\n| ---- | ---- |\n| a\\|b | y    |\n");
    let mut d = parse("| a    | b    |\n| ---- | ---- |\n| x    | y    |\n");
    ops::set_table_cell(&mut d, &[0], 1, 0, "`a|b`").unwrap();
    assert_eq!(d.serialize(), "| a     | b    |\n| ----- | ---- |\n| `a|b` | y    |\n");
}

#[test]
fn t28_indented_paragraph_zwsp() {
    let md = write::paragraph("    starts with spaces");
    assert_eq!(md, "\u{200B}    starts with spaces");
    // re-parses as a paragraph, and the ZWSP is tolerated (not rendered)
    assert_eq!(top_kinds(&parse(&md)), ["para"]);
    assert_eq!(inline_kinds(&md, opts()), ["text:    starts with spaces"]);
}

#[test]
fn t29_crlf_edit_keeps_crlf() {
    let mut d = parse("a\r\nb\r\n");
    assert_eq!(d.line_ending(), LineEnding::CrLf);
    d.edit(1..1, "\nnew line\nanother");
    assert_eq!(d.serialize(), "a\r\nnew line\r\nanother\r\nb\r\n");
    let w = WriteOptions::default();
    ops::insert_block_after(&mut d, Some(0), "# H", &w).unwrap();
    assert!(!d.serialize().replace("\r\n", "").contains('\n'));
}

#[test]
fn t30_no_final_newline_kept() {
    let mut d = parse("single line");
    assert_eq!(d.serialize(), "single line");
    assert!(!d.has_final_newline());
    let end = d.text().len();
    d.edit(end..end, " more");
    assert_eq!(d.serialize(), "single line more");
    let w = WriteOptions::default();
    ops::insert_block_after(&mut d, Some(0), "next", &w).unwrap();
    assert_eq!(d.serialize(), "single line more\n\nnext");
}

fn heading_anchors(text: &str) -> Vec<String> {
    let d = parse(text);
    let o = opts();
    let ctx = InlineContext::default();
    let mut t = SlugTracker::new();
    d.blocks()
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Heading { .. }))
        .map(|b| {
            let c = b.content(d.text());
            t.anchor_for(&plain_text(&c, &parse_inlines(&c, &o, &ctx)))
        })
        .collect()
}

#[test]
fn t31_anchors() {
    assert_eq!(
        heading_anchors("# Intro\n# Intro\n# A — B\n# C++ & D\n"),
        ["intro", "intro-2", "a-----b", "c--d"]
    );
}

#[test]
fn t32_write_anchor() {
    assert_eq!(heading_anchors("# Write\n"), ["write-1"]);
}

#[test]
fn t33_subscript_disabled() {
    assert_eq!(inline_kinds("H~2~O\n", opts()), ["text:H~2~O"]);
}

#[test]
fn t34_subscript_enabled() {
    let o = ParseOptions {
        subscript: true,
        ..opts()
    };
    assert_eq!(inline_kinds("H~2~O\n", o), ["text:H", "sub", "text:2", "text:O"]);
}

#[test]
fn t35_highlight_and_escape() {
    let o = ParseOptions {
        highlight: true,
        ..opts()
    };
    assert_eq!(inline_kinds("==hi==\n", o.clone()), ["highlight", "text:hi"]);
    assert_eq!(inline_kinds("\\==hi==\n", o.clone()), ["escape:\\=", "text:=hi=="]);
    // without the extension `\=` is not an escape
    assert_eq!(inline_kinds("\\==hi==\n", opts()), ["text:\\==hi=="]);
}

#[test]
fn t36_html_block() {
    let t = "<div>\n<b>x</b>\n</div>";
    roundtrip(t);
    assert_eq!(tree(t), "html \"<div>\\n<b>x</b>\\n</div>\"\n");
}

#[test]
fn t37_link_definition() {
    let t = "[id]: http://a.com \"T\"";
    roundtrip(t);
    let d = parse(t);
    assert_eq!(
        d.link_defs().get("ID").cloned(),
        Some(("http://a.com".to_string(), Some("T".to_string())))
    );
    assert_eq!(
        write::link_def("id", "http://a.com", Some("T")),
        "[id]: http://a.com\t\"T\""
    );
}

#[test]
fn t38_footnotes() {
    let t = "[^1]: a\n[^2]: b\n\nText";
    roundtrip(t);
    assert_eq!(top_kinds(&parse(t)), ["fn[1]", "fn[2]", "para"]);
}

#[test]
fn t39_separate_lists_stay_separate() {
    let w = WriteOptions::default();
    let mut d = parse("- a\n- b\n");
    ops::insert_block_after(&mut d, Some(0), "- c\n- d", &w).unwrap();
    assert_eq!(d.serialize(), "- a\n- b\n\n\n- c\n- d\n");
    assert_eq!(top_kinds(&parse(d.text())), ["ul", "ul"]);
    // a different bullet needs no extra blank line
    let mut d = parse("- a\n");
    ops::insert_block_after(&mut d, Some(0), "* c", &w).unwrap();
    assert_eq!(d.serialize(), "- a\n\n* c\n");
}

#[test]
fn t40_quote_depth_limit() {
    assert_eq!(tree(">>>>>>>>>>> deep\n"), "para \">>>>>>>>>>> deep\"\n");
    // exactly 10 levels is still a quote
    let t = format!("{} deep\n", ">".repeat(10));
    assert!(tree(&t).starts_with("quote\n"));
}

#[test]
fn t41_remapped_toc() {
    let o = ParseOptions {
        remap_unicode_punctuation: true,
        ..opts()
    };
    assert_eq!(tree_with("【toc】\n", o), "toc\n");
    assert_eq!(tree("【toc】\n"), "para \"【toc】\"\n");
}

#[test]
fn t42_root_url() {
    let d = parse("---\ntypora-root-url: ./site\n---\n![a](/img/x.png)\n");
    let fm = frontmatter::front_matter(&d);
    let root = frontmatter::get(&fm, "typora-root-url");
    assert_eq!(
        frontmatter::resolve_image_src(Path::new("/docdir"), root, "/img/x.png"),
        PathBuf::from("/docdir/site/img/x.png")
    );
}

#[test]
fn t43_copy_images_to() {
    let d = parse("---\ntypora-copy-images-to: ./${filename}.assets\n---\n");
    let fm = frontmatter::front_matter(&d);
    let v = frontmatter::get(&fm, "typora-copy-images-to").unwrap();
    assert_eq!(
        frontmatter::copy_images_target(Path::new("/w/post.md"), v),
        frontmatter::CopyImagesTarget::Folder {
            folder: PathBuf::from("/w/post.assets"),
            src_prefix: "post.assets".into()
        }
    );
}
