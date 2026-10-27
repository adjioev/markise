//! Export (sanitizer + rendering) must stay linear on adversarial inputs (review pass 2 #4).

use markise_export::{HtmlOptions, render_document, sanitize::sanitize};

type Gen = fn(usize) -> String;

/// Sanitizer-specific pathological inputs.
fn sanitizer_cases() -> Vec<(&'static str, Gen)> {
    vec![
        ("entity-ampersands", |n| {
            format!("<div title=\"{}\">x</div>", "&".repeat(n * 8))
        }),
        ("many-skipped-scripts", |n| {
            format!("<div>{}</div>", "<script></script>".repeat(n))
        }),
        ("unclosed-script", |n| format!("<script>{}", "<b>x</b>".repeat(n))),
        ("unterminated-quotes", |n| "<a title=\"x ".repeat(n)),
        ("tag-soup", |n| "<a b <a b ".repeat(n)),
        ("comment-openers", |n| "x <!-- y ".repeat(n)),
    ]
}

/// One test (not two) so the timings are not disturbed by a parallel test thread.
#[test]
fn sanitizer_and_export_scale_linearly() {
    for (name, f) in sanitizer_cases() {
        markise_corpus::assert_scales_linearly(name, f, &|s| {
            sanitize(s);
        });
    }
    let export = |s: &str| {
        render_document(&markise_md::parse(s), &HtmlOptions::default());
    };
    for (name, f) in markise_corpus::adversarial().into_iter().chain(sanitizer_cases()) {
        markise_corpus::assert_scales_linearly(name, f, &export);
    }
}
