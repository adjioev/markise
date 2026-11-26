//! Tags (spec 06 §10, test table §16.5 G1–G16, ADR 0027): what is and is not a tag, the
//! heading rule, front matter, the neutral view and byte-exact round-trip.

use markise_md::inline::{InlineContext, InlineKind, parse_inlines};
use markise_md::*;

fn on() -> ParseOptions {
    ParseOptions::default()
}

fn off() -> ParseOptions {
    ParseOptions {
        tags: false,
        ..Default::default()
    }
}

/// The tags of inline content `s`, as written (with `#`).
fn tags(s: &str, o: &ParseOptions) -> Vec<String> {
    let nodes = parse_inlines(s, o, &InlineContext::default());
    let mut out = Vec::new();
    fn go(s: &str, n: &[markise_md::inline::Inline], out: &mut Vec<String>) {
        for x in n {
            if x.kind == InlineKind::Tag {
                out.push(x.span.slice(s).to_string());
            }
            go(s, &x.children, out);
        }
    }
    go(s, &nodes, &mut out);
    out
}

/// The tags of a whole document, as the folder index reads them.
fn doc_tags(src: &str) -> Vec<String> {
    tags::document_tags(&Document::parse(src, on()))
}

#[test]
fn g_table_inline() {
    assert_eq!(tags("see #idea today", &on()), ["#idea"]); // G1
    assert!(tags("#1984", &on()).is_empty()); // G3
    assert_eq!(tags("#y1984", &on()), ["#y1984"]); // G4
    assert_eq!(tags("#area/health/sleep", &on()), ["#area/health/sleep"]); // G5
    assert_eq!(tags("#tag. #tag, (#tag)", &on()), ["#tag", "#tag", "#tag"]); // G6
    assert_eq!(
        tags("#привет #日本語 #🚀ship", &on()),
        ["#привет", "#日本語", "#🚀ship"]
    ); // G7
    assert!(tags("a#b", &on()).is_empty()); // G8
    assert!(tags("`#code`", &on()).is_empty()); // G11
    assert!(tags("https://x.com/#frag", &on()).is_empty()); // G12
    assert!(tags("<https://x.com/#frag> [a](#frag) [#x](u)", &on()).is_empty());
    assert_eq!(tags("#tag/", &on()), ["#tag"]); // G16
    assert_eq!(tags("#a//b", &on()), ["#a"]);
    // a digit is enough next to another character (V10.1)
    assert_eq!(tags("#2024/01", &on()), ["#2024/01"]);
}

#[test]
fn not_tags() {
    for s in [
        "#",
        "# x",
        "##x",
        "#123",
        "x#y",
        "$#x$",
        "\\#x",
        "&#35;x",
        "<span>#x</span>",
        "[[Note#x]]",
        "[[#x]]",
        "{>> #x <<}",
        "a.#x",
    ] {
        assert!(tags(s, &on()).is_empty(), "{s}: {:?}", tags(s, &on()));
    }
    assert!(tags("see #idea", &off()).is_empty());
}

#[test]
fn tags_across_text_runs_and_inside_emphasis() {
    assert_eq!(tags("#snake_case and #a-b", &on()), ["#snake_case", "#a-b"]);
    assert_eq!(tags("*see #x* **#y**", &on()), ["#x", "#y"]);
    assert_eq!(tags("line\n#next", &on()), ["#next"]);
    assert_eq!(tags("#a #b", &on()), ["#a", "#b"]);
}

/// G9/G10, spec 06 V1.6 b: `# Heading` is a heading; `#heading-like` is a paragraph with a tag
/// (also in the default, non-strict mode); with tags off it stays a heading as before.
#[test]
fn heading_rule() {
    let kinds = |s: &str, o: ParseOptions| -> Vec<String> {
        let d = Document::parse(s, o);
        d.tops()
            .map(|t| {
                format!("{:?}", t.block.kind)
                    .split([' ', '{'])
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(kinds("# Heading\n", on()), ["Heading"]);
    assert_eq!(kinds("#heading-like text\n", on()), ["Paragraph"]);
    assert_eq!(kinds("#heading-like text\n", off()), ["Heading"]);
    assert_eq!(kinds("##Two\n", on()), ["Heading"]);
    assert_eq!(kinds("#1 first\n", on()), ["Heading"]);
    assert_eq!(doc_tags("# Heading\n\n#heading-like text\n"), ["heading-like"]);
}

#[test]
fn g_table_documents() {
    // G13, G14, G15
    assert_eq!(doc_tags("---\ntags: [a, b/c]\n---\nx\n"), ["a", "b/c"]);
    assert_eq!(doc_tags("---\ntags:\n  - a\n  - \"#b\"\n---\nx\n"), ["a", "b"]);
    assert!(doc_tags("---\ntags: [has space]\n---\nx\n").is_empty());
    // the singular key and comma forms; front matter first, then the text; each once
    assert_eq!(
        doc_tags("---\ntag: one, Two\n---\n\n#two and #three in a #one paragraph\n"),
        ["one", "Two", "three"]
    );
    // code blocks, math, HTML blocks and the front matter's other values hold none
    assert!(doc_tags("```\n#x\n```\n\n    #y\n\n$$\n#z\n$$\n\n<div>\n#w\n</div>\n").is_empty());
    assert!(doc_tags("---\ntitle: \"#nope\"\n---\n").is_empty());
    // lists, quotes, headings and tables
    assert_eq!(
        doc_tags("- item #a\n\n> quote #b\n\n## Head #c\n\n| h #d |\n|---|\n| #e |\n"),
        ["a", "b", "c", "d", "e"]
    );
    // tags off: front matter still counts, the text does not
    let d = Document::parse("---\ntags: [a]\n---\n#b\n", off());
    assert_eq!(tags::document_tags(&d), ["a"]);
}

/// The neutral view: a tag is one node whose child shows the source byte for byte.
#[test]
fn tag_in_the_neutral_view() {
    use markise_doc::InlineKind as NK;
    let d = Document::parse("see #a/b now\n", on());
    let leaf = markise_doc::FormatDoc::leaf(&d, 0).unwrap();
    let rich = markise_doc::FormatDoc::inlines(&d, &leaf);
    let tag = rich.nodes.iter().find(|n| matches!(n.kind, NK::Tag { .. })).unwrap();
    assert_eq!(tag.kind, NK::Tag { name: "a/b".into() });
    assert_eq!(tag.source, 4..8);
    assert!(tag.markers.is_empty());
    assert_eq!(rich.value(&tag.children[0]), "#a/b");
    assert_eq!(tag.children[0].source, 4..8);
}

/// Tags never change the file: every form round-trips byte for byte, also after an edit
/// elsewhere.
#[test]
fn tags_round_trip() {
    let src = "---\ntags: [a, b]\n---\n\n#idea at the start\n\nText #x/y, (#z) and #🚀ship.\n\n- [ ] task #todo\n\n| #t |\n|---|\n| #u |\n";
    let mut d = Document::parse(src, on());
    assert_eq!(d.serialize(), src);
    let at = src.find("Text").unwrap();
    d.edit(at..at, "More ");
    assert_eq!(d.serialize(), src.replacen("Text", "More Text", 1));
}

/// Review of #270: table cells are read one by one, as the editor renders them — a tag right
/// after a pipe counts, inline Markdown never spans cells, `\|` is a literal pipe.
#[test]
fn table_cells_are_read_as_rendered() {
    assert_eq!(doc_tags("|#idea|\n|---|\n|text|\n"), ["idea"]);
    // the row splitter keeps a code span whole (one cell of code), as the editor shows it
    assert!(doc_tags("| `a | #b` |\n|---|\n| x |\n").is_empty());
    assert_eq!(doc_tags("| [x | #b](u) |\n|---|---|\n| x | y |\n"), ["b"]);
    assert_eq!(doc_tags("| a \\| #c |\n|---|\n| `#code` |\n"), ["c"]);
    // a cell beyond the header's columns is not shown, so not read
    assert_eq!(doc_tags("| h |\n|---|\n| #shown | #extra |\n"), ["shown"]);
    // what the editor shows: the same tags in the neutral view of each cell
    let d = Document::parse("|#idea|\n|---|\n|text|\n", on());
    let cell = table::cell_spans(d.text(), &d.tree()[0])[0][0];
    let rich = markise_doc::FormatDoc::inline(
        &d,
        markise_doc::InlineSource::Cell { span: cell, cap: 4096 },
        cell.start,
    );
    assert!(
        rich.nodes
            .iter()
            .any(|n| n.kind == markise_doc::InlineKind::Tag { name: "idea".into() })
    );
}

/// Review of #270 (ADR 0027): the text inside an inline HTML element is not a tag, whatever
/// the white space around it; void elements, comments and closed elements end nothing.
#[test]
fn no_tags_inside_inline_html_elements() {
    assert_eq!(tags("Text <span> #hidden </span> and #shown", &on()), ["#shown"]);
    assert!(tags("<b>#x</b>", &on()).is_empty());
    assert!(tags("<span class=\"a\">\n #x\n</span>", &on()).is_empty());
    assert!(tags("<i>a <b>b</b> #x</i>", &on()).is_empty());
    assert_eq!(tags("a<br> #y", &on()), ["#y"]);
    assert_eq!(tags("<img src=x/> #y", &on()), ["#y"]);
    assert_eq!(tags("<!-- c --> #z", &on()), ["#z"]);
    assert_eq!(tags("</span> #w", &on()), ["#w"]);
    assert_eq!(doc_tags("Text <span> #hidden </span> #shown\n"), ["shown"]);
}

/// Review of #270: many distinct tags in one note are collected in linear time and capped.
#[test]
fn many_tags_are_capped_and_linear() {
    let mut src = String::new();
    for i in 0..20_000 {
        src.push_str(&format!("#t{i} "));
    }
    src.push('\n');
    let t = std::time::Instant::now();
    let got = doc_tags(&src);
    assert_eq!(got.len(), tags::MAX_NOTE_TAGS);
    assert_eq!(got[0], "t0");
    assert_eq!(got[tags::MAX_NOTE_TAGS - 1], format!("t{}", tags::MAX_NOTE_TAGS - 1));
    // a hang guard, not a benchmark (quadratic deduplication took minutes here)
    assert!(t.elapsed() < std::time::Duration::from_secs(60));
}
