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
        lang: String,
    },
    /// Display math, shown as styled TeX source in M1 (placeholder, like the HTML export C36).
    Math,
    /// Diagram fence (`mermaid`, `flow`, `sequence`): rendered through [`crate::Diagrams`] when
    /// diagrams are on (the first segment draws the scene, later segments are empty), else
    /// styled source, segmented like code.
    Diagram {
        lang: String,
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
        widths_hash: u64,
        /// Hash of this segment's row texts and their offsets in the table (cached layouts
        /// carry table-relative source positions).
        rows_hash: u64,
        /// Estimated natural width of each column in characters (shared by all segments).
        col_chars: Box<[u32]>,
    },
    /// Outline entries `start..end` of a `[toc]`.
    Toc {
        start: u32,
        end: u32,
        first: bool,
        last: bool,
    },
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
    /// For a heading, its index in [`DocModel::outline`].
    pub heading: Option<u32>,
    /// Set when this item is one part of a large block.
    pub segment: Option<Segment>,
    /// Set for fenced code with a language (highlighting input) and for diagram fences (diagram
    /// input; `key` identifies the whole block's content).
    pub code: Option<CodeRef>,
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
}

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
    /// Reference labels whose definitions changed: items that may use them get new cache keys
    /// (the view folds reference fingerprints into its keys, ADR 0006 §7.4).
    pub labels: Vec<u64>,
    /// The whole model was rebuilt.
    pub full: bool,
}

impl DocModel {
    /// Builds the view model of `doc`, read from `path` (if any).
    pub fn build(doc: &dyn FormatDoc, path: Option<&Path>) -> Arc<DocModel> {
        Arc::new(Self::build_owned(doc, path))
    }

    /// Like [`DocModel::build`], unshared (for owners that update it with [`DocModel::apply`]).
    pub fn build_owned(doc: &dyn FormatDoc, path: Option<&Path>) -> DocModel {
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
        };
        m.rebuild_outline(doc);
        let _ = m.update_tocs(doc);
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
        if e.deps.all {
            let n = self.items.len();
            let base_dir = self.base_dir.clone();
            let mut fresh = Self::build_owned(doc, None);
            fresh.base_dir = base_dir;
            fresh.ctx_hash = images_hash(&fresh.root_url, &fresh.base_dir);
            *self = fresh;
            return ModelChange {
                replaced: 0..n,
                inserted: 0..self.items.len(),
                rehashed: Vec::new(),
                labels: Vec::new(),
                full: true,
            };
        }
        let mut change = ModelChange::default();
        // outline entries under construction (only when a heading may have changed)
        let mut heads: Option<Vec<Head>> = None;
        let mut affected: HashSet<String> = HashSet::new();
        let mut toc_touched = false;
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
            let n_new = new_items.len();
            let old_heads = (lo..hi).any(|i| self.items[i].heading.is_some());
            let new_heads = new_items.iter().any(|it| matches!(it.kind, ItemKind::Heading { .. }));
            toc_touched = (lo..hi).any(|i| self.items[i].kind == ItemKind::Toc)
                || new_items.iter().any(|it| it.kind == ItemKind::Toc);
            self.items.0.splice(lo..hi, new_items, e.delta);
            change.replaced = lo..hi;
            change.inserted = lo..lo + n_new;
            let shift = n_new as isize - (hi - lo) as isize;
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
                    let mut fresh = Self::build_owned(doc, None);
                    fresh.base_dir = base_dir;
                    fresh.ctx_hash = images_hash(&fresh.root_url, &fresh.base_dir);
                    *self = fresh;
                    return ModelChange {
                        replaced: 0..n_before,
                        inserted: 0..self.items.len(),
                        rehashed: Vec::new(),
                        labels: e.deps.labels.clone(),
                        full: true,
                    };
                }
                let keep: Vec<Option<u32>> = (n_new..n_new + cnt)
                    .map(|i| self.items.get(i).and_then(|it| it.heading))
                    .collect();
                self.items.0.splice(n_new..n_new + cnt, block_items, 0);
                for (j, h) in keep.into_iter().enumerate() {
                    if let Some(it) = self.items.get_mut(n_new + j) {
                        it.heading = h;
                    }
                }
                change.rehashed.extend(n_new..n_new + cnt);
            }
            if old_heads || new_heads {
                let mut hs: Vec<Head> = Vec::with_capacity(self.outline.len() + 4);
                for o in self.outline.iter().filter(|o| o.item < lo) {
                    hs.push(Head::from(o));
                }
                for i in lo..lo + n_new {
                    if let ItemKind::Heading { level } = self.items[i].kind {
                        let info = doc.heading(self.item_start(i)).unwrap_or_default();
                        let key = info.key.to_lowercase();
                        affected.insert(key.clone());
                        hs.push(Head {
                            item: i,
                            level,
                            text: info.text.trim().to_string(),
                            key,
                            anchor: None,
                        });
                    }
                }
                for o in self.outline.iter().filter(|o| o.item >= lo && o.item < hi) {
                    affected.insert(o.key.clone());
                }
                for o in self.outline.iter().filter(|o| o.item >= hi) {
                    let mut h = Head::from(o);
                    h.item = (h.item as isize + shift) as usize;
                    hs.push(h);
                }
                heads = Some(hs);
            } else if shift != 0 {
                for o in &mut self.outline {
                    if o.item >= hi {
                        o.item = (o.item as isize + shift) as usize;
                    }
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
        // reference definitions changed (§7.4): cache keys of the items that may use them change
        // in the view (it folds their fingerprints into the keys); here only heading texts
        // (outline, anchors) need refreshing — a scan over the headings
        if !e.deps.labels.is_empty() {
            change.labels = e.deps.labels.clone();
            let changed: HashSet<u64> = e.deps.labels.iter().copied().collect();
            let retext: Vec<usize> = self
                .outline
                .iter()
                .map(|o| o.item)
                .chain(heads.iter().flatten().map(|h| h.item))
                .filter(|&i| self.item_references(doc, i).iter().any(|r| changed.contains(r)))
                .collect();
            if !retext.is_empty() {
                let hs = heads.get_or_insert_with(|| self.outline.iter().map(Head::from).collect());
                for h in hs.iter_mut().filter(|h| retext.contains(&h.item)) {
                    let info = doc.heading(self.items.start(h.item).unwrap_or(0)).unwrap_or_default();
                    let key = info.key.to_lowercase();
                    if key != h.key {
                        affected.insert(h.key.clone());
                        affected.insert(key.clone());
                        h.anchor = None;
                    }
                    h.text = info.text.trim().to_string();
                    h.key = key;
                }
            }
        }
        if let Some(hs) = heads {
            let before = std::mem::take(&mut self.outline);
            self.set_outline(doc, hs, Some(&affected));
            toc_touched |= before != self.outline;
        }
        if toc_touched {
            let (tocs, resized) = self.update_tocs(doc);
            if resized {
                // a `[toc]` crossed the segmentation threshold and changed its item count: the
                // splices of this change no longer describe the item list (rare: the view
                // resets its heights)
                change.full = true;
            } else {
                change.rehashed.extend(tocs);
            }
        }
        change
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
        let mut heads = Vec::new();
        for (idx, (start, item)) in self.items.iter_with_start().enumerate() {
            if let ItemKind::Heading { level } = item.kind {
                let info = doc.heading(start).unwrap_or_default();
                heads.push(Head {
                    item: idx,
                    level,
                    text: info.text.trim().to_string(),
                    key: info.key.to_lowercase(),
                    anchor: None,
                });
            }
        }
        self.set_outline(doc, heads, None);
    }

    /// Installs outline entries: anchors of the entries whose key is in `affected` (or that
    /// have none yet; all when `affected` is `None`) are renumbered in document order (R7.2:
    /// only anchors sharing a key influence each other), the others are kept. Sets the items'
    /// heading indices.
    fn set_outline(&mut self, doc: &dyn FormatDoc, mut heads: Vec<Head>, affected: Option<&HashSet<String>>) {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for h in heads.iter_mut() {
            if affected.is_none_or(|a| a.contains(&h.key)) || h.anchor.is_none() {
                let n = counts.entry(h.key.clone()).or_default();
                h.anchor = Some(doc.anchor_id(&h.key, *n));
                *n += 1;
            }
        }
        let mut outline = Vec::with_capacity(heads.len());
        for (k, h) in heads.into_iter().enumerate() {
            if let Some(it) = self.items.get_mut(h.item) {
                it.heading = Some(k as u32);
            }
            outline.push(OutlineEntry {
                level: h.level,
                text: h.text,
                anchor: h.anchor.unwrap_or_default(),
                item: h.item,
                key: h.key,
            });
        }
        self.outline = outline;
    }

    /// (Re)segments every `[toc]` block by the current outline and keys its items with the
    /// outline (the TOC's dependency, §7.4); returns the indices of the TOC items afterwards
    /// and whether a TOC changed its number of items.
    #[must_use]
    fn update_tocs(&mut self, doc: &dyn FormatDoc) -> (Vec<usize>, bool) {
        if !self.items.iter().any(|i| i.kind == ItemKind::Toc) {
            return (Vec::new(), false);
        }
        let mut oh = hasher();
        for o in &self.outline {
            (o.level, &o.text, &o.anchor).hash(&mut oh);
        }
        let outline_hash = oh.finish();
        let n_entries = self.outline.len();
        let groups: Vec<std::ops::Range<usize>> = if n_entries > SEG_MIN_UNITS {
            (0..n_entries)
                .step_by(SEG_UNITS)
                .map(|s| s..(s + SEG_UNITS).min(n_entries))
                .collect()
        } else {
            Vec::new()
        };
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
        // runs are rebuilt from the last to the first (earlier indices stay valid); outline
        // indices are remapped once at the end
        let mut sizes: Vec<(usize, usize, usize)> = Vec::with_capacity(runs.len()); // (a, b, k)
        for &(a, b) in runs.iter().rev() {
            let start = self.items.start(a).unwrap_or(0);
            // the run's first item keeps the block's markers and gap; `content_hash` of TOC
            // items is always the unsegmented one
            let mut base = self.items[a].clone();
            base.origin = start;
            base.segment = None;
            let src = &doc.text()[start..start + base.len];
            base.est_lines = (src.bytes().filter(|&c| c == b'\n').count() + 1) as u32;
            let mut new_items = Vec::new();
            if groups.is_empty() {
                let mut it = base.clone();
                it.hash = mix(it.content_hash, outline_hash);
                new_items.push(it);
            } else {
                let n = groups.len();
                for (gi, g) in groups.iter().enumerate() {
                    let mut seg_item = base.clone();
                    let seg = Segment::Toc {
                        start: g.start as u32,
                        end: g.end as u32,
                        first: gi == 0,
                        last: gi + 1 == n,
                    };
                    let mut sh = hasher();
                    (base.content_hash, &seg, outline_hash).hash(&mut sh);
                    seg_item.hash = sh.finish();
                    seg_item.est_lines = (g.end - g.start) as u32;
                    if gi > 0 {
                        seg_item.gap = Gap::None;
                        spend_markers(&mut seg_item.containers);
                    }
                    seg_item.segment = Some(seg);
                    new_items.push(seg_item);
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
        let mut out = Vec::new();
        let mut shift: isize = 0;
        let mut bounds: Vec<(usize, isize)> = Vec::with_capacity(sizes.len()); // (old b, shift after)
        for &(a, b, k) in &sizes {
            let na = (a as isize + shift) as usize;
            out.extend(na..na + k);
            shift += k as isize - (b - a) as isize;
            bounds.push((b, shift));
        }
        if shift != 0 || bounds.iter().any(|&(_, s)| s != 0) {
            for o in &mut self.outline {
                let j = bounds.partition_point(|&(b, _)| b <= o.item);
                if j > 0 {
                    o.item = (o.item as isize + bounds[j - 1].1) as usize;
                }
            }
            for (k, o) in self.outline.iter().enumerate() {
                if let Some(it) = self.items.get_mut(o.item) {
                    it.heading = Some(k as u32);
                }
            }
        }
        let resized = sizes.iter().any(|&(a, b, k)| k != b - a);
        (out, resized)
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
        let Some(it) = self.items.get(i) else { return Vec::new() };
        let mut out = Vec::new();
        match (&it.kind, &it.segment) {
            (ItemKind::Paragraph | ItemKind::Heading { .. }, _) => {
                doc.reference_candidates(&self.item_lines(doc, i), &mut out);
            }
            (ItemKind::Table, seg) => {
                let start = self.item_start(i);
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
        if (self.ctx_hash, &self.root_url) != (other.ctx_hash, &other.root_url) {
            return Err("context differs".into());
        }
        Ok(())
    }
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

/// An outline entry under construction.
struct Head {
    item: usize,
    level: u8,
    text: String,
    /// Lower-cased anchor key.
    key: String,
    anchor: Option<String>,
}

impl From<&OutlineEntry> for Head {
    fn from(o: &OutlineEntry) -> Self {
        Head {
            item: o.item,
            level: o.level,
            text: o.text.clone(),
            key: o.key.clone(),
            anchor: Some(o.anchor.clone()),
        }
    }
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
                    self.items.last_mut().expect("pushed").code = code;
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
                        self.items.last_mut().expect("pushed").code = Some(CodeRef {
                            key: 0,
                            offset: offs[first_unit],
                            len: content_len(lines),
                        });
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
                        widths_hash,
                        rows_hash: rh.finish(),
                        col_chars: col_chars.clone(),
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
        self.items.push(Item {
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
            heading: None,
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
                "mermaid" | "flow" | "sequence" => ItemKind::Diagram { lang },
                _ => ItemKind::Code { lang },
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
