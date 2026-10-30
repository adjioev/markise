//! Side by Side view mode (spec 04 §3.3).
//!
//! The source editor ([`SourceEditor`], M2d) on the left, the rendered document
//! ([`DocViewer`]) on the right, a draggable divider in between. Both panes show the one editor
//! session: an edit in the source pane is a transaction of the document's single live history
//! and updates the rendered pane incrementally (`FormatDoc::apply` → `DocModel::apply` →
//! `View::apply_change`, plus the source view) within the keystroke. The rendered pane is
//! read-only in this slice (caret and selection; menu commands act on the session when it, not
//! the source pane, has the focus).
//!
//! - **Scroll sync** follows only the pane the user scrolls (no feedback loop): a programmatic
//!   scroll of the other pane is never treated as user scrolling. Source → rendered maps the
//!   topmost visible source line through the model's item spans (interpolated inside an item);
//!   rendered → source maps the topmost visible item back to its source lines. Source without a
//!   rendered counterpart maps to the next item, or to the end of the rendered document.
//! - **Caret moves** and typing in the source pane scroll the rendered pane only when the
//!   caret's block would otherwise be out of view.

use std::ops::Range;

use egui::{CursorIcon, Rect, Sense, Ui, UiBuilder, pos2};
use markise_engine::settings::SourceSettings;
use markise_layout::block::color;
use markise_layout::model::Segment;
use markise_layout::source::SourceView;
use markise_layout::{DocModel, ItemKind};
use markise_theme::Theme;

use crate::images::TextureStore;
use crate::source::SourceEditor;
use crate::text_paint::ParleyText;
use crate::viewer::DocViewer;

/// Width of the divider's drag handle.
pub const DIVIDER_W: f32 = 6.0;
/// Divider position before the user moves it (fraction of the width for the source pane).
pub const DEFAULT_SPLIT: f32 = 0.5;

/// Physical lines of a text ↔ document offsets (scroll mapping).
pub trait LineMap {
    /// Number of lines (a trailing line break opens an empty last line).
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Document offset of the start of line `line` (clamped to the last line).
    fn doc_start(&self, line: usize) -> usize;
    /// Line containing document offset `pos`.
    fn line_of_doc(&self, pos: usize) -> usize;
}

/// Line starts of a text (tests, tools).
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(memchr_lf(text));
        LineIndex { starts }
    }
}

fn memchr_lf(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.bytes()
        .enumerate()
        .filter(|(_, b)| *b == b'\n')
        .map(|(i, _)| i + 1)
}

impl LineMap for LineIndex {
    fn len(&self) -> usize {
        self.starts.len()
    }
    fn doc_start(&self, line: usize) -> usize {
        self.starts[line.min(self.starts.len() - 1)]
    }
    fn line_of_doc(&self, pos: usize) -> usize {
        self.starts.partition_point(|&s| s <= pos).saturating_sub(1)
    }
}

/// The lines of the source view's text (no scan: per-item newline counts).
pub struct ViewLines<'a> {
    pub view: &'a SourceView,
    pub text: &'a str,
}

impl LineMap for ViewLines<'_> {
    fn len(&self) -> usize {
        self.view.line_count()
    }
    fn doc_start(&self, line: usize) -> usize {
        self.view.line_start(self.text, line)
    }
    fn line_of_doc(&self, pos: usize) -> usize {
        self.view.line_of(self.text, pos)
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
pub fn source_to_rendered(model: &DocModel, lines: &dyn LineMap, top: f32) -> Option<Target> {
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
pub fn rendered_to_source(model: &DocModel, lines: &dyn LineMap, item: usize, frac: f32) -> f32 {
    if model.items.is_empty() || lines.is_empty() {
        return 0.0;
    }
    let r = item_source_range(model, item.min(model.items.len() - 1));
    let l0 = lines.line_of_doc(r.start) as f32;
    let l1 = (lines.line_of_doc(r.end.saturating_sub(1).max(r.start)) + 1) as f32;
    l0 + frac.clamp(0.0, 1.0) * (l1 - l0).max(1.0)
}

/// What happened in the Side by Side panes this frame.
#[derive(Debug, Default)]
pub struct SbsOutput {
    /// The source pane edited the document this frame.
    pub typed: bool,
    /// A link was clicked in the rendered pane.
    pub link: Option<String>,
    /// The divider was dragged to this position.
    pub split: Option<f32>,
    /// The rendered pane was scrolled to follow the source pane this frame.
    pub synced_rendered: bool,
    /// The source pane will follow the rendered pane on the next frame.
    pub synced_source: bool,
    /// Keystroke processing time of the source pane (input → laid out and painted), ms.
    pub input_ms: Option<f64>,
    /// The rendered pane's rectangle (the formatting toolbar floats over it).
    pub right: Option<Rect>,
}

/// Per-document Side by Side state (the divider position lives in the settings).
#[derive(Default)]
pub struct SideBySide {
    /// Scroll the source pane to the rendered pane on the next frame (entering the mode,
    /// outline clicks).
    pub sync_source: bool,
    /// Scroll the rendered pane to the source pane on the next frame.
    pub sync_rendered: bool,
    /// PROTOTYPE measurement driver (`MARKISE_PROTO_SCRIPT=1`).
    proto: Option<crate::block_snap::ProtoScript>,
    /// PROTOTYPE block-forms measurement driver (`MARKISE_PROTO_FORMS=1`).
    proto_forms: Option<crate::block_edit::FormScript>,
    proto_checked: bool,
    last_frame_ms: Option<f64>,
}

/// Inputs of [`SideBySide::show`] besides the panes.
pub struct Frame<'a> {
    pub split: f32,
    pub images: &'a mut TextureStore,
    pub theme: &'a Theme,
    pub parley: Option<&'a mut ParleyText>,
    pub prefs: &'a SourceSettings,
    /// Edits are accepted in the source pane.
    pub editable: bool,
    /// Height kept free above the rendered pane's document (formatting toolbar).
    pub top_reserve: f32,
    /// Height kept free below the rendered pane's document (formatting toolbar at the bottom).
    pub bottom_reserve: f32,
}

impl SideBySide {
    /// Scrolls the rendered pane so the source pane's top line is at its top.
    fn follow_source(viewer: &mut DocViewer) -> bool {
        let Some(p) = viewer.ed.source.as_ref() else {
            return false;
        };
        let top = p.view.line_at_y(p.view.scroll_y());
        let model = viewer.model().clone();
        let lines = ViewLines {
            view: &p.view,
            text: viewer.ed.session.text(),
        };
        match source_to_rendered(&model, &lines, top) {
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
    fn follow_rendered(src: &mut SourceEditor, viewer: &DocViewer) {
        let Some(p) = viewer.ed.source.as_ref() else { return };
        let (item, off) = viewer.ed.view.anchor();
        let h = viewer.ed.view.height(item).max(1.0);
        let text = viewer.ed.session.text();
        let lines = ViewLines { view: &p.view, text };
        let line = rendered_to_source(viewer.model(), &lines, item, off / h);
        src.scroll_to_px(p.view.y_of_line(text, line));
    }

    /// Keeps the caret's block visible in the rendered pane (scrolls only when it is not).
    fn reveal_caret(viewer: &mut DocViewer, vh: f32) -> bool {
        let Some(p) = viewer.ed.source.as_ref() else {
            return false;
        };
        let text = viewer.ed.session.text();
        let pos = p.sel.primary().focus.offset.min(text.len());
        let line = p.view.line_of(text, pos) as f32;
        let line_y = (p.view.y_of_line(text, line) - p.view.scroll_y()) as f32;
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
    pub fn show(&mut self, ui: &mut Ui, src: &mut SourceEditor, viewer: &mut DocViewer, f: Frame<'_>) -> SbsOutput {
        let mut out = SbsOutput::default();
        let frame_started = std::time::Instant::now();
        if !self.proto_checked {
            self.proto_checked = true;
            if crate::block_snap::ProtoScript::enabled() {
                self.proto = Some(Default::default());
            }
            if crate::block_edit::FormScript::enabled() {
                self.proto_forms = Some(Default::default());
            }
        }
        if let Some(p) = self.proto_forms.as_mut() {
            p.frame(ui.ctx(), viewer);
        }
        if let Some(p) = self.proto.as_mut() {
            p.frame(ui.ctx(), viewer, self.last_frame_ms.take());
        }
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
        let programmatic = src.scroll_pending();
        let mut parley = f.parley;
        let so = {
            let mut lui = ui.new_child(UiBuilder::new().max_rect(left).id_salt("markise-sbs-left"));
            src.show(&mut lui, viewer, f.theme, parley.as_deref_mut(), f.prefs, f.editable)
        };
        out.typed = so.edited;
        out.input_ms = so.input_ms;

        // ---- scroll sync source → rendered / caret visibility
        if (so.user_scrolled && !programmatic) || self.sync_rendered {
            self.sync_rendered = false;
            out.synced_rendered = Self::follow_source(viewer);
        } else if so.moved {
            out.synced_rendered = Self::reveal_caret(viewer, right.height());
        }

        // ---- right: the rendered document (read-only in this version)
        let user_scrolls_rendered = ui.input(|i| {
            let over = i.pointer.hover_pos().is_some_and(|p| right.contains(p));
            over && (i.smooth_scroll_delta.y != 0.0 || i.pointer.primary_down())
        });
        {
            let mut rui = ui.new_child(UiBuilder::new().max_rect(right).id_salt("markise-sbs-right"));
            viewer.snap_blocks = true;
            let vo = viewer.show(&mut rui, f.images, f.theme, parley, false);
            viewer.snap_blocks = false;
            out.link = vo.link;
            // PROTOTYPE (block snapshots): a click on a block image puts the source caret on
            // the block's `:::` line
            if let Some(off) = vo.goto_source {
                if let Some(p) = viewer.ed.source.as_mut() {
                    p.sel = markise_doc::Selection::caret(off);
                }
                viewer.ed.source_store_selection();
                if let Some(p) = viewer.ed.source.as_ref() {
                    let line = p.view.line_of(viewer.ed.session.text(), off) as f32;
                    src.center_line(line);
                }
                src.request_focus();
                ui.ctx().request_repaint();
            }
            out.right = Some(pane);
        }
        if user_scrolls_rendered || self.sync_source {
            self.sync_source = false;
            Self::follow_rendered(src, viewer);
            out.synced_source = true;
            ui.ctx().request_repaint();
        }
        ui.allocate_rect(rect, Sense::hover());
        self.last_frame_ms = Some(frame_started.elapsed().as_secs_f64() * 1e3);
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
    fn line_index_maps_lines_to_document_offsets() {
        let li = LineIndex::new("a\r\nbb\r\nccc\r\n");
        assert_eq!(li.len(), 4);
        assert_eq!((li.doc_start(0), li.doc_start(1), li.doc_start(2)), (0, 3, 7));
        assert_eq!(li.line_of_doc(4), 1);
        assert_eq!(li.line_of_doc(7), 2);
    }

    #[test]
    fn source_lines_map_to_their_blocks_and_back() {
        // spec 04 §3.3 scroll sync: each block's first line maps to its top, and back
        let src = "# Title\n\npara one\nstill one\n\n- a\n- b\n\nend\n";
        let (_d, m) = model(src);
        let li = LineIndex::new(src);
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
        let li = LineIndex::new(src);
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
        let li = LineIndex::new(&src);
        // a line deep inside the block maps to a later segment, interpolated
        let Some(Target::Item(i, t)) = source_to_rendered(&m, &li, 2000.0) else {
            panic!("no target")
        };
        assert!(i > 0, "a later segment");
        let back = rendered_to_source(&m, &li, i, t);
        assert!((back - 2000.0).abs() < 1.0, "round trip: {back}");
    }

    /// The source view's line map agrees with a plain line index.
    #[test]
    fn view_lines_equal_the_line_index() {
        let src = "a\r\n\r\nbb\nccc\n".repeat(200);
        let v = SourceView::new(&src, Default::default(), &Theme::light(), 400.0, 1.0);
        let vl = ViewLines { view: &v, text: &src };
        let li = LineIndex::new(&src);
        assert_eq!(vl.len(), li.len());
        for l in (0..li.len()).step_by(7) {
            assert_eq!(vl.doc_start(l), li.doc_start(l));
            assert_eq!(vl.line_of_doc(li.doc_start(l) + 1), li.line_of_doc(li.doc_start(l) + 1));
        }
    }
}
