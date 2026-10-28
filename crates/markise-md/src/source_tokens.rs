//! Source-mode tokens (spec 02 §12.1.1; `FormatDoc::source_tokens`): token classes for the
//! source editor's colouring, computed from the block tree and the inline parse — exact
//! spans, no regular expressions.
//!
//! Heading text takes a heading class, emphasis / strong / strike / highlight their own,
//! code spans and code blocks the code classes, link text and URLs the link classes; markers
//! (`#`, `*`, `_`, backticks, fences, `>`, list markers and task boxes, link brackets, table
//! pipes and delimiter rows, setext underlines) are dimmed; front matter, math, HTML, `[toc]`
//! and link titles are tinted as metadata.
//!
//! Cost follows the requested range: only the top-level blocks overlapping it are visited, and
//! inside large containers, code blocks and tables only the children / lines overlapping it
//! (binary search). A paragraph is parsed inline as a whole while it is small; a very large
//! paragraph only on the lines overlapping the range (constructs spanning those lines' edges
//! may then be coloured as plain text).

use std::ops::Range;

use markise_doc::{SourceTok, SourceToken};

use crate::Document;
use crate::block::{Block, BlockKind};
use crate::inline::{Inline, InlineKind, parse_inlines};
use crate::text::Span;

/// Paragraphs longer than this are parsed inline line by line (range-limited).
const WHOLE_PARAGRAPH_BYTES: usize = 64 * 1024;
/// Context parsed around the range on a line longer than [`WHOLE_PARAGRAPH_BYTES`].
const LINE_WINDOW: usize = 4 * 1024;

/// Paints token classes into a per-byte buffer covering the requested range.
struct Painter<'a> {
    text: &'a str,
    doc: &'a Document,
    /// The requested (absolute) range; `cls[0]` is `range.start`.
    range: Range<usize>,
    cls: Vec<u8>,
}

/// Indices of the spans in `spans` (sorted, relative to `origin`) overlapping `r` (absolute).
fn overlapping(spans: &[Span], origin: usize, r: &Range<usize>) -> Range<usize> {
    let a = spans.partition_point(|s| origin + s.end < r.start);
    let b = a + spans[a..].partition_point(|s| origin + s.start < r.end);
    a..b.max(a)
}

impl Painter<'_> {
    fn clip(&self, start: usize, end: usize) -> (usize, usize) {
        let s = start.clamp(self.range.start, self.range.end) - self.range.start;
        let e = end.clamp(self.range.start, self.range.end) - self.range.start;
        (s, e)
    }

    fn paint(&mut self, start: usize, end: usize, tok: SourceTok) {
        let (s, e) = self.clip(start, end);
        if s < e {
            self.cls[s..e].fill(tok as u8);
        }
    }

    /// Like `paint`, but keeps classes of a higher priority already painted.
    fn under(&mut self, start: usize, end: usize, tok: SourceTok) {
        let (s, e) = self.clip(start, end);
        for c in &mut self.cls[s..e.max(s)] {
            if *c < tok as u8 {
                *c = tok as u8;
            }
        }
    }

    fn touches(&self, start: usize, end: usize) -> bool {
        start < self.range.end && end >= self.range.start
    }

    fn children(&mut self, b: &Block, origin: usize) {
        let r = self.range.clone();
        let a = b.children.partition_point(|c| origin + c.span.end < r.start);
        let mut i = a;
        while i < b.children.len() && origin + b.children[i].span.start < r.end {
            self.block(&b.children[i], origin);
            i += 1;
        }
    }

    fn block(&mut self, b: &Block, origin: usize) {
        let at = |s: usize| origin + s;
        let (bs, be) = (at(b.span.start), at(b.span.end));
        if !self.touches(bs, be) {
            return;
        }
        match &b.kind {
            BlockKind::Paragraph => self.inlines(b, origin, None),
            BlockKind::Heading { level, setext, .. } => {
                self.paint(bs, be, SourceTok::heading(*level));
                if *setext {
                    // the underline: everything after the last text line
                    if let Some(l) = b.lines.last() {
                        self.paint(at(l.end), be, SourceTok::Marker);
                    }
                } else {
                    let hashes = self.text[bs..be].bytes().take_while(|&c| c == b'#').count();
                    self.paint(bs, bs + hashes, SourceTok::Marker);
                    if let Some(l) = b.lines.first() {
                        // closing hashes
                        let after = at(l.end);
                        if after < be {
                            self.paint(after, be, SourceTok::Marker);
                        }
                    }
                }
                self.inlines(b, origin, Some(SourceTok::heading(*level)));
            }
            BlockKind::ThematicBreak => self.paint(bs, be, SourceTok::Marker),
            BlockKind::FencedCode { opener, closer, .. } => {
                for k in overlapping(&b.lines, origin, &self.range.clone()) {
                    let l = b.lines[k];
                    self.paint(at(l.start), at(l.end), SourceTok::CodeBlock);
                }
                self.paint(at(opener.start), at(opener.end), SourceTok::Marker);
                if let Some(c) = closer {
                    self.paint(at(c.start), at(c.end), SourceTok::Marker);
                }
            }
            BlockKind::IndentedCode => {
                for k in overlapping(&b.lines, origin, &self.range.clone()) {
                    let l = b.lines[k];
                    self.paint(at(l.start), at(l.end), SourceTok::CodeBlock);
                }
            }
            BlockKind::MathBlock { opener, closer, .. } => {
                self.paint(bs, be, SourceTok::Meta);
                self.paint(at(opener.start), at(opener.end), SourceTok::Marker);
                self.paint(at(closer.start), at(closer.end), SourceTok::Marker);
            }
            BlockKind::HtmlBlock | BlockKind::Toc => self.paint(bs, be, SourceTok::Meta),
            BlockKind::FrontMatter { opener, closer } => {
                self.paint(bs, be, SourceTok::Meta);
                self.paint(at(opener.start), at(opener.end), SourceTok::Marker);
                self.paint(at(closer.start), at(closer.end), SourceTok::Marker);
            }
            BlockKind::Table { .. } => {
                for k in overlapping(&b.lines, origin, &self.range.clone()) {
                    let l = b.lines[k];
                    let (s, e) = (at(l.start), at(l.end));
                    if k == 1 {
                        self.paint(s, e, SourceTok::Marker);
                        continue;
                    }
                    self.line_inlines(s, e, if k == 0 { Some(SourceTok::Strong) } else { None });
                    // cell pipes
                    let (cs, ce) = (s.max(self.range.start), e.min(self.range.end));
                    if cs < ce {
                        for (j, c) in self.text.as_bytes()[cs..ce].iter().enumerate() {
                            if *c == b'|' {
                                self.paint(cs + j, cs + j + 1, SourceTok::Marker);
                            }
                        }
                    }
                }
            }
            BlockKind::BlockQuote { .. } => {
                self.paint(bs, be, SourceTok::Quote);
                self.children(b, origin);
                self.quote_markers(bs, be);
            }
            BlockKind::List { .. } => self.children(b, origin),
            BlockKind::ListItem { marker, task, .. } => {
                self.children(b, origin);
                self.paint(at(marker.start), at(marker.end), SourceTok::Marker);
                if let Some((_, t)) = task {
                    self.paint(at(t.start), at(t.end), SourceTok::Marker);
                }
            }
            BlockKind::FootnoteDef { label } => {
                self.children(b, origin);
                self.paint(at(label.start), at(label.end), SourceTok::Link);
            }
            BlockKind::LinkRefDef { label, dest, title } => {
                self.paint(bs, be, SourceTok::Marker);
                self.paint(at(label.start), at(label.end), SourceTok::Link);
                self.paint(at(dest.start), at(dest.end), SourceTok::Url);
                if let Some(t) = title {
                    self.paint(at(t.start), at(t.end), SourceTok::Meta);
                }
            }
        }
    }

    /// The `>` markers at the line starts of a quote, on the lines overlapping the range.
    fn quote_markers(&mut self, s: usize, e: usize) {
        let from = s.max(self.range.start);
        let mut line = self.text[..from].rfind('\n').map_or(0, |k| k + 1).max(s);
        let stop = e.min(self.range.end);
        while line < stop {
            let end = self.text[line..e].find('\n').map_or(e, |k| line + k);
            let t = &self.text[line..end];
            let lead = t.len() - t.trim_start_matches([' ', '>']).len();
            for (k, c) in t.as_bytes()[..lead].iter().enumerate() {
                if *c == b'>' {
                    self.paint(line + k, line + k + 1, SourceTok::Marker);
                }
            }
            line = end + 1;
        }
    }

    /// Inline content of a leaf: the lines in one parse when they are separated by plain line
    /// breaks (offsets map 1:1) and the leaf is not huge, else line by line (only the lines
    /// overlapping the range).
    fn inlines(&mut self, b: &Block, origin: usize, base: Option<SourceTok>) {
        let Some((first, last)) = b.lines.first().zip(b.lines.last()) else {
            return;
        };
        let (s, e) = (origin + first.start, origin + last.end);
        let small = e - s <= WHOLE_PARAGRAPH_BYTES;
        let contiguous = small
            && b.lines
                .windows(2)
                .all(|w| &self.text[origin + w[0].end..origin + w[1].start] == "\n");
        if contiguous {
            self.line_inlines(s, e, base);
        } else {
            for k in overlapping(&b.lines, origin, &self.range.clone()) {
                let l = b.lines[k];
                self.line_inlines(origin + l.start, origin + l.end, base);
            }
        }
    }

    fn line_inlines(&mut self, s: usize, e: usize, base: Option<SourceTok>) {
        if s >= e || !self.touches(s, e) {
            return;
        }
        if let Some(t) = base {
            self.under(s, e, t);
        }
        // a huge line: parse only a window around the range (constructs crossing the window's
        // edges may be coloured as plain text)
        let (s, e) = if e - s > WHOLE_PARAGRAPH_BYTES {
            let mut a = s.max(self.range.start.saturating_sub(LINE_WINDOW));
            let mut b = e.min(self.range.end + LINE_WINDOW);
            while !self.text.is_char_boundary(a) {
                a -= 1;
            }
            while !self.text.is_char_boundary(b) {
                b += 1;
            }
            (a, b)
        } else {
            (s, e)
        };
        let ctx = self.doc.ref_context().clone();
        let nodes = parse_inlines(&self.text[s..e], self.doc.options(), &ctx.inline);
        for n in &nodes {
            self.inline(n, s, SourceTok::Plain);
        }
    }

    fn inline(&mut self, n: &Inline, at: usize, outer: SourceTok) {
        let (s, e) = (at + n.span.start, at + n.span.end);
        if !self.touches(s, e) {
            return;
        }
        let inner = |n: &Inline| {
            let a = n.children.first().map_or(n.span.start, |c| c.span.start);
            let b = n.children.last().map_or(n.span.end, |c| c.span.end);
            (at + a, at + b)
        };
        match &n.kind {
            InlineKind::Emph | InlineKind::Strong | InlineKind::Strike | InlineKind::Highlight => {
                let tok = match (&n.kind, outer) {
                    (InlineKind::Emph, SourceTok::Strong) | (InlineKind::Strong, SourceTok::Emph) => {
                        SourceTok::StrongEmph
                    }
                    (InlineKind::Emph, _) => SourceTok::Emph,
                    (InlineKind::Strong, _) => SourceTok::Strong,
                    (InlineKind::Highlight, _) => SourceTok::Mark,
                    _ => SourceTok::Strike,
                };
                self.paint(s, e, tok);
                for c in &n.children {
                    self.inline(c, at, tok);
                }
                let (a, b) = inner(n);
                if n.children.is_empty() {
                    self.paint(s, e, SourceTok::Marker);
                } else {
                    self.paint(s, a, SourceTok::Marker);
                    self.paint(b, e, SourceTok::Marker);
                }
            }
            InlineKind::Code { content } => {
                self.paint(s, e, SourceTok::Marker);
                self.paint(at + content.start, at + content.end, SourceTok::Code);
            }
            InlineKind::Link { .. } | InlineKind::Image { .. } => {
                self.paint(s, e, SourceTok::Url);
                let (a, b) = inner(n);
                if !n.children.is_empty() {
                    self.paint(a, b, SourceTok::Link);
                    for c in &n.children {
                        self.inline(c, at, SourceTok::Link);
                    }
                }
                // brackets, parentheses and the image `!`
                let (cs, ce) = (s.max(self.range.start), e.min(self.range.end));
                for p in cs..ce {
                    let c = self.text.as_bytes()[p];
                    if matches!(c, b'[' | b']' | b'(' | b')' | b'!') && (p < a || p >= b) {
                        self.paint(p, p + 1, SourceTok::Marker);
                    }
                }
            }
            InlineKind::Autolink { .. } | InlineKind::BareUrl { .. } => self.paint(s, e, SourceTok::Url),
            InlineKind::FootnoteRef { .. } => self.paint(s, e, SourceTok::Link),
            InlineKind::Html | InlineKind::Math { .. } => self.paint(s, e, SourceTok::Meta),
            InlineKind::Escape => self.paint(s, (s + 1).min(e), SourceTok::Marker),
            _ => {
                for c in &n.children {
                    self.inline(c, at, outer);
                }
            }
        }
    }

    /// Run-length form of the painted classes (absolute offsets).
    fn runs(&self, out: &mut Vec<SourceToken>) {
        let mut i = 0;
        let n = self.cls.len();
        while i < n {
            let c = self.cls[i];
            let mut j = i + 1;
            while j < n && self.cls[j] == c {
                j += 1;
            }
            if c != SourceTok::Plain as u8 {
                // runs end on char boundaries: the painted spans come from the parse
                out.push(SourceToken {
                    range: self.range.start + i..self.range.start + j,
                    tok: SourceTok::from_u8(c),
                });
            }
            i = j;
        }
    }
}

/// See the module docs and `FormatDoc::source_tokens`.
pub(crate) fn source_tokens(doc: &Document, range: Range<usize>, out: &mut Vec<SourceToken>) {
    let text = doc.text();
    let range = range.start.min(text.len())..range.end.min(text.len());
    if range.is_empty() {
        return;
    }
    let mut p = Painter {
        text,
        doc,
        range: range.clone(),
        cls: vec![0; range.len()],
    };
    let first = doc.top_partition_point(|t| t.end() < range.start);
    let mut i = first;
    while let Some(t) = doc.top(i) {
        if t.start() >= range.end {
            break;
        }
        p.block(t.block, t.origin);
        i += 1;
    }
    p.runs(out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<(String, SourceTok)> {
        let d = crate::parse(src);
        let mut out = Vec::new();
        source_tokens(&d, 0..src.len(), &mut out);
        out.into_iter().map(|t| (src[t.range].to_string(), t.tok)).collect()
    }

    fn has(v: &[(String, SourceTok)], s: &str, t: SourceTok) -> bool {
        v.iter().any(|(a, b)| a == s && *b == t)
    }

    #[test]
    fn markers_are_dimmed_and_content_styled() {
        let v = toks("# Title\n\nSome *em* and **strong** with `code` and [a link](http://x.y).\n\n- item\n> quote\n");
        assert!(has(&v, "#", SourceTok::Marker), "{v:?}");
        assert!(has(&v, " Title", SourceTok::Heading1), "{v:?}");
        assert!(has(&v, "em", SourceTok::Emph), "{v:?}");
        assert!(has(&v, "strong", SourceTok::Strong), "{v:?}");
        assert!(has(&v, "**", SourceTok::Marker), "{v:?}");
        assert!(has(&v, "code", SourceTok::Code), "{v:?}");
        assert!(has(&v, "a link", SourceTok::Link), "{v:?}");
        assert!(has(&v, "http://x.y", SourceTok::Url), "{v:?}");
        assert!(has(&v, "-", SourceTok::Marker), "{v:?}");
        assert!(has(&v, ">", SourceTok::Marker), "{v:?}");
        assert!(has(&v, " quote", SourceTok::Quote), "{v:?}");
    }

    #[test]
    fn fences_are_markers_and_code_is_code() {
        let v = toks("```rust\nfn main() {}\n```\n");
        assert!(has(&v, "```rust", SourceTok::Marker), "{v:?}");
        assert!(has(&v, "fn main() {}", SourceTok::CodeBlock), "{v:?}");
    }

    #[test]
    fn metadata_blocks_are_tinted() {
        let v = toks("---\na: 1\n---\n\n$$\nx^2\n$$\n\n<div>\nhi\n</div>\n");
        let meta = |needle: &str| v.iter().any(|(s, t)| s.contains(needle) && *t == SourceTok::Meta);
        assert!(meta("a: 1") && meta("x^2"), "{v:?}");
        assert!(
            has(&v, "$$", SourceTok::Marker) && has(&v, "---", SourceTok::Marker),
            "{v:?}"
        );
        assert!(
            v.iter().any(|(s, t)| s.contains("<div>") && *t == SourceTok::Meta),
            "{v:?}"
        );
    }

    /// Tokens of a sub-range equal the whole-document tokens clipped to it.
    #[test]
    fn range_tokens_equal_clipped_document_tokens() {
        let mut src = String::from("# Head\n\n> quote *x*\n> more\n\n");
        for i in 0..300 {
            src.push_str(&format!("- item {i} with **bold** and `c`\n"));
        }
        src.push_str("\n```js\nvar a = 1;\nvar b = 2;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        let d = crate::parse(&src);
        let mut all = Vec::new();
        source_tokens(&d, 0..src.len(), &mut all);
        let per_byte = |v: &[SourceToken], r: Range<usize>| {
            let mut c = vec![SourceTok::Plain; r.len()];
            for t in v {
                for b in t.range.clone() {
                    if r.contains(&b) {
                        c[b - r.start] = t.tok;
                    }
                }
            }
            c
        };
        for (a, b) in [(0, 10), (15, 40), (100, 2000), (src.len() - 60, src.len()), (500, 501)] {
            let mut part = Vec::new();
            source_tokens(&d, a..b, &mut part);
            assert!(part.iter().all(|t| t.range.start >= a && t.range.end <= b));
            assert_eq!(per_byte(&part, a..b), per_byte(&all, a..b), "range {a}..{b}");
        }
    }

    #[test]
    fn reference_links_depend_on_their_definition() {
        let undefined = toks("see [x] here\n");
        assert!(!has(&undefined, "x", SourceTok::Link), "{undefined:?}");
        let defined = toks("see [x] here\n\n[x]: http://e.com\n");
        assert!(has(&defined, "x", SourceTok::Link), "{defined:?}");
    }
}
