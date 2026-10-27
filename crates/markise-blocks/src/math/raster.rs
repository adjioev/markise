//! [`MathLayout`] → RGBA bitmap with `vello_cpu` (already in the tree for the parley text path).
//!
//! egui cannot fill non-convex paths (glyph outlines with holes, radical signs), so the viewer
//! shows formulas as small textures rasterised off the UI thread by [`super::MathService`].

use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{Affine, BezPath, Rect, Stroke};
use vello_cpu::{Pixmap, RenderContext, Resources};

use super::fonts::glyph_outline;
use super::{MathItem, MathLayout, PathCommand, Rgba8};

/// Largest bitmap side; bigger formulas are rendered at a smaller scale.
pub const MAX_SIDE: u32 = 4096;
/// Largest bitmap area in pixels.
pub const MAX_PIXELS: u64 = 8 << 20;

/// A rasterised formula.
#[derive(Clone, Debug)]
pub struct MathImage {
    /// The renderer's cache key of this bitmap (set by [`super::MathService`]; 0 otherwise).
    pub key: u64,
    /// Bitmap size in pixels (includes `pad` on every side).
    pub size: [u32; 2],
    /// Distance from the top of the bitmap to the baseline, in pixels.
    pub baseline: f32,
    /// Transparent padding around the ink box, in pixels (antialiasing room).
    pub pad: u32,
    /// Pixels per math em actually used (smaller than requested when the bitmap was capped).
    pub px_per_em: f32,
    /// Advance width / ascent / descent of the formula box in pixels (without padding).
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Straight-alpha RGBA8, row-major.
    pub rgba: Vec<u8>,
}

impl MathImage {
    pub fn approx_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.rgba.len()
    }
}

fn color(c: &Option<Rgba8>, default: Rgba8) -> AlphaColor<Srgb> {
    let [r, g, b, a] = c.unwrap_or(default);
    AlphaColor::from_rgba8(r, g, b, a)
}

fn bez(cmds: &[PathCommand], ox: f64, oy: f64, s: f64) -> BezPath {
    let mut p = BezPath::new();
    let pt = |x: f64, y: f64| (ox + x * s, oy + y * s);
    let mut open = false;
    for c in cmds {
        match *c {
            PathCommand::MoveTo { x, y } => {
                p.move_to(pt(x, y));
                open = true;
            }
            PathCommand::LineTo { x, y } if open => p.line_to(pt(x, y)),
            PathCommand::QuadTo { x1, y1, x, y } if open => p.quad_to(pt(x1, y1), pt(x, y)),
            PathCommand::CubicTo { x1, y1, x2, y2, x, y } if open => p.curve_to(pt(x1, y1), pt(x2, y2), pt(x, y)),
            PathCommand::Close if open => p.close_path(),
            _ => {}
        }
    }
    p
}

/// Transparent padding around a formula's bitmap, in pixels.
pub const PAD: u32 = 2;

/// The scale and bitmap size a formula box is rendered at — shared by [`rasterize`] and by
/// measuring ([`super::MathService::measure`]), so a measured size is always the drawn size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Pixels per em actually used: the requested scale, reduced to `max_width` and to the
    /// bitmap caps ([`MAX_SIDE`], [`MAX_PIXELS`]).
    pub px_per_em: f32,
    /// Bitmap size including [`PAD`] on every side.
    pub size: [u32; 2],
}

/// Fits a box of `width` × (`height` + `depth`) em at `px_per_em`, at most `max_width` pixels
/// wide and within the bitmap caps; `None` when the box is empty or unbounded.
pub fn fit(width: f64, height: f64, depth: f64, px_per_em: f32, max_width: Option<f32>) -> Option<Fit> {
    let (w, h, d) = (width.max(0.0), height.max(0.0), depth.max(0.0));
    let sane = |v: f64| v.is_finite() && v <= super::MAX_EM;
    if w <= 0.0 || h + d <= 0.0 || !(sane(w) && sane(h) && sane(d)) || !px_per_em.is_finite() || px_per_em <= 0.0 {
        return None;
    }
    let pad = PAD as f64;
    // all sizing in f64 (never overflows for bounded input), converted once at the end
    let mut ppe = (px_per_em as f64).min(10_000.0);
    if let Some(mw) = max_width.filter(|m| m.is_finite() && *m > 0.0) {
        ppe = ppe.min(mw as f64 / w);
    }
    let side = |ppe: f64| ((w * ppe).ceil() + 2.0 * pad, ((h + d) * ppe).ceil() + 2.0 * pad);
    let (sw, sh) = side(ppe);
    let over = (sw / MAX_SIDE as f64)
        .max(sh / MAX_SIDE as f64)
        .max((sw * sh / MAX_PIXELS as f64).sqrt());
    if over > 1.0 {
        ppe /= over * 1.01;
    }
    let (sw, sh) = side(ppe);
    if !(sw.is_finite() && sh.is_finite()) || ppe <= 0.0 {
        return None;
    }
    // f32 scale, so callers computing sizes from it get exactly what the raster uses
    let ppe = ppe as f32;
    let (sw, sh) = side(ppe as f64);
    Some(Fit {
        px_per_em: ppe,
        size: [
            sw.clamp(1.0, MAX_SIDE as f64) as u32,
            sh.clamp(1.0, MAX_SIDE as f64) as u32,
        ],
    })
}

/// Rasterises `layout` at `px_per_em` pixels per math em; items without an explicit TeX color
/// use `text`. `max_width`: the formula is scaled down to at most this many pixels wide (a
/// display formula wider than its column is rendered at the size it is shown, not larger).
/// Returns `None` for an empty formula or one whose size cannot be represented.
pub fn rasterize(layout: &MathLayout, px_per_em: f32, text: Rgba8, max_width: Option<f32>) -> Option<MathImage> {
    if layout.items.is_empty() {
        return None;
    }
    let (w, h, d) = (layout.width.max(0.0), layout.height.max(0.0), layout.depth.max(0.0));
    let pad = PAD;
    let Fit { px_per_em: ppe, size } = fit(layout.width, layout.height, layout.depth, px_per_em, max_width)?;
    let ppe = ppe as f64;
    let (bw, bh) = (size[0] as u16, size[1] as u16);

    let mut ctx = RenderContext::new(bw, bh);
    let o = pad as f64;
    let to_px = Affine::translate((o, o)) * Affine::scale(ppe);
    for item in &layout.items {
        match item {
            MathItem::Glyph {
                x,
                y,
                scale,
                face,
                ch,
                color: c,
            } => {
                let Some(g) = glyph_outline(*face, *ch) else { continue };
                ctx.set_transform(to_px);
                ctx.set_paint(color(c, text));
                ctx.fill_path(&bez(&g.commands, *x, *y, *scale));
            }
            MathItem::Missing {
                x, y, scale, color: c, ..
            } => {
                // placeholder box ("tofu") of 0.5 × 0.7 em
                ctx.set_transform(to_px);
                ctx.set_paint(color(c, text));
                ctx.set_stroke(Stroke::new(0.05 * scale));
                ctx.stroke_rect(&Rect::new(x + 0.05 * scale, y - 0.7 * scale, x + 0.5 * scale, *y));
            }
            MathItem::Rect {
                x,
                y,
                w,
                h,
                color: c,
                dashed,
            } => {
                ctx.set_transform(to_px);
                ctx.set_paint(color(c, text));
                // at least ~1 device pixel tall so thin rules never vanish
                let hh = h.max(0.8 / ppe);
                if *dashed {
                    // dashes and gaps of three rule thicknesses (as the SVG output), at least
                    // two device pixels each; bounded count
                    let dash = (3.0 * h).max(2.0 / ppe).max(w / 2048.0);
                    let mut x0 = *x;
                    while x0 < x + w {
                        ctx.fill_rect(&Rect::new(x0, *y, (x0 + dash).min(x + w), y + hh));
                        x0 += 2.0 * dash;
                    }
                } else {
                    ctx.fill_rect(&Rect::new(*x, *y, x + w, y + hh));
                }
            }
            MathItem::Path {
                x,
                y,
                commands,
                fill,
                color: c,
            } => {
                ctx.set_transform(to_px);
                ctx.set_paint(color(c, text));
                let p = bez(commands, *x, *y, 1.0);
                if *fill {
                    ctx.fill_path(&p);
                } else {
                    ctx.set_stroke(Stroke::new(0.04));
                    ctx.stroke_path(&p);
                }
            }
        }
    }
    ctx.flush();
    let mut pm = Pixmap::new(bw, bh);
    let mut res = Resources::new();
    ctx.render(&mut pm, &mut res);
    let rgba: Vec<u8> = pm
        .take_unpremultiplied()
        .into_iter()
        .flat_map(|p| [p.r, p.g, p.b, p.a])
        .collect();
    let ppe32 = ppe as f32;
    Some(MathImage {
        key: 0,
        size: [bw as u32, bh as u32],
        baseline: pad as f32 + (h * ppe) as f32,
        pad,
        px_per_em: ppe32,
        width: (w * ppe) as f32,
        ascent: (h * ppe) as f32,
        descent: (d * ppe) as f32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{MathMode, layout};
    use super::*;

    fn ink(img: &MathImage) -> usize {
        img.rgba.chunks(4).filter(|p| p[3] > 0).count()
    }

    #[test]
    fn rasterises_ink_inside_the_box() {
        let l = layout(r"\frac{x^2}{\sqrt{y}}", MathMode::Display).unwrap();
        let img = rasterize(&l, 40.0, [10, 20, 30, 255], None).unwrap();
        assert_eq!(img.rgba.len(), (img.size[0] * img.size[1] * 4) as usize);
        assert!(ink(&img) > 100);
        assert!(img.baseline > img.pad as f32 && img.baseline < img.size[1] as f32);
        // inherited color is applied
        assert!(img.rgba.chunks(4).any(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn huge_scale_is_capped() {
        let l = layout(r"\sum_{i=1}^{n} x_i", MathMode::Display).unwrap();
        let img = rasterize(&l, 10_000.0, [0, 0, 0, 255], None).unwrap();
        assert!(img.size[0] <= MAX_SIDE && img.size[1] <= MAX_SIDE);
        assert!((img.size[0] as u64 * img.size[1] as u64) <= MAX_PIXELS + 2 * MAX_SIDE as u64);
        assert!(img.px_per_em < 10_000.0);
    }

    #[test]
    fn max_width_scales_the_bitmap_down() {
        let l = layout(r"a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p", MathMode::Display).unwrap();
        let full = rasterize(&l, 40.0, [0, 0, 0, 255], None).unwrap();
        let fit = rasterize(&l, 40.0, [0, 0, 0, 255], Some(200.0)).unwrap();
        assert!(full.width > 200.0);
        assert!(
            fit.width <= 200.5 && fit.size[0] <= 205,
            "{} {}",
            fit.width,
            fit.size[0]
        );
        assert!(fit.px_per_em < 40.0);
    }

    #[test]
    fn oversized_layouts_are_refused_not_overflowed() {
        // review P2 #1: a hand-made layout far beyond the bounds must not panic or wrap
        let mut l = layout("x", MathMode::Inline).unwrap();
        l.width = 1e20;
        assert!(rasterize(&l, 20.0, [0, 0, 0, 255], None).is_none());
        l.width = f64::NAN;
        assert!(rasterize(&l, 20.0, [0, 0, 0, 255], None).is_none());
        l.width = 1500.0;
        l.height = 1500.0;
        let img = rasterize(&l, f32::MAX, [0, 0, 0, 255], None).unwrap();
        assert!(img.size[0] <= MAX_SIDE && img.size[1] <= MAX_SIDE);
    }

    /// Review round 5: `\hdashline` (and other dashed rules) are dashed in the bitmap too.
    #[test]
    fn dashed_rules_are_dashed() {
        let l = layout(r"\begin{array}{c}a\\\hdashline b\end{array}", MathMode::Display).unwrap();
        let Some((ry, rh)) = l.items.iter().find_map(|i| match i {
            MathItem::Rect { y, h, dashed: true, .. } => Some((*y, *h)),
            _ => None,
        }) else {
            panic!("no dashed rule in {l:?}")
        };
        let img = rasterize(&l, 40.0, [0, 0, 0, 255], None).unwrap();
        let row = (img.pad as f64 + (ry + rh / 2.0) * img.px_per_em as f64) as usize;
        let w = img.size[0] as usize;
        let ink: Vec<bool> = (0..w).map(|x| img.rgba[(row * w + x) * 4 + 3] > 128).collect();
        let runs = ink.windows(2).filter(|p| p[0] != p[1]).count();
        assert!(runs >= 6, "a dashed row alternates ink and gaps ({runs} transitions)");
        // a solid rule (\hline) stays solid
        let l = layout(r"\begin{array}{c}a\\\hline b\end{array}", MathMode::Display).unwrap();
        let (ry, rh) = l
            .items
            .iter()
            .find_map(|i| match i {
                MathItem::Rect {
                    y, h, w, dashed: false, ..
                } if *w > 0.5 => Some((*y, *h)),
                _ => None,
            })
            .unwrap();
        let img = rasterize(&l, 40.0, [0, 0, 0, 255], None).unwrap();
        let row = (img.pad as f64 + (ry + rh / 2.0) * img.px_per_em as f64) as usize;
        let w = img.size[0] as usize;
        let ink: Vec<bool> = (0..w).map(|x| img.rgba[(row * w + x) * 4 + 3] > 128).collect();
        assert!(ink.windows(2).filter(|p| p[0] != p[1]).count() <= 2);
    }

    #[test]
    fn empty_formula_has_no_image() {
        let l = layout("", MathMode::Inline).unwrap();
        assert!(rasterize(&l, 16.0, [0, 0, 0, 255], None).is_none());
    }
}
