//! `markise-export` — HTML export following the rendered-DOM contract of
//! `docs/spec/03-theme-contract.md` §3.1 / §4.3 / §4.4, so Typora-format CSS themes style the
//! exported file.
//!
//! Math is rendered to inline SVG (ADR 0008; spec 04 "math as rendered SVG or as LaTeX source")
//! inside the contract containers, wrapped in `mjx-container[jax=SVG]` (spec 03 §4.3/§4.4) so
//! themes targeting that element apply; [`MathExport::Tex`] keeps the escaped TeX source instead.
//! Invalid TeX is emitted as source with the `md-math-error` class and the message as `title`.
//!
//! Diagram fences (when the document enables diagrams) are rendered to inline SVG inside
//! `pre.md-fences.md-diagram > .md-diagram-panel > .md-diagram-panel-preview` (ADR 0009);
//! unsupported or invalid diagrams keep their source plus a `.md-diagram-notice`.
//!
//! Remaining placeholders (marked in output with classes from the contract):
//! - code is emitted as `<pre class="md-fences …"><code class="language-x">` (C21); with
//!   [`HtmlOptions::highlight`] its tokens are wrapped in `<span class="cm-…">` (spec 03 §4.5
//!   classes, `cm-s-inner` on the `<code>` so theme rules scoped by it apply), optionally with
//!   inline colors for a self-contained file.

pub mod css;
pub mod images;
pub mod job;
pub mod model;
pub mod output;
pub mod pandoc;
pub mod process;
pub mod sanitize;
pub mod vars;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;

use markise_blocks::diagram::{ApproxMeasure, DiagramLang, DiagramPalette, RenderCtx};
use markise_blocks::math::{self, MathMode};
use markise_md::inline::{
    CriticKind, Inline, InlineContext, InlineKind, decode_entity, decode_refs, parse_inlines, plain_text,
};
use markise_md::slug::{SlugTracker, slug};
use markise_md::{Align, Block, BlockKind, Document, MathDelim, frontmatter};
use markise_theme::{Theme, TokenKind, TokenLook, TokenRun};

/// Syntax highlighting of fenced code in export.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CodeHighlight {
    /// Plain `<code>` text (contract default, C21).
    #[default]
    Off,
    /// Token `<span>`s with spec 03 §4.5 classes; colors come from the stylesheet.
    Classes,
    /// Classes plus inline `style` colors from a theme (self-contained file); index =
    /// `TokenKind as usize`.
    Inline(Box<[TokenLook]>),
}

impl CodeHighlight {
    /// Inline colors from `theme`'s color scheme.
    pub fn inline_from(theme: &Theme) -> Self {
        CodeHighlight::Inline(TokenKind::ALL.iter().map(|&k| theme.token_look(k)).collect())
    }
}

/// Colors and type of diagrams rendered to SVG in export (spec 01 §4.6, ADR 0009).
#[derive(Clone, Debug, PartialEq)]
pub struct DiagramExport {
    pub palette: DiagramPalette,
    /// CSS font-family list written on each `<svg>`.
    pub font_family: String,
    /// Label size in points (CSS px in the SVG).
    pub font_size: f32,
}

impl DiagramExport {
    /// Diagram colors from the theme's color scheme, labels in its body font stack.
    pub fn from_theme(theme: &Theme) -> Self {
        let mut fams: Vec<String> = theme
            .fonts
            .body
            .iter()
            .map(|f| {
                if f.contains(' ') && !f.starts_with(['"', '\'']) {
                    format!("'{f}'")
                } else {
                    f.clone()
                }
            })
            .collect();
        if !fams
            .iter()
            .any(|f| f == "sans-serif" || f == "serif" || f == "system-ui")
        {
            fams.push("sans-serif".into());
        }
        DiagramExport {
            palette: DiagramPalette::from_theme(theme),
            font_family: fams.join(", "),
            font_size: theme.font_size.clamp(10.0, 20.0),
        }
    }
}

impl Default for DiagramExport {
    fn default() -> Self {
        DiagramExport::from_theme(&Theme::light())
    }
}

/// How math is written to HTML (spec 04, preference for "HTML (without Styles)").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MathExport {
    /// Rendered SVG (glyph outlines, no fonts or scripts needed by the reader).
    #[default]
    Svg,
    /// Escaped TeX source in `\(…\)` / `\[…\]` for a client-side renderer (C36 placeholder form).
    Tex,
}

/// Export settings.
#[derive(Clone, Debug, Default)]
pub struct HtmlOptions {
    /// `<title>` fallback when the document has no `title` front-matter key or heading.
    pub fallback_title: Option<String>,
    /// CSS inlined into `<style>` (theme CSS). `None` uses a small built-in stylesheet.
    pub css: Option<String>,
    /// Render soft line breaks as `<br/>` ("preserve line breaks on export", R3.1.2).
    pub preserve_line_breaks: bool,
    /// Trusted document: emit raw HTML and all URL schemes unchanged. Default `false`: raw
    /// HTML is sanitized and URLs are restricted to safe schemes (C1, see [`sanitize`]).
    pub trusted: bool,
    /// Syntax highlighting of fenced code.
    pub highlight: CodeHighlight,
    /// "HTML (without styles)": no `<style>` element at all (bare semantic HTML).
    pub no_style: bool,
    /// Extra markup appended to `<head>` / `<body>` (`append-head` / `append-body`, spec 05 §3).
    /// Sanitized like raw HTML in the document unless [`HtmlOptions::trusted`] (C40).
    pub head_extra: Option<String>,
    pub body_extra: Option<String>,
    /// How local image paths are written (the exported file usually lives elsewhere than the
    /// document).
    pub images: ImageLinks,
    /// Cancellation of the export job (checked while embedding images).
    pub control: Option<job::JobControl>,
    /// Diagram colors and font (used when the document enables diagrams).
    pub diagrams: DiagramExport,
    /// Math as SVG (default) or TeX source.
    pub math: MathExport,
    /// Where wikilinks point (spec 06 Q-V11): without a resolver, or for an unresolved target,
    /// a wikilink is written as its label in plain text.
    pub wikilinks: WikiLinkHrefs,
    /// Review comments (CriticMarkup, ADR 0012): kept (default) or stripped. Suggestions
    /// are always written as `<ins>` / `<del>`.
    pub comments: CommentExport,
}

/// How review comments and highlights are exported ([`HtmlOptions::comments`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommentExport {
    /// Highlights as `<mark class="critic" title="comment">`, comments as
    /// `<span class="critic comment">`.
    #[default]
    Keep,
    /// No comments; highlighted text as plain text.
    Strip,
}

/// Resolves a wikilink (`target`, `subpath`) to the `href` written for it (a relative path to
/// the linked file, percent-encoded, with a `#fragment` for a heading), or `None` when it does
/// not resolve.
pub type WikiLinkResolver = dyn Fn(&str, Option<&str>) -> Option<String> + Send + Sync;

/// The wikilink resolver of an export ([`HtmlOptions::wikilinks`]).
#[derive(Clone, Default)]
pub struct WikiLinkHrefs(pub Option<std::sync::Arc<WikiLinkResolver>>);

impl std::fmt::Debug for WikiLinkHrefs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "WikiLinkHrefs(resolver)"
        } else {
            "WikiLinkHrefs(none)"
        })
    }
}

/// Treatment of local image references in exported HTML. Remote URLs (`http:`, `https:`, …)
/// and `data:` URIs are never touched or fetched.
#[derive(Clone, Debug, Default)]
pub enum ImageLinks {
    /// Write `src` as in the document.
    #[default]
    Keep,
    /// Self-contained export: embed local raster images as `data:` URIs up to `max_bytes`
    /// each and `budget` bytes of embedded output per export (each file is encoded once);
    /// anything else falls back to [`ImageLinks::Rebase`] with a warning.
    Embed {
        assets: Assets,
        max_bytes: u64,
        budget: usize,
    },
    /// Rewrite local paths relative to the output folder (or as `file:` URLs when no relative
    /// path exists, e.g. another drive).
    Rebase { assets: Assets },
}

/// Where the document's local resources are and where the exported file goes.
#[derive(Clone, Debug)]
pub struct Assets {
    /// Folder of the document (relative image paths resolve against it).
    pub doc_dir: std::path::PathBuf,
    /// `typora-root-url` of the document (root-relative paths, R8.1), as the viewer uses it.
    pub root_url: Option<String>,
    /// Folder of the exported file.
    pub out_dir: std::path::PathBuf,
}

/// A diagram fence: inline SVG, or the source with a notice when it cannot be rendered.
fn diagram_html(dl: DiagramLang, lang: &str, content: &str, d: &DiagramExport, out: &mut String) {
    let m = ApproxMeasure;
    let ctx = RenderCtx::new(&m, d.font_size);
    let _ = write!(
        out,
        "<pre class='md-fences md-end-block md-diagram md-fences-advanced' lang='{}'><div class='md-diagram-panel'>",
        esc(lang)
    );
    match markise_blocks::diagram::render(dl, content, &ctx) {
        Ok(scene) => {
            out.push_str("<div class='md-diagram-panel-preview'>");
            out.push_str(&markise_blocks::diagram::svg::to_svg(
                &scene,
                &d.palette,
                &d.font_family,
            ));
            out.push_str("</div>");
        }
        Err(e) => {
            let _ = write!(
                out,
                "<div class='md-diagram-notice'>{}</div><code>{}</code>",
                esc(&e.to_string()),
                esc(content)
            );
        }
    }
    out.push_str("</div></pre>\n");
}

/// Warnings kept per export (a document with thousands of broken images reports a sample).
pub const MAX_WARNINGS: usize = 50;

/// Minimal original stylesheet used when no theme CSS is given.
pub const DEFAULT_CSS: &str = "\
html { font-size: 16px; }
body { margin: 0; background: #fff; color: #222; }
#write { max-width: 860px; margin: 0 auto; padding: 30px; font-family: system-ui, sans-serif; line-height: 1.6; }
#write pre.md-fences, #write code { font-family: ui-monospace, monospace; }
#write pre.md-fences { background: #f6f6f4; padding: 8px 12px; overflow-x: auto; }
#write blockquote { border-left: 4px solid #ddd; margin-left: 0; padding-left: 1em; color: #555; }
#write table { border-collapse: collapse; }
#write th, #write td { border: 1px solid #ccc; padding: 4px 8px; }
#write .md-alert { border-left: 4px solid #888; padding-left: 1em; }
#write .md-task-list-item { list-style: none; }
#write .md-task-list-item > input { margin-left: -1.3em; }
#write mjx-container[display=true] { display: block; text-align: center; margin: 1em 0; overflow-x: auto; }
#write .md-math-error { color: #c0392b; }
#write ins.critic { color: #1a7f37; text-decoration: underline; }
#write del.critic { color: #cf222e; text-decoration: line-through; }
#write mark.critic { background: #fbeb9b; }
#write .critic.comment { background: rgba(251,235,155,0.55); font-size: 0.85em; font-style: italic; border-radius: 3px; padding: 0 4px; margin-left: 2px; }
.cm-s-inner .cm-keyword, .cm-s-inner .cm-tag, .cm-s-inner .cm-header { color: #8b3fa8; }
.cm-s-inner .cm-string, .cm-s-inner .cm-string-2 { color: #2d7a3a; }
.cm-s-inner .cm-comment { color: #8d8a82; }
.cm-s-inner .cm-number, .cm-s-inner .cm-atom { color: #b35a13; }
.cm-s-inner .cm-def, .cm-s-inner .cm-builtin, .cm-s-inner .cm-property { color: #1f5f99; }
.cm-s-inner .cm-variable-3, .cm-s-inner .cm-attribute, .cm-s-inner .cm-meta { color: #8a5a00; }
.cm-s-inner .cm-positive { color: #2f8a4f; } .cm-s-inner .cm-negative { color: #c0392b; }
.cm-s-inner .cm-link { color: #1c66a8; } .cm-s-inner .cm-header { font-weight: bold; }
";

/// Escapes text for HTML content and attribute values.
pub fn escape_html(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

/// A comment as one line (inline elements: line breaks become spaces).
fn comment_line(s: &str) -> String {
    s.split(['\n', '\r'])
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    escape_html(s, &mut o);
    o
}

/// URL for `href`/`src`: spaces percent-encoded, then HTML-escaped.
fn esc_url(u: &str) -> String {
    let mut s = String::with_capacity(u.len());
    for c in u.chars() {
        match c {
            ' ' => s.push_str("%20"),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    let _ = write!(s, "%{b:02X}");
                }
            }
            c => s.push(c),
        }
    }
    esc(&s)
}

struct Heading {
    level: u8,
    id: String,
    html: String,
}

struct Ctx<'a> {
    doc: &'a Document,
    text: &'a str,
    inline_ctx: InlineContext,
    opts: &'a HtmlOptions,
    /// Heading ids in document order, assigned in a first pass.
    headings: Vec<Heading>,
    next_heading: usize,
    /// The `[toc]` block's HTML, formatted on first use and copied for every further `[toc]`
    /// (each lists the same headings; issue #29).
    toc_html: Option<String>,
    /// `[toc]` blocks rendered so far.
    tocs_rendered: usize,
    /// TOC entries formatted so far (work counter for the scaling test).
    #[cfg(test)]
    toc_entries_formatted: usize,
    /// Footnote label → number (definition order).
    footnote_numbers: HashMap<String, usize>,
    footnotes: Vec<(usize, String)>,
    /// Raw-HTML sanitizer state for the inline content currently being rendered.
    san: RefCell<sanitize::Sanitizer>,
    /// Problems worth telling the user about (images not embedded, …).
    warnings: RefCell<Vec<String>>,
    /// Image embedding state of this export (dedupe, budget).
    embedder: RefCell<images::Embedder>,
}

/// Renders the full exported HTML page (spec 03 §3.1 skeleton).
pub fn render_document(doc: &Document, opts: &HtmlOptions) -> String {
    render_document_report(doc, opts).0
}

/// [`render_document`] plus warnings (e.g. images too large to embed).
pub fn render_document_report(doc: &Document, opts: &HtmlOptions) -> (String, Vec<String>) {
    match render_page(doc, opts, None) {
        Ok(r) => r,
        Err(_) => unreachable!("no size limit"),
    }
}

/// [`render_document_report`], unless the page is larger than `limit` bytes: then its exact
/// size is returned instead (issue #29: every `[toc]` lists all headings, so a document with
/// many of them exports very large). The size is exact because the page is really rendered,
/// but once it passes the limit every further top-level block is only counted and dropped,
/// so memory stays near `limit` plus the largest block; under the limit the page is kept
/// and nothing is rendered twice.
pub fn render_document_limited(
    doc: &Document,
    opts: &HtmlOptions,
    limit: u64,
) -> Result<(String, Vec<String>), HtmlSize> {
    render_page(doc, opts, Some(limit))
}

fn render_page(doc: &Document, opts: &HtmlOptions, limit: Option<u64>) -> Result<(String, Vec<String>), HtmlSize> {
    let fm = frontmatter::front_matter(doc);
    let mut ctx = Ctx::new(doc, opts);
    let title = frontmatter::get(&fm, "title")
        .map(str::to_string)
        .or_else(|| ctx.headings.first().map(|h| strip_tags(&h.html)))
        .or_else(|| opts.fallback_title.clone())
        .unwrap_or_default();
    let mut head = String::with_capacity(2048);
    head.push_str("<!doctype html>\n<html>\n<head>\n");
    head.push_str("<meta charset='UTF-8'><meta name='viewport' content='width=device-width, initial-scale=1'>\n");
    if let Some(author) = frontmatter::get(&fm, "author") {
        let _ = writeln!(head, "<meta name='author' content=\"{}\">", esc(author));
    }
    if !opts.no_style {
        head.push_str("<style>\n");
        head.push_str(&style_text(opts.css.as_deref().unwrap_or(DEFAULT_CSS)));
        head.push_str("\n</style>\n");
    }
    let _ = writeln!(head, "<title>{}</title>", esc(&title));
    if let Some(h) = &opts.head_extra {
        head.push_str(&extra_markup(h, opts.trusted));
        head.push('\n');
    }
    head.push_str(
        "</head>\n<body class='typora-export'>\n<div class='typora-export-content'>\n<div id='write' class=''>",
    );
    let mut tail = String::from("</div>\n</div>\n");
    if let Some(b) = &opts.body_extra {
        tail.push_str(&extra_markup(b, opts.trusted));
        tail.push('\n');
    }
    tail.push_str("</body>\n</html>\n");
    let frame = (head.len() + tail.len()) as u64;
    let (body, dropped) = ctx.render_write_limited(limit.map(|l| l.saturating_sub(frame)));
    let warnings = ctx.warnings.take();
    let bytes = frame + dropped + body.len() as u64;
    if limit.is_some_and(|l| bytes > l) {
        return Err(HtmlSize {
            bytes,
            tocs: ctx.tocs_rendered,
        });
    }
    let mut out = head;
    out.reserve(body.len() + tail.len());
    out.push_str(&body);
    out.push_str(&tail);
    Ok((out, warnings))
}

/// `[toc]` blocks in `doc` (each one lists every heading in the export).
pub fn toc_count(doc: &Document) -> usize {
    let mut n = 0;
    for b in doc.blocks() {
        b.walk(&mut |b, _| {
            if matches!(b.kind, BlockKind::Toc) {
                n += 1;
            }
        });
    }
    n
}

/// HTML size above which an export asks before writing (UI) or warns (CLI): every `[toc]`
/// lists all headings, so documents with many of them export very large (issue #29).
pub const LARGE_HTML_BYTES: u64 = 200_000_000;

/// The exact size of an HTML export that passed its limit ([`render_document_limited`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtmlSize {
    /// Bytes of the whole page.
    pub bytes: u64,
    /// `[toc]` blocks in the document.
    pub tocs: usize,
}

impl HtmlSize {
    /// The size for people: "1.3 GB", "250 MB", "12 KB" (decimal units).
    pub fn human(&self) -> String {
        human_bytes(self.bytes)
    }
}

/// Decimal size for messages: "1.3 GB", "250 MB", "12 KB", "512 bytes".
pub fn human_bytes(b: u64) -> String {
    match b {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{} MB", b / 1_000_000),
        b if b >= 1_000 => format!("{} KB", b / 1_000),
        b => format!("{b} bytes"),
    }
}

/// CSS made safe for a `<style>` element: no `<` survives, so no `</style` (inside a CSS
/// string or comment) can end the raw-text element. `\3c ` is the CSS escape of `<`: the same
/// character inside strings, harmless in comments; `<` has no other use in CSS.
pub fn style_text(css: &str) -> String {
    css.replace('<', "\\3c ")
}

/// Document-supplied head/body additions: sanitized unless the export is trusted.
fn extra_markup(html: &str, trusted: bool) -> String {
    if trusted {
        html.to_string()
    } else {
        sanitize::sanitize(html)
    }
}

/// Renders only the content of `#write`.
pub fn render_body(doc: &Document, opts: &HtmlOptions) -> String {
    Ctx::new(doc, opts).render_write()
}

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
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

impl<'a> Ctx<'a> {
    fn new(doc: &'a Document, opts: &'a HtmlOptions) -> Self {
        let labels = doc.footnote_labels();
        let mut c = Ctx {
            doc,
            text: doc.text(),
            inline_ctx: InlineContext {
                link_defs: doc.link_defs(),
                footnotes: labels.iter().cloned().collect(),
            },
            opts,
            headings: Vec::new(),
            next_heading: 0,
            toc_html: None,
            tocs_rendered: 0,
            #[cfg(test)]
            toc_entries_formatted: 0,
            footnote_numbers: HashMap::new(),
            footnotes: Vec::new(),
            san: RefCell::new(sanitize::Sanitizer::new()),
            warnings: RefCell::new(Vec::new()),
            embedder: RefCell::new(match &opts.images {
                ImageLinks::Embed { max_bytes, budget, .. } => images::Embedder::new(*max_bytes, *budget),
                _ => images::Embedder::new(0, 0),
            }),
        };
        if !labels.is_empty() {
            c.number_footnotes(&labels);
        }
        c.collect_headings();
        c
    }

    /// Footnote numbers by first reference in document order; unreferenced definitions
    /// follow in definition order (owner decision D2).
    fn number_footnotes(&mut self, labels: &[String]) {
        let mut order: Vec<String> = Vec::new();
        let mut contents = Vec::new();
        for b in self.doc.blocks() {
            b.walk(&mut |b, _| match &b.kind {
                BlockKind::Paragraph | BlockKind::Heading { .. } => contents.push(b.content(self.text)),
                BlockKind::Table { .. } => {
                    for row in markise_md::table::cell_spans(self.text, b) {
                        for c in row {
                            contents.push(c.slice(self.text).to_string());
                        }
                    }
                }
                _ => {}
            });
        }
        fn refs(nodes: &[Inline], out: &mut Vec<String>) {
            for n in nodes {
                if let InlineKind::FootnoteRef { label } = &n.kind {
                    out.push(label.clone());
                }
                refs(&n.children, out);
            }
        }
        for c in contents {
            if !c.contains("[^") {
                continue;
            }
            let nodes = parse_inlines(&c, self.doc.options(), &self.inline_ctx);
            refs(&nodes, &mut order);
        }
        order.extend(labels.iter().cloned());
        for l in order {
            let n = self.footnote_numbers.len() + 1;
            self.footnote_numbers.entry(l).or_insert(n);
        }
    }

    fn collect_headings(&mut self) {
        let mut tracker = SlugTracker::new();
        // `<a name="…">` anchors in raw HTML count for uniqueness (R7.2).
        let mut found = Vec::new();
        for b in self.doc.blocks() {
            b.walk(&mut |b, _| match &b.kind {
                BlockKind::Heading { level, .. } => found.push((Some(*level), b.content(self.text))),
                BlockKind::HtmlBlock | BlockKind::Paragraph | BlockKind::Table { .. } => {
                    let c = b.content(self.text);
                    if c.contains("<a ") || c.contains("<A ") {
                        found.push((None, c));
                    }
                }
                _ => {}
            });
        }
        for (level, content) in found {
            match level {
                Some(level) => {
                    let nodes = parse_inlines(&content, self.doc.options(), &self.inline_ctx);
                    let id = tracker.anchor_for(&plain_text(&content, &nodes));
                    let mut html = String::new();
                    self.san.replace(sanitize::Sanitizer::new());
                    self.inlines(&content, &nodes, &mut html);
                    self.headings.push(Heading { level, id, html });
                }
                None => {
                    for name in anchor_names(&content) {
                        tracker.add_existing(&name);
                    }
                }
            }
        }
    }

    fn render_write(&mut self) -> String {
        self.render_write_limited(None).0
    }

    /// The content of `#write`. With a `limit`, once the output passes it every further
    /// top-level block is only counted: returns the kept output and the bytes dropped.
    fn render_write_limited(&mut self, limit: Option<u64>) -> (String, u64) {
        let mut out = String::with_capacity(self.text.len() * 2);
        let mut dropped = 0u64;
        let blocks = self.doc.blocks();
        for b in blocks {
            self.block(b, &mut out, false);
            if let Some(l) = limit
                && dropped + out.len() as u64 > l
            {
                dropped += out.len() as u64;
                out.clear();
                out.shrink_to(1 << 20);
            }
        }
        if !self.footnotes.is_empty() {
            out.push_str("<div class='footnotes-area'><hr/>\n");
            let mut notes = std::mem::take(&mut self.footnotes);
            notes.sort_by_key(|(n, _)| *n);
            for (n, html) in notes {
                let _ = writeln!(
                    out,
                    "<div class='footnote-line'><span class='md-fn-count'>{n}</span> {html}<a class='reversefootnote' name='dfref-footnote-{n}' href='#ref-footnote-{n}'>↩</a></div>"
                );
            }
            out.push_str("</div>\n");
        }
        (out, dropped)
    }

    fn inline_html(&mut self, content: &str) -> String {
        self.san.replace(sanitize::Sanitizer::new());
        let nodes = parse_inlines(content, self.doc.options(), &self.inline_ctx);
        let mut s = String::new();
        self.inlines(content, &nodes, &mut s);
        s
    }

    fn block(&mut self, b: &Block, out: &mut String, tight: bool) {
        let text = self.text;
        match &b.kind {
            BlockKind::Paragraph => {
                let html = self.inline_html(&b.content(text));
                if tight {
                    out.push_str(&html);
                } else {
                    let _ = writeln!(out, "<p>{html}</p>");
                }
            }
            BlockKind::Heading { level, .. } => {
                let h = self.headings.get(self.next_heading);
                self.next_heading += 1;
                match h {
                    Some(h) => {
                        let _ = writeln!(out, "<h{level} id='{}'>{}</h{level}>", esc(&h.id), h.html);
                    }
                    None => {
                        let html = self.inline_html(&b.content(text));
                        let _ = writeln!(out, "<h{level}>{html}</h{level}>");
                    }
                }
            }
            BlockKind::ThematicBreak => out.push_str("<hr />\n"),
            BlockKind::FencedCode { info, .. } => {
                let info = info.slice(text);
                let lang = markise_md::inline::unescape(&decode_refs(info.split_whitespace().next().unwrap_or("")));
                let content = b.content(text);
                let opts = self.doc.options();
                if opts.code_math && lang.eq_ignore_ascii_case("math") {
                    math_block_html(&content, None, self.opts.math, out);
                } else if opts.diagrams
                    && let Some(dl) = DiagramLang::from_info(&lang)
                {
                    diagram_html(dl, &lang, &content, &self.opts.diagrams, out);
                } else {
                    code_html(&lang, b.lines.iter().map(|l| l.slice(text)), &self.opts.highlight, out);
                }
            }
            BlockKind::IndentedCode => code_html("", b.lines.iter().map(|l| l.slice(text)), &CodeHighlight::Off, out),
            BlockKind::MathBlock { delim, opener, .. } => {
                let suffix = match delim {
                    MathDelim::Dollars => {
                        let o = opener.slice(text).trim();
                        let s = o.trim_start_matches("$$").trim();
                        (!s.is_empty()).then_some(s)
                    }
                    MathDelim::Brackets => None,
                };
                math_block_html(&b.content(text), suffix, self.opts.math, out);
            }
            BlockKind::HtmlBlock => {
                let raw = b.content(text);
                if self.opts.trusted {
                    out.push_str(&raw);
                } else {
                    out.push_str(&sanitize::sanitize(&raw));
                }
                out.push('\n');
            }
            BlockKind::Table { aligns } => self.table(b, aligns, out),
            BlockKind::BlockQuote { alert } => match alert {
                Some(kind) => {
                    let k = kind.as_str();
                    let _ = writeln!(out, "<div class='md-alert md-alert-{k} {k}'>");
                    let mut title = k.to_string();
                    title[..1].make_ascii_uppercase();
                    let _ = writeln!(
                        out,
                        "<p><span class='md-alert-text-container'><span class='md-alert-text md-alert-text-{k}'>{title}</span></span></p>"
                    );
                    for (i, c) in b.children.iter().enumerate() {
                        if i == 0 && matches!(c.kind, BlockKind::Paragraph) {
                            // drop the `[!KIND]` marker line
                            if c.lines.len() > 1 {
                                let rest: Vec<&str> = c.lines[1..].iter().map(|l| l.slice(text)).collect();
                                let html = self.inline_html(&rest.join("\n"));
                                let _ = writeln!(out, "<p>{html}</p>");
                            }
                            continue;
                        }
                        self.block(c, out, false);
                    }
                    out.push_str("</div>\n");
                }
                None => {
                    out.push_str("<blockquote>\n");
                    for c in &b.children {
                        self.block(c, out, false);
                    }
                    out.push_str("</blockquote>\n");
                }
            },
            BlockKind::List {
                ordered, start, tight, ..
            } => {
                let has_tasks = b
                    .children
                    .iter()
                    .any(|i| matches!(i.kind, BlockKind::ListItem { task: Some(_), .. }));
                let class = if has_tasks { " class='task-list'" } else { "" };
                if *ordered {
                    if *start != 1 {
                        let _ = writeln!(out, "<ol start='{start}'>");
                    } else {
                        out.push_str("<ol>\n");
                    }
                } else {
                    let _ = writeln!(out, "<ul{class}>");
                }
                for item in &b.children {
                    self.list_item(item, *tight, out);
                }
                out.push_str(if *ordered { "</ol>\n" } else { "</ul>\n" });
            }
            BlockKind::ListItem { .. } => self.list_item(b, tight, out),
            BlockKind::FrontMatter { .. } | BlockKind::LinkRefDef { .. } => {}
            BlockKind::Toc => self.toc(out),
            BlockKind::FootnoteDef { label } => {
                let key = markise_md::inline::normalize_label(label.slice(text));
                let n = self.footnote_numbers.get(&key).copied().unwrap_or(0);
                let mut html = String::new();
                for (i, c) in b.children.iter().enumerate() {
                    if i == 0 && matches!(c.kind, BlockKind::Paragraph) {
                        html.push_str(&self.inline_html(&c.content(text)));
                    } else {
                        self.block(c, &mut html, false);
                    }
                }
                self.footnotes.push((n, html));
            }
        }
    }

    fn list_item(&mut self, item: &Block, tight: bool, out: &mut String) {
        let BlockKind::ListItem { task, .. } = &item.kind else {
            return;
        };
        match task {
            Some((checked, _)) => {
                let state = if *checked {
                    "task-list-done"
                } else {
                    "task-list-not-done"
                };
                let _ = write!(
                    out,
                    "<li class='md-task-list-item task-list-item {state}'><input type='checkbox'{} />",
                    if *checked { " checked='checked'" } else { "" }
                );
            }
            None => out.push_str("<li>"),
        }
        for (i, c) in item.children.iter().enumerate() {
            if tight && i > 0 && matches!(c.kind, BlockKind::Paragraph) {
                out.push('\n');
            }
            self.block(c, out, tight);
        }
        out.push_str("</li>\n");
    }

    fn table(&mut self, b: &Block, aligns: &[Align], out: &mut String) {
        let text = self.text;
        let rows = markise_md::table::cell_spans(text, b);
        let style = |a: Align| match a {
            Align::Left => " style='text-align:left;'",
            Align::Center => " style='text-align:center;'",
            Align::Right => " style='text-align:right;'",
            Align::None => "",
        };
        out.push_str("<figure class='table-figure'><table>\n<thead>\n<tr>");
        for (ri, row) in rows.iter().enumerate() {
            if ri == 1 {
                out.push_str("<tbody>\n");
            }
            if ri > 0 {
                out.push_str("<tr>");
            }
            let tag = if ri == 0 { "th" } else { "td" };
            for (ci, a) in aligns.iter().enumerate() {
                // GFM: `\|` is unescaped before inline parsing (also inside code spans)
                let cell = row.get(ci).map(|s| s.slice(text)).unwrap_or("").replace("\\|", "|");
                let html = self.inline_html(&cell);
                let _ = write!(out, "<{tag}{}>{html}</{tag}>", style(*a));
            }
            out.push_str("</tr>\n");
            if ri == 0 {
                out.push_str("</thead>\n");
            }
        }
        if rows.len() > 1 {
            out.push_str("</tbody>\n");
        }
        out.push_str("</table></figure>\n");
    }

    fn toc(&mut self, out: &mut String) {
        self.tocs_rendered += 1;
        if self.toc_html.is_none() {
            self.toc_html = Some(self.format_toc());
        }
        if let Some(html) = &self.toc_html {
            out.push_str(html);
        }
    }

    fn format_toc(&mut self) -> String {
        let mut out = String::from("<div class='md-toc' mdtype='toc'><p class='md-toc-content' role='list'>");
        for (i, h) in self.headings.iter().enumerate() {
            let _ = write!(
                out,
                "<span class='md-toc-item md-toc-h{}' data-ref='n{i}'><a class='md-toc-inner' href='#{}'>{}</a></span>",
                h.level,
                esc(&h.id),
                h.html
            );
        }
        out.push_str("</p></div>\n");
        #[cfg(test)]
        {
            self.toc_entries_formatted += self.headings.len();
        }
        out
    }

    fn inlines(&self, s: &str, nodes: &[Inline], out: &mut String) {
        for (i, n) in nodes.iter().enumerate() {
            if let InlineKind::Critic {
                kind: CriticKind::Highlight,
                ..
            } = n.kind
            {
                // the comment written right after a highlight is its title
                let title = nodes.get(i + 1).and_then(|c| match c.kind {
                    InlineKind::Critic {
                        kind: CriticKind::Comment,
                        content,
                    } if c.span.start == n.span.end => Some(content.slice(s)),
                    _ => None,
                });
                self.critic_highlight(s, n, title, out);
                continue;
            }
            self.inline(s, n, out);
        }
    }

    /// A review highlight (ADR 0012): `<mark class="critic">` with its comment as the title,
    /// or its text alone when comments are stripped.
    fn critic_highlight(&self, s: &str, n: &Inline, title: Option<&str>, out: &mut String) {
        if self.opts.comments == CommentExport::Strip {
            self.inlines(s, &n.children, out);
            return;
        }
        out.push_str("<mark class=\"critic\"");
        if let Some(t) = title {
            let _ = write!(out, " title=\"{}\"", esc(&comment_line(t)));
        }
        out.push('>');
        self.inlines(s, &n.children, out);
        out.push_str("</mark>");
    }

    fn wrap(&self, tag: &str, s: &str, n: &Inline, out: &mut String) {
        let _ = write!(out, "<{tag}>");
        self.inlines(s, &n.children, out);
        let _ = write!(out, "</{tag}>");
    }

    /// Records a warning; at most [`MAX_WARNINGS`] are kept (then one "omitted" note).
    fn warn(&self, w: String) {
        let mut ws = self.warnings.borrow_mut();
        match ws.len().cmp(&MAX_WARNINGS) {
            std::cmp::Ordering::Less => ws.push(w),
            std::cmp::Ordering::Equal => ws.push("… further warnings omitted".into()),
            std::cmp::Ordering::Greater => {}
        }
    }

    /// `src` of a document image according to [`HtmlOptions::images`].
    fn image_src(&self, dest: &str) -> String {
        let (assets, embed) = match &self.opts.images {
            ImageLinks::Keep => return dest.to_string(),
            ImageLinks::Embed { assets, .. } => (assets, true),
            ImageLinks::Rebase { assets } => (assets, false),
        };
        let Some(r) = images::local_ref(assets, dest) else {
            return dest.to_string(); // remote, data: or another host's file: — never touched
        };
        if embed {
            match self.embedder.borrow_mut().embed(&r.path, self.opts.control.as_ref()) {
                images::Embedded::Uri(uri) => return uri.to_string(), // suffix dropped
                images::Embedded::Link(Some(why)) => self.warn(format!("image {dest} not embedded: {why}")),
                images::Embedded::Link(None) => {}
            }
        }
        // a reference that cannot be resolved on disk is kept as written
        images::link_from(&assets.out_dir, &r).unwrap_or_else(|| dest.to_string())
    }

    /// URL for an attribute, or `None` when the scheme is not allowed (untrusted export).
    fn url<'u>(&self, u: &'u str, image: bool) -> Option<&'u str> {
        if self.opts.trusted {
            Some(u)
        } else {
            sanitize::safe_url(u, image)
        }
    }

    fn inline(&self, s: &str, n: &Inline, out: &mut String) {
        if !self.opts.trusted && !matches!(n.kind, InlineKind::Html) && self.san.borrow().skipping() {
            return; // inside raw <script>…</script> etc.
        }
        match &n.kind {
            InlineKind::Text => {
                let t = n.span.slice(s);
                if self.doc.options().smart_punctuation_render {
                    escape_html(&smart(t), out);
                } else {
                    escape_html(t, out);
                }
            }
            InlineKind::Escape => escape_html(&s[n.span.start + 1..n.span.end], out),
            InlineKind::Entity => {
                let e = n.span.slice(s);
                let d = decode_entity(e);
                if d == e {
                    out.push_str(e); // unknown named entity: pass through as written
                } else {
                    escape_html(&d, out);
                }
            }
            InlineKind::Code { content } => {
                let c = content.slice(s).replace('\n', " ");
                let c = if c.len() >= 2 && c.starts_with(' ') && c.ends_with(' ') && !c.trim().is_empty() {
                    c[1..c.len() - 1].to_string()
                } else {
                    c
                };
                out.push_str("<code>");
                escape_html(&c, out);
                out.push_str("</code>");
            }
            InlineKind::SoftBreak => out.push_str(if self.opts.preserve_line_breaks {
                "<br/>\n"
            } else {
                "\n"
            }),
            InlineKind::HardBreak => out.push_str("<br/>\n"),
            InlineKind::Emph => self.wrap("em", s, n, out),
            InlineKind::Strong => self.wrap("strong", s, n, out),
            InlineKind::Strike => self.wrap("del", s, n, out),
            InlineKind::Highlight => self.wrap("mark", s, n, out),
            InlineKind::Sub => self.wrap("sub", s, n, out),
            InlineKind::Sup => self.wrap("sup", s, n, out),
            InlineKind::Link { dest, title, .. } => {
                let dest = decode_refs(dest);
                let href = if let Some(frag) = dest.strip_prefix('#') {
                    format!("#{}", slug(frag))
                } else {
                    dest
                };
                out.push_str("<a");
                if let Some(h) = self.url(&href, false) {
                    let _ = write!(out, " href=\"{}\"", esc_url(h));
                }
                if let Some(t) = title {
                    let _ = write!(out, " title=\"{}\"", esc(&decode_refs(t)));
                }
                out.push('>');
                self.inlines(s, &n.children, out);
                out.push_str("</a>");
            }
            InlineKind::Image { dest, title, .. } => {
                let alt = plain_text(s, &n.children);
                let dest = self.image_src(&decode_refs(dest));
                let src = self.url(&dest, true).unwrap_or("");
                let _ = write!(out, "<img src=\"{}\" alt=\"{}\"", esc_url(src), esc(&alt));
                if let Some(t) = title {
                    let _ = write!(out, " title=\"{}\"", esc(&decode_refs(t)));
                }
                out.push_str(" />");
            }
            InlineKind::Autolink { url, email } => {
                let href = if *email { format!("mailto:{url}") } else { url.clone() };
                match self.url(&href, false) {
                    Some(h) => {
                        let _ = write!(out, "<a href=\"{}\">{}</a>", esc_url(h), esc(url));
                    }
                    None => escape_html(url, out),
                }
            }
            InlineKind::BareUrl { url } => match self.url(url, false) {
                Some(h) => {
                    let _ = write!(out, "<a href=\"{}\">{}</a>", esc_url(h), esc(n.span.slice(s)));
                }
                None => escape_html(n.span.slice(s), out),
            },
            InlineKind::Html => {
                let h = n.span.slice(s);
                if self.opts.trusted {
                    out.push_str(h); // trusted: raw, comments included
                } else {
                    self.san.borrow_mut().push(h, out);
                }
            }
            InlineKind::Math { content, delim } => {
                // `$$…$$` inside a paragraph: display style, still in the text flow (spec 01 §4.1)
                let display = *delim == markise_md::inline::MathInline::DoubleDollar;
                inline_math_html(content.slice(s), display, self.opts.math, out)
            }
            InlineKind::FootnoteRef { label } => {
                let num = self.footnote_numbers.get(label).copied().unwrap_or(0);
                let _ = write!(
                    out,
                    "<sup class='md-footnote'><a href='#dfref-footnote-{num}' name='ref-footnote-{num}'>{num}</a></sup>"
                );
            }
            InlineKind::Emoji { emoji } => {
                let _ = write!(out, "<span class='md-emoji'>{emoji}</span>");
            }
            InlineKind::Critic { kind, content } => match kind {
                CriticKind::Insert | CriticKind::New => {
                    out.push_str("<ins class=\"critic\">");
                    self.inlines(s, &n.children, out);
                    out.push_str("</ins>");
                }
                CriticKind::Delete | CriticKind::Old => {
                    out.push_str("<del class=\"critic\">");
                    self.inlines(s, &n.children, out);
                    out.push_str("</del>");
                }
                // a deletion followed by an insertion
                CriticKind::Substitute => self.inlines(s, &n.children, out),
                CriticKind::Highlight => self.critic_highlight(s, n, None, out),
                CriticKind::Comment => {
                    if self.opts.comments == CommentExport::Keep {
                        let _ = write!(
                            out,
                            "<span class=\"critic comment\">{}</span>",
                            esc(&comment_line(content.slice(s)))
                        );
                    }
                }
            },
            InlineKind::WikiLink {
                target, subpath, inner, ..
            } => {
                let c = inner.slice(s);
                let label = markise_md::wikilink::split(c).map(|w| w.label(c)).unwrap_or_default();
                let href = self
                    .opts
                    .wikilinks
                    .0
                    .as_ref()
                    .and_then(|r| r(target, subpath.as_deref()));
                match href.as_deref().and_then(|h| self.url(h, false)) {
                    Some(h) => {
                        let _ = write!(out, "<a href=\"{}\">{}</a>", esc_url(h), esc(&label));
                    }
                    None => escape_html(&label, out),
                }
            }
        }
    }
}

fn code_html<'t>(lang: &str, lines: impl Iterator<Item = &'t str>, hl: &CodeHighlight, out: &mut String) {
    // content as highlighted: every line followed by `\n`
    let mut content = String::new();
    for l in lines {
        content.push_str(l);
        content.push('\n');
    }
    let runs = match hl {
        CodeHighlight::Off => None,
        _ if lang.is_empty() => None,
        _ => markise_blocks::highlight::highlight_block(lang, &content),
    };
    if lang.is_empty() {
        out.push_str("<pre class='md-fences md-end-block' lang=''><code>");
    } else {
        let l = esc(lang);
        let scope = if runs.is_some() { " cm-s-inner" } else { "" };
        let _ = write!(
            out,
            "<pre class='md-fences md-end-block' lang='{l}'><code class='language-{l}{scope}' lang='{l}'>"
        );
    }
    match runs {
        Some(runs) => highlighted_html(&content, &runs, hl, out),
        None => escape_html(&content, out),
    }
    out.push_str("</code></pre>\n");
}

/// `content` with each run wrapped in a `<span>` of its token class (and inline style).
/// Class names and styles come only from [`TokenKind`] and theme colors, never from the
/// document, so the markup cannot be influenced by the code text (all text is escaped).
fn highlighted_html(content: &str, runs: &[TokenRun], hl: &CodeHighlight, out: &mut String) {
    let mut pos = 0;
    for r in runs {
        let (s, e) = (r.start as usize, r.end as usize);
        if s < pos || e > content.len() || !content.is_char_boundary(s) || !content.is_char_boundary(e) {
            continue;
        }
        escape_html(&content[pos..s], out);
        let _ = write!(out, "<span class='{}'", r.kind.cm_class());
        if let CodeHighlight::Inline(looks) = hl
            && let Some(look) = looks.get(r.kind as usize)
            && !look.is_plain()
        {
            out.push_str(" style='");
            if let Some(c) = look.color {
                let [rr, g, b, a] = c.0;
                if a == 255 {
                    let _ = write!(out, "color:#{rr:02x}{g:02x}{b:02x};");
                } else {
                    let _ = write!(out, "color:rgba({rr},{g},{b},{:.3});", a as f32 / 255.0);
                }
            }
            if look.bold {
                out.push_str("font-weight:bold;");
            }
            if look.italic {
                out.push_str("font-style:italic;");
            }
            out.push('\'');
        }
        out.push('>');
        escape_html(&content[s..e], out);
        out.push_str("</span>");
        pos = e;
    }
    escape_html(&content[pos..], out);
}

/// Lays out `tex` for SVG export; `Err` carries the message for the error placeholder.
fn math_svg(tex: &str, mode: MathMode, inline_position: bool) -> Result<String, String> {
    let layout = math::layout(tex, mode).map_err(|e| e.to_string())?;
    let opts = math::svg::SvgOptions {
        label: Some(tex.trim().to_string()),
        inline_position,
        ..Default::default()
    };
    Ok(math::svg::to_svg(&layout, &opts))
}

/// `display`: display style kept in the text flow (inline `$$…$$`).
fn inline_math_html(tex: &str, display: bool, mode: MathExport, out: &mut String) {
    let style = if display { MathMode::Display } else { MathMode::Inline };
    let rendered = match mode {
        MathExport::Svg => Some(math_svg(tex, style, true)),
        MathExport::Tex => None,
    };
    match rendered {
        Some(Ok(svg)) => {
            out.push_str("<span class='md-inline-math'><mjx-container class='MathJax' jax='SVG'>");
            out.push_str(&svg);
            out.push_str("</mjx-container></span>");
        }
        other => {
            out.push_str("<span class='md-inline-math");
            if let Some(Err(e)) = &other {
                let _ = write!(out, " md-math-error' title='{}", esc(e));
            }
            out.push_str("'>\\(");
            if display {
                // a client-side renderer keeps the display style inside `\(…\)`
                out.push_str("\\displaystyle ");
            }
            escape_html(tex, out);
            out.push_str("\\)</span>");
        }
    }
}

fn math_block_html(tex: &str, suffix: Option<&str>, mode: MathExport, out: &mut String) {
    let rendered = match mode {
        MathExport::Svg => Some(math_svg(tex, MathMode::Display, false)),
        MathExport::Tex => None,
    };
    out.push_str("<div class='mathjax-block md-math-block md-rawblock");
    if let Some(Err(_)) = &rendered {
        out.push_str(" md-math-error");
    }
    out.push('\'');
    if let Some(s) = suffix {
        let _ = write!(out, " data-math-tag-after=\"{}\"", esc(s));
    }
    if let Some(Err(e)) = &rendered {
        let _ = write!(out, " title='{}'", esc(e));
    }
    out.push_str("><div class='md-rawblock-container md-math-container'>");
    match rendered {
        Some(Ok(svg)) => {
            out.push_str("<mjx-container class='MathJax' jax='SVG' display='true'>");
            out.push_str(&svg);
            out.push_str("</mjx-container>");
        }
        _ => {
            out.push_str("\\[");
            escape_html(tex, out);
            out.push_str("\\]");
        }
    }
    out.push_str("</div></div>\n");
}

/// Smart punctuation at render time (R4.5.1 "convert on rendering"): `---` → em dash,
/// `--` → en dash, straight quotes → curly quotes.
fn smart(t: &str) -> String {
    let t = t.replace("---", "—").replace("--", "–");
    let mut out = String::with_capacity(t.len());
    let mut prev: Option<char> = None;
    for c in t.chars() {
        let opening = prev.is_none_or(|p| p.is_whitespace() || "([{".contains(p));
        match c {
            '"' => out.push(if opening { '“' } else { '”' }),
            '\'' => out.push(if opening { '‘' } else { '’' }),
            c => out.push(c),
        }
        prev = Some(c);
    }
    out
}

/// `name` values of `<a name="…">` tags.
fn anchor_names(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<a ") {
        let start = from + i;
        let end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
        let tag = &html[start..end];
        let tl = &lower[start..end];
        if let Some(n) = tl.find("name=") {
            let v = &tag[n + 5..];
            let v = v.trim_start();
            let (q, rest) = match v.chars().next() {
                Some(q @ ('"' | '\'')) => (Some(q), &v[1..]),
                _ => (None, v),
            };
            let val: String = match q {
                Some(q) => rest.split(q).next().unwrap_or("").to_string(),
                None => rest.split([' ', '>', '/']).next().unwrap_or("").to_string(),
            };
            if !val.is_empty() {
                out.push(val);
            }
        }
        from = end.max(start + 3);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_md::parse;

    fn body(md: &str) -> String {
        render_body(&parse(md), &HtmlOptions::default())
    }

    #[test]
    fn skeleton() {
        let html = render_document(&parse("---\ntitle: T & U\n---\n# H\n"), &HtmlOptions::default());
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>T &amp; U</title>"));
        assert!(html.contains("<body class='typora-export'>"));
        assert!(html.contains("<div class='typora-export-content'>"));
        assert!(html.contains("<div id='write'"));
        assert!(!html.contains("title: T"), "front matter must not be visible");
    }

    #[test]
    fn headings_and_anchors() {
        let h = body("# Intro\n# Intro\n# Write\n");
        assert!(h.contains("<h1 id='intro'>Intro</h1>"));
        assert!(h.contains("<h1 id='intro-2'>Intro</h1>"));
        assert!(h.contains("<h1 id='write-1'>Write</h1>"));
    }

    #[test]
    fn t36_html_block_rendered() {
        assert_eq!(body("<div>\n<b>x</b>\n</div>"), "<div>\n<b>x</b>\n</div>\n");
    }

    #[test]
    fn table_dom() {
        let h = body("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        assert!(h.starts_with("<figure class='table-figure'><table>"));
        assert!(h.contains("<th style='text-align:left;'>a</th>"));
        assert!(h.contains("<td style='text-align:right;'>2</td>"));
    }

    #[test]
    fn task_list_dom() {
        let h = body("- [x] done\n- [ ] todo\n");
        assert!(h.contains(
            "<li class='md-task-list-item task-list-item task-list-done'><input type='checkbox' checked='checked' />done</li>"
        ));
        assert!(h.contains("task-list-not-done'><input type='checkbox' />todo"));
    }

    #[test]
    fn alert_dom() {
        let h = body("> [!WARNING]\n> careful\n");
        assert!(h.contains("<div class='md-alert md-alert-warning warning'>"));
        assert!(h.contains("<p>careful</p>"));
    }

    #[test]
    fn footnotes_dom() {
        let h = body("a[^1]\n\n[^1]: note\n");
        assert!(h.contains("<sup class='md-footnote'><a href='#dfref-footnote-1' name='ref-footnote-1'>1</a></sup>"));
        assert!(h.contains("<div class='footnotes-area'><hr/>"));
        assert!(h.contains("<span class='md-fn-count'>1</span> note<a class='reversefootnote'"));
    }

    #[test]
    fn toc_dom() {
        let h = body("[TOC]\n\n# A\n## B\n");
        assert!(h.contains("<div class='md-toc' mdtype='toc'><p class='md-toc-content' role='list'>"));
        assert!(h.contains(
            "<span class='md-toc-item md-toc-h2' data-ref='n1'><a class='md-toc-inner' href='#b'>B</a></span>"
        ));
    }

    /// `n` headings with a `[toc]` every `n / m` headings (`m` TOCs).
    fn toc_doc(n: usize, m: usize) -> String {
        let every = n / m;
        let mut s = String::new();
        for i in 0..n {
            if i % every == 0 {
                s.push_str("[TOC]\n\n");
            }
            s.push_str(&format!("## H *{i}*\n\ntext {i}\n\n"));
        }
        s
    }

    /// Issue #29: the table of contents is formatted once per export however many `[toc]`
    /// blocks there are; every `[toc]` still lists all headings (spec 01 R3.11.1).
    #[test]
    fn toc_formatted_once_per_export() {
        for (n, m) in [(100, 1), (100, 10), (100, 100), (1000, 10), (1000, 100), (2000, 50)] {
            let doc = parse(&toc_doc(n, m));
            let opts = HtmlOptions::default();
            let mut ctx = Ctx::new(&doc, &opts);
            let html = ctx.render_write();
            assert_eq!(ctx.headings.len(), n);
            // work: linear in the headings, independent of the TOC count
            assert_eq!(ctx.toc_entries_formatted, n, "{n} headings × {m} TOCs");
            // semantics: every TOC lists every heading
            assert_eq!(html.matches("<div class='md-toc'").count(), m);
            assert_eq!(html.matches("class='md-toc-item").count(), n * m);
            let tocs: Vec<&str> = html
                .split("<div class='md-toc'")
                .skip(1)
                .map(|t| t.split("</div>").next().unwrap_or(""))
                .collect();
            assert!(tocs.windows(2).all(|w| w[0] == w[1]), "all TOCs identical");
        }
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_bytes(1_340_000_000), "1.3 GB");
        assert_eq!(human_bytes(250_400_000), "250 MB");
        assert_eq!(human_bytes(12_345), "12 KB");
        assert_eq!(human_bytes(512), "512 bytes");
    }

    /// `render_document_limited` gives the exact size of `md`'s page when it passes `limit`
    /// and the identical page otherwise.
    fn check_limited(md: &str, opts: &HtmlOptions, limit: u64) -> u64 {
        let doc = parse(md);
        let real = render_document(&doc, opts);
        let size = real.len() as u64;
        assert!(size > limit, "the input must pass the limit ({size} bytes)");
        let over = render_document_limited(&doc, opts, limit).expect_err("over the limit");
        assert_eq!(over.bytes, size, "exact size");
        assert_eq!(over.tocs, toc_count(&doc));
        let (under, _) = render_document_limited(&doc, opts, size).expect("at the limit");
        assert_eq!(under, real, "under the limit: the same page");
        size
    }

    /// Issue #29 / Codex round 2 on #93: the size check is exact, so HTML escaping, syntax
    /// highlighting, rendered diagrams and embedded images cannot slip under the limit (a
    /// source-length estimate let all of these through with a 20 KB limit).
    #[test]
    fn size_limit_is_exact_for_expanding_content() {
        let limit = 20_000;
        let plain = HtmlOptions {
            no_style: true,
            ..Default::default()
        };
        // HTML escaping: 10k `&` → `&amp;`
        let amps = "&".repeat(10_000) + "\n";
        assert!(check_limited(&amps, &plain, limit) > 5 * 10_000);
        // syntax highlighting: token spans around every token
        let code = format!("```rust\n{}```\n", "let a = b + c * d;\n".repeat(200));
        let hl = HtmlOptions {
            highlight: CodeHighlight::Classes,
            ..plain.clone()
        };
        assert!(check_limited(&code, &hl, limit) > 4 * code.len() as u64);
        // rendered Mermaid SVG
        let mermaid = format!(
            "```mermaid\nflowchart TD\n{}```\n",
            (0..40).map(|i| format!("  N{i} --> N{}\n", i + 1)).collect::<String>()
        );
        assert!(check_limited(&mermaid, &plain, limit) > 10 * mermaid.len() as u64);
        // an embedded local image (base64 in the page)
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("big.png"), vec![7u8; 300_000]).unwrap();
        let embed = HtmlOptions {
            images: ImageLinks::Embed {
                assets: Assets {
                    doc_dir: d.path().to_path_buf(),
                    root_url: None,
                    out_dir: d.path().to_path_buf(),
                },
                max_bytes: 1 << 20,
                budget: 1 << 20,
            },
            ..plain.clone()
        };
        assert!(render_document(&parse("![x](big.png)\n"), &embed).contains("data:image/png;base64,"));
        assert!(check_limited("![x](big.png)\n", &embed, limit) > 400_000);
        // many TOCs; the counted blocks past the limit are dropped, not kept
        let tocs = toc_doc(400, 100);
        assert!(check_limited(&tocs, &plain, limit) > 100 * 400 * 50);
        let doc = parse(&tocs);
        let mut ctx = Ctx::new(&doc, &plain);
        let (kept, dropped) = ctx.render_write_limited(Some(limit));
        let toc = ctx.toc_html.as_ref().map_or(0, String::len);
        assert!(dropped > 0 && kept.len() <= toc + 4_096, "kept {} bytes", kept.len());
        // a normal document stays under the default limit unchanged
        let doc = parse("[TOC]\n\n# A\n\ntext & more\n");
        assert_eq!(
            render_document_limited(&doc, &plain, LARGE_HTML_BYTES).unwrap().0,
            render_document(&doc, &plain)
        );
    }

    #[test]
    fn toc_without_toc_blocks_formats_nothing() {
        let doc = parse("# A\n\n## B\n");
        let opts = HtmlOptions::default();
        let mut ctx = Ctx::new(&doc, &opts);
        ctx.render_write();
        assert_eq!(ctx.toc_entries_formatted, 0);
    }

    fn tex_body(md: &str) -> String {
        render_body(
            &markise_md::parse(md),
            &HtmlOptions {
                math: MathExport::Tex,
                ..Default::default()
            },
        )
    }

    #[test]
    fn code_and_math() {
        let h = tex_body("```rust\nfn a() {}\n```\n\n$$\nx<y\n$$\n");
        assert!(h.contains("<pre class='md-fences md-end-block' lang='rust'><code class='language-rust' lang='rust'>fn a() {}\n</code></pre>"));
        assert!(h.contains("<div class='mathjax-block md-math-block md-rawblock'><div class='md-rawblock-container md-math-container'>\\[x&lt;y\\]</div></div>"));
    }

    #[test]
    fn inline_dom() {
        let h = tex_body("**b** _e_ ~~d~~ `c` [l](u \"t\") ![i](s.png) <u>x</u> :smile: $m$\n");
        assert!(h.contains("<strong>b</strong> <em>e</em> <del>d</del> <code>c</code> <a href=\"u\" title=\"t\">l</a> <img src=\"s.png\" alt=\"i\" /> <u>x</u> <span class='md-emoji'>😄</span> <span class='md-inline-math'>\\(m\\)</span>"));
    }

    #[test]
    fn math_as_svg_by_default() {
        let h = body("$x^2$ and\n\n$$\n\\frac{a}{b}\n$$\n\n```math\ny\n```\n");
        assert!(h.contains("<span class='md-inline-math'><mjx-container class='MathJax' jax='SVG'><svg xmlns=\"http://www.w3.org/2000/svg\""), "{h}");
        assert!(h.contains("aria-label=\"x^2\""));
        assert!(h.contains("<div class='mathjax-block md-math-block md-rawblock'><div class='md-rawblock-container md-math-container'><mjx-container class='MathJax' jax='SVG' display='true'><svg"), "{h}");
        assert_eq!(
            h.matches("<mjx-container").count(),
            3,
            "inline, $$ block, ```math block"
        );
        assert!(!h.contains("\\("), "{h}");
    }

    #[test]
    fn inline_display_math_keeps_display_style() {
        // review round 5: `$$…$$` inside a paragraph is display style, still in the text flow
        let h = body("a $$\\sum_{i=1}^n x_i$$ b $\\sum_{i=1}^n x_i$ c\n");
        let svgs: Vec<&str> = h.split("<svg").skip(1).collect();
        assert_eq!(svgs.len(), 2, "{h}");
        assert!(
            svgs.iter()
                .all(|s| !s.split('>').next().unwrap().contains("display:block")),
            "{h}"
        );
        let height = |s: &str| -> f64 {
            let i = s.find("height=\"").unwrap() + 8;
            s[i..].split("em").next().unwrap().parse().unwrap()
        };
        assert!(
            height(svgs[0]) > height(svgs[1]) * 1.3,
            "display limits are taller: {h}"
        );
        assert_eq!(h.matches("<span class='md-inline-math'>").count(), 2);
        let t = tex_body("a $$x$$ b\n");
        assert!(
            t.contains("<span class='md-inline-math'>\\(\\displaystyle x\\)</span>"),
            "{t}"
        );
    }

    #[test]
    fn invalid_math_falls_back_to_source_with_error() {
        let h = body("$\\foo$\n\n$$\n\\frac{1}{\n$$\n\n$\\rule{100000000000000000000em}{1em}$\n");
        assert!(h.contains("\\(\\rule{100000000000000000000em}{1em}\\)"), "{h}");
        assert!(h.contains("<span class='md-inline-math md-math-error' title='"), "{h}");
        assert!(h.contains("\\(\\foo\\)</span>"), "{h}");
        assert!(
            h.contains("<div class='mathjax-block md-math-block md-rawblock md-math-error' title='"),
            "{h}"
        );
        assert!(!h.contains("<svg"));
    }

    #[test]
    fn math_block_suffix_is_kept_with_svg() {
        let h = body("$$ {eq:1}\nx\n$$\n");
        assert!(h.contains("data-math-tag-after=\"{eq:1}\""), "{h}");
        assert!(h.contains("<svg"));
    }

    #[test]
    fn lists_tight_and_loose() {
        assert_eq!(body("- a\n- b\n"), "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n");
        assert_eq!(
            body("3. a\n\n4. b\n"),
            "<ol start='3'>\n<li><p>a</p>\n</li>\n<li><p>b</p>\n</li>\n</ol>\n"
        );
    }

    #[test]
    fn html_anchor_uniqueness() {
        let h = body("<a name=\"intro\"></a>\n\n# Intro\n");
        assert!(h.contains("<h1 id='intro-2'>"));
    }

    // Review P1 #4: attribute injection through the fence info string.
    #[test]
    fn fence_info_cannot_inject_attributes() {
        let h = body("```x'onmouseover='alert(1)\ncode\n```\n");
        assert!(!h.contains("'onmouseover"), "{h}");
        assert!(h.contains("lang='x&#39;onmouseover=&#39;alert(1)'"), "{h}");
        let h = body("```x\"><script>\ncode\n```\n");
        assert!(!h.contains("<script>"), "{h}");
    }

    // Review P1 #5: safe-by-default export.
    #[test]
    fn unsafe_urls_and_raw_html_are_neutralised() {
        let md = "[a](javascript:alert(1)) ![i](javascript:x) <javascript:alert(1)>\n\n<script>alert(1)</script>\n\ntext <img src=x onerror=alert(1)> <script>bad()</script> end\n\n<div onclick=\"x()\"><a href=\"vbscript:x\">v</a></div>\n";
        let h = body(md);
        for bad in [
            "href=\"javascript",
            "src=\"javascript",
            "vbscript:",
            "<script",
            "onerror",
            "onclick",
            "bad()",
            "alert(1)</script",
        ] {
            assert!(!h.to_ascii_lowercase().contains(bad), "found {bad} in {h}");
        }
        assert!(h.contains("<a>a</a>"));
        assert!(h.contains("<img src=\"\" alt=\"i\" />"));
        assert!(h.contains("<img src=\"x\">"));
        // allowed schemes and relative links still work
        let h = body("[a](https://e.com) [b](rel/x.md) [c](#Some Heading) ![d](img/a.png) <mailto:a@b.c>\n");
        assert!(h.contains("href=\"https://e.com\""));
        assert!(h.contains("href=\"rel/x.md\""));
        assert!(h.contains("src=\"img/a.png\""));
    }

    // Codex review P1: front-matter additions obey C40.
    #[test]
    fn appended_markup_is_sanitized_unless_trusted() {
        let mut opts = HtmlOptions {
            head_extra: Some("<script>alert(1)</script><style>p{}</style>".into()),
            body_extra: Some("<img src=x onerror=alert(2)><b>ok</b>".into()),
            ..Default::default()
        };
        let h = render_document(&parse("# x\n"), &opts);
        let lower = h.to_ascii_lowercase();
        assert!(
            !lower.contains("<script") && !lower.contains("onerror") && !lower.contains("alert("),
            "{h}"
        );
        assert!(h.contains("<b>ok</b>"));
        opts.trusted = true;
        let h = render_document(&parse("# x\n"), &opts);
        assert!(h.contains("<script>alert(1)</script>"));
    }

    // Codex review P1: generated CSS cannot close the style element.
    #[test]
    fn css_cannot_close_the_style_element() {
        let opts = HtmlOptions {
            css: Some("/* </style><script>x()</script> */ a { font-family: \"</STYLE><script>y()</script>\"; }".into()),
            ..Default::default()
        };
        let h = render_document(&parse("x\n"), &opts);
        let lower = h.to_ascii_lowercase();
        assert_eq!(lower.matches("</style").count(), 1, "{h}");
        assert!(!lower.contains("<script"), "{h}");
    }

    // Codex round 6: images over the embedding cap stay (rebased) links, with a warning.
    #[test]
    fn oversized_images_are_linked_not_embedded() {
        let d = std::env::temp_dir().join(format!("markise-embed-{}", std::process::id()));
        let docs = d.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        std::fs::write(docs.join("big.png"), vec![0u8; 4096]).unwrap();
        let opts = HtmlOptions {
            images: ImageLinks::Embed {
                assets: Assets {
                    doc_dir: docs.clone(),
                    root_url: None,
                    out_dir: d.join("out"),
                },
                max_bytes: 1024,
                budget: 1 << 20,
            },
            ..Default::default()
        };
        let (html, warnings) = render_document_report(&parse("![x](big.png)\n"), &opts);
        assert!(html.contains("src=\"../docs/big.png\""), "{html}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    // Codex round 9: thousands of rejected images keep a bounded warning list.
    #[test]
    fn warnings_are_bounded() {
        let d = std::env::temp_dir().join(format!("markise-warn-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let md: String = (0..2000).map(|i| format!("![x](missing{i}.png)\n\n")).collect();
        let opts = HtmlOptions {
            images: ImageLinks::Embed {
                assets: Assets {
                    doc_dir: d.clone(),
                    root_url: None,
                    out_dir: d.clone(),
                },
                max_bytes: 1024,
                budget: 1 << 20,
            },
            ..Default::default()
        };
        let (_, warnings) = render_document_report(&parse(&md), &opts);
        assert_eq!(warnings.len(), MAX_WARNINGS + 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trusted_export_keeps_raw_html() {
        let opts = HtmlOptions {
            trusted: true,
            ..Default::default()
        };
        let h = render_body(&parse("<script>x()</script>\n\n[a](javascript:y)\n"), &opts);
        assert!(h.contains("<script>x()</script>"));
        assert!(h.contains("href=\"javascript:y\""));
    }

    // Owner decision D2: footnotes numbered by first reference; unreferenced ones last.
    #[test]
    fn footnotes_numbered_by_first_reference() {
        let h = body("b[^b] a[^a] b again[^b]\n\n[^a]: A\n[^b]: B\n[^c]: C\n");
        assert!(h.contains("b<sup class='md-footnote'><a href='#dfref-footnote-1' name='ref-footnote-1'>1</a></sup> a<sup class='md-footnote'><a href='#dfref-footnote-2'"));
        let area = &h[h.find("footnotes-area").unwrap()..];
        let pos = |t: &str| area.find(t).unwrap();
        assert!(
            pos("</span> B") < pos("</span> A") && pos("</span> A") < pos("</span> C"),
            "{area}"
        );
        assert!(area.contains("<span class='md-fn-count'>3</span> C"));
    }
}
