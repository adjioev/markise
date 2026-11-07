//! Moving and deleting a section as one unit (ADR 0026), by what moving a block does (ADR 0022): one undo
//! step, every section's bytes as they are, only blank lines around them written. A boundary belongs to the
//! list of sections and not to what follows it: the first section has none and stays with none, whoever
//! comes to be first. Metadata is in a section's bytes, so it stays first in it.

use std::ops::Range;

use markise_doc::{DocPos, FormatDoc, HintSet, Patch, PatchSet, PlanStep, Selection};

use super::block_move::{MoveRefused, lift};
use super::block_place::{after_line, blank_line_at, blocks_of, line_end, separated};
use super::section_edit::holds_more_than_metadata;
use super::{EditError, EditorView, TxKind};

const MOVE_LABEL: &str = "Move Section";
const DELETE_LABEL: &str = "Delete Section";

/// A section's bytes.
struct Part {
    /// Its boundary, which the first section does not have.
    boundary: Option<Range<usize>>,
    /// From the first line with something on it to the end of the last, its line ending not included. Blank
    /// lines around it are no part of it. Empty where it holds nothing.
    body: Range<usize>,
}

impl Part {
    /// All it is when it goes: from its boundary, if any, to its end.
    fn whole(&self) -> Range<usize> {
        self.boundary.as_ref().map_or(self.body.start, |b| b.start)..self.body.end
    }
}

fn part(doc: &dyn FormatDoc, index: usize) -> Option<Part> {
    let s = doc.section(index)?;
    let text = doc.text();
    let at_line = s.start == 0 || text[..s.start].ends_with(['\r', '\n']);
    let mut from = if at_line { s.start } else { after_line(text, s.start) };
    while from < s.end && blank_line_at(text, from) {
        from = after_line(text, from);
    }
    let boundary = s.boundary.map(|b| b.start..b.end);
    if from >= s.end {
        // nothing in it: it ends where its boundary does, or where the next one starts
        let at = boundary.as_ref().map_or(s.end, |b| b.end);
        return Some(Part { boundary, body: at..at });
    }
    // the blanks of its last line are that line's
    let last = from + text[from..s.end].trim_end_matches([' ', '\t', '\r', '\n']).len();
    Some(Part {
        boundary,
        body: from..line_end(text, last).min(s.end),
    })
}

/// What moving a section comes to.
struct Planned {
    patches: Vec<Patch>,
    /// Where the lower section's body stands afterwards.
    lower_at: usize,
    /// How far the upper section's bytes go down.
    upper_shift: usize,
}

/// `lower` goes above `upper`, which it follows: whichever of them was asked for, the other one's bytes are
/// not touched. `None`: one of them holds nothing and is first, so there is no way to change places without
/// a boundary where no section ends.
fn plan_past(text: &str, upper: &Part, lower: &Part, eol: &str) -> Option<Result<Planned, MoveRefused>> {
    let body = &text[lower.body.clone()];
    let boundary = lower.boundary.as_ref()?;
    let (moved, to, body_in_moved) = match &upper.boundary {
        Some(above) if body.is_empty() => ("---".to_string(), above.start, 3),
        Some(above) => (format!("---{eol}{eol}{body}"), above.start, 3 + 2 * eol.len()),
        None if upper.body.is_empty() || body.is_empty() => return None,
        None => (format!("{body}{eol}{eol}---"), upper.body.start, 0),
    };
    let (insert, skip) = separated(text, to, &moved, eol);
    let (remove, keep) = match lift(text, &(boundary.start..lower.body.end), eol) {
        Ok(lifted) => lifted,
        Err(why) => return Some(Err(why)),
    };
    let upper_shift = insert.len();
    let mut patches = vec![
        Patch::verbatim(remove, keep.to_string()),
        Patch::verbatim(to..to, insert),
    ];
    patches.sort_by_key(|p| p.range.start);
    Some(Ok(Planned {
        patches,
        lower_at: to + skip + body_in_moved,
        upper_shift,
    }))
}

/// The section has nothing in it, or only metadata.
fn is_hollow(doc: &dyn FormatDoc, index: usize) -> bool {
    doc.section(index)
        .is_some_and(|s| !holds_more_than_metadata(doc, s, s.end))
}

impl EditorView {
    /// The section holding the caret, by its place.
    pub fn section_at_caret(&self) -> usize {
        let at = self.session.selection().primary().focus.offset;
        self.session.doc().section_at(at).index
    }

    /// [`Self::section_at_caret`], by the caret of the pane the keyboard is in.
    pub fn section_in_pane(&mut self, in_source: bool) -> usize {
        if in_source {
            self.source_selection_to_session();
        }
        self.section_at_caret()
    }

    /// The two sections a move of `index` swaps: the upper one first.
    fn section_pair(&self, index: usize, up: bool) -> Option<(usize, usize)> {
        let upper = if up { index.checked_sub(1)? } else { index };
        (upper + 1 < self.session.doc().section_count()).then_some((upper, upper + 1))
    }

    /// Section `index` has a neighbour to go past that way.
    pub fn can_move_section(&self, index: usize, up: bool) -> bool {
        let doc = self.session.doc();
        self.section_pair(index, up).is_some_and(|(upper, lower)| {
            let parts = part(doc, upper).zip(part(doc, lower));
            parts.is_some_and(|(a, b)| a.boundary.is_some() || !(a.body.is_empty() || b.body.is_empty()))
        })
    }

    /// Moves section `index` past the neighbouring section. `Ok(false)`: there is nothing to go past. Refused
    /// if anything would pair or divide differently afterwards. One undo step; the caret stays in what it was
    /// in, or goes to the start of the section it asked for.
    pub fn move_section(&mut self, index: usize, up: bool, in_source: bool) -> Result<bool, MoveRefused> {
        if in_source {
            self.source_selection_to_session();
        }
        if !self.can_move_section(index, up) {
            return Ok(false);
        }
        let Some((upper, lower)) = self.section_pair(index, up) else {
            return Ok(false);
        };
        let doc = self.session.doc();
        let text = doc.text();
        let (Some(a), Some(b)) = (part(doc, upper), part(doc, lower)) else {
            return Ok(false);
        };
        // a site refuses such a section: it is for filling in or deleting, not for moving around
        if is_hollow(doc, upper) || is_hollow(doc, lower) {
            return Err(MoveRefused::WouldLeaveEmpty);
        }
        let eol = doc.line_ending().as_str();
        let Some(planned) = plan_past(text, &a, &b, eol) else {
            return Ok(false);
        };
        let Planned {
            patches,
            lower_at,
            upper_shift,
        } = planned?;
        let patches = PatchSet::new(patches).map_err(|e| MoveRefused::Edit(EditError::Patch(e)))?;
        let (body_a, body_b) = (text[a.body.clone()].to_string(), text[b.body.clone()].to_string());
        let upper_at = a.body.start + upper_shift;
        let want = blocks_of(doc);
        let sections = doc.section_count();
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        // what is in either of them goes with it; on a boundary or a blank line, to the section asked for
        let place = |p: DocPos| -> Option<DocPos> {
            if p.offset >= b.body.start && p.offset <= b.body.end && !(p.virt > 0 && p.offset == b.body.end) {
                Some(DocPos {
                    offset: p.offset - b.body.start + lower_at,
                    ..p
                })
            } else if p.offset >= a.body.start && p.offset <= a.body.end && !(p.virt > 0 && p.offset == a.body.end) {
                Some(DocPos {
                    offset: p.offset + upper_shift,
                    ..p
                })
            } else {
                None
            }
        };
        let sel = self.session.selection().primary();
        let own_start = if index == lower { lower_at } else { upper_at };
        let sel_after = match (place(sel.anchor), place(sel.focus)) {
            (Some(x), Some(y)) => Selection::single(x, y),
            _ => Selection::caret_at(DocPos::new(own_start)),
        };
        // a block's state goes with its bytes (ADR 0021): only the lower section's bytes move
        let modes: Vec<_> = (self.own_modes_now().into_iter())
            .filter(|(at, _)| b.body.contains(at))
            .map(|(at, mode)| (at - b.body.start + lower_at, mode))
            .collect();
        let sticky = self.sticky_at();
        let sticky_after = sticky
            .filter(|at| b.body.contains(at))
            .map(|at| at - b.body.start + lower_at);
        self.release();
        let done = self.session.transact(MOVE_LABEL, TxKind::Structural, |tx| {
            tx.step(PlanStep { patches, hints_after })?;
            let doc = tx.doc();
            let text = doc.text();
            let stands = text.get(lower_at..lower_at + body_b.len()) == Some(body_b.as_str())
                && text.get(upper_at..upper_at + body_a.len()) == Some(body_a.as_str())
                && doc.section_count() == sections
                && doc.section_at(lower_at).index == upper
                && doc.section_at(upper_at).index == lower;
            if !stands || blocks_of(doc) != want {
                return Err(EditError::Aborted(String::new()));
            }
            tx.set_selection(sel_after);
            Ok(())
        });
        self.sync();
        match done {
            Ok(()) => {}
            Err(EditError::Aborted(_)) => return Err(MoveRefused::WouldNotStand),
            Err(e) => return Err(MoveRefused::Edit(e)),
        }
        for (at, mode) in modes {
            self.add_own_mode(at, mode);
        }
        if let Some(at) = sticky_after {
            self.end_sticky();
            self.begin_sticky(at);
        }
        self.follow_sticky();
        self.follow_block_modes();
        self.pin_caret();
        if in_source {
            self.source_selection_from_session();
        }
        Ok(true)
    }

    /// Section `index` can go: another one stays. A document is not emptied by this.
    pub fn can_delete_section(&self, index: usize) -> bool {
        let count = self.session.doc().section_count();
        count > 1 && index < count
    }

    /// All the lines of section `index`, its boundary included.
    pub fn section_lines(&self, index: usize) -> Option<Range<usize>> {
        part(self.session.doc(), index).map(|p| p.whole())
    }

    /// What deleting section `index` takes: its bytes, its boundary, or for the first section, which has none,
    /// the next one's, so that the next one comes to be first.
    fn section_to_delete(&self, index: usize) -> Option<Range<usize>> {
        let doc = self.session.doc();
        let p = part(doc, index)?;
        match p.boundary {
            Some(_) => Some(p.whole()),
            None => {
                let next = doc.section(index + 1)?.boundary?;
                Some(p.body.start.min(next.start)..next.end)
            }
        }
    }

    /// Deletes section `index`, one undo step with no question. `Ok(false)`: it is the only one, or there
    /// is no such section. Refused if anything would pair or divide differently afterwards.
    pub fn delete_section(&mut self, index: usize, in_source: bool) -> Result<bool, MoveRefused> {
        if in_source {
            self.source_selection_to_session();
        }
        if !self.can_delete_section(index) {
            return Ok(false);
        }
        let Some(b) = self.section_to_delete(index) else {
            return Ok(false);
        };
        let sections = self.session.doc().section_count() - 1;
        self.delete_range_as(b, DELETE_LABEL, Some(sections))?;
        if in_source {
            self.source_selection_from_session();
        }
        Ok(true)
    }
}
