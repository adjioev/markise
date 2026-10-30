//! HTML export of documents with many `[TOC]` blocks (issue #29): every `[TOC]` lists all
//! headings, so the output is TOCs × headings entries; the table of contents is formatted once
//! per export and copied for the others.

use criterion::{Criterion, criterion_group, criterion_main};
use markise_export::{HtmlOptions, render_body};

/// `headings` headings with a short paragraph each and a `[TOC]` every `every` headings.
fn doc(headings: usize, every: usize) -> String {
    let mut s = String::new();
    for i in 0..headings {
        if i % every == 0 {
            s.push_str("[TOC]\n\n");
        }
        let level = 1 + i % 3;
        s.push_str(&"#".repeat(level));
        s.push_str(&format!(" Section *{i}* with `code`\n\nSome text for section {i}.\n\n"));
    }
    s
}

fn bench(c: &mut Criterion) {
    let opts = HtmlOptions::default();
    let mut g = c.benchmark_group("export_toc");
    g.sample_size(10);
    for (n, every) in [(2_000, 200), (10_000, 200)] {
        let parsed = markise_md::parse(&doc(n, every));
        let size = render_body(&parsed, &opts).len();
        eprintln!("export_toc/{n}h_{}toc: {size} bytes of HTML", n / every);
        g.bench_function(format!("{n}h_{}toc", n / every), |b| {
            b.iter(|| render_body(&parsed, &opts))
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
