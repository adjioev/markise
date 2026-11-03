//! Highlighted code in HTML export (spec 03 §4.5 token classes).

use markise_export::{CodeHighlight, HtmlOptions, render_body, render_document};
use markise_md::parse;
use markise_theme::Theme;

fn body(md: &str, highlight: CodeHighlight) -> String {
    render_body(
        &parse(md),
        &HtmlOptions {
            highlight,
            ..Default::default()
        },
    )
}

#[test]
fn off_by_default_keeps_contract_markup() {
    let h = render_body(&parse("```rust\nfn a() {}\n```\n"), &HtmlOptions::default());
    assert!(
        h.contains("<code class='language-rust' lang='rust'>fn a() {}\n</code>"),
        "{h}"
    );
}

#[test]
fn classes_mode_wraps_tokens() {
    let h = body(
        "```rust\nfn main() { let s = \"x<y\"; } // c&d\n```\n",
        CodeHighlight::Classes,
    );
    assert!(h.contains("<code class='language-rust cm-s-inner' lang='rust'>"), "{h}");
    assert!(h.contains("<span class='cm-keyword'>fn</span>"), "{h}");
    assert!(h.contains("<span class='cm-def'>main</span>"), "{h}");
    assert!(h.contains("<span class='cm-comment'>// c&amp;d</span>"), "{h}");
    assert!(h.contains("x&lt;y"), "text escaped: {h}");
    assert!(!h.contains("style="), "no inline styles in classes mode");
    // the text content is unchanged
    let text = strip(&h);
    assert!(text.contains("fn main() { let s = \"x<y\"; } // c&d\n"), "{text}");
}

#[test]
fn inline_mode_uses_scheme_colors() {
    let theme = Theme::dark();
    let h = body("```python\ndef f(): pass\n```\n", CodeHighlight::inline_from(&theme));
    let [r, g, b, _] = theme.syntax.keyword.0;
    let kw = format!("<span class='cm-keyword' style='color:#{r:02x}{g:02x}{b:02x};'>def</span>");
    assert!(h.contains(&kw), "{h}");
}

#[test]
fn plain_languages_unchanged() {
    for md in ["```\nfn a\n```\n", "```nosuchlang\nfn a\n```\n", "    fn a\n"] {
        let h = body(md, CodeHighlight::Classes);
        assert!(!h.contains("<span"), "{md:?}: {h}");
        assert!(!h.contains("cm-s-inner"), "{md:?}: {h}");
    }
}

/// Hostile code text cannot escape the `<code>` element or inject attributes.
#[test]
fn hostile_code_stays_text() {
    let md = "```html\n<script>alert(1)</script><img src=x onerror='a'>\n</code></pre><b>x</b>\n```\n";
    let h = body(md, CodeHighlight::inline_from(&Theme::light()));
    assert!(!h.contains("<script"), "{h}");
    assert!(!h.contains("<img"), "{h}");
    assert!(!h.contains("</code></pre><b>"), "{h}");
    assert_eq!(h.matches("</code></pre>").count(), 1, "{h}");
    assert!(strip(&h).contains("<script>alert(1)</script>"));
}

#[test]
fn default_stylesheet_styles_tokens() {
    let html = render_document(
        &parse("```js\nlet a = 1;\n```\n"),
        &HtmlOptions {
            highlight: CodeHighlight::Classes,
            ..Default::default()
        },
    );
    assert!(html.contains(".cm-s-inner .cm-keyword"));
}

/// Text content of HTML (tags removed, entities decoded).
fn strip(h: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in h.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}
