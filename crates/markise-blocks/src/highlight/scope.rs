//! Syntect scope stacks → [`TokenKind`].
//!
//! The rules are scope-name prefixes (TextMate naming conventions), optionally with an ancestor
//! condition. A scope stack is classified from the innermost scope outwards: the first scope
//! that matches a rule decides. Within one scope the first matching rule (table order) wins.
//! `Skip` makes a scope transparent (e.g. the quotes of a string take the string's kind);
//! `Plain` stops with no kind.

use std::collections::HashMap;

use markise_theme::TokenKind;
use syntect::parsing::Scope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Kind(TokenKind),
    Plain,
    Skip,
}

use Act::{Kind, Plain, Skip};
use TokenKind as K;

/// `(ancestor selector or "", scope selector, action)`.
const RULES: &[(&str, &str, Act)] = &[
    // mapping keys (JSON) are strings inside `meta.mapping.key`
    ("meta.mapping.key", "string", Kind(K::Property)),
    ("meta.mapping.key", "punctuation.definition.string", Kind(K::Property)),
    // decimal points, exponent signs, suffix separators inside numbers
    ("constant.numeric", "punctuation", Kind(K::Number)),
    ("", "invalid.deprecated", Skip),
    ("", "invalid", Plain),
    ("", "punctuation.definition.comment", Kind(K::Comment)),
    ("", "comment", Kind(K::Comment)),
    ("", "punctuation.definition.string", Skip),
    ("", "punctuation.definition", Skip),
    ("", "constant.character.escape", Kind(K::StringSpecial)),
    ("", "constant.other.placeholder", Kind(K::StringSpecial)),
    ("", "string.regexp", Kind(K::StringSpecial)),
    ("", "string.other.link", Kind(K::Link)),
    ("", "string.unquoted.heredoc", Kind(K::String)),
    ("", "string", Kind(K::String)),
    ("", "constant.numeric", Kind(K::Number)),
    ("", "constant", Kind(K::Constant)),
    ("", "keyword.operator.word", Kind(K::Keyword)),
    ("", "keyword.operator", Kind(K::Operator)),
    ("", "keyword", Kind(K::Keyword)),
    ("", "storage", Kind(K::Keyword)),
    ("", "support.function", Kind(K::Function)),
    ("", "support.type", Kind(K::Type)),
    ("", "support.class", Kind(K::Type)),
    ("", "support.constant", Kind(K::Constant)),
    ("", "support.variable", Kind(K::Builtin)),
    ("", "support.macro", Kind(K::Function)),
    ("", "entity.name.function", Kind(K::Function)),
    ("", "entity.name.tag", Kind(K::Tag)),
    ("", "entity.other.attribute-name", Kind(K::Attribute)),
    ("", "entity.name.type", Kind(K::Type)),
    ("", "entity.name.class", Kind(K::Type)),
    ("", "entity.name.struct", Kind(K::Type)),
    ("", "entity.name.enum", Kind(K::Type)),
    ("", "entity.name.union", Kind(K::Type)),
    ("", "entity.name.trait", Kind(K::Type)),
    ("", "entity.name.interface", Kind(K::Type)),
    ("", "entity.name.impl", Kind(K::Type)),
    ("", "entity.name.namespace", Kind(K::Type)),
    ("", "entity.other.inherited-class", Kind(K::Type)),
    ("", "entity.name.constant", Kind(K::Constant)),
    ("", "entity.name.section", Kind(K::Heading)),
    ("", "entity.name.label", Kind(K::Constant)),
    ("", "entity.name", Kind(K::Function)),
    ("", "variable.function", Kind(K::Function)),
    ("", "variable.annotation", Kind(K::Meta)),
    ("", "variable.language", Kind(K::Builtin)),
    ("", "variable.parameter", Kind(K::Variable)),
    ("", "variable.other.member", Kind(K::Property)),
    ("", "variable.other.property", Kind(K::Property)),
    ("", "variable.other.object.property", Kind(K::Property)),
    ("", "variable.other.constant", Kind(K::Constant)),
    ("", "variable", Kind(K::Variable)),
    ("", "markup.heading", Kind(K::Heading)),
    ("", "markup.inserted", Kind(K::Inserted)),
    ("", "markup.deleted", Kind(K::Deleted)),
    ("", "markup.underline.link", Kind(K::Link)),
    ("", "markup.link", Kind(K::Link)),
    ("", "markup.raw", Kind(K::String)),
    ("", "markup.quote", Kind(K::Comment)),
    ("", "meta.mapping.key", Kind(K::Property)),
    ("", "meta.annotation", Kind(K::Meta)),
    ("", "meta.attribute", Kind(K::Meta)),
    ("", "meta.decorator", Kind(K::Meta)),
    ("", "meta.preprocessor", Kind(K::Meta)),
    ("", "meta.diff.header", Kind(K::Meta)),
    ("", "meta.diff.range", Kind(K::Meta)),
    ("", "punctuation", Kind(K::Punctuation)),
];

struct Rule {
    ancestor: Option<Scope>,
    scope: Scope,
    act: Act,
}

/// Classifies scope stacks; caches the candidate rules of each scope seen.
pub struct ScopeMapper {
    rules: Vec<Rule>,
    candidates: HashMap<Scope, Box<[u16]>>,
}

impl Default for ScopeMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeMapper {
    pub fn new() -> Self {
        let rules = RULES
            .iter()
            .map(|(a, s, act)| Rule {
                ancestor: (!a.is_empty()).then(|| Scope::new(a).expect("valid rule scope")),
                scope: Scope::new(s).expect("valid rule scope"),
                act: *act,
            })
            .collect();
        ScopeMapper {
            rules,
            candidates: HashMap::new(),
        }
    }

    /// The token kind of the innermost classified scope of `stack` (outermost first).
    pub fn classify(&mut self, stack: &[Scope]) -> Option<TokenKind> {
        for i in (0..stack.len()).rev() {
            let s = stack[i];
            if !self.candidates.contains_key(&s) {
                let c: Box<[u16]> = self
                    .rules
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.scope.is_prefix_of(s))
                    .map(|(k, _)| k as u16)
                    .collect();
                self.candidates.insert(s, c);
            }
            for &k in self.candidates[&s].iter() {
                let r = &self.rules[k as usize];
                if let Some(a) = r.ancestor
                    && !stack[..i].iter().any(|o| a.is_prefix_of(*o))
                {
                    continue;
                }
                match r.act {
                    Kind(kind) => return Some(kind),
                    Plain => return None,
                    Skip => break,
                }
            }
        }
        None
    }

    /// Classifies a stack given as scope names (tests and tools).
    pub fn classify_names(&mut self, names: &[&str]) -> Option<TokenKind> {
        let stack: Vec<Scope> = names.iter().filter_map(|n| Scope::new(n).ok()).collect();
        self.classify(&stack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_to_token_mapping() {
        let mut m = ScopeMapper::new();
        let mut c = |s: &[&str]| m.classify_names(s);
        assert_eq!(c(&["source.rust", "keyword.control.rust"]), Some(K::Keyword));
        assert_eq!(c(&["source.rust", "storage.type.rust"]), Some(K::Keyword));
        assert_eq!(c(&["source.rust", "comment.line.double-slash.rust"]), Some(K::Comment));
        assert_eq!(
            c(&["source.rust", "comment.line", "punctuation.definition.comment.rust"]),
            Some(K::Comment)
        );
        // quotes take the string's kind
        assert_eq!(
            c(&[
                "source.python",
                "string.quoted.double",
                "punctuation.definition.string.begin"
            ]),
            Some(K::String)
        );
        assert_eq!(
            c(&["source.python", "string.quoted.double", "constant.character.escape"]),
            Some(K::StringSpecial)
        );
        assert_eq!(c(&["source.c", "constant.numeric.integer.decimal"]), Some(K::Number));
        assert_eq!(
            c(&["source.json", "constant.numeric.float", "punctuation.separator.decimal"]),
            Some(K::Number)
        );
        assert_eq!(c(&["source.js", "constant.language.boolean"]), Some(K::Constant));
        assert_eq!(
            c(&["source.rust", "meta.function", "entity.name.function"]),
            Some(K::Function)
        );
        assert_eq!(c(&["source.ts", "support.type.primitive.ts"]), Some(K::Type));
        assert_eq!(c(&["source.rust", "entity.name.struct.rust"]), Some(K::Type));
        assert_eq!(c(&["source.js", "keyword.operator.assignment"]), Some(K::Operator));
        assert_eq!(
            c(&["source.python", "keyword.operator.logical.python"]),
            Some(K::Operator)
        );
        assert_eq!(
            c(&["source.js", "punctuation.terminator.statement"]),
            Some(K::Punctuation)
        );
        assert_eq!(c(&["text.html", "meta.tag", "entity.name.tag.html"]), Some(K::Tag));
        assert_eq!(
            c(&["text.html", "meta.tag", "entity.other.attribute-name"]),
            Some(K::Attribute)
        );
        assert_eq!(c(&["source.js", "variable.language.this"]), Some(K::Builtin));
        assert_eq!(c(&["source.diff", "markup.inserted.diff"]), Some(K::Inserted));
        assert_eq!(c(&["source.diff", "markup.deleted.diff"]), Some(K::Deleted));
        assert_eq!(c(&["source.rust", "meta.annotation.rust"]), Some(K::Meta));
        // JSON keys: strings inside meta.mapping.key
        assert_eq!(
            c(&[
                "source.json",
                "meta.mapping.json",
                "meta.mapping.key.json",
                "string.quoted.double.json"
            ]),
            Some(K::Property)
        );
        assert_eq!(
            c(&["source.json", "meta.mapping.value.json", "string.quoted.double.json"]),
            Some(K::String)
        );
        // the innermost classified scope wins: a string inside an annotation stays a string
        assert_eq!(c(&["source.rust", "meta.annotation", "string.quoted"]), Some(K::String));
        // unclassified
        assert_eq!(c(&["source.rust"]), None);
        assert_eq!(c(&["source.rust", "meta.block.rust"]), None);
        assert_eq!(c(&["source.rust", "invalid.illegal"]), None);
    }
}
