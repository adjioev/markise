//! A native form from JSON (ADR 0020): a [`FormSpec`] and values go in, changed values and
//! errors come out. The form knows nothing of what the values mean or where they come from, and
//! draws only generic primitives.
//!
//! ```no_run
//! # use egui_schema_form::SchemaForm;
//! # fn demo(ui: &mut egui::Ui, form: &mut SchemaForm) {
//! if form.ui(ui).changed {
//!     let _ = (form.values(), form.errors());
//! }
//! # }
//! ```
//!
//! Identity: every array row has a key that moves with it, and all widgets of a row derive their
//! ids from the key. Caret, selection, IME composition, focus and scrolling live in egui under
//! those ids, so they follow a row through a reorder and survive redraws. Colours and sizes come
//! from the [`egui::Ui`], so the form follows the host's theme and zoom.
//!
//! The form opens no dialogs and loads no pictures: it asks a [`FormHost`], and the answers come
//! later. The only way to add a widget is [`SchemaForm::register_widget`].

mod choice;
mod draw;
mod host;
mod image;
mod rows;
mod style;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Event, Id, ImeEvent, Ui};
use schema_form_core::{ErrorKind, FieldError, FormSpec};
use serde_json::Value;

pub use host::{FileRequest, FormHost, HostWidget, NoHost, Thumbnail, WidgetContext};
pub use schema_form_core;
pub use style::{Style, button};

use rows::Rows;

/// The form's own words. Everything a person reads that does not come from the hints.
#[derive(Clone)]
pub struct Texts {
    pub add: String,
    pub remove: String,
    pub move_up: String,
    pub move_down: String,
    pub correct: String,
    pub alt: String,
    pub path: String,
    pub choose_file: String,
    pub no_preview: String,
    pub loading: String,
    pub cannot_edit: String,
    pub no_widget: String,
    /// `{}` twice: the new position and the count.
    pub moved: String,
    /// The message of a reason.
    pub message: Arc<dyn Fn(&ErrorKind) -> String + Send + Sync>,
}

impl Default for Texts {
    fn default() -> Self {
        Texts {
            add: "Add".into(),
            remove: "Remove".into(),
            move_up: "Move up".into(),
            move_down: "Move down".into(),
            correct: "Marked".into(),
            alt: "Alt text".into(),
            path: "Path".into(),
            choose_file: "Choose file".into(),
            no_preview: "No preview".into(),
            loading: "Loading".into(),
            cannot_edit: "This value cannot be edited here".into(),
            no_widget: "This field needs a widget the application does not have".into(),
            moved: "Moved to position {} of {}".into(),
            message: Arc::new(ErrorKind::to_string),
        }
    }
}

/// The result of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormOutput {
    /// The values changed. Never during an IME composition: its text is not final, the change
    /// is reported when the composition ends.
    pub changed: bool,
}

/// A form with its values. Keep it from frame to frame.
pub struct SchemaForm {
    spec: FormSpec,
    values: Value,
    id: Id,
    pub(crate) rows: Rows,
    errors: Vec<FieldError>,
    texts: Texts,
    /// The host's colours; without them, the ones for the `Ui`'s visuals.
    style: Option<Style>,
    composing: Option<Id>,
    /// Changed during the composition, not yet reported.
    held_change: bool,
    /// Values from [`SchemaForm::set_values`] waiting for the composition to end.
    pending: Option<Value>,
    /// Which row each focusable widget of the last frame belongs to.
    focus_rows: HashMap<Id, Id>,
    /// What screen readers last heard about a move, per array.
    status: HashMap<Id, String>,
    /// The text editors of the last frame: a composition is this form's only inside one.
    editors: HashSet<Id>,
    /// A new row whose first real control is to take the focus.
    focus_row: Option<Id>,
    /// The first real control is to take the focus.
    focus_first: bool,
    /// Up, down and remove of each row, to pass the focus on when one of them goes away.
    buttons: HashMap<Id, [Id; 3]>,
    /// The widgets the host registered, by name.
    widgets: HashMap<String, Box<dyn HostWidget>>,
    next_request: u64,
    /// The row each open file request is for: it can move while the dialog is open.
    file_requests: HashMap<u64, Id>,
    /// Files delivered and not yet put into their row.
    delivered: HashMap<Id, String>,
}

impl SchemaForm {
    /// `id_salt` keeps forms apart; use a new one for another block.
    pub fn new(spec: FormSpec, values: Value, id_salt: impl std::fmt::Debug + std::hash::Hash) -> Self {
        let errors = spec.validate(&values);
        SchemaForm {
            spec,
            values,
            id: Id::unique(("egui-schema-form", id_salt)),
            rows: Rows::default(),
            errors,
            texts: Texts::default(),
            style: None,
            composing: None,
            held_change: false,
            pending: None,
            focus_rows: HashMap::new(),
            status: HashMap::new(),
            buttons: HashMap::new(),
            widgets: HashMap::new(),
            next_request: 0,
            file_requests: HashMap::new(),
            delivered: HashMap::new(),
            editors: HashSet::new(),
            focus_row: None,
            focus_first: false,
        }
    }

    pub fn with_texts(mut self, texts: Texts) -> Self {
        self.texts = texts;
        self
    }

    /// The host's colours, for instance from its theme.
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = Some(style);
        self
    }

    /// The host's colours from now on: a theme can change while a form is open.
    pub fn set_style(&mut self, style: Style) {
        self.style = Some(style);
    }

    /// Registers a widget for the fields whose hints name it (`"widget": "custom"`, `"custom":
    /// name`). The name means nothing to the form.
    pub fn register_widget(&mut self, name: impl Into<String>, widget: impl HostWidget + 'static) {
        self.widgets.insert(name.into(), Box::new(widget));
    }

    /// The answer to a [`FormHost::request_file`]: the path for the image's row, wherever that
    /// row is now, or `None` if the person cancelled. A row removed meanwhile drops it.
    pub fn deliver_file(&mut self, request: FileRequest, path: Option<String>) {
        if let (Some(row), Some(path)) = (self.file_requests.remove(&request.0), path) {
            self.delivered.insert(row, path);
        }
    }

    /// The first real control takes the keyboard focus when the form is next drawn.
    pub fn request_focus(&mut self) {
        self.focus_first = true;
    }

    /// The values now. During a composition they hold its unfinished text; see
    /// [`FormOutput::changed`].
    pub fn values(&self) -> &Value {
        &self.values
    }

    /// The keys of the rows of the array field `key` at the top of the form, in order. A row keeps its
    /// key through edits, moves and the removal of others, and a new row gets a new one, so a host can
    /// tell which row is which whatever they say: `None` if `key` holds no array.
    pub fn row_keys(&mut self, key: &str) -> Option<Vec<u64>> {
        let len = self.values.get(key)?.as_array()?.len();
        Some(self.rows.keys(self.id.with(key), len))
    }

    /// [`SchemaForm::row_keys`] as they stand, without making any: `None` before the form has keys
    /// for that array.
    pub fn row_keys_now(&self, key: &str) -> Option<Vec<u64>> {
        let len = self.values.get(key)?.as_array()?.len();
        self.rows
            .peek(self.id.with(key))
            .filter(|k| k.len() == len)
            .map(<[u64]>::to_vec)
    }

    /// Every reason the values do not fit, as of the last change.
    pub fn errors(&self) -> &[FieldError] {
        &self.errors
    }

    /// An IME composition is going on in the form.
    pub fn is_composing(&self) -> bool {
        self.composing.is_some()
    }

    /// New values from outside. Rows keep their keys by `id` or equal content, then in order, so
    /// focus and caret stay where they were. During a composition this waits for its end.
    pub fn set_values(&mut self, values: Value) {
        if self.composing.is_some() {
            self.pending = Some(values);
            return;
        }
        self.rows.reconcile(&self.spec.fields, &self.values, &values, self.id);
        self.values = values;
        self.errors = self.spec.validate(&self.values);
    }

    /// Another spec for the same fields, such as their labels in another language: the values,
    /// rows and focus stay.
    pub fn set_spec(&mut self, spec: FormSpec) {
        self.spec = spec;
        self.errors = self.spec.validate(&self.values);
    }

    /// Draws the form for a host without files.
    pub fn ui(&mut self, ui: &mut Ui) -> FormOutput {
        self.ui_with(ui, &mut NoHost)
    }

    /// Draws the form.
    pub fn ui_with(&mut self, ui: &mut Ui, host: &mut dyn FormHost) -> FormOutput {
        self.track_composition(ui);
        let changed = self.draw(ui, host);
        if changed {
            self.errors = self.spec.validate(&self.values);
        }
        self.held_change |= changed;
        let mut out = FormOutput::default();
        if self.composing.is_none() {
            out.changed = std::mem::take(&mut self.held_change);
            if let Some(v) = self.pending.take() {
                self.set_values(v);
            }
        }
        out
    }

    /// IME events go to whatever has the focus, so only an editor of this form makes them ours,
    /// and a composition is that editor's: egui interrupts it when the focus moves, to another
    /// field of this form as well.
    fn track_composition(&mut self, ui: &Ui) {
        let focused = ui.memory(egui::Memory::focused).filter(|f| self.editors.contains(f));
        if self.composing != focused {
            self.composing = None;
        }
        let Some(editor) = focused else {
            return;
        };
        ui.input(|i| {
            for e in &i.events {
                match e {
                    Event::Ime(ImeEvent::Preedit { text, .. }) => {
                        self.composing = (!text.is_empty()).then_some(editor);
                    }
                    Event::Ime(ImeEvent::Commit(_)) => self.composing = None,
                    _ => {}
                }
            }
        });
    }
}
