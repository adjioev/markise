//! Editing a block's fields where it stands (ADR 0021). The block shows through the same items as
//! anything else, so the caret, the selection and IME are the surface's. What is different is
//! what a keystroke means there: it is an edit of one field's value, checked before it is applied
//! ([`crate::block_fields`]), so that the block's structure cannot break by accident. Every other
//! way to change the text (the form, Find, undo, Source mode) changes it as ever; the next
//! keystroke reads what is there.

use std::ops::Range;
use std::sync::Arc;

use markise_doc::{DocPos, Params, Patch, PatchSet, Selection};
use markise_layout::caret::Geometry;
use markise_layout::{BlockCan, BlockViewMode, BlockViews, Images, NativeAt, TextShaper};

use super::command::CommandError;
use super::{EditorView, MarkId, StateId, TypingDir};
use crate::block_fields::{BlockFields, FieldKind, Refused, Slot, SlotCaret, SlotEdit};
use crate::block_view::PackageViews;

/// What a place with no source stands for: a field, and what goes before the text typed there if
/// the field has some already.
#[derive(Clone, Copy)]
struct Absent {
    field: usize,
    sep: &'static str,
}

/// A block with a state of its own: the mark on its opener, where that was last, and its name.
struct OwnMode {
    mark: MarkId,
    mode: BlockViewMode,
    at: usize,
    name: Box<str>,
}

/// What in-place editing holds between keystrokes.
#[derive(Default)]
pub(super) struct InPlace {
    pub(super) fields: Option<Arc<PackageViews>>,
    /// The model of the block last edited: identities and retired ids live on in it.
    model: Option<BlockFields>,
    /// The block being edited stays native until the selection leaves it (ADR 0021).
    sticky: Option<MarkId>,
    /// Carets on a place in a block with no source, and the field each stands for, by the state of
    /// the document they stood in: `DocPos::virt` cannot name a field, and undo brings back the
    /// state and the caret. Only the caret an edit put there has one, newest last.
    absent: Vec<(StateId, DocPos, Absent)>,
    /// How the document shows its blocks …
    pub(super) mode: BlockViewMode,
    /// … and the blocks that differ from it, at their openers.
    own_modes: Vec<OwnMode>,
    /// States brought back for openers not in the document yet (the first part of a large file).
    pending_modes: Vec<(usize, BlockViewMode)>,
    /// What was done in blocks in Practice: never in the document ([`super::practice`]).
    pub(super) practice: Vec<super::practice::Practised>,
    /// Someone chose for the whole document: the package's word on what opens in Practice is over.
    pub(super) defaults_off: bool,
    /// How many of those were right, of how many checked: read per frame, so not worked out per frame.
    pub(super) practice_score: (usize, usize),
}

fn plain_value_is_empty(slot: &Slot) -> bool {
    slot.value().trim().is_empty()
}

/// Places remembered for undo to come back to.
const ABSENT_MAX: usize = 64;

const NOT_HERE: CommandError = CommandError::Disabled("not in a field of a block");

impl EditorView {
    /// What knows about blocks: for how they look ([`markise_layout::View::set_block_views`] takes
    /// that, with any pictures a host adds) and for what their fields are.
    pub fn set_block_fields(&mut self, fields: Option<Arc<PackageViews>>) {
        self.place.fields = fields;
        self.place.model = None;
        self.end_sticky();
    }

    /// How the document shows its blocks, and the blocks that differ, by opener; for what keeps it
    /// (openers not in the document yet included).
    pub fn block_view_state(&self) -> (BlockViewMode, Vec<(usize, BlockViewMode)>) {
        let mut own = self.own_modes_now();
        own.extend(self.place.pending_modes.iter().copied());
        (self.place.mode, own)
    }

    pub(super) fn own_modes_now(&self) -> Vec<(usize, BlockViewMode)> {
        (self.place.own_modes.iter())
            .filter_map(|o| Some((self.session.mark(o.mark)?.offset, o.mode)))
            .collect()
    }

    pub(super) fn has_own_mode(&self, opener: usize) -> bool {
        self.own_modes_now().iter().any(|m| m.0 == opener)
    }

    /// The name of the block whose opener stands at `at`.
    fn opener_name(&self, at: usize) -> Option<Box<str>> {
        let c = self.session.doc().named_containers_at(at).pop()?;
        (c.opener.start == at).then_some(c.name)
    }

    pub(super) fn add_own_mode(&mut self, opener: usize, mode: BlockViewMode) -> bool {
        let Some(name) = self.opener_name(opener) else {
            return false;
        };
        let mark = self.session.add_mark(DocPos::new(opener));
        self.place.own_modes.push(OwnMode {
            mark,
            mode,
            at: opener,
            name,
        });
        true
    }

    /// Brings back what [`Self::block_view_state`] said, as far as blocks stand at those openers.
    /// The others wait for [`Self::retry_block_views`]: the file may not have come whole yet.
    pub fn restore_block_view_state(&mut self, mode: BlockViewMode, own: &[(usize, BlockViewMode)]) {
        self.place.mode = mode;
        self.clear_own_modes();
        for &(opener, m) in own {
            if !self.add_own_mode(opener, m) {
                self.place.pending_modes.push((opener, m));
            }
        }
        self.follow_block_modes();
    }

    /// The rest of a file came: what waited for its openers takes effect, `moved` by the edits made
    /// in the first part meanwhile. What finds no opener now is gone.
    pub(super) fn retry_block_views(&mut self, moved: isize) {
        for (opener, m) in std::mem::take(&mut self.place.pending_modes) {
            self.add_own_mode(opener.saturating_add_signed(moved), m);
        }
        self.follow_block_modes();
    }

    pub(super) fn clear_own_modes(&mut self) {
        for o in std::mem::take(&mut self.place.own_modes) {
            self.session.remove_mark(o.mark);
        }
        self.place.pending_modes.clear();
    }

    /// The next state for the whole document. Blocks that differed follow it.
    pub fn switch_all_block_views(&mut self) {
        let can = BlockCan {
            view: true,
            site: self.view.can_draw_site(),
            practice: self.place.fields.as_ref().is_some_and(|f| f.any_practice()),
        };
        let now = if self.practice_by_default() {
            BlockViewMode::Practice
        } else {
            self.place.mode
        };
        self.place.mode = now.next_for(can);
        self.place.defaults_off = true;
        self.clear_own_modes();
        self.follow_block_modes();
        self.leave_block_for_picture(None);
        self.practice_place_caret();
    }

    /// Whether a block of this name has a view of its own. With no package to say, nothing is ruled out.
    fn name_has_view(&self, name: &str) -> bool {
        self.place.fields.as_ref().is_none_or(|f| f.has_view(name))
    }

    /// What a block of this name can show here.
    fn can_of(&self, name: &str) -> BlockCan {
        BlockCan {
            view: self.name_has_view(name),
            site: self.view.can_draw_site(),
            practice: self.place.fields.as_ref().is_some_and(|f| f.has_practice(name)),
        }
    }

    /// What comes after this block's state. Asked of the switch before it is pressed, to say so.
    pub fn next_block_view(&self, opener: usize) -> BlockViewMode {
        let can = self
            .opener_name(opener)
            .map_or_else(BlockCan::default, |n| self.can_of(&n));
        (self.view.block_mode_of(opener)).next_for(can)
    }

    /// A picture shows what no caret can stand in, and a block holding a selection's end shows what it
    /// holds instead (ADR 0023). So whoever asks for a picture gets one: the caret goes after it. `only`:
    /// the block asked about; a caret in any other is none of this. `None`: every block was.
    fn leave_block_for_picture(&mut self, only: Option<usize>) {
        let p = self.session.selection().primary();
        if !p.is_caret() || !self.view.can_draw_site() {
            return;
        }
        let at = p.focus.offset;
        let around = self.session.doc().named_containers_at(at);
        let Some(c) = around.into_iter().rfind(|c| only.is_none_or(|o| c.opener.start == o)) else {
            return;
        };
        let inside = c.opener.start < at && at < c.closer.end;
        let site = self.view.block_mode_of(c.opener.start).shown(BlockCan {
            site: true,
            ..self.can_of(&c.name)
        });
        if !inside || site != BlockViewMode::Site {
            return;
        }
        let after = match self.session.doc().gap_virt(c.closer.end) {
            Some(virt) => DocPos::with_virt(c.closer.end, virt),
            None => DocPos::new(c.closer.end),
        };
        self.set_selection(Selection::caret_at(after));
    }

    /// The next state for the block with this opener.
    pub fn switch_block_view(&mut self, opener: usize) {
        let can = self
            .opener_name(opener)
            .map_or_else(BlockCan::default, |n| self.can_of(&n));
        let next = self.next_block_view(opener);
        let document = self
            .opener_name(opener)
            .map_or(self.place.mode, |n| self.document_mode(&n));
        let own =
            (self.place.own_modes.iter()).position(|o| self.session.mark(o.mark).is_some_and(|p| p.offset == opener));
        // what shows the same as the document's state needs no state of its own
        match own {
            Some(i) if next.shown(can) == document.shown(can) => {
                let o = self.place.own_modes.remove(i);
                self.session.remove_mark(o.mark);
            }
            Some(i) => self.place.own_modes[i].mode = next,
            None if next.shown(can) == document.shown(can) => {}
            None => {
                self.add_own_mode(opener, next);
            }
        }
        self.follow_block_modes();
        self.leave_block_for_picture(Some(opener));
        self.practice_place_caret();
    }

    /// Hands the view where the blocks with a state of their own stand now.
    pub(super) fn follow_block_modes(&mut self) {
        let own = self.own_modes_now();
        self.view.set_practice_defaults_off(self.place.defaults_off);
        self.view.set_block_modes(self.place.mode, own);
    }

    /// After the document changed: a state belongs to its block, so it goes with it. A mark on a
    /// deleted opener collapses onto whatever follows, which must not inherit it. `edits`: `(start,
    /// end, delta)` of each step, each in the coordinates after the ones before.
    pub(super) fn prune_block_modes(&mut self, edits: &[(usize, usize, isize)]) {
        let mut kept: Vec<OwnMode> = Vec::with_capacity(self.place.own_modes.len());
        for mut o in std::mem::take(&mut self.place.own_modes) {
            let mut at = Some(o.at);
            for &(start, end, delta) in edits {
                at = at.and_then(|p| match p {
                    p if p < start => Some(p),
                    p if p >= end => Some(p.saturating_add_signed(delta)),
                    _ => None,
                });
            }
            let now = self.session.mark(o.mark).map(|p| p.offset);
            let holds = at.is_some()
                && now.is_some_and(|p| {
                    self.opener_name(p).is_some_and(|n| n == o.name)
                        && !kept
                            .iter()
                            .any(|k| self.session.mark(k.mark).map(|q| q.offset) == Some(p))
                });
            if holds {
                o.at = now.unwrap_or(o.at);
                kept.push(o);
            } else {
                self.session.remove_mark(o.mark);
            }
        }
        self.place.own_modes = kept;
    }

    /// The field a place with no source stands for, in the document as it stands.
    fn absent_field(&self, pos: DocPos) -> Option<Absent> {
        let state = self.session.state_id();
        (self.place.absent.iter().rev())
            .find(|(st, p, _)| *st == state && *p == pos)
            .map(|x| x.2)
    }

    fn remember_absent(&mut self, pos: DocPos, field: usize, sep: &'static str) {
        if self.place.absent.len() >= ABSENT_MAX {
            self.place.absent.remove(0);
        }
        let place = Absent { field, sep };
        self.place.absent.push((self.session.state_id(), pos, place));
    }

    /// The model of the block `at` is in, kept up to date with the document.
    fn block_model(&mut self, at: &NativeAt) -> Option<BlockFields> {
        let source = self.session.text().get(at.range.clone())?;
        let base = at.range.start;
        if let Some(m) = &self.place.model
            && m.source() == source
            && m.base() == base
        {
            return Some(m.clone());
        }
        let c = self.session.doc().named_containers_at(base).pop()?;
        let model = match &self.place.model {
            Some(m) if m.base() == base && m.name() == &*c.name && m.args() == &*c.args => {
                m.refreshed(source, base).ok()
            }
            _ => self.place.fields.as_ref()?.fields_of(&c.name, &c.args, source, base),
        }?;
        self.place.model = Some(model.clone());
        Some(model)
    }

    /// Replaces source range `r` as one step, through the checks of a field, when it is in a block
    /// shown natively: what inserts text by a route of its own (link completion) comes here. `None`:
    /// not in such a block, or it holds the block whole.
    pub fn field_replace(&mut self, r: Range<usize>, text: &str) -> Option<Result<(), CommandError>> {
        self.place.fields.as_ref()?;
        if self.practice_touches(r.clone()) {
            return Some(Err(NOT_HERE));
        }
        let at = self.view.native_block(r.start)?;
        if r.start <= at.range.start && r.end >= at.range.end {
            return None;
        }
        let edit = (|| {
            let model = self.block_model(&at)?;
            let rel = r.start - at.range.start..r.end.checked_sub(at.range.start)?;
            let slot = model.slot_at(rel.start).filter(|s| rel.end <= s.range().end)?;
            Some(model.edit_slot(slot.at, rel, text))
        })();
        let Some(edit) = edit else { return Some(Err(NOT_HERE)) };
        self.session.close_typing();
        let done = self.apply_field_edit(&at, edit, TypingDir::Insert);
        self.session.close_typing();
        Some(done)
    }

    /// The slot holding the selection, both its ends; the model and offsets relative to it.
    fn slot_of_selection(&mut self, at: &NativeAt) -> Option<(BlockFields, Slot, Range<usize>)> {
        let r = self.session.selection().primary().range();
        let model = self.block_model(at)?;
        let base = at.range.start;
        let rel = r.start.checked_sub(base)?..r.end.checked_sub(base)?;
        let slot = model.slot_at(rel.start)?;
        let whole = slot.range();
        (rel.end <= whole.end).then_some((model, slot, rel))
    }

    /// Whether this command is one for a block's fields; `None` lets it run as ever.
    pub(super) fn field_command(
        &mut self,
        id: &str,
        params: &Params,
        shaper: &mut dyn TextShaper,
        images: &dyn Images,
    ) -> Option<Result<(), CommandError>> {
        self.place.fields.as_ref()?;
        if let Some(done) = self.practice_command(id, params) {
            return Some(done);
        }
        let p = self.session.selection().primary();
        let r = p.range();
        let at = (self.view.native_block(r.start)).or_else(|| self.view.native_block(r.end))?;
        if r.start <= at.range.start && r.end >= at.range.end {
            return None; // holds the block whole: as anything else
        }
        // the virtual paragraph after a closer shares its end, and is outside the block
        if p.is_caret() && p.focus.virt > 0 && p.focus.offset >= at.range.end {
            return None;
        }
        match id {
            "edit.undo" | "edit.redo" => None,
            "edit.insertText" => {
                let text = params.get_text("text").unwrap_or("").to_string();
                Some(self.field_insert(&at, &text))
            }
            "edit.deleteBackward" => {
                Some(self.field_delete(&at, false, shaper, images, |g, f| g.horizontal(f, false, false)))
            }
            "edit.deleteForward" => {
                Some(self.field_delete(&at, true, shaper, images, |g, f| g.horizontal(f, true, false)))
            }
            "edit.deleteWordBackward" => {
                Some(self.field_delete(&at, false, shaper, images, |g, f| g.horizontal(f, false, true)))
            }
            "edit.deleteWordForward" => {
                Some(self.field_delete(&at, true, shaper, images, |g, f| g.horizontal(f, true, true)))
            }
            "delete.toLineStart" => Some(self.field_delete(&at, false, shaper, images, |g, f| g.row_edge(f, false))),
            "edit.newline" => Some(self.field_enter(&at)),
            "edit.softBreak" => Some(self.field_break(&at, "\n")),
            "edit.indentOrNextCell" => Some(self.field_tab(&at, true)),
            "edit.outdentOrPrevCell" => Some(self.field_tab(&at, false)),
            "format.strong" | "format.emph" | "format.underline" | "format.code" | "format.strike"
            | "format.highlight" | "format.clear" => {
                // marks inside one field's text are text; what comes of them is checked after
                if self
                    .slot_of_selection(&at)
                    .is_some_and(|(_, s, rel)| !s.is_protected(&rel))
                {
                    None
                } else {
                    Some(Err(NOT_HERE))
                }
            }
            _ if self.is_mutating(id) => Some(Err(NOT_HERE)),
            _ => None,
        }
    }

    pub(super) fn is_mutating(&self, id: &str) -> bool {
        (self.registry.get(id)).is_some_and(|c| matches!(c.desc.effect, markise_doc::Effect::Mutates { .. }))
    }

    fn field_insert(&mut self, at: &NativeAt, text: &str) -> Result<(), CommandError> {
        let focus = self.session.selection().primary().focus;
        if focus.virt > 0 {
            // a place with no source: the field it stands for, or nowhere
            let (Some(place), Some(model)) = (self.absent_field(focus), self.block_model(at)) else {
                return Err(NOT_HERE);
            };
            return self.apply_field_edit(at, model.fill_field(place.field, place.sep, text), TypingDir::Insert);
        }
        let Some((model, slot, rel)) = self.slot_of_selection(at) else {
            return Err(NOT_HERE);
        };
        self.apply_field_edit(at, model.edit_slot(slot.at, rel, text), TypingDir::Insert)
    }

    /// Deletes the selection, or what `target` says from the caret, inside the slot.
    fn field_delete(
        &mut self,
        at: &NativeAt,
        forward: bool,
        shaper: &mut dyn TextShaper,
        images: &dyn Images,
        target: impl FnOnce(&mut Geometry<'_>, DocPos) -> DocPos,
    ) -> Result<(), CommandError> {
        let dir = if forward {
            TypingDir::DeleteForward
        } else {
            TypingDir::DeleteBackward
        };
        let p = self.session.selection().primary();
        let Some((model, slot, mut rel)) = self.slot_of_selection(at) else {
            return Err(NOT_HERE);
        };
        if p.is_caret() {
            // off a place with no source: back on the text it follows
            if p.focus.virt > 0 {
                self.edit(|s| s.set_selection(Selection::caret(p.focus.offset)));
                return Ok(());
            }
            let mut g = self.geometry(shaper, images);
            let to = target(&mut g, p.focus).offset.saturating_sub(at.range.start);
            // never out of the field: at its edge, an empty item goes and the rest stays
            let whole = slot.range();
            let to = to.clamp(whole.start, whole.end);
            rel = if forward {
                rel.start..to.max(rel.start)
            } else {
                to.min(rel.start)..rel.start
            };
            if rel.is_empty() {
                if slot.at.item.is_none() {
                    return Ok(());
                }
                let done = self.apply_field_edit(at, model.remove_empty_item(slot.at, forward), dir);
                self.session.close_typing();
                return done;
            }
        }
        self.apply_field_edit(at, model.edit_slot(slot.at, rel, ""), dir)
    }

    /// Enter: a new item in a list, a new answer with a new id, a new paragraph in text, and from
    /// a heading the next field.
    fn field_enter(&mut self, at: &NativeAt) -> Result<(), CommandError> {
        if self.session.selection().primary().focus.virt > 0 {
            return Ok(());
        }
        let Some((model, slot, rel)) = self.slot_of_selection(at) else {
            return Err(NOT_HERE);
        };
        self.session.close_typing();
        match slot.kind {
            FieldKind::Heading { .. } => self.field_tab(at, true),
            FieldKind::Text | FieldKind::Quote => self.field_break(at, "\n\n"),
            _ if plain_value_is_empty(&slot) => {
                // on an empty item: out of the list, if something comes after it and it has company
                let next = model.slots().into_iter().skip_while(|s| s.at != slot.at).nth(1);
                let last = next.as_ref().is_none_or(|n| n.at.field != slot.at.field);
                if last {
                    let done = self.apply_field_edit(at, model.remove_empty_item(slot.at, false), TypingDir::Insert);
                    self.session.close_typing();
                    return done;
                }
                Ok(())
            }
            _ => {
                let done = self.apply_field_edit(at, model.split_item(slot.at, rel), TypingDir::Insert);
                // an item coming is a step of its own, apart from what is typed into it
                self.session.close_typing();
                done
            }
        }
    }

    /// A paragraph break or a line break in text. What it splits is written at once. At the end of
    /// the text there is nothing to split yet, and Markdown has no such thing as a line with nothing
    /// on it: the caret goes on a place for what comes next, and the file changes when that comes.
    fn field_break(&mut self, at: &NativeAt, sep: &'static str) -> Result<(), CommandError> {
        let Some((model, slot, rel)) = self.slot_of_selection(at) else {
            return Err(NOT_HERE);
        };
        if slot.is_one_line() {
            return Ok(());
        }
        if rel.end != slot.range().end {
            let s = model.source();
            // right before another paragraph of the field: there it is
            let next = s[rel.end..].trim_start_matches(['\r', '\n']);
            let gap = s[rel.end..].len() - next.len();
            if rel.is_empty() && sep == "\n\n" && s[rel.end..gap + rel.end].matches('\n').count() >= 2 {
                let caret = at.range.start + rel.end + gap;
                self.edit(|s| s.set_selection(Selection::caret(caret)));
                return Ok(());
            }
            let done = self.apply_field_edit(at, model.edit_slot(slot.at, rel, sep), TypingDir::Insert);
            self.session.close_typing();
            return done;
        }
        if !rel.is_empty() {
            self.apply_field_edit(at, model.edit_slot(slot.at, rel, ""), TypingDir::DeleteBackward)?;
            self.session.close_typing();
            // (emptied altogether, the caret already stands on a place for it)
            if self.session.selection().primary().focus.virt > 0 {
                return Ok(());
            }
            let Some(now) = self.view.native_block(at.range.start) else {
                return Ok(());
            };
            return self.field_break(&now, sep);
        }
        let end = rel.end + at.range.start;
        let leaf_end = (self.session.doc().leaf(end)).map_or(end, |l| l.span.end.max(end));
        let pos = DocPos::with_virt(leaf_end, 1);
        self.remember_absent(pos, slot.at.field, sep);
        self.begin_sticky(at.range.start);
        self.edit(|s| s.set_selection(Selection::caret_at(pos)));
        Ok(())
    }

    /// Tab goes from field to field in a block, and stays at the ends.
    fn field_tab(&mut self, at: &NativeAt, forward: bool) -> Result<(), CommandError> {
        let Some((model, slot, _)) = self.slot_of_selection(at) else {
            return Err(NOT_HERE);
        };
        let slots = model.slots();
        let k = slots.iter().position(|s| s.at == slot.at).unwrap_or(0);
        // (a field can hold more than text: the first or the last text of it, going on if it has none)
        let mut to = if forward { slots.get(k + 1..) } else { slots.get(..k) }
            .unwrap_or(&[])
            .to_vec();
        if !forward {
            to.reverse();
        }
        if let Some(rel) = to.iter().find_map(|t| t.text_edge(forward)) {
            let caret = at.range.start + rel;
            self.edit(|s| s.set_selection(Selection::caret(caret)));
        }
        Ok(())
    }

    /// A press on the mark of the answer item at `at`. Returns whether the document changed.
    pub fn press_answer_mark(&mut self, at: usize) -> bool {
        if self.in_practice(at) {
            return false;
        }
        let Some(block) = self.view.native_block(at) else {
            return false;
        };
        let Some(model) = self.block_model(&block) else {
            return false;
        };
        let base = block.range.start;
        let Some(slot) = at.checked_sub(base).and_then(|rel| model.slot_at(rel)) else {
            return false;
        };
        if !model.is_answer(slot.at) {
            return false;
        }
        let Ok(Some(applied)) = model.press_mark(slot.at, block.block.view.single_answer) else {
            return false;
        };
        let shifted: Vec<Patch> = (applied.patches.patches().iter())
            .map(|p| Patch::verbatim(p.range.start + base..p.range.end + base, p.insert.clone()))
            .collect();
        let Ok(patches) = PatchSet::new(shifted) else {
            return false;
        };
        self.place.model = Some(applied.fields);
        self.edit(|s| s.apply_external(&patches, "Mark Answer", false)).is_ok()
    }

    /// Applies a checked edit as a keystroke of the document, with its typing groups and undo.
    fn apply_field_edit(
        &mut self,
        at: &NativeAt,
        edit: Result<Option<SlotEdit>, Refused>,
        dir: TypingDir,
    ) -> Result<(), CommandError> {
        let edit = match edit {
            Ok(Some(e)) => e,
            Ok(None) => return Ok(()),
            Err(Refused::NotText | Refused::Unsafe(_) | Refused::Fields(_)) => return Err(NOT_HERE),
        };
        let base = at.range.start;
        let shifted: Vec<Patch> = (edit.applied.patches.patches().iter())
            .map(|p| Patch::verbatim(p.range.start + base..p.range.end + base, p.insert.clone()))
            .collect();
        let Ok(patches) = PatchSet::new(shifted) else {
            return Err(NOT_HERE);
        };
        let (caret, absent) = match edit.caret {
            SlotCaret::At(rel) => (DocPos::new(rel + base), None),
            SlotCaret::Absent { after, field } => (DocPos::with_virt(after + base, 1), Some(field)),
        };
        self.begin_sticky(base);
        self.release();
        let r = self.session.field_keystroke(dir, patches, Selection::caret_at(caret));
        self.place.model = r.is_ok().then_some(edit.applied.fields);
        if let (Some(field), true) = (absent, r.is_ok()) {
            self.remember_absent(caret, field, "\n\n");
        }
        self.sync();
        r.map_err(CommandError::from)
    }

    pub(super) fn begin_sticky(&mut self, opener: usize) {
        if self.place.sticky.is_none() {
            self.place.sticky = Some(self.session.add_mark(DocPos::new(opener)));
        }
    }

    /// Where the block kept native while it is edited stands.
    pub(super) fn sticky_at(&self) -> Option<usize> {
        Some(self.session.mark(self.place.sticky?)?.offset)
    }

    pub(super) fn end_sticky(&mut self) {
        if let Some(m) = self.place.sticky.take() {
            self.session.remove_mark(m);
        }
        self.view.set_sticky_block(None);
    }

    /// Nothing to keep up with: the cost of a document without any of this.
    pub(super) fn place_is_idle(&self) -> bool {
        self.place.sticky.is_none() && self.place.own_modes.is_empty() && self.place.practice.is_empty()
    }

    /// Keeps the block being edited native while the selection is in it. Called on every change
    /// of the document or the selection; costs a comparison when nothing is being edited.
    pub(super) fn follow_sticky(&mut self) {
        let Some(m) = self.place.sticky else { return };
        let Some(opener) = self.session.mark(m).map(|p| p.offset) else {
            self.end_sticky();
            return;
        };
        let focus = self.session.selection().primary().focus.offset;
        let doc = self.session.doc();
        let inside = (doc.named_containers_at(focus).pop()).is_some_and(|c| c.opener.start == opener);
        if inside {
            self.view.set_sticky_block(Some(opener));
        } else {
            self.end_sticky();
        }
    }

    /// A selection as the fields allow it. One that starts in a field stays in it. One from
    /// outside a block into it holds the block whole. A caret on a place in a block with no source
    /// is only good where an edit put it.
    pub(super) fn field_selection(&mut self, sel: Selection) -> Selection {
        if self.place.fields.is_none() {
            return sel;
        }
        let sel = self.practice_selection(sel);
        let p = sel.primary();
        if p.is_caret() {
            let virt_inside = p.focus.virt > 0
                && self
                    .view
                    .native_block(p.focus.offset)
                    .is_some_and(|b| p.focus.offset < b.range.end)
                && self.absent_field(p.focus).is_none();
            return if virt_inside {
                Selection::caret(p.focus.offset)
            } else {
                sel
            };
        }
        if let Some(at) = self.view.native_block(p.anchor.offset) {
            let inside = at.range.start <= p.focus.offset && p.focus.offset < at.range.end;
            let held = self.block_model(&at).and_then(|m| {
                let rel = p.anchor.offset.checked_sub(at.range.start)?;
                m.slot_at(rel).map(|s| s.range())
            });
            let Some(whole) = held else {
                return Selection::caret_at(p.anchor);
            };
            let base = at.range.start;
            let focus = p.focus.offset.saturating_sub(base);
            if inside && whole.contains(&focus) || focus == whole.end {
                return sel;
            }
            let to = if p.focus.offset < p.anchor.offset {
                whole.start
            } else {
                whole.end
            };
            return Selection::single(p.anchor, DocPos::new(to + base));
        }
        if let Some(at) = self.view.native_block(p.focus.offset) {
            let to = if p.focus.offset < p.anchor.offset {
                at.range.start
            } else {
                at.range.end
            };
            return Selection::single(p.anchor, DocPos::new(to));
        }
        sel
    }

    /// The caret on a place in a block with no source, which no edit put there (a gap the layout
    /// offers between two items): a motion goes on past it.
    pub(super) fn in_stray_gap(&self) -> bool {
        let p = self.session.selection().primary();
        p.focus.virt > 0
            && self
                .view
                .native_block(p.focus.offset)
                .is_some_and(|b| p.focus.offset < b.range.end)
            && self.absent_field(p.focus).is_none()
    }

    /// Whether `at` is the end or the start of a block shown natively: a selection may stop there,
    /// as a whole block is what it holds, though there is no caret stop.
    pub(super) fn is_native_edge(&self, at: DocPos) -> bool {
        at.virt == 0
            && (self.view.native_block(at.offset))
                .is_some_and(|b| at.offset == b.range.start || at.offset == b.range.end)
            && !self.session.selection().is_caret()
    }

    /// After inline marks ran on a field: the same block has to stand from the same opener to its
    /// own closer and read as before, apart from the text. If not, they did something else, also if
    /// no block is left to ask, and they are taken back and are not there to redo.
    pub(super) fn check_marks(&mut self, before: Option<(BlockFields, usize, StateId)>) {
        let Some((before, len, state)) = before else { return };
        if self.session.state_id() == state {
            return;
        }
        if !self.reads_as(&before, len) {
            self.release();
            self.session.undo();
            self.session.clear_redo();
            self.place.model = None;
            self.sync();
        }
    }

    fn reads_as(&self, before: &BlockFields, len: usize) -> bool {
        let base = before.base();
        let Some(c) = self.session.doc().named_containers_at(base).pop() else {
            return false;
        };
        let moved = self.session.text().len() as isize - len as isize;
        if c.opener.start != base || c.closer.end as isize != (base + before.source().len()) as isize + moved {
            return false;
        }
        let Some(source) = self.session.text().get(c.opener.start..c.closer.end) else {
            return false;
        };
        (self.place.fields.as_ref())
            .and_then(|f| f.fields_of(&c.name, &c.args, source, base))
            .is_some_and(|next| before.same_structure(&next))
    }

    /// The model around the selection, for [`Self::check_marks`].
    pub(super) fn model_for_marks(&mut self) -> Option<(BlockFields, usize, StateId)> {
        self.place.fields.as_ref()?;
        let at = self.session.selection().primary().focus.offset;
        let block = self.view.native_block(at)?;
        let model = self.block_model(&block)?;
        Some((model, self.session.text().len(), self.session.state_id()))
    }
}
