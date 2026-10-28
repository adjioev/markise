//! Software rasteriser shared by the headless screenshot examples: tessellated egui frames
//! to PNG (premultiplied alpha, textures sampled bilinearly). No window, no GPU.

use std::collections::HashMap;
use std::path::Path;

use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, ImageData, TextureId};

pub struct Raster {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>, // premultiplied, gamma space, 0..1
}

pub struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

impl Tex {
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let g = |x: usize, y: usize| {
            let c = self.px[y * self.w + x];
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
        };
        let (a, b, c, d) = (g(x0, y0), g(x1, y0), g(x0, y1), g(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            out[i] = (top + (bot - top) * fy) / 255.0;
        }
        out
    }
}

/// Applies (and consumes) a frame's texture updates (frees are ignored: a short-lived run).
pub fn apply_all(texs: &mut HashMap<TextureId, Tex>, delta: &mut egui::TexturesDelta) {
    for (id, deltas) in &delta.set {
        for d in deltas {
            apply_delta(texs, *id, d);
        }
    }
    delta.clear();
}

fn apply_delta(texs: &mut HashMap<TextureId, Tex>, id: TextureId, delta: &egui::epaint::ImageDelta) {
    let ImageData::Color(img) = &delta.image;
    let [w, h] = img.size;
    match delta.pos {
        None => {
            texs.insert(
                id,
                Tex {
                    w,
                    h,
                    px: img.pixels.clone(),
                },
            );
        }
        Some([x0, y0]) => {
            if let Some(t) = texs.get_mut(&id) {
                for y in 0..h {
                    for x in 0..w {
                        if x0 + x < t.w && y0 + y < t.h {
                            t.px[(y0 + y) * t.w + x0 + x] = img.pixels[y * w + x];
                        }
                    }
                }
            }
        }
    }
}

pub fn rasterize(prims: &[ClippedPrimitive], texs: &HashMap<TextureId, Tex>, ppp: f32, w: usize, h: usize) -> Raster {
    let mut r = Raster {
        w,
        h,
        px: vec![[0.0, 0.0, 0.0, 1.0]; w * h],
    };
    for cp in prims {
        let Primitive::Mesh(mesh) = &cp.primitive else { continue };
        let Some(tex) = texs.get(&mesh.texture_id) else {
            continue;
        };
        let clip = cp.clip_rect;
        let (cx0, cy0) = ((clip.min.x * ppp).max(0.0), (clip.min.y * ppp).max(0.0));
        let (cx1, cy1) = ((clip.max.x * ppp).min(w as f32), (clip.max.y * ppp).min(h as f32));
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let p: Vec<(f32, f32)> = v.iter().map(|v| (v.pos.x * ppp, v.pos.y * ppp)).collect();
            let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).max(cx0).floor() as i32;
            let maxx = p.iter().map(|q| q.0).fold(f32::MIN, f32::max).min(cx1).ceil() as i32;
            let miny = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).max(cy0).floor() as i32;
            let maxy = p.iter().map(|q| q.1).fold(f32::MIN, f32::max).min(cy1).ceil() as i32;
            for y in miny.max(0)..maxy.min(h as i32) {
                for x in minx.max(0)..maxx.min(w as i32) {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5);
                    if sx < cx0 || sx >= cx1 || sy < cy0 || sy >= cy1 {
                        continue;
                    }
                    let w0 = ((p[1].0 - sx) * (p[2].1 - sy) - (p[2].0 - sx) * (p[1].1 - sy)) / area;
                    let w1 = ((p[2].0 - sx) * (p[0].1 - sy) - (p[0].0 - sx) * (p[2].1 - sy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let t = tex.sample(u, vv);
                    let mut src = [0.0f32; 4];
                    for (i, s) in src.iter_mut().enumerate() {
                        let c = |k: usize| v[k].color.to_array()[i] as f32 / 255.0;
                        *s = (w0 * c(0) + w1 * c(1) + w2 * c(2)) * t[i];
                    }
                    let d = &mut r.px[y as usize * w + x as usize];
                    for i in 0..4 {
                        d[i] = src[i] + d[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    r
}

pub fn save_png(r: &Raster, path: &Path) {
    let mut img = image::RgbaImage::new(r.w as u32, r.h as u32);
    for (i, p) in r.px.iter().enumerate() {
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        img.put_pixel(
            (i % r.w) as u32,
            (i / r.w) as u32,
            image::Rgba([to8(p[0]), to8(p[1]), to8(p[2]), 255]),
        );
    }
    img.save(path).expect("write png");
}
