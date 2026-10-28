//! Inline parser (spec 01 §4). Inline content stays raw in the document (R1.2); this module
//! parses it on demand into a tree whose spans point into the block's content string
//! (see [`Block::content`](crate::Block::content)).
//!
//! The emphasis/link resolution follows the delimiter-run algorithm described in the CommonMark
//! specification, extended with strikethrough, highlight, sub/superscript, math, footnote
//! references, emoji shortcodes and bare-URL autolinks.

use std::collections::{HashMap, HashSet};

use crate::options::ParseOptions;
use crate::text::{Span, is_punct_or_symbol};
use crate::ticks::TickRuns;

/// How an inline math span was delimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathInline {
    /// `$…$`
    Dollar,
    /// `$$…$$` inside a paragraph (inline display math)
    DoubleDollar,
    /// `\(…\)`
    Paren,
    /// `` $`…`$ `` (GitLab)
    Gitlab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Inline,
    Full,
    Collapsed,
    Shortcut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineKind {
    Text,
    /// `\x` — the literal is `span.start + 1 .. span.end`.
    Escape,
    /// `&name;` / `&#123;` — passed through as written.
    Entity,
    /// Code span; `content` excludes the backtick runs.
    Code {
        content: Span,
    },
    SoftBreak,
    HardBreak,
    Emph,
    Strong,
    Strike,
    Highlight,
    Sub,
    Sup,
    Link {
        dest: String,
        title: Option<String>,
        kind: LinkKind,
    },
    Image {
        dest: String,
        title: Option<String>,
        kind: LinkKind,
        /// Trailing kramdown attribute list `{…}` (kept, not interpreted).
        attrs: Option<Span>,
    },
    Autolink {
        url: String,
        email: bool,
    },
    /// Bare URL detected for display (source unchanged).
    BareUrl {
        url: String,
    },
    /// Raw inline HTML (tags, comments, processing instructions…).
    Html,
    Math {
        delim: MathInline,
        content: Span,
    },
    FootnoteRef {
        label: String,
    },
    Emoji {
        emoji: &'static str,
    },
    /// `[[target#subpath|display]]` (spec 06 V2.1, [`ParseOptions::wikilinks`]). `inner` is the
    /// text between the brackets, `display` the text after the first `|` (both content spans);
    /// no children.
    WikiLink {
        target: String,
        subpath: Option<String>,
        inner: Span,
        display: Option<Span>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub kind: InlineKind,
    pub span: Span,
    pub children: Vec<Inline>,
}

/// Document-level data inline parsing needs.
#[derive(Clone, Debug, Default)]
pub struct InlineContext {
    pub link_defs: HashMap<String, (String, Option<String>)>,
    pub footnotes: HashSet<String>,
}

/// Normalises a link label for matching (case-fold + whitespace collapse).
pub fn normalize_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut ws = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            ws = true;
            continue;
        }
        if ws && !out.is_empty() {
            out.push(' ');
        }
        ws = false;
        for l in c.to_lowercase() {
            out.extend(l.to_uppercase());
        }
    }
    out
}

/// Characters that may be backslash-escaped (R4.1.1).
pub fn is_escapable(c: u8, opts: &ParseOptions) -> bool {
    matches!(
        c,
        b'\\'
            | b':'
            | b'`'
            | b'*'
            | b'{'
            | b'}'
            | b'['
            | b']'
            | b'('
            | b')'
            | b'#'
            | b'+'
            | b'-'
            | b'.'
            | b'!'
            | b'_'
            | b'>'
            | b'$'
            | b'|'
            | b'<'
            | b'~'
            | b'"'
            | b'\''
            | b'&'
    ) || (c == b'^' && opts.superscript)
        || (c == b'=' && opts.highlight)
}

/// Removes backslash escapes of ASCII punctuation (used for link destinations/titles).
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\'
            && let Some(&n) = it.peek()
            && n.is_ascii_punctuation()
        {
            out.push(n);
            it.next();
            continue;
        }
        out.push(c);
    }
    out
}

const NIL: u32 = u32::MAX;

const FAIL_PAREN: u32 = 1;
const FAIL_GITLAB: u32 = 2;
const FAIL_DD: u32 = 4;
const FAIL_DOLLAR: u32 = 8;
const FAIL_COMMENT: u32 = 16;
const FAIL_PI: u32 = 32;
const FAIL_CDATA: u32 = 64;
const FAIL_DECL: u32 = 128;
const FAIL_DQ: u32 = 256;
const FAIL_SQ: u32 = 512;
const FAIL_TITLE_DQ: u32 = 1024;
const FAIL_TITLE_SQ: u32 = 2048;
const FAIL_TITLE_PAREN: u32 = 4096;
const FAIL_TITLE_CURLY: u32 = 8192;

struct Node {
    kind: InlineKind,
    span: Span,
    first: u32,
    last: u32,
    next: u32,
    prev: u32,
}

#[derive(Clone, Copy)]
struct Delim {
    node: u32,
    ch: u8,
    count: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
    prev: Option<usize>,
    next: Option<usize>,
}

struct Bracket {
    node: u32,
    image: bool,
    active: bool,
    delim_top: Option<usize>,
    /// Position just after `[`.
    text_start: usize,
}

struct P<'a> {
    s: &'a str,
    b: &'a [u8],
    base: usize,
    opts: &'a ParseOptions,
    ctx: &'a InlineContext,
    nodes: Vec<Node>,
    head: u32,
    tail: u32,
    delims: Vec<Delim>,
    delim_tail: Option<usize>,
    brackets: Vec<Bracket>,
    escape_space: bool,
    /// Memo of searches that already failed to the end of the content (keeps adversarial
    /// inputs linear): bit flags `FAIL_*` and failed code-span run lengths.
    fails: u32,
    /// Backtick runs of the content, built on first use.
    ticks: Option<TickRuns>,
    /// Raw link-destination index, built on first use.
    dests: Option<DestIndex>,
    /// A failed `<…>` destination scan `(start, stop)`: later `<…>` scans starting inside it
    /// hit the same line end without a `>` (cache only for this termination rule).
    angle_fail: Option<(usize, usize)>,
    /// Wikilink close search memo: `(from, close)` — the first `]]` at or after `from` (on
    /// `from`'s line) is at `close`; `close == usize::MAX`: none before the line end. Keeps
    /// runs of `[[` linear.
    wiki_close: Option<(usize, usize, usize)>,
    /// For the `]]` at `.0`: the start of the last `[[` between its first opener's content
    /// start and it (`.1`), so a content-contains-`[[` check is O(1) per later opener.
    wiki_last_open: Option<(usize, Option<usize>)>,
    /// `(from, line end)` of the last line-end search.
    wiki_line: Option<(usize, usize)>,
    /// Bytes the wikilink searches scanned (linearity test).
    wiki_scanned: usize,
}

/// Parses inline content `s` (lines joined with `\n`). Spans are offsets into `s`.
pub fn parse_inlines(s: &str, opts: &ParseOptions, ctx: &InlineContext) -> Vec<Inline> {
    let mut v = parse_at(s, 0, opts, ctx, false);
    if opts.auto_links {
        detect_bare_urls(s, &mut v);
    }
    v
}

fn parse_at(s: &str, base: usize, opts: &ParseOptions, ctx: &InlineContext, escape_space: bool) -> Vec<Inline> {
    let mut p = P {
        s,
        b: s.as_bytes(),
        base,
        opts,
        ctx,
        nodes: Vec::new(),
        head: NIL,
        tail: NIL,
        delims: Vec::new(),
        delim_tail: None,
        brackets: Vec::new(),
        escape_space,
        fails: 0,
        ticks: None,
        dests: None,
        angle_fail: None,
        wiki_close: None,
        wiki_last_open: None,
        wiki_line: None,
        wiki_scanned: 0,
    };
    p.run();
    #[cfg(test)]
    WIKI_SCANNED.with(|c| c.set(c.get() + p.wiki_scanned));
    p.process_emphasis(None);
    let mut out = Vec::new();
    p.collect(p.head, &mut out, 0);
    out
}

#[cfg(test)]
thread_local! {
    /// Bytes scanned by wikilink searches in this thread (linearity tests).
    static WIKI_SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn is_ws_char(c: char) -> bool {
    c.is_whitespace()
}

impl<'a> P<'a> {
    fn sp(&self, a: usize, b: usize) -> Span {
        Span::new(self.base + a, self.base + b)
    }

    fn push(&mut self, kind: InlineKind, a: usize, b: usize) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            kind,
            span: self.sp(a, b),
            first: NIL,
            last: NIL,
            next: NIL,
            prev: self.tail,
        });
        if self.tail != NIL {
            self.nodes[self.tail as usize].next = id;
        } else {
            self.head = id;
        }
        self.tail = id;
        id
    }

    fn text(&mut self, a: usize, b: usize) {
        if a < b {
            self.push(InlineKind::Text, a, b);
        }
    }

    fn char_before(&self, pos: usize) -> Option<char> {
        self.s[..pos].chars().next_back()
    }

    fn char_at(&self, pos: usize) -> Option<char> {
        self.s[pos..].chars().next()
    }

    fn run(&mut self) {
        let len = self.b.len();
        let mut pos = 0;
        let mut text_start = 0;
        // Strip trailing whitespace of the final line (CommonMark).
        let mut end = len;
        while end > 0 && matches!(self.b[end - 1], b' ' | b'\t') {
            end -= 1;
        }
        let at_line_start = |p: usize, b: &[u8]| p == 0 || b[p - 1] == b'\n';
        while pos < end {
            let c = self.b[pos];
            // R3.1.4: a leading zero-width space is tolerated (not rendered).
            if c == 0xE2 && at_line_start(pos, self.b) && self.s[pos..].starts_with('\u{200B}') {
                self.text(text_start, pos);
                pos += 3;
                text_start = pos;
                continue;
            }
            let trigger = match c {
                b'\\' | b'`' | b'*' | b'_' | b'~' | b'[' | b']' | b'<' | b'&' => true,
                b'$' => self.opts.inline_math,
                b'=' => self.opts.highlight,
                b'^' => self.opts.superscript,
                b'!' => pos + 1 < end && self.b[pos + 1] == b'[',
                b':' => self.opts.emoji,
                0xE3 => self.opts.remap_unicode_punctuation,
                _ => false,
            };
            if c == b'\n' {
                pos = self.newline(pos, &mut text_start);
                text_start = pos;
                continue;
            }
            if !trigger {
                pos += 1;
                continue;
            }
            self.text(text_start, pos);
            text_start = pos;
            let handled = match c {
                b'\\' => self.backslash(pos, end),
                b'`' => self.code_span(pos, end),
                b'$' => self.dollar(pos, end),
                b'*' | b'_' => Some(self.delim_run(pos, end, c)),
                b'~' => self.tilde(pos, end),
                b'=' => self.equals(pos, end),
                b'^' => self.sup(pos, end),
                b'[' if self.opts.wikilinks && self.b.get(pos + 1) == Some(&b'[') => self
                    .wikilink(pos, end, false)
                    .or_else(|| Some(self.open_bracket(pos, false))),
                b'[' => Some(self.open_bracket(pos, false)),
                b'!' if self.opts.wikilinks && self.b.get(pos + 2) == Some(&b'[') => self
                    .wikilink(pos, end, true)
                    .or_else(|| Some(self.open_bracket(pos, true))),
                b'!' => Some(self.open_bracket(pos, true)),
                b']' => Some(self.close_bracket(pos, end)),
                b'<' => self.angle(pos, end),
                b'&' => self.entity(pos, end),
                b':' => self.emoji(pos, end),
                _ => {
                    if self.s[pos..].starts_with('【') {
                        Some(self.open_bracket(pos, false))
                    } else if self.s[pos..].starts_with('】') {
                        Some(self.close_bracket(pos, end))
                    } else {
                        None
                    }
                }
            };
            match handled {
                Some(next) => {
                    pos = next;
                    text_start = pos;
                }
                None => pos += 1,
            }
        }
        self.text(text_start, end.max(text_start));
    }

    fn backslash(&mut self, pos: usize, end: usize) -> Option<usize> {
        let n = *self.b.get(pos + 1)?;
        if n == b'\n' {
            self.push(InlineKind::HardBreak, pos, pos + 2);
            return Some(pos + 2);
        }
        if pos + 1 >= end {
            return None;
        }
        if n == b'('
            && self.opts.latex_delimiters
            && self.opts.inline_math
            && self.fails & FAIL_PAREN == 0
            && let Some(close) = self.find_memo(pos + 2, end, b"\\)", FAIL_PAREN)
            && close > pos + 2
        {
            self.push(
                InlineKind::Math {
                    delim: MathInline::Paren,
                    content: self.sp(pos + 2, close),
                },
                pos,
                close + 2,
            );
            return Some(close + 2);
        }
        if is_escapable(n, self.opts) || (self.escape_space && n == b' ') {
            self.push(InlineKind::Escape, pos, pos + 2);
            return Some(pos + 2);
        }
        None
    }

    fn code_span(&mut self, pos: usize, end: usize) -> Option<usize> {
        let mut q = pos;
        while q < end && self.b[q] == b'`' {
            q += 1;
        }
        let n = q - pos;
        // Bounded plain scan first; the run index is built only when that is exceeded.
        let found = crate::ticks::find_run(self.b, 0, q, end, n, SCAN_BUDGET, &mut self.ticks);
        if let Some(r) = found {
            self.push(InlineKind::Code { content: self.sp(q, r) }, pos, r + n);
            return Some(r + n);
        }
        // no match: the whole run is literal
        Some(self.literal(pos, q))
    }

    fn literal(&mut self, a: usize, b: usize) -> usize {
        self.push(InlineKind::Text, a, b);
        b
    }

    fn dollar(&mut self, pos: usize, end: usize) -> Option<usize> {
        let b = self.b;
        // GitLab $`…`$
        if b.get(pos + 1) == Some(&b'`') {
            if let Some(close) = self.find_memo(pos + 2, end, b"`$", FAIL_GITLAB)
                && close > pos + 2
            {
                self.push(
                    InlineKind::Math {
                        delim: MathInline::Gitlab,
                        content: self.sp(pos + 2, close),
                    },
                    pos,
                    close + 2,
                );
                return Some(close + 2);
            }
            return None;
        }
        // $$…$$ inline display
        if b.get(pos + 1) == Some(&b'$') {
            if self.fails & FAIL_DD != 0 {
                return Some(self.literal(pos, pos + 2));
            }
            let mut r = pos + 2;
            while r + 1 < end {
                if b[r] == b'\\' {
                    r += 2;
                    continue;
                }
                if b[r] == b'$' && b[r + 1] == b'$' {
                    if r > pos + 2 {
                        self.push(
                            InlineKind::Math {
                                delim: MathInline::DoubleDollar,
                                content: self.sp(pos + 2, r),
                            },
                            pos,
                            r + 2,
                        );
                        return Some(r + 2);
                    }
                    return Some(self.literal(pos, pos + 2));
                }
                r += 1;
            }
            self.fails |= FAIL_DD;
            return Some(self.literal(pos, pos + 2));
        }
        // $…$ (R4.2.1)
        let legacy = self.opts.legacy_inline_math;
        let next = self.char_at(pos + 1)?;
        if !legacy && is_ws_char(next) {
            return None;
        }
        if self.fails & FAIL_DOLLAR != 0 {
            return None;
        }
        let mut r = pos + 1;
        while r < end {
            match b[r] {
                b'\\' => {
                    r += 2;
                    continue;
                }
                b'$' => {
                    let before = self.char_before(r).unwrap_or(' ');
                    let after_digit = b.get(r + 1).is_some_and(|c| c.is_ascii_digit());
                    if r > pos + 1 && (legacy || (!is_ws_char(before) && !after_digit)) {
                        self.push(
                            InlineKind::Math {
                                delim: MathInline::Dollar,
                                content: self.sp(pos + 1, r),
                            },
                            pos,
                            r + 1,
                        );
                        return Some(r + 1);
                    }
                }
                _ => {}
            }
            r += 1;
        }
        self.fails |= FAIL_DOLLAR;
        None
    }

    /// `find_from` with a fail memo flag.
    fn find_memo(&mut self, from: usize, end: usize, pat: &[u8], flag: u32) -> Option<usize> {
        if self.fails & flag != 0 {
            return None;
        }
        let r = find_from(self.b, from, end, pat);
        if r.is_none() {
            self.fails |= flag;
        }
        r
    }

    fn flanking(&self, start: usize, end: usize) -> (bool, bool, bool, bool) {
        let before = self.char_before(start).unwrap_or(' ');
        let after = self.char_at(end).unwrap_or(' ');
        let bw = before.is_whitespace();
        let aw = after.is_whitespace();
        let bp = is_punct_or_symbol(before);
        let ap = is_punct_or_symbol(after);
        let left = !aw && (!ap || bw || bp);
        let right = !bw && (!bp || aw || ap);
        (left, right, bp, ap)
    }

    fn add_delim(&mut self, node: u32, ch: u8, count: usize, can_open: bool, can_close: bool) {
        let idx = self.delims.len();
        self.delims.push(Delim {
            node,
            ch,
            count,
            orig: count,
            can_open,
            can_close,
            prev: self.delim_tail,
            next: None,
        });
        if let Some(t) = self.delim_tail {
            self.delims[t].next = Some(idx);
        }
        self.delim_tail = Some(idx);
    }

    fn delim_run(&mut self, pos: usize, end: usize, ch: u8) -> usize {
        let mut q = pos;
        while q < end && self.b[q] == ch {
            q += 1;
        }
        let (left, right, bp, ap) = self.flanking(pos, q);
        let (can_open, can_close) = if ch == b'*' {
            (left, right)
        } else {
            (left && (!right || bp), right && (!left || ap))
        };
        let node = self.push(InlineKind::Text, pos, q);
        if can_open || can_close {
            self.add_delim(node, ch, q - pos, can_open, can_close);
        }
        q
    }

    fn tilde(&mut self, pos: usize, end: usize) -> Option<usize> {
        let mut q = pos;
        while q < end && self.b[q] == b'~' {
            q += 1;
        }
        let n = q - pos;
        if n == 2 {
            let (left, right, _, _) = self.flanking(pos, q);
            let node = self.push(InlineKind::Text, pos, q);
            if left || right {
                self.add_delim(node, b'~', 2, left, right);
            }
            return Some(q);
        }
        if n == 1 && self.opts.subscript {
            return self.script(pos, end, b'~', InlineKind::Sub);
        }
        Some(self.literal(pos, q))
    }

    fn equals(&mut self, pos: usize, end: usize) -> Option<usize> {
        let mut q = pos;
        while q < end && self.b[q] == b'=' {
            q += 1;
        }
        if q - pos == 2 {
            let (left, right, _, _) = self.flanking(pos, q);
            let node = self.push(InlineKind::Text, pos, q);
            if left || right {
                self.add_delim(node, b'=', 2, left, right);
            }
            return Some(q);
        }
        Some(self.literal(pos, q))
    }

    fn sup(&mut self, pos: usize, end: usize) -> Option<usize> {
        self.script(pos, end, b'^', InlineKind::Sup)
    }

    /// `~x~` / `^x^`: no unescaped whitespace inside (R4.3.1).
    fn script(&mut self, pos: usize, end: usize, ch: u8, kind: InlineKind) -> Option<usize> {
        let b = self.b;
        let mut r = pos + 1;
        while r < end {
            match b[r] {
                b'\\' => r += 2,
                c if c == ch => {
                    if r == pos + 1 {
                        return None;
                    }
                    let inner = &self.s[pos + 1..r];
                    let children = parse_at(inner, self.base + pos + 1, self.opts, self.ctx, true);
                    let id = self.push(kind, pos, r + 1);
                    self.set_children(id, children);
                    return Some(r + 1);
                }
                b' ' | b'\t' | b'\n' => return None,
                _ => r += 1,
            }
        }
        None
    }

    /// Stores already-built children by converting them into arena nodes.
    fn set_children(&mut self, id: u32, children: Vec<Inline>) {
        let mut prev = NIL;
        for ch in children {
            let cid = self.nodes.len() as u32;
            let sub = ch.children;
            self.nodes.push(Node {
                kind: ch.kind,
                span: ch.span,
                first: NIL,
                last: NIL,
                next: NIL,
                prev,
            });
            if prev == NIL {
                self.nodes[id as usize].first = cid;
            } else {
                self.nodes[prev as usize].next = cid;
            }
            self.nodes[id as usize].last = cid;
            if !sub.is_empty() {
                self.set_children(cid, sub);
            }
            prev = cid;
        }
    }

    /// The first `]]` at or after `from`, before the end of `from`'s line and `end`.
    fn find_wiki_close(&mut self, from: usize, end: usize) -> Option<usize> {
        if let Some((a, line_end, close)) = self.wiki_close
            && a <= from
            && from <= line_end
        {
            // nothing closes between `a` and `close`: the same answer for any later start
            if close == usize::MAX {
                return None;
            }
            if from <= close {
                return Some(close);
            }
        }
        // the line end is cached on its own: openers after the last closer on a long line
        // must not rescan to the line end (review P1-3)
        let line_end = match self.wiki_line {
            Some((a, e)) if a <= from && from <= e => e,
            _ => {
                let e = memchr::memchr(b'\n', &self.b[from..end]).map_or(end, |i| from + i);
                self.wiki_scanned += e - from;
                self.wiki_line = Some((from, e));
                e
            }
        };
        let found = memchr::memmem::find(&self.b[from..line_end], b"]]").map(|i| from + i);
        self.wiki_scanned += found.map_or(line_end, |c| c + 2) - from;
        self.wiki_close = Some((from, line_end, found.unwrap_or(usize::MAX)));
        found
    }

    /// `[[…]]` at `pos` (`embed`: `![[…]]`, shown as plain text for now). Spec 06 V2.1.2:
    /// the content up to the first `]]` on the line must be non-empty and free of `[[`; a code
    /// span opened inside it and closed after it takes precedence (V2.1.3). Followed directly
    /// by `(`, the brackets are left to the standard link rules (owner decision 2026-10-28).
    fn wikilink(&mut self, pos: usize, end: usize, embed: bool) -> Option<usize> {
        let open = pos + if embed { 3 } else { 2 };
        if open > end {
            return None;
        }
        let close = self.find_wiki_close(open, end)?;
        // `[[1]](url)`: brackets directly followed by `(` stay a standard link whose text is
        // `[1]` (citation style); the wikilink reading does not apply
        if self.b.get(close + 2) == Some(&b'(') {
            return None;
        }
        // V2.1.2 rule 1: no `[[` inside (checked against the last `[[` before this `]]`)
        let last = match self.wiki_last_open {
            Some((c, l)) if c == close => l,
            _ => {
                // openers are tried left to right and get non-decreasing closers: scanning from
                // the first opener of this closer covers every later one (linear overall)
                let l = memchr::memmem::rfind(&self.b[open..close], b"[[").map(|i| open + i);
                self.wiki_scanned += close - open;
                self.wiki_last_open = Some((close, l));
                l
            }
        };
        if last.is_some_and(|l| l >= open) {
            return None;
        }
        let content = &self.s[open..close];
        let parts = crate::wikilink::split(content)?;
        // a backtick run inside whose code span would end after `]]` wins
        let mut i = open;
        while let Some(rel) = memchr::memchr(b'`', &self.b[i..close]) {
            let a = i + rel;
            let mut q = a;
            while q < end && self.b[q] == b'`' {
                q += 1;
            }
            let n = q - a;
            match crate::ticks::find_run(self.b, 0, q, end, n, SCAN_BUDGET, &mut self.ticks) {
                Some(r) if r + n > close => return None,
                Some(r) => i = (r + n).min(close),
                None => i = q.min(close),
            }
            if i >= close {
                break;
            }
        }
        if embed {
            self.push(InlineKind::Text, pos, close + 2);
            return Some(close + 2);
        }
        let display = parts.display.map(|r| self.sp(open + r.start, open + r.end));
        self.push(
            InlineKind::WikiLink {
                target: parts.target,
                subpath: parts.subpath,
                inner: self.sp(open, close),
                display,
            },
            pos,
            close + 2,
        );
        Some(close + 2)
    }

    fn open_bracket(&mut self, pos: usize, image: bool) -> usize {
        // Footnote reference [^label]
        if !image
            && self.b[pos] == b'['
            && self.b.get(pos + 1) == Some(&b'^')
            && let Some(close) = footnote_label_end(self.b, pos + 2)
        {
            let label = &self.s[pos + 2..close];
            if !label.is_empty() && !label.contains(char::is_whitespace) {
                let key = normalize_label(label);
                if self.ctx.footnotes.contains(&key) {
                    self.push(InlineKind::FootnoteRef { label: key }, pos, close + 1);
                    return close + 1;
                }
            }
        }
        let len = if image {
            2
        } else if self.b[pos] == b'[' {
            1
        } else {
            '【'.len_utf8()
        };
        let node = self.push(InlineKind::Text, pos, pos + len);
        self.brackets.push(Bracket {
            node,
            image,
            active: true,
            delim_top: self.delim_tail,
            text_start: pos + len,
        });
        pos + len
    }

    fn close_bracket(&mut self, pos: usize, end: usize) -> usize {
        let blen = if self.b[pos] == b']' { 1 } else { '】'.len_utf8() };
        let Some(br) = self.brackets.pop() else {
            return self.literal(pos, pos + blen);
        };
        if !br.active {
            return self.literal(pos, pos + blen);
        }
        let after = pos + blen;
        let label_text = &self.s[br.text_start..pos];
        // Inline link/image: ( … )
        let mut result: Option<(String, Option<String>, LinkKind, usize)> = None;
        if let Some((dest, title, e)) = self.inline_dest(after, end) {
            result = Some((dest, title, LinkKind::Inline, e));
        }
        // CommonMark: a link label is at most 999 characters. For full references the label is
        // the second bracket; the display text may be longer (review: `[a…a][x]`).
        if result.is_none() {
            // Full / collapsed reference
            if after < end && self.b[after] == b'[' {
                if let Some(rel) = memchr::memchr(b']', &self.b[after + 1..end.min(after + 1 + MAX_LABEL_BYTES)]) {
                    let close = after + 1 + rel;
                    let lab = &self.s[after + 1..close];
                    let (key, kind) = if lab.trim().is_empty() {
                        (
                            label_len_ok(label_text).then(|| normalize_label(label_text)),
                            LinkKind::Collapsed,
                        )
                    } else {
                        (label_len_ok(lab).then(|| normalize_label(lab)), LinkKind::Full)
                    };
                    if let Some(key) = key
                        && let Some((d, t)) = self.ctx.link_defs.get(&key)
                    {
                        result = Some((d.clone(), t.clone(), kind, close + 1));
                    }
                }
            } else if label_len_ok(label_text) {
                let key = normalize_label(label_text);
                if !key.is_empty()
                    && let Some((d, t)) = self.ctx.link_defs.get(&key)
                {
                    result = Some((d.clone(), t.clone(), LinkKind::Shortcut, after));
                }
            }
        }
        let Some((dest, title, kind, mut link_end)) = result else {
            return self.literal(pos, pos + blen);
        };
        // Image attribute list {…}
        let mut attrs = None;
        if br.image
            && link_end < end
            && self.b[link_end] == b'{'
            && let Some(rel) = memchr::memchr2(b'}', b'\n', &self.b[link_end..end])
            && self.b[link_end + rel] == b'}'
        {
            attrs = Some(self.sp(link_end, link_end + rel + 1));
            link_end += rel + 1;
        }
        // Resolve emphasis inside, then wrap the nodes after the bracket.
        self.process_emphasis(br.delim_top);
        let bn = br.node as usize;
        let first = self.nodes[bn].next;
        let last = if first == NIL { NIL } else { self.tail };
        let start = self.nodes[bn].span.start;
        self.nodes[bn].kind = if br.image {
            InlineKind::Image {
                dest,
                title,
                kind,
                attrs,
            }
        } else {
            InlineKind::Link { dest, title, kind }
        };
        self.nodes[bn].span = Span::new(start, self.base + link_end);
        self.nodes[bn].first = first;
        self.nodes[bn].last = last;
        if first != NIL {
            self.nodes[first as usize].prev = NIL;
        }
        self.nodes[bn].next = NIL;
        self.tail = br.node;
        if !br.image {
            for b in &mut self.brackets {
                if !b.image {
                    b.active = false;
                }
            }
        }
        link_end
    }

    /// Parses `(dest "title")` at `p`. Returns (dest, title, end).
    fn inline_dest(&mut self, p: usize, end: usize) -> Option<(String, Option<String>, usize)> {
        let b = self.b;
        let remap = self.opts.remap_unicode_punctuation;
        let (open_len, close_ch): (usize, &str) = if p < end && b[p] == b'(' {
            (1, ")")
        } else if remap && self.s[p..end].starts_with('（') {
            ('（'.len_utf8(), "）")
        } else {
            return None;
        };
        let mut q = skip_ws_nl(b, p + open_len, end);
        let dest;
        if q < end && b[q] == b'<' {
            if let Some((fs, fe)) = self.angle_fail
                && fs < q
                && q < fe
            {
                return None;
            }
            let s = q + 1;
            let mut r = s;
            while r < end && b[r] != b'>' && b[r] != b'\n' {
                if b[r] == b'\\' {
                    r += 1;
                }
                r += 1;
            }
            if r >= end || b[r] != b'>' {
                self.angle_fail = Some((q, r.min(end)));
                return None;
            }
            dest = unescape(&self.s[s..r]);
            q = r + 1;
        } else if remap && close_ch != ")" {
            // rare path (Unicode remapping): plain scan
            let s = q;
            let mut depth = 0i32;
            while q < end {
                let c = b[q];
                if c == b'\\' && q + 1 < end {
                    q += 2;
                    continue;
                }
                if c <= b' ' || b[q..end].starts_with(close_ch.as_bytes()) {
                    break;
                }
                if c == b'(' {
                    depth += 1;
                } else if c == b')' {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                q += 1;
            }
            if depth != 0 {
                return None;
            }
            dest = unescape(&self.s[s..q]);
        } else {
            // Fast path: bounded plain scan; beyond the budget use the index, which answers
            // in O(log n) (review: `[a](b()` repeated).
            let stop = match raw_dest_scan(b, q, end, SCAN_BUDGET) {
                Some(Ok(stop)) => stop,
                Some(Err(())) => return None,
                None => {
                    let idx = self.dests.get_or_insert_with(|| DestIndex::build(b));
                    idx.raw_end(q, end)?
                }
            };
            dest = unescape(&self.s[q..stop]);
            q = stop;
        }
        let ws_start = q;
        q = skip_ws_nl(b, q, end);
        let mut title = None;
        if q < end && q > ws_start {
            let (open, close): (&str, &str) = match b[q] {
                b'"' => ("\"", "\""),
                b'\'' => ("'", "'"),
                b'(' => ("(", ")"),
                _ if remap && b[q..end].starts_with("“".as_bytes()) => ("“", "”"),
                _ => ("", ""),
            };
            if !open.is_empty() {
                let flag = match open {
                    "\"" => FAIL_TITLE_DQ,
                    "'" => FAIL_TITLE_SQ,
                    "(" => FAIL_TITLE_PAREN,
                    _ => FAIL_TITLE_CURLY,
                };
                if self.fails & flag != 0 {
                    return None;
                }
                let s = q + open.len();
                let mut r = s;
                let mut found = None;
                while r < end {
                    if b[r] == b'\\' {
                        r += 2;
                        continue;
                    }
                    if b[r..end].starts_with(close.as_bytes()) {
                        found = Some(r);
                        break;
                    }
                    r += 1;
                }
                let Some(r) = found else {
                    self.fails |= flag;
                    return None;
                };
                title = Some(unescape(&self.s[s..r]));
                q = skip_ws_nl(b, r + close.len(), end);
            }
        }
        if b[q.min(end)..end].starts_with(close_ch.as_bytes()) {
            Some((dest, title, q + close_ch.len()))
        } else {
            None
        }
    }

    fn angle(&mut self, pos: usize, end: usize) -> Option<usize> {
        let b = self.b;
        let s = self.s;
        // Autolink
        if let Some(rel) = memchr::memchr2(b'>', b'<', &b[pos + 1..end]) {
            let close = pos + 1 + rel;
            if b[close] == b'>' {
                let inner = &s[pos + 1..close];
                if !inner.contains(char::is_whitespace) && !inner.is_empty() {
                    if is_uri(inner) {
                        self.push(
                            InlineKind::Autolink {
                                url: inner.to_string(),
                                email: false,
                            },
                            pos,
                            close + 1,
                        );
                        return Some(close + 1);
                    }
                    if is_email(inner) {
                        self.push(
                            InlineKind::Autolink {
                                url: inner.to_string(),
                                email: true,
                            },
                            pos,
                            close + 1,
                        );
                        return Some(close + 1);
                    }
                    if inner.len() > 4 && inner.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("www.")) {
                        self.push(
                            InlineKind::Autolink {
                                url: format!("http://{inner}"),
                                email: false,
                            },
                            pos,
                            close + 1,
                        );
                        return Some(close + 1);
                    }
                }
            }
        }
        // Raw HTML
        let e = html_inline_end(b, pos, end, &mut self.fails)?;
        self.push(InlineKind::Html, pos, e);
        Some(e)
    }

    fn entity(&mut self, pos: usize, end: usize) -> Option<usize> {
        let b = self.b;
        let mut q = pos + 1;
        if q < end && b[q] == b'#' {
            q += 1;
            let hex = q < end && (b[q] == b'x' || b[q] == b'X');
            if hex {
                q += 1;
            }
            let s = q;
            while q < end
                && (if hex {
                    b[q].is_ascii_hexdigit()
                } else {
                    b[q].is_ascii_digit()
                })
            {
                q += 1;
            }
            let n = q - s;
            if n == 0 || n > if hex { 6 } else { 7 } {
                return None;
            }
        } else {
            let s = q;
            while q < end && b[q].is_ascii_alphanumeric() {
                q += 1;
            }
            if q == s || q - s > 32 || !b[s].is_ascii_alphabetic() {
                return None;
            }
        }
        if q < end && b[q] == b';' {
            self.push(InlineKind::Entity, pos, q + 1);
            Some(q + 1)
        } else {
            None
        }
    }

    fn newline(&mut self, pos: usize, text_start: &mut usize) -> usize {
        // trailing spaces (incl. NBSP, R3.1.3) before the newline
        let s = self.s;
        let mut t = pos;
        let mut spaces = 0;
        loop {
            if t > *text_start && s.as_bytes()[t - 1] == b' ' {
                t -= 1;
                spaces += 1;
            } else if t >= *text_start + 2 && s.as_bytes()[t - 2..t] == [0xC2, 0xA0] {
                t -= 2;
                spaces += 1;
            } else if t > *text_start && s.as_bytes()[t - 1] == b'\t' {
                t -= 1;
            } else {
                break;
            }
        }
        self.text(*text_start, t);
        if spaces >= 2 {
            self.push(InlineKind::HardBreak, t, pos + 1);
        } else {
            self.push(InlineKind::SoftBreak, t, pos + 1);
        }
        // leading whitespace of the next line is not content
        let mut n = pos + 1;
        while n < self.b.len() && matches!(self.b[n], b' ' | b'\t') {
            n += 1;
        }
        *text_start = n;
        n
    }

    fn emoji(&mut self, pos: usize, end: usize) -> Option<usize> {
        let b = self.b;
        let mut q = pos + 1;
        while q < end && (b[q].is_ascii_alphanumeric() || matches!(b[q], b'_' | b'+' | b'-')) {
            q += 1;
        }
        if q == pos + 1 || q >= end || b[q] != b':' {
            return None;
        }
        let name = &self.s[pos + 1..q];
        let e = emojis::get_by_shortcode(name)?;
        self.push(InlineKind::Emoji { emoji: e.as_str() }, pos, q + 1);
        Some(q + 1)
    }

    fn remove_delim(&mut self, i: usize) {
        let (p, n) = (self.delims[i].prev, self.delims[i].next);
        if let Some(p) = p {
            self.delims[p].next = n;
        }
        if let Some(n) = n {
            self.delims[n].prev = p;
        } else {
            self.delim_tail = p;
        }
    }

    fn process_emphasis(&mut self, bottom: Option<usize>) {
        let first = match bottom {
            Some(b) => self.delims.get(b).and_then(|d| d.next),
            None => {
                // first alive delim: walk back from tail
                let mut cur = self.delim_tail;
                let mut first = None;
                while let Some(c) = cur {
                    first = Some(c);
                    cur = self.delims[c].prev;
                }
                first
            }
        };
        let mut openers_bottom: HashMap<(u8, bool, usize), Option<usize>> = HashMap::new();
        let mut closer = first;
        while let Some(ci) = closer {
            let d = self.delims[ci];
            if !d.can_close {
                closer = d.next;
                continue;
            }
            let key = if d.ch == b'*' || d.ch == b'_' {
                (d.ch, d.can_open, d.orig % 3)
            } else {
                (d.ch, false, 0)
            };
            let ob = openers_bottom.get(&key).copied().unwrap_or(bottom);
            let mut oi = d.prev;
            let mut found = None;
            while let Some(o) = oi {
                if Some(o) == ob || Some(o) == bottom {
                    break;
                }
                let od = self.delims[o];
                if od.ch == d.ch && od.can_open {
                    let ok = if d.ch == b'*' || d.ch == b'_' {
                        let odd = (od.can_close || d.can_open)
                            && (od.orig + d.orig).is_multiple_of(3)
                            && !(od.orig.is_multiple_of(3) && d.orig.is_multiple_of(3));
                        !odd
                    } else {
                        od.count == 2 && d.count == 2
                    };
                    if ok {
                        found = Some(o);
                        break;
                    }
                }
                oi = od.prev;
            }
            let Some(o) = found else {
                openers_bottom.insert(key, d.prev);
                if !d.can_open {
                    self.remove_delim(ci);
                }
                closer = d.next;
                continue;
            };
            let od = self.delims[o];
            let (n, kind) = match d.ch {
                b'*' | b'_' => {
                    if od.count >= 2 && d.count >= 2 {
                        (2, InlineKind::Strong)
                    } else {
                        (1, InlineKind::Emph)
                    }
                }
                b'~' => (2, InlineKind::Strike),
                _ => (2, InlineKind::Highlight),
            };
            let on = od.node as usize;
            let cn = d.node as usize;
            self.nodes[on].span.end -= n;
            let estart = self.nodes[on].span.end;
            self.nodes[cn].span.start += n;
            let eend = self.nodes[cn].span.start;
            // new node between opener node and closer node
            let id = self.nodes.len() as u32;
            let first_child = self.nodes[on].next;
            let last_child = self.nodes[cn].prev;
            let has_children = first_child != d.node;
            self.nodes.push(Node {
                kind,
                span: Span::new(estart, eend),
                first: if has_children { first_child } else { NIL },
                last: if has_children { last_child } else { NIL },
                next: d.node,
                prev: od.node,
            });
            if has_children {
                self.nodes[first_child as usize].prev = NIL;
                self.nodes[last_child as usize].next = NIL;
            }
            self.nodes[on].next = id;
            self.nodes[cn].prev = id;
            // remove delims between opener and closer
            self.delims[o].next = Some(ci);
            self.delims[ci].prev = Some(o);
            self.delims[o].count -= n;
            self.delims[ci].count -= n;
            if self.delims[o].count == 0 {
                self.remove_delim(o);
            }
            if self.delims[ci].count == 0 {
                let next = self.delims[ci].next;
                self.remove_delim(ci);
                closer = next;
            }
        }
        // remove all delimiters above bottom
        match bottom {
            Some(b) => {
                self.delims[b].next = None;
                self.delim_tail = Some(b);
            }
            None => self.delim_tail = None,
        }
    }

    fn collect(&self, mut id: u32, out: &mut Vec<Inline>, depth: usize) {
        while id != NIL {
            let n = &self.nodes[id as usize];
            if n.span.start == n.span.end && matches!(n.kind, InlineKind::Text) {
                id = n.next;
                continue;
            }
            if n.first != NIL && depth >= MAX_INLINE_DEPTH {
                // Too deep (adversarial input): keep the source as literal text (C39).
                self.flatten(id, out);
                id = n.next;
                continue;
            }
            let mut children = Vec::new();
            if n.first != NIL {
                self.collect(n.first, &mut children, depth + 1);
            }
            push_merged(
                out,
                Inline {
                    kind: n.kind.clone(),
                    span: n.span,
                    children,
                },
            );
            id = n.next;
        }
    }

    /// Emits the subtree at `root` as flat nodes: wrapper markers become text. Iterative.
    fn flatten(&self, root: u32, out: &mut Vec<Inline>) {
        enum Act {
            Node(u32),
            Text(Span),
        }
        let mut stack = vec![Act::Node(root)];
        while let Some(act) = stack.pop() {
            match act {
                Act::Text(sp) => {
                    if sp.start < sp.end {
                        push_merged(
                            out,
                            Inline {
                                kind: InlineKind::Text,
                                span: sp,
                                children: Vec::new(),
                            },
                        );
                    }
                }
                Act::Node(id) => {
                    let n = &self.nodes[id as usize];
                    if n.first == NIL {
                        if !(n.span.start == n.span.end && matches!(n.kind, InlineKind::Text)) {
                            push_merged(
                                out,
                                Inline {
                                    kind: n.kind.clone(),
                                    span: n.span,
                                    children: Vec::new(),
                                },
                            );
                        }
                        continue;
                    }
                    let first = &self.nodes[n.first as usize];
                    let last = &self.nodes[n.last as usize];
                    stack.push(Act::Text(Span::new(last.span.end, n.span.end)));
                    let mut kids = Vec::new();
                    let mut c = n.first;
                    while c != NIL {
                        kids.push(c);
                        c = self.nodes[c as usize].next;
                    }
                    for k in kids.into_iter().rev() {
                        stack.push(Act::Node(k));
                    }
                    stack.push(Act::Text(Span::new(n.span.start, first.span.start)));
                }
            }
        }
    }
}

/// Bytes scanned linearly before switching to an index (keeps the common case allocation-free
/// and the worst case linear).
const SCAN_BUDGET: usize = 512;

/// Plain raw-destination scan from `q`, giving up after `budget` bytes (`None`).
/// `Some(Err)` means unbalanced parentheses.
fn raw_dest_scan(b: &[u8], mut q: usize, end: usize, budget: usize) -> Option<Result<usize, ()>> {
    let limit = q.saturating_add(budget);
    let mut depth = 0i32;
    while q < end {
        if q >= limit {
            return None;
        }
        let c = b[q];
        if c == b'\\' && q + 1 < end {
            q += 2;
            continue;
        }
        if c <= b' ' {
            break;
        }
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        }
        q += 1;
    }
    Some(if depth == 0 { Ok(q.min(end)) } else { Err(()) })
}

/// Byte bound for a 999-character label (4 bytes per char at most).
const MAX_LABEL_BYTES: usize = 999 * 4 + 1;

/// CommonMark: link labels have at most 999 characters (checked in O(1000)).
fn label_len_ok(s: &str) -> bool {
    s.len() <= 999 || (s.len() < MAX_LABEL_BYTES && s.chars().count() <= 999)
}

/// Index for raw link destinations (`[a](dest)`): where a scan starting at `q` stops (first
/// whitespace/control byte, or the first `)` that would close the link at depth 0), answered by
/// binary search. Escapes pair up left to right, as in the plain scan.
struct DestIndex {
    ws: Vec<usize>,
    /// Unescaped parentheses: position and the depth *after* them.
    parens: Vec<(usize, i32)>,
    /// Depth before → positions of unescaped `)`.
    closes: HashMap<i32, Vec<usize>>,
}

impl DestIndex {
    fn build(b: &[u8]) -> Self {
        let mut ws = Vec::new();
        let mut parens = Vec::new();
        let mut closes: HashMap<i32, Vec<usize>> = HashMap::new();
        let mut depth = 0i32;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == b'\\' && i + 1 < b.len() {
                i += 2;
                continue;
            }
            if c <= b' ' {
                ws.push(i);
            } else if c == b'(' {
                depth += 1;
                parens.push((i, depth));
            } else if c == b')' {
                closes.entry(depth).or_default().push(i);
                depth -= 1;
                parens.push((i, depth));
            }
            i += 1;
        }
        DestIndex { ws, parens, closes }
    }

    fn depth_at(&self, q: usize) -> i32 {
        let i = self.parens.partition_point(|&(p, _)| p < q);
        if i == 0 {
            return 0;
        }
        self.parens[i - 1].1
    }

    /// End of a raw destination starting at `q`, or `None` when its parentheses are unbalanced.
    fn raw_end(&self, q: usize, end: usize) -> Option<usize> {
        let wi = self.ws.partition_point(|&p| p < q);
        let ws_stop = self.ws.get(wi).copied().unwrap_or(end).min(end);
        let dq = self.depth_at(q);
        let close = self.closes.get(&dq).and_then(|v| {
            let i = v.partition_point(|&p| p < q);
            v.get(i).copied()
        });
        match close {
            Some(c) if c < ws_stop => Some(c),
            _ => (self.depth_at(ws_stop) == dq).then_some(ws_stop),
        }
    }
}

/// Maximum nesting depth of inline nodes; deeper structure is kept as literal text (C39).
pub const MAX_INLINE_DEPTH: usize = 64;

fn push_merged(out: &mut Vec<Inline>, n: Inline) {
    if matches!(n.kind, InlineKind::Text)
        && let Some(last) = out.last_mut()
        && matches!(last.kind, InlineKind::Text)
        && last.span.end == n.span.start
    {
        last.span.end = n.span.end;
        return;
    }
    out.push(n);
}

/// Position of the `]` closing a footnote label starting at `from` (no whitespace, ≤ 999
/// bytes, so the scan is bounded).
fn footnote_label_end(b: &[u8], from: usize) -> Option<usize> {
    let mut q = from;
    while q < b.len() && q - from < 1000 {
        match b[q] {
            b']' => return Some(q),
            b' ' | b'\t' | b'\n' | b'[' => return None,
            _ => q += 1,
        }
    }
    None
}

fn find_from(b: &[u8], from: usize, end: usize, pat: &[u8]) -> Option<usize> {
    if from >= end {
        return None;
    }
    memchr::memmem::find(&b[from..end], pat).map(|i| i + from)
}

fn skip_ws_nl(b: &[u8], mut p: usize, end: usize) -> usize {
    let mut nl = false;
    while p < end && matches!(b[p], b' ' | b'\t' | b'\n') {
        if b[p] == b'\n' {
            if nl {
                break;
            }
            nl = true;
        }
        p += 1;
    }
    p
}

fn is_uri(s: &str) -> bool {
    let Some(colon) = s.find(':') else { return false };
    let scheme = &s[..colon];
    (2..=32).contains(&scheme.len())
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
        && !s.bytes().any(|c| c < 0x20 || c == b'<' || c == b'>')
}

fn is_email(s: &str) -> bool {
    let Some(at) = s.find('@') else { return false };
    let (local, domain) = (&s[..at], &s[at + 1..]);
    !local.is_empty()
        && local
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&c))
        && !domain.is_empty()
        && domain.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

/// End of a raw inline HTML construct starting at `pos` (`<`), CommonMark grammar.
fn html_inline_end(b: &[u8], pos: usize, end: usize, fails: &mut u32) -> Option<usize> {
    let s = &b[pos..end];
    let mut memo = |flag: u32, from: usize, pat: &[u8], add: usize| -> Option<usize> {
        if *fails & flag != 0 {
            return None;
        }
        let r = find_from(b, from, end, pat).map(|i| i + add);
        if r.is_none() {
            *fails |= flag;
        }
        r
    };
    if s.starts_with(b"<!--") {
        if s[4..].starts_with(b">") || s[4..].starts_with(b"->") {
            return Some(pos + 4 + if s[4] == b'>' { 1 } else { 2 });
        }
        return memo(FAIL_COMMENT, pos + 4, b"-->", 3);
    }
    if s.starts_with(b"<?") {
        return memo(FAIL_PI, pos + 2, b"?>", 2);
    }
    if s.starts_with(b"<![CDATA[") {
        return memo(FAIL_CDATA, pos + 9, b"]]>", 3);
    }
    if s.len() > 2 && s[1] == b'!' && s[2].is_ascii_alphabetic() {
        return memo(FAIL_DECL, pos + 2, b">", 1);
    }
    let mut q = 1;
    let closing = s.get(1) == Some(&b'/');
    if closing {
        q = 2;
    }
    if !s.get(q)?.is_ascii_alphabetic() {
        return None;
    }
    while q < s.len() && (s[q].is_ascii_alphanumeric() || s[q] == b'-') {
        q += 1;
    }
    if closing {
        while q < s.len() && matches!(s[q], b' ' | b'\t' | b'\n') {
            q += 1;
        }
        return (s.get(q) == Some(&b'>')).then_some(pos + q + 1);
    }
    loop {
        // attributes
        let ws_start = q;
        while q < s.len() && matches!(s[q], b' ' | b'\t' | b'\n') {
            q += 1;
        }
        match s.get(q)? {
            b'>' => return Some(pos + q + 1),
            b'/' => return (s.get(q + 1) == Some(&b'>')).then_some(pos + q + 2),
            c if q > ws_start && (c.is_ascii_alphabetic() || *c == b'_' || *c == b':') => {
                while q < s.len() && (s[q].is_ascii_alphanumeric() || matches!(s[q], b'_' | b'.' | b':' | b'-')) {
                    q += 1;
                }
                let save = q;
                while q < s.len() && matches!(s[q], b' ' | b'\t' | b'\n') {
                    q += 1;
                }
                if s.get(q) == Some(&b'=') {
                    q += 1;
                    while q < s.len() && matches!(s[q], b' ' | b'\t' | b'\n') {
                        q += 1;
                    }
                    match s.get(q)? {
                        b'"' | b'\'' => {
                            let qc = s[q];
                            let flag = if qc == b'"' { FAIL_DQ } else { FAIL_SQ };
                            let e = memo(flag, pos + q + 1, &[qc], 0)?;
                            q = e - pos + 1;
                        }
                        _ => {
                            let st = q;
                            while q < s.len() && !b" \t\n\"'=<>`".contains(&s[q]) {
                                q += 1;
                            }
                            if q == st {
                                return None;
                            }
                        }
                    }
                } else {
                    q = save;
                }
            }
            _ => return None,
        }
    }
}

/// Splits text nodes (outside links) at bare URLs (GFM extended autolinks; display only).
fn detect_bare_urls(s: &str, nodes: &mut Vec<Inline>) {
    let mut i = 0;
    while i < nodes.len() {
        match nodes[i].kind {
            InlineKind::Text => {
                let sp = nodes[i].span;
                if let Some((a, e, url)) = find_bare_url(s, sp.start, sp.end) {
                    let mut repl = Vec::with_capacity(3);
                    if a > sp.start {
                        repl.push(Inline {
                            kind: InlineKind::Text,
                            span: Span::new(sp.start, a),
                            children: vec![],
                        });
                    }
                    repl.push(Inline {
                        kind: InlineKind::BareUrl { url },
                        span: Span::new(a, e),
                        children: vec![],
                    });
                    let has_rest = e < sp.end;
                    if has_rest {
                        repl.push(Inline {
                            kind: InlineKind::Text,
                            span: Span::new(e, sp.end),
                            children: vec![],
                        });
                    }
                    let n = repl.len();
                    nodes.splice(i..=i, repl);
                    i += if has_rest { n - 1 } else { n };
                    continue;
                }
                i += 1;
            }
            InlineKind::Link { .. }
            | InlineKind::Image { .. }
            | InlineKind::Autolink { .. }
            | InlineKind::Code { .. }
            | InlineKind::Math { .. }
            | InlineKind::Html => i += 1,
            _ => {
                detect_bare_urls(s, &mut nodes[i].children);
                i += 1;
            }
        }
    }
}

fn find_bare_url(s: &str, start: usize, end: usize) -> Option<(usize, usize, String)> {
    let b = s.as_bytes();
    let mut p = start;
    while p < end {
        let c = b[p];
        if matches!(c, b'h' | b'w' | b'm' | b'H' | b'W' | b'M') {
            let boundary = p == 0 || {
                let prev = s[..p].chars().next_back().unwrap();
                prev.is_whitespace() || matches!(prev, '*' | '_' | '~' | '(')
            };
            if boundary {
                let rest = &s[p..end];
                let lower: String = rest.chars().take(8).collect::<String>().to_ascii_lowercase();
                let (prefix_len, add) = if lower.starts_with("https://") {
                    (8, "")
                } else if lower.starts_with("http://") {
                    (7, "")
                } else if lower.starts_with("www.") {
                    (4, "http://")
                } else if lower.starts_with("mailto:") {
                    (7, "")
                } else {
                    (0, "")
                };
                if prefix_len > 0 {
                    let mut e = p + prefix_len;
                    while e < end && !b[e].is_ascii_whitespace() && b[e] != b'<' {
                        e += 1;
                    }
                    // trailing punctuation; paren balance counted once (linear)
                    let mut opens = s[p..e].matches('(').count();
                    let mut closes = s[p..e].matches(')').count();
                    loop {
                        if e <= p + prefix_len {
                            break;
                        }
                        let last = b[e - 1];
                        if matches!(
                            last,
                            b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~' | b'\'' | b'"'
                        ) {
                            e -= 1;
                        } else if last == b')' {
                            if closes > opens {
                                e -= 1;
                                closes -= 1;
                            } else {
                                break;
                            }
                        } else if last == b';' {
                            // entity-like suffix &xx;
                            if let Some(amp) = s[p..e].rfind('&') {
                                let ent = &s[p + amp + 1..e - 1];
                                if !ent.is_empty() && ent.bytes().all(|c| c.is_ascii_alphanumeric()) {
                                    e = p + amp;
                                    opens = s[p..e].matches('(').count();
                                    closes = s[p..e].matches(')').count();
                                    continue;
                                }
                            }
                            e -= 1;
                        } else {
                            break;
                        }
                    }
                    let body = &s[p + prefix_len..e];
                    let valid = !body.is_empty()
                        && (lower.starts_with("mailto:") && body.contains('@')
                            || !lower.starts_with("mailto:")
                                && body.split(['/', '?', '#']).next().is_some_and(|d| {
                                    !d.is_empty() && (add.is_empty() || d.contains('.')) && !d.contains('_')
                                }));
                    if valid {
                        return Some((p, e, format!("{add}{}", &s[p..e])));
                    }
                }
            }
        }
        p += 1;
        while p < end && !s.is_char_boundary(p) {
            p += 1;
        }
    }
    None
}

/// Plain text of inline nodes (for alt text, slugs, TOC): markers removed, escapes resolved.
pub fn plain_text(s: &str, nodes: &[Inline]) -> String {
    let mut out = String::new();
    fn go(s: &str, nodes: &[Inline], out: &mut String) {
        for n in nodes {
            match &n.kind {
                InlineKind::Text | InlineKind::BareUrl { .. } => out.push_str(n.span.slice(s)),
                InlineKind::Escape => out.push_str(&s[n.span.start + 1..n.span.end]),
                InlineKind::Entity => out.push_str(&decode_entity(n.span.slice(s))),
                InlineKind::Code { content } => out.push_str(content.slice(s)),
                InlineKind::SoftBreak | InlineKind::HardBreak => out.push(' '),
                InlineKind::Autolink { url, .. } => out.push_str(url),
                InlineKind::Math { content, .. } => out.push_str(content.slice(s)),
                InlineKind::Emoji { emoji } => out.push_str(emoji),
                InlineKind::WikiLink { inner, .. } => {
                    let c = inner.slice(s);
                    if let Some(w) = crate::wikilink::split(c) {
                        out.push_str(&w.label(c));
                    }
                }
                InlineKind::Html | InlineKind::FootnoteRef { .. } => {}
                _ => go(s, &n.children, out),
            }
        }
    }
    go(s, nodes, &mut out);
    out
}

/// Decodes every character reference (`&name;`, `&#N;`, `&#xN;`) in `s`; unknown or malformed
/// references stay as written. Each lookup inspects at most 34 bytes (linear overall).
pub fn decode_refs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match memchr::memchr(b';', &tail.as_bytes()[..tail.len().min(34)]).filter(|&e| e > 1) {
            Some(e) => {
                let ent = &tail[..=e];
                let d = decode_entity(ent);
                if d == ent {
                    out.push('&');
                    rest = &tail[1..];
                } else {
                    out.push_str(&d);
                    rest = &tail[e + 1..];
                }
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decodes a character reference (numeric or any HTML5 named entity); unknown names are
/// returned unchanged.
pub fn decode_entity(e: &str) -> String {
    let inner = &e[1..e.len() - 1];
    if let Some(num) = inner.strip_prefix('#') {
        let v = if let Some(h) = num.strip_prefix(['x', 'X']) {
            u32::from_str_radix(h, 16).ok()
        } else {
            num.parse().ok()
        };
        return v
            .and_then(char::from_u32)
            .filter(|&c| c != '\0')
            .unwrap_or('\u{FFFD}')
            .to_string();
    }
    // Named references: the full HTML5 table.
    let d = html_escape::decode_html_entities(e);
    if d != e && !d.ends_with(';') {
        return d.into_owned();
    }
    e.to_string()
}

#[cfg(test)]
mod tests {
    /// Review P1-3: wikilink searches scan O(n) bytes on adversarial lines — counted, not
    /// timed, so the check is exact under any machine load (issue #42).
    #[test]
    fn wikilink_scanning_is_linear() {
        let opts = crate::options::ParseOptions::default();
        let ctx = super::InlineContext::default();
        let cost = |s: &str| {
            super::WIKI_SCANNED.with(|c| c.set(0));
            let _ = super::parse_inlines(s, &opts, &ctx);
            super::WIKI_SCANNED.with(|c| c.get())
        };
        type Gen = fn(usize) -> String;
        let cases: [(&str, Gen); 6] = [
            ("many links", |n| "[[a]] ".repeat(n)),
            ("openers then closers", |n| {
                format!("{}{}", "[".repeat(n), "]".repeat(n))
            }),
            ("openers without closers", |n| "[[".repeat(n)),
            ("links then trailing openers", |n| {
                format!("{}{}", "[[a]] ".repeat(n), "[[x ".repeat(n))
            }),
            ("nested content", |n| format!("{}]]", "[[a".repeat(n))),
            ("citations", |n| "[[1]](u) ".repeat(n)),
        ];
        for (name, f) in cases {
            let (small, big) = (cost(&f(500)), cost(&f(8000)));
            assert!(
                big <= 20 * small.max(1) + 64,
                "{name}: 16× input scanned {big} bytes vs {small}"
            );
        }
    }

    use super::*;

    fn kinds(s: &str, o: &ParseOptions) -> String {
        let ctx = InlineContext::default();
        let v = parse_inlines(s, o, &ctx);
        dump(s, &v)
    }

    pub(crate) fn dump(s: &str, v: &[Inline]) -> String {
        let mut out = String::new();
        for n in v {
            let name = match &n.kind {
                InlineKind::Text => format!("t({})", n.span.slice(s)),
                InlineKind::Math { content, .. } => format!("math({})", content.slice(s)),
                InlineKind::Code { content } => format!("code({})", content.slice(s)),
                InlineKind::Escape => format!("esc({})", n.span.slice(s)),
                k => {
                    let k = format!("{k:?}");
                    let k = k.split([' ', '{', '(']).next().unwrap().to_lowercase();
                    format!("{k}[{}]", dump(s, &n.children))
                }
            };
            out.push_str(&name);
        }
        out
    }

    #[test]
    fn emphasis_basics() {
        let o = ParseOptions::default();
        assert_eq!(kinds("__bold__ and _em_", &o), "strong[t(bold)]t( and )emph[t(em)]");
        assert_eq!(kinds("***x***", &o), "emph[strong[t(x)]]");
        assert_eq!(kinds("snake_case_name", &o), "t(snake_case_name)");
        assert_eq!(kinds("~~del~~", &o), "strike[t(del)]");
        assert_eq!(kinds("a*b", &o), "t(a*b)");
    }

    #[test]
    fn links_and_code() {
        let o = ParseOptions::default();
        assert_eq!(kinds("[a](b \"t\")", &o), "link[t(a)]");
        assert_eq!(kinds("`a*b*`", &o), "code(a*b*)");
        assert_eq!(kinds("![x](y){width=2}", &o), "image[t(x)]");
        assert_eq!(kinds("[a]", &o), "t([a])");
        assert_eq!(kinds("<https://x.y>", &o), "autolink[]");
        assert_eq!(kinds("<u>x</u>", &o), "html[]t(x)html[]");
    }

    #[test]
    fn math_rules() {
        let o = ParseOptions::default();
        // T15, T16, T17
        assert_eq!(kinds("costs $5 and $6", &o), "t(costs $5 and $6)");
        assert_eq!(kinds("$x^2$ and $ y $", &o), "math(x^2)t( and $ y $)");
        assert_eq!(kinds("\\(a+b\\)", &o), "math(a+b)");
        assert_eq!(kinds("$`a`$", &o), "math(a)");
        assert_eq!(kinds("x $$a$$ y", &o), "t(x )math(a)t( y)");
    }

    #[test]
    fn bare_urls() {
        let o = ParseOptions::default();
        assert_eq!(kinds("see https://a.com/x.", &o), "t(see )bareurl[]t(.)");
        assert_eq!(kinds("www.example.com", &o), "bareurl[]");
    }
}
#[cfg(test)]
mod regress {
    #[test]
    fn html_comment_cjk() {
        let o = crate::ParseOptions::default();
        let _ = super::parse_inlines("<!--日本", &o, &Default::default());
        let _ = super::parse_inlines("!--日本", &o, &Default::default());
    }
}
