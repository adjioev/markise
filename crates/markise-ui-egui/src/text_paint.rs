//! Painting of parley text blocks (ADR 0005 §3 option b, interim for T2/T3): a glyph atlas
//! rasterised with vello_cpu (the rasteriser epaint 0.37 itself depends on; its `text` feature
//! draws outlines, COLRv0/v1 and sbix/CBDT bitmaps), packed into 512² RGBA pages with a shelf
//! packer (§7.1 T0: 512² initial page), uploaded as egui textures and drawn as textured quads.
//!
//! Only used behind the developer switch (`MARKISE_TEXT=parley`, [`TextBackend::Parley`]); the
//! default epaint path never touches this module. Not yet done (T3): page eviction/growth
//! policy, per-frame quad batching across blocks, gamma/stem darkening matching egui's text.
//! Ported from the T0 spike (`spikes/adr-0005/t0/atlas`).

use std::collections::HashMap;

use egui::{Color32, ColorImage, Mesh, Pos2, Rect, Shape, TextureHandle, TextureOptions, pos2};
use markise_layout::ParleyShaper;
use markise_layout::text::ParleyBlock;
use markise_text::GlyphRun;
use skrifa::MetadataProvider;
use skrifa::raw::TableProvider;
use vello_cpu::{Pixmap, RenderContext, Resources};

/// Which text backend lays out and paints document text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextBackend {
    /// epaint galleys (default; M1 rendering).
    #[default]
    Epaint,
    /// parley via `markise-text`, painted through the glyph atlas (developer switch).
    Parley,
}

impl TextBackend {
    /// `MARKISE_TEXT=parley` selects parley; anything else epaint.
    pub fn from_env() -> Self {
        match std::env::var("MARKISE_TEXT").as_deref() {
            Ok("parley") => TextBackend::Parley,
            _ => TextBackend::Epaint,
        }
    }
}

/// Initial (and, for now, only) atlas page size.
pub const PAGE_SIZE: usize = 512;
const SUBPIXEL_BINS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    size_bits: u32,
    bin: u8,
    embolden: bool,
    skew_bits: u32,
    coords_hash: u64,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    page: usize,
    color: bool,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    /// Offset of the bitmap's top-left from the glyph origin (pixels).
    left: f32,
    top: f32,
}

struct Page {
    color: bool,
    /// RGBA8 premultiplied.
    pixels: Vec<u8>,
    shelf_y: usize,
    shelf_h: usize,
    cursor_x: usize,
    /// Dirty since the last upload (min x, min y, max x, max y).
    dirty: Option<(usize, usize, usize, usize)>,
    texture: Option<TextureHandle>,
}

impl Page {
    fn new(color: bool) -> Self {
        Page {
            color,
            pixels: vec![0; PAGE_SIZE * PAGE_SIZE * 4],
            shelf_y: 0,
            shelf_h: 0,
            cursor_x: 0,
            dirty: None,
            texture: None,
        }
    }

    fn alloc(&mut self, w: usize, h: usize) -> Option<(usize, usize)> {
        if w > PAGE_SIZE || h > PAGE_SIZE {
            return None;
        }
        if self.cursor_x + w > PAGE_SIZE {
            self.shelf_y += self.shelf_h + 1;
            self.shelf_h = 0;
            self.cursor_x = 0;
        }
        if self.shelf_y + h > PAGE_SIZE {
            return None;
        }
        let pos = (self.cursor_x, self.shelf_y);
        self.cursor_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        Some(pos)
    }
}

/// The glyph atlas.
pub struct GlyphAtlas {
    pages: Vec<Page>,
    map: HashMap<GlyphKey, Option<Entry>>,
    ctx: RenderContext,
    res: Resources,
    color_fonts: HashMap<(u64, u32), bool>,
    /// Glyphs rasterised so far (diagnostics, tests).
    pub rasterised: usize,
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        GlyphAtlas {
            pages: Vec::new(),
            map: HashMap::new(),
            ctx: RenderContext::new(64, 64),
            res: Resources::new(),
            color_fonts: HashMap::new(),
            rasterised: 0,
        }
    }
}

fn hash_coords(c: &[i16]) -> u64 {
    let mut h: u64 = 1_469_598_103_934_665_603;
    for v in c {
        h = (h ^ (*v as u16 as u64)).wrapping_mul(1_099_511_628_211);
    }
    h
}

impl GlyphAtlas {
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    fn is_color_font(&mut self, run: &GlyphRun) -> bool {
        let key = (run.font.id(), run.font.index());
        *self.color_fonts.entry(key).or_insert_with(|| {
            skrifa::FontRef::from_index(run.font.data(), run.font.index())
                .is_ok_and(|f| f.colr().is_ok() || f.sbix().is_ok() || f.cbdt().is_ok())
        })
    }

    fn place(&mut self, color: bool, w: usize, h: usize) -> Option<(usize, usize, usize)> {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if p.color == color
                && let Some((x, y)) = p.alloc(w, h)
            {
                return Some((i, x, y));
            }
        }
        let mut p = Page::new(color);
        let (x, y) = p.alloc(w, h)?;
        self.pages.push(p);
        Some((self.pages.len() - 1, x, y))
    }

    /// Rasterises (if needed) and returns the atlas entry of one glyph at pixel x `x`.
    fn glyph(&mut self, run: &GlyphRun, gid: u32, x: f32) -> Option<Entry> {
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
        let color = self.is_color_font(run)
            && skrifa::FontRef::from_index(run.font.data(), run.font.index()).is_ok_and(|f| {
                f.color_glyphs().get(skrifa::GlyphId::new(gid)).is_some()
                    || f.bitmap_strikes()
                        .glyph_for_size(skrifa::instance::Size::new(run.size), skrifa::GlyphId::new(gid))
                        .is_some()
            });
        // canvas: one em around the origin (bounds unknown for colour glyphs)
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
        self.rasterised += 1;
        let entry = if x1 <= x0 {
            None
        } else {
            let (w, h) = (x1 - x0, y1 - y0);
            self.place(color, w, h).map(|(page, px0, py0)| {
                let p = &mut self.pages[page];
                for y in 0..h {
                    for x in 0..w {
                        let s = 4 * ((y0 + y) * n + x0 + x);
                        let d = 4 * ((py0 + y) * PAGE_SIZE + px0 + x);
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
                Entry {
                    page,
                    color,
                    x: px0 as u16,
                    y: py0 as u16,
                    w: w as u16,
                    h: h as u16,
                    left: x0 as f32 - ox as f32 + bin as f32 / SUBPIXEL_BINS as f32,
                    top: y0 as f32 - oy as f32,
                }
            })
        };
        self.map.insert(key, entry);
        entry
    }

    /// Uploads new pages and dirty regions.
    fn upload(&mut self, ctx: &egui::Context) {
        for (i, p) in self.pages.iter_mut().enumerate() {
            match &mut p.texture {
                None => {
                    let img = ColorImage::from_rgba_premultiplied([PAGE_SIZE, PAGE_SIZE], &p.pixels);
                    p.texture = Some(ctx.load_texture(format!("markise-glyphs-{i}"), img, TextureOptions::NEAREST));
                    p.dirty = None;
                }
                Some(t) => {
                    if let Some((x0, y0, x1, y1)) = p.dirty.take() {
                        let (w, h) = (x1 - x0, y1 - y0);
                        let mut sub = Vec::with_capacity(w * h * 4);
                        for y in y0..y1 {
                            sub.extend_from_slice(&p.pixels[4 * (y * PAGE_SIZE + x0)..4 * (y * PAGE_SIZE + x1)]);
                        }
                        t.set_partial(
                            [x0, y0],
                            ColorImage::from_rgba_premultiplied([w, h], &sub),
                            TextureOptions::NEAREST,
                        );
                    }
                }
            }
        }
    }

    /// Paints `block` with its anchor at `origin` (points).
    pub fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter, block: &ParleyBlock, origin: Pos2) {
        let ppp = block.scale();
        // quads: (page, screen rect px, uv rect px, colour)
        let mut quads: Vec<(usize, [f32; 4], [u16; 4], Color32)> = Vec::new();
        let (ox, oy) = ((origin.x * ppp).round(), (origin.y * ppp).round());
        for run in block.glyph_runs() {
            let col = block.color(run.style);
            let dx = block.line_dx_px(run.line);
            for g in &run.glyphs {
                let gx = ox + dx + g.x;
                let gy = (oy + g.y).round();
                if let Some(e) = self.glyph(run, g.id, gx) {
                    let x0 = gx.floor() + e.left;
                    let y0 = gy + e.top;
                    quads.push((
                        e.page,
                        [x0, y0, x0 + e.w as f32, y0 + e.h as f32],
                        [e.x, e.y, e.x + e.w, e.y + e.h],
                        if e.color { Color32::WHITE } else { col },
                    ));
                }
            }
        }
        self.upload(ctx);
        let mut meshes: Vec<Option<Mesh>> = (0..self.pages.len()).map(|_| None).collect();
        let size = PAGE_SIZE as f32;
        for (page, r, uv, col) in quads {
            let Some(tex) = self.pages[page].texture.as_ref() else {
                continue;
            };
            let m = meshes[page].get_or_insert_with(|| Mesh::with_texture(tex.id()));
            m.add_rect_with_uv(
                Rect::from_min_max(pos2(r[0] / ppp, r[1] / ppp), pos2(r[2] / ppp, r[3] / ppp)),
                Rect::from_min_max(
                    pos2(uv[0] as f32 / size, uv[1] as f32 / size),
                    pos2(uv[2] as f32 / size, uv[3] as f32 / size),
                ),
                col,
            );
        }
        for m in meshes.into_iter().flatten() {
            painter.add(Shape::mesh(m));
        }
    }
}

/// The parley backend of the viewer: shaper plus glyph atlas.
pub struct ParleyText {
    pub shaper: ParleyShaper,
    pub atlas: GlyphAtlas,
}

impl ParleyText {
    pub fn new(defs: &egui::FontDefinitions) -> Self {
        ParleyText {
            shaper: ParleyShaper::from_font_definitions(defs),
            atlas: GlyphAtlas::default(),
        }
    }

    /// Fonts changed (theme): new font store; old atlas entries are keyed by blob ids that will
    /// no longer occur, so the atlas is reset too.
    pub fn set_fonts(&mut self, defs: &egui::FontDefinitions) {
        self.shaper.set_font_definitions(defs);
        self.atlas = GlyphAtlas::default();
    }
}
