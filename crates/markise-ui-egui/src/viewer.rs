//! The document surface: paints the visible items of the editor's [`View`], handles wheel /
//! keyboard / scrollbar scrolling, link clicks, the front-matter toggle and — when the
//! surface has keyboard focus — live-preview editing (ADR 0006 M2b): a blinking caret,
//! click / drag / shift-click / double- and triple-click selection, keyboard commands routed
//! through the engine keymap and command registry, typing, and marker reveal at the caret.
//!
//! Repaint discipline (NFR-10): without keyboard focus this widget never schedules a repaint;
//! with focus, the caret blinks (a repaint every half second) for [`BLINK_FOR`] seconds after
//! the last input and then stays solid, so an idle window goes quiet again.

use std::sync::Arc;
use std::time::Instant;

use egui::{
    Color32, CornerRadius, CursorIcon, Event, EventFilter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};
use markise_doc::{ContainerKind, DocPos, DocumentFormat, FormatDoc, Params, PatchSet, Selection, planners};
use markise_engine::editor::keymap::{Chord, Keymap, Platform};
use markise_engine::editor::{EditorSession, EditorView, TxKind};
use markise_layout::block::{BlockLayout, Prim, color};
use markise_layout::text::ParleyBlock;
use markise_layout::{DocModel, TextShaper, View};
use markise_theme::Theme;

use crate::images::TextureStore;
use crate::smooth_scroll::{self, SmoothScroll};
use crate::text_paint::ParleyText;

mod card_edit;
mod code_tools;
mod comment_place;
mod comment_presence;
mod scrollbar;
pub use code_tools::{LangEdit, LangItem, ShownTools, lang_matches};

const SCROLLBAR_W: f32 = 10.0;
/// Caret blink half-period (seconds).
const BLINK_HALF: f64 = 0.53;
/// The caret stops blinking (stays solid) this long after the last input (NFR-10).
pub const BLINK_FOR: f64 = 10.0;

/// What happened in the viewer this frame.
#[derive(Default, Debug)]
pub struct ViewerOutput {
    /// A link was opened (Cmd/Ctrl+click; target as written; `#anchor` targets are handled
    /// internally).
    pub link: Option<String>,
    /// Items painted this frame.
    pub painted: usize,
    /// Items laid out (cache misses) this frame.
    pub laid_out: usize,
    /// The document text or selection changed through input this frame.
    pub edited: bool,
    /// Keystroke processing time this frame (input → laid out and painted), when there was
    /// keyboard input (`MARKISE_TIMING`).
    pub input_ms: Option<f64>,
    /// A review command chosen in the document's context menu or the hover bar of a
    /// suggestion (ADR 0012): command id and the source offset it acts at (`None`: at the
    /// selection).
    pub review: Option<(&'static str, Option<usize>)>,
    /// PROTOTYPE (block snapshots): a block image was clicked; source offset of its `:::` line.
    pub goto_source: Option<usize>,
    /// Wheel / trackpad scrolling moved the document this frame (smooth scrolling included).
    pub wheel_scrolled: bool,
}

/// The suggestion under the pointer and its floating Accept / Reject bar (ADR 0012).
#[derive(Clone, Debug)]
struct HoverBar {
    mark: markise_doc::review::ReviewMark,
    /// The document revision the mark was found in: on any other revision the bar is hidden.
    counter: u64,
    /// Where the bar sits (screen, top left), fixed while the same mark is hovered.
    anchor: Pos2,
    /// The bar's rectangle at the last frame (the pointer may move onto it).
    rect: Option<Rect>,
}

/// A review comment shown in the visible part of the document (last frame), screen
/// coordinates and absolute source offsets (ADR 0012).
#[derive(Clone, Debug)]
pub struct ShownComment {
    /// The bubble glyph (revealed: the first row of the raw text).
    pub glyph: Rect,
    /// The whole mark and its text.
    pub source: std::ops::Range<usize>,
    pub content: std::ops::Range<usize>,
    pub text: Arc<str>,
    pub revealed: bool,
    /// Its card (margin card or open popover), when one was drawn.
    pub card: Option<Rect>,
}

/// The bubbles-mode popover (ADR 0012).
#[derive(Clone, Copy, Debug)]
struct Popover {
    /// Source start of the comment it shows.
    key: usize,
    /// When the pointer came onto the glyph (egui time).
    since: f64,
    /// Clicked: stays until Esc or another comment.
    pinned: bool,
}

/// The pointer must rest on a bubble this long before its popover opens.
pub const POPOVER_DELAY: f64 = 0.15;

/// Commands of the document's context menu (ADR 0012), `None`: separator.
pub const CONTEXT_COMMANDS: &[Option<&str>] = &[
    Some("review.addComment"),
    None,
    Some("review.accept"),
    Some("review.reject"),
    Some("review.deleteComment"),
    None,
    Some("review.acceptAll"),
    Some("review.rejectAll"),
];

/// Commands that ask for input in an app dialog before they run (issue #164): their shortcut
/// and context menu item hand them to the app ([`DocViewer::take_app_command`]).
pub const ASKING_COMMANDS: &[&str] = &["table.insert"];

/// Commands that act on the whole document: unavailable while it holds only the first part of
/// its file (progressive open, issue #126; they would leave the rest unchanged).
pub const WHOLE_DOCUMENT_COMMANDS: &[&str] = &["review.acceptAll", "review.rejectAll"];

/// Selection granularity of a pointer drag (single, double, triple click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Block,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    /// The range of the initial click at its unit (word / block); a caret for chars.
    anchor: (DocPos, DocPos),
    unit: Unit,
}

pub(crate) type FindMatches = (u64, Arc<Vec<std::ops::Range<usize>>>, Option<usize>);

pub struct DocViewer {
    pub(crate) find_rects: Vec<Rect>,
    pub(crate) find_matches: Option<FindMatches>,
    pub(crate) find_reveal: bool,
    /// The editing session and its view.
    pub ed: EditorView,
    /// Font generation the view was last told about.
    font_gen: u64,
    keymap: Keymap,
    platform: Platform,
    /// egui time of the last caret activity (blink phase, blink timeout).
    blink_from: f64,
    drag: Option<Drag>,
    /// A press on a task checkbox waiting for its release: its item by source position
    /// (start, ordinal among items starting there: indices shift when a paged model
    /// materializes pages between frames), the list item ordinal and the document's change
    /// count at the press (a press held while the document changed is cancelled).
    task_press: Option<((usize, usize), u16, u64)>,
    /// Last primary press: time, position, click count.
    last_press: Option<(f64, Pos2, u32)>,
    /// Visible items of the last frame (index, top relative to the surface), for hit tests.
    /// (Kept by source position, [`DocViewer::item_key`]: a paged model may materialize pages
    /// between frames, which shifts item indices; [`DocViewer::shown`] resolves them.)
    /// Keys are handled before the pointer: positions are mapped through the edits since the
    /// frame ([`EditorView::map_recent`]; an item whose text an edit changed is not hit).
    last_visible: Vec<((usize, usize), f32)>,
    /// Column left edge of the last frame (screen x).
    last_left: f32,
    /// Paged models: the document height when the scrollbar drag started (the scrollbar's
    /// scale is frozen during a drag while page estimates refine, issue #125).
    drag_total: Option<f64>,
    /// Surface top of the last frame (screen y).
    last_top: f32,
    /// Text to put on the clipboard after this frame's input handling.
    pending_copy: Option<String>,
    /// Widget id and screen rectangle of the surface at the last frame.
    last_surface: Option<(egui::Id, Rect)>,
    /// Take the keyboard focus on the next frame (the surface came back after a view mode
    /// whose widgets held it, e.g. the source editor).
    refocus: bool,
    /// Screen rectangle of the caret at the last frame it was drawn (popups anchor to it).
    pub caret_screen: Option<Rect>,
    /// The hovered suggestion's action bar.
    hover: Option<HoverBar>,
    /// Last suggestion lookup under the pointer: (revision counter, offset, result).
    hover_cache: Option<(u64, usize, Option<markise_doc::review::ReviewMark>)>,
    /// Pointer position and revision counter of the last hover lookup.
    hover_key: Option<(Pos2, u64)>,
    /// How comments are shown (setting `markdown.commentDisplay`).
    comment_display: markise_engine::settings::CommentDisplay,
    /// How review marks are shown (setting `markdown.commentView`, issue #88).
    comment_view: markise_engine::settings::CommentView,
    /// Comments of the visible items at the last frame, with their cards.
    comments: Vec<ShownComment>,
    popover: Option<Popover>,
    /// A message for the app to show (a refused Add Comment shortcut, issue #90).
    notice: Option<String>,
    /// A command the app runs like its menu item (taken once): one that asks first, e.g.
    /// Insert Table… opens its size dialog (issue #164).
    app_command: Option<&'static str>,
    /// The comment whose text is being edited in its card (issue #87).
    card_edit: Option<card_edit::CardEdit>,
    /// The bubble popover editor's size at its last frame (placement).
    card_edit_size: Option<Vec2>,
    /// The surface accepts edits (the last `show`).
    editable: bool,
    /// Debounced "the document has comments" (the comment gutter, issue #91).
    comment_presence: comment_presence::CommentPresence,
    /// The comment gutter was reserved at the last frame.
    gutter: bool,
    /// Copy button and language field of fenced code blocks (issue #76).
    code: code_tools::CodeTools,
    /// A press on the scrollbar (issue #123).
    thumb_drag: scrollbar::ThumbDrag,
    /// Wheel / trackpad smoothing (issue #86).
    smooth: SmoothScroll,
    /// The document holds only the first part of its file (progressive open, issue #126):
    /// [`WHOLE_DOCUMENT_COMMANDS`] are unavailable.
    pub partial: bool,
    /// PROTOTYPE (block snapshots): show `:::goals` / `:::question` as helper snapshots
    /// (set by Side by Side for its rendered pane).
    pub snap_blocks: bool,
    pub snaps: Option<crate::block_snap::BlockSnaps>,
    snaps_tried: bool,
    /// Screen rectangles of the block images at the last frame, with their source offsets.
    snap_rects: Vec<(Rect, usize)>,
    /// PROTOTYPE (native blocks): blocks on screen at the last frame with their controls.
    pub block_ctl: Vec<BlockCtl>,
    /// Natively drawn blocks as units (like `atomic`), their marker lines (caret stops are
    /// skipped), and what editing must not cross: first inner item starts (Backspace), last
    /// inner item ends (Delete) and hidden `{#id}` labels.
    pub native_units: Vec<(usize, usize, usize, usize)>,
    pub native_markers: Vec<(usize, usize, usize, usize)>,
    native_first: Vec<usize>,
    native_last: Vec<usize>,
    native_hidden: Vec<std::ops::Range<usize>>,
    /// PROTOTYPE (block forms): "✎ Edit" was clicked on the block at this offset.
    pub pending_edit: Option<usize>,
    /// PROTOTYPE (block forms): show the Edit button on this block ordinal without hover
    /// (screenshots of the measurement script).
    pub force_edit_button: Option<usize>,
    /// PROTOTYPE (live mode): block images of the last frame as atomic caret units:
    /// (block start, block end, caret stop before, caret stop after).
    pub atomic: Vec<(usize, usize, usize, usize)>,
    /// PROTOTYPE (live mode): the block (start offset) selected by a click on its image.
    pub focused_block: Option<usize>,
    /// PROTOTYPE (live mode): caret focus offset at the end of the last frame.
    last_focus: usize,
    /// PROTOTYPE (block packages): the document's block library (resolved once).
    lib: Option<crate::block_lib::Lib>,
    /// PROTOTYPE (block packages): valid insertion points of the last frame (offset, screen y).
    pub gaps: Vec<(usize, f32)>,
    /// Column left edge and width of the last frame (insertion affordances).
    gap_col: (f32, f32),
    /// PROTOTYPE (live preview): natural layout heights of the last frame's visible items (by
    /// source start of the item).
    natural: std::collections::HashMap<usize, f32>,
    /// PROTOTYPE (live preview): new-block preview this frame: (item's source start, y offset in it, texture,
    /// size, replaces the item).
    draft_paint: Option<(usize, f32, egui::TextureId, Vec2, bool)>,
    /// PROTOTYPE (block move): scripted drag position (y in the pane) instead of the pointer.
    pub debug_drag_y: Option<f32>,
}

/// PROTOTYPE (block move): drag payload — the block starting at this offset.
#[derive(Clone, Copy, Debug)]
pub struct MoveBlock(pub usize);

/// PROTOTYPE (block move): the ⠿ handle left of a block image.
fn handle_rect(r: Rect) -> Rect {
    Rect::from_min_size(pos2(r.left() - 24.0, r.top() + 6.0), vec2(18.0, 24.0))
}

/// PROTOTYPE (block packages): the "+ Добавить блок" pill at an insertion point.
fn plus_rect(left: f32, y: f32) -> Rect {
    Rect::from_min_size(pos2(left - 2.0, y - 11.0), vec2(136.0, 22.0))
}

/// PROTOTYPE: block images in the normal (live) editing mode (`MARKISE_LIVE_SNAPS=0` turns
/// them off).
fn live_snaps() -> bool {
    std::env::var_os("MARKISE_LIVE_SNAPS").is_none_or(|v| v != "0")
}

/// PROTOTYPE (block forms): the "✎ Edit" button of a block.
fn edit_button_rect(r: Rect) -> Rect {
    Rect::from_min_size(pos2(r.right() - 166.0, r.top() + 6.0), vec2(78.0, 22.0))
}

/// PROTOTYPE (native blocks): the view switch of a block (always in the same place: a block's
/// top stays put while it is rotated, so it can be clicked repeatedly).
fn view_button_rect(r: Rect) -> Rect {
    Rect::from_min_size(pos2(r.right() - 84.0, r.top() + 6.0), vec2(78.0, 22.0))
}

/// PROTOTYPE (native blocks): a block on screen with its controls.
#[derive(Clone, Copy, Debug)]
pub struct BlockCtl {
    pub rect: Rect,
    pub offset: usize,
    pub view: crate::block_snap::BlockView,
}

/// PROTOTYPE (native blocks): the accents a package's `view` can name (near the site's tokens).
fn accent_color(name: &str, dark: bool) -> Color32 {
    let (l, d) = match name {
        "blue" => ((37, 99, 235), (124, 176, 255)),
        "purple" => ((93, 63, 186), (179, 157, 255)),
        "green" => ((35, 107, 81), (111, 208, 168)),
        "amber" => ((183, 121, 31), (232, 180, 90)),
        "red" => ((166, 56, 69), (240, 138, 151)),
        _ => ((82, 101, 124), (167, 182, 201)),
    };
    let (r, g, b) = if dark { d } else { l };
    Color32::from_rgb(r, g, b)
}

/// PROTOTYPE (native blocks): the small icons a package's `view` can name.
fn paint_view_icon(p: &egui::Painter, c: Pos2, icon: &str, col: Color32) {
    let s = Stroke::new(1.5, col);
    match icon {
        "lightbulb" => {
            p.circle_stroke(c + vec2(0.0, -1.5), 4.5, s);
            p.line_segment([c + vec2(-2.0, 5.0), c + vec2(2.0, 5.0)], s);
        }
        "flag" => {
            p.line_segment([c + vec2(-4.0, -6.0), c + vec2(-4.0, 6.0)], s);
            p.add(egui::Shape::convex_polygon(
                vec![c + vec2(-4.0, -6.0), c + vec2(5.0, -3.0), c + vec2(-4.0, 0.5)],
                col,
                Stroke::NONE,
            ));
        }
        "image" => {
            p.rect_stroke(Rect::from_center_size(c, vec2(13.0, 10.0)), 1.5, s, StrokeKind::Middle);
            p.line_segment([c + vec2(-5.0, 3.5), c + vec2(-1.0, -1.0)], Stroke::new(1.0, col));
            p.line_segment([c + vec2(-1.0, -1.0), c + vec2(3.0, 3.5)], Stroke::new(1.0, col));
        }
        "question" => {
            p.circle_stroke(c, 6.0, s);
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional(10.0),
                col,
            );
        }
        _ => {
            // a sheet with lines
            p.rect_stroke(Rect::from_center_size(c, vec2(9.0, 12.0)), 1.5, s, StrokeKind::Middle);
            p.line_segment([c + vec2(-2.0, -2.0), c + vec2(2.0, -2.0)], Stroke::new(1.0, col));
            p.line_segment([c + vec2(-2.0, 1.5), c + vec2(2.0, 1.5)], Stroke::new(1.0, col));
        }
    }
}

/// PROTOTYPE (native blocks): where the content of the item at `start` begins, after its
/// Markdown syntax (`### `, `> `, `- [ ] `, `1. `).
fn content_start(text: &str, start: usize) -> usize {
    let rest = text.get(start..).unwrap_or("");
    let line = rest.split('\n').next().unwrap_or("");
    let mut i = 0;
    let b = line.as_bytes();
    let hashes = b.iter().take_while(|c| **c == b'#').count();
    if hashes > 0 && b.get(hashes) == Some(&b' ') {
        i = hashes + 1;
    } else if b.first() == Some(&b'>') {
        i = 1 + usize::from(b.get(1) == Some(&b' '));
    } else if matches!(b.first(), Some(b'-' | b'*' | b'+')) && b.get(1) == Some(&b' ') {
        i = 2;
        if line[2..].starts_with("[ ] ") || line[2..].starts_with("[x] ") || line[2..].starts_with("[X] ") {
            i = 6;
        }
    } else {
        let d = b.iter().take_while(|c| c.is_ascii_digit()).count();
        if d > 0 && matches!(b.get(d), Some(b'.' | b')')) && b.get(d + 1) == Some(&b' ') {
            i = d + 2;
        }
    }
    start + i
}

impl DocViewer {
    pub fn new(
        format: Arc<dyn DocumentFormat>,
        doc: Arc<dyn FormatDoc>,
        model: Arc<DocModel>,
        theme: &Theme,
        path: Option<std::path::PathBuf>,
    ) -> Self {
        let session = EditorSession::from_doc(
            format,
            doc.clone(),
            Default::default(),
            Box::new(markise_engine::editor::SystemClock),
        );
        let view = View::new(doc, model, theme.clone(), 600.0, 1.0);
        let ed = EditorView::new(session, view, path);
        let platform = Platform::current();
        let keymap = Keymap::new(&ed.registry, platform);
        DocViewer {
            ed,
            font_gen: 0,
            keymap,
            platform,
            blink_from: 0.0,
            drag: None,
            task_press: None,
            last_press: None,
            last_visible: Vec::new(),
            find_rects: Vec::new(),
            find_matches: None,
            find_reveal: false,
            last_left: 0.0,
            drag_total: None,
            last_top: 0.0,
            pending_copy: None,
            last_surface: None,
            refocus: false,
            caret_screen: None,
            hover: None,
            hover_cache: None,
            hover_key: None,
            comment_display: Default::default(),
            comments: Vec::new(),
            popover: None,
            card_edit: None,
            notice: None,
            app_command: None,
            comment_view: Default::default(),
            card_edit_size: None,
            editable: false,
            comment_presence: Default::default(),
            gutter: false,
            code: Default::default(),
            snap_blocks: false,
            snaps: None,
            snaps_tried: false,
            snap_rects: Vec::new(),
            block_ctl: Vec::new(),
            native_units: Vec::new(),
            native_markers: Vec::new(),
            native_first: Vec::new(),
            native_last: Vec::new(),
            native_hidden: Vec::new(),
            pending_edit: None,
            force_edit_button: None,
            atomic: Vec::new(),
            focused_block: None,
            last_focus: 0,
            lib: None,
            gaps: Vec::new(),
            gap_col: (0.0, 0.0),
            natural: Default::default(),
            draft_paint: None,
            debug_drag_y: None,
            thumb_drag: Default::default(),
            smooth: Default::default(),
            partial: false,
        }
    }

    /// Search highlight rectangles painted by the last rendered frame (screen coordinates).
    pub fn search_highlights(&self) -> &[Rect] {
        &self.find_rects
    }

    /// A viewer of a parsed Markdown document (tests, tools).
    pub fn from_markdown(doc: Arc<markise_md::Document>, model: Arc<DocModel>, theme: &Theme) -> Self {
        Self::new(Arc::new(markise_md::MarkdownFormat::default()), doc, model, theme, None)
    }

    /// The surface's widget id at the last frame (it holds the keyboard focus while editing).
    pub fn surface_id(&self) -> Option<egui::Id> {
        self.last_surface.map(|(id, _)| id)
    }

    /// Asks the surface to take the keyboard focus on its next frame (after leaving Source or
    /// Side by Side, whose editor had it), so typing continues without a click.
    pub fn request_refocus(&mut self) {
        self.refocus = true;
    }

    /// The surface's screen rectangle at the last frame (document painting is clipped to it).
    pub fn surface_rect(&self) -> Option<Rect> {
        self.last_surface.map(|(_, r)| r)
    }

    /// The shared view (layout, scrolling).
    pub fn view(&mut self) -> &mut View {
        &mut self.ed.view
    }

    pub fn model(&self) -> &Arc<DocModel> {
        self.ed.view.model()
    }

    /// The document shown (the session's current document).
    pub fn doc(&self) -> &Arc<dyn FormatDoc> {
        self.ed.session.doc_arc()
    }

    /// Rebuilds the keymap from the registry defaults plus `keyBinding` overrides (label or
    /// id → accelerator); returns the problems found (unknown names, conflicts).
    pub fn set_key_bindings(&mut self, entries: &[(String, String)]) -> Vec<String> {
        let mut km = Keymap::new(&self.ed.registry, self.platform);
        let problems = km.apply_overrides(&self.ed.registry, entries, self.platform);
        self.keymap = km;
        problems
    }

    /// The keymap in use (menus show its chords).
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Replaces the document and its model without history (a new document).
    pub fn set_document(&mut self, doc: Arc<dyn FormatDoc>, model: Arc<DocModel>) {
        let format = self.ed.session.format().clone();
        let session = EditorSession::from_doc(
            format,
            doc.clone(),
            Default::default(),
            Box::new(markise_engine::editor::SystemClock),
        );
        let path = self.ed.path().map(|p| p.to_path_buf());
        let mut view = std::mem::replace(
            &mut self.ed.view,
            View::new(doc.clone(), model.clone(), Theme::light(), 600.0, 1.0),
        );
        view.set_model(doc, model);
        self.ed = EditorView::new(session, view, path);
        self.code = Default::default();
        // a new document: no card edit of the old one carries over
        self.card_edit = None;
    }

    /// Installs `doc`/`model` as one undoable transaction `label` (reload from disk, a
    /// source-mode edit; `join`: append to the previous transaction of that label).
    pub fn install(
        &mut self,
        doc: Arc<dyn FormatDoc>,
        model: Arc<DocModel>,
        label: &'static str,
        join: bool,
    ) -> Result<(), markise_engine::editor::EditError> {
        self.ed.release();
        // the source pane's selection follows the replacement's diff (its live marks)
        let r = self
            .ed
            .session
            .replace_doc(doc.clone(), label, markise_engine::editor::TxKind::Bulk, join);
        let _ = self.ed.session.take_change();
        let _ = self.ed.session.take_steps();
        self.ed.view.set_model(doc, model);
        let caret = self.ed.session.selection().clone();
        let len = self.ed.session.text().len();
        if caret.primary().end().offset > len {
            self.ed.session.set_selection(Selection::caret(len));
        }
        // the source pane shows the new text; its selection is mapped through the diff and
        // snapped to caret positions
        if let Some(p) = self.ed.source.as_mut() {
            p.view.rebuild(self.ed.session.text());
        }
        self.ed.source_load_selection();
        // a card edit stays only on its own unchanged comment (Codex review of #158)
        self.revalidate_card_edit();
        r
    }

    /// Progressive open (issue #126): installs the whole document of the file whose first
    /// `cut` bytes this viewer shows, carrying the edits made meanwhile over
    /// ([`EditorView::continue_with`]); the content on screen does not move.
    pub fn continue_with(
        &mut self,
        doc: Arc<dyn FormatDoc>,
        model: Arc<DocModel>,
        cut: usize,
    ) -> Result<markise_engine::editor::Continued, String> {
        let r = self.ed.continue_with(doc, model, cut);
        self.revalidate_card_edit();
        r
    }

    /// Font generation the view was last told about.
    pub fn font_generation(&self) -> u64 {
        self.font_gen
    }

    pub fn set_font_generation(&mut self, g: u64) {
        if g != self.font_gen {
            self.font_gen = g;
            self.ed.view.set_font_generation(g);
        }
    }

    /// Scrolls so that heading `anchor` (`#slug`) is at the top.
    /// Screen centre of the first visible link whose target is `target` (last frame's
    /// geometry; tests click it).
    pub fn link_center(&self, target: &str) -> Option<Pos2> {
        for &(idx, y) in &self.shown() {
            let Some(l) = self.ed.view.cache_peek(idx) else {
                continue;
            };
            for hit in l.links.iter().filter(|h| h.target == target) {
                let Some(markise_layout::Prim::Text { pos, text, .. }) = l.prims.get(hit.prim) else {
                    continue;
                };
                if let Some(sp) = text.ink_spans(&hit.range).first() {
                    let x = self.last_left + pos.x + (sp.x0 + sp.x1) * 0.5;
                    let yy = self.last_top + y + pos.y + sp.baseline - sp.ascent * 0.4;
                    return Some(pos2(x, yy));
                }
            }
        }
        None
    }

    /// Scrolls the caret into view on the next frame (benches, like a find match).
    pub fn find_reveal_for_bench(&mut self) {
        self.find_reveal = true;
    }

    pub fn scroll_to_anchor(&mut self, anchor: &str) -> bool {
        let a = anchor.trim_start_matches('#');
        let k = self.model().outline.iter().position(|o| o.anchor == a);
        // a paged model materializes the heading's page first
        if let Some(i) = k.and_then(|k| self.ed.view.ensure_heading(k)) {
            self.ed.view.scroll_to_item(i);
            true
        } else {
            false
        }
    }

    /// Command `id` can run now (registry enablement; [`WHOLE_DOCUMENT_COMMANDS`] not while
    /// the document is partial).
    pub fn command_enabled(&self, id: &str) -> bool {
        !(self.partial && WHOLE_DOCUMENT_COMMANDS.contains(&id))
            && !(self.review_preview() && self.mutates(id))
            && self.ed.registry.enabled(&self.ed.session, id).is_ok()
    }

    /// A command for the app to run like its menu item (shortcut or context menu of a
    /// command that opens a dialog first, [`ASKING_COMMANDS`]); taken once.
    pub fn take_app_command(&mut self) -> Option<&'static str> {
        self.app_command.take()
    }

    /// A message from the last frames for the app to show (taken once).
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    /// After Add Comment ran (shortcut, menu, toolbar, context menu): the new comment opens
    /// in its card (issue #87); refused when it ran (a check enablement does not make, Codex
    /// review of #161), the reason becomes a notice instead of nothing happening.
    fn after_add_comment_result(&mut self, r: &Result<(), markise_engine::editor::command::CommandError>) {
        use markise_engine::editor::command::CommandError;
        match r {
            Ok(()) => self.after_add_comment(),
            Err(CommandError::Plan(markise_doc::PlanError::NotApplicable(why))) => {
                self.notice = Some(format!("Cannot add a comment here: {why}."));
            }
            Err(CommandError::Disabled(_)) => self.notice = Some(self.add_comment_refusal()),
            Err(e) => self.notice = Some(format!("Cannot add a comment here: {e}.")),
        }
    }

    /// Why Add Comment cannot run at the selection (the planner's reason).
    fn add_comment_refusal(&self) -> String {
        if self.review_preview() {
            return "The document is read-only while comments show as No Markup or Original.".into();
        }
        let req = self.ed.session.request(Params::new());
        let why = match self.ed.session.doc().plan(planners::REVIEW_ADD_COMMENT, &req) {
            Err(markise_doc::PlanError::NotApplicable(r)) => r.to_string(),
            Err(e) => e.to_string(),
            Ok(_) => "not available here".into(),
        };
        format!("Cannot add a comment here: {why}.")
    }

    /// Command `id` changes the document.
    fn mutates(&self, id: &str) -> bool {
        self.ed
            .registry
            .get(id)
            .is_some_and(|c| matches!(c.desc.effect, markise_doc::Effect::Mutates { .. }))
    }

    /// Runs command `id` through the editor (menus, tests). `page`: viewport height.
    pub fn run(
        &mut self,
        ctx: &egui::Context,
        parley: Option<&mut ParleyText>,
        id: &str,
        params: &Params,
    ) -> Result<(), markise_engine::editor::command::CommandError> {
        if self.partial && WHOLE_DOCUMENT_COMMANDS.contains(&id) {
            return Err(markise_engine::editor::command::CommandError::Disabled(
                "available once the whole file has loaded",
            ));
        }
        if self.review_preview() && self.mutates(id) {
            return Err(markise_engine::editor::command::CommandError::Disabled(
                "read-only while comments show as No Markup or Original",
            ));
        }
        // an open card edit is saved before another command acts on the document; refused,
        // the command does not run (the draft stays)
        if self.commit_card_edit().is_err() {
            return Err(markise_engine::editor::command::CommandError::Disabled(
                "the comment being edited could not be saved",
            ));
        }
        let images = markise_layout::block::NoImages;
        let r = with_shaper(ctx, parley, |sh| self.ed.execute(id, params, sh, &images, 600.0));
        self.ed.refresh_focus();
        self.blink_from = ctx.input(|i| i.time);
        if id == "review.addComment" {
            self.after_add_comment_result(&r);
        }
        r
    }

    /// `parley`: lay out and paint document text with the parley backend instead of epaint
    /// (developer switch, ADR 0005 T2). `editable`: edits are accepted (otherwise caret and
    /// selection still work).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        images: &mut TextureStore,
        theme: &Theme,
        mut parley: Option<&mut ParleyText>,
        editable: bool,
    ) -> ViewerOutput {
        let mut out = ViewerOutput::default();
        // No Markup / Original (issue #88): a read-only preview
        let editable = editable && !self.review_preview();
        self.editable = editable;
        if !editable {
            // a read-only surface (Side by Side's rendered pane) edits no comment (refused: the
            // draft stays pending, the document unsaved)
            let _ = self.commit_card_edit();
        }
        let rect = ui.available_rect_before_wrap();
        let id = ui.scope_id().with("markise-viewer");
        let page_rect = Rect::from_min_max(rect.min, pos2(rect.right() - SCROLLBAR_W, rect.bottom()));
        let resp = ui.interact(page_rect, id, Sense::click_and_drag());
        self.last_surface = Some((id, rect));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, color(theme.page_bg));
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let started = Instant::now();

        let ppp = ctx.pixels_per_point();
        let mut content_w = rect.width() - SCROLLBAR_W;
        // margin mode with comments in the document: a comment gutter is reserved at the right
        // and the column lays out in the rest (owner decision on #91), unless the window is
        // too narrow for it (then bubbles). "Has comments" is tracked incrementally, reading
        // the text in place (beyond a first step, on a scanner thread, issue #138) and only
        // after review-relevant edits (comment_presence); a gutter change waits for a frame
        // without pointer presses so clicks are hit-tested against one geometry (Codex review
        // of #94).
        // (margin cards only with all markup shown; Simple shows glyphs only, issue #88)
        let margin_mode = self.comment_display == markise_engine::settings::CommentDisplay::Margin
            && self.comment_view == markise_engine::settings::CommentView::All;
        let has = margin_mode && {
            let (has, more) = self.comment_presence.poll(&mut self.ed.session, now, &ctx);
            match more {
                Some(w) if w > 0.0 => ctx.request_repaint_after(std::time::Duration::from_secs_f64(w)),
                Some(_) => ctx.request_repaint(),
                None => {}
            }
            has
        };
        let want = comment_place::reserve_gutter(margin_mode, has, content_w);
        if want != self.gutter {
            let busy = self.drag.is_some()
                || ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released() || i.pointer.any_down());
            if comment_place::gutter_change_allowed(busy) {
                self.gutter = want;
            } else {
                ctx.request_repaint();
            }
        }
        if self.gutter {
            content_w -= comment_place::GUTTER_W;
        }
        let col_w = (content_w - 2.0 * theme.column_padding_x)
            .min(theme.column_max_width)
            .max(120.0);
        let mut left = rect.left() + ((content_w - col_w) * 0.5).max(4.0);
        let snap = smooth_scroll::snap_enabled();
        if snap {
            // the column edge on a device pixel (issue #86)
            left = smooth_scroll::round_to_pixel(left, ppp);
        }
        self.ed.view.set_params(col_w, theme, ppp);
        let vh = rect.height();

        // (a click on a comment's bubble or card opens its card editor, which takes the focus)
        let press_on_comment = ui
            .input(|i| i.pointer.press_origin())
            .is_some_and(|p| self.comment_at(p).is_some() || self.on_edited_card(p));
        if ((resp.clicked() || resp.drag_started()) && !press_on_comment) || std::mem::take(&mut self.refocus) {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                )
            });
        }

        // ---- input: wheel / trackpad (smooth scrolling, issue #86)
        {
            let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
            smooth_scroll::register_pane(&ctx, page_rect, ui.layer_id());
            let line = theme.line_px() * smooth_scroll::LINES_PER_NOTCH;
            // a programmatic scroll since the last frame (outline, anchor, find, Side by Side
            // sync, edits) drops pending motion
            self.smooth.follow(self.ed.view.source_anchor(), false);
            let d = smooth_scroll::take_delta(ui, &mut self.smooth, hovered, line, vh);
            if d.y != 0.0 {
                self.ed.view.scroll_by(-d.y);
                out.wheel_scrolled = true;
            }
            self.smooth.follow(self.ed.view.source_anchor(), true);
        }
        // ---- input: keyboard scrolling, only when no widget has focus
        if !focused && ui.memory(|m| m.focused().is_none()) {
            let (pd, pu, down, up, home, end, space) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::PageDown),
                    i.key_pressed(egui::Key::PageUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                    i.key_pressed(egui::Key::Space) && !i.modifiers.any(),
                )
            });
            if pd || space {
                self.ed.view.scroll_by(vh * 0.9);
            }
            if pu {
                self.ed.view.scroll_by(-vh * 0.9);
            }
            if down {
                self.ed.view.scroll_by(theme.line_px() * 2.0);
            }
            if up {
                self.ed.view.scroll_by(-theme.line_px() * 2.0);
            }
            if home {
                self.ed.view.scroll_to_y(0.0);
            }
            if end {
                self.ed.view.scroll_to_y(f64::MAX);
            }
        }
        // ---- input: scrollbar
        let bar = Rect::from_min_max(pos2(rect.right() - SCROLLBAR_W, rect.top()), rect.right_bottom());
        let bar_resp = ui.interact(bar, id.with("bar"), Sense::click_and_drag());
        let mut total = self.ed.view.total_height().max(1.0);
        let paged = self.ed.view.model().is_paged();
        if paged && bar_resp.dragged() {
            total = *self.drag_total.get_or_insert(total);
        } else {
            self.drag_total = None;
        }
        if (bar_resp.dragged() || bar_resp.clicked())
            && let Some(p) = bar_resp.interact_pointer_pos()
        {
            self.smooth.stop();
            let press = ui.input(|i| i.pointer.press_origin()).map_or(p.y, |o| o.y);
            let y = self.ed.view.scroll_y();
            match self
                .thumb_drag
                .target(press - rect.top(), p.y - rect.top(), vh, total, y)
            {
                scrollbar::Target::Y(y) => self.ed.view.scroll_to_y(y),
                scrollbar::Target::End => self.ed.view.scroll_to_y(f64::MAX),
            }
        }
        if !bar_resp.dragged() {
            self.thumb_drag.release();
        }

        // ---- input: keyboard editing
        let mut follow = false;
        if focused {
            let mut events = ui.input(|i| i.events.clone());
            let mut ran = false;
            let prev_focus = self.ed.session.selection().primary().focus.offset;
            // PROTOTYPE (live mode): Backspace after a block / Delete before it selects the
            // block; Backspace/Delete on a selected block deletes it whole (one undo step) —
            // never merges text into its `:::` lines
            if editable
                && !self.snap_blocks
                && (!self.atomic.is_empty() || !self.native_units.is_empty())
                && self.ed.session.read_only().is_none()
            {
                let units: Vec<(usize, usize, usize, usize)> =
                    self.atomic.iter().chain(self.native_units.iter()).copied().collect();
                let sel = self.ed.session.selection().primary();
                let caret = (sel.anchor == sel.focus).then_some(sel.focus.offset);
                let mut consumed = false;
                for e in &events {
                    let Event::Key { key, pressed: true, .. } = e else {
                        continue;
                    };
                    if !matches!(key, egui::Key::Backspace | egui::Key::Delete) || consumed {
                        continue;
                    }
                    if let Some(fb) = self.focused_block
                        && let Some(&(start, _, _, after)) = units.iter().find(|a| a.0 == fb)
                    {
                        let ps = PatchSet::new(vec![markise_doc::Patch::verbatim(start..after, "")]);
                        if let Ok(ps) = ps {
                            let _ = self.ed.edit(|s| s.apply_external(&ps, "Delete Block", false));
                            self.ed
                                .set_selection(Selection::caret(start.min(self.ed.session.text().len())));
                        }
                        self.focused_block = None;
                        consumed = true;
                    } else if let Some(c) = caret {
                        let t = self.ed.session.text();
                        // never into a card's marker lines or a hidden `{#id}` label
                        let guarded = if *key == egui::Key::Backspace {
                            self.native_first.iter().any(|&f| c >= f && c <= content_start(t, f))
                        } else {
                            self.native_last.contains(&c) || self.native_hidden.iter().any(|h| h.start == c)
                        };
                        if guarded {
                            consumed = true;
                            continue;
                        }
                        let hit = if *key == egui::Key::Backspace {
                            units.iter().find(|a| near_start(t, a.3, c))
                        } else {
                            units.iter().find(|a| a.2 == c)
                        };
                        if let Some(&(start, _, _, after)) = hit {
                            self.focused_block = Some(start);
                            self.ed.set_selection(Selection::caret(after));
                            consumed = true;
                        }
                    }
                }
                if consumed {
                    events.retain(|e| {
                        !matches!(
                            e,
                            Event::Key {
                                key: egui::Key::Backspace | egui::Key::Delete,
                                ..
                            }
                        )
                    });
                    ran = true;
                }
            }
            // PROTOTYPE (native blocks): Enter at the end of an answer: the new answer comes after
            // its hidden label
            if editable && !self.native_hidden.is_empty() {
                let sel = self.ed.session.selection().primary();
                let enter = events.iter().any(|e| {
                    matches!(e, Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.any())
                });
                if enter
                    && sel.anchor == sel.focus
                    && let Some(h) = self.native_hidden.iter().find(|h| h.start == sel.focus.offset)
                {
                    let to = h.end;
                    self.ed.set_selection(Selection::caret(to));
                }
            }
            // PROTOTYPE (block move): ⌥⌘↑ / ⌥⌘↓ move the selected block past its neighbour
            if editable
                && !self.snap_blocks
                && self.ed.session.read_only().is_none()
                && let Some(fb) = self.focused_block
            {
                let mv = events.iter().find_map(|e| match e {
                    Event::Key {
                        key: egui::Key::ArrowUp,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt && modifiers.command => Some(true),
                    Event::Key {
                        key: egui::Key::ArrowDown,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt && modifiers.command => Some(false),
                    _ => None,
                });
                if let Some(up) = mv {
                    events.retain(|e| {
                        !matches!(
                            e,
                            Event::Key {
                                key: egui::Key::ArrowUp | egui::Key::ArrowDown,
                                ..
                            }
                        )
                    });
                    self.move_block_step(fb, up);
                    ran = true;
                }
            }
            if let Some(b) = self.focused_block {
                let enter = events.iter().any(|e| {
                    matches!(e, Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if !modifiers.any())
                });
                if enter {
                    self.pending_edit = Some(b);
                    events.retain(|e| {
                        !matches!(
                            e,
                            Event::Key {
                                key: egui::Key::Enter,
                                ..
                            } | Event::Text(_)
                        )
                    });
                } else if events.iter().any(|e| {
                    matches!(e, Event::Key { pressed: true, key, .. } if !matches!(key, egui::Key::Backspace | egui::Key::Delete))
                        || matches!(e, Event::Text(_))
                }) {
                    self.focused_block = None;
                }
            }
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                for e in events {
                    ran |= self.handle_event(e, sh, &*images, editable, vh);
                }
            });
            let _ = prev_focus;
            // PROTOTYPE (block packages): `/` on an empty line opens the block palette
            if ran && editable && !self.snap_blocks && self.snaps.is_some() {
                let c = self.ed.session.selection().primary().focus.offset;
                let text = self.ed.session.text();
                let ls = text[..c].rfind('\n').map_or(0, |i| i + 1);
                let le = text[c..].find('\n').map_or(text.len(), |i| c + i);
                if &text[ls..le] == "/" && c == le && !in_block(text, ls) {
                    let rev = self.ed.session.revision().counter;
                    crate::block_palette::open_for(Some(crate::block_edit::InsertTarget::new(text, ls, le, rev)));
                }
            }
            if let Some(t) = self.pending_copy.take() {
                ctx.copy_text(t);
            }
            if ran {
                follow = true;
                out.edited = true;
            }
        }

        // ---- input: pointer (hit tests against the last frame's layout)
        let modifiers = ui.input(|i| i.modifiers);
        let on_plus = if editable && !self.snap_blocks && self.snaps.is_some() && self.ed.session.read_only().is_none()
        {
            ui.input(|i| i.pointer.interact_pos()).and_then(|p| {
                self.gaps
                    .iter()
                    .find(|g| plus_rect(self.gap_col.0, g.1).contains(p))
                    .map(|g| g.0)
            })
        } else {
            None
        };
        if let Some(off) = on_plus
            && resp.hovered()
        {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            if resp.clicked() {
                let text = self.ed.session.text();
                let rev = self.ed.session.revision().counter;
                crate::block_palette::open_for(Some(crate::block_edit::InsertTarget::new(text, off, off, rev)));
            }
        }
        let on_handle = if editable && !self.snap_blocks && self.ed.session.read_only().is_none() {
            ui.input(|i| i.pointer.press_origin().or(i.pointer.interact_pos()))
                .and_then(|p| {
                    self.block_ctl
                        .iter()
                        .find(|c| handle_rect(c.rect).contains(p))
                        .map(|c| c.offset)
                })
        } else {
            None
        };
        if let Some(off) = on_handle {
            ctx.set_cursor_icon(CursorIcon::Grab);
            if resp.drag_started() {
                egui::DragAndDrop::set_payload(&ctx, MoveBlock(off));
            } else if resp.clicked()
                && let Some(&(start, _, _, after)) = self.native_units.iter().find(|a| a.0 == off)
            {
                // a click on the handle selects a native block (a click on an image does)
                self.focused_block = Some(start);
                self.ed.set_selection(Selection::caret(after));
                out.edited = true;
            }
        }
        // PROTOTYPE (native blocks): the view switch and the form button of any block
        let on_view = ui.input(|i| i.pointer.interact_pos()).and_then(|p| {
            self.block_ctl
                .iter()
                .find(|c| view_button_rect(c.rect).contains(p))
                .map(|c| c.offset)
        });
        let on_form = ui.input(|i| i.pointer.interact_pos()).and_then(|p| {
            self.block_ctl
                .iter()
                .find(|c| c.view != crate::block_snap::BlockView::Markdown && edit_button_rect(c.rect).contains(p))
                .map(|c| c.offset)
        });
        if on_view.is_some() || on_form.is_some() {
            if resp.hovered() {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                if let (Some(off), Some(s)) = (on_view, self.snaps.as_mut()) {
                    s.rotate_view(off);
                } else if on_form.is_some() && self.ed.session.read_only().is_none() {
                    self.pending_edit = on_form;
                }
                ctx.request_repaint();
            }
        }
        let on_snap = if on_view.is_some() || on_form.is_some() {
            None
        } else {
            ui.input(|i| i.pointer.interact_pos())
                .and_then(|p| self.snap_rects.iter().find(|(r, _)| r.contains(p)).map(|(_, o)| *o))
        };
        if on_snap.is_some() && resp.hovered() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            if resp.double_clicked() && editable && !self.snap_blocks {
                self.pending_edit = on_snap;
            } else if resp.clicked() {
                if editable && !self.snap_blocks {
                    // live mode: the click selects the block (caret parked after it, source not
                    // revealed)
                    if let Some(&(start, _, _, after)) = self.atomic.iter().find(|a| Some(a.0) == on_snap) {
                        self.focused_block = Some(start);
                        self.ed.set_selection(Selection::caret(after));
                        out.edited = true;
                    }
                } else {
                    out.goto_source = on_snap;
                }
            }
        }
        let pressed = ui.input(|i| i.pointer.primary_pressed())
            && resp.hovered()
            && on_snap.is_none()
            && on_plus.is_none()
            && on_handle.is_none()
            && on_view.is_none()
            && on_form.is_none();
        // a click on a task checkbox toggles it (spec 02 §9): the press neither places the caret
        // nor starts a selection; the toggle happens on release over the same box (a press
        // dragged off the box does nothing)
        let task = if pressed {
            ui.input(|i| i.pointer.interact_pos())
                .and_then(|p| self.task_box_at(p, rect.top()))
        } else {
            None
        };
        if let Some((i, ord)) = task {
            self.task_press = Some((self.item_key(i), ord, self.ed.changes()));
        }
        if let Some(t) = self.task_press
            && ui.input(|i| i.pointer.primary_released())
        {
            self.task_press = None;
            let over = ui
                .input(|i| i.pointer.interact_pos())
                .and_then(|p| self.task_box_at(p, rect.top()));
            // (a press held while the document changed is cancelled: it named the old text)
            if let Some((i, ord)) = over
                && editable
                && t.2 == self.ed.changes()
                && (self.item_key(i), ord) == (t.0, t.1)
                && self.toggle_task(i, ord)
            {
                out.edited = true;
            }
        }
        // a press on a comment's bubble or card opens the comment for editing in its card
        // (issue #87); a press inside the card being edited is the field's
        let on_edited = pressed
            && ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| self.on_edited_card(p));
        let comment_press = if pressed && task.is_none() && !on_edited {
            ui.input(|i| i.pointer.interact_pos()).and_then(|p| self.comment_at(p))
        } else {
            None
        };
        if let Some(i) = comment_press {
            self.open_comment(i, true);
            out.edited = true;
            self.blink_from = now;
        }
        if pressed
            && task.is_none()
            && comment_press.is_none()
            && !on_edited
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
            && !self.on_toggle_or_link(p, rect.top(), modifiers)
        {
            let count = match self.last_press {
                Some((t, q, n)) if now - t < 0.45 && q.distance(p) < 5.0 => (n % 3) + 1,
                _ => 1,
            };
            self.last_press = Some((now, p, count));
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                if let Some(pos) = self.hit_screen(p, rect.top(), sh, &*images) {
                    let unit = match count {
                        2 => Unit::Word,
                        3 => Unit::Block,
                        _ => Unit::Char,
                    };
                    let (a, b) = self.unit_range(pos, unit, sh, &*images);
                    // shift-click extends from the retained anchor, and a drag that follows
                    // keeps extending from it
                    let (sel, anchor) = if modifiers.shift && unit == Unit::Char {
                        let an = self.ed.session.selection().primary().anchor;
                        (Selection::single(an, pos), (an, an))
                    } else {
                        (Selection::single(a, b), (a, b))
                    };
                    // the reveal state is frozen while the pointer selects (ADR 0006 §2.4)
                    self.ed.reveal = false;
                    self.ed.set_selection(sel);
                    self.drag = Some(Drag { anchor, unit });
                }
            });
            out.edited = true;
            self.blink_from = now;
        }
        if let Some(d) = self.drag {
            let down = ui.input(|i| i.pointer.primary_down());
            if down {
                if let Some(p) = ui.input(|i| i.pointer.interact_pos())
                    && (resp.dragged() || resp.is_pointer_button_down_on())
                {
                    // auto-scroll when dragging past the edges
                    if p.y < rect.top() {
                        self.ed.view.scroll_by((p.y - rect.top()).max(-40.0));
                        ctx.request_repaint();
                    } else if p.y > rect.bottom() {
                        self.ed.view.scroll_by((p.y - rect.bottom()).min(40.0));
                        ctx.request_repaint();
                    }
                    with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                        if let Some(pos) = self.hit_screen(p, rect.top(), sh, &*images) {
                            let (a, b) = self.unit_range(pos, d.unit, sh, &*images);
                            let (s0, s1) = d.anchor;
                            let sel = if pos.key() < s0.key() {
                                Selection::single(s1, a)
                            } else if d.unit == Unit::Char {
                                Selection::single(s0, pos)
                            } else {
                                Selection::single(s0, b)
                            };
                            if &sel != self.ed.session.selection() {
                                self.ed.set_selection(sel);
                                out.edited = true;
                            }
                        }
                    });
                }
            } else {
                self.drag = None;
                self.ed.reveal = true;
                out.edited = true;
            }
        }

        // PROTOTYPE (live mode): block images are atomic for the caret
        if editable && !self.snap_blocks {
            // (a stop just after a block can be the next block's marker line)
            for _ in 0..6 {
                if !self.keep_caret_out_of_blocks(self.last_focus) {
                    break;
                }
                out.edited = true;
            }
        }
        self.last_focus = self.ed.session.selection().primary().focus.offset;
        if out.edited {
            self.ed.refresh_focus();
            self.blink_from = now;
        }
        if follow || std::mem::take(&mut self.find_reveal) {
            with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                self.ed.scroll_to_caret(vh, sh, &*images)
            });
        }

        // ---- PROTOTYPE: block snapshots replace their items' heights
        let (placed, native_now, raw_now) = if self.snap_blocks || (editable && live_snaps()) {
            if self.snaps.is_none() && !self.snaps_tried {
                use crate::block_lib::Lib;
                let path = self.ed.path().map(|p| p.to_path_buf());
                if self.lib.is_none() {
                    let lib = path
                        .as_deref()
                        .map(|p| crate::block_lib::resolve(p, self.ed.session.text()))
                        .unwrap_or(Lib::None);
                    if std::env::var_os("MARKISE_BLOCK_LOG").is_some() {
                        eprintln!("[blocklib] {lib:?}");
                    }
                    self.lib = Some(lib);
                }
                match self.lib.clone() {
                    Some(Lib::Ready { root, .. }) => {
                        self.snaps_tried = true;
                        self.snaps = crate::block_snap::BlockSnaps::from_env(&ctx, path, Some(root));
                    }
                    Some(Lib::Pending(b)) => {
                        // MARKISE_BLOCK_TRUST=1: accept without asking (scripted runs only)
                        let auto = std::env::var_os("MARKISE_BLOCK_TRUST").is_some_and(|v| v == "1");
                        if let Some(yes) = if auto { Some(true) } else { consent_dialog(&ctx, &b) } {
                            self.lib = Some(if yes {
                                match crate::block_lib::install(&b) {
                                    Ok(root) => {
                                        eprintln!(
                                            "[blocklib] installed {} {} → {}",
                                            b.library,
                                            b.version,
                                            root.display()
                                        );
                                        Lib::Ready {
                                            root,
                                            label: b.library.clone(),
                                            package: !b.is_folder(),
                                        }
                                    }
                                    Err(e) => Lib::Broken(e),
                                }
                            } else {
                                Lib::Broken(String::new())
                            });
                            ctx.request_repaint();
                        }
                    }
                    Some(Lib::Broken(e)) => {
                        self.snaps_tried = true;
                        self.snaps = crate::block_snap::BlockSnaps::from_env(&ctx, path, None);
                        if !e.is_empty()
                            && let Some(s) = self.snaps.as_mut()
                        {
                            s.form.error = Some((format!("Библиотека блоков: {e}"), Instant::now()));
                        }
                    }
                    _ => {
                        self.snaps_tried = true;
                        self.snaps = crate::block_snap::BlockSnaps::from_env(&ctx, path, None);
                    }
                }
            }
            match self.snaps.as_mut() {
                Some(s) => {
                    s.form.poll(&mut self.ed);
                    // PROTOTYPE (native form): the keyboard back after closing; "Markdown view"
                    if s.form.take_refocus() {
                        self.refocus = true;
                    }
                    if let Some(off) = s.form.take_markdown_request() {
                        s.set_view(off, crate::block_snap::BlockView::Markdown);
                    }
                    // PROTOTYPE (block packages): a block chosen in the palette → draft form
                    if editable
                        && !self.snap_blocks
                        && !s.form.is_open()
                        && self.ed.session.read_only().is_none()
                        && let Some((preset, target)) = crate::block_palette::take_choice()
                    {
                        let text = self.ed.session.text();
                        let rev = self.ed.session.revision().counter;
                        let target = target.unwrap_or_else(|| {
                            caret_target(text, self.ed.session.selection().primary().focus.offset, rev)
                        });
                        let tv = crate::block_snap::ThemeVars::from_theme(theme);
                        let res = if in_block(text, target.start) {
                            Err("Внутри блока вставлять блоки нельзя (вложенные блоки не поддерживаются).".to_string())
                        } else {
                            s.form.open_draft(text, &preset.markdown, target, &tv)
                        };
                        if let Err(e) = res {
                            s.form.error = Some((e, Instant::now()));
                        }
                        ctx.request_repaint();
                    }
                    if let Some(why) = self.ed.session.read_only()
                        && self.pending_edit.take().is_some()
                    {
                        s.form.error = Some((why.to_string(), Instant::now()));
                    }
                    if let Some(off) = self.pending_edit.take() {
                        let tv = crate::block_snap::ThemeVars::from_theme(theme);
                        let rev = self.ed.session.revision().counter;
                        ctx.request_repaint();
                        if let Err(e) = s.form.open(self.ed.session.text(), off, rev, &tv) {
                            eprintln!("[blockform] cannot open the dialog: {e}");
                            s.form.error = Some((e, Instant::now()));
                        }
                    }
                    // indices only hold from here on: pages come and go first (ADR 0016)
                    self.ed.view.prepare_for_frame(vh);
                    // a page boundary inside a block near the viewport: bring in the other part
                    // (far ones stay cut, else this would cascade over the document)
                    if !s.cut.is_empty() {
                        let y = self.ed.view.scroll_y();
                        let (lo, hi) = (y - 2.0 * vh as f64, y + 3.0 * vh as f64);
                        let near: Vec<usize> = s
                            .cut
                            .iter()
                            .copied()
                            .filter(|&o| {
                                let i = self.ed.view.model().item_at_offset(o);
                                let yi = self.ed.view.item_y(i);
                                yi >= lo && yi <= hi
                            })
                            .collect();
                        for o in near {
                            self.ed.view.ensure_offset(o);
                        }
                    }
                    let vis: Vec<usize> = self.last_visible.iter().map(|v| v.0.0).collect();
                    {
                        let ed = &self.ed;
                        s.remap_views(|o| ed.map_recent(o));
                    }
                    let model = self.ed.view.model().clone();
                    let width = self.ed.view.width();
                    let paging = self.ed.view.paging_generation();
                    let (mut over, placed) = s.frame(
                        &ctx,
                        &model,
                        self.ed.session.text(),
                        self.ed.session.revision().counter,
                        width,
                        ppp,
                        theme.block_spacing,
                        &vis,
                        &crate::block_snap::ThemeVars::from_theme(theme),
                        paging,
                    );
                    // live preview of a new block: extra height under the item before the point
                    // (or in place of the `/` line), painted after the layout
                    self.draft_paint = None;
                    if let (
                        Some((tex, size)),
                        Some(crate::block_edit::Preview {
                            at: crate::block_edit::PreviewAt::Insert(st, en),
                            ..
                        }),
                    ) = (s.draft_image, s.form.preview.as_ref())
                    {
                        let (st, en) = (*st, *en);
                        let items: Vec<(usize, usize)> =
                            model.items.iter_with_start().map(|(a, it)| (a, it.len)).collect();
                        let spacing = theme.block_spacing;
                        if en > st {
                            if let Some(k) = items.iter().find(|&&(a, l)| a <= st && st < a + l.max(1)).map(|x| x.0) {
                                over.insert(k, spacing + size.y);
                                self.draft_paint = Some((k, spacing, tex, size, true));
                            }
                        } else {
                            let k = placed
                                .iter()
                                .find(|p| p.end == st)
                                .map(|p| p.offset)
                                .or_else(|| items.iter().rfind(|&&(a, l)| a + l <= st && l > 0).map(|x| x.0));
                            if let Some(k) = k {
                                let base = over.get(&k).copied().or_else(|| self.natural.get(&k).copied());
                                if let Some(base) = base {
                                    over.insert(k, base + spacing + size.y);
                                    self.draft_paint = Some((k, base + spacing, tex, size, false));
                                }
                            }
                        }
                    }
                    self.ed.view.set_item_insets(std::mem::take(&mut s.insets));
                    self.ed.view.set_height_overrides(over);
                    (placed, s.native.clone(), s.raw.clone())
                }
                None => (Vec::new(), Vec::new(), Vec::new()),
            }
        } else {
            self.ed.view.set_item_insets(Default::default());
            self.ed.view.set_height_overrides(Default::default());
            (Vec::new(), Vec::new(), Vec::new())
        };

        // keyboard scrolling, the scrollbar or a caret / find reveal in this frame took over
        self.smooth.follow(self.ed.view.source_anchor(), false);
        // ---- layout of the viewport (+ half a screen of margin)
        let mut visible = {
            let view = &mut self.ed.view;
            let imgs: &TextureStore = images;
            match parley.as_deref_mut() {
                Some(p) => {
                    p.shaper.set_scale(ppp);
                    view.frame(vh, vh * 0.5, &mut p.shaper, imgs)
                }
                None => ctx.fonts_mut(|f| view.frame(vh, vh * 0.5, f, imgs)),
            }
        };
        // (layout moves the anchor on once estimated heights become real)
        self.smooth.follow(self.ed.view.source_anchor(), true);
        // the available caret stops may have changed under the selection (a diagram or
        // formula became rendered, front matter collapsed): move it to the nearest stop
        // (not on a read-only surface without the focus: in Side by Side the session's selection
        // may be the source pane's, which need not sit on a rendered caret stop)
        let searching_selection = self.find_matches.as_ref().is_some_and(|(rev, matches, active)| {
            *rev == self.ed.session.revision().counter
                && active
                    .and_then(|i| matches.get(i))
                    .is_some_and(|r| *r == self.ed.session.selection().primary().range())
        });
        if !searching_selection
            && (focused || editable)
            && with_shaper(&ctx, parley.as_deref_mut(), |sh| self.ed.fix_selection(sh, &*images))
        {
            self.ed.refresh_focus();
            ctx.request_repaint();
        }
        // pixel snap (issue #86): render at the scroll offset rounded to device pixels with item
        // origins on device pixels, so every line moves by whole pixels and neighbouring lines
        // never cross a pixel boundary on different frames; `tops` keep (unsnapped, snapped)
        // for things placed inside an item (the caret)
        let scroll_y = self.ed.view.scroll_y();
        let mut tops = Vec::new();
        if snap {
            let shift = smooth_scroll::offset_shift(scroll_y, ppp);
            tops.reserve(visible.len());
            for v in &mut visible {
                let y = smooth_scroll::snap_top(v.y, shift, rect.top(), ppp);
                tops.push((v.y, y));
                v.y = y;
            }
        }
        if smooth_scroll::diag_enabled() {
            let mut f = smooth_scroll::frame_diag();
            f.scroll_y = scroll_y;
            f.total = self.ed.view.total_height();
        }
        out.painted = visible.len();
        out.laid_out = self.ed.view.laid_out.len();
        self.last_visible = visible.iter().map(|v| (self.item_key(v.idx), v.y)).collect();
        self.track_card_edit();
        self.ed.clear_recent_edits();
        self.natural = visible
            .iter()
            .map(|v| (self.ed.view.model().item_start(v.idx), v.layout.height))
            .collect();
        let draft_y = self.draft_paint.and_then(|(k, ..)| {
            visible
                .iter()
                .find(|v| self.ed.view.model().item_start(v.idx) == k)
                .map(|v| v.y)
        });
        self.last_left = left;
        self.last_top = rect.top();
        // PROTOTYPE (block packages): insertion points between top-level items (never inside a
        // block), the "+ Добавить блок" pill on hover and the drop line while dragging a preset
        self.gaps.clear();
        self.gap_col = (left, col_w);
        let mut do_move: Option<(usize, usize)> = None;
        if editable && !self.snap_blocks && self.snaps.is_some() && self.ed.session.read_only().is_none() {
            let text = self.ed.session.text();
            // the blocks of the materialized pages (every frame: no scan of the whole text)
            let ranges: Vec<std::ops::Range<usize>> = self
                .snaps
                .iter()
                .flat_map(|s| s.blocks.iter())
                .filter_map(|b| markise_engine::block_form::block_at(text, b.offset))
                .map(|b| b.range)
                .collect();
            let model = self.ed.view.model().clone();
            let starts: Vec<(usize, usize)> = model.items.iter_with_start().map(|(st, it)| (st, it.len)).collect();
            let outer: Vec<(usize, usize, f32)> = visible
                .iter()
                .filter_map(|v| {
                    let (st, len) = *starts.get(v.idx)?;
                    if ranges.iter().any(|r| st > r.start && st <= r.end) {
                        return None;
                    }
                    let end = ranges.iter().find(|r| r.start == st).map_or(st + len, |r| r.end);
                    Some((v.idx, end, rect.top() + v.y))
                })
                .collect();
            for k in 0..outer.len() {
                let y = match outer.get(k + 1) {
                    Some(n) => n.2 - theme.block_spacing * 0.5,
                    None => continue,
                };
                if y > rect.top() && y < rect.bottom() {
                    self.gaps.push((outer[k].1, y));
                }
            }
            let hover = ctx.input(|i| i.pointer.hover_pos());
            let dragging = egui::DragAndDrop::payload::<crate::block_palette::DragPreset>(&ctx);
            let moving = egui::DragAndDrop::payload::<MoveBlock>(&ctx);
            let accent = theme_color(theme.link);
            if dragging.is_some() || moving.is_some() {
                // Esc cancels; autoscroll near the edges; the line shows the nearest valid point
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    egui::DragAndDrop::clear_payload(&ctx);
                }
                let hy = hover.map(|h| h.y).or(self.debug_drag_y.map(|y| rect.top() + y));
                if let Some(y) = hy {
                    if y < rect.top() + 36.0 && y >= rect.top() - 40.0 {
                        self.ed.view.scroll_by(-14.0);
                        ctx.request_repaint();
                    } else if y > rect.bottom() - 36.0 && y <= rect.bottom() + 40.0 {
                        self.ed.view.scroll_by(14.0);
                        ctx.request_repaint();
                    }
                }
                // a moved block: not at its own position
                let own = moving.as_ref().and_then(|m| {
                    self.atomic
                        .iter()
                        .chain(self.native_units.iter())
                        .find(|a| a.0 == m.0)
                        .copied()
                });
                let valid: Vec<(usize, f32)> = self
                    .gaps
                    .iter()
                    .copied()
                    .filter(|g| own.is_none_or(|a| g.0 != a.1 && g.0 != a.2))
                    .collect();
                if let Some(hy) = hy.filter(|y| *y >= rect.top() && *y <= rect.bottom())
                    && hover.is_none_or(|h| rect.contains(h))
                    && let Some(&(off, y)) = valid
                        .iter()
                        .min_by(|a, b| (a.1 - hy).abs().total_cmp(&(b.1 - hy).abs()))
                {
                    // above the block images (painted later in this frame)
                    let lp = ctx
                        .layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::unique("markise-drop-line"),
                        ))
                        .with_clip_rect(rect);
                    lp.line_segment(
                        [pos2(left - 8.0, y), pos2(left + col_w + 8.0, y)],
                        Stroke::new(3.0, accent),
                    );
                    lp.circle_filled(pos2(left - 8.0, y), 5.0, accent);
                    if ctx.input(|i| i.pointer.any_released()) {
                        let rev = self.ed.session.revision().counter;
                        if let Some(d) = &dragging {
                            let _ = egui::DragAndDrop::take_payload::<crate::block_palette::DragPreset>(&ctx);
                            crate::block_palette::choose(
                                d.0.clone(),
                                Some(crate::block_edit::InsertTarget::new(text, off, off, rev)),
                            );
                        } else if let Some(m) = &moving {
                            let _ = egui::DragAndDrop::take_payload::<MoveBlock>(&ctx);
                            do_move = Some((m.0, off));
                        }
                        ctx.request_repaint();
                    }
                }
                if let Some(m) = &moving
                    && let Some(pos) = hover
                {
                    // ghost: the block's name
                    let name = text
                        .get(m.0..)
                        .and_then(|t| t.lines().next())
                        .unwrap_or(":::")
                        .trim_start_matches(':')
                        .trim()
                        .to_string();
                    let label = if crate::block_edit::lang() == "ru" {
                        format!("Переместить: {name}")
                    } else {
                        format!("Move: {name}")
                    };
                    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("markise-block-move"));
                    let lp = ctx.layer_painter(layer);
                    let g = lp.layout_no_wrap(label, egui::FontId::proportional(13.0), Color32::WHITE);
                    let r = Rect::from_min_size(pos + vec2(14.0, 6.0), vec2(g.size().x + 20.0, 26.0));
                    lp.rect_filled(r, 13.0, accent);
                    lp.galley(r.min + vec2(10.0, (26.0 - g.size().y) / 2.0), g, Color32::WHITE);
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                }
            } else if let Some(h) = hover.filter(|h| h.x > left - 30.0 && h.x < left + col_w && rect.contains(*h))
                && let Some(&(_, y)) = self.gaps.iter().find(|g| (g.1 - h.y).abs() < 9.0)
            {
                let r = plus_rect(left, y);
                let hot = r.contains(h);
                painter.line_segment(
                    [pos2(r.right() + 4.0, y), pos2(left + col_w, y)],
                    Stroke::new(1.0, accent.gamma_multiply(0.5)),
                );
                painter.rect_filled(r, 11.0, if hot { accent } else { accent.gamma_multiply(0.75) });
                painter.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    if crate::block_edit::lang() == "ru" {
                        "+ Добавить блок"
                    } else {
                        "+ Add block"
                    },
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        }
        if let Some((start, dest)) = do_move {
            self.move_block(start, dest);
        }
        // block images: their items are not painted as Markdown
        self.snap_rects.clear();
        self.block_ctl.clear();
        let model = self.ed.view.model().clone();
        for p in &placed {
            if let Some(v) = visible.iter().find(|v| model.item_start(v.idx) == p.offset) {
                let r = Rect::from_min_size(pos2(left, rect.top() + v.y + theme.block_spacing), p.size);
                self.snap_rects.push((r, p.offset));
                self.block_ctl.push(BlockCtl {
                    rect: r,
                    offset: p.offset,
                    view: crate::block_snap::BlockView::Site,
                });
                match p.tex {
                    Some(tex) => painter.image(
                        tex,
                        r,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    ),
                    // being rendered again (its texture was evicted with its page)
                    None => painter.rect_filled(r, 8.0, theme_color(theme.meta_text).gamma_multiply(0.08)),
                };
            }
        }
        // PROTOTYPE (native blocks): cards behind their items (the items are painted below)
        {
            use crate::block_snap::{CARD_BOTTOM, CARD_HEADER, CARD_PAD};
            let width = self.ed.view.width();
            let text_len = self.ed.session.text().len();
            self.native_units.clear();
            self.native_markers.clear();
            self.native_first.clear();
            self.native_last.clear();
            self.native_hidden.clear();
            let item_of = |vis: &[markise_layout::Visible], st: usize| -> Option<(f32, f32, f32)> {
                vis.iter()
                    .find(|v| model.item_start(v.idx) == st)
                    .map(|v| (rect.top() + v.y, v.layout.gap, v.layout.height))
            };
            for n in &native_now {
                let open = model.items.partition_point_start(n.offset);
                let next = model.items.partition_point_start(n.end);
                let before = if open > 0 {
                    model.item_span(open - 1).end
                } else {
                    n.offset
                };
                let after = if next < model.items.len() {
                    model.item_start(next)
                } else {
                    text_len
                };
                let open_end = model.item_span(open).end;
                self.native_units.push((n.offset, n.end, before, after));
                self.native_markers.push((n.offset, open_end, before, n.first_inner));
                self.native_markers
                    .push((n.close_start, n.end, n.last_inner_end, after));
                self.native_first.push(n.first_inner);
                self.native_last.push(n.last_inner_end);
                self.native_hidden.extend(n.hidden.iter().cloned());
                // the part of the card in the laid-out items
                let ys: Vec<f32> = visible
                    .iter()
                    .filter(|v| {
                        let st = model.item_start(v.idx);
                        st >= n.offset && st < n.end
                    })
                    .map(|v| rect.top() + v.y)
                    .collect();
                let (Some(&ymin), Some(&ymax)) = (ys.first(), ys.last()) else {
                    continue;
                };
                let top = match item_of(&visible, n.offset) {
                    Some((y, ..)) => y + theme.block_spacing,
                    None => ymin - 40.0,
                };
                let bottom = match item_of(&visible, n.close_start) {
                    Some((y, ..)) => y + CARD_BOTTOM,
                    None => ymax + rect.height() + 40.0,
                };
                let card = Rect::from_min_max(pos2(left, top), pos2(left + width, bottom));
                let accent = accent_color(&n.spec.accent, theme.dark);
                let tint = if n.spec.kind == crate::block_snap::ViewKind::Callout
                    || n.spec.kind == crate::block_snap::ViewKind::Choices
                {
                    accent
                } else {
                    theme_color(theme.meta_text)
                };
                painter.rect_filled(card, 12.0, tint.gamma_multiply(if theme.dark { 0.10 } else { 0.06 }));
                painter.rect_stroke(
                    card,
                    12.0,
                    Stroke::new(1.0, tint.gamma_multiply(0.35)),
                    StrokeKind::Inside,
                );
                let hc = pos2(left + CARD_PAD + 6.0, top + CARD_HEADER * 0.5);
                paint_view_icon(&painter, hc, &n.spec.icon, accent);
                painter.text(
                    hc + vec2(13.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &n.spec.title,
                    egui::FontId::proportional(13.0),
                    accent,
                );
                // answer rows and the explanation area
                let (rl, rr) = (left + CARD_PAD, left + width - CARD_PAD);
                for (st, correct) in &n.answers {
                    if let Some((y, gap, h)) = item_of(&visible, *st) {
                        let r = Rect::from_min_max(pos2(rl, y + gap), pos2(rr, y + h));
                        let (fill, line) = if *correct {
                            let g = accent_color("green", theme.dark);
                            (g.gamma_multiply(0.14), g.gamma_multiply(0.6))
                        } else {
                            (
                                theme_color(theme.page_bg),
                                theme_color(theme.meta_text).gamma_multiply(0.35),
                            )
                        };
                        painter.rect_filled(r, 8.0, fill);
                        painter.rect_stroke(r, 8.0, Stroke::new(1.0, line), StrokeKind::Inside);
                    }
                }
                let ex: Vec<(f32, f32, f32)> = n.explain.iter().filter_map(|st| item_of(&visible, *st)).collect();
                if let (Some(a), Some(b)) = (ex.first(), ex.last()) {
                    let r = Rect::from_min_max(pos2(rl, a.0 + a.1), pos2(rr, b.0 + b.2));
                    painter.rect_filled(r, 8.0, theme_color(theme.meta_text).gamma_multiply(0.10));
                    if n.explain.first().is_some_and(|f| item_of(&visible, *f).is_some()) {
                        painter.text(
                            pos2(r.left() + crate::block_snap::ROW_PAD, r.top() + 15.0),
                            egui::Align2::LEFT_CENTER,
                            if crate::block_edit::lang() == "ru" {
                                "Объяснение"
                            } else {
                                "Explanation"
                            },
                            egui::FontId::proportional(12.5),
                            theme_color(theme.meta_text),
                        );
                    }
                }
                self.block_ctl.push(BlockCtl {
                    rect: card,
                    offset: n.offset,
                    view: crate::block_snap::BlockView::Native,
                });
            }
            // blocks shown as their text: the controls sit on their lines
            for &(st, end, view) in &raw_now {
                let on: Vec<(f32, f32)> = visible
                    .iter()
                    .filter(|v| {
                        let s = model.item_start(v.idx);
                        s >= st && s < end
                    })
                    .map(|v| (rect.top() + v.y, rect.top() + v.y + v.layout.height))
                    .collect();
                if let (Some(a), Some(b)) = (on.first(), on.last())
                    && item_of(&visible, st).is_some()
                {
                    let top = a.0 + theme.block_spacing * 0.5;
                    self.block_ctl.push(BlockCtl {
                        rect: Rect::from_min_max(pos2(left, top), pos2(left + width, b.1)),
                        offset: st,
                        view,
                    });
                }
            }
        }
        // PROTOTYPE (live mode): atomic caret units and the selected block's outline
        {
            let text_len = self.ed.session.text().len();
            self.atomic = placed
                .iter()
                .map(|p| {
                    // by source position: pages may have come and gone since the scan
                    let open = model.items.partition_point_start(p.offset);
                    let before = if open > 0 {
                        model.item_span(open - 1).end
                    } else {
                        p.offset
                    };
                    let next = model.items.partition_point_start(p.end);
                    let after = if next < model.items.len() {
                        model.item_start(next)
                    } else {
                        text_len
                    };
                    (p.offset, p.end, before, after)
                })
                .collect();
            let caret = self.ed.session.selection().primary().focus.offset;
            if let Some(fb) = self.focused_block {
                let t = self.ed.session.text();
                let still = self
                    .atomic
                    .iter()
                    .chain(self.native_units.iter())
                    .any(|a| a.0 == fb && near_start(t, a.3, caret));
                if !still {
                    self.focused_block = None;
                } else if let Some(c) = self.block_ctl.iter().find(|c| c.offset == fb) {
                    painter.rect_stroke(
                        c.rect.expand(3.0),
                        8.0,
                        Stroke::new(2.0, theme_color(theme.link)),
                        StrokeKind::Outside,
                    );
                }
            }
        }
        // PROTOTYPE: the controls of the hovered block: ⠿ move, view switch, "✎ Edit" (forms)
        let hover = ctx.input(|i| i.pointer.hover_pos());
        let ru = crate::block_edit::lang() == "ru";
        let writable = self.ed.session.read_only().is_none();
        for (i, c) in self.block_ctl.iter().enumerate() {
            let r = &c.rect;
            let forced = self.force_edit_button == Some(i);
            // ⠿ move handle (live mode)
            if editable
                && !self.snap_blocks
                && writable
                && (hover.is_some_and(|h| r.expand2(vec2(28.0, 0.0)).contains(h)) || forced)
            {
                let hr = handle_rect(*r);
                let hot = hover.is_some_and(|h| hr.contains(h));
                if hot {
                    painter.rect_filled(hr, 5.0, theme_color(theme.link).gamma_multiply(0.18));
                }
                let col = if hot {
                    theme_color(theme.link)
                } else {
                    theme_color(theme.meta_text)
                };
                for row in 0..3 {
                    for k in 0..2 {
                        painter.circle_filled(hr.min + vec2(6.0 + 6.0 * k as f32, 6.0 + 6.0 * row as f32), 1.6, col);
                    }
                }
            }
            if !forced && !hover.is_some_and(|h| r.contains(h) && rect.contains(h)) {
                continue;
            }
            let pill = |b: Rect, label: &str, pencil: bool| {
                let hot = hover.is_some_and(|h| b.contains(h));
                let fill = if hot {
                    theme_color(theme.link)
                } else {
                    Color32::from_black_alpha(170)
                };
                painter.rect_filled(b, 6.0, fill);
                if pencil {
                    // a small pencil (the UI font has no ✎ glyph)
                    let o = b.left_center() + vec2(10.0, 0.0);
                    painter.line_segment(
                        [o + vec2(0.0, 5.0), o + vec2(8.0, -3.0)],
                        Stroke::new(2.2, Color32::WHITE),
                    );
                    painter.line_segment(
                        [o + vec2(-1.0, 6.5), o + vec2(0.6, 4.4)],
                        Stroke::new(1.4, Color32::WHITE),
                    );
                }
                painter.text(
                    b.center() + vec2(if pencil { 8.0 } else { 0.0 }, 0.0),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(12.5),
                    Color32::WHITE,
                );
            };
            pill(view_button_rect(*r), c.view.label(), false);
            if c.view != crate::block_snap::BlockView::Markdown && writable {
                pill(edit_button_rect(*r), if ru { "Правка" } else { "Edit" }, true);
            }
        }
        if let Some((e, at)) = self.snaps.as_ref().and_then(|s| s.form.error.clone())
            && at.elapsed().as_secs_f32() < 5.0
        {
            let r = Rect::from_min_size(pos2(left, rect.top() + 4.0), vec2(rect.width().min(600.0), 22.0));
            painter.rect_filled(r, 4.0, Color32::from_rgb(180, 40, 40));
            painter.text(
                r.left_center() + vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                e,
                egui::FontId::proportional(12.5),
                Color32::WHITE,
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if !placed.is_empty() || !native_now.is_empty() {
            visible.retain(|v| {
                let st = model.item_start(v.idx);
                !placed.iter().any(|p| st >= p.offset && st < p.end)
                    && !native_now.iter().any(|n| st == n.offset || st == n.close_start)
            });
        }
        // PROTOTYPE (live preview): the new block's image at its insertion point
        if let Some((k, off, tex, size, replace)) = self.draft_paint
            && let Some(vy) = draft_y
        {
            let r = Rect::from_min_size(pos2(left, rect.top() + vy + off), size);
            painter.image(
                tex,
                r,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.rect_stroke(
                r.expand(3.0),
                8.0,
                Stroke::new(1.5, theme_color(theme.link).gamma_multiply(0.7)),
                StrokeKind::Outside,
            );
            if replace {
                let model = self.ed.view.model().clone();
                visible.retain(|v| model.item_start(v.idx) != k);
            }
        }

        // ---- paint
        images.begin_frame();
        let mut requests = Vec::new();
        for v in &visible {
            let origin = pos2(left, rect.top() + v.y);
            paint_layout(
                &painter,
                ui,
                &v.layout,
                origin,
                images,
                &mut requests,
                parley.as_deref_mut(),
            );
        }
        for src in requests {
            images.request(&ctx, &src);
        }
        // ---- code block tools (copy button, language field)
        {
            let imgs: &TextureStore = images;
            let mut sh = |f: &mut dyn FnMut(&mut dyn TextShaper, &dyn markise_layout::Images)| {
                with_shaper(&ctx, parley.as_deref_mut(), |s| f(s, imgs))
            };
            self.code_tools_ui(ui, &visible, rect, left, theme, focused, editable, &mut out, &mut sh);
        }

        // ---- review comments: margin cards or bubble popovers (visible items only)
        self.comment_cards(ui, &visible, rect, left, col_w, theme);

        self.find_rects.clear();
        // Search rectangles use the same source-to-rendered mapping as selections and
        // touch only visible items. Hidden syntax has no invented rendered rectangle.
        if let Some((rev, matches, active)) = self.find_matches.clone()
            && rev == self.ed.session.revision().counter
        {
            let rects = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                let model = self.ed.view.model().clone();
                let mut out = Vec::new();
                for v in &visible {
                    let span = model.item_span(v.idx);
                    let begin = matches.partition_point(|r| r.end < span.start);
                    for (i, range) in matches
                        .iter()
                        .enumerate()
                        .skip(begin)
                        .take_while(|(_, r)| r.start <= span.end)
                    {
                        let mut g = self.ed.geometry(sh, &*images);
                        for r in g.selection_rects(v.idx, range.clone()) {
                            out.push((r.translate(vec2(left, rect.top() + v.y)), Some(i) == active));
                        }
                    }
                }
                out
            });
            for (r, active) in rects {
                self.find_rects.push(r);
                painter.rect_filled(
                    r,
                    0.0,
                    Color32::from_rgba_unmultiplied(
                        255,
                        if active { 145 } else { 220 },
                        40,
                        if active { 130 } else { 65 },
                    ),
                );
            }
        }
        // ---- selection and caret
        let sel = self.ed.session.selection().primary();
        if !sel.is_caret() {
            let range = sel.range();
            let fill = color(theme.chrome.selection_bg).gamma_multiply(0.55);
            // rectangles first: painting inside `fonts_mut` would deadlock on the context lock
            let rects = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                let model = self.ed.view.model().clone();
                let mut out = Vec::new();
                for v in &visible {
                    let span = model.item_span(v.idx);
                    if span.end < range.start || span.start > range.end {
                        continue;
                    }
                    let mut g = self.ed.geometry(sh, &*images);
                    for r in g.selection_rects(v.idx, range.clone()) {
                        out.push(r.translate(vec2(left, rect.top() + v.y)));
                    }
                }
                out
            });
            for r in rects {
                painter.rect_filled(r, 0.0, fill);
            }
        }
        if focused {
            let since = now - self.blink_from;
            let on = since >= BLINK_FOR || ((since / BLINK_HALF) as u64).is_multiple_of(2);
            let caret = with_shaper(&ctx, parley.as_deref_mut(), |sh| self.ed.caret_rect(sh, &*images));
            self.caret_screen = None;
            if let Some(c) = caret {
                let y0 = c.min.y - scroll_y as f32;
                let y = y0 + smooth_scroll::shift_at(&tops, y0) + rect.top();
                let r = Rect::from_min_size(pos2(left + c.min.x - 0.75, y), vec2(1.5, c.height().max(4.0)));
                self.caret_screen = Some(r);
                if on && r.intersects(rect) {
                    painter.rect_filled(r, 0.0, color(theme.text));
                }
                // IME: allowed while the surface has focus; the candidate window follows the
                // caret (preedit display is M2e, ADR 0006 §6.3)
                ctx.output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        purpose: Default::default(),
                        rect,
                        cursor_rect: r,
                        should_interrupt_composition: false,
                    })
                });
            }
            if since < BLINK_FOR {
                let phase = BLINK_HALF - (since % BLINK_HALF);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(phase.max(0.02)));
            }
        }
        // the I-beam only while the pointer is over the surface itself (not over the sidebar,
        // the toolbar or a window on top of it: `hovered` honours layers; issue #36)
        // (block chrome over the document is not text: this frame's rectangles)
        let chrome = resp.hover_pos().and_then(|p| {
            let writable = self.ed.session.read_only().is_none();
            if egui::DragAndDrop::payload::<MoveBlock>(&ctx).is_some() {
                return Some(CursorIcon::Grabbing);
            }
            if on_plus.is_some() {
                return Some(CursorIcon::PointingHand);
            }
            self.block_ctl.iter().find_map(|c| {
                if view_button_rect(c.rect).contains(p)
                    || (writable
                        && c.view != crate::block_snap::BlockView::Markdown
                        && edit_button_rect(c.rect).contains(p))
                {
                    return Some(CursorIcon::Default);
                }
                if editable && !self.snap_blocks && writable && handle_rect(c.rect).contains(p) {
                    return Some(CursorIcon::Grab);
                }
                if self.snap_rects.iter().any(|(r, o)| *o == c.offset && r.contains(p)) {
                    return Some(CursorIcon::PointingHand);
                }
                // a card's header row
                let native = c.view == crate::block_snap::BlockView::Native;
                let head = Rect::from_min_size(c.rect.min, vec2(c.rect.width(), crate::block_snap::CARD_HEADER));
                (native && head.contains(p)).then_some(CursorIcon::Default)
            })
        });
        if resp.hovered() {
            ctx.set_cursor_icon(chrome.unwrap_or(CursorIcon::Text));
        }

        // ---- pointer: link / toggle hover
        if chrome.is_none()
            && let Some(p) = resp.hover_pos()
            && let Some((_, local, layout)) = hit(&visible, p, left, rect.top())
        {
            let link = layout.link_at(local).is_some() && (modifiers.command || !focused);
            let on_toggle = layout.toggle.is_some_and(|t| t.contains(local));
            let on_task = editable && task_box_hit(layout, local).is_some();
            if link || on_toggle || on_task {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((idx, local, layout)) = hit(&visible, p, left, rect.top())
        {
            if layout.toggle.is_some_and(|t| t.contains(local)) {
                self.ed.view.toggle_expanded(idx);
                ctx.request_repaint();
            } else if modifiers.command
                && let Some(target) = layout.link_at(local).map(str::to_string)
            {
                if target.starts_with('#') {
                    if self.scroll_to_anchor(&target) {
                        ctx.request_repaint();
                    }
                } else {
                    out.link = Some(target);
                }
            }
        }
        if out.edited {
            // the reveal set of the new caret position is laid out next frame
            ctx.request_repaint();
        }

        // ---- review: context menu and the hover bar of suggestions (ADR 0012)
        if editable {
            self.review_ui(ui, &resp, rect, parley, images, &mut out);
        }

        // ---- scrollbar
        let total = self.drag_total.unwrap_or_else(|| self.ed.view.total_height()).max(1.0);
        if total > vh as f64 + 1.0 {
            let (thumb_h, _) = scrollbar::thumb_geometry(vh, total);
            let top = rect.top() + scrollbar::thumb_top(vh, total, self.ed.view.scroll_y());
            let thumb = Rect::from_min_size(pos2(bar.left() + 2.0, top), vec2(SCROLLBAR_W - 4.0, thumb_h));
            let c = color(theme.meta_text).gamma_multiply(if bar_resp.hovered() || bar_resp.dragged() {
                0.8
            } else {
                0.45
            });
            painter.rect_filled(thumb, CornerRadius::same(3), c);
        }
        if follow {
            out.input_ms = Some(started.elapsed().as_secs_f64() * 1e3);
        }
        out
    }

    /// Screen rectangle (last frame) of the visible item holding source offset `offset`:
    /// tests click into list items, paragraphs… through it.
    pub fn item_screen_rect(&self, offset: usize) -> Option<Rect> {
        let model = self.ed.view.model();
        let i = model.item_at_offset(offset);
        let &(_, y) = self.shown().iter().find(|(idx, _)| *idx == i)?;
        let l = self.ed.view.cache_peek(i)?;
        Some(Rect::from_min_size(
            pos2(self.last_left, self.last_top + y),
            vec2(self.ed.view.width(), l.height),
        ))
    }

    /// The document offset under screen point `p` (last frame's geometry; `None` outside the
    /// surface): where dropped files are inserted.
    pub fn offset_at_screen(
        &mut self,
        ctx: &egui::Context,
        parley: Option<&mut ParleyText>,
        images: &dyn markise_layout::Images,
        p: Pos2,
    ) -> Option<usize> {
        let (_, r) = self.last_surface?;
        if !r.contains(p) {
            return None;
        }
        let top = self.last_top;
        with_shaper(ctx, parley, |sh| self.hit_screen(p, top, sh, images)).map(|d| d.offset)
    }

    /// The front-matter toggle or a Cmd/Ctrl-clicked link is under `p` (the press is not a
    /// caret placement).
    /// Sets how comments are shown (setting `markdown.commentDisplay`).
    pub fn set_comment_display(&mut self, d: markise_engine::settings::CommentDisplay) {
        self.comment_display = d;
    }

    /// Sets how review marks are shown (setting `markdown.commentView`, issue #88): the layout
    /// renders them accordingly (display only; the text is never changed).
    pub fn set_comment_view(&mut self, v: markise_engine::settings::CommentView) {
        use markise_engine::settings::CommentView as C;
        use markise_layout::ReviewView as R;
        if v == self.comment_view {
            return;
        }
        self.comment_view = v;
        self.ed.view.set_review_view(match v {
            C::All => R::All,
            C::Simple => R::Simple,
            C::Final => R::Final,
            C::Original => R::Original,
        });
        self.hover = None;
        self.popover = None;
    }

    /// The view is a clean preview of the text (No Markup / Original, issue #88): the rendered
    /// view is read-only, its hidden marks would make edits ambiguous.
    pub fn review_preview(&self) -> bool {
        use markise_engine::settings::CommentView as C;
        matches!(self.comment_view, C::Final | C::Original)
    }

    /// The comments of the visible items at the last frame, with their cards (tests).
    pub fn shown_comments(&self) -> &[ShownComment] {
        &self.comments
    }

    /// The comment gutter was reserved at the last frame (tests).
    pub fn comment_gutter(&self) -> bool {
        self.gutter
    }

    /// Test hook ("background settled", issue #42): the margin's "has comments" tracking has
    /// nothing due (it runs only in margin mode).
    #[doc(hidden)]
    pub fn comments_settled(&self) -> bool {
        self.comment_display != markise_engine::settings::CommentDisplay::Margin || self.comment_presence.settled()
    }

    /// A "has comments" evaluation is due or running (tests).
    pub fn comment_lookup_pending(&self) -> bool {
        self.comment_presence.pending()
    }

    /// Full "has comments" scans started and text bytes they read (tests: typing ordinary
    /// text must not scan).
    pub fn comment_scans(&self) -> (u64, u64) {
        (self.comment_presence.scans, self.comment_presence.scanned)
    }

    /// The content column's left edge and width at the last frame (screen points; tests).
    pub fn column(&self) -> (f32, f32) {
        (self.last_left, self.ed.view.width())
    }

    /// The suggestion hover bar's rectangle at the last frame, when shown (tests).
    pub fn hover_bar_rect(&self) -> Option<Rect> {
        self.hover.as_ref().and_then(|h| h.rect)
    }

    /// A bubbles-mode popover is open (tests).
    pub fn popover_open(&self) -> bool {
        self.comments.iter().any(|c| c.card.is_some()) && self.popover.is_some()
    }

    /// The text of the visible items at the last frame, item after item (tests).
    pub fn visible_text(&self) -> String {
        let mut out = String::new();
        for &(idx, _) in &self.shown() {
            if let Some(l) = self.ed.view.cache_peek(idx) {
                for p in &l.prims {
                    if let markise_layout::Prim::Text { text, .. } = p {
                        out.push_str(text.text());
                        out.push('\n');
                    }
                }
            }
        }
        out
    }

    /// The items shown at the last frame with their tops, at their current indices.
    /// The items painted in the last frame by source position ((start, ordinal among the items
    /// starting there), top relative to the surface): tests and measurements check that the
    /// content on screen does not move.
    pub fn painted(&self) -> &[((usize, usize), f32)] {
        &self.last_visible
    }

    fn shown(&self) -> Vec<(usize, f32)> {
        let m = self.ed.view.model();
        self.last_visible
            .iter()
            .filter_map(|&((start, nth), y)| {
                // moved by the edits since the frame (keys come before the pointer)
                let start = self.ed.map_recent(start)?;
                let i = m.items.partition_point_start(start) + nth;
                (i < m.items.len() && m.item_start(i) == start && !m.is_page(i)).then_some((i, y))
            })
            .collect()
    }

    /// Item `i` by source position: its start and its ordinal among the items starting there
    /// (segments of a large block), stable while a paged model materializes or evicts pages.
    fn item_key(&self, i: usize) -> (usize, usize) {
        let m = self.ed.view.model();
        let start = m.item_start(i);
        (start, i - m.items.partition_point_start(start).min(i))
    }

    /// The shown comment whose bubble or card is under screen point `p`.
    fn comment_at(&self, p: Pos2) -> Option<usize> {
        self.comments
            .iter()
            .position(|c| (!c.revealed && c.glyph.expand(2.0).contains(p)) || c.card.is_some_and(|r| r.contains(p)))
    }

    /// Opens comment `i` for editing in its card (issue #87; the caret goes after it);
    /// `pin`: its popover stays open. On a read-only surface the caret goes after the comment.
    fn open_comment(&mut self, i: usize, pin: bool) {
        let Some(c) = self.comments.get(i).cloned() else { return };
        if self.editable {
            self.start_card_edit(c.source.start, Some(&c.text), false);
        } else {
            // read-only (Side by Side): the caret goes to the end of its text, where the
            // source pane shows it for editing
            self.ed.set_selection(Selection::caret(c.content.end));
            self.ed.refresh_focus();
            self.refocus = true;
        }
        if pin {
            self.popover = Some(Popover {
                key: c.source.start,
                since: 0.0,
                pinned: true,
            });
        }
    }

    /// Lays out and paints the comment cards of the visible items: in the right margin when it
    /// is wide enough and the setting says so, else as a popover over the hovered (or pinned)
    /// bubble. Only visible items are looked at; card text is shaped only for cards shown.
    fn comment_cards(
        &mut self,
        ui: &mut Ui,
        visible: &[markise_layout::Visible],
        rect: Rect,
        left: f32,
        col_w: f32,
        theme: &Theme,
    ) {
        let model = self.ed.view.model().clone();
        let mut shown = Vec::new();
        for v in visible {
            let base = model.item_start(v.idx);
            for c in &v.layout.comments {
                shown.push(ShownComment {
                    glyph: c.rect.translate(vec2(left, rect.top() + v.y)),
                    source: base + c.source.start as usize..base + c.source.end as usize,
                    content: base + c.content.start as usize..base + c.content.end as usize,
                    text: c.text.clone(),
                    revealed: c.revealed,
                    card: None,
                });
            }
        }
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        // issue #91: cards and popovers go only where the surface is actually on screen (not
        // past the window's or the parent's clip edge, not over the scrollbar), and margin
        // cards only into the real room right of the content column (the reserved gutter plus
        // any centring slack; a glyph further right, e.g. in a wide table, counts too)
        let vis = comment_place::visible_area(rect, SCROLLBAR_W, ui.clip_rect(), ctx.content_rect());
        let col_right = shown.iter().map(|c| c.glyph.right()).fold(left + col_w, f32::max);
        // cards only beside a reserved gutter (and only if it is on screen); else bubbles
        let slot = self
            .gutter
            .then(|| comment_place::margin_column(vis, col_right))
            .flatten();
        let use_margin = slot.is_some();
        let look = CardLook::new(theme, &ctx);
        let painter = ui.painter_at(vis);
        let mut clicked = None;
        // the card editor's keys come out of the input before anything reads them (issue #87)
        if let Some(f) = self.card_edit_keys(&ctx) {
            self.finish_card_edit(f, true);
            ctx.request_repaint();
        }
        let mut finish = None;
        let mut edited_drawn = false;
        if let Some((x, w)) = slot {
            self.popover = None;
            let mut next_y = rect.top();
            for (i, c) in shown.iter_mut().enumerate() {
                // nothing is shaped for a card that would start below the viewport; the stack
                // only grows downwards, so the rest would too
                let y = (c.glyph.top() - 4.0).max(next_y);
                if y > vis.bottom() {
                    break;
                }
                let editing = self.editing(c.source.start);
                let card = if editing {
                    let (card, f) = self.card_editor(ui, &look, pos2(x, y), w, vis);
                    edited_drawn = true;
                    finish = finish.or(f);
                    card
                } else {
                    let galley = look.galley(&ctx, &c.text, w);
                    let card = Rect::from_min_size(pos2(x, y), look.size(&galley, w));
                    look.paint(&painter, card, galley);
                    card
                };
                next_y = card.bottom() + 6.0;
                // the connector from the bubble's line to its card, in the margin only (it never
                // crosses text), starting with a dot at the column edge
                let from = pos2(col_right + 6.0, c.glyph.center().y);
                let to = pos2(card.left(), card.top() + 12.0);
                let line = Stroke::new(1.0, look.accent.gamma_multiply(0.5));
                painter.circle_filled(from, 2.0, look.accent.gamma_multiply(0.7));
                painter.line_segment([from, to], line);
                if !editing {
                    let r = ui.interact(
                        card,
                        ui.scope_id().with(("markise-comment-card", c.source.start)),
                        Sense::click(),
                    );
                    let r = r.on_hover_cursor(CursorIcon::PointingHand);
                    if r.clicked() {
                        clicked = Some(i);
                    }
                }
                c.card = Some(card);
            }
        } else {
            // bubbles: a popover over the hovered (after a short rest) or pinned bubble; the
            // comment being edited keeps its popover open
            let pointer = ctx.input(|i| i.pointer.hover_pos());
            if let Some(e) = &self.card_edit {
                self.popover = Some(Popover {
                    key: e.key,
                    since: 0.0,
                    pinned: true,
                });
            } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.popover = None;
            }
            let over_card = |p: Pos2| {
                self.comments
                    .iter()
                    .any(|c| c.card.is_some_and(|r| r.expand(4.0).contains(p)))
            };
            let hovered = pointer.and_then(|p| {
                shown
                    .iter()
                    .find(|c| !c.revealed && c.glyph.expand(2.0).contains(p))
                    .map(|c| c.source.start)
            });
            let simple = self.comment_view != markise_engine::settings::CommentView::All;
            match (hovered, self.popover) {
                _ if self.card_edit.is_some() => {}
                // Simple (issue #88): no popover on hover, only a clicked (pinned) one
                _ if simple && !self.popover.is_some_and(|p| p.pinned) => self.popover = None,
                (Some(k), Some(pop)) if simple && pop.key != k => {}
                (None, Some(_)) if simple => {}
                (Some(k), Some(pop)) if pop.key == k => {}
                (Some(k), Some(pop)) if pop.pinned && pop.key != k => {
                    self.popover = Some(Popover {
                        key: k,
                        since: now,
                        pinned: false,
                    })
                }
                (Some(k), _) => {
                    self.popover = Some(Popover {
                        key: k,
                        since: now,
                        pinned: false,
                    })
                }
                (None, Some(pop)) if !pop.pinned && !pointer.is_some_and(over_card) => self.popover = None,
                _ => {}
            }
            if let Some(pop) = self.popover
                && let Some(i) = shown.iter().position(|c| c.source.start == pop.key)
            {
                let wait = POPOVER_DELAY - (now - pop.since);
                if pop.pinned || wait <= 0.0 {
                    let c = &shown[i];
                    let w = comment_place::popover_width(vis);
                    let editing = self.editing(c.source.start);
                    if editing {
                        // the editor's height is known only once drawn: placed with the size
                        // of its last frame (first frame: one line)
                        let est = self.card_edit_size.unwrap_or(vec2(w, CARD_ICON + 2.0 * CARD_PAD + 8.0));
                        let at = comment_place::popover_pos(c.glyph, est, vis);
                        let area = egui::Area::new(egui::Id::unique(("markise-comment-popover-edit", pop.key)))
                            .order(egui::Order::Foreground)
                            .constrain_to(vis)
                            .fixed_pos(at)
                            .show(&ctx, |ui| self.card_editor(ui, &look, ui.max_rect().min, w, vis));
                        let (card, f) = area.inner;
                        self.card_edit_size = Some(card.size());
                        edited_drawn = true;
                        finish = finish.or(f);
                        shown[i].card = Some(card);
                    } else {
                        let galley = look.galley(&ctx, &c.text, w);
                        let size = look.size(&galley, w);
                        // below the bubble, flipped left / up near the visible area's edges
                        let at = comment_place::popover_pos(c.glyph, size, vis);
                        let area = egui::Area::new(egui::Id::unique(("markise-comment-popover", pop.key)))
                            .order(egui::Order::Foreground)
                            .constrain_to(vis)
                            .fixed_pos(at)
                            .show(&ctx, |ui| {
                                let (r, resp) = ui.allocate_exact_size(size, Sense::click());
                                look.paint(ui.painter(), r, galley);
                                resp.on_hover_cursor(CursorIcon::PointingHand)
                            });
                        if area.inner.clicked() {
                            clicked = Some(i);
                        }
                        shown[i].card = Some(area.response.rect);
                    }
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.01)));
                }
            }
        }
        self.comments = shown;
        if let Some(f) = finish {
            // the field lost the focus to a click elsewhere: that click placed its own caret
            self.finish_card_edit(f, false);
            ctx.request_repaint();
        } else if self.card_edit.is_some() && !edited_drawn && self.card_edit_unseen() {
            ctx.request_repaint();
        }
        if let Some(i) = clicked {
            self.open_comment(i, !use_margin);
            ctx.request_repaint();
        }
    }

    fn on_toggle_or_link(&self, p: Pos2, top: f32, m: egui::Modifiers) -> bool {
        for &(idx, y) in &self.shown() {
            let Some(l) = self.ed.view.cache_peek(idx) else {
                continue;
            };
            let local = pos2(p.x - self.last_left, p.y - top - y);
            if local.y < 0.0 || local.y >= l.height {
                continue;
            }
            return l.toggle.is_some_and(|t| t.contains(local)) || (m.command && l.link_at(local).is_some());
        }
        false
    }

    /// The task checkbox under screen point `p` (last frame's geometry): the visible item and
    /// the box's list item ordinal ([`BlockLayout::tasks`]).
    fn task_box_at(&self, p: Pos2, top: f32) -> Option<(usize, u16)> {
        for &(idx, y) in &self.shown() {
            let Some(l) = self.ed.view.cache_peek(idx) else {
                continue;
            };
            let local = pos2(p.x - self.last_left, p.y - top - y);
            if local.y < 0.0 || local.y >= l.height {
                continue;
            }
            return task_box_hit(l, local).map(|li| (idx, li));
        }
        None
    }

    /// Toggles the task box of list item `li` (ordinal among the enclosing list items, 0 =
    /// outermost) of visible item `idx` as one undo step ("Toggle Task Status"); the selection
    /// is kept as it was (the box is not a caret stop, spec 02 §9).
    fn toggle_task(&mut self, idx: usize, li: u16) -> bool {
        let leaf = self.ed.view.model().item_span(idx).start;
        // that list item's own start: inside it but before any nested list, so the planner's
        // "deepest task item" is exactly this one
        let Some(start) = self
            .ed
            .session
            .doc()
            .containers_at(leaf)
            .into_iter()
            .filter(|c| matches!(c.kind, ContainerKind::ListItem { .. }))
            .nth(li as usize)
            .filter(|c| matches!(c.kind, ContainerKind::ListItem { task: Some(_), .. }))
            .map(|c| c.span.start)
        else {
            return false;
        };
        self.ed
            .edit(|s| {
                s.transact("Toggle Task Status", TxKind::Structural, |tx| {
                    let keep = tx.selection().clone();
                    tx.set_selection(Selection::caret(start));
                    let plan = tx.plan(planners::TOGGLE_TASK, Params::new())?;
                    tx.apply_plan(plan)?;
                    // `[ ]` / `[x]` / `[X]` have the same length: offsets are unchanged
                    tx.set_selection(keep);
                    Ok(())
                })
            })
            .is_ok()
    }

    /// The document position under screen point `p` (last frame's geometry).
    /// The document's context menu (Add Comment, Accept / Reject, Delete Comment, Accept /
    /// Reject All; a right click outside the selection moves the caret there first) and the
    /// Accept / Reject bar over a hovered suggestion.
    fn review_ui(
        &mut self,
        ui: &mut Ui,
        resp: &egui::Response,
        rect: Rect,
        mut parley: Option<&mut ParleyText>,
        images: &TextureStore,
        out: &mut ViewerOutput,
    ) {
        let ctx = ui.ctx().clone();
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let pos = with_shaper(&ctx, parley.as_deref_mut(), |sh| {
                self.hit_screen(p, rect.top(), sh, images)
            });
            if let Some(pos) = pos {
                let sel = self.ed.session.selection().primary();
                let r = sel.range();
                if sel.is_caret() || pos.offset < r.start || pos.offset > r.end {
                    self.ed.set_selection(Selection::single(pos, pos));
                    self.ed.refresh_focus();
                }
            }
            resp.request_focus();
        }
        let mut chosen = None;
        resp.context_menu(|ui| {
            let enabled = |id: &str| self.command_enabled(id);
            // outside a table: Insert Table… (issue #164); in one: the Table submenu (it has it)
            if !enabled("table.addRowBelow")
                && enabled("table.insert")
                && let Some(spec) = self.ed.registry.get("table.insert")
            {
                if ui.button(spec.desc.label).clicked() {
                    chosen = Some(spec.desc.id);
                    ui.close();
                }
                ui.separator();
            }
            if enabled("table.addRowBelow") {
                ui.menu_button("Table", |ui| {
                    for e in crate::menu::TABLE_COMMANDS {
                        let Some(id) = e else {
                            ui.separator();
                            continue;
                        };
                        let Some(spec) = self.ed.registry.get(id) else { continue };
                        if ui
                            .add_enabled(enabled(id), egui::Button::new(spec.desc.label))
                            .clicked()
                        {
                            chosen = Some(spec.desc.id);
                            ui.close();
                        }
                    }
                });
                ui.separator();
            }
            for e in CONTEXT_COMMANDS {
                match e {
                    None => {
                        ui.separator();
                    }
                    Some(id) => {
                        let Some(spec) = self.ed.registry.get(id) else { continue };
                        let mut b = egui::Button::new(spec.desc.label);
                        if let Some(c) = self.keymap.shown_chord_of(id) {
                            b = b.shortcut_text(crate::menu::shortcut_text(c, self.platform));
                        }
                        if ui.add_enabled(enabled(id), b).clicked() {
                            chosen = Some(spec.desc.id);
                            ui.close();
                        }
                    }
                }
            }
        });
        if let Some(id) = chosen {
            if ASKING_COMMANDS.contains(&id) {
                self.app_command = Some(id);
            } else {
                out.review = Some((id, None));
            }
        }
        // hover bar: any edit hides it (review round 5: never act on an obsolete or a
        // different mark); it comes back with fresh data on the next pointer move over a mark
        let counter = self.ed.session.revision().counter;
        if self.hover.as_ref().is_some_and(|h| h.counter != counter) {
            self.hover = None;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let on_bar = self
            .hover
            .as_ref()
            .and_then(|h| h.rect)
            .zip(pointer)
            .is_some_and(|(r, p)| r.expand(6.0).contains(p));
        // looked up again only when the pointer moved or the text changed
        let key = pointer.map(|p| (p, self.ed.session.revision().counter));
        let moved = key != self.hover_key;
        self.hover_key = key;
        if !on_bar && moved {
            let idle = self.drag.is_none() && !ui.input(|i| i.pointer.any_down());
            let found = match pointer {
                Some(p) if idle && resp.hovered() => {
                    let pos = with_shaper(&ctx, parley, |sh| self.hit_screen(p, rect.top(), sh, images));
                    pos.and_then(|pos| self.suggestion_at(pos.offset)).map(|m| (m, p))
                }
                _ => None,
            };
            match found {
                Some((m, p)) => {
                    if self.hover.as_ref().is_none_or(|h| h.mark != m) {
                        self.hover = Some(HoverBar {
                            mark: m,
                            counter: self.ed.session.revision().counter,
                            anchor: pos2(p.x - 24.0, p.y - 40.0),
                            rect: None,
                        });
                    }
                }
                None => self.hover = None,
            }
        }
        if let Some(h) = self.hover.as_mut() {
            let at = h.mark.content.start;
            let area = egui::Area::new(egui::Id::unique("markise-review-hover"))
                .order(egui::Order::Foreground)
                .fixed_pos(h.anchor)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Accept").on_hover_text("Accept this suggestion").clicked() {
                                out.review = Some(("review.accept", Some(at)));
                            }
                            if ui.button("Reject").on_hover_text("Reject this suggestion").clicked() {
                                out.review = Some(("review.reject", Some(at)));
                            }
                        });
                    });
                });
            h.rect = Some(area.response.rect);
            if out.review.is_some() {
                self.hover = None;
            }
        }
    }

    /// The suggestion whose source contains `offset` (cached per revision and offset).
    fn suggestion_at(&mut self, offset: usize) -> Option<markise_doc::review::ReviewMark> {
        let counter = self.ed.session.revision().counter;
        if let Some((c, o, m)) = &self.hover_cache
            && *c == counter
            && *o == offset
        {
            return m.clone();
        }
        let m = markise_doc::review::mark_at(self.ed.session.doc(), offset, |k| k.is_suggestion());
        self.hover_cache = Some((counter, offset, m.clone()));
        m
    }

    fn hit_screen(
        &mut self,
        p: Pos2,
        top: f32,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
    ) -> Option<DocPos> {
        let shown = self.shown();
        let (&(first, fy), &(last, _)) = (shown.first()?, shown.last()?);
        let mut target = None;
        for &(idx, y) in &self.shown() {
            if p.y >= top + y {
                target = Some((idx, y));
            }
        }
        let (idx, y) = target.unwrap_or((first, fy));
        let _ = last;
        let local = pos2(p.x - self.last_left, p.y - top - y);
        let mut g = self.ed.geometry(sh, images);
        g.hit(idx, local)
    }

    /// The range of `unit` at `pos` (a caret for chars).
    fn unit_range(
        &mut self,
        pos: DocPos,
        unit: Unit,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
    ) -> (DocPos, DocPos) {
        let mut g = self.ed.geometry(sh, images);
        match unit {
            Unit::Char => (pos, pos),
            Unit::Word => {
                let b = g.horizontal(pos, true, true);
                let a = g.horizontal(b, false, true);
                if a.key() <= pos.key() && pos.key() <= b.key() {
                    (a, b)
                } else {
                    (pos, pos)
                }
            }
            Unit::Block => match g.item_pos(pos) {
                Some((i, _)) => {
                    let l = g.view.layout(i, g.shaper, g.images);
                    let start = g.view.model().item_start(i);
                    let at = |p: Option<markise_layout::caret::ItemPos>| {
                        p.map(|p| DocPos::new(start + p.rel as usize)).unwrap_or(pos)
                    };
                    (
                        at(markise_layout::caret::edge_stop(&l, false)),
                        at(markise_layout::caret::edge_stop(&l, true)),
                    )
                }
                None => (pos, pos),
            },
        }
    }

    /// One input event; returns `true` when it ran a command.
    fn handle_event(
        &mut self,
        e: Event,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
        editable: bool,
        vh: f32,
    ) -> bool {
        match e {
            Event::Text(t) if editable && !t.is_empty() && !t.chars().any(char::is_control) => {
                self.exec("edit.insertText", &Params::text("text", t), sh, images, vh)
            }
            Event::Ime(egui::ImeEvent::Commit(t)) if editable && !t.is_empty() => {
                // an IME commit is one transaction with boundaries before and after (§4.2)
                self.ed.session.close_typing();
                let r = self.exec("edit.insertText", &Params::text("text", t), sh, images, vh);
                self.ed.session.close_typing();
                r
            }
            Event::Paste(t) if editable && !t.is_empty() => {
                // a paste is its own undo step (spec 02 §11); plain source text (M2e adds
                // HTML conversion and clipping)
                self.ed.session.close_typing();
                let r = self.exec("edit.insertText", &Params::text("text", t), sh, images, vh);
                self.ed.session.close_typing();
                r
            }
            Event::Copy | Event::Cut => {
                let r = self.ed.session.selection().primary().range();
                if !r.is_empty() {
                    // copied after the shaper closure (the context is locked inside it); in a
                    // clean review view what is shown (issue #88: accepted / rejected text)
                    let shown = match self.comment_view {
                        markise_engine::settings::CommentView::Final => {
                            markise_doc::review::visible_text(self.ed.session.doc(), r, true)
                        }
                        markise_engine::settings::CommentView::Original => {
                            markise_doc::review::visible_text(self.ed.session.doc(), r, false)
                        }
                        _ => self.ed.session.text()[r].to_string(),
                    };
                    self.pending_copy = Some(shown);
                    if matches!(e, Event::Cut) && editable {
                        self.ed.session.close_typing();
                        return self.exec("edit.deleteBackward", &Params::new(), sh, images, vh);
                    }
                }
                false
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let Some(chord) = chord_of(key, modifiers, self.platform) else {
                    return false;
                };
                let ids: Vec<&'static str> = self.keymap.lookup(&chord).to_vec();
                for id in ids {
                    let Some(spec) = self.ed.registry.get(id) else { continue };
                    if matches!(spec.run, markise_engine::editor::command::Run::Host) {
                        continue; // file / view commands: the app handles them
                    }
                    if !editable && matches!(spec.desc.effect, markise_doc::Effect::Mutates { .. }) {
                        continue;
                    }
                    if !self.command_enabled(id) {
                        if id == "review.addComment" {
                            // the shortcut says why instead of doing nothing (issue #90)
                            self.notice = Some(self.add_comment_refusal());
                        }
                        continue;
                    }
                    if ASKING_COMMANDS.contains(&id) {
                        self.app_command = Some(spec.desc.id);
                        return true;
                    }
                    return self.exec(id, &Params::new(), sh, images, vh);
                }
                false
            }
            _ => false,
        }
    }

    fn exec(
        &mut self,
        id: &str,
        params: &Params,
        sh: &mut dyn TextShaper,
        images: &dyn markise_layout::Images,
        vh: f32,
    ) -> bool {
        let r = self.ed.execute(id, params, sh, images, vh);
        // each command sees the reveal state of the previous one's caret
        self.ed.refresh_focus();
        if id == "review.addComment" {
            self.after_add_comment_result(&r);
        }
        r.is_ok() || matches!(r, Err(markise_engine::editor::command::CommandError::Disabled(_)))
    }
}

/// Runs `f` with the text shaper of the active backend.
pub(crate) fn with_shaper<R>(
    ctx: &egui::Context,
    parley: Option<&mut ParleyText>,
    f: impl FnOnce(&mut dyn TextShaper) -> R,
) -> R {
    match parley {
        Some(p) => f(&mut p.shaper),
        None => ctx.fonts_mut(|fonts| f(fonts)),
    }
}

/// The engine keymap's chord of an egui key press.
/// How a comment card looks (ADR 0012): a rounded card on the comment tint with a thin
/// accent border, a speech-bubble icon, the author (`@name:`) in bold, then the text as
/// written (plain, upright, wrapped, its line breaks kept).
struct CardLook {
    fill: Color32,
    accent: Color32,
    text: Color32,
    size: f32,
}

const CARD_PAD: f32 = 8.0;
const CARD_ICON: f32 = 18.0;

impl CardLook {
    fn new(theme: &Theme, ctx: &egui::Context) -> Self {
        let rc = theme.review_colors();
        let [pr, pg, pb, _] = theme.page_bg.0;
        let [tr, tg, tb, ta] = rc.comment_bg.0;
        // the comment tint over the page, opaque (the card hides what is under it); computed
        // on the straight (not premultiplied) theme colours
        let a = ta as f32 / 255.0 * 0.75;
        let mix = |p: u8, t: u8| (p as f32 * (1.0 - a) + t as f32 * a).round() as u8;
        let fill = Color32::from_rgb(mix(pr, tr), mix(pg, tg), mix(pb, tb));
        let _ = ctx;
        CardLook {
            fill,
            accent: color(theme.chrome.accent),
            text: color(theme.text),
            size: (theme.font_size * 0.82).clamp(11.0, 18.0),
        }
    }

    fn galley(&self, ctx: &egui::Context, text: &str, w: f32) -> Arc<egui::Galley> {
        let (author, body) = markise_doc::review::split_author(text);
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = (w - 2.0 * CARD_PAD - CARD_ICON).max(40.0);
        let body_fmt = egui::TextFormat {
            font_id: egui::FontId::proportional(self.size),
            color: self.text,
            ..Default::default()
        };
        if let Some(a) = author {
            let bold = egui::FontFamily::Name(markise_layout::FAMILY_BOLD.into());
            let has_bold = ctx.fonts(|f| f.families().contains(&bold));
            job.append(
                &format!("@{a}\n"),
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::new(self.size, if has_bold { bold } else { egui::FontFamily::Proportional }),
                    color: self.text,
                    ..Default::default()
                },
            );
        }
        // the author's prefix is shown above; the rest as written (keeps inner line breaks)
        let rest = if author.is_some() { body } else { text.trim() };
        job.append(if rest.is_empty() { "…" } else { rest }, 0.0, body_fmt);
        ctx.fonts_mut(|f| f.layout_job(job))
    }

    fn size(&self, galley: &egui::Galley, w: f32) -> Vec2 {
        let gw = galley.size().x + 2.0 * CARD_PAD + CARD_ICON;
        // at least 80 wide, unless the room itself is narrower (a tiny visible area)
        vec2(
            gw.min(w).max(80.0f32.min(w)),
            galley.size().y.max(CARD_ICON) + 2.0 * CARD_PAD,
        )
    }

    fn paint(&self, p: &egui::Painter, r: Rect, galley: Arc<egui::Galley>) {
        p.add(self.background(r, false));
        self.paint_icon(p, r);
        p.galley(r.min + vec2(CARD_PAD + CARD_ICON, CARD_PAD), galley, self.text);
    }

    /// The card's rounded background; `editing`: a stronger border (its text is a field).
    fn background(&self, r: Rect, editing: bool) -> Shape {
        let border = if editing {
            Stroke::new(1.5, self.accent)
        } else {
            Stroke::new(1.0, self.accent.gamma_multiply(0.35))
        };
        Shape::Rect(egui::epaint::RectShape::new(
            r,
            CornerRadius::same(6),
            self.fill,
            border,
            StrokeKind::Inside,
        ))
    }

    /// The small speech bubble in the card's top left corner.
    fn paint_icon(&self, p: &egui::Painter, r: Rect) {
        // a small speech bubble
        let o = r.min + vec2(CARD_PAD, CARD_PAD + 1.0);
        let b = Rect::from_min_size(o, vec2(12.0, 9.0));
        let stroke = Stroke::new(1.3, self.accent);
        p.rect_stroke(b, CornerRadius::same(2), stroke, StrokeKind::Middle);
        p.line_segment(
            [b.left_bottom() + vec2(3.0, 0.0), b.left_bottom() + vec2(2.5, 3.5)],
            stroke,
        );
        p.line_segment(
            [b.left_bottom() + vec2(2.5, 3.5), b.left_bottom() + vec2(6.0, 0.0)],
            stroke,
        );
    }
}

pub fn chord_of(key: egui::Key, modifiers: egui::Modifiers, platform: Platform) -> Option<Chord> {
    Some(Chord {
        ctrl: if platform == Platform::Mac {
            modifiers.ctrl
        } else {
            modifiers.ctrl || modifiers.command
        },
        alt: modifiers.alt,
        shift: modifiers.shift,
        meta: platform == Platform::Mac && modifiers.mac_cmd,
        key: key_name(key)?,
    })
}

/// The engine keymap's name of an egui key.
fn key_name(k: egui::Key) -> Option<String> {
    use egui::Key as K;
    Some(
        match k {
            K::ArrowLeft => "Left",
            K::ArrowRight => "Right",
            K::ArrowUp => "Up",
            K::ArrowDown => "Down",
            K::Enter => "Enter",
            K::Backspace => "Backspace",
            K::Delete => "Delete",
            K::Home => "Home",
            K::End => "End",
            K::PageUp => "PageUp",
            K::PageDown => "PageDown",
            K::Escape => "Escape",
            K::Tab => "Tab",
            K::Space => "Space",
            K::Slash => "/",
            K::Backslash => "\\",
            K::Equals => "=",
            K::Plus => "+",
            K::Minus => "-",
            K::Backtick => "`",
            K::OpenBracket => "[",
            K::CloseBracket => "]",
            K::Comma => ",",
            K::Period => ".",
            K::Semicolon => ";",
            _ => {
                let n = k.name();
                if n.len() > 1 && n.starts_with('F') && n[1..].parse::<u8>().is_ok() {
                    return Some(n.to_string()); // function keys (F11 Full Screen)
                }
                return (n.chars().count() == 1).then(|| n.to_uppercase());
            }
        }
        .to_string(),
    )
}

/// The list item ordinal of the task checkbox of `l` under item-local `local` (with a small
/// margin: the box is small).
fn task_box_hit(l: &BlockLayout, local: Pos2) -> Option<u16> {
    l.tasks
        .iter()
        .find(|(r, _)| r.expand(2.0).contains(local))
        .map(|&(_, li)| li)
}

fn hit(visible: &[markise_layout::Visible], p: Pos2, left: f32, top: f32) -> Option<(usize, Pos2, &BlockLayout)> {
    visible.iter().find_map(|v| {
        let y0 = top + v.y;
        (p.y >= y0 && p.y < y0 + v.layout.height).then(|| (v.idx, pos2(p.x - left, p.y - y0), &*v.layout))
    })
}

/// Dash segments one layout may paint per frame (dashes are expanded only inside the clip
/// rect, so this bounds pathological cases, not normal diagrams).
pub const MAX_DASHES_PER_LAYOUT: usize = 20_000;

/// Parameters `(t0, t1)` of the part of segment `a → b` inside `r` (Liang–Barsky), if any.
pub fn clip_segment(a: Pos2, b: Pos2, r: Rect) -> Option<(f32, f32)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [
        (-dx, a.x - r.min.x),
        (dx, r.max.x - a.x),
        (-dy, a.y - r.min.y),
        (dy, r.max.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((t0, t1))
}

/// Dashed polyline, expanded only where it crosses `clip`, with the dash phase measured from
/// the start of the path (so scrolling does not make dashes crawl). Returns the dashes drawn.
pub fn dashed_segments(pts: &[Pos2], dash: f32, gap: f32, clip: Rect, budget: &mut usize) -> Vec<[Pos2; 2]> {
    let (d, g) = (dash.max(0.5), gap.max(0.5));
    let period = d + g;
    let mut out = Vec::new();
    let mut dist = 0.0f32;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        if !(len > 0.0 && len.is_finite()) {
            continue;
        }
        if let Some((t0, t1)) = clip_segment(a, b, clip) {
            let dir = (b - a) / len;
            let (s0, s1) = (t0 * len, t1 * len);
            // start of the dash period containing s0, in path distance
            let mut s = s0 - (dist + s0).rem_euclid(period);
            while s < s1 {
                if *budget == 0 {
                    return out;
                }
                let (ds, de) = (s.max(s0), (s + d).min(s1));
                if de > ds {
                    out.push([a + dir * ds, a + dir * de]);
                    *budget -= 1;
                }
                s += period;
            }
        }
        dist += len;
    }
    out
}

fn paint_dashed(painter: &egui::Painter, pts: &[Pos2], stroke: Stroke, d: f32, g: f32, clip: Rect, budget: &mut usize) {
    for seg in dashed_segments(pts, d, g, clip.expand(stroke.width + 2.0), budget) {
        painter.line_segment(seg, stroke);
    }
}

/// Paints one shaped text block (epaint galley or parley block) with its anchor at `pos`.
pub(crate) fn paint_text_block(
    painter: &egui::Painter,
    ui: &Ui,
    text: &Arc<dyn markise_layout::TextBlock>,
    pos: Pos2,
    parley: Option<&mut ParleyText>,
) {
    if let Some(galley) = text.galley() {
        // Re-request the galley: egui's galley cache returns the same Arc while the font atlas
        // is unchanged, and a freshly rasterised one after an atlas rebuild.
        let g = painter.layout_job((*galley.job).clone());
        painter.galley(pos, g, Color32::PLACEHOLDER);
    } else if let (Some(block), Some(p)) = (text.as_any().downcast_ref::<ParleyBlock>(), parley) {
        p.atlas.paint(ui.ctx(), painter, block, pos);
    }
}

fn paint_layout(
    painter: &egui::Painter,
    ui: &Ui,
    layout: &BlockLayout,
    origin: Pos2,
    images: &mut TextureStore,
    requests: &mut Vec<String>,
    mut parley: Option<&mut ParleyText>,
) {
    let off = origin.to_vec2();
    let clip_rect = painter.clip_rect();
    let mut dash_budget = MAX_DASHES_PER_LAYOUT;
    for prim in &layout.prims {
        match prim {
            Prim::Text { pos, text, .. } => {
                let r = text.rect().translate(pos.to_vec2() + off);
                if !r.intersects(clip_rect) {
                    continue;
                }
                paint_text_block(painter, ui, text, *pos + off, parley.as_deref_mut());
            }
            Prim::Rect {
                rect,
                fill,
                stroke,
                radius,
            } => {
                painter.rect(rect.translate(off), *radius, *fill, *stroke, StrokeKind::Inside);
            }
            Prim::Line { a, b, stroke } => {
                painter.line_segment([*a + off, *b + off], *stroke);
            }
            Prim::Circle {
                center,
                radius,
                fill,
                stroke,
            } => {
                painter.circle(*center + off, *radius, *fill, *stroke);
            }
            Prim::Path {
                points,
                closed,
                fill,
                stroke,
                dash,
            } => {
                let pts: Vec<Pos2> = points.iter().map(|p| *p + off).collect();
                let bb = Rect::from_points(&pts).expand(stroke.width + 1.0);
                if !bb.intersects(clip_rect) {
                    continue;
                }
                if *closed && *fill != Color32::TRANSPARENT && pts.len() >= 3 {
                    painter.add(egui::Shape::convex_polygon(pts.clone(), *fill, Stroke::NONE));
                }
                if stroke.width > 0.0 && stroke.color != Color32::TRANSPARENT {
                    match dash {
                        Some([d, g]) => {
                            let mut v = pts;
                            if *closed && let Some(&first) = v.first() {
                                v.push(first);
                            }
                            paint_dashed(painter, &v, *stroke, *d, *g, clip_rect, &mut dash_budget);
                        }
                        None if *closed => {
                            painter.add(egui::Shape::closed_line(pts, *stroke));
                        }
                        None => {
                            painter.add(egui::Shape::line(pts, *stroke));
                        }
                    }
                }
            }
            Prim::Checkbox {
                rect,
                checked,
                border,
                fill,
            } => {
                let r = rect.translate(off);
                if *checked {
                    painter.rect(r, 3.0, *fill, Stroke::NONE, StrokeKind::Inside);
                    let s = Stroke::new(1.8, Color32::WHITE);
                    let a = pos2(r.left() + r.width() * 0.22, r.top() + r.height() * 0.52);
                    let b = pos2(r.left() + r.width() * 0.43, r.top() + r.height() * 0.72);
                    let c = pos2(r.left() + r.width() * 0.78, r.top() + r.height() * 0.30);
                    painter.line_segment([a, b], s);
                    painter.line_segment([b, c], s);
                } else {
                    painter.rect(
                        r,
                        3.0,
                        Color32::TRANSPARENT,
                        Stroke::new(1.2, *border),
                        StrokeKind::Inside,
                    );
                }
            }
            Prim::Math { rect, key, query, clip } => {
                if !rect.translate(off).intersects(clip_rect) {
                    continue;
                }
                if let Some(tex) = images.math_texture(ui.ctx(), *key, query) {
                    // snap to the pixel grid: the bitmap is rendered 1:1 for this scale
                    let ppp = ui.ctx().pixels_per_point();
                    let min = rect.min + off;
                    let min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
                    // table cells clip their formulas
                    let p = match clip {
                        Some(c) => painter.with_clip_rect(c.translate(off).intersect(clip_rect)),
                        None => painter.clone(),
                    };
                    p.image(
                        tex.id(),
                        Rect::from_min_size(min, rect.size()),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            Prim::Image { rect, src, placeholder } => {
                let r = rect.translate(off);
                match images.texture(src) {
                    Some(tex) => {
                        painter.image(
                            tex.id(),
                            r,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        painter.rect_filled(r, 4.0, *placeholder);
                        requests.push(src.clone());
                    }
                }
            }
        }
    }
    let _ = ui;
}

#[cfg(test)]
mod dash_tests {
    use super::*;

    /// Codex P1: a very long dashed lifeline expands only its visible part.
    #[test]
    fn dashes_are_clipped_to_the_visible_rect() {
        let pts = [pos2(10.0, -500_000.0), pos2(10.0, 600_000.0)];
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));
        let mut budget = MAX_DASHES_PER_LAYOUT;
        let segs = dashed_segments(&pts, 3.0, 3.0, clip, &mut budget);
        assert!(segs.len() <= 101, "{}", segs.len());
        assert!(segs.len() >= 99);
        assert!(segs.iter().all(|s| s[0].y >= 0.0 && s[1].y <= 600.0));
        // phase follows the path, not the clip: dashes start at multiples of 6 from the origin
        for s in &segs {
            let from_start = s[0].y + 500_000.0;
            let r = from_start.rem_euclid(6.0);
            assert!(r < 1e-2 || s[0].y == 0.0, "{r}");
        }
        // outside the clip: nothing
        let far = [pos2(5000.0, 0.0), pos2(5000.0, 100.0)];
        assert!(dashed_segments(&far, 3.0, 3.0, clip, &mut budget).is_empty());
    }

    #[test]
    fn dash_budget_is_enforced() {
        let pts = [pos2(0.0, 0.0), pos2(0.0, 1.0e6)];
        let clip = Rect::from_min_max(pos2(-1.0, -1.0), pos2(1.0, 1.0e6));
        let mut budget = 50;
        assert_eq!(dashed_segments(&pts, 1.0, 1.0, clip, &mut budget).len(), 50);
        assert_eq!(budget, 0);
    }
}

/// PROTOTYPE (block forms): a theme color as an egui color.
fn theme_color(c: markise_theme::Rgba) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

impl DocViewer {
    /// PROTOTYPE (live mode): a block image is one unit for the caret — when the selection's
    /// focus lands inside a block (arrows, clicks, edits) it moves to the caret stop after the
    /// block when it came from before, else to the stop before it. Returns true when moved.
    fn keep_caret_out_of_blocks(&mut self, prev: usize) -> bool {
        let sel = self.ed.session.selection().primary();
        let f = sel.focus.offset;
        // (the card's lines as of the last frame: moved by the edits since)
        let mv = |o: usize| self.ed.map_recent(o).unwrap_or(o);
        let markers: Vec<_> = self
            .native_markers
            .iter()
            .map(|a| (mv(a.0), mv(a.1), mv(a.2), mv(a.3)))
            .collect();
        let Some(&(start, end, before, after)) =
            self.atomic.iter().chain(markers.iter()).find(|a| f >= a.0 && f <= a.1)
        else {
            return false;
        };
        if self.focused_block == Some(start) && near_start(self.ed.session.text(), after, f) {
            return false;
        }
        let to = if prev <= start || prev > end {
            if prev <= start { after } else { before }
        } else {
            after
        };
        let new = if sel.anchor == sel.focus {
            Selection::caret(to)
        } else {
            Selection::single(sel.anchor, DocPos::new(to))
        };
        self.ed.set_selection(new);
        true
    }
}

/// PROTOTYPE (block packages): whether `off` lies strictly inside a `:::` block.
fn in_block(text: &str, off: usize) -> bool {
    markise_engine::block_form::all_blocks(text)
        .iter()
        .any(|b| off > b.range.start && off < b.range.end)
}

/// PROTOTYPE (block packages): the insertion point for the caret: its line when it is empty
/// (replaced), else after the caret's paragraph.
fn caret_target(text: &str, c: usize, rev: u64) -> crate::block_edit::InsertTarget {
    let c = c.min(text.len());
    let ls = text[..c].rfind('\n').map_or(0, |i| i + 1);
    let le = text[c..].find('\n').map_or(text.len(), |i| c + i);
    if text[ls..le].trim().is_empty() || text[ls..le].trim() == "/" {
        return crate::block_edit::InsertTarget::new(text, ls, le, rev);
    }
    // end of the paragraph: the first blank line after the caret
    let end = text[c..].find("\n\n").map_or(text.len(), |i| c + i);
    crate::block_edit::InsertTarget::new(text, end, end, rev)
}

/// PROTOTYPE (block packages): "use this block library?" — Some(true/false) once answered.
fn consent_dialog(ctx: &egui::Context, b: &crate::block_lib::Binding) -> Option<bool> {
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("markise-block-lib-consent")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading("Библиотека блоков");
        ui.add_space(6.0);
        let src = b
            .local_source()
            .map(|p| p.display().to_string())
            .unwrap_or(b.source.clone());
        ui.label(format!(
            "Папка использует библиотеку блоков «{}» v{} из {}. Подключить?",
            b.library, b.version, src
        ));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(if b.is_folder() {
                "Папка разработки: код блоков будет выполняться при показе. Обновления — вручную."
            } else {
                "Архив будет проверен (sha256) и распакован в кеш; дальше работает без сети. Автообновлений нет."
            })
            .small()
            .weak(),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Подключить").clicked() {
                answer = Some(true);
            }
            if ui.button("Не сейчас").clicked() {
                answer = Some(false);
            }
        });
    });
    answer
}

/// PROTOTYPE (live mode): `c` is the first caret stop of the item starting at `start` (only
/// hidden markup such as `## `, `> `, `- ` lies between them).
fn near_start(text: &str, start: usize, c: usize) -> bool {
    c >= start
        && c - start <= 8
        && text
            .get(start..c)
            .is_some_and(|m| m.chars().all(|ch| "#>-*+ 0123456789.[]xX".contains(ch)))
}

impl DocViewer {
    /// PROTOTYPE (block move): top-level units of the document: items outside blocks and whole
    /// `:::` blocks, as (start, end).
    fn top_units(&self) -> Vec<(usize, usize)> {
        let text = self.ed.session.text();
        let blocks = markise_engine::block_form::all_blocks(text);
        let model = self.ed.view.model().clone();
        let mut out = Vec::new();
        for (st, it) in model.items.iter_with_start() {
            if it.len == 0 || blocks.iter().any(|b| st > b.range.start && st < b.range.end) {
                continue;
            }
            match blocks.iter().find(|b| b.range.start == st) {
                Some(b) => out.push((b.range.start, b.range.end)),
                None => out.push((st, st + it.len)),
            }
        }
        out
    }

    /// PROTOTYPE (block move): moves the block at `start` to the insertion point `dest` (the end
    /// of a top-level item) as one undo step «Переместить блок». Returns the new start.
    pub fn move_block(&mut self, start: usize, dest: usize) -> Option<usize> {
        let text = self.ed.session.text();
        let b = markise_engine::block_form::block_at(text, start)?;
        let end = b.range.end;
        // the block and the blank lines after it, up to the next text
        let after = end + text[end..].len() - text[end..].trim_start_matches(['\n', '\r']).len();
        if (start..=after).contains(&dest) || !self.top_units().iter().any(|u| u.1 == dest) {
            return None;
        }
        let block = text[start..end].to_string();
        let ps = PatchSet::new(vec![
            markise_doc::Patch::verbatim(start..after, ""),
            markise_doc::Patch::verbatim(dest..dest, format!("\n\n{block}")),
        ])
        .ok()?;
        self.ed
            .edit(|s| s.apply_external(&ps, "Переместить блок", false))
            .ok()?;
        let new_start = if dest > start {
            dest - (after - start) + 2
        } else {
            dest + 2
        };
        let t = self.ed.session.text();
        let next = t[new_start + block.len()..]
            .find(|c: char| c != '\n' && c != '\r')
            .map_or(t.len(), |i| new_start + block.len() + i);
        self.ed.set_selection(Selection::caret(next));
        self.focused_block = Some(new_start);
        self.last_focus = next;
        Some(new_start)
    }

    /// PROTOTYPE (block move): ⌥⌘↑ / ⌥⌘↓ — past the neighbouring top-level item.
    pub fn move_block_step(&mut self, start: usize, up: bool) -> Option<usize> {
        let units = self.top_units();
        let u = units.iter().position(|x| x.0 == start)?;
        if up {
            let prev = *units.get(u.checked_sub(1)?)?;
            // insert before the previous unit = after the unit before it (or at the top)
            let text = self.ed.session.text();
            let b = markise_engine::block_form::block_at(text, start)?;
            let end = b.range.end;
            let after = end + text[end..].len() - text[end..].trim_start_matches(['\n', '\r']).len();
            let block = text[start..end].to_string();
            let ps = PatchSet::new(vec![
                markise_doc::Patch::verbatim(prev.0..prev.0, format!("{block}\n\n")),
                markise_doc::Patch::verbatim(start..after, ""),
            ])
            .ok()?;
            self.ed
                .edit(|s| s.apply_external(&ps, "Переместить блок", false))
                .ok()?;
            let new_start = prev.0;
            let t = self.ed.session.text();
            let next = t[new_start + block.len()..]
                .find(|c: char| c != '\n' && c != '\r')
                .map_or(t.len(), |i| new_start + block.len() + i);
            self.ed.set_selection(Selection::caret(next));
            self.focused_block = Some(new_start);
            self.last_focus = next;
            Some(new_start)
        } else {
            let next = *units.get(u + 1)?;
            self.move_block(start, next.1)
        }
    }
}
