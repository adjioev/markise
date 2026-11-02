//! Materialization and eviction of the pages of a paged model (issue #125; the model side is
//! `model/paged.rs`).
//!
//! Before each frame ([`View::prepare_pages`]) the view keeps the first and the last page
//! materialized (document edges), evicts runs of items more than [`KEEP_SCREENS`] viewports
//! away (cut at top-level block boundaries, at most a page of blocks each, never the caret's
//! page) and materializes every placeholder within one viewport above and two below. A
//! placeholder has one height in the Fenwick tree (its measured height when it was evicted,
//! else an estimate from its statistics). Materializing the page under the scroll anchor keeps
//! the anchor's relative position in the page; pages above the anchor only shift indices, so
//! the content on screen never moves. Jumps materialize their target first
//! ([`View::ensure_offset`], [`View::ensure_heading`]).

use std::sync::Arc;

use markise_doc::FormatDoc;

use super::View;
use crate::model::{ModelChange, PageStats, Paging};

/// Items farther than this many viewports from the viewport are evicted.
const KEEP_SCREENS: f64 = 4.0;
/// Placeholders within this many viewports above the viewport are materialized …
const AHEAD_ABOVE: f64 = 1.0;
/// … and within this many below.
const AHEAD_BELOW: f64 = 2.0;

/// Counters of a view's page materialization (diagnostics, benchmarks).
#[derive(Clone, Debug, Default)]
pub struct PagingStats {
    /// Layout requests for placeholders (they get an empty layout).
    pub placeholder_layouts: u64,
    pub materialized: u64,
    pub evicted: u64,
    /// Time spent materializing and evicting (ms).
    pub ms: f64,
    /// Largest change of the absolute scroll position caused by materializing the page under
    /// the anchor (px; the content on screen stays, the page was not shown).
    pub max_jump: f64,
    /// Sum of those changes (px).
    pub sum_jump: f64,
}

/// What a view keeps materialized besides the viewport.
#[derive(Clone, Debug, Default)]
pub(super) struct PagingState {
    /// Source offset of the caret: its page and the items around it stay materialized.
    caret: Option<usize>,
    /// Scroll position and item count at the last eviction scan (the scan runs again after
    /// a viewport of scrolling or a page's worth of new items).
    last_scan: Option<(f64, usize)>,
    pub(super) stats: PagingStats,
}

impl View {
    /// Page materialization counters.
    pub fn paging_stats(&self) -> &PagingStats {
        &self.paging.stats
    }

    /// Height estimate of a page placeholder with statistics `s` (its measured height while
    /// the view parameters are those it was measured with).
    pub(super) fn page_estimate(&self, s: &PageStats) -> f32 {
        if let Some((w, k, h)) = s.measured
            && w == (self.width * 4.0) as u32
            && k == self.paging_key()
        {
            return f32::from_bits(h);
        }
        let t = &self.theme;
        let line = t.line_px();
        let avg_char = t.font_size * 0.5;
        let per_line = (self.width / avg_char).max(10.0);
        let shown = s.items.saturating_sub(s.hidden) as f32;
        let mut h = shown * t.block_spacing;
        for (li, &n) in s.heads.iter().enumerate() {
            if n > 0 {
                let hs = &t.headings[li];
                let pad = 2.0 * t.heading_decor[li].padding[1];
                let extra = (hs.margin_top - t.block_spacing).max(0.0);
                h += n as f32 * (extra + hs.size * 1.3 + hs.margin_bottom + pad);
            }
        }
        h += s.code_lines as f32 * (t.mono_size() * 1.5).round() + s.code_blocks as f32 * 2.0 * t.code_block.padding;
        h += s.table_rows as f32 * (line + 2.0 * t.table_cell_padding);
        h += s.front as f32 * (line + 2.0 * t.meta_block.padding) + s.simple as f32 * line;
        // a TOC lists the whole outline, one line per entry
        h += s.tocs as f32 * self.model.outline.len().max(1) as f32 * line;
        h += s.para_lines_at(per_line) * line;
        h
    }

    /// Key of the parameters a measured page height is valid for (theme, fonts, ppp).
    fn paging_key(&self) -> u64 {
        self.theme_fp ^ self.font_gen.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ((self.ppp * 100.0) as u64) << 48
    }

    /// Sets the source offset of the caret (`None`: no caret): its page and the items right
    /// before and after it stay materialized; they are materialized now. No-op for a full
    /// model.
    pub fn set_caret_offset(&mut self, offset: Option<usize>) {
        if !self.model.is_paged() {
            return;
        }
        self.paging.caret = offset;
        if let Some(o) = offset {
            self.ensure_offset(o);
        }
    }

    /// Paged models: materializes the page holding source offset `offset` and the pages of
    /// the items right before and after it (caret moves cross items). No-op otherwise.
    pub fn ensure_offset(&mut self, offset: usize) {
        if !self.model.is_paged() || self.is_empty() {
            return;
        }
        // at most the page itself and its two neighbours
        for _ in 0..3 {
            let i = self.model.item_at_offset(offset);
            let j = [i, i.saturating_sub(1), (i + 1).min(self.len() - 1)]
                .into_iter()
                .find(|&j| self.model.is_page(j));
            match j {
                Some(j) => self.materialize_at(j),
                None => break,
            }
        }
    }

    /// The scroll position by source position, stable across materialization and eviction
    /// (unlike [`View::anchor`]'s item index): the anchor item's start, which of the items
    /// starting there it is (segments of a large block share their start), and the offset
    /// into it in points.
    pub fn source_anchor(&self) -> (usize, usize, f32) {
        let start = self.model.item_start(self.anchor);
        let first = self.model.items.partition_point_start(start);
        (start, self.anchor - first.min(self.anchor), self.anchor_off)
    }

    /// Scrolls to a position from [`View::source_anchor`] (materializing its page first).
    pub fn scroll_to_source(&mut self, start: usize, nth: usize, off: f32) {
        if self.is_empty() {
            return;
        }
        self.ensure_offset(start);
        let first = self.model.items.partition_point_start(start).min(self.len() - 1);
        let mut i = first;
        while i < first + nth && i + 1 < self.len() && self.model.item_start(i + 1) == start {
            i += 1;
        }
        self.scroll_to_item(i);
        self.scroll_by(off);
    }

    /// Materializes the page of outline entry `k` (paged models) and returns the heading's
    /// item.
    pub fn ensure_heading(&mut self, k: usize) -> Option<usize> {
        let i = self.model.outline.get(k)?.item;
        if self.model.is_page(i) {
            self.materialize_at(i);
        }
        let i = self.model.outline.get(k)?.item;
        if self.model.is_paged() {
            let off = self.model.item_start(i);
            self.ensure_offset(off);
        }
        self.model.outline.get(k).map(|o| o.item)
    }

    /// Paged models: after the model was replaced, materializes the page holding `offset`
    /// (the old anchor's position) and puts the anchor on its item.
    pub(super) fn anchor_at_offset(&mut self, offset: usize) {
        if !self.model.is_paged() || self.is_empty() {
            return;
        }
        self.ensure_offset(offset);
        self.anchor = self.model.item_at_offset(offset);
        self.anchor_off = 0.0;
    }

    /// Materializes page placeholder `i`. The content on screen stays where it is: the anchor
    /// follows its item, an anchor on the placeholder keeps its relative position in the page.
    pub(super) fn materialize_at(&mut self, i: usize) {
        let t = std::time::Instant::now();
        let before = self.scroll_y();
        let anchor_in = self.anchor == i;
        let frac = self.anchor_frac() as f64;
        let focus = self.focus.clone();
        let caret_item = self.caret_item;
        let doc = self.doc.clone();
        let ch = Arc::make_mut(&mut self.model).materialize(&*doc, i);
        if ch.replaced.is_empty() {
            return;
        }
        let splices = ch.splices();
        self.apply_change(doc, &ch);
        let map = |j: usize| ModelChange::map_index(&splices, j);
        // the editing focus stays (the caret's item is never a placeholder)
        self.focus = focus.and_then(|(j, f, h)| map(j).map(|j| (j, f, h)));
        self.caret_item = caret_item.and_then(map);
        if let Some(j) = self.focus.as_ref().map(|f| f.0)
            && j < self.exact.len()
        {
            let (v, e) = self.initial_height(j);
            self.heights.set(j, v);
            self.exact[j] = e;
        }
        if anchor_in && !ch.inserted.is_empty() {
            let tot = self.heights.prefix(ch.inserted.end) - self.heights.prefix(ch.inserted.start);
            self.anchor = ch.inserted.start;
            self.anchor_off = (frac * tot) as f32;
            self.scroll_by(0.0);
            let jump = (self.scroll_y() - before).abs();
            self.paging.stats.max_jump = self.paging.stats.max_jump.max(jump);
            self.paging.stats.sum_jump += jump;
        }
        self.paging.stats.materialized += 1;
        self.paging.stats.ms += t.elapsed().as_secs_f64() * 1e3;
    }

    /// Before a frame of a paged model: keeps the first and the last page materialized, evicts
    /// pages far from the viewport and the caret and materializes the pages near it.
    pub(super) fn prepare_pages(&mut self, viewport_h: f32) {
        if self.is_empty() {
            return;
        }
        if self.model.is_page(0) {
            self.materialize_at(0);
        }
        let n = self.len();
        if self.model.is_page(n - 1) {
            self.materialize_at(n - 1);
        }
        self.evict_far(viewport_h);
        let vh = viewport_h.max(1.0) as f64;
        // each round materializes one page (at most a few per frame in practice)
        for _ in 0..64 {
            let y = self.scroll_y();
            let lo = self.item_at_y(y - AHEAD_ABOVE * vh);
            let hi = self.item_at_y(y + AHEAD_BELOW * vh);
            match (lo..=hi).find(|&j| self.model.is_page(j)) {
                Some(j) => self.materialize_at(j),
                None => break,
            }
        }
        if let Some(off) = self.paging.caret {
            self.ensure_offset(off.min(self.doc.text().len()));
        }
    }

    /// Evicts runs of real items farther than [`KEEP_SCREENS`] viewports from the viewport,
    /// in chunks of at most a page of whole top-level blocks. Kept: the first and the last
    /// chunk (document edges), the chunk holding the caret, the focused item, and chunks with
    /// a disclosure marker paired with one outside the chunk (its pairing would be lost).
    fn evict_far(&mut self, viewport_h: f32) {
        let n = self.len();
        if n < 3 {
            return;
        }
        let vh = viewport_h.max(100.0) as f64;
        let y = self.scroll_y();
        if let Some((ly, ln)) = self.paging.last_scan
            && (y - ly).abs() < vh
            && n < ln + 64
        {
            return;
        }
        self.paging.last_scan = Some((y, n));
        let keep = (y - KEEP_SCREENS * vh)..(y + (KEEP_SCREENS + 1.0) * vh);
        // runs of real items
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut start: Option<usize> = None;
        for (i, it) in self.model.items.iter().enumerate() {
            match (it.is_page(), start) {
                (true, Some(a)) => {
                    runs.push((a, i));
                    start = None;
                }
                (false, None) => start = Some(i),
                _ => {}
            }
        }
        if let Some(a) = start {
            runs.push((a, n));
        }
        let doc: Arc<dyn FormatDoc> = self.doc.clone();
        let top = |v: &View, i: usize| doc.top_at(v.model.item_start(i)).unwrap_or(0);
        // the parts of the runs outside the kept band, cut at top-level block boundaries
        let mut far: Vec<std::ops::Range<usize>> = Vec::new();
        for &(a, b) in &runs {
            let ya = self.heights.prefix(a);
            let yb = self.heights.prefix(b);
            if yb <= keep.start || ya >= keep.end {
                far.push(a..b);
                continue;
            }
            if ya < keep.start {
                let mut s = self.heights.index_at(keep.start).clamp(a, b);
                while s > a && top(self, s) == top(self, s - 1) {
                    s -= 1;
                }
                if s > a {
                    far.push(a..s);
                }
            }
            if yb > keep.end {
                let mut s = (self.heights.index_at(keep.end) + 1).clamp(a, b);
                while s < b && top(self, s) == top(self, s - 1) {
                    s += 1;
                }
                if s < b {
                    far.push(s..b);
                }
            }
        }
        if far.is_empty() {
            return;
        }
        let Paging::Paged { tops: page_tops } = self.model.paging() else {
            return;
        };
        let caret_top = self.paging.caret.and_then(|o| doc.top_at(o));
        let focus = self.focus.as_ref().map(|f| f.0);
        let mut chunks: Vec<std::ops::Range<usize>> = Vec::new();
        for r in far {
            let mut s = r.start;
            while s < r.end {
                let t0 = top(self, s);
                let mut e = s + 1;
                while e < r.end && top(self, e) < t0 + page_tops {
                    e += 1;
                }
                let edge = s == 0 || e == n;
                let caret = caret_top.is_some_and(|c| (t0..=top(self, e - 1)).contains(&c));
                let held =
                    focus.is_some_and(|f| (s..e).contains(&f)) || self.caret_item.is_some_and(|c| (s..e).contains(&c));
                if !edge && !caret && !held && !self.disclosures.paired_outside(s..e) {
                    chunks.push(s..e);
                }
                s = e;
            }
        }
        if chunks.is_empty() {
            return;
        }
        let t = std::time::Instant::now();
        let key = self.paging_key();
        let wq = (self.width * 4.0) as u32;
        // last first: the indices of the chunks before stay valid
        for r in chunks.into_iter().rev() {
            let h = (self.heights.prefix(r.end) - self.heights.prefix(r.start)) as f32;
            let ch = Arc::make_mut(&mut self.model).evict(&*doc, r, Some((wq, key, h.to_bits())));
            if ch.replaced.is_empty() {
                continue;
            }
            let splices = ch.splices();
            let focus = self.focus.clone();
            let caret_item = self.caret_item;
            self.apply_change(doc.clone(), &ch);
            let map = |j: usize| ModelChange::map_index(&splices, j);
            self.focus = focus.and_then(|(j, f, h)| map(j).map(|j| (j, f, h)));
            self.caret_item = caret_item.and_then(map);
            self.paging.stats.evicted += 1;
        }
        self.paging.last_scan = Some((self.scroll_y(), self.len()));
        self.paging.stats.ms += t.elapsed().as_secs_f64() * 1e3;
    }
}
