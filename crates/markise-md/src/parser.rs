//! Block parser: a line-oriented container/leaf parser in the style of the CommonMark
//! "parsing strategy" appendix, adapted to the dialect of spec 01. It produces a block tree
//! with byte spans and never copies text.

use markise_doc::HintSet;

use crate::block::{AlertKind, Block, BlockKind, MathDelim};
use crate::options::ParseOptions;
use crate::scan::{self, HtmlEnd, is_ws};
use crate::text::{Line, Lines, Span, count_terminators, is_blank};

/// Maximum blockquote nesting (R3.3.1).
pub const MAX_QUOTE_DEPTH: usize = 10;

/// Maximum number of nested containers (quotes, lists, list items, footnote definitions).
/// Deeper container markers are kept as paragraph text (C39); this bounds the tree depth so
/// recursive traversal and `Drop` cannot overflow the stack.
pub const MAX_NESTING: usize = 64;

#[derive(Clone, Copy, Debug)]
enum St {
    Document,
    BlockQuote,
    List,
    ListItem { content_offset: usize, started_blank: bool },
    FootnoteDef,
    Paragraph,
    Table,
    Fenced { ch: u8, len: usize, indent: usize },
    Indented,
    Html(HtmlEnd),
    Math(MathDelim),
    FrontMatter,
}

struct Open {
    st: St,
    block: Block,
}

/// Line cursor with tab-stop aware column tracking (tab stops of 4, CommonMark).
#[derive(Clone, Copy)]
struct Cur {
    pos: usize,
    end: usize,
    col: usize,
    /// Columns of the tab at `pos` that are still unconsumed (0 = none).
    partial: usize,
}

impl Cur {
    fn indent(&self, b: &[u8]) -> (usize, usize) {
        let mut cols = 0;
        let mut p = self.pos;
        let mut c = self.col;
        if self.partial > 0 {
            cols += self.partial;
            c += self.partial;
            p += 1;
        }
        while p < self.end {
            match b[p] {
                b' ' => {
                    cols += 1;
                    c += 1;
                }
                b'\t' => {
                    let w = 4 - c % 4;
                    cols += w;
                    c += w;
                }
                _ => break,
            }
            p += 1;
        }
        (cols, p)
    }

    fn advance_cols(&mut self, b: &[u8], mut n: usize) {
        while n > 0 {
            if self.partial > 0 {
                if self.partial <= n {
                    n -= self.partial;
                    self.col += self.partial;
                    self.partial = 0;
                    self.pos += 1;
                } else {
                    self.partial -= n;
                    self.col += n;
                    n = 0;
                }
                continue;
            }
            if self.pos >= self.end {
                break;
            }
            match b[self.pos] {
                b' ' => {
                    self.pos += 1;
                    self.col += 1;
                    n -= 1;
                }
                b'\t' => {
                    let w = 4 - self.col % 4;
                    if w <= n {
                        self.pos += 1;
                        self.col += w;
                        n -= w;
                    } else {
                        self.partial = w - n;
                        self.col += n;
                        n = 0;
                    }
                }
                _ => break,
            }
        }
    }

    /// Advances to byte position `p` (consuming a partial tab if needed).
    fn advance_to(&mut self, b: &[u8], p: usize) {
        while self.pos < p {
            if self.partial > 0 {
                self.col += self.partial;
                self.partial = 0;
            } else if b[self.pos] == b'\t' {
                self.col += 4 - self.col % 4;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    fn optional_space(&mut self, b: &[u8]) {
        if self.partial > 0 || (self.pos < self.end && is_ws(b[self.pos])) {
            self.advance_cols(b, 1);
        }
    }

    /// Content position: a partially consumed tab is included in the content.
    fn content_pos(&self) -> usize {
        self.pos
    }
}

/// Result of a (partial) parse.
pub(crate) struct ParseOutput {
    pub blocks: Vec<Block>,
    pub failed_openers: Vec<usize>,
    /// Where parsing stopped because the sync predicate matched (incremental re-parse).
    pub stopped_at: Option<usize>,
}

pub(crate) struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
    opts: &'a ParseOptions,
    stack: Vec<Open>,
    failed_openers: Vec<usize>,
    prev_blank: bool,
    cur_line: usize,
    /// Offset of the first line of the document (after a UTF-8 BOM, C35).
    doc_start: usize,
    math_fail: std::collections::HashMap<(bool, Vec<usize>), usize>,
    /// Literal-line hints (ADR 0006 §2.6): hinted lines open no new block.
    hints: &'a HintSet,
}

/// Parses `text[from..]` (where `from` is a line start at which no block is open).
/// `sync(pos)` is asked at every clean line start after `from`; when it returns `true`
/// parsing stops there.
/// Length of a leading UTF-8 byte-order mark (C35): file-level trivia, not content.
pub fn bom_len(text: &str) -> usize {
    if text.starts_with('\u{FEFF}') { 3 } else { 0 }
}

pub(crate) fn parse_blocks(
    text: &str,
    opts: &ParseOptions,
    from: usize,
    sync: Option<&dyn Fn(usize) -> bool>,
    hints: &HintSet,
) -> ParseOutput {
    let mut p = Parser {
        text,
        b: text.as_bytes(),
        opts,
        stack: vec![Open {
            st: St::Document,
            block: Block::new(BlockKind::Paragraph, Span::new(from, from)),
        }],
        failed_openers: Vec::new(),
        prev_blank: false,
        cur_line: from,
        doc_start: bom_len(text),
        math_fail: Default::default(),
        hints,
    };
    let mut stopped_at = None;
    for line in Lines::new(text, from) {
        if let Some(sync) = sync {
            let after_adjacent_html = p.stack[0].block.children.last().is_some_and(|l| {
                matches!(l.kind, BlockKind::HtmlBlock) && count_terminators(&text[l.span.end..line.start]) <= 1
            });
            if p.stack.len() == 1 && line.start > from && !after_adjacent_html && sync(line.start) {
                stopped_at = Some(line.start);
                break;
            }
        }
        p.line(line);
    }
    while p.stack.len() > 1 {
        p.close_top();
    }
    let doc = p.stack.pop().unwrap();
    ParseOutput {
        blocks: doc.block.children,
        failed_openers: p.failed_openers,
        stopped_at,
    }
}

impl<'a> Parser<'a> {
    fn container_depth(&self, upto: usize) -> usize {
        self.stack[..=upto]
            .iter()
            .filter(|o| matches!(o.st, St::BlockQuote | St::List | St::ListItem { .. } | St::FootnoteDef))
            .count()
    }

    fn quote_depth(&self, upto: usize) -> usize {
        self.stack[..=upto]
            .iter()
            .filter(|o| matches!(o.st, St::BlockQuote))
            .count()
    }

    /// Matches a quote marker at the cursor (after ≤3 spaces). Returns the marker byte length.
    fn quote_marker(&self, c: &Cur) -> Option<(usize, usize)> {
        let (ind, p) = c.indent(self.b);
        if ind > 3 || p >= c.end {
            return None;
        }
        if self.b[p] == b'>' {
            return Some((p, 1));
        }
        if self.opts.remap_unicode_punctuation && self.text[p..c.end].starts_with('》') {
            return Some((p, '》'.len_utf8()));
        }
        None
    }

    /// Number of chained quote markers on the rest of the line.
    fn quote_chain(&self, c: &Cur) -> usize {
        let mut c = *c;
        let mut n = 0;
        while let Some((p, len)) = self.quote_marker(&c) {
            c.advance_to(self.b, p);
            c.pos += len;
            c.col += 1;
            c.optional_space(self.b);
            n += 1;
        }
        n
    }

    /// Tries to continue container `st` on the cursor. Pure with respect to the stack.
    fn match_container(&self, st: St, c: &mut Cur) -> bool {
        let blank = scan::rest_blank(self.b, c.pos, c.end);
        match st {
            St::BlockQuote => {
                if let Some((p, len)) = self.quote_marker(c) {
                    c.advance_to(self.b, p);
                    c.pos += len;
                    c.col += 1;
                    c.optional_space(self.b);
                    true
                } else {
                    false
                }
            }
            St::List | St::Document => true,
            St::ListItem { content_offset, .. } => {
                if blank {
                    true
                } else {
                    let (ind, _) = c.indent(self.b);
                    if ind >= content_offset {
                        c.advance_cols(self.b, content_offset);
                        true
                    } else {
                        false
                    }
                }
            }
            St::FootnoteDef => {
                if blank {
                    true
                } else {
                    let (ind, _) = c.indent(self.b);
                    if ind >= 4 {
                        c.advance_cols(self.b, 4);
                        true
                    } else {
                        false
                    }
                }
            }
            _ => true,
        }
    }

    /// Read-only check that all containers in `stack[1..=upto]` continue on `line`.
    fn prefixes_match(&self, upto: usize, line: Line) -> Option<Cur> {
        let mut c = Cur {
            pos: line.start,
            end: line.content_end,
            col: 0,
            partial: 0,
        };
        for o in &self.stack[1..=upto] {
            if !self.match_container(o.st, &mut c) {
                return None;
            }
        }
        Some(c)
    }

    fn line(&mut self, line: Line) {
        self.cur_line = line.start;
        let b = self.b;
        let text = self.text;
        let clean = self.stack.len() == 1;
        let mut c = Cur {
            pos: line.start,
            end: line.content_end,
            col: 0,
            partial: 0,
        };

        // Phase 1: match open blocks.
        let n = self.stack.len();
        let mut last = 0;
        let mut closer: Option<Span> = None;
        for i in 1..n {
            let st = self.stack[i].st;
            let blank = scan::rest_blank(b, c.pos, c.end);
            let ok = match st {
                St::ListItem { started_blank, .. } => {
                    if blank && started_blank && self.stack[i].block.children.is_empty() && i + 1 == n {
                        false
                    } else {
                        self.match_container(st, &mut c)
                    }
                }
                St::BlockQuote | St::List | St::FootnoteDef => self.match_container(st, &mut c),
                St::Paragraph | St::Table => !blank,
                St::Fenced { ch, len, indent } => {
                    let (ind, p) = c.indent(b);
                    if !blank && ind <= 3 {
                        closer = scan::fence_close(b, p, c.end, ch, len);
                    }
                    if closer.is_none() {
                        c.advance_cols(b, ind.min(indent));
                    }
                    true
                }
                St::Math(delim) => {
                    if scan::math_closer(&text[c.pos..c.end], delim) {
                        let (_, p) = c.indent(b);
                        closer = Some(Span::new(p, scan::trim_end_ws(b, p, c.end)));
                    }
                    true
                }
                St::FrontMatter => {
                    if scan::fm_close(&text[c.pos..c.end], self.opts.remap_unicode_punctuation) {
                        closer = Some(Span::new(c.pos, c.end));
                    }
                    true
                }
                St::Html(end) => !(blank && end == HtmlEnd::Blank),
                St::Indented => {
                    let (ind, _) = c.indent(b);
                    if ind >= 4 {
                        c.advance_cols(b, 4);
                        true
                    } else if blank {
                        c.advance_cols(b, ind);
                        true
                    } else {
                        false
                    }
                }
                St::Document => true,
            };
            if !ok {
                break;
            }
            last = i;
        }
        let all_matched = last == n - 1;
        let blank = scan::rest_blank(b, c.pos, c.end);
        // Matched quote lines extend the quote's span.
        for o in &mut self.stack[1..=last] {
            if matches!(o.st, St::BlockQuote) {
                o.block.span.end = o.block.span.end.max(line.content_end);
            }
        }

        // Phase 2: new block starts.
        let mut container = last;
        let mut started = false;
        let mut consumed = false;
        let accepting = matches!(
            self.stack[last].st,
            St::Fenced { .. } | St::Math(_) | St::FrontMatter | St::Html(_) | St::Indented
        );
        // A hinted line is paragraph text: after its containers continue, no block opens on it
        // at or after the hint's column (ADR 0006 §2.6).
        let hinted_from = if self.hints.is_empty() {
            None
        } else {
            self.hints
                .get(line.start)
                .filter(|h| h.origin.is_literal())
                .map(|h| h.from())
        };
        let force_block = !self.hints.is_empty()
            && self
                .hints
                .get(line.start)
                .is_some_and(|h| h.origin == markise_doc::HintOrigin::ForceBlock);
        if !accepting && hinted_from != Some(line.start) {
            loop {
                let (ind, p) = c.indent(b);
                if p >= c.end {
                    break; // blank remainder
                }
                if hinted_from.is_some_and(|hf| p >= hf) {
                    break;
                }
                let mut in_para = matches!(self.stack[container].st, St::Paragraph) && container == last;
                let tip_para = matches!(self.stack.last().unwrap().st, St::Paragraph);
                // Only a matched paragraph is interrupted (lazy candidates are not, as in
                // CommonMark's reference parser): `- a` / `-` starts an empty sibling item.
                let interrupting = in_para;

                // A list only continues with a compatible item.
                if matches!(self.stack[container].st, St::List) {
                    let compatible = ind <= 3
                        && !scan::thematic_break(b, p, c.end, false)
                        && scan::list_marker(b, p, c.end).is_some_and(|m| {
                            let BlockKind::List { ordered, marker, .. } = self.stack[container].block.kind else {
                                return false;
                            };
                            m.ordered == ordered && m.ch == marker
                        });
                    if !compatible {
                        container -= 1;
                        continue;
                    }
                }

                // Front matter (first line of the document only, R3.10.1).
                if line.start == self.doc_start && self.stack.len() == 1 && container == 0 {
                    let s = &text[line.start..line.content_end];
                    if scan::fm_open(s, self.opts.remap_unicode_punctuation) {
                        if self.fm_has_closer(line.next) {
                            let opener = Span::new(line.start, line.content_end);
                            self.push(
                                St::FrontMatter,
                                BlockKind::FrontMatter { opener, closer: opener },
                                line.start,
                                clean,
                            );
                            self.stack[1].block.span.end = line.content_end;
                            consumed = true;
                            started = true;
                            break;
                        } else {
                            self.failed_openers.push(line.start);
                        }
                    }
                }

                if ind >= 4 {
                    if matches!(self.stack[container].st, St::Table) {
                        break;
                    }
                    // R6.1: an indented line interrupts a paragraph (unlike CommonMark).
                    self.prepare(&mut container, &mut started);
                    c.advance_cols(b, 4);
                    let pos = c.content_pos();
                    self.push(St::Indented, BlockKind::IndentedCode, pos, clean);
                    container = self.stack.len() - 1;
                    break;
                }
                let ch = b[p];

                // Block quote.
                let room = MAX_NESTING.saturating_sub(self.container_depth(container));
                if room >= 1 && self.quote_marker(&c).is_some() {
                    let chain = self.quote_chain(&c);
                    if self.quote_depth(container) + chain <= MAX_QUOTE_DEPTH {
                        let (qp, len) = self.quote_marker(&c).unwrap();
                        self.prepare(&mut container, &mut started);
                        c.advance_to(b, qp);
                        self.push(St::BlockQuote, BlockKind::BlockQuote { alert: None }, qp, clean);
                        c.pos += len;
                        c.col += 1;
                        c.optional_space(b);
                        let top = self.stack.len() - 1;
                        self.stack[top].block.span.end = line.content_end;
                        container = top;
                        continue;
                    }
                }

                // ATX heading.
                if ch == b'#'
                    && let Some((level, content, closed)) = scan::atx_heading(b, p, c.end, self.opts.strict)
                {
                    self.prepare(&mut container, &mut started);
                    let mut blk = Block::new(
                        BlockKind::Heading {
                            level,
                            setext: false,
                            closed,
                        },
                        Span::new(p, scan::trim_end_ws(b, p, c.end)),
                    );
                    blk.lines.push(content);
                    self.add_leaf(blk, clean);
                    consumed = true;
                    break;
                }

                // Fenced code.
                if (ch == b'`' || ch == b'~')
                    && let Some((fch, len, info)) = scan::fence_open(b, p, c.end)
                {
                    self.prepare(&mut container, &mut started);
                    let opener = Span::new(p, scan::trim_end_ws(b, p, c.end));
                    self.push(
                        St::Fenced {
                            ch: fch,
                            len,
                            indent: ind,
                        },
                        BlockKind::FencedCode {
                            fence_char: fch,
                            fence_len: len,
                            indent: ind,
                            info,
                            opener,
                            closer: None,
                        },
                        p,
                        clean,
                    );
                    let top = self.stack.len() - 1;
                    self.stack[top].block.span.end = opener.end;
                    consumed = true;
                    break;
                }

                // Math block (requires a closing line, R3.7.3).
                if (ch == b'$' || ch == b'\\')
                    && let Some(delim) = scan::math_opener(&text[p..c.end], self.opts.latex_delimiters)
                {
                    let upto = if matches!(self.stack[container].st, St::Paragraph | St::Table) {
                        container - 1
                    } else {
                        container
                    };
                    if self.math_has_closer(upto, line.next, delim) {
                        self.prepare(&mut container, &mut started);
                        let opener = Span::new(p, scan::trim_end_ws(b, p, c.end));
                        self.push(
                            St::Math(delim),
                            BlockKind::MathBlock {
                                delim,
                                opener,
                                closer: opener,
                            },
                            p,
                            clean,
                        );
                        let top = self.stack.len() - 1;
                        self.stack[top].block.span.end = opener.end;
                        consumed = true;
                        break;
                    } else {
                        self.failed_openers.push(line.start);
                    }
                }

                // HTML block.
                if ch == b'<'
                    && self.opts.block_html
                    && let Some(end) = scan::html_block_start(b, p, c.end)
                {
                    self.prepare(&mut container, &mut started);
                    self.push(St::Html(end), BlockKind::HtmlBlock, p, clean);
                    container = self.stack.len() - 1;
                    c.advance_to(b, p);
                    break;
                }

                // Setext underline.
                if in_para
                    && ind <= 3
                    && !force_block
                    && let Some(level) = scan::setext_underline(b, p, c.end)
                {
                    if self.paragraph_to_setext(container, level, Span::new(p, scan::trim_end_ws(b, p, c.end))) {
                        consumed = true;
                        started = true;
                        break;
                    }
                    // The paragraph held only link definitions; it is gone now.
                    container -= 1;
                    started = true;
                    in_para = false;
                }

                // Thematic break.
                if scan::thematic_break(b, p, c.end, self.opts.remap_unicode_punctuation) {
                    self.prepare(&mut container, &mut started);
                    let blk = Block::new(BlockKind::ThematicBreak, Span::new(p, scan::trim_end_ws(b, p, c.end)));
                    self.add_leaf(blk, clean);
                    consumed = true;
                    break;
                }

                // Footnote definition.
                if ch == b'['
                    && room >= 1
                    && let Some((label, after)) = scan::footnote_def_start(b, p, c.end)
                {
                    self.prepare(&mut container, &mut started);
                    self.push(St::FootnoteDef, BlockKind::FootnoteDef { label }, p, clean);
                    let top = self.stack.len() - 1;
                    self.stack[top].block.span.end = after;
                    c.advance_to(b, after);
                    let (_, np) = c.indent(b);
                    c.advance_to(b, np);
                    container = top;
                    continue;
                }

                // List item.
                if let Some(m) = scan::list_marker(b, p, c.end).filter(|_| {
                    room >= if matches!(self.stack[container].st, St::List) {
                        1
                    } else {
                        2
                    }
                }) {
                    let after = m.marker.end;
                    let rest_blank = scan::rest_blank(b, after, c.end);
                    let ok = if interrupting {
                        (!rest_blank || force_block) && (!m.ordered || !self.opts.strict_number_start || m.number == 1)
                    } else {
                        true
                    };
                    if ok {
                        let start_col = c.col;
                        // columns
                        let mut mc = c;
                        mc.advance_to(b, p);
                        let marker_col = mc.col;
                        mc.pos = after;
                        mc.col += after - p;
                        let (spaces, _) = mc.indent(b);
                        let width = after - p;
                        let base = marker_col - start_col + width;
                        let (content_offset, started_blank) = if rest_blank {
                            (base + 1, true)
                        } else if spaces >= 5 {
                            (base + 1, false)
                        } else {
                            (base + spaces, false)
                        };
                        // Task box (unordered items only, R3.4.2).
                        let mut task = None;
                        if !m.ordered && !rest_blank && spaces < 5 {
                            let (_, tp) = mc.indent(b);
                            if !hinted_from.is_some_and(|hf| tp >= hf) {
                                task = scan::task_box(b, tp, c.end);
                            }
                        }
                        // Join or create the list.
                        let joins = matches!(self.stack[container].st, St::List);
                        if joins {
                            // close previous item (and anything inside it)
                            self.close_to(container);
                            started = true;
                        } else {
                            self.prepare(&mut container, &mut started);
                            self.push(
                                St::List,
                                BlockKind::List {
                                    ordered: m.ordered,
                                    marker: m.ch,
                                    start: m.number,
                                    tight: true,
                                },
                                p,
                                clean,
                            );
                        }
                        self.push(
                            St::ListItem {
                                content_offset,
                                started_blank,
                            },
                            BlockKind::ListItem {
                                marker: m.marker,
                                number: m.ordered.then_some(m.number),
                                task,
                                content_offset: content_offset - (marker_col - start_col),
                            },
                            p,
                            clean,
                        );
                        let top = self.stack.len() - 1;
                        self.stack[top].block.span.end = match task {
                            Some((_, s)) => s.end,
                            None => after,
                        };
                        if rest_blank {
                            c.advance_to(b, c.end);
                        } else if spaces >= 5 {
                            c.advance_to(b, p);
                            c.pos = after;
                            c.col += width;
                            c.advance_cols(b, 1);
                        } else {
                            c.advance_to(b, p);
                            c.pos = after;
                            c.col += width;
                            c.advance_cols(b, spaces);
                        }
                        if let Some((_, box_span)) = task {
                            c.advance_to(b, box_span.end);
                        }
                        container = top;
                        continue;
                    }
                }

                // Table: header paragraph line + delimiter row (R3.8.2).
                if in_para
                    && self.stack[container].block.lines.len() == 1
                    && let Some(aligns) = scan::table_delim_row(b, p, c.end)
                {
                    let header = self.stack[container].block.lines[0];
                    let cols = scan::split_table_row(b, header.start, header.end).len();
                    if cols == aligns.len() {
                        let o = &mut self.stack[container];
                        o.st = St::Table;
                        o.block.kind = BlockKind::Table { aligns };
                        o.block.lines.push(Span::new(p, scan::trim_end_ws(b, p, c.end)));
                        o.block.span.end = line.content_end;
                        consumed = true;
                        started = true;
                        break;
                    }
                }

                // Table of contents (only where a new paragraph would start).
                if !tip_para || started {
                    let tc = matches!(self.stack[container].st, St::Table);
                    if !tc && scan::is_toc_marker(&text[p..c.end], self.opts.remap_unicode_punctuation) {
                        self.prepare(&mut container, &mut started);
                        let blk = Block::new(BlockKind::Toc, Span::new(p, scan::trim_end_ws(b, p, c.end)));
                        self.add_leaf(blk, clean);
                        consumed = true;
                        break;
                    }
                }
                break;
            }
        }

        // Phase 3: add the line to the tip.
        let blank = if started {
            scan::rest_blank(b, c.pos, c.end)
        } else {
            blank
        };
        if consumed {
            // nothing more
        } else if !started
            && !all_matched
            && !blank
            && matches!(self.stack.last().unwrap().st, St::Paragraph)
            && !matches!(self.stack[container].st, St::Table)
        {
            // Lazy continuation line.
            let (_, p) = c.indent(b);
            let top = self.stack.len() - 1;
            let o = &mut self.stack[top];
            o.block.lines.push(Span::new(p, line.content_end));
            o.block.span.end = line.content_end;
        } else {
            if !started {
                self.close_to(container);
            }
            let top = self.stack.len() - 1;
            let st = self.stack[top].st;
            match st {
                St::Fenced { .. } => {
                    if let Some(cl) = closer {
                        let o = &mut self.stack[top];
                        if let BlockKind::FencedCode { closer: c2, .. } = &mut o.block.kind {
                            *c2 = Some(cl);
                        }
                        o.block.span.end = cl.end;
                        self.close_top();
                    } else {
                        let o = &mut self.stack[top];
                        o.block.lines.push(Span::new(c.content_pos(), line.content_end));
                        o.block.span.end = line.content_end;
                    }
                }
                St::Math(_) | St::FrontMatter => {
                    let o = &mut self.stack[top];
                    if let Some(cl) = closer {
                        match &mut o.block.kind {
                            BlockKind::MathBlock { closer: c2, .. } | BlockKind::FrontMatter { closer: c2, .. } => {
                                *c2 = cl
                            }
                            _ => {}
                        }
                        o.block.span.end = cl.end;
                        self.close_top();
                    } else {
                        o.block.lines.push(Span::new(c.content_pos(), line.content_end));
                        o.block.span.end = line.content_end;
                    }
                }
                St::Indented => {
                    let o = &mut self.stack[top];
                    o.block
                        .lines
                        .push(Span::new(c.content_pos().min(line.content_end), line.content_end));
                    if !blank {
                        o.block.span.end = line.content_end;
                    }
                }
                St::Html(end) => {
                    let o = &mut self.stack[top];
                    let pos = c.content_pos();
                    o.block.lines.push(Span::new(pos, line.content_end));
                    if !blank {
                        o.block.span.end = line.content_end;
                    }
                    if scan::html_end_met(&b[pos..line.content_end], end) {
                        self.close_top();
                    }
                }
                St::Paragraph => {
                    let (_, p) = c.indent(b);
                    let o = &mut self.stack[top];
                    o.block.lines.push(Span::new(p, line.content_end));
                    o.block.span.end = line.content_end;
                }
                St::Table => {
                    let (_, p) = c.indent(b);
                    let o = &mut self.stack[top];
                    o.block
                        .lines
                        .push(Span::new(p, scan::trim_end_ws(b, p, line.content_end)));
                    o.block.span.end = line.content_end;
                }
                St::Document | St::BlockQuote | St::ListItem { .. } | St::FootnoteDef | St::List => {
                    if !blank {
                        let (_, p) = c.indent(b);
                        self.push(St::Paragraph, BlockKind::Paragraph, p, clean);
                        let top = self.stack.len() - 1;
                        let o = &mut self.stack[top];
                        o.block.lines.push(Span::new(p, line.content_end));
                        o.block.span.end = line.content_end;
                    }
                }
            }
        }

        // Two consecutive blank lines end all open lists (decision C24, see spec R3.4.7).
        let truly_blank = blank && !started && !consumed;
        if truly_blank && self.prev_blank {
            let tip_accepts = matches!(
                self.stack.last().unwrap().st,
                St::Fenced { .. } | St::Math(_) | St::FrontMatter | St::Html(_)
            );
            if !tip_accepts && let Some(first_list) = self.stack.iter().position(|o| matches!(o.st, St::List)) {
                self.close_to(first_list - 1);
            }
        }
        self.prev_blank = truly_blank;
    }

    /// Looks ahead for the closing line of a math block opened at `from` (R3.7.3).
    /// Failed searches are memoised per (delimiter, enclosing containers): a later opener in
    /// the same containers before the point where the failed search stopped cannot find a
    /// closer either, which keeps adversarial inputs (many unclosed openers) linear.
    fn math_has_closer(&mut self, upto: usize, from: usize, delim: MathDelim) -> bool {
        let key: (bool, Vec<usize>) = (
            delim == MathDelim::Dollars,
            self.stack[1..=upto].iter().map(|o| o.block.span.start).collect(),
        );
        if let Some(&end) = self.math_fail.get(&key)
            && from < end
        {
            return false;
        }
        let mut stop = self.text.len();
        for l in Lines::new(self.text, from) {
            let Some(c) = self.prefixes_match(upto, l) else {
                stop = l.start;
                break;
            };
            if scan::math_closer(&self.text[c.pos..l.content_end], delim) {
                return true;
            }
        }
        self.math_fail.insert(key, stop);
        false
    }

    fn fm_has_closer(&self, from: usize) -> bool {
        let mut first = true;
        for l in Lines::new(self.text, from) {
            let s = &self.text[l.start..l.content_end];
            if scan::fm_close(s, self.opts.remap_unicode_punctuation) {
                // at least one line between the delimiters
                return !first;
            }
            first = false;
        }
        false
    }

    /// Closes blocks above `container` and, when `container` is a paragraph/table, the leaf
    /// itself, so a new block can be added to the resulting container.
    fn prepare(&mut self, container: &mut usize, started: &mut bool) {
        self.close_to(*container);
        if matches!(self.stack[*container].st, St::Paragraph | St::Table) {
            self.close_top();
            *container -= 1;
        }
        *started = true;
    }

    fn push(&mut self, st: St, kind: BlockKind, start: usize, clean: bool) {
        let mut block = Block::new(kind, Span::new(start, start));
        block.clean_line = (clean && self.stack.len() == 1).then_some(self.cur_line);
        self.stack.push(Open { st, block });
    }

    fn add_leaf(&mut self, mut blk: Block, clean: bool) {
        blk.clean_line = (clean && self.stack.len() == 1).then_some(self.cur_line);
        self.append_child(blk);
    }

    fn append_child(&mut self, blk: Block) {
        let text = self.text;
        let parent = &mut self.stack.last_mut().unwrap().block;
        // R3.14.2: adjacent HTML blocks merge.
        if matches!(blk.kind, BlockKind::HtmlBlock)
            && let Some(prev) = parent.children.last_mut()
            && matches!(prev.kind, BlockKind::HtmlBlock)
            && count_terminators(&text[prev.span.end..blk.span.start]) == 1
            && is_gap_prefix_only(&text[prev.span.end..blk.span.start])
        {
            prev.span.end = blk.span.end;
            prev.lines.extend(blk.lines);
            parent.span.end = parent.span.end.max(prev.span.end);
            return;
        }
        if parent.children.is_empty() && !matches!(parent.kind, BlockKind::Paragraph) {
            // first child; container start already set
        }
        parent.span.end = parent.span.end.max(blk.span.end);
        parent.children.push(blk);
    }

    fn close_to(&mut self, idx: usize) {
        while self.stack.len() > idx + 1 {
            self.close_top();
        }
    }

    fn close_top(&mut self) {
        let Open { st, mut block } = self.stack.pop().unwrap();
        let text = self.text;
        let b = self.b;
        match st {
            St::Paragraph => {
                // a hinted first line is literal text (ADR 0006 §2.6): no definitions
                let literal = block
                    .lines
                    .first()
                    .is_some_and(|l| hinted_line(self.hints, text, l.start));
                let defs = if literal {
                    Vec::new()
                } else {
                    extract_link_defs(text, &mut block)
                };
                for d in defs {
                    let mut d = d;
                    d.clean_line = block.clean_line;
                    block.clean_line = None;
                    self.append_child(d);
                }
                if block.lines.is_empty() {
                    return;
                }
                block.span.start = block.lines[0].start;
            }
            St::Indented => {
                while let Some(l) = block.lines.last() {
                    if is_blank(l.slice(text)) {
                        block.lines.pop();
                    } else {
                        break;
                    }
                }
            }
            St::Html(_) => {
                while block.lines.len() > 1 && is_blank(block.lines.last().unwrap().slice(text)) {
                    block.lines.pop();
                }
            }
            St::List => {
                let mut tight = true;
                let items = &block.children;
                for w in items.windows(2) {
                    if blank_between(text, w[0].span.end, w[1].span.start) {
                        tight = false;
                        break;
                    }
                }
                if tight {
                    'outer: for it in items {
                        for w in it.children.windows(2) {
                            if blank_between(text, w[0].span.end, w[1].span.start) {
                                tight = false;
                                break 'outer;
                            }
                        }
                    }
                }
                if let BlockKind::List { tight: t, .. } = &mut block.kind {
                    *t = tight;
                }
                if let Some(last) = block.children.last() {
                    block.span.end = block.span.end.max(last.span.end);
                }
            }
            St::BlockQuote => {
                if self.opts.alerts
                    && let Some(first) = block.children.first()
                    && matches!(first.kind, BlockKind::Paragraph)
                    && !hinted_line(self.hints, text, first.lines[0].start)
                    && let Some(kind) = alert_kind(first.lines[0].slice(text))
                {
                    block.kind = BlockKind::BlockQuote { alert: Some(kind) };
                }
            }
            St::Table | St::Fenced { .. } | St::Math(_) | St::FrontMatter => {}
            St::ListItem { .. } | St::FootnoteDef => {}
            St::Document => unreachable!(),
        }
        let _ = b;
        self.append_child(block);
    }

    /// Converts the paragraph at `idx` into a setext heading. Returns false when the paragraph
    /// consisted only of link reference definitions (CommonMark).
    fn paragraph_to_setext(&mut self, idx: usize, level: u8, underline: Span) -> bool {
        debug_assert_eq!(idx, self.stack.len() - 1);
        let text = self.text;
        let mut block = std::mem::replace(
            &mut self.stack[idx].block,
            Block::new(BlockKind::Paragraph, Span::default()),
        );
        let defs = extract_link_defs(text, &mut block);
        let clean = block.clean_line;
        if block.lines.is_empty() {
            // Only definitions: keep them; the caller treats the underline by other rules.
            self.stack.pop();
            for (i, mut d) in defs.into_iter().enumerate() {
                d.clean_line = clean.filter(|_| i == 0);
                self.append_child(d);
            }
            return false;
        }
        for (i, d) in defs.into_iter().enumerate() {
            let mut d = d;
            d.clean_line = clean.filter(|_| i == 0);
            block.clean_line = None;
            // insert before the paragraph: append to parent
            let parent = &mut self.stack[idx - 1].block;
            parent.span.end = parent.span.end.max(d.span.end);
            parent.children.push(d);
        }
        block.kind = BlockKind::Heading {
            level,
            setext: true,
            closed: false,
        };
        block.span.start = block.lines[0].start;
        block.span.end = underline.end;
        self.stack.pop();
        self.append_child(block);
        true
    }
}

fn is_gap_prefix_only(s: &str) -> bool {
    s.bytes().all(|c| matches!(c, b'\r' | b'\n' | b' ' | b'\t' | b'>'))
}

/// `true` when at least one blank line lies between two sibling blocks.
fn blank_between(text: &str, a_end: usize, b_start: usize) -> bool {
    a_end < b_start && count_terminators(&text[a_end..b_start]) >= 2
}

fn alert_kind(first_line: &str) -> Option<AlertKind> {
    let t = first_line.trim_matches([' ', '\t']);
    let inner = t.strip_prefix("[!")?.strip_suffix(']')?;
    Some(match inner.to_ascii_lowercase().as_str() {
        "note" => AlertKind::Note,
        "tip" => AlertKind::Tip,
        "important" => AlertKind::Important,
        "warning" => AlertKind::Warning,
        "caution" => AlertKind::Caution,
        _ => return None,
    })
}

/// Removes leading link reference definitions (R3.13.1) from a paragraph and returns them.
fn extract_link_defs(text: &str, para: &mut Block) -> Vec<Block> {
    let mut defs = Vec::new();
    while let Some(first) = para.lines.first() {
        if !first.slice(text).starts_with('[') {
            break;
        }
        match parse_link_def(text, &para.lines) {
            Some((def, used)) => {
                defs.push(def);
                para.lines.drain(..used);
            }
            None => break,
        }
    }
    defs
}

/// Parses one link reference definition from the start of `lines`.
/// Returns the block and the number of lines consumed.
fn parse_link_def(text: &str, lines: &[Span]) -> Option<(Block, usize)> {
    let b = text.as_bytes();
    // Cursor over (line index, position)
    let mut li = 0;
    let mut p = lines[0].start;
    let line_end = |li: usize| lines[li].end;
    // label
    if b[p] != b'[' || b.get(p + 1) == Some(&b'^') {
        return None;
    }
    p += 1;
    let label_start = p;
    let label_line = li;
    let mut label_len = 0;
    loop {
        if p >= line_end(li) {
            li += 1;
            if li >= lines.len() {
                return None;
            }
            p = lines[li].start;
            continue;
        }
        match b[p] {
            b']' => break,
            b'[' => return None,
            b'\\' => {
                p += 2;
                label_len += 2;
            }
            c => {
                p += 1;
                // count characters, not bytes
                if c & 0xC0 != 0x80 {
                    label_len += 1;
                }
            }
        }
        if label_len > 999 {
            return None;
        }
    }
    let label_end = p;
    let _ = label_line;
    if text[label_start..label_end].trim().is_empty() {
        return None;
    }
    p += 1;
    if b.get(p) != Some(&b':') || p >= line_end(li) {
        return None;
    }
    p += 1;
    // whitespace incl. at most one line break
    p = scan::skip_ws(b, p, line_end(li));
    if p >= line_end(li) {
        li += 1;
        if li >= lines.len() {
            return None;
        }
        p = scan::skip_ws(b, lines[li].start, lines[li].end);
    }
    // destination
    let le = line_end(li);
    let dest_start = p;
    if p < le && b[p] == b'<' {
        p += 1;
        while p < le && b[p] != b'>' {
            if b[p] == b'<' {
                return None;
            }
            if b[p] == b'\\' {
                p += 1;
            }
            p += 1;
        }
        if p >= le {
            return None;
        }
        p += 1;
    } else {
        let mut depth = 0i32;
        while p < le && !is_ws(b[p]) && b[p] > 0x20 {
            match b[p] {
                b'\\' => p += 1,
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            p += 1;
        }
        if p == dest_start || depth != 0 {
            return None;
        }
    }
    let p = p.min(le);
    let dest = Span::new(dest_start, p);
    let dest_line = li;
    // title (same line or next line)
    let after_dest = scan::skip_ws(b, p, le);
    let try_title = |li: usize, start: usize| -> Option<(Span, usize)> {
        let le = lines[li].end;
        let q = start;
        if q >= le {
            return None;
        }
        let open = b[q];
        let close = match open {
            b'"' => b'"',
            b'\'' => b'\'',
            b'(' => b')',
            _ => return None,
        };
        let mut li2 = li;
        let mut r = q + 1;
        loop {
            if r >= lines[li2].end {
                li2 += 1;
                if li2 >= lines.len() {
                    return None;
                }
                let s = lines[li2].slice(text);
                if is_blank(s) {
                    return None;
                }
                r = lines[li2].start;
                continue;
            }
            if b[r] == b'\\' {
                r += 2;
                continue;
            }
            if b[r] == close {
                break;
            }
            if open == b'(' && b[r] == b'(' {
                return None;
            }
            r += 1;
        }
        let tend = r + 1;
        if !scan::rest_blank(b, tend, lines[li2].end) {
            return None;
        }
        Some((Span::new(q, tend), li2))
    };
    let (title, last_line) = if after_dest < le {
        // title must be on the same line (separated by whitespace)
        if after_dest == p {
            return None;
        }
        match try_title(dest_line, after_dest) {
            Some((t, l)) => (Some(t), l),
            None => return None,
        }
    } else if dest_line + 1 < lines.len() {
        let nl = dest_line + 1;
        let s = scan::skip_ws(b, lines[nl].start, lines[nl].end);
        match try_title(nl, s) {
            Some((t, l)) => (Some(t), l),
            None => (None, dest_line),
        }
    } else {
        (None, dest_line)
    };
    let end = match title {
        Some(t) => t.end,
        None => dest.end,
    };
    let mut blk = Block::new(
        BlockKind::LinkRefDef {
            label: Span::new(label_start, label_end),
            dest,
            title,
        },
        Span::new(lines[0].start, end),
    );
    blk.lines = lines[..=last_line].to_vec();
    Some((blk, last_line + 1))
}

// ---- nested checkpoints (ADR 0006 §7.2.3) -------------------------------------------------

/// A restart inside a top-level container: the containers on the path to the edit (outermost
/// first, spans relative to `origin`), how many leading children of each are kept, and the
/// line the parse restarts at (the first line of the first re-parsed child of the deepest
/// container, absolute).
pub(crate) struct NestedPlan<'a> {
    pub origin: usize,
    pub path: Vec<&'a Block>,
    pub kept: Vec<usize>,
    pub restart: usize,
}

/// Parser state reconstructed for a [`NestedPlan`] (computed from the unchanged text before
/// the restart line).
pub(crate) struct Prepared {
    levels: Vec<(St, Block)>,
    /// Absolute start of each reconstructed container (identity while parsing).
    starts: Vec<usize>,
    restart: usize,
}

/// Outcome of a nested re-parse.
pub(crate) enum Nested {
    /// After a line that starts the old sibling at `sibling` (new coordinates) of the
    /// reconstructed container at `level` (0 = the top-level container), the parse matches
    /// the old one again. `chain` holds the reconstructed containers `0..=level` (absolute
    /// spans; a container with kept children has a placeholder first child) with the children
    /// parsed since the restart.
    Synced {
        level: usize,
        chain: Vec<Block>,
        sibling: usize,
        failed_openers: Vec<usize>,
    },
    /// The top-level container closed (or the text ended) before a nested re-sync: a normal
    /// top-level output whose first block is the reconstructed container (placeholders inside).
    Top(ParseOutput),
}

/// The placeholder standing for the kept children of a reconstructed container.
pub(crate) fn placeholder(span: Span) -> Block {
    Block::new(BlockKind::ThematicBreak, span)
}

/// Reconstructs the parser state of `plan`'s containers. `None` when a container cannot be
/// reconstructed (the caller falls back to a top-level re-parse).
pub(crate) fn prepare_nested(text: &str, opts: &ParseOptions, plan: &NestedPlan<'_>) -> Option<Prepared> {
    let hints = HintSet::new();
    let p = Parser {
        text,
        b: text.as_bytes(),
        opts,
        stack: Vec::new(),
        failed_openers: Vec::new(),
        prev_blank: false,
        cur_line: 0,
        doc_start: bom_len(text),
        math_fail: Default::default(),
        hints: &hints,
    };
    let o = plan.origin;
    let mut levels: Vec<(St, Block)> = Vec::with_capacity(plan.path.len());
    let mut starts = Vec::with_capacity(plan.path.len());
    for (i, x) in plan.path.iter().enumerate() {
        let st = match &x.kind {
            BlockKind::BlockQuote { .. } => St::BlockQuote,
            BlockKind::List { .. } => St::List,
            BlockKind::FootnoteDef { .. } => St::FootnoteDef,
            BlockKind::ListItem {
                marker, content_offset, ..
            } => {
                // columns of the item's marker line: match (or open) the outer containers
                let ms = marker.start + o;
                let ls = crate::line_start(text, ms);
                let (ce, _) = crate::text::line_end(text, ls);
                let mut c = Cur {
                    pos: ls,
                    end: ce,
                    col: 0,
                    partial: 0,
                };
                for (j, (ost, ob)) in levels.iter().enumerate() {
                    let opened_here = crate::line_start(text, plan.path[j].span.start + o) == ls;
                    if !opened_here {
                        if !p.match_container(*ost, &mut c) {
                            return None;
                        }
                        continue;
                    }
                    match &ob.kind {
                        BlockKind::List { .. } => {}
                        BlockKind::BlockQuote { .. } => {
                            if !p.match_container(*ost, &mut c) {
                                return None;
                            }
                        }
                        BlockKind::ListItem { marker: om, task, .. } => {
                            let St::ListItem { content_offset, .. } = ost else {
                                return None;
                            };
                            // the content column of an item opened on this line
                            let start_col = c.col;
                            c.advance_to(p.b, om.start);
                            let target = start_col + content_offset;
                            c.pos = om.end;
                            c.col += om.end - om.start;
                            if c.col < target {
                                c.advance_cols(p.b, target - c.col);
                            }
                            // a task box is consumed as the full parse does (MD-INC-1)
                            if let Some((_, bs)) = task
                                && bs.end > c.pos
                            {
                                c.advance_to(p.b, bs.end);
                            }
                        }
                        // footnote definitions opened on the marker line of a nested item: rare
                        _ => return None,
                    }
                }
                let start_col = c.col;
                let mut mc = c;
                if ms < mc.pos {
                    return None;
                }
                mc.advance_to(p.b, ms);
                let marker_col = mc.col;
                if marker_col < start_col {
                    return None;
                }
                St::ListItem {
                    content_offset: content_offset + (marker_col - start_col),
                    started_blank: scan::rest_blank(p.b, marker.end + o, ce),
                }
            }
            _ => return None,
        };
        let mut shell = Block::new(x.kind.clone(), x.span);
        shell.clean_line = x.clean_line;
        shell.shift(o as isize);
        let kept = plan.kept[i];
        if kept > 0 {
            let first = x.children[0].span.start + o;
            let last = x.children[kept - 1].span.end + o;
            shell.children.push(placeholder(Span::new(first, last)));
            shell.span.end = last;
        } else {
            shell.span.end = shell.span.start;
        }
        if i > 0 {
            shell.clean_line = None;
        }
        starts.push(shell.span.start);
        levels.push((st, shell));
    }
    Some(Prepared {
        levels,
        starts,
        restart: plan.restart,
    })
}

/// Re-parses from a prepared checkpoint. `nested_sync(level, start)`: is `start` (new
/// coordinates) the start of an old child of the container at `level`, after the edit and
/// after the kept/path children? `top_sync` as for [`parse_blocks`].
pub(crate) fn parse_nested(
    text: &str,
    opts: &ParseOptions,
    prep: Prepared,
    top_sync: &dyn Fn(usize) -> bool,
    nested_sync: &dyn Fn(usize, usize, &BlockKind) -> bool,
    hints: &HintSet,
) -> Nested {
    let from = prep.restart;
    let mut stack = vec![Open {
        st: St::Document,
        block: Block::new(BlockKind::Paragraph, Span::new(from, from)),
    }];
    for (st, block) in prep.levels {
        stack.push(Open { st, block });
    }
    let mut p = Parser {
        text,
        b: text.as_bytes(),
        opts,
        stack,
        failed_openers: Vec::new(),
        prev_blank: false,
        cur_line: from,
        doc_start: bom_len(text),
        math_fail: Default::default(),
        hints,
    };
    let k = prep.starts.len();
    let mut top_mode = false;
    let mut stopped_at = None;
    for line in Lines::new(text, from) {
        if top_mode {
            let after_adjacent_html = p.stack[0].block.children.last().is_some_and(|l| {
                matches!(l.kind, BlockKind::HtmlBlock) && count_terminators(&text[l.span.end..line.start]) <= 1
            });
            if p.stack.len() == 1 && !after_adjacent_html && top_sync(line.start) {
                stopped_at = Some(line.start);
                break;
            }
        }
        p.line(line);
        if top_mode {
            continue;
        }
        let mut alive = 0;
        while alive < k && p.stack.len() > alive + 1 && p.stack[alive + 1].block.span.start == prep.starts[alive] {
            alive += 1;
        }
        if alive == 0 {
            top_mode = true;
            continue;
        }
        for lvl in (0..alive).rev() {
            let ci = lvl + 1;
            let (cand, open) = if p.stack.len() > ci + 1 {
                (&p.stack[ci + 1].block, true)
            } else if let Some(last) = p.stack[ci].block.children.last() {
                (last, false)
            } else {
                continue;
            };
            let start = cand.span.start;
            if start < line.start || start > line.content_end || !nested_sync(lvl, start, &cand.kind) {
                continue;
            }
            // R3.14.2: an HTML block directly after an HTML sibling merges into it when it
            // closes (`append_child`), whatever the old tree held: no re-sync there
            let siblings = &p.stack[ci].block.children;
            let prev = if open {
                siblings.last()
            } else {
                siblings.iter().rev().nth(1)
            };
            if matches!(cand.kind, BlockKind::HtmlBlock)
                && prev.is_some_and(|l| {
                    matches!(l.kind, BlockKind::HtmlBlock) && count_terminators(&text[l.span.end..start]) <= 1
                })
            {
                continue;
            }
            let cand = start;
            // the parse matches the old one from this child on: drop the partial child
            p.stack.truncate(ci + 1);
            if !open {
                p.stack[ci].block.children.pop();
            }
            let chain = p.stack.drain(1..).map(|o| o.block).collect();
            return Nested::Synced {
                level: lvl,
                chain,
                sibling: cand,
                failed_openers: p.failed_openers,
            };
        }
    }
    while p.stack.len() > 1 {
        p.close_top();
    }
    let doc = p.stack.pop().unwrap();
    Nested::Top(ParseOutput {
        blocks: doc.block.children,
        failed_openers: p.failed_openers,
        stopped_at,
    })
}

/// [`refinish`] for a container whose children outside `changed` (indices after the splice) are
/// unchanged and whose state was correct before: a list's tightness is decided from the windows
/// touching `changed` when they are loose or the list was tight; only a loose list whose
/// looseness may have been removed is rescanned (stopping at the first loose spot).
pub(crate) fn refinish_spliced(
    text: &str,
    opts: &ParseOptions,
    hints: &HintSet,
    b: &mut Block,
    origin: usize,
    changed: std::ops::Range<usize>,
) {
    let BlockKind::List { tight, .. } = &b.kind else {
        return refinish(text, opts, hints, b, origin);
    };
    let was_tight = *tight;
    let gap = |a: usize, z: usize| blank_between(text, a + origin, z + origin);
    let items = &b.children;
    let lo = changed.start.saturating_sub(1).min(items.len());
    let hi = (changed.end + 1).min(items.len()).max(lo);
    let mid = changed.start.min(items.len())..changed.end.min(items.len());
    let local_loose = items[lo..hi].windows(2).any(|w| gap(w[0].span.end, w[1].span.start))
        || items[mid]
            .iter()
            .any(|it| it.children.windows(2).any(|w| gap(w[0].span.end, w[1].span.start)));
    if local_loose {
        if let BlockKind::List { tight, .. } = &mut b.kind {
            *tight = false;
        }
    } else if !was_tight {
        refinish(text, opts, hints, b, origin);
    }
}

/// Recomputes what closing a container computes from its children (list tightness, callout
/// kind), for a container whose children were spliced. Spans relative to `origin`.
/// `true` when the physical line holding `pos` carries a literal hint starting at or before
/// `pos` (the text from there on is literal: no alert tag, no task box).
fn hinted_line(hints: &HintSet, text: &str, pos: usize) -> bool {
    !hints.is_empty()
        && hints
            .get(crate::line_start(text, pos))
            .is_some_and(|h| h.origin.is_literal() && h.from() <= pos)
}

pub(crate) fn refinish(text: &str, opts: &ParseOptions, hints: &HintSet, b: &mut Block, origin: usize) {
    match &mut b.kind {
        BlockKind::List { tight, .. } => {
            let gap = |a: usize, z: usize| blank_between(text, a + origin, z + origin);
            let items = &b.children;
            let mut t = !items.windows(2).any(|w| gap(w[0].span.end, w[1].span.start));
            if t {
                t = !items
                    .iter()
                    .any(|it| it.children.windows(2).any(|w| gap(w[0].span.end, w[1].span.start)));
            }
            *tight = t;
        }
        BlockKind::BlockQuote { alert } => {
            *alert = None;
            if opts.alerts
                && let Some(first) = b.children.first()
                && matches!(first.kind, BlockKind::Paragraph)
                && let Some(l) = first.lines.first()
                && !hinted_line(hints, text, l.start + origin)
            {
                *alert = alert_kind(&text[l.start + origin..l.end + origin]);
            }
        }
        _ => {}
    }
}
