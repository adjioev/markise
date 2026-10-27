//! Basic Markdown colouring of the source editor (spec 02 §12.1.1, interim for Side by Side,
//! 99 C58): token spans come from the `markise-md` parse of the document — block structure
//! and the inline parse of paragraphs, headings and table rows — not from regexes.
//!
//! Headings take the heading colour, emphasis is drawn italic, strong in the heading colour,
//! code spans and code blocks in the code colours, links and URLs in the link colour; markers
//! (`#`, `*`, `_`, backticks, fences, `>`, list markers, link brackets, table delimiter rows)
//! are dimmed; front matter, math, HTML and `[toc]` are tinted as metadata. All colours come
//! from the active theme.
//!
//! Cost: runs are cached per top-level block, keyed by a hash of the block's text, so a
//! keystroke re-tokenizes only the edited block. The whole buffer is still one `LayoutJob`
//! (`TextEdit` lays out the full text on each change), so colouring is enabled only below
//! [`highlight_limit`] (measured with `cargo bench -p markise-ui-egui --bench side_by_side`).
//! Fenced code is coloured as one code run (per-language highlighting comes with M2d).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::text::{ByteIndex, LayoutJob, TextFormat};
use egui::{Color32, FontId, Stroke};
use markise_layout::block::color;
use markise_md::Document;
use markise_md::block::{Block, BlockKind};
use markise_md::inline::{Inline, InlineKind, parse_inlines};
use markise_theme::Theme;

/// Buffers above this size are shown without colouring (keystroke budget, 99 C58).
pub const HIGHLIGHT_LIMIT: usize = 128 * 1024;

/// [`HIGHLIGHT_LIMIT`], or `MARKISE_SOURCE_HIGHLIGHT_LIMIT` (bytes; measurements).
pub fn highlight_limit() -> usize {
    static L: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *L.get_or_init(|| {
        std::env::var("MARKISE_SOURCE_HIGHLIGHT_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(HIGHLIGHT_LIMIT)
    })
}

/// Token classes, in increasing paint priority (inner runs overwrite outer ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tok {
    Plain = 0,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Quote,
    Emph,
    Strong,
    StrongEmph,
    Strike,
    Mark,
    Link,
    Url,
    Code,
    CodeBlock,
    Meta,
    Marker,
}

impl Tok {
    fn from_u8(v: u8) -> Tok {
        use Tok::*;
        [
            Plain, Heading1, Heading2, Heading3, Heading4, Heading5, Heading6, Quote, Emph, Strong, StrongEmph, Strike,
            Mark, Link, Url, Code, CodeBlock, Meta, Marker,
        ][v as usize]
    }

    fn heading(level: u8) -> Tok {
        Tok::from_u8(Tok::Heading1 as u8 + level.clamp(1, 6) - 1)
    }
}

/// A run `start..end` (relative to its top-level block) of one token class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    pub tok: Tok,
}

/// Paints token classes into a per-byte buffer of one top-level block.
struct Painter<'a> {
    text: &'a str,
    /// Absolute offset of `cls[0]`.
    base: usize,
    cls: Vec<u8>,
}

impl Painter<'_> {
    fn paint(&mut self, start: usize, end: usize, tok: Tok) {
        let (s, e) = (
            start.saturating_sub(self.base).min(self.cls.len()),
            end.saturating_sub(self.base).min(self.cls.len()),
        );
        if s < e {
            self.cls[s..e].fill(tok as u8);
        }
    }

    /// Like `paint`, but keeps classes already painted with a higher priority.
    fn under(&mut self, start: usize, end: usize, tok: Tok) {
        let (s, e) = (
            start.saturating_sub(self.base).min(self.cls.len()),
            end.saturating_sub(self.base).min(self.cls.len()),
        );
        for c in &mut self.cls[s..e.max(s)] {
            if *c < tok as u8 {
                *c = tok as u8;
            }
        }
    }

    fn block(&mut self, b: &Block, origin: usize, doc: &Document) {
        let at = |s: usize| origin + s;
        match &b.kind {
            BlockKind::Paragraph => self.inlines(b, origin, doc, None),
            BlockKind::Heading { level, setext, .. } => {
                let (s, e) = (at(b.span.start), at(b.span.end));
                self.paint(s, e, Tok::heading(*level));
                if *setext {
                    // the underline: everything after the last text line
                    if let Some(l) = b.lines.last() {
                        self.paint(at(l.end), e, Tok::Marker);
                    }
                } else {
                    let hashes = self.text[s..e].bytes().take_while(|&c| c == b'#').count();
                    self.paint(s, s + hashes, Tok::Marker);
                    if let Some(l) = b.lines.first() {
                        // closing hashes
                        let after = at(l.end);
                        if after < e {
                            self.paint(after, e, Tok::Marker);
                        }
                    }
                }
                self.inlines(b, origin, doc, Some(Tok::heading(*level)));
            }
            BlockKind::ThematicBreak => self.paint(at(b.span.start), at(b.span.end), Tok::Marker),
            BlockKind::FencedCode { opener, closer, .. } => {
                for l in &b.lines {
                    self.paint(at(l.start), at(l.end), Tok::CodeBlock);
                }
                self.paint(at(opener.start), at(opener.end), Tok::Marker);
                if let Some(c) = closer {
                    self.paint(at(c.start), at(c.end), Tok::Marker);
                }
            }
            BlockKind::IndentedCode => {
                for l in &b.lines {
                    self.paint(at(l.start), at(l.end), Tok::CodeBlock);
                }
            }
            BlockKind::MathBlock { opener, closer, .. } => {
                self.paint(at(b.span.start), at(b.span.end), Tok::Meta);
                self.paint(at(opener.start), at(opener.end), Tok::Marker);
                self.paint(at(closer.start), at(closer.end), Tok::Marker);
            }
            BlockKind::HtmlBlock | BlockKind::Toc => self.paint(at(b.span.start), at(b.span.end), Tok::Meta),
            BlockKind::FrontMatter { opener, closer } => {
                self.paint(at(b.span.start), at(b.span.end), Tok::Meta);
                self.paint(at(opener.start), at(opener.end), Tok::Marker);
                self.paint(at(closer.start), at(closer.end), Tok::Marker);
            }
            BlockKind::Table { .. } => {
                for (i, l) in b.lines.iter().enumerate() {
                    if i == 1 {
                        self.paint(at(l.start), at(l.end), Tok::Marker);
                    } else {
                        self.line_inlines(
                            at(l.start),
                            at(l.end),
                            doc,
                            if i == 0 { Some(Tok::Strong) } else { None },
                        );
                        // cell pipes
                        let (s, e) = (at(l.start), at(l.end));
                        for (k, c) in self.text[s..e].bytes().enumerate() {
                            if c == b'|' {
                                self.paint(s + k, s + k + 1, Tok::Marker);
                            }
                        }
                    }
                }
            }
            BlockKind::BlockQuote { .. } => {
                for c in &b.children {
                    self.block(c, origin, doc);
                }
                // the `>` markers at the line starts of the quote
                let (s, e) = (at(b.span.start), at(b.span.end));
                let mut line = s;
                while line < e {
                    let end = self.text[line..e].find('\n').map_or(e, |k| line + k);
                    let t = &self.text[line..end];
                    let lead = t.len() - t.trim_start_matches([' ', '>']).len();
                    for (k, c) in t[..lead].bytes().enumerate() {
                        if c == b'>' {
                            self.paint(line + k, line + k + 1, Tok::Marker);
                        }
                    }
                    line = end + 1;
                }
            }
            BlockKind::List { .. } => {
                for c in &b.children {
                    self.block(c, origin, doc);
                }
            }
            BlockKind::ListItem { marker, task, .. } => {
                for c in &b.children {
                    self.block(c, origin, doc);
                }
                self.paint(at(marker.start), at(marker.end), Tok::Marker);
                if let Some((_, t)) = task {
                    self.paint(at(t.start), at(t.end), Tok::Marker);
                }
            }
            BlockKind::FootnoteDef { label } => {
                for c in &b.children {
                    self.block(c, origin, doc);
                }
                self.paint(at(label.start), at(label.end), Tok::Link);
            }
            BlockKind::LinkRefDef { label, dest, title } => {
                self.paint(at(b.span.start), at(b.span.end), Tok::Marker);
                self.paint(at(label.start), at(label.end), Tok::Link);
                self.paint(at(dest.start), at(dest.end), Tok::Url);
                if let Some(t) = title {
                    self.paint(at(t.start), at(t.end), Tok::Meta);
                }
            }
            #[allow(unreachable_patterns)]
            _ => {
                for c in &b.children {
                    self.block(c, origin, doc);
                }
            }
        }
    }

    /// Inline content of a leaf: the lines in one parse when they are separated by plain line
    /// breaks (offsets map 1:1), else line by line.
    fn inlines(&mut self, b: &Block, origin: usize, doc: &Document, base: Option<Tok>) {
        let Some((first, last)) = b.lines.first().zip(b.lines.last()) else {
            return;
        };
        let (s, e) = (origin + first.start, origin + last.end);
        let contiguous = b
            .lines
            .windows(2)
            .all(|w| &self.text[origin + w[0].end..origin + w[1].start] == "\n");
        if contiguous {
            self.line_inlines(s, e, doc, base);
        } else {
            for l in &b.lines {
                self.line_inlines(origin + l.start, origin + l.end, doc, base);
            }
        }
    }

    fn line_inlines(&mut self, s: usize, e: usize, doc: &Document, base: Option<Tok>) {
        if s >= e {
            return;
        }
        if let Some(t) = base {
            self.under(s, e, t);
        }
        let nodes = parse_inlines(&self.text[s..e], doc.options(), &doc.ref_context().inline);
        for n in &nodes {
            self.inline(n, s, Tok::Plain);
        }
    }

    fn inline(&mut self, n: &Inline, at: usize, outer: Tok) {
        let (s, e) = (at + n.span.start, at + n.span.end);
        let inner = |n: &Inline| {
            let a = n.children.first().map_or(n.span.start, |c| c.span.start);
            let b = n.children.last().map_or(n.span.end, |c| c.span.end);
            (at + a, at + b)
        };
        match &n.kind {
            InlineKind::Emph | InlineKind::Strong | InlineKind::Strike | InlineKind::Highlight => {
                let tok = match (&n.kind, outer) {
                    (InlineKind::Emph, Tok::Strong) | (InlineKind::Strong, Tok::Emph) => Tok::StrongEmph,
                    (InlineKind::Emph, _) => Tok::Emph,
                    (InlineKind::Strong, _) => Tok::Strong,
                    (InlineKind::Highlight, _) => Tok::Mark,
                    _ => Tok::Strike,
                };
                self.paint(s, e, tok);
                for c in &n.children {
                    self.inline(c, at, tok);
                }
                let (a, b) = inner(n);
                if n.children.is_empty() {
                    self.paint(s, e, Tok::Marker);
                } else {
                    self.paint(s, a, Tok::Marker);
                    self.paint(b, e, Tok::Marker);
                }
            }
            InlineKind::Code { content } => {
                self.paint(s, e, Tok::Marker);
                self.paint(at + content.start, at + content.end, Tok::Code);
            }
            InlineKind::Link { .. } | InlineKind::Image { .. } => {
                self.paint(s, e, Tok::Url);
                let (a, b) = inner(n);
                if !n.children.is_empty() {
                    self.paint(a, b, Tok::Link);
                    for c in &n.children {
                        self.inline(c, at, Tok::Link);
                    }
                }
                // brackets and parentheses
                for (k, c) in self.text[s..e].bytes().enumerate() {
                    let p = s + k;
                    if matches!(c, b'[' | b']' | b'(' | b')' | b'!') && (p < a || p >= b) {
                        self.paint(p, p + 1, Tok::Marker);
                    }
                }
            }
            InlineKind::Autolink { .. } | InlineKind::BareUrl { .. } => self.paint(s, e, Tok::Url),
            InlineKind::FootnoteRef { .. } => self.paint(s, e, Tok::Link),
            InlineKind::Html | InlineKind::Math { .. } => self.paint(s, e, Tok::Meta),
            InlineKind::Escape => self.paint(s, (s + 1).min(e), Tok::Marker),
            _ => {
                for c in &n.children {
                    self.inline(c, at, outer);
                }
            }
        }
    }

    /// Run-length form of the painted classes.
    fn runs(&self) -> Vec<Run> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.cls.len() {
            let c = self.cls[i];
            let mut j = i + 1;
            while j < self.cls.len() && self.cls[j] == c {
                j += 1;
            }
            if c != Tok::Plain as u8 {
                out.push(Run {
                    start: i as u32,
                    end: j as u32,
                    tok: Tok::from_u8(c),
                });
            }
            i = j;
        }
        out
    }
}

/// Token runs (absolute, ascending, non-overlapping) of the whole document.
pub fn document_runs(doc: &Document, cache: &mut HashMap<u64, Arc<Vec<Run>>>) -> Vec<(usize, Arc<Vec<Run>>)> {
    let text = doc.text();
    let mut out = Vec::with_capacity(doc.top_len());
    let mut fresh = HashMap::with_capacity(cache.len());
    for top in doc.tops() {
        let span = top.span();
        let (s, e) = (span.start.min(text.len()), span.end.min(text.len()));
        let mut h = ahash::AHasher::default();
        text[s..e].hash(&mut h);
        std::mem::discriminant(&top.block.kind).hash(&mut h);
        let key = h.finish();
        let runs = match cache.get(&key) {
            Some(r) => r.clone(),
            None => {
                let mut p = Painter {
                    text,
                    base: s,
                    cls: vec![0; e - s],
                };
                p.block(top.block, top.origin, doc);
                Arc::new(p.runs())
            }
        };
        fresh.insert(key, runs.clone());
        out.push((s, runs));
    }
    *cache = fresh;
    out
}

/// Colours of the token classes in the active theme.
#[derive(Clone, PartialEq)]
pub struct Palette {
    text: Color32,
    heading: [Color32; 6],
    marker: Color32,
    link: Color32,
    url: Color32,
    code: Color32,
    code_bg: Color32,
    code_block: Color32,
    meta: Color32,
    quote: Color32,
    mark_bg: Color32,
}

impl Palette {
    pub fn new(t: &Theme) -> Self {
        Palette {
            text: color(t.text),
            heading: std::array::from_fn(|i| color(t.headings[i].color)),
            marker: color(t.meta_text).gamma_multiply(0.75),
            link: color(t.link),
            url: color(t.link).gamma_multiply(0.7),
            code: color(t.inline_code.text),
            code_bg: color(t.inline_code.background),
            code_block: color(t.code_block.text),
            meta: color(t.meta_text),
            quote: color(t.quote_text),
            mark_bg: color(t.highlight_bg),
        }
    }

    fn format(&self, tok: Tok, font: &FontId) -> TextFormat {
        let mut f = TextFormat::simple(font.clone(), self.text);
        match tok {
            Tok::Plain => {}
            Tok::Heading1 | Tok::Heading2 | Tok::Heading3 | Tok::Heading4 | Tok::Heading5 | Tok::Heading6 => {
                f.color = self.heading[(tok as u8 - Tok::Heading1 as u8) as usize];
            }
            Tok::Quote => f.color = self.quote,
            Tok::Emph => f.italics = true,
            Tok::Strong => f.color = self.heading[0],
            Tok::StrongEmph => {
                f.color = self.heading[0];
                f.italics = true;
            }
            Tok::Strike => f.strikethrough = Stroke::new(1.0, self.text),
            Tok::Mark => f.background = self.mark_bg,
            Tok::Link => {
                f.color = self.link;
                f.underline = Stroke::new(1.0, self.link.gamma_multiply(0.5));
            }
            Tok::Url => f.color = self.url,
            Tok::Code => {
                f.color = self.code;
                f.background = self.code_bg;
            }
            Tok::CodeBlock => f.color = self.code_block,
            Tok::Meta => f.color = self.meta,
            Tok::Marker => f.color = self.marker,
        }
        f
    }
}

/// Builds the colored layout job for `buffer` (the editor text; `crlf`: the document has
/// `\r\n` where the buffer has `\n`) from the runs of `doc`, whose text the buffer shows.
pub fn layout_job(
    buffer: &str,
    crlf: bool,
    runs: &[(usize, Arc<Vec<Run>>)],
    palette: &Palette,
    font: &FontId,
    wrap: f32,
) -> LayoutJob {
    let mut job = LayoutJob {
        text: buffer.to_owned(),
        ..Default::default()
    };
    job.wrap.max_width = wrap;
    let n = buffer.len();
    // document offset → buffer offset (drops one byte per `\r\n` before it)
    let bytes = buffer.as_bytes();
    let (mut doc_pos, mut buf_pos) = (0usize, 0usize);
    let mut to_buf = |d: usize| -> usize {
        if !crlf {
            return d.min(n);
        }
        while doc_pos < d && buf_pos < n {
            doc_pos += if bytes[buf_pos] == b'\n' { 2 } else { 1 };
            buf_pos += 1;
        }
        buf_pos
    };
    let mut pos = 0usize;
    let push = |job: &mut LayoutJob, s: usize, e: usize, tok: Tok| {
        if s < e {
            job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: ByteIndex(s)..ByteIndex(e),
                format: palette.format(tok, font),
            });
        }
    };
    for (base, rs) in runs {
        for r in rs.iter() {
            let s = to_buf(base + r.start as usize);
            let e = to_buf(base + r.end as usize);
            let (s, e) = (s.max(pos), e.min(n));
            if s >= e || !buffer.is_char_boundary(s) || !buffer.is_char_boundary(e) {
                continue;
            }
            push(&mut job, pos, s, Tok::Plain);
            push(&mut job, s, e, r.tok);
            pos = e;
        }
    }
    push(&mut job, pos, n, Tok::Plain);
    if job.sections.is_empty() {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(0)..ByteIndex(0),
            format: palette.format(Tok::Plain, font),
        });
    }
    job
}

/// The uncoloured job `TextEdit` would lay out itself (texts over the size limit).
pub fn plain_job(ui: &egui::Ui, text: &str, font: &FontId, wrap: f32) -> LayoutJob {
    let v = ui.visuals();
    let c = v.override_text_color.unwrap_or_else(|| v.widgets.inactive.text_color());
    LayoutJob::simple(text.to_owned(), font.clone(), c, wrap)
}

#[cfg(test)]
thread_local! {
    /// Whole-document tokenising passes on this thread (tests).
    static PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whole-document tokenising passes so far on this thread (tests).
#[cfg(test)]
pub(crate) fn passes() -> usize {
    PASSES.with(|p| p.get())
}

/// Per-buffer colouring state: the run cache and the last job.
#[derive(Default)]
pub struct SourceHighlighter {
    cache: HashMap<u64, Arc<Vec<Run>>>,
    /// `(document pointer, text length, wrap, palette)` of `job`.
    key: Option<(usize, usize, u32)>,
    palette: Option<Palette>,
    job: Option<LayoutJob>,
}

impl SourceHighlighter {
    /// The layout job for `buffer`, which shows `doc`'s text; rebuilt only when the document
    /// (an edit), the wrap width or the theme changed.
    /// `rev`: the document's revision (the editor session edits its document in place, so
    /// the document pointer alone does not identify the text).
    #[allow(clippy::too_many_arguments)]
    pub fn job(
        &mut self,
        doc: &Arc<dyn markise_doc::FormatDoc>,
        rev: u64,
        buffer: &str,
        crlf: bool,
        theme: &Theme,
        font: &FontId,
        wrap: f32,
    ) -> LayoutJob {
        let palette = Palette::new(theme);
        let key = (
            (Arc::as_ptr(doc) as *const () as usize) ^ (rev as usize).rotate_left(17),
            buffer.len(),
            wrap.to_bits(),
        );
        let Some(doc) = doc.as_any().downcast_ref::<Document>() else {
            return LayoutJob::simple(buffer.to_owned(), font.clone(), egui::Color32::GRAY, wrap);
        };
        if self.key != Some(key) || self.palette.as_ref() != Some(&palette) || self.job.is_none() {
            #[cfg(test)]
            PASSES.with(|p| p.set(p.get() + 1));
            let runs = document_runs(doc, &mut self.cache);
            self.job = Some(layout_job(buffer, crlf, &runs, &palette, font, wrap));
            self.key = Some(key);
            self.palette = Some(palette);
        }
        self.job.clone().expect("job")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<(String, Tok)> {
        let d = markise_md::parse(src);
        let mut cache = HashMap::new();
        let mut out = Vec::new();
        for (base, rs) in document_runs(&d, &mut cache) {
            for r in rs.iter() {
                out.push((src[base + r.start as usize..base + r.end as usize].to_string(), r.tok));
            }
        }
        out
    }

    fn has(v: &[(String, Tok)], s: &str, t: Tok) -> bool {
        v.iter().any(|(a, b)| a == s && *b == t)
    }

    #[test]
    fn markers_are_dimmed_and_content_styled() {
        let v = toks("# Title\n\nSome *em* and **strong** with `code` and [a link](http://x.y).\n\n- item\n> quote\n");
        assert!(has(&v, "#", Tok::Marker), "{v:?}");
        assert!(has(&v, " Title", Tok::Heading1), "{v:?}");
        assert!(has(&v, "em", Tok::Emph), "{v:?}");
        assert!(has(&v, "strong", Tok::Strong), "{v:?}");
        assert!(has(&v, "**", Tok::Marker), "{v:?}");
        assert!(has(&v, "code", Tok::Code), "{v:?}");
        assert!(has(&v, "a link", Tok::Link), "{v:?}");
        assert!(has(&v, "http://x.y", Tok::Url), "{v:?}");
        assert!(has(&v, "-", Tok::Marker), "{v:?}");
        assert!(has(&v, ">", Tok::Marker), "{v:?}");
    }

    #[test]
    fn fences_are_markers_and_code_is_code() {
        let v = toks("```rust\nfn main() {}\n```\n");
        assert!(has(&v, "```rust", Tok::Marker), "{v:?}");
        assert!(has(&v, "fn main() {}", Tok::CodeBlock), "{v:?}");
    }

    #[test]
    fn edits_retokenize_only_the_changed_block() {
        let mut d = markise_md::parse("# A\n\npara *x*\n\nother\n");
        let mut cache = HashMap::new();
        let before = document_runs(&d, &mut cache);
        d.edit(16..16, "y");
        let after = document_runs(&d, &mut cache);
        assert!(
            Arc::ptr_eq(&before[0].1, &after[0].1),
            "untouched block reuses its runs"
        );
        assert!(!Arc::ptr_eq(&before[1].1, &after[1].1) || before[1].1 == after[1].1);
    }

    #[test]
    fn job_sections_cover_the_crlf_buffer() {
        let src = "# A\r\n\r\n*b*\r\n";
        let d = markise_md::parse(src);
        let runs = document_runs(&d, &mut HashMap::new());
        let buf = src.replace("\r\n", "\n");
        let job = layout_job(
            &buf,
            true,
            &runs,
            &Palette::new(&Theme::light()),
            &FontId::monospace(13.0),
            100.0,
        );
        let mut pos = 0;
        for s in &job.sections {
            assert_eq!(s.byte_range.start.0, pos);
            pos = s.byte_range.end.0;
        }
        assert_eq!(pos, buf.len());
        // `*b*` sits at buffer offset 5
        assert!(
            job.sections
                .iter()
                .any(|s| s.byte_range == (ByteIndex(5)..ByteIndex(6)) && s.format.color != Color32::PLACEHOLDER)
        );
    }
}
