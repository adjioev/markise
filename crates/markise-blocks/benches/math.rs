//! Math rendering cost (`cargo bench -p markise-blocks --bench math`, ADR 0008): first-use cost,
//! per-formula layout / SVG / raster time, and the cost of *all* formulas of the 1 MB
//! math-heavy corpus (the viewer only renders visible ones, so this is an upper bound).

use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use markise_blocks::math::{MathMode, layout, raster::rasterize, svg};

const FORMULAS: &[&str] = &[
    r"x^2 + y^2 = z^2",
    r"\frac{a}{b} + \frac{\frac{1}{x}}{1+\frac{1}{y}}",
    r"\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
    r"\int_0^1 f(x)\,dx",
    r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
    r"\begin{aligned} f(x) &= (x+1)^2 \\ &= x^2 + 2x + 1 \end{aligned}",
    r"\left( \frac{a}{b} \right)^2 \sqrt[3]{x^2+1}",
    r"\mathbb{E}[X] = \int_{-\infty}^{\infty} x f(x)\, dx",
];

fn corpus_formulas() -> Vec<String> {
    let text = markise_corpus::generate_profile(1 << 20, 7, markise_corpus::Profile::Math, false);
    let doc = markise_md::parse(&text);
    doc.blocks()
        .iter()
        .filter(|b| matches!(b.kind, markise_md::BlockKind::MathBlock { .. }))
        .map(|b| b.content(doc.text()))
        .collect()
}

fn bench(c: &mut Criterion) {
    let t = Instant::now();
    let l = layout(r"\frac{a}{b}", MathMode::Display).unwrap();
    eprintln!("first layout (tables, lazy statics): {:?}", t.elapsed());
    let t = Instant::now();
    rasterize(&l, 20.0, [0, 0, 0, 255], None);
    eprintln!("first raster (font parse, outlines): {:?}", t.elapsed());

    let all = corpus_formulas();
    let t = Instant::now();
    let mut items = 0;
    for f in &all {
        items += layout(f, MathMode::Display).map(|l| l.items.len()).unwrap_or(0);
    }
    let lt = t.elapsed();
    let t = Instant::now();
    for f in all.iter().take(500) {
        if let Ok(l) = layout(f, MathMode::Display) {
            rasterize(&l, 24.0, [0, 0, 0, 255], None);
        }
    }
    let rt = t.elapsed();
    eprintln!(
        "1 MB math corpus: {} display formulas, layout of all {:?} ({:.1} µs each, {items} items); layout+raster of 500: {:?} ({:.1} µs each)",
        all.len(),
        lt,
        lt.as_secs_f64() * 1e6 / all.len().max(1) as f64,
        rt,
        rt.as_secs_f64() * 1e6 / 500.0,
    );

    let mut g = c.benchmark_group("math");
    g.bench_function("layout", |b| {
        b.iter(|| {
            for f in FORMULAS {
                let _ = layout(f, MathMode::Display);
            }
        })
    });
    let laid: Vec<_> = FORMULAS.iter().map(|f| layout(f, MathMode::Display).unwrap()).collect();
    g.bench_function("svg", |b| {
        b.iter(|| {
            for l in &laid {
                svg::to_svg(l, &svg::SvgOptions::default());
            }
        })
    });
    g.bench_function("raster_24px", |b| {
        b.iter(|| {
            for l in &laid {
                rasterize(l, 24.0, [0, 0, 0, 255], None);
            }
        })
    });
    g.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench
}
criterion_main!(benches);
