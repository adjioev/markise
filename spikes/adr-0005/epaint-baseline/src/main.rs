//! Throwaway spike (ADR 0005 §7): the epaint 0.33 baseline for the same input as the parley and
//! wordcraft harnesses — Georgia 16 px, 700 px wide, ppp 1, a counter appended per iteration so
//! epaint's galley cache never hits (glyph caches stay warm); `layout_job` = shaping, wrapping,
//! mesh, plus CPU rasterisation of glyphs missing from the atlas.
use epaint::text::{FontData, FontDefinitions, FontFamily, FontId, Fonts, LayoutJob, TextFormat};
use std::time::Instant;

fn main() {
    let mut defs = FontDefinitions::empty(); // Georgia only: no bundled egui fonts
    let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Georgia.ttf").expect("Georgia");
    defs.font_data
        .insert("georgia".into(), FontData::from_owned(bytes).into());
    defs.families
        .insert(FontFamily::Name("g".into()), vec!["georgia".into()]);
    let mut fonts = Fonts::new(8192, Default::default(), defs);
    let para = "The quick brown fox jumps over the lazy dog — office finance, “quotes”, 1/2. ".repeat(20);
    let job = |text: String| {
        let mut j = LayoutJob::single_section(
            text,
            TextFormat {
                font_id: FontId::new(16.0, FontFamily::Name("g".into())),
                ..Default::default()
            },
        );
        j.wrap.max_width = 700.0;
        j
    };
    let _ = fonts.with_pixels_per_point(1.0).layout_job(job(para.clone())); // warm-up
    let mut rounds = Vec::new();
    let mut rows = 0;
    for r in 0..10 {
        let t = Instant::now();
        for i in 0..100 {
            let g = fonts
                .with_pixels_per_point(1.0)
                .layout_job(job(format!("{para}{}", r * 100 + i)));
            rows = std::hint::black_box(g).rows.len();
        }
        rounds.push(t.elapsed().as_secs_f64() * 1e6 / 100.0 / (para.len() as f64 / 1024.0));
    }
    rounds.sort_by(f64::total_cmp);
    println!(
        "epaint layout_job (galley-cache miss, warm glyph atlas) µs/KiB, 10 round averages of 100: min {:.1} median {:.1} max {:.1} ({rows} rows)",
        rounds[0], rounds[5], rounds[9]
    );
}
