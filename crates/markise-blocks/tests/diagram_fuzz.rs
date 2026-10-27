//! Untrusted-input properties of diagram rendering (ADR 0009 §5): no panics, bounded time and
//! output, well-formed scenes, for random and adversarial sources in all three languages.

use std::time::{Duration, Instant};

use markise_blocks::diagram::{ApproxMeasure, DiagramError, DiagramLang, Limits, RenderCtx, render};
use proptest::prelude::*;

const SEEDS: &[(DiagramLang, &str)] = &[
    (
        DiagramLang::Mermaid,
        "flowchart TD\nA[Start] --> B{OK?}\nB -- yes --> C((Done))\nB -. no .-> A\nsubgraph S [Group]\nC --> D[(DB)]\nend\nclassDef x fill:#f00\nclass A x\n",
    ),
    (
        DiagramLang::Mermaid,
        "graph LR\nA & B --> C & D\nC ==>|thick| E>flag]\nE --o F{{hex}} --x G[/in/]\n",
    ),
    (
        DiagramLang::Mermaid,
        "sequenceDiagram\nautonumber\nparticipant A as Alice\nactor B\nA->>+B: hi\nB-->>-A: yo\nloop x\nalt y\nA-xB: z\nelse w\nB-)A: v\nend\nend\nNote over A,B: n\nrect rgb(1,2,3)\nA->A: self\nend\n",
    ),
    (
        DiagramLang::Sequence,
        "Title: t\nA->B: x\nB-->C: y\nC->>A: z\nNote left of A: n\\nm\nparticipant \"Long\" as L\n",
    ),
    (
        DiagramLang::Flow,
        "st=>start: S\nc=>condition: C?\nop=>operation: O\ne=>end: E\nst->c\nc(yes)->op->e\nc(no)->st\n",
    ),
];

fn ctx(m: &ApproxMeasure) -> RenderCtx<'_> {
    RenderCtx::new(m, 14.0).with_limits(Limits {
        time_budget: Duration::from_secs(5),
        ..Limits::default()
    })
}

fn check(lang: DiagramLang, src: &str) {
    let m = ApproxMeasure;
    let t = Instant::now();
    let r = render(lang, src, &ctx(&m));
    assert!(t.elapsed() < Duration::from_secs(8), "slow: {:?}", t.elapsed());
    match r {
        Ok(scene) => {
            assert!(scene.is_well_formed(), "malformed scene for {src:?}");
            assert!(scene.items.len() < 200_000);
        }
        Err(DiagramError::Syntax { line, .. }) => assert!(
            line >= 1 && line <= src.lines().count().max(1),
            "bad line {line} for {src:?}"
        ),
        Err(_) => {}
    }
}

fn mutate(seed: &str, ops: &[(usize, u8, u8)]) -> String {
    const ALPHA: &[u8] = b"[](){}<>/\\-=.|:;&\"'%#*+ox~@ \nABend subgraph loop alt else Note over ->>-->>";
    let mut s = seed.as_bytes().to_vec();
    for &(pos, op, ch) in ops {
        let pos = pos % (s.len() + 1);
        let c = ALPHA[ch as usize % ALPHA.len()];
        match op % 4 {
            0 => s.insert(pos, c),
            1 if pos < s.len() => {
                s.remove(pos);
            }
            2 => {
                for _ in 0..(ch % 16) {
                    s.insert(pos.min(s.len()), c);
                }
            }
            _ => {
                // duplicate a slice (grows structure)
                let end = (pos + (ch as usize % 40)).min(s.len());
                let part = s[pos.min(end)..end].to_vec();
                for (i, b) in part.into_iter().enumerate() {
                    s.insert((end + i).min(s.len()), b);
                }
            }
        }
    }
    String::from_utf8_lossy(&s).into_owned()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn mutated_seeds_never_panic(seed in 0usize..SEEDS.len(), ops in prop::collection::vec((0usize..4096, any::<u8>(), any::<u8>()), 1..24)) {
        let (lang, src) = SEEDS[seed];
        check(lang, &mutate(src, &ops));
    }

    #[test]
    fn arbitrary_text_never_panics(s in "\\PC{0,400}", lang in 0usize..3) {
        let lang = [DiagramLang::Mermaid, DiagramLang::Sequence, DiagramLang::Flow][lang];
        check(lang, &s);
        check(DiagramLang::Mermaid, &format!("flowchart TD\n{s}"));
        check(DiagramLang::Mermaid, &format!("sequenceDiagram\n{s}"));
    }
}

#[test]
fn deep_nesting_is_bounded() {
    let deep_sub = format!(
        "graph TD\n{}A-->B\n{}",
        "subgraph s\n".repeat(5_000),
        "end\n".repeat(5_000)
    );
    assert!(matches!(
        render(DiagramLang::Mermaid, &deep_sub, &ctx(&ApproxMeasure)),
        Err(DiagramError::TooLarge(_))
    ));
    let deep_loop = format!("sequenceDiagram\n{}A->>B: x\n", "loop l\n".repeat(5_000));
    assert!(matches!(
        render(DiagramLang::Mermaid, &deep_loop, &ctx(&ApproxMeasure)),
        Err(DiagramError::TooLarge(_))
    ));
    // within the limit: renders
    let ok = format!(
        "sequenceDiagram\n{}A->>B: x\n{}",
        "loop l\n".repeat(20),
        "end\n".repeat(20)
    );
    assert!(render(DiagramLang::Mermaid, &ok, &ctx(&ApproxMeasure)).is_ok());
}

#[test]
fn huge_labels_and_lines_are_bounded() {
    let long = "x".repeat(200_000);
    check(DiagramLang::Mermaid, &format!("graph TD\nA[{long}]-->B"));
    let s = format!("graph TD\nA[{}]-->B", "y".repeat(5_000));
    let scene = render(DiagramLang::Mermaid, &s, &ctx(&ApproxMeasure)).unwrap();
    assert!(scene.width < 20_000.0, "label must be truncated: {}", scene.width);
    check(DiagramLang::Sequence, &format!("A->B: {}", "z".repeat(200_000)));
    check(DiagramLang::Mermaid, &format!("graph TD\n{}", "A-->".repeat(30_000)));
}

#[test]
fn node_and_edge_counts_are_bounded() {
    let many = format!("graph TD\n{}", (0..3000).map(|i| format!("n{i}\n")).collect::<String>());
    assert!(matches!(
        render(DiagramLang::Mermaid, &many, &ctx(&ApproxMeasure)),
        Err(DiagramError::TooLarge(_))
    ));
    let fan = format!(
        "graph TD\n{} --> {}",
        (0..60).map(|i| format!("a{i}")).collect::<Vec<_>>().join(" & "),
        (0..60).map(|i| format!("b{i}")).collect::<Vec<_>>().join(" & ")
    );
    assert!(matches!(
        render(DiagramLang::Mermaid, &fan, &ctx(&ApproxMeasure)),
        Err(DiagramError::TooLarge(_))
    ));
    let msgs = format!("sequenceDiagram\n{}", "A->>B: x\n".repeat(5_000));
    assert!(matches!(
        render(DiagramLang::Mermaid, &msgs, &ctx(&ApproxMeasure)),
        Err(DiagramError::TooLarge(_))
    ));
}

#[test]
fn moderate_graphs_render_quickly() {
    // 200 nodes / 200 labelled edges: well inside the default 2 s budget (debug build included)
    let src = format!(
        "flowchart TD\n{}",
        (0..200)
            .map(|i| format!("N{i}[Node {i}] -->|e{i}| N{}\n", (i * 7 + 3) % 200))
            .collect::<String>()
    );
    let m = ApproxMeasure;
    let t = Instant::now();
    let scene = render(DiagramLang::Mermaid, &src, &RenderCtx::new(&m, 14.0)).unwrap();
    assert!(scene.is_well_formed());
    eprintln!("200-node flowchart: {:?}", t.elapsed());
}

/// Codex P1 (viewer): a sequence whose messages carry many `<br>` lines would produce a scene
/// over a million points tall; it is refused (source + notice) instead of handed to painters.
#[test]
fn oversized_scenes_are_refused() {
    let msg = format!("A->>B: {}\n", vec!["x"; 39].join("<br>"));
    let src = format!("sequenceDiagram\n{}", msg.repeat(1_200));
    assert!(src.len() < 256 * 1024);
    let m = ApproxMeasure;
    let r = render(DiagramLang::Mermaid, &src, &ctx(&m));
    assert!(
        matches!(r, Err(DiagramError::TooLarge(_))),
        "{:?}",
        r.as_ref().map(|s| (s.width, s.height))
    );
}
