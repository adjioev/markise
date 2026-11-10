//! Choice buttons: one button per option, icon above label, both from data. What a press means
//! is `schema_form_core`'s; here is the drawing, the keyboard and the screen reader.

use std::sync::Arc;

use egui::{Color32, FontId, Galley, Id, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, pos2};
use schema_form_core::{
    ChoiceOption, ChoiceShape, FieldSpec, Selection, selected_options, toggle_option, unknown_rows, write_selected,
};
use serde_json::Value;

use crate::draw::{Cx, may_remove, not_editable};
use crate::host::Thumbnail;
use crate::style::{CARD_RADIUS, Style, paint_mark};
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
        let before = value.as_array().cloned();
        write_selected(c.shape, c.options, &selected, value);
        if let (Some(old), Some(new)) = (before, value.as_array()) {
            follow_rows(cx, id, &old, new);
        }
        cx.changed = true;
        cx.status.clear();
    }
    draw_unknown_rows(ui, c, value, id, first_button, cx);
    for (at, message) in cx.messages_inside(path) {
        let row = at.split('/').next().and_then(|r| r.parse::<usize>().ok());
        let name = row.and_then(|r| row_text(value, r));
        match name {
            Some(n) if !n.is_empty() => error_label(ui, &format!("{n}: {message}"), cx.style),
            _ => error_label(ui, &message, cx.style),
        }
    }
}

/// The rows' keys after a press: a press only marks rows, adds new ones (with no `id` key, so like no
/// row read before) and takes away all rows of an option, so rows that stay are found in order, by all
/// but their mark. A row keeps its key, and with it its identity to a host.
fn follow_rows(cx: &mut Cx, id: Id, old: &[Value], new: &[Value]) {
    let same = |a: &Value, b: &Value| match (a.as_object(), b.as_object()) {
        (Some(x), Some(y)) => {
            let rest_x = x.iter().filter(|(k, _)| k.as_str() != "correct");
            let rest_y = y.iter().filter(|(k, _)| k.as_str() != "correct");
            rest_x.eq(rest_y)
        }
        _ => a == b,
    };
    let old_keys = cx.rows.keys(id, old.len());
    let mut at = 0;
    let keys = (new.iter())
        .map(|n| match (at..old.len()).find(|&j| same(&old[j], n)) {
            Some(j) => {
                at = j + 1;
                old_keys[j]
            }
            None => cx.rows.fresh(),
        })
        .collect();
    cx.rows.store(id, keys);
}

fn row_text(value: &Value, row: usize) -> Option<&str> {
    let e = value.as_array()?.get(row)?;
    e.as_str().or_else(|| e.get("text")?.as_str())
}

const PAD: f32 = 8.0;
const ICON_SIDE: f32 = 26.0;
const LABEL_FONT: f32 = 12.5;

fn label_galley(ui: &Ui, o: &ChoiceOption, width: f32, color: Color32) -> Arc<Galley> {
    ui.painter().layout(
        o.label.clone(),
        FontId::proportional(LABEL_FONT),
        color,
        width - PAD * 2.0,
    )
}

fn button_height(ui: &Ui, o: &ChoiceOption, width: f32) -> f32 {
    let text = label_galley(ui, o, width, Color32::PLACEHOLDER).size().y;
    match o.icon {
        Some(_) => ICON_SIDE + text + PAD * 3.0,
        None => (text + PAD * 2.0).max(36.0),
    }
}

fn option_button(ui: &mut Ui, c: &Choice, i: usize, on: bool, id: Id, size: Vec2, cx: &mut Cx) -> Response {
    let o = &c.options[i];
    let s: Style = cx.style;
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
    let (fill, stroke) = match () {
        () if on => (s.accent_soft, Stroke::new(1.5, s.accent)),
        () if r.hovered() => (s.hover, Stroke::new(1.0, s.line)),
        () => (s.field, Stroke::new(1.0, s.line)),
    };
    let stroke = if r.has_focus() {
        Stroke::new(2.0, s.accent)
    } else {
        stroke
    };
    let text = if on { s.fg } else { s.muted };
    let tone = if on { s.accent } else { s.muted };
    ui.painter()
        .rect(r.rect, f32::from(CARD_RADIUS), fill, stroke, StrokeKind::Inside);
    let state = o.icon.as_ref().map_or(Thumbnail::None, |n| cx.host.icon(n));
    let picture = match state {
        Thumbnail::Ready { texture, size } => Some((texture, size, Color32::WHITE)),
        Thumbnail::Mask { texture, size } => Some((texture, size, tone)),
        _ => None,
    };
    let top = r.rect.top() + PAD;
    if let Some((texture, size, tint)) = picture {
        let scale = (ICON_SIDE / size.x.max(1.0)).min(ICON_SIDE / size.y.max(1.0));
        let at = Rect::from_center_size(pos2(r.rect.center().x, top + ICON_SIDE / 2.0), size * scale);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().image(texture, at, uv, tint);
    }
    let galley = label_galley(ui, o, size.x, text);
    let y = match o.icon {
        Some(_) => r.rect.bottom() - PAD - galley.size().y,
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
                let (r, _) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::hover());
                let name = format!("{}: {text}", cx.texts.correct);
                let node = ui.interact(r, id.with(("mark", shown)), Sense::hover());
                node.widget_info(|| WidgetInfo::selected(Role::CheckBox, false, marked, &name));
                paint_mark(ui, r.center(), false, marked, false, cx.style);
            }
            ui.label(egui::RichText::new(text).color(cx.style.fg));
            let label = format!("{}: {text}", cx.texts.remove);
            let enabled = may_remove(&c.field.rule.limits, n);
            let r = icon_button(ui, id.with(("unknown", shown)), Icon::Cross, &label, enabled, cx.style);
            cx.track(&r);
            if r.clicked() {
                remove = Some(row);
            }
        });
    }
    if let Some(row) = remove {
        let mut keys = cx.rows.keys(id, rows.len());
        keys.remove(row);
        cx.rows.store(id, keys);
        rows.remove(row);
        cx.changed = true;
        if let Some(button) = fallback {
            ui.ctx().memory_mut(|m| m.request_focus(button));
        }
    }
}
