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

mod draw;
mod rows;
mod widgets;

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Event, Id, ImeEvent, Ui};
use schema_form_core::{ErrorKind, FieldError, FormSpec};
use serde_json::Value;

pub use schema_form_core;

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
    composing: bool,
    /// Changed during the composition, not yet reported.
    held_change: bool,
    /// Values from [`SchemaForm::set_values`] waiting for the composition to end.
    pending: Option<Value>,
    /// Which row each focusable widget of the last frame belongs to.
    focus_rows: HashMap<Id, Id>,
    /// What screen readers last heard about a move, per array.
    status: HashMap<Id, String>,
    /// Up, down and remove of each row, to pass the focus on when one of them goes away.
    buttons: HashMap<Id, [Id; 3]>,
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
            composing: false,
            held_change: false,
            pending: None,
            focus_rows: HashMap::new(),
            status: HashMap::new(),
            buttons: HashMap::new(),
        }
    }

    pub fn with_texts(mut self, texts: Texts) -> Self {
        self.texts = texts;
        self
    }

    /// The values now. During a composition they hold its unfinished text; see
    /// [`FormOutput::changed`].
    pub fn values(&self) -> &Value {
        &self.values
    }

    /// Every reason the values do not fit, as of the last change.
    pub fn errors(&self) -> &[FieldError] {
        &self.errors
    }

    /// An IME composition is going on in the form.
    pub fn is_composing(&self) -> bool {
        self.composing
    }

    /// New values from outside. Rows keep their keys by `id` or equal content, then in order, so
    /// focus and caret stay where they were. During a composition this waits for its end.
    pub fn set_values(&mut self, values: Value) {
        if self.composing {
            self.pending = Some(values);
            return;
        }
        self.rows.reconcile(&self.spec.fields, &self.values, &values, self.id);
        self.values = values;
        self.errors = self.spec.validate(&self.values);
    }

    /// Draws the form.
    pub fn ui(&mut self, ui: &mut Ui) -> FormOutput {
        self.track_composition(ui);
        let changed = self.draw(ui);
        if changed {
            self.errors = self.spec.validate(&self.values);
        }
        self.held_change |= changed;
        let mut out = FormOutput::default();
        if !self.composing {
            out.changed = std::mem::take(&mut self.held_change);
            if let Some(v) = self.pending.take() {
                self.set_values(v);
            }
        }
        out
    }

    fn track_composition(&mut self, ui: &Ui) {
        ui.input(|i| {
            for e in &i.events {
                match e {
                    Event::Ime(ImeEvent::Preedit { text, .. }) => self.composing = !text.is_empty(),
                    Event::Ime(ImeEvent::Commit(_)) => self.composing = false,
                    _ => {}
                }
            }
        });
    }
}
