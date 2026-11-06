//! The full-window view (issue #174): a modal overlay that shows one diagram or image at any
//! scale over a dimmed window. A surface opens it (an expand button, a double click) with what
//! to show; the view keeps its own references, so it needs nothing from the document, its pages
//! or the caches afterwards and can never change the document.
//!
//! - A diagram's scene is vector data: it is laid out for the shown scale (labels shaped at that
//!   size, curves flattened for it), so the picture stays sharp. Scales from a wheel or a pinch
//!   are snapped to steps, so a gesture shapes the labels at a limited number of sizes (each size
//!   fills the glyph atlas).
//! - An image is drawn from the document's texture, which is downscaled for a large image
//!   (`MAX_TEXTURE_SIDE`): the whole image is then decoded by one background worker ([`WholeImages`]: one decode at a
//!   time, the latest request wins, a closed view's decode is abandoned) and shown once
//!   the document's texture has too few pixels for the shown scale. One image point at scale 1 is
//!   one image pixel, as in the document.
//!
//! Nothing here animates: an untouched view schedules no repaint.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Mutex, MutexGuard};

use egui::{
    Color32, ColorImage, Context, CornerRadius, Event, Key, MouseWheelSource, MouseWheelUnit, Pos2, Rect, Sense,
    TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use markise_blocks::diagram::{DiagramPalette, DiagramResult, Paint, Role};
use markise_layout::block::{BlockLayout, color, diagram_scene_layout};

use super::MarkiseApp;
use crate::viewer::{ViewTarget, paint_layout, with_shaper};

/// Free space kept around the fitted diagram (points).
pub const FIT_MARGIN: f32 = 48.0;
/// A small diagram is enlarged at most this much to fit the window.
pub const MAX_FIT: f32 = 3.0;
/// The largest scale (a diagram whose fit is larger can still be shown fitted).
pub const MAX_SCALE: f32 = 8.0;
/// The most of the diagram panning always leaves in the window, per axis (points).
pub const KEEP_VISIBLE: f32 = 64.0;
/// Ratio between the scales a wheel or pinch can reach.
const SCALE_STEP: f32 = 1.04;
/// Zoom of one + or - press.
const KEY_ZOOM: f32 = 1.25;
/// Padding of a diagram's card (points): the backdrop starts outside it.
const CARD_PAD: f32 = 16.0;

/// How the content is placed in the window. `offset` is its origin relative to the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    size: Vec2,
    view: Vec2,
    fit: f32,
    /// The scale asked for, between snaps (small gesture steps add up).
    wanted: f32,
    scale: f32,
    offset: Vec2,
    /// Not moved since the fit: follows the window's size.
    fitted: bool,
}

impl ViewTransform {
    /// Fitted to `view`, for content of `size`.
    pub fn new(size: Vec2, view: Vec2) -> Self {
        let mut t = ViewTransform {
            size,
            view,
            fit: 1.0,
            wanted: 1.0,
            scale: 1.0,
            offset: Vec2::ZERO,
            fitted: true,
        };
        t.reset();
        t
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn offset(&self) -> Vec2 {
        self.offset
    }

    /// The scale that fits the window.
    pub fn fit(&self) -> f32 {
        self.fit
    }

    fn fit_for(size: Vec2, view: Vec2) -> f32 {
        let room = (view - Vec2::splat(2.0 * FIT_MARGIN)).max(Vec2::splat(1.0));
        (room.x / size.x).min(room.y / size.y).min(MAX_FIT)
    }

    /// The scales the view can reach.
    pub fn bounds(&self) -> (f32, f32) {
        (self.fit.min(1.0) * 0.5, self.fit.max(MAX_SCALE))
    }

    /// Back to the fit, centered.
    pub fn reset(&mut self) {
        self.fit = Self::fit_for(self.size, self.view);
        self.wanted = self.fit;
        self.scale = self.fit;
        self.offset = (self.view - self.size * self.fit) * 0.5;
        self.fitted = true;
    }

    /// The window's size (each frame).
    pub fn set_view(&mut self, view: Vec2) {
        if view == self.view {
            return;
        }
        self.view = view;
        if self.fitted {
            self.reset();
        } else {
            self.fit = Self::fit_for(self.size, view);
            let (lo, hi) = self.bounds();
            self.wanted = self.wanted.clamp(lo, hi);
            self.scale = self.scale.clamp(lo, hi);
            self.offset = self.clamped(self.offset);
        }
    }

    /// Zooms by `factor`, keeping the scene point under `at` (relative to the window) in place.
    pub fn zoom_about(&mut self, factor: f32, at: Vec2) {
        let (lo, hi) = self.bounds();
        self.wanted = (self.wanted * factor).clamp(lo, hi);
        let snapped = if self.wanted <= lo || self.wanted >= hi {
            self.wanted
        } else {
            SCALE_STEP
                .powf((self.wanted.ln() / SCALE_STEP.ln()).round())
                .clamp(lo, hi)
        };
        if snapped == self.scale {
            return;
        }
        let ratio = snapped / self.scale;
        self.scale = snapped;
        self.offset = self.clamped(at - (at - self.offset) * ratio);
        self.fitted = false;
    }

    /// Moves the diagram.
    pub fn pan(&mut self, delta: Vec2) {
        let o = self.clamped(self.offset + delta);
        if o != self.offset {
            self.offset = o;
            self.fitted = false;
        }
    }

    /// `offset` limited so part of the diagram stays in the window on each axis.
    fn clamped(&self, offset: Vec2) -> Vec2 {
        let axis = |o: f32, size: f32, view: f32| {
            let s = size * self.scale;
            let keep = KEEP_VISIBLE.min(s);
            let (lo, hi) = (keep - s, view - keep);
            o.clamp(lo.min(hi), hi.max(lo))
        };
        vec2(
            axis(offset.x, self.size.x, self.view.x),
            axis(offset.y, self.size.y, self.view.y),
        )
    }
}

/// What the laid-out picture depends on: scale, theme, fonts, device scale.
type LayoutKey = (u32, u64, u64, u32);

/// What the open view shows.
enum Content {
    Diagram {
        /// The view's own reference to the scene (always `Ok`).
        scene: DiagramResult,
        layout: Option<(LayoutKey, Arc<BlockLayout>)>,
        /// Times the picture was laid out (tests: an untouched view does not).
        layouts: u64,
    },
    Image {
        /// The document's texture (the view's own reference: an eviction does not take it).
        preview: Option<TextureHandle>,
        /// The whole image, when the document's texture is smaller.
        full: Option<TextureHandle>,
        /// The decode of the whole image in progress.
        decode: Option<WholeDecode>,
    },
}

/// The open view.
pub struct FullView {
    /// The document it was opened from (another document closes it).
    doc: u64,
    content: Content,
    transform: ViewTransform,
}

/// Decodes a source, giving up when the given check says the result is no longer wanted.
type DecodeFn = dyn Fn(&str, u32, &dyn Fn() -> bool) -> Result<ColorImage, String> + Send + Sync;

/// A decode waiting for the worker.
struct WholeJob {
    id: u64,
    src: String,
    max_side: u32,
    tx: Sender<Result<ColorImage, String>>,
    wake: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
struct WholeState {
    /// The request whose result is wanted (0: none).
    wanted: u64,
    last_id: u64,
    pending: Option<WholeJob>,
    /// The worker thread exists (it ends when nothing is pending).
    running: bool,
}

struct WholeShared {
    state: Mutex<WholeState>,
    decode: Box<DecodeFn>,
}

impl WholeShared {
    fn lock(&self) -> MutexGuard<'_, WholeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wanted(&self, id: u64) -> bool {
        self.lock().wanted == id
    }
}

/// The whole-image decoder of the full-window view: at most one decode runs, a new request
/// replaces the one waiting and makes the running one stale, and a stale decode stops at its next
/// step. Opening and dismissing a large image repeatedly therefore holds one image's memory.
pub struct WholeImages(Arc<WholeShared>);

impl Default for WholeImages {
    fn default() -> Self {
        Self::with_decoder(Box::new(|src, max_side, stale| {
            let budget = markise_blocks::MemBudget::new(markise_blocks::DECODE_BUDGET);
            let d = markise_blocks::decode_unless(src, max_side, Default::default(), &budget, stale)?;
            if stale() {
                return Err(markise_blocks::DECODE_STALE.into());
            }
            Ok(ColorImage::from_rgba_unmultiplied(d.pixels_size, &d.rgba))
        }))
    }
}

impl WholeImages {
    fn with_decoder(decode: Box<DecodeFn>) -> Self {
        WholeImages(Arc::new(WholeShared {
            state: Mutex::default(),
            decode,
        }))
    }

    /// Asks for `src`; `wake` runs after a wanted result was sent. Replaces any earlier request.
    fn request(&self, src: String, max_side: u32, wake: Box<dyn FnOnce() + Send>) -> WholeDecode {
        let (tx, rx) = channel();
        let mut st = self.0.lock();
        st.last_id += 1;
        let id = st.last_id;
        st.wanted = id;
        st.pending = Some(WholeJob {
            id,
            src,
            max_side,
            tx,
            wake,
        });
        if !st.running {
            let shared = self.0.clone();
            let spawned = std::thread::Builder::new()
                .name("markise-whole-image".into())
                .spawn(move || whole_worker(&shared));
            if spawned.is_ok() {
                st.running = true;
            } else {
                // (the sender goes with the job: the receiver reports it disconnected)
                st.pending = None;
            }
        }
        WholeDecode {
            rx,
            id,
            shared: self.0.clone(),
        }
    }
}

/// Lets the next request start a worker if this one dies in a decode.
struct WorkerAlive<'a>(&'a WholeShared);

impl Drop for WorkerAlive<'_> {
    fn drop(&mut self) {
        self.0.lock().running = false;
    }
}

fn whole_worker(shared: &WholeShared) {
    let alive = WorkerAlive(shared);
    loop {
        let job = {
            let mut st = shared.lock();
            match st.pending.take() {
                Some(j) => j,
                None => {
                    // (decided under the lock: a request after this starts a new worker)
                    st.running = false;
                    std::mem::forget(alive);
                    return;
                }
            }
        };
        let r = (shared.decode)(&job.src, job.max_side, &|| !shared.wanted(job.id));
        if shared.wanted(job.id) {
            let _published = crate::wake_track::publishing();
            let _ = job.tx.send(r);
            (job.wake)();
        }
    }
}

/// One request of [`WholeImages`]; dropping it (the view closed) abandons the decode.
pub struct WholeDecode {
    rx: Receiver<Result<ColorImage, String>>,
    id: u64,
    shared: Arc<WholeShared>,
}

impl Drop for WholeDecode {
    fn drop(&mut self) {
        let mut st = self.shared.lock();
        if st.wanted == self.id {
            st.wanted = 0;
            st.pending = None;
        }
    }
}

impl MarkiseApp {
    /// The transform of the open full-window view.
    pub fn full_view(&self) -> Option<ViewTransform> {
        self.full_view.as_ref().map(|v| v.transform)
    }

    /// Times the open view laid its diagram out (tests).
    #[doc(hidden)]
    pub fn full_view_layouts(&self) -> Option<u64> {
        match self.full_view.as_ref()?.content {
            Content::Diagram { layouts, .. } => Some(layouts),
            Content::Image { .. } => None,
        }
    }

    /// Pixel size of the largest texture of the open image view (tests).
    #[doc(hidden)]
    pub fn full_view_image_pixels(&self) -> Option<[usize; 2]> {
        match &self.full_view.as_ref()?.content {
            Content::Image { preview, full, .. } => full.as_ref().or(preview.as_ref()).map(TextureHandle::size),
            Content::Diagram { .. } => None,
        }
    }

    /// No decode of the open view is in progress.
    pub(super) fn full_view_settled(&self) -> bool {
        !matches!(
            &self.full_view,
            Some(FullView {
                content: Content::Image { decode: Some(_), .. },
                ..
            })
        )
    }

    /// Closes the full-window view; the keyboard focus goes back to where it was, or to the
    /// document.
    pub(super) fn close_full_view(&mut self) {
        if self.full_view.take().is_some()
            && self.focus_before_question.is_none()
            && let Some(d) = self.doc.as_mut()
        {
            d.viewer.request_refocus();
        }
    }

    fn open_full_view(&mut self, ctx: &Context, target: ViewTarget) {
        let Some(doc) = self.doc.as_ref().map(|d| d.id) else {
            return;
        };
        if self.is_asking() || self.insert_table.is_some() || self.section_form.is_some() {
            return;
        }
        let (content, size) = match target {
            ViewTarget::Diagram(scene) => {
                let size = match &*scene {
                    Ok(s) if s.width > 0.0 && s.height > 0.0 => vec2(s.width, s.height),
                    _ => return,
                };
                let content = Content::Diagram {
                    scene,
                    layout: None,
                    layouts: 0,
                };
                (content, size)
            }
            ViewTarget::Image { src, size } => {
                let preview = self.images.texture(&src);
                let whole = preview
                    .as_ref()
                    .is_some_and(|t| t.size()[0] as u32 >= size[0] && t.size()[1] as u32 >= size[1]);
                let content = Content::Image {
                    preview,
                    full: None,
                    decode: (!whole).then(|| {
                        // never above what the renderer takes (`load_texture` would panic)
                        let max_side = ctx.input(|i| i.max_texture_side) as u32;
                        let c = ctx.clone();
                        self.whole_images
                            .request(src, max_side, Box::new(move || c.request_repaint()))
                    }),
                };
                (content, vec2(size[0].max(1) as f32, size[1].max(1) as f32))
            }
        };
        // windows that hold on to the keyboard give way
        if self.cheat_sheet.open {
            self.cheat_sheet.close(ctx);
        }
        self.quick.open = false;
        self.full_view = Some(FullView {
            doc,
            content,
            transform: ViewTransform::new(size, ctx.content_rect().size()),
        });
        ctx.request_repaint();
    }

    /// Start of the frame: another document or an open question ends the view.
    pub(super) fn full_view_check(&mut self) {
        let Some(v) = &self.full_view else { return };
        if self.is_asking() || self.doc.as_ref().map(|d| d.id) != Some(v.doc) {
            self.close_full_view();
        }
    }

    /// The view is modal for the keyboard: after the shortcuts (Close and Quit still work), before
    /// anything underneath reads its keys, it takes all of them and acts on its own.
    pub(super) fn full_view_keys(&mut self, ctx: &Context) {
        let Some(v) = self.full_view.as_mut() else { return };
        let mut close = false;
        let center = ctx.content_rect().size() * 0.5;
        let t = &mut v.transform;
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    if *pressed && !modifiers.command && !modifiers.ctrl && !modifiers.alt {
                        match key {
                            Key::Escape => close = true,
                            Key::Plus | Key::Equals => t.zoom_about(KEY_ZOOM, center),
                            Key::Minus => t.zoom_about(1.0 / KEY_ZOOM, center),
                            Key::Num0 => t.reset(),
                            _ => {}
                        }
                    }
                    false
                }
                Event::Text(_) | Event::Ime(_) | Event::Copy | Event::Cut | Event::Paste(_) => false,
                _ => true,
            });
        });
        if close {
            self.close_full_view();
        }
    }

    /// Opens a view a surface asked for and draws the open one.
    pub(super) fn full_view_window(&mut self, ctx: &Context) {
        if let Some(target) = self.full_view_request.take() {
            self.open_full_view(ctx, target);
        }
        let Some(v) = self.full_view.as_mut() else { return };
        let screen = ctx.content_rect();
        v.transform.set_view(screen.size());
        // ---- the whole image arrived
        let mut failed = false;
        if let Content::Image { preview, full, decode } = &mut v.content
            && let Some(d) = decode
        {
            match d.rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(Ok(img)) => {
                    *full = Some(ctx.load_texture("markise-full-view", img, TextureOptions::LINEAR));
                    *decode = None;
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    // the document's texture stays, if there is one
                    failed = preview.is_none();
                    *decode = None;
                }
            }
        }
        if failed {
            self.close_full_view();
            return;
        }
        let theme = &self.theme;
        let images = &mut self.images;
        let mut parley = self.parley.as_mut();
        let font_gen = self.font_gen;
        let look = crate::dialog::Look::get(ctx);
        let id = egui::Id::unique("markise-full-view");
        let area = egui::Modal::default_area(id)
            .fade_in(false)
            .pivot(egui::Align2::LEFT_TOP)
            .fixed_pos(screen.min)
            .constrain(false);
        let close = egui::Modal::new(id)
            .area(area)
            .frame(egui::Frame::NONE)
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let resp = ui.interact(screen, id.with("surface"), Sense::click_and_drag());
                // ---- zoom: wheel and pinch, around the pointer; two fingers on a trackpad scroll
                // the picture like the document
                let (factor, scroll, pointer) =
                    ui.input(|i| (zoom_of(&i.events), trackpad_scroll(&i.events), i.pointer.latest_pos()));
                let at = pointer.unwrap_or(screen.center()) - screen.min;
                if factor != 1.0 {
                    v.transform.zoom_about(factor, at);
                }
                if scroll != Vec2::ZERO {
                    v.transform.pan(scroll);
                }
                if resp.dragged() {
                    v.transform.pan(resp.drag_delta());
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                let scale = v.transform.scale();
                let origin: Pos2 = screen.min + v.transform.offset();
                let shown = Rect::from_min_size(origin, v.transform.size * scale);
                let card = match v.content {
                    Content::Diagram { .. } => shown.expand(CARD_PAD),
                    Content::Image { .. } => shown,
                };
                // ---- the content, on the page's color (a diagram's colors and a transparent
                // image expect it)
                let painter = ui.painter_at(screen);
                let page = DiagramPalette::from_theme(theme).resolve(Paint::Role(Role::Background));
                painter.rect_filled(card, CornerRadius::same(if card == shown { 0 } else { 8 }), color(page));
                match &mut v.content {
                    Content::Diagram { scene, layout, layouts } => {
                        let Ok(sc) = &**scene else { return true };
                        let key = (
                            scale.to_bits(),
                            theme.fingerprint(),
                            font_gen,
                            ctx.pixels_per_point().to_bits(),
                        );
                        let laid = match layout {
                            Some((k, l)) if *k == key => l.clone(),
                            _ => {
                                let l = Arc::new(with_shaper(ctx, parley.as_deref_mut(), |sh| {
                                    diagram_scene_layout(sh, theme, sc, scale)
                                }));
                                *layout = Some((key, l.clone()));
                                *layouts += 1;
                                l
                            }
                        };
                        let mut requests = Vec::new();
                        paint_layout(&painter, ui, &laid, origin, images, &mut requests, parley);
                    }
                    Content::Image { preview, full, .. } => {
                        // the document's texture while it has the pixels for this scale (it is
                        // the smoother one when scaled down), then the whole image
                        let px = shown.width() * ctx.pixels_per_point();
                        let tex = match (&*preview, &*full) {
                            (Some(p), Some(f)) => {
                                if px <= p.size()[0] as f32 {
                                    Some(p)
                                } else {
                                    Some(f)
                                }
                            }
                            (p, f) => p.as_ref().or(f.as_ref()),
                        };
                        if let Some(t) = tex {
                            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                            painter.image(t.id(), shown, uv, Color32::WHITE);
                        }
                    }
                }
                // ---- close: the button (zoomed in, no backdrop may be left), a backdrop click
                let at = screen.right_top() + vec2(-36.0, 12.0);
                painter.rect_filled(
                    Rect::from_min_size(at, vec2(24.0, 24.0)),
                    CornerRadius::same(6),
                    look.fill,
                );
                let button = crate::dialog::close_button(ui, &look, at + vec2(4.0, 4.0));
                let outside = resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| !card.contains(p));
                button.clicked() || outside
            })
            .inner;
        if close {
            self.close_full_view();
        }
    }
}

/// The zoom factor of this frame's wheel and pinch events.
fn zoom_of(events: &[Event]) -> f32 {
    events
        .iter()
        .map(|e| match e {
            Event::Zoom(f) => *f,
            Event::MouseWheel { source, .. } if is_trackpad(*source) => 1.0,
            Event::MouseWheel { unit, delta, .. } => {
                let per = match unit {
                    MouseWheelUnit::Point => 0.0025,
                    MouseWheelUnit::Line => 0.12,
                    MouseWheelUnit::Page => 0.7,
                };
                (delta.y * per).exp()
            }
            _ => 1.0,
        })
        .product()
}

fn is_trackpad(source: MouseWheelSource) -> bool {
    matches!(source, MouseWheelSource::Trackpad | MouseWheelSource::Momentum)
}

/// This frame's two-finger scrolling (points).
fn trackpad_scroll(events: &[Event]) -> Vec2 {
    events
        .iter()
        .filter_map(|e| match e {
            Event::MouseWheel {
                source, unit, delta, ..
            } if is_trackpad(*source) => Some(match unit {
                MouseWheelUnit::Point => *delta,
                _ => *delta * 20.0,
            }),
            _ => None,
        })
        .fold(Vec2::ZERO, |a, b| a + b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A decoder whose runs wait for the gate, recording what ran and how many at once.
    #[derive(Default)]
    struct Probe {
        ran: Mutex<Vec<String>>,
        now: AtomicUsize,
        most: AtomicUsize,
        open: AtomicUsize,
        stale_seen: AtomicUsize,
    }

    fn probed() -> (WholeImages, Arc<Probe>) {
        let p = Arc::new(Probe::default());
        let q = p.clone();
        let w = WholeImages::with_decoder(Box::new(move |src, _, stale| {
            q.ran.lock().unwrap().push(src.to_string());
            let n = q.now.fetch_add(1, Ordering::SeqCst) + 1;
            q.most.fetch_max(n, Ordering::SeqCst);
            while q.open.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            q.now.fetch_sub(1, Ordering::SeqCst);
            if stale() {
                q.stale_seen.fetch_add(1, Ordering::SeqCst);
                return Err("stale".into());
            }
            Ok(ColorImage::new([1, 1], vec![Color32::WHITE]))
        }));
        (w, p)
    }

    fn until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(60), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn req(w: &WholeImages, src: &str) -> WholeDecode {
        w.request(src.into(), 16, Box::new(|| {}))
    }

    /// Codex review of #175: opening and dismissing a large image repeatedly must not pile up
    /// decodes: one runs, the requests in between are never decoded, only the latest answers.
    #[test]
    fn one_decode_runs_and_the_latest_request_wins() {
        let (w, p) = probed();
        let first = req(&w, "first");
        until("the first decode started", || p.now.load(Ordering::SeqCst) == 1);
        for n in 0..20 {
            drop(req(&w, &format!("dismissed {n}")));
        }
        let last = req(&w, "last");
        p.open.store(1, Ordering::SeqCst);
        let r = last
            .rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the latest answers");
        assert!(r.is_ok());
        assert_eq!(*p.ran.lock().unwrap(), ["first", "last"], "those in between never ran");
        assert_eq!(p.most.load(Ordering::SeqCst), 1, "one at a time");
        assert_eq!(p.stale_seen.load(Ordering::SeqCst), 1, "the first gave up");
        assert!(first.rx.try_recv().is_err(), "a stale result is not delivered");
    }

    #[test]
    fn a_closed_view_abandons_its_decode_and_the_worker_ends() {
        let (w, p) = probed();
        let d = req(&w, "running");
        until("started", || p.now.load(Ordering::SeqCst) == 1);
        drop(d);
        p.open.store(1, Ordering::SeqCst);
        until("the stale decode stopped", || p.stale_seen.load(Ordering::SeqCst) == 1);
        until("the worker ended", || !w.0.lock().running);
        // a later request starts a worker again
        let again = req(&w, "again");
        assert!(again.rx.recv_timeout(Duration::from_secs(60)).unwrap().is_ok());
    }

    fn t() -> ViewTransform {
        ViewTransform::new(vec2(2000.0, 400.0), vec2(1000.0, 700.0))
    }

    #[test]
    fn opens_fitted_and_centered_and_follows_the_window() {
        let mut d = t();
        let fit = (1000.0 - 2.0 * FIT_MARGIN) / 2000.0;
        assert_eq!(d.scale(), fit);
        assert_eq!(d.offset().x, FIT_MARGIN);
        assert_eq!(d.offset().y, (700.0 - 400.0 * fit) / 2.0);
        d.set_view(vec2(1500.0, 700.0));
        assert!(d.scale() > fit, "still fitted: follows the window");
        // a small diagram is enlarged only so far
        let small = ViewTransform::new(vec2(100.0, 50.0), vec2(1000.0, 700.0));
        assert_eq!(small.scale(), MAX_FIT);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut d = t();
        let at = vec2(300.0, 350.0);
        let before = (at - d.offset()) / d.scale();
        d.zoom_about(1.5, at);
        assert!(d.scale() > d.fit());
        let after = (at - d.offset()) / d.scale();
        assert!((before - after).length() < 0.01, "{before:?} {after:?}");
    }

    #[test]
    fn small_steps_add_up_to_a_snapped_scale() {
        let mut d = t();
        let start = d.scale();
        d.zoom_about(1.001, Vec2::ZERO);
        d.zoom_about(1.001, Vec2::ZERO);
        let mut scales = vec![d.scale()];
        for _ in 0..200 {
            d.zoom_about(1.001, Vec2::ZERO);
            if scales.last() != Some(&d.scale()) {
                scales.push(d.scale());
            }
        }
        assert!(d.scale() > start * 1.15, "{}", d.scale());
        assert!(scales.len() < 8, "snapped: few distinct sizes, {scales:?}");
    }

    #[test]
    fn scale_and_pan_are_clamped() {
        let mut d = t();
        for _ in 0..100 {
            d.zoom_about(2.0, vec2(500.0, 350.0));
        }
        assert_eq!(d.scale(), MAX_SCALE);
        for _ in 0..100 {
            d.zoom_about(0.5, vec2(500.0, 350.0));
        }
        assert_eq!(d.scale(), d.bounds().0);
        d.reset();
        d.pan(vec2(1.0e6, 1.0e6));
        assert_eq!(d.offset(), vec2(1000.0 - KEEP_VISIBLE, 700.0 - KEEP_VISIBLE));
        d.pan(vec2(-1.0e6, -1.0e6));
        let s = d.scale();
        assert_eq!(d.offset(), vec2(KEEP_VISIBLE - 2000.0 * s, KEEP_VISIBLE - 400.0 * s));
        d.reset();
        assert_eq!(d.scale(), d.fit());
    }
}
