//! Command descriptors: pure data (ADR 0006 §3.1). The engine (L4) turns them into executable
//! commands; formats contribute descriptors whose `planner` they implement.

use std::fmt;

/// Identifies a planner: a format method turning a request into a [`crate::Plan`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlannerId(pub &'static str);

impl fmt::Debug for PlannerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PlannerId({})", self.0)
    }
}

/// Well-known, format-neutral planner ids. A format answers `PlanError::Unsupported` for those
/// it does not implement; format-specific planners use format-scoped ids (`markdown.…`).
pub mod planners {
    use super::PlannerId;

    /// Enter.
    pub const NEWLINE: PlannerId = PlannerId("edit.newline");
    /// Backspace with block-start rules.
    pub const DELETE_BACKWARD: PlannerId = PlannerId("edit.deleteBackward");
    pub const SOFT_BREAK: PlannerId = PlannerId("edit.softBreak");
    pub const TOGGLE_TASK: PlannerId = PlannerId("block.toggleTask");
    pub const PARAGRAPH: PlannerId = PlannerId("block.paragraph");
    pub const HEADING_1: PlannerId = PlannerId("block.heading1");
    pub const HEADING_2: PlannerId = PlannerId("block.heading2");
    pub const HEADING_3: PlannerId = PlannerId("block.heading3");
    pub const HEADING_4: PlannerId = PlannerId("block.heading4");
    pub const HEADING_5: PlannerId = PlannerId("block.heading5");
    pub const HEADING_6: PlannerId = PlannerId("block.heading6");
    pub const STRONG: PlannerId = PlannerId("format.strong");
    pub const EMPH: PlannerId = PlannerId("format.emph");
    pub const UNDERLINE: PlannerId = PlannerId("format.underline");
    pub const CODE: PlannerId = PlannerId("format.code");
    pub const STRIKE: PlannerId = PlannerId("format.strike");
    pub const HIGHLIGHT: PlannerId = PlannerId("format.highlight");
    pub const INLINE_MATH: PlannerId = PlannerId("format.inlineMath");
    pub const COMMENT: PlannerId = PlannerId("format.comment");
    pub const LINK: PlannerId = PlannerId("format.link");
    pub const IMAGE: PlannerId = PlannerId("format.image");
    pub const CLEAR_FORMAT: PlannerId = PlannerId("format.clear");
    /// Forward Delete with block-end rules (C48).
    pub const DELETE_FORWARD: PlannerId = PlannerId("edit.deleteForward");
    /// Tab: list indent, literal tab… (spec 02 §5.3).
    pub const TAB: PlannerId = PlannerId("edit.indentOrNextCell");
    /// Shift+Tab: list outdent, leading-space removal… (spec 02 §5.3).
    pub const SHIFT_TAB: PlannerId = PlannerId("edit.outdentOrPrevCell");
    /// macOS Ctrl+O: paragraph break after the caret, caret stays.
    pub const OPEN_LINE: PlannerId = PlannerId("edit.openLine");
    pub const INDENT: PlannerId = PlannerId("block.indent");
    pub const OUTDENT: PlannerId = PlannerId("block.outdent");
    pub const HEADING_UP: PlannerId = PlannerId("block.headingUp");
    pub const HEADING_DOWN: PlannerId = PlannerId("block.headingDown");
    pub const QUOTE: PlannerId = PlannerId("block.quote");
    pub const BULLET_LIST: PlannerId = PlannerId("block.bulletList");
    pub const ORDERED_LIST: PlannerId = PlannerId("block.orderedList");
    pub const TASK_LIST: PlannerId = PlannerId("block.taskList");
    pub const MOVE_UP: PlannerId = PlannerId("edit.moveBlockUp");
    pub const MOVE_DOWN: PlannerId = PlannerId("edit.moveBlockDown");
    pub const SELECT_WORD: PlannerId = PlannerId("select.word");
    pub const SELECT_LINE: PlannerId = PlannerId("select.line");
    pub const SELECT_SCOPE: PlannerId = PlannerId("select.scope");
    pub const SELECT_BLOCK: PlannerId = PlannerId("select.block");
    pub const DELETE_WORD: PlannerId = PlannerId("delete.word");
    pub const DELETE_LINE: PlannerId = PlannerId("delete.line");
    pub const DELETE_SCOPE: PlannerId = PlannerId("delete.scope");
    pub const DELETE_BLOCK: PlannerId = PlannerId("delete.block");

    /// Heading level for a `block.headingN` id.
    pub fn heading_level(id: PlannerId) -> Option<u8> {
        let n = id.0.strip_prefix("block.heading")?;
        let l: u8 = n.parse().ok()?;
        (1..=6).contains(&l).then_some(l)
    }
}

/// Where a command's key binding applies; more specific scopes win (ADR 0006 §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Editor,
    Table,
    Code,
    Source,
    Find,
}

/// How a mutating command groups into undo history (ADR 0006 §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UndoGroup {
    /// Coalesces with neighbouring keystrokes.
    Typing,
    /// Enter, block conversion, structure edits: always its own transaction.
    Structural,
    /// Formatting commands: own transaction.
    Format,
    /// Paste / drop: own transaction.
    Paste,
    /// Automatic rewrites (input-rule conversions, hint materialisation).
    Auto,
    /// Large edits (Replace All, reload).
    Bulk,
}

/// What a command does to the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Reads only.
    Pure,
    /// Changes view state (selection, scroll, modes), never the text.
    View,
    /// Changes text and/or hints; recorded in history.
    Mutates { group: UndoGroup },
}

/// Per-platform default chord in canonical text (`"Cmd+B"`, `"Ctrl+Shift+K"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DefaultKey {
    pub mac: Option<&'static str>,
    pub other: Option<&'static str>,
}

/// Type of a command parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamKind {
    Text,
    Int,
    Bool,
}

/// One declared parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
}

/// Typed parameter schema (a JSON schema is generated from it for MCP/CLI, M3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParamSchema(pub &'static [ParamSpec]);

impl ParamSchema {
    pub const NONE: ParamSchema = ParamSchema(&[]);
}

/// A command: identity, presentation, scope, effect, parameters and (for format commands) the
/// planner implementing it. Pure data, `const`-constructible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommandDescriptor {
    /// Stable id, e.g. `"format.strong"`, `"markdown.insertLinkRef"`.
    pub id: &'static str,
    /// Menu title; stable (keyBinding overrides, macOS App Shortcuts).
    pub label: &'static str,
    pub menu: Option<&'static [&'static str]>,
    pub keys: &'static [DefaultKey],
    pub scope: Scope,
    pub effect: Effect,
    pub params: ParamSchema,
    pub planner: Option<PlannerId>,
}

/// A parameter value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    Text(String),
    Int(i64),
    Bool(bool),
}

impl ParamValue {
    fn kind(&self) -> ParamKind {
        match self {
            ParamValue::Text(_) => ParamKind::Text,
            ParamValue::Int(_) => ParamKind::Int,
            ParamValue::Bool(_) => ParamKind::Bool,
        }
    }
}

/// Parameters passed to a command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Params(Vec<(String, ParamValue)>);

impl Params {
    pub fn new() -> Self {
        Params(Vec::new())
    }
    pub fn with(mut self, name: &str, value: ParamValue) -> Self {
        self.0.retain(|(n, _)| n != name);
        self.0.push((name.to_string(), value));
        self
    }
    pub fn text(name: &str, value: impl Into<String>) -> Self {
        Params::new().with(name, ParamValue::Text(value.into()))
    }
    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
    pub fn get_text(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            ParamValue::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn get_int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            ParamValue::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            ParamValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// Checks the values against `schema`: required present, types match, nothing unknown.
    pub fn validate(&self, schema: &ParamSchema) -> Result<(), String> {
        for spec in schema.0 {
            match self.get(spec.name) {
                None if spec.required => return Err(format!("missing parameter `{}`", spec.name)),
                Some(v) if v.kind() != spec.kind => {
                    return Err(format!("parameter `{}`: expected {:?}", spec.name, spec.kind));
                }
                _ => {}
            }
        }
        if let Some((n, _)) = self.0.iter().find(|(n, _)| !schema.0.iter().any(|s| s.name == n)) {
            return Err(format!("unknown parameter `{n}`"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_validate() {
        const S: ParamSchema = ParamSchema(&[ParamSpec {
            name: "text",
            kind: ParamKind::Text,
            required: true,
        }]);
        assert!(Params::text("text", "x").validate(&S).is_ok());
        assert!(Params::new().validate(&S).is_err());
        assert!(Params::new().with("text", ParamValue::Int(1)).validate(&S).is_err());
        assert!(
            Params::text("text", "x")
                .with("y", ParamValue::Bool(true))
                .validate(&S)
                .is_err()
        );
    }

    #[test]
    fn heading_levels() {
        assert_eq!(planners::heading_level(planners::HEADING_3), Some(3));
        assert_eq!(planners::heading_level(planners::NEWLINE), None);
    }
}
