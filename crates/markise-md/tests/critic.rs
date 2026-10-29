//! CriticMarkup review marks (ADR 0012, issue #69): recognition, where they are not
//! recognised (code, math, across blocks), the neutral inline tree and byte-identical
//! round-trip with the extension on and off.
//!
//! The syntax cases follow the public CriticMarkup description (criticmarkup.com) in our own
//! words: five marks, no nesting, inline content parsed inside, no escapes of their own.

use markise_doc::{FormatDoc, InlineKind as NK, RichInline};
use markise_md::inline::{CriticKind, InlineContext, InlineKind, parse_inlines, plain_text};
use markise_md::*;

fn on() -> ParseOptions {
    ParseOptions::default()
}

fn off() -> ParseOptions {
    ParseOptions {
        critic_markup: false,
        ..Default::default()
    }
}

/// Inline nodes of `s` as compact strings: `insert(content)` for marks, `text:…` for text,
/// the kind name otherwise; children follow their parent.
fn dump(s: &str, o: &ParseOptions) -> Vec<String> {
    let nodes = parse_inlines(s, o, &InlineContext::default());
    let mut out = Vec::new();
    fn go(s: &str, n: &[markise_md::inline::Inline], out: &mut Vec<String>) {
        for x in n {
            out.push(match &x.kind {
                InlineKind::Text => format!("text:{}", x.span.slice(s)),
                InlineKind::Critic { kind, content } => format!("{}({})", kind.name(), content.slice(s)),
                k => format!("{k:?}").split([' ', '{', '(']).next().unwrap().to_lowercase(),
            });
            go(s, &x.children, out);
        }
    }
    go(s, &nodes, &mut out);
    out
}

fn marks(s: &str) -> Vec<String> {
    dump(s, &on())
        .into_iter()
        .filter(|x| !x.starts_with("text:") && x.contains('('))
        .collect()
}

#[test]
fn the_five_marks_are_recognised() {
    assert_eq!(
        dump("Lorem{++ sit++} amet", &on()),
        ["text:Lorem", "insert( sit)", "text: sit", "text: amet"]
    );
    assert_eq!(
        dump("Lorem{-- ipsum--} dolor", &on()),
        ["text:Lorem", "delete( ipsum)", "text: ipsum", "text: dolor"]
    );
    assert_eq!(
        dump("Lorem {~~hipsum~>ipsum~~} dolor", &on()),
        [
            "text:Lorem ",
            "substitute(hipsum~>ipsum)",
            "old(hipsum)",
            "text:hipsum",
            "new(ipsum)",
            "text:ipsum",
            "text: dolor"
        ]
    );
    assert_eq!(dump("Done.{>>a note<<}", &on()), ["text:Done.", "comment(a note)"]);
    assert_eq!(
        marks("A {==passage==}{>>unclear<<} here"),
        ["highlight(passage)", "comment(unclear)"]
    );
}

#[test]
fn off_everything_stays_text() {
    for s in ["a {++b++} c", "a {--b--} c", "a {~~b~>c~~} d", "{==x==}{>>y<<}"] {
        let d = dump(s, &off());
        assert!(!d.iter().any(|x| x.contains('(')), "{s}: {d:?}");
    }
}

#[test]
fn markdown_inside_marks_is_parsed() {
    assert_eq!(
        dump("{++**bold** and *em*++}", &on()),
        [
            "insert(**bold** and *em*)",
            "strong",
            "text:bold",
            "text: and ",
            "emph",
            "text:em"
        ]
    );
    // a link and inline code inside a deletion
    let d = dump("{--see [x](u) or `c`--}", &on());
    assert_eq!(d[0], "delete(see [x](u) or `c`)");
    assert!(
        d.contains(&"link".to_string()) && d.contains(&"code".to_string()),
        "{d:?}"
    );
    // emphasis around a mark wraps it
    assert_eq!(
        dump("*a {++b++} c*", &on()),
        ["emph", "text:a ", "insert(b)", "text:b", "text: c"]
    );
}

#[test]
fn marks_do_not_nest() {
    // the first closer ends the mark; the inner opener is text
    assert_eq!(
        dump("{++a {--b--} c++}", &on()),
        ["insert(a {--b--} c)", "text:a {--b--} c"]
    );
}

#[test]
fn empty_and_unclosed_marks() {
    assert_eq!(marks("x{>><<}y"), ["comment()"]);
    assert_eq!(marks("{++++}"), ["insert()"]);
    // unclosed: text
    assert!(marks("a {++b c").is_empty());
    assert!(marks("a {>>b c").is_empty());
    // a closer of another kind does not close
    assert!(marks("a {++b--} c").is_empty());
    // a substitution without `~>` is text
    assert!(marks("a {~~b~~} c").is_empty());
    // escaped opener
    assert!(marks("a \\{++b++} c").is_empty());
    // `{` alone and other braces
    assert!(marks("{} {+ +} {a} {: toc}").is_empty());
}

#[test]
fn code_spans_and_math_win() {
    // a mark inside a code span is code
    assert!(marks("`{++x++}`").is_empty());
    // a code span opened inside the mark and closed after it wins over the mark
    assert!(marks("{++a `b++} c`").is_empty());
    // a code span fully inside is code inside the mark
    assert_eq!(marks("{++a `b` c++}"), ["insert(a `b` c)"]);
    // inline math that starts first wins; math inside a mark is math
    assert!(marks("$a {++b++} c$").is_empty());
    let d = dump("{++$x^2$++}", &on());
    assert_eq!(d[0], "insert($x^2$)");
    assert_eq!(d[1], "math");
}

#[test]
fn code_blocks_and_math_blocks_are_not_scanned() {
    let doc = Document::parse("```\n{++x++}\n```\n\n$$\n{--y--}\n$$\n", on());
    let mut found = false;
    for l in leaves(&doc) {
        found |= tree_of(&doc, &l).contains("critic");
    }
    assert!(!found);
}

#[test]
fn marks_never_span_blocks() {
    // CriticMarkup allows paragraph-long marks across a blank line; in v1 a mark stays inside
    // its block: both halves are text (ADR 0012)
    let doc = Document::parse("A{++\n\n++}B\n", on());
    for l in leaves(&doc) {
        assert!(!tree_of(&doc, &l).contains("critic"), "{}", tree_of(&doc, &l));
    }
    // within one paragraph a mark may cross a soft line break
    let doc = Document::parse("one {++two\nthree++} four\n", on());
    let l = &leaves(&doc)[0];
    assert!(tree_of(&doc, l).contains("critic.insert"), "{}", tree_of(&doc, l));
}

#[test]
fn marks_in_headings_lists_quotes_and_tables() {
    for src in [
        "# Title {++new++}\n",
        "- item {--old--}\n",
        "> quoted {~~a~>b~~}\n",
        "| a | b |\n|---|---|\n| {==x==} | y |\n",
    ] {
        let doc = Document::parse(src, on());
        assert_eq!(doc.serialize(), src);
        let mut any = false;
        for l in leaves(&doc) {
            if let markise_doc::LeafKind::Table { .. } = l.kind {
                for line in &l.lines {
                    for cell in doc.table_row_cells(*line) {
                        let r = doc.inline(markise_doc::InlineSource::Cell { span: cell, cap: 4096 }, cell.start);
                        any |= r.debug().contains("critic");
                    }
                }
            } else {
                any |= tree_of(&doc, &l).contains("critic");
            }
        }
        assert!(any, "{src}");
    }
}

#[test]
fn neutral_tree_markers_and_values() {
    let doc = Document::parse("a {~~old~>new~~} b{>>why<<}\n", on());
    let l = &leaves(&doc)[0];
    let r = doc.inlines(l);
    let sub = r
        .nodes
        .iter()
        .find(|n| n.kind == NK::Critic(CriticKind::Substitute))
        .unwrap();
    // `{~~`, `~>`, `~~}`
    assert_eq!(sub.markers, vec![2..5, 8..10, 13..16]);
    assert_eq!(sub.content, 5..13);
    assert_eq!(sub.children.len(), 2);
    assert_eq!(sub.children[0].kind, NK::Critic(CriticKind::Old));
    assert_eq!(sub.children[0].source, 5..8);
    assert!(sub.children[0].markers.is_empty());
    assert_eq!(sub.children[1].kind, NK::Critic(CriticKind::New));
    assert_eq!(sub.children[1].source, 10..13);
    let c = r
        .nodes
        .iter()
        .find(|n| n.kind == NK::Critic(CriticKind::Comment))
        .unwrap();
    assert_eq!(c.markers, vec![18..21, 24..27]);
    assert_eq!(r.value(&c.children[0]), "why");
    assert_eq!(
        r.debug(),
        "text[0..2] critic.substitute[2..16 m:2..5,8..10,13..16](critic.old[5..8](text[5..8]) \
         critic.new[10..13](text[10..13])) text[16..18] critic.comment[18..27 m:18..21,24..27](text[21..24])"
    );
}

#[test]
fn quote_prefixes_map_to_source() {
    // the content string joins lines without the `> ` prefixes; marks map back to the source
    let src = "> one {++two\n> three++} four\n";
    let doc = Document::parse(src, on());
    let l = &leaves(&doc)[0];
    let r = doc.inlines(l);
    let base = l.content().start;
    let ins = find(&r, CriticKind::Insert).unwrap();
    let a = base + ins.source.start as usize;
    let b = base + ins.source.end as usize;
    assert_eq!(&src[a..b], "{++two\n> three++}");
}

#[test]
fn plain_text_reads_the_original() {
    let s = "Keep {--this--}{++that++} {~~old~>new~~}{>>c<<} {==x==}";
    let nodes = parse_inlines(s, &on(), &InlineContext::default());
    assert_eq!(plain_text(s, &nodes), "Keep this old x");
}

#[test]
fn round_trip_is_byte_identical_on_and_off() {
    let docs = [
        "Lorem{++ sit++} amet {--x--} {~~a~>b~~} {==h==}{>>@ai: why?<<}\n",
        "# H {++x\n\nA{++\n\n++}B unclosed {++ here\n",
        "- {>>one<<}\n- {--two--}\n\n> {==q==}\n\n| {++a++} | b |\n|---|---|\n",
        "```\n{++code++}\n```\n\n`{--span--}` $a {++b++}$\r\nCRLF {++x++}\r\n",
        "",
        "{++",
        "{++}++}--}~~}==}<<}{>>{==",
    ];
    for src in docs {
        for o in [on(), off()] {
            let d = Document::parse(src, o.clone());
            assert_eq!(d.serialize(), src, "{src:?} {}", o.critic_markup);
        }
    }
}

/// Review of linearity (issue #69): CriticMarkup searches scan O(n) bytes on adversarial
/// inputs — counted through the public parse, compared for 1× and 16× inputs (not timed).
#[test]
fn adversarial_inputs_parse_in_linear_time() {
    type Gen = fn(usize) -> String;
    let cases: [(&str, Gen); 9] = [
        ("openers without closers", |n| "{++".repeat(n)),
        ("all openers", |n| "{++{--{~~{=={>>".repeat(n)),
        ("openers then one closer", |n| format!("{}++}}", "{++".repeat(n))),
        ("substitutions without arrows", |n| format!("{}~~}}", "{~~a".repeat(n))),
        ("many marks", |n| "{++a++} {--b--} {~~c~>d~~} ".repeat(n)),
        ("code crossing", |n| format!("{}`x`", "{++`a++} ".repeat(n))),
        ("ticks inside", |n| format!("{}++}}", "{++` ".repeat(n))),
        ("closers only", |n| "++}--}~~}==}<<}".repeat(n)),
        ("comments and highlights", |n| "{==x==}{>>y<<}".repeat(n)),
    ];
    for (name, f) in cases {
        let (small, big) = (scanned(&f(400)), scanned(&f(6400)));
        assert!(
            big <= 20 * small.max(1) + 64,
            "{name}: 16× input scanned {big} bytes vs {small}"
        );
    }
}

fn scanned(s: &str) -> usize {
    markise_md::inline::critic_scanned_reset();
    let _ = parse_inlines(s, &on(), &InlineContext::default());
    markise_md::inline::critic_scanned_reset()
}

// ---- helpers

fn leaves(doc: &Document) -> Vec<markise_doc::LeafView> {
    let mut out = Vec::new();
    doc.flatten(0..doc.top_count(), &mut |e| {
        if let markise_doc::FlatEvent::Leaf(l) = e
            && let Some(v) = doc.leaf(l.abs_span().start)
        {
            out.push(v);
        }
    });
    out
}

fn tree_of(doc: &Document, l: &markise_doc::LeafView) -> String {
    doc.inlines(l).debug()
}

fn find(r: &RichInline, k: CriticKind) -> Option<&markise_doc::InlineNode> {
    fn go(nodes: &[markise_doc::InlineNode], k: CriticKind) -> Option<&markise_doc::InlineNode> {
        for n in nodes {
            if n.kind == NK::Critic(k) {
                return Some(n);
            }
            if let Some(x) = go(&n.children, k) {
                return Some(x);
            }
        }
        None
    }
    go(&r.nodes, k)
}
