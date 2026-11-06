//! Sections of a document (ADR 0026, a prototype of a draft syntax): a root-level thematic break
//! written exactly `---` starts a new section, and a `:::section-metadata` block first in a
//! section holds its `key: value` lines.
//!
//! Like [`crate::directive`], this is a document-wide index in chunked relative-offset lists, kept
//! by the edits' re-parsed regions and independent of any view model. It always stands, whatever
//! the document's dialect: whether a boundary means anything is the reader's to say, and asking
//! costs nothing a document without sections would notice.
//!
//! Whether a rule stands inside a directive depends on pairs that can change far away, so both
//! kinds of rule are kept, in two lists. An edit touching no rule shifts the later ones in
//! O(chunks), and the position of a boundary in its list is the section's number, so that the
//! section around a position is O(log rules). Only a change of pairs reads rules again, and only
//! those in its range.

use std::ops::Range;

use markise_doc::chunked::{RelChunks, Relative};
use markise_doc::{FlaggedLine, SectionAt, SectionMetadata, SectionPair};

use crate::block::{Block, BlockKind};
use crate::text::{Span, line_end};
use crate::{Document, line_start};

/// The name of the block that holds a section's metadata.
pub const SECTION_METADATA: &str = "section-metadata";

/// A metadata block larger than this, markers included, is not read: it is asked about on the UI
/// thread.
pub const MAX_METADATA_BYTES: usize = 64 * 1024;

/// Values longer than this, in characters, are flagged.
const MAX_VALUE_CHARS: usize = 200;
/// Keys beyond this many are flagged.
const MAX_KEYS: usize = 20;
/// How much of a flagged line is kept to show.
const FLAGGED_TEXT_CHARS: usize = 80;
/// Top-level blocks of a section looked through for its first heading.
const HEADING_SCAN_BLOCKS: usize = 512;
/// A marker line longer than this is not read as one.
const MARKER_LINE_BYTES: usize = 4096;

/// A marker with its lines right next to it.
pub const MALFORMED_TIGHT: &str = "Malformed: a marker stands apart, with a blank line between it and the lines";
/// Anything but one paragraph between the markers.
pub const MALFORMED_BODY: &str =
    "Malformed: between the markers there is one paragraph of key: value lines and nothing else";

#[derive(Clone, Copy, Debug)]
struct Rule {
    at: usize,
}

impl Relative for Rule {
    fn origin(&self) -> usize {
        self.at
    }
    fn shift(&mut self, delta: isize) {
        self.at = (self.at as isize + delta) as usize;
    }
}

/// See the module docs.
#[derive(Clone, Debug, Default)]
pub(crate) struct SectionIndex {
    /// Root-level rules outside any directive: the boundaries, in order.
    boundaries: RelChunks<Rule>,
    /// Root-level rules inside a paired directive: lines, for as long as the pair stands.
    inside: RelChunks<Rule>,
}

/// The rules among these top-level blocks (absolute spans) whose line is exactly `---`.
pub(crate) fn exact_rules(text: &str, blocks: &[Block]) -> Vec<usize> {
    let exact = |b: &Block| {
        matches!(b.kind, BlockKind::ThematicBreak) && {
            let (end, _) = line_end(text, b.span.start);
            line_start(text, b.span.start) == b.span.start && &text.as_bytes()[b.span.start..end] == b"---"
        }
    };
    blocks.iter().filter(|b| exact(b)).map(|b| b.span.start).collect()
}

/// Replaces the rules of `list` in `old` (old coordinates) with `new` (new coordinates).
fn splice_list(list: &mut RelChunks<Rule>, old: &Range<usize>, new: Vec<Rule>, delta: isize) {
    if list.is_empty() && new.is_empty() {
        return;
    }
    let a = list.partition_point(|r| r.origin < old.start);
    let z = list.partition_point(|r| r.origin < old.end);
    list.splice(a..z, new, delta);
}

impl SectionIndex {
    /// Shifts the rules at or after `pos` by `delta` (an edit before them that touched none).
    pub(crate) fn shift_from(&mut self, pos: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        for list in [&mut self.boundaries, &mut self.inside] {
            if !list.is_empty() {
                let i = list.partition_point(|r| r.origin < pos);
                list.splice(i..i, Vec::new(), delta);
            }
        }
    }
}

impl Document {
    fn divide_rules(&self, rules: Vec<usize>) -> (Vec<Rule>, Vec<Rule>) {
        let (root, inside): (Vec<usize>, Vec<usize>) =
            rules.into_iter().partition(|&at| self.directive_depth_at(at) == 0);
        let to_rules = |v: Vec<usize>| v.into_iter().map(|at| Rule { at }).collect();
        (to_rules(root), to_rules(inside))
    }

    /// Builds the index after a whole-document parse: the directives are in place.
    pub(crate) fn build_sections(&mut self, rules: Vec<usize>) {
        let (root, inside) = self.divide_rules(rules);
        self.sections = SectionIndex {
            boundaries: RelChunks::from_abs_vec(root),
            inside: RelChunks::from_abs_vec(inside),
        };
    }

    /// Takes over the rules of a re-parsed region; the directives are in place. `old` is the
    /// replaced byte range in old coordinates, `pairs` the range (new coordinates) where pairs
    /// changed: only rules in it, or in the region itself, can have come in or out of a directive.
    pub(crate) fn splice_sections(
        &mut self,
        old: Range<usize>,
        rules: Vec<usize>,
        delta: isize,
        pairs: Option<Range<usize>>,
    ) {
        let (root, inside) = self.divide_rules(rules);
        splice_list(&mut self.sections.boundaries, &old, root, delta);
        splice_list(&mut self.sections.inside, &old, inside, delta);
        let Some(pairs) = pairs else { return };
        let old_end = old.end.checked_add_signed(delta).unwrap_or(old.end);
        let range = pairs.start.min(old.start)..pairs.end.max(old_end).saturating_add(1);
        let mut rules: Vec<usize> = Vec::new();
        for list in [&self.sections.boundaries, &self.sections.inside] {
            let a = list.partition_point(|r| r.origin < range.start);
            let z = list.partition_point(|r| r.origin < range.end);
            rules.extend(list.range(a..z).map(|r| r.origin));
        }
        rules.sort_unstable();
        let (root, inside) = self.divide_rules(rules);
        splice_list(&mut self.sections.boundaries, &range, root, 0);
        splice_list(&mut self.sections.inside, &range, inside, 0);
    }

    /// Where the first section starts: after the front matter, whose lines are no boundaries.
    fn first_section_start(&self) -> usize {
        match self.top(0) {
            Some(t) if matches!(t.block.kind, BlockKind::FrontMatter { .. }) => t.span().end,
            _ => 0,
        }
    }

    /// Number of sections: a document without a boundary has one, even an empty document.
    pub fn section_count(&self) -> usize {
        self.sections.boundaries.len() + 1
    }

    /// Section `index`, counting from 0. O(log rules).
    pub fn section(&self, index: usize) -> Option<SectionAt> {
        let list = &self.sections.boundaries;
        let (boundary, start) = match index.checked_sub(1) {
            None => (None, self.first_section_start()),
            Some(i) => {
                let at = list.origin(i)?;
                (Some(Span::new(at, at + 3)), at + 3)
            }
        };
        Some(SectionAt {
            index,
            boundary,
            start,
            end: list.origin(index).unwrap_or(self.text().len()),
        })
    }

    /// The section around `at`; a boundary's line belongs to the section it opens. O(log rules).
    pub fn section_at(&self, at: usize) -> SectionAt {
        let index = self.sections.boundaries.partition_point(|r| r.origin <= at);
        self.section(index)
            .expect("the index counts the boundaries before `at`")
    }

    /// The index of the first top-level block of section `index`, if it has one.
    fn section_first_top(&self, section: &SectionAt) -> Option<usize> {
        let first = self.top.partition_point(|t| t.start() < section.start);
        self.top.get(first).filter(|t| t.start() < section.end).map(|_| first)
    }

    /// `true` when `t` opens a paired block of the metadata name.
    fn opens_metadata(&self, t: &crate::Top<'_>) -> bool {
        match &t.block.kind {
            BlockKind::DirectiveOpen { name, .. } => t.abs(*name).slice(self.text()) == SECTION_METADATA,
            _ => false,
        }
    }

    /// What the metadata block of section `index` says, when one is the first block of the
    /// section. O(log blocks + log markers + the block's size); a block over
    /// [`MAX_METADATA_BYTES`] is not read and counts as none: that is decided from the index, before
    /// any of its lines is.
    pub fn section_metadata(&self, index: usize) -> Option<SectionMetadata> {
        let section = self.section(index)?;
        let first = self.section_first_top(&section)?;
        let block = self.top.get(first)?;
        if !self.opens_metadata(&block) || self.paired_extent(block.start()).is_none_or(|n| n > MAX_METADATA_BYTES) {
            return None;
        }
        let d = self.directives_at(block.start()).into_iter().next()?;
        let text = self.text();
        let (_, body) = line_end(text, d.opener.start);
        let end = line_start(text, d.closer.start);
        let mut parsed = read_metadata(text, body..end);
        let closer = self.top.partition_point(|t| t.start() < d.closer.start);
        let inside: Vec<_> = (first + 1..closer).filter_map(|i| self.top.get(i)).collect();
        let shape = match inside.as_slice() {
            [one] if matches!(one.block.kind, BlockKind::Paragraph) => {
                let all = &text[body..end];
                // the last line: the terminator before the closer's line is not a line
                let all_but = all
                    .strip_suffix("\r\n")
                    .or_else(|| all.strip_suffix(['\n', '\r']))
                    .unwrap_or(all);
                let blank = |l: Option<&str>| l.is_none_or(|l| l.trim().is_empty());
                (!blank(all.lines().next()) || !blank(all_but.rsplit(['\n', '\r']).next())).then_some(MALFORMED_TIGHT)
            }
            _ => Some(MALFORMED_BODY),
        };
        if let Some(reason) = shape {
            parsed.flagged.insert(
                0,
                FlaggedLine {
                    at: d.opener.start,
                    text: clipped(&text[d.opener.start..d.opener.end]),
                    reason,
                },
            );
        }
        if !d.args.is_empty() {
            parsed.flagged.insert(
                0,
                FlaggedLine {
                    at: d.opener.start,
                    text: clipped(&text[d.opener.start..d.opener.end]),
                    reason: "This block takes no arguments",
                },
            );
        }
        Some(SectionMetadata {
            opener: d.opener,
            closer: d.closer,
            ..parsed
        })
    }

    /// A block of the metadata name that opens at `opener` and is not the first block of its section.
    pub fn section_metadata_misplaced(&self, opener: usize) -> bool {
        let t = self.top.partition_point(|t| t.start() < opener);
        if !self
            .top
            .get(t)
            .is_some_and(|b| b.start() == opener && self.opens_metadata(&b))
        {
            return false;
        }
        let section = self.section_at(opener);
        self.section_first_top(&section) != Some(t)
    }

    /// The first block of section `index` is a metadata opener that pairs with nothing, as when a fence
    /// inside it took its closer: where it is.
    pub fn section_metadata_unclosed(&self, index: usize) -> Option<Span> {
        let section = self.section(index)?;
        let first = self.section_first_top(&section)?;
        let t = self.top.get(first)?;
        let span = t.span();
        if !matches!(t.block.kind, BlockKind::Paragraph) || span.len() > MARKER_LINE_BYTES {
            return None;
        }
        let (end, _) = line_end(self.text(), span.start);
        let marker = crate::scan::directive_marker(self.text(), span.start, end)?;
        (marker.name.slice(self.text()) == SECTION_METADATA).then_some(span)
    }

    /// Where the first heading of section `index` starts, among its top-level blocks and not inside a
    /// block. Looks through a bounded number of them, as this is asked on the UI thread: a section
    /// whose first heading lies further on has none as far as this goes.
    pub fn section_first_heading(&self, index: usize) -> Option<usize> {
        let section = self.section(index)?;
        let mut i = self.section_first_top(&section)?;
        for _ in 0..HEADING_SCAN_BLOCKS {
            let t = self.top.get(i).filter(|t| t.start() < section.end)?;
            match t.block.kind {
                BlockKind::Heading { .. } => return Some(t.start()),
                BlockKind::DirectiveOpen { .. } => {
                    let closer = self.directive_closer_at(t.start())?;
                    i = self.top.partition_point(|b| b.start() <= closer);
                }
                _ => i += 1,
            }
        }
        None
    }

    /// A rule at `at`, at the root and outside any block, written with dashes but not as a boundary:
    /// `----`, `- - -`, ` ---`, `--- `.
    pub fn section_near_boundary(&self, at: usize) -> bool {
        let t = self.top.partition_point(|t| t.start() < at);
        let Some(block) = self.top.get(t).filter(|b| b.start() == at) else {
            return false;
        };
        matches!(block.block.kind, BlockKind::ThematicBreak)
            && self.text().as_bytes().get(at) == Some(&b'-')
            && self.directive_depth_at(at) == 0
            && self.section_at(at).boundary.is_none_or(|b| b.start != at)
    }

    /// The `authoring` of the front matter, as written: empty when the key has no value, which is
    /// not the same as no key. Reading it costs the front matter once: an edit that does not reach
    /// the first block keeps the answer.
    pub fn authoring(&self) -> Option<&str> {
        let read = || crate::frontmatter::get(&crate::frontmatter::front_matter(self), "authoring").map(str::to_string);
        self.authoring.get_or_init(read).as_deref()
    }
}

fn clipped(line: &str) -> String {
    line.chars().take(FLAGGED_TEXT_CHARS).collect()
}

fn is_key(key: &str) -> bool {
    let mut b = key.bytes();
    b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// The body of a metadata block, by lines of `range`. This is its own small grammar, not Markdown
/// and not the block field grammar: `key: value`, split at the first colon, both trimmed, values
/// literal. What does not fit is kept to show, and takes nothing else with it. Blank lines say
/// nothing. Spans of the result's markers are left to the caller.
fn read_metadata(text: &str, range: Range<usize>) -> SectionMetadata {
    let mut out = SectionMetadata {
        opener: Span::new(0, 0),
        closer: Span::new(0, 0),
        pairs: Vec::new(),
        flagged: Vec::new(),
    };
    let mut pos = range.start;
    while pos < range.end {
        let (end, next) = line_end(text, pos);
        let line = &text[pos..end];
        let at = pos;
        pos = next.max(pos + 1);
        if line.trim().is_empty() {
            continue;
        }
        let mut flag = |reason: &'static str| {
            out.flagged.push(FlaggedLine {
                at,
                text: clipped(line),
                reason,
            });
        };
        let Some((key, value)) = line.split_once(':') else {
            flag("No colon: a line here is key: value");
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if !is_key(key) {
            flag("A key is a lowercase letter, then lowercase letters, digits and dashes");
        } else if value.is_empty() {
            flag("No value");
        } else if value.chars().any(char::is_control) {
            flag("Control characters in the value");
        } else if out.pairs.iter().any(|p| p.key == key) {
            flag("This key is already there: the first one counts");
        } else if out.pairs.len() >= MAX_KEYS {
            flag("More than 20 keys");
        } else {
            if value.chars().count() > MAX_VALUE_CHARS {
                flag("A value is at most 200 characters");
            }
            out.pairs.push(SectionPair {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }
    out
}
