//! Source-mode token classes (spec 02 §12.1.1, ADR 0006 §5.5): what the source editor colours.
//!
//! A format reports, for a byte range of its text, which parts are heading text, emphasis,
//! markers, code and so on ([`crate::FormatDoc::source_tokens`]). The classes are
//! format-neutral; the layout maps them to the colours of the active theme. Tokens come from
//! the format's own parse (exact spans), never from regular expressions over the text.

use std::ops::Range;

/// A token class of the source view, in increasing paint priority: where the format reports
/// overlapping tokens, the later (higher) class wins byte by byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum SourceTok {
    #[default]
    Plain = 0,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    /// Text inside a block quote.
    Quote,
    Emph,
    Strong,
    StrongEmph,
    Strike,
    /// Highlighted (`==mark==`) text.
    Mark,
    /// Link text, footnote references, reference labels.
    Link,
    /// Link destinations and bare URLs.
    Url,
    /// Inline code content.
    Code,
    /// Content of a code block (fenced or indented).
    CodeBlock,
    /// Metadata tint: front matter, math, raw HTML, `[toc]`, link titles.
    Meta,
    /// Syntax markers, dimmed: `#`, `*`, `_`, backticks, fences, `>`, list markers, brackets,
    /// table pipes and delimiter rows.
    Marker,
}

impl SourceTok {
    pub const ALL: [SourceTok; 19] = [
        SourceTok::Plain,
        SourceTok::Heading1,
        SourceTok::Heading2,
        SourceTok::Heading3,
        SourceTok::Heading4,
        SourceTok::Heading5,
        SourceTok::Heading6,
        SourceTok::Quote,
        SourceTok::Emph,
        SourceTok::Strong,
        SourceTok::StrongEmph,
        SourceTok::Strike,
        SourceTok::Mark,
        SourceTok::Link,
        SourceTok::Url,
        SourceTok::Code,
        SourceTok::CodeBlock,
        SourceTok::Meta,
        SourceTok::Marker,
    ];

    /// The class with this discriminant (`Plain` for unknown values).
    pub fn from_u8(v: u8) -> SourceTok {
        SourceTok::ALL.get(v as usize).copied().unwrap_or(SourceTok::Plain)
    }

    /// The heading class of `level` (clamped to 1–6).
    pub fn heading(level: u8) -> SourceTok {
        SourceTok::from_u8(SourceTok::Heading1 as u8 + level.clamp(1, 6) - 1)
    }

    /// The heading level of a heading class.
    pub fn heading_level(self) -> Option<u8> {
        let v = self as u8;
        (SourceTok::Heading1 as u8..=SourceTok::Heading6 as u8)
            .contains(&v)
            .then(|| v - SourceTok::Heading1 as u8 + 1)
    }
}

/// One run of a token class: absolute byte range of the document text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceToken {
    pub range: Range<usize>,
    pub tok: SourceTok,
}
