//! Wikilink recognition (spec 06 V2.1, test tables §16.2 / §16.7) and round-trip (V2.9).

use markise_md::inline::{InlineContext, InlineKind, parse_inlines, plain_text};
use markise_md::*;

fn on() -> ParseOptions {
    ParseOptions::default()
}

fn off() -> ParseOptions {
    ParseOptions {
        wikilinks: false,
        ..Default::default()
    }
}

/// Inline nodes of `s` as compact strings: `wiki(target|subpath|display)` for wikilinks, the
/// kind name otherwise (text with its source).
fn dump(s: &str, o: &ParseOptions) -> Vec<String> {
    let nodes = parse_inlines(s, o, &InlineContext::default());
    let mut out = Vec::new();
    fn go(s: &str, n: &[markise_md::inline::Inline], out: &mut Vec<String>) {
        for x in n {
            out.push(match &x.kind {
                InlineKind::Text => format!("text:{}", x.span.slice(s)),
                InlineKind::WikiLink {
                    target,
                    subpath,
                    display,
                    inner,
                } => {
                    assert_eq!(inner.start, x.span.start + 2);
                    assert_eq!(inner.end + 2, x.span.end);
                    format!(
                        "wiki({}|{}|{})",
                        target,
                        subpath.as_deref().unwrap_or("-"),
                        display.map_or("-".into(), |d| d.slice(s).to_string())
                    )
                }
                k => format!("{k:?}").split([' ', '{', '(']).next().unwrap().to_lowercase(),
            });
            go(s, &x.children, out);
        }
    }
    go(s, &nodes, &mut out);
    out
}

fn wikis(s: &str) -> Vec<String> {
    dump(s, &on()).into_iter().filter(|x| x.starts_with("wiki(")).collect()
}

#[test]
fn forms_are_recognised() {
    assert_eq!(
        dump("see [[Note]] now", &on()),
        ["text:see ", "wiki(Note|-|-)", "text: now"]
    );
    assert_eq!(wikis("[[Note.md]]"), ["wiki(Note.md|-|-)"]);
    assert_eq!(wikis("[[folder/Note]]"), ["wiki(folder/Note|-|-)"]);
    assert_eq!(wikis("[[Note|alias]]"), ["wiki(Note|-|alias)"]);
    assert_eq!(wikis("[[Note#Heading]]"), ["wiki(Note|Heading|-)"]);
    assert_eq!(wikis("[[Note#Heading|alias]]"), ["wiki(Note|Heading|alias)"]);
    assert_eq!(wikis("[[#Heading]]"), ["wiki(|Heading|-)"]);
    assert_eq!(wikis("[[Spec.pdf]]"), ["wiki(Spec.pdf|-|-)"]);
    assert_eq!(wikis("a [[x]] b [[y]]"), ["wiki(x|-|-)", "wiki(y|-|-)"]);
}

#[test]
fn l24_to_l31_syntax_cases() {
    // L24: whitespace trimmed for resolution, source unchanged
    assert_eq!(wikis("[[  Plan  ]]"), ["wiki(Plan|-|-)"]);
    // L25, L27
    assert_eq!(wikis("[[Ideas|My ideas]]"), ["wiki(Ideas|-|My ideas)"]);
    assert_eq!(wikis("[[Ideas|a|b]]"), ["wiki(Ideas|-|a|b)"]);
    // L26 outside a table: the backslash before the pipe is dropped from the target
    assert_eq!(wikis("[[Ideas\\|My ideas]]"), ["wiki(Ideas|-|My ideas)"]);
    // L19: `#` splits off the subpath
    assert_eq!(wikis("[[Hash#Tag]]"), ["wiki(Hash|Tag|-)"]);
    // L28: not inside a code span
    assert!(wikis("`[[Ideas]]`").is_empty());
    // L29: escaped opening bracket
    assert!(wikis("\\[[Ideas]]").is_empty());
    // L31: empty content
    assert!(wikis("[[]]").is_empty());
    assert!(wikis("[[ ]]").is_empty());
}

#[test]
fn edge_cases() {
    // the first `]]` on the line closes
    assert_eq!(dump("[[a]]]", &on()), ["wiki(a|-|-)", "text:]"]);
    // content with `[[` is not a link; the inner one is
    assert_eq!(wikis("[[a [[b]]"), ["wiki(b|-|-)"]);
    // no closer on the line
    assert!(wikis("[[a\nb]]").is_empty());
    assert!(wikis("[[open").is_empty());
    // a code span starting inside and ending after `]]` wins (V2.1.3)
    assert!(wikis("[[a `b]] c`").is_empty());
    // a code span entirely inside the content does not prevent the link
    assert_eq!(wikis("[[a `b` c]]"), ["wiki(a `b` c|-|-)"]);
    // emphasis does not reach into the link
    assert_eq!(wikis("*[[a*b]]*"), ["wiki(a*b|-|-)"]);
    // unicode, spaces, case
    assert_eq!(wikis("[[Café au lait]]"), ["wiki(Café au lait|-|-)"]);
    assert_eq!(wikis("[[日本語のノート]]"), ["wiki(日本語のノート|-|-)"]);
    assert_eq!(wikis("[[Report v1.8]]"), ["wiki(Report v1.8|-|-)"]);
    // inside link text
    assert_eq!(wikis("[see [[N]]](http://x)"), ["wiki(N|-|-)"]);
    // math wins over a link inside it
    assert!(wikis("$a [[b]] c$").is_empty());
}

#[test]
fn embeds_are_plain_text_for_now() {
    assert_eq!(dump("![[Pic.png]]", &on()), ["text:![[Pic.png]]"]);
    assert_eq!(dump("x ![[Note#H|*a*]] y", &on()), ["text:x ![[Note#H|*a*]] y"]);
}

#[test]
fn toggle_off_keeps_brackets_as_text() {
    let o = off();
    assert!(dump("[[Note]]", &o).iter().all(|k| !k.starts_with("wiki(")));
    let nodes = parse_inlines("[[Note|x]]", &o, &InlineContext::default());
    assert_eq!(plain_text("[[Note|x]]", &nodes), "[[Note|x]]");
}

#[test]
fn plain_text_uses_the_label() {
    let s = "a [[Note#H]] b [[X|shown]]";
    let nodes = parse_inlines(s, &on(), &InlineContext::default());
    assert_eq!(plain_text(s, &nodes), "a Note › H b shown");
}

#[test]
fn adversarial_runs_stay_fast() {
    // many openers without closers: linear thanks to the close-search memo
    let s = "[[".repeat(50_000);
    let t = std::time::Instant::now();
    assert!(wikis(&s).is_empty());
    let s2 = format!("{}]]", "[[a".repeat(30_000));
    assert_eq!(wikis(&s2).len(), 1);
    // nested openers and closers, and many links on one line
    let s3 = format!("{}{}", "[".repeat(40_000), "]".repeat(40_000));
    let _ = wikis(&s3);
    assert_eq!(wikis(&"[[a]] ".repeat(20_000)).len(), 20_000);
    assert!(t.elapsed().as_secs_f64() < 2.0, "{:?}", t.elapsed());
}

#[test]
fn round_trip_is_byte_identical_either_way() {
    let texts = [
        "# T [[Note]]\n\n- [[a|b]] and [[ c ]]\n- ![[img.png|300]]\n\n| x | y |\n|---|---|\n| [[Ideas\\|My ideas]] | 2 |\n",
        "[[_TOC_]]\n\n[[Hash#Tag]] `[[code]]` \\[[esc]]\n",
        "[[unclosed\n[[]] [[a]]]\r\nend [[Café]]",
    ];
    for t in texts {
        for o in [on(), off()] {
            let d = Document::parse(t, o.clone());
            assert_eq!(d.serialize(), t);
            d.validate().unwrap();
        }
    }
}

#[test]
fn l26_table_cell_keeps_its_columns() {
    let t = "| a | b |\n|---|---|\n| [[Ideas\\|My ideas]] | 2 |\n";
    let d = Document::parse(t, on());
    let tree = debug_tree(&d);
    assert!(tree.starts_with("table2"), "{tree}");
}

/// Owner decision (2026-10-28): `[[…]]` directly followed by `(` is not a wikilink; it stays
/// a standard link whose text is `[1]` (Typora / Pandoc citation style).
#[test]
fn brackets_followed_by_a_paren_stay_a_standard_link() {
    for o in [on(), off()] {
        let s = "see [[1]](#ref-1) and [[2]](https://x.org)";
        let nodes = parse_inlines(s, &o, &InlineContext::default());
        let links: Vec<(String, String)> = nodes
            .iter()
            .filter_map(|n| match &n.kind {
                InlineKind::Link { dest, .. } => Some((dest.clone(), plain_text(s, &n.children))),
                _ => None,
            })
            .collect();
        assert_eq!(
            links,
            [
                ("#ref-1".to_string(), "[1]".to_string()),
                ("https://x.org".into(), "[2]".into())
            ]
        );
        assert!(dump(s, &o).iter().all(|k| !k.starts_with("wiki(")));
    }
    // a space before the paren: still a wikilink
    assert_eq!(wikis("[[1]] (x)"), ["wiki(1|-|-)"]);
    // the embed form likewise falls back to the image rules
    assert!(dump("![[a]](b.png)", &on()).iter().any(|k| k == "image"));
    let t = "[[1]](#ref-1)\n";
    assert_eq!(Document::parse(t, on()).serialize(), t);
}
