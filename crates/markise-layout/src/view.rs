//! Virtualised document view: scroll anchor, item heights and the per-frame visible set.
//!
//! The scroll position is an *anchor* — (item index, offset into that item) — not an absolute
//! y. Items that have not been laid out yet carry an estimated height; when an item above the
//! anchor is laid out and its height changes, the content on screen does not move (no scroll
//! jumps). Absolute positions (scrollbar) come from a Fenwick tree of heights in `f64`.

use std::collections::HashSet;
use std::sync::Arc;

use markise_doc::FormatDoc;
use markise_theme::Theme;

use crate::block::{BlockLayout, Images, ItemFocus, LayoutParams, layout_item};
use crate::cache::{CacheKey, LayoutCache};
use crate::diagram::{DiagramQuery, DiagramState, Diagrams, NoDiagrams};
use crate::fenwick::Fenwick;
use crate::highlight::{CodeQuery, Highlights, NoHighlights, item_range};
use crate::links::{AllResolved, LinkTargets};
use crate::model::{DocModel, Item, ItemKind, ModelChange};
use crate::text::TextShaper;

/// An item to paint this frame.
#[derive(Clone, Debug)]
pub struct Visible {
    pub idx: usize,
    /// Top of the item's layout relative to the viewport top (may be negative).
    pub y: f32,
    pub layout: Arc<BlockLayout>,
}

pub struct View {
    /// The document the model was built from (layout reads it through the format seam).
    doc: Arc<dyn FormatDoc>,
    model: Arc<DocModel>,
    heights: Fenwick,
    exact: Vec<bool>,
    theme: Theme,
    theme_fp: u64,
    width: f32,
    ppp: f32,
    anchor: usize,
    anchor_off: f32,
    expanded: HashSet<usize>,
    image_epoch: u64,
    /// Bumped when rendered math arrives ([`View::math_changed`]).
    math_epoch: u64,
    font_gen: u64,
    pub cache: LayoutCache,
    /// Items laid out (cache misses) during the last [`View::frame`] call.
    pub laid_out: Vec<usize>,
    /// Items returned as visible by the last [`View::frame`] call.
    pub last_visible: Vec<usize>,
    /// A laid-out item contains characters the current font chain has no glyph for (the UI
    /// can then load fallback fonts, e.g. CJK, and bump the font generation).
    missing_glyphs: bool,
    /// Source of code highlight runs ([`NoHighlights`] = plain code).
    highlights: Arc<dyn Highlights + Send + Sync>,
    /// Source of diagram scenes ([`NoDiagrams`] = diagrams shown as source).
    diagrams: Arc<dyn Diagrams + Send + Sync>,
    /// Which wikilink targets exist ([`AllResolved`] = all shown resolved).
    links: Arc<dyn LinkTargets + Send + Sync>,
    /// Reference labels per item hash ([`DocModel::item_references`]), computed when an item's
    /// key is first needed: reference definitions enter only the keys of items that may use
    /// them (ADR 0006 §7.4) without a document-wide scan when the model is built.
    refs: std::cell::RefCell<std::collections::HashMap<u64, Box<[u64]>>>,
    /// Editing state of the item holding the caret (ADR 0006 §2.4): part of that item's key.
    focus: Option<(usize, ItemFocus, u64)>,
}

impl View {
    pub fn new(doc: Arc<dyn FormatDoc>, model: Arc<DocModel>, theme: Theme, width: f32, ppp: f32) -> View {
        let theme_fp = theme.fingerprint();
        let mut v = View {
            doc,
            model,
            heights: Fenwick::default(),
            exact: Vec::new(),
            theme,
            theme_fp,
            width,
            ppp,
            anchor: 0,
            anchor_off: 0.0,
            expanded: HashSet::new(),
            image_epoch: 0,
            math_epoch: 0,
            font_gen: 0,
            cache: LayoutCache::default(),
            laid_out: Vec::new(),
            last_visible: Vec::new(),
            missing_glyphs: false,
            highlights: Arc::new(NoHighlights),
            diagrams: Arc::new(NoDiagrams),
            links: Arc::new(AllResolved),
            refs: Default::default(),
            focus: None,
        };
        v.reset_heights();
        v
    }

    pub fn model(&self) -> &Arc<DocModel> {
        &self.model
    }

    /// The document shown.
    pub fn doc(&self) -> &Arc<dyn FormatDoc> {
        &self.doc
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn len(&self) -> usize {
        self.model.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.model.items.is_empty()
    }

    /// Replaces the document (reload, source-mode edit). The scroll anchor stays on the same
    /// byte offset; layouts of unchanged items are reused from the cache (their keys are
    /// item-relative, so items below a change keep them).
    pub fn set_model(&mut self, doc: Arc<dyn FormatDoc>, model: Arc<DocModel>) {
        self.focus = None;
        let anchor_pos = self.model.items.start(self.anchor).unwrap_or(0);
        self.doc = doc;
        self.model = model;
        self.prune_refs();
        self.expanded.clear();
        self.anchor = if self.model.items.is_empty() {
            0
        } else {
            self.model.item_at_offset(anchor_pos)
        };
        self.anchor_off = 0.0;
        self.reset_heights();
    }

    /// Installs a model updated incrementally ([`DocModel::apply`]) for `doc`: heights of the
    /// replaced items are spliced (O(changed) when the item count is unchanged, else one
    /// linear pass over the heights), the anchor stays on its item (or moves to the first
    /// inserted item when its own was replaced), other items keep their heights and cached
    /// layouts.
    pub fn apply(&mut self, doc: Arc<dyn FormatDoc>, model: Arc<DocModel>, change: &ModelChange) {
        if change.full {
            self.set_model(doc, model);
            return;
        }
        let shift = change.inserted.len() as isize - change.replaced.len() as isize;
        let r = change.replaced.clone();
        if r.end > self.exact.len() || self.exact.len() as isize + shift != model.items.len() as isize {
            // the change does not describe the transition from the current model (defensive)
            self.set_model(doc, model);
            return;
        }
        self.model = model;
        self.apply_change(doc, change);
    }

    /// The model, for an in-place incremental update ([`DocModel::apply`]) followed by
    /// [`View::apply_change`]. Clones the model only if someone else holds it.
    pub fn model_mut(&mut self) -> &mut DocModel {
        Arc::make_mut(&mut self.model)
    }

    /// Swaps the document without touching the model or heights (the caller is about to edit
    /// the returned document in place and then call [`View::apply_change`]). Nothing may be
    /// laid out in between.
    pub fn replace_doc(&mut self, doc: Arc<dyn FormatDoc>) -> Arc<dyn FormatDoc> {
        std::mem::replace(&mut self.doc, doc)
    }

    /// Installs `doc` after the view's own model was updated in place ([`View::model_mut`] +
    /// [`DocModel::apply`] returned `change`): heights are spliced as in [`View::apply`].
    pub fn apply_change(&mut self, doc: Arc<dyn FormatDoc>, change: &ModelChange) {
        self.doc = doc;
        let old_focus = self.focus.take().map(|f| f.0);
        if change.full {
            let m = self.model.clone();
            self.set_model(self.doc.clone(), m);
            return;
        }
        let shift = change.inserted.len() as isize - change.replaced.len() as isize;
        let r = change.replaced.clone();
        if r.end > self.exact.len() || self.exact.len() as isize + shift != self.model.items.len() as isize {
            let m = self.model.clone();
            self.set_model(self.doc.clone(), m);
            return;
        }
        // anchor
        if self.anchor >= r.end {
            self.anchor = (self.anchor as isize + shift).max(0) as usize;
        } else if self.anchor >= r.start && self.anchor < r.end {
            self.anchor = change.inserted.start;
            self.anchor_off = 0.0;
        }
        // expanded blocks
        if !self.expanded.is_empty() {
            self.expanded = self
                .expanded
                .iter()
                .filter(|&&i| i < r.start || i >= r.end)
                .map(|&i| if i >= r.end { (i as isize + shift) as usize } else { i })
                .collect();
        }
        // heights of the inserted items: cached layouts (same keys) or estimates
        let n = self.model.items.len();
        let mut values = Vec::with_capacity(change.inserted.len());
        let mut exact = Vec::with_capacity(change.inserted.len());
        for i in change.inserted.clone() {
            let (v, e) = self.initial_height(i);
            values.push(v);
            exact.push(e);
        }
        self.heights.splice(r.clone(), values);
        self.exact.splice(r, exact);
        if self.heights.len() != n {
            // the change does not describe the item list (defensive): start over
            self.reset_heights();
            return;
        }
        self.prune_refs();
        // the last item carries the bottom padding: re-estimate around the ends
        let mut again: Vec<usize> = change.rehashed.clone();
        // the formerly focused item is now laid out without its focus
        if let Some(f) = old_focus {
            if f < change.replaced.start {
                again.push(f);
            } else if f >= change.replaced.end {
                again.push((f as isize + shift) as usize);
            }
        }
        // items that may use a changed reference definition get new keys: re-estimate them
        if !change.labels.is_empty() {
            let memo = self.refs.borrow();
            for (i, it) in self.model.items.iter().enumerate() {
                if memo
                    .get(&it.hash)
                    .is_some_and(|l| l.iter().any(|x| change.labels.contains(x)))
                {
                    again.push(i);
                }
            }
        }
        again.extend([0, n.saturating_sub(1), n.saturating_sub(2)]);
        for i in again {
            if i < n {
                let (v, e) = self.initial_height(i);
                self.heights.set(i, v);
                self.exact[i] = e;
            }
        }
        self.anchor = self.anchor.min(n.saturating_sub(1));
    }

    /// Height of item `i` before this frame's layout: cached layout or estimate.
    fn initial_height(&self, i: usize) -> (f64, bool) {
        match self.cache.peek(&self.key(i)) {
            Some(l) => {
                let (a, b) = self.item_pad(i);
                ((l.height + a + b) as f64, true)
            }
            None => (self.estimate(i) as f64, false),
        }
    }

    /// Updates width / theme / pixels-per-point; any change re-estimates all heights (cached
    /// layouts for the old parameters simply stop matching their keys).
    pub fn set_params(&mut self, width: f32, theme: &Theme, ppp: f32) {
        let width = (width * 4.0).round() / 4.0;
        let fp = theme.fingerprint();
        if width == self.width && fp == self.theme_fp && ppp == self.ppp {
            return;
        }
        let frac = self.anchor_frac();
        self.width = width;
        self.theme = theme.clone();
        self.theme_fp = fp;
        self.ppp = ppp;
        self.reset_heights();
        self.anchor_off = frac * self.height(self.anchor);
    }

    /// Fonts changed (e.g. a fallback font was installed): every cached layout is stale.
    pub fn set_font_generation(&mut self, generation: u64) {
        if generation != self.font_gen {
            let frac = self.anchor_frac();
            self.font_gen = generation;
            self.missing_glyphs = false; // re-checked with the new fonts
            self.reset_heights();
            self.anchor_off = frac * self.height(self.anchor);
        }
    }

    /// Bumps the math epoch: items that may show math get new cache keys and are laid out
    /// again with the formulas rendered since (see [`crate::math`]).
    pub fn math_changed(&mut self) {
        self.math_epoch += 1;
        for (i, it) in self.model.items.iter().enumerate() {
            if it.has_math {
                self.exact[i] = false;
            }
        }
    }

    /// Bumps the image epoch: items that mention images get new cache keys and are laid out
    /// again with the new image sizes.
    pub fn images_changed(&mut self) {
        self.image_epoch += 1;
        for (i, it) in self.model.items.iter().enumerate() {
            if it.has_image {
                self.exact[i] = false;
            }
        }
    }

    /// Sets the source of code highlighting (`NoHighlights` switches it off). Code items get new
    /// cache keys through the source's generation; heights are unaffected (colors only).
    pub fn set_highlights(&mut self, h: Arc<dyn Highlights + Send + Sync>) {
        self.highlights = h;
    }

    pub fn highlights(&self) -> &Arc<dyn Highlights + Send + Sync> {
        &self.highlights
    }

    /// Sets the source of diagram scenes (`NoDiagrams` shows diagram fences as source). Diagram
    /// items get new cache keys through the source's generation and their state; their
    /// heights are re-measured when laid out.
    pub fn set_diagrams(&mut self, d: Arc<dyn Diagrams + Send + Sync>) {
        self.diagrams = d;
        for (i, it) in self.model.items.iter().enumerate() {
            if matches!(it.kind, ItemKind::Diagram { .. }) && i < self.exact.len() {
                self.exact[i] = false;
            }
        }
    }

    pub fn diagrams(&self) -> &Arc<dyn Diagrams + Send + Sync> {
        &self.diagrams
    }

    /// Sets which wikilink targets exist (spec 06 §2.2 unresolved style). Items that may hold
    /// a wikilink get new cache keys through its generation; heights are unaffected.
    pub fn set_links(&mut self, l: Arc<dyn LinkTargets + Send + Sync>) {
        self.links = l;
    }

    pub fn links(&self) -> &Arc<dyn LinkTargets + Send + Sync> {
        &self.links
    }

    fn diagram_query_of<'a>(
        &'a self,
        i: usize,
        it: &'a Item,
        start: impl FnOnce() -> usize,
    ) -> Option<DiagramQuery<'a>> {
        let ItemKind::Diagram { lang } = &it.kind else {
            return None;
        };
        let code = it.code?;
        let lang = markise_blocks::diagram::DiagramLang::from_info(lang)?;
        Some(DiagramQuery {
            lang,
            code,
            doc: &self.doc,
            start: start(),
            item: i,
        })
    }

    /// The diagram state of item `i` (`None` for other items).
    fn diagram_state_of(&self, i: usize, it: &Item, start: impl FnOnce() -> usize) -> Option<DiagramState> {
        self.diagram_query_of(i, it, start).map(|q| self.diagrams.state(&q))
    }

    /// The highlighting question of code item `i`, if it has one.
    fn code_query(&self, i: usize) -> Option<CodeQuery<'_>> {
        let it = self.model.items.get(i)?;
        self.code_query_of(i, it, || self.model.item_start(i))
    }

    fn code_query_of<'a>(&'a self, i: usize, it: &'a Item, start: impl FnOnce() -> usize) -> Option<CodeQuery<'a>> {
        let code = it.code?;
        let ItemKind::Code { lang } = &it.kind else { return None };
        Some(CodeQuery {
            code,
            lang,
            range: item_range(it, &code),
            doc: &self.doc,
            start: start(),
            item: i,
        })
    }

    /// Toggles a collapsible block (front matter).
    pub fn toggle_expanded(&mut self, idx: usize) {
        if !self.expanded.remove(&idx) {
            self.expanded.insert(idx);
        }
        if idx < self.exact.len() {
            self.exact[idx] = false;
        }
    }

    fn anchor_frac(&self) -> f32 {
        let h = self.height(self.anchor);
        if h > 0.0 {
            (self.anchor_off / h).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    fn pad_top(&self) -> f32 {
        self.theme.column_padding_y
    }

    /// Extra space attached to item `i` (column padding above the first / below the last).
    fn item_pad(&self, i: usize) -> (f32, f32) {
        let n = self.model.items.len();
        let top = if i == 0 { self.pad_top() } else { 0.0 };
        let bottom = if i + 1 == n {
            self.theme.column_padding_y * 2.0
        } else {
            0.0
        };
        (top, bottom)
    }

    fn reset_heights(&mut self) {
        let n = self.model.items.len();
        let mut values = Vec::with_capacity(n);
        let mut exact = Vec::with_capacity(n);
        let model = self.model.clone();
        // a fresh view has no cached layouts: skip the key computation
        let peek = !self.cache.is_empty();
        for (i, (start, it)) in model.items.iter_with_start().enumerate() {
            // layouts of unchanged items survive reloads and parameter round-trips
            let cached = if peek {
                let ready = self.code_query_of(i, it, || start).map(|q| self.highlights.ready(&q));
                let dtag = self.diagram_state_of(i, it, || start).map(|d| d.tag());
                self.cache.peek(&self.key_for(i, it, ready, dtag))
            } else {
                None
            };
            match cached {
                Some(l) => {
                    let (a, b) = self.item_pad(i);
                    values.push((l.height + a + b) as f64);
                    exact.push(true);
                }
                None => {
                    values.push(self.estimate_item(i, it) as f64);
                    exact.push(false);
                }
            }
        }
        self.heights = Fenwick::from_values(values);
        self.exact = exact;
        self.anchor = self.anchor.min(n.saturating_sub(1));
    }

    /// Sets the editing state of item `i` (marker reveal, virtual paragraph; ADR 0006 §2.4) or
    /// clears it. Only that item's cache key changes; returns `true` when the focus changed.
    pub fn set_focus(&mut self, focus: Option<(usize, ItemFocus)>) -> bool {
        let focus = focus.filter(|(i, f)| *i < self.len() && !f.is_empty());
        let same = match (&self.focus, &focus) {
            (None, None) => true,
            (Some((a, fa, _)), Some((b, fb))) => a == b && fa == fb,
            _ => false,
        };
        if same {
            return false;
        }
        let old = self.focus.take().map(|f| f.0);
        self.focus = focus.map(|(i, f)| {
            let mut h = crate::model::hasher();
            std::hash::Hash::hash(&f, &mut h);
            (i, f, std::hash::Hasher::finish(&h))
        });
        for i in old.into_iter().chain(self.focus.as_ref().map(|f| f.0)) {
            if i < self.exact.len() {
                let (v, e) = self.initial_height(i);
                self.heights.set(i, v);
                self.exact[i] = e;
            }
        }
        true
    }

    /// The current focus (item, state).
    pub fn focus(&self) -> Option<(usize, &ItemFocus)> {
        self.focus.as_ref().map(|(i, f, _)| (*i, f))
    }

    /// Absolute y of the top of item `i`'s layout (as [`Visible::y`] is relative to the
    /// viewport: includes the column padding above the first item).
    pub fn item_y(&self, i: usize) -> f64 {
        let i = i.min(self.heights.len());
        self.heights.prefix(i) + self.item_pad(i).0 as f64
    }

    /// Index of the item at absolute y (clamped).
    pub fn item_at_y(&self, y: f64) -> usize {
        if self.is_empty() {
            return 0;
        }
        self.heights
            .index_at(y.clamp(0.0, self.total_height()))
            .min(self.len() - 1)
    }

    /// Height estimate of item `i` before layout.
    pub fn estimate(&self, i: usize) -> f32 {
        match self.model.items.get(i) {
            Some(it) => self.estimate_item(i, it),
            None => 0.0,
        }
    }

    fn estimate_item(&self, i: usize, it: &Item) -> f32 {
        let t = &self.theme;
        let (pa, pb) = self.item_pad(i);
        let line = t.line_px();
        let avg_char = t.font_size * 0.5;
        let per_line = (self.width / avg_char).max(10.0);
        let wrapped = |chars: u32, lines: u32| -> f32 { (lines as f32).max(chars as f32 / per_line).ceil() };
        let h = match &it.kind {
            ItemKind::Hidden => return pa + pb,
            ItemKind::Heading { level } => {
                let li = (*level as usize - 1).min(5);
                let hs = &t.headings[li];
                let pad = 2.0 * t.heading_decor[li].padding[1];
                t.block_spacing.max(hs.margin_top) + hs.size * 1.3 + hs.margin_bottom + pad
            }
            ItemKind::Code { .. } | ItemKind::Math | ItemKind::Diagram { .. } | ItemKind::Html => {
                let lines = it.est_lines.saturating_sub(2).max(1) as f32;
                t.block_spacing + lines * (t.mono_size() * 1.5).round() + 2.0 * t.code_block.padding
            }
            ItemKind::Table => {
                let rows = it.est_lines.saturating_sub(1).max(1) as f32;
                t.block_spacing + rows * (line + 2.0 * t.table_cell_padding)
            }
            ItemKind::FrontMatter => t.block_spacing + line + 2.0 * t.meta_block.padding,
            ItemKind::Rule | ItemKind::Empty | ItemKind::Toc => t.block_spacing + line,
            ItemKind::Paragraph => t.block_spacing + wrapped(it.est_chars, it.est_lines) * line,
        };
        h + pa + pb
    }

    fn key(&self, i: usize) -> CacheKey {
        let ready = self.code_query(i).map(|q| self.highlights.ready(&q));
        let dtag = self
            .model
            .items
            .get(i)
            .and_then(|it| self.diagram_state_of(i, it, || self.model.item_start(i)))
            .map(|d| d.tag());
        self.key_with(i, ready, dtag)
    }

    /// The cache key of item `i`; `ready`: for a code item, whether its highlight is ready.
    fn key_with(&self, i: usize, ready: Option<bool>, dtag: Option<u64>) -> CacheKey {
        let it = &self.model.items[i];
        self.key_for(i, it, ready, dtag)
    }

    /// `dtag`: for a diagram item, its [`DiagramState::tag`].
    fn key_for(&self, i: usize, it: &Item, ready: Option<bool>, dtag: Option<u64>) -> CacheKey {
        let mut extra = 0u64;
        if it.has_image {
            extra = self.image_epoch << 1;
        }
        if self.expanded.contains(&i) {
            extra |= 1;
        }
        if let Some((fi, _, fh)) = &self.focus
            && *fi == i
        {
            extra ^= fh.rotate_left(7) | (1 << 62);
        }
        if it.has_math {
            extra ^= self.math_epoch.wrapping_mul(0x2545_f491_4f6c_dd1d);
        }
        if it.has_wikilink {
            extra ^= self
                .links
                .generation()
                .wrapping_add(1)
                .wrapping_mul(0x5851_f42d_4c95_7f2d)
                .rotate_left(29);
        }
        // code: highlighter generation and whether this item's range is highlighted yet
        let mut code_ident = 0;
        if let Some(ready) = ready {
            extra ^= (self.highlights.generation().wrapping_mul(0x9e37_79b9) << 33) ^ ((ready as u64) << 32);
            if ready && let Some(c) = &it.code {
                // the colors of a segment depend on the whole block before it (parser state),
                // not only on its own text: key highlighted layouts by block identity + offset
                let mut h = std::collections::hash_map::DefaultHasher::new();
                std::hash::Hash::hash(&(c.key, c.offset), &mut h);
                code_ident = std::hash::Hasher::finish(&h);
            }
        }
        // image resolution inputs matter only to items showing images (§7.4 `images`);
        // reference definitions only to items that may use them
        let images = if it.has_image { self.model.ctx_hash } else { 0 };
        // diagrams: renderer generation and the item's state (source / pending / scene / error);
        // a scene or error notice depends on the whole block, not on this segment's text: key
        // those by the block's content key too (an edit in a later segment changes the scene
        // drawn by an unchanged first segment)
        let diagram = dtag.map_or(0, |tag| {
            let block = if tag >= 2 { it.code.map_or(0, |c| c.key) } else { 0 };
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&(self.diagrams.generation(), tag, block, 0xd1a6_u32), &mut h);
            std::hash::Hasher::finish(&h)
        });
        CacheKey {
            item_hash: it.hash,
            ctx_hash: images ^ code_ident ^ diagram ^ self.references_fingerprint(i, it),
            width_q: (self.width * 4.0) as u32,
            theme: self.theme_fp ^ self.font_gen.wrapping_mul(0x9e37_79b9_7f4a_7c15),
            ppp_q: (self.ppp * 100.0).round() as u32,
            extra,
        }
    }

    /// Drops reference-memo entries of item hashes no longer in the model once the memo
    /// outgrows it (edits leave old hashes behind). Entries of current items are kept: they are
    /// what definition changes consult to demote the heights of dependent items.
    fn prune_refs(&self) {
        let n = self.model.items.len();
        if self.refs.borrow().len() <= 2 * n + 4096 {
            return;
        }
        let live: std::collections::HashSet<u64> = self.model.items.iter().map(|it| it.hash).collect();
        self.refs.borrow_mut().retain(|h, _| live.contains(h));
    }

    /// Fingerprint of the definitions item `i` may use (0 for none).
    fn references_fingerprint(&self, i: usize, it: &Item) -> u64 {
        if !matches!(
            it.kind,
            ItemKind::Paragraph | ItemKind::Heading { .. } | ItemKind::Table
        ) {
            return 0;
        }
        let mut memo = self.refs.borrow_mut();
        let labels = memo
            .entry(it.hash)
            .or_insert_with(|| self.model.item_references(&*self.doc, i).into_boxed_slice());
        if labels.is_empty() {
            0
        } else {
            self.doc.reference_fingerprints(labels).rotate_left(17)
        }
    }

    /// The cached layout of item `i` for the current parameters, if any (no layout).
    pub fn cache_peek(&self, i: usize) -> Option<&Arc<BlockLayout>> {
        if i >= self.len() {
            return None;
        }
        self.cache.peek(&self.key(i))
    }

    /// The layout-cache key of item `i` (tests, diagnostics).
    pub fn cache_key(&self, i: usize) -> CacheKey {
        self.key(i)
    }

    /// Current height of item `i` (exact if laid out, else estimated).
    pub fn height(&self, i: usize) -> f32 {
        if i < self.heights.len() {
            self.heights.get(i) as f32
        } else {
            0.0
        }
    }

    pub fn is_exact(&self, i: usize) -> bool {
        self.exact.get(i).copied().unwrap_or(false)
    }

    /// Layout of item `i` (cached or computed now).
    pub fn layout(&mut self, i: usize, shaper: &mut dyn TextShaper, images: &dyn Images) -> Arc<BlockLayout> {
        let ready = self.code_query(i).map(|q| {
            let r = self.highlights.ready(&q);
            if !r {
                // shown plain: (re)request its highlight (no-op when already queued)
                self.highlights.want(&q);
            }
            r
        });
        // label metrics for diagrams, measured once per theme / font change with this shaper
        if self
            .model
            .items
            .get(i)
            .is_some_and(|it| matches!(it.kind, ItemKind::Diagram { .. }))
        {
            let fonts = self.theme_fp
                ^ self.font_gen.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ shaper.generation().wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
            if self.diagrams.needs_metrics(fonts) {
                let m = crate::block::label_metrics(shaper);
                self.diagrams.set_metrics(fonts, m);
            }
        }
        let dstate = self.model.items.get(i).and_then(|it| {
            let q = self.diagram_query_of(i, it, || self.model.item_start(i))?;
            let st = self.diagrams.state(&q);
            if matches!(st, DiagramState::Pending) {
                // shown as source: (re)request the scene (no-op when already queued)
                self.diagrams.want(&q);
            }
            Some(st)
        });
        let mut key = self.key_with(i, ready, dstate.as_ref().map(DiagramState::tag));
        // the backend's own generation (0 for epaint: keys unchanged)
        key.theme ^= shaper.generation().wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        let l = match self.cache.get(&key) {
            Some(l) => l,
            None => {
                let runs = match ready {
                    Some(true) => self.code_query(i).and_then(|q| self.highlights.runs(&q)),
                    _ => None,
                };
                // evicted between `ready` and `runs`: lay out plain, but not under the
                // "highlighted" key
                let cacheable = ready != Some(true) || runs.is_some();
                let params = LayoutParams {
                    theme: &self.theme,
                    width: self.width,
                    expanded: self.expanded.contains(&i),
                    code_runs: runs.as_deref(),
                    focus: self.focus.as_ref().filter(|f| f.0 == i).map(|f| &f.1),
                    diagram: dstate.as_ref(),
                    ppp: self.ppp,
                    links: &*self.links,
                };
                let l = Arc::new(layout_item(&*self.doc, &self.model, i, &params, shaper, images));
                if !self.missing_glyphs {
                    self.missing_glyphs = lacks_glyphs(&l, shaper);
                }
                if cacheable {
                    self.cache.insert(key, l.clone());
                }
                self.laid_out.push(i);
                l
            }
        };
        let (a, b) = self.item_pad(i);
        self.heights.set(i, (l.height + a + b) as f64);
        self.exact[i] = true;
        l
    }

    /// Whether a laid-out item needed glyphs the fonts lack (since the last font change).
    pub fn missing_glyphs(&self) -> bool {
        self.missing_glyphs
    }

    /// Total (partly estimated) document height.
    pub fn total_height(&self) -> f64 {
        self.heights.total()
    }

    /// Absolute scroll position of the viewport top.
    pub fn scroll_y(&self) -> f64 {
        self.heights.prefix(self.anchor) + self.anchor_off as f64
    }

    pub fn anchor(&self) -> (usize, f32) {
        (self.anchor, self.anchor_off)
    }

    /// Jumps to an absolute position (scrollbar drag).
    pub fn scroll_to_y(&mut self, y: f64) {
        if self.is_empty() {
            return;
        }
        let y = y.clamp(0.0, self.total_height());
        self.anchor = self.heights.index_at(y);
        self.anchor_off = (y - self.heights.prefix(self.anchor)).max(0.0) as f32;
    }

    /// Puts item `i` at the top of the viewport (outline click).
    pub fn scroll_to_item(&mut self, i: usize) {
        if i < self.len() {
            self.anchor = i;
            self.anchor_off = 0.0;
        }
    }

    /// Scrolls by `dy` points (positive = down) using the current height knowledge.
    pub fn scroll_by(&mut self, dy: f32) {
        let n = self.len();
        if n == 0 {
            return;
        }
        self.anchor_off += dy;
        while self.anchor_off < 0.0 && self.anchor > 0 {
            self.anchor -= 1;
            self.anchor_off += self.height(self.anchor);
        }
        if self.anchor_off < 0.0 {
            self.anchor_off = 0.0;
        }
        while self.anchor + 1 < n && self.anchor_off >= self.height(self.anchor) {
            self.anchor_off -= self.height(self.anchor);
            self.anchor += 1;
        }
    }

    /// Lays out what the viewport (`viewport_h` tall) needs plus `margin` above and below and
    /// returns the items intersecting the viewport, top to bottom. Never touches other items.
    pub fn frame(
        &mut self,
        viewport_h: f32,
        margin: f32,
        shaper: &mut dyn TextShaper,
        images: &dyn Images,
    ) -> Vec<Visible> {
        self.cache.tick();
        self.laid_out.clear();
        let n = self.len();
        if n == 0 {
            self.last_visible.clear();
            return Vec::new();
        }
        let mut out = Vec::new();
        for _attempt in 0..4 {
            out.clear();
            // the anchor offset may exceed the anchor's height once it is laid out
            loop {
                self.layout(self.anchor, shaper, images);
                let h = self.height(self.anchor);
                if self.anchor_off >= h && self.anchor + 1 < n {
                    self.anchor_off -= h;
                    self.anchor += 1;
                } else {
                    break;
                }
            }
            let mut y = -self.anchor_off;
            let mut i = self.anchor;
            while i < n && y < viewport_h + margin {
                let l = self.layout(i, shaper, images);
                let h = self.height(i);
                if y + h > 0.0 && y < viewport_h {
                    let (pad, _) = self.item_pad(i);
                    out.push(Visible {
                        idx: i,
                        y: y + pad,
                        layout: l,
                    });
                }
                y += h;
                i += 1;
            }
            // bottom clamp: do not scroll past the end of the document
            if i >= n && y < viewport_h && (self.anchor > 0 || self.anchor_off > 0.0) {
                self.scroll_by(y - viewport_h);
                continue;
            }
            break;
        }
        // margin above: prefetch (heights become exact before they scroll into view)
        let mut y = -self.anchor_off;
        let mut i = self.anchor;
        while i > 0 && y > -margin {
            i -= 1;
            self.layout(i, shaper, images);
            y -= self.height(i);
        }
        self.last_visible = out.iter().map(|v| v.idx).collect();
        out
    }
}

/// Whether the layout's text has a non-ASCII character without a glyph in the font chain of
/// the run it is in (each run's own font: body, heading, mono and sidebar chains differ).
fn lacks_glyphs(l: &BlockLayout, shaper: &mut dyn TextShaper) -> bool {
    l.prims.iter().any(|p| match p {
        crate::block::Prim::Text { text, .. } => shaper.lacks_glyphs(&**text),
        _ => false,
    })
}
