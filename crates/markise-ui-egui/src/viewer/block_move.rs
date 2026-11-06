//! Moving a block with the pointer (ADR 0022): a quiet handle beside a block, shown while the pointer
//! is over the block. From the press on the block shows as picked up, a line shows where it would land
//! or that it stays, a label follows the pointer, and the view scrolls at the edges. The move itself is
//! [`markise_engine::editor::EditorView::move_block_to`], which the app runs.

use egui::accesskit::Role;
use egui::{
    Align2, CornerRadius, CursorIcon, FontId, Key, LayerId, Modifiers, Order, Rect, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, pos2, vec2,
};
use markise_layout::Visible;
use markise_layout::block::color;
use markise_theme::Theme;

use super::{DocViewer, ViewerOutput};

/// How near an edge the pointer has to be for the view to scroll, and how fast at the very edge.
const SCROLL_EDGE: f32 = 48.0;
const SCROLL_SPEED: f32 = 900.0;
/// How far from the press the pointer has to go for the edges to count.
const MOVED: f32 = 6.0;
/// How far from where it was picked up, in rows, the pointer still means "it stays", also over a tall
/// block: a block's own height is no distance to pull before anything answers.
const STAY_ROWS: f32 = 2.0;

/// A block being dragged.
pub(super) struct BlockDrag {
    opener: usize,
    /// The document as it was when the drag started: any change ends it.
    changes: u64,
    /// Where it would land: the line, and where on screen that shows.
    target: Option<(usize, f32)>,
    /// Where the pointer picked it up, in the document's coordinates: the view may scroll.
    grabbed_y: f64,
    /// Where on screen it was pressed, and that the pointer has left there: a press near an edge, or on a
    /// handle held at the view's top, is not yet a wish to scroll.
    pressed_at: f32,
    has_moved: bool,
    shown: Option<BlockDragShown>,
}

/// What a drag shows on screen in this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDragShown {
    /// The part of the block on screen, outlined and tinted.
    pub outline: Option<Rect>,
    /// Where the line is.
    pub line_y: f32,
    /// The line is a place to land; otherwise the block stays where it is.
    pub moves: bool,
    /// The label following the pointer.
    pub label: String,
}

impl DocViewer {
    /// A block is being dragged by its handle.
    pub fn block_drag_target(&self) -> Option<usize> {
        self.block_drag.as_ref()?.target.map(|t| t.0)
    }

    pub fn is_dragging_block(&self) -> bool {
        self.block_drag.is_some()
    }

    pub(super) fn block_drag_going(&self) -> bool {
        self.block_drag.is_some()
    }

    /// What the drag going on shows.
    pub fn block_drag_shown(&self) -> Option<&BlockDragShown> {
        self.block_drag.as_ref()?.shown.as_ref()
    }

    /// The pointer is on a block's handle, which then stands out.
    pub fn on_block_handle(&self) -> bool {
        self.pointer_on_block_handle
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
        self.pointer_on_block_handle = false;
        if self.block_index.is_none() || self.ed.session.read_only().is_some() {
            self.block_drag = None;
            self.block_handle = None;
            return;
        }
        let side = theme.line_px().clamp(14.0, 20.0);
        let drag_opener = self.block_drag.as_ref().map(|d| d.opener);
        // while one is dragged, the pointer passing over others offers nothing
        let hovered = (drag_opener.is_none())
            .then(|| self.hovered_block(ui, visible, surface))
            .flatten();
        let model = self.ed.view.model().clone();
        let mut handle = None;
        for v in visible {
            let start = model.item_start(v.idx);
            if Some(start) != hovered && Some(start) != drag_opener {
                continue;
            }
            let Some((a, _)) = v.layout.card_top else { continue };
            // a tall picture cut off at the top keeps its handle in reach
            let top = (surface.top() + v.y + a.y + 4.0).max(surface.top() + 4.0);
            let x = (left + a.x - side - 3.0).max(surface.left() + 1.0);
            let r = Rect::from_min_size(pos2(x, top), vec2(side, side));
            if !surface.contains_rect(r) {
                continue;
            }
            // more to hit than to see: up to the block's edge, and a little around it
            let hit = Rect::from_min_max(
                pos2((x - 6.0).max(surface.left()), (top - 4.0).max(surface.top())),
                pos2((left + a.x).max(r.right()), (r.bottom() + 4.0).min(surface.bottom())),
            );
            handle = Some((start, hit));
            let id = ui.scope_id().with(("markise-block-handle", start));
            let resp = ui
                .interact(hit, id, Sense::drag())
                .on_hover_cursor(CursorIcon::Grab)
                .on_hover_text("Drag to move the block");
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Move Block"));
            if resp.drag_started() {
                let y = resp.interact_pointer_pos().map_or(r.center().y, |p| p.y);
                self.block_drag = Some(BlockDrag {
                    opener: start,
                    changes: self.ed.changes(),
                    target: None,
                    grabbed_y: f64::from(y - surface.top()) + self.ed.view.scroll_y(),
                    pressed_at: y,
                    has_moved: false,
                    shown: None,
                });
            }
            let hot = resp.hovered() || resp.dragged();
            if hot {
                self.pointer_on_block_control = true;
                self.pointer_on_block_handle = true;
            }
            let tone = if hot {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let painter = ui.painter_at(surface);
            if hot {
                painter.rect_filled(r.expand(2.0), 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let dot = if hot { 1.35 } else { 1.0 } * (side * 0.07).max(1.0);
            for (dx, dy) in [-1.0f32, 1.0]
                .into_iter()
                .flat_map(|x| [-1.0f32, 0.0, 1.0].map(|y| (x, y)))
            {
                let at = r.center() + vec2(dx * side * 0.14, dy * side * 0.24);
                painter.circle_filled(at, dot, tone);
            }
        }
        self.block_handle = handle;
        self.block_drag_update(ui, visible, surface, left, theme, out);
    }

    /// The innermost block under the pointer, by its opener. Nothing shows through a window on top. The
    /// margin beside a block counts, and so does its handle, which may reach past its first row.
    pub(super) fn hovered_block(&self, ui: &Ui, visible: &[Visible], surface: Rect) -> Option<usize> {
        let p = ui.input(|i| i.pointer.hover_pos())?;
        if !surface.contains(p) || ui.ctx().layer_id_at(p) != Some(ui.layer_id()) {
            return None;
        }
        if let Some((opener, hit)) = self.block_handle
            && hit.contains(p)
        {
            return Some(opener);
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
        let grabbed = (drag.grabbed_y - self.ed.view.scroll_y()) as f32 + surface.top();
        self.pointer_on_block_control = true;
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let (pos, down, dt) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.stable_dt));
        let reach = STAY_ROWS * theme.line_px();
        let target = pos.and_then(|p| self.drop_target(visible, surface, opener, p.y, grabbed, reach));
        if !down {
            out.move_block = target.map(|(line, _)| (opener, line));
            self.block_drag = None;
            ui.ctx().request_repaint();
            return;
        }
        if let (Some(p), Some(d)) = (pos, &mut self.block_drag) {
            d.has_moved |= (p.y - d.pressed_at).abs() > MOVED;
        }
        if let Some(p) = pos.filter(|_| self.block_drag.as_ref().is_some_and(|d| d.has_moved)) {
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
        let shown = self.paint_drag(ui, visible, surface, left, theme, opener, target, pos);
        if let Some(drag) = &mut self.block_drag {
            drag.target = target;
            drag.shown = Some(shown);
        }
        // the pointer may stand still at an edge, and the view has to go on
        ui.ctx().request_repaint();
    }

    /// The block as picked up, the line, and the label by the pointer.
    #[allow(clippy::too_many_arguments)]
    fn paint_drag(
        &self,
        ui: &Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        opener: usize,
        target: Option<(usize, f32)>,
        pos: Option<egui::Pos2>,
    ) -> BlockDragShown {
        let accent = color(theme.chrome.accent);
        let painter = ui.painter_at(surface);
        let outline = self.block_outline(visible, surface, left, opener);
        if let Some(r) = outline {
            painter.rect_filled(r, CornerRadius::same(4), accent.gamma_multiply(0.10));
            painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(2.0, accent), StrokeKind::Outside);
        }
        let (x0, x1) = (left, left + self.ed.view.width());
        let line_y = match target {
            Some((_, y)) => {
                painter.hline(x0..=x1, y, Stroke::new(2.0, accent));
                y
            }
            // it stays: a quiet line along its own place, not a place to land
            None => {
                let y = (outline.map_or(surface.top(), |r| r.top()) - 4.0).max(surface.top() + 1.0);
                let weak = ui.visuals().weak_text_color();
                let dashes = egui::Shape::dashed_line(&[pos2(x0, y), pos2(x1, y)], Stroke::new(1.5, weak), 6.0, 5.0);
                painter.extend(dashes);
                y
            }
        };
        let name = self.block_name(opener);
        let label = match target {
            Some(_) => format!("Move {name} here"),
            None => format!("{name} stays here"),
        };
        if let Some(p) = pos {
            let visuals = ui.visuals();
            let layer = LayerId::new(Order::Tooltip, ui.scope_id().with("markise-block-drag-label"));
            let on_top = ui.ctx().layer_painter(layer);
            let text = on_top.layout_no_wrap(label.clone(), FontId::proportional(12.0), visuals.text_color());
            let at = p + vec2(16.0, 12.0);
            let bg = Rect::from_min_size(at, text.size() + vec2(12.0, 6.0));
            on_top.rect_filled(bg, 4.0, visuals.window_fill());
            on_top.rect_stroke(bg, 4.0, Stroke::new(1.0, accent), StrokeKind::Inside);
            let origin = Align2::LEFT_TOP.align_size_within_rect(text.size(), bg.shrink2(vec2(6.0, 3.0)));
            on_top.galley(origin.min, text, visuals.text_color());
        }
        BlockDragShown {
            outline,
            line_y,
            moves: target.is_some(),
            label,
        }
    }

    /// The name a person knows the block by: the package's label, else what the file says.
    fn block_name(&self, opener: usize) -> String {
        let chain = self.ed.session.doc().named_containers_at(opener);
        let Some(c) = chain.last() else {
            return "The block".to_string();
        };
        let entry = self.block_index.as_ref().and_then(|i| i.block(&c.name));
        let label = entry.and_then(|e| e.schema.label.as_ref());
        label.map_or_else(|| c.name.to_string(), |l| l.get(&self.block_language).to_string())
    }

    /// The part of the block on screen.
    fn block_outline(&self, visible: &[Visible], surface: Rect, left: f32, opener: usize) -> Option<Rect> {
        let model = self.ed.view.model();
        let closer = (self.ed.session.doc().named_containers_at(opener).pop()).map(|c| c.closer.start)?;
        let mut side = (left, left + self.ed.view.width());
        let mut span: Option<(f32, f32)> = None;
        for v in visible {
            let start = model.item_start(v.idx);
            if start < opener || start > closer {
                continue;
            }
            let (mut top, bottom) = (surface.top() + v.y, surface.top() + v.y + v.layout.height);
            if start == opener
                && let Some((a, b)) = v.layout.card_top
            {
                side = (left + a.x, left + b.x);
                top += a.y;
            }
            span = Some(span.map_or((top, bottom), |(t, b)| (t.min(top), b.max(bottom))));
        }
        let (top, bottom) = span?;
        Some(Rect::from_min_max(pos2(side.0, top), pos2(side.1, bottom)).intersect(surface.shrink(2.0)))
    }

    /// The place to land for a pointer at `y`. Near where the block was picked up, over the block, it
    /// stays: a press and a release move nothing. Out of there it lands at the nearest place on screen on
    /// that side of it, the block's own place being none, so there is no way to go that answers nothing.
    fn drop_target(
        &self,
        visible: &[Visible],
        surface: Rect,
        opener: usize,
        y: f32,
        grabbed: f32,
        reach: f32,
    ) -> Option<(usize, f32)> {
        let model = self.ed.view.model();
        let closer = (self.ed.session.doc().named_containers_at(opener).pop()).map(|c| c.closer.start);
        // an end off screen is as far as can be on its side: the view may have scrolled past all of it
        let first = visible.first().map(|v| model.item_start(v.idx));
        let last = visible.last().map(|v| model.item_start(v.idx));
        let mut own_top = if last.is_some_and(|l| opener > l) {
            f32::INFINITY
        } else {
            f32::NEG_INFINITY
        };
        let mut own_bottom = if first.zip(closer).is_some_and(|(f, c)| c < f) {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        };
        let mut places: Vec<(usize, f32)> = Vec::new();
        for v in visible {
            let (start, top) = (model.item_start(v.idx), surface.top() + v.y);
            if start == opener {
                own_top = top;
            }
            if Some(start) == closer {
                own_bottom = top + v.layout.height;
            }
            if let Some(line) = self.ed.drop_line(opener, start) {
                places.push((line, top));
            }
        }
        // after the last thing at the block's level, when that is on screen
        if let Some(line) = self.ed.drop_line_at_end(opener)
            && let Some(last) = visible.last()
            && last.idx + 1 == model.items.len()
        {
            places.push((line, surface.top() + last.y + last.layout.height));
        }
        if y >= own_top.max(grabbed - reach) && y <= own_bottom.min(grabbed + reach) {
            return None;
        }
        let down = y > grabbed;
        places
            .into_iter()
            // only where the line can show: a place above or below the view is no place to land yet
            .filter(|(_, at)| *at >= surface.top() && *at <= surface.bottom())
            .filter(|(_, at)| if down { *at >= own_bottom } else { *at <= own_top })
            .min_by(|a, b| (a.1 - y).abs().total_cmp(&(b.1 - y).abs()))
    }
}
