//! parley + fontique (through `markise-text`) behind the seam: [`ParleyShaper`] → [`ParleyBlock`].
//!
//! Interim backend for ADR 0005 T2/T3 (developer switch `MARKISE_TEXT=parley` in the UI, default
//! off). It shapes, breaks and measures with parley and answers every [`TextBlock`] query; the
//! egui front end paints its glyph runs through a vello_cpu glyph atlas.
//!
//! Known gaps against the epaint backend (T3–T5 work, see ADR 0005 §5):
//! - `valign` (superscript/subscript raise), run backgrounds, backend underline/strikethrough are
//!   ignored (the layout draws its own decorations from [`TextBlock::ink_spans`]);
//! - epaint `FontTweak`s (scale, y offset) of the egui fonts are not applied;
//! - row positions are not snapped to pixels;
//! - theme fonts come from the same faces as the epaint families (one family per face, in
//!   epaint's fallback order; a face of a collection file is copied out on registration), not
//!   yet from CSS face matching (T4).

use std::any::Any;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use epaint::emath::Align;
use epaint::text::{FontDefinitions, FontFamily};
use epaint::{Color32, Pos2, Rect, Vec2, pos2, vec2};
use markise_text::{
    Family, FontStore, Generic, GlyphRun, InlineObject, Slope, StyledText as MtStyledText, TextEngine, TextLayout,
    TextStyle as MtStyle,
};

use super::{Cluster, FontRole, InkSpan, LineInfo, LocalPos, StyledText, TextBlock, TextShaper, TextStyle};

/// Font families per [`FontRole`] (index = [`FontRole::index`]), in fallback order.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleFamilies {
    pub roles: [Vec<Family>; 6],
}

impl Default for RoleFamilies {
    /// Generic families (no faces registered): sans-serif text, monospace code.
    fn default() -> Self {
        let sans = vec![Family::Generic(Generic::SansSerif)];
        let mono = vec![Family::Generic(Generic::Monospace)];
        RoleFamilies {
            roles: [sans.clone(), sans.clone(), sans.clone(), sans, mono.clone(), mono],
        }
    }
}

impl RoleFamilies {
    /// Registers every face of the `markise-*` epaint families of `defs` in `store` (one family
    /// per face, no copy of the font bytes) and returns the roles in epaint's fallback order, so
    /// both backends use the same fonts.
    pub fn from_font_definitions(defs: &FontDefinitions, store: &mut FontStore) -> Self {
        let mut registered: ahash::AHashMap<String, Family> = Default::default();
        let mut roles: [Vec<Family>; 6] = Default::default();
        for role in FontRole::ALL {
            let names = defs
                .families
                .get(&FontFamily::Name(role.family_name().into()))
                .cloned()
                .unwrap_or_default();
            for n in names {
                let fam = match registered.get(&n) {
                    Some(f) => f.clone(),
                    None => {
                        let Some(data) = defs.font_data.get(&n) else { continue };
                        let fam_name = format!("mk-egui-{n}");
                        // the selected face only (a collection's other faces must not
                        // answer for this alias)
                        if !store.register_system_family_shared(&fam_name, data.font.clone(), data.index, 100.0, 400.0)
                        {
                            continue;
                        }
                        let f = Family::named(&fam_name);
                        registered.insert(n.clone(), f.clone());
                        f
                    }
                };
                roles[role.index()].push(fam);
            }
            let generic = if matches!(role, FontRole::Mono | FontRole::MonoBold) {
                Generic::Monospace
            } else {
                Generic::SansSerif
            };
            roles[role.index()].push(Family::Generic(generic));
        }
        RoleFamilies { roles }
    }
}

/// Generation salt: parley layouts never share cache keys with epaint ones.
const PARLEY_SALT: u64 = 0x7061_726c_6579_0000;

/// The parley backend.
pub struct ParleyShaper {
    engine: TextEngine,
    roles: RoleFamilies,
    /// Display scale (pixels per point): glyphs are positioned and rasterised in pixels.
    scale: f32,
    /// Bumped when the font store is replaced (its own generation restarts).
    epoch: u64,
}

impl ParleyShaper {
    pub fn new(engine: TextEngine, roles: RoleFamilies) -> Self {
        ParleyShaper {
            engine,
            roles,
            scale: 1.0,
            epoch: 0,
        }
    }

    /// A shaper over the faces of epaint font definitions (see
    /// [`RoleFamilies::from_font_definitions`]); no system font discovery.
    pub fn from_font_definitions(defs: &FontDefinitions) -> Self {
        let mut store = FontStore::new();
        let roles = RoleFamilies::from_font_definitions(defs, &mut store);
        Self::new(TextEngine::new(store), roles)
    }

    /// Replaces the fonts (theme change): a new store, so the generation changes.
    pub fn set_font_definitions(&mut self, defs: &FontDefinitions) {
        let mut store = FontStore::new();
        let roles = RoleFamilies::from_font_definitions(defs, &mut store);
        self.engine = TextEngine::new(store);
        self.epoch += 1;
        self.roles = roles;
    }

    /// Sets pixels per point (part of layout cache keys already).
    pub fn set_scale(&mut self, ppp: f32) {
        if ppp.is_finite() && ppp > 0.0 {
            self.scale = ppp;
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn engine_mut(&mut self) -> &mut TextEngine {
        &mut self.engine
    }

    fn mt_style(&self, s: &TextStyle) -> MtStyle {
        let c = s.color.to_srgba_unmultiplied();
        MtStyle {
            families: self.roles.roles[s.role.index()].clone(),
            size: s.size,
            weight: 400.0,
            slope: if s.italic { Slope::Italic } else { Slope::Normal },
            color: u32::from_be_bytes(c),
            underline: s.underline,
            strikethrough: s.strike,
            letter_spacing: s.letter_spacing,
            line_height: (s.size > 0.0).then(|| s.line_height / s.size),
            ..MtStyle::default()
        }
    }

    fn convert(&self, text: &StyledText) -> (MtStyledText, Vec<Color32>) {
        let mut styles = Vec::with_capacity(text.runs.len());
        let mut runs = Vec::with_capacity(text.runs.len());
        let mut colors = Vec::with_capacity(text.runs.len());
        let mut boxes = Vec::new();
        for (i, r) in text.runs.iter().enumerate() {
            styles.push(self.mt_style(&r.style));
            colors.push(r.style.color);
            runs.push((r.range.clone(), i as u16));
            if r.lead > 0.0 {
                boxes.push(InlineObject {
                    id: i as u64,
                    index: r.range.start,
                    width: r.lead,
                    height: 0.0,
                });
            }
        }
        (
            MtStyledText {
                text: text.text.clone(),
                styles,
                runs,
                base_dir: markise_text::BaseDir::Auto,
                inline_objects: boxes,
            },
            colors,
        )
    }

    fn layout(&mut self, text: &StyledText, width: Option<f32>) -> ParleyBlock {
        let (mt, colors) = self.convert(text);
        // justified rows are rebuilt one by one and never wrap: `break_on_newline` false only
        // occurs there (and their text holds no `\n`)
        let layout = self.engine.layout(&mt, width, self.scale);
        ParleyBlock::new(layout, colors, text.halign)
    }
}

impl TextShaper for ParleyShaper {
    fn shape(&mut self, text: &StyledText, width: Option<f32>) -> Arc<dyn TextBlock> {
        let mut b = self.layout(text, width);
        b.map = text.source_map.clone();
        Arc::new(b)
    }

    fn has_glyphs(&mut self, style: &TextStyle, s: &str) -> bool {
        let b = self.layout(&StyledText::single(s, style.clone()), None);
        !b.has_notdef()
    }

    fn lacks_glyphs(&mut self, block: &dyn TextBlock) -> bool {
        block
            .as_any()
            .downcast_ref::<ParleyBlock>()
            .is_some_and(ParleyBlock::has_notdef)
    }

    fn generation(&self) -> u64 {
        PARLEY_SALT ^ (self.epoch << 32) ^ self.engine.fonts.generation()
    }
}

/// A parley layout as a [`TextBlock`]. The layout is in pixels (scale = pixels per point); this
/// type converts to points.
pub struct ParleyBlock {
    layout: TextLayout,
    colors: Vec<Color32>,
    /// Per line: horizontal offset (points) applied for `halign` (epaint's anchor convention).
    dx: Vec<f32>,
    lines: Vec<markise_text::LineInfo>,
    rect: Rect,
    runs: OnceLock<Vec<GlyphRun>>,
    map: Option<Arc<super::SourceMap>>,
}

impl std::fmt::Debug for ParleyBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParleyBlock")
            .field("layout", &self.layout)
            .field("rect", &self.rect)
            .finish()
    }
}

impl ParleyBlock {
    fn new(layout: TextLayout, colors: Vec<Color32>, halign: Align) -> Self {
        let s = layout.scale();
        let lines = layout.lines();
        let dx: Vec<f32> = lines
            .iter()
            .map(|l| match halign {
                Align::Min => 0.0,
                Align::Center => -0.5 * l.advance / s,
                Align::Max => -l.advance / s,
            })
            .collect();
        let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
        for (l, d) in lines.iter().zip(&dx) {
            x0 = x0.min(*d + l.x / s);
            x1 = x1.max(*d + (l.x + l.advance) / s);
        }
        if !x0.is_finite() {
            (x0, x1) = (0.0, 0.0);
        }
        let h = lines.last().map_or(layout.size().1, |l| l.bottom) / s;
        ParleyBlock {
            rect: Rect::from_min_max(pos2(x0, 0.0), pos2(x1, h)),
            layout,
            colors,
            dx,
            lines,
            runs: OnceLock::new(),
            map: None,
        }
    }

    /// The parley layout (pixels).
    pub fn layout(&self) -> &TextLayout {
        &self.layout
    }

    /// Pixels per point the layout was made for.
    pub fn scale(&self) -> f32 {
        self.layout.scale()
    }

    /// Glyph runs (pixels, before the per-line alignment offset [`ParleyBlock::line_dx_px`]).
    pub fn glyph_runs(&self) -> &[GlyphRun] {
        self.runs.get_or_init(|| self.layout.glyph_runs())
    }

    /// Alignment offset of a line in pixels.
    pub fn line_dx_px(&self, line: usize) -> f32 {
        self.dx.get(line).copied().unwrap_or(0.0) * self.layout.scale()
    }

    /// Text colour of a style (glyph run `style`).
    pub fn color(&self, style: u16) -> Color32 {
        self.colors.get(style as usize).copied().unwrap_or(Color32::GRAY)
    }

    /// Whether a glyph run uses `.notdef` (a character no font could draw).
    pub fn has_notdef(&self) -> bool {
        self.glyph_runs().iter().any(|r| r.glyphs.iter().any(|g| g.id == 0))
    }

    fn line_at_y(&self, y_pt: f32) -> usize {
        let s = self.layout.scale();
        let y = y_pt * s;
        self.lines
            .iter()
            .position(|l| y < l.bottom)
            .unwrap_or(self.lines.len().saturating_sub(1))
    }

    fn to_pt(&self, r: markise_text::Rect) -> Rect {
        let s = self.layout.scale();
        let line = self.line_at_y((r.y0 + r.y1) * 0.5 / s);
        let d = self.dx.get(line).copied().unwrap_or(0.0);
        Rect::from_min_max(pos2(r.x0 / s + d, r.y0 / s), pos2(r.x1 / s + d, r.y1 / s))
    }
}

impl TextBlock for ParleyBlock {
    fn text(&self) -> &str {
        self.layout.text()
    }

    fn size(&self) -> Vec2 {
        self.rect.size()
    }

    fn rect(&self) -> Rect {
        self.rect
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn lines(&self) -> Vec<LineInfo> {
        let s = self.layout.scale();
        let text = self.layout.text();
        self.lines
            .iter()
            .zip(&self.dx)
            .map(|(l, d)| {
                let (a, b) = l.text_range;
                let nl = text[a..b].ends_with('\n');
                LineInfo {
                    range: a..if nl { b - 1 } else { b },
                    origin: pos2(d + l.x / s, l.top / s),
                    size: vec2(l.advance / s, (l.bottom - l.top) / s),
                    baseline: l.baseline / s,
                    ends_with_newline: nl,
                }
            })
            .collect()
    }

    fn first_line_height(&self) -> Option<f32> {
        let s = self.layout.scale();
        self.lines.first().map(|l| (l.bottom - l.top) / s)
    }

    fn half_leading(&self) -> f32 {
        let s = self.layout.scale();
        self.lines
            .first()
            .map_or(0.0, |l| 0.5 * ((l.bottom - l.top) - (l.ascent + l.descent)) / s)
    }

    fn clusters(&self) -> Vec<Cluster> {
        let s = self.layout.scale();
        let text = self.layout.text();
        self.layout
            .clusters()
            .into_iter()
            .map(|c| Cluster {
                line: c.line,
                byte: c.text_range.start,
                ch: text[c.text_range.start..].chars().next().unwrap_or(' '),
                x: self.dx.get(c.line).copied().unwrap_or(0.0) + c.x / s,
                advance: c.advance / s,
            })
            .collect()
    }

    fn ink_spans(&self, range: &Range<usize>) -> Vec<InkSpan> {
        let s = self.layout.scale();
        let mut out = Vec::new();
        let mut cur: Option<(usize, InkSpan)> = None;
        for c in self.clusters() {
            let hit = range.contains(&c.byte) && !c.ch.is_control();
            if let Some((line, sp)) = cur
                && (!hit || line != c.line)
            {
                out.push(sp);
                cur = None;
            }
            if !hit {
                continue;
            }
            let l = &self.lines[c.line];
            let (ascent, descent) = (l.ascent / s, l.descent / s);
            let stroke = ((ascent + descent) * 0.06).clamp(1.0, 2.5);
            cur = Some(match cur {
                Some((line, sp)) => (
                    line,
                    InkSpan {
                        x1: c.x + c.advance,
                        ..sp
                    },
                ),
                None => (
                    c.line,
                    InkSpan {
                        x0: c.x,
                        x1: c.x + c.advance,
                        baseline: l.baseline / s,
                        ascent,
                        descent: descent * 0.8,
                        stroke,
                    },
                ),
            });
        }
        if let Some((_, sp)) = cur {
            out.push(sp);
        }
        out
    }

    fn hit(&self, pos: Pos2) -> LocalPos {
        let s = self.layout.scale();
        let line = self.line_at_y(pos.y);
        let d = self.dx.get(line).copied().unwrap_or(0.0);
        self.layout.hit((pos.x - d) * s, pos.y * s)
    }

    fn char_at(&self, pos: Pos2) -> Option<usize> {
        if !self.rect.expand(2.0).contains(pos) {
            return None;
        }
        let line = self.line_at_y(pos.y);
        let cs: Vec<Cluster> = self.clusters().into_iter().filter(|c| c.line == line).collect();
        cs.iter()
            .find(|c| pos.x >= c.x && pos.x < c.x + c.advance)
            .or_else(|| {
                // beside the row: the nearest end cluster
                if cs.first().is_some_and(|c| pos.x < c.x) {
                    cs.first()
                } else {
                    cs.last()
                }
            })
            .map(|c| c.byte)
    }

    fn caret_rect(&self, p: LocalPos) -> Rect {
        self.to_pt(self.layout.caret_rect(p))
    }

    fn selection_rects(&self, a: LocalPos, b: LocalPos) -> Vec<Rect> {
        self.layout
            .selection_rects(a, b)
            .into_iter()
            .map(|r| self.to_pt(r))
            .filter(|r| r.width() > 0.0)
            .collect()
    }

    fn source_map(&self) -> Option<&super::SourceMap> {
        self.map.as_deref()
    }

    fn cost(&self) -> usize {
        // glyph runs are built on first paint; before that (cache insertion) count an estimate
        // of what they will hold: one glyph per character, two runs per line
        let estimate = self.layout.text().chars().count() * std::mem::size_of::<markise_text::Glyph>()
            + self.lines.len() * 2 * std::mem::size_of::<GlyphRun>();
        let runs = self.runs.get().map_or(estimate, |runs| {
            runs.iter()
                .map(|r| {
                    std::mem::size_of::<GlyphRun>()
                        + r.glyphs.capacity() * std::mem::size_of::<markise_text::Glyph>()
                        + r.coords.capacity() * std::mem::size_of::<i16>()
                })
                .sum::<usize>()
        });
        self.layout.cost()
            + runs
            + self.colors.len() * 4
            + self.dx.len() * 4
            + self.lines.len() * std::mem::size_of::<markise_text::LineInfo>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
