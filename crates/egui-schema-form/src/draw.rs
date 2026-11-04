//! Drawing: a field at a time, over the JSON values. A value of an unexpected type is shown as
//! not editable and kept as it is; the form never coerces what it did not write.

use std::collections::HashSet;

use egui::{Id, Key, KeyboardShortcut, Modifiers, Response, Role, ScrollArea, TextEdit, Ui, UiBuilder, WidgetInfo};
use schema_form_core::{Control, FieldError, FieldSpec, Limits, Selection};
use serde_json::{Map, Value, json};

use crate::SchemaForm;
use crate::host::{FormHost, HostWidget, WidgetContext};
use crate::rows::Rows;
use crate::widgets::{Icon, error_label, field_label, icon_button, status_label};

const MOVE_UP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowUp);
const MOVE_DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowDown);

/// How many lines a multiline field shows at most before it scrolls.
const MAX_ROWS: f32 = 12.0;

/// A row that a shortcut asked to move: its id and the direction.
type MoveRequest = Option<(Id, isize)>;

pub(crate) struct Cx<'a> {
    pub rows: &'a mut Rows,
    pub host: &'a mut dyn FormHost,
    pub widgets: &'a mut std::collections::HashMap<String, Box<dyn HostWidget>>,
    pub next_request: &'a mut u64,
    pub file_requests: &'a mut std::collections::HashMap<u64, Id>,
    pub delivered: &'a mut std::collections::HashMap<Id, String>,
    errors: &'a [FieldError],
    pub texts: &'a crate::Texts,
    focus_rows: std::collections::HashMap<Id, Id>,
    /// Every text editor of this frame: what a composition can belong to.
    editors: HashSet<Id>,
    /// A new row whose first real control is to take the focus.
    focus_row: &'a mut Option<Id>,
    focus_row_drawn: bool,
    /// Where that focus goes if the row has no real control: its first enabled button.
    focus_fallback: Option<Id>,
    status: &'a mut std::collections::HashMap<Id, String>,
    buttons: &'a mut std::collections::HashMap<Id, [Id; 3]>,
    moving: MoveRequest,
    /// The rows around the widget being drawn, innermost last.
    row_stack: Vec<Id>,
    pub changed: bool,
}

impl Cx<'_> {
    /// Like [`Cx::track`], for a control that is something to edit: a new row around it gives it
    /// the focus.
    fn track_control(&mut self, r: &Response) {
        self.track(r);
        if r.enabled() && self.focus_row.is_some_and(|row| self.row_stack.contains(&row)) {
            r.request_focus();
            *self.focus_row = None;
        }
    }

    /// Notes that a focusable widget belongs to the row being drawn.
    pub fn track(&mut self, r: &Response) {
        if let Some(row) = self.row_stack.last() {
            self.focus_rows.insert(r.id, *row);
        }
    }

    fn messages(&self, path: &str) -> Vec<String> {
        (self.errors.iter())
            .filter(|e| e.path == path)
            .map(|e| (self.texts.message)(&e.kind))
            .collect()
    }

    fn draw_errors(&self, ui: &mut Ui, path: &str) {
        for m in self.messages(path) {
            error_label(ui, &m);
        }
    }

    /// Tells a screen reader the widget's reasons.
    pub fn describe(&self, ui: &Ui, r: &Response, path: &str) {
        let m = self.messages(path);
        if !m.is_empty() {
            ui.ctx().accesskit_node_builder(r.id, |n| {
                n.set_description(m.join(". "));
                n.set_invalid(egui::accesskit::Invalid::True);
            });
        }
    }
}

pub(crate) fn join(path: &str, key: impl std::fmt::Display) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}/{key}")
    }
}

impl SchemaForm {
    pub(crate) fn draw(&mut self, ui: &mut Ui, host: &mut dyn FormHost) -> bool {
        // a shortcut has to be taken before the text field under it sees the arrow key
        let focused = ui.memory(|m| m.focused());
        let row = focused.and_then(|f| self.focus_rows.get(&f).copied());
        let moving = row.and_then(|row| {
            ui.input_mut(|i| {
                if i.consume_shortcut(&MOVE_UP) {
                    Some((row, -1))
                } else if i.consume_shortcut(&MOVE_DOWN) {
                    Some((row, 1))
                } else {
                    None
                }
            })
        });
        let mut cx = Cx {
            rows: &mut self.rows,
            host,
            widgets: &mut self.widgets,
            next_request: &mut self.next_request,
            file_requests: &mut self.file_requests,
            delivered: &mut self.delivered,
            errors: &self.errors,
            texts: &self.texts,
            focus_rows: std::collections::HashMap::new(),
            editors: HashSet::new(),
            focus_row: &mut self.focus_row,
            focus_row_drawn: false,
            focus_fallback: None,
            status: &mut self.status,
            buttons: &mut self.buttons,
            moving,
            row_stack: Vec::new(),
            changed: false,
        };
        if !self.values.is_object() {
            cx.draw_errors(ui, "");
            ui.label(&cx.texts.cannot_edit);
            return false;
        }
        let root = self.values.as_object_mut().expect("checked");
        ui.vertical(|ui| draw_fields(ui, &self.spec.fields, root, self.id, "", &mut cx));
        let changed = cx.changed;
        if cx.focus_row_drawn && cx.focus_row.is_some() {
            *cx.focus_row = None;
            if let Some(button) = cx.focus_fallback {
                ui.memory_mut(|m| m.request_focus(button));
            }
        }
        self.focus_rows = cx.focus_rows;
        self.editors = cx.editors;
        // every live row has taken its file by now: what is left was for a removed row
        self.delivered.clear();
        changed
    }
}

fn draw_fields(ui: &mut Ui, fields: &[FieldSpec], obj: &mut Map<String, Value>, scope: Id, path: &str, cx: &mut Cx) {
    for f in fields {
        let id = scope.with(&f.key);
        let p = join(path, &f.key);
        draw_field(ui, f, obj, id, &p, cx);
        ui.add_space(ui.spacing().item_spacing.y);
    }
}

fn draw_field(ui: &mut Ui, f: &FieldSpec, obj: &mut Map<String, Value>, id: Id, path: &str, cx: &mut Cx) {
    let missing = !obj.contains_key(&f.key);
    if let Control::Checkbox = f.control {
        if obj.get(&f.key).is_some_and(|v| !v.is_boolean()) {
            ui.label(&f.label);
            not_editable(ui, cx);
            cx.draw_errors(ui, path);
            return;
        }
        let mut on = obj.get(&f.key).and_then(Value::as_bool).unwrap_or(false);
        let r = ui.checkbox(&mut on, &f.label);
        cx.track_control(&r);
        if r.changed() {
            obj.insert(f.key.clone(), Value::Bool(on));
            cx.changed = true;
        }
        cx.draw_errors(ui, path);
        return;
    }
    let label = field_label(ui, &f.label, f.required);
    if let Some(h) = &f.help {
        ui.small(h);
    }
    // a missing value is empty for the widget; it enters the values only when edited
    let mut value = obj.get(&f.key).cloned().unwrap_or_else(|| empty_value(&f.control));
    let before = value.clone();
    match &f.control {
        Control::Text { multiline } => match &mut value {
            Value::String(s) => {
                let r = text_editor(ui, s, id, *multiline, ui.available_width(), cx);
                let r = r.labelled_by(label);
                cx.describe(ui, &r, path);
            }
            _ => not_editable(ui, cx),
        },
        Control::Checkbox => {}
        Control::StringList { .. } | Control::Array { .. } | Control::Choices { .. } | Control::Images => {
            match value.as_array_mut() {
                Some(a) => draw_array(ui, f, a, id, path, label, cx),
                None => not_editable(ui, cx),
            }
        }
        Control::Group { fields } => match value.as_object_mut() {
            Some(o) => {
                ui.group(|ui| draw_fields(ui, fields, o, id, path, cx));
            }
            None => not_editable(ui, cx),
        },
        Control::Custom { name } => {
            let errors = cx.messages(path);
            let context = WidgetContext {
                value: &mut value,
                label: &f.label,
                id,
                errors: &errors,
            };
            match cx.widgets.get_mut(name) {
                Some(w) => cx.changed |= w.ui(ui, context),
                None => not_available(ui, cx),
            }
        }
    }
    cx.draw_errors(ui, path);
    if value != before && !(missing && value == empty_value(&f.control)) {
        obj.insert(f.key.clone(), value);
    }
}

fn empty_value(c: &Control) -> Value {
    match c {
        Control::Text { .. } => json!(""),
        Control::Checkbox => json!(false),
        Control::Group { .. } => json!({}),
        Control::Custom { .. } => Value::Null,
        _ => json!([]),
    }
}

pub(crate) fn not_editable(ui: &mut Ui, cx: &Cx) {
    ui.colored_label(ui.visuals().warn_fg_color, &cx.texts.cannot_edit);
}

fn not_available(ui: &mut Ui, cx: &Cx) {
    ui.colored_label(ui.visuals().warn_fg_color, &cx.texts.no_widget);
}

/// The text editor of a field or a row. Its id is the widget's identity, so its caret,
/// selection and composition follow it.
pub(crate) fn text_editor(ui: &mut Ui, s: &mut String, id: Id, multiline: bool, width: f32, cx: &mut Cx) -> Response {
    let editor_id = id.with("text");
    let r = if multiline {
        let line = ui.text_style_height(&egui::TextStyle::Body);
        ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .max_height(line * MAX_ROWS)
            .auto_shrink([false, true])
            .max_width(width)
            .show(ui, |ui| {
                ui.add(
                    TextEdit::multiline(s)
                        .id(editor_id)
                        .desired_width(width)
                        .desired_rows(3),
                )
            })
            .inner
    } else {
        ui.add(TextEdit::singleline(s).id(editor_id).desired_width(width))
    };
    if r.changed() {
        cx.changed = true;
        cx.status.clear();
    }
    cx.editors.insert(r.id);
    cx.track_control(&r);
    r
}

fn new_row(c: &Control) -> Value {
    match c {
        Control::Choices { .. } => json!({"id": "", "text": "", "correct": false}),
        Control::Images => json!({"alt": "", "path": ""}),
        Control::Array { fields } => {
            let m: Map<String, Value> = fields
                .iter()
                .map(|x| (x.key.clone(), empty_value(&x.control)))
                .collect();
            Value::Object(m)
        }
        _ => json!(""),
    }
}

pub(crate) enum RowAction {
    Move(usize, isize),
    Remove(usize),
    Add,
}

fn may_add(l: &Limits, len: usize) -> bool {
    l.max_items.is_none_or(|m| (len as u64) < m)
}

fn may_remove(l: &Limits, len: usize) -> bool {
    l.min_items.is_none_or(|m| (len as u64) > m)
}

fn draw_array(ui: &mut Ui, f: &FieldSpec, arr: &mut Vec<Value>, id: Id, path: &str, label: Id, cx: &mut Cx) {
    let mut keys = cx.rows.keys(id, arr.len());
    let mut action = None;
    // radio buttons: one mark clears the others, after the loop
    let mut exclusive = None;
    let n = arr.len();
    for i in 0..n {
        let row = id.with(keys[i]);
        let p = join(path, i);
        if let Some((m, d)) = cx.moving
            && m == row
        {
            action = Some(RowAction::Move(i, d));
            cx.moving = None;
        }
        cx.focus_row_drawn |= *cx.focus_row == Some(row);
        cx.row_stack.push(row);
        // an explicit scope: egui's automatic ids of the buttons inside then do not depend on
        // the row's position, and the focus can stay on them through a reorder
        ui.scope_builder(UiBuilder::new().scope_id(row), |ui| {
            if let Some(a) = draw_row(ui, f, &mut arr[i], row, &p, i, n, &mut exclusive, cx) {
                action = Some(a);
            }
        });
        cx.row_stack.pop();
        cx.draw_errors(ui, &p);
        if matches!(f.control, Control::Choices { .. }) {
            for key in ["id", "text", "correct"] {
                cx.draw_errors(ui, &join(&p, key));
            }
        }
    }
    if let Some(mark) = exclusive {
        for (i, e) in arr.iter_mut().enumerate() {
            // a mark it cannot read is not its to change
            if let Some(o) = e
                .as_object_mut()
                .filter(|o| o.get("correct").is_none_or(Value::is_boolean))
            {
                o.insert("correct".into(), Value::Bool(i == mark));
            }
        }
        cx.changed = true;
    }
    let add_label = f.add_label.as_deref().unwrap_or(&cx.texts.add);
    // an explicit scope: its id stays the same as rows come and go, so it can take the focus
    let add = ui
        .scope_builder(UiBuilder::new().scope_id(id.with("add")), |ui| {
            ui.add_enabled(may_add(&f.rule.limits, n), egui::Button::new(add_label))
        })
        .inner;
    let add = add.labelled_by(label);
    add.widget_info(|| WidgetInfo::labeled(Role::Button, may_add(&f.rule.limits, n), add_label));
    cx.track_control(&add);
    if add.clicked() {
        action = Some(RowAction::Add);
    }
    match action {
        Some(RowAction::Move(i, d)) => {
            let j = i as isize + d;
            if j >= 0 && (j as usize) < n {
                pass_focus_at_the_edge(ui, cx, id.with(keys[i]), j as usize, n);
                arr.swap(i, j as usize);
                keys.swap(i, j as usize);
                cx.changed = true;
                let t = cx
                    .texts
                    .moved
                    .replacen("{}", &(j + 1).to_string(), 1)
                    .replacen("{}", &n.to_string(), 1);
                cx.status.insert(id, t);
            }
        }
        Some(RowAction::Remove(i)) => {
            arr.remove(i);
            keys.remove(i);
            cx.changed = true;
            cx.status.remove(&id);
            // the pressed button goes away with its row: the focus goes to the next one left,
            // or to Add when the rows have reached their limit and can no longer be removed
            let next = keys.get(i).or(keys.last()).and_then(|k| cx.buttons.get(&id.with(*k)));
            let target = next
                .filter(|_| may_remove(&f.rule.limits, n - 1))
                .map_or(add.id, |b| b[2]);
            ui.ctx().memory_mut(|m| m.request_focus(target));
        }
        Some(RowAction::Add) => {
            arr.push(new_row(&f.control));
            let key = cx.rows.fresh();
            keys.push(key);
            cx.changed = true;
            cx.status.remove(&id);
            *cx.focus_row = Some(id.with(key));
        }
        None => {}
    }
    cx.rows.store(id, keys);
    if let Some(s) = cx.status.get(&id).cloned() {
        status_label(ui, &s);
    }
}

/// A row moved to the first or last place has its up or down button disabled. If that button
/// had the focus, the focus goes to the other one, or a keyboard user would lose their place.
fn pass_focus_at_the_edge(ui: &Ui, cx: &Cx, row: Id, to: usize, n: usize) {
    let Some([up, down, _]) = cx.buttons.get(&row).copied() else {
        return;
    };
    let target = match () {
        () if to == 0 && ui.memory(|m| m.has_focus(up)) => down,
        () if to + 1 == n && ui.memory(|m| m.has_focus(down)) => up,
        () => return,
    };
    ui.ctx().memory_mut(|m| m.request_focus(target));
}

/// Up, down and remove of a row. Their ids move with the row, so focus stays on the button
/// that was pressed while the row travels.
pub(crate) fn row_buttons(ui: &mut Ui, f: &FieldSpec, row: Id, i: usize, n: usize, cx: &mut Cx) -> Option<RowAction> {
    let mut action = None;
    let t = cx.texts;
    let name = format!("{} {}", f.label, i + 1);
    let up = format!("{}: {name}", t.move_up);
    let down = format!("{}: {name}", t.move_down);
    let remove = format!("{}: {name}", t.remove);
    let r = icon_button(ui, row.with("up"), Icon::Up, &up, i > 0);
    let up_id = r.id;
    cx.track(&r);
    if r.clicked() {
        action = Some(RowAction::Move(i, -1));
    }
    let r = icon_button(ui, row.with("down"), Icon::Down, &down, i + 1 < n);
    let down_id = r.id;
    cx.track(&r);
    if r.clicked() {
        action = Some(RowAction::Move(i, 1));
    }
    let r = icon_button(
        ui,
        row.with("remove"),
        Icon::Cross,
        &remove,
        may_remove(&f.rule.limits, n),
    );
    cx.track(&r);
    if r.clicked() {
        action = Some(RowAction::Remove(i));
    }
    if *cx.focus_row == Some(row) && cx.focus_fallback.is_none() {
        let enabled = [
            (i > 0, up_id),
            (i + 1 < n, down_id),
            (may_remove(&f.rule.limits, n), r.id),
        ];
        cx.focus_fallback = enabled.iter().find(|(on, _)| *on).map(|(_, b)| *b);
    }
    cx.buttons.insert(row, [up_id, down_id, r.id]);
    action
}

pub(crate) fn buttons_width(ui: &Ui) -> f32 {
    3.0 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.x)
}

#[expect(clippy::too_many_arguments, reason = "the drawing state of one row")]
fn draw_row(
    ui: &mut Ui,
    f: &FieldSpec,
    v: &mut Value,
    row: Id,
    path: &str,
    i: usize,
    n: usize,
    exclusive: &mut Option<usize>,
    cx: &mut Cx,
) -> Option<RowAction> {
    let row_name = format!("{} {}", f.label, i + 1);
    match &f.control {
        Control::StringList { multiline } => {
            ui.horizontal_top(|ui| {
                let w = ui.available_width() - buttons_width(ui);
                match v.as_str().map(str::to_string) {
                    Some(mut s) => {
                        let r = text_editor(ui, &mut s, row, *multiline, w, cx);
                        ui.ctx().accesskit_node_builder(r.id, |b| b.set_label(row_name.clone()));
                        cx.describe(ui, &r, path);
                        if r.changed() {
                            *v = Value::String(s);
                        }
                    }
                    None => not_editable(ui, cx),
                }
                row_buttons(ui, f, row, i, n, cx)
            })
            .inner
        }
        Control::Choices { selection, .. } => {
            ui.horizontal_top(|ui| {
                let Some(o) = v.as_object_mut() else {
                    not_editable(ui, cx);
                    return row_buttons(ui, f, row, i, n, cx);
                };
                draw_mark(ui, o, *selection, row, i, &row_name, exclusive, cx);
                if o.get("text").is_some_and(|t| !t.is_string()) {
                    not_editable(ui, cx);
                    return row_buttons(ui, f, row, i, n, cx);
                }
                let w = ui.available_width() - buttons_width(ui);
                let mut s = o.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                let r = text_editor(ui, &mut s, row, false, w, cx);
                ui.ctx().accesskit_node_builder(r.id, |b| b.set_label(row_name.clone()));
                cx.describe(ui, &r, &join(path, "text"));
                if r.changed() {
                    o.insert("text".into(), Value::String(s));
                }
                row_buttons(ui, f, row, i, n, cx)
            })
            .inner
        }
        Control::Images => crate::image::image_row(ui, f, v, row, path, i, n, cx),
        Control::Array { fields } => draw_group_row(ui, f, fields, v, row, path, i, n, cx),
        _ => None,
    }
}

#[expect(clippy::too_many_arguments, reason = "the drawing state of one row")]
fn draw_group_row(
    ui: &mut Ui,
    f: &FieldSpec,
    fields: &[FieldSpec],
    v: &mut Value,
    row: Id,
    path: &str,
    i: usize,
    n: usize,
    cx: &mut Cx,
) -> Option<RowAction> {
    ui.group(|ui| {
        let action = ui
            .horizontal(|ui| {
                ui.add_space((ui.available_width() - buttons_width(ui)).max(0.0));
                row_buttons(ui, f, row, i, n, cx)
            })
            .inner;
        match v.as_object_mut() {
            Some(o) => draw_fields(ui, fields, o, row, path, cx),
            None => not_editable(ui, cx),
        }
        action
    })
    .inner
}

#[expect(clippy::too_many_arguments, reason = "the drawing state of one row")]
fn draw_mark(
    ui: &mut Ui,
    o: &mut Map<String, Value>,
    selection: Selection,
    row: Id,
    i: usize,
    row_name: &str,
    exclusive: &mut Option<usize>,
    cx: &mut Cx,
) {
    if o.get("correct").is_some_and(|c| !c.is_boolean()) {
        not_editable(ui, cx);
        return;
    }
    let on = o.get("correct").and_then(Value::as_bool).unwrap_or(false);
    let label = format!("{}: {row_name}", cx.texts.correct);
    ui.push_id(row.with("mark"), |ui| match selection {
        Selection::Single => {
            let r = ui.radio(on, "");
            r.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, &label));
            cx.track(&r);
            if r.clicked() && !on {
                *exclusive = Some(i);
            }
        }
        Selection::Multiple => {
            let mut now = on;
            let r = ui.checkbox(&mut now, "");
            r.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, now, &label));
            cx.track(&r);
            if r.changed() {
                o.insert("correct".into(), Value::Bool(now));
                cx.changed = true;
            }
        }
    });
}
