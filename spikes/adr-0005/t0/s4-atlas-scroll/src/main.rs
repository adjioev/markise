//! Spike S4 (ADR 0005 §6): glyph atlas under scrolling of the 10 MB corpus + COLRv1 behaviour.
//!
//! Headless frame simulation (no window, no GPU): the 10 MB corpus split into paragraphs
//! (one per non-empty source line, every 40th gets colour emoji appended), laid out with
//! markise-text at 700 px, viewport 800 px. Per frame: lay out newly visible paragraphs (cached
//! afterwards), look up / rasterise their glyphs in the atlas (vello_cpu), build quads.
//! "Frame CPU" = layout + atlas + quads; it excludes egui tessellation, texture upload and GPU.
//! Wheel: 600 frames × 40 logical px; jumps: 100 random positions (scrollbar drags).
//! COLRv1: a synthetic COLRv1-only font (PaintGlyph → PaintLinearGradient, CPAL red→blue)
//! rasterised through the same atlas; plus a scan of the system fonts for COLRv1 tables.
//! Run: `cargo run --release -p spike-s4-atlas-scroll`.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atlas::{Atlas, Quad};
use markise_text::testing::TestFont;
use markise_text::{FaceRule, FaceSource, Family, FontStore, StyledText, TextEngine, TextLayout, TextStyle};
use skrifa::MetadataProvider;
use skrifa::raw::TableProvider;

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend(x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend(x.to_be_bytes());
}

/// COLR v1 table: one base glyph → PaintGlyph(gid) → PaintLinearGradient(red → blue).
fn colr_v1(gid: u16) -> Vec<u8> {
    let mut t = Vec::new();
    be16(&mut t, 1); // version
    be16(&mut t, 0); // numBaseGlyphRecords
    be32(&mut t, 0);
    be32(&mut t, 0);
    be16(&mut t, 0); // numLayerRecords
    be32(&mut t, 34); // baseGlyphListOffset
    be32(&mut t, 0); // layerList
    be32(&mut t, 0); // clipList
    be32(&mut t, 0); // varIndexMap
    be32(&mut t, 0); // itemVariationStore
    assert_eq!(t.len(), 34);
    // BaseGlyphList
    be32(&mut t, 1);
    be16(&mut t, gid);
    be32(&mut t, 10); // paint offset from BaseGlyphList
    // PaintGlyph
    t.push(10);
    t.extend([0, 0, 6]); // Offset24 to child paint
    be16(&mut t, gid);
    // PaintLinearGradient
    t.push(4);
    t.extend([0, 0, 16]); // Offset24 to ColorLine
    for v in [0i16, 0, 1000, 0, 0, 1000] {
        be16(&mut t, v as u16);
    }
    // ColorLine
    t.push(0); // extend pad
    be16(&mut t, 2);
    for (off, idx) in [(0u16, 0u16), (0x4000, 1)] {
        be16(&mut t, off);
        be16(&mut t, idx);
        be16(&mut t, 0x4000);
    }
    t
}

fn cpal_red_blue() -> Vec<u8> {
    let mut c = Vec::new();
    for v in [0u16, 2, 1, 2] {
        be16(&mut c, v);
    }
    be32(&mut c, 14);
    be16(&mut c, 0);
    c.extend([0, 0, 255, 255]); // BGRA red
    c.extend([255, 0, 0, 255]); // blue
    c
}

fn colr_check() {
    println!("== COLRv1");
    let base = TestFont::new("Colr One").with_chars("\u{1F600}", 1000);
    let gid = base.glyph_of('\u{1F600}') as u16;
    let font = base
        .with_table(b"COLR", colr_v1(gid))
        .with_table(b"CPAL", cpal_red_blue())
        .build();
    let f = skrifa::FontRef::new(&font).unwrap();
    println!(
        "   synthetic font: COLR version {}, glyph is COLRv1 paint graph: {:?}",
        f.colr().map(|c| c.version()).unwrap_or(0),
        f.color_glyphs()
            .get(skrifa::GlyphId::new(gid as u32))
            .map(|g| matches!(g.format(), skrifa::color::ColorGlyphFormat::ColrV1))
    );
    let mut store = FontStore::new();
    store
        .add_face(FaceRule::new("Colr One", vec![FaceSource::Bytes(Arc::new(font))]))
        .unwrap();
    let mut eng = TextEngine::new(store);
    let l = eng.layout(
        &StyledText::plain("\u{1F600}", TextStyle::new(vec![Family::named("Colr One")], 64.0)),
        None,
        1.0,
    );
    let mut atlas = Atlas::new(512);
    let runs = l.glyph_runs();
    let e = atlas.glyph(&runs[0], runs[0].glyphs[0].id, 0.0).expect("painted");
    let p = &atlas.pages[e.page];
    let mut colours = std::collections::BTreeSet::new();
    let (mut left, mut right) = ([0u32; 3], [0u32; 3]);
    for y in 0..e.h as usize {
        for x in 0..e.w as usize {
            let i = 4 * ((e.y as usize + y) * p.size + e.x as usize + x);
            let px = &p.pixels[i..i + 4];
            if px[3] == 255 {
                colours.insert((px[0], px[1], px[2]));
                let side = if x < e.w as usize / 4 {
                    Some(&mut left)
                } else if x > 3 * e.w as usize / 4 {
                    Some(&mut right)
                } else {
                    None
                };
                if let Some(s) = side {
                    for k in 0..3 {
                        s[k] += px[k] as u32;
                    }
                }
            }
        }
    }
    println!(
        "   rasterised via vello_cpu: colour entry {}, {}×{} px, {} distinct opaque colours; left quarter RGB sum {:?}, right quarter {:?}",
        e.color,
        e.w,
        e.h,
        colours.len(),
        left,
        right
    );
    let verdict = e.color && colours.len() > 16 && left[0] > left[2] && right[2] > right[0];
    println!(
        "   COLRv1 gradient drawn in colour (red→blue): {}",
        if verdict { "YES" } else { "NO" }
    );

    // system fonts with COLR v1
    let mut found = Vec::new();
    let mut colr0 = Vec::new();
    for dir in markise_text::FaceNameIndex::platform_dirs() {
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else { continue };
                if let Ok(f) = skrifa::FontRef::new(&bytes)
                    && let Ok(c) = f.colr()
                {
                    let name = p.file_name().unwrap().to_string_lossy().to_string();
                    if c.version() >= 1 {
                        found.push(name)
                    } else {
                        colr0.push(name)
                    }
                }
            }
        }
    }
    println!("   system fonts with COLR v1: {found:?}; COLR v0 only: {colr0:?}");
}

fn pct(v: &mut [Duration], p: f64) -> f64 {
    v.sort();
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i].as_secs_f64() * 1e3
}

struct Sim {
    eng: TextEngine,
    paras: Vec<String>,
    cache: HashMap<usize, Arc<TextLayout>>,
    order: VecDeque<usize>,
    scale: f32,
    style: TextStyle,
}

impl Sim {
    fn layout(&mut self, i: usize) -> Arc<TextLayout> {
        if let Some(l) = self.cache.get(&i) {
            return l.clone();
        }
        let t = StyledText::plain(self.paras[i].clone(), self.style.clone());
        let l = Arc::new(self.eng.layout(&t, Some(700.0), self.scale));
        self.cache.insert(i, l.clone());
        self.order.push_back(i);
        if self.order.len() > 4096 {
            let old = self.order.pop_front().unwrap();
            self.cache.remove(&old);
        }
        l
    }

    /// One frame at (top paragraph, offset px). Returns (layout time, atlas+quads time, quads).
    fn frame(&mut self, atlas: &mut Atlas, top: usize, off: f32, quads: &mut Vec<Quad>) -> (Duration, Duration) {
        let vh = 800.0 * self.scale;
        let mut y = -off;
        let mut i = top;
        let mut t_layout = Duration::ZERO;
        let mut t_atlas = Duration::ZERO;
        quads.clear();
        while y < vh && i < self.paras.len() {
            let t = Instant::now();
            let l = self.layout(i);
            t_layout += t.elapsed();
            let t = Instant::now();
            atlas.quads(&l.glyph_runs(), (0.0, y), |_| 0x202020FF, quads);
            t_atlas += t.elapsed();
            y += l.size().1 + 8.0 * self.scale;
            i += 1;
        }
        (t_layout, t_atlas)
    }

    fn height(&mut self, i: usize) -> f32 {
        self.layout(i).size().1 + 8.0 * self.scale
    }
}

fn scroll_case(paras: &[String], scale: f32) {
    let mut store = FontStore::new();
    store.start_system_discovery();
    store.wait_for_system(Duration::from_secs(60));
    let mut sim = Sim {
        eng: TextEngine::new(store),
        paras: paras.to_vec(),
        cache: HashMap::new(),
        order: VecDeque::new(),
        scale,
        style: TextStyle::new(Family::parse_list("Georgia, serif"), 16.0),
    };
    let mut atlas = Atlas::new(1024);
    let mut quads = Vec::new();
    // first frame (cold): everything new
    let t = Instant::now();
    sim.frame(&mut atlas, 0, 0.0, &mut quads);
    let first = t.elapsed();
    // wheel
    let mut wheel = Vec::new();
    let mut wl = Vec::new();
    let mut wa = Vec::new();
    let (mut top, mut off) = (0usize, 0.0f32);
    for _ in 0..600 {
        off += 40.0 * scale;
        loop {
            let h = sim.height(top);
            if off >= h {
                off -= h;
                top += 1;
            } else {
                break;
            }
        }
        let t = Instant::now();
        let (l, a) = sim.frame(&mut atlas, top, off, &mut quads);
        wheel.push(t.elapsed());
        wl.push(l);
        wa.push(a);
    }
    let after_wheel = (atlas.mono_pages(), atlas.color_pages(), atlas.stats.rasterised);
    // jumps
    let mut jumps = Vec::new();
    let mut jl = Vec::new();
    let mut x: u64 = 0x9E3779B97F4A7C15;
    for _ in 0..100 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let i = (x % paras.len() as u64) as usize;
        let t = Instant::now();
        let (l, _) = sim.frame(&mut atlas, i, 0.0, &mut quads);
        jumps.push(t.elapsed());
        jl.push(l);
    }
    let used: usize = atlas.pages.iter().map(|p| p.used_px).sum();
    let (mut glyphs, mut notdef) = (0usize, 0usize);
    for i in 0..3000.min(paras.len()) {
        for r in sim.layout(i).glyph_runs() {
            glyphs += r.glyphs.len();
            notdef += r.glyphs.iter().filter(|g| g.id == 0).count();
        }
    }
    println!("   first 3000 paragraphs: {glyphs} glyphs, {notdef} .notdef");
    println!(
        "== scale {scale}×: first frame {:.2} ms, quads/frame ~{}",
        first.as_secs_f64() * 1e3,
        quads.len()
    );
    println!(
        "   wheel 600 frames: frame CPU p50 {:.2} ms p99 {:.2} ms max {:.2} ms (layout p99 {:.2}, atlas+quads p99 {:.2})",
        pct(&mut wheel.clone(), 0.5),
        pct(&mut wheel.clone(), 0.99),
        pct(&mut wheel, 1.0),
        pct(&mut wl, 0.99),
        pct(&mut wa, 0.99)
    );
    println!(
        "   jumps 100: frame CPU p50 {:.2} ms p99 {:.2} ms max {:.2} ms (layout p99 {:.2})",
        pct(&mut jumps.clone(), 0.5),
        pct(&mut jumps.clone(), 0.99),
        pct(&mut jumps, 1.0),
        pct(&mut jl, 0.99)
    );
    println!(
        "   atlas after wheel: {} mono + {} colour pages of 1024², {} glyphs rasterised; after jumps: {} mono + {} colour pages, {} glyphs, {:.1}% of page area used; raster time total {:.1} ms ({:.1} µs/glyph)",
        after_wheel.0,
        after_wheel.1,
        after_wheel.2,
        atlas.mono_pages(),
        atlas.color_pages(),
        atlas.stats.rasterised,
        100.0 * used as f64 / (atlas.pages.len() * 1024 * 1024) as f64,
        atlas.stats.raster_ns as f64 / 1e6,
        atlas.stats.raster_ns as f64 / 1e3 / atlas.stats.rasterised.max(1) as f64
    );
}

fn main() {
    colr_check();
    let corpus = markise_corpus::generate(10 << 20, 42);
    let paras: Vec<String> = corpus
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| {
            if i % 40 == 7 {
                format!("{l} \u{1F600}\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{1F389}")
            } else {
                l.to_string()
            }
        })
        .collect();
    println!("corpus: {} bytes, {} paragraphs", corpus.len(), paras.len());
    scroll_case(&paras, 1.0);
    scroll_case(&paras, 2.0);
}
