//! Lossless round-trip and incremental re-parse tests (spec 01 §1, R2.2–R2.4, NFR-4 design).

use markise_corpus::{Profile, generate_profile};
use markise_md::inline::{InlineContext, parse_inlines};
use markise_md::*;
use proptest::prelude::*;

fn corpus_files() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (p.display().to_string(), String::from_utf8(bytes).unwrap())
        })
        .collect();
    v.sort();
    assert!(v.len() >= 8);
    v
}

/// Parses every leaf's inline content; must not panic.
fn touch_inlines(d: &Document) {
    let ctx = InlineContext {
        link_defs: d.link_defs(),
        footnotes: d.footnote_labels().into_iter().collect(),
    };
    for b in d.blocks() {
        b.walk(&mut |b, _| {
            if !b.is_container() && !b.lines.is_empty() {
                let c = b.content(d.text());
                let nodes = parse_inlines(&c, d.options(), &ctx);
                for n in &nodes {
                    assert!(n.span.end <= c.len());
                }
            }
        });
    }
}

fn check_doc(name: &str, text: &str, opts: ParseOptions) {
    let d = Document::parse(text, opts);
    assert_eq!(d.serialize(), text, "round-trip changed bytes: {name}");
    d.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
    touch_inlines(&d);
}

fn all_option_sets() -> Vec<ParseOptions> {
    let all_on = ParseOptions {
        strict: false,
        legacy_inline_math: false,
        subscript: true,
        superscript: true,
        highlight: true,
        diagrams: true,
        remap_unicode_punctuation: true,
        strict_number_start: true,
        ..Default::default()
    };
    let all_off = ParseOptions {
        strict: true,
        inline_math: false,
        latex_delimiters: false,
        code_math: false,
        alerts: false,
        auto_links: false,
        block_html: false,
        emoji: false,
        wikilinks: false,
        ..Default::default()
    };
    vec![ParseOptions::default(), all_on, all_off]
}

#[test]
fn authored_corpus_roundtrips() {
    for (name, text) in corpus_files() {
        for o in all_option_sets() {
            check_doc(&name, &text, o);
        }
    }
}

#[test]
fn authored_corpus_line_endings() {
    let crlf = corpus_files()
        .into_iter()
        .find(|(n, _)| n.ends_with("crlf.md"))
        .unwrap()
        .1;
    assert_eq!(parse(&crlf).line_ending(), LineEnding::CrLf);
    let mixed = corpus_files()
        .into_iter()
        .find(|(n, _)| n.ends_with("mixed-eol.md"))
        .unwrap()
        .1;
    // R2.2: any CRLF → CRLF; unchanged text still round-trips byte-identically (C20).
    let d = parse(&mixed);
    assert_eq!(d.line_ending(), LineEnding::CrLf);
    assert_eq!(d.serialize(), mixed);
    let mut d2 = d.clone();
    d2.set_line_ending(LineEnding::Lf);
    assert_eq!(d2.text(), "mixed\nendings\nhere\nand\nthere\n");
}

#[test]
fn generated_corpora_roundtrip() {
    for (seed, profile, crlf) in [
        (1, Profile::Mixed, false),
        (2, Profile::Mixed, true),
        (3, Profile::Tables, false),
        (4, Profile::Math, false),
        (5, Profile::Cjk, true),
        (6, Profile::Cjk, false),
    ] {
        let text = generate_profile(200_000, seed, profile, crlf);
        for o in all_option_sets() {
            check_doc(&format!("gen-{seed}"), &text, o);
        }
    }
}

#[test]
fn every_block_kind_appears_in_corpus() {
    let mut text = String::new();
    for (_, t) in corpus_files() {
        text.push_str(&t);
        text.push_str("\n\n");
    }
    // front matter only counts at the top; parse the file separately
    let fm = corpus_files()
        .into_iter()
        .find(|(n, _)| n.ends_with("frontmatter.md"))
        .unwrap()
        .1;
    let mut kinds = std::collections::BTreeSet::new();
    for src in [&text, &fm] {
        let d = parse(src);
        for b in d.blocks() {
            b.walk(&mut |b, _| {
                let k = format!("{:?}", b.kind);
                kinds.insert(k.split([' ', '{', '(']).next().unwrap().to_string());
            });
        }
    }
    for k in [
        "Paragraph",
        "Heading",
        "ThematicBreak",
        "FencedCode",
        "IndentedCode",
        "MathBlock",
        "HtmlBlock",
        "Table",
        "BlockQuote",
        "List",
        "ListItem",
        "FrontMatter",
        "Toc",
        "FootnoteDef",
        "LinkRefDef",
    ] {
        assert!(kinds.contains(k), "corpus lacks {k}: {kinds:?}");
    }
}

/// Incremental edit must give exactly the same tree as a full parse of the new text.
fn check_incremental(text: &str, start: usize, end: usize, insert: &str, opts: &ParseOptions) {
    let mut d = Document::parse(text, opts.clone());
    d.edit(start..end, insert);
    let full = Document::parse(d.text(), opts.clone());
    assert_eq!(
        d.blocks(),
        full.blocks(),
        "incremental != full\ntext: {text:?}\nedit: {start}..{end} -> {insert:?}\nnew: {:?}\n--- incremental\n{}--- full\n{}",
        d.text(),
        debug_tree(&d),
        debug_tree(&full)
    );
    assert_eq!(d.serialize(), d.text());
}

#[test]
fn incremental_examples() {
    let o = ParseOptions::default();
    check_incremental("a\n\nb\n\nc\n", 3, 3, "```\n", &o);
    check_incremental("a\n\n```\nx\n```\n\nb\n", 3, 6, "", &o);
    check_incremental("$$\nx\n\npara\n\ny\n", 11, 11, "\n$$\n", &o);
    check_incremental("---\ntitle\n\n# h\n\nend\n", 16, 16, "\n---\n", &o);
    check_incremental("- a\n\n- b\n\nc\n", 4, 4, "\n", &o);
    check_incremental("<!-- x -->\n\n<div>\n", 10, 11, "", &o);
    check_incremental("# a\npara\n# b\n", 4, 8, "text\n---", &o);
    // Regression (review P1 #3): the old block's line start/indentation changed.
    check_incremental("# a\n\n    \n    # ", 5, 11, "", &o);
    check_incremental("a\n\n  b\n\n    c\n", 5, 6, "", &o);
    check_incremental("x\n\n     y\n", 3, 4, "", &o);
}

#[test]
fn incremental_on_generated_corpus() {
    let text = generate_profile(60_000, 9, Profile::Mixed, false);
    let o = ParseOptions::default();
    let mut rng = markise_corpus::Rng::new(77);
    let inserts = [
        "\n", "\n\n", "```\n", "$$\n", "- ", "> ", "# ", "|---|\n", "x", "", "---\n", "<div>\n",
    ];
    for _ in 0..150 {
        let mut a = rng.below(text.len());
        while !text.is_char_boundary(a) {
            a -= 1;
        }
        let mut b = (a + rng.below(40)).min(text.len());
        while !text.is_char_boundary(b) {
            b -= 1;
        }
        let ins = *rng.pick(&inserts);
        check_incremental(&text, a, b, ins, &o);
    }
}

/// Case count, overridable with `PROPTEST_CASES` for long fuzzing runs.
fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const FRAGMENTS: &[&str] = &[
    "# ",
    "#",
    "## x ##",
    "> ",
    ">",
    "》",
    "- ",
    "* ",
    "+ ",
    "1. ",
    "3) ",
    "```",
    "~~~",
    "```math",
    "$$",
    "$$ {x}",
    "\\[",
    "\\]",
    "---",
    "...",
    "***",
    "=",
    "| a | b |",
    "|---|:-:|",
    "a | b",
    "[^1]: ",
    "[x]: /u \"t\"",
    "[x]",
    "<div>",
    "</div>",
    "<!--",
    "-->",
    "<pre>",
    "</pre>",
    "    ",
    "\t",
    "text",
    "日本",
    "\n",
    "\n",
    "\n",
    "\r\n",
    "\r",
    "\n\n",
    "[toc]",
    " ",
    "   ",
    "【toc】",
    "[ ] ",
    "[x] ",
    "  ",
    "$x$",
    "**",
    "_",
    "`",
    "\u{2424}",
    "\u{200B}",
    "~~",
    "==",
    "![a](b)",
    "<https://x.y>",
    "&amp;",
    "\\",
    "{:toc}",
];

fn doc_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(FRAGMENTS), 0..60).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(512), .. ProptestConfig::default() })]

    #[test]
    fn prop_roundtrip(text in doc_strategy()) {
        for o in all_option_sets() {
            let d = Document::parse(text.as_str(), o);
            prop_assert_eq!(d.serialize(), text.clone());
            prop_assert!(d.validate().is_ok());
            touch_inlines(&d);
        }
    }

    #[test]
    fn prop_incremental_equals_full(
        text in doc_strategy(),
        a in 0usize..400,
        len in 0usize..40,
        ins in prop::collection::vec(prop::sample::select(FRAGMENTS), 0..4),
        opt in 0usize..3,
    ) {
        let mut a = a.min(text.len());
        while !text.is_char_boundary(a) { a -= 1; }
        let mut b = (a + len).min(text.len());
        while !text.is_char_boundary(b) { b -= 1; }
        let o = all_option_sets()[opt].clone();
        check_incremental(&text, a, b, &ins.concat(), &o);
    }
}

const INLINE_FRAGMENTS: &[&str] = &[
    "*", "**", "_", "__", "~", "~~", "=", "==", "^", "`", "``", "$", "$$", "\\(", "\\)", "\\", "[", "]", "(", ")", "!",
    "<", ">", "&", ";", ":", "\"", "'", "“", "”", "（", "）", "【", "】", "日本", "é", " ", "  ", "\n", "a", "http://",
    "www.", "x.com", "mailto:", "@", "{", "}", "#", "smile", "<!--", "-->", "<u>", "</u>", "\u{A0}", "[^1]", "[x]",
    "\u{200B}",
];

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(1024), .. ProptestConfig::default() })]

    #[test]
    fn prop_inline_never_panics(
        parts in prop::collection::vec(prop::sample::select(INLINE_FRAGMENTS), 0..40),
        any in "\\PC{0,30}",
    ) {
        let s = format!("{}{any}", parts.concat());
        let mut ctx = InlineContext::default();
        ctx.footnotes.insert("1".into());
        ctx.link_defs.insert("X".into(), ("/u".into(), None));
        for o in all_option_sets() {
            let nodes = parse_inlines(&s, &o, &ctx);
            fn check(s: &str, n: &[markise_md::inline::Inline]) {
                for x in n {
                    assert!(x.span.start <= x.span.end && x.span.end <= s.len());
                    assert!(s.is_char_boundary(x.span.start) && s.is_char_boundary(x.span.end));
                    check(s, &x.children);
                }
            }
            check(&s, &nodes);
        }
    }
}

#[test]
fn incremental_reparse_is_local() {
    // NFR-4 design: a keystroke re-parses only the neighbourhood of the edit.
    let text = generate_profile(1 << 20, 3, Profile::Mixed, false);
    let mut d = Document::parse(text.as_str(), ParseOptions::default());
    let mut rng = markise_corpus::Rng::new(5);
    let mut max = 0;
    for _ in 0..200 {
        let mut at = rng.below(d.text().len());
        while !d.text().is_char_boundary(at) {
            at -= 1;
        }
        let r = d.edit(at..at, "x");
        max = max.max(r.reparsed.len());
        d.edit(at..at + 1, "");
    }
    assert_eq!(d.text(), text);
    assert!(max < 64 * 1024, "re-parsed {max} bytes for a single keystroke");
}

/// Regression (review P1 #2): unbounded nesting must not overflow the stack.
#[test]
fn deep_nesting_is_bounded() {
    let deep_list = format!("{}x\n", "- ".repeat(100_000));
    let deep_quote = format!("{}x\n", "> ".repeat(100_000));
    let deep_mixed = format!("{}x\n", "- > 1. [^a]: ".repeat(20_000));
    let deep_emph = format!("{}x{}\n", "*a ".repeat(50_000), " a*".repeat(50_000));
    let deep_links = format!("{}x{}\n", "[".repeat(50_000), "](u)".repeat(50_000));
    let deep_images = format!("{}x{}\n", "![".repeat(50_000), "](u)".repeat(50_000));
    for text in [deep_list, deep_quote, deep_mixed, deep_emph, deep_links, deep_images] {
        let d = parse(&text);
        assert_eq!(d.serialize(), text);
        let mut depth = 0;
        for b in d.blocks() {
            b.walk(&mut |_, dd| depth = depth.max(dd));
        }
        assert!(depth <= markise_md::MAX_NESTING + 2, "block depth {depth}");
        touch_inlines(&d);
        let ctx = InlineContext::default();
        for b in d.blocks() {
            b.walk(&mut |b, _| {
                if !b.is_container() && !b.lines.is_empty() {
                    let c = b.content(d.text());
                    let nodes = parse_inlines(&c, d.options(), &ctx);
                    fn depth(n: &[markise_md::inline::Inline]) -> usize {
                        n.iter().map(|x| 1 + depth(&x.children)).max().unwrap_or(0)
                    }
                    assert!(depth(&nodes) <= markise_md::inline::MAX_INLINE_DEPTH + 1);
                    // flattened text keeps every byte of the source
                    let plain = markise_md::inline::plain_text(&c, &nodes);
                    assert!(!plain.is_empty());
                }
            });
        }
        drop(d);
    }
}
