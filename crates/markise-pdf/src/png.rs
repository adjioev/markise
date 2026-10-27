//! PNG back end: rasterises a continuous layout with vello_cpu. Tall documents are split into
//! tiles of at most [`MAX_TILE`] device pixels (one PNG per tile).

use std::sync::Arc;

use markise_export::job::{Cancelled, JobControl};
use markise_theme::Rgba;
use vello_cpu::color::AlphaColor;
use vello_cpu::kurbo::{Affine, BezPath, Rect, RoundedRect, Shape, Stroke};
use vello_cpu::peniko::ImageSampler;
use vello_cpu::{Image, ImageSource, PixelMetadata, Pixmap, RenderContext, Resources};

use crate::image::LoadedImage;
use crate::layout::{Item, Laid};

/// Total decoded image memory kept while rendering one tile.
pub const IMAGE_BUDGET: usize = 512 << 20;

/// Decoded pixmaps keyed by image identity.
#[derive(Default)]
struct ImageCache {
    entries: Vec<(*const LoadedImage, Option<Arc<Pixmap>>)>,
    bytes: usize,
    /// Images left out (undecodable or over the memory budget), reported to the user.
    omitted: Vec<String>,
}

fn pix_bytes(px: &Pixmap) -> usize {
    px.width() as usize * px.height() as usize * 4
}

impl ImageCache {
    /// Drops decoded images the current tile does not show.
    fn retain_only(&mut self, needed: &[&Arc<LoadedImage>]) {
        let keep: Vec<*const LoadedImage> = needed.iter().map(|i| Arc::as_ptr(i)).collect();
        let mut freed = 0;
        self.entries.retain(|(p, px)| {
            let k = keep.contains(p);
            if !k && let Some(px) = px {
                freed += pix_bytes(px);
            }
            k
        });
        self.bytes -= freed.min(self.bytes);
    }

    /// The image decoded at no more than its displayed size (`target` device pixels), or
    /// `None` (recorded in `omitted`) when it cannot be decoded or exceeds the budget.
    fn get(&mut self, img: &Arc<LoadedImage>, target: (u32, u32)) -> Option<Arc<Pixmap>> {
        let key = Arc::as_ptr(img);
        if let Some((_, px)) = self.entries.iter().find(|(p, _)| *p == key) {
            return px.clone();
        }
        let tw = target.0.clamp(1, img.width);
        let th = target.1.clamp(1, img.height);
        let need = tw as usize * th as usize * 4;
        let px = if self.bytes + need > IMAGE_BUDGET {
            self.omitted.push(format!(
                "an image ({}×{}) was left out of the PNG: decoded images exceed the {} MB budget",
                img.width,
                img.height,
                IMAGE_BUDGET >> 20
            ));
            None
        } else {
            let px = decode(img, tw, th);
            if px.is_none() {
                self.omitted
                    .push(format!("an image ({}×{}) could not be decoded", img.width, img.height));
            }
            px
        };
        if let Some(p) = &px {
            self.bytes += pix_bytes(p);
        }
        self.entries.push((key, px.clone()));
        px
    }
}

/// Decodes `img`, scaled down to `w`×`h` when smaller than the source (only one decoded copy
/// is kept).
fn decode(img: &LoadedImage, w: u32, h: u32) -> Option<Arc<Pixmap>> {
    let mut rgba = image::load_from_memory(&img.bytes).ok()?.into_rgba8();
    if (w, h) != rgba.dimensions() {
        rgba = image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle);
    }
    let (iw, ih) = (u16::try_from(rgba.width()).ok()?, u16::try_from(rgba.height()).ok()?);
    Some(Arc::new(Pixmap::from_parts(
        rgba.into_raw(),
        iw,
        ih,
        PixelMetadata {
            may_have_transparency: true,
            alpha_type: vello_cpu::peniko::ImageAlphaType::Alpha,
        },
    )))
}

/// Largest tile height in device pixels (vello_cpu sizes are `u16`).
pub const MAX_TILE: u32 = 16_000;

fn paint(c: Rgba) -> AlphaColor<vello_cpu::color::Srgb> {
    let [r, g, b, a] = c.0;
    AlphaColor::from_rgba8(r, g, b, a)
}

fn rect(r: &crate::layout::R) -> Rect {
    Rect::new(r.x0 as f64, r.y0 as f64, r.x1 as f64, r.y1 as f64)
}

/// Renders `laid` (its first page; a continuous layout has one) at `scale` device pixels per
/// layout unit; returns one PNG per tile, top to bottom, and warnings (images left out).
pub fn render_png(laid: &Laid, scale: f32, control: &JobControl) -> Result<(Vec<Vec<u8>>, Vec<String>), String> {
    let w = (laid.page_width * scale).ceil().clamp(1.0, 16_000.0) as u32;
    let total_h = (laid.page_height * scale).ceil().max(1.0) as u32;
    let mut out = Vec::new();
    let mut resources = Resources::new();
    let Some(page) = laid.pages.first() else {
        return Ok((out, Vec::new()));
    };
    // decoded images, shared by every reference to the same image, decoded only for the tiles
    // that show them and evicted when a tile no longer needs them (bounded memory)
    let mut cache = ImageCache::default();
    let tiles = total_h.div_ceil(MAX_TILE);
    for ti in 0..tiles {
        control.check().map_err(|_: Cancelled| "export cancelled".to_string())?;
        let y0 = ti * MAX_TILE;
        let th = (total_h - y0).min(MAX_TILE);
        let mut ctx = RenderContext::new(w as u16, th as u16);
        let base = Affine::translate((0.0, -(y0 as f64))) * Affine::scale(scale as f64);
        ctx.set_transform(Affine::IDENTITY);
        ctx.set_paint(paint(laid.page_bg));
        ctx.fill_rect(&Rect::new(0.0, 0.0, w as f64, th as f64));
        ctx.set_transform(base);
        let tile_top = y0 as f32 / scale;
        let tile_bottom = (y0 + th) as f32 / scale;
        let needed: Vec<&Arc<LoadedImage>> = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Image { rect: r, image, .. } if r.y1 >= tile_top && r.y0 <= tile_bottom => Some(image),
                _ => None,
            })
            .collect();
        cache.retain_only(&needed);
        for it in &page.items {
            match it {
                Item::Fill { rect: r, color, radius } => {
                    if r.y1 < tile_top || r.y0 > tile_bottom {
                        continue;
                    }
                    ctx.set_paint(paint(*color));
                    if *radius > 0.01 {
                        let rr = RoundedRect::from_rect(rect(r), *radius as f64);
                        ctx.fill_path(&rr.to_path(0.1));
                    } else {
                        ctx.fill_rect(&rect(r));
                    }
                }
                Item::Stroke {
                    rect: r,
                    color,
                    width,
                    radius,
                } => {
                    if r.y1 < tile_top || r.y0 > tile_bottom {
                        continue;
                    }
                    ctx.set_paint(paint(*color));
                    ctx.set_stroke(Stroke::new(*width as f64));
                    let path: BezPath = if *radius > 0.01 {
                        RoundedRect::from_rect(rect(r), *radius as f64).to_path(0.1)
                    } else {
                        rect(r).to_path(0.1)
                    };
                    ctx.stroke_path(&path);
                }
                Item::Line { from, to, color, width } => {
                    let mut p = BezPath::new();
                    p.move_to((from.0 as f64, from.1 as f64));
                    p.line_to((to.0 as f64, to.1 as f64));
                    ctx.set_paint(paint(*color));
                    ctx.set_stroke(Stroke::new(*width as f64));
                    ctx.stroke_path(&p);
                }
                Item::Text(t) => {
                    let Some(g0) = t.glyphs.first() else { continue };
                    if g0.y < tile_top - t.size * 2.0 || g0.y > tile_bottom + t.size * 2.0 {
                        continue;
                    }
                    ctx.set_paint(paint(t.color));
                    let coords: &[i16] = &t.coords;
                    let skew = t
                        .skew
                        .filter(|a| a.abs() > 0.01)
                        .map(|a| Affine::skew(-(a.to_radians().tan()) as f64, 0.0));
                    let glyphs = t.glyphs.iter().map(|g| vello_cpu::Glyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                    });
                    let mut b = ctx
                        .glyph_run(&mut resources, t.font.shared())
                        .font_size(t.size)
                        .normalized_coords(coords)
                        .hint(false);
                    if let Some(s) = skew {
                        b = b.glyph_transform(s);
                    }
                    let _ = b.fill_glyphs(glyphs.clone());
                    if t.embolden {
                        ctx.set_stroke(Stroke::new((t.size * 0.03) as f64));
                        let mut b = ctx
                            .glyph_run(&mut resources, t.font.shared())
                            .font_size(t.size)
                            .normalized_coords(coords)
                            .hint(false);
                        if let Some(s) = skew {
                            b = b.glyph_transform(s);
                        }
                        let _ = b.stroke_glyphs(glyphs);
                    }
                }
                Item::Image { rect: r, image, .. } => {
                    if r.y1 < tile_top || r.y0 > tile_bottom {
                        continue;
                    }
                    let target = ((r.w() * scale).ceil() as u32, (r.h() * scale).ceil() as u32);
                    let Some(px) = cache.get(image, target) else { continue };
                    let sx = r.w() as f64 / px.width() as f64;
                    let sy = r.h() as f64 / px.height() as f64;
                    ctx.set_paint(Image {
                        image: ImageSource::Pixmap(px),
                        sampler: ImageSampler::default(),
                    });
                    ctx.set_paint_transform(
                        Affine::translate((r.x0 as f64, r.y0 as f64)) * Affine::scale_non_uniform(sx, sy),
                    );
                    ctx.fill_rect(&rect(r));
                    ctx.reset_paint_transform();
                }
            }
        }
        ctx.flush();
        let mut pix = Pixmap::new(w as u16, th as u16);
        ctx.render(&mut pix, &mut resources);
        out.push(pix.into_png().map_err(|e| e.to_string())?);
        control.set_progress(0.7 + 0.3 * (ti + 1) as f32 / tiles as f32);
    }
    cache.omitted.dedup();
    Ok((out, cache.omitted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Arc<LoadedImage> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        Arc::new(LoadedImage::from_bytes(bytes).unwrap())
    }

    // Codex review P2: one decode per image, evicted when not needed, bounded.
    #[test]
    fn image_cache_shares_evicts_and_bounds() {
        let a = png(10, 10);
        let mut c = ImageCache::default();
        let p1 = c.get(&a, (100, 100)).unwrap();
        let p2 = c.get(&a, (100, 100)).unwrap();
        assert!(Arc::ptr_eq(&p1, &p2));
        assert_eq!(c.bytes, 400, "never upscaled");
        c.retain_only(&[]);
        assert_eq!(c.bytes, 0);
        assert!(c.entries.is_empty());
        // decoded at the displayed size
        let big = png(400, 200);
        let p = c.get(&big, (40, 20)).unwrap();
        assert_eq!((p.width(), p.height()), (40, 20));
        assert_eq!(c.bytes, 40 * 20 * 4);
        // over the budget: left out and reported
        c.bytes = IMAGE_BUDGET - 100;
        assert!(c.get(&png(10, 10), (10, 10)).is_none(), "over budget");
        assert_eq!(c.omitted.len(), 1);
    }
}
