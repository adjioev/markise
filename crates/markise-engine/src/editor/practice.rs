//! Blocks in Practice (ADR 0024). A practised block's text does not change by anything done in it:
//! typing and block commands are off, and what someone answers is never in the document.

use std::ops::Range;

use markise_doc::{DocPos, Params, Selection};
use markise_layout::{BlockViewMode, BlockViews, NativeAt, PracticeShow};

use super::EditorView;
use super::command::CommandError;

const IN_PRACTICE: CommandError = CommandError::Disabled("the block is being practiced");

/// What was done in one block. A block is who stands at this offset with this length: every edit
/// moves the offset, and any edit that touches those bytes ends this (ADR 0024).
pub(super) struct Practised {
    at: usize,
    len: usize,
    show: PracticeShow,
    /// Once checked, with answers to check: whether they were the right ones.
    right: Option<bool>,
}

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

    /// A command in a block in Practice: Space selects the answer at the caret and Enter checks, or
    /// resets what is checked. Any other that would change the block does nothing. Undo and redo are the
    /// document's, of what was done before; the other commands (a caret's moves) run as ever.
    pub(super) fn practice_command(&mut self, id: &str, params: &Params) -> Option<Result<(), CommandError>> {
        if !self.view.practice_on() || matches!(id, "edit.undo" | "edit.redo") || !self.is_mutating(id) {
            return None;
        }
        let p = self.session.selection().primary();
        if p.is_caret() && p.focus.virt == 0 && self.in_practice(p.focus.offset) {
            match id {
                "edit.insertText" if params.get_text("text") == Some(" ") => {
                    self.practice_press(p.focus.offset);
                    return Some(Ok(()));
                }
                "edit.newline" => {
                    if let Some(at) = self.view.native_block(p.focus.offset) {
                        if self.practice_show(at.range.start).is_some_and(|s| s.checked) {
                            self.practice_reset(at.range.start);
                        } else {
                            self.practice_check(at.range.start);
                        }
                    }
                    return Some(Ok(()));
                }
                _ => {}
            }
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
        self.place.mode == BlockViewMode::Practice || self.practice_by_default()
    }

    /// The package opens blocks in Practice, and no one has chosen otherwise for the document.
    pub(super) fn practice_by_default(&self) -> bool {
        !self.place.defaults_off && self.place.fields.as_ref().is_some_and(|f| f.any_practice_default())
    }

    /// The state of the document, for a block of this name with none of its own.
    pub(super) fn document_mode(&self, name: &str) -> BlockViewMode {
        let by_default =
            !self.place.defaults_off && self.place.fields.as_ref().is_some_and(|f| f.practice_default(name));
        if by_default {
            BlockViewMode::Practice
        } else {
            self.place.mode
        }
    }

    /// For what remembers a document's state: whether someone chose for the document as a whole.
    pub fn practice_defaults_off(&self) -> bool {
        self.place.defaults_off
    }

    pub fn set_practice_defaults_off(&mut self, off: bool) {
        self.place.defaults_off = off;
        self.follow_block_modes();
    }

    /// A block being written here stays what it can be written in. With no such thing, a block that
    /// opens in Practice would close on the first character of it. `edits`: as for [`Self::prune_practice`].
    pub(super) fn keep_written_block_native(&mut self, edits: &[(usize, usize, isize)]) {
        if !self.practice_by_default() {
            return;
        }
        let at = self.session.selection().primary().focus.offset;
        let Some(c) = self.session.doc().named_containers_at(at).pop() else {
            return;
        };
        let written = (edits.iter()).any(|&(start, end, delta)| {
            let end = end.saturating_add_signed(delta).max(start);
            start <= c.opener.end && c.opener.start <= end
        });
        if written && self.document_mode(&c.name) == BlockViewMode::Practice && !self.has_own_mode(c.opener.start) {
            self.add_own_mode(c.opener.start, BlockViewMode::Native);
        }
    }

    /// Practice for the whole document, or out of it. A block with no practice is as it was.
    pub fn toggle_practice_all(&mut self) {
        self.place.mode = if self.practice_whole() {
            BlockViewMode::Native
        } else {
            BlockViewMode::Practice
        };
        self.place.defaults_off = true;
        self.clear_own_modes();
        self.follow_block_modes();
        self.practice_place_caret();
    }

    /// What was done in the block with this opener.
    pub fn practice_show(&self, opener: usize) -> Option<&PracticeShow> {
        (self.place.practice.iter()).find(|p| p.at == opener).map(|p| &p.show)
    }

    /// The block in Practice around `at`, ready to be practised.
    fn practice_block(&self, at: usize) -> Option<NativeAt> {
        self.view
            .native_block(at)
            .filter(|b| self.view.block_mode_of(b.range.start) == BlockViewMode::Practice)
    }

    fn practice_entry(&mut self, block: &NativeAt) -> &mut Practised {
        let opener = block.range.start;
        let i = match self.place.practice.iter().position(|p| p.at == opener) {
            Some(i) => i,
            None => {
                self.place.practice.push(Practised {
                    at: opener,
                    len: block.range.len(),
                    show: PracticeShow::default(),
                    right: None,
                });
                self.place.practice.len() - 1
            }
        };
        &mut self.place.practice[i]
    }

    /// Selects the answer at `at`, or for one of several takes it back. Returns whether it is an answer
    /// that can still change. Nothing in the document changes.
    pub fn practice_press(&mut self, at: usize) -> bool {
        let Some(block) = self.practice_block(at) else {
            return false;
        };
        let Some(parts) = &block.block.practice else {
            return false;
        };
        let Some(n) = parts.answer_at((at - block.range.start) as u32).map(|n| n as u32) else {
            return false;
        };
        let single = block.block.view.single_answer;
        let show = &mut self.practice_entry(&block).show;
        if show.checked {
            return false;
        }
        match show.selected.iter().position(|&s| s == n) {
            Some(_) if single => {}
            Some(i) => {
                show.selected.remove(i);
            }
            None if single => show.selected = vec![n],
            None => {
                show.selected.push(n);
                show.selected.sort_unstable();
            }
        }
        if self.place.practice.iter().any(|p| p.show == PracticeShow::default()) {
            self.place.practice.retain(|p| p.show != PracticeShow::default());
        }
        self.follow_practice();
        true
    }

    /// Checks the block with this opener, which also reveals what it hides. With answers to give, some
    /// have to be given.
    pub fn practice_check(&mut self, opener: usize) -> bool {
        let Some(block) = self.practice_block(opener) else {
            return false;
        };
        let Some(parts) = &block.block.practice else {
            return false;
        };
        let answered = self.practice_show(opener).is_some_and(|s| !s.selected.is_empty());
        if parts.has_choices && !answered {
            return false;
        }
        let right = parts.has_choices.then(|| {
            let correct = (parts.answers.iter().enumerate())
                .filter(|(_, a)| a.correct)
                .map(|(n, _)| n as u32);
            let selected = self.practice_show(opener).map_or(&[][..], |s| &s.selected);
            correct.eq(selected.iter().copied())
        });
        let entry = self.practice_entry(&block);
        entry.show.checked = true;
        entry.right = right;
        self.follow_practice();
        true
    }

    /// As if the block with this opener had not been practised.
    pub fn practice_reset(&mut self, opener: usize) {
        self.place.practice.retain(|p| p.at != opener);
        self.follow_practice();
        self.practice_place_caret();
    }

    /// How the blocks checked so far went: how many were right, of how many. A block with no answers
    /// to give counts for nothing.
    pub fn practice_score(&self) -> (usize, usize) {
        let checked = self.place.practice.iter().filter_map(|p| p.right);
        checked.fold((0, 0), |(right, all), r| (right + usize::from(r), all + 1))
    }

    /// Whether anything has been done that Reset All would take back.
    pub fn practice_started(&self) -> bool {
        !self.place.practice.is_empty()
    }

    /// As if nothing had been practised in the document. Blocks may stand anywhere, on pages long gone.
    pub fn practice_reset_all(&mut self) {
        let revealed = self.place.practice.iter().any(|p| p.show.checked);
        self.place.practice.clear();
        self.follow_practice();
        if revealed {
            self.view.practice_forgotten();
        }
        self.practice_place_caret();
    }

    /// Hands the view what was done, where the blocks stand now.
    pub(super) fn follow_practice(&mut self) {
        let shows = (self.place.practice.iter()).map(|p| (p.at, p.show.clone())).collect();
        self.view.set_block_practice(shows);
    }

    /// After the document changed: what was done goes with any block an edit touched. `edits`: `(start,
    /// end, delta)` of each step, each in the coordinates after the ones before. None: the document was replaced.
    pub(super) fn prune_practice(&mut self, edits: &[(usize, usize, isize)]) {
        if edits.is_empty() {
            self.place.practice.clear();
            return;
        }
        self.place.practice.retain_mut(|p| {
            for &(start, end, delta) in edits {
                if end <= p.at {
                    p.at = p.at.saturating_add_signed(delta);
                } else if start < p.at + p.len {
                    return false;
                }
            }
            true
        });
    }
}
