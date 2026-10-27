//! Renders diagram fences to an HTML gallery for visual checks:
//! `cargo run -p markise-blocks --example diagram_svg -- out.html [file.md ...]`.
//! Without input files a built-in sample set is used; each sample is shown in the light and
//! the dark built-in theme.

use markise_blocks::diagram::{ApproxMeasure, DiagramLang, DiagramPalette, RenderCtx, render, svg::to_svg};
use markise_theme::Theme;

const SAMPLES: &[(&str, &str)] = &[
    (
        "mermaid",
        "flowchart TD\n    A[Start] --> B{Is it working?}\n    B -- Yes --> C[Ship it]\n    B -- No --> D(Debug)\n    D --> B\n    C --> E((Done))",
    ),
    (
        "mermaid",
        "graph LR\n    A[(Database)] -->|query| B[[Service]]\n    B --> C>Flag] & D{{Hex}}\n    D -.-> E[/Input/]\n    E ==> F[\\Output\\]\n    F --o G([Stadium])\n    G --x H[/Trap\\]\n    H <--> I(((Double)))",
    ),
    (
        "mermaid",
        "flowchart TB\n    c1-->a2\n    subgraph one\n    a1-->a2\n    end\n    subgraph two [Second group]\n    b1-->b2\n    end\n    subgraph three\n    c1-->c2\n    end\n    one --> two\n    three --> two\n    two --> c2\n    classDef hot fill:#f96,stroke:#333,color:#000\n    class b1 hot",
    ),
    (
        "mermaid",
        "sequenceDiagram\n    autonumber\n    participant Alice\n    actor Bob\n    Alice->>+John: Hello John, how are you?\n    John-->>-Alice: Great!\n    Alice-)John: See you later!\n    Note right of John: John thinks<br>for a while\n    loop Every minute\n        John->>Bob: Ping\n        alt is sick\n            Bob->>John: Not so good\n        else is well\n            Bob->>John: Feeling fresh\n        end\n    end\n    Note over Alice,John: A typical interaction\n    John->>John: Self call\n    Bob-xAlice: Lost",
    ),
    (
        "sequence",
        "Title: Here is a title\nA->B: Normal line\nB-->C: Dashed line\nC->>D: Open arrow\nD-->>A: Dashed open arrow\nNote left of A: Note to the\\n left of A\nNote over B,C: Across B and C",
    ),
    (
        "flow",
        "st=>start: Start\nop1=>operation: My Operation\nsub1=>subroutine: My Subroutine\ncond=>condition: Yes or No?\nio=>inputoutput: catch something...\ne=>end: End\nst->op1->cond\ncond(yes)->io->e\ncond(no)->sub1(right)->op1",
    ),
    ("mermaid", "classDiagram\n    A <|-- B"),
    ("mermaid", "graph TD\n    A-->"),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "diagrams.html".into());
    let mut fences: Vec<(String, String)> = Vec::new();
    for path in args.iter().skip(1) {
        let text = std::fs::read_to_string(path).expect("read input");
        let mut lines = text.lines();
        while let Some(l) = lines.next() {
            if let Some(info) = l.strip_prefix("```") {
                let mut body = String::new();
                for b in lines.by_ref() {
                    if b.starts_with("```") {
                        break;
                    }
                    body.push_str(b);
                    body.push('\n');
                }
                if DiagramLang::from_info(info.split_whitespace().next().unwrap_or("")).is_some() {
                    fences.push((info.trim().to_string(), body));
                }
            }
        }
    }
    if fences.is_empty() {
        fences = SAMPLES.iter().map(|(l, s)| (l.to_string(), s.to_string())).collect();
    }
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>diagrams</title><body style='margin:0;font-family:system-ui'>",
    );
    for theme in [Theme::light(), Theme::dark()] {
        let pal = DiagramPalette::from_theme(&theme);
        let [r, g, b, _] = theme.page_bg.0;
        html.push_str(&format!("<div style='background:#{r:02x}{g:02x}{b:02x};padding:16px'>"));
        for (lang, src) in &fences {
            let lang = DiagramLang::from_info(lang).unwrap();
            let ctx = RenderCtx::new(&ApproxMeasure, 14.0);
            let t = std::time::Instant::now();
            match render(lang, src, &ctx) {
                Ok(scene) => {
                    eprintln!(
                        "{} {:?}: {} items, {:.0}x{:.0}",
                        lang.as_str(),
                        t.elapsed(),
                        scene.items.len(),
                        scene.width,
                        scene.height
                    );
                    html.push_str(&format!(
                        "<div style='margin:12px 0'>{}</div>",
                        to_svg(&scene, &pal, "system-ui, sans-serif")
                    ));
                }
                Err(e) => html.push_str(&format!("<p style='color:#c33'>{e}</p>")),
            }
        }
        html.push_str("</div>");
    }
    std::fs::write(&out, html).expect("write");
}
