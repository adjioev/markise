//! The source editor widget (spec 02 §12.1 / §12.1.1, ADR 0006 §5.5; M2d): full Source mode and
//! the left pane of Side by Side.
//!
//! The widget paints and drives the editor session's source pane
//! ([`markise_engine::editor::SourcePane`]): a virtualised [`markise_layout::source::SourceView`]
//! of the document text with Markdown colouring from the parse. Only the items in the viewport
//! are laid out, so documents of any size open, scroll and take keystrokes within the budgets
//! (no size limit, no read-only fallback).
//!
//! - **Keyboard**: chords go through the document's keymap; caret, selection, typing,
//!   Backspace/Delete, Enter, Tab/Shift+Tab and Undo/Redo run as source commands
//!   ([`markise_engine::editor::EditorView::source_execute`]); host commands (Save, mode
//!   switches) are taken by the app first. IME commits and pastes are verbatim insertions of
//!   their own undo step; copy/cut put the selected source text on the clipboard.
//! - **Mouse**: click places the caret, shift-click extends, double-click selects a word,
//!   triple-click a line; a drag selects (at the press's unit) and scrolls past the edges.
//! - **Display** (persisted View preferences, spec 02 §12.1.1 defaults): line-number gutter
//!   (off), soft wrap (on; otherwise horizontal scrolling), current-line band (on), whitespace
//!   marks (off: `·` for spaces, `→` for tabs, `↵` after two or more trailing spaces).
//! - **Repaint discipline** (NFR-10): the caret blinks only while focused and for
//!   [`crate::viewer::BLINK_FOR`] seconds after the last input; nothing is scheduled otherwise.

use std::ops::Range;
use std::time::Instant;

use egui::{Align2, CornerRadius, CursorIcon, Event, EventFilter, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use markise_doc::{DocPos, Params, PieceKind, Selection};
use markise_engine::editor::source::source_command_available;
use markise_engine::settings::SourceSettings;
use markise_layout::block::color;
use markise_layout::source::{self as srcview, SourceParams, SourceVisible};
use markise_theme::Theme;

use crate::text_paint::ParleyText;
use crate::viewer::{BLINK_FOR, DocViewer, chord_of, paint_text_block, with_shaper};

const SCROLLBAR_W: f32 = 10.0;
const BLINK_HALF: f64 = 0.53;
/// Space between the gutter (or the pane's left edge) and the text.
const TEXT_PAD: f32 = 12.0;

/// What happened in the source editor this frame.
#[derive(Debug, Default)]
pub struct SourceOutput {
    /// The document text changed through this editor.
    pub edited: bool,
    /// The caret or selection moved (or the text changed).
    pub moved: bool,
    /// The user scrolled the view (wheel, scrollbar, drag past the edge).
    pub user_scrolled: bool,
    /// Keystroke processing time this frame (input → laid out and painted), when there was
    /// keyboard input.
    pub input_ms: Option<f64>,
    /// Items laid out this frame.
    pub laid_out: usize,
}

/// Selection granularity of a pointer drag (single, double, triple click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Line,
}

/// Per-document UI state of the source editor.
pub struct SourceEditor {
    blink_from: f64,
    drag: Option<(Range<usize>, Unit)>,
    last_press: Option<(f64, Pos2, u32)>,
    pending_copy: Option<String>,
    /// Scroll so that this (fractional) line is at the vertical middle on the next frame.
    center_line: Option<f32>,
    /// Scroll so that the viewport top is at this content y on the next frame.
    scroll_px: Option<f64>,
    focus_next: bool,
    hscroll: f32,
    widget: Option<egui::Id>,
    last_rect: Option<Rect>,
    text_left: f32,
}

impl Default for SourceEditor {
    fn default() -> Self {
        Self::new()
    }
}

/// The source view parameters for the display preferences.
pub fn params(p: &SourceSettings) -> SourceParams {
    SourceParams {
        wrap: p.soft_wrap,
        tab_width: 4,
    }
}

impl SourceEditor {
    pub fn new() -> Self {
        SourceEditor {
            blink_from: 0.0,
            drag: None,
            last_press: None,
            pending_copy: None,
            center_line: None,
            scroll_px: None,
            focus_next: false,
            hscroll: 0.0,
            widget: None,
            last_rect: None,
            text_left: 0.0,
        }
    }

    /// The widget's id at the last frame (it holds the keyboard focus while editing).
    pub fn widget_id(&self) -> Option<egui::Id> {
        self.widget
    }

    /// Whether the editor has the keyboard focus.
    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        self.widget.is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
    }

    /// The editor's screen rectangle at the last frame.
    pub fn rect(&self) -> Option<Rect> {
        self.last_rect
    }

    /// Takes the keyboard focus on the next frame.
    pub fn request_focus(&mut self) {
        self.focus_next = true;
    }

    /// Centres the (fractional) physical line `line` on the next frame (entering source mode,
    /// outline clicks).
    pub fn center_line(&mut self, line: f32) {
        self.center_line = Some(line);
        self.scroll_px = None;
    }

    /// Puts the viewport top at content y `y` on the next frame (Side by Side sync).
    pub fn scroll_to_px(&mut self, y: f64) {
        self.scroll_px = Some(y.max(0.0));
        self.center_line = None;
    }

    /// A programmatic scroll is waiting for the next frame.
    pub fn scroll_pending(&self) -> bool {
        self.center_line.is_some() || self.scroll_px.is_some()
    }

    /// Lays out and runs the editor in the space left in `ui`. `editable`: edits are accepted
    /// (otherwise navigation, selection and copy still work).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        viewer: &mut DocViewer,
        theme: &Theme,
        mut parley: Option<&mut ParleyText>,
        prefs: &SourceSettings,
        editable: bool,
    ) -> SourceOutput {
        let mut out = SourceOutput::default();
        let rect = ui.available_rect_before_wrap();
        let id = ui.scope_id().with("markise-source-editor");
        self.widget = Some(id);
        self.last_rect = Some(rect);
        let resp = ui.interact(
            Rect::from_min_max(rect.min, pos2(rect.right() - SCROLLBAR_W, rect.bottom())),
            id,
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, color(theme.page_bg));
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let started = Instant::now();
        let vh = rect.height();
        if viewer.ed.source.is_none() {
            return out;
        }
        let rev0 = viewer.ed.session.revision().counter;
        let sel0 = viewer.ed.source.as_ref().map(|p| p.sel.clone());

        // ---- geometry and parameters
        let (size, _) = srcview::metrics(theme);
        let char_w = size * 0.6;
        let line_count = viewer.ed.source.as_ref().map_or(1, |p| p.view.line_count());
        let gutter = if prefs.line_numbers {
            let digits = (line_count.max(1) as f64).log10().floor() as usize + 1;
            digits.max(3) as f32 * char_w + 2.0 * TEXT_PAD
        } else {
            0.0
        };
        let text_w = (rect.width() - gutter - 2.0 * TEXT_PAD - SCROLLBAR_W).max(40.0);
        let ppp = ctx.pixels_per_point();
        let font_gen = viewer.font_generation();
        if let Some(p) = viewer.ed.source.as_mut() {
            p.view.set_params(params(prefs), text_w, theme, ppp, font_gen);
        }
        if !prefs.soft_wrap {
            // keep the horizontal offset sane
            self.hscroll = self.hscroll.max(0.0);
        } else {
            self.hscroll = 0.0;
        }
        let text_left = rect.left() + gutter + TEXT_PAD - self.hscroll;
        self.text_left = text_left;

        // ---- focus
        if resp.clicked() || resp.drag_started() || std::mem::take(&mut self.focus_next) {
            resp.request_focus();
            self.blink_from = now;
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

        // ---- programmatic scrolls
        if let Some(l) = self.center_line.take() {
            let text = viewer.ed.session.text();
            if let Some(p) = viewer.ed.source.as_mut() {
                let y = p.view.y_of_line(text, l) - vh as f64 * 0.5;
                p.view.scroll_to_y(y.max(0.0));
            }
        }
        if let Some(y) = self.scroll_px.take()
            && let Some(p) = viewer.ed.source.as_mut()
        {
            p.view.scroll_to_y(y);
        }

        // ---- wheel / trackpad
        if resp.hovered() || ui.rect_contains_pointer(rect) {
            let d = ui.input(|i| i.smooth_scroll_delta);
            if d != egui::Vec2::ZERO {
                if let Some(p) = viewer.ed.source.as_mut()
                    && d.y != 0.0
                {
                    p.view.scroll_by(-d.y);
                }
                if !prefs.soft_wrap && d.x != 0.0 {
                    self.hscroll = (self.hscroll - d.x).max(0.0);
                }
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                out.user_scrolled = true;
            }
        }
        // ---- scrollbar
        let bar = Rect::from_min_max(pos2(rect.right() - SCROLLBAR_W, rect.top()), rect.right_bottom());
        let bar_resp = ui.interact(bar, id.with("bar"), Sense::click_and_drag());
        if (bar_resp.dragged() || bar_resp.clicked())
            && let Some(pt) = bar_resp.interact_pointer_pos()
            && let Some(p) = viewer.ed.source.as_mut()
        {
            let total = p.view.total_height().max(1.0);
            let frac = ((pt.y - rect.top()) / vh).clamp(0.0, 1.0) as f64;
            p.view.scroll_to_y(frac * (total - vh as f64).max(0.0));
            out.user_scrolled = true;
        }

        // ---- keyboard
        let mut follow = false;
        if focused {
            let events = ui.input(|i| i.events.clone());
            let keymap = viewer.keymap().clone();
            let platform = viewer.platform();
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                for e in events {
                    follow |= self.handle_event(e, viewer, &keymap, platform, sh, editable, vh);
                }
            });
            if let Some(t) = self.pending_copy.take() {
                ctx.copy_text(t);
            }
        }

        // ---- pointer
        let modifiers = ui.input(|i| i.modifiers);
        let pressed = ui.input(|i| i.pointer.primary_pressed()) && resp.hovered();
        let to_content = |p: Pos2, v: &DocViewer| -> Pos2 {
            let sy = v.ed.source.as_ref().map_or(0.0, |s| s.view.scroll_y()) as f32;
            pos2(p.x - text_left, p.y - rect.top() + sy)
        };
        if pressed && let Some(pt) = ui.input(|i| i.pointer.interact_pos()) {
            let count = match self.last_press {
                Some((t, q, n)) if now - t < 0.45 && q.distance(pt) < 5.0 => (n % 3) + 1,
                _ => 1,
            };
            self.last_press = Some((now, pt, count));
            let cp = to_content(pt, viewer);
            if let Some(pos) = with_shaper(&ctx, parley.as_deref_mut(), |sh| viewer.ed.source_hit(cp, sh)) {
                let unit = match count {
                    2 => Unit::Word,
                    3 => Unit::Line,
                    _ => Unit::Char,
                };
                let r = self.unit_range(viewer, pos, unit);
                let sel = if modifiers.shift && unit == Unit::Char {
                    let an = viewer.ed.source.as_ref().map(|p| p.sel.primary().anchor).unwrap_or(pos);
                    self.drag = Some((an.offset..an.offset, Unit::Char));
                    Selection::single(an, pos)
                } else {
                    self.drag = Some((r.clone(), unit));
                    if unit == Unit::Char {
                        Selection::caret_at(pos)
                    } else {
                        Selection::range(r.start, r.end)
                    }
                };
                viewer.ed.source_set_selection(sel);
            }
            self.blink_from = now;
        }
        if let Some((anchor, unit)) = self.drag.clone() {
            if ui.input(|i| i.pointer.primary_down()) {
                if let Some(pt) = ui.input(|i| i.pointer.interact_pos())
                    && (resp.dragged() || resp.is_pointer_button_down_on())
                {
                    if pt.y < rect.top() || pt.y > rect.bottom() {
                        let dy = if pt.y < rect.top() {
                            (pt.y - rect.top()).max(-40.0)
                        } else {
                            (pt.y - rect.bottom()).min(40.0)
                        };
                        if let Some(p) = viewer.ed.source.as_mut() {
                            p.view.scroll_by(dy);
                        }
                        out.user_scrolled = true;
                        ctx.request_repaint();
                    }
                    let cp = to_content(pt, viewer);
                    if let Some(pos) = with_shaper(&ctx, parley.as_deref_mut(), |sh| viewer.ed.source_hit(cp, sh)) {
                        let r = self.unit_range(viewer, pos, unit);
                        let sel = if pos.offset < anchor.start {
                            Selection::range(anchor.end, if unit == Unit::Char { pos.offset } else { r.start })
                        } else if unit == Unit::Char {
                            Selection::single(DocPos::new(anchor.start), pos)
                        } else {
                            Selection::range(anchor.start, r.end.max(anchor.end))
                        };
                        viewer.ed.source_set_selection(sel);
                    }
                }
            } else {
                self.drag = None;
            }
        }

        if follow {
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                viewer.ed.source_scroll_to_caret(vh, sh)
            });
        }
        out.edited = viewer.ed.session.revision().counter != rev0;
        out.moved = out.edited || viewer.ed.source.as_ref().map(|p| p.sel.clone()) != sel0;
        if out.moved {
            self.blink_from = now;
        }

        // ---- layout of the viewport (+ half a screen of margin)
        let visible = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
            viewer.ed.source_frame(vh, vh * 0.5, sh)
        });
        out.laid_out = viewer.ed.source.as_ref().map_or(0, |p| p.view.laid_out.len());
        let (caret_rect, line_rect) = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
            (viewer.ed.source_caret_rect(sh), viewer.ed.source_line_rect(sh))
        });
        let scroll_y = viewer.ed.source.as_ref().map_or(0.0, |p| p.view.scroll_y()) as f32;
        // horizontal follow without wrap
        if follow
            && !prefs.soft_wrap
            && let Some(c) = caret_rect
        {
            let x = c.min.x;
            if x < self.hscroll {
                self.hscroll = (x - 20.0).max(0.0);
            } else if x > self.hscroll + text_w - 20.0 {
                self.hscroll = x - text_w + 40.0;
            }
        }

        // ---- paint
        let text_clip = Rect::from_min_max(pos2(rect.left() + gutter, rect.top()), pos2(bar.left(), rect.bottom()));
        let tp = painter.with_clip_rect(text_clip.intersect(painter.clip_rect()));
        if prefs.current_line
            && let Some(lr) = line_rect
        {
            let r = Rect::from_min_max(
                pos2(text_clip.left(), lr.min.y - scroll_y + rect.top()),
                pos2(text_clip.right(), lr.max.y - scroll_y + rect.top()),
            );
            painter.rect_filled(r, 0.0, color(theme.meta_text).gamma_multiply(0.08));
        }
        let sel = viewer.ed.source.as_ref().map(|p| p.sel.primary());
        if let (Some(sel), Some(p)) = (sel, viewer.ed.source.as_ref())
            && !sel.is_caret()
        {
            let fill = color(theme.chrome.selection_bg).gamma_multiply(0.55);
            for r in p.view.selection_rects(&visible, sel.range()) {
                tp.rect_filled(r.translate(vec2(text_left, rect.top())), 0.0, fill);
            }
        }
        for v in &visible {
            for ln in &v.layout.lines {
                let pos = pos2(text_left, rect.top() + v.y + ln.y);
                if pos.y > rect.bottom() || pos.y + ln.height < rect.top() {
                    continue;
                }
                paint_text_block(&tp, ui, &ln.text, pos, parley.as_deref_mut());
            }
        }
        if prefs.whitespace_marks {
            self.paint_whitespace(&tp, viewer, &visible, rect.top(), text_left, theme);
        }
        if prefs.line_numbers {
            self.paint_gutter(&painter, viewer, &visible, rect, gutter, size, theme);
        }
        if focused {
            let since = now - self.blink_from;
            let on = since >= BLINK_FOR || ((since / BLINK_HALF) as u64).is_multiple_of(2);
            if let Some(c) = caret_rect {
                let y = c.min.y - scroll_y + rect.top();
                let r = Rect::from_min_size(pos2(text_left + c.min.x - 0.75, y), vec2(1.5, c.height().max(4.0)));
                if on && r.intersects(text_clip) {
                    painter.rect_filled(r, 0.0, color(theme.text));
                }
                // IME: allowed while focused; the candidate window follows the caret (preedit
                // display later, ADR 0006 §6.3)
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
        if resp.hovered() {
            ctx.set_cursor_icon(CursorIcon::Text);
        }
        if out.moved {
            ctx.request_repaint();
        }

        // ---- scrollbar
        if let Some(p) = viewer.ed.source.as_ref() {
            let total = p.view.total_height().max(1.0);
            if total > vh as f64 + 1.0 {
                let y = p.view.scroll_y();
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
        }
        if follow {
            out.input_ms = Some(started.elapsed().as_secs_f64() * 1e3);
        }
        out
    }

    /// The range of `unit` at `pos` (empty for chars).
    fn unit_range(&self, viewer: &DocViewer, pos: DocPos, unit: Unit) -> Range<usize> {
        let t = viewer.ed.session.text();
        let o = pos.offset.min(t.len());
        match unit {
            Unit::Char => o..o,
            Unit::Word => srcview::word_at(t, o),
            Unit::Line => {
                let s = t[..o].rfind('\n').map_or(0, |k| k + 1);
                let e = t[o..].find('\n').map_or(t.len(), |k| o + k + 1);
                s..e
            }
        }
    }

    /// One input event; returns `true` when it ran a command or edited (follow the caret).
    #[allow(clippy::too_many_arguments)]
    fn handle_event(
        &mut self,
        e: Event,
        viewer: &mut DocViewer,
        keymap: &markise_engine::editor::keymap::Keymap,
        platform: markise_engine::editor::keymap::Platform,
        sh: &mut dyn markise_layout::TextShaper,
        editable: bool,
        vh: f32,
    ) -> bool {
        match e {
            Event::Text(t) if editable && !t.is_empty() && !t.chars().any(char::is_control) => {
                viewer.ed.source_insert(&t, false).is_ok()
            }
            Event::Ime(egui::ImeEvent::Commit(t)) if editable && !t.is_empty() => {
                // an IME commit is one undo step with boundaries before and after
                viewer.ed.source_insert(&t, true).is_ok()
            }
            Event::Paste(t) if editable && !t.is_empty() => viewer.ed.source_insert(&t, true).is_ok(),
            Event::Copy | Event::Cut => {
                if let Some(t) = viewer.ed.source_selected_text() {
                    self.pending_copy = Some(t);
                    if matches!(e, Event::Cut) && editable {
                        return viewer.ed.source_cut().is_ok();
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
                let Some(chord) = chord_of(key, modifiers, platform) else {
                    return false;
                };
                for &id in keymap.lookup(&chord) {
                    let Some(spec) = viewer.ed.registry.get(id) else {
                        continue;
                    };
                    if matches!(spec.run, markise_engine::editor::command::Run::Host) || !source_command_available(id) {
                        continue;
                    }
                    if !editable && matches!(spec.desc.effect, markise_doc::Effect::Mutates { .. }) {
                        continue;
                    }
                    let r = viewer.ed.source_execute(id, &Params::new(), sh, vh);
                    return r.is_ok() || matches!(r, Err(markise_engine::editor::command::CommandError::Disabled(_)));
                }
                false
            }
            _ => false,
        }
    }

    /// Whitespace marks: `·` for spaces, an arrow for tabs, `↵` after a hard line break (two or
    /// more trailing spaces before a line break).
    fn paint_whitespace(
        &self,
        painter: &egui::Painter,
        viewer: &DocViewer,
        visible: &[SourceVisible],
        top: f32,
        left: f32,
        theme: &Theme,
    ) {
        let c = color(theme.meta_text).gamma_multiply(0.55);
        let text = viewer.ed.session.text();
        for v in visible {
            for ln in &v.layout.lines {
                let y0 = top + v.y + ln.y;
                let rows = &ln.rows;
                let row_y = |r: usize| rows.get(r).map_or(0.0, |x| x.1 + x.2 * 0.5);
                let map = ln.text.source_map();
                let clusters = ln.text.clusters();
                for cl in &clusters {
                    if cl.ch != ' ' {
                        continue;
                    }
                    let atom = map.is_some_and(|m| {
                        m.pieces
                            .iter()
                            .any(|p| p.kind == PieceKind::Atom && p.text.start as usize == cl.byte)
                    });
                    let y = y0 + row_y(cl.line);
                    if atom {
                        let piece_end = map
                            .and_then(|m| {
                                m.pieces
                                    .iter()
                                    .find(|p| p.kind == PieceKind::Atom && p.text.start as usize == cl.byte)
                            })
                            .map(|p| p.text.end as usize)
                            .unwrap_or(cl.byte + 1);
                        let x1 = clusters
                            .iter()
                            .find(|k| k.byte + 1 >= piece_end && k.byte >= cl.byte)
                            .map_or(cl.x + cl.advance, |k| k.x + k.advance);
                        let (a, b) = (pos2(left + cl.x + 2.0, y), pos2(left + x1 - 2.0, y));
                        painter.line_segment([a, b], Stroke::new(1.0, c));
                        painter.line_segment([b, b + vec2(-3.0, -3.0)], Stroke::new(1.0, c));
                        painter.line_segment([b, b + vec2(-3.0, 3.0)], Stroke::new(1.0, c));
                    } else if !map.is_some_and(|m| {
                        m.pieces.iter().any(|p| {
                            p.kind == PieceKind::Atom && (p.text.start as usize..p.text.end as usize).contains(&cl.byte)
                        })
                    }) {
                        painter.circle_filled(pos2(left + cl.x + cl.advance * 0.5, y), 1.1, c);
                    }
                }
                // hard line break: two or more trailing spaces before the terminator
                let s = v.start + ln.start as usize;
                let e = s + ln.len as usize;
                if e < text.len() && text[s..e].ends_with("  ") && ln.first {
                    let end = ln.text.caret_rect(markise_layout::text::LocalPos::new(
                        ln.text.text().len(),
                        markise_layout::text::Affinity::Upstream,
                    ));
                    let y = y0 + end.center().y;
                    let x = left + end.min.x + 3.0;
                    painter.line_segment([pos2(x + 6.0, y - 4.0), pos2(x + 6.0, y + 2.0)], Stroke::new(1.0, c));
                    painter.line_segment([pos2(x + 6.0, y + 2.0), pos2(x, y + 2.0)], Stroke::new(1.0, c));
                    painter.line_segment([pos2(x, y + 2.0), pos2(x + 3.0, y - 1.0)], Stroke::new(1.0, c));
                    painter.line_segment([pos2(x, y + 2.0), pos2(x + 3.0, y + 5.0)], Stroke::new(1.0, c));
                }
            }
        }
    }

    /// Line numbers, right-aligned in the gutter; the caret's line in the text colour.
    #[allow(clippy::too_many_arguments)]
    fn paint_gutter(
        &self,
        painter: &egui::Painter,
        viewer: &DocViewer,
        visible: &[SourceVisible],
        rect: Rect,
        gutter: f32,
        size: f32,
        theme: &Theme,
    ) {
        let g = Rect::from_min_max(rect.min, pos2(rect.left() + gutter, rect.bottom()));
        painter.rect_filled(g, 0.0, color(theme.page_bg));
        let caret_line = viewer.ed.source.as_ref().map(|p| {
            let t = viewer.ed.session.text();
            p.view.line_of(t, p.sel.primary().focus.offset)
        });
        let font = FontId::monospace((size * 0.85).max(8.0));
        let dim = color(theme.meta_text).gamma_multiply(0.7);
        let cur = color(theme.text);
        for v in visible {
            let mut n = v.first_line;
            for (k, ln) in v.layout.lines.iter().enumerate() {
                if k > 0 && ln.first {
                    n += 1;
                }
                if !ln.first {
                    continue;
                }
                let y = rect.top() + v.y + ln.y + ln.rows.first().map_or(0.0, |r| r.1);
                if y > rect.bottom() || y + ln.height < rect.top() {
                    continue;
                }
                let c = if Some(n) == caret_line { cur } else { dim };
                let row_h = ln.rows.first().map_or(ln.height, |r| r.2);
                painter.text(
                    pos2(g.right() - TEXT_PAD, y + row_h * 0.5),
                    Align2::RIGHT_CENTER,
                    (n + 1).to_string(),
                    font.clone(),
                    c,
                );
            }
        }
    }
}

/// Patches turning `old` into `new` (both document texts): the common prefix and suffix are
/// kept, the rest is split into changed hunks; every patch is verbatim (tests and tools that
/// edit the source text as a whole).
pub fn diff_patches(old: &str, new: &str) -> markise_doc::PatchSet {
    let mut p = old
        .as_bytes()
        .iter()
        .zip(new.as_bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
        p -= 1;
    }
    let max = old.len().min(new.len()) - p;
    let mut s = old
        .as_bytes()
        .iter()
        .rev()
        .zip(new.as_bytes().iter().rev())
        .take(max)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - s) || !new.is_char_boundary(new.len() - s) {
        s -= 1;
    }
    let (o, n) = (&old[p..old.len() - s], &new[p..new.len() - s]);
    if o.is_empty() && n.is_empty() {
        return markise_doc::PatchSet::empty();
    }
    let ps: Vec<markise_doc::Patch> = if o.is_empty() || n.is_empty() {
        vec![markise_doc::Patch::verbatim(p..p + o.len(), n)]
    } else {
        markise_engine::edit::patches(o, n)
            .into_iter()
            .map(|h| markise_doc::Patch::verbatim(p + h.start..p + h.end, h.insert))
            .collect()
    };
    markise_doc::PatchSet::new(ps).unwrap_or_else(|_| markise_doc::PatchSet::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_patches_keep_untouched_bytes() {
        let old = "a\r\nmiddle\nz\r\n";
        let new = "A\r\nmiddle\nZ\r\n";
        let ps = diff_patches(old, new);
        assert_eq!(ps.patches().len(), 2, "{ps:?}");
        assert_eq!(ps.apply_to(old), new);
        assert!(diff_patches("same", "same").is_empty());
        let ps = diff_patches("hello wörld", "hello wörd");
        assert_eq!(ps.patches()[0].range, 10..11);
    }
}
