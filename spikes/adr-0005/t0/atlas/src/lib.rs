//! Spike glyph atlas (ADR 0005 §3 option b) shared by S1 and S4: rasterises each
//! (font, glyph, size px, subpixel-x bin, synthesis, coords) once with vello_cpu's glyph renderer
//! (the rasteriser epaint 0.37 already uses; its `text` feature paints COLRv0/v1 via glifo and
//! sbix/CBDT bitmaps with `png`), packs it into RGBA pages with a shelf packer, and turns
//! markise-text glyph runs into textured quads. Mono glyphs are stored as white + coverage alpha
//! (tinted by the vertex colour), colour glyphs as premultiplied RGBA on separate pages.

use std::collections::HashMap;

use markise_text::GlyphRun;
use skrifa::MetadataProvider;
use skrifa::raw::TableProvider;
use vello_cpu::{Pixmap, RenderContext, Resources};

pub const SUBPIXEL_BINS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: u64,
    pub index: u32,
    pub glyph: u32,
    pub size_bits: u32,
    pub bin: u8,
    pub embolden: bool,
    pub skew_bits: u32,
    pub coords_hash: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub page: usize,
    pub color: bool,
    /// Rectangle in the page (pixels).
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Offset of the bitmap's top-left from the glyph origin (pixels).
    pub left: f32,
    pub top: f32,
}

pub struct Page {
    pub size: usize,
    pub color: bool,
    /// RGBA8 premultiplied.
    pub pixels: Vec<u8>,
    shelf_y: usize,
    shelf_h: usize,
    cursor_x: usize,
    /// Dirty since last upload (min x, min y, max x, max y).
    pub dirty: Option<(usize, usize, usize, usize)>,
    pub used_px: usize,
}

impl Page {
    fn new(size: usize, color: bool) -> Self {
        Page {
            size,
            color,
            pixels: vec![0; size * size * 4],
            shelf_y: 0,
            shelf_h: 0,
            cursor_x: 0,
            dirty: None,
            used_px: 0,
        }
    }

    fn alloc(&mut self, w: usize, h: usize) -> Option<(usize, usize)> {
        if w > self.size || h > self.size {
            return None;
        }
        if self.cursor_x + w > self.size {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_h = 0;
            self.cursor_x = 0;
        }
        if self.shelf_y + h > self.size {
            return None;
        }
        let pos = (self.cursor_x, self.shelf_y);
        self.cursor_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        self.used_px += w * h;
        Some(pos)
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub rasterised: usize,
    pub raster_ns: u128,
    pub lookups: usize,
}

/// A textured quad: screen rect (pixels), page, uv rect (pixels in page), colour.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub page: usize,
    pub u0: u16,
    pub v0: u16,
    pub u1: u16,
    pub v1: u16,
    pub color: u32,
}

pub struct Atlas {
    pub page_size: usize,
    pub pages: Vec<Page>,
    map: HashMap<GlyphKey, Option<Entry>>,
    ctx: RenderContext,
    res: Resources,
    color_fonts: HashMap<(u64, u32), bool>,
    pub stats: Stats,
}

fn hash_coords(c: &[i16]) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for v in c {
        h = (h ^ (*v as u16 as u64)).wrapping_mul(1099511628211);
    }
    h
}

impl Atlas {
    pub fn new(page_size: usize) -> Self {
        Atlas {
            page_size,
            pages: Vec::new(),
            map: HashMap::new(),
            ctx: RenderContext::new(64, 64),
            res: Resources::new(),
            color_fonts: HashMap::new(),
            stats: Stats::default(),
        }
    }

    pub fn mono_pages(&self) -> usize {
        self.pages.iter().filter(|p| !p.color).count()
    }
    pub fn color_pages(&self) -> usize {
        self.pages.iter().filter(|p| p.color).count()
    }

    fn is_color_font(&mut self, run: &GlyphRun) -> bool {
        let key = (run.font.id(), run.font.index());
        *self.color_fonts.entry(key).or_insert_with(|| {
            skrifa::FontRef::from_index(run.font.data(), run.font.index())
                .is_ok_and(|f| f.colr().is_ok() || f.sbix().is_ok() || f.cbdt().is_ok())
        })
    }

    fn place(&mut self, color: bool, w: usize, h: usize) -> (usize, usize, usize) {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if p.color == color
                && let Some((x, y)) = p.alloc(w, h)
            {
                return (i, x, y);
            }
        }
        let mut p = Page::new(self.page_size, color);
        let (x, y) = p.alloc(w, h).expect("glyph larger than a page");
        self.pages.push(p);
        (self.pages.len() - 1, x, y)
    }

    /// Rasterises (if needed) and returns the atlas entry of one glyph.
    pub fn glyph(&mut self, run: &GlyphRun, gid: u32, x: f32) -> Option<Entry> {
        self.stats.lookups += 1;
        let bin = ((x.fract() + 1.0).fract() * SUBPIXEL_BINS as f32) as u8 % SUBPIXEL_BINS as u8;
        let key = GlyphKey {
            font: run.font.id(),
            index: run.font.index(),
            glyph: gid,
            size_bits: run.size.to_bits(),
            bin,
            embolden: run.embolden,
            skew_bits: run.skew.unwrap_or(0.0).to_bits(),
            coords_hash: hash_coords(&run.coords),
        };
        if let Some(e) = self.map.get(&key) {
            return *e;
        }
        let t = std::time::Instant::now();
        let color = self.is_color_font(run)
            && skrifa::FontRef::from_index(run.font.data(), run.font.index()).is_ok_and(|f| {
                f.color_glyphs().get(skrifa::GlyphId::new(gid)).is_some()
                    || f.bitmap_strikes()
                        .glyph_for_size(skrifa::instance::Size::new(run.size), skrifa::GlyphId::new(gid))
                        .is_some()
            });
        // canvas: one em around the origin like epaint (bounds unknown for colour glyphs)
        let px = run.size.max(1.0);
        let canvas = (3.0 * px).ceil().clamp(4.0, 1024.0) as u16;
        let ox = px as f64 + bin as f64 / SUBPIXEL_BINS as f64;
        let oy = 2.0 * px as f64;
        self.ctx.reset_and_resize(canvas, canvas);
        self.ctx
            .set_paint(vello_cpu::color::OpaqueColor::<vello_cpu::color::Srgb>::WHITE);
        let coords = run.coords.clone();
        let mut b = self
            .ctx
            .glyph_run(&mut self.res, run.font.shared())
            .font_size(px)
            .hint(false)
            .normalized_coords(&coords);
        if let Some(skew) = run.skew {
            b = b.glyph_transform(vello_cpu::kurbo::Affine::skew((skew as f64).to_radians().tan(), 0.0));
        }
        b.fill_glyphs(std::iter::once(vello_cpu::Glyph {
            id: gid,
            x: ox as f32,
            y: oy as f32,
        }));
        let mut pm = Pixmap::new(canvas, canvas);
        self.ctx.render(&mut pm, &mut self.res);
        let n = canvas as usize;
        let data = pm.data_as_u8_slice();
        let (mut x0, mut y0, mut x1, mut y1) = (n, n, 0, 0);
        for y in 0..n {
            for x in 0..n {
                if data[4 * (y * n + x) + 3] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        self.stats.rasterised += 1;
        let entry = if x1 <= x0 {
            None
        } else {
            let (w, h) = (x1 - x0, y1 - y0);
            let (page, px0, py0) = self.place(color, w, h);
            let p = &mut self.pages[page];
            for y in 0..h {
                for x in 0..w {
                    let s = 4 * ((y0 + y) * n + x0 + x);
                    let d = 4 * ((py0 + y) * p.size + px0 + x);
                    if color {
                        p.pixels[d..d + 4].copy_from_slice(&data[s..s + 4]);
                    } else {
                        let a = data[s + 3];
                        p.pixels[d..d + 4].copy_from_slice(&[a, a, a, a]);
                    }
                }
            }
            p.dirty = Some(match p.dirty {
                None => (px0, py0, px0 + w, py0 + h),
                Some((a, b, c, d)) => (a.min(px0), b.min(py0), c.max(px0 + w), d.max(py0 + h)),
            });
            Some(Entry {
                page,
                color,
                x: px0 as u16,
                y: py0 as u16,
                w: w as u16,
                h: h as u16,
                left: x0 as f32 - ox as f32 + bin as f32 / SUBPIXEL_BINS as f32,
                top: y0 as f32 - oy as f32,
            })
        };
        self.stats.raster_ns += t.elapsed().as_nanos();
        self.map.insert(key, entry);
        entry
    }

    /// Quads for glyph runs placed at `origin` (pixels); colour from `color_of(style)`.
    pub fn quads(&mut self, runs: &[GlyphRun], origin: (f32, f32), color_of: impl Fn(u16) -> u32, out: &mut Vec<Quad>) {
        for run in runs {
            let col = color_of(run.style);
            for g in &run.glyphs {
                let gx = origin.0 + g.x;
                let gy = (origin.1 + g.y).round();
                if let Some(e) = self.glyph(run, g.id, gx) {
                    let x0 = gx.floor() + e.left;
                    let y0 = gy + e.top;
                    out.push(Quad {
                        x0,
                        y0,
                        x1: x0 + e.w as f32,
                        y1: y0 + e.h as f32,
                        page: e.page,
                        u0: e.x,
                        v0: e.y,
                        u1: e.x + e.w,
                        v1: e.y + e.h,
                        color: if e.color { 0xFFFF_FFFF } else { col },
                    });
                }
            }
        }
    }
}

/// CPU compositing of quads into an RGBA8 image (for screenshots): premultiplied source-over.
pub fn composite(quads: &[Quad], atlas: &Atlas, img: &mut [u8], w: usize, h: usize) {
    for q in quads {
        let p = &atlas.pages[q.page];
        let (r, g, b, a) = (
            (q.color >> 24) as u8 as f32 / 255.0,
            (q.color >> 16) as u8 as f32 / 255.0,
            (q.color >> 8) as u8 as f32 / 255.0,
            q.color as u8 as f32 / 255.0,
        );
        for yy in 0..(q.v1 - q.v0) as i32 {
            let dy = q.y0 as i32 + yy;
            if dy < 0 || dy >= h as i32 {
                continue;
            }
            for xx in 0..(q.u1 - q.u0) as i32 {
                let dx = q.x0 as i32 + xx;
                if dx < 0 || dx >= w as i32 {
                    continue;
                }
                let s = 4 * ((q.v0 as usize + yy as usize) * p.size + q.u0 as usize + xx as usize);
                let src = &p.pixels[s..s + 4];
                // tint: premultiplied texel × premultiplied vertex colour
                let sa = src[3] as f32 / 255.0 * a;
                let sr = src[0] as f32 / 255.0 * r * a;
                let sg = src[1] as f32 / 255.0 * g * a;
                let sb = src[2] as f32 / 255.0 * b * a;
                let d = 4 * (dy as usize * w + dx as usize);
                for (k, v) in [sr, sg, sb, sa].into_iter().enumerate() {
                    let dst = img[d + k] as f32 / 255.0;
                    img[d + k] = ((v + dst * (1.0 - sa)) * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}
