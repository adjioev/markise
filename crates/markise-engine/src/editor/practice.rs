//! Blocks in Practice (ADR 0024). A practised block's text does not change by anything done in it:
//! typing and block commands are off, and what someone answers is never in the document.

use std::ops::Range;

use markise_doc::{DocPos, Selection};

use super::EditorView;
use super::command::CommandError;

const IN_PRACTICE: CommandError = CommandError::Disabled("the block is being practiced");

impl EditorView {
    /// Whether the block around `at` is in Practice.
    pub fn in_practice(&self, at: usize) -> bool {
        self.view.practised_around(at).is_some()
    }

    /// Whether changing source range `r` would change a block in Practice. Over a long range this
    /// answers by what the range holds: any block's marker line, while any block is in Practice.
    pub fn practice_touches(&self, r: Range<usize>) -> bool {
        if !self.view.practice_on() {
            return false;
        }
        if self.in_practice(r.start) || self.in_practice(r.end) {
            return true;
        }
        self.session.text().get(r).is_some_and(|t| t.contains(":::"))
    }

    /// A command that would change a block in Practice does nothing. Undo and redo are the document's,
    /// of what was done before; the other commands (a caret's moves) run as ever.
    pub(super) fn practice_refuses(&self, id: &str) -> Option<Result<(), CommandError>> {
        if !self.view.practice_on() || matches!(id, "edit.undo" | "edit.redo") || !self.is_mutating(id) {
            return None;
        }
        let touched = (self.session.selection().ranges().iter()).any(|s| self.practice_touches(s.range()));
        touched.then_some(Err(IN_PRACTICE))
    }

    /// A caret on what Practice hides stands after the block instead. It would show nothing there, and
    /// nothing is there to do.
    pub(super) fn practice_selection(&self, sel: Selection) -> Selection {
        let p = sel.primary();
        if !self.view.practice_on() || !p.is_caret() || !self.view.practice_conceals(p.focus.offset) {
            return sel;
        }
        let Some(c) = self.view.practised_around(p.focus.offset) else {
            return sel;
        };
        match self.session.doc().gap_virt(c.closer.end) {
            Some(virt) => Selection::caret_at(DocPos::with_virt(c.closer.end, virt)),
            None => Selection::caret(c.closer.end),
        }
    }

    /// After a state changed: the selection as the new state allows it.
    pub(super) fn practice_place_caret(&mut self) {
        if self.view.practice_on() {
            let sel = self.session.selection().clone();
            self.set_selection(sel);
        }
    }

    /// Whether the document as a whole is in Practice.
    pub fn practice_whole(&self) -> bool {
        self.place.mode == markise_layout::BlockViewMode::Practice
    }

    /// Practice for the whole document, or out of it. A block with no practice is as it was.
    pub fn toggle_practice_all(&mut self) {
        self.place.mode = if self.practice_whole() {
            markise_layout::BlockViewMode::Native
        } else {
            markise_layout::BlockViewMode::Practice
        };
        self.clear_own_modes();
        self.follow_block_modes();
        self.practice_place_caret();
    }
}
