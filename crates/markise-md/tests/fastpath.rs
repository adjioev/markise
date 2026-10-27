//! Intra-line fast path (ADR 0006 §7.2.1): an edit accepted by the fast path must leave exactly
//! the block tree a full parse produces, and typing letters inside paragraph lines must take it.

use markise_md::*;
use proptest::prelude::*;

const DOCS: &[&str] = &[
    "Intro text here\nsecond line words\n\n- item one\n- item two\n  more words\n\n> quote line\n> lazy\ncontinued\n\nTitle words\n=====\n\n# Heading text #\n\n| a | b |\n|---|---|\n| c | d |\n\nname | other\n--- | ---\n\nx \\| y | z\n---|---\n\n[label]: http://x \"title\"\n\nsee [label] here\n\nhard break  \nnext\n\n$$\nmath\n$$\n\n1. one\n2. two\n\n```\ncode\n```\n\nend\n",
    "\u{feff}Word one\r\nword two\r\n\r\n> - nested list item text\r\n>   continued here\r\n",
    "Para\n\n$$\nunclosed math\n\nmore text below\n\nand more\n",
    "```rust\nlet a = 1; // x\n  b `c` d\n~~~ e\n$x\n```\n\n    indented code line\n    more  code\n\n> ```\n> quoted code\n> ```\n\n- item\n  ```\n  in item code\n  ```\n\n$$\nmath line\n\\] not closer\n$$\n\n$$\nunclosed math\n\ntext\n\n~~~~\ncode ~~~\n~~~~\n",
    "abc def\n---\n\nabc\n===\n\nword 1. two\nnext 1) x\nline <div> tag\nsee *emph* and **strong** `code` <span>html</span>\nterm: definition\nalpha [link](u) [ref] [^fn]\n\n[ref]: /url\n\n[^fn]: note text\n\n  indented text line\n\n<div>\nhtml block text\n</div>\n\n- [ ] task text\n- [x] done text\n\n> [!NOTE]\n> note body text\n\nabc\n    code like\n",
];

/// Characters typed or deleted (punctuation included: the rule must reject what matters).
const CHARS: &[&str] = &[
    "ab", "a b", "x\ty", "a", "Z", "é", " ", ".", "*", "_", "`", "-", "=", "|", "\\", "#", "[", "]", ">", "<", "1",
    ":", "  ", "$", "~", "!",
];

fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(512)
    ))]
    #[test]
    fn prop_fast_path_equals_full_parse(
        doc in 0..DOCS.len() + 1,
        ops in proptest::collection::vec((0usize..8192, 0usize..3, 0usize..CHARS.len(), any::<bool>()), 1..30),
    ) {
        let src = match DOCS.get(doc) {
            Some(d) => d.to_string(),
            None => markise_corpus::generate(6000, doc as u64),
        };
        let mut d = parse(&src);
        for (pos, del, c, ins) in ops {
            let t = d.text();
            let a = floor(t, pos % (t.len() + 1));
            let b = floor(t, a + del).max(a);
            let s = if ins { CHARS[c] } else { "" };
            if a == b && s.is_empty() {
                continue;
            }
            let before = d.text().to_string();
            let r = d.apply_patches(&PatchSet::single_verbatim(a..b, s));
            let full = parse(d.text());
            prop_assert!(
                d.blocks() == full.blocks(),
                "{:?} at {}..{} (fast: {:?})\nbefore: {:?}\nafter:  {:?}\nfull: {}\nincr: {}",
                s, a, b, r.inline_leaf, before, d.text(), debug_tree(&full), debug_tree(&d)
            );
        }
    }
}

#[test]
fn typing_letters_in_a_paragraph_takes_the_fast_path() {
    let mut d = parse("# T\n\nsome paragraph text\nmore\n\n- list item words\n");
    for word in ["paragraph", "text", "words"] {
        let at = d.text().find(word).unwrap() + 2;
        let r = d.apply_patches(&PatchSet::single_verbatim(at..at, "x"));
        assert!(r.inline_leaf.is_some(), "in {word}: {r:?}");
        assert_eq!(d.blocks(), parse(d.text()).blocks());
    }
    // structural candidates are rejected: a line start, a pipe, a backslash
    for (at, s) in [(5, "x"), (12, "|"), (12, "\\")] {
        let r = d.apply_patches(&PatchSet::single_verbatim(at..at, s));
        assert!(r.inline_leaf.is_none(), "{s:?} at {at}");
        assert_eq!(d.blocks(), parse(d.text()).blocks());
    }
}

#[test]
fn typing_in_code_lines_takes_the_fast_path() {
    let mut d = parse("text\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n\n    indented code\n\n$$\nx + y\n$$\n");
    for word in ["a = 1", "b = 2", "dented", "+ y"] {
        let at = d.text().find(word).unwrap() + 1;
        let r = d.apply_patches(&PatchSet::single_verbatim(at..at, "q|#\\`"));
        assert!(r.inline_leaf.is_some(), "in {word}: {r:?}");
        assert_eq!(d.blocks(), parse(d.text()).blocks());
    }
    // a line that could become a closing fence or math delimiter is re-parsed
    let at = d.text().find("let b").unwrap();
    let r = d.apply_patches(&PatchSet::single_verbatim(at..at, "```"));
    assert!(r.inline_leaf.is_none());
    assert_eq!(d.blocks(), parse(d.text()).blocks());
}
