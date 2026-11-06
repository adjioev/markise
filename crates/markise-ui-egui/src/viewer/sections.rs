//! Sections on screen beyond their layout (ADR 0026): whether the document has them, a metadata
//! block's switch between its plaque and a card, and what a warning says.

use std::sync::Arc;

use egui::accesskit::Role;
use egui::{Rect, Sense, Ui, WidgetInfo, pos2, vec2};
use markise_engine::sections::{Dialect, section_dialect};
use markise_layout::{ItemKind, Visible};
use markise_theme::Theme;

use super::{DocViewer, ViewerOutput};

impl DocViewer {
    /// Reads the document's dialect: a comparison per frame unless the document or its package changed.
    pub fn follow_section_dialect(&mut self) {
        let package = self.block_index.as_ref().map_or(0, |p| Arc::as_ptr(p) as usize);
        let at = (self.ed.changes(), package);
        if self.section_dialect_read_at == Some(at) {
            return;
        }
        self.section_dialect_read_at = Some(at);
        let doc = self.ed.session.doc();
        let dialect = section_dialect(doc.authoring().as_deref(), self.block_index.as_deref());
        let on = dialect.as_ref().is_some_and(Dialect::has_sections);
        self.section_dialect = dialect;
        self.ed.view.set_sections(on);
    }

    /// The document's dialect, when anything names one.
    pub fn section_dialect(&self) -> Option<&Dialect> {
        self.section_dialect.as_ref()
    }

    pub(super) fn sections_ui(
        &mut self,
        ui: &mut Ui,
        visible: &[Visible],
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        self.section_buttons.clear();
        if !self.ed.view.sections_on() {
            return;
        }
        let model = self.ed.view.model().clone();
        let side = theme.line_px().min(22.0);
        for v in visible {
            let origin = vec2(left, surface.top() + v.y);
            let opener = model
                .items
                .get(v.idx)
                .is_some_and(|it| it.kind == (ItemKind::ContainerMarker { close: false }));
            if !opener {
                continue;
            }
            let start = model.item_start(v.idx);
            if !self.ed.view.is_section_metadata(start) {
                continue;
            }
            // the top right corner of the plaque, or of the card it opened as
            let corner = match (v.layout.plaque, v.layout.card_top) {
                (Some(rect), _) => rect.right_top() + origin,
                (None, Some((_, b))) => b + origin,
                _ => continue,
            };
            if let Some(rect) = v.layout.warning {
                self.section_warning_tip(ui, v, rect.translate(origin), surface);
            }
            let r = Rect::from_min_size(pos2(corner.x - side - 4.0, corner.y + 2.0), vec2(side, side));
            if !surface.contains_rect(r) {
                continue;
            }
            self.section_buttons.push(r);
            let next = self.ed.next_block_view(start);
            self.block_view_button(ui, r, surface, start, next, "section metadata", out);
            if ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p)) {
                self.pointer_on_block_control = true;
            }
        }
    }

    fn section_warning_tip(&self, ui: &mut Ui, v: &Visible, r: Rect, surface: Rect) {
        let Some(why) = self.ed.view.section_warnings(v.idx) else {
            return;
        };
        if !surface.contains_rect(r) {
            return;
        }
        let all = why.join("\n");
        let id = ui.scope_id().with(("markise-section-warning", v.idx));
        let resp = ui.interact(r, id, Sense::hover()).on_hover_text(&all);
        resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, format!("Section metadata: {all}")));
    }
}
