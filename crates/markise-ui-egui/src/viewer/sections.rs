//! Sections on screen beyond their layout (ADR 0026): whether the document has them, a metadata
//! block's switch between its plaque and a card, and what a warning says.

use std::sync::Arc;

use egui::accesskit::Role;
use egui::{Rect, Sense, Ui, WidgetInfo, pos2, vec2};
use markise_engine::sections::{Dialect, section_dialect};
use markise_layout::{ItemKind, Prim, Visible};
use markise_theme::Theme;

use super::{DocViewer, ViewerOutput};

/// What a control on a section's divider asks for, by the section's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionAct {
    MoveUp(usize),
    MoveDown(usize),
    Delete(usize),
}

/// The status bar's words, and the state they are for: the document, the section, whether it is whole.
pub(super) type SectionStatus = ((u64, usize, bool), (String, String));

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

    /// What the status bar says of the current section, and more on hover. The caret's section while
    /// the caret is on screen, else the one at the top of the screen. Per frame: a comparison unless the
    /// document or the section changed.
    pub fn section_status(&mut self, partial: bool) -> Option<(String, String)> {
        if self.section_dialect.as_ref().is_some_and(Dialect::is_invalid) {
            return Some((
                "§ off".to_string(),
                "The front matter's authoring is empty or is no name of a dialect, so sections are off. \
                 A package's value is not used instead."
                    .to_string(),
            ));
        }
        if !self.ed.view.sections_on() {
            return None;
        }
        let view = &self.ed.view;
        let caret = self.ed.session.selection().primary().focus.offset;
        let model = view.model();
        let on_screen = view.last_visible.contains(&model.item_at_offset(caret));
        let at = if on_screen {
            caret
        } else {
            model.item_start(view.anchor().0.min(view.len().saturating_sub(1)))
        };
        let index = self.ed.session.doc().section_at(at).index;
        let key = (self.ed.changes(), index, partial);
        if let Some((k, said)) = &self.section_status
            && *k == key
        {
            return Some(said.clone());
        }
        let count = self.ed.session.doc().section_count();
        let mut name = format!("§ {}", view.section_name(index));
        // the last section of a part goes on in what has yet to load
        let last = index + 1 == count;
        if partial && last {
            name.push('…');
        }
        let total = if partial {
            format!("{count}+")
        } else {
            count.to_string()
        };
        let origin = self.section_dialect.as_ref().map_or_else(String::new, Dialect::origin);
        let about = format!("Section {} of {total}. Sections are on by {origin}.", index + 1);
        let said = (name, about);
        self.section_status = Some((key, said.clone()));
        Some(said)
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
        let hovered = self.hovered_block(ui, visible, surface);
        for v in visible {
            let origin = vec2(left, surface.top() + v.y);
            if (model.items.get(v.idx)).is_some_and(|it| it.kind == ItemKind::Rule) {
                self.divider_controls(ui, v, surface, origin.x, theme, out);
                continue;
            }
            let opener = model
                .items
                .get(v.idx)
                .is_some_and(|it| it.kind == (ItemKind::ContainerMarker { close: false }));
            if !opener {
                continue;
            }
            let start = model.item_start(v.idx);
            if !self.ed.view.is_section_metadata(start) {
                // a metadata block where none can be says so
                if let Some(rect) = v.layout.warning {
                    self.section_warning_tip(ui, v, rect.translate(origin), surface);
                }
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
            // Read has no block controls (ADR 0024); a plaque's say only when asked for, as a block's do
            if self.ed.read_mode() {
                continue;
            }
            self.section_buttons.push(r);
            let show = self.block_controls_show(ui, start, hovered);
            let next = self.ed.next_block_view(start);
            self.block_view_button(ui, r, surface, start, next, "section metadata", show, out);
            if ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p)) {
                self.pointer_on_block_control = true;
            }
        }
    }

    /// A boundary's quiet controls, under the pointer or for the caret's section as a block's are (ADR 0026),
    /// at the right end of its row. Read has none. Only what can be done has a control.
    fn divider_controls(
        &mut self,
        ui: &mut Ui,
        v: &Visible,
        surface: Rect,
        left: f32,
        theme: &Theme,
        out: &mut ViewerOutput,
    ) {
        if self.ed.read_mode() || self.ed.session.read_only().is_some() {
            return;
        }
        let start = self.ed.view.model().item_start(v.idx);
        let section = self.ed.session.doc().section_at(start);
        if section.boundary.is_none_or(|b| b.start != start) {
            return;
        }
        let k = section.index;
        let side = (theme.line_px() * 0.85).min(20.0);
        // on the divider's line, which is not the middle of an item with a gap above it
        let line = (v.layout.prims.iter()).find_map(|p| match p {
            Prim::Line { a, .. } => Some(a.y),
            _ => None,
        });
        let y = surface.top() + v.y + line.unwrap_or(v.layout.height * 0.5) - side * 0.5;
        let right = left + self.ed.view.width();
        let row = Rect::from_min_max(
            pos2(surface.left(), surface.top() + v.y),
            pos2(surface.right(), surface.top() + v.y + v.layout.height),
        );
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over_row = pointer.is_some_and(|p| row.contains(p))
            && pointer.and_then(|p| ui.ctx().layer_id_at(p)) == Some(ui.layer_id());
        let caret = self.ed.session.selection().primary().focus.offset;
        let in_it = self.ed.session.doc().section_at(caret).index == k;
        let acts = [
            (
                self.ed.can_move_section(k, true),
                "Move Section Up",
                SectionAct::MoveUp(k),
            ),
            (
                self.ed.can_move_section(k, false),
                "Move Section Down",
                SectionAct::MoveDown(k),
            ),
            (self.ed.can_delete_section(k), "Delete Section", SectionAct::Delete(k)),
        ];
        let mut x = right;
        for (i, (can, label, act)) in acts.into_iter().enumerate().rev() {
            x -= side + 2.0;
            if !can {
                continue;
            }
            let r = Rect::from_min_size(pos2(x, y), vec2(side, side));
            if !surface.contains_rect(r) {
                continue;
            }
            let id = ui.scope_id().with(("markise-section-control", label, start));
            let resp = ui.interact(r, id, Sense::click()).on_hover_text(label);
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
            self.section_buttons.push(r);
            if resp.clicked() {
                out.section_act = Some(act);
                ui.ctx().request_repaint();
            }
            let on_it = pointer.is_some_and(|p| r.contains(p));
            if on_it {
                self.pointer_on_block_control = true;
            }
            if !(over_row || in_it || resp.has_focus()) {
                continue;
            }
            let tone = if resp.hovered() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let painter = ui.painter_at(surface);
            let ground = if resp.hovered() {
                ui.visuals().widgets.hovered.weak_bg_fill
            } else {
                ui.visuals().window_fill().gamma_multiply(0.85)
            };
            painter.rect_filled(r, 4.0, ground);
            let stroke = egui::Stroke::new((side * 0.08).max(1.0), tone);
            let (c, s) = (r.center(), side * 0.22);
            match i {
                0 => {
                    painter.add(egui::Shape::line(
                        vec![c + vec2(-s, s * 0.5), c + vec2(0.0, -s * 0.5), c + vec2(s, s * 0.5)],
                        stroke,
                    ));
                }
                1 => {
                    painter.add(egui::Shape::line(
                        vec![c + vec2(-s, -s * 0.5), c + vec2(0.0, s * 0.5), c + vec2(s, -s * 0.5)],
                        stroke,
                    ));
                }
                _ => super::block_tools::paint_bin(&painter, r, side, tone),
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
