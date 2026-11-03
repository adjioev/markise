//! Nested restart checkpoints (ADR 0006 §7.2.3): an edit inside a large container re-parses
//! from a child boundary with the container stack reconstructed. Every edit must leave exactly
//! the block tree of a fresh parse; the nested path must actually be taken.

use markise_md::*;
use proptest::prelude::*;

/// Documents dominated by containers: flat / nested / ordered / loose lists, quotes with
/// nested lists, footnotes, tabs, items holding code, tables, HTML and math.
fn docs() -> Vec<String> {
    let mut v = Vec::new();
    let mut s = String::from("intro\n\n");
    for i in 0..60 {
        s.push_str(&format!("- item {i} words\n"));
        if i % 7 == 3 {
            s.push_str("  continued line\n");
        }
        if i % 11 == 5 {
            s.push_str("\n  second paragraph\n");
        }
        if i % 13 == 2 {
            s.push_str("  - nested a\n  - nested b\n    - deeper c\n");
        }
        if i % 17 == 8 {
            s.push_str("  ```\n  code in item\n  ```\n");
        }
    }
    s.push_str("\nafter\n");
    v.push(s);
    let mut s = String::new();
    for i in 0..40 {
        s.push_str(&format!("{}. ordered {i} words\n\n", i + 3));
        if i % 5 == 1 {
            s.push_str("   > quote in item\n   > more\n\n");
        }
    }
    v.push(s);
    let mut s = String::from("> # quoted\n>\n");
    for i in 0..50 {
        s.push_str(&format!("> paragraph {i} words\n>\n"));
        if i % 9 == 4 {
            s.push_str("> - list in quote\n>   - nested\n> - two\n>\n");
        }
        if i % 12 == 6 {
            s.push_str("> | a | b |\n> |---|---|\n> | 1 | 2 |\n>\n");
        }
    }
    s.push_str("\ntail\n");
    v.push(s);
    let mut s = String::from("- top\n");
    for i in 0..30 {
        s.push_str(&format!("\t- tab item {i}\n\t\ttab continuation\n"));
        if i % 6 == 0 {
            s.push_str("\t  * mixed marker\n");
        }
    }
    s.push_str("\n[^f]: footnote\n    indented in footnote\n\n    second\n");
    for i in 0..20 {
        s.push_str(&format!("+ plus {i}\n  <div>\n  html</div>\n"));
    }
    s.push_str("\n- a\n  $$\n  x\n  $$\n- b\n");
    v.push(s);
    v.push(merge_doc());
    v
}

/// Containers holding the neighbour-dependent cases (Codex review of PR #17): empty quotes /
/// items (`- >`, `> -`, `> >`, `-`), HTML blocks separated by one line (deleting it merges
/// them, R3.14.2), setext underline candidates, lazy continuation lines, link reference
/// definitions, tables after paragraphs, adjacent lists with different markers.
fn merge_doc() -> String {
    let mut s = String::from("intro\n\n");
    for i in 0..12 {
        s.push_str(&format!(
            "> para {i}\n>\n> <!--\n> c{i}\n> -->\n> ---\n> <!--\n> d{i}\n> -->\n>\n"
        ));
        s.push_str("> >\n>\n> -\n> - x\n>\n> text\n> ===\n>\n> lazy\ncontinued\n>\n");
        s.push_str(&format!(
            "> [l{i}]: /u\n> | a |\n> |---|\n> | 1 |\n>\n> - m\n> + n\n>\n"
        ));
    }
    s.push('\n');
    for i in 0..12 {
        s.push_str(&format!(
            "- >\n\n  para {i}\n  more\n- > -\n- -\n-\n- <div>\n  h{i}\n  </div>\n  x\n  <div>\n  k</div>\n"
        ));
        s.push_str("- a\n  ---\n  b\n  ===\n- [r]: /x\n  t | u\n  --|--\n");
    }
    s.push_str("\n[^e]:\n\n[^f]: >\n\n    para\n");
    s
}

const SNIPS: &[&str] = &[
    "<!--\n",
    "-->",
    "\n<!--\nz\n-->\n",
    "===",
    "\n===",
    "[a]: /u\n",
    "\n>\n",
    "- >",
    "> -",
    "> >",
    "\n-\n",
    "|---|",
    "lazy\n",
    "+ ",
    "</div>",
    "x",
    "ab",
    " ",
    "\n",
    "\n\n",
    "\n- ",
    "\n  - ",
    "\n> ",
    "- ",
    "  ",
    "\t",
    "1. ",
    "```",
    "\n```\n",
    "> ",
    "|",
    "| x |",
    "---",
    "\n---\n",
    "[^f]: n\n",
    "$$",
    "\n    ",
    "#",
    "# h",
    "\\",
    "<div>",
    "* ",
    "2) ",
    "\n\n\n",
    ">",
];

fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn run(doc: &str, ops: &[(usize, usize, usize, bool)], nested: &mut usize) -> Result<(), TestCaseError> {
    let mut d = parse(doc);
    for &(pos, del, snip, ins) in ops {
        let t = d.text();
        let a = floor(t, pos % (t.len() + 1));
        let b = floor(t, a + del).max(a);
        let s = if ins { SNIPS[snip] } else { "" };
        if a == b && s.is_empty() {
            continue;
        }
        let before = d.text().to_string();
        let r = d.apply_patches(&PatchSet::single_verbatim(a..b, s));
        if r.nested_from.is_some() || r.reparsed.start > 0 {
            *nested += r.nested_from.is_some() as usize;
        }
        let full = parse(d.text());
        prop_assert!(
            d.blocks() == full.blocks(),
            "{:?} at {}..{}\nbefore: {:?}\nfull:\n{}\nincr:\n{}",
            s,
            a,
            b,
            before,
            debug_tree(&full),
            debug_tree(&d)
        );
        prop_assert_eq!(d.serialize(), d.text().to_string());
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(1000)
    ))]
    #[test]
    fn prop_nested_restart_equals_full_parse(
        doc in 0usize..5,
        ops in proptest::collection::vec((0usize..8192, 0usize..6, 0usize..SNIPS.len(), any::<bool>()), 1..16),
    ) {
        let docs = docs();
        let mut n = 0;
        run(&docs[doc], &ops, &mut n)?;
    }
}

#[test]
fn nested_path_is_taken_and_correct_on_deterministic_edits() {
    // Enter / typing / Backspace in the middle and at the end of each document
    let mut taken = 0;
    for doc in docs() {
        let n = doc.len();
        for frac in [0.3, 0.5, 0.8, 0.97] {
            let at = floor(&doc, (n as f64 * frac) as usize);
            let line_end = at + doc[at..].find('\n').unwrap_or(0);
            for (r, ins) in [
                (at..at, "x"),
                (line_end..line_end, "\n- new"),
                (line_end..line_end, "\n"),
                (at.saturating_sub(1)..at, ""),
                (line_end..line_end, "\n\n\nloose"),
            ] {
                let r = floor(&doc, r.start)..floor(&doc, r.end);
                let mut d = parse(&doc);
                let res = d.apply_patches(&PatchSet::single_verbatim(r.clone(), ins));
                taken += res.nested_from.is_some() as usize;
                assert_eq!(d.blocks(), parse(d.text()).blocks(), "{ins:?} at {r:?}");
                // and undo
                let back = PatchSet::single_verbatim(r.start..r.start + ins.len(), &doc[r.clone()]);
                d.apply_patches(&back);
                assert_eq!(d.text(), doc);
                assert_eq!(d.blocks(), parse(&doc).blocks());
            }
        }
    }
    assert!(taken > 20, "nested restarts taken: {taken}");
}

#[test]
fn enter_in_a_long_list_reparses_one_item() {
    let mut s = String::new();
    for i in 0..5000 {
        s.push_str(&format!("- item {i} words\n"));
    }
    let mut d = parse(&s);
    let at = s.find("item 2500 words").unwrap() + 15;
    let r = d.apply_patches(&PatchSet::single_verbatim(at..at, "\n- new"));
    assert!(r.nested_from.is_some(), "{r:?}");
    assert_eq!(d.blocks(), parse(d.text()).blocks());
}

/// List tightness after a nested splice is decided from the spliced region when possible
/// (`refinish_spliced`): tight → loose by a blank line in the middle, back to tight, a loose
/// list whose only loose spot is far from the edit, and one whose only loose spot is removed.
#[test]
fn tightness_after_nested_splices() {
    let tight: String = (0..400).map(|i| format!("- item {i} words\n")).collect();
    let mut loose_far = tight.clone();
    loose_far.insert(loose_far.find("- item 3 ").unwrap(), '\n');
    // (doc, needle, delete, insert) applied at the needle's line end; whether the list keeps
    // its tightness (then the model may update the region only: `nested_from`)
    let edits: &[(&str, &str, usize, &str, bool)] = &[
        ("tight", "item 200 words", 0, "\n", false),
        ("tight", "item 200 words", 0, "\n\n- loose", false),
        ("tight", "item 399 words", 0, "\n\n- end", false),
        ("tight", "item 200 words", 0, "\n- tight", true),
        ("loose_far", "item 300 words", 0, "\n- x", true),
        ("loose_far", "item 300 words", 0, "\n\n- y", true),
        ("loose_far", "item 2 words", 1, "", false),
    ];
    for &(which, needle, del, ins, keeps) in edits {
        let doc = if which == "tight" { &tight } else { &loose_far };
        let at = doc.find(needle).unwrap() + needle.len();
        let mut d = parse(doc);
        let r = d.apply_patches(&PatchSet::single_verbatim(at..at + del, ins));
        assert_eq!(r.nested_from.is_some(), keeps, "{which}: {ins:?} after {needle}: {r:?}");
        assert_eq!(d.blocks(), parse(d.text()).blocks(), "{which}: {ins:?} after {needle}");
        // a second edit on the result (state carried over from the spliced tree)
        let t = d.text().to_string();
        let mid = t.find("item 100 words").unwrap() + 14;
        d.apply_patches(&PatchSet::single_verbatim(mid..mid, "\n\n- z"));
        assert_eq!(d.blocks(), parse(d.text()).blocks(), "{which}: second edit");
        let t = d.text().to_string();
        d.apply_patches(&PatchSet::single_verbatim(mid..mid + 5, ""));
        assert_eq!(d.text(), t[..mid].to_string() + &t[mid + 5..]);
        assert_eq!(d.blocks(), parse(d.text()).blocks(), "{which}: undo of the second edit");
    }
}

/// Codex review of PR #17 (P1): deleting the line between two HTML blocks inside a container
/// makes them adjacent; a fresh parse merges them (R3.14.2), so the nested re-sync must not
/// keep the old second block. Inside a quote and inside a list item.
#[test]
fn nested_resync_respects_adjacent_html_merging() {
    let quote = "> intro\n>\n> <!--\n> a\n> -->\n> ---\n> <!--\n> b\n> -->\n";
    let list = "- intro\n\n  <!--\n  a\n  -->\n  ---\n  <!--\n  b\n  -->\n- tail\n";
    for (doc, sep) in [(quote, "> ---\n"), (list, "  ---\n")] {
        let at = doc.find(sep).unwrap();
        for (r, ins) in [(at..at + sep.len(), ""), (at..at + sep.len() - 1, "")] {
            let mut d = parse(doc);
            d.apply_patches(&PatchSet::single_verbatim(r.clone(), ins));
            assert_eq!(d.blocks(), parse(d.text()).blocks(), "{doc:?} delete {r:?}");
            assert_eq!(d.serialize(), d.text());
        }
    }
    // and the reverse: separating two merged HTML blocks again
    let merged = "> intro\n>\n> <!--\n> a\n> -->\n> <!--\n> b\n> -->\n";
    let at = merged.find("> <!--\n> b").unwrap();
    let mut d = parse(merged);
    d.apply_patches(&PatchSet::single_verbatim(at..at, "> ---\n"));
    assert_eq!(d.blocks(), parse(d.text()).blocks());
}

/// Found by the extended generator: a paragraph left after extracting a link reference
/// definition opened on the definition's line; a blank line inserted between them makes the
/// rest a fresh paragraph (here: a table), so the nested re-sync must not reuse it.
#[test]
fn nested_resync_skips_definition_remainders() {
    let doc = "> intro\n>\n> lazy\n>\n> [l]: /u\n> | a |\n> |---\n>\n> tail\n";
    let at = doc.find("/u\n").unwrap() + 2;
    let mut d = parse(doc);
    d.apply_patches(&PatchSet::single_verbatim(at..at + 1, "\n>\n"));
    assert_eq!(d.blocks(), parse(d.text()).blocks());
    let doc = "- a\n- b\n\n  [l]: /u\n  t | u\n  --|--\n- c\n";
    let at = doc.find("/u\n").unwrap() + 2;
    let mut d = parse(doc);
    d.apply_patches(&PatchSet::single_verbatim(at..at + 1, "\n\n"));
    assert_eq!(d.blocks(), parse(d.text()).blocks());
}

/// Found by the extended generator (top-level path): a block re-parsed as HTML directly after
/// an unchanged HTML block merges into it (R3.14.2): the re-parse restarts before it.
#[test]
fn top_level_restart_includes_adjacent_html_predecessor() {
    let doc = "para\n\n<!--\nz\n-->\n|---|x |\n\nafter\n";
    let at = doc.find("|---|").unwrap();
    let mut d = parse(doc);
    d.apply_patches(&PatchSet::single_verbatim(at..at + 4, "<!--\n"));
    assert_eq!(d.blocks(), parse(d.text()).blocks());
}
