//! M0 benchmarks: parse and serialize of the 1 MB / 10 MB corpus (NFR-2/3/8), keystroke
//! re-parse in a 10 MB document (NFR-4 budget share) and inline parsing.
//!
//! Corpus: `scripts/gen-bench-corpus.sh` writes `benches/corpus/*.md`; when absent the same
//! deterministic text is generated in memory.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use markise_corpus::{Profile, generate_profile};
use markise_md::inline::{InlineContext, parse_inlines};
use markise_md::{Document, ParseOptions};

fn corpus(name: &str, size: usize, profile: Profile, crlf: bool) -> String {
    let path = format!("{}/../../benches/corpus/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|_| generate_profile(size, 42, profile, crlf))
}

fn inputs() -> Vec<(&'static str, String)> {
    vec![
        ("1mb", corpus("1mb.md", 1 << 20, Profile::Mixed, false)),
        ("10mb", corpus("10mb.md", 10 << 20, Profile::Mixed, false)),
        ("tables-1mb", corpus("tables.md", 1 << 20, Profile::Tables, false)),
        ("math-1mb", corpus("math.md", 1 << 20, Profile::Math, false)),
        ("cjk-crlf-1mb", corpus("cjk-crlf.md", 1 << 20, Profile::Cjk, true)),
    ]
}

fn bench_parse_serialize(c: &mut Criterion) {
    let inputs = inputs();
    let mut g = c.benchmark_group("md");
    g.sample_size(20).measurement_time(Duration::from_secs(6));
    for (name, text) in &inputs {
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::new("parse", name), text, |b, t| {
            b.iter(|| Document::parse(black_box(t.as_str()), ParseOptions::default()))
        });
        let doc = Document::parse(text.as_str(), ParseOptions::default());
        assert_eq!(doc.serialize(), *text);
        g.bench_with_input(BenchmarkId::new("serialize", name), &doc, |b, d| {
            b.iter(|| black_box(d).serialize())
        });
    }
    g.finish();
}

fn bench_keystroke(c: &mut Criterion) {
    let text = corpus("10mb.md", 10 << 20, Profile::Mixed, false);
    let mut g = c.benchmark_group("md-edit");
    g.sample_size(50);
    // Typing one character in the middle of a 10 MB document, then deleting it again.
    let mid = {
        let mut m = text.len() / 2;
        while !text.is_char_boundary(m) {
            m += 1;
        }
        m
    };
    let mut doc = Document::parse(text.as_str(), ParseOptions::default());
    g.bench_function("keystroke-10mb", |b| {
        b.iter(|| {
            doc.edit(mid..mid, "x");
            doc.edit(mid..mid + 1, "");
        })
    });
    // Opening a fence near the top re-parses until it closes again (worst-ish case).
    let mut doc = Document::parse(text.as_str(), ParseOptions::default());
    let at = doc.blocks()[doc.blocks().len() / 2].span.start;
    g.bench_function("newline-10mb", |b| {
        b.iter(|| {
            doc.edit(at..at, "\n");
            doc.edit(at..at + 1, "");
        })
    });
    g.finish();
}

fn bench_inline(c: &mut Criterion) {
    let text = corpus("1mb.md", 1 << 20, Profile::Mixed, false);
    let doc = Document::parse(text.as_str(), ParseOptions::default());
    let ctx = InlineContext {
        link_defs: doc.link_defs(),
        footnotes: doc.footnote_labels().into_iter().collect(),
    };
    let mut leaves = Vec::new();
    for b in doc.blocks() {
        b.walk(&mut |b, _| {
            if !b.is_container() && !b.lines.is_empty() {
                leaves.push(b.content(doc.text()));
            }
        });
    }
    let mut g = c.benchmark_group("md-inline");
    g.sample_size(20).throughput(Throughput::Bytes(text.len() as u64));
    g.bench_function("inline-all-1mb", |b| {
        b.iter(|| {
            let mut n = 0;
            for l in &leaves {
                n += parse_inlines(black_box(l), doc.options(), &ctx).len();
            }
            n
        })
    });
    g.finish();
}

/// Adversarial inputs (review P2 #6): parse + inline parse must stay linear.
fn bench_adversarial(c: &mut Criterion) {
    let mut g = c.benchmark_group("md-adversarial");
    g.sample_size(10).measurement_time(Duration::from_secs(2));
    for (name, f) in markise_corpus::adversarial() {
        let text = f(25_000);
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(name), &text, |b, t| {
            b.iter(|| {
                let doc = Document::parse(black_box(t.as_str()), ParseOptions::default());
                let ctx = InlineContext::default();
                let mut n = 0;
                for blk in doc.blocks() {
                    blk.walk(&mut |blk, _| {
                        if !blk.is_container() && !blk.lines.is_empty() {
                            n += parse_inlines(&blk.content(doc.text()), doc.options(), &ctx).len();
                        }
                    });
                }
                n
            })
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_parse_serialize,
    bench_keystroke,
    bench_inline,
    bench_adversarial
);
criterion_main!(benches);
