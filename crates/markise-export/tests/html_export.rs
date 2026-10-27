//! Snapshot tests of the two HTML exports (spec 04 §8.1): styled (theme CSS generated from the
//! native theme, highlight classes) and "without styles" (bare semantic HTML).
//!
//! Update the snapshots with `MARKISE_UPDATE_SNAPSHOTS=1 cargo test -p markise-export --test
//! html_export` and review the diff.

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
