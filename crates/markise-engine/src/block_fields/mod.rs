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
mod slots;
mod values;

pub(crate) use encode::{ids_in, new_anchor};
pub use fit::{BlockFit, decls_from_spec};
pub use slots::{Refused, Slot, SlotAt, SlotCaret, SlotEdit};

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
    /// The arguments of the opener, not anything in the body: no place to type, no region of the
    /// block. The form is the only way to it, apart from the Markdown.
    Argument,
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
    /// Empties a heading and keeps it in the source, with its marks and its id: it has a place
    /// for the caret, and the text typed next goes there.
    ClearHeading {
        field: String,
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
    /// The edit or value does not fit the field's kind, or an argument would not read back.
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
    /// What each body block parses as, in order, and which field took it.
    shapes: Vec<(decode::Shape, Option<usize>)>,
    /// What stands below the body blocks, in order, apart from paragraphs and list items.
    nested: Vec<std::mem::Discriminant<markise_md::BlockKind>>,
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

    /// The Markdown nested under a list item or an answer, after its own text, as it stands in the
    /// source: indented, from the first line after that text. `None` when there is none. It is part
    /// of the item, moves and goes with it and keeps its bytes, and is no part of any value.
    pub fn nested_under(&self, item: ItemId) -> Option<&str> {
        let src = (self.fields.iter())
            .filter_map(|f| match &f.value {
                FieldValue::Items(items) => Some(items),
                FieldValue::Text(_) => None,
            })
            .flatten()
            .find(|i| i.id == item)?
            .src
            .as_ref()?;
        let text = self.source[src.under.clone()].trim_start_matches(['\r', '\n']);
        (!text.is_empty()).then_some(text)
    }

    /// Whether any item has Markdown nested under it ([`BlockFields::nested_under`]).
    pub fn has_nested(&self) -> bool {
        (self.fields.iter())
            .filter_map(|f| match &f.value {
                FieldValue::Items(items) => Some(items),
                FieldValue::Text(_) => None,
            })
            .flatten()
            .any(|i| self.nested_under(i.id).is_some())
    }

    /// The regions the fields took, in source order, with the kind of field each is.
    pub fn field_regions(&self) -> Vec<(Range<usize>, FieldKind)> {
        (self.chunks.iter())
            .filter_map(|c| Some((c.range.clone(), self.fields[c.field?].kind)))
            .collect()
    }

    /// The source ranges of the field of this name, ascending.
    pub fn regions_of(&self, name: &str) -> Vec<Range<usize>> {
        let at = self.fields.iter().position(|f| f.name == name);
        (self.chunks.iter())
            .filter(|c| at.is_some() && c.field == at)
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

    /// Whether the two read as the same block, apart from the text in its fields.
    pub fn same_structure(&self, other: &BlockFields) -> bool {
        self.name() == other.name()
            && self.args() == other.args()
            && self.structure() == other.structure()
            && self.nested == other.nested
    }

    /// What the body parses as, apart from how many paragraphs a text field has.
    fn structure(&self) -> Vec<(decode::Shape, Option<usize>)> {
        let mut out: Vec<(decode::Shape, Option<usize>)> = Vec::new();
        for s in &self.shapes {
            if !(s.0 == decode::Shape::Paragraph && out.last() == Some(s)) {
                out.push(*s);
            }
        }
        out
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
