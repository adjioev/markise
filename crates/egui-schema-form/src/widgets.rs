//! Small pieces: quiet painted icon buttons, labels with their accessibility.

use egui::{Color32, Id, Label, Pos2, Response, RichText, Role, Sense, Stroke, Ui, Vec2, WidgetInfo, vec2};

use crate::style::{LABEL_SIZE, REASON_SIZE, ROW_HEIGHT, Style, focus_ring};

/// The width of an icon button.
pub(crate) const ICON_WIDTH: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Up,
    Down,
    Cross,
}

/// A quiet icon button: a glyph, stronger on hover. The icons are painted, so they need no font of
/// the host's.
pub(crate) fn icon_button(ui: &mut Ui, id: Id, icon: Icon, label: &str, enabled: bool, s: Style) -> Response {
    let r = ui
        .push_id(id, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                ui.allocate_response(Vec2::new(ICON_WIDTH, ROW_HEIGHT), Sense::click())
            })
            .inner
        })
        .inner;
    let r = r.on_hover_text(label).on_disabled_hover_text(label);
    r.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    if r.hovered() && enabled {
        ui.painter().rect_filled(r.rect, 6.0, s.hover);
    }
    focus_ring(ui, &r, s);
    let color = match () {
        () if !enabled => s.line,
        () if r.hovered() => s.fg,
        () => s.muted,
    };
    paint(ui, r.rect.center(), icon, color);
    r
}

fn paint(ui: &Ui, c: Pos2, icon: Icon, color: Color32) {
    let p = ui.painter();
    let st = Stroke::new(1.6, color);
    match icon {
        Icon::Cross => {
            p.line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], st);
            p.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], st);
        }
        Icon::Up | Icon::Down => {
            let d = if icon == Icon::Up { 1.0 } else { -1.0 };
            p.line_segment([c + vec2(0.0, 6.0 * d), c + vec2(0.0, -6.0 * d)], st);
            p.line_segment([c + vec2(-4.5, -1.5 * d), c + vec2(0.0, -6.0 * d)], st);
            p.line_segment([c + vec2(4.5, -1.5 * d), c + vec2(0.0, -6.0 * d)], st);
        }
    }
}

/// A field's label. The mark of a required field is beside it, so the label reads as it is.
pub(crate) fn field_label(ui: &mut Ui, text: &str, required: bool, s: Style) -> Id {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let id = ui
            .add(Label::new(RichText::new(text).size(LABEL_SIZE).strong().color(s.muted)).sense(Sense::hover()))
            .id;
        if required {
            ui.add(Label::new(RichText::new("*").size(LABEL_SIZE).color(s.muted)).selectable(false));
        }
        id
    })
    .inner
}

/// A reason: danger-coloured text that a screen reader announces when it appears.
pub(crate) fn error_label(ui: &mut Ui, message: &str, s: Style) {
    let r = ui.add(Label::new(RichText::new(message).color(s.danger).size(REASON_SIZE)).wrap());
    ui.ctx().accesskit_node_builder(r.id, |n| {
        // a label keeps its text as the value; an alert's name is its label
        n.clear_value();
        n.set_label(message);
        n.set_role(Role::Alert);
        n.set_live(egui::accesskit::Live::Polite);
    });
}

/// A status line a screen reader reads when it changes.
pub(crate) fn status_label(ui: &mut Ui, message: &str, s: Style) {
    let r = ui.add(Label::new(RichText::new(message).color(s.muted).size(REASON_SIZE)).wrap());
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_live(egui::accesskit::Live::Polite);
    });
}
