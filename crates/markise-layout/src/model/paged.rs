//! PROTOTYPE (issue #29, `MARKISE_LAZY_VIEW=1`): a paged view model.
//!
//! Instead of one [`Item`] per leaf of the whole document, the model holds one placeholder item
//! ([`ItemKind::Page`]) per page of about [`PAGE_TOPS`] top-level blocks; only pages near the
//! viewport and the caret are *materialized* (replaced by their real items), pages far away are
//! *evicted* back to a placeholder. The placeholder keeps its source bounds in the same chunked
//! relative-offset list as real items (so edits above it shift it in O(chunks)), aggregate
//! statistics for a height estimate, and the measured height of its items when it was evicted.
//! The outline stays complete (headings of a placeholder point at the placeholder's index).
//!
//! This deliberately keeps the global item-index contract of [`DocModel`] (indices change on
//! materialization / eviction and are reported as [`ModelChange`] splices) so the rest of the
//! app (view, caret, editor, viewer) works unchanged; see docs/research/lazy-view-prototype.md.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use markise_doc::chunked::RelChunks;
use markise_doc::{EditSummary, FormatDoc};

use super::{
    DocModel, Gap, Item, ItemKind, Items, ModelChange, OutlineEntry, Segment, flatten, hasher, images_hash, mix,
    toc_block_items, toc_groups, toc_segment_key,
};

/// Top-level blocks per page.
pub const PAGE_TOPS: usize = 128;

/// Whether the lazy (paged) view model is switched on (`MARKISE_LAZY_VIEW=1`, read once).
pub fn lazy_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("MARKISE_LAZY_VIEW").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Aggregate statistics of a page of items (height estimate without the items).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PageStats {
    pub items: u32,
    pub hidden: u32,
    /// Headings per level (1–6).
    pub heads: [u32; 6],
    pub code_blocks: u32,
    pub code_lines: u32,
    pub tables: u32,
    pub table_rows: u32,
    pub front: u32,
    /// Rules, empty list items.
    pub simple: u32,
    pub tocs: u32,
    pub paras: u32,
    pub para_lines: u32,
    pub para_chars: u64,
    /// `(width_q, theme key, height bits)`: the height the page's items had (laid out or
    /// estimated) when it was evicted.
    pub measured: Option<(u32, u64, u32)>,
}

impl PageStats {
    pub fn headings(&self) -> usize {
        self.heads.iter().map(|&h| h as usize).sum()
    }

    fn of(items: &[Item]) -> PageStats {
        let mut s = PageStats::default();
        for it in items {
            s.add(it);
        }
        s
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
                self.para_lines += it.est_lines;
                self.para_chars += it.est_chars as u64;
            }
            ItemKind::Page(p) => {
                self.items -= 1;
                let m = self.measured.take();
                let p = (**p).clone();
                self.items += p.items;
                self.hidden += p.hidden;
                for l in 0..6 {
                    self.heads[l] += p.heads[l];
                }
                self.code_blocks += p.code_blocks;
                self.code_lines += p.code_lines;
                self.tables += p.tables;
                self.table_rows += p.table_rows;
                self.front += p.front;
                self.simple += p.simple;
                self.tocs += p.tocs;
                self.paras += p.paras;
                self.para_lines += p.para_lines;
                self.para_chars += p.para_chars;
                self.measured = m;
            }
        }
    }
}

/// A placeholder item for `items` (absolute origins, non-empty).
fn page_item(items: &[Item], stats: PageStats) -> Item {
    let origin = items[0].origin;
    let end = items.iter().map(|it| it.origin + it.len).max().unwrap_or(origin);
    let mut h = hasher();
    (origin, &stats).hash(&mut h);
    let hash = h.finish() | 1 << 63;
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

/// Counters of the paged model (diagnostics, benchmarks).
#[derive(Clone, Copy, Debug, Default)]
pub struct PagedStats {
    pub materialized: u64,
    pub evicted: u64,
    pub rebuilds: u64,
}

static STATS: std::sync::Mutex<PagedStats> = std::sync::Mutex::new(PagedStats {
    materialized: 0,
    evicted: 0,
    rebuilds: 0,
});

/// Process-wide counters of the paged model.
pub fn paged_stats() -> PagedStats {
    *STATS.lock().unwrap()
}

fn bump(f: impl FnOnce(&mut PagedStats)) {
    f(&mut STATS.lock().unwrap());
}

impl DocModel {
    /// Whether this model is paged (lazy view prototype).
    pub fn is_paged(&self) -> bool {
        self.paged
    }

    /// Whether item `i` is a page placeholder.
    pub fn is_page(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|it| matches!(it.kind, ItemKind::Page(_)))
    }

    /// Builds a paged model: one streaming flatten pass in pages of [`PAGE_TOPS`] top-level
    /// blocks; only the placeholders and the outline are kept.
    pub(super) fn build_paged(doc: &dyn FormatDoc, path: Option<&Path>) -> DocModel {
        let n = doc.top_count();
        let mut pages: Vec<Item> = Vec::with_capacity(n / PAGE_TOPS + 1);
        let mut outline = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut k = 0;
        while k < n {
            let end = (k + PAGE_TOPS).min(n);
            let items = flatten(doc, k..end, pages.is_empty());
            k = end;
            if items.is_empty() {
                continue;
            }
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
            pages.push(page_item(&items, PageStats::of(&items)));
        }
        let base_dir = path.and_then(|p| p.parent()).map(Path::to_path_buf);
        let root_url = doc.resource_root();
        let min_level = outline.iter().map(|o| o.level).min().unwrap_or(1);
        let mut chunks = RelChunks::default();
        chunks.extend_abs(pages);
        DocModel {
            items: Items(chunks),
            outline,
            ctx_hash: images_hash(&root_url, &base_dir),
            base_dir,
            root_url,
            toc_runs: Vec::new(),
            min_level,
            paged: true,
        }
    }

    /// Replaces page placeholder `i` by its items (TOCs segmented and keyed). Indices after
    /// it shift: the returned change describes the splice.
    pub fn materialize(&mut self, doc: &dyn FormatDoc, i: usize) -> ModelChange {
        if !self.is_page(i) {
            return ModelChange {
                replaced: i..i,
                inserted: i..i,
                ..Default::default()
            };
        }
        let start = self.item_start(i);
        let t0 = doc.top_at(start).unwrap_or(0);
        let t1 = if i + 1 < self.items.len() {
            doc.top_at(self.item_start(i + 1)).unwrap_or(doc.top_count())
        } else {
            doc.top_count()
        }
        .max(t0 + 1)
        .min(doc.top_count());
        let raw = flatten(doc, t0..t1, i == 0);
        let items = self.segment_tocs(raw);
        let k = items.len();
        self.items.0.splice(i..i + 1, items, 0);
        self.after_paging();
        bump(|s| s.materialized += 1);
        ModelChange {
            replaced: i..i + 1,
            inserted: i..i + k,
            ..Default::default()
        }
    }

    /// Replaces real items `r` (whole top-level blocks) by one page placeholder that remembers
    /// `measured` (see [`PageStats::measured`]).
    pub fn evict(&mut self, r: std::ops::Range<usize>, measured: Option<(u32, u64, u32)>) -> ModelChange {
        if r.is_empty() || (r.start..r.end).any(|i| self.is_page(i)) {
            return ModelChange {
                replaced: r.start..r.start,
                inserted: r.start..r.start,
                ..Default::default()
            };
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
        let mut stats = PageStats::of(&items);
        stats.measured = measured;
        let page = page_item(&items, stats);
        self.items.0.splice(r.clone(), vec![page], 0);
        self.after_paging();
        bump(|s| s.evicted += 1);
        ModelChange {
            replaced: r.clone(),
            inserted: r.start..r.start + 1,
            ..Default::default()
        }
    }

    /// Segments and keys the `[toc]` blocks among freshly flattened items (absolute origins).
    fn segment_tocs(&self, raw: Vec<Item>) -> Vec<Item> {
        if !raw.iter().any(|it| it.kind == ItemKind::Toc) {
            return raw;
        }
        let n = self.outline.len();
        let groups = toc_groups(n);
        let keys: Vec<u64> = if groups == 0 {
            vec![self.outline_hash()]
        } else {
            (0..groups)
                .map(|gi| self.toc_group_hash(gi * super::SEG_UNITS..((gi + 1) * super::SEG_UNITS).min(n)))
                .collect()
        };
        let mut out = Vec::with_capacity(raw.len() + groups);
        for it in raw {
            if it.kind == ItemKind::Toc && it.segment.is_none() {
                let mut seg = toc_block_items(&it, groups, n);
                if groups == 0 {
                    seg[0].hash = mix(it.content_hash, keys[0]);
                } else {
                    for (s, &gh) in seg.iter_mut().zip(&keys) {
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

    /// After a materialization or eviction: `[toc]` runs and outline indices recomputed from
    /// the items (O(items + headings); the paged item list is small).
    fn after_paging(&mut self) {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut cur: Option<(usize, usize, usize)> = None;
        let mut k = 0;
        let n_out = self.outline.len();
        for (i, (o, it)) in self.items.iter_with_start().enumerate() {
            match cur {
                Some((a, _, co)) if it.kind == ItemKind::Toc && o == co => cur = Some((a, i + 1, co)),
                _ => {
                    if let Some((a, b, _)) = cur.take() {
                        runs.push((a, b - a));
                    }
                    if it.kind == ItemKind::Toc {
                        cur = Some((i, i + 1, o));
                    }
                }
            }
            match &it.kind {
                ItemKind::Heading { .. } => {
                    if k < n_out {
                        self.outline[k].item = i;
                    }
                    k += 1;
                }
                ItemKind::Page(p) => {
                    for _ in 0..p.headings() {
                        if k < n_out {
                            self.outline[k].item = i;
                        }
                        k += 1;
                    }
                }
                _ => {}
            }
        }
        if let Some((a, b, _)) = cur {
            runs.push((a, b - a));
        }
        if k != n_out {
            eprintln!("markise lazy view: outline has {n_out} entries, items {k} headings");
        }
        self.toc_runs = runs;
    }

    /// Paged models: whether edit `e` touches a page placeholder (then the incremental update
    /// cannot proceed; the model is rebuilt).
    pub(super) fn edit_touches_page(&self, e: &EditSummary) -> bool {
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
            if matches!(it.kind, ItemKind::Page(_)) && o + it.len >= lo {
                return true;
            }
            if o + it.len < lo {
                break;
            }
        }
        false
    }

    /// A fresh paged model for `doc` keeping this model's document folder (full rebuild).
    pub(super) fn rebuild_paged(&mut self, doc: &dyn FormatDoc) -> ModelChange {
        let n = self.items.len();
        let base_dir: Option<PathBuf> = self.base_dir.clone();
        let mut fresh = Self::build_paged(doc, None);
        fresh.base_dir = base_dir;
        fresh.ctx_hash = images_hash(&fresh.root_url, &fresh.base_dir);
        *self = fresh;
        bump(|s| s.rebuilds += 1);
        ModelChange {
            replaced: 0..n,
            inserted: 0..self.items.len(),
            full: true,
            ..Default::default()
        }
    }
}

impl DocModel {
    /// PROTOTYPE check: compares this paged model with a full (unpaged) build of `doc`: every
    /// materialized item must equal the full model's item at the same position, placeholders
    /// must cover exactly the full model's items in their span, and the outlines must agree.
    pub fn check_paged(&self, doc: &dyn FormatDoc) -> Result<(), String> {
        let mut full = DocModel {
            paged: false,
            ..Self::build_paged_off(doc)
        };
        full.rebuild_outline(doc);
        full.update_tocs(doc);
        let fi: Vec<(usize, &Item)> = full.items.iter_with_start().collect();
        let mut j = 0;
        let mut heads = 0usize;
        let pi: Vec<(usize, &Item)> = self.items.iter_with_start().collect();
        for (i, &(o, it)) in pi.iter().enumerate() {
            if let ItemKind::Page(p) = &it.kind {
                let next = pi.get(i + 1).map_or(usize::MAX, |x| x.0);
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
            let start = self.item_start(a.item);
            if !self.is_page(a.item) && start != full.item_start(b.item) {
                return Err(format!("outline entry {k} points at item {} ({start})", a.item));
            }
        }
        Ok(())
    }

    /// An unpaged model's items (no outline / TOCs yet).
    fn build_paged_off(doc: &dyn FormatDoc) -> DocModel {
        let mut chunks = RelChunks::default();
        let n = doc.top_count();
        let mut k = 0;
        while k < n {
            let end = (k + 512).min(n);
            let doc_start = chunks.is_empty();
            chunks.extend_abs(flatten(doc, k..end, doc_start));
            k = end;
        }
        DocModel {
            items: Items(chunks),
            outline: Vec::new(),
            ctx_hash: 0,
            base_dir: None,
            root_url: None,
            toc_runs: Vec::new(),
            min_level: 1,
            paged: false,
        }
    }
}
