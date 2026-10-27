//! PDF back end on Krilla: real text (subsetted fonts with Unicode mapping, so text is
//! selectable and searchable), link annotations (URLs and internal destinations), an outline
//! from headings, document metadata, tagged PDF from the layout's structure tree (headers and
//! footers as artifacts), and a fallback that draws the glyphs of a font Krilla cannot embed as
//! outlines.
//!
//! The export *structure* follows WordCraft's PDF crate (text as text, outline, tagging,
//! bad-font fallback; see `docs/research/wordcraft-reuse.md` §13), written against upstream
//! Krilla.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU16;
use std::sync::Arc;

use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::color::rgb;
use krilla::destination::XyzDestination;
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::outline::{Outline, OutlineNode};
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;
use krilla::tagging::{
    ArtifactType, ContentTag, Identifier, ListNumbering, Node, SpanTag, TableHeaderScope, Tag, TagGroup, TagKind,
    TagTree,
};
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::{Document, SerializeSettings};
use markise_export::job::{Cancelled, JobControl};
use markise_theme::Rgba;

use crate::image::{ImageFormat, LoadedImage};
use crate::layout::{Artifact, Item, Laid, LinkTarget, OutlineEntry, R, SChild, SKind, StructTree, TextItem};

/// Document metadata.
#[derive(Clone, Debug, Default)]
pub struct PdfMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub language: Option<String>,
    /// Today as `YYYY-MM-DD` (creation date).
    pub date: Option<String>,
}

/// Errors writing a PDF.
#[derive(Debug)]
pub enum PdfError {
    Cancelled,
    Krilla(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Cancelled => f.write_str("export cancelled"),
            PdfError::Krilla(e) => write!(f, "PDF writer: {e}"),
        }
    }
}

impl std::error::Error for PdfError {}

impl From<Cancelled> for PdfError {
    fn from(_: Cancelled) -> Self {
        PdfError::Cancelled
    }
}

type FontKey = (u64, u32, Vec<i16>);

struct Writer<'a> {
    laid: &'a Laid,
    fonts: HashMap<FontKey, Option<Font>>,
    outlined: HashSet<FontKey>,
    tagged: bool,
    leaf_ids: HashMap<u32, Identifier>,
    annot_ids: HashMap<usize, Vec<Identifier>>,
}

fn color(c: Rgba) -> rgb::Color {
    let [r, g, b, _] = c.0;
    rgb::Color::new(r, g, b)
}

fn opacity(c: Rgba) -> NormalizedF32 {
    NormalizedF32::new(c.0[3] as f32 / 255.0).unwrap_or(NormalizedF32::ONE)
}

fn fill(c: Rgba) -> Fill {
    Fill {
        paint: color(c).into(),
        opacity: opacity(c),
        rule: Default::default(),
    }
}

fn rect_path(r: &R, radius: f32) -> Option<krilla::geom::Path> {
    let mut pb = PathBuilder::new();
    let rad = radius.min(r.w() / 2.0).min(r.h() / 2.0).max(0.0);
    if rad <= 0.01 {
        pb.push_rect(Rect::from_ltrb(r.x0, r.y0, r.x1, r.y1)?);
    } else {
        // rounded rectangle with quadratic corners
        pb.move_to(r.x0 + rad, r.y0);
        pb.line_to(r.x1 - rad, r.y0);
        pb.quad_to(r.x1, r.y0, r.x1, r.y0 + rad);
        pb.line_to(r.x1, r.y1 - rad);
        pb.quad_to(r.x1, r.y1, r.x1 - rad, r.y1);
        pb.line_to(r.x0 + rad, r.y1);
        pb.quad_to(r.x0, r.y1, r.x0, r.y1 - rad);
        pb.line_to(r.x0, r.y0 + rad);
        pb.quad_to(r.x0, r.y0, r.x0 + rad, r.y0);
        pb.close();
    }
    pb.finish()
}

impl Writer<'_> {
    fn font(&mut self, t: &TextItem) -> Option<Font> {
        let key: FontKey = (t.font.id(), t.font.index(), t.coords.clone());
        if let Some(f) = self.fonts.get(&key) {
            return f.clone();
        }
        let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(t.font.shared().data.clone());
        let f = if t.coords.iter().any(|c| *c != 0) {
            let axes = user_coords(t.font.data(), t.font.index(), &t.coords);
            Font::new_variable(data.into(), t.font.index(), &axes)
        } else {
            Font::new(data.into(), t.font.index())
        };
        self.fonts.insert(key, f.clone());
        f
    }

    fn key_of(&self, font: &Font) -> Option<FontKey> {
        self.fonts
            .iter()
            .find(|(_, f)| f.as_ref() == Some(font))
            .map(|(k, _)| k.clone())
    }

    fn draw_text(&mut self, s: &mut Surface, t: &TextItem) {
        let Some(font) = self.font(t) else { return };
        let key: FontKey = (t.font.id(), t.font.index(), t.coords.clone());
        let outlined = self.outlined.contains(&key);
        let size = t.size;
        if t.glyphs.is_empty() || size <= 0.0 {
            return;
        }
        let first = t.glyphs[0];
        let base = first.y;
        let glyphs: Vec<KrillaGlyph> = t
            .glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let next_x = t.glyphs.get(i + 1).map(|n| n.x).unwrap_or(g.x + g.advance);
                let range = t.clusters.get(i).cloned().unwrap_or(0..0);
                let range = if range.end <= t.text.len() && range.start <= range.end {
                    range
                } else {
                    0..0
                };
                KrillaGlyph::new(
                    GlyphId::new(g.id),
                    (next_x - g.x) / size,
                    0.0,
                    (base - g.y) / size,
                    0.0,
                    range,
                    None,
                )
            })
            .collect();
        s.set_fill(Some(fill(t.color)));
        if t.embolden {
            s.set_stroke(Some(Stroke {
                paint: color(t.color).into(),
                width: size * 0.03,
                opacity: opacity(t.color),
                ..Default::default()
            }));
        }
        let skewed = t.skew.filter(|a| a.abs() > 0.01);
        if let Some(a) = skewed {
            let k = a.to_radians().tan();
            s.push_transform(&Transform::from_row(1.0, 0.0, -k, 1.0, k * base, 0.0));
        }
        s.draw_glyphs(Point::from_xy(first.x, base), &glyphs, font, &t.text, size, outlined);
        if skewed.is_some() {
            s.pop();
        }
        if t.embolden {
            s.set_stroke(None);
        }
    }

    fn draw_item(&mut self, s: &mut Surface, item: &Item) {
        match item {
            Item::Fill { rect, color: c, radius } => {
                if let Some(p) = rect_path(rect, *radius) {
                    s.set_fill(Some(fill(*c)));
                    s.set_stroke(None);
                    s.draw_path(&p);
                }
            }
            Item::Stroke {
                rect,
                color: c,
                width,
                radius,
            } => {
                if let Some(p) = rect_path(rect, *radius) {
                    s.set_fill(None);
                    s.set_stroke(Some(Stroke {
                        paint: color(*c).into(),
                        width: *width,
                        opacity: opacity(*c),
                        ..Default::default()
                    }));
                    s.draw_path(&p);
                    s.set_stroke(None);
                }
            }
            Item::Line {
                from,
                to,
                color: c,
                width,
            } => {
                let mut pb = PathBuilder::new();
                pb.move_to(from.0, from.1);
                pb.line_to(to.0, to.1);
                if let Some(p) = pb.finish() {
                    s.set_fill(None);
                    s.set_stroke(Some(Stroke {
                        paint: color(*c).into(),
                        width: *width,
                        opacity: opacity(*c),
                        ..Default::default()
                    }));
                    s.draw_path(&p);
                    s.set_stroke(None);
                }
            }
            Item::Text(t) => self.draw_text(s, t),
            Item::Image { rect, image, .. } => {
                if let (Some(img), Some(size)) = (krilla_image(image), Size::from_wh(rect.w(), rect.h())) {
                    s.push_transform(&Transform::from_translate(rect.x0, rect.y0));
                    s.draw_image(img, size);
                    s.pop();
                }
            }
        }
    }

    fn page(&mut self, doc: &mut Document, index: usize) {
        let laid = self.laid;
        let p = &laid.pages[index];
        let Some(settings) = PageSettings::from_wh(laid.page_width, laid.page_height) else {
            return;
        };
        let mut page = doc.start_page_with(settings);
        {
            let mut s = page.surface();
            if laid.page_bg.0[3] > 0 && laid.page_bg != Rgba::hex(0xffffff) {
                let bg = Item::Fill {
                    rect: R::new(0.0, 0.0, laid.page_width, laid.page_height),
                    color: laid.page_bg,
                    radius: 0.0,
                };
                self.tagged_artifact(&mut s, ArtifactType::Other, |w, s| w.draw_item(s, &bg));
            }
            for item in &p.items {
                let leaf = match item {
                    Item::Text(t) => {
                        if let Some(a) = t.artifact {
                            let kind = match a {
                                Artifact::Header => ArtifactType::Header,
                                Artifact::Footer => ArtifactType::Footer,
                                Artifact::Decoration => ArtifactType::Other,
                            };
                            self.tagged_artifact(&mut s, kind, |w, s| w.draw_item(s, item));
                            continue;
                        }
                        t.leaf
                    }
                    Item::Image { leaf, .. } => *leaf,
                    _ => None,
                };
                match leaf {
                    Some(l) if self.tagged => {
                        let tag = match item {
                            Item::Image { .. } => ContentTag::Other,
                            _ => ContentTag::Span(SpanTag::empty()),
                        };
                        let id = s.start_tagged(tag);
                        self.draw_item(&mut s, item);
                        s.end_tagged();
                        self.leaf_ids.insert(l, id);
                    }
                    _ => self.tagged_artifact(&mut s, ArtifactType::Other, |w, s| w.draw_item(s, item)),
                }
            }
            s.finish();
        }
        for link in &p.links {
            let Some(rect) = Rect::from_ltrb(link.rect.x0, link.rect.y0, link.rect.x1, link.rect.y1) else {
                continue;
            };
            let target = match &link.target {
                LinkTarget::Url(u) => {
                    if !safe_uri(u) {
                        continue;
                    }
                    Target::Action(Action::Link(LinkAction::new(u.clone())))
                }
                LinkTarget::Anchor(a) => match laid.anchors.get(a) {
                    Some(&(pg, y)) => Target::Destination(krilla::destination::Destination::Xyz(XyzDestination::new(
                        pg,
                        Point::from_xy(0.0, y),
                    ))),
                    None => continue,
                },
            };
            let annot = Annotation::new_link(LinkAnnotation::new(rect, target), None);
            if self.tagged {
                let id = page.add_tagged_annotation(annot);
                self.annot_ids.entry(link.node).or_default().push(id);
            } else {
                page.add_annotation(annot);
            }
        }
        page.finish();
    }

    fn tagged_artifact(&mut self, s: &mut Surface, kind: ArtifactType, f: impl FnOnce(&mut Self, &mut Surface)) {
        if self.tagged {
            s.start_tagged(ContentTag::Artifact(krilla::tagging::Artifact::new(kind, None)));
            f(self, s);
            s.end_tagged();
        } else {
            f(self, s);
        }
    }

    fn tag_tree(&mut self, tree: &StructTree) -> TagTree {
        let mut root = TagTree::new();
        for c in &tree.nodes[0].children {
            if let Some(n) = self.node(tree, c) {
                root.push(n);
            }
        }
        root
    }

    fn node(&mut self, tree: &StructTree, c: &SChild) -> Option<Node> {
        match c {
            SChild::Leaf(l) => self.leaf_ids.remove(l).map(Node::Leaf),
            SChild::Node(i) => {
                let n = &tree.nodes[*i];
                let mut children: Vec<Node> = Vec::new();
                if matches!(n.kind, SKind::Link) {
                    for id in self.annot_ids.remove(i).unwrap_or_default() {
                        children.push(Node::Leaf(id));
                    }
                }
                for ch in &n.children {
                    if let Some(x) = self.node(tree, ch) {
                        children.push(x);
                    }
                }
                if children.is_empty() {
                    return None;
                }
                let kind: TagKind = match &n.kind {
                    SKind::Document => Tag::Div.into(),
                    SKind::P => Tag::P.into(),
                    SKind::H(l, title) => Tag::Hn(
                        NonZeroU16::new(u16::from(*l).max(1)).unwrap_or(NonZeroU16::MIN),
                        Some(title.clone()),
                    )
                    .into(),
                    SKind::Code => Tag::Code.into(),
                    SKind::BlockQuote => Tag::BlockQuote.into(),
                    SKind::Div => Tag::Div.into(),
                    SKind::L(ordered) => Tag::L(if *ordered {
                        ListNumbering::Decimal
                    } else {
                        ListNumbering::Disc
                    })
                    .into(),
                    SKind::LI => Tag::LI.into(),
                    SKind::Lbl => Tag::Lbl.into(),
                    SKind::LBody => Tag::LBody.into(),
                    SKind::Table => Tag::Table.into(),
                    SKind::TR => Tag::TR.into(),
                    SKind::TH => Tag::TH(TableHeaderScope::Column).into(),
                    SKind::TD => Tag::TD.into(),
                    SKind::Figure(alt) => Tag::Figure(Some(alt.clone())).into(),
                    SKind::Link => Tag::Link.into(),
                    SKind::Note => Tag::Note.into(),
                    SKind::Toc => Tag::TOC.into(),
                    SKind::TocI => Tag::TOCI.into(),
                };
                Some(Node::Group(TagGroup::with_children(kind, children)))
            }
        }
    }
}

/// URI schemes allowed in link annotations (URL vetting).
fn safe_uri(u: &str) -> bool {
    let lower = u.trim().to_ascii_lowercase();
    match lower.split_once(':') {
        Some((scheme, _)) if scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) => {
            matches!(scheme, "http" | "https" | "mailto" | "ftp" | "file" | "tel")
        }
        _ => true, // relative reference
    }
}

fn krilla_image(img: &Arc<LoadedImage>) -> Option<Image> {
    let data = || -> krilla::Data { img.bytes.clone().into() };
    let r = match img.format {
        ImageFormat::Png => Image::from_png(data(), true),
        ImageFormat::Jpeg => Image::from_jpeg(data(), true),
        ImageFormat::Gif => Image::from_gif(data(), true),
        ImageFormat::Webp => Image::from_webp(data(), true),
        ImageFormat::Other => Err(String::new()),
    };
    r.ok().or_else(|| {
        img.rgba()
            .map(|px| Image::from_rgba8((*px).clone(), img.width, img.height))
    })
}

/// Normalised variation coordinates → user-space axis values (avar ignored).
fn user_coords(data: &[u8], index: u32, coords: &[i16]) -> Vec<(krilla::text::Tag, f32)> {
    use skrifa::MetadataProvider;
    let Ok(font) = skrifa::FontRef::from_index(data, index) else {
        return Vec::new();
    };
    font.axes()
        .iter()
        .zip(coords)
        .map(|(axis, &c)| {
            let n = c as f32 / 16384.0;
            let v = if n < 0.0 {
                axis.default_value() + n * (axis.default_value() - axis.min_value())
            } else {
                axis.default_value() + n * (axis.max_value() - axis.default_value())
            };
            let tag = axis.tag().to_be_bytes();
            (krilla::text::Tag::new(&tag), v)
        })
        .collect()
}

fn outline(entries: &[OutlineEntry]) -> Outline {
    // nest by level with a stack of open nodes
    fn attach(stack: &mut Vec<(u8, OutlineNode)>, root: &mut Outline, level: u8) {
        while let Some((l, _)) = stack.last() {
            if *l < level {
                break;
            }
            let (_, node) = stack.pop().unwrap();
            match stack.last_mut() {
                Some((_, parent)) => parent.push_child(node),
                None => root.push_child(node),
            }
        }
    }
    let mut root = Outline::new();
    let mut stack: Vec<(u8, OutlineNode)> = Vec::new();
    for e in entries {
        attach(&mut stack, &mut root, e.level);
        let title = if e.title.trim().is_empty() {
            "(untitled)".to_string()
        } else {
            e.title.clone()
        };
        stack.push((
            e.level,
            OutlineNode::new(title, XyzDestination::new(e.page, Point::from_xy(0.0, e.y))),
        ));
    }
    attach(&mut stack, &mut root, 0);
    root
}

fn creation_date(s: &str) -> Option<krilla::metadata::DateTime> {
    let mut it = s.split('-').map(|p| p.parse::<u16>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    Some(krilla::metadata::DateTime::new(y).month(m as u8).day(d as u8))
}

/// Writes `laid` as a PDF.
pub fn write_pdf(laid: &Laid, meta: &PdfMeta, tagged: bool, control: &JobControl) -> Result<Vec<u8>, PdfError> {
    let mut outlined: HashSet<FontKey> = HashSet::new();
    // Retry with fonts Krilla could not embed drawn as outlines (bounded).
    for _attempt in 0..32 {
        let settings = SerializeSettings {
            enable_tagging: tagged,
            ..Default::default()
        };
        let mut doc = Document::new_with(settings);
        let mut w = Writer {
            laid,
            fonts: HashMap::new(),
            outlined: outlined.clone(),
            tagged,
            leaf_ids: HashMap::new(),
            annot_ids: HashMap::new(),
        };
        let n = laid.pages.len().max(1);
        for i in 0..laid.pages.len() {
            control.check()?;
            w.page(&mut doc, i);
            control.set_progress(0.7 + 0.25 * (i + 1) as f32 / n as f32);
        }
        if !laid.outline.is_empty() {
            doc.set_outline(outline(&laid.outline));
        }
        let mut m = Metadata::new()
            .creator("Markise".into())
            .producer("Markise (krilla)".into());
        if let Some(t) = &meta.title {
            m = m.title(t.clone());
        }
        if let Some(a) = &meta.author {
            m = m.authors(vec![a.clone()]);
        }
        if let Some(s) = &meta.subject {
            m = m.description(s.clone());
        }
        if let Some(l) = &meta.language {
            m = m.language(l.clone());
        }
        if let Some(d) = meta.date.as_deref().and_then(creation_date) {
            m = m.creation_date(d);
        }
        doc.set_metadata(m);
        if tagged {
            let tree = w.tag_tree(&laid.tree);
            doc.set_tag_tree(tree.with_lang(meta.language.clone()));
        }
        control.check()?;
        let finished = doc.finish();
        control.check()?;
        match finished {
            Ok(bytes) => return Ok(bytes),
            Err(KrillaError::Font(font, msg)) => match w.key_of(&font) {
                Some(k) if !outlined.contains(&k) => {
                    outlined.insert(k);
                    continue;
                }
                _ => return Err(PdfError::Krilla(format!("font error: {msg}"))),
            },
            Err(e) => return Err(PdfError::Krilla(format!("{e:?}"))),
        }
    }
    Err(PdfError::Krilla("too many unusable fonts".into()))
}
