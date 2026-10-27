//! `cargo bench -p markise-text`: parley path (markise-text) vs the epaint 0.37 baseline on the
//! ADR 0005 S0 input — the 83-byte sentence × 20 (1660 B), 16 px, 700 px wide, scale 1, a
//! counter appended per iteration so epaint's galley cache never hits (its glyph atlas stays
//! warm). Same font bytes for both (Georgia on macOS/Windows, DejaVu Serif on Linux, else a
//! synthetic font). Also: CSS theme-face path (cluster matching), caret/hit queries, and a cold
//! "first screen" of the 10 MB corpus (S0 gate: < 10 ms).

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use epaint::text::{FontData, FontDefinitions, FontFamily, FontId, Fonts, LayoutJob, TextFormat};
use markise_text::testing::TestFont;
use markise_text::{FaceRule, FaceSource, Family, FontStore, LocalPos, StyledText, TextEngine, TextStyle};

const SENTENCE: &str = "The quick brown fox jumps over the lazy dog — office finance, “quotes”, 1/2. ";

fn font_bytes() -> (Vec<u8>, &'static str) {
    for p in [
        "/System/Library/Fonts/Supplemental/Georgia.ttf",
        "C:\\Windows\\Fonts\\georgia.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    ] {
        if let Ok(b) = std::fs::read(p) {
            return (b, p);
        }
    }
    let f = TestFont::new("Synthetic")
        .with_range(' ', '~', 500)
        .with_chars("—“”", 600);
    (f.build(), "synthetic")
}

fn bench(c: &mut Criterion) {
    let (bytes, which) = font_bytes();
    eprintln!("bench font: {which}");
    let para = SENTENCE.repeat(20);
    let mut g = c.benchmark_group("shaping");
    g.sample_size(30);

    // epaint 0.37 baseline
    let mut defs = FontDefinitions::empty();
    defs.font_data
        .insert("f".into(), FontData::from_owned(bytes.clone()).into());
    defs.families.insert(FontFamily::Name("f".into()), vec!["f".into()]);
    let mut fonts = Fonts::new(Default::default(), defs);
    let job = |text: String| {
        let mut j = LayoutJob::single_section(
            text,
            TextFormat {
                font_id: FontId::new(16.0, FontFamily::Name("f".into())),
                ..Default::default()
            },
        );
        j.wrap.max_width = 700.0;
        j
    };
    let mut n = 0u64;
    g.bench_function("epaint037-layout_job-1660B", |b| {
        b.iter(|| {
            n += 1;
            let galley = fonts.with_pixels_per_point(1.0).layout_job(job(format!("{para}{n}")));
            if n.is_multiple_of(500) {
                fonts.begin_pass(Default::default(), Default::default());
            }
            galley.rows.len()
        })
    });

    // parley via markise-text: theme face registered from the same bytes (single face, no
    // unicode-range: the CSS matching path still runs per grapheme)
    let mut store = FontStore::new();
    store
        .add_face(FaceRule::new("Bench", vec![FaceSource::Bytes(Arc::new(bytes.clone()))]))
        .unwrap();
    let mut eng = TextEngine::new(store);
    let style = TextStyle::new(vec![Family::named("Bench")], 16.0);
    g.bench_function("markise-text-theme-face-1660B", |b| {
        b.iter(|| {
            n += 1;
            let t = StyledText::plain(format!("{para}{n}"), style.clone());
            eng.layout(&t, Some(700.0), 1.0).line_count()
        })
    });

    // parley with the face as a plain family (fast path: no per-grapheme CSS matching)
    let mut fcx_store = FontStore::new();
    fcx_store
        .add_face(FaceRule::new("Bench", vec![FaceSource::Bytes(Arc::new(bytes.clone()))]))
        .unwrap();
    let private = fcx_store.face_family_name(0).to_owned();
    let mut eng2 = TextEngine::new(fcx_store);
    let style2 = TextStyle::new(vec![Family::named(&private)], 16.0);
    g.bench_function("markise-text-fast-path-1660B", |b| {
        b.iter(|| {
            n += 1;
            let t = StyledText::plain(format!("{para}{n}"), style2.clone());
            eng2.layout(&t, Some(700.0), 1.0).line_count()
        })
    });

    // caret queries on one layout
    let l = eng.layout(&StyledText::plain(para.clone(), style.clone()), Some(700.0), 1.0);
    g.bench_function("caret-rect+hit-per-stop-1660B", |b| {
        b.iter(|| {
            let mut acc = 0usize;
            for &s in l.grapheme_boundaries() {
                let r = l.caret_rect(LocalPos::down(s));
                acc += l.hit(r.x0, r.y0 + 1.0).byte;
            }
            acc
        })
    });

    // cold first screen of the 10 MB corpus: the paragraphs filling an 800 px viewport
    let corpus = markise_corpus::generate(10 << 20, 42);
    let lines: Vec<&str> = corpus.lines().filter(|l| !l.trim().is_empty()).take(60).collect();
    g.bench_function("first-screen-10mb-cold-layout", |b| {
        b.iter(|| {
            let mut h = 0.0f32;
            let mut count = 0;
            for line in &lines {
                n += 1;
                let t = StyledText::plain(format!("{line}{}", n % 10), style.clone());
                h += eng.layout(&t, Some(700.0), 2.0).size().1 / 2.0;
                count += 1;
                if h > 800.0 {
                    break;
                }
            }
            count
        })
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
