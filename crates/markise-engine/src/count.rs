//! Word and character counts of a document's rendered text (status bar, issue #53).
//!
//! **What counts.** The text a reader sees, not the source:
//!
//! - Markup is never counted: emphasis/heading/list/quote markers, backticks, link targets
//!   and titles, image syntax (images count nothing, not even their alt text), footnote
//!   references, inline and block HTML tags, CriticMarkup delimiters.
//! - Front matter, link reference definitions, `[toc]`, thematic breaks, diagrams and math
//!   (inline and display; its TeX source is not visible text) count nothing.
//! - **Fenced and indented code blocks count toward characters but not words** (their text is
//!   visible, but it is not prose); inline code counts like other text.
//! - CriticMarkup is counted *as if accepted*: insertions, the new side of substitutions and
//!   highlights count; deletions, the old side of substitutions and comments do not.
//! - Table cells count like paragraphs (the delimiter row does not).
//! - Raw HTML blocks count the text between their tags, entities decoded (`&amp;` is `&`).
//!
//! **Units.** *Characters* are extended grapheme clusters (an emoji with modifiers is one
//! character), line breaks excluded; a soft line break inside a paragraph shows as a space
//! and counts as one. *Characters without spaces* also drop whitespace. *Words* follow
//! Unicode word segmentation (UAX #29, `unicode-segmentation`): a word is a segment with a
//! letter or digit; each Han, Hiragana, Katakana or Bopomofo character counts as one word
//! (those scripts do not separate words with spaces), while Hangul words are space separated
//! and count normally. Emoji and punctuation are not words.
//!
//! **Incremental, bounded counting.** [`DocCounter`] keeps one [`Counts`] per *block* (a leaf
//! block of the document; the view model may split a large block into several items, which
//! share the block's count) and matches blocks to the view model by the items'
//! offset-independent content hashes. [`DocCounter::reconcile`] compares the current model
//! with the last one (common prefix and suffix of keys, integer compares only) and forgets
//! only the blocks in between; [`DocCounter::fill`] counts forgotten blocks reading the
//! document's text in place, within a [`Budget`] of source bytes and time per call, and resumes
//! inside a block that is larger than that. A block up to [`PIECE_BYTES`] is parsed as a
//! whole (exact inline context); a larger one is parsed in pieces of about that size, cut at
//! line breaks or whitespace runs (the cut whitespace is added back), so inline markup that
//! spans a cut of a huge block can be counted approximately. Selections are counted the same
//! way ([`DocCounter::selection`]): whole blocks from the cache, the partly selected blocks
//! at both ends within the byte budget.
//!
//! Reference definitions are a document-level input: when one is added, removed or changed,
//! every block that may use a reference (its source has a `[`) is counted again.

use std::ops::{Add, AddAssign, Range, Sub, SubAssign};
use std::time::Instant;

use markise_doc::{CriticKind, FormatDoc, InlineKind, InlineNode, InlineSource, RichInline, Span};
use markise_layout::DocModel;
use markise_layout::model::ItemKind;
use unicode_segmentation::UnicodeSegmentation;

/// Source bytes [`DocCounter::fill`] / [`DocCounter::selection`] read at most per call (a
/// frame), plus at most one piece.
pub const FRAME_BYTES: usize = 256 * 1024;
/// Blocks up to this size are parsed whole; larger ones in pieces of about this size.
pub const PIECE_BYTES: usize = 32 * 1024;

/// How much one [`DocCounter::fill`] / [`DocCounter::selection`] call may read: source bytes
/// and, optionally, a deadline (checked between pieces). The first piece is always read, so
/// every call makes progress.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub bytes: usize,
    pub until: Option<Instant>,
}

impl From<usize> for Budget {
    fn from(bytes: usize) -> Budget {
        Budget { bytes, until: None }
    }
}

impl Budget {
    fn spent(&self, bytes: usize) -> bool {
        bytes >= self.bytes || self.until.is_some_and(|t| Instant::now() >= t)
    }
}

/// Reading speed used for the reading time.
pub const WORDS_PER_MINUTE: u64 = 200;

/// Counts of a piece of rendered text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub words: u64,
    /// Grapheme clusters, line breaks excluded.
    pub chars: u64,
    /// Grapheme clusters that are not whitespace.
    pub chars_no_spaces: u64,
}

impl Counts {
    /// Reading time in whole minutes at [`WORDS_PER_MINUTE`] (rounded up; 0 without words).
    pub fn reading_minutes(&self) -> u64 {
        self.words.div_ceil(WORDS_PER_MINUTE)
    }

    /// Counts of rendered prose (words and characters).
    pub fn of_text(text: &str) -> Counts {
        let (chars, chars_no_spaces) = count_chars(text);
        Counts {
            words: count_words(text),
            chars,
            chars_no_spaces,
        }
    }

    /// Counts of visible text that is not prose (code): characters only.
    pub fn of_code(text: &str) -> Counts {
        let (chars, chars_no_spaces) = count_chars(text);
        Counts {
            words: 0,
            chars,
            chars_no_spaces,
        }
    }
}

impl Add for Counts {
    type Output = Counts;
    fn add(mut self, o: Counts) -> Counts {
        self += o;
        self
    }
}

impl AddAssign for Counts {
    fn add_assign(&mut self, o: Counts) {
        self.words += o.words;
        self.chars += o.chars;
        self.chars_no_spaces += o.chars_no_spaces;
    }
}

impl Sub for Counts {
    type Output = Counts;
    fn sub(mut self, o: Counts) -> Counts {
        self -= o;
        self
    }
}

impl SubAssign for Counts {
    fn sub_assign(&mut self, o: Counts) {
        self.words = self.words.saturating_sub(o.words);
        self.chars = self.chars.saturating_sub(o.chars);
        self.chars_no_spaces = self.chars_no_spaces.saturating_sub(o.chars_no_spaces);
    }
}

/// Han, Hiragana, Katakana and Bopomofo: scripts written without spaces between words, where
/// each character counts as a word.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FDF      // CJK radicals, Kangxi radicals
        | 0x3005..=0x3007    // 々 〆 〇
        | 0x3021..=0x3029    // Hangzhou numerals
        | 0x3038..=0x303B
        | 0x3040..=0x30FF    // Hiragana, Katakana
        | 0x3100..=0x312F    // Bopomofo
        | 0x31A0..=0x31FF    // Bopomofo extended, CJK strokes, Katakana phonetic extensions
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK unified ideographs
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0xFF66..=0xFF9F    // halfwidth Katakana
        | 0x20000..=0x3FFFF  // CJK extensions B and later, compatibility supplement
    )
}

/// Words of `text` (see the module docs).
pub fn count_words(text: &str) -> u64 {
    let mut n = 0;
    for w in text.unicode_words() {
        // a segment mixing scripts: every CJK character is a word, every other run of
        // letters/digits (with the punctuation UAX #29 keeps inside words) is one
        let mut in_run = false;
        for c in w.chars() {
            if is_cjk(c) {
                n += 1;
                in_run = false;
            } else if c.is_alphanumeric() && !in_run {
                n += 1;
                in_run = true;
            }
        }
    }
    n
}

/// `(characters, characters without whitespace)` of `text`, line breaks excluded.
pub fn count_chars(text: &str) -> (u64, u64) {
    let (mut all, mut solid) = (0, 0);
    for g in text.graphemes(true) {
        if g.chars().all(char::is_whitespace) {
            if !g.chars().all(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')) {
                all += 1;
            }
        } else {
            all += 1;
            solid += 1;
        }
    }
    (all, solid)
}

/// The largest char boundary of `s` at or below `i`.
fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The part of the rendered value `v` of a leaf whose source content is `cs..ce` (absolute)
/// that lies inside `clip`: exact when the value maps byte for byte onto the content
/// (plain text, most inline code), else proportional (entities, normalised code spans).
fn clip_value<'a>(v: &'a str, cs: usize, ce: usize, clip: &Range<usize>) -> &'a str {
    if clip.start <= cs && ce <= clip.end {
        return v;
    }
    let (a, b) = (clip.start.max(cs) - cs, clip.end.min(ce).max(clip.start.max(cs)) - cs);
    let len = ce - cs;
    let (a, b) = if v.len() == len || len == 0 {
        (a, b)
    } else {
        (a * v.len() / len, (b * v.len()).div_ceil(len))
    };
    let (a, b) = (floor_boundary(v, a), floor_boundary(v, b));
    v.get(a..b.max(a)).unwrap_or("")
}

/// Appends the rendered text of `nodes` (offsets relative to `base`) to `out`; with `clip`
/// (absolute), only the parts inside it.
fn rendered(r: &RichInline, nodes: &[InlineNode], base: usize, clip: Option<&Range<usize>>, out: &mut String) {
    for n in nodes {
        let (s, e) = (base + n.source.start as usize, base + n.source.end as usize);
        if let Some(c) = clip
            && (e <= c.start || s >= c.end)
            && !(s == e && s >= c.start && s < c.end)
        {
            continue;
        }
        match &n.kind {
            InlineKind::Math { .. }
            | InlineKind::Image { .. }
            | InlineKind::FootnoteRef { .. }
            | InlineKind::RawInline
            | InlineKind::Critic(CriticKind::Delete | CriticKind::Old | CriticKind::Comment) => {}
            InlineKind::Break { hard: false } => out.push(' '),
            InlineKind::Break { hard: true } => out.push('\n'),
            _ if !n.children.is_empty() => rendered(r, &n.children, base, clip, out),
            _ => {
                let v = r.value(n);
                match clip {
                    Some(c) => {
                        let (cs, ce) = (base + n.content.start as usize, base + n.content.end as usize);
                        out.push_str(clip_value(v, cs, ce, c));
                    }
                    None => out.push_str(v),
                }
            }
        }
    }
}

/// Where a raw HTML block's text scan is (it continues across pieces).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Strip {
    #[default]
    Text,
    Tag,
    Comment,
}

/// Appends the text of raw HTML `src` without its tags and comments (`st` carries the scan
/// state between calls).
fn strip_tags(src: &str, st: &mut Strip, out: &mut String) {
    let mut i = 0;
    let b = src.as_bytes();
    while i < b.len() {
        match *st {
            Strip::Text => {
                let Some(k) = src[i..].find('<') else {
                    out.push_str(&src[i..]);
                    return;
                };
                out.push_str(&src[i..i + k]);
                i += k;
                let next = b.get(i + 1).copied();
                if src[i..].starts_with("<!--") {
                    *st = Strip::Comment;
                    // a tag separates words like a space would (`<br>`, `</p><p>`)
                    out.push('\n');
                    i += 4;
                } else if next.is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?')) {
                    *st = Strip::Tag;
                    out.push('\n');
                    i += 1;
                } else {
                    out.push('<');
                    i += 1;
                }
            }
            Strip::Tag => match src[i..].find('>') {
                Some(k) => {
                    *st = Strip::Text;
                    i += k + 1;
                }
                None => return,
            },
            Strip::Comment => match src[i..].find("-->") {
                Some(k) => {
                    *st = Strip::Text;
                    i += k + 3;
                }
                None => return,
            },
        }
    }
}

/// `text` with HTML character references decoded (`&amp;` → `&`, `&#233;` → `é`).
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest
            .char_indices()
            .skip(1)
            .take(33)
            .find(|&(_, c)| !(c.is_ascii_alphanumeric() || c == '#'))
            .filter(|&(k, c)| c == ';' && k > 1);
        match end {
            Some((k, _)) => {
                out.push_str(&markise_md::inline::decode_entity(&rest[..=k]));
                rest = &rest[k + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What a block contributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Paragraphs and headings: inline-parsed text.
    Prose,
    /// Code blocks: characters only.
    Code,
    Html,
    Table,
    /// Nothing visible as text (math, diagrams, rules, front matter, definitions, `[toc]`).
    Nothing,
}

/// Counting one block, resumable between calls: its units (content lines, or table rows) are
/// read in pieces of at most about [`PIECE_BYTES`].
#[derive(Debug)]
struct BlockWork {
    /// Items `first..end` of the model show the block.
    first: usize,
    end: usize,
    kind: Kind,
    units: Vec<Span>,
    /// A prose block small enough to parse as a whole.
    whole: bool,
    next: usize,
    /// Bytes of `units[next]` already read (a unit longer than a piece).
    offset: usize,
    acc: Counts,
    clip: Option<Range<usize>>,
    strip: Strip,
    /// The block's source has a `[` (it may use a reference definition).
    refs: bool,
}

/// One piece of a block: the spans to read and the source bytes it covers.
struct Piece {
    spans: Vec<Span>,
    bytes: usize,
}

impl BlockWork {
    fn new(doc: &dyn FormatDoc, model: &DocModel, first: usize, end: usize, clip: Option<Range<usize>>) -> Self {
        let start = model.item_start(first);
        let lines = || doc.leaf(start).map(|l| l.lines).unwrap_or_default();
        let (kind, units) = match model.items.get(first).map(|it| &it.kind) {
            Some(ItemKind::Paragraph | ItemKind::Heading { .. }) => (Kind::Prose, lines()),
            Some(ItemKind::Code { .. }) => (Kind::Code, lines()),
            Some(ItemKind::Html) => (Kind::Html, lines()),
            Some(ItemKind::Table) => match doc.table(start) {
                // rows: 0 = header, 1 = delimiter row, 2.. = body (as the layout draws them)
                Some(t) => (
                    Kind::Table,
                    t.rows
                        .first()
                        .into_iter()
                        .chain(t.rows.iter().skip(2))
                        .copied()
                        .collect(),
                ),
                None => (Kind::Nothing, Vec::new()),
            },
            _ => (Kind::Nothing, Vec::new()),
        };
        let bytes: usize = units.iter().map(|u| u.len()).sum();
        BlockWork {
            first,
            end,
            whole: kind == Kind::Prose && bytes <= PIECE_BYTES,
            kind,
            units,
            next: 0,
            offset: 0,
            acc: Counts::default(),
            clip,
            strip: Strip::default(),
            refs: false,
        }
    }

    fn in_clip(&self, at: usize) -> bool {
        self.clip.as_ref().is_none_or(|c| c.contains(&at))
    }

    fn overlaps_clip(&self, sp: Span) -> bool {
        self.clip
            .as_ref()
            .is_none_or(|c| (sp.start < c.end && sp.end > c.start) || (sp.is_empty() && c.contains(&sp.start)))
    }

    /// The next piece, or `None` when the block is done. Cut whitespace is added to the
    /// characters here (it is lost when the pieces are parsed separately).
    fn next_piece(&mut self, text: &str) -> Option<Piece> {
        if self.whole {
            self.whole = false;
            self.next = self.units.len();
            let bytes = self.units.iter().map(|u| u.len()).sum();
            return (!self.units.is_empty()).then(|| Piece {
                spans: self.units.clone(),
                bytes,
            });
        }
        let mut spans = Vec::new();
        let mut bytes = 0;
        while self.next < self.units.len() {
            let u = self.units[self.next];
            let rest = Span::new(u.start + self.offset, u.end);
            if !self.overlaps_clip(rest) {
                self.next += 1;
                self.offset = 0;
                continue;
            }
            if bytes + rest.len() <= PIECE_BYTES || (spans.is_empty() && self.kind == Kind::Table) {
                spans.push(rest);
                bytes += rest.len();
                self.next += 1;
                self.offset = 0;
                continue;
            }
            if !spans.is_empty() {
                break;
            }
            // one unit longer than a piece: cut it, at a whitespace run where possible
            let s = rest.slice(text);
            let max = floor_boundary(s, PIECE_BYTES).max(s.chars().next().map_or(1, char::len_utf8));
            let ws = |c: char| c == ' ' || c == '\t';
            let mut cut = (max, max);
            if self.kind != Kind::Code
                && let Some(k) = s[..max].rfind(ws).filter(|&k| k >= max / 2)
            {
                let a = s[..k].trim_end_matches(ws).len();
                let b = k + s[k..].len() - s[k..].trim_start_matches(ws).len();
                if a > 0 {
                    cut = (a, b);
                }
            }
            spans.push(Span::new(rest.start, rest.start + cut.0));
            bytes += cut.1;
            self.offset += cut.1;
            if cut.1 > cut.0 && self.in_clip(rest.start + cut.0) {
                let n = s[cut.0..cut.1].chars().count() as u64;
                self.acc.chars += n;
            }
            break;
        }
        if spans.is_empty() {
            return None;
        }
        // a prose piece ending at a line end with more lines to come: the soft break between
        // them shows as a space
        if self.kind == Kind::Prose
            && self.offset == 0
            && self.next < self.units.len()
            && let Some(last) = spans.last()
            && self.in_clip(last.end)
        {
            self.acc.chars += 1;
        }
        Some(Piece { spans, bytes })
    }

    fn count_piece(&mut self, doc: &dyn FormatDoc, p: &Piece) {
        let text = doc.text();
        let clip = self.clip.as_ref();
        let trim = |sp: &Span| match clip {
            Some(c) => text
                .get(c.start.max(sp.start)..c.end.min(sp.end).max(c.start.max(sp.start)))
                .unwrap_or(""),
            None => sp.slice(text),
        };
        let mut out = String::new();
        match self.kind {
            Kind::Prose => {
                self.refs |= p.spans.iter().any(|sp| sp.slice(text).contains('['));
                let base = p.spans[0].start;
                let r = doc.inline(InlineSource::Lines(&p.spans), base);
                rendered(&r, &r.nodes, base, clip, &mut out);
                self.acc += Counts::of_text(&out);
            }
            Kind::Code => {
                for sp in &p.spans {
                    out.push_str(trim(sp));
                    out.push('\n');
                }
                self.acc += Counts::of_code(&out);
            }
            Kind::Html => {
                let mut st = self.strip;
                for sp in &p.spans {
                    strip_tags(trim(sp), &mut st, &mut out);
                    out.push('\n');
                }
                self.strip = st;
                self.acc += Counts::of_text(&decode_entities(&out));
            }
            Kind::Table => {
                for row in &p.spans {
                    self.refs |= row.slice(text).contains('[');
                    for cell in doc.table_row_cells(*row) {
                        if !self.overlaps_clip(cell) {
                            continue;
                        }
                        let r = doc.inline(
                            InlineSource::Cell {
                                span: cell,
                                cap: usize::MAX,
                            },
                            cell.start,
                        );
                        rendered(&r, &r.nodes, cell.start, clip, &mut out);
                        out.push('\n');
                    }
                }
                self.acc += Counts::of_text(&out);
            }
            Kind::Nothing => {}
        }
    }

    /// Reads pieces until `*spent` reaches `budget` (always at least one piece); `true` when
    /// the block is done.
    fn step(&mut self, doc: &dyn FormatDoc, budget: Budget, spent: &mut usize, first_piece: &mut bool) -> bool {
        loop {
            if !self.whole && self.next >= self.units.len() {
                return true;
            }
            if !*first_piece && budget.spent(*spent) {
                return false;
            }
            let Some(p) = self.next_piece(doc.text()) else {
                return true;
            };
            *first_piece = false;
            *spent += p.bytes;
            self.count_piece(doc, &p);
        }
    }
}

/// Item key: offset-independent content hash and length.
pub type ItemKey = (u64, usize);

fn item_key(it: &markise_layout::model::Item) -> ItemKey {
    (it.content_hash, it.len)
}

/// Items `a` and `b` show the same block (segments of one large block).
fn same_block(model: &DocModel, a: usize, b: usize) -> bool {
    match (model.items.get(a), model.items.get(b)) {
        (Some(x), Some(y)) => {
            x.segment.is_some() && y.segment.is_some() && x.len == y.len && model.item_start(a) == model.item_start(b)
        }
        _ => false,
    }
}

/// Items `first..end` of the block that item `i` belongs to.
fn block_bounds(model: &DocModel, i: usize) -> (usize, usize) {
    let n = model.items.len();
    let mut f = i;
    while f > 0 && same_block(model, f - 1, f) {
        f -= 1;
    }
    let mut e = i + 1;
    while e < n && same_block(model, e - 1, e) {
        e += 1;
    }
    (f, e)
}

/// Counts of the block that item `i` of `model` (built for `doc`) belongs to, read at once.
pub fn count_block(doc: &dyn FormatDoc, model: &DocModel, i: usize) -> Counts {
    let (f, e) = block_bounds(model, i);
    let mut w = BlockWork::new(doc, model, f, e, None);
    w.step(doc, usize::MAX.into(), &mut 0, &mut true);
    w.acc
}

/// Counts of the whole document at once (tests and tools).
pub fn count_document(doc: &dyn FormatDoc, model: &DocModel) -> Counts {
    let mut acc = Counts::default();
    let mut i = 0;
    while i < model.items.len() {
        let (_, e) = block_bounds(model, i);
        acc += count_block(doc, model, i);
        i = e;
    }
    acc
}

/// Statistics of a [`DocCounter`] (tests prove that typing does not recount everything and
/// that every call stays within its byte budget).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterStats {
    /// Blocks counted (the document's and selections' partly selected blocks).
    pub blocks_counted: u64,
    /// Source bytes read.
    pub bytes_read: u64,
    /// The most source bytes one [`DocCounter::fill`] or [`DocCounter::selection`] call read.
    pub max_call_bytes: u64,
    /// [`DocCounter::reconcile`] calls that found a change.
    pub reconciles: u64,
}

/// Item flag: a reference definition.
const DEF: u8 = 1;
/// Item flag: its block may use a reference definition.
const REFS: u8 = 2;

/// A selection being counted: whole blocks from the cache, the partly selected blocks at both
/// ends counted with a clip.
#[derive(Debug)]
struct SelWork {
    range: Range<usize>,
    /// Counts known so far (cached whole blocks, finished partial blocks).
    acc: Counts,
    /// Items `a..b` lie fully inside; added from prefix sums once the document is counted.
    middle: Option<(usize, usize)>,
    jobs: Vec<BlockWork>,
}

/// Incremental document counts (see the module docs): one cached [`Counts`] per block, kept on
/// the block's first view-model item (its other items hold zero), matched to the model by
/// content hash.
#[derive(Debug, Default)]
pub struct DocCounter {
    /// Key of each item of the last reconciled model.
    keys: Vec<ItemKey>,
    counts: Vec<Option<Counts>>,
    flags: Vec<u8>,
    /// Sum of the known counts.
    known: Counts,
    /// Items not counted yet (a block's items are all counted or all not).
    missing: usize,
    /// Every item before it is counted.
    cursor: usize,
    /// The block being counted.
    work: Option<BlockWork>,
    sel: Option<SelWork>,
    /// Prefix sums of `counts` (selection counts), built when complete and needed.
    prefix: Option<Vec<Counts>>,
    stats: CounterStats,
}

impl DocCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> CounterStats {
        self.stats
    }

    /// Items not counted yet.
    pub fn missing(&self) -> usize {
        self.missing
    }

    /// The document's counts once every block is counted.
    pub fn total(&self) -> Option<Counts> {
        (self.missing == 0).then_some(self.known)
    }

    fn forget(&mut self, i: usize) {
        if let Some(c) = self.counts[i].take() {
            self.known -= c;
            self.missing += 1;
        }
    }

    /// Matches `model` against the model of the last call: blocks whose items are all in the
    /// common prefix and suffix (same content hash and length) keep their counts, the others
    /// are forgotten (and, when a reference definition changed, every block that may use
    /// one). O(items) comparisons of two integers; no text is read.
    pub fn reconcile(&mut self, model: &DocModel) {
        let items = &model.items;
        let n = items.len();
        let old = self.keys.len();
        let mut pre = 0;
        for (it, k) in items.iter().zip(&self.keys) {
            if item_key(it) != *k {
                break;
            }
            pre += 1;
        }
        if pre == n && pre == old {
            return;
        }
        let mut suf = 0;
        let max_suf = n.min(old) - pre;
        for (it, k) in items.iter().rev().zip(self.keys.iter().rev()) {
            if suf >= max_suf || item_key(it) != *k {
                break;
            }
            suf += 1;
        }
        self.stats.reconciles += 1;
        let mut defs = false;
        for (c, f) in self.counts.drain(pre..old - suf).zip(self.flags.drain(pre..old - suf)) {
            defs |= f & DEF != 0;
            match c {
                Some(c) => self.known -= c,
                None => self.missing -= 1,
            }
        }
        let fresh = n - suf - pre;
        let new: Vec<&markise_layout::model::Item> = items.iter().skip(pre).take(fresh).collect();
        let new_flags: Vec<u8> = new
            .iter()
            .map(|it| if it.kind == ItemKind::Hidden { DEF } else { 0 })
            .collect();
        defs |= new_flags.iter().any(|f| f & DEF != 0);
        self.keys.splice(pre..old - suf, new.iter().map(|it| item_key(it)));
        self.counts.splice(pre..pre, std::iter::repeat_n(None, fresh));
        self.flags.splice(pre..pre, new_flags);
        self.missing += fresh;
        // a block is counted as a whole: forget the rest of the blocks around the change
        let (mut lo, mut hi) = (pre, pre + fresh);
        while lo > 0 && lo < n && same_block(model, lo - 1, lo) {
            lo -= 1;
            self.forget(lo);
        }
        while hi > 0 && hi < n && same_block(model, hi - 1, hi) {
            self.forget(hi);
            hi += 1;
        }
        if defs {
            for i in 0..n {
                if self.flags[i] & REFS != 0 {
                    self.forget(i);
                    lo = lo.min(i);
                }
            }
        }
        self.cursor = self.cursor.min(lo);
        self.work = None;
        self.sel = None;
        self.prefix = None;
        debug_assert_eq!(self.keys.len(), n);
        debug_assert_eq!(self.counts.len(), n);
    }

    fn record(&mut self, spent: usize) {
        self.stats.bytes_read += spent as u64;
        self.stats.max_call_bytes = self.stats.max_call_bytes.max(spent as u64);
    }

    /// Counts forgotten blocks of `model` (reconciled, built for `doc`) within `budget` (plus
    /// one piece); `true` when every block is counted.
    pub fn fill(&mut self, doc: &dyn FormatDoc, model: &DocModel, budget: impl Into<Budget>) -> bool {
        let budget = budget.into();
        let n = model.items.len();
        if self.counts.len() != n {
            return false;
        }
        let mut spent = 0;
        let mut first_piece = true;
        while self.missing > 0 {
            if self.work.is_none() {
                while self.cursor < n && self.counts[self.cursor].is_some() {
                    self.cursor += 1;
                }
                if self.cursor >= n {
                    break;
                }
                let (f, e) = block_bounds(model, self.cursor);
                self.work = Some(BlockWork::new(doc, model, f, e, None));
            }
            let Some(w) = self.work.as_mut() else { break };
            if !w.step(doc, budget, &mut spent, &mut first_piece) {
                break;
            }
            let Some(w) = self.work.take() else { break };
            for i in w.first..w.end {
                let c = if i == w.first { w.acc } else { Counts::default() };
                if self.counts[i].replace(c).is_none() {
                    self.missing -= 1;
                }
                self.flags[i] = (self.flags[i] & !REFS) | if w.refs { REFS } else { 0 };
            }
            self.known += w.acc;
            self.stats.blocks_counted += 1;
            self.cursor = w.end;
        }
        self.record(spent);
        self.missing == 0
    }

    /// Counts of the source byte range `sel` of the document (reconciled `model` built for
    /// `doc`) within `budget` per call: whole blocks from the cache, the partly selected blocks
    /// at both ends with a clip. `None` until done (call again next frame; a large selection
    /// also needs the document's counts complete).
    pub fn selection(
        &mut self,
        doc: &dyn FormatDoc,
        model: &DocModel,
        sel: Range<usize>,
        budget: impl Into<Budget>,
    ) -> Option<Counts> {
        let budget = budget.into();
        if model.items.is_empty() || sel.is_empty() {
            return Some(Counts::default());
        }
        if self.counts.len() != model.items.len() {
            return None;
        }
        if self.sel.as_ref().is_none_or(|s| s.range != sel) {
            let a = model.item_at_offset(sel.start);
            let b = model.item_at_offset(sel.end.saturating_sub(1).max(sel.start));
            let ends = [block_bounds(model, a), block_bounds(model, b)];
            let mut w = SelWork {
                range: sel.clone(),
                acc: Counts::default(),
                middle: (ends[1].0 > ends[0].1).then_some((ends[0].1, ends[1].0)),
                jobs: Vec::new(),
            };
            for (k, &(f, e)) in ends.iter().enumerate() {
                if k == 1 && ends[1] == ends[0] {
                    break;
                }
                let span = model.item_span(f);
                match self.counts[f] {
                    Some(c) if sel.start <= span.start && span.end <= sel.end => w.acc += c,
                    _ => w.jobs.push(BlockWork::new(doc, model, f, e, Some(sel.clone()))),
                }
            }
            self.sel = Some(w);
        }
        let mut spent = 0;
        let mut first_piece = true;
        let missing = self.missing;
        let w = self.sel.as_mut()?;
        while let Some(j) = w.jobs.last_mut() {
            if !j.step(doc, budget, &mut spent, &mut first_piece) {
                break;
            }
            w.acc += j.acc;
            w.jobs.pop();
            self.stats.blocks_counted += 1;
        }
        let done = w.jobs.is_empty();
        let acc = w.acc;
        let middle = w.middle;
        self.record(spent);
        if !done {
            return None;
        }
        let Some((a, b)) = middle else {
            return Some(acc);
        };
        if missing > 0 {
            return None;
        }
        let counts = &self.counts;
        let prefix = self.prefix.get_or_insert_with(|| {
            let mut p = Vec::with_capacity(counts.len() + 1);
            let mut s = Counts::default();
            p.push(s);
            for c in counts {
                s += c.unwrap_or_default();
                p.push(s);
            }
            p
        });
        Some(acc + (prefix[b] - prefix[a]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_md::{Document, ParseOptions};

    fn md(src: &str) -> (Document, std::sync::Arc<DocModel>) {
        let doc = Document::parse(src, ParseOptions::default());
        let model = DocModel::build(&doc, None);
        (doc, model)
    }

    fn count(src: &str) -> Counts {
        let (doc, model) = md(src);
        count_document(&doc, &model)
    }

    fn words(src: &str) -> u64 {
        count(src).words
    }

    #[test]
    fn plain_words_and_characters() {
        let c = count("Hello, world! It's 3.14 o'clock.\n");
        assert_eq!(c.words, 5);
        assert_eq!(c.chars, "Hello, world! It's 3.14 o'clock.".len() as u64);
        assert_eq!(c.chars_no_spaces, "Hello,world!It's3.14o'clock.".len() as u64);
        assert_eq!(count(""), Counts::default());
    }

    #[test]
    fn markdown_syntax_is_not_counted() {
        assert_eq!(count("# Title\n").chars, 5);
        assert_eq!(count("## Title ##\n").chars, 5);
        assert_eq!(words("**bold** and *em* and `code` and ~~gone~~\n"), 7);
        assert_eq!(count("**bold**\n").chars, 4);
        assert_eq!(count("`code`\n").chars, 4);
        // link text counts, its target does not
        assert_eq!(count("[the site](https://example.com/a/b \"Title\")\n").chars, 8);
        assert_eq!(
            words("see <https://example.com> now\n"),
            4,
            "an autolink's text is visible"
        );
        // images count nothing
        assert_eq!(count("![alt text](pic.png)\n"), Counts::default());
        // lists, quotes, task markers
        assert_eq!(count("- one\n- two\n\n> three\n\n1. four\n- [ ] five\n").words, 5);
        assert_eq!(count("- one\n- two\n").chars, 6);
        // footnote references and definitions' labels
        assert_eq!(words("Text[^1].\n\n[^1]: Note here.\n"), 3);
        // a reference definition is hidden
        assert_eq!(words("[a]: https://example.com\n"), 0);
        // inline HTML tags are not text
        assert_eq!(count("a <span class=\"x\">b</span> c\n").chars, 5);
        // emphasis inside a word keeps the word whole
        assert_eq!(words("un**believ**able\n"), 1);
        // escapes and entities count as what they show
        assert_eq!(count("\\*a\\* &amp;\n").chars, 5);
        // thematic breaks and [toc]
        assert_eq!(count("---\n\n[toc]\n\n# H\n").words, 1);
    }

    #[test]
    fn soft_breaks_are_spaces_and_blocks_are_separate() {
        let c = count("one\ntwo\n\nthree\n");
        assert_eq!(c.words, 3);
        // "one two" + "three": the soft break shows as a space; paragraph breaks do not count
        assert_eq!(c.chars, 12);
        assert_eq!(c.chars_no_spaces, 11);
    }

    #[test]
    fn code_and_math_count_characters_but_not_words() {
        let c = count("```rust\nlet x = 1;\n```\n");
        assert_eq!(c.words, 0, "fenced code is not prose");
        assert_eq!(c.chars, "let x = 1;".len() as u64);
        assert_eq!(count("    indented code\n").words, 0);
        assert_eq!(count("    indented code\n").chars, 13);
        // math (display and inline) counts nothing
        assert_eq!(count("$$\nx^2 + y^2\n$$\n"), Counts::default());
        assert_eq!(words("area $\\pi r^2$ here\n"), 2);
        // a diagram fence counts nothing
        assert_eq!(count("```mermaid\ngraph TD; A-->B\n```\n"), Counts::default());
    }

    #[test]
    fn front_matter_is_not_counted() {
        assert_eq!(
            words("---\ntitle: A long title here\ntags: [x, y]\n---\n\nBody text.\n"),
            2
        );
    }

    #[test]
    fn criticmarkup_counts_as_if_accepted() {
        assert_eq!(count("a {++new++} b\n").words, 3);
        assert_eq!(count("a {--old--} b\n").words, 2);
        assert_eq!(count("a {~~old~>new~~} b\n").words, 3);
        assert_eq!(count("a {~~old~>new~~} b\n").chars, 7, "a new b");
        assert_eq!(count("{==marked==}{>>a remark here<<}\n").words, 1);
        assert_eq!(count("x {>>comment<<}\n").chars, 2, "x + space");
    }

    #[test]
    fn tables_count_cells_not_pipes_or_delimiters() {
        let c = count("| Name | Value |\n|---|:---:|\n| a b | c |\n");
        assert_eq!(c.words, 5);
        assert_eq!(c.chars_no_spaces, "NameValueabc".len() as u64);
    }

    #[test]
    fn html_blocks_count_their_text() {
        assert_eq!(words("<div class=\"note\">\n<p>Hello there</p>\n</div>\n"), 2);
        assert_eq!(words("<!-- a hidden comment -->\n"), 0);
    }

    #[test]
    fn cjk_characters_are_words() {
        assert_eq!(words("日本語の文章\n"), 6);
        assert_eq!(words("カタカナ\n"), 4);
        assert_eq!(words("中文 English mix\n"), 4);
        assert_eq!(words("我爱Rust语言\n"), 5);
        // Hangul is written with spaces between words
        assert_eq!(words("안녕하세요 세계\n"), 2);
        let c = count("日本語\n");
        assert_eq!((c.chars, c.chars_no_spaces), (3, 3));
    }

    #[test]
    fn emoji_are_one_character_and_no_word() {
        let c = count("I ❤️ Rust 👨‍👩‍👧‍👦\n");
        assert_eq!(c.words, 2);
        // I, space, heart, space, R-u-s-t, space, the family
        assert_eq!(c.chars, 10);
        let c = count("👍🏽\n");
        assert_eq!((c.words, c.chars), (0, 1));
    }

    #[test]
    fn reading_time_rounds_up() {
        let c = |w| Counts {
            words: w,
            ..Default::default()
        };
        assert_eq!(c(0).reading_minutes(), 0);
        assert_eq!(c(1).reading_minutes(), 1);
        assert_eq!(c(200).reading_minutes(), 1);
        assert_eq!(c(201).reading_minutes(), 2);
    }

    fn counted(src: &str) -> (Document, std::sync::Arc<DocModel>, DocCounter) {
        let (doc, model) = md(src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        assert!(c.fill(&doc, &model, usize::MAX));
        (doc, model, c)
    }

    fn sel(src: &str, a: usize, b: usize) -> Counts {
        let (doc, model, mut c) = counted(src);
        c.selection(&doc, &model, a..b, usize::MAX).unwrap()
    }

    #[test]
    fn counter_matches_full_count_and_recounts_only_changed_blocks() {
        let mut src = String::new();
        for k in 0..300 {
            src.push_str(&format!("Paragraph number {k} with *five* words.\n\n"));
        }
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        assert!(c.total().is_none());
        assert!(c.fill(&doc, &model, usize::MAX));
        assert_eq!(c.total(), Some(count_document(&doc, &model)));
        assert_eq!(c.total().unwrap().words, 300 * 6);
        let before = c.stats().blocks_counted;
        assert_eq!(before, 300);

        // an edit in the middle (and the model's offsets shifting below it)
        let edited = src.replacen("Paragraph number 150 with", "Paragraph number 150 and more with", 1);
        let (doc2, model2) = md(&edited);
        c.reconcile(&model2);
        assert!(c.fill(&doc2, &model2, usize::MAX));
        assert_eq!(c.stats().blocks_counted - before, 1, "only the edited paragraph");
        assert_eq!(c.total(), Some(count_document(&doc2, &model2)));

        // inserting and deleting paragraphs
        let edited = edited.replacen("Paragraph number 10 ", "New one.\n\nParagraph number 10 ", 1);
        let edited = edited.replacen("Paragraph number 200 with *five* words.\n\n", "", 1);
        let (doc3, model3) = md(&edited);
        let before = c.stats().blocks_counted;
        c.reconcile(&model3);
        assert!(c.fill(&doc3, &model3, usize::MAX));
        assert!(
            c.stats().blocks_counted - before <= 200,
            "only the blocks between the two edits"
        );
        assert_eq!(c.total(), Some(count_document(&doc3, &model3)));

        // unchanged model: nothing to do
        let r = c.stats().reconciles;
        c.reconcile(&model3);
        assert_eq!(c.stats().reconciles, r);
    }

    #[test]
    fn fill_reads_at_most_its_byte_budget_per_call_and_resumes() {
        let src = "Some words here.\n\n".repeat(20_000);
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        let mut calls = 0;
        while !c.fill(&doc, &model, 4096) {
            calls += 1;
            assert!(calls < 100_000);
        }
        assert!(calls > 10, "a small budget takes several calls: {calls}");
        assert!(c.stats().max_call_bytes <= 4096 + PIECE_BYTES as u64);
        assert_eq!(c.total().unwrap().words, 20_000 * 3);
    }

    #[test]
    fn a_passed_deadline_still_reads_one_piece_per_call() {
        let src = "Some words here.\n\n".repeat(20_000);
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        let past = Budget {
            bytes: usize::MAX,
            until: Some(Instant::now()),
        };
        assert!(!c.fill(&doc, &model, past));
        assert_eq!(c.stats().blocks_counted, 1, "one piece (one small block)");
        assert!(c.fill(&doc, &model, usize::MAX));
        assert_eq!(c.total().unwrap().words, 20_000 * 3);
    }

    /// Review P1-1: one huge block (a heading of hundreds of KB) is counted in pieces across calls, never
    /// in one go, and the result matches a whole count of the same text.
    #[test]
    fn a_huge_single_block_is_counted_in_pieces() {
        let body = "lorem ipsum dolor sit amet ".repeat(16_000); // ~430 KB, one line
        let src = format!("# {body}\n");
        let (doc, model) = md(&src);
        assert_eq!(model.items.len(), 1);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        let mut calls = 1;
        while !c.fill(&doc, &model, FRAME_BYTES) {
            calls += 1;
        }
        assert!(calls >= 2, "430 KB at 256 KB per call: {calls} calls");
        assert!(c.stats().max_call_bytes <= (FRAME_BYTES + PIECE_BYTES) as u64);
        let t = c.total().unwrap();
        assert_eq!(t.words, 80_000);
        let trimmed = body.trim_end();
        assert_eq!(t.chars, trimmed.len() as u64, "cut whitespace is added back");
        assert_eq!(t.chars_no_spaces, trimmed.replace(' ', "").len() as u64);
        // a huge paragraph of many lines: the soft breaks between pieces count as spaces
        let lines = "word word\n".repeat(10_000);
        let (doc, model) = md(&lines);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        while !c.fill(&doc, &model, FRAME_BYTES) {}
        let t = c.total().unwrap();
        assert_eq!(t.words, 20_000);
        assert_eq!(t.chars, (lines.len() - 1) as u64);
        // selections inside the huge block are bounded too
        let src2 = format!("# {body}\n");
        let (doc, model) = md(&src2);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        while !c.fill(&doc, &model, FRAME_BYTES) {}
        let (a, b) = (2 + 6, src2.len() - 1 - 6); // "ipsum … sit"
        let before = c.stats().max_call_bytes;
        let mut calls = 1;
        let s = loop {
            if let Some(s) = c.selection(&doc, &model, a..b, FRAME_BYTES) {
                break s;
            }
            calls += 1;
        };
        assert!(calls >= 2, "{calls}");
        assert!(c.stats().max_call_bytes.max(before) <= (FRAME_BYTES + PIECE_BYTES) as u64);
        assert_eq!(s.words, 80_000 - 2, "all but the first and the last word");
    }

    /// Review P2-3: a block split into layout segments is counted as one block (inline context
    /// and soft breaks across segment boundaries are kept).
    #[test]
    fn segmented_blocks_count_as_one_block() {
        let src = "a\n".repeat(130);
        let (doc, model) = md(&src);
        assert!(model.items.len() > 1, "segmented");
        let c = count_document(&doc, &model);
        assert_eq!((c.words, c.chars), (130, 259));
        let (_, _, k) = counted(&src);
        assert_eq!(k.total().unwrap().chars, 259);
        // strong emphasis spanning the segments: its delimiters are not counted
        let src = format!("**{}**\n", "word\n".repeat(200).trim_end());
        let (doc, model) = md(&src);
        assert!(model.items.len() > 1, "segmented");
        let c = count_document(&doc, &model);
        assert_eq!(c.words, 200);
        assert_eq!(c.chars, (200 * 4 + 199) as u64);
        // an edit in one segment recounts the whole block
        let (_, _, mut k) = counted(&src);
        let edited = src.replacen("word\nword", "word\nwords", 1);
        let (doc2, model2) = md(&edited);
        k.reconcile(&model2);
        assert!(k.fill(&doc2, &model2, usize::MAX));
        assert_eq!(k.total(), Some(count_document(&doc2, &model2)));
        assert_eq!(k.total().unwrap().chars, (200 * 4 + 199 + 1) as u64);
    }

    /// Review P2-4: a selection inside a later segment of a large table is counted.
    #[test]
    fn selections_in_later_table_segments_are_counted() {
        let mut src = String::from("| Name | Value |\n|---|---|\n");
        for k in 0..400 {
            src.push_str(&format!("| row{k} | cell{k} |\n"));
        }
        let (_, model) = md(&src);
        assert!(model.items.len() > 2, "segmented table");
        let a = src.find("| row350 |").unwrap() + 2;
        let s = sel(&src, a, a + "row350".len());
        assert_eq!((s.words, s.chars), (1, 6));
        // across rows of different segments
        let b = src.find("cell360").unwrap() + "cell360".len();
        let s = sel(&src, a, b);
        assert_eq!(s.words, 11 * 2);
    }

    /// Review P2-5: partial selections cut inline code and HTML lines to the selection.
    #[test]
    fn partial_selections_cut_code_spans_and_html() {
        let src = "x `abcdef` y\n";
        let a = src.find("bcd").unwrap();
        let s = sel(src, a, a + 3);
        assert_eq!(s.chars, 3);
        let src = "<div>\nalpha beta gamma\n</div>\n";
        let a = src.find("beta").unwrap();
        let s = sel(src, a, a + 4);
        assert_eq!((s.words, s.chars), (1, 4));
        let src = "```\nlet value = 1;\n```\n";
        let a = src.find("value").unwrap();
        assert_eq!(sel(src, a, a + 5).chars, 5);
    }

    /// Review P2-6: adding or removing a reference definition recounts the blocks using it.
    #[test]
    fn reference_definition_edits_recount_their_users() {
        let with = "See [the site][r] now.\n\n[r]: https://example.com\n";
        let without = "See [the site][r] now.\n";
        let (_, _, mut c) = counted(with);
        assert_eq!(c.total().unwrap().chars, "See the site now.".len() as u64);
        let (doc, model) = md(without);
        c.reconcile(&model);
        assert!(c.fill(&doc, &model, usize::MAX));
        assert_eq!(c.total(), Some(count_document(&doc, &model)));
        assert_eq!(c.total().unwrap().chars, "See [the site][r] now.".len() as u64);
        let (doc, model) = md(with);
        c.reconcile(&model);
        assert!(c.fill(&doc, &model, usize::MAX));
        assert_eq!(c.total().unwrap().chars, "See the site now.".len() as u64);
    }

    /// Review P2-8: entities in raw HTML count as the characters they show.
    #[test]
    fn html_entities_are_decoded() {
        let c = count("<p>a &amp; b &#233; &lt;</p>\n");
        assert_eq!(c.chars, "a & b é <".len() as u64 - 1, "é is one character");
        assert_eq!(c.chars, 9);
        assert_eq!(
            count("<p>AT&T &unknown; x</p>\n").chars,
            "AT&T &unknown; x".len() as u64
        );
        assert_eq!(decode_entities("&amp;&#x41;&;"), "&A&;");
    }

    #[test]
    fn selection_counts() {
        let src = "Alpha beta gamma.\n\nDelta epsilon.\n\n```\ncode here\n```\n\nZeta eta theta iota.\n";
        let a = src.find("beta").unwrap();
        let b = src.find("eta theta").unwrap() + 3;
        assert_eq!(sel(src, a, b).words, 2 + 2 + 2);
        let (doc, model, mut c) = counted(src);
        assert_eq!(c.selection(&doc, &model, 0..src.len(), usize::MAX), c.total());
        let s = sel(src, a, a + 2);
        assert_eq!((s.words, s.chars), (1, 2));
        // markers inside the selection are not counted
        let src = "Some **bold** text.\n";
        assert_eq!(sel(src, 0, src.len() - 1).chars, "Some bold text.".len() as u64);
    }

    #[test]
    fn large_selection_uses_prefix_sums() {
        let src = "One two three.\n\n".repeat(500);
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        assert_eq!(
            c.selection(&doc, &model, 0..src.len() - 2, usize::MAX),
            None,
            "the middle needs the document counted"
        );
        c.fill(&doc, &model, usize::MAX);
        let before = c.stats().blocks_counted;
        let s = c.selection(&doc, &model, 4..src.len() - 9, usize::MAX).unwrap();
        assert_eq!(s.words, 500 * 3 - 2);
        assert_eq!(c.stats().blocks_counted - before, 2, "only the two partial blocks");
    }
}
