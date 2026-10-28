//! Neutral block and inline views (ADR 0006 §1.3, §1.4): what layout, caret and commands may
//! know about a document without knowing its format.
//!
//! - [`LeafView`]: one leaf with its container chain and content lines (caret/commands);
//! - [`FlatEvent`]: the block structure as an event stream ([`crate::FormatDoc::flatten`]),
//!   item-relative (each leaf carries its top-level block's origin and relative spans), from
//!   which `markise-layout` builds its items;
//! - [`RichInline`]: the inline tree with marker ranges and **rendered values** (the text a
//!   renderer shows for each leaf node), offsets relative to a caller-chosen base;
//! - [`TableView`], [`HeadingInfo`]: tables and outline contributions;
//! - [`SourceMap`]: rendered text ↔ source pieces of one text fragment (ADR 0006 §2.3).

use std::fmt::Write as _;
use std::ops::Range;

use crate::text::Span;

/// Callout (admonition) kinds; Markdown's GFM alerts map onto the first five.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CalloutKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
    Other(Box<str>),
}

/// A container around a leaf, outermost first in [`LeafView::containers`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    Quote,
    Callout(CalloutKind),
    List { ordered: bool },
    ListItem { ordered: bool, task: Option<bool> },
    Footnote,
}

/// Kind of a leaf block.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LeafKind {
    Paragraph,
    Heading {
        level: u8,
    },
    CodeBlock {
        info: Box<str>,
    },
    Math,
    /// Raw markup of the format (e.g. an HTML block).
    RawBlock,
    ThematicBreak,
    Table {
        columns: u16,
    },
    /// Document metadata (front matter, Org property drawers).
    Metadata,
    Toc,
    /// A reference/link definition (usually hidden).
    Definition,
}

impl LeafKind {
    /// Short neutral name used by debug trees and tests.
    pub fn name(&self) -> String {
        match self {
            LeafKind::Paragraph => "para".into(),
            LeafKind::Heading { level } => format!("h{level}"),
            LeafKind::CodeBlock { info } => format!("code[{info}]"),
            LeafKind::Math => "math".into(),
            LeafKind::RawBlock => "raw".into(),
            LeafKind::ThematicBreak => "hr".into(),
            LeafKind::Table { columns } => format!("table{columns}"),
            LeafKind::Metadata => "meta".into(),
            LeafKind::Toc => "toc".into(),
            LeafKind::Definition => "def".into(),
        }
    }
}

/// A leaf block as seen through the seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafView {
    pub kind: LeafKind,
    /// From the first byte of the block (after container prefixes) to the end of its content.
    pub span: Span,
    /// Content span of each physical line (container prefixes excluded).
    pub lines: Vec<Span>,
    /// Enclosing containers, outermost first.
    pub containers: Vec<ContainerKind>,
    /// `true` when the leaf's first line carries a literal-line hint.
    pub hinted: bool,
}

impl LeafView {
    /// From the first content byte to the end of the last content line (`span.start..` for a
    /// leaf without lines).
    pub fn content(&self) -> Span {
        match (self.lines.first(), self.lines.last()) {
            (Some(f), Some(l)) => Span::new(f.start, l.end.max(f.start)),
            _ => Span::new(self.span.start, self.span.start),
        }
    }
    /// The leaf's own syntax before its content (ATX `## `); it can end after the span (the
    /// space of an empty heading `## `).
    pub fn leading_syntax(&self) -> Span {
        Span::new(self.span.start, self.content().start.max(self.span.start))
    }
    /// The leaf's own syntax after its last content line, inside its span (a setext underline
    /// line, an ATX closing sequence).
    pub fn trailing_syntax(&self) -> Span {
        let last = self.lines.last().map_or(self.span.end, |x| x.end);
        Span::new(last.min(self.span.end), self.span.end)
    }
}

/// Kind of an inline node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlineKind {
    Text,
    Emph,
    Strong,
    Underline,
    Strike,
    Highlight,
    Sub,
    Sup,
    Code,
    Link {
        target: Box<str>,
        title: Option<Box<str>>,
    },
    Image {
        src: Box<str>,
        title: Option<Box<str>>,
    },
    /// Inline math; `display`: display style (e.g. Markdown `$$…$$` inside a paragraph) while
    /// still positioned inline.
    Math {
        display: bool,
    },
    FootnoteRef {
        label: Box<str>,
    },
    RawInline,
    Break {
        hard: bool,
    },
    /// Atomic substitution (entity, escape, emoji…): one caret stop over its source range.
    Atom,
}

/// One inline node. Ranges are byte offsets relative to the request's base
/// ([`InlineSource`]; for [`crate::FormatDoc::inlines`] the leaf's first content byte), so
/// they stay valid when text above the leaf changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineNode {
    pub kind: InlineKind,
    pub source: Range<u32>,
    /// The content between the markers (equal to `source` for marker-less nodes).
    pub content: Range<u32>,
    /// Marker ranges (e.g. the two `**` of strong text).
    pub markers: Vec<Range<u32>>,
    pub children: Vec<InlineNode>,
    /// The rendered value of a leaf node — a range of [`RichInline::text`]: the text of
    /// `Text`, the substitution of an `Atom` (entity, escape, emoji), the normalised content of
    /// `Code`, the TeX of `Math`, the markup of `RawInline`, the alt text of `Image`; empty for
    /// containers and breaks.
    pub text: Range<u32>,
}

impl InlineNode {
    /// A marker-less leaf node.
    pub fn leaf(kind: InlineKind, source: Range<u32>, text: Range<u32>) -> Self {
        InlineNode {
            kind,
            content: source.clone(),
            source,
            markers: Vec::new(),
            children: Vec::new(),
            text,
        }
    }
}

/// The neutral inline tree of one leaf (or cell, or segment).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichInline {
    pub nodes: Vec<InlineNode>,
    /// Pool of rendered values ([`InlineNode::text`]).
    pub text: String,
}

impl RichInline {
    /// The rendered value of `n`.
    #[inline]
    pub fn value(&self, n: &InlineNode) -> &str {
        &self.text[n.text.start as usize..n.text.end as usize]
    }

    /// Plain rendered text of `nodes` (alt texts, outline entries): leaf values concatenated,
    /// breaks as spaces.
    pub fn plain(&self, nodes: &[InlineNode]) -> String {
        fn go(r: &RichInline, nodes: &[InlineNode], out: &mut String) {
            for n in nodes {
                match &n.kind {
                    InlineKind::Break { .. } => out.push(' '),
                    InlineKind::Image { .. } => out.push_str(r.value(n)),
                    _ if n.children.is_empty() => out.push_str(r.value(n)),
                    _ => go(r, &n.children, out),
                }
            }
        }
        let mut out = String::new();
        go(self, nodes, &mut out);
        out
    }

    /// Compact dump: `strong[0..8 m:0..2,6..8](text[2..6])`.
    pub fn debug(&self) -> String {
        fn go(n: &InlineNode, out: &mut String) {
            let name = match &n.kind {
                InlineKind::Link { .. } => "link".to_string(),
                InlineKind::Image { .. } => "image".to_string(),
                InlineKind::FootnoteRef { .. } => "fnref".to_string(),
                InlineKind::Math { .. } => "math".to_string(),
                InlineKind::Break { hard } => if *hard { "hardbreak" } else { "softbreak" }.to_string(),
                k => format!("{k:?}").to_lowercase(),
            };
            let _ = write!(out, "{name}[{}..{}", n.source.start, n.source.end);
            if !n.markers.is_empty() {
                let m: Vec<String> = n.markers.iter().map(|m| format!("{}..{}", m.start, m.end)).collect();
                let _ = write!(out, " m:{}", m.join(","));
            }
            out.push(']');
            if !n.children.is_empty() {
                out.push('(');
                for (i, c) in n.children.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    go(c, out);
                }
                out.push(')');
            }
        }
        let mut out = String::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            go(n, &mut out);
        }
        out
    }
}

/// What [`crate::FormatDoc::inline`] builds an inline tree for.
#[derive(Clone, Copy, Debug)]
pub enum InlineSource<'a> {
    /// Content lines (absolute spans, container prefixes excluded) joined with line breaks: a
    /// whole leaf, or a part of one (a segment of a very large paragraph, a callout body).
    Lines(&'a [Span]),
    /// One table cell (absolute span of its raw content), shown truncated to at most `cap`
    /// bytes (a truncated cell gets a synthetic ` …`).
    Cell { span: Span, cap: usize },
}

// ---- block structure (flatten) -------------------------------------------------------------

/// Marker of a list item.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ListMarker {
    Bullet,
    /// Ordered item: its number and delimiter (`.` / `)`).
    Number {
        n: u64,
        delim: char,
    },
    Task {
        checked: bool,
    },
}

/// A container opened in the [`FlatEvent`] stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlatContainer {
    Quote { callout: Option<CalloutKind> },
    List { ordered: bool, tight: bool },
    ListItem { marker: ListMarker },
    Footnote { label: Box<str> },
}

/// A leaf in the [`FlatEvent`] stream. Spans are relative to `origin` (the origin of the
/// enclosing top-level block): absolute offset = `origin` + relative offset.
#[derive(Clone, Debug)]
pub struct FlatLeaf<'a> {
    pub kind: LeafKind,
    pub origin: usize,
    /// From the first byte of the block (after container prefixes) to the end of its content.
    pub span: Span,
    /// Content span of each physical line (container prefixes excluded).
    pub lines: &'a [Span],
    /// Index of the enclosing top-level block.
    pub top: usize,
}

impl FlatLeaf<'_> {
    /// Absolute span of the leaf.
    #[inline]
    pub fn abs_span(&self) -> Span {
        Span::new(self.origin + self.span.start, self.origin + self.span.end)
    }
    /// Absolute content lines.
    pub fn abs_lines(&self) -> impl ExactSizeIterator<Item = Span> + '_ {
        let o = self.origin;
        self.lines.iter().map(move |l| Span::new(o + l.start, o + l.end))
    }
}

/// One event of [`crate::FormatDoc::flatten`]: the leaves in document order with their
/// container chains as balanced `Open`/`Close` pairs.
#[derive(Clone, Debug)]
pub enum FlatEvent<'a> {
    Open(FlatContainer),
    /// A container whose beginning lies before the emitted region
    /// ([`crate::FormatDoc::flatten_region`]): its first-leaf decorations (list marker, quote
    /// start, footnote label) are already spent.
    Resume(FlatContainer),
    Close,
    Leaf(FlatLeaf<'a>),
    /// The innermost open container (a list item or footnote) has no leaf at all; `span`
    /// (absolute) is the container's own span.
    Empty {
        span: Span,
        top: usize,
    },
}

// ---- tables and outline --------------------------------------------------------------------

/// Column alignment of a table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColumnAlign {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// A table leaf: column alignments and the physical rows (absolute spans; row 0 is the header,
/// row 1 the delimiter row when the format has one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableView {
    pub aligns: Vec<ColumnAlign>,
    pub rows: Vec<Span>,
}

impl TableView {
    pub fn columns(&self) -> usize {
        self.aligns.len().max(1)
    }
}

/// What a heading contributes to the outline and anchors (ADR 0006 §7.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HeadingInfo {
    /// Plain rendered text (untrimmed).
    pub text: String,
    /// Anchor key: the base slug (compared case-insensitively).
    pub key: String,
}

// ---- source maps ---------------------------------------------------------------------------

/// How a piece of rendered text relates to the source (ADR 0006 §2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PieceKind {
    /// Byte-for-byte copy of the source.
    Visible,
    /// A revealed marker (byte-for-byte; only present while revealed).
    Marker,
    /// One atomic substitution (entity, escape, emoji, line break, transformed text): the whole
    /// rendered range maps to the whole source range.
    Atom,
    /// Generated text without source (bullets, decorations, TOC entries); snaps to `source`
    /// (an empty range: the adjacent caret stop).
    Synthetic,
}

/// One piece of a [`SourceMap`]: `text` (bytes of the fragment's rendered text) ↔ `source`
/// (item-relative source bytes).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MapPiece {
    pub text: Range<u32>,
    pub source: Range<u32>,
    pub kind: PieceKind,
}

/// Item-relative source map of one text fragment: sorted, non-overlapping pieces by `text`.
/// Rendered bytes without a piece (e.g. padding) have no source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SourceMap {
    pub pieces: Vec<MapPiece>,
}

impl SourceMap {
    /// Appends a piece, merging it into the previous one when both are `Visible` and
    /// contiguous in text and source.
    pub fn push(&mut self, text: Range<u32>, source: Range<u32>, kind: PieceKind) {
        if text.is_empty() && kind != PieceKind::Synthetic {
            return;
        }
        if kind == PieceKind::Visible
            && let Some(last) = self.pieces.last_mut()
            && last.kind == PieceKind::Visible
            && last.text.end == text.start
            && last.source.end == source.start
        {
            last.text.end = text.end;
            last.source.end = source.end;
            return;
        }
        self.pieces.push(MapPiece { text, source, kind });
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Rendered byte `t` → item-relative source offset (`None` when `t` has no source).
    /// Inside a `Visible`/`Marker` piece byte for byte; inside an `Atom` the source start
    /// (its end for `t` at the piece end); `Synthetic` pieces map to their (empty) source.
    pub fn to_source(&self, t: u32) -> Option<u32> {
        let i = self.pieces.partition_point(|p| p.text.end < t);
        let p = self.pieces.get(i)?;
        if t < p.text.start {
            return None;
        }
        Some(match p.kind {
            PieceKind::Visible | PieceKind::Marker => p.source.start + (t - p.text.start),
            PieceKind::Atom if t >= p.text.end => p.source.end,
            PieceKind::Atom | PieceKind::Synthetic => p.source.start,
        })
    }

    /// Item-relative source offset `s` → rendered byte. A source position inside a hidden
    /// marker or prefix maps to the nearest rendered boundary after it (before it at the end
    /// of the fragment); inside an atom to the atom's start (its end at the atom's end).
    pub fn to_text(&self, s: u32) -> Option<u32> {
        let mut best: Option<u32> = None;
        for p in &self.pieces {
            if p.kind == PieceKind::Synthetic {
                continue;
            }
            if s >= p.source.start && s <= p.source.end {
                return Some(match p.kind {
                    PieceKind::Visible | PieceKind::Marker => p.text.start + (s - p.source.start),
                    _ if s == p.source.end => p.text.end,
                    _ => p.text.start,
                });
            }
            if p.source.start > s {
                return Some(p.text.start);
            }
            best = Some(p.text.end);
        }
        best
    }

    /// The pieces of rendered range `r`, re-based to start at 0 (for a fragment shaped from a
    /// part of a text, e.g. one justified row).
    pub fn slice(&self, r: Range<u32>) -> SourceMap {
        let mut out = SourceMap::default();
        for p in &self.pieces {
            let a = p.text.start.max(r.start);
            let b = p.text.end.min(r.end);
            if a > b || (a == b && !(p.text.is_empty() && p.text.start >= r.start && p.text.start < r.end)) {
                continue;
            }
            let source = match p.kind {
                PieceKind::Visible | PieceKind::Marker => {
                    p.source.start + (a - p.text.start)..p.source.start + (b - p.text.start)
                }
                _ => p.source.clone(),
            };
            out.pieces.push(MapPiece {
                text: a - r.start..b - r.start,
                source,
                kind: p.kind,
            });
        }
        out
    }
}
