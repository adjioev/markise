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
//! - **Foldable callouts** (ADR 0025): a quote folded as one, without paired markers. Every fold
//!   is known by the start of its first leaf and its extent, read from the format's containers;
//!   the reader's choice lives on it the way it does on a `<details>` start. They need no pairing,
//!   so an edit touching one costs no document-wide work here.
//!
//! Headings, slugs and anchors are the outline ([`super::OutlineEntry`], document-wide in every
//! model); reference definitions are resolved by the format's own document-wide context.
//!
//! Markers are kept in a chunked relative-offset list (like the items): an edit shifts the ones
//! after it in O(chunks), and groups refer to markers by index, so ordinary typing (no marker
//! touched) costs no work per disclosure. Scopes are queried ([`FormatDoc::containers_at`])
//! only when the markers are paired again, i.e. when an edit touched one or changed whether a
//! summary is the block right after its start (the one thing pairing reads from other blocks).
//!
//! Format-neutral: markers come from [`FormatDoc::raw_presentation`] through the items.

use std::sync::Arc;

use markise_doc::chunked::{RelChunks, Relative};
use markise_doc::{EditSummary, FormatDoc, RawPresentation};

use super::Item;
use super::PageStats;

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

/// A foldable callout, by source position (absolute start while outside the chunked list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldMarker {
    /// Start of the quote's first leaf.
    pub start: usize,
    /// Bytes from `start` to the end of the quote.
    pub len: usize,
    /// As written: `+` open, `-` closed.
    pub written_open: bool,
    /// The reader's open/closed choice (`None`: as written).
    pub toggled: Option<bool>,
    /// What the body counts in a page's estimate: its hidden items as they are counted when
    /// shown, less the line the header keeps. From the items when they were last at hand; only a
    /// placeholder reads it, and a placeholder is made with its items in hand.
    pub body: Arc<PageStats>,
}

impl FoldMarker {
    pub fn open(&self) -> bool {
        self.toggled.unwrap_or(self.written_open)
    }
    pub fn end(&self) -> usize {
        self.start + self.len
    }
    /// Whether `at` lies in this callout, its first line included.
    pub fn holds(&self, at: usize) -> bool {
        self.start <= at && at <= self.end()
    }
}

impl Relative for FoldMarker {
    fn origin(&self) -> usize {
        self.start
    }
    fn shift(&mut self, delta: isize) {
        self.start = (self.start as isize + delta) as usize;
    }
}

/// Bytes from `start` (a callout's first leaf) to the end of its quote: the innermost container
/// there.
fn fold_len(doc: &dyn FormatDoc, start: usize) -> usize {
    doc.containers_at(start)
        .last()
        .map_or(0, |c| c.span.end.saturating_sub(start))
}

/// The foldable callouts among freshly flattened `items` (absolute origins, document order).
pub(super) fn folds_of(doc: &dyn FormatDoc, items: &[Item]) -> Vec<FoldMarker> {
    let mut out = Vec::new();
    for (k, it) in items.iter().enumerate() {
        for c in it.containers.iter() {
            if let super::Container::Quote { head: Some(head), .. } = c
                && let Some(written_open) = head.open
            {
                let len = fold_len(doc, it.origin);
                let end = it.origin + len;
                let mut body = PageStats::default();
                for hidden in items[k..].iter().take_while(|h| h.origin <= end) {
                    body.add(hidden);
                }
                body.keep_header();
                out.push(FoldMarker {
                    start: it.origin,
                    len,
                    written_open,
                    toggled: None,
                    body: Arc::new(body),
                });
            }
        }
    }
    out
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
    /// Foldable callouts in document order.
    folds: RelChunks<FoldMarker>,
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

    /// The foldable callouts in document order (absolute starts).
    pub fn folds(&self) -> Vec<FoldMarker> {
        self.folds.iter().map(|r| r.to_abs()).collect()
    }

    /// Starts of the closed folds holding `at`.
    pub(crate) fn closed_folds_around(&self, at: usize) -> Vec<usize> {
        let hi = self.folds.partition_point(|r| r.origin <= at);
        self.folds
            .range(0..hi)
            .map(|r| r.to_abs())
            .filter(|f| !f.open() && f.holds(at))
            .map(|f| f.start)
            .collect()
    }

    /// Calls `f` with the body of each closed fold starting in `lo..hi`. A fold inside a closed
    /// one is part of its body.
    pub(crate) fn closed_fold_bodies(&self, lo: usize, hi: usize, mut f: impl FnMut(&PageStats)) {
        let a = self.folds.partition_point(|r| r.origin < lo);
        let b = self.folds.partition_point(|r| r.origin < hi).max(a);
        let mut covered = lo;
        for r in self.folds.range(a..b) {
            if r.item.open() || r.origin < covered {
                continue;
            }
            covered = r.origin + r.item.len;
            f(&r.item.body);
        }
    }

    /// The fold starting at `start`.
    pub(crate) fn fold_at(&self, start: usize) -> Option<FoldMarker> {
        let k = self.folds.partition_point(|r| r.origin < start);
        (self.folds.origin(k) == Some(start))
            .then(|| self.folds.range(k..k + 1).next().map(|r| r.to_abs()))
            .flatten()
    }

    /// `items` (absolute origins) become a placeholder: the folds starting in them count what
    /// the items hold now (an edit in a body changed it without touching the fold's marker).
    pub(super) fn refresh_fold_bodies(&mut self, doc: &dyn FormatDoc, items: &[Item]) {
        for fresh in folds_of(doc, items) {
            let k = self.folds.partition_point(|r| r.origin < fresh.start);
            if self.folds.origin(k) == Some(fresh.start)
                && let Some(f) = self.folds.get_mut(k)
            {
                f.body = fresh.body;
            }
        }
    }

    /// The reader's choices of markers and folds at their positions after `steps` (see
    /// [`super::Moved`]): each patch of a composed edit moves them on its own, so one between
    /// two patches is inside the covering range without being touched. One inside a step goes.
    pub(super) fn choices_through(&self, steps: &[(usize, usize, isize)]) -> Vec<(usize, bool)> {
        let through = |mut p: usize| {
            for &(s, e, delta) in steps {
                if p >= e {
                    p = (p as isize + delta) as usize;
                } else if p >= s {
                    return None;
                }
            }
            Some(p)
        };
        let markers = self
            .markers
            .iter()
            .filter_map(|r| Some((through(r.origin)?, r.item.toggled?)));
        let folds = self
            .folds
            .iter()
            .filter_map(|r| Some((through(r.origin)?, r.item.toggled?)));
        markers.chain(folds).collect()
    }

    /// Whether two indexes hold the same markers and pairs (the reader's choices aside).
    pub fn same_disclosures(&self, other: &CrossBlockIndex) -> bool {
        let f = |c: &CrossBlockIndex| -> Vec<(usize, usize, bool)> {
            c.folds()
                .into_iter()
                .map(|f| (f.start, f.len, f.written_open))
                .collect()
        };
        if f(self) != f(other) {
            return false;
        }
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
    pub(super) fn add_from(&mut self, doc: &dyn FormatDoc, items: &[Item]) {
        self.markers.extend_abs(markers_of(items));
        self.folds.extend_abs(folds_of(doc, items));
    }

    /// The edit `e` replaced the items starting at `removed` (pre-edit starts, ascending) by
    /// `new_items` (absolute origins after the edit) and shifted everything after them. The
    /// markers follow in O(chunks); when markers went or came, the reader's choices of markers
    /// found again at their (mapped) positions carry over and the markers are paired again.
    /// Returns whether markers changed or a fold's end moved.
    pub(super) fn replace(
        &mut self,
        doc: &dyn FormatDoc,
        removed: &[usize],
        new_items: &[Item],
        e: &EditSummary,
    ) -> bool {
        let old = match (removed.first(), removed.last()) {
            (Some(&a), Some(&b)) => a..b + 1,
            _ => e.reparsed.start..e.reparsed.start,
        };
        self.replace_range(doc, old, markers_of(new_items), folds_of(doc, new_items), e)
    }

    /// The folds starting in `old` (pre-edit offsets) are replaced by `fresh`, and those before
    /// the edit get their new extents. `true` when any was replaced or a kept one's end moved.
    fn replace_folds(
        &mut self,
        doc: &dyn FormatDoc,
        old: &std::ops::Range<usize>,
        fresh: Vec<FoldMarker>,
        e: &EditSummary,
    ) -> bool {
        let (changed, delta) = (&e.changed, e.delta);
        let lo = self.folds.partition_point(|r| r.origin < old.start);
        let hi = self.folds.partition_point(|r| r.origin < old.end).max(lo);
        if lo == hi && fresh.is_empty() {
            self.folds.splice(lo..lo, Vec::new(), delta);
            return self.refresh_folds_before(doc, e);
        }
        let toggles: Vec<(usize, bool)> = self
            .folds
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
        let before: Vec<bool> = self.folds.range(lo..hi).map(|r| r.item.open()).collect();
        let n = fresh.len();
        self.folds.splice(lo..hi, fresh, delta);
        self.reapply_toggles(&toggles);
        let after: Vec<bool> = self.folds.range(lo..lo + n).map(|r| r.item.open()).collect();
        if before != after {
            self.gens.disclosures += 1;
        }
        self.refresh_folds_before(doc, e);
        true
    }

    /// An edit in a callout's body keeps its first leaf and so its marker: the extents of the
    /// folds starting before the edit in the re-parsed top-level blocks are read again. The edit
    /// need not be in them any more, since it can end the quote above itself, turning a lazy
    /// line into a block of its own. Other blocks are unchanged, so no fold there is looked at.
    ///
    /// `true` when a fold's end moved other than with the text typed in it: what it hides changed
    /// and the view must derive that again.
    fn refresh_folds_before(&mut self, doc: &dyn FormatDoc, e: &EditSummary) -> bool {
        let lo = self.folds.partition_point(|r| r.origin < e.reparsed.start);
        let hi = self.folds.partition_point(|r| r.origin <= e.changed.start).max(lo);
        let mut moved = false;
        for k in lo..hi {
            let Some(start) = self.folds.origin(k) else { continue };
            let len = fold_len(doc, start);
            if let Some(f) = self.folds.get_mut(k) {
                moved |= len != f.len && len as isize != f.len as isize + e.delta;
                f.len = len;
            }
        }
        if moved {
            self.gens.disclosures += 1;
        }
        moved
    }

    /// [`CrossBlockIndex::replace`] by byte range: the markers starting in `old` (pre-edit
    /// offsets) are replaced by `fresh` (absolute starts after the edit, document order), the
    /// folds by `fresh_folds`.
    pub(super) fn replace_range(
        &mut self,
        doc: &dyn FormatDoc,
        old: std::ops::Range<usize>,
        fresh: Vec<DisclosureMarker>,
        fresh_folds: Vec<FoldMarker>,
        e: &EditSummary,
    ) -> bool {
        let folds = self.replace_folds(doc, &old, fresh_folds, e);
        self.replace_markers(doc, old, fresh, e.changed.clone(), e.delta) | folds
    }

    fn replace_markers(
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
            // a block came or went between a start and the summary after it: that gives the title
            if !self.summary_link_changed(doc, lo) {
                return false;
            }
            let generation = self.gens.disclosures;
            self.pair(doc);
            return self.gens.disclosures != generation;
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

    /// Whether the group of the start just before marker `next` takes marker `next` as its summary
    /// now but did not when it was paired, or the other way round. Looks at these two markers only,
    /// so typing in a block between them costs no pairing.
    fn summary_link_changed(&self, doc: &dyn FormatDoc, next: usize) -> bool {
        let Some(start) = next.checked_sub(1) else {
            return false;
        };
        let at = self.groups.partition_point(|g| g.start < start);
        let Some(group) = self.groups.get(at).filter(|g| g.start == start) else {
            return false; // an unpaired start has no title to change
        };
        let (Some(m), Some(n)) = (
            self.markers.range(start..start + 1).next().map(|r| r.to_abs()),
            self.markers.range(next..next + 1).next().map(|r| r.to_abs()),
        ) else {
            return false;
        };
        let takes = matches!(&m.kind, RawPresentation::DisclosureStart { title, .. } if title.is_empty())
            && matches!(n.kind, RawPresentation::DisclosureSummary { .. })
            && scope(doc, m.start) == scope(doc, n.start)
            && next_block(doc, doc.text(), &m, &n);
        takes != (group.summary == Some(next))
    }

    /// Clears the markers (a full rebuild collects them again); returns the reader's choices
    /// for markers whose positions `keep` maps to the rebuilt document.
    pub(super) fn take_toggles(&mut self, keep: impl Fn(usize) -> Option<usize>) -> Vec<(usize, bool)> {
        let out = self
            .markers
            .iter()
            .filter_map(|r| Some((keep(r.origin)?, r.item.toggled?)))
            .chain(
                self.folds
                    .iter()
                    .filter_map(|r| Some((keep(r.origin)?, r.item.toggled?))),
            )
            .collect();
        self.markers = RelChunks::default();
        self.folds = RelChunks::default();
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
            let k = self.folds.partition_point(|r| r.origin < at);
            if self.folds.origin(k) == Some(at)
                && let Some(f) = self.folds.get_mut(k)
            {
                f.toggled = Some(open);
            }
        }
    }

    /// Sets the reader's open/closed choice of the disclosure starting at `start`; `false`
    /// when there is none.
    pub fn set_open(&mut self, start: usize, open: bool) -> bool {
        let k = self.folds.partition_point(|r| r.origin < start);
        if self.folds.origin(k) == Some(start)
            && let Some(f) = self.folds.get_mut(k)
        {
            let was = f.open();
            f.toggled = Some(open);
            if was != open {
                self.gens.disclosures += 1;
            }
            return true;
        }
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

    /// Takes over the reader's open/closed choices of `from` (an index of a document this one
    /// continues: progressive open, issue #126) for the disclosures starting at the same
    /// positions here.
    pub fn adopt_choices(&mut self, from: &CrossBlockIndex) {
        for m in from.markers() {
            if let Some(open) = m.toggled {
                self.set_open(m.start, open);
            }
        }
        for f in from.folds() {
            if let Some(open) = f.toggled {
                self.set_open(f.start, open);
            }
        }
    }

    /// Pairs the markers document-wide: a start opens a group in its container scope, an end
    /// closing `count` details closes that many open ones of its scope (ignored when fewer are
    /// open), a summary block right after a start in the same container gives the title of a
    /// start without its own.
    pub(super) fn pair(&mut self, doc: &dyn FormatDoc) {
        let text = doc.text();
        let ms = self.markers();
        let scopes: Vec<Vec<usize>> = ms.iter().map(|m| scope(doc, m.start)).collect();
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
        let fold = self.folds.partition_point(|r| r.origin < lo);
        self.folds.origin(fold).is_some_and(|s| s < hi)
            || self
                .groups
                .iter()
                .any(|g| self.marker_start(g.start) < hi && self.marker_start(g.end) >= lo)
    }
}

/// The container scope of a marker: markers pair only within one.
fn scope(doc: &dyn FormatDoc, at: usize) -> Vec<usize> {
    doc.containers_at(at).iter().map(|c| c.span.start).collect()
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
