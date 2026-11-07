//! Moving and deleting a section as one unit (ADR 0026), by what moving a block does (ADR 0022): one undo
//! step, every section's bytes as they are, only blank lines around them written. A boundary belongs to the
//! list of sections and not to what follows it: the first section has none and stays with none, whoever
//! comes to be first. Metadata is in a section's bytes, so it stays first in it.

use std::collections::BTreeSet;
use std::ops::Range;

use markise_doc::{DocPos, FormatDoc, HintSet, LeafKind, Patch, PatchSet, PlanStep, Selection, Span};

use super::block_move::{MoveRefused, lift};
use super::block_place::{after_line, blank_line_at, blocks_in, blocks_of, line_end, separated, trailing_ids_in};
use super::section_edit::holds_more_than_metadata;
use super::{EditError, EditorView, TxKind};
use crate::section_id::{next_section_id, section_id};

const MOVE_LABEL: &str = "Move Section";
const DELETE_LABEL: &str = "Delete Section";
const DUPLICATE_LABEL: &str = "Duplicate Section";

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

/// The section's bytes, ready to go after it: a heading's id outside a block and its metadata's `id`, which are
/// unique on a page, get new ones that nothing on the page has, and its first heading one if it has none, for what
/// a site makes of a heading. What is inside a block keeps its ids as written: they are its fields', per field, and
/// a name there can mean something (ADR 0022). Every other byte is as it was. Also: where the id given to a
/// heading ends, within those bytes.
fn with_new_ids(
    doc: &dyn FormatDoc,
    index: usize,
    body: Range<usize>,
    mut taken: BTreeSet<String>,
) -> (String, Option<usize>) {
    let text = doc.text();
    // where an id goes, and whether it comes as a new one with none there before
    let mut places: Vec<(Range<usize>, bool)> = trailing_ids_in(doc, body.clone())
        .into_iter()
        .filter(|(a, _)| doc.named_containers_at(*a).is_empty())
        .map(|(a, b)| (a..b, false))
        .collect();
    let metadata = doc.section_metadata(index);
    let id_value = metadata.and_then(|m| m.pairs.iter().find(|p| p.key == "id").map(|p| p.value_span));
    places.extend(id_value.map(|v| (v.start..v.end, false)));
    let heading = (doc.section_first_heading(index)).filter(|at| body.contains(at));
    if let Some(at) = heading
        && let Some(end) = atx_line_end(doc, at)
    {
        places.push((end..end, true));
    }
    places.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(body.len() + 16 * places.len());
    let mut from = body.start;
    let mut added_end = None;
    for (r, added) in places {
        out.push_str(&text[from..r.start]);
        let id = if added {
            // what its text makes is what the original goes by on a site: past that one
            let title = heading.and_then(|at| doc.heading(at)).map_or(String::new(), |h| h.text);
            let own = section_id(&title, &taken);
            let also: BTreeSet<String> = taken.iter().cloned().chain([own]).collect();
            section_id(&title, &also)
        } else {
            next_section_id(&text[r.clone()], &taken)
        };
        if added {
            out.push_str(&format!(" {{#{id}}}"));
            added_end = Some(out.len());
        } else {
            out.push_str(&id);
        }
        taken.insert(id);
        from = r.end;
    }
    out.push_str(&text[from..body.end]);
    (out, added_end)
}

/// Where a `{#id}` can be added to the heading at `at`: its one line's end, when it is written with `#`, has
/// some text and no id or closing `#` of its own.
fn atx_line_end(doc: &dyn FormatDoc, at: usize) -> Option<usize> {
    let text = doc.text();
    let line = at..line_end(text, at);
    let end = line.start + text[line.clone()].trim_end_matches([' ', '\t']).len();
    let written = &text[line.start..end];
    let has_id = doc.hidden_note(at).is_some();
    let is_atx = written.trim_start_matches(' ').starts_with('#');
    let has_text = doc.heading(at).is_some_and(|h| !h.text.trim().is_empty());
    (is_atx && has_text && !has_id && !written.ends_with('#')).then_some(end)
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

    /// Section `index` has something to copy: not nothing, and not only metadata, which a site refuses.
    pub fn can_duplicate_section(&self, index: usize) -> bool {
        index < self.session.doc().section_count() && !is_hollow(self.session.doc(), index)
    }

    /// Puts a copy of section `index` right after it, as a section of its own: its bytes as they are but for its
    /// ids (see [`with_new_ids`]). `Ok(false)`: there is nothing to copy. One undo step, the caret in the copy.
    /// Refused if anything would pair or divide differently afterwards.
    pub fn duplicate_section(&mut self, index: usize, in_source: bool) -> Result<bool, MoveRefused> {
        if in_source {
            self.source_selection_to_session();
        }
        if !self.can_duplicate_section(index) {
            return Ok(false);
        }
        let taken = self.ids_on_page();
        let doc = self.session.doc();
        let text = doc.text();
        let Some(p) = part(doc, index) else {
            return Ok(false);
        };
        let eol = doc.line_ending().as_str();
        let (copy, added_id_end) = with_new_ids(doc, index, p.body.clone(), taken);
        let line = after_line(text, p.body.end);
        let lead = 3 + 2 * eol.len();
        let moved = format!("---{eol}{eol}{copy}");
        let (insert, skip) = separated(text, line, &moved, eol);
        let at = line + skip;
        let sections = doc.section_count() + 1;
        let mut want = blocks_of(doc);
        want.extend(blocks_in(doc, &p.body));
        want.sort();
        let hints_after = (!doc.hints().is_empty()).then(HintSet::new);
        self.release();
        let done = self.session.transact(DUPLICATE_LABEL, TxKind::Structural, |tx| {
            tx.step(PlanStep {
                patches: PatchSet::single_verbatim(line..line, &insert),
                hints_after,
            })?;
            let doc = tx.doc();
            let stands = doc.text().get(at + lead..at + lead + copy.len()) == Some(copy.as_str())
                && doc.section_count() == sections
                && doc.section_at(at + lead).index == index + 1
                && doc.section_at(at + lead).boundary == Some(Span::new(at, at + 3))
                && added_id_end.is_none_or(|e| {
                    let end = at + lead + e;
                    doc.hidden_note(end).is_some_and(|n| n.end == end)
                });
            if !stands || blocks_of(doc) != want {
                return Err(EditError::Aborted(String::new()));
            }
            // past its metadata, which shows as a plaque while the caret is elsewhere
            let start = (doc.section_metadata(index + 1)).map_or(at + lead, |m| m.closer.end);
            let text = doc.text();
            let mut caret = if start > at + lead {
                after_line(text, start)
            } else {
                start
            };
            while caret < text.len() && blank_line_at(text, caret) {
                caret = after_line(text, caret);
            }
            tx.set_selection(Selection::caret_at(DocPos::new(caret.min(text.len()))));
            Ok(())
        });
        self.sync();
        self.follow_block_modes();
        self.pin_caret();
        if in_source {
            self.source_selection_from_session();
        }
        match done {
            Ok(()) => Ok(true),
            Err(EditError::Aborted(_)) => Err(MoveRefused::WouldNotStand),
            Err(e) => Err(MoveRefused::Edit(e)),
        }
    }

    /// The section holding the document's `#` title when it has exactly one: a level 1 heading outside any block.
    /// Walks the top-level blocks until a second one: for what happens once, on a command or a drop.
    pub fn only_title_section(&self) -> Option<usize> {
        let doc = self.session.doc();
        let mut found = None;
        for t in 0..doc.top_count() {
            let Some(s) = doc.top_span(t) else { continue };
            let is_title = matches!(doc.leaf(s.start).map(|l| l.kind), Some(LeafKind::Heading { level: 1 }))
                && doc.named_containers_at(s.start).is_empty();
            if is_title {
                if found.is_some() {
                    return None;
                }
                found = Some(s.start);
            }
        }
        found.map(|at| doc.section_at(at).index)
    }

    /// Where the end of section `index` is, for what is to go after it: none for a first section with nothing in it.
    pub(super) fn section_end(&self, index: usize) -> Option<usize> {
        let p = part(self.session.doc(), index)?;
        (p.boundary.is_some() || !p.body.is_empty()).then_some(p.body.end)
    }
}
