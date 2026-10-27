//! The text shaping seam (ADR 0005 §4.1 "Seam in `markise-layout`", milestone T2).
//!
//! Block layout describes text as a backend-neutral [`StyledText`] (text + style runs with
//! leading space, alignment, wrapping) and asks a [`TextShaper`] for a [`TextBlock`]: an opaque,
//! paintable, shaped and line-broken piece of text that answers every geometric question the
//! layout and (later) the editor need — size, lines, clusters, decoration spans, hit testing,
//! caret rectangles, selection rectangles — without exposing the backend's own types.
//!
//! Implementations:
//! - [`EpaintShaper`] (egui's `FontsView`): wraps epaint galleys. The conversion
//!   [`StyledText`] → `LayoutJob` is exact, so the epaint path renders pixel-identically to the
//!   M1 code that built `LayoutJob`s directly. This is the default and the runtime rollback.
//! - [`ParleyShaper`]: parley + fontique through `markise-text` (behind the `MARKISE_TEXT=parley`
//!   developer switch in the UI; ADR 0005 T3 makes it a real backend).
//!
//! Geometry is in points, relative to the text block's anchor (the `pos` of
//! [`Prim::Text`](crate::Prim::Text)); as with epaint galleys, rows of centred/right-aligned
//! text extend to the left of the anchor (see [`TextBlock::rect`]).
//!
//! Caret positions are [`LocalPos`] (byte offset on a caret stop + affinity), the type
//! `markise-text` uses; document positions (`DocPos`) map to them through the fragment source
//! maps of ADR 0005 §4.3 / ADR 0006 §2.3: every text block of a laid-out item carries an
//! item-relative [`SourceMap`] ([`TextBlock::source_map`], set from
//! [`StyledText::source_map`]) and an item-relative [`FragmentId`].

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use epaint::emath::Align;
use epaint::{Color32, Pos2, Rect, Vec2};
pub use markise_doc::{MapPiece, PieceKind, SourceMap};
pub use markise_text::{Affinity, LocalPos};

pub mod epaint_backend;
pub mod parley_backend;

pub use epaint_backend::{EpaintBlock, EpaintShaper};
pub use parley_backend::{ParleyBlock, ParleyShaper, RoleFamilies};

/// The font role of a run: which of the Markise font families it uses (body/heading/mono,
/// regular/bold). Backends map roles to concrete faces (epaint: the `markise-*` families of
/// [`crate::block::register_families`]; parley: [`RoleFamilies`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontRole {
    Body,
    Bold,
    Heading,
    HeadingBold,
    Mono,
    MonoBold,
}

impl FontRole {
    pub const ALL: [FontRole; 6] = [
        FontRole::Body,
        FontRole::Bold,
        FontRole::Heading,
        FontRole::HeadingBold,
        FontRole::Mono,
        FontRole::MonoBold,
    ];

    /// The bold role of the same family.
    pub fn bold(self) -> FontRole {
        match self {
            FontRole::Body | FontRole::Bold => FontRole::Bold,
            FontRole::Heading | FontRole::HeadingBold => FontRole::HeadingBold,
            FontRole::Mono | FontRole::MonoBold => FontRole::MonoBold,
        }
    }

    /// The monospace role of the same weight.
    pub fn mono(self) -> FontRole {
        match self {
            FontRole::Bold | FontRole::HeadingBold | FontRole::MonoBold => FontRole::MonoBold,
            _ => FontRole::Mono,
        }
    }

    pub fn is_bold(self) -> bool {
        matches!(self, FontRole::Bold | FontRole::HeadingBold | FontRole::MonoBold)
    }

    /// Index into [`FontRole::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The epaint family name of the role.
    pub fn family_name(self) -> &'static str {
        match self {
            FontRole::Body => crate::FAMILY_BODY,
            FontRole::Bold => crate::FAMILY_BOLD,
            FontRole::Heading => crate::FAMILY_HEADING,
            FontRole::HeadingBold => crate::FAMILY_HEADING_BOLD,
            FontRole::Mono => crate::FAMILY_MONO,
            FontRole::MonoBold => crate::FAMILY_MONO_BOLD,
        }
    }
}

/// Style of one run (backend-neutral; colours and alignment use epaint's plain value types,
/// which every primitive of a [`BlockLayout`](crate::BlockLayout) uses).
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Font size in points.
    pub size: f32,
    /// Line height in points.
    pub line_height: f32,
    pub role: FontRole,
    pub italic: bool,
    pub color: Color32,
    /// Background fill behind the glyphs (transparent = none).
    pub background: Color32,
    /// The background grows by this much around the glyphs.
    pub expand_bg: f32,
    /// Backend-drawn underline / strikethrough (1 pt, in the text colour).
    pub underline: bool,
    pub strike: bool,
    /// Vertical alignment of a smaller run in its row (superscript = `TOP`).
    pub valign: Align,
    /// `letter-spacing` in points.
    pub letter_spacing: f32,
}

impl TextStyle {
    pub fn new(size: f32, line_height: f32, role: FontRole, color: Color32) -> Self {
        TextStyle {
            size,
            line_height,
            role,
            italic: false,
            color,
            background: Color32::TRANSPARENT,
            expand_bg: 0.0,
            underline: false,
            strike: false,
            valign: Align::BOTTOM,
            letter_spacing: 0.0,
        }
    }
}

/// One style run of a [`StyledText`].
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    /// Byte range in [`StyledText::text`].
    pub range: Range<usize>,
    /// Extra horizontal space (points) before the run's first character (inline-box padding,
    /// list indentation, justification).
    pub lead: f32,
    pub style: TextStyle,
}

/// Backend-neutral shaping input: text, contiguous style runs and paragraph settings.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledText {
    pub text: String,
    /// Sorted, contiguous runs covering the text (built with [`StyledText::append`]).
    pub runs: Vec<TextRun>,
    /// Horizontal alignment of each row relative to the anchor (`Min`: rows start at x = 0;
    /// `Center`: rows are centred on x = 0; `Max`: rows end at x = 0).
    pub halign: Align,
    /// `\n` starts a new row (true everywhere except rebuilt justified rows).
    pub break_on_newline: bool,
    /// Item-relative source map of `text` (ADR 0006 §2.3); shapers hand it to the
    /// [`TextBlock`] they build ([`TextBlock::source_map`]). Not a shaping input.
    pub source_map: Option<Arc<SourceMap>>,
}

impl Default for StyledText {
    fn default() -> Self {
        StyledText {
            text: String::new(),
            runs: Vec::new(),
            halign: Align::Min,
            break_on_newline: true,
            source_map: None,
        }
    }
}

impl StyledText {
    /// One run for the whole text (also for empty text).
    pub fn single(text: impl Into<String>, style: TextStyle) -> Self {
        let text = text.into();
        StyledText {
            runs: vec![TextRun {
                range: 0..text.len(),
                lead: 0.0,
                style,
            }],
            text,
            ..Default::default()
        }
    }

    /// Appends `s` with `lead` points of space before it. Like epaint's `LayoutJob::append`, a
    /// run with the same style and no lead extends the previous run.
    pub fn append(&mut self, s: &str, lead: f32, style: TextStyle) {
        let start = self.text.len();
        self.text.push_str(s);
        let end = self.text.len();
        if lead == 0.0
            && let Some(last) = self.runs.last_mut()
            && last.style == style
        {
            last.range.end = end;
            return;
        }
        self.runs.push(TextRun {
            range: start..end,
            lead,
            style,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Style of the run containing byte `b` (the last run for `b == text.len()`).
    pub fn style_at(&self, b: usize) -> Option<&TextStyle> {
        self.runs
            .iter()
            .find(|r| r.range.contains(&b))
            .or_else(|| self.runs.last())
            .map(|r| &r.style)
    }
}

/// One visual line (row) of a [`TextBlock`].
#[derive(Clone, Debug, PartialEq)]
pub struct LineInfo {
    /// Byte range of the row's text, without a terminating `\n`.
    pub range: Range<usize>,
    /// Top-left of the row box (anchor-relative).
    pub origin: Pos2,
    /// Row box size (width = advance of the row's content, height = line height).
    pub size: Vec2,
    /// Baseline y (anchor-relative) of the row's tallest run.
    pub baseline: f32,
    /// The row ends with a hard line break.
    pub ends_with_newline: bool,
}

/// One cluster (epaint: one character) in visual order.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    /// Index into [`TextBlock::lines`].
    pub line: usize,
    /// Byte offset of the cluster's first character.
    pub byte: usize,
    /// The cluster's first character.
    pub ch: char,
    /// Left edge (anchor-relative) and advance.
    pub x: f32,
    pub advance: f32,
}

/// The ink extent of a byte range on one row: what text decorations (inline-code boxes,
/// strikethrough, link underlines) are drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkSpan {
    pub x0: f32,
    pub x1: f32,
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Decoration stroke width for this font size.
    pub stroke: f32,
}

/// Which piece of an item a text block shows (ADR 0005 §4.3, ADR 0006 §2.3 / Q10). Ids are
/// item-relative so cached layouts stay valid when text above the item changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FragmentId {
    /// Byte offset of the fragment's source (a table cell's row, a code chunk's first line)
    /// relative to the item's source start; 0 for paragraph text and labels.
    pub block: u32,
    pub kind: FragmentKind,
    /// Byte offset of this block's text inside the fragment's text (non-zero for the rows of a
    /// justified paragraph, which are shaped one by one).
    pub offset: u32,
}

impl FragmentId {
    pub const fn new(kind: FragmentKind) -> Self {
        FragmentId {
            block: 0,
            kind,
            offset: 0,
        }
    }

    pub const fn label() -> Self {
        Self::new(FragmentKind::Label)
    }
}

/// Fragment kinds (ADR 0005 §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FragmentKind {
    /// Paragraph or heading text.
    #[default]
    Para,
    /// Table cell (`row` = source line index of the row in its table, 0 = header).
    Cell { row: u32, col: u32 },
    /// Chunk `n` of a code block, TOC or similar chunked text ([`crate::block::CHUNK_LINES`]).
    CodeChunk(u32),
    /// Text run `n` between block-level inline images of a paragraph.
    Run(u32),
    /// Generated text without source (alert titles, list numbers, code labels, placeholders).
    Label,
}

/// A shaped, line-broken, paintable piece of text (ADR 0005 §4.1). All geometry is in points,
/// relative to the anchor the block is placed at.
pub trait TextBlock: Send + Sync + std::fmt::Debug {
    /// The shaped text.
    fn text(&self) -> &str;
    /// Size of the laid-out text (width of the widest row, total height).
    fn size(&self) -> Vec2;
    /// Bounding rectangle of all rows (may start left of 0 for centred/right-aligned text).
    fn rect(&self) -> Rect;
    /// Number of rows.
    fn line_count(&self) -> usize;
    /// Rows, top to bottom.
    fn lines(&self) -> Vec<LineInfo>;
    /// Height of the first row (the line height of an empty block).
    fn first_line_height(&self) -> Option<f32>;
    /// Offset of the first row's glyphs inside its row box when the line height exceeds the
    /// font height (CSS half-leading; negative when the line height is smaller).
    fn half_leading(&self) -> f32;
    /// Clusters in visual order, row by row.
    fn clusters(&self) -> Vec<Cluster>;
    /// Ink extents, row by row, of the clusters starting inside `range`.
    fn ink_spans(&self, range: &Range<usize>) -> Vec<InkSpan>;
    /// Nearest caret position to `pos`.
    fn hit(&self, pos: Pos2) -> LocalPos;
    /// Byte offset of the character under `pos`, if `pos` is on (or within 2 pt of) the text.
    fn char_at(&self, pos: Pos2) -> Option<usize>;
    /// Caret rectangle (zero width, row height) of a position.
    fn caret_rect(&self, p: LocalPos) -> Rect;
    /// Selection rectangles between two positions, one or more per row.
    fn selection_rects(&self, a: LocalPos, b: LocalPos) -> Vec<Rect>;
    /// Item-relative source map of the block (rendered bytes ↔ item source bytes): `None`
    /// for generated labels.
    fn source_map(&self) -> Option<&SourceMap> {
        None
    }
    /// Estimated heap bytes (layout-cache accounting).
    fn cost(&self) -> usize;
    /// Downcasting for the painter of the matching backend.
    fn as_any(&self) -> &dyn Any;
}

impl<'a> dyn TextBlock + 'a {
    /// The epaint galley behind an [`EpaintBlock`] (tests, the egui painter).
    pub fn galley(&self) -> Option<&Arc<epaint::Galley>> {
        self.as_any().downcast_ref::<EpaintBlock>().map(EpaintBlock::galley)
    }
}

/// Text shaping backend (ADR 0005 §4.1).
pub trait TextShaper {
    /// Shapes `text`, wrapping rows at `width` points (`None` = no wrapping).
    fn shape(&mut self, text: &StyledText, width: Option<f32>) -> Arc<dyn TextBlock>;

    /// Whether the fonts of `style` have a glyph for every character of `s` (generated content
    /// checks).
    fn has_glyphs(&mut self, style: &TextStyle, s: &str) -> bool {
        let _ = (style, s);
        true
    }

    /// Whether `block` (shaped by this backend) shows a non-ASCII character without a glyph
    /// ("install fallback fonts" hint).
    fn lacks_glyphs(&mut self, block: &dyn TextBlock) -> bool {
        let _ = block;
        false
    }

    /// Changes whenever shaping results may change (fonts installed, backend switched); part of
    /// layout cache keys.
    fn generation(&self) -> u64 {
        0
    }
}
