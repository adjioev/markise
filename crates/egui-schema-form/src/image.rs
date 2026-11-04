//! An image row: thumbnail, alt text, path and a picker. The file and the thumbnail come through
//! the host, so a slow disk never holds up a frame.

use egui::load::SizedTexture;
use egui::{Button, Id, Image, Response, Role, Sense, Spinner, Ui, Vec2, WidgetInfo};
use schema_form_core::FieldSpec;
use serde_json::{Map, Value};

use crate::draw::{Cx, RowAction, buttons_width, join, not_editable, row_buttons, text_editor};
use crate::host::{FileRequest, Thumbnail};
use crate::widgets::error_label;

/// The thumbnail's side, in lines of text, so it follows the zoom.
const THUMB_LINES: f32 = 4.0;

fn thumbnail(ui: &mut Ui, state: &Thumbnail, alt: &str, loading: &str, side: f32) {
    let frame = Vec2::splat(side);
    match state {
        Thumbnail::Ready { texture, size } => {
            let image = Image::new(SizedTexture::new(*texture, *size))
                .alt_text(alt)
                .fit_to_exact_size(frame);
            ui.add_sized(frame, image);
        }
        Thumbnail::Loading => {
            let r = ui.add_sized(frame, Spinner::new());
            r.widget_info(|| WidgetInfo::labeled(Role::ProgressIndicator, true, loading));
        }
        Thumbnail::Failed(_) | Thumbnail::None => {
            let (rect, _) = ui.allocate_exact_size(frame, Sense::hover());
            let v = ui.visuals();
            ui.painter()
                .rect_filled(rect, v.widgets.noninteractive.corner_radius, v.faint_bg_color);
        }
    }
}

fn labelled_text(
    ui: &mut Ui,
    o: &mut Map<String, Value>,
    key: &str,
    label: &str,
    row: Id,
    width: f32,
    cx: &mut Cx,
) -> Response {
    let mut s = o.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    ui.small(label);
    let r = text_editor(ui, &mut s, row.with(key), false, width, cx);
    if r.changed() {
        o.insert(key.to_string(), Value::String(s));
    }
    r
}

/// Puts a delivered file into its row.
fn take_delivery(o: &mut Map<String, Value>, row: Id, cx: &mut Cx) {
    if let Some(path) = cx.delivered.remove(&row) {
        o.insert("path".to_string(), Value::String(path));
        cx.changed = true;
    }
}

fn picker(ui: &mut Ui, label: &str, row: Id, row_name: &str, cx: &mut Cx) {
    if !cx.host.can_pick_files() {
        return;
    }
    let name = format!("{label}: {row_name}");
    let r = ui.add(Button::new(label));
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
        return row_buttons(ui, f, row, i, n, cx);
    };
    take_delivery(o, row, cx);
    let file = o.get("path").and_then(Value::as_str).unwrap_or("").to_string();
    let alt = o.get("alt").and_then(Value::as_str).unwrap_or("").to_string();
    let state = if file.is_empty() {
        Thumbnail::None
    } else {
        cx.host.thumbnail(&file)
    };
    let side = ui.text_style_height(&egui::TextStyle::Body) * THUMB_LINES;
    let t = cx.texts;
    let (alt_label, path_label, choose, loading) = (&t.alt, &t.path, &t.choose_file, &t.loading);
    ui.horizontal_top(|ui| {
        thumbnail(ui, &state, &alt, loading, side);
        let width = (ui.available_width() - buttons_width(ui) - ui.spacing().item_spacing.x).max(side);
        ui.vertical(|ui| {
            ui.set_width(width);
            let r = labelled_text(ui, o, "alt", alt_label, row, width, cx);
            ui.ctx()
                .accesskit_node_builder(r.id, |b| b.set_label(format!("{alt_label}: {row_name}")));
            cx.describe(ui, &r, &join(path, "alt"));
            let r = labelled_text(ui, o, "path", path_label, row, width, cx);
            ui.ctx()
                .accesskit_node_builder(r.id, |b| b.set_label(format!("{path_label}: {row_name}")));
            cx.describe(ui, &r, &join(path, "path"));
            picker(ui, choose, row, &row_name, cx);
            if let Thumbnail::Failed(why) = &state {
                error_label(ui, why);
            }
        });
        row_buttons(ui, f, row, i, n, cx)
    })
    .inner
}
