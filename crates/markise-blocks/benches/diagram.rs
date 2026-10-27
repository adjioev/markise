//! Diagram parse + layout times (ADR 0009). Budgets: a typical diagram (≤ 50 nodes) renders
//! well under one frame; a 200-node flowchart stays below ~50 ms on the worker thread.

use criterion::{Criterion, criterion_group, criterion_main};
use markise_blocks::diagram::{ApproxMeasure, DiagramLang, Limits, RenderCtx, render};
use std::hint::black_box;
use std::time::Duration;

fn flowchart(n: usize) -> String {
    let mut s = String::from("flowchart TD\n");
    for i in 0..n {
        s.push_str(&format!(
            "  N{i}[Node {i}] -->|edge {i}| N{}[Node {}]\n",
            (i * 7 + 3) % n,
            (i * 7 + 3) % n
        ));
    }
    s
}

fn sequence(n: usize) -> String {
    let mut s = String::from("sequenceDiagram\n");
    for i in 0..n {
        s.push_str(&format!("  P{}->>P{}: message {i}\n", i % 6, (i + 1) % 6));
        if i % 10 == 0 {
            s.push_str(&format!("  Note over P{}: note {i}\n", i % 6));
        }
    }
    s
}

fn bench(c: &mut Criterion) {
    let m = ApproxMeasure;
    let limits = Limits {
        time_budget: Duration::from_secs(60),
        ..Limits::default()
    };
    for (name, lang, src) in [
        ("flowchart_10", DiagramLang::Mermaid, flowchart(10)),
        ("flowchart_50", DiagramLang::Mermaid, flowchart(50)),
        ("flowchart_200", DiagramLang::Mermaid, flowchart(200)),
        ("sequence_50", DiagramLang::Mermaid, sequence(50)),
        ("sequence_500", DiagramLang::Mermaid, sequence(500)),
    ] {
        c.bench_function(name, |b| {
            b.iter(|| {
                let ctx = RenderCtx::new(&m, 14.0).with_limits(limits);
                black_box(render(lang, black_box(&src), &ctx).unwrap())
            })
        });
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
