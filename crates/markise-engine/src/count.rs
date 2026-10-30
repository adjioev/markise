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
//! - Raw HTML blocks count the text between their tags.
//!
//! **Units.** *Characters* are extended grapheme clusters (an emoji with modifiers is one
//! character), line breaks excluded; a soft line break inside a paragraph shows as a space
//! and counts as one. *Characters without spaces* also drop whitespace. *Words* follow
//! Unicode word segmentation (UAX #29, `unicode-segmentation`): a word is a segment with a
//! letter or digit; each Han, Hiragana, Katakana or Bopomofo character counts as one word
//! (those scripts do not separate words with spaces), while Hangul words are space separated
//! and count normally. Emoji and punctuation are not words.
//!
//! **Incremental counting.** [`DocCounter`] keeps one [`Counts`] per view-model item keyed by
//! the item's offset-independent content hash. [`DocCounter::reconcile`] matches the current
//! model against the last one (common prefix and suffix of keys) and forgets only the items in
//! between; [`DocCounter::fill`] counts forgotten items within a time budget, so typing never
//! recounts the whole document and a huge document is counted in bounded slices. Reference
//! definitions are a document-level input that does not re-key items: a reference link whose
//! definition appears or disappears keeps its previous count until the item itself changes.

use std::ops::{Add, AddAssign, Range, Sub, SubAssign};
use std::time::{Duration, Instant};

use markise_doc::{CriticKind, FormatDoc, InlineKind, InlineNode, InlineSource, RichInline, Span};
use markise_layout::DocModel;
use markise_layout::model::{ItemKind, Segment};
use unicode_segmentation::UnicodeSegmentation;

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

/// Appends the rendered text of `nodes` (offsets relative to `base`) to `out`; with `clip`
/// (absolute), only the nodes overlapping it (plain text cut to it).
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
            InlineKind::Text => {
                let v = r.value(n);
                match clip {
                    // plain text maps byte for byte onto its source: cut it to the clip
                    Some(c) if v.len() == e - s && (s < c.start || e > c.end) => {
                        let a = c.start.max(s) - s;
                        let b = c.end.min(e) - s;
                        out.push_str(v.get(a..b).unwrap_or(v));
                    }
                    _ => out.push_str(v),
                }
            }
            _ => out.push_str(r.value(n)),
        }
    }
}

/// The text of a raw HTML block without its tags and comments.
fn strip_tags(src: &str, out: &mut String) {
    let mut rest = src;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let tagish = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if !tagish {
            out.push('<');
            rest = after;
            continue;
        }
        let end = if after.starts_with("!--") {
            after.find("-->").map(|k| k + 3)
        } else {
            after.find('>').map(|k| k + 1)
        };
        match end {
            Some(k) => {
                // a tag separates words like a space would (`<br>`, `</p><p>`)
                out.push('\n');
                rest = &after[k..];
            }
            None => return,
        }
    }
    out.push_str(rest);
}

/// Counts of view-model item `i` of `model` (built for `doc`); with `clip` (absolute byte
/// range), only the part of the item inside it.
pub fn count_item(doc: &dyn FormatDoc, model: &DocModel, i: usize, clip: Option<&Range<usize>>) -> Counts {
    let Some(it) = model.items.get(i) else {
        return Counts::default();
    };
    let overlaps = |sp: &Span| {
        clip.is_none_or(|c| sp.start < c.end && sp.end > c.start || (sp.start == sp.end && c.contains(&sp.start)))
    };
    let text = doc.text();
    let mut out = String::new();
    match &it.kind {
        ItemKind::Paragraph | ItemKind::Heading { .. } => {
            let lines = model.item_lines(doc, i);
            if lines.is_empty() || !lines.iter().any(overlaps) {
                return Counts::default();
            }
            let base = lines[0].start;
            let r = doc.inline(InlineSource::Lines(&lines), base);
            rendered(&r, &r.nodes, base, clip, &mut out);
            Counts::of_text(&out)
        }
        ItemKind::Code { .. } => {
            for l in model.item_lines(doc, i).iter().filter(|l| overlaps(l)) {
                let (a, b) = match clip {
                    Some(c) => (c.start.max(l.start), c.end.min(l.end)),
                    None => (l.start, l.end),
                };
                out.push_str(text.get(a..b.max(a)).unwrap_or(""));
                out.push('\n');
            }
            Counts::of_code(&out)
        }
        ItemKind::Html => {
            let mut src = String::new();
            for l in model.item_lines(doc, i).iter().filter(|l| overlaps(l)) {
                src.push_str(l.slice(text));
                src.push('\n');
            }
            strip_tags(&src, &mut out);
            Counts::of_text(&out)
        }
        ItemKind::Table => {
            let Some(t) = doc.table(model.item_start(i)) else {
                return Counts::default();
            };
            let n = t.rows.len();
            // rows: 0 = header, 1 = delimiter row, 2.. = body (as the layout draws them)
            let (header, body) = match &it.segment {
                Some(Segment::Rows { start, end, first, .. }) => {
                    (*first, (*start as usize).max(2)..(*end as usize).min(n))
                }
                _ => (true, 2.min(n)..n),
            };
            let rows = header.then_some(0).into_iter().filter(|_| n > 0).chain(body);
            for row in rows {
                for cell in doc.table_row_cells(t.rows[row]) {
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
        ItemKind::Math
        | ItemKind::Diagram { .. }
        | ItemKind::Rule
        | ItemKind::FrontMatter
        | ItemKind::Toc
        | ItemKind::Hidden
        | ItemKind::Empty => Counts::default(),
    }
}

/// Counts of the whole document, item by item (no caching; tests and tools).
pub fn count_document(doc: &dyn FormatDoc, model: &DocModel) -> Counts {
    (0..model.items.len()).fold(Counts::default(), |acc, i| acc + count_item(doc, model, i, None))
}

/// Item key: offset-independent content hash and length.
pub type ItemKey = (u64, usize);

fn item_key(it: &markise_layout::model::Item) -> ItemKey {
    (it.content_hash, it.len)
}

/// Every item's key and counts, in order (a worker counting a whole document; the result
/// goes to [`DocCounter::absorb`]).
pub fn count_all(doc: &dyn FormatDoc, model: &DocModel) -> Vec<(ItemKey, Counts)> {
    model
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| (item_key(it), count_item(doc, model, i, None)))
        .collect()
}

/// Statistics of a [`DocCounter`] (tests prove that typing does not recount everything).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterStats {
    /// Items counted so far by this counter's caller thread ([`DocCounter::fill`] and
    /// selections; not the items taken from [`DocCounter::absorb`]).
    pub items_counted: u64,
    /// Items taken from [`DocCounter::absorb`].
    pub items_absorbed: u64,
    /// [`DocCounter::reconcile`] calls that found a change.
    pub reconciles: u64,
}

/// Incremental document counts (see the module docs): one cached [`Counts`] per view-model
/// item, matched to the model by content hash.
#[derive(Debug, Default)]
pub struct DocCounter {
    /// `(content hash, length)` of each item of the last reconciled model.
    keys: Vec<(u64, usize)>,
    counts: Vec<Option<Counts>>,
    /// Sum of the known counts.
    known: Counts,
    /// Items not counted yet.
    missing: usize,
    /// Every item before it is counted.
    cursor: usize,
    /// Prefix sums of `counts` (selection counts), built when complete and needed.
    prefix: Option<Vec<Counts>>,
    stats: CounterStats,
}

/// A selection spanning at most this many items is counted directly, even before the
/// document's counts are complete.
const DIRECT_ITEMS: usize = 64;

impl DocCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> CounterStats {
        self.stats
    }

    /// Forgets everything (another document).
    pub fn clear(&mut self) {
        let stats = self.stats;
        *self = DocCounter {
            stats,
            ..Default::default()
        };
    }

    /// Matches `model` against the model of the last call: items of the common prefix and
    /// suffix (same content hash and length) keep their counts, the others are forgotten.
    /// O(items) comparisons of two integers, no text is read.
    pub fn reconcile(&mut self, model: &DocModel) {
        let items = &model.items;
        let n = items.len();
        let old = self.keys.len();
        let key = item_key;
        let mut pre = 0;
        for (it, k) in items.iter().zip(&self.keys) {
            if key(it) != *k {
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
            if suf >= max_suf || key(it) != *k {
                break;
            }
            suf += 1;
        }
        self.stats.reconciles += 1;
        for c in self.counts.drain(pre..old - suf) {
            match c {
                Some(c) => self.known -= c,
                None => self.missing -= 1,
            }
        }
        let fresh = n - suf - pre;
        self.counts.splice(pre..pre, std::iter::repeat_n(None, fresh));
        self.missing += fresh;
        let new_keys: Vec<(u64, usize)> = items.iter().skip(pre).take(fresh).map(key).collect();
        self.keys.splice(pre..old - suf, new_keys);
        self.cursor = self.cursor.min(pre);
        self.prefix = None;
        debug_assert_eq!(self.keys.len(), n);
        debug_assert_eq!(self.counts.len(), n);
    }

    /// Items not counted yet.
    pub fn missing(&self) -> usize {
        self.missing
    }

    /// Takes counts made elsewhere (a worker's [`count_all`] of an earlier or the current
    /// state of the document): items of the common prefix and suffix of keys that are not
    /// counted yet get them. Items in between stay missing.
    pub fn absorb(&mut self, done: &[(ItemKey, Counts)]) {
        let n = self.keys.len();
        let take = |me: &mut Self, i: usize, c: Counts| {
            if me.counts[i].is_none() {
                me.counts[i] = Some(c);
                me.known += c;
                me.missing -= 1;
                me.stats.items_absorbed += 1;
            }
        };
        let mut pre = 0;
        while pre < n && pre < done.len() && self.keys[pre] == done[pre].0 {
            take(self, pre, done[pre].1);
            pre += 1;
        }
        let mut suf = 0;
        while suf < n - pre && suf < done.len() - pre && self.keys[n - 1 - suf] == done[done.len() - 1 - suf].0 {
            take(self, n - 1 - suf, done[done.len() - 1 - suf].1);
            suf += 1;
        }
        self.prefix = None;
    }

    /// Counts forgotten items of `model` (reconciled, built for `doc`) until `budget` is
    /// spent; `true` when every item is counted.
    pub fn fill(&mut self, doc: &dyn FormatDoc, model: &DocModel, budget: Duration) -> bool {
        let t = Instant::now();
        let n = self.counts.len().min(model.items.len());
        let mut done = 0usize;
        while self.missing > 0 && self.cursor < n {
            if self.counts[self.cursor].is_none() {
                // check the clock every few items (an item is at most a few KB)
                if done % 8 == 7 && t.elapsed() >= budget {
                    return false;
                }
                let c = count_item(doc, model, self.cursor, None);
                self.counts[self.cursor] = Some(c);
                self.known += c;
                self.missing -= 1;
                self.stats.items_counted += 1;
                done += 1;
            }
            self.cursor += 1;
        }
        self.missing == 0
    }

    /// The document's counts once every item is counted.
    pub fn total(&self) -> Option<Counts> {
        (self.missing == 0).then_some(self.known)
    }

    /// Counts of the source byte range `sel` of the document (reconciled `model` built for
    /// `doc`): whole items from the cache, the partial items at both ends counted directly.
    /// `None` while the counts of a large selection are not complete.
    pub fn selection(&mut self, doc: &dyn FormatDoc, model: &DocModel, sel: Range<usize>) -> Option<Counts> {
        if model.items.is_empty() || sel.is_empty() {
            return Some(Counts::default());
        }
        let a = model.item_at_offset(sel.start);
        let b = model.item_at_offset(sel.end.saturating_sub(1).max(sel.start));
        let within = |i: usize| {
            let s = model.item_span(i);
            sel.start <= s.start && s.end <= sel.end
        };
        let direct = |me: &mut Self, i: usize| {
            me.stats.items_counted += 1;
            if within(i) {
                count_item(doc, model, i, None)
            } else {
                count_item(doc, model, i, Some(&sel))
            }
        };
        if b < a + DIRECT_ITEMS || self.counts.len() != model.items.len() {
            if b >= a + DIRECT_ITEMS {
                return None;
            }
            return Some((a..=b).fold(Counts::default(), |acc, i| acc + direct(self, i)));
        }
        self.total()?;
        let prefix = self.prefix.get_or_insert_with(|| {
            let mut p = Vec::with_capacity(self.counts.len() + 1);
            let mut acc = Counts::default();
            p.push(acc);
            for c in &self.counts {
                acc += c.unwrap_or_default();
                p.push(acc);
            }
            p
        });
        // items a+1 ..= b-1 lie inside the selection
        let middle = prefix[b] - prefix[a + 1];
        Some(middle + direct(self, a) + direct(self, b))
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

    #[test]
    fn counter_matches_full_count_and_recounts_only_changed_items() {
        let mut src = String::new();
        for k in 0..300 {
            src.push_str(&format!("Paragraph number {k} with *five* words.\n\n"));
        }
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        assert!(c.total().is_none());
        assert!(c.fill(&doc, &model, Duration::from_secs(10)));
        assert_eq!(c.total(), Some(count_document(&doc, &model)));
        assert_eq!(c.total().unwrap().words, 300 * 6);
        let before = c.stats().items_counted;
        assert_eq!(before, 300);

        // an edit in the middle (and the model's offsets shifting below it)
        let edited = src.replacen("Paragraph number 150 with", "Paragraph number 150 and more with", 1);
        let (doc2, model2) = md(&edited);
        c.reconcile(&model2);
        assert!(c.fill(&doc2, &model2, Duration::from_secs(10)));
        assert_eq!(c.stats().items_counted - before, 1, "only the edited paragraph");
        assert_eq!(c.total(), Some(count_document(&doc2, &model2)));

        // inserting and deleting paragraphs
        let edited = edited.replacen("Paragraph number 10 ", "New one.\n\nParagraph number 10 ", 1);
        let edited = edited.replacen("Paragraph number 200 with *five* words.\n\n", "", 1);
        let (doc3, model3) = md(&edited);
        let before = c.stats().items_counted;
        c.reconcile(&model3);
        assert!(c.fill(&doc3, &model3, Duration::from_secs(10)));
        assert!(
            c.stats().items_counted - before <= 200,
            "only the items between the two edits"
        );
        assert_eq!(c.total(), Some(count_document(&doc3, &model3)));

        // unchanged model: nothing to do
        let r = c.stats().reconciles;
        c.reconcile(&model3);
        assert_eq!(c.stats().reconciles, r);
    }

    #[test]
    fn absorb_takes_a_workers_counts_where_the_keys_still_match() {
        let src: String = (0..100).map(|k| format!("Word {k}.\n\n")).collect();
        let (doc, model) = md(&src);
        let all = count_all(&doc, &model);
        // the document changed in the middle since the worker's snapshot
        let edited = src.replacen("Word 50.", "Word 50 two.", 1);
        let (doc2, model2) = md(&edited);
        let mut c = DocCounter::new();
        c.reconcile(&model2);
        c.absorb(&all);
        assert_eq!(c.missing(), 1, "the changed paragraph");
        assert_eq!(c.stats().items_absorbed, 99);
        assert!(c.fill(&doc2, &model2, Duration::from_secs(10)));
        assert_eq!(c.stats().items_counted, 1);
        assert_eq!(c.total(), Some(count_document(&doc2, &model2)));
    }

    #[test]
    fn fill_respects_its_budget_and_resumes() {
        let src = "Some words here.\n\n".repeat(2000);
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        let mut rounds = 0;
        while !c.fill(&doc, &model, Duration::ZERO) {
            rounds += 1;
            assert!(rounds < 10_000);
        }
        assert!(rounds > 1, "a zero budget counts a few items per call");
        assert_eq!(c.total().unwrap().words, 2000 * 3);
    }

    #[test]
    fn selection_counts() {
        let src = "Alpha beta gamma.\n\nDelta epsilon.\n\n```\ncode here\n```\n\nZeta eta theta iota.\n";
        let (doc, model) = md(src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        c.fill(&doc, &model, Duration::from_secs(10));
        // "beta gamma." … "Zeta eta"
        let a = src.find("beta").unwrap();
        let b = src.find("eta theta").unwrap() + 3;
        let s = c.selection(&doc, &model, a..b).unwrap();
        assert_eq!(s.words, 2 + 2 + 2);
        // the whole document
        assert_eq!(c.selection(&doc, &model, 0..src.len()), c.total());
        // inside one word
        let s = c.selection(&doc, &model, a..a + 2).unwrap();
        assert_eq!((s.words, s.chars), (1, 2));
        // markers inside the selection are not counted
        let src = "Some **bold** text.\n";
        let (doc, model) = md(src);
        let s = DocCounter::new().selection(&doc, &model, 0..src.len() - 1).unwrap();
        assert_eq!(s.chars, "Some bold text.".len() as u64);
    }

    #[test]
    fn large_selection_uses_prefix_sums() {
        let src = "One two three.\n\n".repeat(500);
        let (doc, model) = md(&src);
        let mut c = DocCounter::new();
        c.reconcile(&model);
        assert_eq!(c.selection(&doc, &model, 0..src.len()), None, "not counted yet");
        c.fill(&doc, &model, Duration::from_secs(10));
        let before = c.stats().items_counted;
        let s = c.selection(&doc, &model, 4..src.len() - 9).unwrap();
        assert_eq!(s.words, 500 * 3 - 2);
        assert_eq!(c.stats().items_counted - before, 2, "only the two partial items");
    }
}
