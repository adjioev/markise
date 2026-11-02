//! Cross-block facts of a document (issue #125, phase 3): what the rendering of one top-level
//! block depends on in other blocks, kept by source position for the whole document so a paged
//! model answers without building items for pages it has not materialized.
//!
//! - **Disclosures** (`<details>` / `<summary>` / `</details>` as separate raw blocks, spec 03
//!   common HTML): every marker, paired document-wide into groups (start, optional separate
//!   summary, end) within their container scope, nested and malformed ones as a full model
//!   pairs them; the open/closed state the reader toggled lives on the start marker (it
//!   survives materialization, eviction, edits elsewhere and re-parses of its container).
//! - **Dependency generations** of document-level inputs: the outline (`[toc]` content and
//!   height), the resource root (image resolution), reference definitions and the disclosures'
//!   state. A placeholder's measured height is valid only while the generations its blocks
//!   depend on are unchanged.
//!
//! Headings, slugs and anchors are the outline ([`super::OutlineEntry`], document-wide in every
//! model); reference definitions are resolved by the format's own document-wide context.
//!
//! Markers are kept in a chunked relative-offset list (like the items): an edit shifts the ones
//! after it in O(chunks), and groups refer to markers by index, so ordinary typing (no marker
//! touched) costs no work per disclosure. Scopes are queried ([`FormatDoc::containers_at`])
//! only when the markers are paired again, i.e. when an edit touched one.
//!
//! Format-neutral: markers come from [`FormatDoc::raw_presentation`] through the items.

use markise_doc::chunked::{RelChunks, Relative};
use markise_doc::{FormatDoc, RawPresentation};

use super::Item;

/// A disclosure marker: a raw block presenting as `<details>`, `<summary>` or `</details>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisclosureMarker {
    /// Absolute start of the block (relative while it is in the chunked list).
    pub start: usize,
    pub len: usize,
    pub kind: RawPresentation,
    /// The reader's open/closed choice for a start marker (`None`: as written).
    pub toggled: Option<bool>,
}

impl Relative for DisclosureMarker {
    fn origin(&self) -> usize {
        self.start
    }
    fn shift(&mut self, delta: isize) {
        self.start = (self.start as isize + delta) as usize;
    }
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

/// A group by marker indices.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    start: usize,
    summary: Option<usize>,
    end: usize,
    title: String,
    open: bool,
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
    /// Disclosure pairing and open/closed state: what a closed disclosure hides.
    pub disclosures: u64,
}

/// Cross-block facts of a document (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct CrossBlockIndex {
    /// Disclosure markers in document order.
    markers: RelChunks<DisclosureMarker>,
    /// Pairing of `markers` (document order of the starts).
    groups: Vec<Group>,
    pub gens: DepGens,
    /// Pairings computed (tests: ordinary typing pairs nothing).
    pub pairings: u64,
}

/// The markers among freshly flattened `items` (absolute origins, document order).
pub(super) fn markers_of(items: &[Item]) -> Vec<DisclosureMarker> {
    let mut out: Vec<DisclosureMarker> = Vec::new();
    for it in items {
        let Some(raw) = it.raw.as_deref() else { continue };
        if matches!(raw, RawPresentation::Comment) || out.last().is_some_and(|m| m.start == it.origin) {
            continue; // segments of one block share its start
        }
        out.push(DisclosureMarker {
            start: it.origin,
            len: it.len,
            kind: raw.clone(),
            toggled: None,
        });
    }
    out
}

impl CrossBlockIndex {
    /// The disclosure markers in document order (absolute starts).
    pub fn markers(&self) -> Vec<DisclosureMarker> {
        self.markers.iter().map(|r| r.to_abs()).collect()
    }

    fn marker_start(&self, k: usize) -> usize {
        self.markers.origin(k).unwrap_or(0)
    }

    fn group_at(&self, g: &Group) -> DisclosureGroup {
        DisclosureGroup {
            start: self.marker_start(g.start),
            summary: g.summary.map(|k| self.marker_start(k)),
            end: self.marker_start(g.end),
            title: g.title.clone(),
            open: g.open,
        }
    }

    /// The paired disclosures in document order of their starts.
    pub fn disclosures(&self) -> Vec<DisclosureGroup> {
        self.groups.iter().map(|g| self.group_at(g)).collect()
    }

    /// Whether two indexes hold the same markers and pairs (the reader's choices aside).
    pub fn same_disclosures(&self, other: &CrossBlockIndex) -> bool {
        let m = |c: &CrossBlockIndex| -> Vec<(usize, usize, RawPresentation)> {
            c.markers().into_iter().map(|m| (m.start, m.len, m.kind)).collect()
        };
        let g = |c: &CrossBlockIndex| -> Vec<(usize, Option<usize>, usize, String)> {
            c.disclosures()
                .into_iter()
                .map(|g| (g.start, g.summary, g.end, g.title))
                .collect()
        };
        m(self) == m(other) && g(self) == g(other)
    }

    /// Adds the markers among freshly flattened `items` (absolute origins), which follow every
    /// marker already known (a model build, in document order).
    pub(super) fn add_from(&mut self, items: &[Item]) {
        self.markers.extend_abs(markers_of(items));
    }

    /// An edit replaced the items starting at `removed` (pre-edit starts, ascending; `at`: the
    /// pre-edit position of the replacement when nothing was removed) by `new_items` (absolute
    /// origins after the edit) and shifted everything after them by `delta`. The markers
    /// follow in O(chunks); when markers went or came, the reader's choices of markers found
    /// again at their (mapped) positions carry over and the markers are paired again. Returns
    /// whether markers changed.
    pub(super) fn replace(
        &mut self,
        doc: &dyn FormatDoc,
        removed: &[usize],
        at: usize,
        new_items: &[Item],
        changed: std::ops::Range<usize>,
        delta: isize,
    ) -> bool {
        let old = match (removed.first(), removed.last()) {
            (Some(&a), Some(&b)) => a..b + 1,
            _ => at..at,
        };
        self.replace_range(doc, old, markers_of(new_items), changed, delta)
    }

    /// [`CrossBlockIndex::replace`] by byte range: the markers starting in `old` (pre-edit
    /// offsets) are replaced by `fresh` (absolute starts after the edit, document order).
    pub(super) fn replace_range(
        &mut self,
        doc: &dyn FormatDoc,
        old: std::ops::Range<usize>,
        fresh: Vec<DisclosureMarker>,
        changed: std::ops::Range<usize>,
        delta: isize,
    ) -> bool {
        let lo = self.markers.partition_point(|r| r.origin < old.start);
        let hi = self.markers.partition_point(|r| r.origin < old.end).max(lo);
        if lo == hi && fresh.is_empty() {
            // ordinary typing: only shifts (groups refer to markers by index)
            self.markers.splice(lo..lo, Vec::new(), delta);
            return false;
        }
        // choices of the replaced markers outside the changed bytes, at their new positions
        let toggles: Vec<(usize, bool)> = self
            .markers
            .range(lo..hi)
            .filter_map(|r| {
                let open = r.item.toggled?;
                let p = if r.origin < changed.start {
                    r.origin
                } else if r.origin >= changed.end {
                    (r.origin as isize + delta) as usize
                } else {
                    return None;
                };
                Some((p, open))
            })
            .collect();
        self.markers.splice(lo..hi, fresh, delta);
        self.reapply_toggles(&toggles);
        self.pair(doc);
        true
    }

    /// Clears the markers (a full rebuild collects them again); returns the reader's choices
    /// for markers whose positions `keep` maps to the rebuilt document.
    pub(super) fn take_toggles(&mut self, keep: impl Fn(usize) -> Option<usize>) -> Vec<(usize, bool)> {
        let out = self
            .markers
            .iter()
            .filter_map(|r| Some((keep(r.origin)?, r.item.toggled?)))
            .collect();
        self.markers = RelChunks::default();
        self.groups.clear();
        out
    }

    /// Re-applies toggles from [`CrossBlockIndex::take_toggles`] and pairs again.
    pub(super) fn restore_toggles(&mut self, doc: &dyn FormatDoc, toggles: &[(usize, bool)]) {
        self.reapply_toggles(toggles);
        self.pair(doc);
    }

    fn reapply_toggles(&mut self, toggles: &[(usize, bool)]) {
        for &(at, open) in toggles {
            let k = self.markers.partition_point(|r| r.origin < at);
            if self.markers.origin(k) == Some(at)
                && let Some(m) = self.markers.get_mut(k)
            {
                m.toggled = Some(open);
            }
        }
    }

    /// Sets the reader's open/closed choice of the disclosure starting at `start`; `false`
    /// when there is none.
    pub fn set_open(&mut self, start: usize, open: bool) -> bool {
        let k = self.markers.partition_point(|r| r.origin < start);
        if self.markers.origin(k) != Some(start) {
            return false;
        }
        if let Some(m) = self.markers.get_mut(k) {
            m.toggled = Some(open);
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.start == k)
            && g.open != open
        {
            g.open = open;
            self.gens.disclosures += 1;
        }
        true
    }

    /// Pairs the markers document-wide: a start opens a group in its container scope, an end
    /// closing `count` details closes that many open ones of its scope (ignored when fewer are
    /// open), a summary block right after a start in the same container gives the title of a
    /// start without its own.
    pub(super) fn pair(&mut self, doc: &dyn FormatDoc) {
        let text = doc.text();
        let ms = self.markers();
        let scopes: Vec<Vec<usize>> = ms
            .iter()
            .map(|m| doc.containers_at(m.start).iter().map(|c| c.span.start).collect())
            .collect();
        let mut groups: Vec<Group> = Vec::new();
        let mut stacks: std::collections::HashMap<&[usize], Vec<usize>> = Default::default();
        for (k, m) in ms.iter().enumerate() {
            match &m.kind {
                RawPresentation::DisclosureStart { .. } => stacks.entry(&scopes[k]).or_default().push(k),
                RawPresentation::DisclosureEnd { count } => {
                    let stack = stacks.entry(&scopes[k]).or_default();
                    if *count > stack.len() {
                        continue;
                    }
                    for _ in 0..*count {
                        let s = stack.pop().expect("an open start");
                        let RawPresentation::DisclosureStart { title, open } = &ms[s].kind else {
                            unreachable!()
                        };
                        let next = (s + 1 < ms.len())
                            .then_some(s + 1)
                            .filter(|&n| scopes[n] == scopes[s] && next_block(doc, text, &ms[s], &ms[n]));
                        let (title, summary) = match next.map(|n| (&ms[n].kind, n)) {
                            Some((RawPresentation::DisclosureSummary { title: t }, n)) if title.is_empty() => {
                                (t.clone(), Some(n))
                            }
                            _ => (title.clone(), None),
                        };
                        groups.push(Group {
                            start: s,
                            summary,
                            end: k,
                            title: if title.is_empty() { "Details".into() } else { title },
                            open: ms[s].toggled.unwrap_or(*open),
                        });
                    }
                }
                _ => {}
            }
        }
        groups.sort_by_key(|g| g.start);
        self.pairings += 1;
        let old = self.disclosures();
        self.groups = groups;
        if self.disclosures() != old {
            self.gens.disclosures += 1;
        }
    }

    /// Whether any group's source interval meets `lo..hi` (a page holding part of a disclosure:
    /// its height depends on the disclosure's state).
    pub fn disclosure_meets(&self, lo: usize, hi: usize) -> bool {
        self.groups
            .iter()
            .any(|g| self.marker_start(g.start) < hi && self.marker_start(g.end) >= lo)
    }
}

/// Whether marker `n` is the block right after marker `m` in their container (a separate
/// summary): the format's sibling query sees through container prefixes such as `> `; a
/// format without it: only whitespace between them.
fn next_block(doc: &dyn FormatDoc, text: &str, m: &DisclosureMarker, n: &DisclosureMarker) -> bool {
    match doc.adjacent_block(m.start, true) {
        Some(a) => a.leaf.map(|l| l.span.start) == Some(n.start) || a.block.start == n.start,
        None => text.get(m.start + m.len..n.start).is_some_and(|b| b.trim().is_empty()),
    }
}
