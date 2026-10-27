//! Throwaway spike (ADR 0005): parley 0.12 + fontique — register a theme font file under its
//! own family name, shape+break a paragraph, mixed-script fallback, grapheme caret movement,
//! hit testing, RSS after fallback. Run: `cargo run --release` (macOS font paths).
use parley::fontique::{Blob, FontInfoOverride};
use parley::{Cursor, FontContext, Layout, LayoutContext, PositionedLayoutItem, Selection, StyleProperty};
use std::time::Instant;
fn main() {
    let t = Instant::now();
    let mut fcx = FontContext::new();
    let mut lcx: LayoutContext<()> = LayoutContext::new();
    println!("FontContext::new (system discovery) {:?}", t.elapsed());
    let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Georgia.ttf").unwrap();
    let r = fcx.collection.register_fonts(
        Blob::new(std::sync::Arc::new(bytes)),
        Some(FontInfoOverride {
            family_name: Some("ThemeFace"),
            ..Default::default()
        }),
    );
    println!("registered {} family(ies) as ThemeFace", r.len());
    let para = "The quick brown fox jumps over the lazy dog — office finance, “quotes”, 1/2. ".repeat(20);
    let build = |lcx: &mut LayoutContext<()>, fcx: &mut FontContext, text: &str| -> Layout<()> {
        let mut b = lcx.ranged_builder(fcx, text, 1.0, true);
        b.push_default(StyleProperty::FontFamily("ThemeFace, sans-serif".into()));
        b.push_default(StyleProperty::FontSize(16.0));
        let mut l = b.build(text);
        l.break_all_lines(Some(700.0));
        l
    };
    // Same input as the wordcraft and epaint harnesses (ADR 0005 §7): Georgia 16 px, 700 px
    // wide, scale 1, a counter appended per iteration; build + shape + break + drop.
    let _ = build(&mut lcx, &mut fcx, &para); // warm-up
    let mut rounds = Vec::new();
    let mut lines = 0;
    for r in 0..10 {
        let t = Instant::now();
        for i in 0..100 {
            let text = format!("{para}{}", r * 100 + i);
            lines = std::hint::black_box(build(&mut lcx, &mut fcx, &text)).len();
        }
        rounds.push(t.elapsed().as_secs_f64() * 1e6 / 100.0 / (para.len() as f64 / 1024.0));
    }
    rounds.sort_by(f64::total_cmp);
    println!(
        "build+shape+break µs/KiB over 10 rounds x 100: min {:.1} median {:.1} max {:.1} ({lines} lines)",
        rounds[0], rounds[5], rounds[9]
    );
    let pid = std::process::id().to_string();
    let rss = || {
        let o = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    println!("RSS before mixed-script fallback: {} KiB", rss());
    let mixed = "Hello مرحبا بالعالم 123 world, 日本語のテキスト, emoji 👩‍👩‍👧 done.";
    let l = build(&mut lcx, &mut fcx, mixed);
    let mut fonts = std::collections::BTreeSet::new();
    for line in l.lines() {
        for it in line.items() {
            if let PositionedLayoutItem::GlyphRun(g) = it {
                let r = g.run();
                let nd = g.glyphs().filter(|x| x.id == 0).count();
                fonts.insert(format!(
                    "blob{} idx{} rtl={} notdef={}",
                    r.font().data.id(),
                    r.font().index,
                    r.is_rtl(),
                    nd
                ));
            }
        }
    }
    println!("distinct (font blob, face index, rtl, notdef count) tuples over all glyph runs: {fonts:?}");
    let c = Cursor::from_byte_index(&l, 6, parley::Affinity::Downstream);
    let mut s = Selection::from(c);
    let mut idx = vec![];
    for _ in 0..8 {
        s = s.next_visual(&l, false);
        idx.push(s.focus().index());
    }
    println!("visual right-arrow byte indices from 6 through RTL: {idx:?}");
    let fam = "👩‍👩‍👧";
    let e = mixed.find(fam).unwrap();
    let s2 = Selection::from(Cursor::from_byte_index(&l, e, parley::Affinity::Downstream)).next_visual(&l, false);
    println!(
        "one arrow over family emoji moves {} bytes (grapheme = {})",
        s2.focus().index() - e,
        fam.len()
    );
    println!(
        "shape-only RSS after Arabic+CJK+emoji fallback (no rasterisation): {} KiB",
        rss()
    );
    let mut b = lcx.ranged_builder(&mut fcx, "👩क한日م", 1.0, true);
    b.push_default(StyleProperty::FontFamily("ThemeFace".into()));
    let mut l2: Layout<()> = b.build("👩क한日م");
    l2.break_all_lines(None);
    println!("shape-only RSS after + Devanagari/Korean: {} KiB", rss());
    let hit = Cursor::from_point(&l, 30.0, 5.0);
    println!("hit (30,5) -> byte {}", hit.index());
}
