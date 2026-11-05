//! The editing surface (ADR 0006 §2.5, M2b): an [`EditorSession`] together with the layout
//! [`View`] that shows it. UI-free and headless-testable (CPU test fonts): the egui front end
//! feeds it commands and paints what it lays out.
//!
//! Per command: caret commands run against the view's caret geometry; mutating commands first
//! release the view's reference to the document (so the session edits it in place instead of
//! copying a 10 MB document), then the session's change summary updates the view model
//! incrementally ([`DocModel::apply`] + [`View::apply_change`]) and the focus (marker reveal,
//! virtual paragraph) follows the caret.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use epaint::Rect;
use markise_doc::{DocPos, DocumentFormat, EditSummary, FormatDoc, FormatOptions, Params, Selection};
use markise_layout::caret::Geometry;
use markise_layout::{DocModel, Images, TextShaper, View};

use super::command::{CommandError, Registry};
use super::{DocChange, EditorSession};

/// Caret geometry the caret commands need (ADR 0006 §2.5 `CaretGeometry`).
pub trait CaretGeometry {
    /// One grapheme (or word) to the left/right, crossing blocks.
    fn horizontal(&mut self, pos: DocPos, forward: bool, word: bool) -> DocPos;
    /// One visual row up/down at `goal_x`; returns the goal x to keep.
    fn vertical(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32);
    /// Start or end of the visual row (soft-wrap affinity).
    fn row_edge(&mut self, pos: DocPos, end: bool) -> DocPos;
    /// One viewport up/down.
    fn page(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32);
    /// First or last caret stop of the document.
    fn doc_edge(&mut self, end: bool) -> DocPos;
    /// The command is about to edit the document: drop references the geometry holds.
    fn release_doc(&mut self) {}
}

/// [`CaretGeometry`] over a [`View`].
pub struct ViewCaret<'a> {
    pub geo: Geometry<'a>,
    /// Viewport height (Page Up/Down).
    pub page: f32,
    /// Stand-in document the view holds while the session edits the real one.
    pub placeholder: Option<&'a Arc<dyn FormatDoc>>,
}

impl CaretGeometry for ViewCaret<'_> {
    fn horizontal(&mut self, pos: DocPos, forward: bool, word: bool) -> DocPos {
        self.geo.horizontal(pos, forward, word)
    }
    fn vertical(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32) {
        self.geo.vertical(pos, goal_x, down)
    }
    fn row_edge(&mut self, pos: DocPos, end: bool) -> DocPos {
        self.geo.row_edge(pos, end)
    }
    fn page(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32) {
        self.geo.page(pos, goal_x, down, (self.page - 40.0).max(40.0))
    }
    fn doc_edge(&mut self, end: bool) -> DocPos {
        self.geo.doc_edge(end)
    }
    fn release_doc(&mut self) {
        if let Some(p) = self.placeholder {
            self.geo.view.replace_doc(p.clone());
        }
    }
}

/// What [`EditorView::continue_with`] did (timing log, tests).
#[derive(Clone, Copy, Debug)]
pub struct Continued {
    /// Edits made in the first part were carried over.
    pub edited: bool,
    /// Time spent carrying the edits over …
    pub merge: std::time::Duration,
    /// … and in all.
    pub total: std::time::Duration,
}

/// A session and the view showing it.
pub struct EditorView {
    pub session: EditorSession,
    pub view: View,
    pub registry: Registry,
    path: Option<PathBuf>,
    placeholder: Arc<dyn FormatDoc>,
    /// `false` freezes the current reveal state (while a pointer drag selects, ADR 0006 §2.4);
    /// [`EditorView::refresh_focus`] recomputes it once this is `true` again.
    pub reveal: bool,
    /// The source editor (Source and Side by Side modes; M2d), kept in step by
    /// [`EditorView::sync`].
    pub source: Option<super::SourcePane>,
    /// The document's edits since [`EditorView::clear_recent_edits`] (`None`: replaced, or more
    /// than [`RECENT_MAX`] — the rendered surface was not drawn meanwhile, e.g. Source mode):
    /// the UI maps positions it kept from its last frame through them
    /// ([`EditorView::map_recent`]).
    recent: Option<Vec<EditSummary>>,
    /// Document changes so far (a pressed checkbox is cancelled when the document changed
    /// while it was held).
    changes: u64,
    /// Editing blocks' fields where they stand ([`super::inplace`]).
    pub(super) place: super::inplace::InPlace,
}

/// Recent edits kept for [`EditorView::map_recent`]; beyond them positions kept from the last
/// frame are dropped.
const RECENT_MAX: usize = 64;

impl EditorView {
    /// Wraps `session`, showing it in `view` (built for the session's document).
    pub fn new(session: EditorSession, view: View, path: Option<PathBuf>) -> Self {
        let placeholder: Arc<dyn FormatDoc> =
            Arc::from(session.format().parse(String::new(), &FormatOptions::default()));
        let registry = Registry::for_format(session.format().as_ref());
        let mut e = EditorView {
            session,
            view,
            registry,
            path,
            placeholder,
            reveal: true,
            source: None,
            recent: Some(Vec::new()),
            changes: 0,
            place: Default::default(),
        };
        e.sync();
        e
    }

    /// Opens `text` headlessly (tests, benches): a fresh session, model and view.
    pub fn open(
        format: Arc<dyn DocumentFormat>,
        text: impl Into<String>,
        theme: markise_theme::Theme,
        width: f32,
    ) -> Self {
        let session = EditorSession::new(format, text);
        let model = DocModel::build(session.doc(), None);
        let view = View::new(session.doc_arc().clone(), model, theme, width, 1.0);
        Self::new(session, view, None)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    /// Brings the view up to the session's document (after any session change).
    pub fn sync(&mut self) {
        let steps = self.session.take_steps();
        match self.session.take_change() {
            DocChange::None => {
                // the view may still hold the placeholder after a no-op edit
                if !Arc::ptr_eq(self.view.doc(), self.session.doc_arc()) {
                    self.view.replace_doc(self.session.doc_arc().clone());
                }
            }
            DocChange::Edited(e) => {
                self.changes += 1;
                if let Some(r) = &mut self.recent {
                    r.push(e.clone());
                }
                if self.recent.as_ref().is_some_and(|r| r.len() > RECENT_MAX) {
                    self.recent = None;
                }
                let mut ch = self.view.model_mut().apply(self.session.doc(), &e);
                // the edits one by one, not their covering range (a position between two of
                // them only moved)
                if ch.moved.is_some()
                    && let Some(steps) = steps
                {
                    ch.moved = Some(markise_layout::Moved::from_steps(steps));
                }
                self.view.apply_change(self.session.doc_arc().clone(), &ch);
                if let Some(p) = self.source.as_mut() {
                    p.view.apply(self.session.doc().text(), &e);
                }
                // a change made elsewhere (the rendered pane, Undo/Redo) moves the source
                // selection through its patches (live marks); source edits set it afterwards
                self.source_load_selection();
            }
            DocChange::Replaced => {
                self.changes += 1;
                self.recent = None;
                // the same kind of model as before (paged or full)
                let paging = self.view.model().paging();
                let model = Arc::new(DocModel::build_with(self.session.doc(), self.path.as_deref(), paging));
                self.view.set_model(self.session.doc_arc().clone(), model);
                if let Some(p) = self.source.as_mut() {
                    p.view.rebuild(self.session.doc().text());
                }
                self.source_load_selection();
            }
        }
        if !self.place_is_idle() {
            self.follow_sticky();
            self.follow_block_modes();
        }
        self.pin_caret();
    }

    /// Progressive open (issue #126): the session holds the first `cut` bytes of a file's
    /// text (and the edits made since); `doc` / `model` are the whole file as read. The edits
    /// are applied to `doc` — one verbatim patch set, the diff of the session's text against
    /// the original part, with the session's literal hints; `model` follows incrementally —
    /// and the result replaces the session's document without an undo step
    /// ([`EditorSession::adopt_continuation`]) and the view's model without moving the content
    /// on screen ([`View::set_model_keep`]).
    pub fn continue_with(
        &mut self,
        mut doc: Arc<dyn FormatDoc>,
        mut model: Arc<DocModel>,
        cut: usize,
    ) -> Result<Continued, String> {
        let t = std::time::Instant::now();
        let text = doc.text();
        if cut > text.len() || !text.is_char_boundary(cut) {
            return Err(format!("the first part ({cut} bytes) is not a prefix of the file"));
        }
        let cur = self.session.doc_arc().clone();
        let edited = cur.text() != &text[..cut];
        let hints = cur.hints().clone();
        if edited || !hints.is_empty() {
            let patches: Vec<markise_doc::Patch> = crate::edit::patches(&text[..cut], cur.text())
                .into_iter()
                .map(|p| markise_doc::Patch::verbatim(p.start..p.end, p.insert))
                .collect();
            let ps = markise_doc::PatchSet::new(patches).map_err(|e| e.to_string())?;
            if Arc::get_mut(&mut doc).is_none() {
                doc = Arc::from(doc.clone_box());
            }
            let d = Arc::get_mut(&mut doc).expect("unique document");
            let e = if edited {
                d.apply_with_hints(&ps, (!hints.is_empty()).then_some(&hints))
            } else {
                d.set_hints(&hints)
            };
            Arc::make_mut(&mut model).apply(&*doc, &e);
        }
        let shown = cur.text().len();
        drop(cur);
        let merge = t.elapsed();
        let sel = self.session.selection();
        let keep: Vec<usize> = sel
            .ranges()
            .iter()
            .flat_map(|r| [r.anchor.offset, r.focus.offset])
            .collect();
        self.release();
        self.session.adopt_continuation(doc.clone());
        self.view.set_model_keep(doc, model, shown, &keep);
        if let Some(p) = self.source.as_mut() {
            p.view.rebuild(self.session.doc().text());
        }
        // (positions in the text shown so far stay valid: `recent` and `changes` are kept, so
        // hit tests from the last frame and a pressed checkbox survive)
        self.source_load_selection();
        self.pin_caret();
        Ok(Continued {
            edited,
            merge,
            total: t.elapsed(),
        })
    }

    /// The selection reaches into a block shown natively without holding it whole: what edits
    /// the text there is its fields, and anything else has to ask ([`super::inplace`]).
    pub fn in_native_block(&self) -> bool {
        let p = self.session.selection().primary();
        let r = p.range();
        [p.anchor, p.focus].into_iter().any(|at| {
            self.view
                .native_block(at.offset)
                // the virtual paragraph after a closer shares its end, and is outside the block
                .filter(|n| at.virt == 0 || at.offset < n.range.end)
                .is_some_and(|n| r.start > n.range.start || r.end < n.range.end)
        })
    }

    /// Document changes so far (edits, replacements).
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Forgets the recent edits (the UI took a fresh look at the document: its last frame).
    pub fn clear_recent_edits(&mut self) {
        self.recent = Some(Vec::new());
    }

    /// Source position `at` of the document as it was at [`EditorView::clear_recent_edits`],
    /// in the current document: moved by the edits since, `None` when an edit changed the text
    /// there (or the document was replaced). For positions the UI keeps from its last frame
    /// (hit geometry, a pressed checkbox) when keys edit the text before the pointer is handled.
    pub fn map_recent(&self, at: usize) -> Option<usize> {
        let mut at = at;
        for e in self.recent.as_ref()? {
            if at >= e.changed.end {
                at = (at as isize + e.delta) as usize;
            } else if at >= e.changed.start && !e.changed.is_empty() {
                return None;
            }
        }
        Some(at)
    }

    /// Paged models (issue #125): the pages of the caret and of both ends of every selection
    /// range, and the items around them, stay materialized (not while the view holds a
    /// placeholder document during an edit): caret geometry and selection fixing never see a
    /// placeholder at a selection end.
    fn pin_caret(&mut self) {
        if self.view.model().is_paged() && Arc::ptr_eq(self.view.doc(), self.session.doc_arc()) {
            let sel = self.session.selection();
            let mut pins: Vec<usize> = vec![sel.primary().focus.offset, sel.primary().anchor.offset];
            for r in sel.ranges() {
                pins.extend([r.anchor.offset, r.focus.offset]);
            }
            pins.sort_unstable();
            pins.dedup();
            self.view.set_pinned_offsets(&pins);
        }
    }

    /// Releases the view's document reference before an edit (see [`ViewCaret::release_doc`]).
    pub fn release(&mut self) {
        self.view.replace_doc(self.placeholder.clone());
    }

    /// Runs a session operation that may edit the document, then syncs the view.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut EditorSession) -> R) -> R {
        self.release();
        let r = f(&mut self.session);
        self.sync();
        r
    }

    /// Runs command `id` (any kind) and syncs the view. `page`: viewport height.
    pub fn execute(
        &mut self,
        id: &str,
        params: &Params,
        shaper: &mut dyn TextShaper,
        images: &dyn Images,
        page: f32,
    ) -> Result<(), CommandError> {
        if let Some(done) = self.field_command(id, params, shaper, images) {
            return done;
        }
        let before = if id.starts_with("format.") {
            self.model_for_marks()
        } else {
            None
        };
        let mut r = self.run_command(id, params, shaper, images, page);
        if matches!(
            self.registry.get(id).map(|c| c.desc.effect),
            Some(markise_doc::Effect::View)
        ) {
            // a motion goes on past what is no place for the caret in a block's fields
            for _ in 0..4 {
                if !self.in_stray_gap() {
                    break;
                }
                r = self.run_command(id, params, shaper, images, page);
            }
            let sel = self.field_selection(self.session.selection().clone());
            self.release();
            if &sel != self.session.selection() {
                self.session.set_selection(sel);
            }
            self.session.expire_hints_left_behind();
        }
        self.sync();
        self.check_marks(before);
        r
    }

    fn run_command(
        &mut self,
        id: &str,
        params: &Params,
        shaper: &mut dyn TextShaper,
        images: &dyn Images,
        page: f32,
    ) -> Result<(), CommandError> {
        let mut g = ViewCaret {
            geo: Geometry {
                view: &mut self.view,
                shaper,
                images,
            },
            page,
            placeholder: Some(&self.placeholder),
        };
        self.registry.execute_with(&mut self.session, &mut g, id, params)
    }

    /// Updates the view's focus (reveal set, virtual paragraph) for the current selection.
    /// Returns `true` when the focused item's layout input changed.
    pub fn refresh_focus(&mut self) -> bool {
        if !self.reveal {
            // frozen (pointer drag): the markers shown stay as they are, so the geometry under
            // the pointer does not shift mid-selection (ADR 0006 §2.4, M2b-6)
            return false;
        }
        self.pin_caret();
        let focus = self.session.selection().primary().focus;
        let f = markise_layout::caret::focus_for(&self.view, focus, self.reveal);
        self.view.set_focus(f)
    }

    /// Geometry over the view (hit testing, caret rectangles) for the UI.
    pub fn geometry<'a>(&'a mut self, shaper: &'a mut dyn TextShaper, images: &'a dyn Images) -> Geometry<'a> {
        Geometry {
            view: &mut self.view,
            shaper,
            images,
        }
    }

    /// Caret rectangle in absolute document coordinates (y from the document top).
    pub fn caret_rect(&mut self, shaper: &mut dyn TextShaper, images: &dyn Images) -> Option<Rect> {
        let focus = self.session.selection().primary().focus;
        if self.view.model().items.is_empty() {
            // an empty document: the caret at the top of the column
            let t = self.view.theme();
            let h = t.font_size * t.line_height;
            return Some(epaint::Rect::from_min_size(
                epaint::pos2(0.0, 0.0),
                epaint::vec2(0.0, h),
            ));
        }
        let mut g = self.geometry(shaper, images);
        let (i, r) = g.caret_rect(focus)?;
        let y = self.view.item_y(i) as f32;
        Some(r.translate(epaint::vec2(0.0, y)))
    }

    /// Moves the selection ends to caret stops when they are no longer on one — the available
    /// stops changed under them (a diagram or formula became rendered, front matter collapsed,
    /// a view-mode change): each end goes to the nearest stop after the object. No history
    /// entry. Returns `true` when the selection changed.
    pub fn fix_selection(&mut self, shaper: &mut dyn TextShaper, images: &dyn Images) -> bool {
        // paged models: both ends are materialized before they are checked (an end in a
        // placeholder would be "normalized" to another caret stop, changing the selection)
        self.pin_caret();
        let sel = self.session.selection().primary();
        // (a selection holding a native block whole ends at its edge, which has no caret stop)
        let (focus_edge, anchor_edge) = (self.is_native_edge(sel.focus), self.is_native_edge(sel.anchor));
        let mut g = self.geometry(shaper, images);
        // whitespace after the text of a paragraph, heading or cell is laid out only while the
        // focus is in it: a selection end there (e.g. the anchor of Shift+← from it) is still a
        // valid position, not one to move past into the next block (issue #16)
        let in_trailing_ws = |g: &Geometry<'_>, p: DocPos| {
            markise_layout::caret::focus_for(g.view, p, true).is_some_and(|(_, f)| f.trailing_ws.is_some())
        };
        // an end in a page placeholder (not materialized) is left alone: there is no layout
        // to check it against
        let in_page = |g: &Geometry<'_>, p: DocPos| {
            let m = g.view.model();
            m.is_paged() && m.is_page(m.item_at_offset(p.offset))
        };
        let ok = |g: &mut Geometry<'_>, p: DocPos| {
            in_page(g, p)
                || in_trailing_ws(g, p)
                || if p.virt > 0 {
                    // a virtual paragraph (its line is laid out once the focus is on it)
                    g.normalize(p) == p
                } else {
                    g.caret_rect(p).is_some() && g.normalize(p).offset == p.offset
                }
        };
        if (focus_edge || ok(&mut g, sel.focus)) && (sel.is_caret() || anchor_edge || ok(&mut g, sel.anchor)) {
            return false;
        }
        // inside an object (a rendered formula): the stop after it
        let after = |g: &mut Geometry<'_>, p: DocPos| {
            let n = g.normalize(p);
            if n.offset < p.offset {
                let m = g.horizontal(n, true, false);
                if m.offset >= p.offset {
                    return m;
                }
            }
            n
        };
        let f = after(&mut g, sel.focus);
        let a = if sel.is_caret() { f } else { after(&mut g, sel.anchor) };
        let next = Selection::single(a, f);
        if &next == self.session.selection() {
            return false;
        }
        self.edit(|s| s.set_selection(next));
        true
    }

    /// Scrolls the view so that the caret is visible in a viewport `vh` tall (with a margin).
    pub fn scroll_to_caret(&mut self, vh: f32, shaper: &mut dyn TextShaper, images: &dyn Images) {
        self.pin_caret();
        let Some(r) = self.caret_rect(shaper, images) else {
            return;
        };
        let top = self.view.scroll_y() as f32;
        let margin = (vh * 0.1).min(48.0);
        if r.min.y < top + margin {
            self.view.scroll_to_y((r.min.y - margin).max(0.0) as f64);
        } else if r.max.y > top + vh - margin {
            self.view.scroll_to_y((r.max.y - vh + margin) as f64);
        }
    }

    /// Places the caret (click), or extends the selection to `pos` (shift-click, drag).
    pub fn set_caret(&mut self, pos: DocPos, extend: bool) {
        let sel = if extend {
            Selection::single(self.session.selection().primary().anchor, pos)
        } else {
            Selection::caret_at(pos)
        };
        self.set_selection(sel);
    }

    /// Sets the selection (a caret jump: closes the typing group, expires literal hints left
    /// behind).
    pub fn set_selection(&mut self, sel: Selection) {
        let sel = self.field_selection(sel);
        if &sel == self.session.selection() {
            return;
        }
        self.edit(|s| {
            s.set_selection(sel);
            s.expire_hints_left_behind();
        });
    }
}
