//! The button on a block's card that opens its form (ADR 0020). It shows only for a block whose
//! package is in use and has a schema for it; a document with no package never gets here.

use std::sync::Arc;

use egui::accesskit::Role;
use egui::{Pos2, Rect, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
use markise_layout::block::Prim;
use markise_layout::{ItemKind, Visible};
use markise_package::PackageIndex;
use markise_theme::Theme;

use super::{DocViewer, ViewerOutput};

impl DocViewer {
    /// The package whose blocks have a form; `None` when there is none in use.
    pub fn set_block_index(&mut self, index: Option<Arc<PackageIndex>>) {
        self.block_index = index;
    }

    pub(super) fn block_tools_ui(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        let Some(index) = self.block_index.clone() else {
            return;
        };
        let model = self.ed.view.model().clone();
        let side = theme.line_px().min(22.0);
        for v in visible {
            let opener = model
                .items
                .get(v.idx)
                .is_some_and(|it| it.kind == (ItemKind::ContainerMarker { close: false }));
            if !opener {
                continue;
            }
            let start = model.item_start(v.idx);
            let containers = self.ed.session.doc().named_containers_at(start);
            let Some(c) = containers.last().filter(|c| index.block(&c.name).is_some()) else {
                continue;
            };
            // the card's top edge, the line a marker draws from side to side
            let edge = v.layout.prims.iter().find_map(|p| match p {
                Prim::Line { a, b, .. } if (a.y - b.y).abs() < 0.01 => Some((*a, *b)),
                _ => None,
            });
            let Some((_, b)) = edge else { continue };
            let origin = vec2(left, surface.top() + v.y);
            let top_right: Pos2 = b + origin;
            let r = Rect::from_min_size(pos2(top_right.x - side - 4.0, top_right.y + 4.0), vec2(side, side));
            if !surface.contains_rect(r) {
                continue;
            }
            let label = format!("Edit Block: {}", c.name);
            let resp = ui
                .interact(r, ui.scope_id().with(("markise-block-edit", start)), Sense::click())
                .on_hover_text("Edit Block");
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
            if resp.clicked() {
                out.open_block_form = Some(start);
                ui.ctx().request_repaint();
            }
            let tone = if resp.hovered() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let painter = ui.painter_at(surface);
            if resp.hovered() {
                painter.rect_filled(r, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            // a pencil: a slanted stroke and its tip
            let s = side * 0.28;
            let c0 = r.center();
            let stroke = Stroke::new((side * 0.09).max(1.0), tone);
            painter.line_segment([c0 + vec2(s, -s), c0 + vec2(-s * 0.6, s * 0.6)], stroke);
            painter.line_segment([c0 + vec2(-s * 0.6, s * 0.6), c0 + vec2(-s, s)], stroke);
            painter.line_segment([c0 + vec2(s * 0.45, -s * 1.1), c0 + vec2(s * 1.1, -s * 0.45)], stroke);
        }
    }
}
