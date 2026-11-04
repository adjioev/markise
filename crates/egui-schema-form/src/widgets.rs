//! Small pieces: painted icon buttons, labels with their accessibility.

use egui::{Button, Color32, Id, Label, Pos2, Response, RichText, Role, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Up,
    Down,
    Cross,
}

/// A square icon button. The icons are painted, so they need no font of the host's.
pub(crate) fn icon_button(ui: &mut Ui, id: Id, icon: Icon, label: &str, enabled: bool) -> Response {
    let side = ui.spacing().interact_size.y;
    let r = ui
        .push_id(id, |ui| {
            ui.add_enabled(enabled, Button::new("").min_size(Vec2::splat(side)))
        })
        .inner;
    let r = r.on_hover_text(label).on_disabled_hover_text(label);
    r.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let color = ui.style().interact(&r).fg_stroke.color;
    let color = if enabled { color } else { ui.visuals().weak_text_color() };
    paint(ui, r.rect.center(), side * 0.22, icon, color);
    r
}

fn paint(ui: &Ui, c: Pos2, s: f32, icon: Icon, color: Color32) {
    let p = ui.painter();
    match icon {
        Icon::Up | Icon::Down => {
            let d = if icon == Icon::Up { -1.0 } else { 1.0 };
            let points = vec![
                c + Vec2::new(0.0, d * s),
                c + Vec2::new(-s, -d * s * 0.7),
                c + Vec2::new(s, -d * s * 0.7),
            ];
            p.add(Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Icon::Cross => {
            let stroke = Stroke::new((s * 0.35).max(1.0), color);
            p.line_segment([c + Vec2::new(-s, -s), c + Vec2::new(s, s)], stroke);
            p.line_segment([c + Vec2::new(-s, s), c + Vec2::new(s, -s)], stroke);
        }
    }
}

/// A field's label. The mark of a required field is beside it, so the label reads as it is.
pub(crate) fn field_label(ui: &mut Ui, text: &str, required: bool) -> Id {
    ui.horizontal(|ui| {
        let id = ui
            .add(Label::new(RichText::new(text).strong()).sense(Sense::hover()))
            .id;
        if required {
            ui.add(Label::new(RichText::new("*").weak()).selectable(false));
        }
        id
    })
    .inner
}

/// A reason: red text that a screen reader announces when it appears.
pub(crate) fn error_label(ui: &mut Ui, message: &str) {
    let color = ui.visuals().error_fg_color;
    let r = ui.add(Label::new(RichText::new(message).color(color).small()).wrap());
    ui.ctx().accesskit_node_builder(r.id, |n| {
        // a label keeps its text as the value; an alert's name is its label
        n.clear_value();
        n.set_label(message);
        n.set_role(Role::Alert);
        n.set_live(egui::accesskit::Live::Polite);
    });
}

/// A status line a screen reader reads when it changes.
pub(crate) fn status_label(ui: &mut Ui, message: &str) {
    let r = ui.add(Label::new(RichText::new(message).weak().small()).wrap());
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_live(egui::accesskit::Live::Polite);
    });
}
