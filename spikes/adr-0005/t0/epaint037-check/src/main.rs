//! Evidence for ADR 0005's "stay on epaint?" re-check (egui/epaint 0.37 shape with harfrust):
//! lays out mixed-direction strings with epaint 0.37 and prints the visual order of characters
//! (glyph x positions), so missing bidi reordering is visible, plus cursor granularity.
//! Run: `cargo run --release -p spike-epaint037-check` (macOS: Arial has Latin/Hebrew/Arabic).
use epaint::text::{FontData, FontDefinitions, FontFamily, FontId, Fonts, LayoutJob, TextFormat};

fn main() {
    let mut defs = FontDefinitions::empty();
    let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Arial.ttf").expect("Arial");
    defs.font_data
        .insert("arial".into(), FontData::from_owned(bytes).into());
    defs.families.insert(FontFamily::Name("a".into()), vec!["arial".into()]);
    let mut fonts = Fonts::new(Default::default(), defs);
    for text in [
        "abc \u{5D0}\u{5D1}\u{5D2} def",
        "Hello مرحبا 123",
        "\u{5D0}\u{5D1} (abc) \u{5D2}",
    ] {
        let job = LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                font_id: FontId::new(16.0, FontFamily::Name("a".into())),
                ..Default::default()
            },
        );
        let g = fonts.with_pixels_per_point(1.0).layout_job(job);
        let row = &g.rows[0];
        let mut by_x: Vec<(f32, char)> = row.glyphs.iter().map(|gl| (gl.pos.x, gl.chr)).collect();
        by_x.sort_by(|a, b| a.0.total_cmp(&b.0));
        let visual: String = by_x.iter().map(|x| x.1).collect();
        println!(
            "logical {text:?}\n  epaint visual (by glyph x) {visual:?}; glyphs {} = chars {}",
            row.glyphs.len(),
            text.chars().count()
        );
    }
    // a ZWJ family: one glyph? how many cursor stops (glyphs) does the row have?
    let fam = "a\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}b";
    let job = LayoutJob::single_section(
        fam.to_owned(),
        TextFormat {
            font_id: FontId::new(16.0, FontFamily::Name("a".into())),
            ..Default::default()
        },
    );
    let g = fonts.with_pixels_per_point(1.0).layout_job(job);
    println!(
        "ZWJ family string: {} chars, {} row glyphs (cursor stops are char-indexed: CCursor)",
        fam.chars().count(),
        g.rows[0].glyphs.len()
    );
}
