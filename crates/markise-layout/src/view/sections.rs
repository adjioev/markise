//! Sections on screen (ADR 0026): a boundary is a quiet labelled divider, and a section's metadata block
//! a compact plaque. The plaque is built as a block's picture is (ADR 0023): its opener item holds it, the
//! content items are hidden as a closed disclosure's are, and the closer stays with no height. So nothing
//! about the items changes, pages and source positions go on as they do, and the block's text is
//! still there for a card, for Source and for whatever selects it.
//!
//! A document without sections pays one comparison per item.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use markise_doc::SectionPair;

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

/// What item `i`'s layout gets of its section.
#[derive(Clone, Copy, Debug)]
pub enum SectionLook<'a> {
    /// A boundary, with what to call the section it opens.
    Divider(&'a str),
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
    Divider(Arc<str>),
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
            Role::Divider(label) => Some(SectionLook::Divider(label)),
            Role::Opener(plaque) => Some(SectionLook::Plaque(plaque)),
            Role::Closer => Some(SectionLook::End),
        }
    }
}

#[derive(Default)]
pub(super) struct SectionsState {
    on: bool,
    /// By item start, for the document and states as they stand.
    roles: RefCell<HashMap<usize, Role>>,
    /// The openers of the metadata blocks around the selection's ends: those show what they hold.
    held: Vec<usize>,
    /// Outline entries before each section asked about, for the document as it stands.
    heads: RefCell<HashMap<usize, usize>>,
}

impl SectionsState {
    pub(super) fn forget(&self) {
        if self.on {
            self.roles.borrow_mut().clear();
            self.heads.borrow_mut().clear();
        }
    }
}

/// A row of the outline with sections above its headings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineRow {
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

    /// Rows of the outline with sections: every section, each followed by its listed headings.
    pub fn outline_row_count(&self, only: Option<&[usize]>) -> usize {
        self.doc.section_count() + only.map_or(self.model.outline.len(), <[usize]>::len)
    }

    /// Row `row` of that outline. O(log sections) lookups: the rows are not built, so a list of
    /// any size costs what is on screen.
    pub fn outline_row(&self, row: usize, only: Option<&[usize]>) -> OutlineRow {
        let (mut lo, mut hi) = (0, self.doc.section_count());
        // the last section whose row is at or before this one
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if mid + self.listed_before(mid, only) <= row {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if lo + self.listed_before(lo, only) == row {
            OutlineRow::Section(lo)
        } else {
            let i = row - lo - 1;
            OutlineRow::Heading(only.map_or(i, |o| o.get(i).copied().unwrap_or(i)))
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
        match place {
            Some(p) if p >= before => p + section + 1,
            _ => section + before,
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
            self.sections.held.clear();
            self.reset_heights();
        }
    }

    pub fn sections_on(&self) -> bool {
        self.sections.on
    }

    /// What to call section `index`: its `id`, else its first heading, else its number from 1.
    pub fn section_name(&self, index: usize) -> String {
        if let Some(id) = (self.doc.section_metadata(index)).and_then(|m| m.get("id").map(str::to_string)) {
            return id;
        }
        let heading = (self.doc.section_first_heading(index)).and_then(|at| self.doc.heading(at));
        match heading.map(|h| h.text.trim().to_string()).filter(|t| !t.is_empty()) {
            Some(text) => text,
            None => format!("Section {}", index + 1),
        }
    }

    /// Part of what a measured page height holds for.
    pub(super) fn sections_key(&self) -> u64 {
        u64::from(self.sections.on)
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
            Some(b) if b.start == start => Role::Divider(self.section_name(section.index).into()),
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
        match self.section_role(i, it) {
            Role::Plain | Role::Inside => return 0,
            Role::Divider(label) | Role::Hint(label) => label.hash(&mut h),
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
            Role::Closer => Some(0.0),
            _ => None,
        }
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
