//! PROTOTYPE: a form for egui from JSON. The host passes a JSON Schema, a UI Schema and the
//! values (`serde_json`) and gets the edited values and the field errors back. The crate knows
//! nothing about its host's documents or formats; special widgets are registered by the host
//! under a `format` name ([`Widgets`]).
//!
//! ```ignore
//! let form = FormSchema::parse(&schema, &ui_schema, "en");   // once
//! let mut state = FormState::new();                          // once
//! state.begin(ui.ctx());                                     // every frame, first
//! let r = egui_schema_form::show(ui, &form, &mut values, &mut state, &mut widgets);
//! ```

pub mod model;

use std::collections::HashMap;
use std::hash::Hash;

use egui::{Color32, FontId, Id, Key, Margin, Modifiers, Rect, Response, RichText, Sense, Stroke, Ui, vec2};
use serde_json::{Value, json};

pub use model::{Field, FieldError, FieldKind, FormSchema, Texts};

/// What one frame of the form did.
#[derive(Debug, Default)]
pub struct FormResponse {
    /// `values` changed this frame.
    pub changed: bool,
    /// All current errors (schema rules and what widgets reported).
    pub errors: Vec<FieldError>,
    /// Fields nothing here can edit (shown with a note): (id or label, reason).
    pub unsupported: Vec<(String, String)>,
}

/// Colours; [`Style::from_dark`] gives the defaults.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub fg: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub hover: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub field: Color32,
    pub danger: Color32,
}

impl Style {
    pub fn from_dark(dark: bool) -> Self {
        let c = |h: u32| Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8);
        if dark {
            Style {
                fg: c(0xececee),
                muted: c(0xa8a8b0),
                line: c(0x3c3d44),
                hover: c(0x2b2840),
                accent: c(0xa08af0),
                accent_soft: c(0x2f2a4a),
                field: c(0x222327),
                danger: c(0xf0837a),
            }
        } else {
            Style {
                fg: c(0x1d1d1f),
                muted: c(0x5f5f66),
                line: c(0xd9d9df),
                hover: c(0xf0eefb),
                accent: c(0x5d3fba),
                accent_soft: c(0xece7fb),
                field: c(0xffffff),
                danger: c(0xc0392b),
            }
        }
    }
}

/// Widgets the host provides, by `options.format`.
#[derive(Default)]
pub struct Widgets<'a> {
    list: Vec<(String, WidgetFn<'a>)>,
}

type WidgetFn<'a> = Box<dyn FnMut(&mut Ui, &mut WidgetCtx<'_>) + 'a>;

impl<'a> Widgets<'a> {
    pub fn add(&mut self, format: &str, f: impl FnMut(&mut Ui, &mut WidgetCtx<'_>) + 'a) -> &mut Self {
        self.list.push((format.to_string(), Box::new(f)));
        self
    }
}

/// The form's widget memory across frames; a new one for every opened form.
#[derive(Debug)]
pub struct FormState {
    /// Stable identity of array rows, so a reordered row keeps its caret and undo.
    rows: HashMap<String, Vec<u64>>,
    next_uid: u64,
    /// Interactive widgets in order, `true` for Tab stops (last frame / being built).
    order: Vec<(Id, bool)>,
    building: Vec<(Id, bool)>,
    tail: Vec<(Id, bool)>,
    had_focus: bool,
    /// Give the first text field the keyboard.
    focus_first: bool,
    pending_focus: Option<Id>,
    /// Scripts: rectangles of named controls.
    pub rects: Option<HashMap<String, Rect>>,
}

impl Default for FormState {
    fn default() -> Self {
        Self::new()
    }
}

impl FormState {
    pub fn new() -> Self {
        FormState {
            rows: HashMap::new(),
            next_uid: 1,
            order: Vec::new(),
            building: Vec::new(),
            tail: Vec::new(),
            had_focus: false,
            focus_first: true,
            pending_focus: None,
            rects: None,
        }
    }

    /// The keyboard was in the form at the end of the last frame.
    pub fn had_focus(&self) -> bool {
        self.had_focus
    }

    /// Call first every frame: Tab moves within the form and wraps (it never leaves for widgets
    /// behind the form); `Alt`-arrows move rows, see the lists.
    pub fn begin(&mut self, ctx: &egui::Context) {
        let free = ctx.memory(|m| m.focused()).is_none();
        if !(self.had_focus || free) || self.order.is_empty() {
            return;
        }
        let (tab, back) = ctx.input(|i| (i.key_pressed(Key::Tab), i.modifiers.shift));
        if !tab {
            return;
        }
        let at = ctx
            .memory(|m| m.focused())
            .and_then(|f| self.order.iter().position(|(id, _)| *id == f));
        let n = self.order.len();
        let mut i = at.unwrap_or(if back { 0 } else { n - 1 });
        for _ in 0..n {
            i = if back { (i + n - 1) % n } else { (i + 1) % n };
            if self.order[i].1 {
                let id = self.order[i].0;
                ctx.memory_mut(|m| {
                    m.move_focus(egui::FocusDirection::None);
                    m.request_focus(id);
                });
                break;
            }
        }
    }

    /// A control the host draws around the form (a footer button); Tab reaches it after the
    /// fields. Call before [`show`] in the frame.
    pub fn add_stop(&mut self, r: &Response) {
        self.tail.push((r.id, true));
        if r.gained_focus() {
            r.scroll_to_me(None);
        }
    }

    pub fn name(&mut self, name: &str, r: Rect) {
        if let Some(m) = self.rects.as_mut() {
            m.insert(name.to_string(), r);
        }
    }
}

/// What a widget gets for its field; also the building blocks of the built-in fields.
pub struct WidgetCtx<'a> {
    pub field: &'a Field,
    pub value: &'a mut Value,
    pub texts: &'a Texts,
    pub style: Style,
    /// Set by whoever changes `value` directly.
    pub changed: bool,
    /// Errors of this field not shown yet: shown under the field; `take_errors` claims them.
    pub errors: Vec<FieldError>,
    shown: Vec<FieldError>,
    st: &'a mut FormState,
    base: Id,
}

/// Options of [`WidgetCtx::text_field`].
#[derive(Clone, Copy, Debug, Default)]
pub struct TextOpts<'t> {
    /// Line breaks allowed; otherwise the text still wraps, Enter does nothing and pasted line
    /// breaks become spaces.
    pub multi: bool,
    pub rows: usize,
    pub hint: Option<&'t str>,
    pub invalid: bool,
    /// 0: the available width.
    pub width: f32,
    /// Highlighted (a correct answer).
    pub marked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Icon {
    Up,
    Down,
    Close,
}

impl WidgetCtx<'_> {
    pub fn id_of(&self, salt: impl Hash + std::fmt::Debug) -> Id {
        self.base.with(salt)
    }

    /// Records a control's rectangle for scripts (`field/name`).
    pub fn rec(&mut self, r: Rect, name: impl FnOnce() -> String) {
        if let Some(m) = self.st.rects.as_mut() {
            m.insert(format!("{}/{}", self.field.id, name()), r);
        }
    }

    /// Claims the errors at `<field>/<sub>` (and below) to show them at a row.
    pub fn take_errors(&mut self, sub: &str) -> Vec<String> {
        let p = format!("{}/{sub}", self.field.id);
        let deep = format!("{p}/");
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.errors.len() {
            if self.errors[i].path == p || self.errors[i].path.starts_with(&deep) {
                let e = self.errors.remove(i);
                out.push(e.message.clone());
                self.shown.push(e);
            } else {
                i += 1;
            }
        }
        out
    }

    /// A validation error found by a widget.
    pub fn error(&mut self, message: impl Into<String>) {
        self.errors.push(FieldError {
            path: self.field.id.clone(),
            message: message.into(),
        });
    }

    pub fn show_errors(&self, ui: &mut Ui, list: &[String]) {
        for e in list {
            ui.label(RichText::new(e).size(12.0).color(self.style.danger));
        }
    }

    fn track(&mut self, r: &Response, stop: bool) {
        self.st.building.push((r.id, stop));
        if r.gained_focus() {
            r.scroll_to_me(None);
        }
    }

    /// Stable identities of the `n` rows of this array field.
    pub fn rows(&mut self, n: usize) -> Vec<u64> {
        let st = &mut *self.st;
        let v = st.rows.entry(self.field.id.clone()).or_default();
        while v.len() < n {
            v.push(st.next_uid);
            st.next_uid += 1;
        }
        v.truncate(n);
        v.clone()
    }

    /// The array value (made one if it is not).
    pub fn items(&mut self) -> &mut Vec<Value> {
        if !self.value.is_array() {
            *self.value = json!([]);
        }
        self.value.as_array_mut().expect("array")
    }

    pub fn swap_items(&mut self, a: usize, b: usize) {
        let n = self.items().len();
        if a < n && b < n {
            self.items().swap(a, b);
            if let Some(v) = self.st.rows.get_mut(&self.field.id) {
                v.swap(a, b);
            }
            self.changed = true;
        }
    }

    pub fn remove_item(&mut self, i: usize) {
        if i < self.items().len() {
            self.items().remove(i);
            if let Some(v) = self.st.rows.get_mut(&self.field.id) {
                v.remove(i);
            }
            self.changed = true;
        }
    }

    /// Adds a row; returns its identity.
    pub fn push_item(&mut self, v: Value) -> u64 {
        self.items().push(v);
        let n = self.items().len();
        self.changed = true;
        *self.rows(n).last().expect("row")
    }

    /// The next frame's field with this salt takes the keyboard.
    pub fn focus_later(&mut self, salt: impl Hash + std::fmt::Debug) {
        self.st.pending_focus = Some(self.base.with(salt));
    }

    /// A text field in the form's look. Sets `changed` when edited.
    pub fn text_field(
        &mut self,
        ui: &mut Ui,
        salt: impl Hash + std::fmt::Debug,
        text: &mut String,
        o: TextOpts<'_>,
    ) -> Response {
        let s = self.style;
        let id = self.base.with(salt);
        let bg = ui.painter().add(egui::Shape::Noop);
        let width = if o.width > 0.0 { o.width } else { ui.available_width() };
        let mut te = egui::TextEdit::multiline(text)
            .id(id)
            .frame(egui::Frame::NONE.inner_margin(Margin::symmetric(10, 7)))
            .font(FontId::proportional(14.0))
            .text_color(s.fg)
            .desired_rows(o.rows.max(1))
            .desired_width(width);
        if !o.multi {
            te = te.return_key(None);
        }
        if let Some(h) = o.hint {
            te = te.hint_text(RichText::new(h).color(s.muted));
        }
        let r = ui.add(te);
        if r.changed() {
            if !o.multi && text.contains('\n') {
                *text = text.replace(['\r', '\n'], " ");
            }
            self.changed = true;
        }
        if std::mem::take(&mut self.st.focus_first) || self.st.pending_focus == Some(id) {
            self.st.pending_focus = None;
            r.request_focus();
        }
        let focus = r.has_focus();
        let stroke = if o.invalid {
            Stroke::new(1.5, s.danger)
        } else if focus {
            Stroke::new(1.5, s.accent)
        } else {
            Stroke::new(1.0, s.line)
        };
        let fill = if o.marked { s.accent_soft } else { s.field };
        ui.painter().set(
            bg,
            egui::epaint::RectShape::new(r.rect, 8.0, fill, stroke, egui::StrokeKind::Inside),
        );
        self.track(&r, true);
        r
    }

    /// A small icon button; not a Tab stop (rows have keyboard shortcuts instead).
    pub fn icon_button(&mut self, ui: &mut Ui, icon: Icon, enabled: bool, tip: &str) -> Response {
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 32.0), sense);
        let s = self.style;
        if resp.hovered() && enabled {
            ui.painter().rect_filled(r, 6.0, s.hover);
        }
        focus_ring(ui, &resp, s);
        let col = if !enabled {
            s.line
        } else if resp.hovered() {
            s.fg
        } else {
            s.muted
        };
        paint_icon(ui, r, icon, col);
        if enabled {
            self.track(&resp, false);
            resp.on_hover_text(tip)
        } else {
            resp
        }
    }

    /// The `+ add` row button.
    pub fn add_button(&mut self, ui: &mut Ui, label: &str, enabled: bool) -> Response {
        let s = self.style;
        let text = format!("+ {label}");
        let g = ui.painter().layout_no_wrap(text, FontId::proportional(13.0), s.accent);
        let size = vec2(g.size().x + 20.0, 30.0);
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let (r, resp) = ui.allocate_exact_size(size, sense);
        if resp.hovered() && enabled {
            ui.painter().rect_filled(r, 6.0, s.hover);
        }
        focus_ring(ui, &resp, s);
        let pos = r.left_center() + vec2(10.0, -g.size().y / 2.0);
        ui.painter().galley(pos, g, if enabled { s.accent } else { s.line });
        if enabled {
            self.track(&resp, true);
        }
        resp
    }

    /// A button with text in the form's look; a Tab stop.
    pub fn text_button(&mut self, ui: &mut Ui, label: &str) -> Response {
        let r = button(ui, label, false, self.style);
        self.track(&r, true);
        r
    }

    /// Any other control of a widget.
    pub fn add_control(&mut self, r: &Response) {
        self.track(r, true);
    }
}

/// A focus ring for keyboard users.
pub fn focus_ring(ui: &Ui, r: &Response, s: Style) {
    if r.has_focus() {
        ui.painter()
            .rect_stroke(r.rect, 6.0, Stroke::new(1.5, s.accent), egui::StrokeKind::Inside);
    }
}

/// A text button (`primary`: filled with the accent).
pub fn button(ui: &mut Ui, label: &str, primary: bool, s: Style) -> Response {
    let col = if primary {
        if ui.visuals().dark_mode {
            Color32::BLACK
        } else {
            Color32::WHITE
        }
    } else {
        s.fg
    };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(13.5), col);
    let size = vec2(g.size().x + 24.0, 32.0);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = match (primary, resp.hovered()) {
        (true, _) => s.accent,
        (false, true) => s.hover,
        (false, false) => s.field,
    };
    let stroke = if resp.has_focus() {
        Stroke::new(2.0, if primary { s.fg } else { s.accent })
    } else if primary {
        Stroke::NONE
    } else {
        Stroke::new(1.0, s.line)
    };
    ui.painter().rect(r, 8.0, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, col);
    resp
}

fn paint_icon(ui: &Ui, r: Rect, icon: Icon, col: Color32) {
    let c = r.center();
    let st = Stroke::new(1.6, col);
    let p = ui.painter();
    match icon {
        Icon::Close => {
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

/// One frame of the form. Call [`FormState::begin`] first.
pub fn show(
    ui: &mut Ui,
    form: &FormSchema,
    values: &mut Value,
    st: &mut FormState,
    widgets: &mut Widgets<'_>,
) -> FormResponse {
    let style = Style::from_dark(ui.visuals().dark_mode);
    let mut errors = form.validate(values);
    let mut out = FormResponse::default();
    st.building.clear();
    let base = ui.scope_id().with("schema-form");
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    for (n, f) in form.fields.iter().enumerate() {
        if n > 0 {
            ui.add_space(8.0);
        }
        let mut unsupported = None;
        match &f.kind {
            FieldKind::Label => {
                // a trailing padlock is drawn (fonts render the emoji small and blurred)
                let (text, locked) = match f.label.strip_suffix('🔒') {
                    Some(t) => (t.trim_end(), true),
                    None => (f.label.as_str(), false),
                };
                let t = RichText::new(text).size(12.5).color(style.muted);
                ui.horizontal(|ui| {
                    ui.label(if f.options["section"] == true { t.strong() } else { t });
                    if locked {
                        let (r, _) = ui.allocate_exact_size(vec2(10.0, 12.0), Sense::hover());
                        let body = Rect::from_min_max(r.left_top() + vec2(0.0, 5.0), r.right_bottom());
                        ui.painter().rect_filled(body, 2.0, style.muted);
                        let c = egui::pos2(r.center().x, r.top() + 5.0);
                        ui.painter().circle_stroke(c, 3.0, Stroke::new(1.3, style.muted));
                    }
                });
                continue;
            }
            FieldKind::Unsupported(why) => unsupported = Some(why.clone()),
            FieldKind::Custom(name) if !widgets.list.iter().any(|(k, _)| k == name) => {
                unsupported = Some(name.clone());
            }
            _ => {}
        }
        if let Some(why) = unsupported {
            let shown = if f.label.is_empty() { &f.id } else { &f.label };
            ui.label(RichText::new(shown).size(12.5).strong().color(style.muted));
            ui.label(
                RichText::new(format!("{} ({why})", form.texts.unsupported))
                    .size(12.5)
                    .color(style.danger),
            );
            out.unsupported.push((shown.clone(), why));
            continue;
        }
        ui.label(RichText::new(&f.label).size(12.5).strong().color(style.muted));
        let prefix = format!("{}/", f.id);
        let mine: Vec<FieldError>;
        (mine, errors) = errors
            .into_iter()
            .partition(|e| e.path == f.id || e.path.starts_with(&prefix));
        let mut placeholder = Value::Null;
        let value = match values.get_mut(&f.id) {
            Some(v) => v,
            None => &mut placeholder,
        };
        let mut ctx = WidgetCtx {
            field: f,
            value,
            texts: &form.texts,
            style,
            changed: false,
            errors: mine,
            shown: Vec::new(),
            st,
            base: base.with(&f.id),
        };
        match &f.kind {
            FieldKind::Text { multi } => text(ui, &mut ctx, *multi),
            FieldKind::StringList => string_list(ui, &mut ctx),
            FieldKind::Choices => choices(ui, &mut ctx),
            FieldKind::Custom(name) => {
                if let Some((_, w)) = widgets.list.iter_mut().find(|(k, _)| k == name) {
                    w(ui, &mut ctx);
                }
            }
            _ => {}
        }
        let rest: Vec<String> = ctx.errors.iter().map(|e| e.message.clone()).collect();
        ctx.show_errors(ui, &rest);
        out.changed |= ctx.changed;
        out.errors.append(&mut ctx.shown);
        out.errors.append(&mut ctx.errors);
    }
    st.building.append(&mut st.tail);
    st.order = std::mem::take(&mut st.building);
    let focused = ui.memory(|m| m.focused());
    st.had_focus = focused.is_some_and(|f| st.order.iter().any(|(id, _)| *id == f));
    if out.changed {
        ui.ctx().request_repaint();
    }
    out
}

fn text(ui: &mut Ui, ctx: &mut WidgetCtx<'_>, multi: bool) {
    let mut s = ctx.value.as_str().unwrap_or("").to_string();
    let invalid = !ctx.errors.is_empty();
    let r = ctx.text_field(
        ui,
        "text",
        &mut s,
        TextOpts {
            multi,
            rows: if multi { 3 } else { 1 },
            invalid,
            ..Default::default()
        },
    );
    ctx.rec(r.rect, || "text".into());
    if r.changed() {
        *ctx.value = Value::String(s);
    }
}

/// `Alt`+arrows on the row whose field has the keyboard (checked before the field reads keys).
fn row_keys(
    ui: &Ui,
    ctx: &WidgetCtx<'_>,
    salt: impl Hash + std::fmt::Debug,
    i: usize,
    n: usize,
) -> Option<(usize, usize)> {
    if !ui.memory(|m| m.has_focus(ctx.id_of(salt))) {
        return None;
    }
    ui.input_mut(|inp| {
        if i > 0 && inp.consume_key(Modifiers::ALT, Key::ArrowUp) {
            Some((i, i - 1))
        } else if i + 1 < n && inp.consume_key(Modifiers::ALT, Key::ArrowDown) {
            Some((i, i + 1))
        } else {
            None
        }
    })
}

fn string_list(ui: &mut Ui, ctx: &mut WidgetCtx<'_>) {
    let min = ctx.field.schema["minItems"].as_u64().unwrap_or(0) as usize;
    let max = ctx.field.schema["maxItems"].as_u64().map_or(usize::MAX, |m| m as usize);
    let n = ctx.items().len();
    let uids = ctx.rows(n);
    let texts = ctx.texts.clone();
    let (mut swap, mut remove) = (None, None);
    for i in 0..n {
        let uid = uids[i];
        swap = swap.or(row_keys(ui, ctx, uid, i, n));
        let errs = ctx.take_errors(&i.to_string());
        ui.horizontal_top(|ui| {
            let mut s = ctx.items()[i].as_str().unwrap_or("").to_string();
            let width = ui.available_width() - 3.0 * 32.0;
            let r = ctx.text_field(
                ui,
                uid,
                &mut s,
                TextOpts {
                    width,
                    invalid: !errs.is_empty(),
                    ..Default::default()
                },
            );
            ctx.rec(r.rect, || format!("{i}/text"));
            if r.changed() {
                ctx.items()[i] = Value::String(s);
            }
            let b = ctx.icon_button(ui, Icon::Up, i > 0, texts.up);
            ctx.rec(b.rect, || format!("{i}/up"));
            if b.clicked() {
                swap = Some((i, i - 1));
            }
            let b = ctx.icon_button(ui, Icon::Down, i + 1 < n, texts.down);
            ctx.rec(b.rect, || format!("{i}/down"));
            if b.clicked() {
                swap = Some((i, i + 1));
            }
            let b = ctx.icon_button(ui, Icon::Close, n > min, texts.remove);
            ctx.rec(b.rect, || format!("{i}/remove"));
            if b.clicked() {
                remove = Some(i);
            }
        });
        ctx.show_errors(ui, &errs);
    }
    if let Some((a, b)) = swap {
        ctx.swap_items(a, b);
    }
    if let Some(i) = remove {
        ctx.remove_item(i);
    }
    let b = ctx.add_button(ui, texts.add, n < max);
    ctx.rec(b.rect, || "add".into());
    if b.clicked() {
        let uid = ctx.push_item(json!(""));
        ctx.focus_later(uid);
    }
}

fn choices(ui: &mut Ui, ctx: &mut WidgetCtx<'_>) {
    let single = ctx.field.options["single"] == true;
    let min = ctx.field.schema["minItems"].as_u64().unwrap_or(1) as usize;
    let n = ctx.items().len();
    let uids = ctx.rows(n);
    let texts = ctx.texts.clone();
    let s = ctx.style;
    let (mut swap, mut remove, mut set) = (None, None, None);
    for i in 0..n {
        let uid = uids[i];
        swap = swap.or(row_keys(ui, ctx, uid, i, n));
        let errs = ctx.take_errors(&i.to_string());
        let correct = ctx.items()[i]["correct"] == true;
        ui.horizontal_top(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 32.0), Sense::click());
            let c = r.center();
            if single {
                ui.painter()
                    .circle_stroke(c, 7.5, Stroke::new(1.5, if correct { s.accent } else { s.muted }));
                if correct {
                    ui.painter().circle_filled(c, 4.0, s.accent);
                }
            } else {
                let b = Rect::from_center_size(c, vec2(15.0, 15.0));
                if correct {
                    ui.painter().rect_filled(b, 4.0, s.accent);
                    let st = Stroke::new(1.8, s.field);
                    ui.painter()
                        .line_segment([c + vec2(-3.5, 0.0), c + vec2(-1.0, 3.0)], st);
                    ui.painter()
                        .line_segment([c + vec2(-1.0, 3.0), c + vec2(4.0, -3.0)], st);
                } else {
                    ui.painter()
                        .rect_stroke(b, 4.0, Stroke::new(1.5, s.muted), egui::StrokeKind::Inside);
                }
            }
            focus_ring(ui, &resp, s);
            ctx.track(&resp, true);
            ctx.rec(r, || format!("{i}/correct"));
            if resp.on_hover_text(texts.correct).clicked() {
                set = Some(i);
            }
            let id = ctx.items()[i]["id"].as_str().unwrap_or("").to_string();
            let mut t = ctx.items()[i]["text"].as_str().unwrap_or("").to_string();
            let id_w = ui
                .painter()
                .layout_no_wrap(id.clone(), FontId::proportional(11.0), s.muted)
                .size()
                .x;
            let width = ui.available_width() - 3.0 * 32.0 - id_w - 6.0;
            let r = ctx.text_field(
                ui,
                uid,
                &mut t,
                TextOpts {
                    width,
                    invalid: !errs.is_empty(),
                    marked: correct,
                    hint: Some(texts.answer),
                    ..Default::default()
                },
            );
            ctx.rec(r.rect, || format!("{i}/text"));
            if r.changed() {
                ctx.items()[i]["text"] = Value::String(t);
            }
            let (ir, _) = ui.allocate_exact_size(vec2(id_w, 32.0), Sense::hover());
            ui.painter().text(
                ir.left_center(),
                egui::Align2::LEFT_CENTER,
                &id,
                FontId::proportional(11.0),
                s.muted,
            );
            let b = ctx.icon_button(ui, Icon::Up, i > 0, texts.up);
            ctx.rec(b.rect, || format!("{i}/up"));
            if b.clicked() {
                swap = Some((i, i - 1));
            }
            let b = ctx.icon_button(ui, Icon::Down, i + 1 < n, texts.down);
            ctx.rec(b.rect, || format!("{i}/down"));
            if b.clicked() {
                swap = Some((i, i + 1));
            }
            let b = ctx.icon_button(ui, Icon::Close, n > min, texts.remove);
            ctx.rec(b.rect, || format!("{i}/remove"));
            if b.clicked() {
                remove = Some(i);
            }
        });
        ctx.show_errors(ui, &errs);
    }
    if let Some(i) = set {
        for (j, x) in ctx.items().iter_mut().enumerate() {
            if j == i {
                x["correct"] = json!(if single { true } else { x["correct"] != true });
            } else if single {
                x["correct"] = json!(false);
            }
        }
        ctx.changed = true;
    }
    if let Some((a, b)) = swap {
        ctx.swap_items(a, b);
    }
    if let Some(i) = remove {
        ctx.remove_item(i);
    }
    let b = ctx.add_button(ui, texts.add_answer, true);
    ctx.rec(b.rect, || "add".into());
    if b.clicked() {
        // the next free aN, also among labels the opened block had (`options.taken`)
        let used = |id: &str, ctx: &mut WidgetCtx<'_>| {
            let free = !ctx.items().iter().any(|x| x["id"] == id);
            let taken = ctx.field.options["taken"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == id));
            !free || taken
        };
        let mut k = 1;
        while used(&format!("a{k}"), ctx) {
            k += 1;
        }
        let uid = ctx.push_item(json!({"id": format!("a{k}"), "text": "", "correct": false}));
        ctx.focus_later(uid);
    }
}
