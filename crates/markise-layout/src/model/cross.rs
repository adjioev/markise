//! Cross-block facts of a document (issue #125, phase 3): what the rendering of one top-level
//! block depends on in other blocks, kept by source position for the whole document so a paged
//! model answers without building items for pages it has not materialized.
//!
//! - **Disclosures** (`<details>` / `<summary>` / `</details>` as separate raw blocks, spec 03
//!   common HTML): every marker with its container scope, paired document-wide into groups
//!   (start, optional separate summary, end), nested and malformed ones as a full model pairs
//!   them; the open/closed state the reader toggled lives on the start marker (it survives
//!   materialization, eviction and edits elsewhere).
//! - **Dependency generations** of document-level inputs: the outline (`[toc]` content and
//!   height), the resource root (image resolution) and reference definitions. A placeholder's
//!   measured height is valid only while the generations its blocks depend on are unchanged.
//!
//! Headings, slugs and anchors are the outline ([`super::OutlineEntry`], document-wide in every
//! model); reference definitions are resolved by the format's own document-wide context.
//!
//! Format-neutral: markers come from [`FormatDoc::raw_presentation`] through the items and
//! their scopes from [`FormatDoc::containers_at`]. Kept in step by [`super::DocModel::apply`]
//! from the replaced items of each edit.

use markise_doc::{FormatDoc, RawPresentation};

use super::Item;

/// A disclosure marker: a raw block presenting as `<details>`, `<summary>` or `</details>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisclosureMarker {
    /// Absolute start and length of the block.
    pub start: usize,
    pub len: usize,
    pub kind: RawPresentation,
    /// Starts of the containers around the block (outermost first): markers pair only within
    /// the same containers.
    pub scope: Box<[usize]>,
    /// The reader's open/closed choice for a start marker (`None`: as written).
    pub toggled: Option<bool>,
}

/// A paired disclosure, by source position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisclosureGroup {
    /// Start of the `<details>` block.
    pub start: usize,
    /// Start of the separate `<summary>` block right after it, if any.
    pub summary: Option<usize>,
    /// Start of the `</details>` block closing it.
    pub end: usize,
    pub title: String,
    /// Open now (the reader's choice, else as written).
    pub open: bool,
}

/// Generations of the document-level inputs layouts depend on (bumped on every change).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DepGens {
    /// Outline entries (text, level, anchor, count): `[toc]` blocks.
    pub outline: u64,
    /// Resource root and document folder: image items.
    pub resources: u64,
    /// Reference definitions: items that may use one.
    pub refs: u64,
}

/// Cross-block facts of a document (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct CrossBlockIndex {
    /// Disclosure markers in document order.
    markers: Vec<DisclosureMarker>,
    /// Pairing of `markers` (document order of the starts).
    groups: Vec<DisclosureGroup>,
    pub gens: DepGens,
}

impl CrossBlockIndex {
    /// The disclosure markers in document order.
    pub fn markers(&self) -> &[DisclosureMarker] {
        &self.markers
    }

    /// Whether two indexes hold the same markers and pairs (the reader's choices aside).
    pub fn same_disclosures(&self, other: &CrossBlockIndex) -> bool {
        let m = |c: &CrossBlockIndex| -> Vec<(usize, usize, RawPresentation, Box<[usize]>)> {
            c.markers
                .iter()
                .map(|m| (m.start, m.len, m.kind.clone(), m.scope.clone()))
                .collect()
        };
        let g = |c: &CrossBlockIndex| -> Vec<(usize, Option<usize>, usize, String)> {
            c.groups
                .iter()
                .map(|g| (g.start, g.summary, g.end, g.title.clone()))
                .collect()
        };
        m(self) == m(other) && g(self) == g(other)
    }

    /// The paired disclosures in document order of their starts.
    pub fn disclosures(&self) -> &[DisclosureGroup] {
        &self.groups
    }

    /// Adds the markers among freshly flattened `items` (absolute origins, in document order,
    /// after every marker already known or replacing a removed range: re-sorted).
    pub(super) fn add_from(&mut self, doc: &dyn FormatDoc, items: &[Item]) {
        let mut added = false;
        for it in items {
            let Some(raw) = it.raw.as_deref() else { continue };
            if matches!(raw, RawPresentation::Comment) {
                continue;
            }
            let scope: Box<[usize]> = doc.containers_at(it.origin).iter().map(|c| c.span.start).collect();
            self.markers.push(DisclosureMarker {
                start: it.origin,
                len: it.len,
                kind: raw.clone(),
                scope,
                toggled: None,
            });
            added = true;
        }
        if added {
            self.markers.sort_by_key(|m| m.start);
            self.markers.dedup_by_key(|m| m.start);
        }
    }

    /// An edit replaced the items starting at `removed` (pre-edit offsets) and shifted
    /// everything at or after `old_end` (pre-edit) by `delta`: their markers go (their toggled
    /// state with them), later ones move.
    pub(super) fn edit(&mut self, removed: &[usize], old_end: usize, delta: isize) {
        let shift = |x: &mut usize| {
            if *x >= old_end {
                *x = (*x as isize + delta) as usize;
            }
        };
        self.markers.retain(|m| removed.binary_search(&m.start).is_err());
        for m in &mut self.markers {
            shift(&mut m.start);
            for s in m.scope.iter_mut() {
                shift(s);
            }
        }
    }

    /// Clears the markers (a full rebuild collects them again); the reader's choices are kept
    /// for markers found again at the same position (`keep` maps old to new positions).
    pub(super) fn take_toggles(&mut self, keep: impl Fn(usize) -> Option<usize>) -> Vec<(usize, bool)> {
        let out = self
            .markers
            .iter()
            .filter_map(|m| Some((keep(m.start)?, m.toggled?)))
            .collect();
        self.markers.clear();
        self.groups.clear();
        out
    }

    /// Re-applies toggles from [`CrossBlockIndex::take_toggles`] and pairs again.
    pub(super) fn restore_toggles(&mut self, doc: &dyn FormatDoc, toggles: &[(usize, bool)]) {
        for &(at, open) in toggles {
            if let Ok(k) = self.markers.binary_search_by_key(&at, |m| m.start) {
                self.markers[k].toggled = Some(open);
            }
        }
        self.pair(doc);
    }

    /// Sets the reader's open/closed choice of the disclosure starting at `start`; `false`
    /// when there is none.
    pub fn set_open(&mut self, start: usize, open: bool) -> bool {
        let Ok(k) = self.markers.binary_search_by_key(&start, |m| m.start) else {
            return false;
        };
        self.markers[k].toggled = Some(open);
        if let Some(g) = self.groups.iter_mut().find(|g| g.start == start) {
            g.open = open;
        }
        true
    }

    /// Pairs the markers document-wide: a start opens a group in its container scope, an end
    /// closing `count` details closes that many open ones of its scope (ignored when fewer are
    /// open), a summary block right after a start (nothing but whitespace between) gives the
    /// title of a start without its own.
    pub(super) fn pair(&mut self, doc: &dyn FormatDoc) {
        let text = doc.text();
        let mut groups: Vec<DisclosureGroup> = Vec::new();
        let mut stacks: std::collections::HashMap<&[usize], Vec<usize>> = Default::default();
        for (k, m) in self.markers.iter().enumerate() {
            match &m.kind {
                RawPresentation::DisclosureStart { .. } => stacks.entry(&m.scope).or_default().push(k),
                RawPresentation::DisclosureEnd { count } => {
                    let stack = stacks.entry(&m.scope).or_default();
                    if *count > stack.len() {
                        continue;
                    }
                    for _ in 0..*count {
                        let s = stack.pop().expect("an open start");
                        let RawPresentation::DisclosureStart { title, open } = &self.markers[s].kind else {
                            unreachable!()
                        };
                        let next = self.markers.get(s + 1).filter(|n| {
                            let a = self.markers[s].start + self.markers[s].len;
                            n.scope == self.markers[s].scope
                                && text.get(a..n.start).is_some_and(|between| between.trim().is_empty())
                        });
                        let (title, summary) = match next.map(|n| (&n.kind, n.start)) {
                            Some((RawPresentation::DisclosureSummary { title: t }, at)) if title.is_empty() => {
                                (t.clone(), Some(at))
                            }
                            _ => (title.clone(), None),
                        };
                        groups.push(DisclosureGroup {
                            start: self.markers[s].start,
                            summary,
                            end: m.start,
                            title: if title.is_empty() { "Details".into() } else { title },
                            open: self.markers[s].toggled.unwrap_or(*open),
                        });
                    }
                }
                _ => {}
            }
        }
        groups.sort_by_key(|g| g.start);
        self.groups = groups;
    }

    /// Shifts the groups like [`CrossBlockIndex::edit`] shifts the markers (no marker was
    /// removed or added: the pairing stands).
    pub(super) fn shift_groups(&mut self, old_end: usize, delta: isize) {
        let shift = |x: &mut usize| {
            if *x >= old_end {
                *x = (*x as isize + delta) as usize;
            }
        };
        for g in &mut self.groups {
            shift(&mut g.start);
            shift(&mut g.end);
            if let Some(s) = &mut g.summary {
                shift(s);
            }
        }
    }
}
