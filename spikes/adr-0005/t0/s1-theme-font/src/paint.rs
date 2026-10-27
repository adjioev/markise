//! markise-text layouts → glyph atlas (vello_cpu) → egui textures + meshes (ADR 0005 §3 b).
//! Shared by S1 and S2 (`#[path]` include).

use atlas::{Atlas, Quad};
use egui::{Color32, ColorImage, Mesh, Pos2, Rect, Shape, TextureHandle, TextureOptions, pos2};
use markise_text::TextLayout;

pub struct TextPainter {
    pub atlas: Atlas,
    textures: Vec<TextureHandle>,
    quads: Vec<Quad>,
    pub upload_bytes: usize,
}

impl Default for TextPainter {
    fn default() -> Self {
        TextPainter {
            atlas: Atlas::new(1024),
            textures: Vec::new(),
            quads: Vec::new(),
            upload_bytes: 0,
        }
    }
}

impl TextPainter {
    /// Paints `layout` (laid out with scale = ppp, i.e. in pixels) at `origin` (points).
    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        layout: &TextLayout,
        origin: Pos2,
        ppp: f32,
        color: Color32,
    ) {
        self.quads.clear();
        let c = color.to_array();
        let rgba = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        let runs = layout.glyph_runs();
        let mut quads = std::mem::take(&mut self.quads);
        self.atlas.quads(
            &runs,
            ((origin.x * ppp).round(), (origin.y * ppp).round()),
            |_| rgba,
            &mut quads,
        );
        self.upload(ctx);
        let mut meshes: Vec<Mesh> = self.textures.iter().map(|t| Mesh::with_texture(t.id())).collect();
        for q in &quads {
            let size = self.atlas.page_size as f32;
            let col = if q.color == 0xFFFF_FFFF { Color32::WHITE } else { color };
            meshes[q.page].add_rect_with_uv(
                Rect::from_min_max(pos2(q.x0 / ppp, q.y0 / ppp), pos2(q.x1 / ppp, q.y1 / ppp)),
                Rect::from_min_max(
                    pos2(q.u0 as f32 / size, q.v0 as f32 / size),
                    pos2(q.u1 as f32 / size, q.v1 as f32 / size),
                ),
                col,
            );
        }
        self.quads = quads;
        for m in meshes {
            if !m.is_empty() {
                painter.add(Shape::mesh(m));
            }
        }
    }

    fn upload(&mut self, ctx: &egui::Context) {
        let size = self.atlas.page_size;
        for (i, p) in self.atlas.pages.iter_mut().enumerate() {
            if i >= self.textures.len() {
                let img = ColorImage::from_rgba_premultiplied([size, size], &p.pixels);
                self.upload_bytes += p.pixels.len();
                self.textures
                    .push(ctx.load_texture(format!("mk-atlas-{i}"), img, TextureOptions::NEAREST));
                p.dirty = None;
                continue;
            }
            if let Some((x0, y0, x1, y1)) = p.dirty.take() {
                let (w, h) = (x1 - x0, y1 - y0);
                let mut sub = Vec::with_capacity(w * h * 4);
                for y in y0..y1 {
                    sub.extend_from_slice(&p.pixels[4 * (y * size + x0)..4 * (y * size + x1)]);
                }
                self.upload_bytes += sub.len();
                self.textures[i].set_partial(
                    [x0, y0],
                    ColorImage::from_rgba_premultiplied([w, h], &sub),
                    TextureOptions::NEAREST,
                );
            }
        }
    }
}
