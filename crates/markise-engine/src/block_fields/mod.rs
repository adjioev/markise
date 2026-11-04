//! The field model of a `:::name` block (ADR 0020): Markdown source to named fields, and field
//! edits to minimal patches.
//!
//! The model is renderer-independent. A side panel, a form and in-place editing all read
//! [`BlockFields`] and change it with [`FieldEdit`]s. The source stays the truth: every value is
//! derived from it, a value that did not change keeps its bytes, and unknown content is kept as
//! [`BlockFields::extras`]. Markise holds no vocabulary here: a block's fields come from the
//! caller as [`FieldDecl`]s.

mod decode;
mod encode;
mod escape;
mod fit;
mod identity;
mod values;

pub use fit::{BlockFit, decls_from_spec};

use std::collections::BTreeSet;
use std::ops::Range;

use markise_doc::{FormatDoc, Patch, PatchError, PatchSet};
use markise_md::ParseOptions;

use crate::editor::{EditError, EditorSession};

/// What a field holds and how it is written in Markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// One heading; with a level only a heading of that level fits. Its `{#id}` is kept hidden.
    Heading { level: Option<u8> },
    /// Paragraphs and whatever no other field takes, as one Markdown string.
    Text,
    /// One block quote, its text without the `>` prefixes.
    Quote,
    /// One list of strings.
    List,
    /// One task list: answers with a persistent `{#id}` and a correctness flag.
    Choices,
    /// Paragraphs holding only images, one image per line.
    Image,
}

/// One declared field; the order of the declarations is the order in the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: String,
    pub kind: FieldKind,
}

impl FieldDecl {
    pub fn new(name: impl Into<String>, kind: FieldKind) -> Self {
        FieldDecl {
            name: name.into(),
            kind,
        }
    }
}

/// The identity of a list item, an answer or an image. It lasts as long as the model and its
/// successors ([`Applied::fields`], [`BlockFields::refreshed`]), through edits and reorders; it
/// is not written to the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(u32);

/// The content of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemData {
    Text(String),
    /// `anchor` is the `{#id}` without braces and `#`; `None` when the source has none.
    Choice {
        anchor: Option<String>,
        text: String,
        correct: bool,
    },
    Image {
        alt: String,
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub data: ItemData,
    /// Where it was read from; a new item has none.
    src: Option<decode::ItemSrc>,
}

/// A field's current value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    /// Heading, text and quote. The empty string means the field is absent.
    Text(String),
    /// List, choices and images.
    Items(Vec<Item>),
}

/// One field of a decoded block. `name` is its stable identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub value: FieldValue,
    src: decode::FieldSrc,
}

/// One change. Edits apply in order; the indexes are positions at that moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldEdit {
    /// Heading, text, quote. Empty removes the field from the source.
    SetText {
        field: String,
        text: String,
    },
    /// A list item's or an answer's text.
    SetItemText {
        field: String,
        item: ItemId,
        text: String,
    },
    SetCorrect {
        field: String,
        item: ItemId,
        correct: bool,
    },
    SetImage {
        field: String,
        item: ItemId,
        alt: String,
        path: String,
    },
    InsertItem {
        field: String,
        at: usize,
        item: ItemData,
    },
    RemoveItem {
        field: String,
        item: ItemId,
    },
    MoveItem {
        field: String,
        item: ItemId,
        to: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldsError {
    /// The text is not one paired `:::name` block from its first byte to its last.
    NotABlock,
    UnknownField(String),
    /// An item id that is not in the field (or an index past its end).
    UnknownItem {
        field: String,
    },
    /// The edit or value does not fit the field's kind.
    WrongKind {
        field: String,
    },
    /// The document no longer holds this model's source at its place.
    Stale,
    Patch(PatchError),
    Edit(EditError),
}

impl std::fmt::Display for FieldsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FieldsError::NotABlock => write!(f, "not a paired ::: block"),
            FieldsError::UnknownField(n) => write!(f, "no field `{n}`"),
            FieldsError::UnknownItem { field } => write!(f, "no such item in field `{field}`"),
            FieldsError::WrongKind { field } => write!(f, "the value does not fit field `{field}`"),
            FieldsError::Stale => write!(f, "the document changed since the block was read"),
            FieldsError::Patch(e) => write!(f, "{e}"),
            FieldsError::Edit(e) => write!(f, "{e:?}"),
        }
    }
}

impl std::error::Error for FieldsError {}

/// A decoded block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockFields {
    source: String,
    base: usize,
    name: Range<usize>,
    args: Range<usize>,
    /// End of the opener line, line terminator excluded.
    opener_end: usize,
    /// Start of the closer's line.
    closer_start: usize,
    eol: &'static str,
    opts: ParseOptions,
    decls: Vec<FieldDecl>,
    fields: Vec<Field>,
    /// The regions of the block in source order.
    chunks: Vec<decode::Chunk>,
    next_item: u32,
    /// Anchors removed through this model and its predecessors: never given out again.
    retired: BTreeSet<String>,
}

/// The result of [`BlockFields::apply`].
#[derive(Clone, Debug)]
pub struct Applied {
    /// Minimal, verbatim patches in the coordinates of the model's [`BlockFields::source`].
    pub patches: PatchSet,
    /// The model of the new source, keeping the item identities.
    pub fields: BlockFields,
    /// The ids of the [`FieldEdit::InsertItem`]s, in order.
    pub inserted: Vec<ItemId>,
}

impl BlockFields {
    /// Reads `source`, exactly one `:::name` block from its opener's first colon to the end of
    /// its closer line. `base` is where it sits in the document.
    pub fn decode(source: &str, base: usize, decls: &[FieldDecl], opts: &ParseOptions) -> Result<Self, FieldsError> {
        decode::decode(source, base, decls, opts)
    }

    /// Reads the block's new source, keeping item identities by anchor, then by equal content,
    /// then by position. For a source changed elsewhere (typing in Markdown view, undo).
    pub fn refreshed(&self, source: &str, base: usize) -> Result<Self, FieldsError> {
        let mut next = decode::decode(source, base, &self.decls, &self.opts)?;
        next.retired = self.retired.clone();
        identity::carry_ids(self, &mut next);
        Ok(next)
    }

    /// The block's source as read.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// The offset of [`BlockFields::source`] in the document.
    pub fn base(&self) -> usize {
        self.base
    }
    pub fn name(&self) -> &str {
        &self.source[self.name.clone()]
    }
    pub fn args(&self) -> &str {
        &self.source[self.args.clone()]
    }
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
    /// Regions of the source no field took, in source coordinates. No edit touches them.
    pub fn extras(&self) -> Vec<Range<usize>> {
        (self.chunks.iter())
            .filter(|c| c.field.is_none())
            .map(|c| c.range.clone())
            .collect()
    }

    /// Applies `edits` as one change. No edit changes nothing, and neither do edits that give
    /// the same values: the patches are empty.
    pub fn apply(&self, edits: &[FieldEdit]) -> Result<Applied, FieldsError> {
        encode::apply(self, edits)
    }

    /// [`BlockFields::apply`] on the session's document, as one undo step named `label`.
    pub fn apply_to_session(
        &self,
        session: &mut EditorSession,
        edits: &[FieldEdit],
        label: &'static str,
    ) -> Result<Applied, FieldsError> {
        if session.text().get(self.base..self.base + self.source.len()) != Some(self.source.as_str()) {
            return Err(FieldsError::Stale);
        }
        let mut out = self.apply(edits)?;
        let shifted = (out.patches.patches().iter())
            .map(|p| Patch::verbatim(p.range.start + self.base..p.range.end + self.base, p.insert.clone()))
            .collect();
        let patches = PatchSet::new(shifted).map_err(FieldsError::Patch)?;
        session
            .apply_external(&patches, label, false)
            .map_err(FieldsError::Edit)?;
        out.fields.base = self.base;
        Ok(out)
    }

    fn field_index(&self, name: &str) -> Result<usize, FieldsError> {
        (self.fields.iter().position(|f| f.name == name)).ok_or_else(|| FieldsError::UnknownField(name.to_string()))
    }
}

/// The source range of the innermost named container around `at`, for [`BlockFields::decode`].
pub fn block_range_at(doc: &dyn FormatDoc, at: usize) -> Option<Range<usize>> {
    let c = doc.named_containers_at(at).pop()?;
    Some(c.opener.start..c.closer.end)
}
