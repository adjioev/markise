//! Moving a block with the pointer (ADR 0022): a quiet handle beside a block, shown while the pointer
//! is over the block, an insertion line where it would land, and autoscroll at the edges. The move
//! itself is [`markise_engine::editor::EditorView::move_block_to`], which the app runs.

use egui::accesskit::Role;
use egui::{CursorIcon, Key, Modifiers, Rect, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
use markise_layout::Visible;
use markise_layout::block::color;
use markise_theme::Theme;

use super::{DocViewer, ViewerOutput};

/// How near an edge the pointer has to be for the view to scroll, and how fast at the very edge.
const SCROLL_EDGE: f32 = 48.0;
const SCROLL_SPEED: f32 = 900.0;

/// A block being dragged.
pub(super) struct BlockDrag {
    opener: usize,
    /// The document as it was when the drag started: any change ends it.
    changes: u64,
    /// Where it would land: the line, and where on screen that shows.
    target: Option<(usize, f32)>,
}

impl DocViewer {
    /// A block is being dragged by its handle.
    pub fn block_drag_target(&self) -> Option<usize> {
        self.block_drag.as_ref()?.target.map(|t| t.0)
    }

    pub fn is_dragging_block(&self) -> bool {
        self.block_drag.is_some()
    }

    pub(super) fn block_move_ui(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        if self.ed.session.read_only().is_some() {
            self.block_drag = None;
            return;
        }
        let side = theme.line_px().clamp(14.0, 20.0);
        let hovered = self.hovered_block(ui, visible, surface, left - side - 6.0);
        let drag_opener = self.block_drag.as_ref().map(|d| d.opener);
        let model = self.ed.view.model().clone();
        for v in visible {
            let start = model.item_start(v.idx);
            if Some(start) != hovered && Some(start) != drag_opener {
                continue;
            }
            let Some((a, _)) = v.layout.card_top else { continue };
            let top = surface.top() + v.y + a.y;
            let x = (left + a.x - side - 3.0).max(surface.left() + 1.0);
            let r = Rect::from_min_size(pos2(x, top + 4.0), vec2(side, side));
            if !surface.contains_rect(r) {
                continue;
            }
            let id = ui.scope_id().with(("markise-block-handle", start));
            let resp = ui
                .interact(r, id, Sense::drag())
                .on_hover_cursor(CursorIcon::Default)
                .on_hover_text("Drag to move the block");
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Move Block"));
            if resp.drag_started() {
                self.block_drag = Some(BlockDrag {
                    opener: start,
                    changes: self.ed.changes(),
                    target: None,
                });
            }
            if resp.hovered() {
                self.pointer_on_block_control = true;
            }
            let tone = if resp.hovered() || resp.dragged() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let painter = ui.painter_at(surface);
            for (dx, dy) in [-1.0f32, 1.0]
                .into_iter()
                .flat_map(|x| [-1.0f32, 0.0, 1.0].map(|y| (x, y)))
            {
                let at = r.center() + vec2(dx * side * 0.14, dy * side * 0.24);
                painter.circle_filled(at, (side * 0.07).max(1.0), tone);
            }
        }
        self.block_drag_update(ui, visible, surface, left, theme, out);
    }

    /// The innermost block under the pointer, by its opener. Nothing shows through a window on top.
    fn hovered_block(&self, ui: &Ui, visible: &[Visible], surface: Rect, left: f32) -> Option<usize> {
        let p = ui.input(|i| i.pointer.hover_pos())?;
        if !surface.contains(p) || p.x < left || ui.ctx().layer_id_at(p) != Some(ui.layer_id()) {
            return None;
        }
        let v = visible.iter().find(|v| {
            let y = surface.top() + v.y;
            p.y >= y && p.y < y + v.layout.height
        })?;
        let at = self.ed.view.model().item_start(v.idx);
        let c = self.ed.session.doc().named_containers_at(at).pop()?;
        Some(c.opener.start)
    }

    fn block_drag_update(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        let Some(drag) = &self.block_drag else { return };
        let opener = drag.opener;
        let cancelled =
            drag.changes != self.ed.changes() || ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if cancelled {
            self.block_drag = None;
            return;
        }
        self.pointer_on_block_control = true;
        let (pos, down, dt) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.stable_dt));
        let target = pos.and_then(|p| self.drop_target(visible, surface, opener, p.y));
        if !down {
            out.move_block = target.map(|(line, _)| (opener, line));
            self.block_drag = None;
            ui.ctx().request_repaint();
            return;
        }
        if let Some(p) = pos {
            let up = SCROLL_EDGE - (p.y - surface.top());
            let below = SCROLL_EDGE - (surface.bottom() - p.y);
            let speed = SCROLL_SPEED / SCROLL_EDGE;
            let dy = if up > 0.0 {
                -up * speed * dt
            } else if below > 0.0 {
                below * speed * dt
            } else {
                0.0
            };
            if dy != 0.0 {
                self.ed.view.scroll_by(dy);
            }
        }
        if let Some(drag) = &mut self.block_drag {
            drag.target = target;
        }
        if let Some((_, y)) = target {
            let right = left + self.ed.view.width();
            let stroke = Stroke::new(2.0, color(theme.chrome.accent));
            ui.painter_at(surface).hline(left..=right, y, stroke);
        }
        // the pointer may stand still at an edge, and the view has to go on
        ui.ctx().request_repaint();
    }

    /// The place to land nearest the pointer among what is on screen. The block's own place is one of
    /// them, where it lands nowhere: a press and a release move nothing.
    fn drop_target(&self, visible: &[Visible], surface: Rect, opener: usize, y: f32) -> Option<(usize, f32)> {
        let model = self.ed.view.model();
        let closer = (self.ed.session.doc().named_containers_at(opener).pop()).map(|c| c.closer.start);
        let mut places: Vec<(Option<usize>, f32)> = Vec::new();
        for v in visible {
            let (start, top) = (model.item_start(v.idx), surface.top() + v.y);
            if start == opener {
                places.push((None, top));
            }
            if Some(start) == closer {
                places.push((None, top + v.layout.height));
            }
            if let Some(line) = self.ed.drop_line(opener, start) {
                places.push((Some(line), top));
            }
        }
        // after the last thing at the block's level, when that is on screen
        if let Some(line) = self.ed.drop_line_at_end(opener)
            && let Some(last) = visible.last()
            && last.idx + 1 == model.items.len()
        {
            places.push((Some(line), surface.top() + last.y + last.layout.height));
        }
        let (line, at) = places
            .into_iter()
            .min_by(|a, b| (a.1 - y).abs().total_cmp(&(b.1 - y).abs()))?;
        Some((line?, at))
    }
}
