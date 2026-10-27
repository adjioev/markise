//! Paragraph layout (ADR 0005 §4.1 `layout.rs` + `caret.rs`): wraps `parley::Layout`, exposes
//! lines, metrics and glyph runs, and implements grapheme-correct carets, hit testing in both
//! directions and selection rectangles on top of parley's cluster geometry.

use std::ops::Range;
use std::sync::Arc;

use std::borrow::Cow;

use parley::setting::Tag;
use parley::{
    Affinity as PAffinity, Cursor, FontFamily, FontFeature, FontFeatures, FontStyle, FontVariation, FontVariations,
    FontWeight, FontWidth, InlineBox, InlineBoxKind, Layout, LayoutContext, LineHeight, PositionedLayoutItem,
    Selection, StyleProperty, VerticalAlign,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::matching::FaceSlope;
use crate::store::FontStore;
use crate::style::{Slope, TextStyle};
use crate::styled::{BaseDir, StyleId, StyledText};

/// Axis-aligned rectangle in layout coordinates (origin at the layout's top-left, y down).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
}

/// Which side of a position the caret belongs to where one offset has two visual positions
/// (bidi run boundaries, soft-wrap ends; ADR 0005 §4.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Affinity {
    /// Attached to the character before the offset.
    Upstream,
    /// Attached to the character after the offset.
    #[default]
    Downstream,
}

/// A caret position local to one layout: byte offset (always a grapheme boundary) + affinity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LocalPos {
    pub byte: usize,
    pub affinity: Affinity,
}

impl LocalPos {
    pub const fn new(byte: usize, affinity: Affinity) -> Self {
        LocalPos { byte, affinity }
    }
    pub const fn down(byte: usize) -> Self {
        LocalPos {
            byte,
            affinity: Affinity::Downstream,
        }
    }
    pub const fn up(byte: usize) -> Self {
        LocalPos {
            byte,
            affinity: Affinity::Upstream,
        }
    }
}

fn to_p(a: Affinity) -> PAffinity {
    match a {
        Affinity::Upstream => PAffinity::Upstream,
        Affinity::Downstream => PAffinity::Downstream,
    }
}

fn from_p(a: PAffinity) -> Affinity {
    match a {
        PAffinity::Upstream => Affinity::Upstream,
        PAffinity::Downstream => Affinity::Downstream,
    }
}

/// Brush carried through parley: our style id.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleBrush(pub StyleId);

/// Metrics of one visual line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineInfo {
    pub text_range: (usize, usize),
    /// Top and bottom of the line box.
    pub top: f32,
    pub bottom: f32,
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Left edge of the content after alignment, and its advance.
    pub x: f32,
    pub advance: f32,
}

/// One grapheme cluster on a line (see [`TextLayout::clusters`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterInfo {
    pub line: usize,
    pub text_range: Range<usize>,
    /// Left edge and advance (scaled units).
    pub x: f32,
    pub advance: f32,
    pub is_space: bool,
}

/// Font of a glyph run (wraps the shared font data; `id` is stable for the blob's lifetime).
#[derive(Clone)]
pub struct FontHandle(parley::FontData);

impl FontHandle {
    pub fn data(&self) -> &[u8] {
        self.0.data.data()
    }
    pub fn index(&self) -> u32 {
        self.0.index
    }
    /// Unique id of the font blob (atlas/cache key with `index`).
    pub fn id(&self) -> u64 {
        self.0.data.id()
    }

    /// The shared font data (`linebender_resource_handle::FontData`, the same type as
    /// `peniko::FontData` that vello_cpu's glyph renderer takes): no copy of the font bytes.
    pub fn shared(&self) -> &parley::FontData {
        &self.0
    }
}

impl std::fmt::Debug for FontHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontHandle(blob {}, index {})", self.id(), self.index())
    }
}

/// A positioned glyph (layout coordinates of the glyph origin on the baseline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
}

/// One run of glyphs sharing font, size, synthesis, variation and style — the unit a glyph
/// atlas consumes.
#[derive(Clone, Debug)]
pub struct GlyphRun {
    pub font: FontHandle,
    /// Font size in layout units times the layout scale (i.e. pixels when scale = ppp).
    pub size: f32,
    pub embolden: bool,
    /// Synthetic skew in degrees.
    pub skew: Option<f32>,
    /// Normalised variation coordinates (F2Dot14 as i16).
    pub coords: Vec<i16>,
    pub style: StyleId,
    pub line: usize,
    pub baseline: f32,
    pub rtl: bool,
    pub text_range: Range<usize>,
    pub glyphs: Vec<Glyph>,
}

/// Shaping engine: fonts plus parley's reusable layout context.
pub struct TextEngine {
    pub fonts: FontStore,
    lcx: LayoutContext<StyleBrush>,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new(FontStore::new())
    }
}

fn features(style: &TextStyle, marker: u8) -> Vec<FontFeature> {
    let mut v: Vec<FontFeature> = style
        .features
        .iter()
        .map(|f| FontFeature {
            tag: Tag::new(&f.tag),
            value: f.value,
        })
        .collect();
    if marker != 0 {
        // private tag, ignored by shapers: only forces a shaping-item boundary (rule 4b)
        v.push(FontFeature {
            tag: Tag::new(b"mksp"),
            value: u16::from(marker),
        });
    }
    v
}

fn fontique_style(s: Slope) -> FontStyle {
    match s {
        Slope::Normal => FontStyle::Normal,
        Slope::Italic => FontStyle::Italic,
        Slope::Oblique(a) => FontStyle::Oblique(Some(a)),
    }
}

fn face_slope(s: FaceSlope) -> FontStyle {
    match s {
        FaceSlope::Normal => FontStyle::Normal,
        FaceSlope::Italic => FontStyle::Italic,
        FaceSlope::Oblique(lo, _) => FontStyle::Oblique(Some(lo)),
    }
}

impl TextEngine {
    pub fn new(fonts: FontStore) -> Self {
        TextEngine {
            fonts,
            lcx: LayoutContext::new(),
        }
    }

    /// Shapes and breaks `text` at `max_width` (layout units; `None` = no wrapping). `scale` is
    /// the display scale: all output geometry (positions, sizes, rects) is in scaled units
    /// (physical pixels when `scale` = pixels per point).
    /// Malformed input is normalised (see [`StyledText`]); a non-finite or non-positive
    /// `scale` is 1, a non-finite `max_width` means no wrapping, a negative one 0.
    pub fn layout(&mut self, text: &StyledText, max_width: Option<f32>, scale: f32) -> TextLayout {
        let owned;
        let text = if text.needs_normalizing() {
            owned = text.normalized();
            &owned
        } else {
            text
        };
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let max_width = max_width.filter(|w| w.is_finite()).map(|w| w.max(0.0));
        // 1. cluster matching → pieces with family lists (needs &mut fonts: system lookups)
        let mut props: Vec<(Range<usize>, Vec<StyleProperty<'static, StyleBrush>>)> = Vec::new();
        for (r, id) in text.resolved_runs() {
            let style = &text.styles[id as usize];
            for p in self.fonts.itemize(&text.text, r, style) {
                let list = self.fonts.family_list(style, &p.choice);
                let mut v: Vec<StyleProperty<'static, StyleBrush>> = vec![
                    StyleProperty::FontFamily(FontFamily::List(Cow::Owned(list))),
                    StyleProperty::FontSize(style.size),
                    StyleProperty::Brush(StyleBrush(id)),
                    StyleProperty::Underline(style.underline),
                    StyleProperty::Strikethrough(style.strikethrough),
                    StyleProperty::LetterSpacing(style.letter_spacing),
                    StyleProperty::WordSpacing(style.word_spacing),
                    StyleProperty::LineHeight(match style.line_height {
                        Some(m) => LineHeight::FontSizeRelative(m),
                        None => LineHeight::MetricsRelative(1.0),
                    }),
                    StyleProperty::FontFeatures(FontFeatures::List(Cow::Owned(features(style, p.choice.split_marker)))),
                ];
                let mut variations = Vec::new();
                match p.choice.face {
                    Some(f) => {
                        let face = &self.fonts.faces[f];
                        let d = face.rule.descriptors;
                        let w = if p.choice.synth_bold { style.weight } else { d.weight.0 };
                        let st = if p.choice.synth_slant {
                            fontique_style(style.slope)
                        } else {
                            face_slope(d.slope)
                        };
                        v.push(StyleProperty::FontWeight(FontWeight::new(w)));
                        v.push(StyleProperty::FontStyle(st));
                        v.push(StyleProperty::FontWidth(FontWidth::from_percentage(d.stretch.0)));
                        let has = |t: &[u8; 4]| face.axes.contains(t);
                        let mut var = |t: &[u8; 4], v: Option<f32>| {
                            if has(t)
                                && let Some(value) = v
                            {
                                variations.push(FontVariation {
                                    tag: Tag::new(t),
                                    value,
                                });
                            }
                        };
                        // the instance CSS matching resolved (ADR 0005 §4.1 step 5)
                        var(b"wght", p.choice.wght);
                        var(b"wdth", p.choice.wdth);
                        var(b"slnt", p.choice.oblique.map(|a| -a));
                    }
                    None => {
                        v.push(StyleProperty::FontWeight(FontWeight::new(style.weight)));
                        v.push(StyleProperty::FontStyle(fontique_style(style.slope)));
                        v.push(StyleProperty::FontWidth(FontWidth::from_percentage(style.stretch)));
                    }
                }
                v.push(StyleProperty::FontVariations(FontVariations::List(Cow::Owned(
                    variations,
                ))));
                props.push((p.range, v));
            }
        }
        // 2. build
        let mut b = self.lcx.ranged_builder(&mut self.fonts.fcx, &text.text, scale, true);
        if let Some(s0) = text.styles.first() {
            b.push_default(StyleProperty::FontSize(s0.size));
        }
        match text.base_dir {
            BaseDir::Auto => {}
            BaseDir::Ltr => b.set_base_direction(parley::BaseDirection::Ltr),
            BaseDir::Rtl => b.set_base_direction(parley::BaseDirection::Rtl),
        }
        for (r, v) in props {
            for p in v {
                b.push(p, r.clone());
            }
        }
        for o in &text.inline_objects {
            b.push_inline_box(InlineBox {
                id: o.id,
                kind: InlineBoxKind::InFlow,
                index: o.index.min(text.text.len()),
                width: o.width * scale,
                height: o.height * scale,
                baseline: None,
                vertical_align: VerticalAlign::BASELINE,
            });
        }
        let mut layout: Layout<StyleBrush> = b.build(&text.text);
        layout.break_all_lines(max_width.map(|w| w * scale));
        let mut graphemes: Vec<usize> = text.text.grapheme_indices(true).map(|(i, _)| i).collect();
        graphemes.push(text.text.len());
        TextLayout {
            layout,
            text: text.text.as_str().into(),
            graphemes,
            scale,
        }
    }
}

/// A shaped, broken paragraph. Geometry is in scaled units (see [`TextEngine::layout`]).
pub struct TextLayout {
    pub(crate) layout: Layout<StyleBrush>,
    text: Arc<str>,
    /// Grapheme boundaries, sorted, including 0 and `text.len()`.
    graphemes: Vec<usize>,
    scale: f32,
}

impl std::fmt::Debug for TextLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextLayout")
            .field("len", &self.text.len())
            .field("lines", &self.layout.len())
            .finish()
    }
}

fn rect(b: parley::BoundingBox) -> Rect {
    Rect {
        x0: b.x0 as f32,
        y0: b.y0 as f32,
        x1: b.x1 as f32,
        y1: b.y1 as f32,
    }
}

impl TextLayout {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// (width, height) of the laid-out text.
    pub fn size(&self) -> (f32, f32) {
        (self.layout.width(), self.layout.height())
    }

    /// Paragraph direction resolved by UAX #9 (or set explicitly).
    pub fn is_rtl(&self) -> bool {
        self.layout.is_rtl()
    }

    /// Grapheme-cluster boundaries: the only caret stops (R1).
    pub fn grapheme_boundaries(&self) -> &[usize] {
        &self.graphemes
    }

    pub fn is_caret_stop(&self, byte: usize) -> bool {
        self.graphemes.binary_search(&byte).is_ok()
    }

    fn floor_stop(&self, byte: usize) -> usize {
        match self.graphemes.binary_search(&byte) {
            Ok(i) => self.graphemes[i],
            Err(i) => self.graphemes[i.saturating_sub(1)],
        }
    }

    fn ceil_stop(&self, byte: usize) -> usize {
        match self.graphemes.binary_search(&byte) {
            Ok(i) => self.graphemes[i],
            Err(i) => self.graphemes[i.min(self.graphemes.len() - 1)],
        }
    }

    /// Number of visual lines.
    pub fn line_count(&self) -> usize {
        self.layout.len()
    }

    pub fn lines(&self) -> Vec<LineInfo> {
        self.layout
            .lines()
            .map(|l| {
                let m = l.metrics();
                let r = l.text_range();
                LineInfo {
                    text_range: (r.start, r.end),
                    top: m.block_min_coord,
                    bottom: m.block_max_coord,
                    baseline: m.baseline,
                    ascent: m.baseline - m.content_block_min_coord,
                    descent: m.content_block_max_coord - m.baseline,
                    x: m.offset,
                    advance: m.advance,
                }
            })
            .collect()
    }

    /// Glyph runs in visual order per line (input for a glyph atlas).
    pub fn glyph_runs(&self) -> Vec<GlyphRun> {
        let mut out = Vec::new();
        for (li, line) in self.layout.lines().enumerate() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else {
                    continue;
                };
                let run = gr.run();
                let syn = run.synthesis();
                out.push(GlyphRun {
                    font: FontHandle(run.font().clone()),
                    size: run.font_size(),
                    embolden: syn.embolden(),
                    skew: syn.skew(),
                    coords: run.normalized_coords().iter().map(|c| c.to_bits()).collect(),
                    style: gr.style().brush.0,
                    line: li,
                    baseline: gr.baseline(),
                    rtl: run.is_rtl(),
                    text_range: run.text_range(),
                    glyphs: gr
                        .positioned_glyphs()
                        .map(|g| Glyph {
                            id: g.id,
                            x: g.x,
                            y: g.y,
                            advance: g.advance,
                        })
                        .collect(),
                });
            }
        }
        out
    }

    /// Clusters in visual order, line by line, with their left edge (scaled units) and advance;
    /// hard line breaks are omitted. Ligature components share the ligature's advance evenly
    /// (parley's rule). Input for justification and decoration geometry.
    pub fn clusters(&self) -> Vec<ClusterInfo> {
        let mut out = Vec::new();
        for (li, line) in self.layout.lines().enumerate() {
            let mut current: Option<(Range<usize>, std::iter::Peekable<_>)> = None;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else {
                    continue;
                };
                let run = gr.run();
                let key = run.text_range();
                if current.as_ref().is_none_or(|(k, _)| *k != key) {
                    current = Some((key, run.visual_clusters().peekable()));
                }
                let Some((_, it)) = current.as_mut() else { continue };
                let mut x = gr.offset();
                while let Some(c) = it.peek() {
                    if c.style_index() != gr.style_index() {
                        break;
                    }
                    let Some(c) = it.next() else { break };
                    let adv = c.advance();
                    if !c.is_hard_line_break() {
                        out.push(ClusterInfo {
                            line: li,
                            text_range: c.text_range(),
                            x,
                            advance: adv,
                            is_space: c.is_space_or_nbsp(),
                        });
                    }
                    x += adv;
                }
            }
        }
        out
    }

    /// Source text cluster of every glyph, parallel to [`TextLayout::glyph_runs`] (same runs,
    /// same glyph order): `out[run][glyph]` is the byte range of the cluster the glyph belongs
    /// to. Used where glyphs must map back to text (PDF ToUnicode / ActualText). When the
    /// cluster glyph counts do not add up, every glyph of the run gets the covered range.
    pub fn glyph_clusters(&self) -> Vec<Vec<Range<usize>>> {
        let mut out = Vec::new();
        for line in self.layout.lines() {
            // A shaped run is split into several glyph runs where the style changes; they consume
            // the run's clusters (visual order) one after the other.
            let mut cur: Option<usize> = None;
            let mut clusters: Vec<(Range<usize>, usize)> = Vec::new();
            let mut pos = 0;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else {
                    continue;
                };
                let run = gr.run();
                if cur != Some(run.index()) {
                    cur = Some(run.index());
                    clusters = run
                        .visual_clusters()
                        .map(|c| (c.text_range(), c.glyphs().count()))
                        .collect();
                    pos = 0;
                }
                let n = gr.positioned_glyphs().count();
                let mut v: Vec<Range<usize>> = Vec::with_capacity(n);
                // a cluster without glyphs (ligature continuation) joins the previous glyph's
                while pos < clusters.len() && (v.len() < n || clusters[pos].1 == 0) {
                    let (r, k) = &clusters[pos];
                    if *k == 0 {
                        if let Some(old) = v.last().cloned() {
                            let merged = old.start.min(r.start)..old.end.max(r.end);
                            // every glyph of the same cluster carries the merged range
                            for g in v.iter_mut().rev().take_while(|g| **g == old) {
                                *g = merged.clone();
                            }
                        }
                    } else {
                        for _ in 0..*k {
                            v.push(r.clone());
                        }
                    }
                    pos += 1;
                }
                if v.len() != n {
                    let lo = v.iter().map(|r| r.start).min().unwrap_or(0);
                    let hi = v.iter().map(|r| r.end).max().unwrap_or(0);
                    v = vec![lo..hi; n];
                }
                out.push(v);
            }
        }
        out
    }

    fn cursor(&self, p: LocalPos) -> Cursor {
        Cursor::from_byte_index(&self.layout, p.byte.min(self.text.len()), to_p(p.affinity))
    }

    fn pos_of(&self, c: Cursor) -> LocalPos {
        LocalPos::new(c.index(), from_p(c.affinity()))
    }

    /// Caret rectangle (zero width; full line-box height) of a position.
    pub fn caret_rect(&self, p: LocalPos) -> Rect {
        let mut r = rect(self.cursor(p).geometry(&self.layout, 0.0));
        r.x1 = r.x0;
        r
    }

    /// Point → position (R2). Positions are snapped to grapheme boundaries: if parley reports a
    /// boundary inside a grapheme (fonts switching inside a cluster, rule 4b), the nearer
    /// boundary by x wins.
    pub fn hit(&self, x: f32, y: f32) -> LocalPos {
        let c = Cursor::from_point(&self.layout, x, y);
        let p = self.pos_of(c);
        if self.is_caret_stop(p.byte) {
            return p;
        }
        let a = LocalPos::new(self.floor_stop(p.byte), Affinity::Downstream);
        let b = LocalPos::new(self.ceil_stop(p.byte), Affinity::Upstream);
        let da = (self.caret_rect(a).x0 - x).abs();
        let db = (self.caret_rect(b).x0 - x).abs();
        if da <= db { a } else { b }
    }

    /// Hit test on a given visual line at `x` (Up/Down with goal x, §4.3).
    pub fn hit_on_line(&self, line: usize, x: f32) -> Option<LocalPos> {
        let l = self.layout.get(line)?;
        let m = l.metrics();
        Some(self.hit(x, (m.block_min_coord + m.block_max_coord) / 2.0))
    }

    /// Visual line containing the caret of `p`.
    pub fn line_at(&self, p: LocalPos) -> usize {
        let (_, y) = self.caret_rect(p).center();
        let n = self.layout.len();
        for (i, l) in self.layout.lines().enumerate() {
            if y < l.metrics().block_max_coord {
                return i;
            }
        }
        n.saturating_sub(1)
    }

    /// Caret x of `p` (goal x for vertical moves).
    pub fn x_on_line(&self, p: LocalPos) -> f32 {
        self.caret_rect(p).x0
    }

    /// Selection rectangles between two positions (split at bidi boundaries and per line).
    pub fn selection_rects(&self, a: LocalPos, b: LocalPos) -> Vec<Rect> {
        let a = LocalPos::new(self.floor_stop(a.byte), a.affinity);
        let b = LocalPos::new(self.floor_stop(b.byte), b.affinity);
        Selection::new(self.cursor(a), self.cursor(b))
            .geometry(&self.layout)
            .into_iter()
            .map(|(bb, _)| rect(bb))
            .collect()
    }

    /// Next grapheme boundary in logical order (Right in LTR text, "forward delete" extent).
    pub fn next_logical(&self, p: LocalPos) -> LocalPos {
        let i = self.graphemes.partition_point(|&g| g <= p.byte);
        LocalPos::down(*self.graphemes.get(i).unwrap_or(&self.text.len()))
    }

    /// Previous grapheme boundary in logical order (Backspace extent for clusters).
    pub fn prev_logical(&self, p: LocalPos) -> LocalPos {
        let i = self.graphemes.partition_point(|&g| g < p.byte);
        LocalPos::down(if i == 0 { 0 } else { self.graphemes[i - 1] })
    }

    /// Repeats a parley cursor step until it lands on a grapheme boundary (a caret stop) or
    /// stops moving (layout edge). Parley clusters can be finer than graphemes (rule 4b splits
    /// a grapheme into one shaping item per mark), so the walk is not bounded by a fixed
    /// count; progress protection: each step must move to a new (index, affinity) and the walk
    /// is bounded by the number of possible cursor positions (2 per byte + 2).
    fn step_to_stop(&self, p: LocalPos, f: impl Fn(Cursor) -> Cursor) -> LocalPos {
        let mut c = self.cursor(p);
        let bound = 2 * self.text.len() + 2;
        let mut seen = std::collections::HashSet::new();
        seen.insert((c.index(), from_p(c.affinity())));
        for _ in 0..bound {
            let n = f(c);
            if n == c || !seen.insert((n.index(), from_p(n.affinity()))) {
                break; // edge or cycle
            }
            c = n;
            if self.is_caret_stop(c.index()) {
                return self.pos_of(c);
            }
        }
        // edge reached inside a grapheme (should not happen: edges are stops): snap
        if c.index() != p.byte {
            let b = if c.index() > p.byte {
                self.ceil_stop(c.index())
            } else {
                self.floor_stop(c.index())
            };
            return LocalPos::new(b, from_p(c.affinity()));
        }
        p
    }

    /// One grapheme to the right on screen (visual order, bidi-aware).
    pub fn move_right(&self, p: LocalPos) -> LocalPos {
        self.step_to_stop(p, |c| c.next_visual(&self.layout))
    }

    /// One grapheme to the left on screen.
    pub fn move_left(&self, p: LocalPos) -> LocalPos {
        self.step_to_stop(p, |c| c.previous_visual(&self.layout))
    }

    /// Word to the right / left on screen (UAX #29 word boundaries via parley).
    pub fn word_right(&self, p: LocalPos) -> LocalPos {
        self.step_to_stop(p, |c| c.next_visual_word(&self.layout))
    }

    pub fn word_left(&self, p: LocalPos) -> LocalPos {
        self.step_to_stop(p, |c| c.previous_visual_word(&self.layout))
    }

    /// Visual line start / end of the line containing `p`.
    pub fn line_start(&self, p: LocalPos) -> LocalPos {
        let s = Selection::from(self.cursor(p)).line_start(&self.layout, false);
        self.pos_of(s.focus())
    }

    pub fn line_end(&self, p: LocalPos) -> LocalPos {
        let s = Selection::from(self.cursor(p)).line_end(&self.layout, false);
        let f = s.focus();
        let q = self.pos_of(f);
        LocalPos::new(self.floor_stop(q.byte), q.affinity)
    }

    /// Up/Down by `delta` visual lines keeping `goal_x`; `None` when the target line is outside
    /// this layout (the caller continues in the neighbouring fragment, ADR 0005 §4.3).
    pub fn move_lines(&self, p: LocalPos, delta: isize, goal_x: Option<f32>) -> Option<(LocalPos, f32)> {
        let x = goal_x.unwrap_or_else(|| self.x_on_line(p));
        let line = self.line_at(p) as isize + delta;
        if line < 0 || line as usize >= self.layout.len() {
            return None;
        }
        self.hit_on_line(line as usize, x).map(|q| (q, x))
    }

    /// Approximate heap cost in bytes (layout-cache accounting).
    pub fn cost(&self) -> usize {
        // parley keeps per-char, per-cluster and per-glyph records; measured ~ (see bench)
        self.text.len() * 48 + self.graphemes.len() * 8 + 256
    }
}
