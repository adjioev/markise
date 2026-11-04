//! Forms from JSON (ADR 0020): a declared subset of JSON Schema plus UI hints become a tree of
//! fields, and values are checked against it. No GUI here, so a native form and a host's own
//! checks read the same rules; nothing here knows what the values mean or where they are kept.
//!
//! The subset is versioned ([`SPEC_VERSION`]). A schema or hints using anything outside it is an
//! error ([`FormSpec::parse`]), not silently ignored: what parses is what is promised.

mod parse;
mod validate;

pub use validate::{ErrorKind, FieldError};

/// The version of the subset and of the hints format.
pub const SPEC_VERSION: u64 = 1;

/// How many answers can be marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Radio buttons: marking one clears the others.
    Single,
    /// Checkboxes.
    Multiple,
}

/// What must hold for the marks of a choices field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorrectRule {
    Any,
    ExactlyOne,
    AtLeastOne,
}

/// What a field is, as a form draws it.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// A string.
    Text { multiline: bool },
    /// A boolean.
    Checkbox,
    /// An array of strings, reorderable.
    StringList { multiline: bool },
    /// An array of objects drawn as groups of `fields`, reorderable.
    Array { fields: Vec<FieldSpec> },
    /// An array of `{id: string, text: string, correct: boolean}`, reorderable.
    Choices { selection: Selection, correct: CorrectRule },
    /// An array of `{alt: string, path: string}`, reorderable.
    Images,
    /// An object drawn as a group.
    Group { fields: Vec<FieldSpec> },
    /// Drawn by what the host registered under this name. Its schema still holds; without a
    /// `type` any value goes.
    Custom { name: String },
}

/// The limits of a value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
}

/// What a value must look like, whatever draws it. Parsed from the schema, in the hints' order.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueRule {
    pub json_type: JsonType,
    pub limits: Limits,
    /// The rule of an array's entries.
    pub items: Option<Box<ValueRule>>,
    /// The properties of an object.
    pub props: Vec<PropRule>,
    /// `additionalProperties: false`.
    pub closed: bool,
    /// An array of answers: what must hold for their marks. Their ids are checked as well.
    pub choices: Option<CorrectRule>,
}

/// One property of an object.
#[derive(Clone, Debug, PartialEq)]
pub struct PropRule {
    pub key: String,
    pub required: bool,
    pub rule: ValueRule,
}

/// One field.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpec {
    /// The key in its object; the field's identity.
    pub key: String,
    pub label: String,
    pub help: Option<String>,
    /// The label of the add button of an array.
    pub add_label: Option<String>,
    pub required: bool,
    pub control: Control,
    pub rule: ValueRule,
    /// Hint keys starting with `x-`: ignored by the form, for a host of its own.
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonType {
    String,
    Boolean,
    Array,
    Object,
    /// A custom widget's value without a declared type.
    Any,
}

/// A parsed form; parse once per schema, hints and variant.
#[derive(Clone, Debug, PartialEq)]
pub struct FormSpec {
    pub fields: Vec<FieldSpec>,
    /// What the values must look like as a whole.
    pub rule: ValueRule,
}

/// Why a schema or its hints are outside the subset. `path` names the place, `/` separated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecError {
    pub path: String,
    pub reason: String,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

impl std::error::Error for SpecError {}
