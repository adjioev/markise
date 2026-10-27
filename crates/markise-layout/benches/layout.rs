//! `cargo bench -p markise-layout`: view-model build and first-screen layout (NFR-2/3 parts).

use criterion::{Criterion, criterion_group, criterion_main};
use std::time::Instant;

use markise_layout::block::{Images, NoImages};
use markise_layout::{DocModel, MathBox, MathQuery, MathState, testing};
use markise_theme::Theme;

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("layout");
    g.sample_size(20);
    for (name, size) in [("1mb", 1usize << 20), ("10mb", 10 << 20)] {
        let text = markise_corpus::generate(size, 42);
        let doc = markise_md::parse(&text);
        g.bench_function(format!("doc-model-build-{name}"), |b| {
            // the clone keeps the measurement comparable with M1 (the document was moved in)
            b.iter(|| {
                let d = doc.clone();
                DocModel::build(&d, None)
            })
        });
        let shown = testing::Shown::new(std::sync::Arc::new(doc));
        let model = shown.model.clone();
        let mut fonts = testing::fonts();
        g.bench_function(format!("first-screen-cold-{name}"), |b| {
            b.iter(|| {
                // flush epaint's galley cache (twice: it keeps the previous pass) so every
                // iteration shapes the screen from scratch; glyph rasters stay in the atlas
                fonts.begin_pass(testing::text_options(), Default::default());
                fonts.begin_pass(testing::text_options(), Default::default());
                let mut f = fonts.with_pixels_per_point(2.0);
                let mut v = shown.view(Theme::light(), 700.0, 2.0);
                v.scroll_to_item(model.items.len() / 2);
                v.frame(800.0, 400.0, &mut f, &NoImages).len()
            })
        });
    }
    math_first_screen(&mut g);
    // diagram label metrics (ADR 0009 §4): measured on the UI thread once per theme/font change
    let mut fonts = testing::fonts();
    g.bench_function("diagram-label-metrics-cold", |b| {
        b.iter(|| {
            fonts.begin_pass(testing::text_options(), Default::default());
            fonts.begin_pass(testing::text_options(), Default::default());
            let mut f = fonts.with_pixels_per_point(2.0);
            markise_layout::block::label_metrics(&mut f)
        })
    });
    g.finish();
}

/// Math through the real renderer (ADR 0008): the math-heavy 1 MB corpus, first screen (formulas
/// pending: what the user sees first), then the time until every visible formula is rendered and
/// the screen laid out again.
struct RealMath(markise_blocks::math::MathService);

impl Images for RealMath {
    fn state(&self, _src: &str) -> markise_layout::block::ImageState {
        markise_layout::block::ImageState::Pending
    }

    fn math(&self, q: &MathQuery) -> MathState {
        use markise_blocks::math::{MathImageLookup, MathMode, MathRequest};
        let req = MathRequest {
            tex: q.tex.to_string(),
            mode: if q.display { MathMode::Display } else { MathMode::Inline },
            px_per_em: q.px_per_em,
            color: q.color.to_srgba_unmultiplied(),
            max_width: q.max_width,
        };
        if q.measure {
            return match self.0.measure(&req) {
                markise_blocks::math::MathMeasure::Ready { width, ascent, descent } => MathState::Ready(MathBox {
                    key: 0,
                    size: [0, 0],
                    baseline: 0.0,
                    pad: 0.0,
                    width,
                    ascent,
                    descent,
                }),
                markise_blocks::math::MathMeasure::Empty => MathState::Empty,
                markise_blocks::math::MathMeasure::Failed(e) => MathState::Failed(e.to_string()),
                markise_blocks::math::MathMeasure::Pending => MathState::Pending,
            };
        }
        match self.0.get(&req) {
            MathImageLookup::Ready(i) => MathState::Ready(MathBox {
                key: i.key,
                size: i.size,
                baseline: i.baseline,
                pad: i.pad as f32,
                width: i.width,
                ascent: i.ascent,
                descent: i.descent,
            }),
            MathImageLookup::Empty => MathState::Empty,
            MathImageLookup::Failed(e) => MathState::Failed(e.to_string()),
            MathImageLookup::Pending => MathState::Pending,
        }
    }
}

fn math_first_screen(g: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    let text = markise_corpus::generate_profile(1 << 20, 42, markise_corpus::Profile::Math, false);
    let shown = testing::Shown::new(std::sync::Arc::new(markise_md::parse(&text)));
    let mut fonts = testing::fonts();
    // one-off: cold renderer, first screen until all formulas are drawn
    {
        let svc = RealMath(markise_blocks::math::MathService::new(std::sync::Arc::new(|| {})).unwrap());
        let mut f = fonts.with_pixels_per_point(2.0);
        let t = Instant::now();
        let mut v = shown.view(Theme::light(), 700.0, 2.0);
        v.frame(800.0, 400.0, &mut f, &svc);
        let first = t.elapsed();
        while svc.0.busy() {
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        v.math_changed();
        let n = v.frame(800.0, 400.0, &mut f, &svc).len();
        eprintln!(
            "math-1mb: first screen (formulas pending) {first:?}; all visible formulas rendered and laid out after {:?} ({n} items, {} formulas)",
            t.elapsed(),
            svc.0.stats().rendered
        );
    }
    g.bench_function("first-screen-cold-math-1mb", |b| {
        b.iter(|| {
            fonts.begin_pass(testing::text_options(), Default::default());
            fonts.begin_pass(testing::text_options(), Default::default());
            let mut f = fonts.with_pixels_per_point(2.0);
            let mut v = shown.view(Theme::light(), 700.0, 2.0);
            v.frame(800.0, 400.0, &mut f, &NoImages).len()
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
