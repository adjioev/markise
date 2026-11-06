//! What a reader works with in a block in Practice (ADR 0024): its answers and its buttons. Nothing here
//! touches the document, and nothing here is labelled with a mark before the block is checked.

use egui::accesskit::Role;
use egui::{Rect, Sense, Stroke, Ui, UiBuilder, WidgetInfo, pos2, vec2};
use markise_doc::Selection;
use markise_layout::{PracticeItem, Visible};

use super::DocViewer;

impl DocViewer {
    pub(super) fn practice_ui(&mut self, ui: &mut Ui, visible: &[Visible], surface: Rect, left: f32) {
        if !self.ed.view.practice_on() {
            return;
        }
        for v in visible {
            match self.ed.view.practice_item_of(v.idx) {
                Some(PracticeItem::Answer { selected, correct }) => {
                    self.answer_ui(ui, v, surface, left, selected, correct);
                }
                Some(PracticeItem::Row) => self.buttons_ui(ui, v, surface, left),
                _ => {}
            }
        }
    }

    fn answer_ui(&mut self, ui: &mut Ui, v: &Visible, surface: Rect, left: f32, selected: bool, correct: Option<bool>) {
        let start = self.ed.view.model().item_start(v.idx);
        let Some(block) = self.ed.view.native_item(v.idx) else {
            return;
        };
        let Some(answer) = (block.block.practice.as_ref()).and_then(|parts| {
            let n = parts.answer_at((start - block.range.start) as u32)?;
            parts.answers.get(n)
        }) else {
            return;
        };
        let y = surface.top() + v.y;
        let r = Rect::from_min_max(
            pos2(left, y + v.layout.gap),
            pos2(left + self.ed.view.width(), y + v.layout.height),
        );
        if !surface.contains_rect(r) {
            return;
        }
        let single = block.block.view.single_answer;
        let kind = if single { Role::RadioButton } else { Role::CheckBox };
        // before a check the words are the answer's own, as in the source they have no mark
        let label = match correct {
            None => answer.text.to_string(),
            Some(true) => format!("{}: correct", answer.text),
            Some(false) => format!("{}: wrong", answer.text),
        };
        let id = ui.scope_id().with(("markise-practice-answer", start));
        let resp = ui.interact(r, id, Sense::click());
        resp.widget_info(|| WidgetInfo::selected(kind, correct.is_none(), selected, &label));
        if resp.hovered() {
            self.pointer_on_block_control = true;
        }
        if resp.clicked() {
            self.ed.practice_press(start);
            self.ed.set_selection(Selection::caret(start));
            self.ed.refresh_focus();
            self.request_refocus();
            ui.ctx().request_repaint();
        }
        // where the keyboard is: Space works on this one
        let p = self.ed.session.selection().primary();
        if p.is_caret() && p.focus.offset >= start && p.focus.offset <= block.range.start + answer.range.end as usize {
            let stroke = Stroke::new(1.5, ui.visuals().selection.stroke.color);
            let ring = r.shrink2(vec2(self.answer_ring_inset(), 0.0));
            ui.painter_at(surface)
                .rect_stroke(ring, 3.0, stroke, egui::StrokeKind::Inside);
        }
    }

    /// The ring around an answer keeps inside the block's frame.
    fn answer_ring_inset(&self) -> f32 {
        4.0
    }

    fn buttons_ui(&mut self, ui: &mut Ui, v: &Visible, surface: Rect, left: f32) {
        let Some(row) = v.layout.practice_row else { return };
        let row = row.translate(vec2(left, surface.top() + v.y));
        if !surface.contains_rect(row) {
            return;
        }
        let Some(block) = self.ed.view.native_item(v.idx) else {
            return;
        };
        let Some(parts) = block.block.practice.as_ref() else {
            return;
        };
        let opener = block.range.start;
        let show = self.ed.practice_show(opener);
        let checked = show.is_some_and(|s| s.checked);
        let answered = show.is_some_and(|s| !s.selected.is_empty());
        let (label, enabled) = match (checked, parts.has_choices) {
            (true, _) => ("Reset", true),
            (false, true) => ("Check", answered),
            (false, false) => ("Show answer", true),
        };
        let builder = UiBuilder::new()
            .id_salt(("markise-practice-row", opener))
            .max_rect(row)
            .layout(egui::Layout::left_to_right(egui::Align::Center));
        let resp = ui
            .scope_builder(builder, |ui| ui.add_enabled(enabled, egui::Button::new(label)))
            .inner;
        if resp.hovered() {
            self.pointer_on_block_control = true;
        }
        if resp.clicked() {
            if checked {
                self.ed.practice_reset(opener);
            } else {
                self.ed.practice_check(opener);
            }
            self.request_refocus();
            ui.ctx().request_repaint();
        }
    }
}
