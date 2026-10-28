//! The source editor on the editing engine (spec 02 §12.1 / §12.1.1, ADR 0006 §4.3, §5.5; M2d).
//!
//! Full Source mode and the source pane of Side by Side edit the same [`EditorSession`] as the
//! rendered view; the text is shown by a [`SourceView`] (layout crate) kept in step with every
//! document change by [`EditorView::sync`].
//!
//! - **Verbatim edits** (99 C58): source typing, deletion, paste and line breaks are plain
//!   patches of the bytes the user sees — no input rules, no line-ending conversion of typed
//!   or pasted text. A typed line break repeats the terminator of the line it splits (a CRLF
//!   line gets CRLF), so a uniformly CRLF document stays CRLF; the last line without a
//!   terminator takes the document's line ending.
//! - **History** (ADR 0006 §4.3): Side by Side edits are ordinary transactions of the single
//!   live history (typing groups as in the rendered view). Full Source mode opens a *source
//!   session* ([`EditorSession::begin_source_session`]): undo cannot go below the state it was
//!   entered in, the redo stack of the rendered view is set aside, and on exit
//!   ([`EditorSession::end_source_session`]) the transactions made in the session are collapsed
//!   into "Source Edit" transactions — one per segment between saved states (a state that was
//!   ever saved stays a boundary, so undo reaches every saved state and reads clean there).
//!   A session without edits leaves the history untouched.
//! - **Commands**: caret and selection commands run through the shared command registry with
//!   the source view as [`CaretGeometry`]; Undo/Redo use the session; typing, Backspace/Delete
//!   (grapheme, word, to line start), Enter, Tab/Shift+Tab (indent unit / line indent) are the
//!   source editor's own verbatim versions. Format and block commands are not available in the
//!   source pane yet (M2d slice 2).
//! - **Selection**: the source pane keeps its own selection and hands it to the session before
//!   each edit (the rendered pane of Side by Side may hold another one).

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use epaint::{Pos2, Rect};
use markise_doc::{DocPos, FormatDoc, HintSet, Params, Patch, PatchSet, PlanStep, Selection};
use markise_layout::source::{self as src, SourceCtx, SourceParams, SourceView, SourceVisible};
use markise_layout::{Highlights, TextShaper};

use super::command::CommandError;
use super::history::{Transaction, TxKind, TypingDir};
use super::{CaretGeometry, EditError, EditorSession, EditorView};

/// Bookkeeping of an open full-Source-mode session.
#[derive(Debug)]
pub(crate) struct SourceSession {
    /// Undo-stack length at entry (the floor undo cannot go below).
    floor: usize,
    /// `History::evicted` at entry (evicted transactions lower the floor).
    evicted: u64,
    /// The rendered view's redo stack, set aside while the session runs.
    stash: Vec<Transaction>,
}

impl EditorSession {
    /// Opens full Source mode's own history (ADR 0006 §4.3). No-op when one is open.
    pub fn begin_source_session(&mut self) {
        if self.source.is_some() {
            return;
        }
        self.close_typing();
        let stash = self.history.take_redo();
        self.source = Some(SourceSession {
            floor: self.history.undo_len(),
            evicted: self.history.evicted(),
            stash,
        });
    }

    /// Whether full Source mode's history is open.
    pub fn in_source_session(&self) -> bool {
        self.source.is_some()
    }

    fn source_floor(&self) -> usize {
        match &self.source {
            Some(s) => s.floor.saturating_sub((self.history.evicted() - s.evicted) as usize),
            None => 0,
        }
    }

    /// Undo is possible (in a source session: not below the state it was entered in).
    pub fn can_undo(&self) -> bool {
        self.history.can_undo() && self.history.undo_len() > self.source_floor()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Closes full Source mode's history: the session's transactions become "Source Edit"
    /// transactions, split after every state that was ever saved; without edits the rendered
    /// view's redo stack comes back.
    pub fn end_source_session(&mut self) {
        let Some(s) = self.source.take() else { return };
        self.close_typing();
        let floor = s.floor.saturating_sub((self.history.evicted() - s.evicted) as usize);
        let n = self.history.undo_len();
        if n <= floor {
            // nothing left above the entry state (no edits, or all undone): the document is
            // in the entry state, so the set-aside redo stack applies again
            self.history.restore_redo(s.stash);
            return;
        }
        let mut txs = Vec::with_capacity(n - floor);
        for _ in floor..n {
            if let Some(t) = self.history.pop_undo() {
                txs.push(t);
            }
        }
        txs.reverse();
        let mut segs: Vec<Transaction> = Vec::new();
        let mut cur: Option<Transaction> = None;
        for t in txs {
            let boundary = self.saved_ever.contains(&t.state_after);
            match cur.as_mut() {
                None => {
                    cur = Some(Transaction {
                        label: Cow::Borrowed("Source Edit"),
                        kind: TxKind::Bulk,
                        steps: t.steps,
                        sel_before: t.sel_before,
                        sel_after: t.sel_after,
                        state_before: t.state_before,
                        state_after: t.state_after,
                    })
                }
                Some(c) => {
                    c.steps.extend(t.steps);
                    c.sel_after = t.sel_after;
                    c.state_after = t.state_after;
                }
            }
            if boundary {
                segs.extend(cur.take());
            }
        }
        segs.extend(cur);
        self.history.clear_redo();
        for t in segs {
            self.history.push_undo_keep_redo(t);
        }
    }

    /// A verbatim keystroke of the source editor: `range` replaced by `insert` (bytes as
    /// given), caret at `caret_after`; coalesces into the open typing group like typing in the
    /// rendered view (C13; C56 for a space typed after a word).
    pub fn source_keystroke(
        &mut self,
        dir: TypingDir,
        range: Range<usize>,
        insert: &str,
        caret_after: usize,
    ) -> Result<(), EditError> {
        if dir == TypingDir::Insert
            && self.open.as_ref().is_some_and(|o| o.dir == TypingDir::Insert)
            && insert.starts_with([' ', '\t'])
            && range.is_empty()
            && self.doc.text()[..range.start]
                .chars()
                .next_back()
                .is_some_and(|c| c != ' ' && c != '\t')
        {
            self.close_typing();
        }
        let hints_after = (!self.doc.hints().is_empty()).then(HintSet::new);
        let step = PlanStep {
            patches: PatchSet::single_verbatim(range.clone(), insert),
            hints_after,
        };
        let leaf = self.leaf_info(range.start).map(|l| l.0);
        self.keystroke(dir, step, Some(Selection::caret(caret_after)), leaf)
    }

    /// A verbatim edit of the source editor as its own transaction `label` (line breaks,
    /// indentation); the selection follows the patches unless `sel_after` is given.
    pub fn source_edit(
        &mut self,
        label: &'static str,
        patches: PatchSet,
        sel_after: Option<Selection>,
    ) -> Result<(), EditError> {
        let hints_after = (!self.doc.hints().is_empty()).then(HintSet::new);
        self.transact(label, TxKind::Structural, |tx| {
            tx.step(PlanStep { patches, hints_after })?;
            if let Some(s) = sel_after {
                tx.set_selection(s);
            }
            Ok(())
        })
    }
}

/// The source view as caret geometry for the registry's caret commands.
pub struct SourceCaret<'a> {
    pub view: &'a mut SourceView,
    pub ctx: SourceCtx<'a>,
    pub shaper: &'a mut dyn TextShaper,
    /// Viewport height (Page Up/Down).
    pub page: f32,
}

impl CaretGeometry for SourceCaret<'_> {
    fn horizontal(&mut self, pos: DocPos, forward: bool, word: bool) -> DocPos {
        let t = self.ctx.doc.text();
        let o = pos.offset.min(t.len());
        DocPos::new(match (forward, word) {
            (true, false) => src::next_grapheme(t, o),
            (false, false) => src::prev_grapheme(t, o),
            (f, true) => src::word_move(t, o, f),
        })
    }
    fn vertical(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32) {
        self.view.vertical(pos, goal_x, down, &self.ctx, self.shaper)
    }
    fn row_edge(&mut self, pos: DocPos, end: bool) -> DocPos {
        self.view.row_edge(pos, end, &self.ctx, self.shaper)
    }
    fn page(&mut self, pos: DocPos, goal_x: Option<f32>, down: bool) -> (DocPos, f32) {
        let p = (self.page - 40.0).max(40.0);
        self.view.page(pos, goal_x, down, p, &self.ctx, self.shaper)
    }
    fn doc_edge(&mut self, end: bool) -> DocPos {
        DocPos::new(if end { self.ctx.doc.text().len() } else { 0 })
    }
}

/// The source editor's state next to the session: its view and its own selection.
pub struct SourcePane {
    pub view: SourceView,
    pub sel: Selection,
    /// Spaces inserted by Tab and removed by Shift+Tab (the code indent unit, spec 02 §16).
    pub indent: usize,
}

/// Commands the source pane runs (others are disabled there in this version).
pub fn source_command_available(id: &str) -> bool {
    matches!(
        id,
        "edit.undo"
            | "edit.redo"
            | "edit.insertText"
            | "edit.deleteBackward"
            | "edit.deleteForward"
            | "edit.deleteWordBackward"
            | "edit.deleteWordForward"
            | "delete.toLineStart"
            | "edit.newline"
            | "edit.softBreak"
            | "edit.indentOrNextCell"
            | "edit.outdentOrPrevCell"
            | "select.all"
    ) || id.starts_with("caret.")
        || id.starts_with("select.")
        || id.starts_with("nav.")
}

/// The line terminator a line break typed at `at` gets: the terminator of the line it splits,
/// the document's line ending on a last line without one.
fn eol_at(doc: &dyn FormatDoc, at: usize) -> &'static str {
    let t = doc.text();
    match t[at.min(t.len())..].find('\n') {
        Some(k) => {
            let n = at + k;
            if n > 0 && t.as_bytes()[n - 1] == b'\r' {
                "\r\n"
            } else {
                "\n"
            }
        }
        None => doc.line_ending().as_str(),
    }
}

/// Byte offsets of the starts of the lines touched by `r` (a range ending at a line start
/// does not touch that line, unless the range is empty).
fn touched_lines(t: &str, r: &Range<usize>) -> Vec<usize> {
    let first = t[..r.start].rfind('\n').map_or(0, |k| k + 1);
    let mut out = vec![first];
    let end = if r.end > r.start && r.end > 0 && t.as_bytes()[r.end - 1] == b'\n' {
        r.end - 1
    } else {
        r.end
    };
    let mut p = first;
    while let Some(k) = t[p..end.max(p)].find('\n') {
        p += k + 1;
        if p > end {
            break;
        }
        out.push(p);
    }
    out
}

/// `sel` with both ends on caret positions of `text` (char boundaries, not inside a CRLF
/// terminator, within the text): every selection the source pane takes goes through this.
pub fn snap_selection(text: &str, sel: &Selection) -> Selection {
    let p = sel.primary();
    let snap = |d: DocPos| DocPos {
        offset: src::snap_offset(text, d.offset),
        virt: 0,
        ..d
    };
    let (a, f) = (snap(p.anchor), snap(p.focus));
    if a == p.anchor && f == p.focus && sel.ranges().len() == 1 {
        return sel.clone();
    }
    let mut out = Selection::single(a, f);
    out.goal_x = sel.goal_x;
    out
}

/// A position mapped through one combined change (`EditSummary`: `changed` before the edit,
/// `new_len` after it): before the change it stays, after it shifts, inside it moves to the
/// end of the new text.
fn map_through(pos: DocPos, e: &markise_doc::EditSummary) -> DocPos {
    let o = pos.offset;
    let offset = if o < e.changed.start || (o == e.changed.start && e.changed.is_empty() && e.new_len == 0) {
        o
    } else if o >= e.changed.end {
        (o as isize + e.delta).max(0) as usize
    } else {
        e.changed.start + e.new_len
    };
    DocPos { offset, ..pos }
}

impl SourcePane {
    /// Maps the pane's selection through a document change made elsewhere (the rendered pane,
    /// undo from a menu) and snaps it to caret positions of the new `text`.
    pub fn map_selection(&mut self, text: &str, e: &markise_doc::EditSummary) {
        let p = self.sel.primary();
        let mut s = Selection::single(map_through(p.anchor, e), map_through(p.focus, e));
        s.goal_x = None;
        self.sel = snap_selection(text, &s);
    }

    /// Clamps and snaps the selection to `text` (the document was replaced).
    pub fn clamp_selection(&mut self, text: &str) {
        self.sel = snap_selection(text, &self.sel);
    }
}

impl EditorView {
    /// Opens the source pane (entering Source or Side by Side): a source view of the current
    /// text with the caret where the session's is (a selection collapses to its focus, spec 02
    /// §12.1 position mapping).
    pub fn open_source(&mut self, params: SourceParams, width: f32, ppp: f32) {
        let theme = self.view.theme().clone();
        let view = SourceView::new(self.session.text(), params, &theme, width, ppp);
        let focus = self.session.selection().primary().focus;
        let caret = DocPos::new(focus.offset.min(self.session.text().len()));
        self.source = Some(SourcePane {
            view,
            sel: Selection::caret_at(caret),
            indent: 4,
        });
    }

    /// Drops the source pane (back to the rendered view).
    pub fn close_source(&mut self) {
        self.source = None;
    }

    /// The source pane's caret rectangle in content coordinates.
    pub fn source_caret_rect(&mut self, shaper: &mut dyn TextShaper) -> Option<Rect> {
        let pane = self.source.as_mut()?;
        let doc = self.session.doc_arc();
        let ctx = SourceCtx {
            doc,
            model: Some(self.view.model()),
            highlights: &**self.view.highlights(),
        };
        let focus = pane.sel.primary().focus;
        Some(pane.view.caret_rect(focus, &ctx, shaper))
    }

    /// The physical-line band of the source caret (content coordinates).
    pub fn source_line_rect(&mut self, shaper: &mut dyn TextShaper) -> Option<Rect> {
        let pane = self.source.as_mut()?;
        let ctx = SourceCtx {
            doc: self.session.doc_arc(),
            model: Some(self.view.model()),
            highlights: &**self.view.highlights(),
        };
        let focus = pane.sel.primary().focus;
        Some(pane.view.line_rect(focus, &ctx, shaper))
    }

    /// The source position under content point `p`.
    pub fn source_hit(&mut self, p: Pos2, shaper: &mut dyn TextShaper) -> Option<DocPos> {
        let pane = self.source.as_mut()?;
        let ctx = SourceCtx {
            doc: self.session.doc_arc(),
            model: Some(self.view.model()),
            highlights: &**self.view.highlights(),
        };
        Some(pane.view.hit(p, &ctx, shaper))
    }

    /// Lays out the source viewport (see [`SourceView::frame`]).
    pub fn source_frame(&mut self, vh: f32, margin: f32, shaper: &mut dyn TextShaper) -> Vec<SourceVisible> {
        let Some(pane) = self.source.as_mut() else {
            return Vec::new();
        };
        let ctx = SourceCtx {
            doc: self.session.doc_arc(),
            model: Some(self.view.model()),
            highlights: &**self.view.highlights(),
        };
        pane.view.frame(vh, margin, &ctx, shaper)
    }

    /// Scrolls the source view so the caret is visible in a viewport `vh` tall.
    pub fn source_scroll_to_caret(&mut self, vh: f32, shaper: &mut dyn TextShaper) {
        let Some(r) = self.source_caret_rect(shaper) else {
            return;
        };
        let Some(pane) = self.source.as_mut() else { return };
        let top = pane.view.scroll_y() as f32;
        let margin = (vh * 0.1).min(48.0);
        if r.min.y < top + margin {
            pane.view.scroll_to_y((r.min.y - margin).max(0.0) as f64);
        } else if r.max.y > top + vh - margin {
            pane.view.scroll_to_y((r.max.y - vh + margin) as f64);
        }
    }

    /// Sets the source selection (click, drag): a caret jump closes the typing group.
    pub fn source_set_selection(&mut self, sel: Selection) {
        let sel = snap_selection(self.session.text(), &sel);
        let Some(pane) = self.source.as_mut() else { return };
        if pane.sel == sel {
            return;
        }
        pane.sel = sel.clone();
        self.session.set_selection(sel);
    }

    /// Hands the pane's selection to the session (before a session operation).
    fn source_selection_to_session(&mut self) {
        // guard: the pane's selection is always on caret positions of the current text
        let text = self.session.text();
        if let Some(pane) = self.source.as_mut() {
            pane.sel = snap_selection(text, &pane.sel);
        }
        if let Some(pane) = &self.source
            && self.session.selection() != &pane.sel
        {
            let s = pane.sel.clone();
            self.session.set_selection(s);
        }
    }

    /// Takes the session's selection after an operation.
    fn source_selection_from_session(&mut self) {
        let text = self.session.text();
        if let Some(pane) = self.source.as_mut() {
            pane.sel = snap_selection(text, self.session.selection());
        }
    }

    /// Runs an edit on the session (view reference released, view synced afterwards).
    fn source_edit_with(
        &mut self,
        f: impl FnOnce(&mut EditorSession) -> Result<(), EditError>,
    ) -> Result<(), CommandError> {
        self.source_selection_to_session();
        self.release();
        let r = f(&mut self.session);
        self.sync();
        self.source_selection_from_session();
        r.map_err(CommandError::Edit)
    }

    /// Inserts `text` verbatim at the source selection: typing (`own_step == false`), or a
    /// paste / IME commit (its own undo step).
    pub fn source_insert(&mut self, text: &str, own_step: bool) -> Result<(), CommandError> {
        if self.source.is_none() || text.is_empty() {
            return Ok(());
        }
        if own_step {
            self.session.close_typing();
        }
        self.source_selection_to_session();
        let r = self.source.as_ref().map(|p| p.sel.primary().range()).unwrap_or(0..0);
        let caret = r.start + text.len();
        let res = self.source_edit_with(|s| s.source_keystroke(TypingDir::Insert, r, text, caret));
        if own_step {
            self.session.close_typing();
        }
        res
    }

    /// The selected source text (copy / cut).
    pub fn source_selected_text(&self) -> Option<String> {
        let pane = self.source.as_ref()?;
        let r = pane.sel.primary().range();
        (!r.is_empty()).then(|| self.session.text()[r].to_string())
    }

    /// Deletes the source selection as its own step (cut).
    pub fn source_cut(&mut self) -> Result<(), CommandError> {
        self.source_selection_to_session();
        let Some(r) = self.source.as_ref().map(|p| p.sel.primary().range()) else {
            return Ok(());
        };
        if r.is_empty() {
            return Ok(());
        }
        self.session.close_typing();
        let res = self.source_edit_with(|s| s.source_keystroke(TypingDir::DeleteBackward, r.clone(), "", r.start));
        self.session.close_typing();
        res
    }

    /// Runs command `id` in the source pane (keymap, menus). See the module docs.
    pub fn source_execute(
        &mut self,
        id: &str,
        params: &Params,
        shaper: &mut dyn TextShaper,
        page: f32,
    ) -> Result<(), CommandError> {
        if self.source.is_none() {
            return Err(CommandError::Disabled("no source pane"));
        }
        if !source_command_available(id) {
            return Err(CommandError::Disabled("not available in the source editor yet"));
        }
        self.source_selection_to_session();
        let sel = self.source.as_ref().expect("pane").sel.primary();
        let r = sel.range();
        match id {
            "edit.undo" | "edit.redo" => {
                self.source_selection_to_session();
                self.release();
                let ok = if id == "edit.undo" {
                    self.session.undo()
                } else {
                    self.session.redo()
                };
                self.sync();
                self.source_selection_from_session();
                if ok {
                    Ok(())
                } else {
                    Err(CommandError::Disabled("nothing to undo or redo"))
                }
            }
            "edit.insertText" => {
                let t = params.get_text("text").unwrap_or_default().to_string();
                self.source_insert(&t, false)
            }
            "edit.deleteBackward" | "edit.deleteForward" => {
                let fwd = id == "edit.deleteForward";
                let t = self.session.text();
                let range = if !r.is_empty() {
                    r.clone()
                } else if fwd {
                    r.start..src::next_grapheme(t, r.start)
                } else {
                    src::prev_grapheme(t, r.start)..r.start
                };
                if range.is_empty() {
                    return Ok(());
                }
                let dir = if fwd && r.is_empty() {
                    TypingDir::DeleteForward
                } else {
                    TypingDir::DeleteBackward
                };
                self.source_edit_with(|s| s.source_keystroke(dir, range.clone(), "", range.start))
            }
            "edit.deleteWordBackward" | "edit.deleteWordForward" | "delete.toLineStart" => {
                let fwd = id == "edit.deleteWordForward";
                let range = if !r.is_empty() {
                    r.clone()
                } else if id == "delete.toLineStart" {
                    let doc = self.session.doc_arc().clone();
                    let pane = self.source.as_mut().expect("pane");
                    let ctx = SourceCtx {
                        doc: &doc,
                        model: Some(self.view.model()),
                        highlights: &**self.view.highlights(),
                    };
                    let a = pane.view.row_edge(sel.focus, false, &ctx, shaper).offset;
                    // at a row start: like Backspace (joins lines)
                    if a < r.start {
                        a..r.start
                    } else {
                        src::prev_grapheme(doc.text(), r.start)..r.start
                    }
                } else {
                    let t = self.session.text();
                    let o = src::word_move(t, r.start, fwd);
                    if fwd { r.start..o } else { o..r.start }
                };
                if range.is_empty() {
                    return Ok(());
                }
                let dir = if fwd {
                    TypingDir::DeleteForward
                } else {
                    TypingDir::DeleteBackward
                };
                self.source_edit_with(|s| s.source_keystroke(dir, range.clone(), "", range.start))
            }
            "edit.newline" | "edit.softBreak" => {
                let eol = eol_at(self.session.doc(), r.start);
                let caret = r.start + eol.len();
                self.source_edit_with(|s| {
                    s.close_typing();
                    s.source_edit(
                        "New Line",
                        PatchSet::single_verbatim(r.clone(), eol),
                        Some(Selection::caret(caret)),
                    )
                })
            }
            "edit.indentOrNextCell" | "edit.outdentOrPrevCell" => {
                let indent = self.source.as_ref().map_or(4, |p| p.indent.max(1));
                let outdent = id == "edit.outdentOrPrevCell";
                let t = self.session.text().to_string();
                let multi = t[r.clone()].contains('\n');
                if !outdent && !multi {
                    let spaces = " ".repeat(indent);
                    let caret = r.start + indent;
                    return self.source_edit_with(|s| s.source_keystroke(TypingDir::Insert, r.clone(), &spaces, caret));
                }
                let lines = touched_lines(&t, &r);
                let mut ps = Vec::new();
                for &l in &lines {
                    if outdent {
                        let b = t.as_bytes();
                        let n = if b.get(l) == Some(&b'\t') {
                            1
                        } else {
                            b[l..].iter().take(indent).take_while(|&&c| c == b' ').count()
                        };
                        if n > 0 {
                            ps.push(Patch::verbatim(l..l + n, ""));
                        }
                    } else if !matches!(t.as_bytes().get(l), None | Some(b'\n') | Some(b'\r')) {
                        ps.push(Patch::verbatim(l..l, " ".repeat(indent)));
                    }
                }
                if ps.is_empty() {
                    return Ok(());
                }
                let ps = PatchSet::new(ps).map_err(|e| CommandError::Edit(e.into()))?;
                let label = if outdent { "Outdent" } else { "Indent" };
                self.source_edit_with(|s| {
                    s.close_typing();
                    s.source_edit(label, ps, None)
                })
            }
            _ => {
                // caret and selection commands: the registry with the source view as geometry
                self.source_selection_to_session();
                let doc: Arc<dyn FormatDoc> = self.session.doc_arc().clone();
                let hl: Arc<dyn Highlights + Send + Sync> = self.view.highlights().clone();
                let pane = self.source.as_mut().expect("pane");
                let res = {
                    let mut g = SourceCaret {
                        view: &mut pane.view,
                        ctx: SourceCtx {
                            doc: &doc,
                            model: Some(self.view.model()),
                            highlights: &*hl,
                        },
                        shaper,
                        page,
                    };
                    self.registry.execute_with(&mut self.session, &mut g, id, params)
                };
                drop(doc);
                self.source_selection_from_session();
                res
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touched_lines_follow_the_selection() {
        let t = "a\nbb\nccc\n";
        assert_eq!(touched_lines(t, &(0..0)), vec![0]);
        assert_eq!(touched_lines(t, &(1..3)), vec![0, 2]);
        assert_eq!(touched_lines(t, &(0..5)), vec![0, 2], "ends at a line start");
        assert_eq!(touched_lines(t, &(3..9)), vec![2, 5]);
    }
}
