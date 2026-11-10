//! Sections on screen (ADR 0026): a boundary is a quiet labelled divider, and a section's metadata block
//! a compact plaque. The plaque is built as a block's picture is (ADR 0023): its opener item holds it, the
//! content items are hidden as a closed disclosure's are, and the closer stays with no height. So nothing
//! about the items changes, pages and source positions go on as they do, and the block's text is
//! still there for a card, for Source and for whatever selects it.
//!
//! A document without sections pays one comparison per item.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use markise_doc::{LeafKind, SectionPair};

use super::View;
use super::native::BlockViewMode;
use crate::model::{Container, Item, ItemKind};

/// What a metadata block shows as a plaque.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Plaque {
    pub pairs: Vec<SectionPair>,
    /// What does not fit, in words, one per line that does not.
    pub warnings: Vec<String>,
}

/// How sections look on screen, for what the package says of them: the defaults with none.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SectionStyle {
    /// Each section is a card, rather than being divided from the next by a line.
    pub frame: bool,
    /// A colour's name, as a view's accent (ADR 0021).
    pub accent: Option<Arc<str>>,
    /// A boundary says the section's number.
    pub numbered: bool,
    /// What sections are called on the page, for one with no name of its own.
    pub word: Arc<str>,
    /// The level a section's name comes from first, 1 to 6.
    pub title_level: u8,
}

impl Default for SectionStyle {
    fn default() -> SectionStyle {
        SectionStyle {
            frame: false,
            accent: None,
            numbered: true,
            word: "Section".into(),
            title_level: 2,
        }
    }
}

/// What item `i`'s layout gets of its section.
#[derive(Clone, Copy, Debug)]
pub enum SectionLook<'a> {
    /// A boundary, with the number of the section it opens, and what to call it if there is more to say
    /// than what stands below it.
    Divider {
        number: Option<usize>,
        name: Option<&'a str>,
        style: &'a SectionStyle,
    },
    Plaque(&'a Plaque),
    /// The closer of a block shown as a plaque: no height, and the gap after it stays.
    End,
    /// A quiet word about this item, on the row of a rule or under a paragraph.
    Hint(&'a str),
}

/// What an item is to its section.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Role {
    Plain,
    Divider {
        number: Option<usize>,
        name: Option<Arc<str>>,
        style: Arc<SectionStyle>,
    },
    Opener(Arc<Plaque>),
    Inside,
    Closer,
    Hint(Arc<str>),
    /// A card with a warning: a metadata block where none can be.
    Warned(Arc<[String]>),
}

/// A rule written nearly as a boundary.
const NEAR_RULE: &str = "No section starts here: a separator is exactly --- alone on its line";
/// A metadata opener that pairs with nothing.
const UNCLOSED: &str = "Malformed: this metadata block has no closing marker, or a fence inside it took it";
/// A metadata block not first in its section.
const MISPLACED: &str = "Section metadata goes first in its section: this one means nothing here";

impl Role {
    pub(super) fn look(&self) -> Option<SectionLook<'_>> {
        match self {
            Role::Plain | Role::Inside | Role::Warned(_) => None,
            Role::Hint(text) => Some(SectionLook::Hint(text)),
            Role::Divider { number, name, style } => Some(SectionLook::Divider {
                number: *number,
                name: name.as_deref(),
                style,
            }),
            Role::Opener(plaque) => Some(SectionLook::Plaque(plaque)),
            Role::Closer => Some(SectionLook::End),
        }
    }
}

#[derive(Default)]
pub(super) struct SectionsState {
    on: bool,
    style: Arc<SectionStyle>,
    /// By item start, for the document and states as they stand.
    roles: RefCell<HashMap<usize, Role>>,
    /// The openers of the metadata blocks around the selection's ends: those show what they hold.
    held: Vec<usize>,
    /// Outline entries before each section asked about, for the document as it stands.
    heads: RefCell<HashMap<usize, usize>>,
    /// For each section, how many sections before it are named by a heading whose row is left out, and at the
    /// end the number of all of them. For the document as it stands, with how many sections and outline entries.
    named: RefCell<Option<NamedRows>>,
    /// Where the first section's number stands, by the start of the top-level block that carries it, once
    /// asked: `Some(None)` when it shows none. For the document as it stands.
    lead: RefCell<Option<Option<usize>>>,
    /// The item that carried it when last asked, so that an edit can move it: its height holds it.
    pub(super) lead_seen: Cell<Option<usize>>,
}

/// See [`SectionsState::named`].
pub(super) type NamedRows = (Arc<LeftOut>, (usize, usize));

/// What the outline leaves out of the rows under sections.
#[derive(Debug)]
pub(super) struct LeftOut {
    /// For each section, how many headings are left out under those before it, and at the end all of them.
    counts: Vec<u32>,
    /// The document's title has a row above the sections.
    title: bool,
}

impl LeftOut {
    fn before(&self, k: usize) -> usize {
        self.counts.get(k).or(self.counts.last()).map_or(0, |n| *n as usize)
    }
}

impl SectionsState {
    pub(super) fn named_now(&self) -> Option<NamedRows> {
        self.named.borrow().clone()
    }

    /// What was forgotten comes back, for an edit that cannot change it.
    pub(super) fn keep_named(&self, named: Option<NamedRows>) {
        if self.on && named.is_some() {
            *self.named.borrow_mut() = named;
        }
    }

    pub(super) fn forget_lead(&self) {
        *self.lead.borrow_mut() = None;
    }

    pub(super) fn forget(&self) {
        if self.on {
            self.roles.borrow_mut().clear();
            self.heads.borrow_mut().clear();
            *self.named.borrow_mut() = None;
            *self.lead.borrow_mut() = None;
        }
    }
}

/// A row of the outline with sections above its headings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineRow {
    /// The document's title, above the sections: entry `k` of the model's outline.
    Title(usize),
    /// Section `index`.
    Section(usize),
    /// Entry `k` of the model's outline.
    Heading(usize),
}

impl View {
    /// How many outline entries lie before section `k`. The model's outline holds items, so that
    /// a heading in a placeholder counts as the placeholder: a boundary in one reads that one
    /// page, materializing nothing. Kept until the document changes.
    fn heads_before(&self, k: usize) -> usize {
        if let Some(&n) = self.sections.heads.borrow().get(&k) {
            return n;
        }
        let n = match self.doc.section(k).and_then(|s| s.boundary) {
            None => 0,
            Some(b) => {
                let i = self.model.item_at_offset(b.start);
                let whole = self.model.outline.partition_point(|o| o.item < i);
                let heading = |it: &Item| matches!(it.kind, ItemKind::Heading { .. });
                let part = if self.model.is_page(i) {
                    let items = self.model.page_items(&*self.doc, i);
                    items.iter().filter(|(at, it)| *at < b.start && heading(it)).count()
                } else {
                    0
                };
                whole + part
            }
        };
        self.sections.heads.borrow_mut().insert(k, n);
        n
    }

    /// Outline entries before section `k` that are listed: `only` names them, when not all are (ADR 0024).
    fn listed_before(&self, k: usize, only: Option<&[usize]>) -> usize {
        let n = self.heads_before(k);
        only.map_or(n, |o| o.partition_point(|&e| e < n))
    }

    /// Where the document's title is: a `#` that is the first thing in the first section, which stands above the
    /// sections in the outline and names none of them. On a page whose sections go by `#` it is the first one's name.
    fn document_title(&self) -> Option<usize> {
        if self.sections.style.title_level < 2 {
            return None;
        }
        let at = self.doc.section_heading_from(0, 0, None)?;
        if !matches!(self.leaf_kind(at), Some(LeafKind::Heading { level: 1 })) {
            return None;
        }
        let section = self.doc.section(0)?;
        let metadata = self.doc.section_metadata(0);
        let last = self.doc.top_at(at)?;
        let nothing_before = (0..last)
            .rev()
            .map_while(|t| self.doc.top_span(t).filter(|s| s.start >= section.start))
            .all(|s| {
                let in_metadata = metadata
                    .as_ref()
                    .is_some_and(|m| m.opener.start <= s.start && s.start < m.closer.end);
                let kind = self.leaf_kind(s.start);
                matches!(kind, Some(LeafKind::Metadata))
                    || (in_metadata && !matches!(kind, Some(LeafKind::Heading { .. })))
            });
        nothing_before.then_some(at)
    }

    fn leaf_kind(&self, at: usize) -> Option<LeafKind> {
        self.doc.leaf(at).map(|l| l.kind)
    }

    /// The direct heading section `k` goes by, with what it says: the first at the level the page's names are at,
    /// else the first at any level, the document's title being neither. None when it has an `id`, or no such
    /// heading says anything.
    fn naming_heading(&self, k: usize, metadata: Option<&markise_doc::SectionMetadata>) -> Option<(usize, String)> {
        if metadata.is_some_and(|m| m.get("id").is_some()) {
            return None;
        }
        let from = if k == 0 {
            self.document_title().map_or(0, |at| at + 1)
        } else {
            0
        };
        let level = self.sections.style.title_level;
        let said = |at: usize| {
            let text = self.doc.heading(at)?.text.trim().to_string();
            (!text.is_empty()).then_some((at, text))
        };
        (self.doc.section_heading_from(k, from, Some(level)))
            .and_then(said)
            .or_else(|| self.doc.section_heading_from(k, from, None).and_then(said))
    }

    /// What section `k` itself goes by, if anything does: its `id`, or a heading that names it.
    pub fn section_own_name(&self, k: usize) -> Option<String> {
        let metadata = self.doc.section_metadata(k);
        (metadata.as_ref().and_then(|m| m.get("id")).map(str::to_string))
            .or_else(|| self.naming_heading(k, metadata.as_ref()).map(|n| n.1))
    }

    /// What sections are called on this page.
    pub fn section_word(&self) -> &str {
        &self.sections.style.word
    }

    /// What section `k` shows for a number in the outline, as a boundary does; `None` where a boundary says none.
    pub fn section_shown_number(&self, k: usize) -> Option<usize> {
        self.sections
            .style
            .numbered
            .then(|| self.section_number(k))
            .filter(|n| *n > 0)
    }

    /// How many of the headings at the head of section `k` have no row under it: the document's title, which has
    /// one above all the sections, and the heading that names it, which says what its row says. That one only when
    /// nothing is listed before it: a heading in a block, its metadata's if any, or another of its own.
    fn left_out_in(&self, k: usize) -> u32 {
        let title = if k == 0 { self.document_title() } else { None };
        let metadata = self.doc.section_metadata(k);
        let Some((at, _)) = self.naming_heading(k, metadata.as_ref()) else {
            return u32::from(title.is_some());
        };
        let named = self.nothing_listed_before(k, at, title, metadata.as_ref());
        u32::from(title.is_some()) + u32::from(named)
    }

    fn nothing_listed_before(
        &self,
        k: usize,
        at: usize,
        title: Option<usize>,
        metadata: Option<&markise_doc::SectionMetadata>,
    ) -> bool {
        let Some(section) = self.doc.section(k) else {
            return false;
        };
        let Some(last) = self.doc.top_at(at) else { return false };
        (0..last)
            .rev()
            .map_while(|t| self.doc.top_span(t).filter(|s| s.start >= section.start))
            .all(|s| {
                let in_metadata = metadata.is_some_and(|m| m.opener.start <= s.start && s.start < m.closer.end);
                let heading = matches!(self.leaf_kind(s.start), Some(LeafKind::Heading { .. }));
                let boxed = !self.doc.named_containers_at(s.start).is_empty();
                Some(s.start) == title || (!heading && (!boxed || in_metadata))
            })
    }

    /// What [`Self::named_rows`] holds, when this edit does not change it: as many sections and outline entries as
    /// there were, and the sections the edit is in say what they said. So typing goes on at no cost for it, however
    /// many sections there are. An edit that pairs far away can change what a far section starts with, which then shows
    /// until the next change that is not typing: a row too many or too few, and nothing in the wrong place.
    pub(super) fn sections_names_stand(&self, change: &crate::model::ModelChange) -> Option<NamedRows> {
        let named = self.sections.named_now()?;
        let count = self.doc.section_count();
        if change.full || change.splices().len() != 1 || named.1 != (count, self.model.outline.len()) {
            return None;
        }
        let last = self.doc.text().len();
        let from =
            (self.model.items.get(change.inserted.start)).map(|_| self.model.item_start(change.inserted.start))?;
        let to =
            (self.model.items.get(change.inserted.end)).map_or(last, |_| self.model.item_start(change.inserted.end));
        let (a, b) = (self.doc.section_at(from).index, self.doc.section_at(to).index);
        let same = (a..=b).all(|k| {
            let was = named.0.counts.get(k + 1).zip(named.0.counts.get(k)).map(|(x, y)| x - y);
            was == Some(self.left_out_in(k)) && (k > 0 || named.0.title == self.document_title().is_some())
        });
        same.then_some(named)
    }

    /// The running count of [`Self::left_out_in`]. Goes over the sections once per state of the document,
    /// reading only what each starts with, and only while the outline is asked for.
    fn named_rows(&self) -> Arc<LeftOut> {
        if let Some((n, _)) = &*self.sections.named.borrow() {
            return n.clone();
        }
        let count = self.doc.section_count();
        let mut counts = Vec::with_capacity(count + 1);
        let mut n = 0u32;
        for k in 0..count {
            counts.push(n);
            n += self.left_out_in(k);
        }
        counts.push(n);
        let out = Arc::new(LeftOut {
            counts,
            title: self.document_title().is_some(),
        });
        *self.sections.named.borrow_mut() = Some((out.clone(), (count, self.model.outline.len())));
        out
    }

    /// Rows of the outline with sections: the document's title if it has one, then every section, each followed by
    /// its listed headings, but for one that says what its section's row says.
    pub fn outline_row_count(&self, only: Option<&[usize]>) -> usize {
        let listed = only.map_or(self.model.outline.len(), <[usize]>::len);
        let left = self.named_rows();
        let title = usize::from(left.title);
        title + self.doc.section_count() + listed.saturating_sub(left.before(self.doc.section_count()))
    }

    /// Row `row` of that outline. O(log sections) lookups: the rows are not built, so a list of
    /// any size costs what is on screen.
    pub fn outline_row(&self, row: usize, only: Option<&[usize]>) -> OutlineRow {
        let left = self.named_rows();
        let first = |i: usize| only.map_or(i, |o| o.get(i).copied().unwrap_or(i));
        let title = usize::from(left.title);
        if row < title {
            return OutlineRow::Title(first(0));
        }
        let row = row - title;
        let (mut lo, mut hi) = (0, self.doc.section_count());
        // the last section whose row is at or before this one
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if (mid + self.listed_before(mid, only)).saturating_sub(left.before(mid)) <= row {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if (lo + self.listed_before(lo, only)).saturating_sub(left.before(lo)) == row {
            OutlineRow::Section(lo)
        } else {
            OutlineRow::Heading(first(row - lo - 1 + left.before(lo + 1)))
        }
    }

    /// The row of outline entry `heading`, or of `section` if the entry is not listed or lies before it.
    /// For what is on screen.
    pub fn outline_row_of(&self, section: usize, heading: Option<usize>, only: Option<&[usize]>) -> usize {
        let before = self.listed_before(section, only);
        let place = heading.and_then(|k| match only {
            Some(o) => o.partition_point(|&e| e <= k).checked_sub(1),
            None => Some(k),
        });
        let left = self.named_rows();
        let title = usize::from(left.title);
        // at the title, its row; at what is left out under a section, that section's
        if title == 1 && section == 0 && place == Some(0) {
            return 0;
        }
        let under = left.before(section + 1) - left.before(section);
        match place {
            Some(p) if p >= before + under => title + p + section + 1 - left.before(section + 1),
            _ => title + (section + before).saturating_sub(left.before(section)),
        }
    }

    /// Where a jump to section `k` goes: its boundary, or for the first section its start. Materializes
    /// that page.
    pub fn ensure_section(&mut self, k: usize) -> Option<usize> {
        let s = self.doc.section(k)?;
        let at = s.boundary.map_or(s.start, |b| b.start);
        self.ensure_offset(at);
        Some(self.model.item_at_offset(at))
    }

    /// Whether the document is in a dialect with sections. It reads as a document without them when not.
    pub fn set_sections(&mut self, on: bool) {
        if on != self.sections.on {
            self.sections.on = on;
            self.sections.roles.borrow_mut().clear();
            *self.sections.lead.borrow_mut() = None;
            self.sections.held.clear();
            self.reset_heights();
        }
    }

    pub fn sections_on(&self) -> bool {
        self.sections.on
    }

    /// How sections look, for what the package says of them.
    pub fn set_section_style(&mut self, style: SectionStyle) {
        if *self.sections.style != style {
            self.sections.style = Arc::new(style);
            self.sections.roles.borrow_mut().clear();
            *self.sections.lead.borrow_mut() = None;
            self.reset_heights();
        }
    }

    /// How far the column has to stay in from its edges for a section's frame to stand around it.
    pub fn section_frame_inset(&self, theme: &markise_theme::Theme) -> f32 {
        if self.sections.on && self.sections.style.frame {
            crate::block::section_frame_padding(theme)
        } else {
            0.0
        }
    }

    /// The number a boundary says, from 1 among what shows: a first section with nothing in it is none.
    fn section_number(&self, index: usize) -> usize {
        let empty_first = (self.doc.section(0)).is_some_and(|s| {
            (self.doc.text().get(s.start..s.end)).is_some_and(|t| t.trim_matches([' ', '\t', '\r', '\n']).is_empty())
        });
        if empty_first { index } else { index + 1 }
    }

    /// Where the first section's number stands: the start of the top-level block it goes above, which is no
    /// boundary, as the first section has none and nothing is written for it. That is its first block after
    /// front matter, or, when the document's title is that, the next one in the section, since the title stands
    /// above the sections as in the outline. None when sections are off, the first section has nothing in it,
    /// or there is nothing to show: no number, no frame and no name.
    pub(super) fn section_lead(&self) -> Option<usize> {
        if !self.sections.on {
            return None;
        }
        if let Some(found) = *self.sections.lead.borrow() {
            return found;
        }
        let found = self.find_section_lead();
        *self.sections.lead.borrow_mut() = Some(found);
        found
    }

    fn find_section_lead(&self) -> Option<usize> {
        let style = &self.sections.style;
        if self.section_number(0) == 0 || !(style.numbered || style.frame || self.divider_name(0).is_some()) {
            return None;
        }
        let end = self.doc.section(0)?.end;
        let items = &self.model.items;
        let first = usize::from(items.get(0).is_some_and(|it| it.kind == ItemKind::FrontMatter));
        if first >= items.len() {
            return None;
        }
        let at = self.model.item_start(first);
        if at >= end {
            return None;
        }
        let Some(title) = self.document_title() else {
            return self.first_shown_from(first, end);
        };
        let t = self.doc.top_at(title)?;
        if self.doc.top_span(t).is_none_or(|s| s.start != at) {
            return self.first_shown_from(first, end);
        }
        // with nothing shown after the title in its section, the title is what there is
        let after = self
            .model
            .item_at_offset(self.doc.top_span(t + 1).map_or(end, |s| s.start));
        (self.doc.top_span(t + 1).is_some_and(|s| s.start < end))
            .then(|| self.first_shown_from(after, end))
            .flatten()
            .or(Some(at))
    }

    /// The start of the first item from `i` on, before `end`, that shows whatever the caret does: a
    /// hidden comment, a closed fold's body or what a plaque or a site view hides takes no number.
    fn first_shown_from(&self, mut i: usize, end: usize) -> Option<usize> {
        while i < self.model.items.len() && self.model.item_start(i) < end {
            if !self.hidden_at_rest(i) {
                return Some(self.model.item_start(i));
            }
            i += 1;
        }
        None
    }

    /// The item the first section's number stands above, if it shows one.
    pub(super) fn section_lead_item(&self) -> Option<usize> {
        let i = self.section_lead().map(|at| self.model.item_at_offset(at));
        self.sections.lead_seen.set(i);
        i
    }

    /// The room above item `i` for the first section's number and the top of its frame.
    pub(super) fn section_lead_room(&self, i: usize) -> f32 {
        if self.section_lead_item() == Some(i) {
            crate::block::section_lead_height(&self.theme, self.sections.style.frame)
        } else {
            0.0
        }
    }

    /// Room above item `i`'s top that is its own (the first section's number and the top of its frame):
    /// a point there is a point in the item, though it stands above where the item's layout starts.
    pub fn room_above(&self, i: usize) -> f32 {
        self.section_lead_room(i)
    }

    /// What item `i`, the first section's first, shows above itself: its number and name.
    pub(super) fn section_lead_prims(
        &self,
        i: usize,
        shaper: &mut dyn crate::text::TextShaper,
    ) -> Vec<crate::block::Prim> {
        if self.section_lead_item() != Some(i) {
            return Vec::new();
        }
        let style = &self.sections.style;
        let number = style.numbered.then(|| self.section_number(0));
        let name = self.divider_name(0);
        let top = -crate::block::section_lead_height(&self.theme, style.frame);
        crate::block::section_lead_prims(shaper, &self.theme, number, name.as_deref(), style, self.width(), top)
    }

    /// What a boundary calls its section besides its number: its `id`, unless that is what the first heading
    /// below says. A heading says itself, and a number with nothing else is enough.
    fn divider_name(&self, index: usize) -> Option<Arc<str>> {
        let id = (self.doc.section_metadata(index)).and_then(|m| m.get("id").map(str::to_string))?;
        let heading = (self.doc.section_first_heading(index)).and_then(|at| self.doc.heading(at));
        (heading.is_none_or(|h| h.text.trim() != id)).then(|| id.into())
    }

    /// What a section's frame comes to in item `i`, whose layout has this height and gap (ADR 0026). All the
    /// items of a section have one, so that it is one shape however many items and pages it takes up, as a
    /// native block's ground is (`merged_bands`), and it stands out from the column over the inset.
    pub(super) fn section_frame_bands(&self, i: usize, it: &Item, height: f32, gap: f32) -> Vec<crate::block::Band> {
        use crate::block::Band;
        let style = &self.sections.style;
        if !self.sections.on || !style.frame || it.kind == ItemKind::FrontMatter {
            return Vec::new();
        }
        let t = &self.theme;
        let pad = crate::block::section_frame_padding(t);
        let colour = crate::block::section_color(style, t);
        let fill = {
            let c = if style.accent.is_some() {
                colour
            } else {
                crate::block::color(t.text)
            };
            let a = match (style.accent.is_some(), t.dark) {
                (true, true) => 22,
                (true, false) => 14,
                (false, true) => 12,
                (false, false) => 8,
            };
            epaint::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
        };
        let band = |top: f32, bottom: f32, opens: bool, closes: bool| Band {
            rect: epaint::Rect::from_min_max(epaint::pos2(-pad, top), epaint::pos2(self.width() + pad, bottom)),
            fill,
            opens,
            closes,
            stroke: epaint::Stroke::new(1.5, colour),
            radius: (t.font_size * 0.7).round(),
        };
        let after_front = i > 0
            && self
                .model
                .items
                .get(i - 1)
                .is_some_and(|p| p.kind == ItemKind::FrontMatter);
        let last = i + 1 == self.model.items.len();
        // the first section's frame starts where its number stands, the title above it outside
        if let Some(lead) = self.section_lead_item() {
            if i < lead {
                return Vec::new();
            }
            if i == lead {
                let top = -crate::block::section_lead_height(t, true) + crate::block::section_lead_frame_top(t);
                return vec![band(top, if last { height + pad } else { height }, true, last)];
            }
        }
        let first = (i == 0 || after_front) && self.section_lead_item().is_none();
        if let Role::Divider { .. } = self.section_role(i, it) {
            let m = crate::block::divider_metrics(t, true);
            let below = band(gap + m.opens, if last { height + pad } else { height }, true, last);
            // a first section with nothing in it has no frame to end
            return if first {
                vec![below]
            } else {
                vec![band(0.0, gap + m.tail, false, true), below]
            };
        }
        vec![band(
            // above a document's first item there is room; under front matter only its gap
            match (first, after_front) {
                (true, true) => gap * 0.3,
                (true, false) => -pad,
                _ => 0.0,
            },
            if last { height + pad } else { height },
            first,
            last,
        )]
    }

    /// What to call section `index`: what it goes by, else what sections are called here with its number. The
    /// outline, the status bar and whatever speaks of it say this.
    pub fn section_name(&self, index: usize) -> String {
        self.section_own_name(index).unwrap_or_else(|| {
            let word = self.section_word();
            match self.section_number(index) {
                0 => word.to_string(),
                n => format!("{word} {n}"),
            }
        })
    }

    /// Part of what a measured page height holds for.
    pub(super) fn sections_key(&self) -> u64 {
        if !self.sections.on {
            return 0;
        }
        let mut h = crate::model::hasher();
        self.sections.style.hash(&mut h);
        h.finish() | 1
    }

    /// The block with this opener is a section's metadata block, in a document with sections.
    pub fn is_section_metadata(&self, opener: usize) -> bool {
        self.metadata_at(opener).is_some_and(|m| m.opener.start == opener)
    }

    fn metadata_at(&self, at: usize) -> Option<markise_doc::SectionMetadata> {
        if !self.sections.on {
            return None;
        }
        self.doc.section_metadata(self.doc.section_at(at).index)
    }

    /// What this item is to its section. Cheap: a comparison with no sections, a lookup for a rule or
    /// an item in a card with them.
    pub(super) fn section_role(&self, i: usize, it: &Item) -> Role {
        if !self.sections.on {
            return Role::Plain;
        }
        let card = it.containers.iter().any(|c| matches!(c, Container::Card));
        let rule = it.kind == ItemKind::Rule && it.containers.is_empty();
        let paragraph = it.kind == ItemKind::Paragraph && it.containers.is_empty();
        if !card && !rule && !paragraph {
            return Role::Plain;
        }
        let start = self.model.item_start(i);
        if let Some(found) = self.sections.roles.borrow().get(&start) {
            return found.clone();
        }
        // a rule in a metadata block is what the block makes of it
        let role = if card {
            self.card_role(start, it)
        } else if rule {
            self.rule_role(start)
        } else {
            self.paragraph_role(start)
        };
        self.sections.roles.borrow_mut().insert(start, role.clone());
        role
    }

    fn rule_role(&self, start: usize) -> Role {
        let section = self.doc.section_at(start);
        match section.boundary {
            Some(b) if b.start == start => {
                let style = self.sections.style.clone();
                Role::Divider {
                    number: style.numbered.then(|| self.section_number(section.index)),
                    name: self.divider_name(section.index),
                    style,
                }
            }
            _ if self.doc.section_near_boundary(start) => Role::Hint(NEAR_RULE.into()),
            _ => Role::Plain,
        }
    }

    fn paragraph_role(&self, start: usize) -> Role {
        let section = self.doc.section_at(start);
        match self.doc.section_metadata_unclosed(section.index) {
            Some(opener) if opener.start == start => Role::Hint(UNCLOSED.into()),
            _ => Role::Plain,
        }
    }

    fn card_role(&self, start: usize, it: &Item) -> Role {
        if it.kind == (ItemKind::ContainerMarker { close: false }) && self.doc.section_metadata_misplaced(start) {
            return Role::Warned(Arc::new([MISPLACED.to_string()]));
        }
        let Some(m) = self.metadata_at(start) else {
            return Role::Plain;
        };
        if !(m.opener.start..=m.closer.start).contains(&start)
            || self.sections.held.contains(&m.opener.start)
            // Read shows no card of what it has a plaque for, whatever the block's state in Edit
            || (self.block_mode_of(m.opener.start) == BlockViewMode::Markdown && !self.native.practice.read())
        {
            return Role::Plain;
        }
        let own = self.doc.named_containers_at(start).len() == 1;
        match it.kind {
            ItemKind::ContainerMarker { close: false } if own => Role::Opener(Arc::new(Plaque {
                warnings: m.flagged.iter().map(|f| format!("{}: {}", f.reason, f.text)).collect(),
                pairs: m.pairs,
            })),
            ItemKind::ContainerMarker { close: true } if own => Role::Closer,
            _ => Role::Inside,
        }
    }

    /// Items a plaque hides.
    pub(super) fn section_hidden(&self, i: usize) -> bool {
        (self.model.items.get(i)).is_some_and(|it| self.section_role(i, it) == Role::Inside)
    }

    /// What of its section enters the cache key of `it`.
    pub(super) fn section_item_key(&self, i: usize, it: &Item) -> u64 {
        if !self.sections.on {
            return 0;
        }
        let mut h = crate::model::hasher();
        // the first section's number and the start of its frame are in the item it stands above
        let lead = self.section_lead_item();
        if lead.is_some() {
            lead.map(|l| i.cmp(&l)).hash(&mut h);
            if lead == Some(i) {
                (
                    self.sections.style.numbered.then(|| self.section_number(0)),
                    self.divider_name(0),
                )
                    .hash(&mut h);
                self.sections.style.hash(&mut h);
            }
        }
        // a frame's ends are in the items at the document's ends, and all of them hold it
        if self.sections.style.frame {
            self.sections.style.hash(&mut h);
            (i == 0
                || self
                    .model
                    .items
                    .get(i - 1)
                    .is_some_and(|p| p.kind == ItemKind::FrontMatter))
            .hash(&mut h);
            (i + 1 == self.model.items.len()).hash(&mut h);
        } else if lead != Some(i) && matches!(self.section_role(i, it), Role::Plain | Role::Inside) {
            return 0;
        }
        match self.section_role(i, it) {
            Role::Plain | Role::Inside => {}
            Role::Divider { number, name, style } => (number, name, style).hash(&mut h),
            Role::Hint(label) => label.hash(&mut h),
            Role::Warned(why) => why.hash(&mut h),
            Role::Opener(plaque) => plaque.hash(&mut h),
            Role::Closer => 1u8.hash(&mut h),
        }
        h.finish().rotate_left(19)
    }

    /// The height estimate of an item a plaque has to do with.
    pub(super) fn section_estimate(&self, i: usize, it: &Item) -> Option<f32> {
        match self.section_role(i, it) {
            Role::Opener(_) => Some(self.theme.block_spacing + crate::block::plaque_row_height(&self.theme)),
            Role::Divider { style, .. } => {
                let m = crate::block::divider_metrics(&self.theme, style.frame);
                Some(self.theme.block_spacing + m.end)
            }
            Role::Closer => Some(0.0),
            _ => None,
        }
    }

    /// What a page's boundaries add to what its rules are counted as: `source` is what the page holds.
    pub(super) fn section_page_estimate(&self, source: std::ops::Range<usize>) -> f32 {
        if !self.sections.on || source.is_empty() {
            return 0.0;
        }
        let first = self.doc.section_at(source.start);
        let last = self.doc.section_at(source.end - 1);
        let mut boundaries = last.index - first.index;
        if first.boundary.is_some_and(|b| b.start == source.start) {
            boundaries += 1;
        }
        let style = &self.sections.style;
        let m = crate::block::divider_metrics(&self.theme, style.frame);
        boundaries as f32 * (m.end - self.theme.line_px())
    }

    /// Where the plaque around item `i` is, when it hides `i`.
    pub(super) fn section_opener_of(&self, i: usize) -> Option<usize> {
        if !self.section_hidden(i) {
            return None;
        }
        let m = self.metadata_at(self.model.item_start(i))?;
        Some(self.model.items.partition_point_start(m.opener.start))
    }

    /// What does not fit in the metadata item `i` shows as a plaque.
    pub fn section_warnings(&self, i: usize) -> Option<Vec<String>> {
        let it = self.model.items.get(i)?;
        match self.section_role(i, it) {
            Role::Opener(plaque) if !plaque.warnings.is_empty() => Some(plaque.warnings.clone()),
            Role::Warned(why) => Some(why.to_vec()),
            _ => None,
        }
    }

    /// Where the selection's ends are: a metadata block around one shows what it holds, so that what is
    /// selected, found or being typed in can be seen. Called on every change of the selection.
    pub fn set_selection_for_sections(&mut self, ends: &[usize]) {
        if !self.sections.on {
            return;
        }
        let mut held = Vec::new();
        for &at in ends {
            if let Some(m) = self.metadata_at(at)
                && m.opener.start < at
                && at < m.closer.end
                && !held.contains(&m.opener.start)
            {
                held.push(m.opener.start);
            }
        }
        if held != self.sections.held {
            self.sections.held = held;
            self.sections.roles.borrow_mut().clear();
            self.reset_heights();
        }
    }
}
