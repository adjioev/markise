//! Headless editing session (ADR 0006 §2.5, §4): the mutable format document, selection, live
//! marks, undo history with typing groups, atomic multi-step transactions with rollback, the
//! `counter` / `StateId` revision split and deferred external effects. UI-free; the command
//! registry ([`command`]) drives it.
//!
//! Every edit goes through one path: plan step → canonical patch set → `FormatDoc::apply` →
//! live-mark transform → hint update → `Step` recorded. Undo/redo replay recorded steps (never
//! re-plan) and restore the transaction's immutable selections.

pub mod command;
pub mod history;
pub mod keymap;
pub mod surface;

use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use markise_doc::{
    Affinity, DocPos, DocumentFormat, EditPrefs, EditSummary, FormatDoc, FormatOptions, HintSet, LeafKind, Params,
    Patch, PatchError, PatchSet, Plan, PlanError, PlanRequest, PlanStep, Selection, planners,
};

pub use history::{History, HistoryLimits, StateId, Step, Transaction, TxKind, TypingDir};
pub use surface::{CaretGeometry, EditorView, ViewCaret};

/// Time source for typing-group pauses (C13); injectable for tests.
pub trait Clock: Send {
    fn now(&self) -> Instant;
}

/// The system monotonic clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock advanced by hand (tests, scripted sessions).
#[derive(Clone, Debug)]
pub struct ManualClock(Arc<Mutex<Instant>>);

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock(Arc::new(Mutex::new(Instant::now())))
    }
}

impl ManualClock {
    pub fn advance(&self, d: std::time::Duration) {
        *self.0.lock().unwrap() += d;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

/// Why an edit failed. A failed transaction is rolled back completely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    Patch(PatchError),
    Plan(PlanError),
    /// Raised by a transaction body.
    Aborted(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Patch(e) => write!(f, "{e}"),
            EditError::Plan(e) => write!(f, "{e}"),
            EditError::Aborted(why) => write!(f, "aborted: {why}"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<PatchError> for EditError {
    fn from(e: PatchError) -> Self {
        EditError::Patch(e)
    }
}

impl From<PlanError> for EditError {
    fn from(e: PlanError) -> Self {
        EditError::Plan(e)
    }
}

/// Handle of a live mark (find result, composition anchor, bookmark…).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MarkId(usize);

/// External effects; run only after a transaction commits (discarded on rollback).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEffect {
    /// The text or hints changed; `counter` is the new monotonic revision (autosave, workers).
    TextChanged { counter: u64 },
    /// An effect queued by a transaction body (deferred until commit).
    Custom(String),
}

/// Revision pair (ADR 0006 §4.3): `counter` grows on every applied step, undo, redo and
/// rollback (orders worker results); `state` identifies the (text, hints) state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRev {
    pub counter: u64,
    pub state: StateId,
}

/// What a save writes: hint-free bytes plus the revision it was taken at.
#[derive(Clone, Debug)]
pub struct SaveSnapshot {
    pub text: Arc<str>,
    pub counter: u64,
    pub state: StateId,
}

#[derive(Clone, Debug)]
struct OpenTyping {
    dir: TypingDir,
    leaf_start: Option<usize>,
    last_at: Instant,
    caret: usize,
}

#[derive(Clone)]
struct Journal {
    sel: Selection,
    marks: Vec<Option<DocPos>>,
    hints: HintSet,
    state: StateId,
}

/// What changed in the document since the last [`EditorSession::take_change`] (for the view
/// model owner: incremental `DocModel::apply`, or a rebuild after a replaced document).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DocChange {
    #[default]
    None,
    /// Incremental changes, combined into one summary (pre-change → current coordinates).
    Edited(EditSummary),
    /// The document object was replaced ([`EditorSession::replace_doc`]) or the change could
    /// not be described incrementally: rebuild the model.
    Replaced,
}

impl DocChange {
    fn push(&mut self, e: EditSummary) {
        *self = match std::mem::take(self) {
            DocChange::None => DocChange::Edited(e),
            DocChange::Edited(a) => DocChange::Edited(a.then(e)),
            DocChange::Replaced => DocChange::Replaced,
        };
    }
}

/// One open document being edited.
pub struct EditorSession {
    format: Arc<dyn DocumentFormat>,
    /// Shared with the view (layout reads it); edited in place when no one else holds it
    /// (otherwise copied first — the caller should release other references before editing).
    doc: Arc<dyn FormatDoc>,
    change: DocChange,
    /// Clones of a shared document made by [`EditorSession::doc_mut`] (diagnostics, tests).
    pub copies: u64,
    sel: Selection,
    marks: Vec<Option<DocPos>>,
    history: History,
    rev: TextRev,
    next_state: u64,
    saved_state: Option<StateId>,
    /// Every state that was ever saved, submitted for saving (`prepare_save`) or opened from
    /// disk: permanent transaction boundaries — nothing joins across them, whichever state
    /// matches the disk now.
    saved_ever: std::collections::HashSet<StateId>,
    open: Option<OpenTyping>,
    effects: Vec<SessionEffect>,
    clock: Box<dyn Clock>,
    /// Editing preferences passed to planners (auto-pair, smart punctuation…).
    prefs: EditPrefs,
}

impl fmt::Debug for EditorSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorSession")
            .field("format", &self.format.id())
            .field("len", &self.doc.text().len())
            .field("sel", &self.sel)
            .field("rev", &self.rev)
            .field("saved_state", &self.saved_state)
            .finish()
    }
}

impl EditorSession {
    /// Opens `text` with `format`; the opened state is clean.
    pub fn new(format: Arc<dyn DocumentFormat>, text: impl Into<String>) -> Self {
        Self::with(format, text, HistoryLimits::default(), Box::new(SystemClock))
    }

    pub fn with(
        format: Arc<dyn DocumentFormat>,
        text: impl Into<String>,
        limits: HistoryLimits,
        clock: Box<dyn Clock>,
    ) -> Self {
        let doc: Arc<dyn FormatDoc> = Arc::from(format.parse(text.into(), &FormatOptions::default()));
        Self::from_doc(format, doc, limits, clock)
    }

    /// Opens an already parsed document (e.g. from the background loader); the opened state is
    /// clean.
    pub fn from_doc(
        format: Arc<dyn DocumentFormat>,
        doc: Arc<dyn FormatDoc>,
        limits: HistoryLimits,
        clock: Box<dyn Clock>,
    ) -> Self {
        let state = StateId(0);
        EditorSession {
            format,
            doc,
            change: DocChange::None,
            copies: 0,
            sel: Selection::caret(0),
            marks: Vec::new(),
            history: History::new(limits),
            rev: TextRev { counter: 0, state },
            next_state: 1,
            saved_state: Some(state),
            saved_ever: std::iter::once(state).collect(),
            open: None,
            effects: Vec::new(),
            clock,
            prefs: EditPrefs::default(),
        }
    }

    /// Editing preferences (spec 02 §4, §13, §16).
    pub fn prefs(&self) -> &EditPrefs {
        &self.prefs
    }
    pub fn set_prefs(&mut self, prefs: EditPrefs) {
        self.prefs = prefs;
    }

    pub fn format(&self) -> &Arc<dyn DocumentFormat> {
        &self.format
    }
    pub fn doc(&self) -> &dyn FormatDoc {
        self.doc.as_ref()
    }
    /// The shared document (hand it to the view after each change).
    pub fn doc_arc(&self) -> &Arc<dyn FormatDoc> {
        &self.doc
    }
    /// Changes since the last call.
    pub fn take_change(&mut self) -> DocChange {
        std::mem::take(&mut self.change)
    }
    /// The document for an in-place edit: copied first when it is shared.
    fn doc_mut(&mut self) -> &mut dyn FormatDoc {
        if Arc::get_mut(&mut self.doc).is_none() {
            self.copies += 1;
            self.doc = Arc::from(self.doc.clone_box());
        }
        Arc::get_mut(&mut self.doc).expect("unique document")
    }
    fn doc_apply(&mut self, ps: &PatchSet, hints: Option<&HintSet>) {
        let e = self.doc_mut().apply_with_hints(ps, hints);
        self.change.push(e);
    }
    fn doc_set_hints(&mut self, hints: &HintSet) {
        let e = self.doc_mut().set_hints(hints);
        self.change.push(e);
    }
    pub fn text(&self) -> &str {
        self.doc.text()
    }
    pub fn hints(&self) -> &HintSet {
        self.doc.hints()
    }
    pub fn selection(&self) -> &Selection {
        &self.sel
    }
    pub fn revision(&self) -> TextRev {
        self.rev
    }
    pub fn state_id(&self) -> StateId {
        self.rev.state
    }
    pub fn saved_state(&self) -> Option<StateId> {
        self.saved_state
    }
    /// Dirty = the current (text, hints) state is not the saved one.
    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.rev.state)
    }
    pub fn history(&self) -> &History {
        &self.history
    }
    /// `true` while keystrokes still coalesce into the newest transaction.
    pub fn typing_group_open(&self) -> bool {
        self.open.is_some()
    }
    /// Committed external effects since the last call.
    pub fn take_effects(&mut self) -> Vec<SessionEffect> {
        std::mem::take(&mut self.effects)
    }

    /// Sets the selection. Any change not produced by typing is a caret jump and closes the
    /// open typing group (C13, spec 02 §11).
    pub fn set_selection(&mut self, sel: Selection) {
        if sel != self.sel {
            self.close_typing();
            self.sel = sel;
        }
    }

    /// ADR 0006 §2.6: a literal hint expires when the caret leaves the hinted block (the
    /// escape is materialised as its own `Auto` transaction). Called by the editing surface
    /// after caret moves (the session's `set_selection` itself never edits).
    pub fn expire_hints_left_behind(&mut self) {
        if self.doc.hints().is_empty() {
            return;
        }
        let caret = self.sel.primary().focus.offset;
        let here = self.doc.leaf(caret).map(|l| l.span.start);
        let left = self
            .doc
            .hints()
            .iter()
            .any(|h| self.doc.leaf(h.line).map(|l| l.span.start) != here);
        if left {
            // the escape patch shifts text after it; the selection is transformed like any
            // live mark. A failure leaves the hint (it is materialised before saving).
            let _ = self.expire_hints();
        }
    }

    /// Closes the open typing group (undo boundary).
    pub fn close_typing(&mut self) {
        self.open = None;
    }

    /// Adds a live mark, transformed through every applied patch set.
    pub fn add_mark(&mut self, pos: DocPos) -> MarkId {
        self.marks.push(Some(pos));
        MarkId(self.marks.len() - 1)
    }
    pub fn mark(&self, id: MarkId) -> Option<DocPos> {
        self.marks.get(id.0).copied().flatten()
    }
    pub fn remove_mark(&mut self, id: MarkId) {
        if let Some(m) = self.marks.get_mut(id.0) {
            *m = None;
        }
    }

    /// A planner request for the current state.
    pub fn request(&self, params: Params) -> PlanRequest {
        PlanRequest {
            selection: self.sel.clone(),
            params,
            hints: self.doc.hints().clone(),
            source_mode: false,
            prefs: self.prefs,
        }
    }

    fn fresh_state(&mut self) -> StateId {
        let s = StateId(self.next_state);
        self.next_state += 1;
        s
    }

    fn transform_live(&mut self, ps: &PatchSet) {
        self.sel = self.sel.transform(ps);
        for m in self.marks.iter_mut().flatten() {
            *m = m.transform(ps);
        }
    }

    /// The single mutation path: canonicalise, validate, apply, transform live marks, update
    /// hints, bump the counter. Nothing is changed when validation fails.
    fn apply_raw(&mut self, patches: &PatchSet, hints_after: Option<HintSet>) -> Result<Step, EditError> {
        let forward = self.doc.canonicalize(patches);
        forward.validate(self.doc.text())?;
        let removed = forward.removed_from(self.doc.text());
        let hints_before = self.doc.hints().clone();
        if !forward.is_empty() {
            // text + final hints in one change (no intermediate parse without the hint)
            self.doc_apply(&forward, hints_after.as_ref());
            self.transform_live(&forward);
        } else if let Some(h) = hints_after
            && h != *self.doc.hints()
        {
            self.doc_set_hints(&h);
        }
        let hints_after = self.doc.hints().clone();
        self.rev.counter += 1;
        Ok(Step {
            forward,
            removed,
            hints_before,
            hints_after,
        })
    }

    fn replay_inverse(&mut self, step: &Step) {
        let inv = step.inverse();
        if !inv.is_empty() {
            self.doc_apply(&inv, Some(&step.hints_before));
            self.transform_live(&inv);
        } else if *self.doc.hints() != step.hints_before {
            let h = step.hints_before.clone();
            self.doc_set_hints(&h);
        }
    }

    fn replay_forward(&mut self, step: &Step) {
        if !step.forward.is_empty() {
            self.doc_apply(&step.forward, Some(&step.hints_after));
            self.transform_live(&step.forward);
        } else if *self.doc.hints() != step.hints_after {
            let h = step.hints_after.clone();
            self.doc_set_hints(&h);
        }
    }

    fn changed(&mut self) {
        self.effects.push(SessionEffect::TextChanged {
            counter: self.rev.counter,
        });
    }

    fn leaf_info(&self, at: usize) -> Option<(usize, LeafKind)> {
        self.doc.leaf(at).map(|l| (l.span.start, l.kind))
    }

    // ---- Typing (ADR 0006 §5.1, grouping §4.2) ----

    /// `edit.insertText`: the typing pipeline. Coalesces into the open typing group (C13, C56
    /// word boundaries); an
    /// immediate input rule completed by this text is pushed as its own `Auto` transaction.
    pub fn insert_text(&mut self, typed: &str) -> Result<(), EditError> {
        if typed.is_empty() {
            return Ok(());
        }
        // C56: whitespace typed after a non-whitespace character of the group starts a new group.
        if self.open.as_ref().is_some_and(|o| o.dir == TypingDir::Insert)
            && typed.starts_with([' ', '\t'])
            && self.sel.is_caret()
            && self.doc.text()[..self.sel.primary().focus.offset]
                .chars()
                .next_back()
                .is_some_and(|c| c != ' ' && c != '\t')
        {
            self.close_typing();
        }
        if !self.sel.is_caret() {
            let r = self.sel.primary().range();
            let w = self.widen_from_heading_start(r.clone());
            if w != r {
                self.set_selection(Selection::single(DocPos::new(w.start), DocPos::new(w.end)));
            }
        }
        let req = self.request(Params::new());
        let tp = self.doc.on_text(&req, typed);
        let leaf_before = self.leaf_info(self.sel.primary().start().offset);
        self.keystroke(
            TypingDir::Insert,
            tp.step,
            tp.selection_after,
            leaf_before.as_ref().map(|l| l.0),
        )?;
        // block-type change closes the group (C13)
        let leaf_after = self.leaf_info(self.sel.primary().focus.offset);
        if let (Some((_, a)), Some((_, b))) = (&leaf_before, &leaf_after)
            && a != b
        {
            self.close_typing();
        }
        if let Some(rule) = tp.rule {
            self.close_typing();
            self.apply_plan(rule.label, TxKind::Auto, rule.plan)?;
        }
        Ok(())
    }

    /// `edit.deleteBackward` without layout: the previous grapheme (see
    /// [`EditorSession::delete_backward_to`]).
    pub fn delete_backward(&mut self) -> Result<(), EditError> {
        self.delete_backward_to(None)
    }

    /// `edit.deleteBackward`. `prev_stop`: the previous caret stop (from the caret geometry;
    /// covers atoms, hidden markers and line-break prefixes), else the previous grapheme.
    ///
    /// Block rules (spec 02 §5.2, the M2b subset): on the virtual empty paragraph the
    /// paragraph is dropped (no text change); at the start of a heading's text the heading
    /// becomes a paragraph (rule 4); at the start of a paragraph after a paragraph or heading
    /// the two are merged (rule 5); the start of any other block is a no-op. The remaining
    /// rules (lists, quotes, HR, empty blocks, tables) are M2c/M2d.
    pub fn delete_backward_to(&mut self, prev_stop: Option<usize>) -> Result<(), EditError> {
        let p = self.sel.primary();
        if p.is_caret() && p.focus.virt > 0 {
            self.set_selection(Selection::caret(p.focus.offset));
            return Ok(());
        }
        let r = p.range();
        if r.is_empty()
            && let Ok(plan) = self.doc.plan(planners::DELETE_BACKWARD, &self.request(Params::new()))
        {
            // the format's block rules (spec 02 §5.2: list items, quotes…)
            return self.apply_key_plan(plan, TypingDir::DeleteBackward);
        }
        let range = if !r.is_empty() {
            self.widen_from_heading_start(r)
        } else {
            match self.block_start_backward(r.start) {
                Some(BlockEdit::Edit { patches, label, caret }) => {
                    self.close_typing();
                    return self.apply_plan(
                        label,
                        TxKind::Structural,
                        Plan::single(PlanStep::patches(patches), Some(Selection::caret(caret))),
                    );
                }
                Some(BlockEdit::Nothing) => return Ok(()),
                Some(BlockEdit::Join(range)) => {
                    let leaf = self.leaf_info(range.start).map(|l| l.0);
                    return self.keystroke(
                        TypingDir::DeleteBackward,
                        PlanStep::patches(PatchSet::delete(range)),
                        None,
                        leaf,
                    );
                }
                None => {}
            }
            if r.start == 0 {
                return Ok(());
            }
            let t = self.doc.text();
            let p = match prev_stop.filter(|&s| s < r.start) {
                Some(s) => s,
                None if t[..r.start].ends_with("\r\n") => r.start - 2,
                None => prev_grapheme(t, r.start),
            };
            if !self.deletion_allowed(p..r.start) {
                return Ok(());
            }
            p..r.start
        };
        let leaf = self.leaf_info(range.start).map(|l| l.0);
        self.keystroke(
            TypingDir::DeleteBackward,
            PlanStep::patches(PatchSet::delete(range)),
            None,
            leaf,
        )
    }

    /// `edit.deleteForward` without layout (next grapheme).
    pub fn delete_forward(&mut self) -> Result<(), EditError> {
        self.delete_forward_to(None)
    }

    /// `edit.deleteForward`. `next_stop`: the next caret stop (caret geometry), else the next
    /// grapheme. At the end of a paragraph or heading followed by a paragraph or heading the
    /// next block's text is merged (C48, M2b subset); at the end of other blocks a no-op.
    pub fn delete_forward_to(&mut self, next_stop: Option<usize>) -> Result<(), EditError> {
        let p = self.sel.primary();
        if p.is_caret() && p.focus.virt > 0 {
            return Ok(());
        }
        let r = p.range();
        if r.is_empty()
            && let Ok(plan) = self.doc.plan(planners::DELETE_FORWARD, &self.request(Params::new()))
        {
            return self.apply_key_plan(plan, TypingDir::DeleteForward);
        }
        let range = if !r.is_empty() {
            self.widen_from_heading_start(r)
        } else {
            let t = self.doc.text();
            if r.end >= t.len() {
                return Ok(());
            }
            match self.block_end_forward(r.end) {
                Some(BlockEdit::Edit { patches, label, caret }) => {
                    self.close_typing();
                    return self.apply_plan(
                        label,
                        TxKind::Structural,
                        Plan::single(PlanStep::patches(patches), Some(Selection::caret(caret))),
                    );
                }
                Some(BlockEdit::Nothing) => return Ok(()),
                Some(BlockEdit::Join(range)) => {
                    let leaf = self.leaf_info(range.start).map(|l| l.0);
                    return self.keystroke(
                        TypingDir::DeleteForward,
                        PlanStep::patches(PatchSet::delete(range)),
                        None,
                        leaf,
                    );
                }
                None => {}
            }
            let t = self.doc.text();
            let n = match next_stop.filter(|&s| s > r.end) {
                Some(s) => s,
                None if t[r.end..].starts_with("\r\n") => r.end + 2,
                None => next_grapheme(t, r.end),
            };
            if !self.deletion_allowed(r.end..n) {
                return Ok(());
            }
            r.end..n
        };
        let leaf = self.leaf_info(range.start).map(|l| l.0);
        self.keystroke(
            TypingDir::DeleteForward,
            PlanStep::patches(PatchSet::delete(range)),
            None,
            leaf,
        )
    }

    /// A deletion command's generated range (word / line deletes): from the caret to
    /// `target`, clamped to the content of the caret's block part (one content line of a
    /// paragraph/heading, one table cell, the content of code/math/HTML/front matter), so it
    /// never removes structure (Codex review of PR #21). An empty clamped range falls back to
    /// a plain Backspace/Delete (whose block rules apply). A user's own selection is not
    /// clamped (it goes through the selection deletion).
    pub fn delete_generated(&mut self, target: usize, backward: bool) -> Result<(), EditError> {
        let p = self.sel.primary();
        if !p.is_caret() || p.focus.virt > 0 {
            return if backward {
                self.delete_backward_to(None)
            } else {
                self.delete_forward_to(None)
            };
        }
        let c = p.focus.offset;
        let bounds = self.content_bounds(c);
        let range = if backward {
            target.max(bounds.start).min(c)..c
        } else {
            c..target.min(bounds.end).max(c)
        };
        if range.is_empty() {
            return if backward {
                self.delete_backward_to(None)
            } else {
                self.delete_forward_to(None)
            };
        }
        let leaf = self.leaf_info(range.start).map(|l| l.0);
        let dir = if backward {
            TypingDir::DeleteBackward
        } else {
            TypingDir::DeleteForward
        };
        self.keystroke(dir, PlanStep::patches(PatchSet::delete(range)), None, leaf)
    }

    /// The editable content around caret `c` that one deletion command may touch.
    fn content_bounds(&self, c: usize) -> std::ops::Range<usize> {
        let Some(leaf) = self.doc.leaf(c) else { return c..c };
        let (Some(first), Some(last)) = (leaf.lines.first(), leaf.lines.last()) else {
            return c..c;
        };
        match leaf.kind {
            LeafKind::Paragraph | LeafKind::Heading { .. } => leaf
                .lines
                .iter()
                .find(|l| l.start <= c && c <= l.end)
                .map_or(c..c, |l| l.start..l.end),
            LeafKind::CodeBlock { .. } | LeafKind::Math | LeafKind::RawBlock | LeafKind::Metadata => {
                if first.start <= c && c <= last.end {
                    first.start..last.end
                } else {
                    c..c
                }
            }
            LeafKind::Table { .. } => leaf
                .lines
                .iter()
                .find(|l| l.start <= c && c <= l.end)
                .and_then(|row| {
                    self.doc
                        .table_row_cells(*row)
                        .into_iter()
                        .find(|cell| cell.start <= c && c <= cell.end)
                })
                .map_or(c..c, |cell| cell.start..cell.end),
            _ => c..c,
        }
    }

    /// Whether a caret keystroke may delete `r` (plain Backspace/Delete, after the block
    /// rules): only content bytes of one block. Structure — container prefixes (list markers,
    /// quote `>`), fence and front-matter delimiter lines, table pipes and the delimiter row,
    /// anything between blocks — is never removed by a single keystroke before the M2c/M2d
    /// rules exist (Codex review of PR #21). Text-like blocks: inside one content line; code,
    /// math, HTML and front matter: inside the content (line joins allowed); tables: inside one
    /// cell's content.
    fn deletion_allowed(&self, r: std::ops::Range<usize>) -> bool {
        if self.in_heading_trailing_ws(r.clone()) {
            return true;
        }
        let Some(leaf) = self.doc.leaf(r.start) else {
            return false;
        };
        let (Some(first), Some(last)) = (leaf.lines.first(), leaf.lines.last()) else {
            return false;
        };
        match leaf.kind {
            LeafKind::Paragraph | LeafKind::Heading { .. } => {
                leaf.lines.iter().any(|l| l.start <= r.start && r.end <= l.end)
            }
            LeafKind::CodeBlock { .. } | LeafKind::Math | LeafKind::RawBlock | LeafKind::Metadata => {
                first.start <= r.start && r.end <= last.end
            }
            LeafKind::Table { .. } => {
                let Some(row) = leaf.lines.iter().find(|l| l.start <= r.start && r.end <= l.end) else {
                    return false;
                };
                self.doc
                    .table_row_cells(*row)
                    .iter()
                    .any(|c| c.start <= r.start && r.end <= c.end)
            }
            _ => false,
        }
    }

    /// BE-4: a selection from the start of a heading's text into a later block removes the
    /// heading's leading syntax too, so what remains of the later block keeps its own type
    /// (from the middle of the heading the heading stays and the rest is appended).
    fn widen_from_heading_start(&self, r: std::ops::Range<usize>) -> std::ops::Range<usize> {
        match self.doc.leaf(r.start) {
            Some(l)
                if matches!(l.kind, LeafKind::Heading { .. })
                    && l.lines.first().is_some_and(|f| f.start == r.start)
                    && r.end > l.span.end
                    && self.doc.leaf(r.end).is_some_and(|n| n.span.start > l.span.start) =>
            {
                l.span.start..r.end
            }
            _ => r,
        }
    }

    /// Whether `r` lies in the spaces and tabs right after a heading's text on its line. An ATX
    /// heading's span stops at its text, so spaces typed after it are outside the block, yet
    /// they are its content for the caret (issue #16).
    fn in_heading_trailing_ws(&self, r: std::ops::Range<usize>) -> bool {
        let b = self.doc.text().as_bytes();
        if r.is_empty() || r.end > b.len() || !b[r.clone()].iter().all(|c| matches!(c, b' ' | b'\t')) {
            return false;
        }
        let mut q = r.start;
        while q > 0 && matches!(b[q - 1], b' ' | b'\t') {
            q -= 1;
        }
        q > 0
            && self.doc.leaf(q - 1).is_some_and(|l| {
                // after the text: past the span (open ATX) or before a closing sequence
                matches!(l.kind, LeafKind::Heading { .. }) && l.lines.last().is_some_and(|x| x.end == q)
            })
    }

    /// Block-level Backspace at `at` (a caret), or `None` for a plain deletion.
    fn block_start_backward(&self, at: usize) -> Option<BlockEdit> {
        let leaf = self.doc.leaf(at)?;
        let first = *leaf.lines.first()?;
        let text_like = matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. });
        // inside the leaf, at the start of a later content line: join with the line above
        if let Some(i) = leaf.lines.iter().position(|l| l.start == at)
            && i > 0
        {
            let prev = leaf.lines[i - 1];
            if !text_like {
                return None; // code etc.: the line break is a plain character
            }
            return Some(BlockEdit::Join(prev.end..at));
        }
        if at != first.start || at > leaf.span.end {
            return None;
        }
        if !text_like {
            return Some(BlockEdit::Nothing);
        }
        if let LeafKind::Heading { .. } = leaf.kind {
            // rule 4: the heading becomes a paragraph — its block syntax before the text (ATX
            // `## `) and after it (setext underline, ATX closing sequence) is removed
            let (lead, trail) = block_syntax(&leaf);
            if !lead.is_empty() || !trail.is_empty() {
                let patches = PatchSet::new(
                    [lead.clone(), trail]
                        .into_iter()
                        .filter(|r| !r.is_empty())
                        .map(|r| Patch::verbatim(r, ""))
                        .collect(),
                )
                .ok()?;
                return Some(BlockEdit::Edit {
                    patches,
                    label: "Paragraph",
                    caret: first.start - lead.len(),
                });
            }
        }
        // rule 5: merge with the previous paragraph/heading
        let Some(Some(prev)) = self.doc.adjacent_leaf(&leaf, false) else {
            return Some(BlockEdit::Nothing);
        };
        if prev.span.start >= leaf.span.start
            || !matches!(prev.kind, LeafKind::Paragraph | LeafKind::Heading { .. })
            || prev.containers != leaf.containers
            || in_list_item(&leaf)
        {
            return Some(BlockEdit::Nothing);
        }
        merge_blocks(self.doc.text(), &prev, &leaf, 0)
    }

    /// Block-level Delete at `at` (a caret), or `None` for a plain deletion.
    fn block_end_forward(&self, at: usize) -> Option<BlockEdit> {
        // at the end of the line after spaces typed after a heading's text: the end of the
        // heading, and a merge keeps those spaces (issue #16)
        let keep = at;
        let b = self.doc.text().as_bytes();
        let ws = b[..at].iter().rev().take_while(|c| matches!(c, b' ' | b'\t')).count();
        let at =
            if ws > 0 && matches!(b.get(at), None | Some(b'\n' | b'\r')) && self.in_heading_trailing_ws(at - ws..at) {
                at - ws
            } else {
                at
            };
        let leaf = self.doc.leaf(at)?;
        if leaf.span.end < at {
            return None; // between blocks
        }
        let last = *leaf.lines.last()?;
        let text_like = matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. });
        if let Some(i) = leaf.lines.iter().position(|l| l.end == at)
            && i + 1 < leaf.lines.len()
        {
            if !text_like {
                return None;
            }
            return Some(BlockEdit::Join(at..leaf.lines[i + 1].start));
        }
        if at != last.end {
            return None;
        }
        if !text_like {
            return Some(BlockEdit::Nothing);
        }
        // C48: merge the next paragraph/heading's text — the next block as the format's
        // structure gives it, never one past it (an empty quote, a list…: nothing)
        let t = self.doc.text();
        match self.doc.adjacent_leaf(&leaf, true) {
            Some(Some(n))
                if matches!(n.kind, LeafKind::Paragraph | LeafKind::Heading { .. })
                    && n.containers == leaf.containers
                    && !in_list_item(&leaf) =>
            {
                merge_blocks(t, &leaf, &n, keep)
            }
            _ => Some(BlockEdit::Nothing),
        }
    }

    /// One keystroke step. `leaf_start` is the leaf at the edit position before the keystroke;
    /// the open group continues only if it equals the leaf the previous keystroke ended in.
    fn keystroke(
        &mut self,
        dir: TypingDir,
        step: PlanStep,
        sel_after: Option<Selection>,
        leaf_start: Option<usize>,
    ) -> Result<(), EditError> {
        let now = self.clock.now();
        let pause = self.history.limits().typing_pause;
        let coalesce = self.open.as_ref().is_some_and(|o| {
            o.dir == dir
                && now.saturating_duration_since(o.last_at) <= pause
                && o.leaf_start == leaf_start
                && self.sel.is_caret()
                && self.sel.primary().focus.offset == o.caret
        });
        if !coalesce {
            self.close_typing();
        }
        let sel_before = self.sel.clone();
        let state_before = self.rev.state;
        let range_end = self.sel.primary().range().end;
        let st = self.apply_raw(&step.patches, step.hints_after)?;
        if st.is_noop() {
            // e.g. stepping over an auto-paired closer: the caret moves, the group stays
            if let Some(sel) = sel_after {
                self.sel = snap_selection(self.doc.text(), sel);
                if let Some(o) = self.open.as_mut() {
                    o.caret = self.sel.primary().focus.offset;
                }
            }
            return Ok(());
        }
        self.sel = snap_selection(
            self.doc.text(),
            sel_after.unwrap_or_else(|| Selection::caret(st.forward.map_offset(range_end, Affinity::Downstream))),
        );
        let state_after = self.fresh_state();
        let sel_now = self.sel.clone();
        if coalesce {
            self.history.amend_last(|tx| {
                let merged = tx.steps.last_mut().is_some_and(|last| merge_typing(last, &st, dir));
                if !merged {
                    tx.steps.push(st);
                }
                tx.sel_after = sel_now;
                tx.state_after = state_after;
            });
        } else {
            self.history.push(Transaction {
                label: Cow::Borrowed("Typing"),
                kind: TxKind::Typing(dir),
                steps: vec![st],
                sel_before,
                sel_after: sel_now,
                state_before,
                state_after,
            });
        }
        self.rev.state = state_after;
        // The group continues while keystrokes stay in the leaf the caret is in now.
        let caret = self.sel.primary().focus.offset;
        self.open = Some(OpenTyping {
            dir,
            leaf_start: self.leaf_info(caret).map(|l| l.0),
            last_at: now,
            caret,
        });
        self.changed();
        Ok(())
    }

    /// Applies a key planner's plan: with a label its own structural transaction, else a
    /// keystroke of the typing group (`dir`).
    fn apply_key_plan(&mut self, plan: Plan, dir: TypingDir) -> Result<(), EditError> {
        match plan.label {
            Some(label) => {
                self.close_typing();
                self.apply_plan(label, TxKind::Structural, plan)
            }
            None if plan.steps.len() <= 1 => {
                let leaf = self.leaf_info(self.sel.primary().start().offset).map(|l| l.0);
                let step = plan.steps.into_iter().next().unwrap_or_default();
                self.keystroke(dir, step, plan.selection_after, leaf)
            }
            None => self.apply_plan("Typing", TxKind::Structural, plan),
        }
    }

    /// `edit.softBreak` (Shift+Enter, spec 02 §5.1): the format's soft-break planner (its own
    /// transaction "New Line" when it acts as Enter, else part of the typing group); where it
    /// has none (code, math…) the Enter behaviour.
    pub fn soft_break(&mut self) -> Result<(), EditError> {
        if !self.sel.is_caret() {
            return self.newline();
        }
        match self.doc.plan(planners::SOFT_BREAK, &self.request(Params::new())) {
            Ok(plan) => {
                self.close_typing();
                let label = plan.label.unwrap_or("Line Break");
                self.apply_plan(label, TxKind::Structural, plan)
            }
            Err(_) => self.newline(),
        }
    }

    /// Tab / Shift+Tab (spec 02 §5.3): the format's indent/outdent rules (list items…);
    /// elsewhere Tab inserts a literal tab (not at a line start, where it would open
    /// indented code) and Shift+Tab does nothing.
    pub fn tab(&mut self, shift: bool) -> Result<(), EditError> {
        let id = if shift { planners::SHIFT_TAB } else { planners::TAB };
        match self.doc.plan(id, &self.request(Params::new())) {
            Ok(plan) => {
                let plan = if plan.label.is_none() {
                    plan.labeled(if shift { "Outdent" } else { "Indent" })
                } else {
                    plan
                };
                self.apply_key_plan(plan, TypingDir::Insert)
            }
            Err(_) if !shift => {
                let p = self.sel.primary();
                if p.is_caret() && (p.focus.virt > 0 || self.doc.is_line_start(p.focus.offset)) {
                    return Ok(());
                }
                if let Some(l) = self.doc.leaf(p.focus.offset)
                    && l.lines.first().is_some_and(|f| f.start == p.start().offset)
                {
                    return Ok(()); // the start of a block's text: a tab would change the block
                }
                self.insert_text("\t")
            }
            Err(_) => Ok(()),
        }
    }

    /// `edit.newline`: the format's Enter planner; where it has no rule (M2a: everything but
    /// hinted Enter-only lines) a plain line break. Always its own transaction.
    pub fn newline(&mut self) -> Result<(), EditError> {
        self.close_typing();
        if self.sel.is_caret() && self.sel.primary().focus.virt > 0 {
            // the format may move the virtual paragraph out of a container (spec 02 §5.1:
            // an empty paragraph in a quote leaves it); otherwise Enter waits (Q1)
            if let Ok(plan) = self.doc.plan(planners::NEWLINE, &self.request(Params::new())) {
                return self.apply_plan("New Line", TxKind::Structural, plan);
            }
            return Ok(());
        }
        if !self.sel.is_caret() {
            // Enter over a selection: delete it, then Enter at the caret — one transaction
            let r = self.sel.primary().range();
            return self.transact("New Line", TxKind::Structural, |tx| {
                tx.step(PlanStep::patches(PatchSet::delete(r.clone())))?;
                tx.set_selection(Selection::caret(r.start));
                let plan = match tx.plan(planners::NEWLINE, Params::new()) {
                    Ok(p) => p,
                    Err(EditError::Plan(PlanError::Unsupported | PlanError::NotApplicable(_))) => {
                        let eol = tx.doc().line_ending().as_str().to_string();
                        let caret = r.start + eol.len();
                        Plan::single(
                            PlanStep::patches(PatchSet::single_verbatim(r.start..r.start, eol)),
                            Some(Selection::caret(caret)),
                        )
                    }
                    Err(e) => return Err(e),
                };
                tx.apply_plan(plan)
            });
        }
        let req = self.request(Params::new());
        match self.doc.plan(planners::NEWLINE, &req) {
            Ok(plan) => self.apply_plan("New Line", TxKind::Structural, plan),
            Err(PlanError::Unsupported | PlanError::NotApplicable(_)) => {
                let r = self.sel.primary().range();
                let eol = self.doc.line_ending().as_str();
                let caret = r.start + eol.len();
                let plan = Plan::single(
                    PlanStep::patches(PatchSet::single_verbatim(r, eol)),
                    Some(Selection::caret(caret)),
                );
                self.apply_plan("New Line", TxKind::Structural, plan)
            }
            Err(e) => Err(e.into()),
        }
    }

    // ---- Transactions (ADR 0006 §4.4) ----

    /// Applies a plan as one transaction.
    pub fn apply_plan(
        &mut self,
        label: impl Into<Cow<'static, str>>,
        kind: TxKind,
        plan: Plan,
    ) -> Result<(), EditError> {
        self.transact(label, kind, |tx| tx.apply_plan(plan))
    }

    /// Runs `f` as one atomic transaction: steps apply immediately (later planners see earlier
    /// results); external effects are deferred. On error every applied step is inverted in
    /// reverse, live marks / selection / hints / `StateId` come back from the journal taken at
    /// the start, the counter is bumped (never reused) and the error is returned.
    pub fn transact<T>(
        &mut self,
        label: impl Into<Cow<'static, str>>,
        kind: TxKind,
        f: impl FnOnce(&mut Tx<'_>) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        self.close_typing();
        let journal = Journal {
            sel: self.sel.clone(),
            marks: self.marks.clone(),
            hints: self.doc.hints().clone(),
            state: self.rev.state,
        };
        let mut tx = Tx {
            s: self,
            steps: Vec::new(),
            deferred: Vec::new(),
        };
        let r = f(&mut tx);
        let Tx { steps, deferred, .. } = tx;
        match r {
            Ok(v) => {
                if !steps.is_empty() {
                    let state_after = self.fresh_state();
                    self.history.push(Transaction {
                        label: label.into(),
                        kind,
                        steps,
                        sel_before: journal.sel,
                        sel_after: self.sel.clone(),
                        state_before: journal.state,
                        state_after,
                    });
                    self.rev.state = state_after;
                    self.changed();
                }
                self.effects.extend(deferred);
                Ok(v)
            }
            Err(e) => {
                for step in steps.iter().rev() {
                    let inv = step.inverse();
                    if !inv.is_empty() {
                        self.doc_apply(&inv, Some(&step.hints_before));
                    }
                }
                if *self.doc.hints() != journal.hints {
                    self.doc_set_hints(&journal.hints);
                }
                self.sel = journal.sel;
                self.marks = journal.marks;
                self.rev.state = journal.state;
                self.rev.counter += 1;
                Err(e)
            }
        }
    }

    // ---- Undo / redo (ADR 0006 §4.1) ----

    /// Undoes the newest transaction: inverse steps in reverse, exact hints, `sel_before`.
    pub fn undo(&mut self) -> bool {
        self.close_typing();
        let Some(tx) = self.history.pop_undo() else {
            return false;
        };
        for step in tx.steps.iter().rev() {
            self.replay_inverse(step);
        }
        self.sel = tx.sel_before.clone();
        self.rev.state = tx.state_before;
        self.rev.counter += 1;
        self.history.push_redo(tx);
        self.changed();
        true
    }

    /// Redoes: forward steps in order, exact hints, `sel_after`.
    pub fn redo(&mut self) -> bool {
        self.close_typing();
        let Some(tx) = self.history.pop_redo() else {
            return false;
        };
        for step in &tx.steps {
            self.replay_forward(step);
        }
        self.sel = tx.sel_after.clone();
        self.rev.state = tx.state_after;
        self.rev.counter += 1;
        self.history.push_undo_keep_redo(tx);
        self.changed();
        true
    }

    // ---- Hints, save (ADR 0006 §2.6, §4.3) ----

    /// Expires every literal hint: one step materialising the format's escapes and removing the
    /// hints. Joins the open typing transaction (which closes) if there is one, otherwise it is
    /// its own `Auto` transaction "Escape Literal Syntax".
    pub fn expire_hints(&mut self) -> Result<(), EditError> {
        if self.doc.hints().is_empty() {
            return Ok(());
        }
        let mut patches = Vec::new();
        for h in self.doc.hints().iter() {
            if let Some(ps) = self.doc.materialize_hint(h) {
                patches.extend(ps.patches().iter().cloned());
            }
        }
        let step = PlanStep {
            patches: PatchSet::new(patches)?,
            hints_after: Some(HintSet::new()),
        };
        if self.open.is_some() && self.history.last().is_some() {
            let st = self.apply_raw(&step.patches, step.hints_after)?;
            let state_after = self.fresh_state();
            let sel_now = self.sel.clone();
            self.history.amend_last(|tx| {
                tx.steps.push(st);
                tx.sel_after = sel_now;
                tx.state_after = state_after;
            });
            self.rev.state = state_after;
            self.close_typing();
            self.changed();
            Ok(())
        } else {
            self.transact("Escape Literal Syntax", TxKind::Auto, |tx| tx.step(step))
        }
    }

    /// Prepares a save: materialises hints, closes the typing group (the saved state is a
    /// transaction boundary) and snapshots the hint-free text with its revision.
    pub fn prepare_save(&mut self) -> Result<SaveSnapshot, EditError> {
        self.expire_hints()?;
        self.close_typing();
        // the state being written is a permanent boundary from the start of the save, so
        // edits made while the save runs never join its transaction; kept even if the save
        // fails (a harmless extra undo step), so the order of completion never matters
        self.saved_ever.insert(self.rev.state);
        Ok(SaveSnapshot {
            text: self.doc.snapshot(),
            counter: self.rev.counter,
            state: self.rev.state,
        })
    }

    /// Records a successful save of `snap`.
    pub fn mark_saved(&mut self, snap: &SaveSnapshot) {
        self.saved_state = Some(snap.state);
        self.saved_ever.insert(snap.state);
    }

    /// Marks `state` as the one on disk: a save that finished after the saved text was
    /// installed under that state (e.g. a source-mode save whose rebuild landed first).
    pub fn mark_saved_state(&mut self, state: StateId) {
        self.saved_state = Some(state);
        self.saved_ever.insert(state);
    }

    /// No state of the history is on disk (a save wrote text the session has not reached yet).
    pub fn clear_saved_state(&mut self) {
        self.saved_state = None;
    }

    /// Marks the current state as the one on disk (after a reload from disk).
    pub fn mark_current_saved(&mut self) {
        self.saved_state = Some(self.rev.state);
        self.saved_ever.insert(self.rev.state);
    }

    /// Replaces the document object with `doc` (reload from disk, a rebuilt source-mode
    /// edit; ADR 0006 §4.3): recorded as one undoable transaction of the minimal byte diff
    /// (verbatim patches), hints dropped, live marks and the selection transformed. With
    /// `join`, the step is appended to the newest transaction when it has the same label, the
    /// current state was never saved and nothing else happened since (one "Source Edit" per
    /// source-mode segment between saves; a state that was ever saved stays a boundary for
    /// good, so undo reaches every saved state).
    /// Undo/redo of it replay the diff through `FormatDoc::apply`.
    pub fn replace_doc(
        &mut self,
        doc: Arc<dyn FormatDoc>,
        label: &'static str,
        kind: TxKind,
        join: bool,
    ) -> Result<(), EditError> {
        self.close_typing();
        let old = self.doc.text();
        let patches: Vec<Patch> = crate::edit::patches(old, doc.text())
            .into_iter()
            .map(|p| Patch::verbatim(p.start..p.end, p.insert))
            .collect();
        let forward = PatchSet::new(patches)?;
        let hints_before = self.doc.hints().clone();
        if forward.is_empty() && hints_before.is_empty() {
            self.doc = doc;
            self.change = DocChange::Replaced;
            return Ok(());
        }
        let removed = forward.removed_from(old);
        let sel_before = self.sel.clone();
        let state_before = self.rev.state;
        self.doc = doc;
        if !doc_hints_empty(&self.doc) {
            self.doc_mut().set_hints(&HintSet::new());
        }
        self.change = DocChange::Replaced;
        self.transform_live(&forward);
        self.rev.counter += 1;
        let step = Step {
            forward,
            removed,
            hints_before,
            hints_after: HintSet::new(),
        };
        self.record_bulk(step, label, kind, join, sel_before, state_before);
        Ok(())
    }

    /// Applies patches made outside the session's planners (Side by Side source edits; the
    /// patches are the document text's minimal diff, verbatim) as one step of a joinable
    /// transaction `label` (same join rule as [`EditorSession::replace_doc`]). Hints are
    /// dropped. The view model is updated incrementally from the change summary.
    pub fn apply_external(&mut self, patches: &PatchSet, label: &'static str, join: bool) -> Result<(), EditError> {
        self.close_typing();
        let sel_before = self.sel.clone();
        let state_before = self.rev.state;
        let st = self.apply_raw(patches, Some(HintSet::new()))?;
        if st.is_noop() {
            return Ok(());
        }
        self.record_bulk(st, label, TxKind::Bulk, join, sel_before, state_before);
        Ok(())
    }

    /// Records `step` as a new transaction, or joins it to the newest one when `join` and the
    /// newest has the same label, ends in the current state, nothing was undone and that state
    /// was never saved (saved states are permanent boundaries).
    fn record_bulk(
        &mut self,
        step: Step,
        label: &'static str,
        kind: TxKind,
        join: bool,
        sel_before: Selection,
        state_before: StateId,
    ) {
        let state_after = self.fresh_state();
        let sel_after = self.sel.clone();
        let joined = join
            && self.history.redo_len() == 0
            && !self.saved_ever.contains(&state_before)
            && self
                .history
                .last()
                .is_some_and(|t| t.label == label && t.state_after == state_before)
            && self
                .history
                .amend_last(|t| {
                    t.steps.push(step.clone());
                    t.sel_after = sel_after.clone();
                    t.state_after = state_after;
                })
                .is_some();
        if !joined {
            self.history.push(Transaction {
                label: Cow::Borrowed(label),
                kind,
                steps: vec![step],
                sel_before,
                sel_after,
                state_before,
                state_after,
            });
        }
        self.rev.state = state_after;
        self.changed();
    }
}

enum BlockEdit {
    /// Apply these patches as one structural transaction with this label; caret after.
    Edit {
        patches: PatchSet,
        label: &'static str,
        caret: usize,
    },
    /// Delete this range as a plain keystroke (joining soft lines inside a block).
    Join(std::ops::Range<usize>),
    /// The key does nothing here.
    Nothing,
}

/// The block syntax of a text leaf around its content: before the first content byte (ATX
/// `## `, after container prefixes) and after the last (setext underline line, ATX closing
/// sequence).
fn block_syntax(l: &markise_doc::LeafView) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let first = l.lines.first().map_or(l.span.start, |x| x.start);
    let last = l.lines.last().map_or(l.span.end, |x| x.end);
    (l.span.start..first.max(l.span.start), last.min(l.span.end)..l.span.end)
}

/// Merges text leaf `next` into `prev` (spec 02 §5.2 rule 5, C48): `next`'s content is
/// appended to `prev`'s; the merged block keeps `prev`'s block syntax (an ATX marker before,
/// a setext underline after the merged text) and drops `next`'s on both sides. One patch
/// replacing everything from `prev`'s content end to `next`'s end; the caret at the join.
/// A planner's selection must lie on char boundaries inside the text: a position that does
/// not is a planner bug (debug builds assert); it is snapped back to the previous boundary.
fn snap_selection(text: &str, sel: Selection) -> Selection {
    let snap = |p: DocPos| -> DocPos {
        let mut o = p.offset.min(text.len());
        debug_assert!(
            p.offset <= text.len() && text.is_char_boundary(p.offset),
            "selection off a char boundary: {} in {} bytes",
            p.offset,
            text.len()
        );
        while !text.is_char_boundary(o) {
            o -= 1;
        }
        DocPos { offset: o, ..p }
    };
    let ok = sel.ranges().iter().all(|r| {
        [r.anchor, r.focus]
            .iter()
            .all(|p| p.offset <= text.len() && text.is_char_boundary(p.offset))
    });
    if ok {
        return sel;
    }
    let p = sel.primary();
    let mut out = Selection::single(snap(p.anchor), snap(p.focus));
    out.goal_x = sel.goal_x;
    out
}

/// `keep`: a caret after spaces that follow `prev`'s text outside its span (an open ATX
/// heading): the join goes after them, so they stay (else 0).
fn merge_blocks(
    text: &str,
    prev: &markise_doc::LeafView,
    next: &markise_doc::LeafView,
    keep: usize,
) -> Option<BlockEdit> {
    let (_, prev_trail) = block_syntax(prev);
    let (_, next_trail) = block_syntax(next);
    let first = next.lines.first()?.start;
    let last = next.lines.last()?.end;
    let join = if prev_trail.is_empty() {
        prev_trail.start.max(keep)
    } else {
        prev_trail.start
    };
    let insert = format!("{}{}", &text[first..last], &text[prev_trail.clone()]);
    let patches = PatchSet::new(vec![Patch::verbatim(join..next_trail.end.max(last), insert)]).ok()?;
    Some(BlockEdit::Edit {
        patches,
        label: "Merge Paragraphs",
        caret: join,
    })
}

/// Merging across list items dissolves an item (spec 02 §5.2 rule 3, S-5.4): M2c. Until then
/// the boundary is a no-op.
fn in_list_item(l: &markise_doc::LeafView) -> bool {
    l.containers
        .iter()
        .any(|c| matches!(c, markise_doc::ContainerKind::ListItem { .. }))
}

fn doc_hints_empty(d: &Arc<dyn FormatDoc>) -> bool {
    d.hints().is_empty()
}

fn prev_grapheme(t: &str, at: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    let from = t[..at].char_indices().rev().nth(32).map_or(0, |(i, _)| i);
    t[from..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(from, |(i, _)| from + i)
}

fn next_grapheme(t: &str, at: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    let to = t[at..].char_indices().nth(32).map_or(t.len(), |(i, _)| at + i);
    t[at..to].graphemes(true).next().map_or(to, |g| at + g.len())
}

/// Typing coalescing (ADR 0006 §4.1): a keystroke adjacent to the last step's insert (append)
/// or removal (Backspace prepends, Delete appends) extends that patch in place.
fn merge_typing(last: &mut Step, new: &Step, dir: TypingDir) -> bool {
    if last.hints_after != new.hints_before
        || new.hints_before != new.hints_after
        || last.forward.len() != 1
        || new.forward.len() != 1
    {
        return false;
    }
    let q = &new.forward.patches()[0];
    let q_removed = &new.removed[0];
    let mut p = last.forward.patches()[0].clone();
    let removed = match dir {
        TypingDir::Insert if q.range.is_empty() && q.range.start == p.range.start + p.insert.len() => {
            p.insert.push_str(&q.insert);
            last.removed[0].clone()
        }
        TypingDir::DeleteBackward if p.insert.is_empty() && q.insert.is_empty() && q.range.end == p.range.start => {
            p.range.start = q.range.start;
            Arc::from(format!("{q_removed}{}", last.removed[0]))
        }
        TypingDir::DeleteForward if p.insert.is_empty() && q.insert.is_empty() && q.range.start == p.range.start => {
            p.range.end += q.range.len();
            Arc::from(format!("{}{q_removed}", last.removed[0]))
        }
        _ => return false,
    };
    // rebuilt through the checked constructor (invariants re-verified)
    match PatchSet::new(vec![p]) {
        Ok(ps) if ps.len() == 1 => {
            last.forward = ps;
            last.removed[0] = removed;
            true
        }
        _ => false,
    }
}

/// An open transaction (see [`EditorSession::transact`]).
pub struct Tx<'a> {
    s: &'a mut EditorSession,
    steps: Vec<Step>,
    deferred: Vec<SessionEffect>,
}

impl Tx<'_> {
    pub fn doc(&self) -> &dyn FormatDoc {
        self.s.doc.as_ref()
    }
    pub fn selection(&self) -> &Selection {
        &self.s.sel
    }
    pub fn request(&self, params: Params) -> PlanRequest {
        self.s.request(params)
    }
    /// Runs a planner against the current (partially edited) document.
    pub fn plan(&self, id: markise_doc::PlannerId, params: Params) -> Result<Plan, EditError> {
        Ok(self.s.doc.plan(id, &self.s.request(params))?)
    }
    /// Applies one step now.
    pub fn step(&mut self, step: PlanStep) -> Result<(), EditError> {
        let st = self.s.apply_raw(&step.patches, step.hints_after)?;
        if !st.is_noop() {
            self.steps.push(st);
        }
        Ok(())
    }
    /// Applies every step of `plan`, then its `selection_after`.
    pub fn apply_plan(&mut self, plan: Plan) -> Result<(), EditError> {
        for st in plan.steps {
            self.step(st)?;
        }
        if let Some(sel) = plan.selection_after {
            self.s.sel = snap_selection(self.s.doc.text(), sel);
        }
        Ok(())
    }
    pub fn set_selection(&mut self, sel: Selection) {
        self.s.sel = sel;
    }
    pub fn mark(&self, id: MarkId) -> Option<DocPos> {
        self.s.mark(id)
    }
    /// Queues an external effect, run only if the transaction commits.
    pub fn defer(&mut self, effect: SessionEffect) {
        self.deferred.push(effect);
    }
    /// Number of steps applied so far.
    pub fn applied(&self) -> usize {
        self.steps.len()
    }
}
