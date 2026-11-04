//! An image row: thumbnail, alt text, path and a picker. The file and the thumbnail come through
//! the host, so a slow disk never holds up a frame.

use egui::load::SizedTexture;
use egui::{
    Align2, Color32, FontId, Id, Image, Rect, Response, RichText, Role, Sense, Spinner, TextureId, Ui, Vec2, WidgetInfo,
};
use schema_form_core::FieldSpec;
use serde_json::{Map, Value};

use crate::draw::{Cx, RowAction, buttons_width, join, not_editable, row_buttons, text_editor};
use crate::host::{FileRequest, Thumbnail};
use crate::style::{FieldLook, NOTE_SIZE, button, sub_card};
use crate::widgets::error_label;

/// The tallest a thumbnail gets, in lines of text, so it follows the zoom.
const THUMB_MAX_HEIGHT: f32 = 150.0;
const PLACEHOLDER_HEIGHT: f32 = 54.0;

fn picture(ui: &mut Ui, texture: TextureId, size: Vec2, alt: &str, tint: Option<Color32>) {
    let w = ui.available_width();
    let scale = (w / size.x.max(1.0)).min(THUMB_MAX_HEIGHT / size.y.max(1.0)).min(1.0);
    ui.vertical_centered(|ui| {
        let mut image = Image::new(SizedTexture::new(texture, size))
            .alt_text(alt)
            .corner_radius(6.0)
            .fit_to_exact_size(size * scale);
        if let Some(t) = tint {
            image = image.tint(t);
        }
        ui.add(image);
    });
}

fn thumbnail(ui: &mut Ui, state: &Thumbnail, alt: &str, cx: &Cx) {
    let s = cx.style;
    match state {
        Thumbnail::Ready { texture, size } => picture(ui, *texture, *size, alt, None),
        Thumbnail::Mask { texture, size } => picture(ui, *texture, *size, alt, Some(s.fg)),
        Thumbnail::Loading | Thumbnail::Failed(_) | Thumbnail::None => {
            let frame = Vec2::new(ui.available_width(), PLACEHOLDER_HEIGHT);
            let (rect, _) = ui.allocate_exact_size(frame, Sense::hover());
            ui.painter().rect_filled(rect, 6.0, s.hover);
            if matches!(state, Thumbnail::Loading) {
                let at = Rect::from_center_size(rect.center(), Vec2::splat(20.0));
                let r = ui.put(at, Spinner::new());
                r.widget_info(|| WidgetInfo::labeled(Role::ProgressIndicator, true, &cx.texts.loading));
            } else {
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    &cx.texts.no_preview,
                    FontId::proportional(12.0),
                    s.muted,
                );
            }
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "the drawing state of one field")]
fn labelled_text(
    ui: &mut Ui,
    o: &mut Map<String, Value>,
    key: &str,
    label: &str,
    row: Id,
    width: f32,
    look: FieldLook,
    cx: &mut Cx,
) -> Option<Response> {
    ui.label(RichText::new(label).size(NOTE_SIZE).color(cx.style.muted));
    // a value of another type is not an empty text to type over
    if o.get(key).is_some_and(|v| !v.is_string()) {
        not_editable(ui, cx);
        return None;
    }
    let mut s = o.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let r = text_editor(ui, &mut s, row.with(key), false, width, look, cx);
    if r.changed() {
        o.insert(key.to_string(), Value::String(s));
    }
    Some(r)
}

/// Puts a delivered file into its row.
fn take_delivery(o: &mut Map<String, Value>, row: Id, cx: &mut Cx) {
    if let Some(path) = cx.delivered.remove(&row)
        && o.get("path").is_none_or(Value::is_string)
    {
        o.insert("path".to_string(), Value::String(path));
        cx.changed = true;
    }
}

fn picker(ui: &mut Ui, label: &str, row: Id, row_name: &str, cx: &mut Cx) {
    if !cx.host.can_pick_files() {
        return;
    }
    let name = format!("{label}: {row_name}");
    let r = button(ui, label, false, true, cx.style);
    r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
    cx.track(&r);
    if r.clicked() {
        *cx.next_request += 1;
        let token = *cx.next_request;
        cx.file_requests.insert(token, row);
        cx.host.request_file(FileRequest(token), row_name);
    }
}

#[expect(clippy::too_many_arguments, reason = "the drawing state of one row")]
pub(crate) fn image_row(
    ui: &mut Ui,
    f: &FieldSpec,
    v: &mut Value,
    row: Id,
    path: &str,
    i: usize,
    n: usize,
    cx: &mut Cx,
) -> Option<RowAction> {
    let row_name = format!("{} {}", f.label, i + 1);
    let Some(o) = v.as_object_mut() else {
        not_editable(ui, cx);
        return ui.horizontal(|ui| row_buttons(ui, f, row, i, n, cx)).inner;
    };
    take_delivery(o, row, cx);
    let file = o.get("path").and_then(Value::as_str).unwrap_or("").to_string();
    let alt = o.get("alt").and_then(Value::as_str).unwrap_or("").to_string();
    let state = if file.is_empty() {
        Thumbnail::None
    } else {
        cx.host.thumbnail(&file)
    };
    let t = cx.texts;
    let st = cx.style;
    let (alt_label, path_label, choose) = (&t.alt, &t.path, &t.choose_file);
    sub_card(ui, st, |ui| {
        let action = ui
            .horizontal(|ui| {
                ui.label(RichText::new(&row_name).size(NOTE_SIZE).color(st.muted));
                ui.add_space((ui.available_width() - buttons_width(ui)).max(0.0));
                row_buttons(ui, f, row, i, n, cx)
            })
            .inner;
        thumbnail(ui, &state, &alt, cx);
        let width = ui.available_width();
        for (key, label) in [("alt", alt_label), ("path", path_label)] {
            let at = join(path, key);
            let look = FieldLook {
                invalid: !cx.messages(&at).is_empty(),
                marked: false,
            };
            if let Some(r) = labelled_text(ui, o, key, label, row, width, look, cx) {
                ui.ctx()
                    .accesskit_node_builder(r.id, |b| b.set_label(format!("{label}: {row_name}")));
                cx.describe(ui, &r, &at);
            }
            cx.draw_errors(ui, &at);
        }
        if o.get("path").is_none_or(Value::is_string) {
            picker(ui, choose, row, &row_name, cx);
        }
        if let Thumbnail::Failed(why) = &state {
            error_label(ui, why, st);
        }
        action
    })
}
