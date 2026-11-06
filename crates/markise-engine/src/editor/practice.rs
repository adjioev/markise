//! Read and Edit, and blocks in Practice (ADR 0024). Read is a document's state: the session refuses
//! every change, and what someone answers is never in the document.

use std::ops::Range;

use markise_doc::{DocPos, Selection};
use markise_layout::{BlockViewMode, NativeAt, PracticeShow};

use super::EditorView;

/// Why the session refuses a change in Read.
pub const READ_ONLY_REASON: &str = "Read Mode is on: switch to Edit to change the document.";

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

    /// Whether the document is in Read.
    pub fn read_mode(&self) -> bool {
        self.place.read
    }

    /// Read or Edit, by someone's choice, which a package then has no say over.
    pub fn set_read_mode(&mut self, read: bool) {
        self.place.read_chosen = true;
        self.apply_read(read);
    }

    fn apply_read(&mut self, read: bool) {
        self.place.read = read;
        self.session.close_typing();
        self.session.set_read_only(read.then_some(READ_ONLY_REASON));
        self.end_sticky();
        self.view.set_read_mode(read);
        self.follow_block_modes();
        self.practice_place_caret();
    }

    /// What is remembered of a document's state: `Some` when someone chose.
    pub fn read_choice(&self) -> Option<bool> {
        self.place.read_chosen.then_some(self.place.read)
    }

    /// What was remembered of a document's state comes back.
    pub fn restore_read_choice(&mut self, choice: Option<bool>) {
        if let Some(read) = choice {
            self.set_read_mode(read);
        }
    }

    /// A package can say that a document holding some of its blocks opens in Read. One that says so was
    /// not looked up yet when the document came, or the document has only now come whole.
    pub(super) fn open_as_the_package_says(&mut self) {
        if self.place.read_chosen || self.place.read {
            return;
        }
        let Some(names) = self.place.fields.as_ref().map(|f| f.read_by_default().to_vec()) else {
            return;
        };
        if names.iter().any(|name| self.holds_block(name)) {
            self.apply_read(true);
        }
    }

    /// Whether the document holds a block of this name.
    fn holds_block(&self, name: &str) -> bool {
        let text = self.session.text();
        let doc = self.session.doc();
        let wanted = format!(":::{name}");
        let mut from = 0;
        while let Some(i) = text[from..].find(&wanted) {
            let at = from + i;
            let around = doc.named_containers_at(at + 1);
            if around
                .iter()
                .any(|c| &*c.name == name && c.opener.start <= at && at < c.opener.end)
            {
                return true;
            }
            from = at + wanted.len();
        }
        false
    }

    /// Space in Read: the answer at the caret. `true`: something happened.
    pub fn practice_space(&mut self) -> bool {
        let p = self.session.selection().primary();
        p.is_caret() && p.focus.virt == 0 && self.practice_press(p.focus.offset)
    }

    /// Return in Read: checks the block at the caret, or resets what is checked.
    pub fn practice_enter(&mut self) -> bool {
        let p = self.session.selection().primary();
        if !p.is_caret() || p.focus.virt > 0 {
            return false;
        }
        let Some(at) = self.practice_block(p.focus.offset) else {
            return false;
        };
        let opener = at.range.start;
        if self.practice_show(opener).is_some_and(|s| s.checked) {
            self.practice_reset(opener);
            true
        } else {
            self.practice_check(opener)
        }
    }

    /// The text of source range `r` for the clipboard, with what Practice hides left out.
    pub fn practice_copy(&self, r: Range<usize>) -> String {
        let text = self.session.text();
        let mut cuts = Vec::new();
        let mut pos = r.start;
        while pos < r.end {
            // the outermost block in Practice standing here
            let around = self.session.doc().named_containers_at(pos);
            let block = (around.iter()).find(|c| c.opener.start <= pos && self.in_practice(c.opener.start));
            if let Some(c) = block {
                cuts.extend(self.view.practice_copy_cuts(c));
                pos = c.closer.end.max(pos + 1);
                continue;
            }
            match text[pos..r.end].find(":::") {
                // a marker line is where anything changes; past this one if we stand on it
                Some(0) => pos += 3,
                Some(i) => pos += i,
                None => break,
            }
        }
        let mut out = String::with_capacity(r.len());
        let mut at = r.start;
        for (cut, instead) in cuts {
            let (start, end) = (cut.start.max(r.start), cut.end.min(r.end));
            if start >= end || start < at {
                continue;
            }
            out.push_str(&text[at..start]);
            // (part of a mark's source is not a mark)
            if end - start == cut.len() {
                out.push_str(instead);
            }
            at = end;
        }
        out.push_str(&text[at..r.end]);
        out
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
        self.place.practice_score
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

    /// The document was replaced whole, past the edits that say what moved: a reload from disk or an edit
    /// made as source. What was done is lost with it.
    pub fn practice_document_replaced(&mut self) {
        self.place.practice.clear();
        self.follow_practice();
    }

    /// Hands the view what was done, where the blocks stand now.
    pub(super) fn follow_practice(&mut self) {
        let checked = self.place.practice.iter().filter_map(|p| p.right);
        self.place.practice_score = checked.fold((0, 0), |(right, all), r| (right + usize::from(r), all + 1));
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
