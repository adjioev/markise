//! Layout of one [`Item`](crate::Item) into positioned primitives.
//!
//! Text is described backend-neutrally ([`StyledText`]) and shaped through the [`TextShaper`]
//! seam (ADR 0005 T2) into [`TextBlock`]s: epaint galleys by default (egui's `FontsView`), parley
//! behind a developer switch. Coordinates in a [`BlockLayout`] are relative to the item's
//! top-left corner (left edge of the content column).

use std::ops::Range;
use std::sync::Arc;

use epaint::text::{FontData, FontDefinitions, FontFamily};
use epaint::{Color32, Pos2, Rect, Stroke, emath::Align, pos2, vec2};
use markise_doc::{
    CalloutKind, ColumnAlign, FormatDoc, InlineKind, InlineNode, InlineSource, PieceKind, RichInline, SourceMap, Span,
};
use markise_theme::{BoxStyle, Rgba, TextAlign, TextTransform, Theme, TokenKind, TokenRun};

use crate::diagram::DiagramState;
use crate::math::{MATH_SCALE, MathBox, MathQuery, MathState};
use crate::model::{Container, DocModel, Gap, ItemKind, Marker, Segment as BlockSegment};
use crate::text::{FontRole, FragmentId, FragmentKind, StyledText, TextBlock, TextShaper, TextStyle};
use crate::{
    FAMILY_BODY, FAMILY_BOLD, FAMILY_HEADING, FAMILY_HEADING_BOLD, FAMILY_MONO, FAMILY_MONO_BOLD, FAMILY_SIDEBAR,
    FAMILY_SIDEBAR_BOLD,
};

/// Long code blocks and TOCs are split into text blocks of this many lines so painting can
/// skip the off-screen parts of a block taller than the viewport.
pub const CHUNK_LINES: usize = 48;

/// Emoji-presentation characters: egui has no color emoji font, so themes' emoji in generated
/// content (`content: "💡"`) would render as small monochrome outlines.
fn is_emoji(c: char) -> bool {
    // Emoji_Presentation characters (default emoji style); text-style dingbats such as
    // `❝` or `✓` stay drawable
    matches!(c as u32,
        0x1F000..=0x1FAFF
            | 0xFE0F
            | 0x200D
            | 0x231A..=0x231B
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55)
}

/// Whether generated content (`::before`/`::after` text of a theme) can be drawn: every
/// character has a glyph in the font chain and none is an emoji. Undrawable content is
/// skipped (and listed in the theme report) instead of showing tofu or odd fallbacks.
pub fn generated_drawable(text: &str, mut has_glyph: impl FnMut(&str) -> bool) -> bool {
    !text.chars().any(is_emoji) && has_glyph(text)
}

/// The theme's generated content strings that cannot be drawn with the current fonts:
/// (where, text).
pub fn undrawable_generated(theme: &Theme, shaper: &mut dyn TextShaper) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, d) in theme.heading_decor.iter().enumerate() {
        let role = if theme.headings[i].bold {
            FontRole::HeadingBold
        } else {
            FontRole::Heading
        };
        let font = TextStyle::new(theme.headings[i].size, theme.headings[i].size, role, Color32::BLACK);
        for (what, t) in [("::before", &d.prefix), ("::after", &d.suffix)] {
            if let Some(t) = t.as_deref().filter(|t| !t.trim().is_empty())
                && !generated_drawable(t.trim(), |s| shaper.has_glyphs(&font, s))
            {
                out.push((format!("h{}{what}", i + 1), t.to_string()));
            }
        }
    }
    if let Some(t) = theme.quote_prefix.as_deref().filter(|t| !t.trim().is_empty()) {
        let font = TextStyle::new(theme.font_size, theme.font_size, FontRole::Body, Color32::BLACK);
        if !generated_drawable(t.trim(), |s| shaper.has_glyphs(&font, s)) {
            out.push(("blockquote::before".into(), t.to_string()));
        }
    }
    out
}

/// Load state of an image, as far as layout is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageState {
    /// Not decoded yet: a placeholder is laid out.
    Pending,
    /// Decoded; natural size in pixels.
    Ready {
        size: [u32; 2],
    },
    Failed(String),
}

/// Image size provider (the UI owns decoding and textures); also the source of rendered math
/// ([`crate::math`]).
pub trait Images {
    /// `src` is the resolved path or URL.
    fn state(&self, src: &str) -> ImageState;

    /// Render state of a formula; asking queues it for rendering (cheap when cached or queued).
    /// Default: no math renderer (formulas stay styled source).
    fn math(&self, _q: &MathQuery) -> MathState {
        MathState::Unavailable
    }
}

/// Every image pending (tests, headless measurement).
pub struct NoImages;

impl Images for NoImages {
    fn state(&self, _src: &str) -> ImageState {
        ImageState::Pending
    }
}

/// A positioned drawing primitive.
#[derive(Clone, Debug)]
pub enum Prim {
    /// Shaped text placed with its anchor at `pos` (see [`crate::text`]).
    Text {
        pos: Pos2,
        frag: FragmentId,
        text: Arc<dyn TextBlock>,
    },
    Rect {
        rect: Rect,
        fill: Color32,
        stroke: Stroke,
        radius: f32,
    },
    Line {
        a: Pos2,
        b: Pos2,
        stroke: Stroke,
    },
    Circle {
        center: Pos2,
        radius: f32,
        fill: Color32,
        stroke: Stroke,
    },
    Checkbox {
        rect: Rect,
        checked: bool,
        border: Color32,
        fill: Color32,
    },
    /// A polyline or polygon (diagrams). Filled polygons are convex or nearly so (epaint
    /// fills convex paths); `dash` = dash and gap length of the stroke.
    Path {
        points: Vec<Pos2>,
        closed: bool,
        fill: Color32,
        stroke: Stroke,
        dash: Option<[f32; 2]>,
    },
    /// An image; the UI draws the texture when available, otherwise `placeholder`.
    Image {
        rect: Rect,
        src: String,
        placeholder: Color32,
    },
    /// A rendered formula's bitmap (padding included); `key` names it in the renderer's cache,
    /// `query` lets the painter ask for it again after an eviction.
    Math {
        rect: Rect,
        key: u64,
        query: Arc<MathQuery>,
        /// Paint only inside this rectangle (table cells).
        clip: Option<Rect>,
    },
}

/// A clickable link inside a text primitive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkHit {
    /// Index of the [`Prim::Text`] in [`BlockLayout::prims`].
    pub prim: usize,
    /// Byte range in the text block's text.
    pub range: Range<usize>,
    pub target: String,
}

/// Result of laying out one item.
#[derive(Clone, Debug, Default)]
pub struct BlockLayout {
    /// Total height including the gap above the item.
    pub height: f32,
    pub prims: Vec<Prim>,
    pub links: Vec<LinkHit>,
    /// Clickable header of a collapsible block (front matter).
    pub toggle: Option<Rect>,
    /// Task checkboxes (item-local rectangles) and the list item each belongs to, as the
    /// ordinal among the leaf's enclosing list items (0 = outermost): one leaf can draw the
    /// boxes of several items (`- [ ] - [x] a`).
    pub tasks: Vec<(Rect, u16)>,
    /// Caret anchor of an item without rendered text (empty paragraph or heading, ADR 0006
    /// §2.2 "explicit anchors"): the caret rectangle (item-local, zero width) and the
    /// item-relative source offset it stands for.
    pub empty_anchor: Option<(Rect, u32)>,
    /// Caret rectangle (item-local) of the editor-only empty paragraph shown after the item
    /// ([`ItemFocus::virtual_after`], ADR 0006 §2.2).
    pub virtual_caret: Option<Rect>,
}

impl BlockLayout {
    /// Estimated heap bytes held (shaped text dominates), for the cache's memory bound.
    pub fn cost(&self) -> usize {
        let mut b = std::mem::size_of::<Self>() + self.prims.len() * std::mem::size_of::<Prim>();
        for p in &self.prims {
            match p {
                Prim::Text { text, .. } => b += text.cost(),
                Prim::Path { points, .. } => b += points.capacity() * std::mem::size_of::<Pos2>(),
                _ => {}
            }
        }
        for l in &self.links {
            b += 32 + l.target.len();
        }
        b
    }

    /// Target of the link at item-local position `pos`, if any.
    pub fn link_at(&self, pos: Pos2) -> Option<&str> {
        for l in &self.links {
            let Some(Prim::Text { pos: p, text, .. }) = self.prims.get(l.prim) else {
                continue;
            };
            let local = (pos - *p).to_pos2();
            let Some(bi) = text.char_at(local) else {
                continue;
            };
            if l.range.contains(&bi) {
                return Some(&l.target);
            }
        }
        None
    }
}

/// Parameters that, together with the item, fully determine its layout.
#[derive(Clone, Copy, Debug)]
pub struct LayoutParams<'a> {
    pub theme: &'a Theme,
    /// Width of the content column in points.
    pub width: f32,
    /// Collapsible block (front matter) expanded.
    pub expanded: bool,
    /// Highlight runs of a code item (content offsets, see [`crate::highlight`]); `None` =
    /// plain.
    pub code_runs: Option<&'a [TokenRun]>,
    /// Editing state of the focused item (marker reveal, virtual paragraph); `None` for every
    /// other item.
    pub focus: Option<&'a ItemFocus>,
    /// State of a diagram item ([`crate::diagram`]); `None` = shown as source.
    pub diagram: Option<&'a DiagramState>,
    /// Physical pixels per point (rendered math is sized in pixels).
    pub ppp: f32,
    /// Which wikilink targets exist (unresolved links are styled apart).
    pub links: &'a dyn crate::links::LinkTargets,
}

/// Layout input of the item holding the caret (ADR 0006 §2.4): part of that item's cache key
/// only, so caret moves that keep it unchanged cost no layout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ItemFocus {
    /// Item-relative source ranges of the inline nodes whose markers are shown
    /// ([`reveal_set`]).
    pub reveal: Vec<Range<u32>>,
    /// Show the editor-only empty paragraph after the item (the caret is in it).
    pub virtual_after: bool,
    /// Containers around the item the virtual paragraph has left (a list counts as two
    /// levels: list and item; ADR 0006 §2.2, `virt - 1`): it is drawn at their indentation.
    pub virtual_exit: u16,
    /// Show the spaces and tabs from this item-relative source offset on (the end of the
    /// rendered content of the paragraph, heading or table cell holding the caret), which the
    /// collapsed rendering drops: a space just typed at the end of the text needs a caret stop
    /// after it (issue #31). Set only while the caret is inside that whitespace.
    pub trailing_ws: Option<u32>,
    /// The length in bytes of that whitespace. Part of the cache key: after an ATX heading the
    /// whitespace lies outside the item's source span, so the item hash does not change while
    /// it grows (issue #16).
    pub trailing_len: u32,
}

impl ItemFocus {
    pub fn is_empty(&self) -> bool {
        self.reveal.is_empty() && !self.virtual_after && self.trailing_ws.is_none()
    }
}

/// What may follow the trailing whitespace of rendered content ([`trailing_ws_len`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WsEnd {
    /// A paragraph: the end of the text or a line break.
    Line,
    /// A heading: also an ATX closing sequence (`# Title #`).
    Heading,
    /// A table cell: also the cell's closing pipe.
    Cell,
}

/// Length in bytes of the spaces and tabs that start `after` (the source after the rendered
/// content of a paragraph, heading or table cell) when nothing rendered follows them on the
/// line (see [`WsEnd`]); else 0.
pub fn trailing_ws_len(after: &str, end: WsEnd) -> usize {
    let b = after.as_bytes();
    let ws = |from: usize| from + b[from..].iter().take_while(|c| matches!(c, b' ' | b'\t')).count();
    let eol = |at: usize| matches!(b.get(at), None | Some(b'\n' | b'\r'));
    let n = ws(0);
    if n == 0 || eol(n) {
        return n;
    }
    let closed = match end {
        WsEnd::Line => false,
        WsEnd::Cell => b[n] == b'|',
        WsEnd::Heading => {
            let h = n + b[n..].iter().take_while(|&&c| c == b'#').count();
            h > n && eol(ws(h))
        }
    };
    if closed { n } else { 0 }
}

/// The reveal set for a caret at item-relative offset `caret` (ADR 0006 §2.4, spec 02 §3.2):
/// every inline node with markers whose source range contains or touches the caret, at all
/// nesting levels. Images and inline math (rendered objects, one atomic caret stop each) and
/// atoms stay collapsed.
pub fn reveal_set(rich: &RichInline, caret: u32) -> Vec<Range<u32>> {
    fn go(nodes: &[InlineNode], caret: u32, out: &mut Vec<Range<u32>>) {
        for n in nodes {
            if n.source.start > caret {
                break;
            }
            if n.source.end < caret {
                continue;
            }
            // images and formulas render as one object: one atomic caret stop, not revealed
            if !n.markers.is_empty() && !matches!(n.kind, InlineKind::Image { .. } | InlineKind::Math { .. }) {
                out.push(n.source.clone());
            }
            go(&n.children, caret, out);
        }
    }
    let mut out = Vec::new();
    go(&rich.nodes, caret, &mut out);
    out
}

pub fn color(c: Rgba) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Concrete faces for one font role, in fallback order (first = preferred). Names must be
/// unique across the whole [`FontSet`] (they key egui's font data).
#[derive(Clone, Default)]
pub struct RoleFaces {
    pub regular: Vec<(String, Arc<FontData>)>,
    pub bold: Vec<(String, Arc<FontData>)>,
}

/// The faces the layout's font families are built from. Empty roles fall back to egui's
/// bundled fonts; a missing bold face falls back to the regular face (no synthetic bold).
#[derive(Clone, Default)]
pub struct FontSet {
    pub body: RoleFaces,
    /// Headings; `None` = same as `body`.
    pub heading: Option<RoleFaces>,
    pub mono: RoleFaces,
    /// Sidebar items; `None` = egui's UI (proportional) fonts.
    pub sidebar: Option<RoleFaces>,
}

/// Registers the Markise families ([`FAMILY_BODY`], [`FAMILY_BOLD`], [`FAMILY_HEADING`],
/// [`FAMILY_HEADING_BOLD`], [`FAMILY_MONO`], [`FAMILY_MONO_BOLD`]): each role's faces in front of
/// egui's default proportional / monospace fallback chain (which keeps any fallbacks already in
/// `defs`, e.g. a CJK font).
pub fn register_families(defs: &mut FontDefinitions, set: &FontSet) {
    let prop = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mono_default = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut add = |faces: &[(String, Arc<FontData>)], tail: &[String]| -> Vec<String> {
        let mut list = Vec::with_capacity(faces.len() + tail.len());
        for (name, data) in faces {
            // shared, not copied
            defs.font_data.entry(name.clone()).or_insert_with(|| data.clone());
            if !list.contains(name) {
                list.push(name.clone());
            }
        }
        for n in tail {
            if !list.contains(n) {
                list.push(n.clone());
            }
        }
        list
    };
    let body = add(&set.body.regular, &prop);
    let bold = add(&set.body.bold, &body);
    let (heading, heading_bold) = match &set.heading {
        Some(h) => {
            let r = add(&h.regular, &prop);
            let b = add(&h.bold, &r);
            (r, b)
        }
        None => (body.clone(), bold.clone()),
    };
    let mono = add(&set.mono.regular, &mono_default);
    let mono_bold = add(&set.mono.bold, &mono);
    let (side, side_bold) = match &set.sidebar {
        Some(sf) => {
            let r = add(&sf.regular, &prop);
            let b = add(&sf.bold, &r);
            (r, b)
        }
        None => (prop.clone(), prop.clone()),
    };
    for (fam, list) in [
        (FAMILY_BODY, body),
        (FAMILY_BOLD, bold),
        (FAMILY_HEADING, heading),
        (FAMILY_HEADING_BOLD, heading_bold),
        (FAMILY_MONO, mono),
        (FAMILY_MONO_BOLD, mono_bold),
        (FAMILY_SIDEBAR, side),
        (FAMILY_SIDEBAR_BOLD, side_bold),
    ] {
        defs.families.insert(FontFamily::Name(fam.into()), list);
    }
}

#[derive(Clone)]
struct Style {
    size: f32,
    line_height: f32,
    fam: FontRole,
    italic: bool,
    color: Color32,
    bg: Color32,
    underline: bool,
    strike: bool,
    valign: Align,
    /// `letter-spacing` in points.
    letter_spacing: f32,
    /// Background grows by this much around the text (inline code padding).
    expand_bg: f32,
    transform: TextTransform,
}

impl Style {
    fn base(size: f32, line_height: f32, color: Color32) -> Self {
        Style {
            size,
            line_height,
            fam: FontRole::Body,
            italic: false,
            color,
            bg: Color32::TRANSPARENT,
            underline: false,
            strike: false,
            valign: Align::BOTTOM,
            letter_spacing: 0.0,
            expand_bg: 0.0,
            transform: TextTransform::None,
        }
    }

    fn text_style(&self) -> TextStyle {
        TextStyle {
            size: self.size,
            line_height: self.line_height,
            role: self.fam,
            italic: self.italic,
            color: self.color,
            background: self.bg,
            expand_bg: self.expand_bg,
            underline: self.underline,
            strike: self.strike,
            valign: self.valign,
            letter_spacing: self.letter_spacing,
        }
    }
}

/// Link byte ranges of a job and their targets.
type Links = Vec<(Range<usize>, Mark)>;

/// What a byte range of a job carries besides its text format.
#[derive(Clone, Debug)]
enum Mark {
    /// A link target (hit testing; underlined when the theme says so).
    Link(String),
    /// Strikethrough drawn through the glyphs (epaint's own sits at the line-box centre,
    /// far below the glyphs when the line height is large).
    Strike(Color32),
    /// A background box hugging the glyphs (inline code, `mark`): padding, corner radius.
    /// The padding is part of the run's advance (neighbours keep their distance).
    /// `stretch`: spaces inside may be widened by justification (not inside code).
    Box {
        fill: Color32,
        pad: f32,
        radius: f32,
        stretch: bool,
    },
    /// Room reserved for a rendered inline formula: the range is its invisible placeholder
    /// (see [`InlineBuilder::push_math`]); the bitmap is placed over it after shaping.
    Math {
        query: Arc<MathQuery>,
        mbox: MathBox,
        ppp: f32,
    },
}

/// One run of a paragraph: text (with its item-relative source map) or a block-level image.
enum Segment {
    Text(StyledText, Links),
    Image { src: String, alt: String },
}

/// The strut of one rendered inline formula: its byte offset in the job, the text line height
/// and the formula's excess over the text's ascent / descent room (points).
struct Strut {
    at: usize,
    line_height: f32,
    extra_above: f32,
    extra_below: f32,
}

/// Builds styled text from a neutral inline tree ([`RichInline`], ADR 0006 §1.3), recording an
/// item-relative [`SourceMap`] for every text job (ADR 0006 §2.3).
struct InlineBuilder<'a> {
    theme: &'a Theme,
    model: &'a DocModel,
    job: StyledText,
    links: Links,
    /// Source map of `job` (rendered bytes ↔ item-relative source bytes).
    map: SourceMap,
    segments: Vec<Segment>,
    split_images: bool,
    /// Inside a link: emphasis keeps the link color.
    in_link: bool,
    /// Space owed before the next run (right padding of an inline box).
    pending_lead: f32,
    /// Revealed nodes (item-relative source ranges, [`ItemFocus::reveal`]) …
    reveal: &'a [Range<u32>],
    /// … where the shown trailing whitespace starts ([`ItemFocus::trailing_ws`]) …
    trailing: Option<u32>,
    /// … and the item's source (marker text), starting at the item start.
    src: &'a str,
    /// Math renderer and pixels per point (`None`: inline math stays source).
    math: Option<(&'a dyn Images, f32)>,
    /// The job holds an inline formula deeper than the text's descent: its runs are centred
    /// in their rows when the job ends (see [`InlineBuilder::push_math`]).
    center_rows: bool,
    /// Placeholder struts of the current job (sized in [`InlineBuilder::finish`]).
    struts: Vec<Strut>,
    /// Column width in points (see [`InlineBuilder::with_math`]).
    math_width: f32,
    /// Formulas are only measured, never drawn (see [`InlineBuilder::measuring`]).
    measure: bool,
    /// Which wikilink targets exist (`None`: all shown resolved).
    link_targets: Option<&'a dyn crate::links::LinkTargets>,
}

impl<'a> InlineBuilder<'a> {
    fn new(theme: &'a Theme, model: &'a DocModel, split_images: bool) -> Self {
        InlineBuilder {
            theme,
            model,
            job: StyledText::default(),
            links: Vec::new(),
            map: SourceMap::default(),
            segments: Vec::new(),
            split_images,
            in_link: false,
            pending_lead: 0.0,
            reveal: &[],
            trailing: None,
            src: "",
            math: None,
            center_rows: false,
            struts: Vec::new(),
            math_width: f32::INFINITY,
            measure: false,
            link_targets: None,
        }
    }

    /// Styles wikilinks by whether their target exists.
    fn with_links(mut self, l: &'a dyn crate::links::LinkTargets) -> Self {
        self.link_targets = Some(l);
        self
    }

    /// A wikilink (spec 06 §2.2): collapsed, its label as one caret stop over the whole
    /// source; revealed (caret inside or touching), the brackets as markers and the text
    /// between them verbatim. Unresolved targets use a muted link color.
    fn wikilink(
        &mut self,
        r: &RichInline,
        n: &InlineNode,
        target: &str,
        subpath: Option<&str>,
        st: &Style,
        revealed: bool,
    ) {
        let resolved = self.link_targets.is_none_or(|l| l.resolves(target));
        let mut c = st.clone();
        c.color = color(self.theme.link);
        if !resolved {
            c.color = c.color.gamma_multiply(0.55);
        }
        let start = self.job.text.len();
        if revealed {
            self.markers(n, st, true);
            let raw = self
                .src
                .get(n.content.start as usize..n.content.end as usize)
                .unwrap_or("")
                .to_string();
            let mut rs = c.clone();
            rs.transform = TextTransform::None;
            self.push_src(&raw, &rs, &n.content, true);
            self.markers(n, st, false);
        } else {
            let label = n.children.first().map(|ch| r.value(ch)).unwrap_or("").to_string();
            self.push_src(&label, &c, &n.source, false);
        }
        let end = self.job.text.len();
        if end > start {
            let link = match subpath {
                Some(sp) => format!("[[{target}#{sp}]]"),
                None => format!("[[{target}]]"),
            };
            self.links.push((start..end, Mark::Link(link)));
        }
    }

    /// Shows the markers of the nodes in `reveal` (`src`: the item's source from its start).
    fn reveal(mut self, src: &'a str, focus: Option<&'a ItemFocus>) -> Self {
        if let Some(f) = focus {
            self.reveal = &f.reveal;
            self.trailing = f.trailing_ws;
            self.src = src;
        }
        self
    }

    /// Pushes the trailing whitespace after item-relative offset `end` (the end of the rendered
    /// content) when it is shown ([`ItemFocus::trailing_ws`]), byte-for-byte, so the caret has
    /// stops inside and after it.
    fn trailing_ws(&mut self, end: u32, st: &Style) {
        if self.trailing != Some(end) {
            return;
        }
        let end = end as usize;
        let Some(after) = self.src.get(end..) else { return };
        let n = after.bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
        if n > 0 {
            let mut ws = st.clone();
            ws.transform = TextTransform::None;
            self.push_src(&after[..n], &ws, &(end as u32..(end + n) as u32), true);
        }
    }

    /// Pushes the revealed markers of `n` before (`open`) or after its content.
    fn markers(&mut self, n: &InlineNode, st: &Style, open: bool) {
        let mut ms = st.clone();
        ms.color = color(self.theme.meta_text);
        ms.transform = TextTransform::None;
        for m in &n.markers {
            let side = if open {
                m.end <= n.content.start
            } else {
                m.start >= n.content.end
            };
            if !side || m.is_empty() {
                continue;
            }
            let Some(text) = self.src.get(m.start as usize..m.end as usize) else {
                continue;
            };
            let a = self.job.text.len() as u32;
            self.append(text, 0.0, &ms);
            let b = self.job.text.len() as u32;
            self.map.push(a..b, m.clone(), PieceKind::Marker);
        }
    }

    /// Renders inline math through `images` at `ppp` pixels per point.
    /// Natural-width pass: formulas reserve their shown size but render no pixels; the job must
    /// not be painted (its formulas have no bitmaps).
    fn measuring(mut self, on: bool) -> Self {
        self.measure = on;
        self
    }

    /// `width`: the column width in points (formulas are never rendered wider).
    fn with_math(mut self, images: &'a dyn Images, ppp: f32, width: f32) -> Self {
        self.math = Some((images, ppp));
        self.math_width = width.max(1.0);
        self
    }

    /// Reserves room for a rendered inline formula: an invisible no-break-space *strut* whose
    /// line height makes the row tall enough for the formula, then a no-break space of the
    /// text's own style preceded by the formula's width as leading space. No-break spaces offer
    /// no line break between the two, so the leading space never lands at a row start (where it
    /// would be dropped). The bitmap is placed over the pair on the baseline of the second space
    /// (the text baseline) in [`Out::text`].
    ///
    /// Rows are bottom-aligned, so a taller strut only adds room *above* the text. When the
    /// formula is also deeper than the text's descent, the job's runs are centred in their rows
    /// instead ([`InlineBuilder::finish`]) and the strut grows by twice the larger excess: the
    /// row then gains the same room above and below the text. Rows without a strut are not
    /// affected (every glyph there is as tall as the row, so alignment does not move it).
    fn push_math(&mut self, st: &Style, src: &Range<u32>, query: Arc<MathQuery>, mbox: MathBox, ppp: f32) {
        let w = mbox.width / ppp;
        let ascent = mbox.ascent / ppp;
        let descent = mbox.descent / ppp;
        // text descent below the baseline in a row (about a quarter em of the glyphs plus half
        // the line gap) and the room above it
        let below = (st.line_height - st.size).max(0.0) * 0.5 + st.size * 0.25;
        let above = st.line_height - below;
        let extra_above = (ascent + 1.0 - above).max(0.0);
        let extra_below = (descent + 1.0 - below).max(0.0);
        // final strut heights depend on the alignment of the whole job: set in `finish`
        self.center_rows |= extra_below > 0.0;
        let strut_lh = st.line_height + 2.0 * extra_above.max(extra_below);
        let mut strut = st.clone();
        strut.size = 1.0;
        strut.color = Color32::TRANSPARENT;
        strut.bg = Color32::TRANSPARENT;
        strut.underline = false;
        strut.strike = false;
        strut.valign = Align::BOTTOM;
        strut.letter_spacing = 0.0;
        strut.line_height = strut_lh;
        let mut space = strut.clone();
        space.size = st.size;
        space.line_height = st.line_height;
        let a = self.job.text.len() as u32;
        self.append("\u{a0}", 0.0, &strut);
        self.struts.push(Strut {
            at: a as usize,
            line_height: st.line_height,
            extra_above,
            extra_below,
        });
        // the no-break space itself is about a fifth of an em wide
        self.append("\u{a0}", (w - st.size * 0.2).max(0.0), &space);
        let b = self.job.text.len() as u32;
        self.map.push(a..b, src.clone(), PieceKind::Atom);
        self.links
            .push((a as usize..b as usize, Mark::Math { query, mbox, ppp }));
    }

    /// Pushes `s` (styled `st`) as the rendering of source range `src`: `Visible` when the
    /// rendered bytes are the source bytes, else one `Atom`.
    fn push_src(&mut self, s: &str, st: &Style, src: &Range<u32>, verbatim: bool) {
        let a = self.job.text.len() as u32;
        self.push(s, st);
        let b = self.job.text.len() as u32;
        let kind = if verbatim && (b - a) as usize == src.len() {
            PieceKind::Visible
        } else {
            PieceKind::Atom
        };
        self.map.push(a..b, src.clone(), kind);
    }

    /// Pushes generated text without source, snapping to source offset `at`.
    fn push_synthetic(&mut self, s: &str, st: &Style, at: u32) {
        let a = self.job.text.len() as u32;
        self.push(s, st);
        let b = self.job.text.len() as u32;
        if b > a {
            self.map.push(a..b, at..at, PieceKind::Synthetic);
        }
    }

    /// Appends `s` with `lead` points of extra space before it (plus any owed padding).
    fn append(&mut self, s: &str, lead: f32, st: &Style) {
        let lead = lead + std::mem::take(&mut self.pending_lead);
        self.job.append(s, lead, st.text_style());
    }

    /// Pushes a run inside a padded box: the padding is added to the advance on both sides.
    fn push_padded(&mut self, s: &str, st: &Style, pad: f32) {
        if s.is_empty() {
            return;
        }
        self.append(s, pad, st);
        self.pending_lead = pad;
    }

    fn push(&mut self, s: &str, st: &Style) {
        if s.is_empty() {
            return;
        }
        match st.transform {
            TextTransform::None => self.append(s, 0.0, st),
            TextTransform::Uppercase => self.append(&s.to_uppercase(), 0.0, st),
            TextTransform::Lowercase => self.append(&s.to_lowercase(), 0.0, st),
            TextTransform::Capitalize => {
                let mut out = String::with_capacity(s.len());
                // continue a word across runs (`**Bo**ld`)
                let mut start = !self.job.text.chars().next_back().is_some_and(char::is_alphanumeric);
                for c in s.chars() {
                    if start && c.is_alphanumeric() {
                        out.extend(c.to_uppercase());
                    } else {
                        out.push(c);
                    }
                    start = !c.is_alphanumeric();
                }
                self.append(&out, 0.0, st);
            }
        }
    }

    /// Materialises owed right padding of a trailing inline box as an invisible zero-width
    /// character after it, so the padding is part of the measured advance.
    /// Ends the job: owed right padding of a trailing box is dropped (it never leaks into a
    /// later segment); its room is reserved by wrapping narrower (see [`box_reserve`]).
    fn close(&mut self) {
        self.pending_lead = 0.0;
    }

    /// Ends a job: centres its runs in their rows when a deep inline formula asked for it.
    ///
    /// Strut heights follow the job's final alignment (review of PR #15): bottom-aligned rows
    /// only gain room above the text, so a strut adds the formula's excess ascent; centred rows
    /// gain half their extra height on each side, so every strut adds twice the larger excess —
    /// also for a tall but shallow formula when another formula of the job is deep.
    fn finish(&mut self, job: &mut StyledText) {
        let center = std::mem::take(&mut self.center_rows);
        for s in std::mem::take(&mut self.struts) {
            let lh = if center {
                s.line_height + 2.0 * s.extra_above.max(s.extra_below)
            } else {
                s.line_height + s.extra_above
            };
            match job.runs.iter_mut().find(|r| r.range.start == s.at) {
                Some(r) if r.range.end == s.at + '\u{a0}'.len_utf8() => r.style.line_height = lh,
                _ => debug_assert!(false, "math strut merged into another run"),
            }
        }
        if center {
            for r in &mut job.runs {
                r.style.valign = Align::Center;
            }
        }
    }

    /// Takes the finished job (its source map attached) and its marks.
    fn take(&mut self) -> (StyledText, Links) {
        self.close();
        let mut job = std::mem::take(&mut self.job);
        self.finish(&mut job);
        job.source_map = Some(Arc::new(std::mem::take(&mut self.map)));
        (job, std::mem::take(&mut self.links))
    }

    fn flush(&mut self) {
        self.close();
        if !self.job.text.is_empty() {
            let mut job = std::mem::take(&mut self.job);
            self.finish(&mut job);
            job.source_map = Some(Arc::new(std::mem::take(&mut self.map)));
            let links = std::mem::take(&mut self.links);
            self.segments.push(Segment::Text(job, links));
        } else {
            self.map = SourceMap::default();
            self.struts.clear();
            self.center_rows = false;
        }
    }

    fn resolve(&self, dest: &str) -> String {
        // the one resource resolver shared with the HTML and PDF/PNG exports (query/fragment,
        // percent-decoding, `file:` URLs, resource root); non-local references stay as written
        let dir = self.model.base_dir.clone().unwrap_or_default();
        match markise_doc::resource::local_file(&dir, self.model.root_url.as_deref(), dest) {
            Some(l) => l.path.to_string_lossy().into_owned(),
            None => dest.to_string(),
        }
    }

    fn nodes(&mut self, r: &RichInline, nodes: &[InlineNode], st: &Style, top: bool) {
        for n in nodes {
            self.node(r, n, st, top);
        }
    }

    fn node(&mut self, r: &RichInline, n: &InlineNode, st: &Style, top: bool) {
        if let InlineKind::WikiLink { target, subpath } = &n.kind {
            let revealed = !self.reveal.is_empty() && self.reveal.contains(&n.source);
            self.wikilink(r, n, target, subpath.as_deref(), st, revealed);
            return;
        }
        let revealed = !self.reveal.is_empty()
            && !n.markers.is_empty()
            && !matches!(
                n.kind,
                InlineKind::Image { .. } | InlineKind::Math { .. } | InlineKind::Atom | InlineKind::Text
            )
            && !(top
                && self.split_images
                && n.children.len() == 1
                && matches!(n.children[0].kind, InlineKind::Image { .. }))
            && self.reveal.contains(&n.source);
        if revealed {
            self.markers(n, st, true);
        }
        self.node_inner(r, n, st, top);
        if revealed {
            self.markers(n, st, false);
        }
    }

    fn node_inner(&mut self, r: &RichInline, n: &InlineNode, st: &Style, top: bool) {
        let t = self.theme;
        match &n.kind {
            InlineKind::Text => self.push_src(r.value(n), st, &n.source, st.transform == TextTransform::None),
            InlineKind::Atom => self.push_src(r.value(n), st, &n.source, false),
            InlineKind::Code => {
                let c = r.value(n);
                let mut cs = st.clone();
                cs.fam = st.fam.mono();
                cs.size = (st.size * t.mono_scale).round();
                cs.color = color(t.inline_code.text);
                cs.letter_spacing = 0.0;
                cs.transform = TextTransform::None;
                let pad = t.inline_code.padding.clamp(0.0, 6.0);
                let start = self.job.text.len();
                self.push_padded(c, &cs, pad);
                let verbatim = c.len() == n.content.len();
                let (a, b) = (start as u32, self.job.text.len() as u32);
                let (src, kind) = if verbatim {
                    (n.content.clone(), PieceKind::Visible)
                } else {
                    (n.source.clone(), PieceKind::Atom)
                };
                self.map.push(a..b, src, kind);
                self.links.push((
                    start..self.job.text.len(),
                    Mark::Box {
                        fill: color(t.inline_code.background),
                        pad,
                        radius: t.inline_code.radius,
                        stretch: false,
                    },
                ));
            }
            // A23: single line breaks stay visible in the live view.
            InlineKind::Break { .. } => self.push_src("\n", st, &n.source, false),
            InlineKind::Emph => {
                let mut c = st.clone();
                c.italic = true;
                if let Some(col) = t.inline.em
                    && !self.in_link
                {
                    c.color = color(col);
                }
                self.nodes(r, &n.children, &c, false);
            }
            InlineKind::Strong => {
                let mut c = st.clone();
                c.fam = c.fam.bold();
                if let Some(col) = t.inline.strong
                    && !self.in_link
                {
                    c.color = color(col);
                }
                self.nodes(r, &n.children, &c, false);
            }
            InlineKind::Strike => {
                let mut c = st.clone();
                if let Some(col) = t.inline.del {
                    c.color = color(col);
                }
                let start = self.job.text.len();
                self.nodes(r, &n.children, &c, false);
                self.links.push((start..self.job.text.len(), Mark::Strike(c.color)));
            }
            InlineKind::Highlight => {
                let mut c = st.clone();
                if let Some(col) = t.inline.mark_text {
                    c.color = color(col);
                }
                let start = self.job.text.len();
                self.pending_lead += 1.0;
                self.nodes(r, &n.children, &c, false);
                self.pending_lead += 1.0;
                self.links.push((
                    start..self.job.text.len(),
                    Mark::Box {
                        fill: color(t.highlight_bg),
                        pad: 1.0,
                        radius: 2.0,
                        stretch: true,
                    },
                ));
            }
            InlineKind::Sub | InlineKind::Sup => {
                let mut c = st.clone();
                c.size = (st.size * 0.75).round();
                c.valign = if matches!(n.kind, InlineKind::Sup) {
                    Align::TOP
                } else {
                    Align::BOTTOM
                };
                self.nodes(r, &n.children, &c, false);
            }
            InlineKind::Link { target, .. } => {
                // a link wrapping only an image (badge pattern) splits like an image
                if top
                    && self.split_images
                    && n.children.len() == 1
                    && let InlineKind::Image { src: isrc, .. } = &n.children[0].kind
                {
                    let alt = r.value(&n.children[0]).to_string();
                    self.flush();
                    let src = self.resolve(isrc);
                    self.segments.push(Segment::Image { src, alt });
                    return;
                }
                // links are distinguished by color only: egui draws underlines at the bottom
                // of the (1.6×) line box, far below the baseline
                let mut c = st.clone();
                c.color = color(t.link);
                let start = self.job.text.len();
                let was = std::mem::replace(&mut self.in_link, true);
                self.nodes(r, &n.children, &c, false);
                self.in_link = was;
                let end = self.job.text.len();
                if end > start {
                    self.links.push((start..end, Mark::Link(target.to_string())));
                }
            }
            InlineKind::Image { src, .. } => {
                let alt = r.value(n);
                if top && self.split_images {
                    let alt = alt.to_string();
                    self.flush();
                    let src = self.resolve(src);
                    self.segments.push(Segment::Image { src, alt });
                } else {
                    let mut c = st.clone();
                    c.color = color(t.meta_text);
                    let shown = format!("[{}]", if alt.is_empty() { "image" } else { alt });
                    self.push_src(&shown, &c, &n.source, false);
                }
            }
            InlineKind::RawInline => {
                // M1: raw inline HTML is shown as de-emphasised source
                let mut c = st.clone();
                c.color = color(t.meta_text);
                self.push_src(r.value(n), &c, &n.source, st.transform == TextTransform::None);
            }
            InlineKind::Math { display } => {
                let mut failed = false;
                if let Some((images, ppp)) = self.math {
                    let tex = r.value(n);
                    let q = MathQuery {
                        tex: tex.into(),
                        // display style (`$$…$$` in a paragraph), still placed in the text flow
                        display: *display,
                        px_per_em: st.size * MATH_SCALE * ppp,
                        color: st.color,
                        max_width: None,
                        measure: self.measure,
                    };
                    let (query, state) = ask_math(images, q, self.math_width * ppp);
                    match state {
                        MathState::Ready(mbox) => {
                            self.push_math(st, &n.source, query, mbox, ppp);
                            return;
                        }
                        MathState::Empty => {
                            self.push_src("", st, &n.source, false);
                            return;
                        }
                        MathState::Failed(_) => failed = true,
                        MathState::Pending | MathState::Unavailable => {}
                    }
                }
                // not rendered (yet): TeX source in monospace (like the export's C36 placeholder)
                let mut c = st.clone();
                c.fam = st.fam.mono();
                c.size = (st.size * t.mono_scale).round();
                c.color = if failed {
                    color(t.alert[4])
                } else {
                    color(t.math_block.text)
                };
                c.transform = TextTransform::None;
                let v = r.value(n);
                let verbatim = v.len() == n.content.len();
                let a = self.job.text.len() as u32;
                self.push(v, &c);
                let b = self.job.text.len() as u32;
                if verbatim {
                    self.map.push(a..b, n.content.clone(), PieceKind::Visible);
                } else {
                    self.map.push(a..b, n.source.clone(), PieceKind::Atom);
                }
            }
            InlineKind::FootnoteRef { label } => {
                let mut c = st.clone();
                c.size = (st.size * 0.75).round();
                c.valign = Align::TOP;
                c.color = color(t.link);
                self.push_src(&format!("[{label}]"), &c, &n.source, false);
            }
            InlineKind::Underline => self.nodes(r, &n.children, st, false),
            // handled in `node` (it needs the reveal state)
            InlineKind::WikiLink { .. } => {}
        }
    }
}

struct Out {
    prims: Vec<Prim>,
    links: Vec<LinkHit>,
    /// Underline links in this color (`text-decoration: underline` on `a`).
    underline: Option<Color32>,
    /// Column bounds (item-local x): inline boxes are clamped to them, so a box starting a
    /// row (whose left padding epaint drops at the line start) or ending one never paints
    /// outside the column.
    col: (f32, f32),
}

impl Out {
    fn text(&mut self, pos: Pos2, text: Arc<dyn TextBlock>, marks: Links, frag: FragmentId) {
        let mut after = Vec::new();
        for (range, mark) in &marks {
            let spans = text.ink_spans(range);
            match mark {
                Mark::Box { fill, pad, radius, .. } => {
                    for sp in spans {
                        let (c0, c1) = (self.col.0 - pos.x, self.col.1 - pos.x);
                        let rect = Rect::from_min_max(
                            pos2((sp.x0 - pad).max(c0.min(sp.x0)), sp.baseline - sp.ascent - pad * 0.5),
                            pos2((sp.x1 + pad).min(c1.max(sp.x1)), sp.baseline + sp.descent + pad * 0.5),
                        );
                        self.prims.push(Prim::Rect {
                            rect: rect.translate(pos.to_vec2()),
                            fill: *fill,
                            stroke: Stroke::NONE,
                            radius: *radius,
                        });
                    }
                }
                Mark::Strike(c) => {
                    for sp in spans {
                        let y = sp.baseline - sp.ascent * 0.32;
                        after.push(Prim::Line {
                            a: pos + vec2(sp.x0, y),
                            b: pos + vec2(sp.x1, y),
                            stroke: Stroke::new(sp.stroke, *c),
                        });
                    }
                }
                Mark::Link(_) => {
                    if let Some(c) = self.underline {
                        for sp in spans {
                            let y = sp.baseline + (sp.ascent * 0.12).max(1.5);
                            self.prims.push(Prim::Line {
                                a: pos + vec2(sp.x0, y),
                                b: pos + vec2(sp.x1, y),
                                stroke: Stroke::new(sp.stroke, c),
                            });
                        }
                    }
                }
                Mark::Math { query, .. } if query.measure => {}
                Mark::Math { query, mbox, ppp } => {
                    // the placeholder pair never breaks: one span; its last glyph (the
                    // text-style space) carries the text baseline
                    let Some(sp) = spans.last() else { continue };
                    let base = text
                        .ink_spans(&(range.end.saturating_sub(2)..range.end))
                        .last()
                        .map_or(sp.baseline, |s| s.baseline);
                    let w = mbox.width / ppp;
                    let x = (sp.x0 + sp.x1 - w) * 0.5;
                    let size = vec2(mbox.size[0] as f32, mbox.size[1] as f32) / *ppp;
                    let min = pos2(x - mbox.pad / ppp, base - mbox.baseline / ppp);
                    after.push(Prim::Math {
                        rect: Rect::from_min_size(min, size).translate(pos.to_vec2()),
                        key: mbox.key,
                        query: query.clone(),
                        clip: None,
                    });
                }
            }
        }
        let prim = self.prims.len();
        self.prims.push(Prim::Text { pos, frag, text });
        self.prims.extend(after);
        for (range, mark) in marks {
            if let Mark::Link(target) = mark {
                self.links.push(LinkHit { prim, range, target });
            }
        }
    }
}

/// Asks for a formula shown at most `limit_px` wide (its column or table cell). The provider
/// renders it at natural size when it fits and scaled to the column (bucketed) when not, and
/// keeps geometry apart from pixels (`markise_blocks::math::MathService`), so neither a column
/// resize nor cache pressure from full-size bitmaps re-renders formulas that are shown as-is.
fn ask_math(images: &dyn Images, mut q: MathQuery, limit_px: f32) -> (Arc<MathQuery>, MathState) {
    q.max_width = Some(limit_px.max(1.0));
    let state = images.math(&q);
    (Arc::new(q), state)
}

/// Places a rendered display formula centred in the column with half an em above and below;
/// a formula wider than the column is scaled down to fit. Returns the new `y`.
#[allow(clippy::too_many_arguments)]
fn math_display(
    out: &mut Out,
    mbox: &MathBox,
    query: Arc<MathQuery>,
    x: f32,
    y: f32,
    width: f32,
    font_size: f32,
    ppp: f32,
) -> f32 {
    let margin = font_size * 0.5;
    let pad = mbox.pad / ppp;
    let (w, h) = (mbox.size[0] as f32 / ppp, mbox.size[1] as f32 / ppp);
    let s = if w - 2.0 * pad > width && w > 0.0 {
        (width / (w - 2.0 * pad)).min(1.0)
    } else {
        1.0
    };
    let (dw, dh) = (w * s, h * s);
    let min = pos2(x + (width - dw) * 0.5, y + margin);
    out.prims.push(Prim::Math {
        rect: Rect::from_min_size(min, vec2(dw, dh)),
        key: mbox.key,
        query,
        clip: None,
    });
    y + margin + dh + margin
}

/// Horizontal room to keep free at the right edge of a wrapped job so the right padding of an
/// inline box (code, `mark`) at the end of a row stays inside the column: the largest box
/// padding in the job. Wrapping at `width - reserve` keeps the padding attached to its box —
/// it can never wrap to another row on its own, and the painted box cannot overflow.
fn box_reserve(marks: &Links) -> f32 {
    marks
        .iter()
        .filter_map(|(_, m)| match m {
            Mark::Box { pad, .. } => Some(*pad),
            _ => None,
        })
        .fold(0.0, f32::max)
}

/// Largest extra width per space, in multiples of the normal space width: a justified space is
/// at most 1.5× its normal width (owner decision); rows that would need more stay ragged.
pub const JUSTIFY_MAX_EXTRA: f32 = 0.5;

/// Characters justification may widen (word separators), CSS Text §7.
fn is_stretch_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{3000}' | '\u{1361}')
}

/// Justified paragraph text: shaped once with left alignment for line breaking, then each
/// row that should be justified is re-built as its own job whose inter-word spaces carry the
/// extra width (as leading space of the following run). Letters are never spaced apart.
/// Not justified: the last row, rows ending in a hard break, rows with fewer than two
/// stretchable spaces, rows whose per-space expansion would exceed [`JUSTIFY_MAX_EXTRA`] × the
/// normal space width (left ragged instead), and spaces inside code spans. Returns (height, first row
/// height).
fn justify_text(
    out: &mut Out,
    shaper: &mut dyn TextShaper,
    job: StyledText,
    marks: Links,
    at: Pos2,
    width: f32,
    frag: FragmentId,
) -> (f32, f32) {
    let g = shaper.shape(&job, wrapped(width));
    let first_h = g.first_line_height().unwrap_or(0.0);
    let total_h = g.size().y;
    let text = &job.text;
    let no_stretch: Vec<Range<usize>> = marks
        .iter()
        .filter(|(_, m)| matches!(m, Mark::Box { stretch: false, .. } | Mark::Math { .. }))
        .map(|(r, _)| r.clone())
        .collect();
    let lines = g.lines();
    let clusters = g.clusters();
    // per row: (byte range, byte offsets of stretch spaces, extra per space)
    let mut plans: Vec<(Range<usize>, Vec<usize>, f32)> = Vec::new();
    let nrows = lines.len();
    let mut any = false;
    let mut k = 0usize;
    for (ri, line) in lines.iter().enumerate() {
        let start = k;
        while k < clusters.len() && clusters[k].line == ri {
            k += 1;
        }
        let row = &clusters[start..k];
        let mut spaces = Vec::new();
        let mut extra = 0.0;
        let last = ri + 1 == nrows || line.ends_with_newline;
        let first_ink = row.iter().position(|c| !c.ch.is_whitespace());
        let last_ink = row.iter().rposition(|c| !c.ch.is_whitespace());
        if !last && let (Some(fi), Some(li)) = (first_ink, last_ink) {
            // the cap must hold for every participating space: compare with the narrowest
            let mut space_w: f32 = f32::MAX;
            for c in row.iter().take(li).skip(fi + 1) {
                if is_stretch_space(c.ch) && !no_stretch.iter().any(|r| r.contains(&c.byte)) {
                    spaces.push(c.byte);
                    space_w = space_w.min(c.advance);
                }
            }
            // clusters come in visual order: in RTL or mixed rows the offsets are not ascending
            spaces.sort_unstable();
            spaces.dedup();
            let right = row[li].x + row[li].advance;
            let slack = width - right;
            if spaces.len() >= 2 && slack > 0.0 {
                let e = slack / spaces.len() as f32;
                if e <= JUSTIFY_MAX_EXTRA * space_w {
                    extra = e;
                    any = true;
                } else {
                    spaces.clear();
                }
            } else {
                spaces.clear();
            }
        }
        plans.push((line.range.clone(), spaces, extra));
    }
    if !any {
        out.text(at, g, marks, frag);
        return (total_h, first_h);
    }
    for (line, (range, spaces, extra)) in lines.iter().zip(plans) {
        // the row's text, runs clipped to the row; a stretched space gives its extra width
        // to the run that follows it
        let mut rj = StyledText {
            break_on_newline: false,
            source_map: job
                .source_map
                .as_ref()
                .map(|m| Arc::new(m.slice(range.start as u32..(range.start + row_len(text, &range)) as u32))),
            ..StyledText::default()
        };
        let row_text = &text[range.clone()];
        let row_text = row_text.strip_suffix('\n').unwrap_or(row_text);
        let end = range.start + row_text.len();
        let mut cuts: Vec<usize> = spaces
            .iter()
            .map(|&b| b + text[b..].chars().next().map_or(1, char::len_utf8))
            .collect();
        cuts.retain(|&c| c > range.start && c < end);
        cuts.sort_unstable();
        cuts.dedup();
        for run in &job.runs {
            let s0 = run.range.start.max(range.start);
            let s1 = run.range.end.min(end);
            if s0 >= s1 {
                continue;
            }
            let mut pos = s0;
            let mut lead = if run.range.start >= range.start { run.lead } else { 0.0 };
            // a stretched space just before this run: its extra width leads the run
            if cuts.contains(&s0) {
                lead += extra;
            }
            for &c in cuts.iter().filter(|&&c| c > s0 && c < s1) {
                rj.append(&text[pos..c], lead, run.style.clone());
                pos = c;
                lead = extra;
            }
            rj.append(&text[pos..s1], lead, run.style.clone());
        }
        if rj.text.is_empty() {
            continue;
        }
        let row_marks: Links = marks
            .iter()
            .filter_map(|(r, m)| {
                let a = r.start.max(range.start);
                let b = r.end.min(end);
                (a < b).then(|| (a - range.start..b - range.start, m.clone()))
            })
            .collect();
        let rg = shaper.shape(&rj, None);
        let row_frag = FragmentId {
            offset: range.start as u32,
            ..frag
        };
        out.text(at + vec2(line.origin.x, line.origin.y), rg, row_marks, row_frag);
    }
    (total_h, first_h)
}

/// Length of a justified row's text without its trailing `\n`.
fn row_len(text: &str, range: &Range<usize>) -> usize {
    let t = &text[range.clone()];
    t.strip_suffix('\n').unwrap_or(t).len()
}

/// Horizontal offset of a galley anchor for `halign` within `width` (epaint places centered
/// rows around x = 0 and right-aligned rows left of it).
fn align_dx(a: Align, width: f32) -> f32 {
    match a {
        Align::Center => width * 0.5,
        Align::Max => width,
        Align::Min => 0.0,
    }
}

fn halign(a: TextAlign) -> (Align, bool) {
    match a {
        TextAlign::Start => (Align::Min, false),
        TextAlign::Center => (Align::Center, false),
        TextAlign::End => (Align::Max, false),
        TextAlign::Justify => (Align::Min, true),
    }
}

/// Marker text for list item number `n` in CSS `list-style-type` `style`.
fn format_number(n: u64, style: &str) -> Option<String> {
    let alpha = |upper: bool| -> String {
        // bijective base 26: 1 = a, 26 = z, 27 = aa
        let mut n = n.max(1);
        let mut v = Vec::new();
        while n > 0 {
            n -= 1;
            let c = (b'a' + (n % 26) as u8) as char;
            v.push(if upper { c.to_ascii_uppercase() } else { c });
            n /= 26;
        }
        v.iter().rev().collect()
    };
    let roman = |upper: bool| -> String {
        if n == 0 || n >= 4000 {
            return n.to_string();
        }
        let table = [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ];
        let mut n = n;
        let mut out = String::new();
        for (v, s) in table {
            while n >= v {
                out.push_str(s);
                n -= v;
            }
        }
        if upper { out.to_uppercase() } else { out }
    };
    Some(match style {
        "lower-alpha" | "lower-latin" => alpha(false),
        "upper-alpha" | "upper-latin" => alpha(true),
        "lower-roman" => roman(false),
        "upper-roman" => roman(true),
        "decimal-leading-zero" => format!("{n:02}"),
        "decimal" => n.to_string(),
        _ => return None,
    })
}

/// Wrap width for [`TextShaper::shape`] (at least 1 point).
fn wrapped(width: f32) -> Option<f32> {
    Some(width.max(1.0))
}

/// Theme slot of a callout kind (`Theme::alert` order); unknown kinds use the note slot.
fn alert_index(k: &CalloutKind) -> usize {
    match k {
        CalloutKind::Note | CalloutKind::Other(_) => 0,
        CalloutKind::Tip => 1,
        CalloutKind::Important => 2,
        CalloutKind::Warning => 3,
        CalloutKind::Caution => 4,
    }
}

/// Title of a callout ("Note", "Tip", …).
fn callout_title(k: &CalloutKind) -> String {
    let s = match k {
        CalloutKind::Note => "note",
        CalloutKind::Tip => "tip",
        CalloutKind::Important => "important",
        CalloutKind::Warning => "warning",
        CalloutKind::Caution => "caution",
        CalloutKind::Other(s) => s,
    };
    let mut t = s.to_string();
    if let Some(c) = t.get_mut(..1) {
        c.make_ascii_uppercase();
    }
    t
}

/// Lays out item `idx` of `model` (a view model of `doc`). The result is item-relative: it
/// does not depend on where the item is in the document (positions, fragment ids and source
/// maps are relative to the item's start), so it is cached by content.
pub fn layout_item(
    doc: &dyn FormatDoc,
    model: &DocModel,
    idx: usize,
    p: &LayoutParams,
    shaper: &mut dyn TextShaper,
    images: &dyn Images,
) -> BlockLayout {
    let t = p.theme;
    let Some(item) = model.items.get(idx) else {
        return BlockLayout::default();
    };
    if item.kind == ItemKind::Hidden {
        return BlockLayout::default();
    }
    let origin = model.item_start(idx);
    let text = doc.text();
    // content lines of the item (absolute): its segment's pieces, or the leaf's lines
    let lines: Vec<Span> = match &item.kind {
        ItemKind::Empty | ItemKind::Rule | ItemKind::Toc => Vec::new(),
        _ => model.item_lines(doc, idx),
    };

    let gap = match item.gap {
        Gap::None => 0.0,
        Gap::Tight => 3.0,
        Gap::Normal => match item.kind {
            ItemKind::Heading { level } => t.headings[(level as usize - 1).min(5)].margin_top.max(t.block_spacing),
            _ => t.block_spacing,
        },
    };

    // ---- containers: indentation, quote bars, markers
    let mut x = 0.0f32;
    let mut text_color = color(t.text);
    let mut deco: Vec<Prim> = Vec::new();
    // (x, marker) drawn after the content height is known
    let mut markers: Vec<(f32, f32, MarkerKind)> = Vec::new();
    let mut quote_bars: Vec<(f32, Color32, bool)> = Vec::new();
    let mut quote_bgs: Vec<(f32, bool)> = Vec::new();
    let mut alert_title: Option<(usize, String)> = None;
    let mut in_alert_first = false;
    // plain (non-alert) quote styling of the innermost quote
    let mut in_quote = false;
    // first leaf of a quote with generated `::before` text
    let mut quote_prefix_here = false;
    // x before each container and its level count (for the virtual paragraph's indentation)
    let mut level_x: Vec<(f32, u16)> = Vec::new();
    // ordinal of the next list item container (outermost first)
    let mut list_item = 0u16;
    for c in item.containers.iter() {
        level_x.push((x, if matches!(c, Container::ListItem { .. }) { 2 } else { 1 }));
        match c {
            Container::Quote { callout: alert, first } => {
                let bar = match alert {
                    Some(k) => color(t.alert[alert_index(k)]),
                    None => color(t.quote_bar),
                };
                if t.quote_bar_width > 0.0 || alert.is_some() {
                    quote_bars.push((x, bar, *first));
                }
                if alert.is_none() && t.quote_bg.0[3] > 0 {
                    quote_bgs.push((x, *first));
                }
                in_quote = alert.is_none();
                quote_prefix_here = alert.is_none() && *first && t.quote_prefix.is_some();
                if let (Some(k), true) = (alert, *first) {
                    alert_title = Some((alert_index(k), callout_title(k)));
                    in_alert_first = true;
                }
                x += t.quote_indent;
                if alert.is_none() {
                    text_color = color(t.quote_text);
                }
            }
            Container::ListItem { marker, .. } => {
                quote_prefix_here = false;
                if let Some(m) = marker {
                    markers.push((x, t.list_indent, MarkerKind::List(m.clone(), list_item)));
                }
                list_item += 1;
                x += t.list_indent;
            }
            Container::Footnote { label } => {
                quote_prefix_here = false;
                if let Some(l) = label {
                    markers.push((x, t.list_indent * 1.4, MarkerKind::Footnote(l.clone())));
                }
                x += t.list_indent * 1.4;
            }
        }
    }
    let width = (p.width - x).max(40.0);
    let line_px = t.line_px();
    let mut out = Out {
        prims: Vec::new(),
        links: Vec::new(),
        underline: t.link_underline.then(|| color(t.link)),
        col: (x, x + width),
    };
    let mut y = gap;
    let mut toggle = None;
    let mut empty_anchor = None;

    if let Some((ai, title)) = &alert_title {
        let mut st = Style::base(t.font_size, line_px, color(t.alert[*ai]));
        st.fam = FontRole::Bold;
        let g = shaper.shape(&StyledText::single(title.clone(), st.text_style()), wrapped(width));
        let h = g.size().y;
        out.text(pos2(x, y), g, Vec::new(), FragmentId::label());
        y += h + 2.0;
    }

    let content_top = y;
    let mut first_line_h = line_px;
    // whether the first line is text (not a block image): markers centre on its glyphs
    let mut first_is_text = true;
    match &item.kind {
        ItemKind::Paragraph => {
            // a segment of a very large paragraph shows its line pieces: inline markup
            // spanning segment borders is not matched (documented approximation)
            let mut content_lines: &[Span] = &lines;
            if in_alert_first {
                // drop the `[!KIND]` marker line (spec 01 R3.3.4), as the export does
                content_lines = content_lines.get(1..).unwrap_or(&[]);
            }
            // (lines joined with `\n` are empty only for no lines or one empty line)
            let empty = content_lines.is_empty() || (content_lines.len() == 1 && content_lines[0].is_empty());
            if empty && item.segment.is_none() {
                let rel = content_lines.first().map_or(0, |l| l.start.saturating_sub(origin)) as u32;
                empty_anchor = Some((Rect::from_min_size(pos2(x, y), vec2(0.0, line_px)), rel));
            }
            if !empty {
                let rich = doc.inline(InlineSource::Lines(content_lines), origin);
                let mut st = Style::base(t.font_size, line_px, text_color);
                st.italic = in_quote && t.quote_italic;
                let mut b = InlineBuilder::new(t, model, true)
                    .with_math(images, p.ppp, width)
                    .with_links(p.links)
                    .reveal(&text[origin..], p.focus);
                if quote_prefix_here
                    && item.segment.as_ref().is_none_or(BlockSegment::first)
                    && let Some(pre) = &t.quote_prefix
                    && generated_drawable(pre.trim(), |s| shaper.has_glyphs(&st.text_style(), s))
                {
                    b.push_synthetic(pre, &st, 0);
                    if !pre.ends_with(char::is_whitespace) {
                        // CSS themes space the generated marker with a margin
                        b.push_synthetic("\u{2009}", &st, 0);
                    }
                }
                b.nodes(&rich, &rich.nodes, &st, true);
                if let Some(n) = rich.nodes.last() {
                    b.trailing_ws(n.source.end, &st);
                }
                b.flush();
                let (ha, justify) = halign(t.text_align);
                let only_image = b.segments.len() == 1 && matches!(b.segments[0], Segment::Image { .. });
                let multi = b.segments.len() > 1;
                let mut first = true;
                for (si, seg) in b.segments.into_iter().enumerate() {
                    match seg {
                        Segment::Text(mut job, links) => {
                            // whitespace-only runs between images collapse
                            if job.text.trim().is_empty() {
                                continue;
                            }
                            let frag = FragmentId::new(if multi {
                                FragmentKind::Run(si as u32)
                            } else {
                                FragmentKind::Para
                            });
                            let tw = (width - box_reserve(&links)).max(40.0);
                            let (h, first_h) = if justify {
                                justify_text(&mut out, shaper, job, links, pos2(x, y), tw, frag)
                            } else {
                                job.halign = ha;
                                let g = shaper.shape(&job, wrapped(tw));
                                let fh = g.first_line_height().unwrap_or(line_px);
                                let h = g.size().y;
                                out.text(pos2(x + align_dx(ha, tw), y), g, links, frag);
                                (h, fh)
                            };
                            if first {
                                first_line_h = first_h;
                            }
                            y += h;
                        }
                        Segment::Image { src, alt } => {
                            let h = layout_image(&mut out, shaper, images, t, &src, &alt, x, y, width, only_image);
                            if first {
                                first_line_h = h.min(line_px);
                                first_is_text = false;
                            }
                            y += h;
                        }
                    }
                    first = false;
                }
            }
        }
        ItemKind::Heading { level } => {
            let li = (*level as usize - 1).min(5);
            let hs = &t.headings[li];
            let d = &t.heading_decor[li];
            let rich = doc.inline(InlineSource::Lines(&lines), origin);
            let mut st = Style::base(hs.size, hs.size * 1.3, color(hs.color));
            st.fam = if hs.bold {
                FontRole::HeadingBold
            } else {
                FontRole::Heading
            };
            st.italic = d.italic;
            st.letter_spacing = d.letter_spacing;
            st.transform = d.transform;
            let mut b = InlineBuilder::new(t, model, false)
                .with_math(images, p.ppp, width)
                .with_links(p.links)
                .reveal(&text[origin..], p.focus);
            let head_font = st.text_style();
            if let Some(pre) = d
                .prefix
                .as_deref()
                .filter(|p| !p.trim().is_empty() && generated_drawable(p.trim(), |s| shaper.has_glyphs(&head_font, s)))
            {
                let mut ps = st.clone();
                if let Some(c) = d.prefix_color {
                    ps.color = color(c);
                }
                ps.transform = TextTransform::None;
                b.push_synthetic(pre, &ps, 0);
            }
            b.nodes(&rich, &rich.nodes, &st, true);
            if let Some(n) = rich.nodes.last() {
                b.trailing_ws(n.source.end, &st);
            }
            if let Some(suf) = d
                .suffix
                .as_deref()
                .filter(|p| !p.trim().is_empty() && generated_drawable(p.trim(), |s| shaper.has_glyphs(&head_font, s)))
            {
                let mut ss = st.clone();
                ss.transform = TextTransform::None;
                let end = item.len as u32;
                b.push_synthetic(suf, &ss, end);
            }
            let (job, links) = b.take();
            let empty_heading = job.text.is_empty();
            let job = if job.text.is_empty() {
                StyledText::single(" ", st.text_style())
            } else {
                job
            };
            // box: optional left bar, padding, background (full width or hugging the text)
            let bar_w = d.bar.map_or(0.0, |(w, _)| w);
            let [pad_x, pad_y] = [d.padding[0].min(width * 0.25), d.padding[1]];
            let inner_w = (width - bar_w - 2.0 * pad_x - box_reserve(&links)).max(40.0);
            // justification of headings is not applied (single lines in practice)
            let (ha, _) = halign(d.align);
            let mut job = job;
            job.halign = ha;
            let g = shaper.shape(&job, wrapped(inner_w));
            first_line_h = g.first_line_height().unwrap_or(line_px) + pad_y;
            let h = g.size().y;
            let text_x = x + bar_w + pad_x + align_dx(ha, inner_w);
            let box_h = h + 2.0 * pad_y;
            if let Some(bg) = d.background {
                let rect = if d.fit {
                    // the text rect is anchor-relative (negative for centred rows)
                    let gr = g.rect().translate(vec2(text_x, 0.0));
                    Rect::from_min_size(
                        pos2(gr.min.x - pad_x - bar_w, y),
                        vec2(gr.width() + 2.0 * pad_x + bar_w, box_h),
                    )
                } else {
                    Rect::from_min_size(pos2(x, y), vec2(width, box_h))
                };
                out.prims.push(Prim::Rect {
                    rect,
                    fill: color(bg),
                    stroke: Stroke::NONE,
                    radius: d.radius,
                });
            }
            if let Some((w, c)) = d.bar {
                out.prims.push(Prim::Rect {
                    rect: Rect::from_min_size(pos2(x, y), vec2(w, box_h)),
                    fill: color(c),
                    stroke: Stroke::NONE,
                    radius: 0.0,
                });
            }
            if empty_heading {
                let rel = lines.first().map_or(0, |l| l.start.saturating_sub(origin)) as u32;
                empty_anchor = Some((Rect::from_min_size(pos2(text_x, y + pad_y), vec2(0.0, h)), rel));
            }
            out.text(pos2(text_x, y + pad_y), g, links, FragmentId::new(FragmentKind::Para));
            y += box_h;
            if let Some(rule) = hs.rule {
                y += 5.0;
                out.prims.push(Prim::Line {
                    a: pos2(x, y),
                    b: pos2(x + width, y),
                    stroke: Stroke::new(1.0, color(rule)),
                });
                y += 1.0;
            }
            y += hs.margin_bottom;
        }
        ItemKind::Code { lang } => {
            let hl = match (p.code_runs, &item.code) {
                (Some(runs), Some(c)) => Some(CodeHl { runs, offset: c.offset }),
                _ => None,
            };
            y = code_box_hl(
                &mut out,
                shaper,
                t,
                &t.code_block,
                &lines,
                origin,
                item.segment.as_ref(),
                text,
                lang,
                x,
                y,
                width,
                hl,
            );
        }
        ItemKind::Math => {
            // a whole block (not one part of a huge one) is rendered; otherwise, while pending,
            // and for invalid TeX the source box is shown (invalid: in the error color)
            let mut state = MathState::Unavailable;
            let mut query = None;
            if item.segment.is_none() {
                let tex: Vec<&str> = lines.iter().map(|l| l.slice(text)).collect();
                let q = MathQuery {
                    tex: tex.join("\n").into(),
                    display: true,
                    px_per_em: t.font_size * MATH_SCALE * p.ppp,
                    color: text_color,
                    max_width: None,
                    measure: false,
                };
                let (q, st) = ask_math(images, q, width * p.ppp);
                state = st;
                query = Some(q);
            }
            match (state, query) {
                (MathState::Ready(mbox), Some(q)) => {
                    y = math_display(&mut out, &mbox, q, x, y, width, t.font_size, p.ppp);
                }
                (state, _) => {
                    let mut style = t.math_block;
                    if matches!(state, MathState::Failed(_)) {
                        style.text = t.alert[4];
                    }
                    y = code_box(
                        &mut out,
                        shaper,
                        t,
                        &style,
                        &lines,
                        origin,
                        item.segment.as_ref(),
                        text,
                        "math",
                        x,
                        y,
                        width,
                    );
                }
            }
        }
        ItemKind::Diagram { lang } => {
            let first = !matches!(item.segment, Some(BlockSegment::Lines { first: false, .. }));
            if let Some(DiagramState::Done(r)) = p.diagram
                && let Ok(scene) = &**r
            {
                if !first {
                    // the first segment draws the whole scene
                    return BlockLayout::default();
                }
                y = diagram_scene(&mut out, shaper, t, scene, x, y, width);
            } else {
                if let Some(DiagramState::Done(r)) = p.diagram
                    && let Err(e) = &**r
                    && first
                {
                    let msg = e.to_string();
                    // the notice above the source (spec 02 §8: errors shown inline)
                    let size = t.font_size * 0.85;
                    let st = Style::base(size, (size * 1.4).round(), color(t.alert[3]));
                    let g = shaper.shape(&StyledText::single(msg, st.text_style()), wrapped(width));
                    let h = g.size().y;
                    out.text(pos2(x, y), g, Vec::new(), FragmentId::label());
                    y += h + 4.0;
                }
                let label = format!("{lang} diagram");
                y = code_box(
                    &mut out,
                    shaper,
                    t,
                    &t.math_block,
                    &lines,
                    origin,
                    item.segment.as_ref(),
                    text,
                    &label,
                    x,
                    y,
                    width,
                );
            }
        }
        ItemKind::Html => {
            y = code_box(
                &mut out,
                shaper,
                t,
                &t.meta_block,
                &lines,
                origin,
                item.segment.as_ref(),
                text,
                "html",
                x,
                y,
                width,
            );
        }
        ItemKind::Table => {
            y = layout_table(
                &mut out,
                shaper,
                t,
                doc,
                model,
                origin,
                item.segment.as_ref(),
                text_color,
                x,
                y,
                width,
                images,
                p.ppp,
                p.focus,
                p.links,
            );
        }
        ItemKind::Rule => {
            let mid = y + line_px * 0.5;
            out.prims.push(Prim::Line {
                a: pos2(x, mid),
                b: pos2(x + width, mid),
                stroke: Stroke::new(1.5, color(t.rule)),
            });
            y += line_px;
        }
        ItemKind::FrontMatter => {
            let bx = &t.meta_block;
            let n = lines.len();
            let mut st = Style::base(t.font_size * 0.85, t.font_size * 0.85 * 1.5, color(bx.text));
            let header = if p.expanded {
                "Front matter (click to collapse)".to_string()
            } else {
                format!(
                    "Front matter · {n} line{} (click to expand)",
                    if n == 1 { "" } else { "s" }
                )
            };
            let hg = shaper.shape(
                &StyledText::single(header, st.text_style()),
                wrapped(width - 2.0 * bx.padding),
            );
            let mut inner = hg.size().y;
            let mut body = None;
            if p.expanded {
                st.fam = FontRole::Mono;
                st.size = t.mono_size() * 0.95;
                st.line_height = st.size * 1.45;
                let mut body_job = StyledText::single(lines_text(text, &lines), st.text_style());
                body_job.source_map = Some(Arc::new(lines_map(&lines, origin)));
                let g = shaper.shape(&body_job, wrapped(width - 2.0 * bx.padding));
                inner += 4.0 + g.size().y;
                body = Some(g);
            }
            let rect = Rect::from_min_size(pos2(x, y), vec2(width, inner + 2.0 * bx.padding));
            push_box(&mut out, rect, bx);
            toggle = Some(Rect::from_min_size(
                pos2(x, y),
                vec2(width, hg.size().y + 2.0 * bx.padding),
            ));
            let hh = hg.size().y;
            out.text(
                pos2(x + bx.padding, y + bx.padding),
                hg,
                Vec::new(),
                FragmentId::label(),
            );
            if let Some(g) = body {
                let frag = FragmentId::new(FragmentKind::CodeChunk(0));
                out.text(pos2(x + bx.padding, y + bx.padding + hh + 4.0), g, Vec::new(), frag);
            }
            y = rect.max.y;
        }
        ItemKind::Toc => {
            let st = Style::base(t.font_size, line_px, color(t.link));
            let min_level = model.outline.iter().map(|o| o.level).min().unwrap_or(1);
            if model.outline.is_empty() {
                let mut e = st.clone();
                e.color = color(t.meta_text);
                let g = shaper.shape(
                    &StyledText::single("Table of contents (no headings)", e.text_style()),
                    wrapped(width),
                );
                let h = g.size().y;
                out.text(pos2(x, y), g, Vec::new(), FragmentId::label());
                y += h;
            }
            // chunked: off-screen chunks of a long TOC are culled when painting
            let entries = match &item.segment {
                Some(BlockSegment::Toc { start, end, .. }) => {
                    &model.outline[(*start as usize).min(model.outline.len())..(*end as usize).min(model.outline.len())]
                }
                _ => &model.outline[..],
            };
            for (ci, chunk) in entries.chunks(CHUNK_LINES).enumerate() {
                let mut job = StyledText::default();
                let mut links = Vec::new();
                for (i, o) in chunk.iter().enumerate() {
                    if i > 0 {
                        job.append("\n", 0.0, st.text_style());
                    }
                    let start = job.text.len();
                    let indent = (o.level - min_level) as f32 * t.list_indent * 0.75;
                    job.append(if o.text.is_empty() { " " } else { &o.text }, indent, st.text_style());
                    links.push((start..job.text.len(), Mark::Link(format!("#{}", o.anchor))));
                }
                // TOC entries are generated: one synthetic piece snapping to the `[toc]` start
                let mut map = SourceMap::default();
                map.push(0..job.text.len() as u32, 0..0, PieceKind::Synthetic);
                job.source_map = Some(Arc::new(map));
                let g = shaper.shape(&job, wrapped(width));
                let h = g.size().y;
                out.text(
                    pos2(x, y),
                    g,
                    links,
                    FragmentId::new(FragmentKind::CodeChunk(ci as u32)),
                );
                y += h;
            }
        }
        ItemKind::Empty => {
            // an empty list item / footnote: the caret sits at the end of its syntax
            // (after the marker's spacing, ADR 0006 §2.2 explicit anchors)
            let rel = item.len as u32;
            empty_anchor = Some((Rect::from_min_size(pos2(x, y), vec2(0.0, line_px)), rel));
            y += line_px;
        }
        ItemKind::Hidden => {}
    }
    let content_bottom = y;

    // ---- markers aligned with the first line
    // epaint (since 0.37) centres the glyphs in a taller line box, like CSS half-leading:
    // measure that offset on the body style and centre markers on the text, not on the row
    let lead = if markers.is_empty() || !first_is_text {
        0.0
    } else {
        let st = Style::base(t.font_size, line_px, color(t.list_marker));
        let g = shaper.shape(&StyledText::single("x", st.text_style()), None);
        g.half_leading()
    };
    let mut tasks = Vec::new();
    for (mx, mw, m) in markers {
        let cy = content_top + lead + (t.font_size * 0.62).min(first_line_h * 0.5);
        match m {
            MarkerKind::List(Marker::Bullet { depth }, _) => {
                let center = pos2(mx + mw * 0.45, cy);
                let c = color(t.list_marker);
                let styled = (!t.list_styles.bullets.is_empty()).then(|| {
                    let b = &t.list_styles.bullets;
                    b[(depth as usize).min(b.len() - 1)].as_str()
                });
                let shape = match styled {
                    Some("disc") => 0,
                    Some("circle") => 1,
                    Some("square") => 2,
                    Some("none") => 3,
                    _ => depth % 3,
                };
                match shape {
                    3 => {}
                    0 => deco.push(Prim::Circle {
                        center,
                        radius: 2.6,
                        fill: c,
                        stroke: Stroke::NONE,
                    }),
                    1 => deco.push(Prim::Circle {
                        center,
                        radius: 2.6,
                        fill: Color32::TRANSPARENT,
                        stroke: Stroke::new(1.0, c),
                    }),
                    _ => deco.push(Prim::Rect {
                        rect: Rect::from_center_size(center, vec2(4.6, 4.6)),
                        fill: c,
                        stroke: Stroke::NONE,
                        radius: 0.0,
                    }),
                }
            }
            MarkerKind::List(Marker::Task { checked }, li) => {
                let s = (t.font_size * 0.85).round();
                let rect = Rect::from_center_size(
                    pos2(mx + (mw * 0.45).min(mw - s * 0.5 - 5.0).max(s * 0.5), cy),
                    vec2(s, s),
                );
                tasks.push((rect, li));
                deco.push(Prim::Checkbox {
                    rect,
                    checked,
                    border: color(t.checkbox_border),
                    fill: color(t.checkbox_checked),
                });
            }
            MarkerKind::List(Marker::Number(n), _) => {
                let n = match t.list_styles.numbers.first().map(String::as_str) {
                    Some("none") => String::new(),
                    Some(style) => {
                        let digits = n.trim_end_matches(['.', ')']);
                        let suffix = &n[digits.len()..];
                        match digits.parse::<u64>().ok().and_then(|v| format_number(v, style)) {
                            Some(f) => format!("{f}{suffix}"),
                            None => n,
                        }
                    }
                    None => n,
                };
                let st = Style::base(t.font_size, line_px, color(t.list_marker));
                let g = shaper.shape(&StyledText::single(n, st.text_style()), None);
                let w = g.size().x;
                let h = g.size().y;
                let _ = h;
                // same font and line height as the text: share its row top
                deco.push(Prim::Text {
                    pos: pos2(mx + mw - (t.font_size * 0.45).max(6.0) - w, content_top),
                    frag: FragmentId::label(),
                    text: g,
                });
            }
            MarkerKind::Footnote(label) => {
                let st = Style::base(t.font_size * 0.8, t.font_size, color(t.link));
                let g = shaper.shape(&StyledText::single(format!("[^{label}]"), st.text_style()), None);
                let h = g.size().y;
                deco.push(Prim::Text {
                    pos: pos2(mx, cy - h * 0.5),
                    frag: FragmentId::label(),
                    text: g,
                });
            }
        }
    }
    // ---- quote backgrounds (`blockquote` background), continuous like the bars
    for (bx, first) in quote_bgs {
        let top = if first { gap } else { 0.0 };
        deco.insert(
            0,
            Prim::Rect {
                rect: Rect::from_min_max(pos2(bx, top), pos2(p.width, content_bottom)),
                fill: color(t.quote_bg),
                stroke: Stroke::NONE,
                radius: 0.0,
            },
        );
    }
    // ---- quote bars: continuous with the previous item of the same quote
    for (bx, c, first) in quote_bars {
        let top = if first { gap } else { 0.0 };
        deco.push(Prim::Rect {
            rect: Rect::from_min_max(pos2(bx, top), pos2(bx + t.quote_bar_width, content_bottom)),
            fill: c,
            stroke: Stroke::NONE,
            radius: 0.0,
        });
    }

    // decorations first (painted below text); shift link prim indices accordingly
    let shift = deco.len();
    let mut prims = deco;
    prims.extend(out.prims);
    let links = out
        .links
        .into_iter()
        .map(|mut l| {
            l.prim += shift;
            l
        })
        .collect();
    // ---- the editor-only empty paragraph after the item (caret in it)
    let mut height = content_bottom;
    let mut virtual_caret = None;
    if p.focus.is_some_and(|f| f.virtual_after) {
        height += t.block_spacing;
        let mut vx = x;
        let mut left = p.focus.map_or(0, |f| f.virtual_exit);
        for &(xb, n) in level_x.iter().rev() {
            if left < n {
                break;
            }
            left -= n;
            vx = xb;
        }
        virtual_caret = Some(Rect::from_min_size(pos2(vx, height), vec2(0.0, line_px)));
        height += line_px;
    }
    BlockLayout {
        height,
        prims,
        links,
        toggle,
        tasks,
        empty_anchor,
        virtual_caret,
    }
}

enum MarkerKind {
    /// A list item's marker and the item's ordinal among the leaf's list items.
    List(Marker, u16),
    Footnote(String),
}

/// Label metrics of the body font (the faces registered for [`FontRole::Body`] / `Bold`)
/// through `shaper`, for diagram layout on the worker (ADR 0009 §4): per-character advances of
/// printable ASCII, regular and bold, in em.
pub fn label_metrics(shaper: &mut dyn TextShaper) -> markise_blocks::diagram::TableMeasure {
    const S: f32 = 100.0;
    let style = |bold: bool| {
        let mut st = Style::base(S, S * 1.2, Color32::BLACK);
        if bold {
            st.fam = FontRole::Bold;
        }
        st.text_style()
    };
    let (reg, bold) = (style(false), style(true));
    let line = shaper.shape(&StyledText::single("Xg", reg.clone()), None).size().y / S;
    let mut width = |s: &str, b: bool| {
        shaper
            .shape(&StyledText::single(s, if b { bold.clone() } else { reg.clone() }), None)
            .size()
            .x
    };
    let mut space = [0.0f32; 2];
    for (k, b) in [false, true].into_iter().enumerate() {
        // a lone space may be trimmed by the shaper: measure it between two glyphs
        space[k] = ((width("x x", b) - width("xx", b)) / S).max(0.1);
    }
    let mut cache: std::collections::HashMap<(char, bool), f32> = std::collections::HashMap::new();
    for b in [false, true] {
        for c in (33u8..127).map(char::from) {
            let w = width(c.encode_utf8(&mut [0; 4]), b) / S;
            cache.insert((c, b), w);
        }
    }
    markise_blocks::diagram::TableMeasure::from_fn(
        |c, b| {
            if c == ' ' {
                space[b as usize]
            } else {
                cache.get(&(c, b)).copied().unwrap_or(0.6)
            }
        },
        line,
    )
}

/// Flattens a cubic Bézier into `out` (excluding the start point).
fn flatten_cubic(out: &mut Vec<Pos2>, p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2) {
    let len = p0.distance(p1) + p1.distance(p2) + p2.distance(p3);
    let n = ((len / 6.0).ceil() as usize).clamp(2, 32);
    for k in 1..=n {
        let t = k as f32 / n as f32;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push(pos2(
            a * p0.x + b * p1.x + c * p2.x + d * p3.x,
            a * p0.y + b * p1.y + c * p2.y + d * p3.y,
        ));
    }
}

/// Paints a diagram [`Scene`](markise_blocks::diagram::Scene) at `(x, y)`, scaled down to
/// `width` if wider and centered; colors from the theme's color scheme, labels shaped with the
/// layout's text stack (and shrunk if the font is wider than the metrics the scene was laid out
/// with, so they stay inside their shapes). Returns the bottom.
fn diagram_scene(
    out: &mut Out,
    shaper: &mut dyn TextShaper,
    t: &Theme,
    scene: &markise_blocks::diagram::Scene,
    x: f32,
    y: f32,
    width: f32,
) -> f32 {
    use markise_blocks::diagram::{Anchor, DiagramPalette, Item as DItem, Paint, Seg};
    let pal = DiagramPalette::from_theme(t);
    let col = |p: Paint| color(pal.resolve(p));
    let s = if scene.width > width && scene.width > 0.0 {
        width / scene.width
    } else {
        1.0
    };
    let x0 = x + ((width - scene.width * s) / 2.0).max(0.0);
    let pt = |p: [f32; 2]| pos2(x0 + p[0] * s, y + p[1] * s);
    let stroke_of = |st: Option<markise_blocks::diagram::Stroke>| match st {
        Some(st) => (
            Stroke::new((st.width * s).max(0.6), col(st.paint)),
            st.dash.map(|[d, g]| [d * s, g * s]),
        ),
        None => (Stroke::NONE, None),
    };
    for it in &scene.items {
        match it {
            DItem::Rect {
                rect,
                radius,
                fill,
                stroke,
            } => {
                let (st, dash) = stroke_of(*stroke);
                let r = Rect::from_min_size(pt([rect[0], rect[1]]), vec2(rect[2] * s, rect[3] * s));
                if dash.is_some() {
                    let pts = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
                    out.prims.push(Prim::Path {
                        points: pts,
                        closed: true,
                        fill: fill.map_or(Color32::TRANSPARENT, col),
                        stroke: st,
                        dash,
                    });
                } else {
                    out.prims.push(Prim::Rect {
                        rect: r,
                        fill: fill.map_or(Color32::TRANSPARENT, col),
                        stroke: st,
                        radius: radius * s,
                    });
                }
            }
            DItem::Ellipse {
                center,
                radii,
                fill,
                stroke,
            } => {
                let (st, dash) = stroke_of(*stroke);
                let c = pt(*center);
                let (rx, ry) = (radii[0] * s, radii[1] * s);
                let n = ((rx.max(ry) * 0.8) as usize).clamp(16, 64);
                let pts = (0..n)
                    .map(|k| {
                        let a = k as f32 / n as f32 * std::f32::consts::TAU;
                        pos2(c.x + rx * a.cos(), c.y + ry * a.sin())
                    })
                    .collect();
                out.prims.push(Prim::Path {
                    points: pts,
                    closed: true,
                    fill: fill.map_or(Color32::TRANSPARENT, col),
                    stroke: st,
                    dash,
                });
            }
            DItem::Path { segs, fill, stroke } => {
                let (st, dash) = stroke_of(*stroke);
                let fill = fill.map_or(Color32::TRANSPARENT, col);
                let mut pts: Vec<Pos2> = Vec::new();
                let mut closed = false;
                let flush = |pts: &mut Vec<Pos2>, closed: bool, out: &mut Out| {
                    if pts.len() >= 2 {
                        out.prims.push(Prim::Path {
                            points: std::mem::take(pts),
                            closed,
                            fill,
                            stroke: st,
                            dash,
                        });
                    } else {
                        pts.clear();
                    }
                };
                for sg in segs {
                    match *sg {
                        Seg::MoveTo(p) => {
                            flush(&mut pts, closed, out);
                            closed = false;
                            pts.push(pt(p));
                        }
                        Seg::LineTo(p) => pts.push(pt(p)),
                        Seg::CubicTo(a, b, c) => {
                            let p0 = pts.last().copied().unwrap_or(pt(a));
                            flatten_cubic(&mut pts, p0, pt(a), pt(b), pt(c));
                        }
                        Seg::Close => closed = true,
                    }
                }
                flush(&mut pts, closed, out);
            }
            DItem::Text {
                pos,
                text,
                size,
                width,
                anchor,
                bold,
                paint,
            } => {
                let size_s = size * s;
                let mut st = Style::base(size_s, (size_s * 1.2).round().max(1.0), col(*paint));
                if *bold {
                    st.fam = FontRole::Bold;
                }
                let mut g = shaper.shape(&StyledText::single(text.as_str(), st.text_style()), None);
                let planned = width * s;
                let w = g.size().x;
                if w > planned * 1.03 && planned > 0.0 {
                    let k = (planned / w).max(0.7);
                    st.size = size_s * k;
                    st.line_height = (st.size * 1.2).round().max(1.0);
                    g = shaper.shape(&StyledText::single(text.as_str(), st.text_style()), None);
                }
                let gs = g.size();
                let p = pt(*pos);
                let gx = match anchor {
                    Anchor::Start => p.x,
                    Anchor::Middle => p.x - gs.x / 2.0,
                    Anchor::End => p.x - gs.x,
                };
                out.text(pos2(gx, p.y - gs.y / 2.0), g, Vec::new(), FragmentId::label());
            }
        }
    }
    y + scene.height * s
}

fn push_box(out: &mut Out, rect: Rect, bx: &BoxStyle) {
    out.prims.push(Prim::Rect {
        rect,
        fill: color(bx.background),
        stroke: bx.border.map_or(Stroke::NONE, |b| Stroke::new(1.0, color(b))),
        radius: bx.radius,
    });
}

/// Highlight input of one code item: runs with content offsets and the content offset of the
/// item's first line piece.
#[derive(Clone, Copy)]
struct CodeHl<'a> {
    runs: &'a [TokenRun],
    offset: usize,
}

#[allow(clippy::too_many_arguments)]
fn code_box(
    out: &mut Out,
    shaper: &mut dyn TextShaper,
    t: &Theme,
    bx: &BoxStyle,
    lines: &[Span],
    origin: usize,
    seg: Option<&BlockSegment>,
    text: &str,
    label: &str,
    x: f32,
    y: f32,
    width: f32,
) -> f32 {
    code_box_hl(out, shaper, t, bx, lines, origin, seg, text, label, x, y, width, None)
}

/// `lines` (absolute) joined with `\n`.
fn lines_text(text: &str, lines: &[Span]) -> String {
    lines.iter().map(|l| l.slice(text)).collect::<Vec<_>>().join("\n")
}

/// Source map of `lines` joined with `\n`: each line `Visible`, each `\n` an `Atom` over the
/// line break and container prefix between two lines (item-relative to `origin`).
fn lines_map(lines: &[Span], origin: usize) -> SourceMap {
    let mut map = SourceMap::default();
    let mut pos = 0u32;
    for (k, l) in lines.iter().enumerate() {
        let (a, b) = ((l.start - origin) as u32, (l.end - origin) as u32);
        if k > 0 {
            let prev = (lines[k - 1].end - origin) as u32;
            map.push(pos..pos + 1, prev..a.max(prev), PieceKind::Atom);
            pos += 1;
        }
        map.push(pos..pos + (b - a), a..b, PieceKind::Visible);
        pos += b - a;
    }
    map
}

/// Appends `s` (content offsets `at..at + s.len()`) to `job`, split into sections by the runs
/// that intersect it. `next` is the index of the first run that may intersect (runs sorted).
fn append_highlighted(
    job: &mut StyledText,
    s: &str,
    at: usize,
    runs: &[TokenRun],
    next: &mut usize,
    plain: &TextStyle,
    looks: &[TextStyle],
) {
    let end = at + s.len();
    while *next < runs.len() && (runs[*next].end as usize) <= at {
        *next += 1;
    }
    let mut pos = at;
    let mut k = *next;
    while k < runs.len() && (runs[k].start as usize) < end {
        let r = runs[k];
        let (rs, re) = ((r.start as usize).max(pos), (r.end as usize).min(end));
        // runs come from another component: validate before slicing (malformed runs, or runs
        // splitting a character, are drawn plain)
        if re > rs && s.is_char_boundary(rs - at) && s.is_char_boundary(re - at) {
            if rs > pos {
                job.append(&s[pos - at..rs - at], 0.0, plain.clone());
            }
            let look = looks.get(r.kind as usize).unwrap_or(plain);
            job.append(&s[rs - at..re - at], 0.0, look.clone());
            pos = re;
        }
        if (r.end as usize) > end {
            break; // continues into the next piece
        }
        k += 1;
    }
    *next = k;
    if pos < end {
        job.append(&s[pos - at..], 0.0, plain.clone());
    }
}

#[allow(clippy::too_many_arguments)]
fn code_box_hl(
    out: &mut Out,
    shaper: &mut dyn TextShaper,
    t: &Theme,
    bx: &BoxStyle,
    block_lines: &[Span],
    origin: usize,
    seg: Option<&BlockSegment>,
    text: &str,
    label: &str,
    x: f32,
    y: f32,
    width: f32,
    hl: Option<CodeHl>,
) -> f32 {
    let size = t.mono_size();
    let mut st = Style::base(size, (size * 1.5).round(), color(bx.text));
    st.fam = FontRole::Mono;
    // chunks of CHUNK_LINES source lines: long code blocks are culled per chunk when painting
    let inner_w = width - 2.0 * bx.padding;
    let mut blocks = Vec::new();
    let mut h = 0.0;
    // a segment of a large block draws only its lines; padding/label at the block's ends
    let (lines, first, last): (&[Span], bool, bool) = match seg {
        Some(BlockSegment::Lines { first, last, .. }) => (block_lines, *first, *last),
        _ => (block_lines, true, true),
    };
    let pad_top = if first { bx.padding } else { 0.0 };
    let pad_bottom = if last { bx.padding } else { 0.0 };
    // token formats, indexed by `TokenKind as usize`
    let plain = st.text_style();
    let looks: Vec<TextStyle> = match hl {
        Some(_) => TokenKind::ALL
            .iter()
            .map(|&k| {
                let look = t.token_look(k);
                let mut ts = st.clone();
                if let Some(c) = look.color {
                    ts.color = color(c);
                }
                if look.bold {
                    ts.fam = FontRole::MonoBold;
                }
                ts.italic = look.italic;
                ts.text_style()
            })
            .collect(),
        None => Vec::new(),
    };
    // content offset of each line piece (a piece continued by the next one has no newline)
    let mut at = hl.map_or(0, |h| h.offset);
    let mut next_run = 0;
    let chunks: Vec<&[Span]> = lines.chunks(CHUNK_LINES).collect();
    if chunks.is_empty() {
        let g = shaper.shape(&StyledText::single(" ", plain.clone()), wrapped(inner_w));
        h += g.size().y;
        blocks.push((0.0, g));
    }
    for (ci, chunk) in chunks.iter().enumerate() {
        let mut job = match hl {
            None => {
                let mut s = chunk.iter().map(|l| l.slice(text)).collect::<Vec<_>>().join("\n");
                if s.is_empty() {
                    s.push(' ');
                }
                StyledText::single(s, plain.clone())
            }
            Some(hl) => {
                let mut job = StyledText::default();
                for (k, l) in chunk.iter().enumerate() {
                    if k > 0 {
                        job.append("\n", 0.0, plain.clone());
                    }
                    append_highlighted(&mut job, l.slice(text), at, hl.runs, &mut next_run, &plain, &looks);
                    at += l.len();
                    let following = chunk.get(k + 1).or_else(|| chunks.get(ci + 1).and_then(|c| c.first()));
                    if following.is_none_or(|n| n.start != l.end) {
                        at += 1;
                    }
                }
                if job.text.is_empty() {
                    job.append(" ", 0.0, plain.clone());
                }
                job
            }
        };
        job.source_map = Some(Arc::new(lines_map(chunk, origin)));
        let g = shaper.shape(&job, wrapped(inner_w));
        let gh = g.size().y;
        blocks.push((h, g));
        h += gh;
    }
    let mut label_g = None;
    if !label.is_empty() && first {
        let ls = Style::base(size * 0.8, size, color(t.meta_text));
        label_g = Some(shaper.shape(&StyledText::single(label, ls.text_style()), None));
    }
    let rect = Rect::from_min_size(pos2(x, y), vec2(width, h + pad_top + pad_bottom));
    if seg.is_some() {
        // square corners so consecutive segments join into one box
        out.prims.push(Prim::Rect {
            rect,
            fill: color(bx.background),
            stroke: Stroke::NONE,
            radius: 0.0,
        });
    } else {
        push_box(out, rect, bx);
    }
    for (ci, (dy, g)) in blocks.into_iter().enumerate() {
        let mut frag = FragmentId::new(FragmentKind::CodeChunk(ci as u32));
        if let Some(first) = chunks.get(ci).and_then(|c| c.first()) {
            frag.block = (first.start - origin) as u32;
        }
        out.text(pos2(x + bx.padding, y + pad_top + dy), g, Vec::new(), frag);
    }
    if let Some(lg) = label_g {
        let w = lg.size().x;
        out.text(
            pos2(rect.max.x - w - 6.0, rect.min.y + 3.0),
            lg,
            Vec::new(),
            FragmentId::label(),
        );
    }
    rect.max.y
}

#[allow(clippy::too_many_arguments)]
fn layout_image(
    out: &mut Out,
    shaper: &mut dyn TextShaper,
    images: &dyn Images,
    t: &Theme,
    src: &str,
    alt: &str,
    x: f32,
    y: f32,
    width: f32,
    center: bool,
) -> f32 {
    let ph = color(t.image_placeholder);
    match images.state(src) {
        ImageState::Ready { size: [w, h] } => {
            let (w, h) = (w.max(1) as f32, h.max(1) as f32);
            let scale = (width / w).min(1.0);
            let size = vec2(w * scale, h * scale);
            let left = if center { x + (width - size.x) * 0.5 } else { x };
            out.prims.push(Prim::Image {
                rect: Rect::from_min_size(pos2(left, y + 4.0), size),
                src: src.to_string(),
                placeholder: ph,
            });
            size.y + 8.0
        }
        state => {
            let failed = matches!(state, ImageState::Failed(_));
            let label = match &state {
                ImageState::Failed(why) => format!("⚠ {} — {why}", if alt.is_empty() { src } else { alt }),
                _ => format!("Loading image… {}", if alt.is_empty() { "" } else { alt }),
            };
            let h = if failed { t.line_px() * 1.8 } else { 120.0 };
            let w = if failed { width } else { width.min(320.0) };
            let left = if center { x + (width - w) * 0.5 } else { x };
            let rect = Rect::from_min_size(pos2(left, y + 4.0), vec2(w, h));
            out.prims.push(Prim::Image {
                rect,
                src: src.to_string(),
                placeholder: ph,
            });
            let st = Style::base(t.font_size * 0.85, t.font_size * 1.2, color(t.meta_text));
            let g = shaper.shape(&StyledText::single(label, st.text_style()), wrapped(w - 16.0));
            let gs = g.size();
            out.text(rect.center() - gs * 0.5, g, Vec::new(), FragmentId::label());
            h + 8.0
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_table(
    out: &mut Out,
    shaper: &mut dyn TextShaper,
    t: &Theme,
    doc: &dyn FormatDoc,
    model: &DocModel,
    origin: usize,
    seg: Option<&BlockSegment>,
    text_color: Color32,
    x: f32,
    y: f32,
    width: f32,
    images: &dyn Images,
    ppp: f32,
    focus: Option<&ItemFocus>,
    links: &dyn crate::links::LinkTargets,
) -> f32 {
    let src = doc.text().get(origin..).unwrap_or("");
    let Some(table) = doc.table(origin) else {
        return y;
    };
    let aligns = &table.aligns;
    let ncols = aligns.len().max(1);
    let pad = t.table_cell_padding;
    let line_px = t.line_px();
    let nlines = table.rows.len();
    // rows drawn by this item (line indices; 0 = header, 1 = delimiter row, 2.. = body)
    let (body, first, last) = match seg {
        Some(BlockSegment::Rows {
            start,
            end,
            first,
            last,
            ..
        }) => ((*start as usize).max(2)..(*end as usize).min(nlines), *first, *last),
        _ => (2.min(nlines)..nlines, true, true),
    };
    let sample_end = nlines;
    // `math_w`: per column, the widest a formula of that cell may be rendered (points)
    // `measure`: natural-width pass (formulas measured, not rendered; such rows are not drawn)
    let cell_job = |line: usize, math_w: &[f32], measure: bool| -> Vec<(StyledText, Links)> {
        let mut st = Style::base(t.font_size, line_px, text_color);
        if line == 0 {
            st.fam = FontRole::Bold;
        }
        let spans = doc.table_row_cells(table.rows[line]);
        (0..ncols)
            .map(|ci| {
                let rich = match spans.get(ci) {
                    Some(&span) => doc.inline(
                        InlineSource::Cell {
                            span,
                            cap: crate::model::MAX_CELL_BYTES,
                        },
                        origin,
                    ),
                    None => RichInline::default(),
                };
                let mw = math_w.get(ci).copied().unwrap_or(width);
                let mut b = InlineBuilder::new(t, model, false)
                    .with_math(images, ppp, mw)
                    .with_links(links)
                    .measuring(measure)
                    .reveal(src, focus);
                b.nodes(&rich, &rich.nodes, &st, true);
                if let Some(&span) = spans.get(ci) {
                    b.trailing_ws(span.end.saturating_sub(origin) as u32, &st);
                }
                let (mut job, links) = b.take();
                if job.text.is_empty() {
                    job.append(" ", 0.0, st.text_style());
                }
                (job, links)
            })
            .collect()
    };
    let estimated: Option<&[u32]> = match seg {
        Some(BlockSegment::Rows { col_chars, .. }) => Some(col_chars),
        _ => None,
    };
    // small tables: shape every row for natural widths; segmented: the model's estimate
    let sample: Vec<usize> = if estimated.is_some() {
        Vec::new()
    } else {
        std::iter::once(0)
            .chain(2..sample_end)
            .filter(|&l| l < nlines)
            .collect()
    };
    // natural (unwrapped) text blocks; reused below when a cell needs no wrapping
    let mut natural = vec![0.0f32; ncols];
    type Shaped = (StyledText, Arc<dyn TextBlock>, Links);
    let mut unwrapped: std::collections::HashMap<usize, Vec<Shaped>> = Default::default();
    let drawn: Vec<usize> = (if first { vec![0] } else { vec![] })
        .into_iter()
        .chain(body.clone())
        .filter(|&l| l < nlines)
        .collect();
    let full_w = vec![width; ncols];
    for &l in &sample {
        let mut row = Vec::with_capacity(ncols);
        for (ci, (job, links)) in cell_job(l, &full_w, true).into_iter().enumerate() {
            let g = shaper.shape(&job, None);
            natural[ci] = natural[ci].max(g.size().x + box_reserve(&links) + 2.0 * pad);
            row.push((job, g, links));
        }
        // a row with (measured, bitmap-less) formulas is rebuilt for drawing
        let has_math = row
            .iter()
            .any(|c: &Shaped| c.2.iter().any(|(_, m)| matches!(m, Mark::Math { .. })));
        if drawn.contains(&l) && !has_math {
            unwrapped.insert(l, row);
        }
    }
    if let Some(cc) = estimated {
        let char_w = t.font_size * 0.55;
        for (ci, n) in natural.iter_mut().enumerate() {
            *n = cc.get(ci).copied().unwrap_or(1) as f32 * char_w + 2.0 * pad;
        }
    }
    let avail = width.max(ncols as f32 * 30.0);
    let total: f32 = natural.iter().sum();
    let widths: Vec<f32> = if total <= avail {
        natural.clone()
    } else {
        // water-filling: narrow columns keep their width, wide ones share the rest
        let mut w = vec![0.0f32; ncols];
        let mut fixed = vec![false; ncols];
        let mut remaining = avail;
        loop {
            let open: Vec<usize> = (0..ncols).filter(|&i| !fixed[i]).collect();
            if open.is_empty() {
                break;
            }
            let share = remaining / open.len() as f32;
            let mut changed = false;
            for &i in &open {
                if natural[i] <= share {
                    w[i] = natural[i];
                    fixed[i] = true;
                    remaining -= natural[i];
                    changed = true;
                }
            }
            if !changed {
                for &i in &open {
                    w[i] = share.max(30.0);
                }
                break;
            }
        }
        w
    };
    let table_w: f32 = widths.iter().sum();
    let border = Stroke::new(1.0, color(t.table_border));
    let mut cy = y;
    let mut row_rects = Vec::new();
    // (text position, text, marks, fragment, cell rect)
    type Placed = (Pos2, Arc<dyn TextBlock>, Links, FragmentId, Rect);
    let mut placed: Vec<Placed> = Vec::new();
    // content width of each column: formulas are rendered no wider (review of PR #15)
    let inners: Vec<f32> = widths.iter().map(|w| (w - 2.0 * pad).max(1.0)).collect();
    for &l in &drawn {
        let mut pre = unwrapped.remove(&l);
        // a pre-shaped row whose cells do not fit is rebuilt with formulas sized to the cells
        if pre
            .as_ref()
            .is_some_and(|p| p.iter().enumerate().any(|(ci, c)| c.1.size().x > inners[ci] + 0.5))
        {
            pre = None;
        }
        let jobs = if pre.is_none() {
            Some(cell_job(l, &inners, false))
        } else {
            None
        };
        let mut gs = Vec::with_capacity(ncols);
        let mut rh: f32 = 0.0;
        for ci in 0..ncols {
            let reserve = match (&pre, &jobs) {
                (Some(p), _) => box_reserve(&p[ci].2),
                (None, Some(js)) => box_reserve(&js[ci].1),
                _ => 0.0,
            };
            let inner = (widths[ci] - 2.0 * pad - reserve).max(1.0);
            let (g, links) = match (&pre, &jobs) {
                (Some(p), _) if p[ci].1.size().x <= inner + 0.5 => (p[ci].1.clone(), p[ci].2.clone()),
                (Some(p), _) => (shaper.shape(&p[ci].0, wrapped(inner)), p[ci].2.clone()),
                (None, Some(js)) => (shaper.shape(&js[ci].0, wrapped(inner)), js[ci].1.clone()),
                _ => unreachable!("either pre-shaped or jobs"),
            };
            rh = rh.max(g.size().y);
            gs.push((g, links));
        }
        let rh = rh + 2.0 * pad;
        let mut cx = x;
        for (ci, (g, links)) in gs.into_iter().enumerate() {
            let gw = g.size().x;
            let inner = widths[ci] - 2.0 * pad;
            let dx = match aligns.get(ci).copied().unwrap_or_default() {
                ColumnAlign::Center => (inner - gw) * 0.5,
                ColumnAlign::Right => inner - gw,
                _ => 0.0,
            }
            .max(0.0);
            let mut frag = FragmentId::new(FragmentKind::Cell {
                row: l as u32,
                col: ci as u32,
            });
            frag.block = (table.rows[l].start - origin) as u32;
            let cell = Rect::from_min_size(pos2(cx, cy), vec2(widths[ci], rh));
            placed.push((pos2(cx + pad + dx, cy + pad), g, links, frag, cell));
            cx += widths[ci];
        }
        row_rects.push((l, Rect::from_min_size(pos2(x, cy), vec2(table_w, rh))));
        cy += rh;
    }
    for (l, rect) in &row_rects {
        // body row number (1-based) decides the stripe, so segments continue the pattern
        let fill = if *l == 0 {
            color(t.table_header_bg)
        } else if (l - 1) % 2 == 0 {
            color(t.table_stripe_bg)
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            out.prims.push(Prim::Rect {
                rect: *rect,
                fill,
                stroke: Stroke::NONE,
                radius: 0.0,
            });
        }
        if *l > 0 {
            out.prims.push(Prim::Line {
                a: rect.left_top(),
                b: rect.right_top(),
                stroke: border,
            });
        }
    }
    let mut cx = x;
    for w in &widths[..ncols - 1] {
        cx += w;
        out.prims.push(Prim::Line {
            a: pos2(cx, y),
            b: pos2(cx, cy),
            stroke: border,
        });
    }
    // outer border: sides always, top on the first segment, bottom on the last
    for (a, b) in [(pos2(x, y), pos2(x, cy)), (pos2(x + table_w, y), pos2(x + table_w, cy))] {
        out.prims.push(Prim::Line { a, b, stroke: border });
    }
    if first {
        out.prims.push(Prim::Line {
            a: pos2(x, y),
            b: pos2(x + table_w, y),
            stroke: border,
        });
    }
    if last {
        out.prims.push(Prim::Line {
            a: pos2(x, cy),
            b: pos2(x + table_w, cy),
            stroke: border,
        });
    }
    for (pos, g, links, frag, cell) in placed {
        let n = out.prims.len();
        out.text(pos, g, links, frag);
        // formulas never paint outside their cell
        for p in &mut out.prims[n..] {
            if let Prim::Math { clip, .. } = p {
                *clip = Some(cell);
            }
        }
    }
    cy
}
