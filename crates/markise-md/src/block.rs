//! Block-level concrete syntax tree.
//!
//! Every block records byte spans into the document text. Spans never own text: the Markdown
//! source is the single source of truth (R1.2), the tree is an index over it.

use crate::text::Span;

/// Column alignment of a table column (R3.8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// GitHub alert kinds (R3.3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::Note => "note",
            AlertKind::Tip => "tip",
            AlertKind::Important => "important",
            AlertKind::Warning => "warning",
            AlertKind::Caution => "caution",
        }
    }
}

/// Delimiter style of a display-math block (R3.7.1, R3.7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathDelim {
    /// `$$` … `$$` (optionally with a `{…}` suffix, kept verbatim).
    Dollars,
    /// `\[` … `\]`.
    Brackets,
}

/// The kind of a block plus the structural facts ("pattern", R1.3) needed to re-emit it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// `lines` hold the content of each line.
    Paragraph,
    /// `lines` hold the heading text (ATX: one line, closing hashes stripped; setext: text lines).
    Heading {
        level: u8,
        setext: bool,
        /// ATX with a closing hash sequence (`## A ##`).
        closed: bool,
    },
    ThematicBreak,
    /// `lines` hold the content lines (without fences). `info` is the trimmed info string.
    FencedCode {
        fence_char: u8,
        fence_len: usize,
        /// Indentation of the opening fence (stripped from content lines, CommonMark).
        indent: usize,
        info: Span,
        opener: Span,
        closer: Option<Span>,
    },
    /// Indented code read from a file (R3.6.1); `lines` hold content after the 4-column indent.
    IndentedCode,
    /// `lines` hold the TeX content lines.
    MathBlock {
        delim: MathDelim,
        opener: Span,
        closer: Span,
    },
    /// Raw HTML; `lines` hold every line verbatim.
    HtmlBlock,
    /// GFM table; `lines[0]` header row, `lines[1]` delimiter row, then body rows.
    Table {
        aligns: Vec<Align>,
    },
    BlockQuote {
        alert: Option<AlertKind>,
    },
    List {
        ordered: bool,
        /// Bullet char (`-`, `+`, `*`) or ordered delimiter (`.`, `)`).
        marker: u8,
        start: u64,
        tight: bool,
    },
    ListItem {
        /// The bullet or number+delimiter.
        marker: Span,
        /// Ordered number when ordered.
        number: Option<u64>,
        /// Task state and the span of the `[ ]`/`[x]` box (R3.4.2).
        task: Option<(bool, Span)>,
        /// Content column relative to the marker start (marker width + spacing).
        content_offset: usize,
    },
    /// YAML front matter; `lines` hold the raw content lines (R3.10).
    FrontMatter {
        opener: Span,
        closer: Span,
    },
    /// `[toc]` and friends (R3.11.1); `span` is the marker line.
    Toc,
    /// `[^label]: …` container (R3.12.1).
    FootnoteDef {
        label: Span,
    },
    /// `[label]: url "title"` (R3.13.1).
    LinkRefDef {
        label: Span,
        dest: Span,
        title: Option<Span>,
    },
}

/// One node of the block tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// From the first byte of the block (after container prefixes) to the end of its last
    /// line's content (line terminator and trailing blank lines excluded).
    pub span: Span,
    /// Content spans of a leaf block, one per line (see `BlockKind`).
    pub lines: Vec<Span>,
    pub children: Vec<Block>,
    /// `Some(line start)` when the parser had no open block at the start of this block's first
    /// line. Only meaningful on top-level blocks; incremental re-parse uses such blocks as
    /// synchronisation points.
    pub(crate) clean_line: Option<usize>,
}

impl Block {
    pub(crate) fn new(kind: BlockKind, span: Span) -> Self {
        Block {
            kind,
            span,
            lines: Vec::new(),
            children: Vec::new(),
            clean_line: None,
        }
    }

    /// `true` for blocks that contain other blocks.
    pub fn is_container(&self) -> bool {
        matches!(
            self.kind,
            BlockKind::BlockQuote { .. }
                | BlockKind::List { .. }
                | BlockKind::ListItem { .. }
                | BlockKind::FootnoteDef { .. }
        )
    }

    /// Source text of the block (including interior container prefixes).
    pub fn source<'a>(&self, text: &'a str) -> &'a str {
        self.span.slice(text)
    }

    pub(crate) fn shift(&mut self, delta: isize) {
        self.span.shift(delta);
        if let Some(l) = &mut self.clean_line {
            *l = (*l as isize + delta) as usize;
        }
        for l in &mut self.lines {
            l.shift(delta);
        }
        match &mut self.kind {
            BlockKind::FencedCode {
                info, opener, closer, ..
            } => {
                info.shift(delta);
                opener.shift(delta);
                if let Some(c) = closer {
                    c.shift(delta);
                }
            }
            BlockKind::MathBlock { opener, closer, .. } => {
                opener.shift(delta);
                closer.shift(delta);
            }
            BlockKind::ListItem { marker, task, .. } => {
                marker.shift(delta);
                if let Some((_, s)) = task {
                    s.shift(delta);
                }
            }
            BlockKind::FrontMatter { opener, closer } => {
                opener.shift(delta);
                closer.shift(delta);
            }
            BlockKind::FootnoteDef { label } => label.shift(delta),
            BlockKind::LinkRefDef { label, dest, title } => {
                label.shift(delta);
                dest.shift(delta);
                if let Some(t) = title {
                    t.shift(delta);
                }
            }
            _ => {}
        }
        for c in &mut self.children {
            c.shift(delta);
        }
    }

    /// Applies an edit inside the block that changes no structure (ADR 0006 §7.2.1): `old_end`
    /// is the end of the replaced range (block coordinates); spans starting at or after it move
    /// by `delta`, spans ending at or after it (and starting before) grow or shrink by `delta`.
    pub(crate) fn stretch(&mut self, old_end: usize, delta: isize) {
        let f = |s: &mut Span| {
            if s.start >= old_end {
                s.shift(delta);
            } else if s.end >= old_end {
                s.end = (s.end as isize + delta) as usize;
            }
        };
        self.for_each_span(&f);
        // children ending before the edit are untouched (their spans lie inside theirs)
        // children starting at or after the edit end move as a whole
        let first = self.children.partition_point(|c| c.span.end < old_end);
        let whole = first + self.children[first..].partition_point(|c| c.span.start < old_end);
        for c in &mut self.children[first..whole] {
            c.stretch(old_end, delta);
        }
        for c in &mut self.children[whole..] {
            c.shift(delta);
        }
    }

    /// Visits every span of this block (not its children; `clean_line` excluded).
    fn for_each_span(&mut self, f: &dyn Fn(&mut Span)) {
        f(&mut self.span);
        for l in &mut self.lines {
            f(l);
        }
        match &mut self.kind {
            BlockKind::FencedCode {
                info, opener, closer, ..
            } => {
                f(info);
                f(opener);
                if let Some(c) = closer {
                    f(c);
                }
            }
            BlockKind::MathBlock { opener, closer, .. } | BlockKind::FrontMatter { opener, closer } => {
                f(opener);
                f(closer);
            }
            BlockKind::ListItem { marker, task, .. } => {
                f(marker);
                if let Some((_, s)) = task {
                    f(s);
                }
            }
            BlockKind::FootnoteDef { label } => f(label),
            BlockKind::LinkRefDef { label, dest, title } => {
                f(label);
                f(dest);
                if let Some(t) = title {
                    f(t);
                }
            }
            _ => {}
        }
    }

    /// Depth-first visit of this block and its descendants.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Block, usize)) {
        fn go<'a>(b: &'a Block, depth: usize, f: &mut impl FnMut(&'a Block, usize)) {
            f(b, depth);
            for c in &b.children {
                go(c, depth + 1, f);
            }
        }
        go(self, 0, f);
    }

    /// Content of a leaf block joined with `\n` (container prefixes removed).
    pub fn content(&self, text: &str) -> String {
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(l.slice(text));
        }
        s
    }
}
