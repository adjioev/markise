//! Snapshot tests of the two HTML exports (spec 04 §8.1): styled (theme CSS generated from the
//! native theme, highlight classes) and "without styles" (bare semantic HTML).
//!
//! Update the snapshots with `MARKISE_UPDATE_SNAPSHOTS=1 cargo test -p markise-export --test
//! it html_export::` and review the diff.

use std::path::PathBuf;

use markise_export::{CodeHighlight, HtmlOptions, css, render_document};
use markise_theme::Theme;

const DOC: &str = "---\ntitle: Snapshot\nauthor: Ann\nappend-head: <meta name='x' content='y'>\n---\n\
# Heading\n\nText with **bold**, `code` and a [link](https://example.org).\n\n\
> [!TIP]\n> Alert text.\n\n- [x] task\n- item\n\n| a | b |\n|:-:|--|\n| 1 | 2 |\n\n\
```rust\nfn main() { println!(\"hi\"); }\n```\n\n$$\nx^2\n$$\n";

fn check(name: &str, got: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name);
    if std::env::var_os("MARKISE_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}; run with MARKISE_UPDATE_SNAPSHOTS=1",
            path.display()
        )
    });
    if want != got {
        let diff = similar::TextDiff::from_lines(&want, got);
        panic!("snapshot {name} differs:\n{}", diff.unified_diff());
    }
}

/// A normal document with tables of contents (issue #29: the TOC is formatted once and reused;
/// the output must not change). Headings with inline markup, duplicates and an HTML anchor.
const TOC_DOC: &str = "[TOC]\n\n# Intro\n\nText.\n\n## Details *with* `code`\n\n<a name=\"usage\"></a>\n\n\
## Usage\n\n### Intro\n\n[toc]\n\n# Intro\n\n- item\n";

#[test]
fn toc_html_snapshot() {
    let doc = markise_md::parse(TOC_DOC);
    let html = render_document(
        &doc,
        &HtmlOptions {
            no_style: true,
            ..Default::default()
        },
    );
    check("toc.html", &html);
}

#[test]
fn styled_html_snapshot() {
    let doc = markise_md::parse(DOC);
    let html = render_document(
        &doc,
        &HtmlOptions {
            css: Some(css::theme_css(&Theme::light())),
            highlight: CodeHighlight::Classes,
            head_extra: Some("<meta name='x' content='y'>".into()),
            ..Default::default()
        },
    );
    check("styled.html", &html);
}

#[test]
fn plain_html_snapshot() {
    let doc = markise_md::parse(DOC);
    let html = render_document(
        &doc,
        &HtmlOptions {
            no_style: true,
            ..Default::default()
        },
    );
    assert!(!html.contains("<style"));
    check("plain.html", &html);
}

#[test]
fn dark_theme_css_differs() {
    assert_ne!(css::theme_css(&Theme::light()), css::theme_css(&Theme::dark()));
}

#[test]
fn common_html_export_keeps_native_elements_and_existing_comment_policy() {
    let src = "<details open>\n<summary>Hint</summary>\n\nPress <kbd>Enter</kbd>.\n\n</details>\n\n<!-- private -->\n";
    let doc = markise_md::parse(src);
    let html = render_document(&doc, &HtmlOptions::default());
    for raw in [
        "<details open>",
        "<summary>Hint</summary>",
        "<kbd>Enter</kbd>",
        "</details>",
    ] {
        assert!(html.contains(raw), "{raw}");
    }
    assert!(!html.contains("<!-- private -->"));
    let trusted = render_document(
        &doc,
        &HtmlOptions {
            trusted: true,
            ..Default::default()
        },
    );
    assert!(trusted.contains("<!-- private -->"));
    assert_eq!(doc.text(), src);
}

// ---- `:::` blocks (ADR 0018) ---------------------------------------------------------------

fn body(md: &str) -> String {
    let d = markise_md::parse(md);
    render_document(&d, &HtmlOptions::default())
}

#[test]
fn a_directive_exports_as_a_div_with_its_content_inside() {
    let h = body(":::note some args\n\n# Title\n\ntext\n\n:::\n\nafter\n");
    let open = h
        .find("<div class='md-directive md-directive-note' data-name='note' data-args='some args'>")
        .unwrap();
    let label = h.find("<p class='md-directive-label'>note some args</p>").unwrap();
    let title = h.find("<h1").unwrap();
    let close = h[title..].find("</div>").unwrap() + title;
    let after = h.find("<p>after</p>").unwrap();
    assert!(open < label && label < title && title < close && close < after, "{h}");
    assert!(!h.contains(":::"), "{h}");
}

#[test]
fn directives_nest_and_unpaired_markers_stay_text() {
    let h = body("::::a\n\n:::b\n\nx\n\n:::\n\n::::\n\n:::stray\n");
    let (a, b) = (h.find("data-name='a'").unwrap(), h.find("data-name='b'").unwrap());
    assert!(a < b);
    assert!(h.contains("<p>:::stray</p>"), "{h}");
}

#[test]
fn directive_names_and_args_are_escaped() {
    let h = body(":::note-1 '><script>alert(1)</script> & \"x\"\n\ntext\n\n:::\n");
    assert!(!h.contains("<script>alert"), "{h}");
    assert!(h.contains("data-args='&#39;&gt;&lt;script&gt;"), "{h}");
    // a name in any script keeps its text and gets a safe class
    let h = body(":::заметка\n\ntext\n\n:::\n");
    assert!(h.contains("md-directive--------"), "{h}");
    assert!(h.contains("data-name='заметка'"), "{h}");
}

#[test]
fn headings_inside_a_directive_keep_their_anchors_and_toc_entries() {
    let h = body("[toc]\n\n:::note\n\n# Inside\n\n:::\n");
    assert!(h.contains("#inside"), "{h}");
    assert!(h.contains("id='inside'") || h.contains("id=\"inside\""), "{h}");
}
