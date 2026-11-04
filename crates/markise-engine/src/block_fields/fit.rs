//! A block against its form spec. A block that does not fit stays editable and can be saved:
//! this only reports, field by field. Strict checks belong to whoever publishes.

use std::ops::Range;

use schema_form_core::{ChoiceShape, Control, FieldError, FieldSpec, FormSpec};
use serde_json::Value;

use super::{BlockFields, FieldDecl, FieldKind};

/// How a block fits its spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockFit {
    /// The reasons per field; see [`FormSpec::validate`].
    pub errors: Vec<FieldError>,
    /// Content no field took, in source coordinates of the block.
    pub extras: Vec<Range<usize>>,
}

impl BlockFit {
    pub fn fits(&self) -> bool {
        self.errors.is_empty() && self.extras.is_empty()
    }
}

impl BlockFields {
    pub fn check(&self, spec: &FormSpec) -> BlockFit {
        BlockFit {
            errors: spec.validate(&self.values()),
            extras: self.extras(),
        }
    }
}

/// Where the field lives in Markdown: from the hint `x-markdown` (`{"kind": "heading", "level":
/// 2}`, or the kind alone), else from its control. The form crate ignores `x-` hints, so it
/// stays free of Markdown.
fn kind_of(f: &FieldSpec) -> Option<FieldKind> {
    let hint = f.extensions.get("x-markdown");
    let named = hint.and_then(|h| h.as_str().or_else(|| h.get("kind")?.as_str()));
    let level = hint
        .and_then(|h| h.get("level"))
        .and_then(Value::as_u64)
        .and_then(|l| u8::try_from(l).ok());
    match named {
        Some("heading") => Some(FieldKind::Heading { level }),
        Some("text") => Some(FieldKind::Text),
        Some("quote") => Some(FieldKind::Quote),
        Some("list") => Some(FieldKind::List),
        Some("choices") => Some(FieldKind::Choices),
        Some("image") => Some(FieldKind::Image),
        Some(_) => None,
        None => match f.control {
            Control::Text { .. } => Some(FieldKind::Text),
            Control::StringList { .. } => Some(FieldKind::List),
            Control::Choices { .. } => Some(FieldKind::Choices),
            Control::ChoiceButtons {
                shape: ChoiceShape::Marks,
                ..
            } => Some(FieldKind::Choices),
            Control::Images => Some(FieldKind::Image),
            _ => None,
        },
    }
}

/// The declared fields of a block with this spec, in the spec's order. `Err` names a field that
/// has no place in Markdown.
pub fn decls_from_spec(spec: &FormSpec) -> Result<Vec<FieldDecl>, String> {
    (spec.fields.iter())
        .map(|f| {
            kind_of(f)
                .map(|k| FieldDecl::new(&f.key, k))
                .ok_or_else(|| f.key.clone())
        })
        .collect()
}
