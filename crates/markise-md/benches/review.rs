//! Review marks (ADR 0012) on 10 MB documents: listing every mark (the comments panel,
//! Accept All, clean copies) without marks — one byte scan — and with a mark every few
//! paragraphs; a one-mark lookup at an offset (hover bar, Accept at the caret: one leaf);
//! inline parsing of a paragraph dense with marks.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use markise_corpus::{Profile, generate_profile};
use markise_doc::FormatDoc;
use markise_doc::review;
use markise_md::inline::{InlineContext, parse_inlines};
use markise_md::{Document, ParseOptions};

fn with_marks(base: &str) -> String {
    // a comment and a suggestion after every 20th blank line
    let mut out = String::with_capacity(base.len() + base.len() / 20);
    for (i, para) in base.split("\n\n").enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(para);
        if i % 20 == 0 && !para.trim_start().starts_with(['`', '|', '$', '<', '-', '#', '>']) && !para.is_empty() {
            out.push_str(" {==see==}{>>@ai: check this<<} {~~old~>new~~}");
        }
    }
    out
}

fn bench_review(c: &mut Criterion) {
    let mut g = c.benchmark_group("review");
    g.sample_size(20).measurement_time(Duration::from_secs(4));
    let plain = generate_profile(10 << 20, 42, Profile::Mixed, false);
    let marked = with_marks(&plain);
    for (name, text) in [("10mb-no-marks", plain.as_str()), ("10mb-marks", marked.as_str())] {
        let doc = Document::parse(text, ParseOptions::default());
        g.bench_function(format!("all_marks/{name}"), |b| {
            b.iter(|| black_box(review::all_marks(black_box(&doc as &dyn FormatDoc))).len())
        });
        let at = text.len() / 2;
        g.bench_function(format!("mark_at/{name}"), |b| {
            b.iter(|| black_box(review::mark_at(&doc, black_box(at), |k| k.is_suggestion())))
        });
    }
    let dense = "word {++ins++} {--del--} {~~a~>b~~} {==hl==}{>>c<<} ".repeat(2000);
    g.bench_function("parse_inlines/dense-100kb", |b| {
        b.iter(|| {
            black_box(parse_inlines(
                black_box(&dense),
                &ParseOptions::default(),
                &InlineContext::default(),
            ))
            .len()
        })
    });
    g.finish();
}

criterion_group!(benches, bench_review);
criterion_main!(benches);
