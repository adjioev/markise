//! The view model of a document: its leaf blocks flattened into a list of [`Item`]s (the unit
//! of layout, caching and virtualisation), the outline, and the document-level dependencies.
//!
//! The model is built from the format-neutral views of `markise-doc` ([`FormatDoc::flatten`],
//! [`FormatDoc::heading`], …; ADR 0006 §1.4) and does not own the document (C-2): layout reads
//! the document through `&dyn FormatDoc`. Items are **item-relative** (ADR 0006 §2.3, C-18):
//! an item stores no absolute offsets — its origin lives in a chunked relative-offset list
//! ([`Items`], gate G1) — so its hash and its cached layout stay valid when text above it
//! changes. [`DocModel::apply`] updates the model from an edit's [`EditSummary`]: only the
//! items of the re-parsed top-level blocks (or, for an intra-line edit, of one leaf) are
//! rebuilt (ADR 0006 §7.2), and dependencies are invalidated per heading key and `[toc]`
//! (§7.4). Reference-definition dependencies are not part of item hashes: the view folds the
//! fingerprints of the labels an item may look up ([`DocModel::item_references`]) into its
//! cache key, computed on first use, so building the model needs no document-wide label scan.
//!
//! Building a [`DocModel`] touches every block once (no inline parsing except heading text,
//! no shaping), so it is cheap enough to run on the loader thread right after parsing.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use markise_doc::chunked::{RelChunks, Relative};
use markise_doc::{
    CalloutKind, EditSummary, FlatContainer, FlatEvent, FlatLeaf, FormatDoc, LeafKind, ListMarker, Span,
};

/// One enclosing container of a leaf item, outermost first.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    Quote {
        /// A callout (GFM alert) instead of a plain quote.
        callout: Option<CalloutKind>,
        /// This is the quote's first leaf (its bar starts here; callouts draw their title).
        first: bool,
    },
    ListItem {
        /// Text of the marker to draw (`•`, `3.`, …) — only on the item's first leaf.
        marker: Option<Marker>,
        tight: bool,
    },
    Footnote {
        /// `[^label]` shown on the definition's first leaf.
        label: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Marker {
    Bullet { depth: u8 },
    Number(String),
    Task { checked: bool },
}

/// What a leaf item renders.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Paragraph,
    Heading {
        level: u8,
    },
    /// Fenced or indented code. `lang` is the first word of the info string.
    Code {
        lang: Box<str>,
    },
    /// Display math, shown as styled TeX source in M1 (placeholder, like the HTML export C36).
    Math,
    /// Diagram fence (`mermaid`, `flow`, `sequence`): rendered through [`crate::Diagrams`] when
    /// diagrams are on (the first segment draws the scene, later segments are empty), else
    /// styled source, segmented like code.
    Diagram {
        lang: Box<str>,
    },
    /// Raw HTML block, shown as source (M1 placeholder).
    Html,
    Table,
    Rule,
    FrontMatter,
    Toc,
    /// Link reference definition: not rendered (zero height), like the export.
    Hidden,
    /// An empty list item (`- `): only its marker is drawn.
    Empty,
    /// A page of top-level blocks of a paged model that is not materialized (a placeholder,
    /// see [`paged`]): never laid out or shown (the view materializes it first).
    Page(Box<paged::PageStats>),
}

/// Vertical gap above an item, resolved to points by the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gap {
    None,
    /// Between items of a tight list.
    Tight,
    Normal,
}

/// Units (lines / rows / TOC entries) per segment of a large block.
pub const SEG_UNITS: usize = 64;
/// Bytes per segment (soft cap: a segment ends after the unit that crosses it).
pub const SEG_BYTES: usize = 16 * 1024;
/// Lines longer than this are split into pieces (each piece renders on its own row).
pub const MAX_UNIT_BYTES: usize = 4 * 1024;
/// Blocks with more units or bytes than this are split into segments.
pub const SEG_MIN_UNITS: usize = 2 * SEG_UNITS;
pub const SEG_MIN_BYTES: usize = 2 * SEG_BYTES;

/// Part of a large block. Large code/math/HTML blocks, paragraphs, tables and `[toc]`s are
/// split into several items so that layout, caching and painting stay bounded per item
/// (viewport virtualisation inside a block).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Content pieces (lines, or pieces of over-long lines) of a code-like block or paragraph,
    /// **relative to the item's origin** (the leaf's first byte): offset-independent, so the
    /// item hash survives edits above the block (C-18).
    Lines {
        spans: Box<[Span]>,
        first: bool,
        last: bool,
        /// Hash of the pieces' texts (the segment's content; lets an edit re-key the other
        /// segments of the block without re-reading their text).
        text_hash: u64,
    },
    /// Table body rows `start..end` (indices into the table's rows); the first segment also
    /// draws the header. Column widths come from the header and the first [`SEG_UNITS`] body
    /// rows (`widths_hash` covers them).
    Rows {
        start: u32,
        end: u32,
        first: bool,
        last: bool,
        /// Boxed: keeps [`Segment`] (and so every [`Item`]) small (issue #29, part 4).
        data: Box<RowsData>,
    },
    /// Outline entries `start..end` of a `[toc]`.
    Toc {
        start: u32,
        end: u32,
        first: bool,
        last: bool,
    },
}

/// The larger part of a [`Segment::Rows`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RowsData {
    pub widths_hash: u64,
    /// Hash of this segment's row texts and their offsets in the table (cached layouts
    /// carry table-relative source positions).
    pub rows_hash: u64,
    /// Estimated natural width of each column in characters (shared by all segments).
    pub col_chars: Box<[u32]>,
}

impl Segment {
    pub fn first(&self) -> bool {
        match self {
            Segment::Lines { first, .. } | Segment::Rows { first, .. } | Segment::Toc { first, .. } => *first,
        }
    }

    pub fn last(&self) -> bool {
        match self {
            Segment::Lines { last, .. } | Segment::Rows { last, .. } | Segment::Toc { last, .. } => *last,
        }
    }
}

/// Identity and position of a code item's text, for syntax highlighting. The block's
/// *content* is its lines, each followed by `\n` (fence lines excluded); highlight runs use
/// byte offsets into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodeRef {
    /// Hash of the fence language (lowercase) and every content line of the block: the key of
    /// highlight results (identical blocks share them).
    pub key: u64,
    /// Content offset of this item's first line piece (0 unless it is a later segment).
    pub offset: usize,
    /// Content length of the whole block.
    pub len: usize,
}

/// A leaf block (or a segment of one) plus its container context. Item-relative: the item's
/// absolute start is [`DocModel::item_start`]; offsets inside it (segment spans) are relative
/// to that start.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    /// Boxed: rare, and the item record is per leaf (issue #29, part 4).
    pub raw: Option<Box<markise_doc::RawPresentation>>,
    /// Byte length of the leaf block's span (all segments of a block share it).
    pub len: usize,
    pub containers: Box<[Container]>,
    pub gap: Gap,
    /// Hash of everything the item's layout depends on besides width/theme/images/reference
    /// definitions: kind, item-relative source, containers, gap, segment (and the outline for
    /// a `[toc]`).
    pub hash: u64,
    /// The part of [`Item::hash`] that depends on the item alone (offset-independent).
    pub content_hash: u64,
    /// Rough size statistics for height estimates before layout.
    pub est_chars: u32,
    pub est_lines: u32,
    /// The source mentions an image (`![` or `<img`): layout depends on image load state.
    pub has_image: bool,
    /// A math block, or source that may contain inline math (`$`, `\(`): layout depends on
    /// math render state ([`crate::math`]). Over-approximated; only costs a re-layout.
    pub has_math: bool,
    /// The source contains `[[` (a possible wikilink): its style depends on which link targets
    /// exist ([`crate::links`]). Over-approximated; only costs a re-layout.
    pub has_wikilink: bool,
    /// Set when this item is one part of a large block.
    pub segment: Option<Segment>,
    /// Set for fenced code with a language (highlighting input) and for diagram fences (diagram
    /// input; `key` identifies the whole block's content).
    /// Boxed: only code and diagram items have one (keeps [`Item`] small, issue #29, part 4).
    pub code: Option<Box<CodeRef>>,
    /// Absolute start while the item is outside the model; 0 inside it (see [`Items`]).
    origin: usize,
}

impl Relative for Item {
    fn origin(&self) -> usize {
        self.origin
    }
    fn shift(&mut self, delta: isize) {
        self.origin = (self.origin as isize + delta) as usize;
    }
}

/// The items of a model in a chunked relative-offset list (gate G1): replacing items and
/// shifting the ones after an edit is O(chunk + chunks), not O(items).
#[derive(Clone, Debug, Default)]
pub struct Items(RelChunks<Item>);

impl Items {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn get(&self, i: usize) -> Option<&Item> {
        self.0.get(i).map(|r| r.item)
    }
    /// Absolute start of item `i`.
    pub fn start(&self, i: usize) -> Option<usize> {
        self.0.origin(i)
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Item> + '_ {
        self.0.iter().map(|r| r.item)
    }
    /// `(absolute start, item)` pairs in order.
    pub fn iter_with_start(&self) -> impl DoubleEndedIterator<Item = (usize, &Item)> + '_ {
        self.0.iter().map(|r| (r.origin, r.item))
    }
    fn get_mut(&mut self, i: usize) -> Option<&mut Item> {
        self.0.get_mut(i)
    }
    /// Index of the first item starting at or after absolute offset `start`.
    pub fn partition_point_start(&self, start: usize) -> usize {
        self.0.partition_point(|r| r.origin < start)
    }
    fn partition_point(&self, mut pred: impl FnMut(usize, &Item) -> bool) -> usize {
        self.0.partition_point(|r| pred(r.origin, r.item))
    }
}

impl std::ops::Index<usize> for Items {
    type Output = Item;
    fn index(&self, i: usize) -> &Item {
        &self.0[i]
    }
}

/// One outline entry (heading).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    pub level: u8,
    pub text: String,
    /// Anchor id (spec 01 §7 slug rule, unique per document).
    pub anchor: String,
    /// Index of the heading's item.
    pub item: usize,
    /// Anchor key (base slug reported by the format, lower-cased).
    pub key: String,
}

/// A parsed document prepared for viewing.
#[derive(Debug, Clone)]
pub struct DocModel {
    pub items: Items,
    pub outline: Vec<OutlineEntry>,
    /// Hash of the inputs image resolution depends on (resource root, document folder): part
    /// of the cache key of items that show images. The only model-side input of the view's
    /// context key; when [`DocModel::apply`] changes it, the image items are reported in
    /// [`ModelChange::rehashed`].
    pub ctx_hash: u64,
    /// Folder of the document, for resolving relative image paths.
    pub base_dir: Option<PathBuf>,
    /// Resource root of the document (Markdown: `typora-root-url`).
    pub root_url: Option<String>,
    /// The `[toc]` blocks: `(first item, item count)` of each, in order (kept in step by
    /// [`DocModel::apply`], so outline changes re-key TOC items without scanning the items).
    toc_runs: Vec<(usize, usize)>,
    /// Lowest heading level of the outline (TOC indentation base; 1 when empty).
    min_level: u8,
    /// Top-level blocks per page of a paged model ([`paged`]); 0: a full model.
    page_tops: usize,
}

mod paged;
pub use paged::{PAGE_TOPS, PageStats, Paging, PagingCounters, paging_counters};

pub(crate) fn hasher() -> ahash::AHasher {
    // fixed keys: hashes must be stable within a process run (cache keys), not across runs
    ahash::RandomState::with_seeds(0x6d61_726b, 0x6973_652d, 0x6c61_796f, 0x7574_2121).build_hasher()
}

/// What an [`DocModel::apply`] changed, for the view (heights / anchor).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelChange {
    /// Items replaced (indices before the change) …
    pub replaced: std::ops::Range<usize>,
    /// … by these items (indices after the change).
    pub inserted: std::ops::Range<usize>,
    /// Other items whose hash changed (indices after the change; dependency invalidation).
    pub rehashed: Vec<usize>,
    /// Items whose hash changed but nothing their height estimate reads (`[toc]` items whose
    /// entries changed but not their number): the view keeps their heights and only marks laid-out ones for
    /// a new layout, so a large re-keying costs O(1) per item.
    pub rekeyed: Vec<usize>,
    /// Reference labels whose definitions changed: items that may use them get new cache keys
    /// (the view folds reference fingerprints into its keys, ADR 0006 §7.4).
    pub labels: Vec<u64>,
    /// Further splices besides `replaced` → `inserted`: `[toc]` blocks outside the edit whose
    /// number of segments followed the outline gain or lose segments. Each is `(old range, new
    /// range)` like that pair: old ranges number the items before the change, new ranges (and
    /// `inserted`, `rehashed`, `rekeyed`) after it. Together with `replaced` they are disjoint
    /// and ascending ([`ModelChange::splices`]).
    pub resized: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    /// The whole model was rebuilt.
    pub full: bool,
}

impl ModelChange {
    /// Every splice of the change (`replaced` → `inserted` and [`ModelChange::resized`]) as
    /// `(old range, new range)`, ascending and disjoint.
    pub fn splices(&self) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
        let mut v = Vec::with_capacity(self.resized.len() + 1);
        v.push((self.replaced.clone(), self.inserted.clone()));
        v.extend(self.resized.iter().cloned());
        // an insertion at the end of a `[toc]` block that ends where the edit starts comes first
        v.sort_by_key(|(o, n)| (o.start, o.end, n.start));
        v
    }

    /// The new index of old item `i` (`None` when it was replaced); `splices` as returned by
    /// [`ModelChange::splices`].
    pub fn map_index(splices: &[(std::ops::Range<usize>, std::ops::Range<usize>)], i: usize) -> Option<usize> {
        // the last splice ending at or before `i` gives the shift (`new.end - old.end` is the
        // total shift after it)
        let k = splices.partition_point(|(o, _)| o.end <= i);
        if let Some((o, _)) = splices.get(k)
            && o.start <= i
        {
            return None;
        }
        Some(match k.checked_sub(1) {
            Some(j) => {
                let (o, n) = &splices[j];
                (i as isize + n.end as isize - o.end as isize) as usize
            }
            None => i,
        })
    }
}

impl DocModel {
    /// Builds the view model of `doc`, read from `path` (if any).
    pub fn build(doc: &dyn FormatDoc, path: Option<&Path>) -> Arc<DocModel> {
        Arc::new(Self::build_owned(doc, path))
    }

    /// Like [`DocModel::build`], unshared (for owners that update it with [`DocModel::apply`]).
    /// Paged by default ([`Paging::from_env`]).
    pub fn build_owned(doc: &dyn FormatDoc, path: Option<&Path>) -> DocModel {
        Self::build_with(doc, path, Paging::from_env())
    }

    /// Like [`DocModel::build_owned`] with an explicit [`Paging`] (a format without
    /// [`FormatDoc::top_at`] always gets a full model).
    pub fn build_with(doc: &dyn FormatDoc, path: Option<&Path>, paging: Paging) -> DocModel {
        match paging {
            Paging::Paged { tops } if tops > 0 && doc.top_at(0).is_some() => Self::build_paged(doc, path, tops),
            _ => Self::build_full(doc, path),
        }
    }

    /// A full model: one item per leaf (segment) of the whole document.
    fn build_full(doc: &dyn FormatDoc, path: Option<&Path>) -> DocModel {
        // flattened in batches straight into the chunked list (no full-size intermediate)
        let mut chunks = RelChunks::default();
        let n = doc.top_count();
        let mut k = 0;
        while k < n {
            let end = (k + 512).min(n);
            let doc_start = chunks.is_empty();
            chunks.extend_abs(flatten(doc, k..end, doc_start));
            k = end;
        }
        let base_dir = path.and_then(|p| p.parent()).map(Path::to_path_buf);
        let root_url = doc.resource_root();
        let mut m = DocModel {
            items: Items(chunks),
            outline: Vec::new(),
            ctx_hash: images_hash(&root_url, &base_dir),
            base_dir,
            root_url,
            toc_runs: Vec::new(),
            min_level: 1,
            page_tops: 0,
        };
        m.rebuild_outline(doc);
        m.update_tocs(doc);
        m
    }

    /// Updates the model after `doc` changed by `e` (the summary of the edit that produced
    /// `doc`'s current state from the state this model was built for). Rebuilds the items of
    /// the re-parsed top-level blocks only, shifts the rest (O(chunk + chunks), gate G1), and
    /// re-keys items whose dependencies changed (§7.4): items that may use a changed reference
    /// label, headings whose anchor key collides with a changed one, `[toc]` items when the
    /// outline changed. Equivalent to a full [`DocModel::build`] (property-tested).
    ///
    /// Contract with the format ([`EditSummary`]): the replaced top-level blocks covered the
    /// bytes `reparsed.start .. reparsed.end - delta` before the edit, the new ones cover
    /// `reparsed` after it.
    pub fn apply(&mut self, doc: &dyn FormatDoc, e: &EditSummary) -> ModelChange {
        if self.is_paged() && (e.deps.all || self.edit_touches_page(e)) {
            return self.rebuild_paged(doc, e.deps.labels.clone());
        }
        if e.deps.all {
            let n = self.items.len();
            let base_dir = self.base_dir.clone();
            let mut fresh = Self::build_with(doc, None, self.paging());
            fresh.base_dir = base_dir;
            fresh.ctx_hash = images_hash(&fresh.root_url, &fresh.base_dir);
            *self = fresh;
            return ModelChange {
                replaced: 0..n,
                inserted: 0..self.items.len(),
                full: true,
                ..Default::default()
            };
        }
        let mut change = ModelChange::default();
        // the outline is updated in place: entries `at..at + added` replaced `old`
        let mut edit: Option<OutlineEdit> = None;
        let mut affected: HashSet<String> = HashSet::new();
        // the `[toc]` runs after the splice, each with its start before the change (`None`: a
        // `[toc]` block the edit inserted, keyed in full below)
        let mut run_old: Vec<Option<usize>> = self.toc_runs.iter().map(|r| Some(r.0)).collect();
        // first index of the inserted items (before `[toc]` resizes elsewhere)
        let mut lo_new = 0;
        if !(e.replaced_top.is_empty() && e.new_top.is_empty()) {
            // items of the replaced blocks: origins in [reparsed.start, old resume point)
            let lo_pos = e.reparsed.start;
            let hi_pos = (e.reparsed.end as isize - e.delta) as usize;
            let mut lo = self.items.partition_point(|o, _| o < lo_pos);
            let mut hi = self.items.partition_point(|o, _| o < hi_pos).max(lo);
            // intra-line fast path (§7.2.1): only the items of one leaf are rebuilt, with the
            // container context of the old ones (the structure is unchanged)
            let fast = e.inline_leaf.and_then(|start| {
                let a = self.items.partition_point(|o, _| o < start);
                let b = self.items.partition_point(|o, _| o <= start);
                if b > a + 1
                    && let Some(items) = self.refresh_segment(doc, a, b, start, e)
                {
                    return Some((a, b, items));
                }
                let leaf = doc.leaf(start)?;
                (b > a && leaf.span.start == start).then(|| (a, b, self.refresh_leaf(doc, a, &leaf)))
            });
            // nested restart (§7.2.3): only the leaves from `nested_from` to the re-parse end
            let nested = e.nested_from.filter(|_| fast.is_none()).map(|from| {
                let a = self.items.partition_point(|o, _| o < from);
                let mut items = flatten_region(doc, e.new_top.clone(), from, e.reparsed.end, a == 0);
                items.retain(|it| it.origin >= from && it.origin < e.reparsed.end);
                (a, items)
            });
            let new_items = match (fast, nested) {
                (Some((a, b, items)), _) => {
                    (lo, hi) = (a, b);
                    items
                }
                (None, Some((a, items))) => {
                    lo = a;
                    hi = hi.max(lo);
                    items
                }
                (None, None) => flatten(doc, e.new_top.clone(), lo == 0),
            };
            let is_head = |it: &Item| matches!(it.kind, ItemKind::Heading { .. });
            let old_heads = self.items.0.range(lo..hi).any(|r| is_head(r.item));
            let new_heads = new_items.iter().any(is_head);
            // inserted `[toc]` blocks come segmented for the outline after the edit (keyed once
            // the outline is final)
            let mut new_items = new_items;
            let mut new_runs: Vec<(usize, usize)> = Vec::new(); // (offset in `new_items`, count)
            if new_items
                .iter()
                .any(|it| it.kind == ItemKind::Toc && it.segment.is_none())
            {
                let removed =
                    self.outline.partition_point(|o| o.item < hi) - self.outline.partition_point(|o| o.item < lo);
                let n = self.outline.len() - removed + new_items.iter().filter(|it| is_head(it)).count();
                let groups = toc_groups(n);
                let mut out = Vec::with_capacity(new_items.len());
                for it in new_items {
                    if it.kind == ItemKind::Toc && it.segment.is_none() {
                        let seg = toc_block_items(&it, groups, n);
                        new_runs.push((out.len(), seg.len()));
                        out.extend(seg);
                    } else {
                        out.push(it);
                    }
                }
                new_items = out;
            }
            let n_new = new_items.len();
            self.items.0.splice(lo..hi, new_items, e.delta);
            change.replaced = lo..hi;
            change.inserted = lo..lo + n_new;
            lo_new = lo;
            let shift = n_new as isize - (hi - lo) as isize;
            // `[toc]` runs: the replaced ones go (a run lies wholly inside or outside the
            // replaced items: its items share one origin), the inserted ones come in
            let mut runs: Vec<(usize, usize)> = Vec::with_capacity(self.toc_runs.len() + new_runs.len());
            run_old.clear();
            let at = self.toc_runs.partition_point(|r| r.0 < lo);
            for &r in &self.toc_runs[..at] {
                runs.push(r);
                run_old.push(Some(r.0));
            }
            for &(off, k) in &new_runs {
                runs.push((lo + off, k));
                run_old.push(None);
            }
            for &r in &self.toc_runs[at..] {
                if r.0 >= hi {
                    runs.push(((r.0 as isize + shift) as usize, r.1));
                    run_old.push(Some(r.0));
                }
            }
            self.toc_runs = runs;
            // the first item of the document has no gap: re-flatten the block after the
            // change when it became / stopped being the first one
            if lo == 0 && (hi == 0) != (n_new == 0) && e.new_top.end < doc.top_count() {
                // the first following block that has items (empty quotes, definitions… have
                // none and do not take the "first item" status)
                let mut k = e.new_top.end;
                let mut block_items = flatten(doc, k..k + 1, n_new == 0);
                while block_items.is_empty() && k + 1 < doc.top_count() && k < e.new_top.end + 64 {
                    k += 1;
                    block_items = flatten(doc, k..k + 1, n_new == 0);
                }
                let cnt = block_items.len();
                // the block's current items: every item from `n_new` up to the last origin of
                // the block (segments share their block's origin)
                let last_origin = block_items.iter().map(|it| it.origin).max().unwrap_or(0);
                let n_items = self.items.len();
                let old_cnt = (n_new..n_items)
                    .take_while(|&i| self.items.start(i).is_some_and(|o| o <= last_origin))
                    .count();
                if old_cnt != cnt || block_items.iter().any(|it| it.kind == ItemKind::Toc) {
                    // a segmented block (e.g. a `[toc]`, segmented by the outline) does not map
                    // item for item: rebuild (rare: the document's first block changed status)
                    let n_before = (n_items as isize - shift) as usize;
                    let base_dir = self.base_dir.clone();
                    let mut fresh = Self::build_with(doc, None, self.paging());
                    fresh.base_dir = base_dir;
                    fresh.ctx_hash = images_hash(&fresh.root_url, &fresh.base_dir);
                    *self = fresh;
                    return ModelChange {
                        replaced: 0..n_before,
                        inserted: 0..self.items.len(),
                        labels: e.deps.labels.clone(),
                        full: true,
                        ..Default::default()
                    };
                }
                self.items.0.splice(n_new..n_new + cnt, block_items, 0);
                change.rehashed.extend(n_new..n_new + cnt);
            }
            if old_heads || new_heads {
                // only the entries of the replaced items change; the others shift (O(headings)
                // integer updates, no text copies)
                let ka = self.outline.partition_point(|o| o.item < lo);
                let kb = self.outline.partition_point(|o| o.item < hi);
                for o in &self.outline[ka..kb] {
                    affected.insert(o.key.clone());
                }
                let mut fresh = Vec::new();
                for (i, r) in (lo..).zip(self.items.0.range(lo..lo + n_new)) {
                    if let ItemKind::Heading { level } = r.item.kind {
                        let info = doc.heading(r.origin).unwrap_or_default();
                        let key = info.key.to_lowercase();
                        affected.insert(key.clone());
                        fresh.push(OutlineEntry {
                            level,
                            text: info.text.trim().to_string(),
                            anchor: String::new(), // numbered below (its key is affected)
                            item: i,
                            key,
                        });
                    }
                }
                if shift != 0 {
                    for o in &mut self.outline[kb..] {
                        o.item = (o.item as isize + shift) as usize;
                    }
                }
                let added = fresh.len();
                let old: Vec<OutlineEntry> = self.outline.splice(ka..kb, fresh).collect();
                edit = Some(OutlineEdit { at: ka, old, added });
            } else if shift != 0 {
                let k = self.outline.partition_point(|o| o.item < hi);
                for o in &mut self.outline[k..] {
                    o.item = (o.item as isize + shift) as usize;
                }
            }
        }
        if e.deps.resources {
            self.root_url = doc.resource_root();
            let h = images_hash(&self.root_url, &self.base_dir);
            if h != self.ctx_hash {
                self.ctx_hash = h;
                // every item showing an image has a new cache key (the view folds `ctx_hash`
                // into the keys of image items): report them so their heights are re-estimated
                change.rehashed.extend(
                    self.items
                        .iter()
                        .enumerate()
                        .filter(|(_, it)| it.has_image)
                        .map(|(i, _)| i),
                );
            }
        }
        // outline entries whose text, level or anchor may have changed
        let mut touched: Vec<usize> = Vec::new();
        // reference definitions changed (§7.4): cache keys of the items that may use them change
        // in the view (it folds their fingerprints into the keys); here only heading texts
        // (outline, anchors) need refreshing — a scan over the headings
        if !e.deps.labels.is_empty() {
            change.labels = e.deps.labels.clone();
            let changed: HashSet<u64> = e.deps.labels.iter().copied().collect();
            // (entry, heading start) of the headings using a changed label; the headings of
            // a page placeholder are read from its blocks (paged models)
            let mut retext: Vec<(usize, usize)> = Vec::new();
            let mut k = 0;
            while k < self.outline.len() {
                let i = self.outline[k].item;
                if let Some(ItemKind::Page(st)) = self.items.get(i).map(|it| &it.kind) {
                    let kb = self.outline.partition_point(|o| o.item <= i);
                    if st.heading_refs == 0 {
                        k = kb;
                        continue;
                    }
                    let heads = self
                        .page_items(doc, i)
                        .into_iter()
                        .filter(|(_, it)| matches!(it.kind, ItemKind::Heading { .. }));
                    for (j, (start, it)) in (k..kb).zip(heads) {
                        if references_of(doc, &it, start).iter().any(|r| changed.contains(r)) {
                            retext.push((j, start));
                        }
                    }
                    k = kb;
                    continue;
                }
                if self.item_references(doc, i).iter().any(|r| changed.contains(r)) {
                    retext.push((k, self.item_start(i)));
                }
                k += 1;
            }
            for (k, start) in retext {
                let info = doc.heading(start).unwrap_or_default();
                let key = info.key.to_lowercase();
                let o = &mut self.outline[k];
                if key != o.key {
                    affected.insert(std::mem::replace(&mut o.key, key.clone()));
                    affected.insert(key);
                }
                o.text = info.text.trim().to_string();
                touched.push(k);
            }
        }
        // anchors of the affected keys are renumbered in document order (R7.2: only anchors
        // sharing a key influence each other); the others are kept
        if !affected.is_empty() {
            touched.extend(self.renumber_anchors(doc, &affected));
        }
        let old_count = match &edit {
            Some(ed) => {
                // replaced entries that came back unchanged do not touch the outline
                if ed.old.len() == ed.added {
                    touched.extend(
                        (ed.at..ed.at + ed.added).filter(|&k| !same_entry(&self.outline[k], &ed.old[k - ed.at])),
                    );
                } else {
                    touched.extend(ed.at..ed.at + ed.added);
                }
                self.outline.len() - ed.added + ed.old.len()
            }
            None => self.outline.len(),
        };
        let count_changed = old_count != self.outline.len();
        let mut min_changed = false;
        if edit.is_some() || !touched.is_empty() {
            let min_level = self.outline.iter().map(|o| o.level).min().unwrap_or(1);
            min_changed = min_level != self.min_level;
            self.min_level = min_level;
        }
        let outline_changed = count_changed || min_changed || !touched.is_empty();
        if !self.toc_runs.is_empty() && (outline_changed || run_old.iter().any(Option::is_none)) {
            // entries from here on moved (their indices shifted)
            let moved = edit.as_ref().filter(|_| count_changed).map(|ed| ed.at);
            let shape = TocChange {
                old_count,
                moved,
                min_changed,
                outline_changed,
                touched: &touched,
                run_old: &run_old,
                lo: lo_new,
            };
            self.sync_tocs(doc, &shape, &mut change);
        }
        change
    }

    /// Brings the `[toc]` blocks in step with the outline after an edit (§7.4), without a scan
    /// over the items: blocks outside the edit whose number of segments changed gain or lose
    /// trailing segments ([`ModelChange::resized`]); their segments showing changed entries are
    /// re-keyed in place; blocks the edit inserted are keyed in full.
    fn sync_tocs(&mut self, doc: &dyn FormatDoc, c: &TocChange<'_>, change: &mut ModelChange) {
        let n = self.outline.len();
        let (g_old, g_new) = (toc_groups(c.old_count), toc_groups(n));
        if g_old != g_new {
            self.resize_tocs(doc, c, g_old, g_new, change);
        }
        let runs = self.toc_runs.clone();
        let is_new = |ri: usize| c.run_old[ri].is_none();
        if g_new == 0 {
            // unsegmented TOCs (a small outline): each depends on the whole outline
            let oh = self.outline_hash();
            for (ri, &(a, _)) in runs.iter().enumerate() {
                if !c.outline_changed && !is_new(ri) {
                    continue;
                }
                if let Some(it) = self.items.get_mut(a) {
                    it.hash = mix(it.content_hash, oh);
                }
                if is_new(ri) {
                    continue;
                }
                // the estimate reads the number of entries (issue #124)
                if c.old_count != n {
                    change.rehashed.push(a);
                } else {
                    change.rekeyed.push(a);
                }
            }
            return;
        }
        let groups = g_new;
        let key_of = |m: &DocModel, gi: usize| -> (usize, Segment, u64, u32) {
            let r = gi * SEG_UNITS..((gi + 1) * SEG_UNITS).min(n);
            let seg = toc_segment(gi, groups, n);
            let key = toc_segment_key(&seg, m.toc_group_hash(r.clone()));
            (gi, seg, key, r.len() as u32)
        };
        // segments of the blocks the edit inserted: all
        if runs.iter().enumerate().any(|(ri, _)| is_new(ri)) {
            let all: Vec<(usize, Segment, u64, u32)> = (0..groups).map(|gi| key_of(self, gi)).collect();
            for (ri, &(a, k)) in runs.iter().enumerate() {
                if !is_new(ri) {
                    continue;
                }
                debug_assert_eq!(k, groups, "new TOC run of {k} items for {groups} segments");
                for (it, s) in self.items.0.range_mut(a..a + k).zip(&all) {
                    it.hash = mix(it.content_hash, s.2);
                }
            }
        }
        if !c.outline_changed {
            return;
        }
        // dirty segments of the other blocks, ascending
        let dirty: Vec<usize> = if c.min_changed || g_old == 0 {
            (0..groups).collect()
        } else if let Some(m) = c.moved.or((g_old != g_new).then_some(0)) {
            // the last segment before a resize is no longer (or newly) the last one
            let mut from = (m / SEG_UNITS).min(groups - 1);
            if g_old != g_new {
                from = from.min(g_old.min(g_new) - 1);
            }
            let mut d: Vec<usize> = c.touched.iter().map(|&k| k / SEG_UNITS).filter(|&x| x < from).collect();
            d.extend(from..groups);
            d.sort_unstable();
            d.dedup();
            d
        } else {
            let mut d: Vec<usize> = c
                .touched
                .iter()
                .map(|&k| k / SEG_UNITS)
                .filter(|&x| x < groups)
                .collect();
            d.sort_unstable();
            d.dedup();
            d
        };
        let segs: Vec<(usize, Segment, u64, u32)> = dirty.iter().map(|&gi| key_of(self, gi)).collect();
        // many dirty segments (from `first` on, after a count or level change): walk each TOC's
        // items in order instead of looking each one up
        let first = segs.first().map_or(groups, |s| s.0);
        change.rekeyed.reserve(segs.len() * runs.len());
        let walk = segs.len() * 4 >= groups - first;
        let mut by_seg: Vec<Option<usize>> = vec![None; if walk { groups } else { 0 }];
        if walk {
            for (j, s) in segs.iter().enumerate() {
                by_seg[s.0] = Some(j);
            }
        }
        for (ri, &(a, k)) in runs.iter().enumerate() {
            if is_new(ri) {
                continue;
            }
            debug_assert_eq!(k, groups, "TOC run of {k} items for {groups} segments");
            let mut update = |i: usize, it: &mut Item, (_, seg, key, lines): &(usize, Segment, u64, u32)| {
                it.hash = mix(it.content_hash, *key);
                if it.est_lines != *lines || it.segment.as_ref() != Some(seg) {
                    it.est_lines = *lines;
                    it.segment = Some(seg.clone());
                    change.rehashed.push(i);
                } else {
                    change.rekeyed.push(i);
                }
            };
            if walk {
                for ((gi, i), it) in (first..).zip(a + first..).zip(self.items.0.range_mut(a + first..a + k)) {
                    if let Some(j) = by_seg[gi] {
                        update(i, it, &segs[j]);
                    }
                }
            } else {
                for s in &segs {
                    let i = a + s.0;
                    if let Some(it) = self.items.get_mut(i) {
                        update(i, it, s);
                    }
                }
            }
        }
    }

    /// Gives every `[toc]` block outside the edit `g_new` segments instead of `g_old` (trailing
    /// segments added or removed; the whole block rebuilt when it crosses the segmentation
    /// threshold). Records the splices in `change.resized` and moves the indices of the change,
    /// the outline and the runs to the final numbering. Segment keys are set by the caller.
    fn resize_tocs(
        &mut self,
        doc: &dyn FormatDoc,
        c: &TocChange<'_>,
        g_old: usize,
        g_new: usize,
        change: &mut ModelChange,
    ) {
        let n = self.outline.len();
        // (position after the primary splice, removed, added, position before the change)
        let mut splices: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut edits: Vec<(std::ops::Range<usize>, Vec<Item>)> = Vec::new();
        for ri in 0..self.toc_runs.len() {
            let Some(oa) = c.run_old[ri] else { continue };
            let (a, k) = self.toc_runs[ri];
            let base = self.toc_base(doc, a);
            let (at, removed, items) = if g_old > 0 && g_new > 0 {
                if g_new > g_old {
                    let items: Vec<Item> = (g_old..g_new).map(|gi| toc_seg_item(&base, gi, g_new, n)).collect();
                    (a + k, 0, items)
                } else {
                    (a + g_new, k - g_new, Vec::new())
                }
            } else {
                (a, k, toc_block_items(&base, g_new, n))
            };
            splices.push((at, removed, items.len(), oa + (at - a)));
            edits.push((at..at + removed, items));
        }
        // all at once: the chunk index is rebuilt once, not per block
        self.items.0.splice_many(edits);
        // positions after the primary splice → final positions
        let mut acc: Vec<isize> = Vec::with_capacity(splices.len());
        let mut sum = 0isize;
        for &(_, r, ad, _) in &splices {
            sum += ad as isize - r as isize;
            acc.push(sum);
        }
        let map = |i: usize| -> Option<usize> {
            let k = splices.partition_point(|&(at, r, _, _)| at + r <= i);
            if let Some(&(at, _, _, _)) = splices.get(k)
                && at <= i
            {
                return None; // a removed segment
            }
            Some(match k.checked_sub(1) {
                Some(j) => (i as isize + acc[j]) as usize,
                None => i,
            })
        };
        // the splices of the blocks before a block shift it (one splice per old block, in order)
        let k_new = g_new.max(1);
        let mut shift = 0isize;
        let mut si = 0;
        for (ri, r) in self.toc_runs.iter_mut().enumerate() {
            r.0 = (r.0 as isize + shift) as usize;
            if c.run_old[ri].is_some() {
                r.1 = k_new;
                shift = acc[si];
                si += 1;
            }
        }
        for o in &mut self.outline {
            o.item = map(o.item).expect("a heading is never a TOC segment");
        }
        change.rehashed = change.rehashed.iter().filter_map(|&i| map(i)).collect();
        change.rekeyed = change.rekeyed.iter().filter_map(|&i| map(i)).collect();
        let len = change.inserted.len();
        let start = map(c.lo).unwrap_or(c.lo);
        change.inserted = start..start + len;
        let mut before = 0isize;
        for (j, &(at, r, ad, oa)) in splices.iter().enumerate() {
            let s = (at as isize + before) as usize;
            change.resized.push((oa..oa + r, s..s + ad));
            before = acc[j];
        }
    }

    /// The unsegmented item of the `[toc]` block starting at item `a`, outside the model
    /// (absolute origin).
    fn toc_base(&self, doc: &dyn FormatDoc, a: usize) -> Item {
        let start = self.items.start(a).unwrap_or(0);
        let mut base = self.items[a].clone();
        base.origin = start;
        base.segment = None;
        let src = &doc.text()[start..start + base.len];
        base.est_lines = (src.bytes().filter(|&c| c == b'\n').count() + 1) as u32;
        base.hash = base.content_hash;
        base
    }

    /// Renumbers the anchors of the outline entries whose key is in `affected`, in document
    /// order; returns the indices of the entries whose anchor changed. One pass over the
    /// outline comparing keys (no allocation per entry).
    fn renumber_anchors(&mut self, doc: &dyn FormatDoc, affected: &HashSet<String>) -> Vec<usize> {
        let few: Vec<&str> = if affected.len() <= 8 {
            affected.iter().map(String::as_str).collect()
        } else {
            Vec::new()
        };
        let hit = |k: &str| {
            if affected.len() <= 8 {
                few.iter().any(|a| a.len() == k.len() && *a == k)
            } else {
                affected.contains(k)
            }
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut changed = Vec::new();
        for (k, o) in self.outline.iter_mut().enumerate() {
            if hit(&o.key) {
                let n = counts.entry(o.key.clone()).or_default();
                let a = doc.anchor_id(&o.key, *n);
                *n += 1;
                if a != o.anchor {
                    o.anchor = a;
                    changed.push(k);
                }
            }
        }
        changed
    }

    /// Hash of the whole outline as a TOC shows it.
    fn outline_hash(&self) -> u64 {
        let mut oh = hasher();
        for o in &self.outline {
            (o.level, &o.text, &o.anchor).hash(&mut oh);
        }
        oh.finish()
    }

    /// Hash of outline entries `r` as a TOC segment shows them (with the indentation base).
    fn toc_group_hash(&self, r: std::ops::Range<usize>) -> u64 {
        let mut h = hasher();
        self.min_level.hash(&mut h);
        for o in &self.outline[r] {
            (o.level, &o.text, &o.anchor).hash(&mut h);
        }
        h.finish()
    }

    /// Lowest heading level of the outline (the indentation base of TOC entries).
    pub fn outline_min_level(&self) -> u8 {
        self.min_level
    }

    /// Intra-line edit in a segmented block (items `a..b`, leaf start `start`): when the edited
    /// line is one whole piece that stays one piece and the segment boundaries do not move,
    /// only the edited segment is rebuilt from the text; the others shift (their spans are
    /// relative to the block start) and are re-keyed from their stored text hashes. `None`:
    /// rebuild the whole leaf.
    fn refresh_segment(
        &self,
        doc: &dyn FormatDoc,
        a: usize,
        b: usize,
        start: usize,
        e: &EditSummary,
    ) -> Option<Vec<Item>> {
        let delta = e.delta;
        let rel = e.changed.start.checked_sub(start)?;
        let rel_end = e.changed.end.checked_sub(start)?;
        let spans_of = |i: usize| match &self.items[i].segment {
            Some(Segment::Lines { spans, .. }) => Some(spans.clone()),
            _ => None,
        };
        // the segment and piece holding the edit
        let mut found = None;
        for k in a..b {
            let sp = spans_of(k)?;
            if let Some(u) = sp.iter().position(|s| s.start <= rel && rel_end <= s.end) {
                found = Some((k, u, sp));
                break;
            }
        }
        let (k, u, spans) = found?;
        // a whole line: not continued from / into a neighbouring piece
        let prev_end = if u > 0 {
            Some(spans[u - 1].end)
        } else if k > a {
            spans_of(k - 1)?.last().map(|s| s.end)
        } else {
            None
        };
        let next_start = if u + 1 < spans.len() {
            Some(spans[u + 1].start)
        } else if k + 1 < b {
            spans_of(k + 1)?.first().map(|s| s.start)
        } else {
            None
        };
        if prev_end == Some(spans[u].start) || next_start == Some(spans[u].end) {
            return None;
        }
        let new_len = (spans[u].len() as isize + delta) as usize;
        if new_len > MAX_UNIT_BYTES {
            return None;
        }
        // the segment still ends where it did (`group_units`)
        let mut bytes = 0;
        for (i, s) in spans.iter().enumerate() {
            bytes += if i == u { new_len } else { s.len() };
            let trigger = i + 1 >= SEG_UNITS || bytes >= SEG_BYTES;
            let last = i + 1 == spans.len();
            if trigger != last && !(last && k + 1 == b) {
                return None;
            }
        }
        // and the block stays segmented
        let units: usize = (a..b).map(|i| self.items[i].est_lines as usize).sum();
        let total = (a..b).map(|i| self.items[i].est_chars as usize).sum::<usize>() as isize + delta;
        if units <= SEG_MIN_UNITS && total as usize <= SEG_MIN_BYTES {
            return None;
        }
        let text = doc.text();
        let shift = |s: Span| Span::new((s.start as isize + delta) as usize, (s.end as isize + delta) as usize);
        let mut out = Vec::with_capacity(b - a);
        for j in a..b {
            let mut it = self.items[j].clone();
            it.origin = start;
            it.len = (it.len as isize + delta) as usize;
            if let Some(Segment::Lines { spans, text_hash, .. }) = &mut it.segment {
                if j == k {
                    let new: Box<[Span]> = spans
                        .iter()
                        .enumerate()
                        .map(|(i, s)| match i.cmp(&u) {
                            std::cmp::Ordering::Less => *s,
                            std::cmp::Ordering::Equal => Span::new(s.start, s.start + new_len),
                            std::cmp::Ordering::Greater => shift(*s),
                        })
                        .collect();
                    *text_hash = pieces_hash(text, new.iter().map(|s| Span::new(s.start + start, s.end + start)));
                    let mut img = false;
                    let mut math = it.kind == ItemKind::Math;
                    let mut wiki = false;
                    for s in new.iter() {
                        let t = &text[s.start + start..s.end + start];
                        img |= memchr_img(t);
                        math |= may_have_math(t);
                        wiki |= may_have_wikilink(t);
                    }
                    it.has_image = img;
                    it.has_math = math;
                    it.has_wikilink = wiki;
                    it.est_chars = (it.est_chars as isize + delta) as u32;
                    *spans = new;
                } else if j > k {
                    *spans = spans.iter().map(|s| shift(*s)).collect();
                }
            }
            if let Some(c) = &mut it.code {
                c.len = (c.len as isize + delta) as usize;
                if j > k {
                    c.offset = (c.offset as isize + delta) as usize;
                }
            }
            let seg = it.segment.as_ref().expect("segment");
            it.content_hash = lines_item_hash(&it.kind, &it.containers, it.gap, seg);
            it.hash = it.content_hash;
            out.push(it);
        }
        // the shared whole-block key (highlighter and diagram renderer input) follows the edit
        if let ItemKind::Code { lang } | ItemKind::Diagram { lang } = &out[0].kind
            && out[0].code.is_some()
        {
            let key = segmented_code_key(lang, &out);
            for it in &mut out {
                if let Some(c) = &mut it.code {
                    c.key = key;
                }
            }
        }
        Some(out)
    }

    /// Rebuilds the items of one leaf (its segments `first..`) in the container context of
    /// the old first item: same containers (markers), same gap.
    fn refresh_leaf(&self, doc: &dyn FormatDoc, first: usize, leaf: &markise_doc::LeafView) -> Vec<Item> {
        let old = &self.items[first];
        let mut f = Flattener {
            doc,
            text: doc.text(),
            items: Vec::new(),
            containers: old.containers.to_vec(),
            frames: Vec::new(),
            tight: old.gap == Gap::Tight,
            list_start: false,
            bullet_depth: 0,
            doc_start: old.gap == Gap::None,
            lines_buf: Vec::new(),
        };
        let fl = FlatLeaf {
            kind: leaf.kind.clone(),
            origin: 0,
            span: leaf.span,
            lines: &leaf.lines,
            top: 0,
        };
        f.leaf(&fl, item_kind(&fl.kind));
        f.items
    }

    /// Builds the outline of a fresh model: heading text and anchors of every heading item.
    fn rebuild_outline(&mut self, doc: &dyn FormatDoc) {
        let mut outline = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (idx, (start, item)) in self.items.iter_with_start().enumerate() {
            if let ItemKind::Heading { level } = item.kind {
                let info = doc.heading(start).unwrap_or_default();
                let key = info.key.to_lowercase();
                let n = counts.entry(key.clone()).or_default();
                let anchor = doc.anchor_id(&key, *n);
                *n += 1;
                outline.push(OutlineEntry {
                    level,
                    text: info.text.trim().to_string(),
                    anchor,
                    item: idx,
                    key,
                });
            }
        }
        self.min_level = outline.iter().map(|o| o.level).min().unwrap_or(1);
        self.outline = outline;
    }

    /// (Re)segments every `[toc]` block by the current outline and keys its items with the
    /// outline (the TOC's dependency, §7.4): an unsegmented TOC with the whole outline, a
    /// segment with its own entries ([`DocModel::toc_group_hash`]), so that an outline change
    /// re-keys only the segments showing changed entries ([`DocModel::sync_tocs`]). Scans the
    /// items for the TOC blocks and records them (a fresh model; edits keep them in step
    /// without a scan).
    fn update_tocs(&mut self, doc: &dyn FormatDoc) {
        // runs of TOC items belonging to one block (same origin)
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut cur: Option<(usize, usize, usize)> = None; // (first, end, origin)
        for (i, (o, it)) in self.items.iter_with_start().enumerate() {
            match cur {
                Some((a, _, co)) if it.kind == ItemKind::Toc && o == co => cur = Some((a, i + 1, co)),
                _ => {
                    if let Some((a, b, _)) = cur.take() {
                        runs.push((a, b));
                    }
                    if it.kind == ItemKind::Toc {
                        cur = Some((i, i + 1, o));
                    }
                }
            }
        }
        if let Some((a, b, _)) = cur {
            runs.push((a, b));
        }
        if runs.is_empty() {
            self.toc_runs.clear();
            return;
        }
        let outline_hash = self.outline_hash();
        let n_entries = self.outline.len();
        let groups: Vec<(std::ops::Range<usize>, u64)> = if n_entries > SEG_MIN_UNITS {
            (0..n_entries)
                .step_by(SEG_UNITS)
                .map(|s| {
                    let r = s..(s + SEG_UNITS).min(n_entries);
                    let h = self.toc_group_hash(r.clone());
                    (r, h)
                })
                .collect()
        } else {
            Vec::new()
        };
        // runs are rebuilt from the last to the first (earlier indices stay valid); outline
        // indices are remapped once at the end
        let mut sizes: Vec<(usize, usize, usize)> = Vec::with_capacity(runs.len()); // (a, b, k)
        for &(a, b) in runs.iter().rev() {
            // the run's first item keeps the block's markers and gap; `content_hash` of TOC
            // items is always the unsegmented one
            let base = self.toc_base(doc, a);
            let mut new_items = toc_block_items(&base, groups.len(), n_entries);
            if groups.is_empty() {
                new_items[0].hash = mix(base.content_hash, outline_hash);
            } else {
                for (it, (_, gh)) in new_items.iter_mut().zip(&groups) {
                    let seg = it.segment.as_ref().expect("a TOC segment");
                    it.hash = mix(base.content_hash, toc_segment_key(seg, *gh));
                }
            }
            let k = new_items.len();
            if k != b - a {
                self.items.0.splice(a..b, new_items, 0);
            } else {
                for (j, it) in new_items.into_iter().enumerate() {
                    if let Some(slot) = self.items.get_mut(a + j) {
                        let mut it = it;
                        it.origin = 0;
                        *slot = it;
                    }
                }
            }
            sizes.push((a, b, k));
        }
        sizes.reverse();
        // final indices of the runs and the outline remap
        let mut shift: isize = 0;
        let mut bounds: Vec<(usize, isize)> = Vec::with_capacity(sizes.len()); // (old b, shift after)
        self.toc_runs.clear();
        for &(a, b, k) in &sizes {
            let na = (a as isize + shift) as usize;
            self.toc_runs.push((na, k));
            shift += k as isize - (b - a) as isize;
            bounds.push((b, shift));
        }
        if bounds.iter().any(|&(_, s)| s != 0) {
            for o in &mut self.outline {
                let j = bounds.partition_point(|&(b, _)| b <= o.item);
                if j > 0 {
                    o.item = (o.item as isize + bounds[j - 1].1) as usize;
                }
            }
        }
    }

    /// Absolute start of item `i`.
    pub fn item_start(&self, i: usize) -> usize {
        self.items.start(i).unwrap_or(0)
    }

    /// Absolute byte span of the leaf block of item `i`.
    pub fn item_span(&self, i: usize) -> Span {
        let s = self.item_start(i);
        Span::new(s, s + self.items.get(i).map_or(0, |it| it.len))
    }

    /// Content lines (absolute) of item `i`: its segment's pieces, else the leaf's lines.
    pub fn item_lines(&self, doc: &dyn FormatDoc, i: usize) -> Vec<Span> {
        let start = self.item_start(i);
        match self.items.get(i).and_then(|it| it.segment.as_ref()) {
            Some(Segment::Lines { spans, .. }) => spans
                .iter()
                .map(|s| Span::new(start + s.start, start + s.end))
                .collect(),
            _ => doc.leaf(start).map(|l| l.lines).unwrap_or_default(),
        }
    }

    /// Reference labels item `i` may look up ([`markise_doc::label_key`]s; empty for items
    /// without inline references): the dependency of its layout on reference definitions
    /// (ADR 0006 §7.4). Computed on demand (the view memoises it per item hash).
    pub fn item_references(&self, doc: &dyn FormatDoc, i: usize) -> Vec<u64> {
        match self.items.get(i) {
            Some(it) => references_of(doc, it, self.item_start(i)),
            None => Vec::new(),
        }
    }

    /// Index of the item containing byte offset `pos` (or the next item after it).
    pub fn item_at_offset(&self, pos: usize) -> usize {
        let mut i = self.items.partition_point(|o, it| o + it.len < pos);
        // within a segmented block, the segment whose first line starts at or before `pos`
        while let Some(next) = self.items.get(i + 1)
            && let Some(cur) = self.items.get(i)
            && self.items.start(i) == self.items.start(i + 1)
            && cur.len == next.len
            && matches!(&next.segment, Some(Segment::Lines { spans, .. }) if self.item_start(i + 1) + spans[0].start <= pos)
        {
            i += 1;
        }
        i.min(self.items.len().saturating_sub(1))
    }

    /// Index of the outline entry for heading anchor `anchor` (`#` optional).
    pub fn heading_by_anchor(&self, anchor: &str) -> Option<&OutlineEntry> {
        let a = anchor.trim_start_matches('#');
        self.outline.iter().find(|o| o.anchor == a)
    }

    /// The outline entry whose section contains item `i` (the last heading at or before it).
    pub fn current_heading(&self, item: usize) -> Option<usize> {
        let k = self.outline.partition_point(|o| o.item <= item);
        k.checked_sub(1)
    }

    /// Compares two models item by item (absolute starts included); `Err` describes the first
    /// difference. For tests of incremental updates.
    pub fn diff(&self, other: &DocModel) -> Result<(), String> {
        if self.items.len() != other.items.len() {
            return Err(format!("item count {} != {}", self.items.len(), other.items.len()));
        }
        for (i, ((sa, a), (sb, b))) in self
            .items
            .iter_with_start()
            .zip(other.items.iter_with_start())
            .enumerate()
        {
            if sa != sb {
                return Err(format!("item {i}: start {sa} != {sb}"));
            }
            if a != b {
                return Err(format!("item {i}: {a:?}\n  != {b:?}"));
            }
        }
        if self.outline != other.outline {
            return Err(format!("outline {:?}\n  != {:?}", self.outline, other.outline));
        }
        if (&self.toc_runs, self.min_level) != (&other.toc_runs, other.min_level) {
            return Err(format!(
                "TOC runs / lowest level {:?} {} != {:?} {}",
                self.toc_runs, self.min_level, other.toc_runs, other.min_level
            ));
        }
        if (self.ctx_hash, &self.root_url) != (other.ctx_hash, &other.root_url) {
            return Err("context differs".into());
        }
        Ok(())
    }
}

/// Reference labels item `it` (absolute start `start`) may look up (see
/// [`DocModel::item_references`]).
fn references_of(doc: &dyn FormatDoc, it: &Item, start: usize) -> Vec<u64> {
    let mut out = Vec::new();
    let span = Span::new(start, start + it.len);
    // a whole leaf without anything that could be a reference: no lookup of its lines
    if matches!(
        it.kind,
        ItemKind::Paragraph | ItemKind::Heading { .. } | ItemKind::Table
    ) && it.segment.is_none()
        && !doc.may_reference(span)
    {
        return out;
    }
    match (&it.kind, &it.segment) {
        (ItemKind::Paragraph | ItemKind::Heading { .. }, seg) => {
            let lines = match seg {
                Some(Segment::Lines { spans, .. }) => spans
                    .iter()
                    .map(|s| Span::new(start + s.start, start + s.end))
                    .collect(),
                _ => doc.leaf(start).map(|l| l.lines).unwrap_or_default(),
            };
            doc.reference_candidates(&lines, &mut out);
        }
        (ItemKind::Table, seg) => {
            if let Some(t) = doc.table(start) {
                let rows: Vec<Span> = match seg {
                    Some(Segment::Rows { start, end, .. }) => std::iter::once(t.rows[0])
                        .chain(
                            t.rows[(*start as usize).min(t.rows.len())..(*end as usize).min(t.rows.len())]
                                .iter()
                                .copied(),
                        )
                        .collect(),
                    _ => t.rows,
                };
                doc.reference_candidates(&rows, &mut out);
            }
        }
        _ => {}
    }
    out
}

fn images_hash(root_url: &Option<String>, base_dir: &Option<PathBuf>) -> u64 {
    let mut h = hasher();
    root_url.hash(&mut h);
    base_dir.hash(&mut h);
    h.finish()
}

fn mix(a: u64, b: u64) -> u64 {
    let mut h = hasher();
    (a, b).hash(&mut h);
    h.finish()
}

/// Key of a `[toc]` segment: the segment and its entries' hash (the same in every TOC).
fn toc_segment_key(seg: &Segment, group_hash: u64) -> u64 {
    let mut h = hasher();
    (seg, group_hash).hash(&mut h);
    h.finish()
}

/// Number of segments of a `[toc]` over `n` outline entries (0: one unsegmented item).
fn toc_groups(n: usize) -> usize {
    if n > SEG_MIN_UNITS { n.div_ceil(SEG_UNITS) } else { 0 }
}

/// Segment `gi` of `groups` of a `[toc]` over `n` outline entries.
fn toc_segment(gi: usize, groups: usize, n: usize) -> Segment {
    Segment::Toc {
        start: (gi * SEG_UNITS) as u32,
        end: ((gi + 1) * SEG_UNITS).min(n) as u32,
        first: gi == 0,
        last: gi + 1 == groups,
    }
}

/// Item `gi` of a `[toc]` block with `groups` segments over `n` entries, from the block's
/// unsegmented item `base` (its hash is set when the segment is keyed).
fn toc_seg_item(base: &Item, gi: usize, groups: usize, n: usize) -> Item {
    let mut it = base.clone();
    let seg = toc_segment(gi, groups, n);
    if let Segment::Toc { start, end, .. } = seg {
        it.est_lines = end - start;
    }
    if gi > 0 {
        it.gap = Gap::None;
        spend_markers(&mut it.containers);
    }
    it.segment = Some(seg);
    it.hash = it.content_hash;
    it
}

/// The items of a `[toc]` block with `groups` segments (0: unsegmented) over `n` entries.
fn toc_block_items(base: &Item, groups: usize, n: usize) -> Vec<Item> {
    if groups == 0 {
        vec![base.clone()]
    } else {
        (0..groups).map(|gi| toc_seg_item(base, gi, groups, n)).collect()
    }
}

/// What [`DocModel::sync_tocs`] needs to know about the outline change of an edit.
struct TocChange<'a> {
    /// Outline entries before the edit.
    old_count: usize,
    /// Entries from this one on moved (the count changed).
    moved: Option<usize>,
    min_changed: bool,
    /// Any entry or the entry count changed.
    outline_changed: bool,
    /// Entries whose text, level or anchor changed.
    touched: &'a [usize],
    /// Per `[toc]` run: its first item before the edit (`None`: inserted by it).
    run_old: &'a [Option<usize>],
    /// First inserted item (before `[toc]` resizes elsewhere).
    lo: usize,
}

/// An in-place outline update of [`DocModel::apply`]: entries `at..at + added` replaced `old`.
struct OutlineEdit {
    at: usize,
    old: Vec<OutlineEntry>,
    added: usize,
}

/// Whether two outline entries look the same in a TOC (the item index aside).
fn same_entry(a: &OutlineEntry, b: &OutlineEntry) -> bool {
    a.level == b.level && a.text == b.text && a.anchor == b.anchor && a.key == b.key
}

/// Markers / quote starts belong to a block's first item only.
fn spend_markers(containers: &mut [Container]) {
    for c in containers.iter_mut() {
        match c {
            Container::ListItem { marker, .. } => *marker = None,
            Container::Footnote { label } => *label = None,
            Container::Quote { first, .. } => *first = false,
        }
    }
}

/// Flattens the region `from..to` of top-level blocks `tops` (see
/// [`FormatDoc::flatten_region`]); the caller filters the items by origin.
fn flatten_region(
    doc: &dyn FormatDoc,
    tops: std::ops::Range<usize>,
    from: usize,
    to: usize,
    doc_start: bool,
) -> Vec<Item> {
    let mut f = Flattener {
        doc,
        text: doc.text(),
        items: Vec::new(),
        containers: Vec::new(),
        frames: Vec::new(),
        tight: false,
        list_start: false,
        bullet_depth: 0,
        doc_start,
        lines_buf: Vec::new(),
    };
    doc.flatten_region(tops, from, to, &mut |ev| f.event(ev));
    f.items
}

/// Flattens top-level blocks `tops` into items with absolute origins. `doc_start`: the first
/// item produced is the first item of the document (no gap above it).
fn flatten(doc: &dyn FormatDoc, tops: std::ops::Range<usize>, doc_start: bool) -> Vec<Item> {
    let mut f = Flattener {
        doc,
        text: doc.text(),
        items: Vec::new(),
        containers: Vec::new(),
        frames: Vec::new(),
        tight: false,
        list_start: false,
        bullet_depth: 0,
        doc_start,
        lines_buf: Vec::new(),
    };
    doc.flatten(tops, &mut |ev| f.event(ev));
    f.items
}

enum Frame {
    Quote,
    List { saved_tight: bool, ordered: bool },
    ListItem,
    Footnote,
}

struct Flattener<'a> {
    doc: &'a dyn FormatDoc,
    text: &'a str,
    items: Vec<Item>,
    containers: Vec<Container>,
    frames: Vec<Frame>,
    tight: bool,
    /// The next leaf is the first one of a list that starts after non-list content.
    list_start: bool,
    bullet_depth: u8,
    /// No item precedes the ones produced here.
    doc_start: bool,
    lines_buf: Vec<Span>,
}

impl Flattener<'_> {
    fn event(&mut self, ev: FlatEvent<'_>) {
        match ev {
            FlatEvent::Open(c) => match c {
                FlatContainer::Quote { callout } => {
                    self.containers.push(Container::Quote { callout, first: true });
                    self.frames.push(Frame::Quote);
                }
                FlatContainer::List { ordered, tight } => {
                    let saved_tight = self.tight;
                    self.tight = tight;
                    self.list_start = !self
                        .containers
                        .iter()
                        .any(|c| matches!(c, Container::ListItem { tight: true, .. }));
                    if !ordered {
                        self.bullet_depth += 1;
                    }
                    self.frames.push(Frame::List { saved_tight, ordered });
                }
                FlatContainer::ListItem { marker } => {
                    let m = match marker {
                        ListMarker::Task { checked } => Marker::Task { checked },
                        ListMarker::Number { n, delim } => Marker::Number(format!("{n}{delim}")),
                        ListMarker::Bullet => Marker::Bullet {
                            depth: self.bullet_depth.saturating_sub(1),
                        },
                    };
                    self.containers.push(Container::ListItem {
                        marker: Some(m),
                        tight: self.tight,
                    });
                    self.frames.push(Frame::ListItem);
                }
                FlatContainer::Footnote { label } => {
                    self.containers.push(Container::Footnote {
                        label: Some(label.into()),
                    });
                    self.frames.push(Frame::Footnote);
                }
            },
            FlatEvent::Resume(c) => match c {
                // the container's first leaf lies before the region: decorations spent
                FlatContainer::Quote { callout } => {
                    self.containers.push(Container::Quote { callout, first: false });
                    self.frames.push(Frame::Quote);
                }
                FlatContainer::List { ordered, tight } => {
                    let saved_tight = self.tight;
                    self.tight = tight;
                    self.list_start = false;
                    if !ordered {
                        self.bullet_depth += 1;
                    }
                    self.frames.push(Frame::List { saved_tight, ordered });
                }
                FlatContainer::ListItem { .. } => {
                    self.containers.push(Container::ListItem {
                        marker: None,
                        tight: self.tight,
                    });
                    self.frames.push(Frame::ListItem);
                }
                FlatContainer::Footnote { .. } => {
                    self.containers.push(Container::Footnote { label: None });
                    self.frames.push(Frame::Footnote);
                }
            },
            FlatEvent::Close => match self.frames.pop() {
                Some(Frame::Quote | Frame::ListItem | Frame::Footnote) => {
                    self.containers.pop();
                }
                Some(Frame::List { saved_tight, ordered }) => {
                    if !ordered {
                        self.bullet_depth -= 1;
                    }
                    self.tight = saved_tight;
                }
                None => {}
            },
            FlatEvent::Empty { span, .. } => self.push(ItemKind::Empty, span, None),
            FlatEvent::Leaf(l) => {
                let kind = item_kind(&l.kind);
                self.leaf(&l, kind);
            }
        }
    }

    /// Pushes a leaf, split into segments when it is large.
    fn leaf(&mut self, l: &FlatLeaf<'_>, kind: ItemKind) {
        // reused buffer: no allocation per leaf
        let mut lines = std::mem::take(&mut self.lines_buf);
        lines.clear();
        lines.extend(l.abs_lines());
        self.leaf_lines(l, kind, &lines);
        self.lines_buf = lines;
    }

    fn leaf_lines(&mut self, l: &FlatLeaf<'_>, kind: ItemKind, lines: &[Span]) {
        let span = l.abs_span();
        match kind {
            ItemKind::Paragraph
            | ItemKind::Code { .. }
            | ItemKind::Math
            | ItemKind::Diagram { .. }
            | ItemKind::Html => {
                let units = split_units(self.text, lines);
                let bytes: usize = units.iter().map(|u| u.len()).sum();
                let lang = match &kind {
                    ItemKind::Code { lang } if !lang.is_empty() => Some(lang.clone()),
                    // diagrams: the content key of the whole block (worker input); large fences
                    // are still segmented like code, so their source fallback stays bounded per
                    // item, and a rendered scene is drawn by the first segment alone
                    ItemKind::Diagram { lang } => Some(lang.clone()),
                    _ => None,
                };
                if units.len() <= SEG_MIN_UNITS && bytes <= SEG_MIN_BYTES {
                    let code = lang.map(|lang| code_ref(self.text, &lang, lines));
                    self.push(kind, span, None);
                    self.items.last_mut().expect("pushed").code = code.map(Box::new);
                    return;
                }
                let offsets = lang.as_ref().map(|_| unit_offsets(lines, &units));
                let groups = group_units(&units, |u| u.len());
                let n = groups.len();
                let first_item = self.items.len();
                for (gi, g) in groups.into_iter().enumerate() {
                    let first_unit = g.start;
                    let text_hash = pieces_hash(self.text, units[g.clone()].iter().copied());
                    let rel: Box<[Span]> = units[g]
                        .iter()
                        .map(|u| Span::new(u.start - span.start, u.end - span.start))
                        .collect();
                    let seg = Segment::Lines {
                        spans: rel,
                        first: gi == 0,
                        last: gi + 1 == n,
                        text_hash,
                    };
                    self.push(kind.clone(), span, Some(seg));
                    if let Some(offs) = &offsets {
                        self.items.last_mut().expect("pushed").code = Some(Box::new(CodeRef {
                            key: 0,
                            offset: offs[first_unit],
                            len: content_len(lines),
                        }));
                    }
                }
                // a segmented block's highlight key: from its segments' text hashes
                if let Some(lang) = lang {
                    let key = segmented_code_key(&lang, &self.items[first_item..]);
                    for it in &mut self.items[first_item..] {
                        if let Some(c) = &mut it.code {
                            c.key = key;
                        }
                    }
                }
            }
            ItemKind::Table if lines.len() > 2 => {
                let body = 2..lines.len();
                let bytes: usize = lines.iter().map(|l| l.len()).sum();
                if body.len() <= SEG_MIN_UNITS && bytes <= SEG_MIN_BYTES {
                    return self.push(kind, span, None);
                }
                // column widths of a segmented table: estimated once, here, from the header and
                // the first SEG_UNITS body rows (cells capped at MAX_CELL_BYTES) — no shaping,
                // and identical for every segment
                let ncols = match &l.kind {
                    LeafKind::Table { columns } => (*columns as usize).max(1),
                    _ => 1,
                };
                let mut col_chars = vec![1u32; ncols];
                for (k, row) in std::iter::once(&lines[0])
                    .chain(lines.iter().skip(2).take(SEG_UNITS))
                    .enumerate()
                {
                    for (ci, cell) in self.doc.table_row_cells(*row).iter().take(ncols).enumerate() {
                        let c = cap_cell(cell.slice(self.text)).0.trim().chars().count() as u32;
                        // the header is bold: a little wider
                        let c = if k == 0 { c + c / 8 } else { c };
                        col_chars[ci] = col_chars[ci].max(c);
                    }
                }
                let mut h = hasher();
                lines[0].slice(self.text).hash(&mut h);
                // the delimiter row carries the column alignment (second review item 10)
                lines[1].slice(self.text).hash(&mut h);
                col_chars.hash(&mut h);
                let widths_hash = h.finish();
                let col_chars: Box<[u32]> = col_chars.into_boxed_slice();
                let rows: Vec<usize> = body.collect();
                let groups = group_units(&rows, |&r| lines[r].len());
                let n = groups.len();
                for (gi, g) in groups.into_iter().enumerate() {
                    let mut rh = hasher();
                    for &r in &rows[g.clone()] {
                        lines[r].slice(self.text).hash(&mut rh);
                        // the layout's cell source maps / fragment ids hold table-relative row
                        // offsets: they are part of the key
                        (lines[r].start - span.start).hash(&mut rh);
                    }
                    let seg = Segment::Rows {
                        start: rows[g.start] as u32,
                        end: rows[g.end - 1] as u32 + 1,
                        first: gi == 0,
                        last: gi + 1 == n,
                        data: Box::new(RowsData {
                            widths_hash,
                            rows_hash: rh.finish(),
                            col_chars: col_chars.clone(),
                        }),
                    };
                    self.push(kind.clone(), span, Some(seg));
                }
            }
            ItemKind::Heading { .. } | ItemKind::Table => self.push(kind, span, None),
            _ => self.push(kind, span, None),
        }
    }

    fn push(&mut self, kind: ItemKind, span: Span, segment: Option<Segment>) {
        // A leaf shows the markers of every enclosing list item / footnote that has not produced
        // a leaf yet (`- - a` draws both bullets on the same line); afterwards they are spent.
        let containers = self.containers.clone();
        spend_markers(&mut self.containers);

        let list_start = std::mem::take(&mut self.list_start);
        let gap = if segment.as_ref().is_some_and(|s| !s.first()) || (self.items.is_empty() && self.doc_start) {
            Gap::None
        } else if list_start {
            Gap::Normal
        } else if self.tight
            && !matches!(kind, ItemKind::Heading { .. })
            && containers.iter().any(|c| matches!(c, Container::ListItem { .. }))
        {
            Gap::Tight
        } else {
            Gap::Normal
        };
        let mut h = hasher();
        kind.hash(&mut h);
        containers.hash(&mut h);
        gap.hash(&mut h);
        let (est_lines, est_chars, has_image, mut has_math, has_wikilink) = match &segment {
            // the pieces' texts are covered by the segment's `text_hash`
            Some(Segment::Lines { spans, .. }) => {
                let mut chars = 0;
                let mut img = false;
                let mut math = false;
                let mut wiki = false;
                for sp in spans.iter() {
                    let t = &self.text[span.start + sp.start..span.start + sp.end];
                    chars += t.len();
                    img |= memchr_img(t);
                    math |= may_have_math(t);
                    wiki |= may_have_wikilink(t);
                }
                (spans.len() as u32, chars as u32, img, math, wiki)
            }
            // row texts are covered by `rows_hash`, the header / width sample by `widths_hash`
            Some(Segment::Rows { start, end, .. }) => (*end - *start + 1, 0, false, true, true),
            Some(Segment::Toc { .. }) | None => {
                let src = span.slice(self.text);
                src.hash(&mut h);
                (
                    (src.bytes().filter(|&c| c == b'\n').count() + 1) as u32,
                    src.len() as u32,
                    memchr_img(src),
                    may_have_math(src),
                    may_have_wikilink(src),
                )
            }
        };
        has_math |= kind == ItemKind::Math;
        // item-relative segment spans: offset-independent (C-18)
        segment.hash(&mut h);
        let content_hash = h.finish();
        let raw = if matches!(kind, ItemKind::Html | ItemKind::Paragraph) {
            self.doc.raw_presentation(span).map(Box::new)
        } else {
            None
        };
        self.items.push(Item {
            raw,
            kind,
            len: span.len(),
            containers: containers.into_boxed_slice(),
            gap,
            hash: content_hash,
            content_hash,
            est_chars,
            est_lines,
            has_image,
            has_math,
            has_wikilink,
            segment,
            code: None,
            origin: span.start,
        });
    }
}

/// The item kind of a neutral leaf.
fn item_kind(k: &LeafKind) -> ItemKind {
    match k {
        LeafKind::Paragraph => ItemKind::Paragraph,
        LeafKind::Heading { level } => ItemKind::Heading { level: *level },
        LeafKind::ThematicBreak => ItemKind::Rule,
        LeafKind::CodeBlock { info } => {
            let lang = info.split_whitespace().next().unwrap_or("").to_string();
            match lang.to_ascii_lowercase().as_str() {
                "math" => ItemKind::Math,
                "mermaid" | "flow" | "sequence" => ItemKind::Diagram { lang: lang.into() },
                _ => ItemKind::Code { lang: lang.into() },
            }
        }
        LeafKind::Math => ItemKind::Math,
        LeafKind::RawBlock => ItemKind::Html,
        LeafKind::Table { .. } => ItemKind::Table,
        LeafKind::Metadata => ItemKind::FrontMatter,
        LeafKind::Toc => ItemKind::Toc,
        LeafKind::Definition => ItemKind::Hidden,
    }
}

/// Content lines of a leaf, with over-long lines split into pieces of at most
/// [`MAX_UNIT_BYTES`] (at char boundaries, preferring a space).
fn split_units(text: &str, lines: &[Span]) -> Vec<Span> {
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        let mut start = l.start;
        while l.end - start > MAX_UNIT_BYTES {
            let mut cut = start + MAX_UNIT_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            // prefer breaking after a space in the second half; both ends of the searched
            // slice must be char boundaries (multibyte text, second review item 5)
            let mut lo = start + MAX_UNIT_BYTES / 2;
            while !text.is_char_boundary(lo) {
                lo += 1;
            }
            if lo < cut
                && let Some(sp) = text[lo..cut].rfind(' ')
            {
                cut = lo + sp + 1;
            }
            out.push(Span::new(start, cut));
            start = cut;
        }
        out.push(Span::new(start, l.end));
    }
    out
}

/// Hash of a segment's pieces (texts in order).
fn pieces_hash(text: &str, pieces: impl Iterator<Item = Span>) -> u64 {
    let mut h = hasher();
    for p in pieces {
        p.slice(text).hash(&mut h);
    }
    h.finish()
}

/// Content length (lines each followed by `\n`) of a code block.
fn content_len(lines: &[Span]) -> usize {
    lines.iter().map(|l| l.len() + 1).sum()
}

/// The highlight key of a segmented code block: language and its segments' text hashes.
fn segmented_code_key(lang: &str, segments: &[Item]) -> u64 {
    let mut h = hasher();
    lang.to_lowercase().hash(&mut h);
    for it in segments {
        if let Some(Segment::Lines { text_hash, .. }) = &it.segment {
            text_hash.hash(&mut h);
        }
    }
    h.finish()
}

/// The content hash of a lines-segment item (as [`Flattener::push`] computes it).
fn lines_item_hash(kind: &ItemKind, containers: &[Container], gap: Gap, segment: &Segment) -> u64 {
    let mut h = hasher();
    kind.hash(&mut h);
    containers.to_vec().hash(&mut h);
    gap.hash(&mut h);
    Some(segment).cloned().hash(&mut h);
    h.finish()
}

/// The [`CodeRef`] of a whole code block (offset 0).
fn code_ref(text: &str, lang: &str, lines: &[Span]) -> CodeRef {
    let mut h = hasher();
    lang.to_lowercase().hash(&mut h);
    let mut len = 0;
    for l in lines {
        let s = l.slice(text);
        s.hash(&mut h);
        len += s.len() + 1;
    }
    CodeRef {
        key: h.finish(),
        offset: 0,
        len,
    }
}

/// Content offset (lines joined, each followed by `\n`) of each unit of [`split_units`].
fn unit_offsets(lines: &[Span], units: &[Span]) -> Vec<usize> {
    let mut out = Vec::with_capacity(units.len());
    let mut li = 0;
    let mut line_off = 0;
    for u in units {
        // units are line pieces in line order
        while li + 1 < lines.len() && u.start >= lines[li].end && u.start != lines[li].start {
            line_off += lines[li].len() + 1;
            li += 1;
        }
        out.push(line_off + (u.start - lines[li].start));
    }
    out
}

/// Table cells longer than this are shown truncated (with "…") so one huge cell cannot make
/// a single layout unbounded.
pub const MAX_CELL_BYTES: usize = 2 * 1024;

/// `(cell text capped at MAX_CELL_BYTES on a char boundary, truncated?)`.
pub fn cap_cell(s: &str) -> (&str, bool) {
    if s.len() <= MAX_CELL_BYTES {
        return (s, false);
    }
    let mut cut = MAX_CELL_BYTES;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    (&s[..cut], true)
}

/// Groups consecutive units into segments of at most [`SEG_UNITS`] units / ~[`SEG_BYTES`].
fn group_units<T>(units: &[T], len: impl Fn(&T) -> usize) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (i, u) in units.iter().enumerate() {
        bytes += len(u);
        if i + 1 - start >= SEG_UNITS || bytes >= SEG_BYTES {
            out.push(start..i + 1);
            start = i + 1;
            bytes = 0;
        }
    }
    if start < units.len() {
        out.push(start..units.len());
    }
    out
}

fn may_have_math(s: &str) -> bool {
    s.contains('$') || s.contains("\\(")
}

fn may_have_wikilink(s: &str) -> bool {
    s.contains("[[")
}

fn memchr_img(s: &str) -> bool {
    s.contains("![") || s.contains("<img") || s.contains("<IMG")
}
