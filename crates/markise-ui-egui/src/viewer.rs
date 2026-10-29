//! The document surface: paints the visible items of the editor's [`View`], handles wheel /
//! keyboard / scrollbar scrolling, link clicks, the front-matter toggle and — when the
//! surface has keyboard focus — live-preview editing (ADR 0006 M2b): a blinking caret,
//! click / drag / shift-click / double- and triple-click selection, keyboard commands routed
//! through the engine keymap and command registry, typing, and marker reveal at the caret.
//!
//! Repaint discipline (NFR-10): without keyboard focus this widget never schedules a repaint;
//! with focus, the caret blinks (a repaint every half second) for [`BLINK_FOR`] seconds after
//! the last input and then stays solid, so an idle window goes quiet again.

use std::sync::Arc;
use std::time::Instant;

use egui::{
    Color32, CornerRadius, CursorIcon, Event, EventFilter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use markise_doc::{ContainerKind, DocPos, DocumentFormat, FormatDoc, Params, Selection, planners};
use markise_engine::editor::keymap::{Chord, Keymap, Platform};
use markise_engine::editor::{EditorSession, EditorView, TxKind};
use markise_layout::block::{BlockLayout, Prim, color};
use markise_layout::text::ParleyBlock;
use markise_layout::{DocModel, TextShaper, View};
use markise_theme::Theme;

use crate::images::TextureStore;
use crate::text_paint::ParleyText;

mod code_tools;
pub use code_tools::{LangEdit, LangItem, ShownTools, lang_matches};

const SCROLLBAR_W: f32 = 10.0;
/// Caret blink half-period (seconds).
const BLINK_HALF: f64 = 0.53;
/// The caret stops blinking (stays solid) this long after the last input (NFR-10).
pub const BLINK_FOR: f64 = 10.0;

/// What happened in the viewer this frame.
#[derive(Default, Debug)]
pub struct ViewerOutput {
    /// A link was opened (Cmd/Ctrl+click; target as written; `#anchor` targets are handled
    /// internally).
    pub link: Option<String>,
    /// Items painted this frame.
    pub painted: usize,
    /// Items laid out (cache misses) this frame.
    pub laid_out: usize,
    /// The document text or selection changed through input this frame.
    pub edited: bool,
    /// Keystroke processing time this frame (input → laid out and painted), when there was
    /// keyboard input (`MARKISE_TIMING`).
    pub input_ms: Option<f64>,
    /// A review command chosen in the document's context menu or the hover bar of a
    /// suggestion (ADR 0012): command id and the source offset it acts at (`None`: at the
    /// selection).
    pub review: Option<(&'static str, Option<usize>)>,
}

/// The suggestion under the pointer and its floating Accept / Reject bar (ADR 0012).
#[derive(Clone, Debug)]
struct HoverBar {
    mark: markise_doc::review::ReviewMark,
    /// The document revision the mark was found in: on any other revision the bar is hidden.
    counter: u64,
    /// Where the bar sits (screen, top left), fixed while the same mark is hovered.
    anchor: Pos2,
    /// The bar's rectangle at the last frame (the pointer may move onto it).
    rect: Option<Rect>,
}

/// A review comment shown in the visible part of the document (last frame), screen
/// coordinates and absolute source offsets (ADR 0012).
#[derive(Clone, Debug)]
pub struct ShownComment {
    /// The bubble glyph (revealed: the first row of the raw text).
    pub glyph: Rect,
    /// The whole mark and its text.
    pub source: std::ops::Range<usize>,
    pub content: std::ops::Range<usize>,
    pub text: Arc<str>,
    pub revealed: bool,
    /// Its card (margin card or open popover), when one was drawn.
    pub card: Option<Rect>,
}

/// The bubbles-mode popover (ADR 0012).
#[derive(Clone, Copy, Debug)]
struct Popover {
    /// Source start of the comment it shows.
    key: usize,
    /// When the pointer came onto the glyph (egui time).
    since: f64,
    /// Clicked: stays until Esc or another comment.
    pinned: bool,
}

/// The pointer must rest on a bubble this long before its popover opens.
pub const POPOVER_DELAY: f64 = 0.15;
/// Narrowest right margin that still holds comment cards.
const MIN_MARGIN: f32 = 190.0;

/// Commands of the document's context menu (ADR 0012), `None`: separator.
pub const CONTEXT_COMMANDS: &[Option<&str>] = &[
    Some("review.addComment"),
    None,
    Some("review.accept"),
    Some("review.reject"),
    Some("review.deleteComment"),
    None,
    Some("review.acceptAll"),
    Some("review.rejectAll"),
];

/// Selection granularity of a pointer drag (single, double, triple click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Block,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    /// The range of the initial click at its unit (word / block); a caret for chars.
    anchor: (DocPos, DocPos),
    unit: Unit,
}

pub struct DocViewer {
    /// The editing session and its view.
    pub ed: EditorView,
    /// Font generation the view was last told about.
    font_gen: u64,
    keymap: Keymap,
    platform: Platform,
    /// egui time of the last caret activity (blink phase, blink timeout).
    blink_from: f64,
    drag: Option<Drag>,
    /// A press on a task checkbox (item, list item ordinal) waiting for its release.
    task_press: Option<(usize, u16)>,
    /// Last primary press: time, position, click count.
    last_press: Option<(f64, Pos2, u32)>,
    /// Visible items of the last frame (index, top relative to the surface), for hit tests.
    last_visible: Vec<(usize, f32)>,
    /// Column left edge of the last frame (screen x).
    last_left: f32,
    /// Surface top of the last frame (screen y).
    last_top: f32,
    /// Text to put on the clipboard after this frame's input handling.
    pending_copy: Option<String>,
    /// Widget id and screen rectangle of the surface at the last frame.
    last_surface: Option<(egui::Id, Rect)>,
    /// Take the keyboard focus on the next frame (the surface came back after a view mode
    /// whose widgets held it, e.g. the source editor).
    refocus: bool,
    /// Screen rectangle of the caret at the last frame it was drawn (popups anchor to it).
    pub caret_screen: Option<Rect>,
    /// The hovered suggestion's action bar.
    hover: Option<HoverBar>,
    /// Last suggestion lookup under the pointer: (revision counter, offset, result).
    hover_cache: Option<(u64, usize, Option<markise_doc::review::ReviewMark>)>,
    /// Pointer position and revision counter of the last hover lookup.
    hover_key: Option<(Pos2, u64)>,
    /// How comments are shown (setting `markdown.commentDisplay`).
    comment_display: markise_engine::settings::CommentDisplay,
    /// Comments of the visible items at the last frame, with their cards.
    comments: Vec<ShownComment>,
    popover: Option<Popover>,
    /// Copy button and language field of fenced code blocks (issue #76).
    code: code_tools::CodeTools,
}

impl DocViewer {
    pub fn new(
        format: Arc<dyn DocumentFormat>,
        doc: Arc<dyn FormatDoc>,
        model: Arc<DocModel>,
        theme: &Theme,
        path: Option<std::path::PathBuf>,
    ) -> Self {
        let session = EditorSession::from_doc(
            format,
            doc.clone(),
            Default::default(),
            Box::new(markise_engine::editor::SystemClock),
        );
        let view = View::new(doc, model, theme.clone(), 600.0, 1.0);
        let ed = EditorView::new(session, view, path);
        let platform = Platform::current();
        let keymap = Keymap::new(&ed.registry, platform);
        DocViewer {
            ed,
            font_gen: 0,
            keymap,
            platform,
            blink_from: 0.0,
            drag: None,
            task_press: None,
            last_press: None,
            last_visible: Vec::new(),
            last_left: 0.0,
            last_top: 0.0,
            pending_copy: None,
            last_surface: None,
            refocus: false,
            caret_screen: None,
            hover: None,
            hover_cache: None,
            hover_key: None,
            comment_display: Default::default(),
            comments: Vec::new(),
            popover: None,
            code: Default::default(),
        }
    }

    /// A viewer of a parsed Markdown document (tests, tools).
    pub fn from_markdown(doc: Arc<markise_md::Document>, model: Arc<DocModel>, theme: &Theme) -> Self {
        Self::new(Arc::new(markise_md::MarkdownFormat::default()), doc, model, theme, None)
    }

    /// The surface's widget id at the last frame (it holds the keyboard focus while editing).
    pub fn surface_id(&self) -> Option<egui::Id> {
        self.last_surface.map(|(id, _)| id)
    }

    /// Asks the surface to take the keyboard focus on its next frame (after leaving Source or
    /// Side by Side, whose editor had it), so typing continues without a click.
    pub fn request_refocus(&mut self) {
        self.refocus = true;
    }

    /// The surface's screen rectangle at the last frame (document painting is clipped to it).
    pub fn surface_rect(&self) -> Option<Rect> {
        self.last_surface.map(|(_, r)| r)
    }

    /// The shared view (layout, scrolling).
    pub fn view(&mut self) -> &mut View {
        &mut self.ed.view
    }

    pub fn model(&self) -> &Arc<DocModel> {
        self.ed.view.model()
    }

    /// The document shown (the session's current document).
    pub fn doc(&self) -> &Arc<dyn FormatDoc> {
        self.ed.session.doc_arc()
    }

    /// Rebuilds the keymap from the registry defaults plus `keyBinding` overrides (label or
    /// id → accelerator); returns the problems found (unknown names, conflicts).
    pub fn set_key_bindings(&mut self, entries: &[(String, String)]) -> Vec<String> {
        let mut km = Keymap::new(&self.ed.registry, self.platform);
        let problems = km.apply_overrides(&self.ed.registry, entries, self.platform);
        self.keymap = km;
        problems
    }

    /// The keymap in use (menus show its chords).
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Replaces the document and its model without history (a new document).
    pub fn set_document(&mut self, doc: Arc<dyn FormatDoc>, model: Arc<DocModel>) {
        let format = self.ed.session.format().clone();
        let session = EditorSession::from_doc(
            format,
            doc.clone(),
            Default::default(),
            Box::new(markise_engine::editor::SystemClock),
        );
        let path = self.ed.path().map(|p| p.to_path_buf());
        let mut view = std::mem::replace(
            &mut self.ed.view,
            View::new(doc.clone(), model.clone(), Theme::light(), 600.0, 1.0),
        );
        view.set_model(doc, model);
        self.ed = EditorView::new(session, view, path);
        self.code = Default::default();
    }

    /// Installs `doc`/`model` as one undoable transaction `label` (reload from disk, a
    /// source-mode edit; `join`: append to the previous transaction of that label).
    pub fn install(
        &mut self,
        doc: Arc<dyn FormatDoc>,
        model: Arc<DocModel>,
        label: &'static str,
        join: bool,
    ) -> Result<(), markise_engine::editor::EditError> {
        self.ed.release();
        // the source pane's selection follows the replacement's diff (its live marks)
        let r = self
            .ed
            .session
            .replace_doc(doc.clone(), label, markise_engine::editor::TxKind::Bulk, join);
        let _ = self.ed.session.take_change();
        self.ed.view.set_model(doc, model);
        let caret = self.ed.session.selection().clone();
        let len = self.ed.session.text().len();
        if caret.primary().end().offset > len {
            self.ed.session.set_selection(Selection::caret(len));
        }
        // the source pane shows the new text; its selection is mapped through the diff and
        // snapped to caret positions
        if let Some(p) = self.ed.source.as_mut() {
            p.view.rebuild(self.ed.session.text());
        }
        self.ed.source_load_selection();
        r
    }

    /// Font generation the view was last told about.
    pub fn font_generation(&self) -> u64 {
        self.font_gen
    }

    pub fn set_font_generation(&mut self, g: u64) {
        if g != self.font_gen {
            self.font_gen = g;
            self.ed.view.set_font_generation(g);
        }
    }

    /// Scrolls so that heading `anchor` (`#slug`) is at the top.
    /// Screen centre of the first visible link whose target is `target` (last frame's
    /// geometry; tests click it).
    pub fn link_center(&self, target: &str) -> Option<Pos2> {
        for &(idx, y) in &self.last_visible {
            let Some(l) = self.ed.view.cache_peek(idx) else {
                continue;
            };
            for hit in l.links.iter().filter(|h| h.target == target) {
                let Some(markise_layout::Prim::Text { pos, text, .. }) = l.prims.get(hit.prim) else {
                    continue;
                };
                if let Some(sp) = text.ink_spans(&hit.range).first() {
                    let x = self.last_left + pos.x + (sp.x0 + sp.x1) * 0.5;
                    let yy = self.last_top + y + pos.y + sp.baseline - sp.ascent * 0.4;
                    return Some(pos2(x, yy));
                }
            }
        }
        None
    }

    pub fn scroll_to_anchor(&mut self, anchor: &str) -> bool {
        let item = self.model().heading_by_anchor(anchor).map(|e| e.item);
        if let Some(i) = item {
            self.ed.view.scroll_to_item(i);
            true
        } else {
            false
        }
    }

    /// Runs command `id` through the editor (menus, tests). `page`: viewport height.
    pub fn run(
        &mut self,
        ctx: &egui::Context,
        parley: Option<&mut ParleyText>,
        id: &str,
        params: &Params,
    ) -> Result<(), markise_engine::editor::command::CommandError> {
        let images = markise_layout::block::NoImages;
        let r = with_shaper(ctx, parley, |sh| self.ed.execute(id, params, sh, &images, 600.0));
        self.ed.refresh_focus();
        self.blink_from = ctx.input(|i| i.time);
        r
    }

    /// `parley`: lay out and paint document text with the parley backend instead of epaint
    /// (developer switch, ADR 0005 T2). `editable`: edits are accepted (otherwise caret and
    /// selection still work).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        images: &mut TextureStore,
        theme: &Theme,
        mut parley: Option<&mut ParleyText>,
        editable: bool,
    ) -> ViewerOutput {
        let mut out = ViewerOutput::default();
        let rect = ui.available_rect_before_wrap();
        let id = ui.scope_id().with("markise-viewer");
        let page_rect = Rect::from_min_max(rect.min, pos2(rect.right() - SCROLLBAR_W, rect.bottom()));
        let resp = ui.interact(page_rect, id, Sense::click_and_drag());
        self.last_surface = Some((id, rect));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, color(theme.page_bg));
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let started = Instant::now();

        let ppp = ctx.pixels_per_point();
        let content_w = rect.width() - SCROLLBAR_W;
        let col_w = (content_w - 2.0 * theme.column_padding_x)
            .min(theme.column_max_width)
            .max(120.0);
        let left = rect.left() + ((content_w - col_w) * 0.5).max(4.0);
        self.ed.view.set_params(col_w, theme, ppp);
        let vh = rect.height();

        if resp.clicked() || resp.drag_started() || std::mem::take(&mut self.refocus) {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                )
            });
        }

        // ---- input: wheel / trackpad
        if resp.hovered() || ui.rect_contains_pointer(rect) {
            let d = ui.input(|i| i.smooth_scroll_delta);
            if d.y != 0.0 {
                self.ed.view.scroll_by(-d.y);
                ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
            }
        }
        // ---- input: keyboard scrolling, only when no widget has focus
        if !focused && ui.memory(|m| m.focused().is_none()) {
            let (pd, pu, down, up, home, end, space) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::PageDown),
                    i.key_pressed(egui::Key::PageUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                    i.key_pressed(egui::Key::Space) && !i.modifiers.any(),
                )
            });
            if pd || space {
                self.ed.view.scroll_by(vh * 0.9);
            }
            if pu {
                self.ed.view.scroll_by(-vh * 0.9);
            }
            if down {
                self.ed.view.scroll_by(theme.line_px() * 2.0);
            }
            if up {
                self.ed.view.scroll_by(-theme.line_px() * 2.0);
            }
            if home {
                self.ed.view.scroll_to_y(0.0);
            }
            if end {
                self.ed.view.scroll_to_y(f64::MAX);
            }
        }
        // ---- input: scrollbar
        let bar = Rect::from_min_max(pos2(rect.right() - SCROLLBAR_W, rect.top()), rect.right_bottom());
        let bar_resp = ui.interact(bar, id.with("bar"), Sense::click_and_drag());
        let total = self.ed.view.total_height().max(1.0);
        if (bar_resp.dragged() || bar_resp.clicked())
            && let Some(p) = bar_resp.interact_pointer_pos()
        {
            let frac = ((p.y - rect.top()) / vh).clamp(0.0, 1.0) as f64;
            self.ed.view.scroll_to_y(frac * (total - vh as f64).max(0.0));
        }

        // ---- input: keyboard editing
        let mut follow = false;
        if focused {
            let events = ui.input(|i| i.events.clone());
            let mut ran = false;
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                for e in events {
                    ran |= self.handle_event(e, sh, &*images, editable, vh);
                }
            });
            if let Some(t) = self.pending_copy.take() {
                ctx.copy_text(t);
            }
            if ran {
                follow = true;
                out.edited = true;
            }
        }

        // ---- input: pointer (hit tests against the last frame's layout)
        let modifiers = ui.input(|i| i.modifiers);
        let pressed = ui.input(|i| i.pointer.primary_pressed()) && resp.hovered();
        // a click on a task checkbox toggles it (spec 02 §9): the press neither places the caret
        // nor starts a selection; the toggle happens on release over the same box (a press
        // dragged off the box does nothing)
        let task = if pressed {
            ui.input(|i| i.pointer.interact_pos())
                .and_then(|p| self.task_box_at(p, rect.top()))
        } else {
            None
        };
        if task.is_some() {
            self.task_press = task;
        }
        if let Some(t) = self.task_press
            && ui.input(|i| i.pointer.primary_released())
        {
            self.task_press = None;
            let over = ui
                .input(|i| i.pointer.interact_pos())
                .and_then(|p| self.task_box_at(p, rect.top()));
            if editable && over == Some(t) && self.toggle_task(t.0, t.1) {
                out.edited = true;
            }
        }
        // a press on a comment's bubble or card opens the comment for editing (ADR 0012)
        let comment_press = if pressed && task.is_none() {
            ui.input(|i| i.pointer.interact_pos()).and_then(|p| self.comment_at(p))
        } else {
            None
        };
        if let Some(i) = comment_press {
            self.open_comment(i, true);
            out.edited = true;
            self.blink_from = now;
        }
        if pressed
            && task.is_none()
            && comment_press.is_none()
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
            && !self.on_toggle_or_link(p, rect.top(), modifiers)
        {
            let count = match self.last_press {
                Some((t, q, n)) if now - t < 0.45 && q.distance(p) < 5.0 => (n % 3) + 1,
                _ => 1,
            };
            self.last_press = Some((now, p, count));
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                if let Some(pos) = self.hit_screen(p, rect.top(), sh, &*images) {
                    let unit = match count {
                        2 => Unit::Word,
                        3 => Unit::Block,
                        _ => Unit::Char,
                    };
                    let (a, b) = self.unit_range(pos, unit, sh, &*images);
                    // shift-click extends from the retained anchor, and a drag that follows
                    // keeps extending from it
                    let (sel, anchor) = if modifiers.shift && unit == Unit::Char {
                        let an = self.ed.session.selection().primary().anchor;
                        (Selection::single(an, pos), (an, an))
                    } else {
                        (Selection::single(a, b), (a, b))
                    };
                    // the reveal state is frozen while the pointer selects (ADR 0006 §2.4)
                    self.ed.reveal = false;
                    self.ed.set_selection(sel);
                    self.drag = Some(Drag { anchor, unit });
                }
            });
            out.edited = true;
            self.blink_from = now;
        }
        if let Some(d) = self.drag {
            let down = ui.input(|i| i.pointer.primary_down());
            if down {
                if let Some(p) = ui.input(|i| i.pointer.interact_pos())
                    && (resp.dragged() || resp.is_pointer_button_down_on())
                {
                    // auto-scroll when dragging past the edges
                    if p.y < rect.top() {
                        self.ed.view.scroll_by((p.y - rect.top()).max(-40.0));
                        ctx.request_repaint();
                    } else if p.y > rect.bottom() {
                        self.ed.view.scroll_by((p.y - rect.bottom()).min(40.0));
                        ctx.request_repaint();
                    }
                    with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                        if let Some(pos) = self.hit_screen(p, rect.top(), sh, &*images) {
                            let (a, b) = self.unit_range(pos, d.unit, sh, &*images);
                            let (s0, s1) = d.anchor;
                            let sel = if pos.key() < s0.key() {
                                Selection::single(s1, a)
                            } else if d.unit == Unit::Char {
                                Selection::single(s0, pos)
                            } else {
                                Selection::single(s0, b)
                            };
                            if &sel != self.ed.session.selection() {
                                self.ed.set_selection(sel);
                                out.edited = true;
                            }
                        }
                    });
                }
            } else {
                self.drag = None;
                self.ed.reveal = true;
                out.edited = true;
            }
        }

        if out.edited {
            self.ed.refresh_focus();
            self.blink_from = now;
        }
        if follow {
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                self.ed.scroll_to_caret(vh, sh, &*images)
            });
        }

        // ---- layout of the viewport (+ half a screen of margin)
        let visible = {
            let view = &mut self.ed.view;
            let imgs: &TextureStore = images;
            match parley.as_deref_mut() {
                Some(p) => {
                    p.shaper.set_scale(ppp);
                    view.frame(vh, vh * 0.5, &mut p.shaper, imgs)
                }
                None => ctx.fonts_mut(|f| view.frame(vh, vh * 0.5, f, imgs)),
            }
        };
        // the available caret stops may have changed under the selection (a diagram or
        // formula became rendered, front matter collapsed): move it to the nearest stop
        // (not on a read-only surface without the focus: in Side by Side the session's selection
        // may be the source pane's, which need not sit on a rendered caret stop)
        if (focused || editable) && with_shaper(&ctx, parley.as_deref_mut(), |sh| self.ed.fix_selection(sh, &*images)) {
            self.ed.refresh_focus();
            ctx.request_repaint();
        }
        out.painted = visible.len();
        out.laid_out = self.ed.view.laid_out.len();
        self.last_visible = visible.iter().map(|v| (v.idx, v.y)).collect();
        self.last_left = left;
        self.last_top = rect.top();

        // ---- paint
        images.begin_frame();
        let mut requests = Vec::new();
        for v in &visible {
            let origin = pos2(left, rect.top() + v.y);
            paint_layout(
                &painter,
                ui,
                &v.layout,
                origin,
                images,
                &mut requests,
                parley.as_deref_mut(),
            );
        }
        for src in requests {
            images.request(&ctx, &src);
        }
        // ---- code block tools (copy button, language field)
        {
            let imgs: &TextureStore = images;
            let mut sh = |f: &mut dyn FnMut(&mut dyn TextShaper, &dyn markise_layout::Images)| {
                with_shaper(&ctx, parley.as_deref_mut(), |s| f(s, imgs))
            };
            self.code_tools_ui(ui, &visible, rect, left, theme, focused, editable, &mut out, &mut sh);
        }

        // ---- review comments: margin cards or bubble popovers (visible items only)
        self.comment_cards(ui, &visible, rect, left, col_w, theme);

        // ---- selection and caret
        let sel = self.ed.session.selection().primary();
        if !sel.is_caret() {
            let range = sel.range();
            let fill = color(theme.chrome.selection_bg).gamma_multiply(0.55);
            // rectangles first: painting inside `fonts_mut` would deadlock on the context lock
            let rects = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                let model = self.ed.view.model().clone();
                let mut out = Vec::new();
                for v in &visible {
                    let span = model.item_span(v.idx);
                    if span.end < range.start || span.start > range.end {
                        continue;
                    }
                    let mut g = self.ed.geometry(sh, &*images);
                    for r in g.selection_rects(v.idx, range.clone()) {
                        out.push(r.translate(vec2(left, rect.top() + v.y)));
                    }
                }
                out
            });
            for r in rects {
                painter.rect_filled(r, 0.0, fill);
            }
        }
        if focused {
            let since = now - self.blink_from;
            let on = since >= BLINK_FOR || ((since / BLINK_HALF) as u64).is_multiple_of(2);
            let caret = with_shaper(&ctx, parley.as_deref_mut(), |sh| self.ed.caret_rect(sh, &*images));
            self.caret_screen = None;
            if let Some(c) = caret {
                let y = c.min.y - self.ed.view.scroll_y() as f32 + rect.top();
                let r = Rect::from_min_size(pos2(left + c.min.x - 0.75, y), vec2(1.5, c.height().max(4.0)));
                self.caret_screen = Some(r);
                if on && r.intersects(rect) {
                    painter.rect_filled(r, 0.0, color(theme.text));
                }
                // IME: allowed while the surface has focus; the candidate window follows the
                // caret (preedit display is M2e, ADR 0006 §6.3)
                ctx.output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        purpose: Default::default(),
                        rect,
                        cursor_rect: r,
                        should_interrupt_composition: false,
                    })
                });
            }
            if since < BLINK_FOR {
                let phase = BLINK_HALF - (since % BLINK_HALF);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(phase.max(0.02)));
            }
        }
        // the I-beam only while the pointer is over the surface itself (not over the sidebar,
        // the toolbar or a window on top of it: `hovered` honours layers; issue #36)
        if resp.hovered() {
            ctx.set_cursor_icon(CursorIcon::Text);
        }

        // ---- pointer: link / toggle hover
        if let Some(p) = resp.hover_pos()
            && let Some((_, local, layout)) = hit(&visible, p, left, rect.top())
        {
            let link = layout.link_at(local).is_some() && (modifiers.command || !focused);
            let on_toggle = layout.toggle.is_some_and(|t| t.contains(local));
            let on_task = editable && task_box_hit(layout, local).is_some();
            if link || on_toggle || on_task {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((idx, local, layout)) = hit(&visible, p, left, rect.top())
        {
            if layout.toggle.is_some_and(|t| t.contains(local)) {
                self.ed.view.toggle_expanded(idx);
                ctx.request_repaint();
            } else if modifiers.command
                && let Some(target) = layout.link_at(local).map(str::to_string)
            {
                if target.starts_with('#') {
                    if self.scroll_to_anchor(&target) {
                        ctx.request_repaint();
                    }
                } else {
                    out.link = Some(target);
                }
            }
        }
        if out.edited {
            // the reveal set of the new caret position is laid out next frame
            ctx.request_repaint();
        }

        // ---- review: context menu and the hover bar of suggestions (ADR 0012)
        if editable {
            self.review_ui(ui, &resp, rect, parley, images, &mut out);
        }

        // ---- scrollbar
        let total = self.ed.view.total_height().max(1.0);
        if total > vh as f64 + 1.0 {
            let y = self.ed.view.scroll_y();
            let thumb_h = ((vh as f64 / total) * vh as f64).max(24.0) as f32;
            let frac = (y / (total - vh as f64).max(1.0)).clamp(0.0, 1.0) as f32;
            let top = rect.top() + frac * (vh - thumb_h);
            let thumb = Rect::from_min_size(pos2(bar.left() + 2.0, top), vec2(SCROLLBAR_W - 4.0, thumb_h));
            let c = color(theme.meta_text).gamma_multiply(if bar_resp.hovered() || bar_resp.dragged() {
                0.8
            } else {
                0.45
            });
            painter.rect_filled(thumb, CornerRadius::same(3), c);
        }
        if follow {
            out.input_ms = Some(started.elapsed().as_secs_f64() * 1e3);
        }
        out
    }

    /// Screen rectangle (last frame) of the visible item holding source offset `offset`:
    /// tests click into list items, paragraphs… through it.
    pub fn item_screen_rect(&self, offset: usize) -> Option<Rect> {
        let model = self.ed.view.model();
        let i = model.item_at_offset(offset);
        let &(_, y) = self.last_visible.iter().find(|(idx, _)| *idx == i)?;
        let l = self.ed.view.cache_peek(i)?;
        Some(Rect::from_min_size(
            pos2(self.last_left, self.last_top + y),
            vec2(self.ed.view.width(), l.height),
        ))
    }

    /// The document offset under screen point `p` (last frame's geometry; `None` outside the
    /// surface): where dropped files are inserted.
    pub fn offset_at_screen(
        &mut self,
        ctx: &egui::Context,
        parley: Option<&mut ParleyText>,
        images: &dyn markise_layout::Images,
        p: Pos2,
    ) -> Option<usize> {
        let (_, r) = self.last_surface?;
        if !r.contains(p) {
            return None;
        }
        let top = self.last_top;
        with_shaper(ctx, parley, |sh| self.hit_screen(p, top, sh, images)).map(|d| d.offset)
    }

    /// The front-matter toggle or a Cmd/Ctrl-clicked link is under `p` (the press is not a
    /// caret placement).
    /// Sets how comments are shown (setting `markdown.commentDisplay`).
    pub fn set_comment_display(&mut self, d: markise_engine::settings::CommentDisplay) {
        self.comment_display = d;
    }

    /// The comments of the visible items at the last frame, with their cards (tests).
    pub fn shown_comments(&self) -> &[ShownComment] {
        &self.comments
    }

    /// The suggestion hover bar's rectangle at the last frame, when shown (tests).
    pub fn hover_bar_rect(&self) -> Option<Rect> {
        self.hover.as_ref().and_then(|h| h.rect)
    }

    /// A bubbles-mode popover is open (tests).
    pub fn popover_open(&self) -> bool {
        self.comments.iter().any(|c| c.card.is_some()) && self.popover.is_some()
    }

    /// The text of the visible items at the last frame, item after item (tests).
    pub fn visible_text(&self) -> String {
        let mut out = String::new();
        for &(idx, _) in &self.last_visible {
            if let Some(l) = self.ed.view.cache_peek(idx) {
                for p in &l.prims {
                    if let markise_layout::Prim::Text { text, .. } = p {
                        out.push_str(text.text());
                        out.push('\n');
                    }
                }
            }
        }
        out
    }

    /// The shown comment whose bubble or card is under screen point `p`.
    fn comment_at(&self, p: Pos2) -> Option<usize> {
        self.comments
            .iter()
            .position(|c| (!c.revealed && c.glyph.expand(2.0).contains(p)) || c.card.is_some_and(|r| r.contains(p)))
    }

    /// Puts the caret at the end of comment `i`'s text (its source is then revealed for
    /// editing); `pin`: its popover stays open.
    fn open_comment(&mut self, i: usize, pin: bool) {
        let Some(c) = self.comments.get(i).cloned() else { return };
        self.ed.set_selection(Selection::caret(c.content.end));
        self.ed.refresh_focus();
        self.refocus = true;
        if pin {
            self.popover = Some(Popover {
                key: c.source.start,
                since: 0.0,
                pinned: true,
            });
        }
    }

    /// Lays out and paints the comment cards of the visible items: in the right margin when it
    /// is wide enough and the setting says so, else as a popover over the hovered (or pinned)
    /// bubble. Only visible items are looked at; card text is shaped only for cards shown.
    fn comment_cards(
        &mut self,
        ui: &mut Ui,
        visible: &[markise_layout::Visible],
        rect: Rect,
        left: f32,
        col_w: f32,
        theme: &Theme,
    ) {
        let model = self.ed.view.model().clone();
        let mut shown = Vec::new();
        for v in visible {
            let base = model.item_start(v.idx);
            for c in &v.layout.comments {
                shown.push(ShownComment {
                    glyph: c.rect.translate(vec2(left, rect.top() + v.y)),
                    source: base + c.source.start as usize..base + c.source.end as usize,
                    content: base + c.content.start as usize..base + c.content.end as usize,
                    text: c.text.clone(),
                    revealed: c.revealed,
                    card: None,
                });
            }
        }
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let margin = rect.right() - SCROLLBAR_W - (left + col_w);
        let use_margin =
            self.comment_display == markise_engine::settings::CommentDisplay::Margin && margin >= MIN_MARGIN;
        let look = CardLook::new(theme, &ctx);
        let painter = ui.painter_at(rect);
        let mut clicked = None;
        if use_margin {
            self.popover = None;
            let x = left + col_w + 20.0;
            let w = (margin - 36.0).min(240.0);
            let mut next_y = rect.top();
            for (i, c) in shown.iter_mut().enumerate() {
                // nothing is shaped for a card that would start below the viewport; the stack
                // only grows downwards, so the rest would too
                let y = (c.glyph.top() - 4.0).max(next_y);
                if y > rect.bottom() {
                    break;
                }
                let galley = look.galley(&ctx, &c.text, w);
                let card = Rect::from_min_size(pos2(x, y), look.size(&galley, w));
                next_y = card.bottom() + 6.0;
                // the connector from the bubble's line to its card, in the margin only (it never
                // crosses text), starting with a dot at the column edge
                let from = pos2(left + col_w + 6.0, c.glyph.center().y);
                let to = pos2(card.left(), card.top() + 12.0);
                let line = Stroke::new(1.0, look.accent.gamma_multiply(0.5));
                painter.circle_filled(from, 2.0, look.accent.gamma_multiply(0.7));
                painter.line_segment([from, to], line);
                look.paint(&painter, card, galley);
                let r = ui.interact(
                    card,
                    ui.scope_id().with(("markise-comment-card", c.source.start)),
                    Sense::click(),
                );
                let r = r.on_hover_cursor(CursorIcon::PointingHand);
                if r.clicked() {
                    clicked = Some(i);
                }
                c.card = Some(card);
            }
        } else {
            // bubbles: a popover over the hovered (after a short rest) or pinned bubble
            let pointer = ctx.input(|i| i.pointer.hover_pos());
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.popover = None;
            }
            let over_card = |p: Pos2| {
                self.comments
                    .iter()
                    .any(|c| c.card.is_some_and(|r| r.expand(4.0).contains(p)))
            };
            let hovered = pointer.and_then(|p| {
                shown
                    .iter()
                    .find(|c| !c.revealed && c.glyph.expand(2.0).contains(p))
                    .map(|c| c.source.start)
            });
            match (hovered, self.popover) {
                (Some(k), Some(pop)) if pop.key == k => {}
                (Some(k), Some(pop)) if pop.pinned && pop.key != k => {
                    self.popover = Some(Popover {
                        key: k,
                        since: now,
                        pinned: false,
                    })
                }
                (Some(k), _) => {
                    self.popover = Some(Popover {
                        key: k,
                        since: now,
                        pinned: false,
                    })
                }
                (None, Some(pop)) if !pop.pinned && !pointer.is_some_and(over_card) => self.popover = None,
                _ => {}
            }
            if let Some(pop) = self.popover
                && let Some(i) = shown.iter().position(|c| c.source.start == pop.key)
            {
                let wait = POPOVER_DELAY - (now - pop.since);
                if pop.pinned || wait <= 0.0 {
                    let c = &shown[i];
                    let w = 240.0f32.min(rect.width() - 24.0).max(120.0);
                    let galley = look.galley(&ctx, &c.text, w);
                    let size = look.size(&galley, w);
                    let mut at = pos2(c.glyph.left() - 8.0, c.glyph.bottom() + 4.0);
                    at.x =
                        at.x.min(rect.right() - SCROLLBAR_W - size.x - 4.0)
                            .max(rect.left() + 4.0);
                    if at.y + size.y > rect.bottom() {
                        at.y = (c.glyph.top() - 4.0 - size.y).max(rect.top());
                    }
                    let area = egui::Area::new(egui::Id::unique(("markise-comment-popover", pop.key)))
                        .order(egui::Order::Foreground)
                        .fixed_pos(at)
                        .show(&ctx, |ui| {
                            let (r, resp) = ui.allocate_exact_size(size, Sense::click());
                            look.paint(ui.painter(), r, galley);
                            resp.on_hover_cursor(CursorIcon::PointingHand)
                        });
                    if area.inner.clicked() {
                        clicked = Some(i);
                    }
                    shown[i].card = Some(area.response.rect);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.01)));
                }
            }
        }
        self.comments = shown;
        if let Some(i) = clicked {
            self.open_comment(i, !use_margin);
            ctx.request_repaint();
        }
    }

    fn on_toggle_or_link(&self, p: Pos2, top: f32, m: egui::Modifiers) -> bool {
        for &(idx, y) in &self.last_visible {
            let Some(l) = self.ed.view.cache_peek(idx) else {
                continue;
            };
            let local = pos2(p.x - self.last_left, p.y - top - y);
            if local.y < 0.0 || local.y >= l.height {
                continue;
            }
            return l.toggle.is_some_and(|t| t.contains(local)) || (m.command && l.link_at(local).is_some());
        }
        false
    }

    /// The task checkbox under screen point `p` (last frame's geometry): the visible item and
    /// the box's list item ordinal ([`BlockLayout::tasks`]).
    fn task_box_at(&self, p: Pos2, top: f32) -> Option<(usize, u16)> {
        for &(idx, y) in &self.last_visible {
            let Some(l) = self.ed.view.cache_peek(idx) else {
                continue;
            };
            let local = pos2(p.x - self.last_left, p.y - top - y);
            if local.y < 0.0 || local.y >= l.height {
                continue;
            }
            return task_box_hit(l, local).map(|li| (idx, li));
        }
        None
    }

    /// Toggles the task box of list item `li` (ordinal among the enclosing list items, 0 =
    /// outermost) of visible item `idx` as one undo step ("Toggle Task Status"); the selection
    /// is kept as it was (the box is not a caret stop, spec 02 §9).
    fn toggle_task(&mut self, idx: usize, li: u16) -> bool {
        let leaf = self.ed.view.model().item_span(idx).start;
        // that list item's own start: inside it but before any nested list, so the planner's
        // "deepest task item" is exactly this one
        let Some(start) = self
            .ed
            .session
            .doc()
            .containers_at(leaf)
            .into_iter()
            .filter(|c| matches!(c.kind, ContainerKind::ListItem { .. }))
            .nth(li as usize)
            .filter(|c| matches!(c.kind, ContainerKind::ListItem { task: Some(_), .. }))
            .map(|c| c.span.start)
        else {
            return false;
        };
        self.ed
            .edit(|s| {
                s.transact("Toggle Task Status", TxKind::Structural, |tx| {
                    let keep = tx.selection().clone();
                    tx.set_selection(Selection::caret(start));
                    let plan = tx.plan(planners::TOGGLE_TASK, Params::new())?;
                    tx.apply_plan(plan)?;
                    // `[ ]` / `[x]` / `[X]` have the same length: offsets are unchanged
                    tx.set_selection(keep);
                    Ok(())
                })
            })
            .is_ok()
    }

    /// The document position under screen point `p` (last frame's geometry).
    /// The document's context menu (Add Comment, Accept / Reject, Delete Comment, Accept /
    /// Reject All; a right click outside the selection moves the caret there first) and the
    /// Accept / Reject bar over a hovered suggestion.
    fn review_ui(
        &mut self,
        ui: &mut Ui,
        resp: &egui::Response,
        rect: Rect,
        mut parley: Option<&mut ParleyText>,
        images: &TextureStore,
        out: &mut ViewerOutput,
    ) {
        let ctx = ui.ctx().clone();
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let pos = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                self.hit_screen(p, rect.top(), sh, images)
            });
            if let Some(pos) = pos {
                let sel = self.ed.session.selection().primary();
                let r = sel.range();
                if sel.is_caret() || pos.offset < r.start || pos.offset > r.end {
                    self.ed.set_selection(Selection::single(pos, pos));
                    self.ed.refresh_focus();
                }
            }
            resp.request_focus();
        }
        let mut chosen = None;
        resp.context_menu(|ui| {
            let enabled = |id: &str| self.ed.registry.enabled(&self.ed.session, id).is_ok();
            for e in CONTEXT_COMMANDS {
                match e {
                    None => {
                        ui.separator();
                    }
                    Some(id) => {
                        let Some(spec) = self.ed.registry.get(id) else { continue };
                        let mut b = egui::Button::new(spec.desc.label);
                        if let Some(c) = self.keymap.shown_chord_of(id) {
                            b = b.shortcut_text(crate::menu::shortcut_text(c, self.platform));
                        }
                        if ui.add_enabled(enabled(id), b).clicked() {
                            chosen = Some(spec.desc.id);
                            ui.close();
                        }
                    }
                }
            }
        });
        if let Some(id) = chosen {
            out.review = Some((id, None));
        }
        // hover bar: any edit hides it (review round 5: never act on an obsolete or a
        // different mark); it comes back with fresh data on the next pointer move over a mark
        let counter = self.ed.session.revision().counter;
        if self.hover.as_ref().is_some_and(|h| h.counter != counter) {
            self.hover = None;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let on_bar = self
            .hover
            .as_ref()
            .and_then(|h| h.rect)
            .zip(pointer)
            .is_some_and(|(r, p)| r.expand(6.0).contains(p));
        // looked up again only when the pointer moved or the text changed
        let key = pointer.map(|p| (p, self.ed.session.revision().counter));
        let moved = key != self.hover_key;
        self.hover_key = key;
        if !on_bar && moved {
            let idle = self.drag.is_none() && !ui.input(|i| i.pointer.any_down());
            let found = match pointer {
                Some(p) if idle && resp.hovered() => {
                    let pos = with_shaper(&ctx, parley, |sh| self.hit_screen(p, rect.top(), sh, images));
                    pos.and_then(|pos| self.suggestion_at(pos.offset)).map(|m| (m, p))
                }
                _ => None,
            };
            match found {
                Some((m, p)) => {
                    if self.hover.as_ref().is_none_or(|h| h.mark != m) {
                        self.hover = Some(HoverBar {
                            mark: m,
                            counter: self.ed.session.revision().counter,
                            anchor: pos2(p.x - 24.0, p.y - 40.0),
                            rect: None,
                        });
                    }
                }
                None => self.hover = None,
            }
        }
        if let Some(h) = self.hover.as_mut() {
            let at = h.mark.content.start;
            let area = egui::Area::new(egui::Id::unique("markise-review-hover"))
                .order(egui::Order::Foreground)
                .fixed_pos(h.anchor)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Accept").on_hover_text("Accept this suggestion").clicked() {
                                out.review = Some(("review.accept", Some(at)));
                            }
                            if ui.button("Reject").on_hover_text("Reject this suggestion").clicked() {
                                out.review = Some(("review.reject", Some(at)));
                            }
                        });
                    });
                });
            h.rect = Some(area.response.rect);
            if out.review.is_some() {
                self.hover = None;
            }
        }
    }

    /// The suggestion whose source contains `offset` (cached per revision and offset).
    fn suggestion_at(&mut self, offset: usize) -> Option<markise_doc::review::ReviewMark> {
        let counter = self.ed.session.revision().counter;
        if let Some((c, o, m)) = &self.hover_cache
            && *c == counter
            && *o == offset
        {
            return m.clone();
        }
        let m = markise_doc::review::mark_at(self.ed.session.doc(), offset, |k| k.is_suggestion());
        self.hover_cache = Some((counter, offset, m.clone()));
        m
    }

    fn hit_screen(
        &mut self,
        p: Pos2,
        top: f32,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
    ) -> Option<DocPos> {
        let (&(first, fy), &(last, _)) = (self.last_visible.first()?, self.last_visible.last()?);
        let mut target = None;
        for &(idx, y) in &self.last_visible {
            if p.y >= top + y {
                target = Some((idx, y));
            }
        }
        let (idx, y) = target.unwrap_or((first, fy));
        let _ = last;
        let local = pos2(p.x - self.last_left, p.y - top - y);
        let mut g = self.ed.geometry(sh, images);
        g.hit(idx, local)
    }

    /// The range of `unit` at `pos` (a caret for chars).
    fn unit_range(
        &mut self,
        pos: DocPos,
        unit: Unit,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
    ) -> (DocPos, DocPos) {
        let mut g = self.ed.geometry(sh, images);
        match unit {
            Unit::Char => (pos, pos),
            Unit::Word => {
                let b = g.horizontal(pos, true, true);
                let a = g.horizontal(b, false, true);
                if a.key() <= pos.key() && pos.key() <= b.key() {
                    (a, b)
                } else {
                    (pos, pos)
                }
            }
            Unit::Block => match g.item_pos(pos) {
                Some((i, _)) => {
                    let l = g.view.layout(i, g.shaper, g.images);
                    let start = g.view.model().item_start(i);
                    let at = |p: Option<markise_layout::caret::ItemPos>| {
                        p.map(|p| DocPos::new(start + p.rel as usize)).unwrap_or(pos)
                    };
                    (
                        at(markise_layout::caret::edge_stop(&l, false)),
                        at(markise_layout::caret::edge_stop(&l, true)),
                    )
                }
                None => (pos, pos),
            },
        }
    }

    /// One input event; returns `true` when it ran a command.
    fn handle_event(
        &mut self,
        e: Event,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
        editable: bool,
        vh: f32,
    ) -> bool {
        match e {
            Event::Text(t) if editable && !t.is_empty() && !t.chars().any(char::is_control) => {
                self.exec("edit.insertText", &Params::text("text", t), sh, images, vh)
            }
            Event::Ime(egui::ImeEvent::Commit(t)) if editable && !t.is_empty() => {
                // an IME commit is one transaction with boundaries before and after (§4.2)
                self.ed.session.close_typing();
                let r = self.exec("edit.insertText", &Params::text("text", t), sh, images, vh);
                self.ed.session.close_typing();
                r
            }
            Event::Paste(t) if editable && !t.is_empty() => {
                // a paste is its own undo step (spec 02 §11); plain source text (M2e adds
                // HTML conversion and clipping)
                self.ed.session.close_typing();
                let r = self.exec("edit.insertText", &Params::text("text", t), sh, images, vh);
                self.ed.session.close_typing();
                r
            }
            Event::Copy | Event::Cut => {
                let r = self.ed.session.selection().primary().range();
                if !r.is_empty() {
                    // copied after the shaper closure (the context is locked inside it)
                    self.pending_copy = Some(self.ed.session.text()[r].to_string());
                    if matches!(e, Event::Cut) && editable {
                        self.ed.session.close_typing();
                        return self.exec("edit.deleteBackward", &Params::new(), sh, images, vh);
                    }
                }
                false
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let Some(chord) = chord_of(key, modifiers, self.platform) else {
                    return false;
                };
                let ids: Vec<&'static str> = self.keymap.lookup(&chord).to_vec();
                for id in ids {
                    let Some(spec) = self.ed.registry.get(id) else { continue };
                    if matches!(spec.run, markise_engine::editor::command::Run::Host) {
                        continue; // file / view commands: the app handles them
                    }
                    if !editable && matches!(spec.desc.effect, markise_doc::Effect::Mutates { .. }) {
                        continue;
                    }
                    if self.ed.registry.enabled(&self.ed.session, id).is_err() {
                        continue;
                    }
                    return self.exec(id, &Params::new(), sh, images, vh);
                }
                false
            }
            _ => false,
        }
    }

    fn exec(
        &mut self,
        id: &str,
        params: &Params,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
        vh: f32,
    ) -> bool {
        let r = self.ed.execute(id, params, sh, images, vh);
        // each command sees the reveal state of the previous one's caret
        self.ed.refresh_focus();
        r.is_ok() || matches!(r, Err(markise_engine::editor::command::CommandError::Disabled(_)))
    }
}

/// Runs `f` with the text shaper of the active backend.
pub(crate) fn with_shaper<R>(
    ctx: &egui::Context,
    parley: Option<&mut ParleyText>,
    f: impl FnOnce(&mut dyn TextShaper) -> R,
) -> R {
    match parley {
        Some(p) => f(&mut p.shaper),
        None => ctx.fonts_mut(|fonts| f(fonts)),
    }
}

/// The engine keymap's chord of an egui key press.
/// How a comment card looks (ADR 0012): a rounded card on the comment tint with a thin
/// accent border, a speech-bubble icon, the author (`@name:`) in bold, then the text as
/// written (plain, upright, wrapped, its line breaks kept).
struct CardLook {
    fill: Color32,
    accent: Color32,
    text: Color32,
    size: f32,
}

const CARD_PAD: f32 = 8.0;
const CARD_ICON: f32 = 18.0;

impl CardLook {
    fn new(theme: &Theme, ctx: &egui::Context) -> Self {
        let rc = theme.review_colors();
        let [pr, pg, pb, _] = theme.page_bg.0;
        let [tr, tg, tb, ta] = rc.comment_bg.0;
        // the comment tint over the page, opaque (the card hides what is under it); computed
        // on the straight (not premultiplied) theme colours
        let a = ta as f32 / 255.0 * 0.75;
        let mix = |p: u8, t: u8| (p as f32 * (1.0 - a) + t as f32 * a).round() as u8;
        let fill = Color32::from_rgb(mix(pr, tr), mix(pg, tg), mix(pb, tb));
        let _ = ctx;
        CardLook {
            fill,
            accent: color(theme.chrome.accent),
            text: color(theme.text),
            size: (theme.font_size * 0.82).clamp(11.0, 18.0),
        }
    }

    fn galley(&self, ctx: &egui::Context, text: &str, w: f32) -> Arc<egui::Galley> {
        let (author, body) = markise_doc::review::split_author(text);
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = (w - 2.0 * CARD_PAD - CARD_ICON).max(40.0);
        let body_fmt = egui::TextFormat {
            font_id: egui::FontId::proportional(self.size),
            color: self.text,
            ..Default::default()
        };
        if let Some(a) = author {
            let bold = egui::FontFamily::Name(markise_layout::FAMILY_BOLD.into());
            let has_bold = ctx.fonts(|f| f.families().contains(&bold));
            job.append(
                &format!("@{a}\n"),
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::new(self.size, if has_bold { bold } else { egui::FontFamily::Proportional }),
                    color: self.text,
                    ..Default::default()
                },
            );
        }
        // the author's prefix is shown above; the rest as written (keeps inner line breaks)
        let rest = if author.is_some() { body } else { text.trim() };
        job.append(if rest.is_empty() { "…" } else { rest }, 0.0, body_fmt);
        ctx.fonts_mut(|f| f.layout_job(job))
    }

    fn size(&self, galley: &egui::Galley, w: f32) -> Vec2 {
        let gw = galley.size().x + 2.0 * CARD_PAD + CARD_ICON;
        vec2(gw.clamp(80.0, w), galley.size().y.max(CARD_ICON) + 2.0 * CARD_PAD)
    }

    fn paint(&self, p: &egui::Painter, r: Rect, galley: Arc<egui::Galley>) {
        p.rect(
            r,
            CornerRadius::same(6),
            self.fill,
            Stroke::new(1.0, self.accent.gamma_multiply(0.35)),
            StrokeKind::Inside,
        );
        // a small speech bubble
        let o = r.min + vec2(CARD_PAD, CARD_PAD + 1.0);
        let b = Rect::from_min_size(o, vec2(12.0, 9.0));
        let stroke = Stroke::new(1.3, self.accent);
        p.rect_stroke(b, CornerRadius::same(2), stroke, StrokeKind::Middle);
        p.line_segment(
            [b.left_bottom() + vec2(3.0, 0.0), b.left_bottom() + vec2(2.5, 3.5)],
            stroke,
        );
        p.line_segment(
            [b.left_bottom() + vec2(2.5, 3.5), b.left_bottom() + vec2(6.0, 0.0)],
            stroke,
        );
        p.galley(r.min + vec2(CARD_PAD + CARD_ICON, CARD_PAD), galley, self.text);
    }
}

pub fn chord_of(key: egui::Key, modifiers: egui::Modifiers, platform: Platform) -> Option<Chord> {
    Some(Chord {
        ctrl: if platform == Platform::Mac {
            modifiers.ctrl
        } else {
            modifiers.ctrl || modifiers.command
        },
        alt: modifiers.alt,
        shift: modifiers.shift,
        meta: platform == Platform::Mac && modifiers.mac_cmd,
        key: key_name(key)?,
    })
}

/// The engine keymap's name of an egui key.
fn key_name(k: egui::Key) -> Option<String> {
    use egui::Key as K;
    Some(
        match k {
            K::ArrowLeft => "Left",
            K::ArrowRight => "Right",
            K::ArrowUp => "Up",
            K::ArrowDown => "Down",
            K::Enter => "Enter",
            K::Backspace => "Backspace",
            K::Delete => "Delete",
            K::Home => "Home",
            K::End => "End",
            K::PageUp => "PageUp",
            K::PageDown => "PageDown",
            K::Escape => "Escape",
            K::Tab => "Tab",
            K::Space => "Space",
            K::Slash => "/",
            K::Backslash => "\\",
            K::Equals => "=",
            K::Plus => "+",
            K::Minus => "-",
            K::Backtick => "`",
            K::OpenBracket => "[",
            K::CloseBracket => "]",
            K::Comma => ",",
            K::Period => ".",
            K::Semicolon => ";",
            _ => {
                let n = k.name();
                if n.len() > 1 && n.starts_with('F') && n[1..].parse::<u8>().is_ok() {
                    return Some(n.to_string()); // function keys (F11 Full Screen)
                }
                return (n.chars().count() == 1).then(|| n.to_uppercase());
            }
        }
        .to_string(),
    )
}

/// The list item ordinal of the task checkbox of `l` under item-local `local` (with a small
/// margin: the box is small).
fn task_box_hit(l: &BlockLayout, local: Pos2) -> Option<u16> {
    l.tasks
        .iter()
        .find(|(r, _)| r.expand(2.0).contains(local))
        .map(|&(_, li)| li)
}

fn hit(visible: &[markise_layout::Visible], p: Pos2, left: f32, top: f32) -> Option<(usize, Pos2, &BlockLayout)> {
    visible.iter().find_map(|v| {
        let y0 = top + v.y;
        (p.y >= y0 && p.y < y0 + v.layout.height).then(|| (v.idx, pos2(p.x - left, p.y - y0), &*v.layout))
    })
}

/// Dash segments one layout may paint per frame (dashes are expanded only inside the clip
/// rect, so this bounds pathological cases, not normal diagrams).
pub const MAX_DASHES_PER_LAYOUT: usize = 20_000;

/// Parameters `(t0, t1)` of the part of segment `a → b` inside `r` (Liang–Barsky), if any.
pub fn clip_segment(a: Pos2, b: Pos2, r: Rect) -> Option<(f32, f32)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [
        (-dx, a.x - r.min.x),
        (dx, r.max.x - a.x),
        (-dy, a.y - r.min.y),
        (dy, r.max.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((t0, t1))
}

/// Dashed polyline, expanded only where it crosses `clip`, with the dash phase measured from
/// the start of the path (so scrolling does not make dashes crawl). Returns the dashes drawn.
pub fn dashed_segments(pts: &[Pos2], dash: f32, gap: f32, clip: Rect, budget: &mut usize) -> Vec<[Pos2; 2]> {
    let (d, g) = (dash.max(0.5), gap.max(0.5));
    let period = d + g;
    let mut out = Vec::new();
    let mut dist = 0.0f32;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        if !(len > 0.0 && len.is_finite()) {
            continue;
        }
        if let Some((t0, t1)) = clip_segment(a, b, clip) {
            let dir = (b - a) / len;
            let (s0, s1) = (t0 * len, t1 * len);
            // start of the dash period containing s0, in path distance
            let mut s = s0 - (dist + s0).rem_euclid(period);
            while s < s1 {
                if *budget == 0 {
                    return out;
                }
                let (ds, de) = (s.max(s0), (s + d).min(s1));
                if de > ds {
                    out.push([a + dir * ds, a + dir * de]);
                    *budget -= 1;
                }
                s += period;
            }
        }
        dist += len;
    }
    out
}

fn paint_dashed(painter: &egui::Painter, pts: &[Pos2], stroke: Stroke, d: f32, g: f32, clip: Rect, budget: &mut usize) {
    for seg in dashed_segments(pts, d, g, clip.expand(stroke.width + 2.0), budget) {
        painter.line_segment(seg, stroke);
    }
}

/// Paints one shaped text block (epaint galley or parley block) with its anchor at `pos`.
pub(crate) fn paint_text_block(
    painter: &egui::Painter,
    ui: &Ui,
    text: &Arc<dyn markise_layout::TextBlock>,
    pos: Pos2,
    parley: Option<&mut ParleyText>,
) {
    if let Some(galley) = text.galley() {
        // Re-request the galley: egui's galley cache returns the same Arc while the font atlas
        // is unchanged, and a freshly rasterised one after an atlas rebuild.
        let g = painter.layout_job((*galley.job).clone());
        painter.galley(pos, g, Color32::PLACEHOLDER);
    } else if let (Some(block), Some(p)) = (text.as_any().downcast_ref::<ParleyBlock>(), parley) {
        p.atlas.paint(ui.ctx(), painter, block, pos);
    }
}

fn paint_layout(
    painter: &egui::Painter,
    ui: &Ui,
    layout: &BlockLayout,
    origin: Pos2,
    images: &mut TextureStore,
    requests: &mut Vec<String>,
    mut parley: Option<&mut ParleyText>,
) {
    let off = origin.to_vec2();
    let clip_rect = painter.clip_rect();
    let mut dash_budget = MAX_DASHES_PER_LAYOUT;
    for prim in &layout.prims {
        match prim {
            Prim::Text { pos, text, .. } => {
                let r = text.rect().translate(pos.to_vec2() + off);
                if !r.intersects(clip_rect) {
                    continue;
                }
                paint_text_block(painter, ui, text, *pos + off, parley.as_deref_mut());
            }
            Prim::Rect {
                rect,
                fill,
                stroke,
                radius,
            } => {
                painter.rect(rect.translate(off), *radius, *fill, *stroke, StrokeKind::Inside);
            }
            Prim::Line { a, b, stroke } => {
                painter.line_segment([*a + off, *b + off], *stroke);
            }
            Prim::Circle {
                center,
                radius,
                fill,
                stroke,
            } => {
                painter.circle(*center + off, *radius, *fill, *stroke);
            }
            Prim::Path {
                points,
                closed,
                fill,
                stroke,
                dash,
            } => {
                let pts: Vec<Pos2> = points.iter().map(|p| *p + off).collect();
                let bb = Rect::from_points(&pts).expand(stroke.width + 1.0);
                if !bb.intersects(clip_rect) {
                    continue;
                }
                if *closed && *fill != Color32::TRANSPARENT && pts.len() >= 3 {
                    painter.add(egui::Shape::convex_polygon(pts.clone(), *fill, Stroke::NONE));
                }
                if stroke.width > 0.0 && stroke.color != Color32::TRANSPARENT {
                    match dash {
                        Some([d, g]) => {
                            let mut v = pts;
                            if *closed && let Some(&first) = v.first() {
                                v.push(first);
                            }
                            paint_dashed(painter, &v, *stroke, *d, *g, clip_rect, &mut dash_budget);
                        }
                        None if *closed => {
                            painter.add(egui::Shape::closed_line(pts, *stroke));
                        }
                        None => {
                            painter.add(egui::Shape::line(pts, *stroke));
                        }
                    }
                }
            }
            Prim::Checkbox {
                rect,
                checked,
                border,
                fill,
            } => {
                let r = rect.translate(off);
                if *checked {
                    painter.rect(r, 3.0, *fill, Stroke::NONE, StrokeKind::Inside);
                    let s = Stroke::new(1.8, Color32::WHITE);
                    let a = pos2(r.left() + r.width() * 0.22, r.top() + r.height() * 0.52);
                    let b = pos2(r.left() + r.width() * 0.43, r.top() + r.height() * 0.72);
                    let c = pos2(r.left() + r.width() * 0.78, r.top() + r.height() * 0.30);
                    painter.line_segment([a, b], s);
                    painter.line_segment([b, c], s);
                } else {
                    painter.rect(
                        r,
                        3.0,
                        Color32::TRANSPARENT,
                        Stroke::new(1.2, *border),
                        StrokeKind::Inside,
                    );
                }
            }
            Prim::Math { rect, key, query, clip } => {
                if !rect.translate(off).intersects(clip_rect) {
                    continue;
                }
                if let Some(tex) = images.math_texture(ui.ctx(), *key, query) {
                    // snap to the pixel grid: the bitmap is rendered 1:1 for this scale
                    let ppp = ui.ctx().pixels_per_point();
                    let min = rect.min + off;
                    let min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
                    // table cells clip their formulas
                    let p = match clip {
                        Some(c) => painter.with_clip_rect(c.translate(off).intersect(clip_rect)),
                        None => painter.clone(),
                    };
                    p.image(
                        tex.id(),
                        Rect::from_min_size(min, rect.size()),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            Prim::Image { rect, src, placeholder } => {
                let r = rect.translate(off);
                match images.texture(src) {
                    Some(tex) => {
                        painter.image(
                            tex.id(),
                            r,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        painter.rect_filled(r, 4.0, *placeholder);
                        requests.push(src.clone());
                    }
                }
            }
        }
    }
    let _ = ui;
}

#[cfg(test)]
mod dash_tests {
    use super::*;

    /// Codex P1: a very long dashed lifeline expands only its visible part.
    #[test]
    fn dashes_are_clipped_to_the_visible_rect() {
        let pts = [pos2(10.0, -500_000.0), pos2(10.0, 600_000.0)];
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));
        let mut budget = MAX_DASHES_PER_LAYOUT;
        let segs = dashed_segments(&pts, 3.0, 3.0, clip, &mut budget);
        assert!(segs.len() <= 101, "{}", segs.len());
        assert!(segs.len() >= 99);
        assert!(segs.iter().all(|s| s[0].y >= 0.0 && s[1].y <= 600.0));
        // phase follows the path, not the clip: dashes start at multiples of 6 from the origin
        for s in &segs {
            let from_start = s[0].y + 500_000.0;
            let r = from_start.rem_euclid(6.0);
            assert!(r < 1e-2 || s[0].y == 0.0, "{r}");
        }
        // outside the clip: nothing
        let far = [pos2(5000.0, 0.0), pos2(5000.0, 100.0)];
        assert!(dashed_segments(&far, 3.0, 3.0, clip, &mut budget).is_empty());
    }

    #[test]
    fn dash_budget_is_enforced() {
        let pts = [pos2(0.0, 0.0), pos2(0.0, 1.0e6)];
        let clip = Rect::from_min_max(pos2(-1.0, -1.0), pos2(1.0, 1.0e6));
        let mut budget = 50;
        assert_eq!(dashed_segments(&pts, 1.0, 1.0, clip, &mut budget).len(), 50);
        assert_eq!(budget, 0);
    }
}
