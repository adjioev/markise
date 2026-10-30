//! Paginated layout of the export model ([`ExDoc`]) into pages of positioned drawing items.
//!
//! Text is shaped and broken by `markise-text` (parley, ADR 0005); this module turns the
//! resulting lines and glyph runs into page items, splits the flow into pages (line
//! granularity; table rows, images and rules are atomic), draws block decorations (code and
//! math boxes, quote and alert bars) per page segment, records anchors (headings, footnotes)
//! for internal links and the outline, and builds a structure tree for tagged PDF.
//!
//! Coordinates: output units (PDF points or PNG CSS pixels), origin at the page's top-left,
//! y down. Theme lengths (CSS px) are multiplied by [`LayoutOptions::unit`].

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use markise_export::job::{Cancelled, JobControl};
use markise_export::model::{ExBlock, ExDoc, ExInline, ExItem, inline_text};
use markise_export::vars::Vars;
use markise_md::{AlertKind, Align};
use markise_text::{Family, FontHandle, Generic, Slope, StyledText, TextEngine, TextLayout, TextStyle};
use markise_theme::{BoxStyle, Rgba, Theme, TokenRun};

use crate::image::{LoadError, LoadedImage, load_image_capped};

/// Axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl R {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> R {
        R { x0, y0, x1, y1 }
    }
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// A positioned glyph (absolute page coordinates of its origin on the baseline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
}

/// Header/footer content is an artifact, not part of the document structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artifact {
    Header,
    Footer,
    /// Decorative (backgrounds, bars, rules, list bullets drawn as shapes).
    Decoration,
}

/// A run of glyphs in one font and color.
#[derive(Clone, Debug)]
pub struct TextItem {
    pub font: FontHandle,
    pub size: f32,
    pub coords: Vec<i16>,
    pub embolden: bool,
    pub skew: Option<f32>,
    pub color: Rgba,
    pub glyphs: Vec<PGlyph>,
    /// The paragraph text the clusters index into.
    pub text: Arc<str>,
    /// Source cluster of every glyph.
    pub clusters: Vec<Range<usize>>,
    /// Structure-tree leaf (tagged PDF), or an artifact kind.
    pub leaf: Option<u32>,
    pub artifact: Option<Artifact>,
}

/// One drawing operation.
#[derive(Clone, Debug)]
pub enum Item {
    Fill {
        rect: R,
        color: Rgba,
        radius: f32,
    },
    Stroke {
        rect: R,
        color: Rgba,
        width: f32,
        radius: f32,
    },
    Line {
        from: (f32, f32),
        to: (f32, f32),
        color: Rgba,
        width: f32,
    },
    Text(TextItem),
    Image {
        rect: R,
        image: Arc<LoadedImage>,
        leaf: Option<u32>,
    },
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Url(String),
    /// Internal anchor (heading id, `fn-N`).
    Anchor(String),
}

/// A clickable area.
#[derive(Clone, Debug)]
pub struct LinkBox {
    pub rect: R,
    pub target: LinkTarget,
    /// Structure node (`Link`) the annotation belongs to.
    pub node: usize,
}

/// One laid-out page.
#[derive(Clone, Debug, Default)]
pub struct Page {
    /// Items in paint order.
    pub items: Vec<Item>,
    pub links: Vec<LinkBox>,
}

/// An outline (bookmark) entry.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    pub level: u8,
    pub title: String,
    pub page: usize,
    pub y: f32,
}

/// Structure kinds (a subset of the PDF standard structure types).
#[derive(Clone, Debug, PartialEq)]
pub enum SKind {
    Document,
    P,
    H(u8, String),
    Code,
    BlockQuote,
    Div,
    L(bool),
    LI,
    Lbl,
    LBody,
    Table,
    TR,
    TH,
    TD,
    Figure(String),
    Link,
    Note,
    Toc,
    TocI,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SChild {
    Node(usize),
    Leaf(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SNode {
    pub kind: SKind,
    pub children: Vec<SChild>,
}

/// Logical structure for tagged PDF; node 0 is the document.
#[derive(Clone, Debug, PartialEq)]
pub struct StructTree {
    pub nodes: Vec<SNode>,
}

impl StructTree {
    fn new() -> Self {
        StructTree {
            nodes: vec![SNode {
                kind: SKind::Document,
                children: Vec::new(),
            }],
        }
    }

    fn add(&mut self, parent: usize, kind: SKind) -> usize {
        let id = self.nodes.len();
        self.nodes.push(SNode {
            kind,
            children: Vec::new(),
        });
        self.nodes[parent].children.push(SChild::Node(id));
        id
    }
}

/// Layout settings.
#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub page_width: f32,
    /// `f32::INFINITY` for one continuous page (image export).
    pub page_height: f32,
    /// top, right, bottom, left
    pub margins: [f32; 4],
    /// Output units per theme CSS pixel (0.75 for PDF points, 1 for PNG).
    pub unit: f32,
    pub theme: Theme,
    /// Folder relative image paths are resolved against.
    pub base_dir: Option<PathBuf>,
    /// Start every top-level heading (h1) except the first block on a new page.
    pub break_before_h1: bool,
    /// Header/footer templates (A17): one line each, `${…}` variables.
    pub header: Option<String>,
    pub footer: Option<String>,
    /// Variables for header/footer (`pageNo` / `totalPages` are added per page).
    pub vars: Vars,
}

/// Layout result.
#[derive(Clone, Debug)]
pub struct Laid {
    pub pages: Vec<Page>,
    pub page_width: f32,
    /// Page height (for continuous layout: the content height).
    pub page_height: f32,
    pub outline: Vec<OutlineEntry>,
    pub anchors: HashMap<String, (usize, f32)>,
    pub tree: StructTree,
    /// Background color of the page.
    pub page_bg: Rgba,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------------------------

/// Inline style flags while flattening inline content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Inl {
    bold: bool,
    italic: bool,
    mono: bool,
    strike: bool,
    underline: bool,
    sup: bool,
    sub: bool,
    color: Option<Rgba>,
    bg: Option<Rgba>,
    link: Option<usize>,
}

/// Per-style data the text crate does not carry.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StyleMeta {
    color: Rgba,
    bg: Option<Rgba>,
    underline: bool,
    strike: bool,
    shift: f32,
    link: Option<usize>,
}

/// A styled paragraph ready to shape.
struct Para {
    st: StyledText,
    metas: Vec<StyleMeta>,
}

/// Text context inherited by nested blocks.
#[derive(Clone, Copy, Debug)]
struct Ctx {
    color: Rgba,
    size: f32,
    mono: bool,
    bold: bool,
    italic: bool,
    line_height: f32,
}

/// One shaped paragraph with its per-line data.
struct Shaped {
    layout: TextLayout,
    metas: Vec<StyleMeta>,
    text: Arc<str>,
}

/// An open decorated block.
struct Frame {
    page: usize,
    y: f32,
    x0: f32,
    x1: f32,
    deco: Deco,
    depth: i32,
}

#[derive(Clone, Copy)]
enum Deco {
    Box(BoxStyle),
    Bar { color: Rgba, width: f32, bg: Rgba },
}

/// A pending list marker, drawn at the first line of the item.
struct Marker {
    text: String,
    right: f32,
    task: Option<bool>,
    node: usize,
}

struct Flow<'a> {
    eng: &'a mut TextEngine,
    o: &'a LayoutOptions,
    t: &'a Theme,
    doc: &'a ExDoc,
    control: &'a JobControl,
    pages: Vec<Vec<(i32, Item)>>,
    links: Vec<Vec<LinkBox>>,
    y: f32,
    top: f32,
    bottom: f32,
    x0: f32,
    x1: f32,
    pending: f32,
    at_page_top: bool,
    frames: Vec<Frame>,
    marker: Option<Marker>,
    tree: StructTree,
    node: usize,
    next_leaf: u32,
    link_targets: Vec<LinkTarget>,
    link_nodes: HashMap<usize, usize>,
    outline: Vec<OutlineEntry>,
    anchors: HashMap<String, (usize, f32)>,
    body_fams: Vec<Family>,
    mono_fams: Vec<Family>,
    heading_fams: Vec<Family>,
    warnings: Vec<String>,
    blocks_done: usize,
    blocks_total: usize,
    max_y: f32,
    /// Encoded bytes of the images loaded so far (bounded by [`IMAGE_BYTES_BUDGET`]).
    image_bytes: usize,
    /// Loaded images by source (each file decoded once per export).
    images: HashMap<String, Result<Arc<LoadedImage>, String>>,
    /// Content drawn now is decorative (repeated table headers): no structure, artifact tags.
    artifact_mode: bool,
}

/// Encoded image bytes one PDF/PNG export may hold (beyond: placeholders with a warning).
pub const IMAGE_BYTES_BUDGET: usize = 256 << 20;

#[cfg(not(test))]
fn image_budget() -> usize {
    IMAGE_BYTES_BUDGET
}

#[cfg(test)]
thread_local! {
    static TEST_IMAGE_BUDGET: std::cell::Cell<usize> = const { std::cell::Cell::new(IMAGE_BYTES_BUDGET) };
}

#[cfg(test)]
fn image_budget() -> usize {
    TEST_IMAGE_BUDGET.with(|b| b.get())
}

const Z_DECO: i32 = 0;
const Z_CONTENT: i32 = 1000;

fn families(stack: &[String], generic: Generic) -> Vec<Family> {
    let mut v: Vec<Family> = stack.iter().map(|s| Family::parse(s)).collect();
    if !v.iter().any(|f| matches!(f, Family::Generic(_))) {
        v.push(Family::Generic(generic));
    }
    v
}

fn rgba_u32(c: Rgba) -> u32 {
    let [r, g, b, a] = c.0;
    u32::from_be_bytes([r, g, b, a])
}

/// Lays out `doc`.
pub fn layout(eng: &mut TextEngine, doc: &ExDoc, o: &LayoutOptions, control: &JobControl) -> Result<Laid, Cancelled> {
    let t = &o.theme;
    let [mt, mr, mb, ml] = o.margins;
    let bottom = if o.page_height.is_finite() {
        o.page_height - mb
    } else {
        f32::INFINITY
    };
    let mut f = Flow {
        eng,
        o,
        t,
        doc,
        control,
        pages: vec![Vec::new()],
        links: vec![Vec::new()],
        y: mt,
        top: mt,
        bottom,
        x0: ml,
        x1: o.page_width - mr,
        pending: 0.0,
        at_page_top: true,
        frames: Vec::new(),
        marker: None,
        tree: StructTree::new(),
        node: 0,
        next_leaf: 0,
        link_targets: Vec::new(),
        link_nodes: HashMap::new(),
        outline: Vec::new(),
        anchors: HashMap::new(),
        body_fams: families(&t.fonts.body, Generic::SansSerif),
        mono_fams: families(&t.fonts.mono, Generic::Monospace),
        heading_fams: if t.fonts.heading.is_empty() {
            families(&t.fonts.body, Generic::SansSerif)
        } else {
            families(&t.fonts.heading, Generic::SansSerif)
        },
        warnings: Vec::new(),
        blocks_done: 0,
        blocks_total: count_blocks(&doc.blocks).max(1),
        max_y: mt,
        artifact_mode: false,
        images: HashMap::new(),
        image_bytes: 0,
    };
    let ctx = f.base_ctx();
    for (i, b) in doc.blocks.iter().enumerate() {
        if o.break_before_h1 && i > 0 && matches!(b, ExBlock::Heading { level: 1, .. }) && !f.at_page_top {
            f.new_page();
        }
        f.block(b, ctx)?;
    }
    if !doc.footnotes.is_empty() {
        f.footnotes(ctx)?;
    }
    control.check()?;
    let page_height = if o.page_height.is_finite() {
        o.page_height
    } else {
        (f.max_y.max(f.y) + mb).ceil()
    };
    let mut pages: Vec<Page> = f
        .pages
        .into_iter()
        .zip(f.links)
        .map(|(mut items, links)| {
            items.sort_by_key(|(z, _)| *z);
            Page {
                items: items.into_iter().map(|(_, it)| it).collect(),
                links,
            }
        })
        .collect();
    // Header and footer (A17), now that the page count is known.
    let total = pages.len();
    let small = TextStyle {
        families: f.body_fams.clone(),
        size: t.font_size * 0.8 * o.unit,
        color: rgba_u32(t.meta_text),
        ..TextStyle::default()
    };
    for (i, page) in pages.iter_mut().enumerate() {
        let mut vars = o.vars.clone();
        vars.set("pageNo", (i + 1).to_string());
        vars.set("totalPages", total.to_string());
        for (tpl, art) in [(&o.header, Artifact::Header), (&o.footer, Artifact::Footer)] {
            let Some(tpl) = tpl else { continue };
            let text = vars.expand(tpl);
            let text = text.lines().next().unwrap_or("").trim().to_string();
            if text.is_empty() {
                continue;
            }
            let st = StyledText::plain(text.as_str(), small.clone());
            let lay = f.eng.layout(&st, None, 1.0);
            let (w, h) = lay.size();
            let x = ml + ((o.page_width - ml - mr) - w).max(0.0) / 2.0;
            let y = match art {
                Artifact::Header => (mt - h) / 2.0,
                _ => page_height - mb + (mb - h) / 2.0,
            }
            .max(0.0);
            let shaped = Shaped {
                layout: lay,
                metas: vec![StyleMeta {
                    color: t.meta_text,
                    bg: None,
                    underline: false,
                    strike: false,
                    shift: 0.0,
                    link: None,
                }],
                text: text.as_str().into(),
            };
            let lines = shaped.layout.lines();
            let runs = shaped.layout.glyph_runs();
            let clusters = shaped.layout.glyph_clusters();
            for (run, cl) in runs.into_iter().zip(clusters) {
                let dy = y - lines.first().map(|l| l.top).unwrap_or(0.0);
                page.items.push(Item::Text(TextItem {
                    font: run.font.clone(),
                    size: run.size,
                    coords: run.coords.clone(),
                    embolden: run.embolden,
                    skew: run.skew,
                    color: t.meta_text,
                    glyphs: run
                        .glyphs
                        .iter()
                        .map(|g| PGlyph {
                            id: g.id,
                            x: g.x + x,
                            y: g.y + dy,
                            advance: g.advance,
                        })
                        .collect(),
                    text: shaped.text.clone(),
                    clusters: cl,
                    leaf: None,
                    artifact: Some(art),
                }));
            }
        }
    }
    Ok(Laid {
        pages,
        page_width: o.page_width,
        page_height,
        outline: f.outline,
        anchors: f.anchors,
        tree: f.tree,
        page_bg: t.page_bg,
        warnings: f.warnings,
    })
}

fn count_blocks(b: &[ExBlock]) -> usize {
    b.len()
}

impl Flow<'_> {
    fn u(&self, v: f32) -> f32 {
        v * self.o.unit
    }

    fn base_ctx(&self) -> Ctx {
        Ctx {
            color: self.t.text,
            size: self.u(self.t.font_size),
            mono: false,
            bold: false,
            italic: false,
            line_height: self.t.line_height,
        }
    }

    fn page(&self) -> usize {
        self.pages.len() - 1
    }

    fn new_page(&mut self) {
        self.max_y = self.max_y.max(self.y);
        self.pages.push(Vec::new());
        self.links.push(Vec::new());
        self.y = self.top;
        self.pending = 0.0;
        self.at_page_top = true;
    }

    /// Vertical space before the next item (collapsing margins; dropped at a page top).
    fn space(&mut self, s: f32) {
        self.pending = self.pending.max(s);
    }

    /// Reserves `h` of vertical space, breaking the page when it does not fit; returns the
    /// (page, top y) of the reserved box.
    fn place(&mut self, h: f32) -> (usize, f32) {
        let gap = if self.at_page_top { 0.0 } else { self.pending };
        if self.y + gap + h > self.bottom && !self.at_page_top {
            self.new_page();
        } else {
            self.y += gap;
        }
        self.pending = 0.0;
        self.at_page_top = false;
        let top = self.y;
        self.y += h;
        self.max_y = self.max_y.max(self.y);
        (self.page(), top)
    }

    /// Breaks the page unless `h` more fits (keep-with-next for headings).
    fn ensure(&mut self, h: f32) {
        if !self.at_page_top && self.y + self.pending + h > self.bottom {
            self.new_page();
        }
    }

    fn push(&mut self, page: usize, z: i32, item: Item) {
        self.pages[page].push((z, item));
    }

    fn leaf(&mut self, node: usize) -> u32 {
        let id = self.next_leaf;
        self.next_leaf += 1;
        self.tree.nodes[node].children.push(SChild::Leaf(id));
        id
    }

    fn progress(&mut self) -> Result<(), Cancelled> {
        self.blocks_done += 1;
        self.control
            .set_progress(0.05 + 0.6 * (self.blocks_done as f32 / self.blocks_total as f32).min(1.0));
        self.control.check()
    }

    // ---- decorations

    fn begin_frame(&mut self, deco: Deco, inset: f32) {
        let gap = if self.at_page_top { 0.0 } else { self.pending };
        // the box starts where its first content would start
        if self.y + gap > self.bottom - 2.0 * inset && !self.at_page_top {
            self.new_page();
        } else {
            self.y += gap;
        }
        self.pending = 0.0;
        self.at_page_top = false;
        self.frames.push(Frame {
            page: self.page(),
            y: self.y,
            x0: self.x0,
            x1: self.x1,
            deco,
            depth: self.frames.len() as i32,
        });
        self.y += inset;
    }

    fn end_frame(&mut self, inset: f32) {
        let Some(fr) = self.frames.pop() else { return };
        self.y = (self.y + inset).min(self.bottom.max(self.y));
        self.max_y = self.max_y.max(self.y);
        let end_page = self.page();
        for p in fr.page..=end_page {
            let y0 = if p == fr.page { fr.y } else { self.top };
            let y1 = if p == end_page { self.y } else { self.bottom };
            if y1 <= y0 {
                continue;
            }
            let z = Z_DECO + fr.depth;
            match fr.deco {
                Deco::Box(b) => {
                    let rect = R::new(fr.x0, y0, fr.x1, y1);
                    let radius = self.u(b.radius);
                    self.push(
                        p,
                        z,
                        Item::Fill {
                            rect,
                            color: b.background,
                            radius,
                        },
                    );
                    if let Some(c) = b.border {
                        self.push(
                            p,
                            z,
                            Item::Stroke {
                                rect,
                                color: c,
                                width: self.u(1.0),
                                radius,
                            },
                        );
                    }
                }
                Deco::Bar { color, width, bg } => {
                    if bg.0[3] > 0 {
                        self.push(
                            p,
                            z,
                            Item::Fill {
                                rect: R::new(fr.x0, y0, fr.x1, y1),
                                color: bg,
                                radius: 0.0,
                            },
                        );
                    }
                    self.push(
                        p,
                        z,
                        Item::Fill {
                            rect: R::new(fr.x0, y0, fr.x0 + width, y1),
                            color,
                            radius: 0.0,
                        },
                    );
                }
            }
        }
    }

    // ---- text

    fn style(&self, ctx: &Ctx, inl: &Inl, heading: bool) -> (TextStyle, StyleMeta) {
        let mono = ctx.mono || inl.mono;
        let mut size = ctx.size;
        if inl.mono && !ctx.mono {
            size *= self.t.mono_scale;
        }
        if inl.sup || inl.sub {
            size *= 0.75;
        }
        let families = if mono {
            self.mono_fams.clone()
        } else if heading {
            self.heading_fams.clone()
        } else {
            self.body_fams.clone()
        };
        let color = inl.color.unwrap_or(ctx.color);
        let mut st = TextStyle {
            families,
            size,
            weight: if ctx.bold || inl.bold { 700.0 } else { 400.0 },
            slope: if ctx.italic || inl.italic {
                Slope::Italic
            } else {
                Slope::Normal
            },
            color: rgba_u32(color),
            line_height: Some(ctx.line_height),
            ..TextStyle::default()
        };
        if mono {
            st = st.no_ligatures();
        }
        let shift = if inl.sup {
            -ctx.size * 0.35
        } else if inl.sub {
            ctx.size * 0.15
        } else {
            0.0
        };
        (
            st,
            StyleMeta {
                color,
                bg: inl.bg,
                underline: inl.underline,
                strike: inl.strike,
                shift,
                link: inl.link,
            },
        )
    }

    fn new_para(&self) -> Para {
        Para {
            st: StyledText {
                text: String::new(),
                styles: Vec::new(),
                runs: Vec::new(),
                base_dir: Default::default(),
                inline_objects: Vec::new(),
            },
            metas: Vec::new(),
        }
    }

    fn push_text(&self, p: &mut Para, ctx: &Ctx, inl: &Inl, heading: bool, s: &str) {
        if s.is_empty() {
            return;
        }
        let (st, meta) = self.style(ctx, inl, heading);
        let start = p.st.text.len();
        p.st.text.push_str(s);
        let end = p.st.text.len();
        // reuse an identical style
        let id = match p
            .st
            .styles
            .iter()
            .zip(&p.metas)
            .position(|(x, m)| *x == st && *m == meta)
        {
            Some(i) => i,
            None => {
                p.st.styles.push(st);
                p.metas.push(meta);
                p.st.styles.len() - 1
            }
        };
        p.st.runs.push((start..end, id as u16));
    }

    /// Flattens inline content into `p`; images are returned separately (block images).
    fn inlines(&mut self, p: &mut Para, ctx: &Ctx, inl: Inl, heading: bool, nodes: &[ExInline]) {
        let t = self.t;
        for n in nodes {
            match n {
                ExInline::Text(s) => self.push_text(p, ctx, &inl, heading, s),
                ExInline::Code(s) => {
                    let i = Inl {
                        mono: true,
                        color: Some(t.inline_code.text),
                        bg: Some(t.inline_code.background),
                        ..inl
                    };
                    self.push_text(p, ctx, &i, heading, s);
                }
                ExInline::Emph(c) => self.inlines(
                    p,
                    ctx,
                    Inl {
                        italic: true,
                        color: t.inline.em.or(inl.color),
                        ..inl
                    },
                    heading,
                    c,
                ),
                ExInline::Strong(c) => self.inlines(
                    p,
                    ctx,
                    Inl {
                        bold: true,
                        color: t.inline.strong.or(inl.color),
                        ..inl
                    },
                    heading,
                    c,
                ),
                ExInline::Strike(c) => self.inlines(
                    p,
                    ctx,
                    Inl {
                        strike: true,
                        color: t.inline.del.or(inl.color),
                        ..inl
                    },
                    heading,
                    c,
                ),
                ExInline::Underline(c) => self.inlines(p, ctx, Inl { underline: true, ..inl }, heading, c),
                ExInline::Highlight(c) => self.inlines(
                    p,
                    ctx,
                    Inl {
                        bg: Some(t.highlight_bg),
                        color: t.inline.mark_text.or(inl.color),
                        ..inl
                    },
                    heading,
                    c,
                ),
                ExInline::Sub(c) => self.inlines(p, ctx, Inl { sub: true, ..inl }, heading, c),
                ExInline::Sup(c) => self.inlines(p, ctx, Inl { sup: true, ..inl }, heading, c),
                ExInline::Kbd(c) => self.inlines(
                    p,
                    ctx,
                    Inl {
                        mono: true,
                        bg: Some(t.inline_code.background),
                        ..inl
                    },
                    heading,
                    c,
                ),
                ExInline::Link { url, children, .. } => {
                    let target = match url.strip_prefix('#') {
                        Some(a) => LinkTarget::Anchor(a.to_string()),
                        None => LinkTarget::Url(url.clone()),
                    };
                    let idx = self.link_targets.len();
                    self.link_targets.push(target);
                    let i = Inl {
                        link: Some(idx),
                        color: Some(t.link),
                        underline: inl.underline || t.link_underline,
                        ..inl
                    };
                    self.inlines(p, ctx, i, heading, children);
                }
                ExInline::Image { alt, .. } => {
                    // inline images are lifted to block images by the caller; keep the alt text
                    // only when the image could not be lifted (in headings, table cells)
                    if heading {
                        self.push_text(p, ctx, &inl, heading, alt);
                    }
                }
                ExInline::Math(tex) => {
                    let i = Inl {
                        mono: true,
                        color: Some(t.math_block.text),
                        ..inl
                    };
                    self.push_text(p, ctx, &i, heading, tex);
                }
                ExInline::FootnoteRef(num) => {
                    let idx = self.link_targets.len();
                    self.link_targets.push(LinkTarget::Anchor(format!("fn-{num}")));
                    let i = Inl {
                        sup: true,
                        link: Some(idx),
                        color: Some(t.link),
                        ..inl
                    };
                    self.push_text(p, ctx, &i, heading, &num.to_string());
                }
                ExInline::SoftBreak => self.push_text(p, ctx, &inl, heading, " "),
                ExInline::HardBreak => self.push_text(p, ctx, &inl, heading, "\n"),
                ExInline::RawHtml(_) => {}
            }
        }
    }

    fn shape(&mut self, p: Para, width: f32) -> Shaped {
        let text: Arc<str> = p.st.text.as_str().into();
        let layout = self.eng.layout(&p.st, Some(width.max(1.0)), 1.0);
        Shaped {
            layout,
            metas: p.metas,
            text,
        }
    }

    /// Draws the lines of `s` flowing through pages; `align` 0 = start, 0.5 = center, 1 = end.
    fn flow_text(&mut self, s: &Shaped, x: f32, width: f32, align: f32, node: usize) {
        let lines = s.layout.lines();
        let runs = s.layout.glyph_runs();
        let clusters = s.layout.glyph_clusters();
        let mut by_line: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
        for (i, r) in runs.iter().enumerate() {
            if let Some(v) = by_line.get_mut(r.line) {
                v.push(i);
            }
        }
        for (li, line) in lines.iter().enumerate() {
            if li % 64 == 63 && self.control.is_cancelled() {
                return; // the caller's next progress check reports the cancellation
            }
            let h = line.bottom - line.top;
            let (page, top) = self.place(h);
            let dy = top - line.top;
            let dx = x + (width - line.advance).max(0.0) * align - if align > 0.0 { line.x } else { 0.0 };
            if li == 0
                && let Some(m) = self.marker.take()
            {
                self.draw_marker(m, page, line.baseline + dy, line.ascent);
            }
            for &ri in &by_line[li] {
                self.draw_run(s, &runs[ri], &clusters[ri], page, dx, dy, node);
            }
        }
    }

    /// Draws the lines of `s` lying within `clip` (layout y range) at a fixed offset (table
    /// cells, possibly split across pages).
    #[allow(clippy::too_many_arguments)]
    fn text_at(
        &mut self,
        s: &Shaped,
        page: usize,
        x: f32,
        y: f32,
        width: f32,
        align: f32,
        node: usize,
        clip: (f32, f32),
    ) {
        let lines = s.layout.lines();
        let runs = s.layout.glyph_runs();
        let clusters = s.layout.glyph_clusters();
        for (ri, run) in runs.iter().enumerate() {
            let line = &lines[run.line];
            if line.top < clip.0 - 0.01 || line.bottom > clip.1 + 0.01 {
                continue;
            }
            let dx = x + (width - line.advance).max(0.0) * align - if align > 0.0 { line.x } else { 0.0 };
            self.draw_run(s, run, &clusters[ri], page, dx, y, node);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &mut self,
        s: &Shaped,
        run: &markise_text::GlyphRun,
        clusters: &[Range<usize>],
        page: usize,
        dx: f32,
        dy: f32,
        node: usize,
    ) {
        if run.glyphs.is_empty() {
            return;
        }
        let meta = s.metas.get(run.style as usize).copied().unwrap_or(StyleMeta {
            color: self.t.text,
            bg: None,
            underline: false,
            strike: false,
            shift: 0.0,
            link: None,
        });
        let glyphs: Vec<PGlyph> = run
            .glyphs
            .iter()
            .map(|g| PGlyph {
                id: g.id,
                x: g.x + dx,
                y: g.y + dy + meta.shift,
                advance: g.advance,
            })
            .collect();
        let gx0 = glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
        let gx1 = glyphs.iter().map(|g| g.x + g.advance).fold(f32::NEG_INFINITY, f32::max);
        let base = run.baseline + dy + meta.shift;
        let size = run.size;
        if let Some(bg) = meta.bg {
            let r = R::new(
                gx0 - size * 0.12,
                base - size * 0.92,
                gx1 + size * 0.12,
                base + size * 0.28,
            );
            self.push(
                page,
                Z_CONTENT - 1,
                Item::Fill {
                    rect: r,
                    color: bg,
                    radius: size * 0.15,
                },
            );
        }
        if self.artifact_mode {
            self.push(
                page,
                Z_CONTENT,
                Item::Text(TextItem {
                    font: run.font.clone(),
                    size,
                    coords: run.coords.clone(),
                    embolden: run.embolden,
                    skew: run.skew,
                    color: meta.color,
                    glyphs,
                    text: s.text.clone(),
                    clusters: clusters.to_vec(),
                    leaf: None,
                    artifact: Some(Artifact::Decoration),
                }),
            );
            return;
        }
        // structure: link text lives in a Link node under the paragraph node
        let parent = match meta.link {
            Some(l) => *self
                .link_nodes
                .entry(l)
                .or_insert_with(|| self.tree.add(node, SKind::Link)),
            None => node,
        };
        let leaf = self.leaf(parent);
        if let Some(l) = meta.link
            && let Some(target) = self.link_targets.get(l).cloned()
        {
            let r = R::new(gx0, base - size * 0.95, gx1, base + size * 0.3);
            self.links[page].push(LinkBox {
                rect: r,
                target,
                node: parent,
            });
        }
        let thick = (size * 0.06).max(0.5);
        if meta.underline {
            let y = base + size * 0.12;
            self.push(
                page,
                Z_CONTENT + 1,
                Item::Line {
                    from: (gx0, y),
                    to: (gx1, y),
                    color: meta.color,
                    width: thick,
                },
            );
        }
        if meta.strike {
            let y = base - size * 0.28;
            self.push(
                page,
                Z_CONTENT + 1,
                Item::Line {
                    from: (gx0, y),
                    to: (gx1, y),
                    color: meta.color,
                    width: thick,
                },
            );
        }
        self.push(
            page,
            Z_CONTENT,
            Item::Text(TextItem {
                font: run.font.clone(),
                size,
                coords: run.coords.clone(),
                embolden: run.embolden,
                skew: run.skew,
                color: meta.color,
                glyphs,
                text: s.text.clone(),
                clusters: clusters.to_vec(),
                leaf: Some(leaf),
                artifact: None,
            }),
        );
    }

    fn draw_marker(&mut self, m: Marker, page: usize, baseline: f32, ascent: f32) {
        let t = self.t;
        let size = self.u(t.font_size);
        if let Some(checked) = m.task {
            let s = size * 0.8;
            let x1 = m.right;
            let r = R::new(x1 - s, baseline - ascent * 0.5 - s / 2.0 - size * 0.05, x1, 0.0);
            let r = R::new(r.x0, r.y0, r.x1, r.y0 + s);
            if checked {
                self.push(
                    page,
                    Z_CONTENT,
                    Item::Fill {
                        rect: r,
                        color: t.checkbox_checked,
                        radius: s * 0.2,
                    },
                );
                let w = (s * 0.12).max(0.8);
                let white = Rgba::hex(0xffffff);
                let p1 = (r.x0 + s * 0.22, r.y0 + s * 0.52);
                let p2 = (r.x0 + s * 0.42, r.y0 + s * 0.72);
                let p3 = (r.x0 + s * 0.78, r.y0 + s * 0.3);
                self.push(
                    page,
                    Z_CONTENT + 1,
                    Item::Line {
                        from: p1,
                        to: p2,
                        color: white,
                        width: w,
                    },
                );
                self.push(
                    page,
                    Z_CONTENT + 1,
                    Item::Line {
                        from: p2,
                        to: p3,
                        color: white,
                        width: w,
                    },
                );
            } else {
                self.push(
                    page,
                    Z_CONTENT,
                    Item::Stroke {
                        rect: r,
                        color: t.checkbox_border,
                        width: (s * 0.08).max(0.6),
                        radius: s * 0.2,
                    },
                );
            }
            // the label still carries the state as text for extraction
            let label = if checked { "[x]" } else { "[ ]" };
            let _ = label;
            return;
        }
        let ctx = Ctx {
            color: t.list_marker,
            ..self.base_ctx()
        };
        let mut p = self.new_para();
        self.push_text(&mut p, &ctx, &Inl::default(), false, &m.text);
        let s = self.shape(p, 1000.0);
        let lines = s.layout.lines();
        let Some(l0) = lines.first() else { return };
        let w = l0.advance;
        let dy = baseline - l0.baseline;
        let dx = m.right - w;
        let lbl = self.tree.add(m.node, SKind::Lbl);
        let runs = s.layout.glyph_runs();
        let clusters = s.layout.glyph_clusters();
        for (ri, run) in runs.iter().enumerate() {
            self.draw_run(&s, run, &clusters[ri], page, dx, dy, lbl);
        }
    }

    // ---- blocks

    fn paragraph(&mut self, ctx: Ctx, nodes: &[ExInline], kind: SKind) -> Result<(), Cancelled> {
        // lift images out of the paragraph
        let mut chunk: Vec<ExInline> = Vec::new();
        let mut first = true;
        for n in nodes {
            // images anywhere inside the node (also `**[![a](x)](url)**`) become block images
            // at their position: the text before and after keeps its formatting and order
            let segs = segments(n, None);
            if !segs.iter().any(|s| matches!(s, Seg::Image { .. })) {
                chunk.push(n.clone());
                continue;
            }
            for seg in segs {
                match seg {
                    Seg::Inline(i) => chunk.push(i),
                    Seg::Image { src, alt, link } => {
                        if !chunk.is_empty() && !only_space(&chunk) {
                            self.text_block(ctx, &chunk, kind.clone(), first)?;
                            first = false;
                        }
                        chunk.clear();
                        self.image(&src, &alt, link.as_deref());
                    }
                }
            }
        }
        if !chunk.is_empty() && !only_space(&chunk) {
            self.text_block(ctx, &chunk, kind, first)?;
        } else if let Some(m) = self.marker.take() {
            let (page, y) = self.place(0.0);
            self.draw_marker(m, page, y + ctx.size, ctx.size);
        }
        Ok(())
    }

    fn text_block(&mut self, ctx: Ctx, nodes: &[ExInline], kind: SKind, _first: bool) -> Result<(), Cancelled> {
        let mut p = self.new_para();
        self.inlines(&mut p, &ctx, Inl::default(), false, nodes);
        let (x, w) = (self.x0, self.x1 - self.x0);
        let s = self.shape(p, w);
        let node = self.tree.add(self.node, kind);
        self.flow_text(&s, x, w, 0.0, node);
        Ok(())
    }

    fn image(&mut self, src: &str, alt: &str, link: Option<&str>) {
        let width = self.x1 - self.x0;
        let loaded = match self.images.get(src) {
            Some(r) => r.clone(),
            None => {
                // all images of one export together stay within a memory budget, checked on
                // the file size before anything is read
                let remaining = image_budget().saturating_sub(self.image_bytes) as u64;
                let r = match load_image_capped(self.o.base_dir.as_deref(), self.doc.resource_root(), src, remaining) {
                    Ok(img) => {
                        self.image_bytes += img.bytes.len();
                        Ok(Arc::new(img))
                    }
                    Err(LoadError::OverAllowance(_)) => Err(format!(
                        "images of this export exceed the {} MB limit; shown as a placeholder",
                        image_budget() >> 20
                    )),
                    Err(e) => Err(e.to_string()),
                };
                self.images.insert(src.to_string(), r.clone());
                r
            }
        };
        let parent = match link {
            Some(_) => self.tree.add(self.node, SKind::Link),
            None => self.node,
        };
        let target = link.map(|u| match u.strip_prefix('#') {
            Some(a) => LinkTarget::Anchor(a.to_string()),
            None => LinkTarget::Url(u.to_string()),
        });
        match loaded {
            Ok(img) => {
                let (pw, ph) = (img.width as f32, img.height as f32);
                let mut w = self.u(pw).min(width);
                let mut h = if pw > 0.0 { w * ph / pw } else { 0.0 };
                let max_h = if self.bottom.is_finite() {
                    (self.bottom - self.top) * 0.95
                } else {
                    f32::INFINITY
                };
                if h > max_h {
                    w *= max_h / h;
                    h = max_h;
                }
                let (page, y) = self.place(h);
                let fig = self.tree.add(parent, SKind::Figure(alt.to_string()));
                let leaf = self.leaf(fig);
                let x = self.x0;
                let rect = R::new(x, y, x + w, y + h);
                self.push(
                    page,
                    Z_CONTENT,
                    Item::Image {
                        rect,
                        image: img,
                        leaf: Some(leaf),
                    },
                );
                if let Some(target) = target {
                    self.links[page].push(LinkBox {
                        rect,
                        target,
                        node: parent,
                    });
                }
                self.space(self.u(self.t.block_spacing));
            }
            Err(e) => {
                if self.warnings.len() < 50 {
                    self.warnings.push(format!("image {src}: {e}"));
                }
                // placeholder box with the alt text
                let t = self.t;
                let ctx = Ctx {
                    color: t.meta_text,
                    ..self.base_ctx()
                };
                let label = if alt.is_empty() { src } else { alt };
                let b = BoxStyle {
                    background: t.image_placeholder,
                    text: t.meta_text,
                    border: None,
                    padding: 6.0,
                    radius: 3.0,
                };
                let pad = self.u(b.padding);
                self.begin_frame(Deco::Box(b), pad);
                let (ox0, ox1) = (self.x0, self.x1);
                self.x0 += pad;
                self.x1 -= pad;
                let mut p = self.new_para();
                self.push_text(&mut p, &ctx, &Inl::default(), false, &format!("[{label}]"));
                let (x, w) = (self.x0, self.x1 - self.x0);
                let s = self.shape(p, w);
                let fig = self.tree.add(parent, SKind::Figure(alt.to_string()));
                self.flow_text(&s, x, w, 0.0, fig);
                self.x0 = ox0;
                self.x1 = ox1;
                self.end_frame(pad);
                self.space(self.u(t.block_spacing));
            }
        }
    }

    fn with_node<T>(&mut self, kind: SKind, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.node;
        self.node = self.tree.add(saved, kind);
        let r = f(self);
        self.node = saved;
        r
    }

    fn blocks(&mut self, blocks: &[ExBlock], ctx: Ctx, tight: bool) -> Result<(), Cancelled> {
        for b in blocks {
            if tight && let ExBlock::Paragraph(c) = b {
                self.paragraph(ctx, c, SKind::P)?;
                self.space(self.u(self.t.block_spacing) * 0.25);
                continue;
            }
            self.block(b, ctx)?;
        }
        Ok(())
    }

    fn block(&mut self, b: &ExBlock, ctx: Ctx) -> Result<(), Cancelled> {
        let t = self.t;
        let gap = self.u(t.block_spacing);
        match b {
            ExBlock::Paragraph(c) => {
                self.paragraph(ctx, c, SKind::P)?;
                self.space(gap);
            }
            ExBlock::Heading { level, id, content } => self.heading(*level, id, content, ctx),
            ExBlock::Code { text, runs, .. } => {
                let code_ctx = Ctx {
                    color: t.code_block.text,
                    size: self.u(t.mono_size()),
                    mono: true,
                    line_height: 1.45,
                    ..ctx
                };
                self.code_box(t.code_block, code_ctx, text, runs.as_deref(), SKind::Code);
                self.space(gap);
            }
            ExBlock::Math(src) => {
                let c = Ctx {
                    color: t.math_block.text,
                    size: self.u(t.mono_size()),
                    mono: true,
                    line_height: 1.45,
                    ..ctx
                };
                self.code_box(t.math_block, c, src, None, SKind::Div);
                self.space(gap);
            }
            ExBlock::Diagram { source, .. } => {
                let c = Ctx {
                    color: t.math_block.text,
                    size: self.u(t.mono_size()),
                    mono: true,
                    line_height: 1.45,
                    ..ctx
                };
                self.code_box(t.math_block, c, source, None, SKind::Code);
                self.space(gap);
            }
            ExBlock::RawHtml(h) => {
                let text = strip_tags(h);
                if !text.trim().is_empty() {
                    let c = Ctx {
                        color: t.meta_text,
                        ..ctx
                    };
                    self.paragraph(c, &[ExInline::Text(text.trim().to_string())], SKind::P)?;
                    self.space(gap);
                }
            }
            ExBlock::Quote(inner) => {
                let c = Ctx {
                    color: t.quote_text,
                    italic: ctx.italic || t.quote_italic,
                    ..ctx
                };
                self.bar_block(t.quote_bar, t.quote_bg, None, inner, c)?;
                self.space(gap);
            }
            ExBlock::Alert { kind, blocks } => {
                let col = t.alert[alert_index(*kind)];
                self.bar_block(col, Rgba::TRANSPARENT, Some((*kind, col)), blocks, ctx)?;
                self.space(gap);
            }
            ExBlock::List {
                ordered,
                start,
                tight,
                items,
            } => {
                self.list(*ordered, *start, *tight, items, ctx)?;
                self.space(gap);
            }
            ExBlock::Table { aligns, header, rows } => {
                self.table(aligns, header, rows, ctx);
                self.space(gap);
            }
            ExBlock::Rule => {
                self.space(gap);
                let (page, y) = self.place(self.u(1.0));
                let (x0, x1) = (self.x0, self.x1);
                self.push(
                    page,
                    Z_DECO,
                    Item::Line {
                        from: (x0, y),
                        to: (x1, y),
                        color: t.rule,
                        width: self.u(1.0),
                    },
                );
                self.space(gap * 1.5);
            }
            ExBlock::Toc => {
                self.toc(ctx);
                self.space(gap);
            }
        }
        self.progress()
    }

    fn heading(&mut self, level: u8, id: &str, content: &[ExInline], ctx: Ctx) {
        let t = self.t;
        let li = (level.clamp(1, 6) - 1) as usize;
        let hs = t.headings[li];
        let decor = &t.heading_decor[li];
        let c = Ctx {
            color: hs.color,
            size: self.u(hs.size),
            bold: hs.bold,
            italic: decor.italic,
            line_height: 1.3,
            mono: false,
        };
        let _ = ctx;
        self.space(self.u(hs.margin_top));
        let mut p = self.new_para();
        if let Some(pre) = &decor.prefix {
            let i = Inl {
                color: decor.prefix_color,
                ..Inl::default()
            };
            self.push_text(&mut p, &c, &i, true, pre);
        }
        self.inlines(&mut p, &c, Inl::default(), true, content);
        if let Some(suf) = &decor.suffix {
            self.push_text(&mut p, &c, &Inl::default(), true, suf);
        }
        let (x, w) = (self.x0, self.x1 - self.x0);
        let s = self.shape(p, w);
        let (_, h) = s.layout.size();
        // keep with the next two lines
        self.ensure(h + 2.0 * self.u(t.line_px()));
        // anchor at the heading's top
        let title = inline_text(content);
        let node = self.tree.add(self.node, SKind::H(level, title.clone()));
        let anchor_page_y = {
            let gap = if self.at_page_top { 0.0 } else { self.pending };
            (self.page(), self.y + gap)
        };
        self.anchors.insert(id.to_string(), anchor_page_y);
        self.outline.push(OutlineEntry {
            level,
            title,
            page: anchor_page_y.0,
            y: anchor_page_y.1,
        });
        let align = match decor.align {
            markise_theme::TextAlign::Center => 0.5,
            markise_theme::TextAlign::End => 1.0,
            _ => 0.0,
        };
        self.flow_text(&s, x, w, align, node);
        if let Some(rule) = hs.rule {
            let (page, y) = self.place(self.u(4.0));
            let (x0, x1) = (self.x0, self.x1);
            self.push(
                page,
                Z_DECO,
                Item::Line {
                    from: (x0, y + self.u(3.0)),
                    to: (x1, y + self.u(3.0)),
                    color: rule,
                    width: self.u(1.0),
                },
            );
        }
        self.space(self.u(hs.margin_bottom));
    }

    fn code_box(&mut self, b: BoxStyle, ctx: Ctx, text: &str, runs: Option<&[TokenRun]>, kind: SKind) {
        let pad = self.u(b.padding);
        self.begin_frame(Deco::Box(b), pad);
        let (ox0, ox1) = (self.x0, self.x1);
        self.x0 += pad;
        self.x1 -= pad;
        let body = text.strip_suffix('\n').unwrap_or(text);
        let mut p = self.new_para();
        match runs {
            Some(runs) => {
                let mut pos = 0;
                for r in runs {
                    let (s, e) = (r.start as usize, (r.end as usize).min(body.len()));
                    if s < pos || s >= e || !body.is_char_boundary(s) || !body.is_char_boundary(e) {
                        continue;
                    }
                    self.push_text(&mut p, &ctx, &Inl::default(), false, &body[pos..s]);
                    let look = self.t.token_look(r.kind);
                    let i = Inl {
                        color: look.color,
                        bold: look.bold,
                        italic: look.italic,
                        ..Inl::default()
                    };
                    self.push_text(&mut p, &ctx, &i, false, &body[s..e]);
                    pos = e;
                }
                self.push_text(&mut p, &ctx, &Inl::default(), false, &body[pos.min(body.len())..]);
            }
            None => self.push_text(&mut p, &ctx, &Inl::default(), false, body),
        }
        if p.st.text.is_empty() {
            self.push_text(&mut p, &ctx, &Inl::default(), false, " ");
        }
        let (x, w) = (self.x0, self.x1 - self.x0);
        let s = self.shape(p, w);
        let node = self.tree.add(self.node, kind);
        self.flow_text(&s, x, w, 0.0, node);
        self.x0 = ox0;
        self.x1 = ox1;
        self.end_frame(pad);
    }

    fn bar_block(
        &mut self,
        bar: Rgba,
        bg: Rgba,
        alert: Option<(AlertKind, Rgba)>,
        inner: &[ExBlock],
        ctx: Ctx,
    ) -> Result<(), Cancelled> {
        let t = self.t;
        let width = self.u(if alert.is_some() { 4.0 } else { t.quote_bar_width });
        let indent = self.u(t.quote_indent);
        let vpad = self.u(4.0);
        self.begin_frame(Deco::Bar { color: bar, width, bg }, vpad);
        let (ox0, ox1) = (self.x0, self.x1);
        self.x0 += indent;
        if bg.0[3] > 0 {
            self.x1 -= indent * 0.5;
        }
        let kind = if alert.is_some() { SKind::Div } else { SKind::BlockQuote };
        let r = self.with_node(kind, |f| {
            if let Some((k, col)) = alert {
                let c = Ctx {
                    color: col,
                    bold: true,
                    ..ctx
                };
                f.paragraph(c, &[ExInline::Text(alert_title(k).to_string())], SKind::P)?;
                f.space(f.u(t.block_spacing) * 0.4);
            }
            f.blocks(inner, ctx, false)
        });
        self.pending = 0.0; // inner bottom margin collapses into the bar's padding
        self.x0 = ox0;
        self.x1 = ox1;
        self.end_frame(vpad);
        r
    }

    fn list(&mut self, ordered: bool, start: u64, tight: bool, items: &[ExItem], ctx: Ctx) -> Result<(), Cancelled> {
        let t = self.t;
        let indent = self.u(t.list_indent);
        let depth = self.list_depth();
        let (ox0, ox1) = (self.x0, self.x1);
        let r = self.with_node(SKind::L(ordered), |f| {
            for (i, it) in items.iter().enumerate() {
                let marker_text = if ordered {
                    format!("{}.", start + i as u64)
                } else {
                    ["•", "◦", "▪"][depth % 3].to_string()
                };
                let li = f.tree.add(f.node, SKind::LI);
                f.marker = Some(Marker {
                    text: marker_text,
                    right: ox0 + indent - f.u(6.0),
                    task: it.task,
                    node: li,
                });
                f.x0 = ox0 + indent;
                let saved = f.node;
                f.node = f.tree.add(li, SKind::LBody);
                if it.blocks.is_empty() {
                    let (page, y) = f.place(f.u(t.line_px()));
                    if let Some(m) = f.marker.take() {
                        f.draw_marker(m, page, y + ctx.size, ctx.size);
                    }
                }
                let r = f.blocks(&it.blocks, ctx, tight);
                f.node = saved;
                f.marker = None;
                r?;
                if !tight {
                    f.space(f.u(t.block_spacing));
                }
            }
            Ok(())
        });
        self.x0 = ox0;
        self.x1 = ox1;
        r
    }

    fn list_depth(&self) -> usize {
        let mut d = 0;
        let mut x = self.x0 - self.o.margins[3];
        let step = self.u(self.t.list_indent);
        while x > step * 0.5 && d < 10 {
            x -= step;
            d += 1;
        }
        d
    }

    fn table(&mut self, aligns: &[Align], header: &[Vec<ExInline>], rows: &[Vec<Vec<ExInline>>], ctx: Ctx) {
        let t = self.t;
        let ncols = aligns.len().max(1);
        let pad = self.u(t.table_cell_padding);
        let avail = self.x1 - self.x0;
        // shape every cell twice: natural (no wrap) and minimum widths
        let build = |f: &mut Self, cell: &[ExInline], head: bool| {
            let c = Ctx { bold: head, ..ctx };
            let mut p = f.new_para();
            f.inlines(&mut p, &c, Inl::default(), true, cell);
            p
        };
        let mut nat = vec![0f32; ncols];
        let mut minw = vec![0f32; ncols];
        let mut all_rows: Vec<(bool, &[Vec<ExInline>])> = vec![(true, header)];
        all_rows.extend(rows.iter().map(|r| (false, r.as_slice())));
        for (head, row) in &all_rows {
            for ci in 0..ncols {
                let cell = row.get(ci).map(Vec::as_slice).unwrap_or(&[]);
                let p = build(self, cell, *head);
                let natural = self.eng.layout(&p.st, None, 1.0).size().0;
                let min = self.eng.layout(&p.st, Some(0.0), 1.0).size().0;
                nat[ci] = nat[ci].max(natural);
                minw[ci] = minw[ci].max(min);
            }
        }
        let inner_avail = (avail - 2.0 * pad * ncols as f32).max(ncols as f32 * 10.0);
        let widths = column_widths(&nat, &minw, inner_avail);
        let col_x: Vec<f32> = widths
            .iter()
            .scan(self.x0, |x, w| {
                let s = *x;
                *x += w + 2.0 * pad;
                Some(s)
            })
            .collect();
        let table_w: f32 = widths.iter().map(|w| w + 2.0 * pad).sum();
        let table_node = self.tree.add(self.node, SKind::Table);
        let mut shaped_rows: Vec<(bool, Vec<Shaped>, f32)> = Vec::new();
        for (head, row) in &all_rows {
            let mut shaped = Vec::new();
            let mut row_h: f32 = 0.0;
            for (ci, w) in widths.iter().enumerate() {
                let cell = row.get(ci).map(Vec::as_slice).unwrap_or(&[]);
                let p = build(self, cell, *head);
                let s = self.shape(p, w.max(1.0));
                row_h = row_h.max(s.layout.size().1);
                shaped.push(s);
            }
            let row_h = row_h.max(self.u(t.line_px()) * 0.8) + 2.0 * pad * 0.6;
            shaped_rows.push((*head, shaped, row_h));
        }
        // keep the header with the start of the first body row
        let line_h = self.u(t.line_px());
        let header_h = shaped_rows.first().map(|r| r.2).unwrap_or(0.0);
        let second = shaped_rows.get(1).map(|r| r.2.min(3.0 * line_h)).unwrap_or(0.0);
        self.ensure(header_h + second);
        let vpad = pad * 0.6;
        // the header is repeated on continuation pages only when it leaves room for content
        let repeat_h = if header_h + line_h + 2.0 * vpad <= (self.bottom - self.top) * 0.5 {
            header_h
        } else {
            0.0
        };
        for ri in 0..shaped_rows.len() {
            if self.control.is_cancelled() {
                return;
            }
            let head = shaped_rows[ri].0;
            let lines: Vec<Vec<(f32, f32)>> = shaped_rows[ri]
                .1
                .iter()
                .map(|s| s.layout.lines().iter().map(|l| (l.top, l.bottom)).collect())
                .collect();
            let nodes: Vec<usize> = {
                let tr = self.tree.add(table_node, SKind::TR);
                (0..ncols)
                    .map(|_| self.tree.add(tr, if head { SKind::TH } else { SKind::TD }))
                    .collect()
            };
            // next undrawn line per cell
            let mut next: Vec<usize> = vec![0; ncols];
            let mut first_fragment = true;
            // a page break just happened: the next fragment must make progress
            let mut just_broke = false;
            loop {
                if self.control.is_cancelled() {
                    return;
                }
                let done = next.iter().zip(&lines).all(|(n, l)| *n >= l.len());
                if done && !first_fragment {
                    break;
                }
                let gap = if self.at_page_top { 0.0 } else { self.pending };
                let room = self.bottom - self.y - gap - 2.0 * vpad;
                let rest_h = |next: &[usize]| -> f32 {
                    next.iter()
                        .zip(&lines)
                        .map(|(&n, l)| match (l.get(n), l.last()) {
                            (Some(a), Some(z)) => z.1 - a.0,
                            _ => 0.0,
                        })
                        .fold(0.0, f32::max)
                };
                let whole = rest_h(&next);
                // the part of each cell that fits on this page
                let mut upto: Vec<usize> = next.clone();
                let page_room = self.bottom - self.top - 2.0 * vpad - repeat_h;
                if whole > room && whole <= page_room && !self.at_page_top && !just_broke {
                    // fits on a fresh page: move it there instead of splitting
                    self.table_page_break(
                        ri > 0,
                        &shaped_rows[0].1,
                        repeat_h,
                        &col_x,
                        &widths,
                        pad,
                        aligns,
                        table_node,
                        table_w,
                    );
                    just_broke = true;
                    continue;
                }
                if whole <= room {
                    upto = lines.iter().map(Vec::len).collect();
                } else {
                    for (ci, l) in lines.iter().enumerate() {
                        let Some(start) = l.get(next[ci]).map(|x| x.0) else {
                            continue;
                        };
                        let mut k = next[ci];
                        while k < l.len() && l[k].1 - start <= room {
                            k += 1;
                        }
                        upto[ci] = k;
                    }
                    let progress = upto.iter().zip(&next).any(|(u, n)| u > n);
                    if !progress && !self.at_page_top && !just_broke {
                        self.table_page_break(
                            ri > 0,
                            &shaped_rows[0].1,
                            repeat_h,
                            &col_x,
                            &widths,
                            pad,
                            aligns,
                            table_node,
                            table_w,
                        );
                        continue;
                    }
                    if !progress {
                        // a single line taller than the remaining page: draw it anyway
                        for (ci, l) in lines.iter().enumerate() {
                            if next[ci] < l.len() {
                                upto[ci] = next[ci] + 1;
                            }
                        }
                    }
                }
                let frag: Vec<(f32, f32)> = (0..ncols)
                    .map(|ci| {
                        let l = &lines[ci];
                        match (l.get(next[ci]), upto[ci].checked_sub(1).and_then(|u| l.get(u))) {
                            (Some(a), Some(z)) if upto[ci] > next[ci] => (a.0, z.1),
                            _ => (0.0, 0.0),
                        }
                    })
                    .collect();
                let content_h = frag.iter().map(|(a, z)| z - a).fold(0.0, f32::max);
                let h = if upto.iter().zip(&lines).all(|(u, l)| *u >= l.len()) && first_fragment {
                    shaped_rows[ri].2
                } else {
                    content_h + 2.0 * vpad
                };
                let (page, y) = self.place(h);
                self.draw_row(
                    &shaped_rows[ri].1,
                    &frag,
                    page,
                    y,
                    h,
                    &col_x,
                    &widths,
                    pad,
                    aligns,
                    head,
                    ri,
                    &nodes,
                    table_w,
                );
                first_fragment = false;
                next = upto;
                if next.iter().zip(&lines).all(|(n, l)| *n >= l.len()) {
                    break;
                }
                self.table_page_break(
                    ri > 0,
                    &shaped_rows[0].1,
                    repeat_h,
                    &col_x,
                    &widths,
                    pad,
                    aligns,
                    table_node,
                    table_w,
                );
                just_broke = true;
            }
            // a later row that does not fit starts a new page with the header repeated
            if let Some(nextrow) = shaped_rows.get(ri + 1) {
                let gap = if self.at_page_top { 0.0 } else { self.pending };
                let min_h = nextrow.2.min(line_h + 2.0 * vpad);
                if self.y + gap + min_h > self.bottom && !self.at_page_top {
                    self.table_page_break(
                        true,
                        &shaped_rows[0].1,
                        repeat_h,
                        &col_x,
                        &widths,
                        pad,
                        aligns,
                        table_node,
                        table_w,
                    );
                }
            }
        }
    }

    /// New page in the middle of a table: the header row is repeated as an artifact.
    #[allow(clippy::too_many_arguments)]
    fn table_page_break(
        &mut self,
        repeat: bool,
        header: &[Shaped],
        header_h: f32,
        col_x: &[f32],
        widths: &[f32],
        pad: f32,
        aligns: &[Align],
        table_node: usize,
        table_w: f32,
    ) {
        self.new_page();
        if !repeat || header_h <= 0.0 || !self.bottom.is_finite() {
            return;
        }
        let (hp, hy) = self.place(header_h);
        let frag: Vec<(f32, f32)> = header
            .iter()
            .map(|s| {
                let l = s.layout.lines();
                (
                    l.first().map(|x| x.top).unwrap_or(0.0),
                    l.last().map(|x| x.bottom).unwrap_or(0.0),
                )
            })
            .collect();
        let nodes = vec![table_node; header.len()];
        self.artifact_mode = true;
        self.draw_row(
            header, &frag, hp, hy, header_h, col_x, widths, pad, aligns, true, 0, &nodes, table_w,
        );
        self.artifact_mode = false;
    }

    /// Draws one fragment of a row: cell `ci` shows its lines within `frag[ci]` (layout y).
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        cells: &[Shaped],
        frag: &[(f32, f32)],
        page: usize,
        y: f32,
        h: f32,
        col_x: &[f32],
        widths: &[f32],
        pad: f32,
        aligns: &[Align],
        head: bool,
        ri: usize,
        nodes: &[usize],
        table_w: f32,
    ) {
        let t = self.t;
        let x0 = col_x.first().copied().unwrap_or(self.x0);
        let bg = if head {
            Some(t.table_header_bg)
        } else if ri.is_multiple_of(2) {
            Some(t.table_stripe_bg)
        } else {
            None
        };
        if let Some(bg) = bg {
            self.push(
                page,
                Z_DECO,
                Item::Fill {
                    rect: R::new(x0, y, x0 + table_w, y + h),
                    color: bg,
                    radius: 0.0,
                },
            );
        }
        let bw = self.u(1.0);
        for (ci, s) in cells.iter().enumerate() {
            let cx = col_x[ci];
            let cw = widths[ci] + 2.0 * pad;
            self.push(
                page,
                Z_DECO + 1,
                Item::Stroke {
                    rect: R::new(cx, y, cx + cw, y + h),
                    color: t.table_border,
                    width: bw,
                    radius: 0.0,
                },
            );
            let align = match aligns.get(ci) {
                Some(Align::Center) => 0.5,
                Some(Align::Right) => 1.0,
                _ => 0.0,
            };
            let (from, to) = frag.get(ci).copied().unwrap_or((0.0, 0.0));
            if to <= from {
                continue;
            }
            let node = nodes.get(ci).copied().unwrap_or(0);
            self.text_at(
                s,
                page,
                cx + pad,
                y + pad * 0.6 - from,
                widths[ci],
                align,
                node,
                (from, to),
            );
        }
    }

    fn toc(&mut self, ctx: Ctx) {
        let t = self.t;
        // the model's heading list is built once per export (`ExDoc::build`); borrow it
        // rather than copying it for every `[toc]` (issue #29)
        let doc = self.doc;
        let saved = self.node;
        self.node = self.tree.add(saved, SKind::Toc);
        let (ox0, ox1) = (self.x0, self.x1);
        for h in &doc.headings {
            self.x0 = ox0 + self.u(18.0) * (h.level.saturating_sub(1)) as f32;
            let idx = self.link_targets.len();
            self.link_targets.push(LinkTarget::Anchor(h.id.clone()));
            let mut p = self.new_para();
            let i = Inl {
                link: Some(idx),
                color: Some(t.link),
                ..Inl::default()
            };
            self.push_text(&mut p, &ctx, &i, false, &h.text);
            let (x, w) = (self.x0, self.x1 - self.x0);
            let s = self.shape(p, w);
            let node = self.tree.add(self.node, SKind::TocI);
            self.flow_text(&s, x, w, 0.0, node);
        }
        self.x0 = ox0;
        self.x1 = ox1;
        self.node = saved;
    }

    fn footnotes(&mut self, ctx: Ctx) -> Result<(), Cancelled> {
        let t = self.t;
        let gap = self.u(t.block_spacing);
        self.space(gap * 2.0);
        let (page, y) = self.place(self.u(1.0));
        let (x0, x1) = (self.x0, self.x1);
        self.push(
            page,
            Z_DECO,
            Item::Line {
                from: (x0, y),
                to: (x0 + (x1 - x0) * 0.33, y),
                color: t.rule,
                width: self.u(1.0),
            },
        );
        self.space(gap);
        let c = Ctx {
            size: ctx.size * 0.88,
            color: t.meta_text,
            ..ctx
        };
        let notes = self.doc.footnotes.clone();
        let saved = self.node;
        let indent = self.u(t.list_indent);
        for (num, blocks) in &notes {
            self.node = self.tree.add(saved, SKind::Note);
            let anchor = (self.page(), self.y + if self.at_page_top { 0.0 } else { self.pending });
            self.anchors.insert(format!("fn-{num}"), anchor);
            self.marker = Some(Marker {
                text: format!("{num}."),
                right: x0 + indent - self.u(6.0),
                task: None,
                node: self.node,
            });
            self.x0 = x0 + indent;
            self.blocks(blocks, c, true)?;
            self.marker = None;
            self.x0 = x0;
        }
        self.node = saved;
        Ok(())
    }
}

/// Auto table layout: natural widths when they fit; otherwise columns whose natural width fits
/// their fair share keep it and the rest share the remaining space between their minimum and
/// natural widths.
fn column_widths(nat: &[f32], minw: &[f32], avail: f32) -> Vec<f32> {
    let n = nat.len();
    if nat.iter().sum::<f32>() <= avail {
        return nat.to_vec();
    }
    let mut fixed = vec![false; n];
    let mut remaining = avail;
    loop {
        let open: Vec<usize> = (0..n).filter(|&i| !fixed[i]).collect();
        if open.is_empty() {
            break;
        }
        let share = remaining / open.len() as f32;
        let small: Vec<usize> = open.iter().copied().filter(|&i| nat[i] <= share).collect();
        if small.is_empty() {
            break;
        }
        for i in small {
            fixed[i] = true;
            remaining -= nat[i];
        }
    }
    let open: Vec<usize> = (0..n).filter(|&i| !fixed[i]).collect();
    let mut w: Vec<f32> = nat.to_vec();
    let min_sum: f32 = open.iter().map(|&i| minw[i]).sum();
    if min_sum >= remaining {
        // even the minimum widths do not fit: shrink proportionally (text overflows by words)
        let total_min: f32 = minw.iter().sum::<f32>().max(1e-3);
        return minw.iter().map(|m| m / total_min * avail).collect();
    }
    let flex: f32 = open.iter().map(|&i| nat[i] - minw[i]).sum::<f32>().max(1e-3);
    let extra = remaining - min_sum;
    for &i in &open {
        w[i] = minw[i] + (nat[i] - minw[i]) / flex * extra;
    }
    w
}

/// Inline content split at images, in document order.
#[derive(Clone, Debug, PartialEq)]
enum Seg {
    Inline(ExInline),
    Image {
        src: String,
        alt: String,
        link: Option<String>,
    },
}

/// Splits `n` into ordered inline/image segments. Formatting and links around a split are
/// repeated on each side (`**a ![i](x) b**` → `**a**`, image, `**b**`); an image keeps its
/// innermost enclosing link.
fn segments(n: &ExInline, link: Option<&str>) -> Vec<Seg> {
    fn split_children(c: &[ExInline], link: Option<&str>, rewrap: &dyn Fn(Vec<ExInline>) -> ExInline) -> Vec<Seg> {
        let mut out = Vec::new();
        let mut run: Vec<ExInline> = Vec::new();
        for x in c {
            for seg in segments(x, link) {
                match seg {
                    Seg::Inline(i) => run.push(i),
                    img => {
                        if !run.is_empty() {
                            out.push(Seg::Inline(rewrap(std::mem::take(&mut run))));
                        }
                        out.push(img);
                    }
                }
            }
        }
        if !run.is_empty() {
            out.push(Seg::Inline(rewrap(run)));
        }
        out
    }
    match n {
        ExInline::Image { src, alt, .. } => vec![Seg::Image {
            src: src.clone(),
            alt: alt.clone(),
            link: link.map(str::to_string),
        }],
        ExInline::Link { url, title, children } => split_children(children, Some(url), &|c| ExInline::Link {
            url: url.clone(),
            title: title.clone(),
            children: c,
        }),
        ExInline::Emph(c) => split_children(c, link, &ExInline::Emph),
        ExInline::Strong(c) => split_children(c, link, &ExInline::Strong),
        ExInline::Strike(c) => split_children(c, link, &ExInline::Strike),
        ExInline::Highlight(c) => split_children(c, link, &ExInline::Highlight),
        ExInline::Underline(c) => split_children(c, link, &ExInline::Underline),
        ExInline::Sub(c) => split_children(c, link, &ExInline::Sub),
        ExInline::Sup(c) => split_children(c, link, &ExInline::Sup),
        ExInline::Kbd(c) => split_children(c, link, &ExInline::Kbd),
        other => vec![Seg::Inline(other.clone())],
    }
}

fn only_space(v: &[ExInline]) -> bool {
    v.iter().all(|n| match n {
        ExInline::Text(t) => t.trim().is_empty(),
        ExInline::SoftBreak | ExInline::HardBreak | ExInline::RawHtml(_) => true,
        _ => false,
    })
}

fn alert_index(k: AlertKind) -> usize {
    match k {
        AlertKind::Note => 0,
        AlertKind::Tip => 1,
        AlertKind::Important => 2,
        AlertKind::Warning => 3,
        AlertKind::Caution => 4,
    }
}

fn alert_title(k: AlertKind) -> &'static str {
    match k {
        AlertKind::Note => "Note",
        AlertKind::Tip => "Tip",
        AlertKind::Important => "Important",
        AlertKind::Warning => "Warning",
        AlertKind::Caution => "Caution",
    }
}

/// Text content of raw HTML (tags removed, common entities decoded).
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(src: &str) -> ExInline {
        ExInline::Image {
            src: src.into(),
            alt: src.into(),
            title: None,
        }
    }

    fn t(s: &str) -> ExInline {
        ExInline::Text(s.into())
    }

    // Codex review: images nested in formatting/links keep their link; text order is kept.
    #[test]
    fn segments_keep_order_formatting_and_links() {
        let n = ExInline::Strong(vec![
            t("see "),
            ExInline::Link {
                url: "https://example.org".into(),
                title: None,
                children: vec![ExInline::Emph(vec![img("logo.png")])],
            },
        ]);
        assert_eq!(
            segments(&n, None),
            vec![
                Seg::Inline(ExInline::Strong(vec![t("see ")])),
                Seg::Image {
                    src: "logo.png".into(),
                    alt: "logo.png".into(),
                    link: Some("https://example.org".into())
                },
            ]
        );
        // `**before ![a](a.png) mid ![b](b.png) after**`
        let n = ExInline::Strong(vec![t("before "), img("a.png"), t(" mid "), img("b.png"), t(" after")]);
        let segs = segments(&n, None);
        let kinds: Vec<String> = segs
            .iter()
            .map(|s| match s {
                Seg::Inline(ExInline::Strong(c)) => inline_text(c).trim().to_string(),
                Seg::Image { src, .. } => format!("[{src}]"),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, ["before", "[a.png]", "mid", "[b.png]", "after"]);
        assert_eq!(segments(&img("x.png"), None).len(), 1);
    }

    // self-review (Codex round 9): images of one export share a memory budget.
    #[test]
    fn image_memory_budget_applies_across_images() {
        let d = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_fn(64, 64, |x, y| image::Rgba([x as u8, y as u8, 7, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut md = String::new();
        for i in 0..6 {
            std::fs::write(d.path().join(format!("i{i}.png")), &png).unwrap();
            md.push_str(&format!("![i](i{i}.png)\n\n"));
        }
        TEST_IMAGE_BUDGET.with(|b| b.set(png.len() * 3 + 10));
        let doc = ExDoc::build(&markise_md::parse(&md), &Default::default());
        let o = LayoutOptions {
            page_width: 400.0,
            page_height: f32::INFINITY,
            margins: [10.0; 4],
            unit: 1.0,
            theme: Theme::light(),
            base_dir: Some(d.path().to_path_buf()),
            break_before_h1: false,
            header: None,
            footer: None,
            vars: Vars::new(),
        };
        let laid = layout(&mut TextEngine::default(), &doc, &o, &JobControl::new()).unwrap();
        TEST_IMAGE_BUDGET.with(|b| b.set(IMAGE_BYTES_BUDGET));
        let images = laid.pages[0]
            .items
            .iter()
            .filter(|i| matches!(i, Item::Image { .. }))
            .count();
        assert_eq!(images, 3);
        assert_eq!(laid.warnings.len(), 3, "{:?}", laid.warnings);
    }
}
