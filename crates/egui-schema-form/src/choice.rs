//! Choice buttons: one button per option, icon above label, both from data. What a press means
//! is `schema_form_core`'s; here is the drawing, the keyboard and the screen reader.

use std::sync::Arc;

use egui::{
    Color32, Galley, Id, Rect, Response, Role, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo, pos2,
};
use schema_form_core::{
    ChoiceOption, ChoiceShape, FieldSpec, Selection, selected_options, toggle_option, unknown_rows, write_selected,
};
use serde_json::Value;

use crate::draw::{Cx, may_remove, not_editable};
use crate::host::Thumbnail;
use crate::widgets::{Icon, error_label, icon_button};

/// The narrowest a button with an icon gets before a row holds fewer of them.
const MIN_ICON_WIDTH: f32 = 64.0;
const MIN_LABEL_WIDTH: f32 = 96.0;
/// Buttons per row when the hints do not say.
const DEFAULT_ICON_COLUMNS: usize = 4;
const DEFAULT_LABEL_COLUMNS: usize = 2;

pub(crate) struct Choice<'a> {
    pub field: &'a FieldSpec,
    pub options: &'a [ChoiceOption],
    pub selection: Selection,
    pub shape: ChoiceShape,
    pub columns: Option<u32>,
}

/// The options per row: as many as fit, up to the columns asked for. In a choice that has icons,
/// an option without one takes a row of its own, wide and low, as it says something else.
fn rows_of(options: &[ChoiceOption], columns: usize) -> Vec<std::ops::Range<usize>> {
    let any_icon = options.iter().any(|o| o.icon.is_some());
    let mut out = Vec::new();
    let mut start = 0;
    while start < options.len() {
        let same = |o: &ChoiceOption| !any_icon || o.icon.is_some();
        let mut end = start + 1;
        if same(&options[start]) {
            while end < options.len() && end - start < columns && same(&options[end]) {
                end += 1;
            }
        }
        out.push(start..end);
        start = end;
    }
    out
}

pub(crate) fn draw(ui: &mut Ui, c: &Choice, value: &mut Value, id: Id, path: &str, cx: &mut Cx) {
    let readable = match c.shape {
        ChoiceShape::String => value.is_string(),
        _ => value.is_array(),
    };
    if !readable {
        not_editable(ui, cx);
        return;
    }
    let any_icon = c.options.iter().any(|o| o.icon.is_some());
    let gap = ui.spacing().item_spacing.x;
    let width = ui.available_width();
    let min = if any_icon { MIN_ICON_WIDTH } else { MIN_LABEL_WIDTH };
    let asked = c.columns.map_or(
        if any_icon {
            DEFAULT_ICON_COLUMNS
        } else {
            DEFAULT_LABEL_COLUMNS
        },
        |n| n as usize,
    );
    let fit = (((width + gap) / (min + gap)).floor() as usize).max(1);
    let columns = asked.min(fit);
    let cell = (width - gap * (columns as f32 - 1.0)) / columns as f32;

    let mut selected = selected_options(c.shape, c.options, value);
    let mut pressed = None;
    let mut first_button = None;
    let full = cell * columns as f32 + gap * (columns as f32 - 1.0);
    for row in rows_of(c.options, columns) {
        let width_of = |o: &ChoiceOption| if any_icon && o.icon.is_none() { full } else { cell };
        let height = (row.clone())
            .map(|i| button_height(ui, &c.options[i], width_of(&c.options[i])))
            .fold(0.0, f32::max);
        ui.horizontal(|ui| {
            for i in row {
                let size = Vec2::new(width_of(&c.options[i]), height);
                let r = option_button(ui, c, i, selected[i], id, size, cx);
                first_button.get_or_insert(r.id);
                if r.clicked() {
                    pressed = Some(i);
                }
            }
        });
    }
    if let Some(i) = pressed {
        toggle_option(c.options, c.selection, &mut selected, i);
        write_selected(c.shape, c.options, &selected, value);
        cx.changed = true;
        cx.status.clear();
    }
    draw_unknown_rows(ui, c, value, id, first_button, cx);
    for (at, message) in cx.messages_inside(path) {
        let row = at.split('/').next().and_then(|r| r.parse::<usize>().ok());
        let name = row.and_then(|r| row_text(value, r));
        match name {
            Some(n) if !n.is_empty() => error_label(ui, &format!("{n}: {message}")),
            _ => error_label(ui, &message),
        }
    }
}

fn row_text(value: &Value, row: usize) -> Option<&str> {
    let e = value.as_array()?.get(row)?;
    e.as_str().or_else(|| e.get("text")?.as_str())
}

fn pad(ui: &Ui) -> f32 {
    ui.spacing().button_padding.y.max(4.0)
}

fn icon_side(ui: &Ui) -> f32 {
    ui.text_style_height(&TextStyle::Body) * 1.8
}

fn label_galley(ui: &Ui, o: &ChoiceOption, width: f32, color: Color32) -> Arc<Galley> {
    let font = TextStyle::Body.resolve(ui.style());
    ui.painter().layout(o.label.clone(), font, color, width - pad(ui) * 2.0)
}

fn button_height(ui: &Ui, o: &ChoiceOption, width: f32) -> f32 {
    let text = label_galley(ui, o, width, Color32::PLACEHOLDER).size().y;
    match o.icon {
        Some(_) => icon_side(ui) + text + pad(ui) * 3.0,
        None => text + pad(ui) * 2.0,
    }
}

fn option_button(ui: &mut Ui, c: &Choice, i: usize, on: bool, id: Id, size: Vec2, cx: &mut Cx) -> Response {
    let o = &c.options[i];
    // the id comes from the option, not its place, so that the focus stays put
    let r = ui
        .push_id(id.with(("option", &o.value)), |ui| {
            ui.allocate_response(size, Sense::click())
        })
        .inner;
    let role = match c.selection {
        Selection::Single => Role::RadioButton,
        Selection::Multiple => Role::CheckBox,
    };
    r.widget_info(|| WidgetInfo::selected(role, true, on, &o.label));
    cx.track_control(&r);
    let visuals = ui.style().interact_selectable(&r, on);
    let selection = ui.visuals().selection;
    let fill = if on { selection.bg_fill } else { visuals.weak_bg_fill };
    let stroke = match () {
        () if r.has_focus() => Stroke::new(2.0, selection.stroke.color),
        () if on => selection.stroke,
        () => visuals.bg_stroke,
    };
    let text = if on {
        selection.stroke.color
    } else {
        visuals.text_color()
    };
    ui.painter()
        .rect(r.rect, visuals.corner_radius, fill, stroke, StrokeKind::Inside);
    let state = o.icon.as_ref().map_or(Thumbnail::None, |n| cx.host.icon(n));
    let picture = match state {
        Thumbnail::Ready { texture, size } => Some((texture, size, Color32::WHITE)),
        Thumbnail::Mask { texture, size } => Some((texture, size, text)),
        _ => None,
    };
    let top = r.rect.top() + pad(ui);
    if let Some((texture, size, tint)) = picture {
        let side = icon_side(ui);
        let scale = (side / size.x.max(1.0)).min(side / size.y.max(1.0));
        let at = Rect::from_center_size(pos2(r.rect.center().x, top + side / 2.0), size * scale);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().image(texture, at, uv, tint);
    }
    let galley = label_galley(ui, o, size.x, text);
    let y = match o.icon {
        Some(_) => r.rect.bottom() - pad(ui) - galley.size().y,
        None => r.rect.center().y - galley.size().y / 2.0,
    };
    let at = pos2(r.rect.center().x - galley.size().x / 2.0, y);
    ui.painter().galley(at, galley, text);
    match state {
        Thumbnail::Failed(why) => r.on_hover_text(why),
        _ => r,
    }
}

/// Rows of no option: their text, whether they are marked, and a way to remove them. What they
/// say is kept unless the author removes it, here or in the Markdown.
fn draw_unknown_rows(ui: &mut Ui, c: &Choice, value: &mut Value, id: Id, fallback: Option<Id>, cx: &mut Cx) {
    let unknown = unknown_rows(c.shape, c.options, value);
    let Some(rows) = value.as_array_mut() else {
        return;
    };
    let mut remove = None;
    let n = rows.len();
    for (shown, &row) in unknown.iter().enumerate() {
        let e = &rows[row];
        let text = e.as_str().or_else(|| e.get("text")?.as_str()).unwrap_or("");
        let marked = e.get("correct") == Some(&Value::Bool(true));
        ui.horizontal(|ui| {
            if c.shape == ChoiceShape::Marks {
                let mut mark = marked;
                let r = ui.add_enabled(false, egui::Checkbox::new(&mut mark, ""));
                let name = format!("{}: {text}", cx.texts.correct);
                r.widget_info(|| WidgetInfo::selected(Role::CheckBox, false, marked, &name));
            }
            ui.label(text);
            let label = format!("{}: {text}", cx.texts.remove);
            let enabled = may_remove(&c.field.rule.limits, n);
            let r = icon_button(ui, id.with(("unknown", shown)), Icon::Cross, &label, enabled);
            cx.track(&r);
            if r.clicked() {
                remove = Some(row);
            }
        });
    }
    if let Some(row) = remove {
        rows.remove(row);
        cx.changed = true;
        if let Some(button) = fallback {
            ui.ctx().memory_mut(|m| m.request_focus(button));
        }
    }
}
