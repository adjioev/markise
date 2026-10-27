//! Code-highlighting token categories (spec 03 §4.5) and their look in a theme.
//!
//! The highlighter (`markise-blocks`) classifies code into [`TokenKind`]s; it never picks
//! colors. Colors and styles come from the active color scheme at layout / export time:
//! [`Theme::token_look`] resolves a kind through, in order,
//! 1. the theme's imported CodeMirror token styles ([`Theme::syntax_tokens`], `cm-*` rules),
//! 2. the six [`SyntaxColors`](crate::SyntaxColors) slots every scheme defines,
//! 3. a derivation from other scheme colors for the kinds without a slot of their own
//!    (constants use the number color, tags the keyword color, inserted / deleted lines the
//!    tip / caution alert colors, …), or the plain code color.
//!
//! Highlight results therefore do not depend on the scheme: switching schemes re-lays out code
//! but never re-highlights it.

use crate::{Rgba, Theme};

/// A token category. `#[repr(u8)]` so highlight runs stay small.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum TokenKind {
    Keyword,
    String,
    /// Escapes, regular expressions, interpolation markers inside strings (`cm-string-2`).
    StringSpecial,
    Number,
    Comment,
    /// Function / definition names (`cm-def`).
    Function,
    /// Type, class and trait names (`cm-variable-3`).
    Type,
    /// Language constants: `true`, `null`, character constants (`cm-atom`).
    Constant,
    /// Built-in names: `self`, `this`, library variables (`cm-builtin`).
    Builtin,
    Operator,
    /// Brackets, separators, terminators (`cm-bracket`).
    Punctuation,
    Variable,
    /// Object properties / mapping keys (`cm-property`).
    Property,
    /// Markup tag names; YAML keys (`cm-tag`).
    Tag,
    /// Markup attribute names (`cm-attribute`).
    Attribute,
    /// Annotations, attributes, preprocessor lines (`cm-meta`).
    Meta,
    /// Headings in markup languages (`cm-header`).
    Heading,
    /// Inserted lines in diffs (`cm-positive`).
    Inserted,
    /// Deleted lines in diffs (`cm-negative`).
    Deleted,
    /// Links / URLs in markup (`cm-link`).
    Link,
}

impl TokenKind {
    pub const ALL: [TokenKind; 20] = [
        TokenKind::Keyword,
        TokenKind::String,
        TokenKind::StringSpecial,
        TokenKind::Number,
        TokenKind::Comment,
        TokenKind::Function,
        TokenKind::Type,
        TokenKind::Constant,
        TokenKind::Builtin,
        TokenKind::Operator,
        TokenKind::Punctuation,
        TokenKind::Variable,
        TokenKind::Property,
        TokenKind::Tag,
        TokenKind::Attribute,
        TokenKind::Meta,
        TokenKind::Heading,
        TokenKind::Inserted,
        TokenKind::Deleted,
        TokenKind::Link,
    ];

    /// The CodeMirror token class of spec 03 §4.5 (HTML export class, theme import key).
    pub const fn cm_class(self) -> &'static str {
        match self {
            TokenKind::Keyword => "cm-keyword",
            TokenKind::String => "cm-string",
            TokenKind::StringSpecial => "cm-string-2",
            TokenKind::Number => "cm-number",
            TokenKind::Comment => "cm-comment",
            TokenKind::Function => "cm-def",
            TokenKind::Type => "cm-variable-3",
            TokenKind::Constant => "cm-atom",
            TokenKind::Builtin => "cm-builtin",
            TokenKind::Operator => "cm-operator",
            TokenKind::Punctuation => "cm-bracket",
            TokenKind::Variable => "cm-variable",
            TokenKind::Property => "cm-property",
            TokenKind::Tag => "cm-tag",
            TokenKind::Attribute => "cm-attribute",
            TokenKind::Meta => "cm-meta",
            TokenKind::Heading => "cm-header",
            TokenKind::Inserted => "cm-positive",
            TokenKind::Deleted => "cm-negative",
            TokenKind::Link => "cm-link",
        }
    }

    /// Other class names a theme may use for the same kind (importer aliases).
    const fn cm_aliases(self) -> &'static [&'static str] {
        match self {
            TokenKind::Type => &["cm-type"],
            TokenKind::Variable => &["cm-variable-2"],
            TokenKind::Property => &["cm-qualifier"],
            _ => &[],
        }
    }
}

/// One highlighted byte range of a code block: `start..end` are byte offsets into the block's
/// content (its lines, each followed by `\n`). Gaps between runs are plain code text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TokenRun {
    pub start: u32,
    pub end: u32,
    pub kind: TokenKind,
}

/// How a token kind is drawn. `color: None` = the code block's text color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TokenLook {
    pub color: Option<Rgba>,
    pub bold: bool,
    pub italic: bool,
}

impl TokenLook {
    pub fn is_plain(&self) -> bool {
        self.color.is_none() && !self.bold && !self.italic
    }
}

impl Theme {
    /// The look of `kind` in this theme (see the module docs for the resolution order).
    pub fn token_look(&self, kind: TokenKind) -> TokenLook {
        let imported = self
            .syntax_tokens
            .iter()
            .find(|t| t.class == kind.cm_class())
            .or_else(|| {
                kind.cm_aliases()
                    .iter()
                    .find_map(|a| self.syntax_tokens.iter().find(|t| t.class == *a))
            });
        if let Some(t) = imported {
            return TokenLook {
                color: t.color.or_else(|| self.token_color(kind)),
                bold: t.bold,
                italic: t.italic,
            };
        }
        TokenLook {
            color: self.token_color(kind),
            bold: kind == TokenKind::Heading,
            italic: false,
        }
    }

    /// The scheme color of `kind` without imported token styles.
    fn token_color(&self, kind: TokenKind) -> Option<Rgba> {
        let s = &self.syntax;
        let code = self.code_block.text;
        let c = match kind {
            TokenKind::Keyword | TokenKind::Tag | TokenKind::Heading => s.keyword,
            TokenKind::String | TokenKind::StringSpecial => s.string,
            TokenKind::Number | TokenKind::Constant => s.number,
            TokenKind::Comment => s.comment,
            TokenKind::Function | TokenKind::Property | TokenKind::Builtin => s.function,
            TokenKind::Type | TokenKind::Attribute | TokenKind::Meta => s.type_name,
            TokenKind::Inserted => self.alert[1],
            TokenKind::Deleted => self.alert[4],
            TokenKind::Link => self.link,
            TokenKind::Operator | TokenKind::Punctuation | TokenKind::Variable => return None,
        };
        (c != code).then_some(c)
    }

    /// Hash of every input of [`Theme::token_look`] (part of highlight-dependent cache keys).
    pub fn token_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for k in TokenKind::ALL {
            self.token_look(k).hash(&mut h);
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenStyle;

    #[test]
    fn classes_are_unique_and_from_spec_03() {
        let mut seen = std::collections::HashSet::new();
        for k in TokenKind::ALL {
            assert!(seen.insert(k.cm_class()), "{k:?}");
            assert!(k.cm_class().starts_with("cm-"));
        }
    }

    #[test]
    fn slots_and_derivations() {
        let t = Theme::light();
        assert_eq!(t.token_look(TokenKind::Keyword).color, Some(t.syntax.keyword));
        assert_eq!(t.token_look(TokenKind::Function).color, Some(t.syntax.function));
        assert_eq!(t.token_look(TokenKind::Type).color, Some(t.syntax.type_name));
        assert_eq!(t.token_look(TokenKind::Constant).color, Some(t.syntax.number));
        assert_eq!(t.token_look(TokenKind::Inserted).color, Some(t.alert[1]));
        assert!(t.token_look(TokenKind::Operator).is_plain());
        assert!(t.token_look(TokenKind::Heading).bold);
    }

    #[test]
    fn imported_token_styles_win() {
        let mut t = Theme::dark();
        t.syntax_tokens.push(TokenStyle {
            class: "cm-comment".into(),
            color: None,
            bold: false,
            italic: true,
        });
        t.syntax_tokens.push(TokenStyle {
            class: "cm-operator".into(),
            color: Some(Rgba::hex(0x123456)),
            bold: true,
            italic: false,
        });
        t.syntax_tokens.push(TokenStyle {
            class: "cm-type".into(),
            color: Some(Rgba::hex(0x654321)),
            bold: false,
            italic: false,
        });
        let c = t.token_look(TokenKind::Comment);
        assert_eq!(c.color, Some(t.syntax.comment), "color falls back to the slot");
        assert!(c.italic);
        let o = t.token_look(TokenKind::Operator);
        assert_eq!((o.color, o.bold), (Some(Rgba::hex(0x123456)), true));
        assert_eq!(t.token_look(TokenKind::Type).color, Some(Rgba::hex(0x654321)), "alias");
        assert_ne!(t.token_fingerprint(), Theme::dark().token_fingerprint());
    }
}
