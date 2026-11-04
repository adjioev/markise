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
//! **Estimates (documented approximation).** A block whose source is larger than
//! [`BIG_BLOCK`] (a pathological single paragraph, table or code block) is not parsed: its
//! counts are estimated from its byte length (characters ≈ bytes, words ≈ bytes / 6;
//! [`estimate`]) and marked with [`Counts::approx`], which the status bar shows as `~`.
//! Documents and selections of normal-sized blocks are counted exactly, however large: a
//! selection's fully selected middle is summed from cached totals of runs of blocks (issue
//! #125; it was estimated beyond 4,096 blocks).
//!
//! **Incremental, bounded counting.** [`DocCounter`] keeps one [`Counts`] per *top-level
//! block* of the document (not per view-model item: a paged view model materializes items only
//! near the viewport, issue #125). [`DocCounter::apply`] follows the document's edits from
//! their summaries ([`EditSummary`]: the replaced top-level blocks are forgotten, the others
//! shift; no text is read); [`DocCounter::fill`] counts forgotten blocks reading the
//! document's text in place, about a byte budget per call, leaf block by leaf block (each
//! leaf is at most [`BIG_BLOCK`]; a large container is counted over several calls).
//! [`DocCounter::selection`] reads only the leaves of the two partly selected blocks that the
//! selection touches.
//!
//! Reference definitions are a document-level input: when the format reports one added,
//! removed or changed, every block that may use a reference (its source has a `[`) is
//! counted again.

use std::ops::{Add, AddAssign, Range, Sub, SubAssign};

use markise_doc::{
    CriticKind, EditSummary, FlatEvent, FormatDoc, InlineKind, InlineNode, InlineSource, LeafKind, RichInline, Span,
};
use unicode_segmentation::UnicodeSegmentation;

/// Blocks with a larger source are estimated, not parsed.
pub const BIG_BLOCK: usize = 64 * 1024;
/// Source bytes [`DocCounter::fill`] reads per step of the counting worker
/// ([`crate::count_worker`]; an edit waits at most one step), plus at most one block.
pub const FRAME_BYTES: usize = 32 * 1024;
/// Average bytes per word of the estimate.
const BYTES_PER_WORD: u64 = 6;

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
    /// Number of estimated parts included (blocks larger than [`BIG_BLOCK`], very large
    /// selections); the UI marks such counts with `~`.
    pub approx: u64,
}

impl Counts {
    /// Reading time in whole minutes at [`WORDS_PER_MINUTE`] (rounded up; 0 without words).
    pub fn reading_minutes(&self) -> u64 {
        self.words.div_ceil(WORDS_PER_MINUTE)
    }

    /// Whether an estimate is included.
    pub fn is_approx(&self) -> bool {
        self.approx > 0
    }

    /// Counts of rendered prose (words and characters).
    pub fn of_text(text: &str) -> Counts {
        let (chars, chars_no_spaces) = count_chars(text);
        Counts {
            words: count_words(text),
            chars,
            chars_no_spaces,
            approx: 0,
        }
    }

    /// Counts of visible text that is not prose (code): characters only.
    pub fn of_code(text: &str) -> Counts {
        let (chars, chars_no_spaces) = count_chars(text);
        Counts {
            words: 0,
            chars,
            chars_no_spaces,
            approx: 0,
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
        self.approx += o.approx;
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
        self.approx = self.approx.saturating_sub(o.approx);
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

/// Where the scan of a raw HTML block is (it continues across the block's lines).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Strip {
    #[default]
    Text,
    Tag,
    Comment,
}

/// Appends the text of one line of a raw HTML block (`src`, starting at absolute offset
/// `base`) without tags and comments; `st` carries the scan state from the block's start.
/// With `clip`, only text whose source lies inside it is emitted (tags are always skipped,
/// so a clip starting inside a tag emits nothing of it).
fn strip_tags(src: &str, base: usize, st: &mut Strip, clip: Option<&Range<usize>>, out: &mut String) {
    let b = src.as_bytes();
    let keep = |at: usize| clip.is_none_or(|c| c.contains(&(base + at)));
    let mut i = 0;
    while i < b.len() {
        match *st {
            Strip::Text => {
                if b[i] == b'<' {
                    if src[i..].starts_with("<!--") {
                        *st = Strip::Comment;
                        // a tag separates words like a space would (`<br>`, `</p><p>`)
                        out.push('\n');
                        i += 4;
                        continue;
                    }
                    if b.get(i + 1)
                        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
                    {
                        *st = Strip::Tag;
                        out.push('\n');
                        i += 1;
                        continue;
                    }
                }
                let ch = src[i..].chars().next().unwrap_or(' ');
                if keep(i) {
                    out.push(ch);
                }
                i += ch.len_utf8();
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

/// What a leaf block of `kind` contributes (as the view shows it: math and diagram fences are
/// not text).
fn leaf_kind(kind: &LeafKind) -> Kind {
    match kind {
        LeafKind::Paragraph | LeafKind::Heading { .. } => Kind::Prose,
        LeafKind::CodeBlock { info } => {
            let lang = info.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            match lang.as_str() {
                "math" | "mermaid" | "flow" | "sequence" => Kind::Nothing,
                _ => Kind::Code,
            }
        }
        LeafKind::RawBlock => Kind::Html,
        LeafKind::Table { .. } => Kind::Table,
        _ => Kind::Nothing,
    }
}

/// The estimate for `bytes` of source of a block of `kind` (see the module docs).
fn estimate_kind(kind: Kind, bytes: usize) -> Counts {
    let b = bytes as u64;
    match kind {
        Kind::Nothing => Counts::default(),
        Kind::Code => Counts {
            words: 0,
            chars: b,
            chars_no_spaces: b - b / BYTES_PER_WORD,
            approx: 1,
        },
        _ => Counts {
            words: b / BYTES_PER_WORD,
            chars: b,
            chars_no_spaces: b - b / BYTES_PER_WORD,
            approx: 1,
        },
    }
}

/// The estimate for a prose block of `bytes` bytes (characters ≈ bytes, words ≈ bytes / 6).
pub fn estimate(bytes: usize) -> Counts {
    estimate_kind(Kind::Prose, bytes)
}

/// A counted block: its counts, whether it may use reference definitions, source bytes read.
#[derive(Clone, Copy, Debug, Default)]
struct Counted {
    counts: Counts,
    refs: bool,
    read: usize,
}

/// Counts the leaf block `span` of `kind` (with `clip`, only the part inside it).
fn count_leaf(doc: &dyn FormatDoc, span: Span, kind: Kind, clip: Option<&Range<usize>>) -> Counted {
    let none = Counted {
        counts: Counts::default(),
        refs: false,
        read: 0,
    };
    if kind == Kind::Nothing {
        return none;
    }
    let part = match clip {
        Some(c) => c.end.min(span.end).saturating_sub(c.start.max(span.start)),
        None => span.len(),
    };
    if span.len() > BIG_BLOCK {
        return Counted {
            counts: estimate_kind(kind, part),
            ..none
        };
    }
    let text = doc.text();
    let overlaps = |sp: &Span| {
        clip.is_none_or(|c| (sp.start < c.end && sp.end > c.start) || (sp.is_empty() && c.contains(&sp.start)))
    };
    let trim = |sp: &Span| match clip {
        Some(c) => text
            .get(c.start.max(sp.start)..c.end.min(sp.end).max(c.start.max(sp.start)))
            .unwrap_or(""),
        None => sp.slice(text),
    };
    let mut out = String::new();
    let mut refs = false;
    let counts = match kind {
        Kind::Prose => {
            let lines = doc.leaf(span.start).map(|l| l.lines).unwrap_or_default();
            if lines.is_empty() || !lines.iter().any(overlaps) {
                return none;
            }
            refs = lines.iter().any(|l| l.slice(text).contains('['));
            let base = lines[0].start;
            let r = doc.inline(InlineSource::Lines(&lines), base);
            rendered(&r, &r.nodes, base, clip, &mut out);
            Counts::of_text(&out)
        }
        Kind::Code => {
            for l in doc
                .leaf(span.start)
                .map(|l| l.lines)
                .unwrap_or_default()
                .iter()
                .filter(|l| overlaps(l))
            {
                out.push_str(trim(l));
                out.push('\n');
            }
            Counts::of_code(&out)
        }
        Kind::Html => {
            // scanned from the block's start so tags and comments are known at the clip
            let mut st = Strip::default();
            for l in doc.leaf(span.start).map(|l| l.lines).unwrap_or_default() {
                strip_tags(l.slice(text), l.start, &mut st, clip, &mut out);
                out.push('\n');
            }
            Counts::of_text(&decode_entities(&out))
        }
        Kind::Table => {
            let Some(t) = doc.table(span.start) else { return none };
            // rows: 0 = header, 1 = delimiter row, 2.. = body (as the layout draws them)
            for row in t.rows.first().into_iter().chain(t.rows.iter().skip(2)) {
                refs |= row.slice(text).contains('[');
                for cell in doc.table_row_cells(*row) {
                    if !overlaps(&cell) {
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
            Counts::of_text(&out)
        }
        Kind::Nothing => Counts::default(),
    };
    Counted {
        counts,
        refs,
        read: span.len(),
    }
}

/// Counts of the whole document at once (tests and tools).
pub fn count_document(doc: &dyn FormatDoc) -> Counts {
    let mut acc = Counts::default();
    for t in 0..doc.top_count() {
        acc += count_top(doc, t, None, 0, usize::MAX).counted.counts;
    }
    acc
}

/// Counts of the source byte range `sel` of `doc`, read directly (no cache): every leaf
/// block inside or partly inside it, clipped. Reads about `sel.len()` bytes plus the leaves
/// that `sel` cuts, so the status bar uses it for small selections only; larger ones go
/// through [`DocCounter::selection`] on the counting worker ([`crate::count_worker`]).
pub fn count_range(doc: &dyn FormatDoc, sel: Range<usize>) -> Counts {
    let n = doc.top_count();
    if n == 0 || sel.is_empty() {
        return Counts::default();
    }
    let top_at = |o: usize| doc.top_at(o).unwrap_or(0).min(n - 1);
    let (a, b) = (top_at(sel.start), top_at(sel.end.saturating_sub(1).max(sel.start)));
    let mut acc = Counts::default();
    for t in a..=b {
        acc += count_top(doc, t, Some(&sel), 0, usize::MAX).counted.counts;
    }
    acc
}

/// What [`count_top`] found.
struct TopCount {
    counted: Counted,
    /// Where to resume (the start of the next leaf to count); `None`: the block is done.
    next: Option<usize>,
}

/// Counts the leaf blocks of top-level block `t` that start at or after `from` (with `clip`,
/// only the parts inside it), until about `budget` source bytes are read (at least one
/// leaf; each leaf reads at most [`BIG_BLOCK`] bytes).
fn count_top(doc: &dyn FormatDoc, t: usize, clip: Option<&Range<usize>>, from: usize, budget: usize) -> TopCount {
    let mut leaves: Vec<(Kind, Span)> = Vec::new();
    doc.flatten(t..t + 1, &mut |ev| {
        if let FlatEvent::Leaf(l) = ev {
            leaves.push((leaf_kind(&l.kind), l.abs_span()));
        }
    });
    let mut out = TopCount {
        counted: Counted::default(),
        next: None,
    };
    for (kind, span) in leaves {
        if span.start < from {
            continue;
        }
        if out.counted.read > 0 && out.counted.read >= budget {
            out.next = Some(span.start);
            break;
        }
        if clip.is_some_and(|c| span.end < c.start || span.start > c.end) {
            continue;
        }
        let c = count_leaf(doc, span, kind, clip);
        out.counted.counts += c.counts;
        out.counted.refs |= c.refs;
        out.counted.read += c.read;
    }
    out
}

/// Statistics of a [`DocCounter`] (tests prove that typing does not recount everything and
/// that every call stays within its byte budget).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterStats {
    /// Top-level blocks counted (the document's and selections' partly selected blocks).
    pub blocks_counted: u64,
    /// Source bytes read.
    pub bytes_read: u64,
    /// The most source bytes one [`DocCounter::fill`] or [`DocCounter::selection`] call read.
    pub max_call_bytes: u64,
    /// Changes applied ([`DocCounter::apply`], [`DocCounter::reset`]).
    pub reconciles: u64,
}

/// A block's cached [`Counts`] (or none yet) in 16 bytes instead of 40 (issue #29, part 4).
/// Counts that do not fit in 32 bits (only a block of ~4 GiB) are kept exactly, out of line in
/// [`DocCounter::big`]; the slot then holds their index ([`Slot::BIG`] tag). Never capped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot([u32; 4]);

impl Slot {
    const NONE: Slot = Slot([u32::MAX, 0, 0, 0]);
    /// Tag in the first word: counts are in `DocCounter::big[self.0[1]]`.
    const BIG: u32 = u32::MAX - 1;
    /// Largest value stored inline.
    const MAX_INLINE: u64 = u32::MAX as u64 - 2;

    /// The inline slot of `c`, `None` when a value does not fit.
    fn inline(c: Counts) -> Option<Slot> {
        let f = |v: u64| (v <= Slot::MAX_INLINE).then_some(v as u32);
        Some(Slot([f(c.words)?, f(c.chars)?, f(c.chars_no_spaces)?, f(c.approx)?]))
    }

    fn is_some(self) -> bool {
        self != Slot::NONE
    }
}

/// Progress of a selection counted in steps: the two end blocks (`a`, `b`), the end being
/// counted (index into them), where in it to resume, the counts so far.
#[derive(Debug)]
struct SelPartial {
    sel: Range<usize>,
    a: usize,
    b: usize,
    end: usize,
    from: usize,
    acc: Counts,
}

/// Block flag: it may use a reference definition.
const REFS: u8 = 1;

/// Incremental document counts (see the module docs): one cached [`Counts`] per top-level
/// block of the document, kept in step with the document's edits ([`DocCounter::apply`]).
#[derive(Debug, Default)]
pub struct DocCounter {
    counts: Vec<Slot>,
    /// Out-of-line counts of [`Slot::BIG`] slots, and the free entries among them.
    big: Vec<Counts>,
    big_free: Vec<u32>,
    flags: Vec<u8>,
    /// Sum of the known counts.
    known: Counts,
    /// Blocks not counted yet.
    missing: usize,
    /// Every block before it is counted.
    cursor: usize,
    /// A block counted in part (a large container takes several calls): its index, the start
    /// of its next leaf, the counts so far.
    partial: Option<(usize, usize, Counted)>,
    /// The last selection's counts.
    sel: Option<(Range<usize>, Counts)>,
    /// A selection counted in steps ([`DocCounter::selection_step`]) not finished yet.
    sel_partial: Option<SelPartial>,
    /// Totals of the blocks `k * SUM_BLOCKS ..` (`None`: not summed since a change): a
    /// selection's fully selected middle is summed from them exactly (issue #125).
    sums: Vec<Option<Counts>>,
    stats: CounterStats,
}

/// Top-level blocks per cached total ([`DocCounter::sums`]).
const SUM_BLOCKS: usize = 1024;

impl DocCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> CounterStats {
        self.stats
    }

    /// Top-level blocks not counted yet.
    pub fn missing(&self) -> usize {
        self.missing
    }

    /// The document's counts once every block is counted.
    pub fn total(&self) -> Option<Counts> {
        (self.missing == 0).then_some(self.known)
    }

    /// Blocks the counter holds (the document's top-level blocks once in step with it).
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// A slot holding `c` (out of line when it does not fit inline).
    fn store(&mut self, c: Counts) -> Slot {
        if let Some(s) = Slot::inline(c) {
            return s;
        }
        let k = match self.big_free.pop() {
            Some(k) => {
                self.big[k as usize] = c;
                k
            }
            None => {
                self.big.push(c);
                (self.big.len() - 1) as u32
            }
        };
        Slot([Slot::BIG, k, 0, 0])
    }

    /// The counts held by `s` (`None`: not counted).
    fn load(&self, s: Slot) -> Option<Counts> {
        match s.0[0] {
            u32::MAX => None,
            Slot::BIG => Some(self.big[s.0[1] as usize]),
            _ => Some(Counts {
                words: s.0[0] as u64,
                chars: s.0[1] as u64,
                chars_no_spaces: s.0[2] as u64,
                approx: s.0[3] as u64,
            }),
        }
    }

    /// `s` is no longer held anywhere: frees its out-of-line entry.
    fn release(&mut self, s: Slot) {
        if s.0[0] == Slot::BIG {
            self.big_free.push(s.0[1]);
        }
    }

    fn forget(&mut self, i: usize) {
        self.dirty(i);
        let s = std::mem::replace(&mut self.counts[i], Slot::NONE);
        if let Some(c) = self.load(s) {
            self.known -= c;
            self.missing += 1;
        }
        self.release(s);
        if self.partial.is_some_and(|p| p.0 == i) {
            self.partial = None;
        }
        self.cursor = self.cursor.min(i);
    }

    /// Starts over for `doc`: every top-level block uncounted.
    pub fn reset(&mut self, doc: &dyn FormatDoc) {
        let n = doc.top_count();
        *self = DocCounter {
            counts: vec![Slot::NONE; n],
            flags: vec![0; n],
            missing: n,
            stats: self.stats,
            ..Default::default()
        };
        self.stats.reconciles += 1;
    }

    /// Applies an edit of the document (`e` describes the change from the state the counter
    /// is in step with to the next one): the counts of the replaced top-level blocks are
    /// forgotten, the new blocks are uncounted, the others shift; when a reference definition
    /// may have changed, every block that may use one is forgotten too. No text is read;
    /// edits are applied in the order they were made. `false` when `e` does not fit the
    /// counter (the caller starts over with [`DocCounter::reset`]).
    pub fn apply(&mut self, e: &EditSummary) -> bool {
        // selection counts are by byte range: recomputed after any change
        self.sel = None;
        self.sel_partial = None;
        if e.deps.all {
            return false;
        }
        let r = e.replaced_top.clone();
        if r.end > self.counts.len() || r.start != e.new_top.start {
            return false;
        }
        let fresh = e.new_top.len();
        if r.is_empty() && fresh == 0 && e.deps.labels.is_empty() {
            return true;
        }
        self.stats.reconciles += 1;
        // a block counted in part: forgotten when replaced, moved along when after the edit
        self.partial = match self.partial.take() {
            Some((p, _, _)) if r.contains(&p) => None,
            Some((p, from, c)) if p >= r.end => Some((p + fresh - r.len(), (from as isize + e.delta) as usize, c)),
            other => other,
        };
        let gone: Vec<Slot> = self
            .counts
            .splice(r.clone(), std::iter::repeat_n(Slot::NONE, fresh))
            .collect();
        self.flags.splice(r.clone(), std::iter::repeat_n(0, fresh));
        if fresh == r.len() {
            for k in r.start / SUM_BLOCKS..r.end.div_ceil(SUM_BLOCKS) {
                self.dirty(k * SUM_BLOCKS);
            }
        } else {
            // the blocks after the splice moved: their totals start over
            self.sums.truncate(r.start / SUM_BLOCKS);
        }
        for s in gone {
            match self.load(s) {
                Some(c) => self.known -= c,
                None => self.missing -= 1,
            }
            self.release(s);
        }
        self.missing += fresh;
        self.cursor = self.cursor.min(r.start);
        // reference definitions the format reports as added, removed or changed: blocks that
        // may use one count again
        if !e.deps.labels.is_empty() {
            self.forget_refs();
        }
        true
    }

    /// Forgets every block that may use a reference definition.
    /// The cached total holding block `i` is stale.
    fn dirty(&mut self, i: usize) {
        if let Some(s) = self.sums.get_mut(i / SUM_BLOCKS) {
            *s = None;
        }
    }

    /// Exact counts of blocks `r` (all counted), from the cached totals where whole.
    fn sum_range(&mut self, r: Range<usize>) -> Counts {
        let slots = |c: &Self, r: Range<usize>| {
            c.counts[r]
                .iter()
                .fold(Counts::default(), |s, x| s + c.load(*x).unwrap_or_default())
        };
        let n = self.counts.len();
        self.sums.resize(n.div_ceil(SUM_BLOCKS), None);
        let mut acc = Counts::default();
        let mut i = r.start;
        while i < r.end {
            let k = i / SUM_BLOCKS;
            let (a, b) = (k * SUM_BLOCKS, ((k + 1) * SUM_BLOCKS).min(n));
            if i == a && b <= r.end {
                let t = match self.sums[k] {
                    Some(t) => t,
                    None => {
                        let t = slots(self, a..b);
                        self.sums[k] = Some(t);
                        t
                    }
                };
                acc += t;
                i = b;
            } else {
                let e = b.min(r.end);
                acc += slots(self, i..e);
                i = e;
            }
        }
        acc
    }

    fn forget_refs(&mut self) {
        // a block counted in part: the leaves counted so far may use one too (review of #129)
        if self.partial.is_some_and(|(_, _, c)| c.refs) {
            self.partial = None;
        }
        for i in 0..self.counts.len() {
            if self.flags[i] & REFS != 0 {
                self.forget(i);
            }
        }
    }

    fn record(&mut self, read: usize) {
        self.stats.bytes_read += read as u64;
        self.stats.max_call_bytes = self.stats.max_call_bytes.max(read as u64);
    }

    /// Counts forgotten blocks of `doc` (the document the counter is in step with) until about
    /// `budget` source bytes are read (at least one leaf block, each at most [`BIG_BLOCK`]);
    /// `true` when every block is counted.
    pub fn fill(&mut self, doc: &dyn FormatDoc, budget: usize) -> bool {
        let n = self.counts.len();
        if n != doc.top_count() {
            return false;
        }
        let mut read = 0;
        let mut first = true;
        while self.missing > 0 && (first || read < budget) {
            while self.cursor < n && self.counts[self.cursor].is_some() {
                self.cursor += 1;
            }
            if self.cursor >= n {
                break;
            }
            let t = self.cursor;
            let (from, mut acc) = match self.partial.take() {
                Some((p, from, acc)) if p == t => (from, acc),
                _ => (0, Counted::default()),
            };
            let left = if first { budget } else { budget.saturating_sub(read) };
            let tc = count_top(doc, t, None, from, left.max(1));
            acc.counts += tc.counted.counts;
            acc.refs |= tc.counted.refs;
            read += tc.counted.read;
            first = false;
            if let Some(next) = tc.next {
                self.partial = Some((t, next, acc));
                break;
            }
            let v = self.store(acc.counts);
            self.dirty(t);
            let old = std::mem::replace(&mut self.counts[t], v);
            if !old.is_some() {
                self.missing -= 1;
            }
            self.release(old);
            self.flags[t] = if acc.refs { REFS } else { 0 };
            self.known += acc.counts;
            self.stats.blocks_counted += 1;
            self.cursor = t + 1;
        }
        self.record(read);
        self.missing == 0
    }

    /// Counts of the source byte range `sel` of `doc` (the document the counter is in step
    /// with, every block counted — else `None`): whole blocks from the cache, the partly
    /// selected blocks at both ends with a clip (only their leaves inside it are read, each
    /// at most [`BIG_BLOCK`]); the fully selected middle exactly, from cached totals of runs
    /// of [`SUM_BLOCKS`] blocks (O(middle / SUM_BLOCKS) once they are summed). The result is
    /// kept until the selection or the document
    /// changes. Reads everything at once; [`DocCounter::selection_step`] does it in steps.
    pub fn selection(&mut self, doc: &dyn FormatDoc, sel: Range<usize>) -> Option<Counts> {
        loop {
            if let Some(r) = self.selection_step(doc, sel.clone(), usize::MAX) {
                return r;
            }
        }
    }

    /// [`DocCounter::selection`] in steps of about `budget` source bytes (at least one leaf
    /// block, each at most [`BIG_BLOCK`]; issue #132: the counting worker reads the lent
    /// document in short steps even for a selection of most of a huge list): `None` while not
    /// done — call again (with the same `sel`; another range starts over), `Some` with the
    /// result of [`DocCounter::selection`] once done. A document change forgets the progress.
    pub fn selection_step(&mut self, doc: &dyn FormatDoc, sel: Range<usize>, budget: usize) -> Option<Option<Counts>> {
        let n = self.counts.len();
        if n == 0 || sel.is_empty() {
            return Some(Some(Counts::default()));
        }
        if self.total().is_none() {
            return Some(None);
        }
        if n != doc.top_count() {
            return Some(None);
        }
        if let Some((r, c)) = &self.sel
            && *r == sel
        {
            return Some(Some(*c));
        }
        let mut p = match self.sel_partial.take() {
            Some(p) if p.sel == sel => p,
            _ => {
                let top_at = |o: usize| doc.top_at(o).unwrap_or(0).min(n - 1);
                let a = top_at(sel.start);
                let b = top_at(sel.end.saturating_sub(1).max(sel.start));
                SelPartial {
                    sel: sel.clone(),
                    a,
                    b,
                    end: 0,
                    from: 0,
                    acc: Counts::default(),
                }
            }
        };
        let (a, b) = (p.a, p.b);
        let ends: &[usize] = if a == b { &[a] } else { &[a, b] };
        let mut read = 0;
        while p.end < ends.len() {
            if read > 0 && read >= budget {
                self.record(read);
                self.sel_partial = Some(p);
                return None;
            }
            let t = ends[p.end];
            let span = doc.top_span(t).unwrap_or_default();
            match self.load(self.counts[t]) {
                Some(c) if p.from == 0 && sel.start <= span.start && span.end <= sel.end => p.acc += c,
                _ => {
                    let left = budget.saturating_sub(read).max(1);
                    let tc = count_top(doc, t, Some(&sel), p.from, left);
                    p.acc += tc.counted.counts;
                    read += tc.counted.read;
                    if let Some(next) = tc.next {
                        p.from = next;
                        self.record(read);
                        self.sel_partial = Some(p);
                        return None;
                    }
                    self.stats.blocks_counted += 1;
                }
            }
            p.end += 1;
            p.from = 0;
        }
        let mut acc = p.acc;
        let (ma, mb) = (a + 1, b);
        if mb > ma {
            // exact, from the cached totals of whole runs of blocks (issue #125)
            acc += self.sum_range(ma..mb);
        }
        self.record(read);
        self.sel = Some((sel, acc));
        Some(Some(acc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_md::{Document, ParseOptions};

    fn md(src: &str) -> Document {
        Document::parse(src, ParseOptions::default())
    }

    fn count(src: &str) -> Counts {
        count_document(&md(src))
    }

    /// Replaces `at` by `text` in `doc` and applies the edit to `c`.
    fn edit(doc: &mut Document, c: &mut DocCounter, at: Range<usize>, text: &str) {
        let ps = markise_doc::PatchSet::new(vec![markise_doc::Patch::verbatim(at, text)]).unwrap();
        let e = FormatDoc::apply(doc, &ps);
        assert!(c.apply(&e), "the counter takes the edit: {e:?}");
    }

    /// The counts of every leaf block as the view model (a full build) lists them: the
    /// counter's document-based blocks give the same total.
    fn count_by_items(src: &str) -> Counts {
        let doc = md(src);
        let model = markise_layout::DocModel::build_with(&doc, None, markise_layout::Paging::Full);
        let mut acc = Counts::default();
        let mut prev = None;
        for (start, it) in model.items.iter_with_start() {
            if prev == Some((start, it.len)) {
                continue; // a later segment of the same block
            }
            prev = Some((start, it.len));
            let kind = match &it.kind {
                markise_layout::ItemKind::Paragraph | markise_layout::ItemKind::Heading { .. } => Kind::Prose,
                markise_layout::ItemKind::Code { .. } => Kind::Code,
                markise_layout::ItemKind::Html => Kind::Html,
                markise_layout::ItemKind::Table => Kind::Table,
                _ => Kind::Nothing,
            };
            acc += count_leaf(&doc, Span::new(start, start + it.len), kind, None).counts;
        }
        acc
    }

    /// Issue #125: the counter counts the document's top-level blocks (not the view model's
    /// items, which a paged model materializes only near the viewport) and gets what counting
    /// every leaf the view shows gets.
    #[test]
    fn document_blocks_count_like_view_items() {
        let mut src = String::from("---\ntitle: x\n---\n\n[r]: /u\n\n");
        for k in 0..40 {
            src.push_str(&format!(
                "# H{k}\n\nParagraph {k} with [r] *five* words.\n\n- item {k}\n- more `c`\n  > quoted {k}\n\n```\ncode {k}\n```\n\n```math\nx\n```\n\n| a | b |\n|---|---|\n| c{k} | d |\n\n<div>html {k}</div>\n\n"
            ));
        }
        src.push_str(&"long line\n".repeat(300));
        assert_eq!(count(&src), count_by_items(&src));
        assert_eq!(count(&src), counted(&src).1.total().unwrap());
    }

    fn counted(src: &str) -> (Document, DocCounter) {
        let doc = md(src);
        let mut c = DocCounter::new();
        c.reset(&doc);
        assert!(c.fill(&doc, usize::MAX));
        (doc, c)
    }

    /// Marker lines of a `:::` block are structure, not words; unpaired they are text.
    #[test]
    fn directive_markers_are_not_counted_and_follow_pairing() {
        assert_eq!(count(":::note extra\n\ntwo words\n\n:::\n").words, 2);
        assert_eq!(count(":::note\n\ntwo words\n").words, 3);
        let (mut doc, mut c) = counted(":::note\n\ntwo words\n");
        assert_eq!(c.total().unwrap().words, 3);
        let end = doc.text().len();
        edit(&mut doc, &mut c, end..end, ":::\n");
        c.fill(&doc, usize::MAX);
        assert_eq!(c.total().unwrap().words, 2);
        assert_eq!(c.total(), Some(count_document(&doc)));
    }

    fn sel(src: &str, a: usize, b: usize) -> Counts {
        let (doc, mut c) = counted(src);
        c.selection(&doc, a..b).unwrap()
    }

    fn words(src: &str) -> u64 {
        count(src).words
    }

    #[test]
    fn cached_slot_is_compact_and_keeps_large_counts_exactly() {
        // issue #29, part 4: one compact slot per block; counts beyond 32 bits (a ~4 GiB
        // block) go out of line instead of being capped
        assert_eq!(std::mem::size_of::<Slot>(), 16);
        let mut d = DocCounter::new();
        let c = Counts {
            words: 3,
            chars: 17,
            chars_no_spaces: 14,
            approx: 1,
        };
        for v in [c, Counts::default()] {
            let s = d.store(v);
            assert_eq!(d.load(s), Some(v));
        }
        assert_eq!(d.load(Slot::NONE), None);
        assert!(d.big.is_empty());
        let huge = Counts {
            words: 1 << 33,
            chars: (1 << 34) + 5,
            ..c
        };
        let edge = Counts {
            chars: Slot::MAX_INLINE + 1,
            ..c
        };
        let (a, b) = (d.store(huge), d.store(edge));
        assert_eq!((d.load(a), d.load(b)), (Some(huge), Some(edge)));
        assert_eq!(d.big.len(), 2);
        d.release(a);
        let a2 = d.store(edge);
        assert_eq!(d.big.len(), 2, "freed entry reused");
        assert_eq!((d.load(a2), d.load(b)), (Some(edge), Some(edge)));
        assert_eq!(
            Slot::inline(Counts {
                chars: Slot::MAX_INLINE,
                ..c
            })
            .map(|s| s.0[1]),
            Some(u32::MAX - 2)
        );
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

    #[test]
    fn counter_matches_full_count_and_recounts_only_changed_blocks() {
        let mut src = String::new();
        for k in 0..300 {
            src.push_str(&format!("Paragraph number {k} with *five* words.\n\n"));
        }
        let mut doc = md(&src);
        let mut c = DocCounter::new();
        c.reset(&doc);
        assert!(c.total().is_none());
        assert!(c.fill(&doc, usize::MAX));
        assert_eq!(c.total(), Some(count_document(&doc)));
        assert_eq!(c.total().unwrap().words, 300 * 6);
        assert!(!c.total().unwrap().is_approx(), "normal documents are exact");
        let before = c.stats().blocks_counted;
        assert_eq!(before, 300);

        // an edit in the middle (and the offsets shifting below it)
        let at = doc.text().find("Paragraph number 150 with").unwrap() + "Paragraph number 150".len();
        edit(&mut doc, &mut c, at..at, " and more");
        assert!(c.fill(&doc, usize::MAX));
        assert_eq!(c.stats().blocks_counted - before, 1, "only the edited paragraph");
        assert_eq!(c.total(), Some(count_document(&doc)));

        // inserting and deleting paragraphs far apart: only those blocks
        let before = c.stats().blocks_counted;
        let at = doc.text().find("Paragraph number 10 ").unwrap();
        edit(&mut doc, &mut c, at..at, "New one.\n\n");
        let at = doc.text().find("Paragraph number 200 with").unwrap();
        let end = at + "Paragraph number 200 with *five* words.\n\n".len();
        edit(&mut doc, &mut c, at..end, "");
        assert!(c.fill(&doc, usize::MAX));
        assert!(
            c.stats().blocks_counted - before <= 4,
            "only the edited blocks: {}",
            c.stats().blocks_counted - before
        );
        assert_eq!(c.total(), Some(count_document(&doc)));
        assert_eq!(c.len(), doc.top_count());
    }

    #[test]
    fn fill_reads_about_its_byte_budget_per_call_and_resumes() {
        let src = "Some words here.\n\n".repeat(20_000);
        let doc = md(&src);
        let mut c = DocCounter::new();
        c.reset(&doc);
        let mut calls = 1;
        while !c.fill(&doc, 4096) {
            calls += 1;
            assert!(calls < 100_000);
        }
        assert!(calls > 10, "a small budget takes several calls: {calls}");
        assert!(c.stats().max_call_bytes <= 4096 + BIG_BLOCK as u64);
        assert_eq!(c.total().unwrap().words, 20_000 * 3);
        // a zero budget still counts one block per call
        let doc = md("One.\n\nTwo.\n");
        let mut c = DocCounter::new();
        c.reset(&doc);
        assert!(!c.fill(&doc, 0));
        assert!(c.fill(&doc, 0));
    }

    /// Issue #125: a top-level container larger than a call's budget (a long list) is counted
    /// over several calls, leaf by leaf, and an edit in it while it is half counted is right.
    #[test]
    fn large_containers_are_counted_across_calls() {
        let src = "- Some words here.\n".repeat(5_000);
        let mut doc = md(&src);
        assert_eq!(doc.top_count(), 1);
        let mut c = DocCounter::new();
        c.reset(&doc);
        let mut calls = 1;
        while !c.fill(&doc, 4096) {
            calls += 1;
            if calls == 5 {
                // half counted: an edit in the list starts it over; one after it shifts it
                edit(&mut doc, &mut c, 2..2, "More ");
            }
            assert!(calls < 100_000);
        }
        assert!(calls > 10, "{calls}");
        assert!(c.stats().max_call_bytes <= 4096 + BIG_BLOCK as u64);
        assert_eq!(c.total(), Some(count_document(&doc)));
        assert_eq!(c.total().unwrap().words, 5_000 * 3 + 1);
        let doc = md(&format!("Before.\n\n{src}"));
        let mut c = DocCounter::new();
        c.reset(&doc);
        assert!(!c.fill(&doc, 4096));
        assert!(!c.fill(&doc, 4096));
        let mut doc = doc;
        edit(&mut doc, &mut c, 0..0, "Two words. ");
        while !c.fill(&doc, 4096) {}
        assert_eq!(c.total(), Some(count_document(&doc)));
    }

    /// Review round 2: a block larger than [`BIG_BLOCK`] (a huge heading, table or code block)
    /// is estimated from its length, not parsed, and marked approximate; so are selections in
    /// it. Normal blocks next to it stay exact.
    #[test]
    fn huge_blocks_are_estimated_and_marked() {
        let body = "lorem ipsum dolor sit amet ".repeat(4_000); // ~108 KB, one line
        let src = format!("# {body}\n\nTwo words.\n");
        let (doc, mut c) = counted(&src);
        let t = c.total().unwrap();
        assert!(t.is_approx());
        let span = doc.leaf(0).unwrap().span.len();
        assert_eq!(t.words, estimate(span).words + 2);
        assert_eq!(t.chars, span as u64 + "Two words.".len() as u64);
        assert_eq!(
            c.stats().bytes_read,
            "Two words.".len() as u64,
            "the huge block is not read"
        );
        // a selection inside it: estimated from the selected bytes
        let s = c.selection(&doc, 2..2 + 600).unwrap();
        assert!(s.is_approx());
        assert_eq!(s.words, 100);
        // the exact paragraph alone
        let a = src.find("Two").unwrap();
        let s = c.selection(&doc, a..a + 3).unwrap();
        assert!(!s.is_approx());
        assert_eq!(s.words, 1);
        // a huge table and a huge code block: estimated without being parsed
        let mut table = String::from("| a | b |\n|---|---|\n");
        while table.len() <= BIG_BLOCK {
            table.push_str("| cell text | more text here |\n");
        }
        let c = count(&table);
        assert!(c.is_approx());
        let code = format!("```\n{}```\n", "let x = 1;\n".repeat(8_000));
        let c = count(&code);
        assert!(c.is_approx());
        assert_eq!(c.words, 0, "code has no words");
        // math is never text, however large
        let math = format!("$$\n{}$$\n", "x^2 + y^2\n".repeat(8_000));
        assert_eq!(count(&math), Counts::default());
    }

    /// Review P2-3: a block split into layout segments is counted as one block (inline context
    /// and soft breaks across segment boundaries are kept).
    #[test]
    fn segmented_blocks_count_as_one_block() {
        let src = "a\n".repeat(130);
        let c = count(&src);
        assert_eq!((c.words, c.chars), (130, 259));
        let (_, k) = counted(&src);
        assert_eq!(k.total().unwrap().chars, 259);
        // strong emphasis spanning the segments: its delimiters are not counted
        let src = format!("**{}**\n", "word\n".repeat(200).trim_end());
        let c = count(&src);
        assert_eq!(c.words, 200);
        assert_eq!(c.chars, (200 * 4 + 199) as u64);
        // an edit in one segment recounts the whole block
        let (mut doc, mut k) = counted(&src);
        let at = src.find("word\nword").unwrap() + "word\nword".len();
        edit(&mut doc, &mut k, at..at, "s");
        assert!(k.fill(&doc, usize::MAX));
        assert_eq!(k.total(), Some(count_document(&doc)));
        assert_eq!(k.total().unwrap().chars, (200 * 4 + 199 + 1) as u64);
    }

    /// Review P2-4: a selection inside a later segment of a large table is counted.
    #[test]
    fn selections_in_later_table_segments_are_counted() {
        let mut src = String::from("| Name | Value |\n|---|---|\n");
        for k in 0..400 {
            src.push_str(&format!("| row{k} | cell{k} |\n"));
        }
        let a = src.find("| row350 |").unwrap() + 2;
        let s = sel(&src, a, a + "row350".len());
        assert_eq!((s.words, s.chars), (1, 6));
        let b = src.find("cell360").unwrap() + "cell360".len();
        assert_eq!(sel(&src, a, b).words, 11 * 2);
    }

    /// Review P2-5 / round 2 P2-7: partial selections cut inline code, code lines and the
    /// text of raw HTML (after its tags are removed, scanned from the block's start).
    #[test]
    fn partial_selections_cut_code_spans_and_html() {
        let src = "x `abcdef` y\n";
        let a = src.find("bcd").unwrap();
        assert_eq!(sel(src, a, a + 3).chars, 3);
        let src = "<div>\nalpha beta gamma\n</div>\n";
        let a = src.find("beta").unwrap();
        let s = sel(src, a, a + 4);
        assert_eq!((s.words, s.chars), (1, 4));
        // a clip starting inside a tag counts none of the tag
        let src = "<p class=\"note\">hello world</p>\n";
        let a = src.find("note").unwrap();
        let b = src.find("world").unwrap();
        assert_eq!(sel(src, a, b).chars_no_spaces, "hello".len() as u64);
        // a comment spanning lines, clip inside it
        let src = "<!-- one\ntwo -->\n<p>three four</p>\n";
        let a = src.find("two").unwrap();
        assert_eq!(sel(src, a, src.len()).words, 2);
        let src = "```\nlet value = 1;\n```\n";
        let a = src.find("value").unwrap();
        assert_eq!(sel(src, a, a + 5).chars, 5);
    }

    /// Review round 3: an edit that only moves blocks (blank lines inserted, no block key
    /// changes) still recomputes a selection of the same byte range.
    #[test]
    fn selection_is_recomputed_after_an_offset_only_edit() {
        let src = "One two.\n\nThree four five.\n";
        let (mut doc, mut c) = counted(src);
        let s = c.selection(&doc, 0..8).unwrap();
        assert_eq!(s.words, 2);
        edit(&mut doc, &mut c, 0..0, "\n\n\n\n\n\n\n\n\n\n");
        assert!(c.fill(&doc, usize::MAX));
        let s = c.selection(&doc, 0..18).unwrap();
        assert_eq!(s.words, 2, "\\n×10 + \"One two.\"");
        let s = c.selection(&doc, 0..8).unwrap();
        assert_eq!(s.words, 0, "only blank lines now");
    }

    /// Review P2-6: adding or removing a reference definition recounts the blocks using it.
    #[test]
    fn reference_definition_edits_recount_their_users() {
        let with = "See [the site][r] now.\n\n[r]: https://example.com\n";
        let (mut doc, mut c) = counted(with);
        assert_eq!(c.total().unwrap().chars, "See the site now.".len() as u64);
        let at = with.find("\n\n[r]").unwrap();
        edit(&mut doc, &mut c, at..with.len(), "\n");
        assert!(c.fill(&doc, usize::MAX));
        assert_eq!(c.total(), Some(count_document(&doc)));
        assert_eq!(c.total().unwrap().chars, "See [the site][r] now.".len() as u64);
        let end = doc.text().len();
        edit(&mut doc, &mut c, end..end, "\n[r]: https://example.com\n");
        assert!(c.fill(&doc, usize::MAX));
        assert_eq!(c.total().unwrap().chars, "See the site now.".len() as u64);
    }

    /// Review of #129 (P2): a reference definition removed while a large container that
    /// uses it is counted in part: the leaves counted so far are counted again, so the total
    /// does not mix the old and the new meaning of the reference.
    #[test]
    fn definition_edits_during_a_partial_count_recount_the_part() {
        let mut src = "- see ![alt text][r] here\n".repeat(3_000);
        src.push_str("\nA paragraph between.\n\n[r]: /img.png\n");
        let mut doc = md(&src);
        let mut c = DocCounter::new();
        c.reset(&doc);
        assert!(!c.fill(&doc, 4096));
        assert!(c.partial.is_some_and(|p| p.2.refs), "the list is counted in part");
        let at = doc.text().find("[r]:").unwrap();
        let end = doc.text().len();
        let before = count_document(&doc);
        let ps = markise_doc::PatchSet::new(vec![markise_doc::Patch::verbatim(at..end, "")]).unwrap();
        let e = FormatDoc::apply(&mut doc, &ps);
        assert_ne!(before, count_document(&doc), "the reference changes the list's text");
        assert_eq!(e.replaced_top.start, 2, "only the definition is re-parsed: {e:?}");
        assert!(c.apply(&e));
        while !c.fill(&doc, 4096) {}
        assert_eq!(c.total(), Some(count_document(&doc)));
        // the definition back, again in the middle of the count
        c.reset(&doc);
        assert!(!c.fill(&doc, 4096));
        let end = doc.text().len();
        edit(&mut doc, &mut c, end..end, "[r]: /img.png\n");
        while !c.fill(&doc, 4096) {}
        assert_eq!(c.total(), Some(count_document(&doc)));
    }

    /// Review P2-8: entities in raw HTML count as the characters they show.
    #[test]
    fn html_entities_are_decoded() {
        assert_eq!(count("<p>a &amp; b &#233; &lt;</p>\n").chars, 9, "a & b é <");
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
        let (doc, mut c) = counted(src);
        assert_eq!(c.selection(&doc, 0..src.len()), c.total());
        let s = sel(src, a, a + 2);
        assert_eq!((s.words, s.chars), (1, 2));
        // markers inside the selection are not counted
        let src = "Some **bold** text.\n";
        assert_eq!(sel(src, 0, src.len() - 1).chars, "Some bold text.".len() as u64);
    }

    /// Issue #132: the status bar counts small selections directly ([`count_range`]); it gets
    /// what the cached counter gets for the same range.
    #[test]
    fn direct_range_counts_match_the_counter() {
        let mut src = String::from("[r]: /u\n\n");
        for k in 0..12 {
            src.push_str(&format!(
                "# H{k}\n\nPara {k} with [r] *five* words.\n\n- item {k}\n- more `c`\n  > quoted {k}\n\n```\ncode {k}\n```\n\n| a | b |\n|---|---|\n| c{k} | d |\n\n<div>html {k}</div>\n\n"
            ));
        }
        let (doc, mut c) = counted(&src);
        assert_eq!(count_range(&doc, 0..src.len()), count_document(&doc));
        let mut x = 7usize;
        for _ in 0..300 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let a = (x >> 33) % src.len();
            let b = a + (x >> 13) % 400;
            let (a, b) = (a, b.min(src.len()));
            if !src.is_char_boundary(a) || !src.is_char_boundary(b) {
                continue;
            }
            assert_eq!(count_range(&doc, a..b), c.selection(&doc, a..b).unwrap(), "{a}..{b}");
        }
    }

    /// Issue #125 (phase 4): selections of any size are exact (a middle of tens of thousands of
    /// blocks too), summed from cached totals; edits keep the totals right.
    #[test]
    fn large_selections_are_exact() {
        let n = 5 * SUM_BLOCKS + 37;
        let src = "One two three.\n\n".repeat(n);
        let mut doc = md(&src);
        let mut c = DocCounter::new();
        c.reset(&doc);
        assert_eq!(c.selection(&doc, 0..10), None, "needs the document counted");
        assert!(c.fill(&doc, usize::MAX));
        let before = c.stats().blocks_counted;
        // almost everything
        let s = c.selection(&doc, 4..src.len() - 9).unwrap();
        assert!(!s.is_approx());
        assert_eq!(s.words, n as u64 * 3 - 2);
        assert_eq!(
            c.stats().blocks_counted - before,
            2,
            "only the two partial blocks are read"
        );
        // the middle half: exact
        let q = src.len() / 4;
        let (a, b) = (q - q % 16, 3 * q - (3 * q) % 16);
        let s = c.selection(&doc, a..b).unwrap();
        assert!(!s.is_approx());
        assert_eq!(s, count_range(&doc, a..b));
        // after edits (a block gained a word, blocks came and went), still exact
        edit(&mut doc, &mut c, 16 * 700..16 * 700, "Four ");
        edit(&mut doc, &mut c, 16 * 3000..16 * 3000, "New.\n\n");
        edit(&mut doc, &mut c, 16 * 10..16 * 12, "");
        assert!(c.fill(&doc, usize::MAX));
        let len = doc.text().len();
        for (a, b) in [(4, len - 9), (16 * 5, len / 2), (len / 3, len - 100)] {
            let s = c.selection(&doc, a..b).unwrap();
            assert_eq!(s, count_range(&doc, a..b), "{a}..{b}");
        }
    }
}
