//! Image textures: decode requests go to the `markise-blocks` worker pool; decoded pixels are
//! uploaded as egui textures when the UI polls. Layout sees the natural size through the
//! [`Images`] trait; until then it lays out a placeholder.
//!
//! Bounded: textures live in a byte-budgeted LRU ([`TEXTURE_BUDGET`]); an evicted image keeps
//! its natural size (layout does not change) and is decoded again when it is painted. At
//! most [`MAX_UPLOADS_PER_FRAME`] textures are uploaded per frame. Every document switch bumps
//! a generation that cancels queued decodes and discards their late results.
//!
//! Rendered math (ADR 0008) goes through the same store: layout asks [`Images::math`], which
//! queries the `markise-blocks` [`MathService`] (background layout + rasterisation, its own
//! result cache); bitmaps are uploaded when first painted and kept in a byte-budgeted LRU
//! ([`MATH_TEXTURE_BUDGET`]). The service's wake callback sets a flag that
//! [`TextureStore::poll_math`] reports, so the view re-lays out math items once per frame.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{ColorImage, TextureHandle, TextureOptions};
use markise_blocks::ImageLoader;
use markise_blocks::math::{MathImageLookup, MathMeasure, MathMode, MathRequest, MathService};
use markise_layout::block::{ImageState, Images};
use markise_layout::{MathBox, MathQuery, MathState};

/// Bytes of texture pixels kept alive.
pub const TEXTURE_BUDGET: usize = 128 << 20;
pub const MAX_UPLOADS_PER_FRAME: usize = 4;
/// Bytes of formula bitmaps kept as textures.
pub const MATH_TEXTURE_BUDGET: usize = 32 << 20;
/// Formula textures uploaded per frame, and their bytes (one larger bitmap still goes alone);
/// the rest follow on the next frames (review P2 #3: conversion + upload runs on the UI thread).
pub const MAX_MATH_UPLOADS_PER_FRAME: usize = 8;
pub const MAX_MATH_UPLOAD_BYTES_PER_FRAME: usize = 4 << 20;

struct MathTex {
    tex: TextureHandle,
    bytes: usize,
    last_used: u64,
}

enum Entry {
    /// Queued or decoding; `size` is known when this is a reload of an evicted image.
    Loading {
        size: Option<[u32; 2]>,
    },
    Ready {
        tex: TextureHandle,
        size: [u32; 2],
        bytes: usize,
        last_used: u64,
    },
    Evicted {
        size: [u32; 2],
    },
    Failed(String),
}

pub struct TextureStore {
    loader: Option<ImageLoader>,
    loader_error: Option<String>,
    entries: HashMap<String, Entry>,
    /// Requests the bounded job queue did not accept yet.
    retry: VecDeque<String>,
    generation: u64,
    bytes: usize,
    budget: usize,
    frame: u64,
    math: Option<MathService>,
    math_error: Option<String>,
    /// Set by the math worker when a formula finished.
    math_done: Arc<AtomicBool>,
    math_tex: HashMap<u64, MathTex>,
    math_bytes: usize,
    /// Formula uploads (count, bytes) in the current frame.
    math_uploads: (usize, usize),
}

impl Default for TextureStore {
    fn default() -> Self {
        Self::with_budget(TEXTURE_BUDGET)
    }
}

impl TextureStore {
    pub fn with_budget(budget: usize) -> Self {
        TextureStore {
            loader: None,
            loader_error: None,
            entries: HashMap::new(),
            retry: VecDeque::new(),
            generation: 1,
            bytes: 0,
            budget,
            frame: 0,
            math: None,
            math_error: None,
            math_done: Arc::new(AtomicBool::new(false)),
            math_tex: HashMap::new(),
            math_bytes: 0,
            math_uploads: (0, 0),
        }
    }

    /// Starts the math worker (once).
    pub fn ensure_math(&mut self, ctx: &egui::Context) {
        if self.math.is_some() || self.math_error.is_some() {
            return;
        }
        let (c, done) = (ctx.clone(), self.math_done.clone());
        match MathService::new(Arc::new(move || {
            done.store(true, Ordering::Release);
            c.request_repaint();
        })) {
            Ok(m) => self.math = Some(m),
            Err(e) => self.math_error = Some(e),
        }
    }

    /// Whether formulas finished rendering since the last call (re-lay out math items).
    pub fn poll_math(&mut self) -> bool {
        self.math_done.swap(false, Ordering::AcqRel)
    }

    fn math_request(q: &MathQuery) -> MathRequest {
        MathRequest {
            tex: q.tex.to_string(),
            mode: if q.display { MathMode::Display } else { MathMode::Inline },
            px_per_em: q.px_per_em,
            color: q.color.to_srgba_unmultiplied(),
            max_width: q.max_width,
        }
    }

    /// Texture of a laid-out formula for painting (uploaded on first use; asked for again when
    /// the renderer's cache evicted it).
    pub fn math_texture(&mut self, ctx: &egui::Context, key: u64, q: &MathQuery) -> Option<TextureHandle> {
        let frame = self.frame;
        if let Some(t) = self.math_tex.get_mut(&key) {
            t.last_used = frame;
            return Some(t.tex.clone());
        }
        let MathImageLookup::Ready(img) = self.math.as_ref()?.get(&Self::math_request(q)) else {
            return None;
        };
        let (n, b) = self.math_uploads;
        if n >= MAX_MATH_UPLOADS_PER_FRAME || (n > 0 && b + img.rgba.len() > MAX_MATH_UPLOAD_BYTES_PER_FRAME) {
            ctx.request_repaint(); // the rest on the next frames
            return None;
        }
        self.math_uploads = (n + 1, b + img.rgba.len());
        let size = [img.size[0] as usize, img.size[1] as usize];
        let ci = ColorImage::from_rgba_unmultiplied(size, &img.rgba);
        let tex = ctx.load_texture(format!("math-{key:016x}"), ci, TextureOptions::LINEAR);
        let bytes = img.rgba.len();
        self.math_bytes += bytes;
        self.math_tex.insert(
            key,
            MathTex {
                tex: tex.clone(),
                bytes,
                last_used: frame,
            },
        );
        self.evict_math();
        Some(tex)
    }

    fn evict_math(&mut self) {
        if self.math_bytes <= MATH_TEXTURE_BUDGET {
            return;
        }
        let mut old: Vec<(u64, u64)> = self
            .math_tex
            .iter()
            .filter(|(_, t)| t.last_used < self.frame)
            .map(|(k, t)| (t.last_used, *k))
            .collect();
        old.sort_unstable();
        for (_, k) in old {
            if self.math_bytes <= MATH_TEXTURE_BUDGET * 3 / 4 {
                break;
            }
            if let Some(t) = self.math_tex.remove(&k) {
                self.math_bytes -= t.bytes;
            }
        }
    }

    /// Bytes of formula textures held.
    pub fn math_bytes(&self) -> usize {
        self.math_bytes
    }

    /// Bytes of uploaded texture pixels currently held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn texture_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| matches!(e, Entry::Ready { .. }))
            .count()
    }

    /// Queues `src` for decoding (once, or again after eviction).
    pub fn request(&mut self, ctx: &egui::Context, src: &str) {
        let size = match self.entries.get(src) {
            None => None,
            Some(Entry::Evicted { size }) => Some(*size),
            Some(_) => return,
        };
        if self.loader.is_none() && self.loader_error.is_none() {
            let c = ctx.clone();
            match ImageLoader::new(2, Arc::new(move || c.request_repaint())) {
                Ok(l) => {
                    l.set_generation(self.generation);
                    self.loader = Some(l);
                }
                Err(e) => self.loader_error = Some(e),
            }
        }
        if let Some(e) = &self.loader_error {
            self.entries.insert(src.to_string(), Entry::Failed(e.clone()));
            return;
        }
        self.entries.insert(src.to_string(), Entry::Loading { size });
        let accepted = self.loader.as_ref().is_some_and(|l| l.request(src, self.generation));
        if !accepted {
            self.retry.push_back(src.to_string());
        }
    }

    /// Advances the LRU clock; call once per frame before painting.
    pub fn begin_frame(&mut self) {
        self.math_uploads = (0, 0);
        self.frame += 1;
    }

    /// Uploads finished decodes (bounded per frame) and evicts over-budget textures; returns
    /// `true` when an image's layout-relevant state changed.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let Some(loader) = &self.loader else { return false };
        while let Some(src) = self.retry.front() {
            if loader.request(src, self.generation) {
                self.retry.pop_front();
            } else {
                break;
            }
        }
        let mut changed = false;
        let mut uploads = 0;
        while uploads < MAX_UPLOADS_PER_FRAME {
            let Some(d) = loader.try_recv() else { break };
            if d.generation != self.generation || !self.entries.contains_key(&d.src) {
                continue; // result for a closed document
            }
            let known = match self.entries.get(&d.src) {
                Some(Entry::Loading { size }) => *size,
                _ => None,
            };
            let entry = match d.result {
                Ok(img) => {
                    let bytes = img.pixels_size[0] * img.pixels_size[1] * 4;
                    let ci = ColorImage::from_rgba_unmultiplied(img.pixels_size, &img.rgba);
                    let tex = ctx.load_texture(&d.src, ci, TextureOptions::LINEAR);
                    self.bytes += bytes;
                    uploads += 1;
                    changed |= known != Some(img.size);
                    Entry::Ready {
                        tex,
                        size: img.size,
                        bytes,
                        last_used: self.frame,
                    }
                }
                Err(e) => {
                    changed = true;
                    Entry::Failed(e)
                }
            };
            self.entries.insert(d.src, entry);
        }
        if uploads == MAX_UPLOADS_PER_FRAME {
            ctx.request_repaint(); // more results may be waiting
        }
        self.evict();
        changed
    }

    /// Drops least-recently painted textures above the budget (never those painted in the
    /// current frame); their sizes are kept for layout.
    fn evict(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut ready: Vec<(u64, String)> = self
            .entries
            .iter()
            .filter_map(|(k, e)| match e {
                Entry::Ready { last_used, .. } if *last_used < self.frame => Some((*last_used, k.clone())),
                _ => None,
            })
            .collect();
        ready.sort();
        for (_, k) in ready {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(Entry::Ready { size, bytes, .. }) = self.entries.remove(&k) {
                self.bytes -= bytes;
                self.entries.insert(k, Entry::Evicted { size });
            }
        }
    }

    /// Texture for painting (marks it used this frame).
    pub fn texture(&mut self, src: &str) -> Option<TextureHandle> {
        let frame = self.frame;
        match self.entries.get_mut(src) {
            Some(Entry::Ready { tex, last_used, .. }) => {
                *last_used = frame;
                Some(tex.clone())
            }
            _ => None,
        }
    }

    /// Drops all textures and cancels pending decodes (document closed / switched).
    pub fn clear(&mut self) {
        self.generation += 1;
        if let Some(l) = &self.loader {
            l.set_generation(self.generation);
        }
        self.entries.clear();
        self.retry.clear();
        self.bytes = 0;
        if let Some(m) = &self.math {
            m.clear_queue();
        }
        self.math_tex.clear();
        self.math_bytes = 0;
    }
}

impl Images for TextureStore {
    fn state(&self, src: &str) -> ImageState {
        match self.entries.get(src) {
            Some(Entry::Ready { size, .. } | Entry::Evicted { size } | Entry::Loading { size: Some(size) }) => {
                ImageState::Ready { size: *size }
            }
            Some(Entry::Failed(e)) => ImageState::Failed(e.clone()),
            _ => ImageState::Pending,
        }
    }

    fn math(&self, q: &MathQuery) -> MathState {
        let Some(m) = &self.math else {
            return MathState::Unavailable;
        };
        let req = Self::math_request(q);
        if q.measure {
            return match m.measure(&req) {
                MathMeasure::Ready { width, ascent, descent } => MathState::Ready(MathBox {
                    key: 0,
                    size: [0, 0],
                    baseline: 0.0,
                    pad: 0.0,
                    width,
                    ascent,
                    descent,
                }),
                MathMeasure::Empty => MathState::Empty,
                MathMeasure::Failed(e) => MathState::Failed(e.to_string()),
                MathMeasure::Pending => MathState::Pending,
            };
        }
        match m.get(&req) {
            MathImageLookup::Ready(img) => MathState::Ready(MathBox {
                key: img.key,
                size: img.size,
                baseline: img.baseline,
                pad: img.pad as f32,
                width: img.width,
                ascent: img.ascent,
                descent: img.descent,
            }),
            MathImageLookup::Empty => MathState::Empty,
            MathImageLookup::Failed(e) => MathState::Failed(e.to_string()),
            MathImageLookup::Pending => MathState::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn math_uploads_are_bounded_per_frame() {
        let ctx = egui::Context::default();
        let mut store = TextureStore::default();
        store.ensure_math(&ctx);
        let queries: Vec<MathQuery> = (0..20)
            .map(|i| MathQuery {
                tex: format!("x_{{{i}}}+1").into(),
                display: false,
                px_per_em: 20.0,
                color: egui::Color32::BLACK,
                max_width: Some(600.0),
                measure: false,
            })
            .collect();
        let keys: Vec<u64> = queries
            .iter()
            .map(|q| {
                let t = Instant::now();
                loop {
                    if let MathState::Ready(b) = store.math(q) {
                        break b.key;
                    }
                    assert!(t.elapsed() < Duration::from_secs(20));
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
            .collect();
        store.begin_frame();
        let first = keys
            .iter()
            .zip(&queries)
            .filter(|(k, q)| store.math_texture(&ctx, **k, q).is_some())
            .count();
        assert_eq!(first, MAX_MATH_UPLOADS_PER_FRAME);
        let mut frames = 1;
        while keys
            .iter()
            .zip(&queries)
            .any(|(k, q)| store.math_texture(&ctx, *k, q).is_none())
        {
            store.begin_frame();
            frames += 1;
            assert!(frames < 10);
        }
        assert_eq!(frames, 3, "20 uploads at 8 per frame");
    }

    #[test]
    fn math_uploads_respect_the_byte_budget_per_frame() {
        let ctx = egui::Context::default();
        let mut store = TextureStore::default();
        store.ensure_math(&ctx);
        // big display formulas: each bitmap is well over a quarter of the per-frame byte budget
        let queries: Vec<MathQuery> = (0..6)
            .map(|i| MathQuery {
                tex: format!("\\sum_{{i={i}}}^n \\frac{{x_i^2}}{{y_i + {i}}}").into(),
                display: true,
                px_per_em: 400.0,
                color: egui::Color32::BLACK,
                max_width: None,
                measure: false,
            })
            .collect();
        let mut bytes = Vec::new();
        let keys: Vec<u64> = queries
            .iter()
            .map(|q| {
                let t = Instant::now();
                loop {
                    if let MathState::Ready(b) = store.math(q) {
                        bytes.push(b.size[0] as usize * b.size[1] as usize * 4);
                        break b.key;
                    }
                    assert!(t.elapsed() < Duration::from_secs(30));
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
            .collect();
        assert!(
            bytes.iter().all(|&b| b > MAX_MATH_UPLOAD_BYTES_PER_FRAME / 4),
            "{bytes:?}"
        );
        let mut frames = 0;
        let mut done = vec![false; keys.len()];
        while done.iter().any(|d| !d) {
            store.begin_frame();
            frames += 1;
            assert!(frames < 20);
            let mut sent = 0;
            for (i, (k, q)) in keys.iter().zip(&queries).enumerate() {
                if !done[i] && store.math_texture(&ctx, *k, q).is_some() {
                    done[i] = true;
                    sent += bytes[i];
                }
            }
            let (n, b) = store.math_uploads;
            assert_eq!(b, sent);
            // within the byte budget, unless a single bitmap went alone
            assert!(b <= MAX_MATH_UPLOAD_BYTES_PER_FRAME || n == 1, "{n} uploads, {b} bytes");
            assert!(
                n < MAX_MATH_UPLOADS_PER_FRAME,
                "the byte budget, not the count, limits this"
            );
        }
        assert!(frames > 1, "spread over several frames");
    }

    #[test]
    fn texture_lru_respects_the_budget_and_keeps_sizes() {
        let dir = std::env::temp_dir().join(format!("markise-ui-tex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        // budget for ~2 images of 64×64×4 = 16 KiB
        let mut store = TextureStore::with_budget(40 * 1024);
        let paths: Vec<String> = (0..6)
            .map(|i| {
                let p = dir.join(format!("i{i}.png"));
                image::RgbaImage::from_pixel(64, 64, image::Rgba([i as u8, 0, 0, 255]))
                    .save(&p)
                    .unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        for p in &paths {
            store.begin_frame();
            store.request(&ctx, p);
            let t = Instant::now();
            while !matches!(store.state(p), ImageState::Ready { .. }) {
                store.poll(&ctx);
                assert!(t.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(2));
            }
            let _ = store.texture(p);
        }
        store.begin_frame();
        store.poll(&ctx);
        assert!(store.bytes() <= 40 * 1024, "bytes {}", store.bytes());
        assert!(store.texture_count() <= 2);
        // evicted images still report their size, so layouts do not jump
        for p in &paths {
            assert_eq!(store.state(p), ImageState::Ready { size: [64, 64] });
        }
        // switching documents discards everything, including late results
        store.request(&ctx, &paths[0]);
        store.clear();
        assert_eq!(store.state(&paths[0]), ImageState::Pending);
        std::thread::sleep(Duration::from_millis(100));
        store.poll(&ctx);
        assert_eq!(store.state(&paths[0]), ImageState::Pending, "stale result discarded");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
