//! Paged models (issue #125, lazy view model): a model that holds real items only near the
//! viewport.
//!
//! Instead of one [`Item`] per leaf of the whole document, a paged model holds one placeholder
//! item ([`ItemKind::Page`]) per page of about [`PAGE_TOPS`] top-level blocks; only pages near
//! the viewport and the caret are *materialized* (replaced by their real items,
//! [`DocModel::materialize`]), pages far away are *evicted* back to a placeholder
//! ([`DocModel::evict`]); the view decides which (`view/paging.rs`). A placeholder keeps its
//! source bounds in the same chunked relative-offset list as real items (edits above it shift
//! it in O(chunks)), aggregate statistics for a height estimate ([`PageStats`]), and the
//! measured height of its items when it was evicted. The outline stays complete: the entries
//! of the headings inside a placeholder point at the placeholder's index.
//!
//! The global item-index contract of [`DocModel`] is kept: materialization and eviction are
//! ordinary splices reported as [`ModelChange`]s, so the view, caret geometry, the editor and
//! the viewer work with item indices as before (a placeholder is one item). Code that walks
//! every item sees placeholders and must treat them explicitly ([`Item::is_page`]).
//!
//! A materialized item is always equal to the item a full build ([`Paging::Full`]) has at the
//! same source position, and placeholders cover exactly the full build's items between their
//! neighbours ([`DocModel::check_against_full`], property-tested).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use markise_doc::chunked::RelChunks;
use markise_doc::{EditSummary, FormatDoc};

use super::{
    CrossBlockIndex, DepGens, DocModel, Gap, Item, ItemKind, Items, ModelChange, OutlineEntry, SEG_UNITS, Segment,
    flatten, hasher, images_hash, mix, toc_block_items, toc_groups, toc_segment_key,
};

/// Top-level blocks per page of a paged model.
pub const PAGE_TOPS: usize = 128;

/// Line lengths (characters) at which [`PageStats::para_wrapped`] is kept; a page estimate
/// interpolates between them.
pub const PARA_LINE_CHARS: [u32; 12] = [10, 20, 30, 40, 50, 60, 70, 80, 100, 120, 160, 240];

/// How a [`DocModel`] holds its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paging {
    /// One item per leaf block (or segment of one) of the whole document.
    Full,
    /// Placeholders for pages of `tops` top-level blocks; real items only where materialized.
    Paged { tops: usize },
}

impl Paging {
    /// The default for new models: paged with [`PAGE_TOPS`]-block pages. `MARKISE_FULL_VIEW=1`
    /// switches back to full models (comparison during the transition, issue #125);
    /// `MARKISE_PAGE_TOPS=n` sets the page size (the test suites can run with tiny pages).
    /// Read once per process.
    pub fn from_env() -> Paging {
        static MODE: OnceLock<Paging> = OnceLock::new();
        *MODE.get_or_init(|| {
            let on = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0");
            if on("MARKISE_FULL_VIEW") {
                return Paging::Full;
            }
            let tops = std::env::var("MARKISE_PAGE_TOPS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(PAGE_TOPS);
            Paging::Paged { tops }
        })
    }
}

/// Aggregate statistics of the items of a page placeholder: its height estimate without the
/// items.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PageStats {
    /// Leaf items (a segmented `[toc]` counts once).
    pub items: u32,
    /// Link reference definitions (not rendered).
    pub hidden: u32,
    /// Headings per level (1–6).
    pub heads: [u32; 6],
    /// Code, math, diagram and HTML blocks (and segments of them) …
    pub code_blocks: u32,
    /// … and their lines.
    pub code_lines: u32,
    pub tables: u32,
    pub table_rows: u32,
    pub front: u32,
    /// Rules, empty list items.
    pub simple: u32,
    /// Opening and closing lines of cards (ADR 0018).
    pub card_openers: u32,
    pub card_closers: u32,
    /// Headings per level, per name of the block they are directly in (ADR 0021: the estimate of
    /// a page depends on which names show natively).
    pub card_heads: Vec<(Box<str>, [u32; 6])>,
    /// `[toc]` blocks (each lists the whole outline).
    pub tocs: u32,
    /// Headings whose text may use a reference definition (their outline entries are
    /// refreshed from the page's blocks when a definition changes).
    pub heading_refs: u32,
    pub paras: u32,
    /// Estimated paragraph lines for the line lengths of [`PARA_LINE_CHARS`]: the sum of each
    /// paragraph's own estimate (its source lines, or its characters wrapped at that length,
    /// whichever is more, rounded up), so a page estimate equals the sum of its items'.
    pub para_wrapped: [u32; PARA_LINE_CHARS.len()],
    /// Items showing an image (their height depends on the resource root).
    pub images: u32,
    /// Items that may use a reference definition (their height may depend on one).
    pub ref_items: u32,
    /// The page held part of a disclosure when it was measured (its height depends on the
    /// disclosure's state).
    pub in_disclosure: bool,
    /// `(width_q, parameter key, height bits)`: the height the page's items had (laid out or
    /// estimated) when it was evicted; used while the view parameters are the same and the
    /// document-level inputs the page depends on are unchanged ([`PageStats::measured_gens`]).
    pub measured: Option<(u32, u64, u32)>,
    /// The dependency generations when the height was measured.
    pub measured_gens: DepGens,
}

impl PageStats {
    /// Estimated paragraph lines at `per_line` characters per line (interpolated).
    pub fn para_lines_at(&self, per_line: f32) -> f32 {
        let t = &PARA_LINE_CHARS;
        let w = &self.para_wrapped;
        let k = t.partition_point(|&n| (n as f32) < per_line);
        if k == 0 {
            return w[0] as f32;
        }
        if k == t.len() {
            return w[t.len() - 1] as f32;
        }
        let (a, b) = (t[k - 1] as f32, t[k] as f32);
        let f = (per_line - a) / (b - a);
        w[k - 1] as f32 + f * (w[k] as f32 - w[k - 1] as f32)
    }

    /// Headings in the page.
    pub fn headings(&self) -> usize {
        self.heads.iter().map(|&h| h as usize).sum()
    }

    fn of(items: &[Item], doc: &dyn FormatDoc) -> PageStats {
        let mut s = PageStats::default();
        for it in items {
            s.add(it);
            if let ItemKind::Heading { level } = it.kind
                && it.containers.iter().any(|c| matches!(c, super::Container::Card))
                && let Some(c) = doc.named_containers_at(it.origin).pop()
            {
                let li = (level as usize).clamp(1, 6) - 1;
                match s.card_heads.iter_mut().find(|(n, _)| *n == c.name) {
                    Some((_, heads)) => heads[li] += 1,
                    None => {
                        let mut heads = [0; 6];
                        heads[li] = 1;
                        s.card_heads.push((c.name, heads));
                    }
                }
            }
            s.images += it.has_image as u32;
            let prose = matches!(
                it.kind,
                ItemKind::Heading { .. } | ItemKind::Paragraph | ItemKind::Table
            );
            if prose && doc.may_reference(markise_doc::Span::new(it.origin, it.origin + it.len)) {
                s.ref_items += 1;
                if matches!(it.kind, ItemKind::Heading { .. }) {
                    s.heading_refs += 1;
                }
            }
        }
        s
    }

    /// Whether a measured height is still valid for dependency generations `now`: the
    /// generations of the inputs this page depends on (the outline for a `[toc]`, the resource
    /// root for images, reference definitions) did not change since it was measured.
    pub fn deps_unchanged(&self, now: &DepGens) -> bool {
        let m = &self.measured_gens;
        (self.tocs == 0 || m.outline == now.outline)
            && (self.images == 0 || m.resources == now.resources)
            && (self.ref_items == 0 || m.refs == now.refs)
            && (!self.in_disclosure || m.disclosures == now.disclosures)
    }

    fn add(&mut self, it: &Item) {
        self.items += 1;
        match &it.kind {
            ItemKind::Hidden => self.hidden += 1,
            ItemKind::Heading { level } => self.heads[(*level as usize).clamp(1, 6) - 1] += 1,
            ItemKind::Code { .. } | ItemKind::Math | ItemKind::Diagram { .. } | ItemKind::Html => {
                self.code_blocks += 1;
                self.code_lines += it.est_lines.saturating_sub(2).max(1);
            }
            ItemKind::Table => {
                self.tables += 1;
                self.table_rows += it.est_lines.saturating_sub(1).max(1);
            }
            ItemKind::FrontMatter => self.front += 1,
            ItemKind::Rule | ItemKind::Empty => self.simple += 1,
            ItemKind::ContainerMarker { close: false } => self.card_openers += 1,
            ItemKind::ContainerMarker { close: true } => self.card_closers += 1,
            ItemKind::Toc => {
                // a segmented TOC counts once (its segments share the block)
                if it.segment.as_ref().is_none_or(Segment::first) {
                    self.tocs += 1;
                } else {
                    self.items -= 1;
                }
            }
            ItemKind::Paragraph => {
                self.paras += 1;
                for (w, &n) in self.para_wrapped.iter_mut().zip(&PARA_LINE_CHARS) {
                    *w += it.est_lines.max(it.est_chars.div_ceil(n));
                }
            }
            ItemKind::Page(_) => unreachable!("a page placeholder inside a page"),
        }
    }
}

/// The placeholder item of `items` (absolute origins, non-empty). Its hashes are
/// offset-independent (the items' content hashes and relative positions): an unchanged page
/// keeps them when text above it changes.
fn page_item(items: &[Item], stats: PageStats) -> Item {
    let origin = items[0].origin;
    let end = items.iter().map(|it| it.origin + it.len).max().unwrap_or(origin);
    let mut h = hasher();
    for it in items {
        (it.content_hash, it.origin - origin).hash(&mut h);
    }
    let hash = h.finish();
    Item {
        kind: ItemKind::Page(Box::new(stats)),
        raw: None,
        len: end - origin,
        containers: Box::new([]),
        gap: Gap::Normal,
        hash,
        content_hash: hash,
        est_chars: 0,
        est_lines: 0,
        has_image: false,
        has_math: false,
        has_wikilink: false,
        segment: None,
        code: None,
        origin,
    }
}

/// Process-wide counters of paged models (diagnostics, benchmarks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PagingCounters {
    /// Pages materialized.
    pub materialized: u64,
    /// Runs of items evicted to a placeholder.
    pub evicted: u64,
    /// Edits that rebuilt a paged model (the format reported a change of everything).
    pub rebuilds: u64,
    /// Edits applied by re-paging the region they reached ([`DocModel::apply_region`]).
    pub regions: u64,
}

static MATERIALIZED: AtomicU64 = AtomicU64::new(0);
static EVICTED: AtomicU64 = AtomicU64::new(0);
static REBUILDS: AtomicU64 = AtomicU64::new(0);
static REGIONS: AtomicU64 = AtomicU64::new(0);

/// The process-wide counters of paged models.
pub fn paging_counters() -> PagingCounters {
    PagingCounters {
        materialized: MATERIALIZED.load(Ordering::Relaxed),
        evicted: EVICTED.load(Ordering::Relaxed),
        rebuilds: REBUILDS.load(Ordering::Relaxed),
        regions: REGIONS.load(Ordering::Relaxed),
    }
}

/// A splice that changes nothing at `i`.
fn no_change(i: usize) -> ModelChange {
    ModelChange {
        replaced: i..i,
        inserted: i..i,
        ..Default::default()
    }
}

impl DocModel {
    /// Whether this model is paged ([`Paging::Paged`]).
    pub fn is_paged(&self) -> bool {
        self.page_tops > 0
    }

    /// How this model holds its items.
    pub fn paging(&self) -> Paging {
        match self.page_tops {
            0 => Paging::Full,
            tops => Paging::Paged { tops },
        }
    }

    /// Whether item `i` is a page placeholder.
    pub fn is_page(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(Item::is_page)
    }

    /// Number of page placeholders (O(items)).
    pub fn page_count(&self) -> usize {
        self.items.iter().filter(|it| it.is_page()).count()
    }

    /// Builds a paged model with pages of `tops` top-level blocks: one streaming flatten pass
    /// that keeps only the placeholders and the outline; the first and the last page are
    /// materialized (the view keeps them so: a document of at most two pages is a full model).
    pub(super) fn build_paged(doc: &dyn FormatDoc, path: Option<&Path>, tops: usize) -> DocModel {
        let n = doc.top_count();
        let mut pages: Vec<Item> = Vec::with_capacity(n / tops + 1);
        let mut outline = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut cross = CrossBlockIndex::default();
        let mut k = 0;
        while k < n {
            let end = (k + tops).min(n);
            let items = flatten(doc, k..end, pages.is_empty());
            k = end;
            if items.is_empty() {
                continue;
            }
            cross.add_from(doc, &items);
            for it in &items {
                if let ItemKind::Heading { level } = it.kind {
                    let info = doc.heading(it.origin).unwrap_or_default();
                    let key = info.key.to_lowercase();
                    let c = counts.entry(key.clone()).or_default();
                    let anchor = doc.anchor_id(&key, *c);
                    *c += 1;
                    outline.push(OutlineEntry {
                        level,
                        text: info.text.trim().to_string(),
                        anchor,
                        item: pages.len(),
                        key,
                    });
                }
            }
            pages.push(page_item(&items, PageStats::of(&items, doc)));
        }
        let base_dir = path.and_then(|p| p.parent()).map(Path::to_path_buf);
        let root_url = doc.resource_root();
        let min_level = outline.iter().map(|o| o.level).min().unwrap_or(1);
        let mut chunks = RelChunks::default();
        chunks.extend_abs(pages);
        let mut m = DocModel {
            items: Items(chunks),
            outline,
            ctx_hash: images_hash(&root_url, &base_dir),
            base_dir,
            root_url,
            toc_runs: Vec::new(),
            min_level,
            page_tops: tops,
            cross,
            toc_keys: None,
        };
        m.cross.pair(doc);
        let last = m.items.len().saturating_sub(1);
        m.materialize(doc, last);
        m.materialize(doc, 0);
        m
    }

    /// Top-level blocks of page placeholder `i`.
    fn page_tops_of(&self, doc: &dyn FormatDoc, i: usize) -> std::ops::Range<usize> {
        let n = doc.top_count();
        let t0 = doc.top_at(self.item_start(i)).unwrap_or(0);
        let t1 = match self.items.start(i + 1) {
            Some(next) => doc.top_at(next).unwrap_or(n),
            None => n,
        };
        t0..t1.clamp(t0 + 1, n.max(t0 + 1))
    }

    /// The items page placeholder `i` stands for, with their absolute starts, without
    /// materializing it (`[toc]` blocks unsegmented; empty when `i` is not a placeholder). For
    /// consumers that read a page's blocks once (the word count).
    pub fn page_items(&self, doc: &dyn FormatDoc, i: usize) -> Vec<(usize, Item)> {
        if !self.is_page(i) {
            return Vec::new();
        }
        let tops = self.page_tops_of(doc, i);
        flatten(doc, tops, i == 0)
            .into_iter()
            .map(|it| (it.origin, it))
            .collect()
    }

    /// Replaces page placeholder `i` by its items (`[toc]` blocks segmented and keyed for the
    /// current outline). Indices after it shift: the returned change describes the splice (no
    /// change when `i` is not a placeholder).
    pub fn materialize(&mut self, doc: &dyn FormatDoc, i: usize) -> ModelChange {
        let Some(ItemKind::Page(stats)) = self.items.get(i).map(|it| &it.kind) else {
            return no_change(i);
        };
        let heads = stats.headings();
        let tops = self.page_tops_of(doc, i);
        let items = self.segment_tocs(flatten(doc, tops, i == 0));
        let k = items.len();
        // outline entries of the page's headings point at the heading items now
        let ka = self.outline.partition_point(|o| o.item < i);
        let kb = self.outline.partition_point(|o| o.item <= i);
        debug_assert_eq!(kb - ka, heads, "page {i}: outline entries vs. page headings");
        let mut heading_items = items
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it.kind, ItemKind::Heading { .. }))
            .map(|(j, _)| i + j);
        for o in &mut self.outline[ka..kb] {
            o.item = heading_items.next().unwrap_or(i);
        }
        for o in &mut self.outline[kb..] {
            o.item = o.item + k - 1;
        }
        // `[toc]` runs: the page's own come in, later ones shift
        let at = self.toc_runs.partition_point(|r| r.0 < i);
        let mut tail = self.toc_runs.split_off(at);
        for r in &mut tail {
            r.0 = r.0 + k - 1;
        }
        self.toc_runs.extend(toc_runs_of(&items, i));
        self.toc_runs.extend(tail);
        self.items.0.splice(i..i + 1, items, 0);
        MATERIALIZED.fetch_add(1, Ordering::Relaxed);
        ModelChange {
            replaced: i..i + 1,
            inserted: i..i + k,
            disclosures: true,
            ..Default::default()
        }
    }

    /// Replaces real items `r` (whole top-level blocks of `doc`, no placeholder among them) by
    /// one page placeholder that remembers `measured` (see [`PageStats::measured`]). No
    /// change when `r` is empty or holds a placeholder.
    pub fn evict(
        &mut self,
        doc: &dyn FormatDoc,
        r: std::ops::Range<usize>,
        measured: Option<(u32, u64, u32)>,
    ) -> ModelChange {
        if r.is_empty() || r.end > self.items.len() || r.clone().any(|i| self.is_page(i)) {
            return no_change(r.start);
        }
        let items: Vec<Item> = self
            .items
            .0
            .range(r.clone())
            .map(|rel| {
                let mut it = rel.item.clone();
                it.origin = rel.origin;
                it
            })
            .collect();
        let mut stats = PageStats::of(&items, doc);
        stats.measured = measured;
        stats.measured_gens = self.cross.gens;
        let lo = items[0].origin;
        let hi = items.iter().map(|it| it.origin + it.len).max().unwrap_or(lo);
        stats.in_disclosure = self.cross.disclosure_meets(lo, hi);
        let page = page_item(&items, stats);
        let removed = r.len() - 1;
        let ka = self.outline.partition_point(|o| o.item < r.start);
        for o in &mut self.outline[ka..] {
            o.item = if o.item < r.end { r.start } else { o.item - removed };
        }
        self.toc_runs.retain(|t| !r.contains(&t.0));
        for t in &mut self.toc_runs {
            if t.0 >= r.end {
                t.0 -= removed;
            }
        }
        self.items.0.splice(r.clone(), vec![page], 0);
        EVICTED.fetch_add(1, Ordering::Relaxed);
        ModelChange {
            replaced: r.clone(),
            inserted: r.start..r.start + 1,
            disclosures: true,
            ..Default::default()
        }
    }

    /// Segments and keys the `[toc]` blocks among freshly flattened items (absolute origins)
    /// for the current outline, as [`DocModel::update_tocs`] does for a fresh model.
    fn segment_tocs(&mut self, raw: Vec<Item>) -> Vec<Item> {
        if !raw.iter().any(|it| it.kind == ItemKind::Toc) {
            return raw;
        }
        let n = self.outline.len();
        let groups = toc_groups(n);
        let keys = self.toc_keys();
        let mut out = Vec::with_capacity(raw.len() + groups);
        for it in raw {
            if it.kind == ItemKind::Toc && it.segment.is_none() {
                let mut seg = toc_block_items(&it, groups, n);
                if groups == 0 {
                    seg[0].hash = mix(it.content_hash, keys[0]);
                } else {
                    for (s, &gh) in seg.iter_mut().zip(keys.iter()) {
                        let sg = s.segment.as_ref().expect("a TOC segment");
                        s.hash = mix(it.content_hash, toc_segment_key(sg, gh));
                    }
                }
                out.extend(seg);
            } else {
                out.push(it);
            }
        }
        out
    }

    /// The keys of the `[toc]` segments for the current outline (one per segment, or the
    /// whole outline's hash for an unsegmented TOC), cached until the outline changes (issue
    /// #125, phase 5: materializing pages holding repeated TOCs hashed the outline per
    /// segment every time).
    fn toc_keys(&mut self) -> std::sync::Arc<[u64]> {
        if let Some(k) = &self.toc_keys {
            return k.clone();
        }
        let n = self.outline.len();
        let groups = toc_groups(n);
        let keys: std::sync::Arc<[u64]> = if groups == 0 {
            vec![self.outline_hash()].into()
        } else {
            (0..groups)
                .map(|gi| self.toc_group_hash(gi * SEG_UNITS..((gi + 1) * SEG_UNITS).min(n)))
                .collect()
        };
        self.toc_keys = Some(keys.clone());
        keys
    }

    /// Whether edit `e` (of the document this model was built for) reaches a page placeholder:
    /// then the incremental update cannot proceed and the model is rebuilt.
    pub(super) fn edit_touches_page(&self, e: &EditSummary) -> bool {
        // the pre-edit bytes the update replaces: the edit and the re-parsed blocks
        let lo = e.changed.start.min(e.reparsed.start);
        let hi = e
            .changed
            .end
            .max((e.reparsed.end as isize - e.delta).max(0) as usize)
            .max(lo + 1);
        let k = self.items.partition_point(|o, _| o < hi);
        // the "first item of the document" path re-flattens the block after the edit
        if (e.new_top.start == 0 || e.replaced_top.start == 0) && self.is_page(k) {
            return true;
        }
        let mut j = k;
        while j > 0 {
            j -= 1;
            let (Some(o), Some(it)) = (self.items.start(j), self.items.get(j)) else {
                break;
            };
            if it.is_page() && o + it.len >= lo {
                return true;
            }
            if o + it.len < lo {
                break;
            }
        }
        false
    }

    /// Paged models: applies edit `e` by re-paging only the region it reaches (issue #125,
    /// phase 5) instead of rebuilding the model: the items and placeholders the re-parsed
    /// blocks overlap (extended to whole placeholders) are replaced by fresh placeholders for
    /// the region's top-level blocks, built in page-sized batches (no full-size intermediate);
    /// everything else shifts. The outline entries of the region are replaced (anchors of the
    /// affected keys renumbered), the disclosure markers likewise, `[toc]` blocks re-keyed.
    /// Used when an edit reaches a placeholder, or inserts more blocks than two pages hold (a
    /// large paste: only the pages near the view are then materialized, by the view). The
    /// view takes the result as a new model whose positions moved by the edit
    /// ([`ModelChange::moved`]).
    pub(super) fn apply_region(&mut self, doc: &dyn FormatDoc, e: &EditSummary) -> ModelChange {
        let n_before = self.items.len();
        let tops = self.page_tops.max(1);
        // pre-edit bytes the update replaces: the edit and the re-parsed blocks
        let old_lo = e.changed.start.min(e.reparsed.start);
        let old_hi = e
            .changed
            .end
            .max((e.reparsed.end as isize - e.delta).max(0) as usize)
            .max(old_lo);
        // the items and placeholders overlapping them (a placeholder as a whole)
        let lo = self.items.partition_point(|o, it| o + it.len <= old_lo && o < old_lo);
        let mut hi = self.items.partition_point(|o, _| o < old_hi).max(lo);
        if old_hi == old_lo {
            hi = lo;
        }
        let a = self.items.start(lo).map_or(old_lo, |o| o.min(old_lo));
        let b = (lo..hi)
            .map(|i| self.item_span(i).end)
            .max()
            .unwrap_or(old_hi)
            .max(old_hi);
        // the region's top-level blocks after the edit
        let n_tops = doc.top_count();
        // the first block after those of the item before the region (unchanged by the edit)
        let t0 = match lo.checked_sub(1) {
            Some(p) => doc
                .top_at(self.item_span(p).end.saturating_sub(1).max(self.item_start(p)))
                .map_or(0, |t| t + 1),
            None => 0,
        }
        .min(n_tops);
        let t1 = match self.items.start(hi) {
            Some(next) => doc.top_at((next as isize + e.delta).max(0) as usize).unwrap_or(n_tops),
            None => n_tops,
        }
        .clamp(t0, n_tops);
        if 2 * (t1 - t0) > n_tops {
            // most of the document (Replace All): a streaming rebuild costs less than
            // replacing it piecewise
            return self.rebuild_paged(doc, e);
        }
        // fresh placeholders, headings and markers, a page of blocks at a time
        let mut pages: Vec<Item> = Vec::new();
        let mut heads: Vec<(usize, u8, usize)> = Vec::new(); // (page, level, heading start)
        let mut markers = Vec::new();
        let mut folds = Vec::new();
        let mut k = t0;
        while k < t1 {
            let end = (k + tops).min(t1);
            let items = flatten(doc, k..end, lo == 0 && pages.is_empty());
            k = end;
            if items.is_empty() {
                continue;
            }
            for it in &items {
                if let ItemKind::Heading { level } = it.kind {
                    heads.push((pages.len(), level, it.origin));
                }
            }
            markers.extend(super::cross::markers_of(&items));
            folds.extend(super::cross::folds_of(doc, &items));
            pages.push(page_item(&items, PageStats::of(&items, doc)));
        }
        if lo == 0 && pages.is_empty() && hi < n_before && !self.is_page(hi) {
            // the document's first block changed status (its gap): rare, rebuild
            return self.rebuild_paged(doc, e);
        }
        let added = pages.len();
        self.items.0.splice(lo..hi, pages, e.delta);
        let shift = added as isize - (hi - lo) as isize;
        // outline: the region's entries replaced, later ones shift
        let mut affected: HashSet<String> = HashSet::new();
        let ka = self.outline.partition_point(|o| o.item < lo);
        let kb = self.outline.partition_point(|o| o.item < hi).max(ka);
        let fresh: Vec<OutlineEntry> = heads
            .iter()
            .map(|&(p, level, start)| {
                let info = doc.heading(start).unwrap_or_default();
                let key = info.key.to_lowercase();
                affected.insert(key.clone());
                OutlineEntry {
                    level,
                    text: info.text.trim().to_string(),
                    anchor: String::new(), // numbered below (its key is affected)
                    item: lo + p,
                    key,
                }
            })
            .collect();
        for o in &mut self.outline[kb..] {
            o.item = (o.item as isize + shift) as usize;
        }
        let old_entries: Vec<OutlineEntry> = self.outline.splice(ka..kb, fresh).collect();
        let old_count = self.outline.len() + old_entries.len() - heads.len();
        for o in &old_entries {
            affected.insert(o.key.clone());
        }
        // `[toc]` runs of the replaced items go, later ones shift (re-keyed below)
        self.toc_runs.retain(|r| r.0 < lo || r.0 >= hi);
        for r in &mut self.toc_runs {
            if r.0 >= hi {
                r.0 = (r.0 as isize + shift) as usize;
            }
        }
        // disclosure markers of the region
        self.cross
            .replace_range(doc, a..b, markers, folds, e.changed.clone(), e.delta);
        // document-level inputs
        let mut full = false;
        if e.deps.resources {
            self.root_url = doc.resource_root();
            let h = images_hash(&self.root_url, &self.base_dir);
            if h != self.ctx_hash {
                self.ctx_hash = h;
                self.cross.gens.resources += 1;
                full = true; // every image item has a new key
            }
        }
        let mut touched: Vec<usize> = Vec::new();
        if !e.deps.labels.is_empty() {
            self.cross.gens.refs += 1;
            self.retext_headings(doc, &e.deps.labels, &mut affected, &mut touched);
        }
        if !affected.is_empty() {
            touched.extend(self.renumber_anchors(doc, &affected));
        }
        let min_level = self.outline.iter().map(|o| o.level).min().unwrap_or(1);
        let changed_outline = old_count != self.outline.len()
            || min_level != self.min_level
            || !touched.is_empty()
            || old_entries.len() != heads.len()
            || old_entries
                .iter()
                .zip(&self.outline[ka..ka + heads.len()])
                .any(|(x, y)| !super::same_entry(x, y));
        self.min_level = min_level;
        if changed_outline {
            self.cross.gens.outline += 1;
            self.toc_keys = None;
        }
        // the real `[toc]` blocks follow the outline (segments and keys): re-segmented in
        // place, so the view takes a new model; otherwise the view splices the region
        if changed_outline && !self.toc_runs.is_empty() {
            self.update_tocs(doc);
            full = true;
        }
        if lo == 0 && added > 0 {
            full = true; // the first item's gap may have changed
        }
        REGIONS.fetch_add(1, Ordering::Relaxed);
        if full {
            return ModelChange {
                replaced: 0..n_before,
                inserted: 0..self.items.len(),
                labels: e.deps.labels.clone(),
                full: true,
                moved: Some(super::Moved::of(e)),
                ..Default::default()
            };
        }
        ModelChange {
            replaced: lo..hi,
            inserted: lo..lo + added,
            labels: e.deps.labels.clone(),
            moved: Some(super::Moved::of(e)),
            disclosures: true,
            ..Default::default()
        }
    }

    /// A fresh paged model for `doc` keeping this model's document folder (full rebuild).
    pub(super) fn rebuild_paged(&mut self, doc: &dyn FormatDoc, e: &EditSummary) -> ModelChange {
        let n = self.items.len();
        let labels = e.deps.labels.clone();
        *self = self.rebuilt(doc, e);
        REBUILDS.fetch_add(1, Ordering::Relaxed);
        ModelChange {
            replaced: 0..n,
            inserted: 0..self.items.len(),
            labels,
            full: true,
            moved: Some(super::Moved::of(e)),
            ..Default::default()
        }
    }

    /// Checks this model against a full build of `doc` (tests, benchmarks): every real item
    /// equals the full model's item at the same position, placeholders cover exactly the full
    /// model's items between their neighbours (with as many headings), the outlines agree
    /// (entries of real headings point at them) and the `[toc]` runs are those of the real
    /// items. `Err` describes the first difference.
    pub fn check_against_full(&self, doc: &dyn FormatDoc) -> Result<(), String> {
        let full = Self::build_full(doc, None);
        let fi: Vec<(usize, &Item)> = full.items.iter_with_start().collect();
        let pi: Vec<(usize, &Item)> = self.items.iter_with_start().collect();
        let mut j = 0;
        let mut heads = 0usize;
        for (i, &(o, it)) in pi.iter().enumerate() {
            if let ItemKind::Page(p) = &it.kind {
                let next = pi.get(i + 1).map_or(usize::MAX, |x| x.0);
                if fi.get(j).is_some_and(|f| f.0 != o) {
                    return Err(format!(
                        "page {i} at {o}: the full model's item {j} starts at {}",
                        fi[j].0
                    ));
                }
                let a = j;
                while j < fi.len() && fi[j].0 < next {
                    j += 1;
                }
                let h = fi[a..j]
                    .iter()
                    .filter(|(_, x)| matches!(x.kind, ItemKind::Heading { .. }))
                    .count();
                if h != p.headings() {
                    return Err(format!(
                        "page {i} at {o}: {} headings, full model has {h}",
                        p.headings()
                    ));
                }
                let end = fi[a..j].iter().map(|(fo, x)| fo + x.len).max().unwrap_or(o);
                if a == j || end - o != it.len {
                    return Err(format!("page {i} at {o}: len {}, full items span {}", it.len, end - o));
                }
                heads += h;
                continue;
            }
            let Some(&(fo, f)) = fi.get(j) else {
                return Err(format!("item {i} at {o}: past the end of the full model"));
            };
            if fo != o || f != it {
                return Err(format!("item {i}: ({o}, {it:?})\n  != full item {j}: ({fo}, {f:?})"));
            }
            if matches!(it.kind, ItemKind::Heading { .. }) {
                heads += 1;
            }
            j += 1;
        }
        if j != fi.len() {
            return Err(format!("{} full items not covered", fi.len() - j));
        }
        if heads != self.outline.len() || self.outline.len() != full.outline.len() {
            return Err(format!(
                "outline {} entries, {heads} headings in items, full {}",
                self.outline.len(),
                full.outline.len()
            ));
        }
        for (k, (a, b)) in self.outline.iter().zip(&full.outline).enumerate() {
            if (a.level, &a.text, &a.anchor, &a.key) != (b.level, &b.text, &b.anchor, &b.key) {
                return Err(format!("outline entry {k}: {a:?} != {b:?}"));
            }
            let real = matches!(
                self.items.get(a.item).map(|it| &it.kind),
                Some(ItemKind::Heading { .. })
            );
            if !real && !self.is_page(a.item) {
                return Err(format!("outline entry {k} points at item {}, not a heading", a.item));
            }
            if real && self.item_start(a.item) != full.item_start(b.item) {
                return Err(format!("outline entry {k} points at item {}", a.item));
            }
        }
        if self.min_level != full.min_level {
            return Err(format!("lowest level {} != {}", self.min_level, full.min_level));
        }
        let runs = toc_runs_of_items(&self.items);
        if runs != self.toc_runs {
            return Err(format!("TOC runs {:?} != {:?} (from the items)", self.toc_runs, runs));
        }
        if !self.cross.same_disclosures(&full.cross) {
            return Err(format!(
                "disclosures {:?}\n  != full {:?}",
                self.cross.disclosures(),
                full.cross.disclosures()
            ));
        }
        if self.root_url != full.root_url {
            return Err(format!("resource root {:?} != {:?}", self.root_url, full.root_url));
        }
        Ok(())
    }
}

impl Item {
    /// Whether this is a page placeholder of a paged model.
    pub fn is_page(&self) -> bool {
        matches!(self.kind, ItemKind::Page(_))
    }
}

/// `(first item, item count)` of the `[toc]` blocks among `items` (absolute origins), the first
/// of them at index `base`.
fn toc_runs_of(items: &[Item], base: usize) -> Vec<(usize, usize)> {
    runs_of(items.iter().map(|it| (it.origin, it)), base)
}

/// [`toc_runs_of`] over a model's items.
fn toc_runs_of_items(items: &Items) -> Vec<(usize, usize)> {
    runs_of(items.iter_with_start(), 0)
}

fn runs_of<'a>(items: impl Iterator<Item = (usize, &'a Item)>, base: usize) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut prev: Option<usize> = None;
    for (j, (o, it)) in items.enumerate() {
        if it.kind != ItemKind::Toc {
            prev = None;
            continue;
        }
        match (prev, runs.last_mut()) {
            (Some(p), Some(r)) if p == o => r.1 += 1,
            _ => runs.push((base + j, 1)),
        }
        prev = Some(o);
    }
    runs
}
