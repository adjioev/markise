//! How the form looks: its colours and the controls it paints itself, so that a field is a field and
//! a selected answer is plainly selected, in any theme. Sizes are in points, so the host's zoom
//! scales them.

use egui::epaint::RectShape;
use egui::{
    Color32, FontId, Frame, Id, Margin, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, pos2,
    vec2,
};

/// The form's colours. Give it the host's with [`crate::SchemaForm::with_style`]; without that it
/// takes [`Style::for_visuals`] each frame, so it still follows light and dark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub fg: Color32,
    pub muted: Color32,
    pub field: Color32,
    pub line: Color32,
    pub hover: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub danger: Color32,
    /// Text on a filled accent button.
    pub on_accent: Color32,
}

const fn rgb(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

impl Style {
    pub const DARK: Style = Style {
        fg: rgb(0xececee),
        muted: rgb(0xa8a8b0),
        field: rgb(0x222327),
        line: rgb(0x3c3d44),
        hover: rgb(0x2b2840),
        accent: rgb(0xa08af0),
        accent_soft: rgb(0x2f2a4a),
        danger: rgb(0xf0837a),
        on_accent: Color32::BLACK,
    };

    pub const LIGHT: Style = Style {
        fg: rgb(0x1d1d1f),
        muted: rgb(0x5f5f66),
        field: rgb(0xffffff),
        line: rgb(0xd9d9df),
        hover: rgb(0xf0eefb),
        accent: rgb(0x5d3fba),
        accent_soft: rgb(0xece7fb),
        danger: rgb(0xc0392b),
        on_accent: Color32::WHITE,
    };

    pub fn for_visuals(v: &egui::Visuals) -> Style {
        if v.dark_mode { Style::DARK } else { Style::LIGHT }
    }

    /// This style with another accent: the soft fill and the hover follow it, and the text on it
    /// stays readable.
    pub fn with_accent(mut self, accent: Color32) -> Style {
        self.accent = accent;
        self.accent_soft = mix(self.field, accent, 0.22);
        self.hover = mix(self.field, accent, 0.12);
        let [r, g, b, _] = accent.to_array();
        let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        self.on_accent = if luma > 140.0 { Color32::BLACK } else { Color32::WHITE };
        self
    }
}

/// `b` over `a`, by `t` of 1.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// The inner margin of a text field.
pub(crate) const FIELD_MARGIN: Margin = Margin::symmetric(10, 7);
const FIELD_RADIUS: f32 = 8.0;
pub(crate) const CARD_RADIUS: u8 = 10;
pub(crate) const CARD_MARGIN: i8 = 10;
/// A row's control height: as high as a one-line field.
pub(crate) const ROW_HEIGHT: f32 = 32.0;
pub(crate) const LABEL_SIZE: f32 = 12.5;
pub(crate) const NOTE_SIZE: f32 = 11.5;
pub(crate) const REASON_SIZE: f32 = 12.0;

/// What a field's box says about it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FieldLook {
    pub invalid: bool,
    pub marked: bool,
}

/// Paints a field's box under what was already drawn: `slot` is a shape reserved before it.
pub(crate) fn paint_field_box(
    ui: &Ui,
    slot: egui::layers::ShapeIdx,
    rect: Rect,
    look: FieldLook,
    focus: bool,
    s: Style,
) {
    let stroke = if look.invalid {
        Stroke::new(1.5, s.danger)
    } else if focus {
        Stroke::new(1.5, s.accent)
    } else {
        Stroke::new(1.0, s.line)
    };
    let fill = if look.marked { s.accent_soft } else { s.field };
    ui.painter().set(
        slot,
        RectShape::new(rect, FIELD_RADIUS, fill, stroke, StrokeKind::Inside),
    );
}

/// A framed, rounded part of the form: a picture, a row of a group.
pub(crate) fn sub_card<R>(ui: &mut Ui, s: Style, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::NONE
        .stroke(Stroke::new(1.0, s.line))
        .corner_radius(CARD_RADIUS)
        .inner_margin(Margin::same(CARD_MARGIN))
        .show(ui, add)
        .inner
}

/// A focus ring for keyboard users.
pub(crate) fn focus_ring(ui: &Ui, r: &Response, s: Style) {
    if r.has_focus() {
        ui.painter()
            .rect_stroke(r.rect, 6.0, Stroke::new(1.5, s.accent), StrokeKind::Inside);
    }
}

/// A text button in the form's look; `primary` is filled with the accent. For a host's controls
/// around the form as well as the form's own.
pub fn button(ui: &mut Ui, label: &str, primary: bool, enabled: bool, s: Style) -> Response {
    let color = match (enabled, primary) {
        (false, _) => s.muted,
        (true, true) => s.on_accent,
        (true, false) => s.fg,
    };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(13.5), color);
    let size = vec2(g.size().x + 24.0, ROW_HEIGHT);
    let r = ui
        .add_enabled_ui(enabled, |ui| ui.allocate_response(size, Sense::click()))
        .inner;
    r.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let fill = match (primary && enabled, r.hovered() && enabled) {
        (true, _) => s.accent,
        (false, true) => s.hover,
        (false, false) => s.field,
    };
    let stroke = match () {
        () if r.has_focus() => Stroke::new(2.0, if primary { s.fg } else { s.accent }),
        () if primary && enabled => Stroke::NONE,
        () => Stroke::new(1.0, s.line),
    };
    ui.painter()
        .rect(r.rect, FIELD_RADIUS, fill, stroke, StrokeKind::Inside);
    ui.painter().galley(r.rect.center() - g.size() / 2.0, g, color);
    r
}

/// The quiet `+ add` of a list.
pub(crate) fn add_button(ui: &mut Ui, id: Id, label: &str, enabled: bool, s: Style) -> Response {
    let color = if enabled { s.accent } else { s.line };
    let text = format!("+ {label}");
    let g = ui.painter().layout_no_wrap(text, FontId::proportional(13.0), color);
    let size = vec2(g.size().x + 20.0, ROW_HEIGHT - 2.0);
    // an explicit scope: its id stays the same as rows come and go, so it can take the focus
    let r = ui
        .scope_builder(egui::UiBuilder::new().scope_id(id), |ui| {
            ui.add_enabled_ui(enabled, |ui| ui.allocate_response(size, Sense::click()))
                .inner
        })
        .inner;
    r.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    if r.hovered() && enabled {
        ui.painter().rect_filled(r.rect, 6.0, s.hover);
    }
    focus_ring(ui, &r, s);
    ui.painter()
        .galley(r.rect.left_center() + vec2(10.0, -g.size().y / 2.0), g, color);
    r
}

/// The mark of an answer or a checkbox: a circle with a dot, or a box with a tick. Clear in accent
/// when on. The response is the whole of `size`, so a label beside it takes clicks too.
pub(crate) fn paint_mark(ui: &Ui, center: egui::Pos2, round: bool, on: bool, enabled: bool, s: Style) {
    let edge = if enabled { s.muted } else { s.line };
    let accent = if enabled { s.accent } else { s.muted };
    let p = ui.painter();
    if round {
        p.circle_stroke(center, 7.5, Stroke::new(1.5, if on { accent } else { edge }));
        if on {
            p.circle_filled(center, 4.0, accent);
        }
    } else {
        let b = Rect::from_center_size(center, Vec2::splat(15.0));
        if on {
            p.rect_filled(b, 4.0, accent);
            let tick = Stroke::new(1.8, s.field);
            p.line_segment([center + vec2(-3.5, 0.0), center + vec2(-1.0, 3.0)], tick);
            p.line_segment([center + vec2(-1.0, 3.0), center + vec2(4.0, -3.0)], tick);
        } else {
            p.rect_stroke(b, 4.0, Stroke::new(1.5, edge), StrokeKind::Inside);
        }
    }
}

/// An answer's mark as a control of its own.
pub(crate) fn mark_control(ui: &mut Ui, round: bool, on: bool, enabled: bool, name: &str, s: Style) -> Response {
    let r = ui
        .add_enabled_ui(enabled, |ui| {
            ui.allocate_response(vec2(22.0, ROW_HEIGHT), Sense::click())
        })
        .inner;
    let role = if round { Role::RadioButton } else { Role::CheckBox };
    r.widget_info(|| WidgetInfo::selected(role, enabled, on, name));
    if r.hovered() && enabled {
        ui.painter().rect_filled(r.rect.shrink(2.0), 6.0, s.hover);
    }
    focus_ring(ui, &r, s);
    paint_mark(ui, r.rect.center(), round, on, enabled, s);
    r
}

/// A checkbox with its label beside it, the whole of it taking the click.
pub(crate) fn labelled_checkbox(ui: &mut Ui, on: bool, label: &str, s: Style) -> Response {
    let room = (ui.available_width() - 24.0).max(1.0);
    let g = ui
        .painter()
        .layout(label.to_string(), FontId::proportional(14.0), s.fg, room);
    let size = vec2(24.0 + g.size().x, g.size().y.max(20.0));
    let r = ui.allocate_response(size, Sense::click());
    r.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, on, label));
    focus_ring(ui, &r, s);
    paint_mark(ui, pos2(r.rect.left() + 10.0, r.rect.center().y), false, on, true, s);
    let at = pos2(r.rect.left() + 24.0, r.rect.center().y - g.size().y / 2.0);
    ui.painter().galley(at, g, s.fg);
    r
}
