//! Untrusted export must never emit active content, whatever the input (review P1 #4/#5).

use markise_export::{HtmlOptions, render_body};
use proptest::prelude::*;

fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const FRAGS: &[&str] = &[
    "<script>",
    "</script>",
    "<SCRIPT >",
    "alert(1)",
    "<img src=x onerror=alert(1)>",
    "<a href=\"javascript:x\">",
    "[l](javascript:x)",
    "![i](jav&#x61;script:x)",
    "<javascript:x>",
    "```",
    "x'onmouseover='y",
    "\"",
    "'",
    "<",
    ">",
    "\n",
    "\n\n",
    "<div onclick=x>",
    "</div>",
    "<style>",
    "<iframe src=//e>",
    "<svg onload=x>",
    "<!--",
    "-->",
    "| a | b |\n|---|---|\n",
    "# h",
    "- ",
    "> ",
    "[^1]",
    "[^1]: n",
    "&#106;",
    "<a href=jAvAsCrIpT:x>",
    "<b style=\"x:expression(y)\">",
    "text",
    "<math><mi>",
    "<form action=x>",
    "$x$",
    "$\\frac{a}{<b>}$",
    "$\\text{<script>}$",
    "$$\n",
    "\\[\n",
    "$\\foo{\"onload=x}$",
];

/// Lower-cased tags of the output.
fn tags(html: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        let t = &rest[i..];
        let e = t.find('>').unwrap_or(t.len());
        v.push(t[..e].to_ascii_lowercase());
        rest = &t[e.min(t.len())..];
        if rest.is_empty() {
            break;
        }
    }
    v
}

/// The tag with quoted attribute values blanked (text inside a value is inert).
fn unquoted(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut quote: Option<char> = None;
    for c in tag.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                out.push_str("\"\"");
            }
            None => out.push(c),
        }
    }
    out
}

/// The value of the tag's `style` attribute (empty without one; unquoted values to the next space).
fn style_value(tag: &str) -> &str {
    let Some(i) = tag.find(" style=") else { return "" };
    let v = &tag[i + 7..];
    match v.chars().next() {
        Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
        _ => v.split_whitespace().next().unwrap_or(""),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(512), .. ProptestConfig::default() })]

    #[test]
    fn prop_untrusted_export_is_inert(parts in prop::collection::vec(prop::sample::select(FRAGS), 0..30)) {
        let md = parts.concat();
        let html = render_body(&markise_md::parse(&md), &HtmlOptions::default());
        let all = tags(&html);
        for (i, t) in all.iter().enumerate() {
            prop_assert!(!t.starts_with("<script") && !t.starts_with("<style"), "tag {t}");
            // the only SVG is rendered math: generated right inside an `mjx-container`
            let math_svg = t.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"")
                && i > 0
                && all[i - 1].starts_with("<mjx-container");
            prop_assert!(!t.starts_with("<svg") || math_svg, "tag {t}");
            prop_assert!(!t.starts_with("<math") && !t.starts_with("<form"), "tag {t}");
            // attributes: no event handlers, no script URLs
            for attr in unquoted(t).split_whitespace().skip(1) {
                prop_assert!(!attr.starts_with("on"), "handler in {t}");
            }
            for a in ["href=\"", "src=\"", "href='", "src='", "poster=\"", "cite=\""] {
                if let Some(i) = t.find(a) {
                    let v = &t[i + a.len()..];
                    prop_assert!(!v.trim_start().starts_with("javascript") && !v.starts_with("vbscript"), "script url in {t}");
                }
            }
            prop_assert!(!style_value(t).contains("expression("), "css expression in {t}");
            if t.starts_with("<iframe") {
                prop_assert!(t.contains("sandbox"), "{t}");
            }
        }
    }
}
