//! Position → block ownership (ADR 0011): which leaf block a source offset belongs to, which
//! part of that block it is in, and the blocks next to it — answered once, from the format's
//! parsed structure, for every layer (planners, the engine's editing helpers, the caret, the
//! toolbar).
//!
//! # The ownership rule
//!
//! A byte offset `at` is **owned** by a leaf when it lies
//!
//! 1. inside the leaf's source: from its first byte to the end of its span or of its last
//!    content line, whichever is later (both ends inclusive) — [`BlockPart::Marker`],
//!    [`BlockPart::Text`], [`BlockPart::LineBreak`], [`BlockPart::TrailingWs`] or
//!    [`BlockPart::Closing`];
//! 2. in the spaces and tabs that end the leaf's last line after that, up to and including the
//!    line end (`## Title··|`: the spaces belong to the heading, spec 02 BE-1) —
//!    [`BlockPart::TrailingWs`] or, after a closing sequence, [`BlockPart::Closing`];
//! 3. in the container prefix on the leaf's first line, before its first byte (`- `, `> `,
//!    indentation) — [`BlockPart::Prefix`].
//!
//! Every other offset is **between blocks** (a line break after a block, blank lines, a
//! container line without a leaf such as an empty quote line `>`): [`FormatDoc::block_owner`]
//! returns `None` there, and callers that need a block pick a side explicitly with
//! [`FormatDoc::block_owner_biased`] (upstream: the block before, downstream: the block after).
//! A position at a block's end before its line break is owned by that block (rule 1).
//!
//! Neighbours ([`FormatDoc::adjacent_block`]) come from the structure, in document order and
//! never past an intervening block: when the next block holds no text leaf at its near edge (an
//! empty quote, an empty list item, a container whose first block is another container
//! without leaves…), the answer is that block with `leaf: None` — and a join or a deletion
//! must stop there.
//!
//! [`FormatDoc::block_owner`]: crate::FormatDoc::block_owner
//! [`FormatDoc::block_owner_biased`]: crate::FormatDoc::block_owner_biased
//! [`FormatDoc::adjacent_block`]: crate::FormatDoc::adjacent_block

use crate::text::Span;
use crate::view::{ContainerKind, LeafKind, LeafView};

/// Which part of its owning leaf an offset is in ([`BlockOwner::part`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockPart {
    /// Container prefix on the leaf's first line, before the leaf's first byte (`- `, `> `).
    Prefix,
    /// The leaf's own syntax before its content (ATX `## `, an opening fence line, front
    /// matter's opening delimiter line).
    Marker,
    /// Inside a content line (both ends inclusive), before the trailing whitespace.
    Text,
    /// Between two content lines: the line break and the next line's container prefix.
    LineBreak,
    /// In the spaces and tabs that end the text of a paragraph or heading
    /// ([`BlockOwner::trailing_ws`], start exclusive, end inclusive).
    TrailingWs,
    /// After the content and its trailing whitespace: closing syntax (an ATX closing sequence,
    /// a setext underline, a closing fence) and the whitespace after it on its line.
    Closing,
    /// Not owned: the offset lies between blocks next to this leaf (only from
    /// [`crate::FormatDoc::block_owner_biased`]).
    Gap,
}

/// A leaf as the ownership query reports it: kind, span and container chain, without its
/// content lines (a query must not copy a block's lines: one paragraph or code block can span
/// the whole document). [`crate::FormatDoc::leaf`] gives the full [`LeafView`] when needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockLeaf {
    pub kind: LeafKind,
    /// From the first byte of the block (after container prefixes) to the end of its content.
    pub span: Span,
    /// Enclosing containers, outermost first.
    pub containers: Vec<ContainerKind>,
    /// `true` when the leaf's first line carries a literal-line hint.
    pub hinted: bool,
}

impl From<&LeafView> for BlockLeaf {
    fn from(l: &LeafView) -> Self {
        BlockLeaf {
            kind: l.kind.clone(),
            span: l.span,
            containers: l.containers.clone(),
            hinted: l.hinted,
        }
    }
}

/// The owning leaf of an offset, with the ranges of its parts (all absolute).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockOwner {
    /// The offset this answer is for.
    pub at: usize,
    pub leaf: BlockLeaf,
    pub part: BlockPart,
    /// The content line holding `at`, else the last one starting before it, else the first
    /// (absolute; the leaf's span start when it has no lines).
    pub line: Span,
    /// Container prefix on the leaf's first line: from that line's start to the leaf's first
    /// byte (empty for a top-level leaf at a line start).
    pub prefix: Span,
    /// The leaf's own syntax before its content: from its first byte to its first content
    /// byte (empty for paragraphs). Can end after the span (an empty ATX heading `## `).
    pub marker: Span,
    /// From the first content byte to the end of the last content line, as the format's lines
    /// give it (a paragraph's last line keeps its trailing spaces).
    pub content: Span,
    /// The spaces and tabs that end the text of a paragraph or heading on its last line,
    /// wherever they are: inside the last content line, after it up to the line end (an open
    /// ATX heading's span stops at its text), or up to a closing sequence. Its start is the
    /// end of the text ([`BlockOwner::text_end`]). Empty for other kinds.
    pub trailing_ws: Span,
    /// After the content and the trailing whitespace: up to the end of the span, extended over
    /// spaces and tabs that end the line.
    pub closing: Span,
}

impl BlockOwner {
    /// [`BlockOwner::resolve_lines`] for a full leaf view.
    pub fn resolve(text: &str, leaf: &LeafView, at: usize) -> Option<BlockOwner> {
        Self::resolve_lines(text, BlockLeaf::from(leaf), &leaf.lines, 0, at)
    }

    /// Resolves the parts of `leaf` for offset `at` by the ownership rule (module docs);
    /// `None` when `at` is not owned by `leaf`. `lines` are the leaf's content lines relative
    /// to `origin` (borrowed from the format's storage, not copied; the cost is a binary
    /// search over them plus the bytes of the offset's line around the leaf's edges).
    pub fn resolve_lines(text: &str, leaf: BlockLeaf, lines: &[Span], origin: usize, at: usize) -> Option<BlockOwner> {
        let b = text.as_bytes();
        if at > b.len() {
            return None;
        }
        let abs = |l: &Span| Span::new(l.start + origin, l.end + origin);
        let span = leaf.span;
        let content = match (lines.first().map(abs), lines.last().map(abs)) {
            (Some(f), Some(l)) => Span::new(f.start, l.end.max(f.start)),
            _ => Span::new(span.start, span.start),
        };
        let marker = Span::new(span.start, content.start.max(span.start));
        let text_like = matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. });
        let last_start = lines.last().map_or(content.start, |l| l.start + origin);
        let trailing_ws = if text_like {
            let mut te = content.end;
            while te > last_start && is_blank(b[te - 1]) {
                te -= 1;
            }
            let mut we = te;
            while we < b.len() && is_blank(b[we]) {
                we += 1;
            }
            // whitespace that ends the line, or stops at the leaf's own closing syntax
            if we == te || at_eol(b, we) || we < span.end {
                Span::new(te, we)
            } else {
                Span::new(te, content.end.max(te))
            }
        } else {
            Span::new(content.end, content.end)
        };
        let body_end = span.end.max(content.end).max(trailing_ws.end);
        let mut tail = body_end;
        while tail < b.len() && is_blank(b[tail]) {
            tail += 1;
        }
        let owned_end = if at_eol(b, tail) { tail } else { body_end };
        let closing = Span::new(content.end.max(trailing_ws.end).min(owned_end), owned_end);
        let line_start = {
            let mut s = span.start.min(b.len());
            while s > 0 && !matches!(b[s - 1], b'\n' | b'\r') {
                s -= 1;
            }
            s
        };
        let prefix = Span::new(line_start, span.start);
        let part = if at < line_start || at > owned_end {
            return None;
        } else if at < span.start {
            BlockPart::Prefix
        } else if at < content.start {
            BlockPart::Marker
        } else if at <= content.end {
            // inside a content line, or between two of them
            let i = lines.partition_point(|l| l.end + origin < at);
            match lines.get(i).map(abs) {
                Some(l) if l.start <= at => {
                    if i + 1 == lines.len() && at > trailing_ws.start && at <= trailing_ws.end {
                        BlockPart::TrailingWs
                    } else {
                        BlockPart::Text
                    }
                }
                Some(_) => BlockPart::LineBreak,
                None => BlockPart::Text, // no lines (a thematic break): its start
            }
        } else if at > trailing_ws.start && at <= trailing_ws.end {
            BlockPart::TrailingWs
        } else {
            BlockPart::Closing
        };
        // the line holding `at`, else the last one starting before it, else the first
        let li = lines.partition_point(|l| l.start + origin <= at);
        let line = lines
            .get(li.saturating_sub(1))
            .map(abs)
            .unwrap_or(Span::new(span.start, span.start));
        Some(BlockOwner {
            at,
            leaf,
            line,
            part,
            prefix,
            marker,
            content,
            trailing_ws,
            closing,
        })
    }

    /// The end of the text: the content end without the trailing whitespace.
    pub fn text_end(&self) -> usize {
        self.trailing_ws.start
    }

    /// Whether the leaf is a paragraph or heading (the text-holding blocks the block-edge
    /// rules are about).
    pub fn is_text_block(&self) -> bool {
        matches!(self.leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. })
    }

    /// Whether the offset lies past the leaf's span and its content lines — in whitespace the
    /// format's span does not cover (after an open ATX heading's text, after a closing
    /// fence…). Such offsets are owned, but format code that works on the span sees them as
    /// outside the block.
    pub fn outside_span(&self) -> bool {
        self.at > self.leaf.span.end.max(self.content.end)
    }

    /// The offset clamped into the leaf's content (`content.start..=content.end`).
    pub fn clamp_to_content(&self) -> usize {
        self.at.clamp(self.content.start, self.content.end)
    }
}

/// A container around an offset, with its absolute source span
/// ([`crate::FormatDoc::containers_at`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerAt {
    pub kind: ContainerKind,
    pub span: Span,
}

/// The block next to an offset in document order ([`crate::FormatDoc::adjacent_block`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adjacent {
    /// Absolute span of the sibling block, at the level where it was found: a leaf, or a whole
    /// container (a list, a quote, an empty list item).
    pub block: Span,
    /// The text-holding leaf at the sibling's near edge (its first leaf going forward, its last
    /// going backward; without its lines: [`crate::FormatDoc::adjacent_leaf`] gives the full
    /// view) when one begins / ends exactly there; `None` for a block without such a
    /// leaf (an empty quote, an empty item, a container that opens with an empty container).
    pub leaf: Option<BlockLeaf>,
}

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t')
}

fn at_eol(b: &[u8], at: usize) -> bool {
    matches!(b.get(at), None | Some(b'\n' | b'\r'))
}

/// Line-local fallback of the ownership query for formats without a block tree
/// ([`crate::FormatDoc::block_owner`]'s default): containment from `leaf`, the trailing
/// whitespace and the container prefix from the offset's own line only.
pub(crate) fn owner_by_lines(text: &str, at: usize, leaf: &dyn Fn(usize) -> Option<LeafView>) -> Option<BlockOwner> {
    let b = text.as_bytes();
    if at > b.len() {
        return None;
    }
    if let Some(l) = leaf(at) {
        return BlockOwner::resolve(text, &l, at);
    }
    // the leaf ending earlier on this line (whitespace after it)
    let mut q = at;
    while q > 0 && is_blank(b[q - 1]) {
        q -= 1;
    }
    if q > 0
        && !matches!(b[q - 1], b'\n' | b'\r')
        && let Some(o) = leaf(q - 1).and_then(|l| BlockOwner::resolve(text, &l, at))
    {
        return Some(o);
    }
    // the leaf starting later on this line (container prefix before it)
    let mut p = at;
    while p < b.len() && !matches!(b[p], b'\n' | b'\r') {
        if let Some(l) = leaf(p) {
            return BlockOwner::resolve(text, &l, at);
        }
        p += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(span: (usize, usize), line: (usize, usize)) -> LeafView {
        LeafView {
            kind: LeafKind::Heading { level: 2 },
            span: Span::new(span.0, span.1),
            lines: vec![Span::new(line.0, line.1)],
            containers: Vec::new(),
            hinted: false,
        }
    }

    #[test]
    fn open_atx_heading_parts() {
        // "## Title··⏎⏎p"
        let t = "## Title  \n\np";
        let h = || heading((0, 8), (3, 8));
        let part = |at| BlockOwner::resolve(t, &h(), at).map(|o| o.part);
        assert_eq!(part(0), Some(BlockPart::Marker));
        assert_eq!(part(2), Some(BlockPart::Marker));
        assert_eq!(part(3), Some(BlockPart::Text));
        assert_eq!(part(8), Some(BlockPart::Text));
        assert_eq!(part(9), Some(BlockPart::TrailingWs));
        assert_eq!(part(10), Some(BlockPart::TrailingWs));
        assert_eq!(part(11), None);
        let o = BlockOwner::resolve(t, &h(), 10).unwrap();
        assert_eq!(o.trailing_ws, Span::new(8, 10));
        assert_eq!(o.text_end(), 8);
        assert!(o.outside_span());
        assert_eq!(o.clamp_to_content(), 8);
        assert_eq!(o.marker, Span::new(0, 3));
        assert_eq!(o.closing, Span::new(10, 10));
    }

    #[test]
    fn closed_atx_heading_parts() {
        // "## Title ##··"
        let t = "## Title ##  \nx";
        let o = |at| BlockOwner::resolve(t, &heading((0, 11), (3, 8)), at);
        assert_eq!(o(9).unwrap().part, BlockPart::TrailingWs);
        assert_eq!(o(9).unwrap().trailing_ws, Span::new(8, 9));
        assert_eq!(o(10).unwrap().part, BlockPart::Closing);
        assert_eq!(o(13).unwrap().part, BlockPart::Closing);
        assert_eq!(o(13).unwrap().closing, Span::new(9, 13));
        assert!(o(14).is_none());
    }

    #[test]
    fn line_fallback_finds_trailing_and_prefix() {
        let t = "* a  \n";
        let item = LeafView {
            kind: LeafKind::Paragraph,
            span: Span::new(2, 3),
            lines: vec![Span::new(2, 3)],
            containers: Vec::new(),
            hinted: false,
        };
        let leaf = |at: usize| (2..=3).contains(&at).then(|| item.clone());
        assert_eq!(owner_by_lines(t, 0, &leaf).map(|o| o.part), Some(BlockPart::Prefix));
        assert_eq!(owner_by_lines(t, 5, &leaf).map(|o| o.part), Some(BlockPart::TrailingWs));
        assert!(owner_by_lines(t, 6, &leaf).is_none());
    }
}
