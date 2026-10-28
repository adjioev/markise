//! Side by Side view mode (spec 04 §3.3) — interim delivery (owner, 2026-10-27).
//!
//! The existing source editor ([`SourceState`], a `TextEdit` with the M1 2 MB limit) on the
//! left, the rendered document ([`DocViewer`]) on the right, a draggable divider in between.
//! The rendered pane is read-only until M2b; both panes become editors with M2d.
//!
//! - **Edits** in the left pane are applied to the document as minimal patches through the
//!   incremental path (`FormatDoc::apply` → `DocModel::apply` → `View::apply`) on the UI
//!   thread, so the right pane follows each keystroke without a full rebuild or relayout.
//! - **Scroll sync** follows only the pane the user scrolls (no feedback loop): a programmatic
//!   scroll of the other pane is never treated as user scrolling. Source → rendered maps the
//!   topmost visible source line through the model's item spans (interpolated inside an item);
//!   rendered → source maps the topmost visible item back to its source lines. Source without a
//!   rendered counterpart maps to the next item, or to the end of the rendered document.
//! - **Caret moves** and typing in the left pane scroll the right pane only when the caret's
//!   block would otherwise be out of view.
//! - **Undo** in the left pane is `TextEdit`'s own; the shared live history arrives with the
//!   M2d source editor (spec 02 §12.1.1).

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use egui::{CursorIcon, Rect, Sense, Ui, UiBuilder, pos2};
use markise_doc::{Patch, PatchSet};
use markise_layout::block::color;
use markise_layout::model::Segment;
use markise_layout::{DocModel, ItemKind};
use markise_theme::Theme;

use crate::images::TextureStore;
use crate::source::SourceState;
use crate::text_paint::ParleyText;
use crate::viewer::DocViewer;

/// Width of the divider's drag handle.
pub const DIVIDER_W: f32 = 6.0;
/// Divider position before the user moves it (fraction of the width for the source pane).
pub const DEFAULT_SPLIT: f32 = 0.5;

/// Line starts of the source buffer, convertible to document offsets.
#[derive(Clone, Debug)]
pub struct LineIndex {
    /// Buffer offsets of line starts (first is 0).
    starts: Vec<usize>,
    /// The buffer shows `\n` for a uniformly CRLF document: document offset of a line start
    /// = buffer offset + line number.
    crlf: bool,
}

impl LineIndex {
    pub fn new(buffer: &str, crlf: bool) -> Self {
        let mut starts = Vec::with_capacity(buffer.len() / 40 + 1);
        starts.push(0);
        starts.extend(
            buffer
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        LineIndex { starts, crlf }
    }

    /// Number of lines (a trailing line break opens an empty last line).
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Document offset of the start of line `line` (clamped to the last line).
    pub fn doc_start(&self, line: usize) -> usize {
        let l = line.min(self.starts.len() - 1);
        self.starts[l] + if self.crlf { l } else { 0 }
    }

    /// Line containing document offset `pos`.
    pub fn line_of_doc(&self, pos: usize) -> usize {
        // doc starts are strictly increasing: binary search on them
        let (mut lo, mut hi) = (0usize, self.starts.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.doc_start(mid) <= pos {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.saturating_sub(1)
    }

    /// Line containing buffer offset `pos`.
    pub fn line_of_buffer(&self, pos: usize) -> usize {
        self.starts.partition_point(|&s| s <= pos).saturating_sub(1)
    }

    /// Buffer offset → document offset.
    pub fn buffer_to_doc(&self, pos: usize) -> usize {
        if self.crlf { pos + self.line_of_buffer(pos) } else { pos }
    }
}

/// Absolute source range of item `i`: its own pieces for one segment of a split block, the
/// leaf's span otherwise. Table row segments are approximated by their share of the rows.
pub fn item_source_range(model: &DocModel, i: usize) -> Range<usize> {
    let Some(item) = model.items.get(i) else {
        return 0..0;
    };
    let start = model.item_start(i);
    let end = start + item.len;
    match &item.segment {
        Some(Segment::Lines { spans, .. }) if !spans.is_empty() => {
            start + spans[0].start..start + spans[spans.len() - 1].end
        }
        Some(Segment::Rows { start: r0, end: r1, .. }) => {
            // total rows: the end of the block's last segment (a few segments follow at most)
            let mut total = *r1;
            let mut j = i + 1;
            while let Some(next) = model.items.get(j)
                && model.item_start(j) == start
                && let Some(Segment::Rows { end, .. }) = &next.segment
            {
                total = *end;
                j += 1;
            }
            let len = item.len as f64;
            let frac = |r: u32| (len * r as f64 / total.max(1) as f64) as usize;
            let a = if *r0 == 0 { 0 } else { frac(*r0) };
            start + a..start + frac(*r1).max(a)
        }
        _ => start..end,
    }
}

/// Where a source position lands in the rendered document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// Item index and the fraction of its height.
    Item(usize, f32),
    /// Past the last item (trailing source without a rendered counterpart).
    End,
}

/// Rendered target for the source line `top` (fractional: `3.5` = halfway through line 3).
/// Inside an item the position is interpolated by the line's place in the item's source;
/// hidden source (definitions, blank lines, front matter not shown) maps to the next item.
pub fn source_to_rendered(model: &DocModel, lines: &LineIndex, top: f32) -> Option<Target> {
    let n = model.items.len();
    if n == 0 || lines.is_empty() {
        return None;
    }
    let top = top.max(0.0);
    let line = top.floor() as usize;
    let pos = lines.doc_start(line);
    let mut i = model.item_at_offset(pos);
    // hidden items (definitions, folded front matter) have no rendered counterpart
    let mut hidden = false;
    while model.items.get(i).is_some_and(|it| matches!(it.kind, ItemKind::Hidden)) {
        hidden = true;
        if i + 1 >= n {
            return Some(Target::End);
        }
        i += 1;
    }
    let r = item_source_range(model, i);
    if hidden || pos < r.start {
        return Some(Target::Item(i, 0.0));
    }
    if pos >= r.end && i + 1 >= n && pos > r.start {
        return Some(Target::End);
    }
    let l0 = lines.line_of_doc(r.start) as f32;
    let l1 = (lines.line_of_doc(r.end.saturating_sub(1).max(r.start)) + 1) as f32;
    let t = ((top - l0) / (l1 - l0).max(1.0)).clamp(0.0, 1.0);
    Some(Target::Item(i, t))
}

/// Source line (fractional) for the rendered position `frac` (0..1) of item `item`.
pub fn rendered_to_source(model: &DocModel, lines: &LineIndex, item: usize, frac: f32) -> f32 {
    if model.items.is_empty() || lines.is_empty() {
        return 0.0;
    }
    let r = item_source_range(model, item.min(model.items.len() - 1));
    let l0 = lines.line_of_doc(r.start) as f32;
    let l1 = (lines.line_of_doc(r.end.saturating_sub(1).max(r.start)) + 1) as f32;
    l0 + frac.clamp(0.0, 1.0) * (l1 - l0).max(1.0)
}

/// Length of the common prefix of `a` and `b` (bytes; memcmp over blocks, then bytes).
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const B: usize = 256;
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + B <= n && a[i..i + B] == b[i..i + B] {
        i += B;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

/// Length of the common suffix of `a` and `b`, at most `max`.
fn common_suffix(a: &[u8], b: &[u8], max: usize) -> usize {
    const B: usize = 256;
    let n = a.len().min(b.len()).min(max);
    let (la, lb) = (a.len(), b.len());
    let mut i = 0;
    while i + B <= n && a[la - i - B..la - i] == b[lb - i - B..lb - i] {
        i += B;
    }
    while i < n && a[la - i - 1] == b[lb - i - 1] {
        i += 1;
    }
    i
}

/// Patches turning the document text `old` into `new` (the editor text in document form).
///
/// Every patch is **verbatim**: the editor text is the source of truth byte for byte, so the
/// document always equals it after an apply (a uniformly CRLF document's `\r\n` is restored
/// by [`crate::source::to_doc_text`] before diffing, which is where typed line breaks take the
/// document's line ending, C23). Unchanged regions — including line terminators of untouched
/// lines between two edits of one frame (grouped undo, multi-edit) — are never part of a
/// replacement: a cheap common prefix/suffix narrows the change, then the line/char diff of
/// `markise_engine::edit` splits what remains into its changed hunks.
fn buffer_patches(old: &str, new: &str) -> Result<PatchSet, String> {
    let mut p = common_prefix(old.as_bytes(), new.as_bytes());
    while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
        p -= 1;
    }
    let mut s = common_suffix(old.as_bytes(), new.as_bytes(), old.len().min(new.len()) - p);
    while !old.is_char_boundary(old.len() - s) || !new.is_char_boundary(new.len() - s) {
        s -= 1;
    }
    let (o, n) = (&old[p..old.len() - s], &new[p..new.len() - s]);
    if o.is_empty() && n.is_empty() {
        return Ok(PatchSet::empty());
    }
    let ps = if o.is_empty() || n.is_empty() {
        // a pure insertion or deletion: one hunk
        vec![Patch::verbatim(p..p + o.len(), n)]
    } else {
        markise_engine::edit::patches(o, n)
            .into_iter()
            .map(|h| Patch::verbatim(p + h.start..p + h.end, h.insert))
            .collect()
    };
    PatchSet::new(ps).map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
thread_local! {
    static FAIL_FOR_TESTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests: make incremental updates on this thread fail (as a parser bug would).
#[cfg(test)]
pub(crate) fn fail_incremental_updates_for_tests(on: bool) {
    FAIL_FOR_TESTS.with(|f| f.set(on));
}

/// Applies the editor buffer to the viewer's document through the incremental path. Returns
/// `Ok(false)` when the document already has the buffer's text.
pub fn apply_buffer(buffer: &SourceState, viewer: &mut DocViewer) -> Result<bool, String> {
    apply_text(&buffer.buffer, buffer.is_crlf(), viewer)
}

/// [`apply_buffer`] for an editor text `buffer` (`crlf`: see [`SourceState::is_crlf`]).
pub fn apply_text(buffer: &str, crlf: bool, viewer: &mut DocViewer) -> Result<bool, String> {
    let converted;
    let new: &str = if crlf {
        converted = crate::source::to_doc_text(buffer, true);
        &converted
    } else {
        buffer
    };
    let set = buffer_patches(viewer.doc().text(), new)?;
    if set.is_empty() {
        return Ok(false);
    }
    // the edit goes through the editor session (one joinable "Source Edit" per segment
    // between saves, ADR 0006 §4.3) and updates the view model incrementally; a parser bug
    // must not take the window down (the background rebuild takes over)
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        if FAIL_FOR_TESTS.with(|f| f.get()) {
            panic!("injected incremental-update failure (test)");
        }
        viewer.ed.edit(|s| s.apply_external(&set, "Source Edit", true))
    }));
    match r {
        Ok(Ok(())) => Ok(true),
        Ok(Err(e)) => Err(format!("the incremental update failed: {e}")),
        Err(_) => Err("the incremental update failed".into()),
    }
}

/// Character index → byte offset in `s` (clamped to the end).
fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

/// What happened in the Side by Side panes this frame.
#[derive(Debug, Default)]
pub struct SbsOutput {
    /// The buffer was applied to the document: `Some(Ok(true))` changed it,
    /// `Some(Err(_))` failed (the preview stays behind).
    pub applied: Option<Result<bool, String>>,
    /// The buffer was edited this frame (in addition to any catch-up).
    pub typed: bool,
    /// A link was clicked in the rendered pane.
    pub link: Option<String>,
    /// The divider was dragged to this position.
    pub split: Option<f32>,
    /// The rendered pane was scrolled to follow the source pane this frame.
    pub synced_rendered: bool,
    /// The source pane will follow the rendered pane on the next frame.
    pub synced_source: bool,
    /// Time spent applying the edit (patch + parse + model + view), ms.
    pub apply_ms: Option<f64>,
    /// The rendered pane's rectangle (the formatting toolbar floats over it).
    pub right: Option<Rect>,
}

/// Per-document Side by Side state (the divider position lives in the settings).
#[derive(Default)]
pub struct SideBySide {
    /// `(doc id, text revision)` the line index was built for.
    lines_key: Option<(u64, u64)>,
    lines: Option<LineIndex>,
    last_caret: Option<usize>,
    /// Scroll the source pane to the rendered pane on the next frame (entering the mode,
    /// outline clicks).
    pub sync_source: bool,
    /// Scroll the rendered pane to the source pane on the next frame.
    pub sync_rendered: bool,
}

/// Inputs of [`SideBySide::show`] besides the panes.
pub struct Frame<'a> {
    pub split: f32,
    pub doc_id: u64,
    pub text_rev: u64,
    /// The model is behind the buffer (apply even without a keystroke this frame).
    pub behind: bool,
    /// The incremental path may run (it has not failed for the current text).
    pub incremental: bool,
    pub images: &'a mut TextureStore,
    pub theme: &'a Theme,
    pub parley: Option<&'a mut ParleyText>,
    /// Colours the source pane (only below [`crate::source_highlight::highlight_limit`]).
    pub highlighter: Option<&'a mut crate::source_highlight::SourceHighlighter>,
    /// Height kept free above the rendered pane's document (formatting toolbar).
    pub top_reserve: f32,
    /// Height kept free below the rendered pane's document (formatting toolbar at the bottom).
    pub bottom_reserve: f32,
}

impl SideBySide {
    fn lines(&mut self, buffer: &SourceState, key: (u64, u64)) -> &LineIndex {
        if self.lines_key != Some(key) || self.lines.is_none() {
            self.lines = Some(LineIndex::new(&buffer.buffer, buffer.is_crlf()));
            self.lines_key = Some(key);
        }
        self.lines.as_ref().expect("line index")
    }

    /// Scrolls the rendered pane so the source pane's top line is at its top.
    fn follow_source(&mut self, buffer: &SourceState, viewer: &mut DocViewer, key: (u64, u64)) -> bool {
        let top = buffer.line_at_y(buffer.offset);
        let model = viewer.model().clone();
        let lines = self.lines(buffer, key);
        match source_to_rendered(&model, lines, top) {
            Some(Target::Item(i, t)) => {
                viewer.ed.view.scroll_to_item(i);
                let h = viewer.ed.view.height(i);
                viewer.ed.view.scroll_by(t * h);
                true
            }
            Some(Target::End) => {
                viewer.ed.view.scroll_to_y(f64::MAX);
                true
            }
            None => false,
        }
    }

    /// Requests a source-pane scroll matching the rendered pane's top item.
    fn follow_rendered(&mut self, buffer: &mut SourceState, viewer: &DocViewer, key: (u64, u64)) {
        let (item, off) = viewer.ed.view.anchor();
        let h = viewer.ed.view.height(item).max(1.0);
        let model = viewer.model().clone();
        let line = rendered_to_source(&model, self.lines(buffer, key), item, off / h);
        let y = buffer.y_of_line(line);
        buffer.scroll_to_px(y);
    }

    /// Keeps the caret's block visible in the rendered pane (scrolls only when it is not).
    fn reveal_caret(&mut self, buffer: &SourceState, viewer: &mut DocViewer, vh: f32, key: (u64, u64)) -> bool {
        let Some(ci) = buffer.caret else { return false };
        let byte = char_to_byte(&buffer.buffer, ci);
        let src_off = buffer.offset;
        let lines = self.lines(buffer, key);
        let pos = lines.buffer_to_doc(byte);
        let line = lines.line_of_buffer(byte) as f32;
        let line_y = buffer.y_of_line(line) - src_off;
        let model = viewer.model();
        if model.items.is_empty() {
            return false;
        }
        let item = model.item_at_offset(pos);
        let (a, aoff) = viewer.ed.view.anchor();
        let visible = if item < a {
            false
        } else {
            let mut y = -aoff;
            let mut i = a;
            while i < item && y < vh {
                y += viewer.ed.view.height(i);
                i += 1;
            }
            y < vh && y + viewer.ed.view.height(item) > 0.0
        };
        if visible {
            return false;
        }
        // put the block where the caret line is in the source pane
        viewer.ed.view.scroll_to_item(item);
        viewer.ed.view.scroll_by(-line_y.clamp(0.0, vh * 0.8));
        true
    }

    /// Lays out and runs both panes in the space left in `ui`.
    pub fn show(&mut self, ui: &mut Ui, buffer: &mut SourceState, viewer: &mut DocViewer, f: Frame<'_>) -> SbsOutput {
        let mut out = SbsOutput::default();
        let rect = ui.available_rect_before_wrap();
        let w = rect.width().max(2.0 * DIVIDER_W + 2.0);
        let left_w = ((w - DIVIDER_W) * f.split).round();
        let left = Rect::from_min_max(rect.min, pos2(rect.left() + left_w, rect.bottom()));
        let div = Rect::from_min_max(
            pos2(left.right(), rect.top()),
            pos2(left.right() + DIVIDER_W, rect.bottom()),
        );
        let pane = Rect::from_min_max(pos2(div.right(), rect.top()), rect.max);
        // the strip kept free for the formatting toolbar (the document starts below it)
        let room = (pane.height() - 40.0).max(0.0);
        let reserve = f.top_reserve.clamp(0.0, room);
        let below = f.bottom_reserve.clamp(0.0, room - reserve);
        let right = Rect::from_min_max(
            pos2(pane.left(), pane.top() + reserve),
            pos2(pane.right(), pane.bottom() - below),
        );
        let key = (f.doc_id, f.text_rev);

        // ---- divider
        let id = ui.scope_id().with("markise-sbs-divider");
        let d = ui.interact(div, id, Sense::drag());
        if d.hovered() || d.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if d.dragged()
            && let Some(p) = d.interact_pointer_pos()
        {
            let (lo, hi) = markise_engine::settings::SPLIT_RANGE;
            out.split = Some(((p.x - rect.left() - DIVIDER_W * 0.5) / (w - DIVIDER_W)).clamp(lo, hi));
        }
        let line_c = color(f.theme.meta_text).gamma_multiply(if d.hovered() || d.dragged() { 0.6 } else { 0.25 });
        ui.painter()
            .vline(div.center().x, div.y_range(), egui::Stroke::new(1.0, line_c));

        // ---- left: the source editor
        let programmatic = buffer.scroll_pending();
        let mut layouter_applied: Option<Result<bool, String>> = None;
        let mut layouter_ms = 0.0;
        let (off0, caret0) = (buffer.offset, buffer.caret);
        {
            let mut lui = ui.new_child(
                UiBuilder::new()
                    .max_rect(left.shrink2(egui::vec2(4.0, 0.0)))
                    .id_salt("markise-sbs-left"),
            );
            if buffer.readonly {
                lui.weak("Large file: source is read-only in this version.");
            }
            let crlf = buffer.is_crlf();
            match f.highlighter {
                Some(hl) if !buffer.readonly && buffer.buffer.len() <= crate::source_highlight::highlight_limit() => {
                    let font = egui::FontId::monospace(crate::source::FONT_SIZE);
                    let theme = f.theme;
                    let incremental = f.incremental;
                    let (applied, ms) = (&mut layouter_applied, &mut layouter_ms);
                    let vw = &mut *viewer;
                    // the layouter sees the edited text first: bring the document up to it, so
                    // the colours come from the parse of exactly this text
                    let mut lay = |ui: &Ui, tb: &dyn egui::TextBuffer, wrap: f32| -> Arc<egui::Galley> {
                        let text = tb.as_str();
                        if incremental && !matches!(applied, Some(Err(_))) {
                            let t0 = Instant::now();
                            match apply_text(text, crlf, vw) {
                                Ok(true) => {
                                    *applied = Some(Ok(true));
                                    *ms += t0.elapsed().as_secs_f64() * 1e3;
                                }
                                Ok(false) => {}
                                Err(e) => *applied = Some(Err(e)),
                            }
                        }
                        // checked on the text being laid out: a paste can push it over the limit
                        let job = if text.len() <= crate::source_highlight::highlight_limit() {
                            hl.job(
                                vw.doc(),
                                vw.ed.session.revision().counter,
                                text,
                                crlf,
                                theme,
                                &font,
                                wrap,
                            )
                        } else {
                            crate::source_highlight::plain_job(ui, text, &font, wrap)
                        };
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    buffer.show_with(&mut lui, Some(&mut lay));
                }
                _ => buffer.show_with(&mut lui, None),
            }
        }
        out.typed = buffer.dirty;
        let user_scrolled_source =
            !programmatic && (buffer.offset - off0).abs() > 0.5 && !buffer.dirty && buffer.caret == caret0;

        // ---- edits: patch the document incrementally before the right pane paints
        if let Some(r) = layouter_applied {
            out.apply_ms = Some(layouter_ms);
            out.applied = Some(r);
        }
        // an edit is a new revision: the incremental path is tried (again) for it
        let dirty = std::mem::take(&mut buffer.dirty);
        if (dirty || f.behind) && out.applied.is_none() {
            let t0 = Instant::now();
            let r = apply_buffer(buffer, viewer);
            if r != Ok(false) {
                out.apply_ms = Some(t0.elapsed().as_secs_f64() * 1e3);
                out.applied = Some(r);
            }
        }
        // a new text revision follows any edit: the line index is rebuilt on demand
        let key = if out.typed { (key.0, key.1 + 1) } else { key };

        // ---- scroll sync source → rendered / caret visibility
        let caret_moved = buffer.caret != self.last_caret;
        self.last_caret = buffer.caret;
        if user_scrolled_source || self.sync_rendered {
            self.sync_rendered = false;
            out.synced_rendered = self.follow_source(buffer, viewer, key);
        } else if caret_moved || out.typed {
            out.synced_rendered = self.reveal_caret(buffer, viewer, right.height(), key);
        }

        // ---- right: the rendered document (read-only in this version)
        let user_scrolls_rendered = ui.input(|i| {
            let over = i.pointer.hover_pos().is_some_and(|p| right.contains(p));
            let keys = [
                egui::Key::PageDown,
                egui::Key::PageUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowUp,
                egui::Key::Home,
                egui::Key::End,
                egui::Key::Space,
            ];
            (over && (i.smooth_scroll_delta.y != 0.0 || i.pointer.primary_down()))
                || keys.iter().any(|k| i.key_pressed(*k))
        }) && (ui.input(|i| i.pointer.hover_pos().is_some_and(|p| right.contains(p)))
            || ui.memory(|m| m.focused().is_none()));
        {
            let mut rui = ui.new_child(UiBuilder::new().max_rect(right).id_salt("markise-sbs-right"));
            // interim: the rendered pane is read-only (caret and selection only)
            let vo = viewer.show(&mut rui, f.images, f.theme, f.parley, false);
            out.link = vo.link;
            out.right = Some(pane);
        }
        if user_scrolls_rendered || self.sync_source {
            self.sync_source = false;
            self.follow_rendered(buffer, viewer, key);
            out.synced_source = true;
            ui.ctx().request_repaint();
        }
        ui.allocate_rect(rect, Sense::hover());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str) -> (markise_md::Document, DocModel) {
        let d = markise_md::parse(src);
        let m = DocModel::build_owned(&d, None);
        (d, m)
    }

    #[test]
    fn line_index_maps_crlf_buffers_to_document_offsets() {
        // the buffer of a uniformly CRLF document shows `\n`
        let li = LineIndex::new("a\nbb\nccc\n", true);
        assert_eq!(li.len(), 4);
        assert_eq!((li.doc_start(0), li.doc_start(1), li.doc_start(2)), (0, 3, 7));
        assert_eq!(li.line_of_doc(4), 1);
        assert_eq!(li.line_of_doc(7), 2);
        assert_eq!(li.buffer_to_doc(3), 4);
        let lf = LineIndex::new("a\nbb\n", false);
        assert_eq!((lf.doc_start(1), lf.line_of_doc(2), lf.buffer_to_doc(3)), (2, 1, 3));
    }

    #[test]
    fn source_lines_map_to_their_blocks_and_back() {
        // spec 04 §3.3 scroll sync: each block's first line maps to its top, and back
        let src = "# Title\n\npara one\nstill one\n\n- a\n- b\n\nend\n";
        let (_d, m) = model(src);
        let li = LineIndex::new(src, false);
        let title = m.item_at_offset(0);
        assert_eq!(source_to_rendered(&m, &li, 0.0), Some(Target::Item(title, 0.0)));
        // line 3 = second line of the paragraph: halfway into it
        let para = m.item_at_offset(src.find("para").unwrap());
        assert_eq!(source_to_rendered(&m, &li, 3.0), Some(Target::Item(para, 0.5)));
        assert_eq!(rendered_to_source(&m, &li, para, 0.0), 2.0);
        assert_eq!(rendered_to_source(&m, &li, para, 0.5), 3.0);
        let end = m.item_at_offset(src.find("end").unwrap());
        assert_eq!(rendered_to_source(&m, &li, end, 0.0), 8.0);
    }

    #[test]
    fn hidden_source_maps_to_the_next_block_or_the_end() {
        // spec 04 §3.3: link definitions have no rendered counterpart
        let src = "first\n\n[x]: http://example.com\n\nsecond\n\n[y]: http://example.com\n";
        let (_d, m) = model(src);
        let li = LineIndex::new(src, false);
        let second = m.item_at_offset(src.find("second").unwrap());
        // the blank line and the definition before `second` map to its top
        assert_eq!(source_to_rendered(&m, &li, 1.0), Some(Target::Item(second, 0.0)));
        assert_eq!(source_to_rendered(&m, &li, 2.0), Some(Target::Item(second, 0.0)));
        // the trailing definition: end of the rendered document
        assert_eq!(source_to_rendered(&m, &li, 6.0), Some(Target::End));
    }

    #[test]
    fn segments_of_a_long_code_block_map_to_their_own_lines() {
        let mut src = String::from("```\n");
        for i in 0..3000 {
            src.push_str(&format!("line {i}\n"));
        }
        src.push_str("```\n");
        let (_d, m) = model(&src);
        assert!(m.items.len() > 1, "the block is segmented");
        let li = LineIndex::new(&src, false);
        // a line deep inside the block maps to a later segment, interpolated
        let Some(Target::Item(i, t)) = source_to_rendered(&m, &li, 2000.0) else {
            panic!("no target")
        };
        assert!(i > 0, "a later segment");
        let back = rendered_to_source(&m, &li, i, t);
        assert!((back - 2000.0).abs() < 1.0, "round trip: {back}");
    }

    #[test]
    fn buffer_edits_reach_the_viewer_incrementally() {
        let src = "# Title\n\nalpha\n\nbeta\n";
        let d = Arc::new(markise_md::parse(src));
        let m = Arc::new(DocModel::build_owned(&*d, None));
        let mut v = DocViewer::from_markdown(d.clone(), m, &Theme::light());
        let mut s = SourceState::new(&d, 0);
        s.buffer = s.buffer.replace("beta", "beta gamma");
        assert_eq!(apply_buffer(&s, &mut v), Ok(true));
        assert_eq!(v.doc().text(), "# Title\n\nalpha\n\nbeta gamma\n");
        v.model()
            .diff(&DocModel::build_owned(&**v.doc(), None))
            .expect("incremental == full");
        assert_eq!(apply_buffer(&s, &mut v), Ok(false));
        // a typed line break in a CRLF document becomes CRLF (C23)
        let d = Arc::new(markise_md::parse("a\r\nb\r\n"));
        let m = Arc::new(DocModel::build_owned(&*d, None));
        let mut v = DocViewer::from_markdown(d.clone(), m, &Theme::light());
        let mut s = SourceState::new(&d, 0);
        s.buffer = "a\nnew\nb\n".into();
        assert_eq!(apply_buffer(&s, &mut v), Ok(true));
        assert_eq!(v.doc().text(), "a\r\nnew\r\nb\r\n");
    }

    /// Review P1: two edits of one frame around an untouched LF in a mixed-ending document.
    #[test]
    fn untouched_line_endings_survive_multi_edits_c23() {
        let src = "a\r\nmiddle\nz\r\n";
        let d = Arc::new(markise_md::parse(src));
        let m = Arc::new(DocModel::build_owned(&*d, None));
        let mut v = DocViewer::from_markdown(d.clone(), m, &Theme::light());
        let mut s = SourceState::new(&d, 0);
        assert!(!s.is_crlf(), "mixed: raw buffer");
        s.buffer = "A\r\nmiddle\nZ\r\n".into();
        assert_eq!(apply_buffer(&s, &mut v), Ok(true));
        assert_eq!(v.doc().text(), "A\r\nmiddle\nZ\r\n");
        let ps = buffer_patches(src, "A\r\nmiddle\nZ\r\n").unwrap();
        assert_eq!(ps.patches().len(), 2, "{ps:?}");
    }

    /// Random edit batches (several edits per apply, typed breaks, mixed endings): the
    /// document always equals the editor text and the incremental model equals a full build.
    #[test]
    fn document_always_equals_the_editor_text() {
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut rnd = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        };
        let pieces = [
            "\n", "\r\n", "- ", "# ", "*x*", "word ", "> ", "```\n", "|a|b|\n", "é", "\n\n",
        ];
        for start in [
            "a\r\nmiddle\nz\r\n",
            "one\r\ntwo\r\n\r\nthree\r\n",
            "x\n\n- a\n- b\n",
            "",
        ] {
            let d = Arc::new(markise_md::parse(start));
            let m = Arc::new(DocModel::build_owned(&*d, None));
            let mut v = DocViewer::from_markdown(d.clone(), m, &Theme::light());
            let mut s = SourceState::new(&d, 0);
            for _ in 0..150 {
                for _ in 0..1 + rnd(3) {
                    let len = s.buffer.len();
                    let mut a = rnd(len + 1);
                    while !s.buffer.is_char_boundary(a) {
                        a -= 1;
                    }
                    if rnd(3) == 0 && a < len {
                        let mut b = (a + 1 + rnd(6)).min(len);
                        while !s.buffer.is_char_boundary(b) {
                            b += 1;
                        }
                        s.buffer.replace_range(a..b, "");
                    } else {
                        let piece = pieces[rnd(pieces.len())];
                        // a uniformly CRLF buffer shows `\n` only
                        let piece = if s.is_crlf() { "\n" } else { piece };
                        s.buffer.insert_str(a, piece);
                    }
                }
                apply_buffer(&s, &mut v).unwrap();
                assert_eq!(v.doc().text(), s.doc_text(), "document == editor text");
                assert_eq!(v.doc().text(), s.doc_text());
                v.model()
                    .diff(&DocModel::build_owned(&**v.doc(), None))
                    .expect("incremental == full");
            }
        }
    }

    #[test]
    fn buffer_patches_find_one_small_replacement() {
        let ps = buffer_patches("hello wörld", "hello wörd").unwrap();
        assert_eq!(ps.patches().len(), 1);
        assert_eq!(ps.patches()[0].range, 10..11);
        assert!(buffer_patches("same", "same").unwrap().is_empty());
        let a = "x".repeat(1000);
        assert_eq!(
            buffer_patches(&a, &format!("{a}y")).unwrap().patches()[0].range,
            1000..1000
        );
    }
}
