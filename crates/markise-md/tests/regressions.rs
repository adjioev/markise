//! Regression tests for the M0 review findings (P1/P2 items).

use markise_md::*;

#[test]
fn p2_7_bom_is_file_trivia() {
    let t = "\u{FEFF}---\ntitle: X\n---\n# H\n";
    let d = parse(t);
    assert!(d.has_bom());
    assert_eq!(debug_tree(&d), "fm \"title: X\"\nh1 \"H\"\n");
    assert_eq!(d.serialize(), t);
    assert_eq!(frontmatter::get(&frontmatter::front_matter(&d), "title"), Some("X"));
    // incremental edits keep the BOM handling
    let mut d2 = d.clone();
    let end = d2.text().len();
    d2.edit(end..end, "more\n");
    assert_eq!(d2.blocks(), parse(d2.text()).blocks());
    // removing / adding the BOM re-parses consistently
    let mut d3 = d.clone();
    d3.edit(0..3, "");
    assert_eq!(debug_tree(&d3), "fm \"title: X\"\nh1 \"H\"\n");
    d3.edit(0..0, "\u{FEFF}");
    assert_eq!(d3.text(), t);
    assert_eq!(d3.blocks(), parse(t).blocks());
    // an edit right after the BOM
    let mut d4 = parse("\u{FEFF}para\n");
    d4.edit(3..3, "---\nk: v\n---\n");
    assert_eq!(d4.blocks(), parse(d4.text()).blocks());
    assert_eq!(debug_tree(&d4), "fm \"k: v\"\npara \"para\"\n");
    // ops insert after the BOM, not before it
    let w = WriteOptions::default();
    let mut d5 = parse("\u{FEFF}text\n");
    ops::set_front_matter(&mut d5, "a: b").unwrap();
    assert_eq!(d5.text(), "\u{FEFF}---\na: b\n---\ntext\n");
    let mut d6 = parse("\u{FEFF}text\n");
    ops::insert_block_after(&mut d6, None, "# T", &w).unwrap();
    assert_eq!(d6.text(), "\u{FEFF}# T\n\ntext\n");
}

#[test]
fn p2_8_insert_separates_both_sides() {
    let w = WriteOptions::default();
    let mut d = parse("# A\nold\n");
    ops::insert_block_after(&mut d, Some(0), "new", &w).unwrap();
    assert_eq!(d.text(), "# A\n\nnew\n\nold\n");
    assert_eq!(debug_tree(&d), "h1 \"A\"\npara \"new\"\npara \"old\"\n");
    // existing blank lines are reused, not doubled
    let mut d = parse("# A\n\nold\n");
    ops::insert_block_after(&mut d, Some(0), "new", &w).unwrap();
    assert_eq!(d.text(), "# A\n\nnew\n\nold\n");
    // list followed by a same-type list needs two blank lines on the far side too
    let mut d = parse("# A\n- x\n");
    ops::insert_block_after(&mut d, Some(0), "- y", &w).unwrap();
    assert_eq!(
        debug_tree(&parse(d.text()))
            .lines()
            .filter(|l| l.starts_with("ul"))
            .count(),
        2
    );
    // at the start
    let mut d = parse("old\n");
    ops::insert_block_after(&mut d, None, "new", &w).unwrap();
    assert_eq!(d.text(), "new\n\nold\n");
}

#[test]
fn p1_1_multibyte_after_dollar_in_cells() {
    assert_eq!(table::escape_cell("$é$"), "$é$");
    let mut d = parse("| a    | b    |\n| ---- | ---- |\n| x    | y    |\n");
    ops::set_table_cell(&mut d, &[0], 1, 1, "$é$ и |").unwrap();
    assert!(d.text().contains("$é$ и \\|"));
    let md = write::new_table(&[Align::None], &[vec!["$ü".into()], vec!["`é|".into()]]);
    assert!(
        parse(&md).blocks()[0].kind
            == BlockKind::Table {
                aligns: vec![Align::None]
            }
    );
}

#[test]
fn p1_3_incremental_sync_checks_line_start() {
    let mut d = parse("# a\n\n    \n    # ");
    d.edit(5..11, "");
    assert_eq!(d.blocks(), parse(d.text()).blocks());
    assert_eq!(debug_tree(&d), "h1 \"a\"\nh1 \"\"\n");
}

// ---- second review pass ----

fn links(md: &str) -> Vec<(String, String)> {
    use markise_md::inline::*;
    let d = parse(md);
    let ctx = InlineContext {
        link_defs: d.link_defs(),
        footnotes: d.footnote_labels().into_iter().collect(),
    };
    let mut out = Vec::new();
    for b in d.blocks() {
        b.walk(&mut |b, _| {
            if matches!(b.kind, BlockKind::Paragraph | BlockKind::Heading { .. }) {
                let c = b.content(d.text());
                fn go(s: &str, n: &[Inline], out: &mut Vec<(String, String)>) {
                    for x in n {
                        if let InlineKind::Link { dest, .. } = &x.kind {
                            out.push((plain_text(s, &x.children), dest.clone()));
                        }
                        go(s, &x.children, out);
                    }
                }
                go(&c, &parse_inlines(&c, d.options(), &ctx), &mut out);
            }
        });
    }
    out
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[test]
fn r2_1_failed_angle_dest_does_not_block_raw_dest() {
    assert_eq!(links("[bad](<foo[ok](bar))\n"), [pair("ok", "bar")]);
}

#[test]
fn r2_2_balanced_parens_in_dest() {
    assert_eq!(links("[a](b(c)d)\n"), [pair("a", "b(c)d")]);
    // unbalanced parentheses are not a destination (CommonMark)
    assert!(links("[a](b(c d)\n").is_empty());
    assert_eq!(links("[a](b()[x](y)\n"), [pair("x", "y")]);
    assert_eq!(links("[a](\\(b)\n"), [pair("a", "(b")]);
}

#[test]
fn r2_5_multiline_label_in_containers() {
    assert_eq!(
        links("> [foo\n> bar]: /url\n>\n> [foo bar]\n"),
        [pair("foo bar", "/url")]
    );
    assert_eq!(
        links("- [foo\n  bar]: /url \"multi\n  line\"\n\n  [Foo Bar]\n"),
        [pair("Foo Bar", "/url")]
    );
    let d = parse("> [t]: /u \"a\n> b\"\n");
    assert_eq!(d.link_defs().get("T").unwrap().1.as_deref(), Some("a\nb"));
    assert_eq!(d.serialize(), "> [t]: /u \"a\n> b\"\n");
}

#[test]
fn r2_6_label_limit_applies_to_label_not_text() {
    let text = "a".repeat(1000);
    assert_eq!(links(&format!("[{text}][x]\n\n[x]: /url\n")), [pair(&text, "/url")]);
    // a 999-character label of multi-byte chars is still a label; 1000 is not
    let lab = "é".repeat(999);
    assert_eq!(links(&format!("[t][{lab}]\n\n[{lab}]: /u\n")), [pair("t", "/u")]);
    let lab = "é".repeat(1000);
    assert!(links(&format!("[t][{lab}]\n\n[{lab}]: /u\n")).is_empty());
}

/// Incremental-parse mismatch found by the M2c property test (30 000 cases): the nested
/// restart rebuilt a list opened on a task item's marker line without consuming the task
/// box, so later lines re-parsed as indented code (MD-INC-1, fixed in `prepare_nested`).
#[test]
fn md_inc_1_nested_restart_after_task_box() {
    use markise_md::{Document, ParseOptions, PatchSet, debug_tree};
    let src = "- [ ] 1. \n      2. \n      3. = H\n";
    let mut d = Document::parse(src, ParseOptions::default());
    d.apply_patches(&PatchSet::single_verbatim(31..32, ""));
    let full = Document::parse(d.text(), ParseOptions::default());
    assert_eq!(debug_tree(&d), debug_tree(&full));
}
