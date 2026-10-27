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
use markise_doc::{DocPos, DocumentFormat, FormatDoc, FormatOptions, Params, Selection};
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
}

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
        match self.session.take_change() {
            DocChange::None => {
                // the view may still hold the placeholder after a no-op edit
                if !Arc::ptr_eq(self.view.doc(), self.session.doc_arc()) {
                    self.view.replace_doc(self.session.doc_arc().clone());
                }
            }
            DocChange::Edited(e) => {
                let ch = self.view.model_mut().apply(self.session.doc(), &e);
                self.view.apply_change(self.session.doc_arc().clone(), &ch);
            }
            DocChange::Replaced => {
                let model = DocModel::build(self.session.doc(), self.path.as_deref());
                self.view.set_model(self.session.doc_arc().clone(), model);
            }
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
        let r = {
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
        };
        if matches!(
            self.registry.get(id).map(|c| c.desc.effect),
            Some(markise_doc::Effect::View)
        ) {
            self.release();
            self.session.expire_hints_left_behind();
        }
        self.sync();
        r
    }

    /// Updates the view's focus (reveal set, virtual paragraph) for the current selection.
    /// Returns `true` when the focused item's layout input changed.
    pub fn refresh_focus(&mut self) -> bool {
        if !self.reveal {
            // frozen (pointer drag): the markers shown stay as they are, so the geometry under
            // the pointer does not shift mid-selection (ADR 0006 §2.4, M2b-6)
            return false;
        }
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
        let sel = self.session.selection().primary();
        let mut g = self.geometry(shaper, images);
        let ok = |g: &mut Geometry<'_>, p: DocPos| g.caret_rect(p).is_some() && g.normalize(p).offset == p.offset;
        if ok(&mut g, sel.focus) && (sel.is_caret() || ok(&mut g, sel.anchor)) {
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
        if &sel == self.session.selection() {
            return;
        }
        self.edit(|s| {
            s.set_selection(sel);
            s.expire_hints_left_behind();
        });
    }
}
