//! HTML export of diagram fences (spec 01 R3.5.6 / §4.6, spec 03 diagram row, ADR 0009).

use markise_export::{DiagramExport, HtmlOptions, render_body};
use markise_md::{Document, ParseOptions};
use markise_theme::Theme;

fn html(md: &str, diagrams: bool) -> String {
    let doc = Document::parse(
        md,
        ParseOptions {
            diagrams,
            ..ParseOptions::default()
        },
    );
    render_body(&doc, &HtmlOptions::default())
}

#[test]
fn r4_6_1_diagrams_render_as_inline_svg() {
    for (lang, src) in [
        ("mermaid", "graph TD\nA[Start] --> B[End]\n"),
        ("mermaid", "sequenceDiagram\nA->>B: hi\n"),
        ("sequence", "A->B: hi\n"),
        ("flow", "st=>start: S\ne=>end: E\nst->e\n"),
    ] {
        let out = html(&format!("```{lang}\n{src}```\n"), true);
        assert!(out.contains(&format!("<pre class='md-fences md-end-block md-diagram md-fences-advanced' lang='{lang}'><div class='md-diagram-panel'><div class='md-diagram-panel-preview'><svg ")), "{out}");
        assert!(out.contains("</svg></div></div></pre>"), "{out}");
    }
}

#[test]
fn r3_5_6_diagrams_off_stays_code() {
    let out = html("```mermaid\ngraph TD\nA-->B\n```\n", false);
    assert!(!out.contains("<svg"));
    assert!(out.contains("language-mermaid"), "{out}");
}

#[test]
fn unsupported_and_broken_diagrams_keep_source_with_notice() {
    let out = html("```mermaid\nclassDiagram\nA <|-- B\n```\n", true);
    assert!(out.contains("<div class='md-diagram-notice'>"), "{out}");
    assert!(out.contains("classDiagram\nA &lt;|-- B"), "{out}");
    let out = html("```mermaid\ngraph TD\nA-->\n```\n", true);
    assert!(out.contains("md-diagram-notice") && out.contains("line 2"), "{out}");
    // spec 02 §8: empty diagram placeholder
    let out = html("```mermaid\n```\n", true);
    assert!(out.contains("Empty Diagram"), "{out}");
}

#[test]
fn diagram_labels_cannot_inject_markup() {
    let out = html(
        "```mermaid\ngraph TD\nA[\"<script>alert(1)</script>\"] --> B[<img src=x onerror=alert(1)>]\n```\n",
        true,
    );
    assert!(!out.contains("<script"), "{out}");
    assert!(!out.contains("<img"), "{out}");
    assert!(out.contains("&lt;script&gt;"));
}

#[test]
fn diagram_colors_follow_the_theme() {
    let doc = Document::parse(
        "```mermaid\ngraph TD\nA-->B\n```\n",
        ParseOptions {
            diagrams: true,
            ..ParseOptions::default()
        },
    );
    let light = render_body(&doc, &HtmlOptions::default());
    let dark = render_body(
        &doc,
        &HtmlOptions {
            diagrams: DiagramExport::from_theme(&Theme::dark()),
            ..HtmlOptions::default()
        },
    );
    assert_ne!(light, dark);
}
