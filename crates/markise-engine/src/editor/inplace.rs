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
use markise_layout::{Images, NativeAt, TextShaper};

use super::command::CommandError;
use super::{EditorView, MarkId, TypingDir};
use crate::block_fields::{BlockFields, Refused, Slot, SlotCaret, SlotEdit};
use crate::block_view::PackageViews;

/// What in-place editing holds between keystrokes.
#[derive(Default)]
pub(super) struct InPlace {
    pub(super) fields: Option<Arc<PackageViews>>,
    /// The model of the block last edited: identities and retired ids live on in it.
    model: Option<BlockFields>,
    /// The block being edited stays native until the selection leaves it (ADR 0021).
    sticky: Option<MarkId>,
    /// A caret on a place in a block with no source yet, and the field that place stands for. Only
    /// the caret an edit put there has one.
    absent: Option<(DocPos, usize)>,
}

const NOT_HERE: CommandError = CommandError::Disabled("not in a field of a block");

impl EditorView {
    /// What knows about blocks: for how they look ([`markise_layout::View::set_block_views`] takes
    /// that, with any pictures a host adds) and for what their fields are.
    pub fn set_block_fields(&mut self, fields: Option<Arc<PackageViews>>) {
        self.place.fields = fields;
        self.place.model = None;
        self.end_sticky();
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
        let p = self.session.selection().primary();
        let r = p.range();
        let at = (self.view.native_block(r.start)).or_else(|| self.view.native_block(r.end))?;
        if r.start <= at.range.start && r.end >= at.range.end {
            return None; // holds the block whole: as anything else
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
            // the rules of Enter and of a field's edges are the next thing to come
            "edit.newline" | "edit.softBreak" | "edit.indentOrNextCell" | "edit.outdentOrPrevCell" => Some(Ok(())),
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

    fn is_mutating(&self, id: &str) -> bool {
        (self.registry.get(id)).is_some_and(|c| matches!(c.desc.effect, markise_doc::Effect::Mutates { .. }))
    }

    fn field_insert(&mut self, at: &NativeAt, text: &str) -> Result<(), CommandError> {
        let focus = self.session.selection().primary().focus;
        if let Some((pos, field)) = self.place.absent
            && pos == focus
        {
            let Some(model) = self.block_model(at) else {
                return Err(NOT_HERE);
            };
            return self.apply_field_edit(at, model.fill_field(field, text), TypingDir::Insert);
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
            let mut g = self.geometry(shaper, images);
            let to = target(&mut g, p.focus).offset.saturating_sub(at.range.start);
            // never out of the field: its edges have rules of their own
            let whole = slot.range();
            let to = to.clamp(whole.start, whole.end);
            rel = if forward {
                rel.start..to.max(rel.start)
            } else {
                to.min(rel.start)..rel.start
            };
            if rel.is_empty() {
                return Ok(());
            }
        }
        self.apply_field_edit(at, model.edit_slot(slot.at, rel, ""), dir)
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
            SlotCaret::Absent { after, field } => {
                let pos = DocPos::with_virt(after + base, 1);
                (pos, Some((pos, field)))
            }
        };
        self.begin_sticky(base);
        self.release();
        let r = self.session.field_keystroke(dir, patches, Selection::caret_at(caret));
        self.place.model = r.is_ok().then_some(edit.applied.fields);
        self.place.absent = absent.filter(|_| r.is_ok());
        self.sync();
        r.map_err(CommandError::from)
    }

    fn begin_sticky(&mut self, opener: usize) {
        if self.place.sticky.is_none() {
            self.place.sticky = Some(self.session.add_mark(DocPos::new(opener)));
        }
    }

    pub(super) fn end_sticky(&mut self) {
        if let Some(m) = self.place.sticky.take() {
            self.session.remove_mark(m);
        }
        self.view.set_sticky_block(None);
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
        let p = sel.primary();
        if p.is_caret() {
            let virt_inside = p.focus.virt > 0
                && self
                    .view
                    .native_block(p.focus.offset)
                    .is_some_and(|b| p.focus.offset < b.range.end)
                && self.place.absent.is_none_or(|(pos, _)| pos != p.focus);
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
            && self.place.absent.is_none_or(|(pos, _)| pos != p.focus)
    }

    /// Whether `at` is the end or the start of a block shown natively: a selection may stop there,
    /// as a whole block is what it holds, though there is no caret stop.
    pub(super) fn is_native_edge(&self, at: DocPos) -> bool {
        at.virt == 0
            && (self.view.native_block(at.offset))
                .is_some_and(|b| at.offset == b.range.start || at.offset == b.range.end)
            && !self.session.selection().is_caret()
    }

    /// After inline marks ran on a field: if the block's fields do not read as before, they did
    /// something else, and they are taken back.
    pub(super) fn check_marks(&mut self, before: Option<BlockFields>, at: usize) {
        let (Some(before), Some(block)) = (before, self.view.native_block(at)) else {
            return;
        };
        let Some(next) = self.block_model(&block) else { return };
        if !before.same_structure(&next) {
            self.release();
            self.session.undo();
            self.sync();
        }
    }

    /// The model around the selection, for [`Self::check_marks`].
    pub(super) fn model_for_marks(&mut self) -> Option<BlockFields> {
        self.place.fields.as_ref()?;
        let at = self.session.selection().primary().focus.offset;
        let block = self.view.native_block(at)?;
        self.block_model(&block)
    }
}
