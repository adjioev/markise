//! Extended callouts (ADR 0025): what the first line declares, in a tree of quotes; unchanged
//! bytes; edits that stay minimal; incremental parses equal to full ones.

use markise_md::*;
use proptest::prelude::*;

fn tree(s: &str) -> String {
    debug_tree(&parse(s))
}

#[test]
fn every_form_of_the_first_line() {
    assert_eq!(
        tree("> [!warning] Optional title\n> Body.\n"),
        "callout-warning\n  para \"[!warning] Optional title\\nBody.\"\n"
    );
    assert!(tree("> [!success]- Collapsed\n> Hidden\n").starts_with("callout-success-\n"));
    assert!(tree("> [!info]+ Open\n> Body\n").starts_with("callout-info+\n"));
    assert!(
        tree("> [!HINT]\n> x\n").starts_with("callout-tip\n"),
        "an alias, any case"
    );
    assert!(
        tree("> [!mine]\n> x\n").starts_with("callout-note\n"),
        "unknown: the note style"
    );
}

#[test]
fn the_narrow_case_stays_an_alert() {
    assert!(tree("> [!WARNING]\n> x\n").starts_with("alert-warning\n"));
    assert!(tree("> [!note]  \n> x\n").starts_with("alert-note\n"));
    assert!(tree("> [!NOTE] Title\n> x\n").starts_with("callout-note\n"));
    assert!(tree("> [!NOTE]-\n> x\n").starts_with("callout-note-\n"));
}

#[test]
fn what_is_not_a_callout_stays_a_quote() {
    for s in [
        "> [!a](url)\n",
        "> [!a]text\n",
        "> [!a b]\n",
        "> text\n> [!a]\n",
        "> # [!a]\n",
        "> \n> ```\n> [!a]\n> ```\n",
    ] {
        assert!(tree(s).starts_with("quote\n"), "{s:?}: {}", tree(s));
    }
    assert!(!tree("[!a] outside a quote\n").contains("callout"));
}

#[test]
fn the_option_switches_it_off_as_a_unit() {
    let o = ParseOptions {
        callouts: false,
        ..Default::default()
    };
    let plain = |s: &str| debug_tree(&Document::parse(s, o.clone()));
    assert!(plain("> [!warning] T\n> x\n").starts_with("quote\n"));
    assert!(plain("> [!info]\n> x\n").starts_with("quote\n"));
    assert!(plain("> [!NOTE]-\n> x\n").starts_with("quote\n"));
    assert!(
        plain("> [!NOTE]\n> x\n").starts_with("alert-note\n"),
        "alerts are their own option"
    );
}

#[test]
fn nested_and_inside_other_containers() {
    let t = tree("> [!info] Outer\n> a\n>\n> > [!warning]- Inner\n> > b\n");
    assert!(t.starts_with("callout-info\n"), "{t}");
    assert!(t.contains("callout-warning-"), "{t}");
    assert!(tree("- > [!tip] In a list\n  > a\n").contains("callout-tip"));
    assert!(tree("> > [!bug] In a quote\n").contains("callout-bug"));
    assert!(tree("[^n]: > [!todo] In a note\n").contains("callout-todo"));
}

const CASES: &[&str] = &[
    "> [!warning] Optional title\n> Body.\n",
    "> [!success]- Collapsed with a title\n> Hidden until opened.\n",
    "> [!info]+ Open by default\n> Body.\n",
    ">[!tip]\n>x\n",
    ">    [!TIP]   spaces   \n>   x  \n",
    "> [!note]\r\n> crlf\r\n\r\ntail\r\n",
    "> [!x] no newline at the end",
    "> [!a]- \n",
    "> [!a]\n>\n> > [!b]+ **T**\n> > c\n>\n> d\n\ne\n",
    "- > [!tip] T\n  > a\n- b\n",
    "1. x\n\n   > [!bug]- T\n   >\n   > body\n",
    "> [!заметка] Тема\n> текст\n",
];

#[test]
fn bytes_stay() {
    for c in CASES {
        let d = parse(c);
        assert_eq!(d.serialize(), *c);
        assert_eq!(d.text(), *c);
    }
}

#[test]
fn an_edit_inside_touches_only_its_bytes() {
    let src = "> [!warning]- Title\n> Body.\n\nafter\n";
    let at = src.find("Body").unwrap();
    let mut d = parse(src);
    d.apply_patches(&PatchSet::single_verbatim(at..at, "Some "));
    assert_eq!(d.serialize(), "> [!warning]- Title\n> Some Body.\n\nafter\n");
    assert_eq!(d.blocks(), parse(d.text()).blocks());
    // the fold mark goes: a callout of another shape, and an incremental parse agrees
    let at = src.find("]-").unwrap() + 1;
    let mut d = parse(src);
    d.apply_patches(&PatchSet::single_verbatim(at..at + 1, ""));
    assert!(debug_tree(&d).starts_with("callout-warning\n"));
    assert_eq!(d.blocks(), parse(d.text()).blocks());
}

const PIECES: &[&str] = &[
    "> ",
    ">",
    "[!",
    "[!note]",
    "[!warning]",
    "[!mine]",
    "[!TIP]",
    "]",
    "]-",
    "]+",
    "-",
    "+",
    " ",
    "Title",
    "**b**",
    "\n",
    "\n\n",
    "\r\n",
    "> > ",
    "- ",
    "1. ",
    "```\n",
    "text",
    "\t",
    "[",
    "!",
    "é",
    "\u{301}",
];

/// A document made of pieces that together read as callouts, some of the time.
fn doc() -> impl Strategy<Value = String> {
    proptest::collection::vec(prop::sample::select(PIECES), 0..40).prop_map(|p| p.concat())
}

fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

proptest! {
    /// Bytes survive a parse, and an option never changes them.
    #[test]
    fn prop_round_trip(src in doc()) {
        prop_assert_eq!(parse(&src).serialize(), src.clone());
        let off = ParseOptions { callouts: false, ..Default::default() };
        prop_assert_eq!(Document::parse(src.clone(), off).serialize(), src);
    }

    /// Every edit leaves the tree of a fresh parse, callout state included.
    #[test]
    fn prop_edits_equal_a_full_parse(
        src in doc(),
        ops in proptest::collection::vec((0usize..256, 0usize..4, prop::sample::select(PIECES), any::<bool>()), 1..8),
    ) {
        let mut d = parse(&src);
        for (pos, del, piece, ins) in ops {
            let t = d.text();
            let a = floor(t, pos % (t.len() + 1));
            let b = floor(t, a + del).max(a);
            let s = if ins { piece } else { "" };
            if a == b && s.is_empty() {
                continue;
            }
            let expected = format!("{}{s}{}", &t[..a], &t[b..]);
            d.apply_patches(&PatchSet::single_verbatim(a..b, s));
            prop_assert_eq!(d.text(), expected.as_str());
            prop_assert_eq!(d.serialize(), expected.clone());
            let full = parse(&expected);
            prop_assert!(d.blocks() == full.blocks(), "{:?}\nfull {}\nincr {}", expected, debug_tree(&full), debug_tree(&d));
        }
    }
}
