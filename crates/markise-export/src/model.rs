//! The export model: a small, resolved semantic tree built from a parsed [`Document`] for the
//! non-HTML exporters (PDF/PNG layout in `markise-pdf`, the Pandoc bridge in [`crate::pandoc`]).
//!
//! Everything a back end needs is resolved here once: inline parsing, entity decoding, link
//! reference resolution, heading anchors (spec 01 §7, the same ids as the HTML export),
//! footnote numbering by first reference (D2), code highlighting runs, alert markers removed,
//! front-matter metadata. Raw HTML is reduced to the inline formatting tags a non-HTML target
//! can express (`<u>`, `<b>`, `<i>`, `<s>`, `<sub>`, `<sup>`, `<mark>`, `<kbd>`, `<br>`); other
//! raw HTML is kept as [`ExInline::RawHtml`] / [`ExBlock::RawHtml`] for targets that understand
//! it (Pandoc) and dropped by the page layout.

use std::collections::HashMap;

use markise_md::inline::{Inline, InlineContext, InlineKind, decode_entity, decode_refs, parse_inlines, plain_text};
use markise_md::slug::{SlugTracker, slug};
use markise_md::{AlertKind, Align, Block, BlockKind, Document, frontmatter};
use markise_theme::TokenRun;

/// Inline content.
#[derive(Clone, Debug, PartialEq)]
pub enum ExInline {
    Text(String),
    /// Code span (content after CommonMark stripping).
    Code(String),
    Emph(Vec<ExInline>),
    Strong(Vec<ExInline>),
    Strike(Vec<ExInline>),
    /// `==mark==` / `<mark>`.
    Highlight(Vec<ExInline>),
    Underline(Vec<ExInline>),
    Sub(Vec<ExInline>),
    Sup(Vec<ExInline>),
    /// `<kbd>`.
    Kbd(Vec<ExInline>),
    Link {
        /// Resolved URL; internal links are `#anchor` with the heading slug.
        url: String,
        title: Option<String>,
        children: Vec<ExInline>,
    },
    Image {
        src: String,
        alt: String,
        title: Option<String>,
    },
    /// Inline math, TeX source (rendered as styled source; math rendering is postponed).
    Math(String),
    /// Footnote reference with its number.
    FootnoteRef(usize),
    SoftBreak,
    HardBreak,
    /// Raw inline HTML that has no structural equivalent.
    RawHtml(String),
}

/// One list item.
#[derive(Clone, Debug, PartialEq)]
pub struct ExItem {
    /// Task state (`Some(checked)`) for task items.
    pub task: Option<bool>,
    pub blocks: Vec<ExBlock>,
}

/// Block content.
#[derive(Clone, Debug, PartialEq)]
pub enum ExBlock {
    Heading {
        level: u8,
        /// Anchor id (same as the HTML export).
        id: String,
        content: Vec<ExInline>,
    },
    Paragraph(Vec<ExInline>),
    /// Fenced or indented code; `runs` are highlight token runs over `text` (each line ends
    /// with `\n` in `text`).
    Code {
        lang: String,
        text: String,
        runs: Option<Vec<TokenRun>>,
    },
    /// Display math (TeX source).
    Math(String),
    /// Diagram fence (`mermaid`, `sequence`, `flow`): source.
    Diagram {
        lang: String,
        source: String,
    },
    RawHtml(String),
    Quote(Vec<ExBlock>),
    Alert {
        kind: AlertKind,
        blocks: Vec<ExBlock>,
    },
    List {
        ordered: bool,
        start: u64,
        tight: bool,
        items: Vec<ExItem>,
    },
    Table {
        aligns: Vec<Align>,
        header: Vec<Vec<ExInline>>,
        rows: Vec<Vec<Vec<ExInline>>>,
    },
    Rule,
    /// `[toc]` marker (back ends expand it from [`ExDoc::headings`]).
    Toc,
}

/// A heading entry (outline, TOC, bookmarks).
#[derive(Clone, Debug, PartialEq)]
pub struct HeadingEntry {
    pub level: u8,
    pub id: String,
    /// Plain text of the heading.
    pub text: String,
}

/// The resolved export document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExDoc {
    /// Front matter (shallow keys, in order).
    pub meta: Vec<(String, String)>,
    /// Raw front-matter YAML (for nested lookups by [`crate::vars`]).
    pub meta_yaml: String,
    pub blocks: Vec<ExBlock>,
    pub headings: Vec<HeadingEntry>,
    /// Footnotes in number order: (number, content).
    pub footnotes: Vec<(usize, Vec<ExBlock>)>,
}

impl ExDoc {
    /// `title` front-matter key, else the first heading, else `fallback`.
    pub fn title(&self, fallback: &str) -> String {
        self.meta_value("title")
            .map(str::to_string)
            .or_else(|| self.headings.first().map(|h| h.text.clone()))
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Front-matter value (case-insensitive key); `unset` counts as missing (spec 05 §3).
    pub fn meta_value(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.trim().is_empty() && *v != "unset")
    }

    /// The document's resource root (`typora-root-url`, R8.1), exactly as the viewer and the
    /// HTML export read it (`markise_md::frontmatter::resource_root`).
    pub fn resource_root(&self) -> Option<&str> {
        frontmatter::get(&self.meta, "typora-root-url")
    }

    pub fn author(&self) -> Option<&str> {
        self.meta_value("author")
    }
}

/// Options of [`ExDoc::build`].
#[derive(Clone, Debug, Default)]
pub struct ModelOptions {
    /// Run the code highlighter on fenced code with a language.
    pub highlight: bool,
    /// Review comments kept (as bracketed italic text after the passage, highlights
    /// highlighted) or stripped (ADR 0012). Suggestions are always shown as underlined
    /// insertions and struck deletions.
    pub comments: crate::CommentExport,
}

/// Plain text of inline content (alt text, outline titles).
pub fn inline_text(nodes: &[ExInline]) -> String {
    let mut s = String::new();
    fn go(nodes: &[ExInline], s: &mut String) {
        for n in nodes {
            match n {
                ExInline::Text(t) | ExInline::Code(t) | ExInline::Math(t) => s.push_str(t),
                ExInline::Emph(c)
                | ExInline::Strong(c)
                | ExInline::Strike(c)
                | ExInline::Highlight(c)
                | ExInline::Underline(c)
                | ExInline::Sub(c)
                | ExInline::Sup(c)
                | ExInline::Kbd(c)
                | ExInline::Link { children: c, .. } => go(c, s),
                ExInline::Image { alt, .. } => s.push_str(alt),
                ExInline::FootnoteRef(n) => s.push_str(&n.to_string()),
                ExInline::SoftBreak | ExInline::HardBreak => s.push(' '),
                ExInline::RawHtml(_) => {}
            }
        }
    }
    go(nodes, &mut s);
    s
}

struct Builder<'a> {
    doc: &'a Document,
    text: &'a str,
    ictx: InlineContext,
    opts: &'a ModelOptions,
    heading_ids: Vec<String>,
    next_heading: usize,
    footnote_numbers: HashMap<String, usize>,
    footnotes: Vec<(usize, Vec<ExBlock>)>,
}

impl ExDoc {
    /// Builds the export model of `doc`.
    pub fn build(doc: &Document, opts: &ModelOptions) -> ExDoc {
        let labels = doc.footnote_labels();
        let mut b = Builder {
            doc,
            text: doc.text(),
            ictx: InlineContext {
                link_defs: doc.link_defs(),
                footnotes: labels.iter().cloned().collect(),
            },
            opts,
            heading_ids: Vec::new(),
            next_heading: 0,
            footnote_numbers: HashMap::new(),
            footnotes: Vec::new(),
        };
        b.number_footnotes(&labels);
        let headings = b.collect_headings();
        let mut blocks = Vec::new();
        for blk in doc.blocks() {
            b.block(blk, &mut blocks);
        }
        let mut footnotes = std::mem::take(&mut b.footnotes);
        footnotes.sort_by_key(|(n, _)| *n);
        let meta_yaml = match doc.blocks().first() {
            Some(f) if matches!(f.kind, BlockKind::FrontMatter { .. }) => f.content(doc.text()),
            _ => String::new(),
        };
        ExDoc {
            meta: frontmatter::front_matter(doc),
            meta_yaml,
            blocks,
            headings,
            footnotes,
        }
    }
}

impl Builder<'_> {
    fn parse(&self, content: &str) -> Vec<Inline> {
        parse_inlines(content, self.doc.options(), &self.ictx)
    }

    fn number_footnotes(&mut self, labels: &[String]) {
        if labels.is_empty() {
            return;
        }
        let mut order = Vec::new();
        fn refs(nodes: &[Inline], out: &mut Vec<String>) {
            for n in nodes {
                if let InlineKind::FootnoteRef { label } = &n.kind {
                    out.push(label.clone());
                }
                refs(&n.children, out);
            }
        }
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
        for c in contents {
            if c.contains("[^") {
                refs(&self.parse(&c), &mut order);
            }
        }
        order.extend(labels.iter().cloned());
        for l in order {
            let n = self.footnote_numbers.len() + 1;
            self.footnote_numbers.entry(l).or_insert(n);
        }
    }

    fn collect_headings(&mut self) -> Vec<HeadingEntry> {
        let mut tracker = SlugTracker::new();
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
        let mut out = Vec::new();
        for (level, content) in found {
            match level {
                Some(level) => {
                    let nodes = self.parse(&content);
                    let text = plain_text(&content, &nodes);
                    let id = tracker.anchor_for(&text);
                    self.heading_ids.push(id.clone());
                    let text = inline_text(&self.inlines(&content, &nodes));
                    out.push(HeadingEntry { level, id, text });
                }
                None => {
                    for name in crate::anchor_names(&content) {
                        tracker.add_existing(&name);
                    }
                }
            }
        }
        out
    }

    fn inline_content(&self, content: &str) -> Vec<ExInline> {
        let nodes = self.parse(content);
        self.inlines(content, &nodes)
    }

    fn block(&mut self, b: &Block, out: &mut Vec<ExBlock>) {
        let text = self.text;
        match &b.kind {
            BlockKind::Paragraph => {
                let c = self.inline_content(&b.content(text));
                if !c.is_empty() {
                    out.push(ExBlock::Paragraph(c));
                }
            }
            BlockKind::Heading { level, .. } => {
                let id = self.heading_ids.get(self.next_heading).cloned().unwrap_or_default();
                self.next_heading += 1;
                out.push(ExBlock::Heading {
                    level: *level,
                    id,
                    content: self.inline_content(&b.content(text)),
                });
            }
            BlockKind::ThematicBreak => out.push(ExBlock::Rule),
            BlockKind::FencedCode { info, .. } => {
                let info = info.slice(text);
                let lang = markise_md::inline::unescape(&decode_refs(info.split_whitespace().next().unwrap_or("")));
                let opts = self.doc.options();
                if opts.code_math && lang.eq_ignore_ascii_case("math") {
                    out.push(ExBlock::Math(b.content(text)));
                } else if opts.diagrams && matches!(lang.to_ascii_lowercase().as_str(), "mermaid" | "sequence" | "flow")
                {
                    out.push(ExBlock::Diagram {
                        lang,
                        source: b.content(text),
                    });
                } else {
                    let code = code_text(b, text);
                    let runs = if self.opts.highlight && !lang.is_empty() {
                        markise_blocks::highlight::highlight_block(&lang, &code)
                    } else {
                        None
                    };
                    out.push(ExBlock::Code { lang, text: code, runs });
                }
            }
            BlockKind::IndentedCode => out.push(ExBlock::Code {
                lang: String::new(),
                text: code_text(b, text),
                runs: None,
            }),
            BlockKind::MathBlock { .. } => out.push(ExBlock::Math(b.content(text))),
            BlockKind::HtmlBlock => {
                let raw = b.content(text);
                if !raw.trim().is_empty() {
                    out.push(ExBlock::RawHtml(raw));
                }
            }
            BlockKind::Table { aligns } => {
                let rows = markise_md::table::cell_spans(text, b);
                let mut cells = rows.iter().map(|row| {
                    aligns
                        .iter()
                        .enumerate()
                        .map(|(ci, _)| {
                            let cell = row.get(ci).map(|s| s.slice(text)).unwrap_or("").replace("\\|", "|");
                            self.inline_content(&cell)
                        })
                        .collect::<Vec<_>>()
                });
                let header = cells.next().unwrap_or_default();
                let body: Vec<_> = cells.collect();
                out.push(ExBlock::Table {
                    aligns: aligns.clone(),
                    header,
                    rows: body,
                });
            }
            BlockKind::BlockQuote { alert } => {
                let mut inner = Vec::new();
                for (i, c) in b.children.iter().enumerate() {
                    if alert.is_some() && i == 0 && matches!(c.kind, BlockKind::Paragraph) {
                        // drop the `[!KIND]` marker line
                        if c.lines.len() > 1 {
                            let rest: Vec<&str> = c.lines[1..].iter().map(|l| l.slice(text)).collect();
                            let content = self.inline_content(&rest.join("\n"));
                            if !content.is_empty() {
                                inner.push(ExBlock::Paragraph(content));
                            }
                        }
                        continue;
                    }
                    self.block(c, &mut inner);
                }
                out.push(match alert {
                    Some(kind) => ExBlock::Alert {
                        kind: *kind,
                        blocks: inner,
                    },
                    None => ExBlock::Quote(inner),
                });
            }
            BlockKind::List {
                ordered, start, tight, ..
            } => {
                let mut items = Vec::new();
                for item in &b.children {
                    let BlockKind::ListItem { task, .. } = &item.kind else {
                        continue;
                    };
                    let mut blocks = Vec::new();
                    for c in &item.children {
                        self.block(c, &mut blocks);
                    }
                    items.push(ExItem {
                        task: task.map(|(c, _)| c),
                        blocks,
                    });
                }
                out.push(ExBlock::List {
                    ordered: *ordered,
                    start: *start,
                    tight: *tight,
                    items,
                });
            }
            BlockKind::ListItem { .. } => {
                for c in &b.children {
                    self.block(c, out);
                }
            }
            BlockKind::FrontMatter { .. } | BlockKind::LinkRefDef { .. } => {}
            BlockKind::Toc => out.push(ExBlock::Toc),
            BlockKind::FootnoteDef { label } => {
                let key = markise_md::inline::normalize_label(label.slice(text));
                let n = self.footnote_numbers.get(&key).copied().unwrap_or(0);
                let mut inner = Vec::new();
                for c in &b.children {
                    self.block(c, &mut inner);
                }
                self.footnotes.push((n, inner));
            }
        }
    }

    fn inlines(&self, s: &str, nodes: &[Inline]) -> Vec<ExInline> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < nodes.len() {
            let n = &nodes[i];
            // Recognised inline HTML formatting pairs (`<u>…</u>` etc.) wrap their siblings.
            if let InlineKind::Html = n.kind
                && let Some(tag) = html_open_tag(n.span.slice(s))
            {
                if tag == "br" {
                    out.push(ExInline::HardBreak);
                    i += 1;
                    continue;
                }
                if let Some(close) = nodes[i + 1..]
                    .iter()
                    .position(|m| matches!(m.kind, InlineKind::Html) && html_close_tag(m.span.slice(s)) == Some(tag))
                {
                    let inner = self.inlines(s, &nodes[i + 1..i + 1 + close]);
                    out.push(match tag {
                        "u" | "ins" => ExInline::Underline(inner),
                        "b" | "strong" => ExInline::Strong(inner),
                        "i" | "em" => ExInline::Emph(inner),
                        "s" | "del" | "strike" => ExInline::Strike(inner),
                        "sub" => ExInline::Sub(inner),
                        "sup" => ExInline::Sup(inner),
                        "mark" => ExInline::Highlight(inner),
                        _ => ExInline::Kbd(inner),
                    });
                    i += close + 2;
                    continue;
                }
            }
            self.inline(s, n, &mut out);
            i += 1;
        }
        merge_text(out)
    }

    fn inline(&self, s: &str, n: &Inline, out: &mut Vec<ExInline>) {
        match &n.kind {
            InlineKind::Text => {
                let t = n.span.slice(s);
                if self.doc.options().smart_punctuation_render {
                    out.push(ExInline::Text(crate::smart(t)));
                } else {
                    out.push(ExInline::Text(t.to_string()));
                }
            }
            InlineKind::Escape => out.push(ExInline::Text(s[n.span.start + 1..n.span.end].to_string())),
            InlineKind::Entity => out.push(ExInline::Text(decode_entity(n.span.slice(s)))),
            InlineKind::Code { content } => {
                let c = content.slice(s).replace('\n', " ");
                let c = if c.len() >= 2 && c.starts_with(' ') && c.ends_with(' ') && !c.trim().is_empty() {
                    c[1..c.len() - 1].to_string()
                } else {
                    c
                };
                out.push(ExInline::Code(c));
            }
            InlineKind::SoftBreak => out.push(ExInline::SoftBreak),
            InlineKind::HardBreak => out.push(ExInline::HardBreak),
            InlineKind::Emph => out.push(ExInline::Emph(self.inlines(s, &n.children))),
            InlineKind::Strong => out.push(ExInline::Strong(self.inlines(s, &n.children))),
            InlineKind::Strike => out.push(ExInline::Strike(self.inlines(s, &n.children))),
            InlineKind::Highlight => out.push(ExInline::Highlight(self.inlines(s, &n.children))),
            InlineKind::Sub => out.push(ExInline::Sub(self.inlines(s, &n.children))),
            InlineKind::Sup => out.push(ExInline::Sup(self.inlines(s, &n.children))),
            InlineKind::Link { dest, title, .. } => {
                let dest = decode_refs(dest);
                let url = match dest.strip_prefix('#') {
                    Some(frag) => format!("#{}", slug(frag)),
                    None => dest,
                };
                out.push(ExInline::Link {
                    url,
                    title: title.as_deref().map(decode_refs),
                    children: self.inlines(s, &n.children),
                });
            }
            InlineKind::Image { dest, title, .. } => out.push(ExInline::Image {
                src: decode_refs(dest),
                alt: plain_text(s, &n.children),
                title: title.as_deref().map(decode_refs),
            }),
            InlineKind::Autolink { url, email } => out.push(ExInline::Link {
                url: if *email { format!("mailto:{url}") } else { url.clone() },
                title: None,
                children: vec![ExInline::Text(url.clone())],
            }),
            InlineKind::BareUrl { url } => out.push(ExInline::Link {
                url: url.clone(),
                title: None,
                children: vec![ExInline::Text(n.span.slice(s).to_string())],
            }),
            InlineKind::Html => out.push(ExInline::RawHtml(n.span.slice(s).to_string())),
            InlineKind::Math { content, .. } => out.push(ExInline::Math(content.slice(s).to_string())),
            InlineKind::FootnoteRef { label } => out.push(ExInline::FootnoteRef(
                self.footnote_numbers.get(label).copied().unwrap_or(0),
            )),
            InlineKind::Emoji { emoji } => out.push(ExInline::Text((*emoji).to_string())),
            InlineKind::Critic { kind, content } => {
                use markise_md::inline::CriticKind as K;
                let keep = self.opts.comments == crate::CommentExport::Keep;
                match kind {
                    K::Insert | K::New => out.push(ExInline::Underline(self.inlines(s, &n.children))),
                    K::Delete | K::Old => out.push(ExInline::Strike(self.inlines(s, &n.children))),
                    K::Substitute => out.extend(self.inlines(s, &n.children)),
                    K::Highlight if keep => out.push(ExInline::Highlight(self.inlines(s, &n.children))),
                    K::Highlight => out.extend(self.inlines(s, &n.children)),
                    K::Comment if keep => {
                        let t = content.slice(s).split(['\n', '\r']).collect::<Vec<_>>().join(" ");
                        out.push(ExInline::Emph(vec![ExInline::Text(format!(" [{}]", t.trim()))]));
                    }
                    K::Comment => {}
                }
            }
            // wikilinks: their label as text (PDF/PNG/Pandoc have no vault to resolve against)
            InlineKind::WikiLink { inner, .. } => {
                let c = inner.slice(s);
                if let Some(w) = markise_md::wikilink::split(c) {
                    out.push(ExInline::Text(w.label(c)));
                }
            }
        }
    }
}

/// Code block text: every line followed by `\n`.
fn code_text(b: &Block, text: &str) -> String {
    let mut s = String::new();
    for l in &b.lines {
        s.push_str(l.slice(text));
        s.push('\n');
    }
    s
}

const FORMAT_TAGS: [&str; 15] = [
    "u", "ins", "b", "strong", "i", "em", "s", "del", "strike", "sub", "sup", "mark", "kbd", "br", "code",
];

/// Lower-case name of a recognised opening formatting tag (`<u>`, `<br/>`, …; no attributes).
fn html_open_tag(h: &str) -> Option<&'static str> {
    let inner = h.strip_prefix('<')?.strip_suffix('>')?;
    let inner = inner.trim_end_matches('/').trim();
    FORMAT_TAGS
        .iter()
        .copied()
        .find(|t| inner.eq_ignore_ascii_case(t) && *t != "code")
}

fn html_close_tag(h: &str) -> Option<&'static str> {
    let inner = h.strip_prefix("</")?.strip_suffix('>')?.trim();
    FORMAT_TAGS.iter().copied().find(|t| inner.eq_ignore_ascii_case(t))
}

/// Merges adjacent text nodes.
fn merge_text(v: Vec<ExInline>) -> Vec<ExInline> {
    let mut out: Vec<ExInline> = Vec::with_capacity(v.len());
    for n in v {
        if let ExInline::Text(t) = &n
            && let Some(ExInline::Text(prev)) = out.last_mut()
        {
            prev.push_str(t);
            continue;
        }
        if matches!(&n, ExInline::Text(t) if t.is_empty()) {
            continue;
        }
        out.push(n);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_md::parse;

    fn model(md: &str) -> ExDoc {
        ExDoc::build(
            &parse(md),
            &ModelOptions {
                highlight: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn headings_ids_match_html_export() {
        let d = model("# Intro\n# Intro\n## A *b*\n");
        let ids: Vec<_> = d.headings.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["intro", "intro-2", "a-b"]);
        assert_eq!(d.headings[2].text, "A b");
        assert!(matches!(&d.blocks[2], ExBlock::Heading { level: 2, id, .. } if id == "a-b"));
    }

    #[test]
    fn inline_resolution() {
        let d = model("a **b** [l][r] <u>u</u> &amp; <mark>m</mark>\n\n[r]: http://x.org \"T\"\n");
        let ExBlock::Paragraph(p) = &d.blocks[0] else { panic!() };
        assert_eq!(p[0], ExInline::Text("a ".into()));
        assert_eq!(p[1], ExInline::Strong(vec![ExInline::Text("b".into())]));
        assert!(matches!(&p[3], ExInline::Link { url, title: Some(t), .. } if url == "http://x.org" && t == "T"));
        assert_eq!(p[5], ExInline::Underline(vec![ExInline::Text("u".into())]));
        assert_eq!(p[6], ExInline::Text(" & ".into()));
        assert_eq!(p[7], ExInline::Highlight(vec![ExInline::Text("m".into())]));
    }

    #[test]
    fn alerts_footnotes_tasks_code() {
        let d = model(
            "> [!NOTE]\n> hi\n\nx[^b] y[^a]\n\n[^a]: A\n[^b]: B\n\n- [x] done\n- open\n\n```rust\nfn a() {}\n```\n",
        );
        assert!(matches!(&d.blocks[0], ExBlock::Alert { kind: AlertKind::Note, blocks } if blocks.len() == 1));
        let ExBlock::Paragraph(p) = &d.blocks[1] else { panic!() };
        assert_eq!(p[1], ExInline::FootnoteRef(1));
        assert_eq!(d.footnotes.len(), 2);
        assert_eq!(d.footnotes[0].0, 1);
        assert!(
            matches!(&d.blocks[2], ExBlock::List { items, .. } if items[0].task == Some(true) && items[1].task.is_none())
        );
        assert!(matches!(&d.blocks[3], ExBlock::Code { lang, runs: Some(r), .. } if lang == "rust" && !r.is_empty()));
    }

    #[test]
    fn front_matter_meta() {
        let d = model("---\ntitle: T\nauthor: unset\n---\n# H\n");
        assert_eq!(d.title("f"), "T");
        assert_eq!(d.author(), None);
        assert!(d.meta_yaml.contains("title: T"));
    }
}
