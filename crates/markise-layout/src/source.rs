//! The source view (spec 02 §12.1, ADR 0006 §5.5, M2d): the document's text as a virtualised
//! code view — the layout half of the source editor.
//!
//! - **Items are runs of physical lines** (at most [`ITEM_LINES`] lines / about [`ITEM_BYTES`]
//!   bytes). A line longer than [`PIECE_BYTES`] is cut into pieces, each its own item, so no
//!   item is ever unbounded (a 10 MB file without line breaks still lays out piece by piece;
//!   a piece starts a new visual row). Items cover every byte of the text.
//! - **Incremental:** an edit re-chunks only the items it touches ([`SourceView::apply`]);
//!   the item lists after an edit and after a full rebuild describe the same lines (chunk
//!   boundaries may differ: they are an implementation detail; property-tested).
//! - **Virtualised:** item heights live in a Fenwick tree (estimated until laid out); only the
//!   items intersecting the viewport (plus a margin) are laid out, line by line, through the
//!   backend-neutral [`TextShaper`]. Shaped lines are cached by content, tokens and parameters,
//!   so an edit re-shapes only the edited line. Line numbers come from per-item newline counts
//!   (another Fenwick tree): no walk over the document for gutters, scrolling to the end, or
//!   line ↔ offset mapping.
//! - **Colours** come from the format's parse ([`FormatDoc::source_tokens`]) and, for code
//!   blocks, from the same background highlighter as the rendered view (the rendered model's
//!   code items, [`Highlights`]). Tokens are recomputed for the items in the edit's re-parsed
//!   range; a change of reference definitions (or any document-wide dependency) re-tokenises
//!   every item lazily (dependency-aware invalidation, ADR 0006 §7.4).
//! - **Geometry** (hit testing, caret and selection rectangles, vertical moves with a goal x,
//!   visual-row edges with soft-wrap affinity, pages) works on the laid-out lines; grapheme and
//!   word moves work on the text.
//!
//! Positions are absolute byte offsets of the document text ([`DocPos`]); everything here is
//! format-neutral.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use epaint::{Color32, Pos2, Rect, pos2};
use markise_doc::{
    Affinity as DocAffinity, DocPos, EditSummary, FormatDoc, PieceKind, SourceMap, SourceTok, SourceToken,
};
use markise_text::{Affinity, LocalPos};
use markise_theme::{Theme, TokenKind};
use unicode_segmentation::UnicodeSegmentation;

use crate::block::color;
use crate::fenwick::Fenwick;
use crate::highlight::{CodeQuery, Highlights, item_range};
use crate::model::{DocModel, ItemKind, Segment};
use crate::text::{FontRole, StyledText, TextBlock, TextShaper, TextStyle};

/// Lines per item (soft maximum).
pub const ITEM_LINES: usize = 64;
/// Bytes per item (soft maximum: an item ends after the line that crosses it).
pub const ITEM_BYTES: usize = 16 * 1024;
/// Longer lines are cut into pieces of at most this many bytes.
pub const PIECE_BYTES: usize = 4 * 1024;
/// Laid-out items kept before layouts far from the viewport are dropped.
const MAX_LAID_ITEMS: usize = 768;
/// Shaped lines kept in the line cache before it is cleared.
const MAX_CACHED_LINES: usize = 16 * 1024;
/// Space above the first line and below the last one (points).
pub const PAD_TOP: f32 = 8.0;
pub const PAD_BOTTOM: f32 = 48.0;

/// Display options of the source view (spec 02 §12.1.1 display preferences that change layout;
/// line numbers, the current-line band and whitespace marks are painted by the front end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceParams {
    /// Soft wrap at the view width (otherwise lines extend to the right).
    pub wrap: bool,
    /// Columns per tab stop.
    pub tab_width: u32,
}

impl Default for SourceParams {
    fn default() -> Self {
        SourceParams {
            wrap: true,
            tab_width: 4,
        }
    }
}

/// Font size and line height of the source view for a theme (monospace).
pub fn metrics(theme: &Theme) -> (f32, f32) {
    let size = theme.mono_size().max(8.0);
    (size, (size * 1.5).round())
}

#[derive(Clone, Debug)]
struct Item {
    len: usize,
    newlines: u32,
    /// Byte length of each unit (a physical line, or one piece of an over-long line) without
    /// its line terminator.
    units: Box<[u32]>,
    /// The item starts in the middle of a physical line (a later piece of an over-long line).
    mid: bool,
    /// Hash of the item's text (tests, diagnostics).
    hash: u64,
    layout: Option<Arc<SourceLayout>>,
}

/// One laid-out unit (a physical line or a piece of one).
#[derive(Debug)]
pub struct SourceLine {
    /// Item-relative byte offset of the unit's first byte.
    pub start: u32,
    /// Byte length of the unit's content (without the line terminator).
    pub len: u32,
    /// The unit starts a physical line (it shows a line number); `false` for later pieces.
    pub first: bool,
    /// Top of the line relative to the item's top.
    pub y: f32,
    pub height: f32,
    /// The shaped text (tabs expanded; its source map is unit-relative).
    pub text: Arc<dyn TextBlock>,
    /// Visual rows: byte range in the shaped text, top and height (relative to the line).
    pub rows: Vec<(Range<usize>, f32, f32)>,
}

impl SourceLine {
    fn map(&self) -> Option<&SourceMap> {
        self.text.source_map()
    }

    /// Unit-relative source offset → shaped byte.
    fn to_text(&self, rel: u32) -> usize {
        match self.map() {
            Some(m) if !m.is_empty() => m.to_text(rel).unwrap_or(0) as usize,
            _ => (rel as usize).min(self.text.text().len()),
        }
    }

    /// Shaped byte → unit-relative source offset.
    fn to_source(&self, t: usize) -> u32 {
        match self.map() {
            Some(m) if !m.is_empty() => m.to_source(t as u32).unwrap_or(0).min(self.len),
            _ => (t as u32).min(self.len),
        }
    }

    /// The visual row holding shaped byte `t` (`upstream`: at a soft-wrap boundary, the row
    /// that ends there).
    fn row_of(&self, t: usize, upstream: bool) -> usize {
        let n = self.rows.len();
        for (k, (r, _, _)) in self.rows.iter().enumerate() {
            if t < r.end {
                return k;
            }
            if t == r.end && (upstream || k + 1 == n || self.rows[k + 1].0.start > t) {
                return k;
            }
        }
        n.saturating_sub(1)
    }
}

/// The layout of one item.
#[derive(Debug)]
pub struct SourceLayout {
    pub lines: Vec<SourceLine>,
    pub height: f32,
    /// A code block's highlight was requested but is not ready: lay out again when it is.
    pending_code: bool,
}

/// An item to paint this frame.
#[derive(Clone, Debug)]
pub struct SourceVisible {
    pub idx: usize,
    /// Top of the item relative to the viewport top (may be negative).
    pub y: f32,
    pub layout: Arc<SourceLayout>,
    /// Absolute byte offset of the item's first byte.
    pub start: usize,
    /// Number (0-based) of the physical line the item's first unit belongs to.
    pub first_line: usize,
}

/// What layout reads besides the text: the document (tokens), and for code highlighting the
/// rendered view's model and highlighter.
pub struct SourceCtx<'a> {
    pub doc: &'a Arc<dyn FormatDoc>,
    pub model: Option<&'a DocModel>,
    pub highlights: &'a dyn Highlights,
}

/// Colours and font roles of the token classes in the active theme.
#[derive(Clone, Debug, PartialEq)]
struct Palette {
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
    fn new(t: &Theme) -> Self {
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
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = ahash::AHasher::default();
    b.hash(&mut h);
    h.finish()
}

/// Chunks `text[start..end]` into items (see the module docs). `mid`: `start` is inside a
/// physical line (after a piece cut).
fn chunk(text: &str, start: usize, end: usize, mut mid: bool) -> Vec<Item> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut pos = start;
    while pos < end {
        let item_start = pos;
        let item_mid = mid;
        let mut units = Vec::new();
        let mut newlines = 0u32;
        loop {
            // look for the line end only within one piece (+ CRLF): a longer line is cut
            // without scanning the rest of it (no quadratic scans of a huge line)
            let window = (pos + PIECE_BYTES + 2).min(end);
            let nl = memchr::memchr(b'\n', &b[pos..window]).map(|k| pos + k);
            let content_end = match nl {
                Some(n) if n > pos && b[n - 1] == b'\r' => n - 1,
                Some(n) => n,
                None if window < end => window, // longer than a piece
                None => end,
            };
            if content_end - pos > PIECE_BYTES {
                if !units.is_empty() {
                    break; // the over-long line starts its own item
                }
                let mut cut = pos + PIECE_BYTES;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                units.push((cut - pos) as u32);
                pos = cut;
                mid = true;
                break;
            }
            units.push((content_end - pos) as u32);
            match nl {
                Some(n) => {
                    pos = n + 1;
                    newlines += 1;
                    mid = false;
                }
                None => {
                    pos = end;
                    mid = end < text.len() && !text[..end].ends_with('\n');
                }
            }
            if units.len() >= ITEM_LINES || pos - item_start >= ITEM_BYTES || pos >= end {
                break;
            }
        }
        out.push(Item {
            len: pos - item_start,
            newlines,
            units: units.into_boxed_slice(),
            mid: item_mid,
            hash: hash_bytes(&b[item_start..pos]),
            layout: None,
        });
    }
    out
}

fn empty_item() -> Item {
    Item {
        len: 0,
        newlines: 0,
        units: Box::new([]),
        mid: false,
        hash: hash_bytes(b""),
        layout: None,
    }
}

/// See the module docs.
pub struct SourceView {
    items: Vec<Item>,
    bytes: Fenwick,
    newlines: Fenwick,
    heights: Fenwick,
    exact: Vec<bool>,
    params: SourceParams,
    theme: Theme,
    theme_fp: u64,
    palette: Palette,
    width: f32,
    ppp: f32,
    font_gen: u64,
    shaper_gen: u64,
    /// Bumped when every item must be re-tokenised (reference definitions changed).
    tok_epoch: u64,
    anchor: usize,
    anchor_off: f32,
    lines_cache: HashMap<u64, Arc<dyn TextBlock>>,
    laid: usize,
    /// Text length the view describes (consistency checks).
    text_len: usize,
    /// The text ends with a line break (or is empty): the last item shows an extra empty line.
    final_line: bool,
    /// Items laid out (cache misses) during the last frame.
    pub laid_out: Vec<usize>,
    /// Lines shaped (line-cache misses) during the last frame (tests, measurements).
    pub shaped: usize,
}

impl SourceView {
    pub fn new(text: &str, params: SourceParams, theme: &Theme, width: f32, ppp: f32) -> Self {
        let mut v = SourceView {
            items: Vec::new(),
            bytes: Fenwick::default(),
            newlines: Fenwick::default(),
            heights: Fenwick::default(),
            exact: Vec::new(),
            params,
            theme: theme.clone(),
            theme_fp: theme.fingerprint(),
            palette: Palette::new(theme),
            width: (width * 4.0).round() / 4.0,
            ppp,
            font_gen: 0,
            shaper_gen: 0,
            tok_epoch: 0,
            anchor: 0,
            anchor_off: 0.0,
            lines_cache: HashMap::new(),
            laid: 0,
            text_len: 0,
            final_line: true,
            laid_out: Vec::new(),
            shaped: 0,
        };
        v.rebuild(text);
        v
    }

    /// Rebuilds every item from `text` (document replaced). The scroll anchor stays on the same
    /// byte offset.
    pub fn rebuild(&mut self, text: &str) {
        let anchor_pos = if self.items.is_empty() {
            0
        } else {
            self.item_start(self.anchor)
        };
        let mut items = chunk(text, 0, text.len(), false);
        if items.is_empty() {
            items.push(empty_item());
        }
        self.items = items;
        self.text_len = text.len();
        self.final_line = text.is_empty() || text.ends_with('\n');
        self.bytes = Fenwick::from_values(self.items.iter().map(|it| it.len as f64).collect());
        self.newlines = Fenwick::from_values(self.items.iter().map(|it| it.newlines as f64).collect());
        self.laid = 0;
        self.reset_heights();
        self.anchor = self.item_at(anchor_pos.min(text.len()));
        self.anchor_off = 0.0;
    }

    // ------------------------------------------------------------------ structure

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text_len == 0
    }

    /// Bytes described (the text length).
    pub fn text_len(&self) -> usize {
        self.text_len
    }

    pub fn params(&self) -> SourceParams {
        self.params
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    /// Absolute byte offset of item `i`'s first byte.
    pub fn item_start(&self, i: usize) -> usize {
        self.bytes.prefix(i) as usize
    }

    fn item_end(&self, i: usize) -> usize {
        self.item_start(i) + self.items.get(i).map_or(0, |it| it.len)
    }

    /// Byte range of item `i`.
    pub fn item_range(&self, i: usize) -> Range<usize> {
        let s = self.item_start(i);
        s..s + self.items.get(i).map_or(0, |it| it.len)
    }

    /// The item holding byte offset `off` (the item starting there; the last item for the
    /// text end).
    pub fn item_at(&self, off: usize) -> usize {
        if self.items.len() <= 1 {
            return 0;
        }
        if off >= self.text_len {
            return self.items.len() - 1;
        }
        self.bytes.index_at(off as f64).min(self.items.len() - 1)
    }

    /// Number of physical lines (a final line break opens an empty last line).
    pub fn line_count(&self) -> usize {
        self.newlines.total() as usize + 1
    }

    /// The physical line (0-based) holding byte offset `off`.
    pub fn line_of(&self, text: &str, off: usize) -> usize {
        let off = off.min(text.len());
        let i = self.item_at(off);
        let s = self.item_start(i);
        let before = self.newlines.prefix(i) as usize;
        before + memchr::memchr_iter(b'\n', &text.as_bytes()[s..off.max(s)]).count()
    }

    /// Byte offset of the start of physical line `line` (clamped to the last line).
    pub fn line_start(&self, text: &str, line: usize) -> usize {
        if line == 0 || self.items.is_empty() {
            return 0;
        }
        let line = line.min(self.line_count() - 1);
        // the item holding the `line`-th line break
        let i = self.newlines.index_at(line as f64 - 0.5).min(self.items.len() - 1);
        let mut i = i;
        while i + 1 < self.items.len() && (self.newlines.prefix(i + 1) as usize) < line {
            i += 1;
        }
        let s = self.item_start(i);
        let need = line - self.newlines.prefix(i) as usize;
        let r = self.item_range(i);
        match memchr::memchr_iter(b'\n', &text.as_bytes()[r.clone()]).nth(need.saturating_sub(1)) {
            Some(k) => s + k + 1,
            None => r.end,
        }
    }

    /// Byte offset of the end of physical line `line`'s content (before its terminator).
    pub fn line_end(&self, text: &str, line: usize) -> usize {
        let s = self.line_start(text, line);
        let b = text.as_bytes();
        match memchr::memchr(b'\n', &b[s..]) {
            Some(k) if k > 0 && b[s + k - 1] == b'\r' => s + k - 1,
            Some(k) => s + k,
            None => text.len(),
        }
    }

    /// Number of the physical line item `i`'s first unit belongs to.
    pub fn first_line_of_item(&self, i: usize) -> usize {
        self.newlines.prefix(i) as usize
    }

    /// The physical lines as byte ranges of their content, reconstructed from the items
    /// (tests: compares incremental updates with a full rebuild).
    pub fn lines_debug(&self, text: &str) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        let mut pos = 0usize;
        for it in &self.items {
            let mut p = pos;
            for (k, &u) in it.units.iter().enumerate() {
                let r = p..p + u as usize;
                if k == 0 && it.mid {
                    if let Some(last) = out.last_mut() {
                        last.end = r.end;
                    }
                } else {
                    out.push(r.clone());
                }
                p = r.end;
                if text[p..].starts_with("\r\n") && p + 2 <= pos + it.len {
                    p += 2;
                } else if text[p..].starts_with('\n') && p < pos + it.len {
                    p += 1;
                }
            }
            pos += it.len;
        }
        if self.final_line {
            out.push(text.len()..text.len());
        }
        out
    }

    /// Checks the item invariants against `text` (tests): coverage, counts, limits, hashes.
    pub fn check(&self, text: &str) -> Result<(), String> {
        if self.items.iter().map(|i| i.len).sum::<usize>() != text.len() || self.text_len != text.len() {
            return Err("items do not cover the text".into());
        }
        let mut pos = 0;
        for (k, it) in self.items.iter().enumerate() {
            let t = &text[pos..pos + it.len];
            if it.hash != hash_bytes(t.as_bytes()) {
                return Err(format!("item {k}: stale hash"));
            }
            if it.newlines as usize != t.bytes().filter(|&b| b == b'\n').count() {
                return Err(format!("item {k}: newline count"));
            }
            if it.units.len() > ITEM_LINES || it.units.iter().any(|&u| u as usize > PIECE_BYTES) {
                return Err(format!("item {k}: over the limits"));
            }
            if it.len > ITEM_BYTES + PIECE_BYTES + 2 {
                return Err(format!("item {k}: {} bytes", it.len));
            }
            if (self.bytes.get(k) as usize) != it.len || self.newlines.get(k) as u32 != it.newlines {
                return Err(format!("item {k}: Fenwick out of date"));
            }
            if it.mid != (pos > 0 && !text[..pos].ends_with('\n')) {
                return Err(format!("item {k}: mid flag"));
            }
            pos += it.len;
        }
        if self.heights.len() != self.items.len() || self.exact.len() != self.items.len() {
            return Err("height list out of date".into());
        }
        Ok(())
    }

    // ------------------------------------------------------------------ edits

    /// Updates the items after an edit of the document (`text` is the text after it): the
    /// items touching the change are re-chunked, heights spliced, the anchor kept; items in
    /// the re-parsed range lose their tokens (re-laid out lazily); a document-wide dependency
    /// change re-tokenises everything lazily.
    pub fn apply(&mut self, text: &str, e: &EditSummary) {
        if self.items.is_empty() || e.changed.end > self.text_len {
            self.rebuild(text);
            return;
        }
        let delta = text.len() as isize - self.text_len as isize;
        if delta != e.delta {
            self.rebuild(text);
            return;
        }
        let changed = e.changed.clone();
        let mut i0 = self.item_at(changed.start);
        if i0 > 0 && changed.start == self.item_start(i0) {
            i0 -= 1;
        }
        let mut i1 = self.item_at(changed.end).max(i0);
        let start = self.item_start(i0);
        let mid = self.items[i0].mid;
        let shifted = |end: usize| (end as isize + delta) as usize;
        let mut new_end = shifted(self.item_end(i1));
        if i1 + 1 == self.items.len() {
            new_end = text.len();
        }
        let mut new_items = chunk(text, start, new_end.min(text.len()), mid);
        // a small last item merges with the next one (no fragmentation over many edits)
        if i1 + 1 < self.items.len()
            && new_items
                .last()
                .is_none_or(|l| l.units.len() < ITEM_LINES / 4 && l.len < ITEM_BYTES / 4)
        {
            i1 += 1;
            new_end = if i1 + 1 == self.items.len() {
                text.len()
            } else {
                shifted(self.item_end(i1))
            };
            new_items = chunk(text, start, new_end.min(text.len()), mid);
        }
        let replaced = i0..i1 + 1;
        let n_new = new_items.len();
        let shift = n_new as isize - replaced.len() as isize;
        let laid_removed = self.items[replaced.clone()]
            .iter()
            .filter(|it| it.layout.is_some())
            .count();
        self.laid = self.laid.saturating_sub(laid_removed);
        let bytes: Vec<f64> = new_items.iter().map(|it| it.len as f64).collect();
        let nls: Vec<f64> = new_items.iter().map(|it| it.newlines as f64).collect();
        self.items.splice(replaced.clone(), new_items);
        self.text_len = text.len();
        self.final_line = text.is_empty() || text.ends_with('\n');
        if self.items.is_empty() {
            self.items.push(empty_item());
            self.rebuild(text);
            return;
        }
        self.bytes.splice(replaced.clone(), bytes);
        self.newlines.splice(replaced.clone(), nls);
        let est: Vec<f64> = (i0..i0 + n_new).map(|i| self.estimate(i) as f64).collect();
        self.heights.splice(replaced.clone(), est);
        self.exact.splice(replaced.clone(), std::iter::repeat_n(false, n_new));
        // the last item may show (or stop showing) the extra empty line
        let last = self.items.len() - 1;
        if !(i0..i0 + n_new).contains(&last) {
            self.forget(last);
        }
        // anchor
        if self.anchor > i1 {
            self.anchor = (self.anchor as isize + shift).max(0) as usize;
        } else if self.anchor >= i0 {
            self.anchor = i0;
            self.anchor_off = self.anchor_off.min(self.height(i0));
        }
        self.anchor = self.anchor.min(self.items.len() - 1);
        // tokens: the re-parsed range (the parse around the edit may have changed), and every
        // item when a document-wide dependency changed
        if e.deps.all || !e.deps.labels.is_empty() || e.deps.resources {
            self.tok_epoch += 1;
            self.drop_layouts();
        } else if !e.reparsed.is_empty() {
            let a = self.item_at(e.reparsed.start);
            let b = self.item_at(e.reparsed.end);
            for i in a..=b.min(self.items.len() - 1) {
                self.forget(i);
            }
        }
    }

    /// Drops item `i`'s layout (re-tokenised and re-laid out when next needed).
    fn forget(&mut self, i: usize) {
        if let Some(it) = self.items.get_mut(i)
            && it.layout.take().is_some()
        {
            self.laid = self.laid.saturating_sub(1);
            self.exact[i] = false;
        }
    }

    fn drop_layouts(&mut self) {
        for (i, it) in self.items.iter_mut().enumerate() {
            if it.layout.take().is_some() {
                self.exact[i] = false;
            }
        }
        self.laid = 0;
    }

    // ------------------------------------------------------------------ parameters, heights

    /// Updates the parameters; a change re-estimates the heights and drops the layouts (the
    /// scroll position stays on the same item).
    pub fn set_params(&mut self, params: SourceParams, width: f32, theme: &Theme, ppp: f32, font_gen: u64) {
        let width = (width * 4.0).round() / 4.0;
        let fp = theme.fingerprint();
        if params == self.params
            && width == self.width
            && fp == self.theme_fp
            && ppp == self.ppp
            && font_gen == self.font_gen
        {
            return;
        }
        let frac = self.anchor_frac();
        let layout_only = params == self.params && width == self.width && ppp == self.ppp && font_gen == self.font_gen;
        self.params = params;
        self.width = width;
        if fp != self.theme_fp {
            self.theme = theme.clone();
            self.theme_fp = fp;
            self.palette = Palette::new(theme);
        }
        self.ppp = ppp;
        self.font_gen = font_gen;
        self.drop_layouts();
        if !layout_only {
            self.lines_cache.clear();
        }
        self.reset_heights();
        self.anchor_off = frac * self.height(self.anchor);
    }

    fn reset_heights(&mut self) {
        let values: Vec<f64> = (0..self.items.len()).map(|i| self.estimate(i) as f64).collect();
        self.heights = Fenwick::from_values(values);
        self.exact = vec![false; self.items.len()];
        for (i, it) in self.items.iter().enumerate() {
            if let Some(l) = &it.layout {
                let (a, b) = self.pad(i);
                self.heights.set(i, (l.height + a + b) as f64);
                self.exact[i] = true;
            }
        }
    }

    fn pad(&self, i: usize) -> (f32, f32) {
        let n = self.items.len();
        (
            if i == 0 { PAD_TOP } else { 0.0 },
            if i + 1 == n { PAD_BOTTOM } else { 0.0 },
        )
    }

    /// Height estimate of item `i` before layout (wrapped rows from the unit lengths).
    fn estimate(&self, i: usize) -> f32 {
        let Some(it) = self.items.get(i) else { return 0.0 };
        let (size, line_h) = metrics(&self.theme);
        let cols = if self.params.wrap {
            (self.width / (size * 0.6)).max(10.0) as usize
        } else {
            usize::MAX
        };
        let mut rows: usize = it.units.iter().map(|&u| (u as usize).div_ceil(cols).max(1)).sum();
        if i + 1 == self.items.len() && self.final_line {
            rows += 1;
        }
        let (a, b) = self.pad(i);
        rows as f32 * line_h + a + b
    }

    /// Current height of item `i` (exact once laid out).
    pub fn height(&self, i: usize) -> f32 {
        if i < self.heights.len() {
            self.heights.get(i) as f32
        } else {
            0.0
        }
    }

    pub fn is_exact(&self, i: usize) -> bool {
        self.exact.get(i).copied().unwrap_or(false)
    }

    /// Absolute y of the top of item `i`'s first line.
    pub fn item_y(&self, i: usize) -> f64 {
        let i = i.min(self.heights.len());
        self.heights.prefix(i) + self.pad(i).0 as f64
    }

    pub fn total_height(&self) -> f64 {
        self.heights.total()
    }

    // ------------------------------------------------------------------ line ↔ y

    /// Content y of the (fractional) physical line `line` (`2.5` = halfway down line 2): exact
    /// for laid-out items, interpolated over the item's height otherwise.
    pub fn y_of_line(&self, text: &str, line: f32) -> f64 {
        let line = line.max(0.0);
        let l = (line.floor() as usize).min(self.line_count() - 1);
        let frac = (line - l as f32).clamp(0.0, 1.0);
        let off = self.line_start(text, l);
        let i = self.item_at(off);
        let base = self.item_y(i);
        let rel = (off - self.item_start(i)) as u32;
        if let Some(lay) = &self.items[i].layout {
            if let Some(ln) = lay.lines.iter().rev().find(|ln| ln.start <= rel) {
                return base + (ln.y + frac * ln.height) as f64;
            }
            return base;
        }
        let n = self.items[i].units.len() + usize::from(i + 1 == self.items.len() && self.final_line);
        let k = l - self.first_line_of_item(i);
        let (a, b) = self.pad(i);
        let h = (self.height(i) - a - b).max(0.0) as f64;
        base + h * (k as f64 + frac as f64) / n.max(1) as f64
    }

    /// The (fractional) physical line at content y (inverse of [`SourceView::y_of_line`]).
    pub fn line_at_y(&self, y: f64) -> f32 {
        if self.items.is_empty() {
            return 0.0;
        }
        let y = y.clamp(0.0, self.total_height());
        let i = self.heights.index_at(y).min(self.items.len() - 1);
        let first = self.first_line_of_item(i) as f32;
        let local = (y - self.item_y(i)).max(0.0) as f32;
        if let Some(lay) = &self.items[i].layout {
            let k = lay.lines.iter().rposition(|ln| ln.y <= local).unwrap_or(0);
            let Some(ln) = lay.lines.get(k) else { return first };
            let line = first + lay.lines[1..=k].iter().filter(|l| l.first).count() as f32;
            let frac = ((local - ln.y) / ln.height.max(1.0)).clamp(0.0, 0.999);
            return line + frac;
        }
        let n = self.items[i].units.len() + usize::from(i + 1 == self.items.len() && self.final_line);
        let (a, b) = self.pad(i);
        let h = (self.height(i) - a - b).max(1.0);
        let units = local / h * n.max(1) as f32;
        let lines_in_item = self.items[i].newlines as f32 + usize::from(i + 1 == self.items.len()) as f32;
        first + units.min(lines_in_item.max(1.0) - 0.001).max(0.0)
    }

    // ------------------------------------------------------------------ scrolling

    fn anchor_frac(&self) -> f32 {
        let h = self.height(self.anchor);
        if h > 0.0 {
            (self.anchor_off / h).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn anchor(&self) -> (usize, f32) {
        (self.anchor, self.anchor_off)
    }

    /// Absolute y of the viewport top.
    pub fn scroll_y(&self) -> f64 {
        self.heights.prefix(self.anchor) + self.anchor_off as f64
    }

    pub fn scroll_to_y(&mut self, y: f64) {
        let y = y.clamp(0.0, self.total_height());
        self.anchor = self.heights.index_at(y).min(self.items.len().saturating_sub(1));
        self.anchor_off = (y - self.heights.prefix(self.anchor)).max(0.0) as f32;
    }

    pub fn scroll_by(&mut self, dy: f32) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        self.anchor_off += dy;
        while self.anchor_off < 0.0 && self.anchor > 0 {
            self.anchor -= 1;
            self.anchor_off += self.height(self.anchor);
        }
        if self.anchor_off < 0.0 {
            self.anchor_off = 0.0;
        }
        while self.anchor + 1 < n && self.anchor_off >= self.height(self.anchor) {
            self.anchor_off -= self.height(self.anchor);
            self.anchor += 1;
        }
    }

    // ------------------------------------------------------------------ layout

    fn line_style(&self, tok: SourceTok, code: Option<TokenKind>) -> TextStyle {
        let p = &self.palette;
        let (size, line_h) = metrics(&self.theme);
        let mut s = TextStyle::new(size, line_h, FontRole::Mono, p.text);
        match tok {
            SourceTok::Plain => {}
            SourceTok::Heading1
            | SourceTok::Heading2
            | SourceTok::Heading3
            | SourceTok::Heading4
            | SourceTok::Heading5
            | SourceTok::Heading6 => {
                let l = tok.heading_level().unwrap_or(1) as usize;
                s.color = p.heading[l - 1];
                s.role = FontRole::MonoBold;
            }
            SourceTok::Quote => s.color = p.quote,
            SourceTok::Emph => s.italic = true,
            SourceTok::Strong => s.role = FontRole::MonoBold,
            SourceTok::StrongEmph => {
                s.role = FontRole::MonoBold;
                s.italic = true;
            }
            SourceTok::Strike => s.strike = true,
            SourceTok::Mark => s.background = p.mark_bg,
            SourceTok::Link => {
                s.color = p.link;
                s.underline = true;
            }
            SourceTok::Url => s.color = p.url,
            SourceTok::Code => {
                s.color = p.code;
                s.background = p.code_bg;
            }
            SourceTok::CodeBlock => s.color = p.code_block,
            SourceTok::Meta => s.color = p.meta,
            SourceTok::Marker => s.color = p.marker,
        }
        if let Some(k) = code {
            let look = self.theme.token_look(k);
            if let Some(c) = look.color {
                s.color = color(c);
            }
            if look.bold {
                s.role = FontRole::MonoBold;
            }
            s.italic |= look.italic;
        }
        s
    }

    /// Shapes one unit: `line` = its source text at absolute offset `abs`.
    #[allow(clippy::too_many_arguments)]
    fn shape_line(
        &mut self,
        line: &str,
        abs: usize,
        toks: &[SourceToken],
        code: &[(Range<usize>, TokenKind)],
        shaper: &mut dyn TextShaper,
    ) -> Arc<dyn TextBlock> {
        let end = abs + line.len();
        let t0 = toks.partition_point(|t| t.range.end <= abs);
        let mut t1 = t0;
        while t1 < toks.len() && toks[t1].range.start < end {
            t1 += 1;
        }
        let c0 = code.partition_point(|c| c.0.end <= abs);
        let mut c1 = c0;
        while c1 < code.len() && code[c1].0.start < end {
            c1 += 1;
        }
        let wrap = self.params.wrap.then_some(self.width.max(40.0));
        let mut h = ahash::AHasher::default();
        line.hash(&mut h);
        for t in &toks[t0..t1] {
            (t.range.start.max(abs) - abs, t.range.end.min(end) - abs, t.tok as u8).hash(&mut h);
        }
        for c in &code[c0..c1] {
            (c.0.start.max(abs) - abs, c.0.end.min(end) - abs, c.1 as u8).hash(&mut h);
        }
        (
            wrap.map(f32::to_bits),
            self.params.tab_width,
            self.theme_fp,
            self.ppp.to_bits(),
            self.font_gen,
            shaper.generation(),
        )
            .hash(&mut h);
        let key = h.finish();
        if let Some(b) = self.lines_cache.get(&key) {
            return b.clone();
        }
        // per-byte classes of the unit, then runs of equal style split at tabs
        let n = line.len();
        let mut cls = vec![(SourceTok::Plain, None::<TokenKind>); n];
        for t in &toks[t0..t1] {
            for c in &mut cls[t.range.start.max(abs) - abs..t.range.end.min(end) - abs] {
                c.0 = t.tok;
            }
        }
        for r in &code[c0..c1] {
            for c in &mut cls[r.0.start.max(abs) - abs..r.0.end.min(end) - abs] {
                c.1 = Some(r.1);
            }
        }
        let mut st = StyledText::default();
        let mut map = SourceMap::default();
        let tab = self.params.tab_width.max(1) as usize;
        let mut col = 0usize;
        let mut i = 0usize;
        while i < n {
            let mut j = i + 1;
            while j < n && cls[j] == cls[i] && line.as_bytes()[j] != b'\t' && line.as_bytes()[i] != b'\t' {
                j += 1;
            }
            while j < n && !line.is_char_boundary(j) {
                j += 1;
            }
            let style = self.line_style(cls[i].0, cls[i].1);
            let piece = &line[i..j];
            let t = st.text.len() as u32;
            if piece == "\t" {
                let k = tab - col % tab;
                st.append(&" ".repeat(k), 0.0, style);
                map.push(t..t + k as u32, i as u32..j as u32, PieceKind::Atom);
                col += k;
            } else {
                st.append(piece, 0.0, style);
                map.push(t..t + piece.len() as u32, i as u32..j as u32, PieceKind::Visible);
                col += piece.chars().count();
            }
            i = j;
        }
        if st.runs.is_empty() {
            st = StyledText::single("", self.line_style(SourceTok::Plain, None));
        }
        st.break_on_newline = false;
        st.source_map = Some(Arc::new(map));
        let block = shaper.shape(&st, wrap);
        self.shaped += 1;
        if self.lines_cache.len() >= MAX_CACHED_LINES {
            self.lines_cache.clear();
        }
        self.lines_cache.insert(key, block.clone());
        block
    }

    /// Code-highlight runs (absolute source ranges) of code blocks overlapping `r`, from the
    /// rendered view's code items; `true` when some highlight is still pending.
    fn code_runs(ctx: &SourceCtx<'_>, text: &str, r: Range<usize>) -> (Vec<(Range<usize>, TokenKind)>, bool) {
        let mut out = Vec::new();
        let mut pending = false;
        let Some(model) = ctx.model else { return (out, false) };
        let n = model.items.len();
        if n == 0 {
            return (out, false);
        }
        let mut i = model.item_at_offset(r.start);
        // a code block segmented into several items: start from the block's first segment that
        // can overlap (item_at_offset picks the segment containing `r.start`)
        while i < n {
            let start = model.item_start(i);
            if start >= r.end {
                break;
            }
            let it = &model.items[i];
            if let (ItemKind::Code { lang }, Some(code)) = (&it.kind, it.code) {
                let units: Vec<markise_doc::Span> = match &it.segment {
                    Some(Segment::Lines { spans, .. }) => spans
                        .iter()
                        .map(|s| markise_doc::Span::new(start + s.start, start + s.end))
                        .collect(),
                    _ => ctx.doc.leaf(start).map(|l| l.lines).unwrap_or_default(),
                };
                let overlaps =
                    units.first().is_some_and(|f| f.start < r.end) && units.last().is_some_and(|l| l.end >= r.start);
                if overlaps && !lang.is_empty() {
                    let range = item_range(it, &code);
                    let q = CodeQuery {
                        code,
                        lang,
                        range,
                        doc: ctx.doc,
                        start,
                        item: i,
                    };
                    if ctx.highlights.ready(&q) {
                        if let Some(runs) = ctx.highlights.runs(&q) {
                            // content offsets → source: walk the item's units
                            let mut cpos = code.offset;
                            let mut spans = Vec::with_capacity(units.len());
                            for (k, u) in units.iter().enumerate() {
                                spans.push((cpos, u.start, u.len()));
                                cpos += u.len();
                                if units.get(k + 1).is_none_or(|nx| nx.start != u.end) {
                                    cpos += 1;
                                }
                            }
                            for run in runs {
                                let (rs, re) = (run.start as usize, run.end as usize);
                                let k0 = spans.partition_point(|s| s.0 + s.2 <= rs);
                                for s in &spans[k0..] {
                                    if s.0 >= re {
                                        break;
                                    }
                                    let a = rs.max(s.0) - s.0;
                                    let b = re.min(s.0 + s.2) - s.0;
                                    if a < b && s.1 + b <= text.len() {
                                        let abs = s.1 + a..s.1 + b;
                                        if abs.end > r.start && abs.start < r.end {
                                            out.push((abs, run.kind));
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        ctx.highlights.want(&q);
                        pending = true;
                    }
                }
            }
            i += 1;
        }
        out.sort_by_key(|c| c.0.start);
        (out, pending)
    }

    /// The layout of item `i` (cached on the item, or computed now).
    pub fn layout(&mut self, i: usize, ctx: &SourceCtx<'_>, shaper: &mut dyn TextShaper) -> Arc<SourceLayout> {
        let text = ctx.doc.text();
        if let Some(l) = &self.items[i].layout {
            return l.clone();
        }
        let r = self.item_range(i);
        let mut toks = Vec::new();
        ctx.doc.source_tokens(r.clone(), &mut toks);
        let (code, pending_code) = Self::code_runs(ctx, text, r.clone());
        let (_, line_h) = metrics(&self.theme);
        let units = self.items[i].units.clone();
        let mid = self.items[i].mid;
        let mut lines = Vec::with_capacity(units.len() + 1);
        let mut pos = r.start;
        let mut y = 0.0f32;
        for (k, &u) in units.iter().enumerate() {
            let s = pos;
            let e = (pos + u as usize).min(text.len());
            let block = self.shape_line(&text[s..e], s, &toks, &code, shaper);
            let rows: Vec<(Range<usize>, f32, f32)> = block
                .lines()
                .into_iter()
                .map(|l| (l.range, l.origin.y, l.size.y.max(1.0)))
                .collect();
            let h = block.size().y.max(line_h);
            lines.push(SourceLine {
                start: (s - r.start) as u32,
                len: u,
                first: !(k == 0 && mid),
                y,
                height: h,
                text: block,
                rows,
            });
            y += h;
            pos = e;
            if text[pos..].starts_with("\r\n") && pos + 2 <= r.end {
                pos += 2;
            } else if text[pos..].starts_with('\n') && pos < r.end {
                pos += 1;
            }
        }
        if i + 1 == self.items.len() && self.final_line {
            let block = self.shape_line("", text.len(), &[], &[], shaper);
            let rows = vec![(0..0, 0.0, line_h)];
            lines.push(SourceLine {
                start: (text.len() - r.start) as u32,
                len: 0,
                first: true,
                y,
                height: line_h,
                text: block,
                rows,
            });
            y += line_h;
        }
        let l = Arc::new(SourceLayout {
            lines,
            height: y,
            pending_code,
        });
        self.items[i].layout = Some(l.clone());
        self.laid += 1;
        self.laid_out.push(i);
        let (a, b) = self.pad(i);
        self.heights.set(i, (y + a + b) as f64);
        self.exact[i] = true;
        l
    }

    /// Lays out what a viewport `vh` tall needs plus `margin` above and below and returns the
    /// items intersecting it, top to bottom.
    pub fn frame(
        &mut self,
        vh: f32,
        margin: f32,
        ctx: &SourceCtx<'_>,
        shaper: &mut dyn TextShaper,
    ) -> Vec<SourceVisible> {
        self.laid_out.clear();
        self.shaped = 0;
        let sg = shaper.generation();
        if sg != self.shaper_gen {
            self.shaper_gen = sg;
            self.drop_layouts();
            self.lines_cache.clear();
        }
        let n = self.items.len();
        // items waiting for a code highlight: laid out again once it is ready (checked once
        // per frame, for the items around the viewport)
        let mut i = self.anchor.saturating_sub(4);
        let mut y = -(self.heights.prefix(self.anchor) - self.heights.prefix(i)) as f32;
        while i < n && y < vh + margin {
            if self.items[i].layout.as_ref().is_some_and(|l| l.pending_code)
                && !Self::code_runs(ctx, ctx.doc.text(), self.item_range(i)).1
            {
                self.forget(i);
            }
            y += self.height(i);
            i += 1;
        }
        let mut out = Vec::new();
        for _attempt in 0..4 {
            out.clear();
            loop {
                self.layout(self.anchor, ctx, shaper);
                let h = self.height(self.anchor);
                if self.anchor_off >= h && self.anchor + 1 < n {
                    self.anchor_off -= h;
                    self.anchor += 1;
                } else {
                    break;
                }
            }
            let mut y = -self.anchor_off;
            let mut i = self.anchor;
            while i < n && y < vh + margin {
                let l = self.layout(i, ctx, shaper);
                let h = self.height(i);
                if y + h > 0.0 && y < vh {
                    out.push(SourceVisible {
                        idx: i,
                        y: y + self.pad(i).0,
                        layout: l,
                        start: self.item_start(i),
                        first_line: self.first_line_of_item(i),
                    });
                }
                y += h;
                i += 1;
            }
            // do not scroll past the end
            if i >= n && y < vh && (self.anchor > 0 || self.anchor_off > 0.0) {
                self.scroll_by(y - vh);
                continue;
            }
            break;
        }
        let mut y = -self.anchor_off;
        let mut i = self.anchor;
        while i > 0 && y > -margin {
            i -= 1;
            self.layout(i, ctx, shaper);
            y -= self.height(i);
        }
        if self.laid > MAX_LAID_ITEMS {
            let lo = self.anchor.saturating_sub(MAX_LAID_ITEMS / 4);
            let hi = self.anchor + MAX_LAID_ITEMS / 2;
            for k in 0..n {
                if (k < lo || k > hi) && self.items[k].layout.is_some() {
                    self.items[k].layout = None;
                    // the height stays (exact) until the item changes: no scroll jumps
                }
            }
            self.laid = self.items.iter().filter(|it| it.layout.is_some()).count();
        }
        out
    }

    // ------------------------------------------------------------------ geometry

    /// Item, line index and shaped position of `pos` (lays the item out).
    fn locate(
        &mut self,
        pos: DocPos,
        ctx: &SourceCtx<'_>,
        shaper: &mut dyn TextShaper,
    ) -> (usize, Arc<SourceLayout>, usize, LocalPos) {
        let off = pos.offset.min(self.text_len);
        let mut i = self.item_at(off);
        let up = pos.affinity == DocAffinity::Upstream;
        if up && i > 0 && off == self.item_start(i) && self.items[i].mid {
            i -= 1; // the end of the previous piece
        }
        let l = self.layout(i, ctx, shaper);
        let rel = (off - self.item_start(i)) as u32;
        let k = l.lines.iter().rposition(|ln| ln.start <= rel).unwrap_or(0);
        let ln = &l.lines[k];
        let t = ln.to_text(rel.saturating_sub(ln.start).min(ln.len));
        let aff = if up { Affinity::Upstream } else { Affinity::Downstream };
        (i, l.clone(), k, LocalPos::new(t, aff))
    }

    fn to_doc(&self, i: usize, ln: &SourceLine, lp: LocalPos) -> DocPos {
        let off = self.item_start(i) + (ln.start + ln.to_source(lp.byte)) as usize;
        DocPos {
            offset: off,
            affinity: match lp.affinity {
                Affinity::Upstream => DocAffinity::Upstream,
                Affinity::Downstream => DocAffinity::Downstream,
            },
            virt: 0,
        }
    }

    /// Caret rectangle of `pos` in content coordinates (x from the text's left edge, y from the
    /// document top).
    pub fn caret_rect(&mut self, pos: DocPos, ctx: &SourceCtx<'_>, shaper: &mut dyn TextShaper) -> Rect {
        let (i, l, k, lp) = self.locate(pos, ctx, shaper);
        let ln = &l.lines[k];
        let r = ln.text.caret_rect(lp);
        let (_, line_h) = metrics(&self.theme);
        let h = r.height().max(line_h * 0.8);
        let top = self.item_y(i) as f32 + ln.y + r.min.y;
        Rect::from_min_max(pos2(r.min.x, top), pos2(r.min.x, top + h))
    }

    /// Rectangle of the physical line (all its rows) holding `pos`, content coordinates
    /// (current-line band).
    pub fn line_rect(&mut self, pos: DocPos, ctx: &SourceCtx<'_>, shaper: &mut dyn TextShaper) -> Rect {
        let (i, l, k, _) = self.locate(pos, ctx, shaper);
        let ln = &l.lines[k];
        let top = self.item_y(i) as f32 + ln.y;
        Rect::from_min_max(pos2(0.0, top), pos2(self.width, top + ln.height))
    }

    /// The position nearest to content point `p` (x from the text's left edge, y from the
    /// document top).
    pub fn hit(&mut self, p: Pos2, ctx: &SourceCtx<'_>, shaper: &mut dyn TextShaper) -> DocPos {
        if self.items.is_empty() {
            return DocPos::new(0);
        }
        let y = (p.y as f64).clamp(0.0, self.total_height());
        let mut i = self.heights.index_at(y).min(self.items.len() - 1);
        let mut l = self.layout(i, ctx, shaper);
        // the estimate may have been off: the point can be below the laid-out item now
        let mut ly = p.y - self.item_y(i) as f32;
        while ly >= l.height && i + 1 < self.items.len() && p.y as f64 >= self.item_y(i + 1) {
            i += 1;
            l = self.layout(i, ctx, shaper);
            ly = p.y - self.item_y(i) as f32;
        }
        let k = l
            .lines
            .iter()
            .rposition(|ln| ln.y <= ly)
            .unwrap_or(0)
            .min(l.lines.len().saturating_sub(1));
        let Some(ln) = l.lines.get(k) else {
            return DocPos::new(self.item_start(i));
        };
        let local = pos2(p.x, (ly - ln.y).clamp(0.0, (ln.height - 0.5).max(0.0)));
        let lp = ln.text.hit(local);
        self.to_doc(i, ln, lp)
    }

    /// Selection rectangles of `range` within the visible items (viewport-relative y as in
    /// [`SourceVisible::y`]; x from the text's left edge). A selected line break shows as a
    /// narrow box after the line.
    pub fn selection_rects(&self, visible: &[SourceVisible], range: Range<usize>) -> Vec<Rect> {
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        let (size, _) = metrics(&self.theme);
        for v in visible {
            for ln in &v.layout.lines {
                let s = v.start + ln.start as usize;
                let e = s + ln.len as usize;
                if e < range.start || s > range.end {
                    continue;
                }
                let a = ln.to_text((range.start.max(s) - s) as u32);
                let b = ln.to_text((range.end.min(e) - s) as u32);
                let top = v.y + ln.y;
                for r in ln.text.selection_rects(LocalPos::down(a), LocalPos::down(b)) {
                    out.push(r.translate(epaint::vec2(0.0, top)));
                }
                // the line break itself is selected
                if range.end > e && range.start <= e && (ln.first || ln.len > 0) {
                    let last = ln.rows.last().map_or(0.0, |r| r.1);
                    let x = ln
                        .text
                        .caret_rect(LocalPos::new(ln.text.text().len(), Affinity::Upstream))
                        .min
                        .x;
                    let h = ln.rows.last().map_or(ln.height, |r| r.2);
                    out.push(Rect::from_min_size(pos2(x, top + last), epaint::vec2(size * 0.5, h)));
                }
            }
        }
        out
    }

    /// One visual row up or down from `pos` at `goal_x` (the caret's x when `None`); returns the
    /// new position and the goal x to keep. At the first / last row: the document start / end.
    pub fn vertical(
        &mut self,
        pos: DocPos,
        goal_x: Option<f32>,
        down: bool,
        ctx: &SourceCtx<'_>,
        shaper: &mut dyn TextShaper,
    ) -> (DocPos, f32) {
        let (i, l, k, lp) = self.locate(pos, ctx, shaper);
        let ln = &l.lines[k];
        let x = goal_x.unwrap_or_else(|| ln.text.caret_rect(lp).min.x);
        let row = ln.row_of(lp.byte, lp.affinity == Affinity::Upstream);
        // the target row: (item, line, row)
        let target = if down {
            if row + 1 < ln.rows.len() {
                Some((i, l.clone(), k, row + 1))
            } else if k + 1 < l.lines.len() {
                Some((i, l.clone(), k + 1, 0))
            } else if i + 1 < self.items.len() {
                let nl = self.layout(i + 1, ctx, shaper);
                (!nl.lines.is_empty()).then(|| (i + 1, nl, 0, 0))
            } else {
                None
            }
        } else if row > 0 {
            Some((i, l.clone(), k, row - 1))
        } else if k > 0 {
            let r = l.lines[k - 1].rows.len().saturating_sub(1);
            Some((i, l.clone(), k - 1, r))
        } else if i > 0 {
            let pl = self.layout(i - 1, ctx, shaper);
            pl.lines.last().map(|last| {
                let r = last.rows.len().saturating_sub(1);
                let kk = pl.lines.len() - 1;
                (i - 1, pl.clone(), kk, r)
            })
        } else {
            None
        };
        let Some((ti, tl, tk, tr)) = target else {
            let edge = if down { self.text_len } else { 0 };
            return (DocPos::new(edge), x);
        };
        let tln = &tl.lines[tk];
        let ry = tln.rows.get(tr).map_or(0.0, |r| r.1 + r.2 * 0.5);
        let lp = tln.text.hit(pos2(x, ry));
        (self.to_doc(ti, tln, lp), x)
    }

    /// Start or end of the visual row holding `pos` (a soft-wrapped row ends upstream, before
    /// the first character of the next row).
    pub fn row_edge(&mut self, pos: DocPos, end: bool, ctx: &SourceCtx<'_>, shaper: &mut dyn TextShaper) -> DocPos {
        let (i, l, k, lp) = self.locate(pos, ctx, shaper);
        let ln = &l.lines[k];
        let row = ln.row_of(lp.byte, lp.affinity == Affinity::Upstream);
        let Some((r, _, _)) = ln.rows.get(row) else {
            return pos;
        };
        let lp = if end {
            let last = row + 1 == ln.rows.len();
            // a wrapped row's trailing spaces belong to it: stop before the next row's text
            let b = if last {
                ln.text.text().len()
            } else {
                ln.rows[row + 1].0.start
            };
            LocalPos::new(b, if last { Affinity::Downstream } else { Affinity::Upstream })
        } else {
            LocalPos::down(r.start)
        };
        self.to_doc(i, ln, lp)
    }

    /// One page (`page` points) up or down at `goal_x`.
    #[allow(clippy::too_many_arguments)]
    pub fn page(
        &mut self,
        pos: DocPos,
        goal_x: Option<f32>,
        down: bool,
        page: f32,
        ctx: &SourceCtx<'_>,
        shaper: &mut dyn TextShaper,
    ) -> (DocPos, f32) {
        let r = self.caret_rect(pos, ctx, shaper);
        let x = goal_x.unwrap_or(r.min.x);
        let y = if down { r.center().y + page } else { r.center().y - page };
        if y < 0.0 {
            return (DocPos::new(0), x);
        }
        if y as f64 > self.total_height() {
            return (DocPos::new(self.text_len), x);
        }
        (self.hit(pos2(x, y), ctx, shaper), x)
    }
}

// ---------------------------------------------------------------------- text-based moves

/// The previous grapheme boundary before `at` (a CRLF pair is one grapheme).
pub fn prev_grapheme(t: &str, at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    let from = t[..at].char_indices().rev().nth(32).map_or(0, |(i, _)| i);
    t[from..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(from, |(i, _)| from + i)
}

/// The next grapheme boundary after `at`.
pub fn next_grapheme(t: &str, at: usize) -> usize {
    if at >= t.len() {
        return t.len();
    }
    let to = t[at..].char_indices().nth(32).map_or(t.len(), |(i, _)| at + i);
    t[at..to].graphemes(true).next().map_or(to, |g| at + g.len())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Word move (Alt/Ctrl + arrow): forward to the end of the next word, backward to the start of
/// the previous one, within the line; at a line edge one step crosses the line break.
pub fn word_move(t: &str, at: usize, forward: bool) -> usize {
    let at = at.min(t.len());
    if forward {
        let rest = &t[at..];
        if rest.starts_with("\r\n") {
            return at + 2;
        }
        if rest.starts_with('\n') {
            return at + 1;
        }
        let mut it = rest.char_indices().peekable();
        // skip separators (not crossing the line end), then the word
        let mut end = at;
        let mut seen_word = false;
        while let Some(&(i, c)) = it.peek() {
            if c == '\n' || c == '\r' {
                return at + i;
            }
            if is_word(c) {
                seen_word = true;
                end = at + i + c.len_utf8();
                it.next();
            } else if seen_word {
                break;
            } else {
                end = at + i + c.len_utf8();
                it.next();
            }
        }
        if seen_word { end } else { end.max(at) }
    } else {
        let before = &t[..at];
        if before.ends_with("\r\n") {
            return at - 2;
        }
        if before.ends_with('\n') {
            return at - 1;
        }
        let mut start = at;
        let mut seen_word = false;
        for (i, c) in before.char_indices().rev() {
            if c == '\n' {
                return i + 1;
            }
            if is_word(c) {
                seen_word = true;
                start = i;
            } else if seen_word {
                break;
            } else {
                start = i;
            }
        }
        if seen_word { start } else { start.min(at) }
    }
}

/// The word around `at` (double click): a run of word characters, else of other non-space
/// characters, else of spaces; never crosses a line break.
pub fn word_at(t: &str, at: usize) -> Range<usize> {
    let at = at.min(t.len());
    let line_s = t[..at].rfind('\n').map_or(0, |k| k + 1);
    let line_e = t[at..].find(['\r', '\n']).map_or(t.len(), |k| at + k);
    let class = |c: char| {
        if is_word(c) {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let c = t[at..line_e]
        .chars()
        .next()
        .or_else(|| t[line_s..at].chars().next_back());
    let Some(c) = c else { return at..at };
    let k = class(c);
    let mut s = at;
    for (i, ch) in t[line_s..at].char_indices().rev() {
        if class(ch) != k {
            break;
        }
        s = line_s + i;
    }
    let mut e = at;
    for (i, ch) in t[at..line_e].char_indices() {
        if class(ch) != k {
            break;
        }
        e = at + i + ch.len_utf8();
    }
    s..e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::NoHighlights;

    fn doc(src: &str) -> Arc<dyn FormatDoc> {
        Arc::new(markise_md::parse(src))
    }

    #[test]
    fn chunks_cover_the_text_and_count_lines() {
        let mut src = String::new();
        for i in 0..1000 {
            src.push_str(&format!("line {i}\r\n"));
        }
        src.push_str(&"x".repeat(10_000));
        src.push('\n');
        let v = SourceView::new(&src, SourceParams::default(), &Theme::light(), 600.0, 1.0);
        v.check(&src).unwrap();
        assert_eq!(v.line_count(), 1002);
        let lines = v.lines_debug(&src);
        assert_eq!(lines.len(), 1002);
        assert_eq!(&src[lines[3].clone()], "line 3");
        assert_eq!(lines[1000].len(), 10_000, "pieces join into one physical line");
        assert_eq!(v.line_of(&src, 0), 0);
        let l5 = v.line_start(&src, 5);
        assert_eq!(&src[l5..l5 + 6], "line 5");
        assert_eq!(v.line_of(&src, l5 + 2), 5);
        assert_eq!(v.line_end(&src, 5), l5 + 6, "CRLF terminator excluded");
        assert_eq!(v.line_start(&src, 1001), src.len());
        let e = SourceView::new("", SourceParams::default(), &Theme::light(), 600.0, 1.0);
        e.check("").unwrap();
        assert_eq!(e.line_count(), 1);
    }

    fn apply(v: &mut SourceView, d: &mut markise_md::Document, at: usize, del: usize, ins: &str) {
        use markise_doc::FormatDoc as _;
        let ps = markise_doc::PatchSet::single_verbatim(at..at + del, ins);
        let e = d.apply(&ps);
        v.apply(d.text(), &e);
    }

    #[test]
    fn incremental_edits_match_a_rebuild() {
        let mut src = String::new();
        for i in 0..400 {
            src.push_str(&format!("- item {i} with **bold**\n"));
        }
        let mut d = markise_md::parse(&src);
        let mut v = SourceView::new(&src, SourceParams::default(), &Theme::light(), 600.0, 1.0);
        apply(&mut v, &mut d, 10, 0, "\n\nnew para\n");
        apply(&mut v, &mut d, 500, 30, "");
        apply(&mut v, &mut d, 0, 0, "# head\n");
        let len = markise_doc::FormatDoc::text(&d).len();
        apply(&mut v, &mut d, len, 0, "tail without newline");
        let t = markise_doc::FormatDoc::text(&d).to_string();
        v.check(&t).unwrap();
        let full = SourceView::new(&t, SourceParams::default(), &Theme::light(), 600.0, 1.0);
        assert_eq!(v.lines_debug(&t), full.lines_debug(&t));
        assert_eq!(v.line_count(), full.line_count());
    }

    #[test]
    fn layout_and_geometry_round_trip() {
        let src = "# Title\n\nsome *text* here\n\tindented\nlast";
        let d = doc(src);
        let mut fonts = crate::testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let ctx = SourceCtx {
            doc: &d,
            model: None,
            highlights: &NoHighlights,
        };
        let mut v = SourceView::new(src, SourceParams::default(), &Theme::light(), 600.0, 1.0);
        let vis = v.frame(400.0, 200.0, &ctx, &mut f);
        assert_eq!(vis.len(), 1);
        assert_eq!(vis[0].layout.lines.len(), 5);
        // the caret rect of every offset hits back to it
        for off in [0, 3, 9, 14, 26, 28, src.len()] {
            let r = v.caret_rect(DocPos::new(off), &ctx, &mut f);
            let back = v.hit(pos2(r.min.x + 0.1, r.center().y), &ctx, &mut f);
            assert_eq!(back.offset, off, "offset {off}");
        }
        // down from "Title" goes to the empty line, then into "some"
        let (p, x) = v.vertical(DocPos::new(4), None, true, &ctx, &mut f);
        assert_eq!(p.offset, 8);
        let (p, _) = v.vertical(p, Some(x), true, &ctx, &mut f);
        assert_eq!(p.offset, 13);
        // the tab expands to the tab stop: the caret after it is 4 columns in
        let tab = src.find('\t').unwrap();
        let a = v.caret_rect(DocPos::new(tab), &ctx, &mut f).min.x;
        let b = v.caret_rect(DocPos::new(tab + 1), &ctx, &mut f).min.x;
        let one = v.caret_rect(DocPos::new(tab + 2), &ctx, &mut f).min.x - b;
        assert!(b - a > 3.0 * one && b - a < 5.0 * one, "{a} {b} {one}");
        assert_eq!(v.row_edge(DocPos::new(12), true, &ctx, &mut f).offset, 25);
        assert_eq!(v.row_edge(DocPos::new(12), false, &ctx, &mut f).offset, 9);
    }

    #[test]
    fn soft_wrap_rows_have_affinity() {
        let src = format!("{}\nend\n", "word ".repeat(60));
        let d = doc(&src);
        let mut fonts = crate::testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let ctx = SourceCtx {
            doc: &d,
            model: None,
            highlights: &NoHighlights,
        };
        let mut v = SourceView::new(&src, SourceParams::default(), &Theme::light(), 200.0, 1.0);
        v.frame(400.0, 0.0, &ctx, &mut f);
        let l = v.items[0].layout.clone().unwrap();
        assert!(l.lines[0].rows.len() > 2, "the long line wraps");
        let end = v.row_edge(DocPos::new(0), true, &ctx, &mut f);
        assert_eq!(end.affinity, DocAffinity::Upstream);
        let a = v.caret_rect(end, &ctx, &mut f);
        let b = v.caret_rect(DocPos::new(end.offset), &ctx, &mut f);
        assert!(b.min.y > a.min.y, "the same offset downstream is on the next row");
        // without wrap: one row
        v.set_params(
            SourceParams {
                wrap: false,
                ..Default::default()
            },
            200.0,
            &Theme::light(),
            1.0,
            0,
        );
        v.frame(400.0, 0.0, &ctx, &mut f);
        assert_eq!(v.items[0].layout.clone().unwrap().lines[0].rows.len(), 1);
    }

    #[test]
    fn word_and_grapheme_moves() {
        let t = "foo bar_baz, qux\r\nnext";
        assert_eq!(word_move(t, 0, true), 3);
        assert_eq!(word_move(t, 3, true), 11);
        assert_eq!(word_move(t, 11, true), 16);
        assert_eq!(word_move(t, 16, true), 18, "crosses the CRLF in one step");
        assert_eq!(word_move(t, 18, false), 16);
        assert_eq!(word_move(t, 16, false), 13);
        assert_eq!(word_move(t, 4, false), 0);
        assert_eq!(next_grapheme(t, 16), 18, "CRLF is one grapheme");
        assert_eq!(prev_grapheme(t, 18), 16);
        assert_eq!(word_at(t, 5), 4..11);
        assert_eq!(word_at(t, 3), 3..4);
        let e = "e\u{301}x";
        assert_eq!(next_grapheme(e, 0), 3);
    }

    #[test]
    fn tokens_are_invalidated_by_definitions() {
        let src = format!("see [x] here\n\n{}", "filler line\n".repeat(300));
        let src = src.as_str();
        let mut d = markise_md::parse(src);
        let mut v = SourceView::new(src, SourceParams::default(), &Theme::light(), 600.0, 1.0);
        let mut fonts = crate::testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        {
            let a: Arc<dyn FormatDoc> = Arc::new(d.clone());
            let ctx = SourceCtx {
                doc: &a,
                model: None,
                highlights: &NoHighlights,
            };
            v.frame(400.0, 0.0, &ctx, &mut f);
        }
        assert!(v.items[0].layout.is_some());
        assert!(v.len() > 2);
        let len = markise_doc::FormatDoc::text(&d).len();
        apply(&mut v, &mut d, len, 0, "\n[x]: http://e.com\n");
        assert!(
            v.items[0].layout.is_none(),
            "a new definition re-tokenises unchanged items"
        );
    }
}
