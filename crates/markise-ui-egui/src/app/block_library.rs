//! The block library's glue (ADR 0022): when it opens and closes and where the keyboard goes. See
//! [`crate::block_library`].

use egui::Context;
use markise_layout::block::color;
use markise_package::LibraryItem;

use super::block_form::DraftNext;
use super::{Ask, MarkiseApp};
use crate::block_form::{package_language, style_for};
use crate::block_library::{LibraryAct, OpenLibrary, Opened};
use crate::viewer::SectionAct;

/// The library of the session.
#[derive(Default)]
pub(super) struct BlockLibraryState {
    open: Option<OpenLibrary>,
    /// `/` was typed: opens on the next frame, with what was typed after it.
    requested: Option<String>,
    /// A block was dropped: its opener and the line it goes to, in this document at this revision.
    requested_move: Option<((usize, usize), (u64, u64))>,
    /// A block's delete button was pressed, by its opener, in this document and revision.
    requested_delete: Option<(usize, (u64, u64))>,
    /// A control on a section's divider was pressed, in this document and revision.
    requested_section: Option<(SectionAct, (u64, u64))>,
}

impl BlockLibraryState {
    pub(super) fn request(&mut self, query: Option<String>) {
        if query.is_some() {
            self.requested = query;
        }
    }

    pub(super) fn request_delete(&mut self, opener: Option<usize>, origin: Option<(u64, u64)>) {
        if let (Some(opener), Some(origin)) = (opener, origin) {
            self.requested_delete = Some((opener, origin));
        }
    }

    pub(super) fn request_section(&mut self, act: Option<SectionAct>, origin: Option<(u64, u64)>) {
        if let (Some(act), Some(origin)) = (act, origin) {
            self.requested_section = Some((act, origin));
        }
    }

    pub(super) fn request_move(&mut self, drop: Option<(usize, usize)>, origin: Option<(u64, u64)>) {
        if let (Some(drop), Some(origin)) = (drop, origin) {
            self.requested_move = Some((drop, origin));
        }
    }
}

/// What a move says of the document's `#` title, standing in section `title`, when section `index` goes before
/// the section at `to`: it moves with its section, or from the top when another one comes first.
/// A quiet word after a move or an insert: a site may refuse a section with nothing or only metadata
/// when it is published, which is the site's to check (ADR 0026).
pub(super) const HOLLOW_HINT: &str =
    "A section here has nothing in it but metadata, which a site may refuse when it is published.";

fn title_warning(index: usize, to: usize, title: usize) -> Option<String> {
    if title == index {
        Some("Moved, and the document's # title with it.".to_string())
    } else if to == 0 && title == 0 {
        Some("Moved above the document's # title, so the title moves too.".to_string())
    } else {
        None
    }
}

impl MarkiseApp {
    /// The open library.
    pub fn block_library(&self) -> Option<&OpenLibrary> {
        self.block_library.open.as_ref()
    }

    /// There is something to offer and the document can take it: the Insert Block command.
    pub(super) fn block_library_available(&self) -> bool {
        (self.block_package().is_some_and(|p| !p.index.presets().is_empty()) || self.document_has_sections())
            && self.progressive.is_none()
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.viewer.ed.session.read_only().is_none() && d.viewer.card_edit().is_none())
    }

    /// The document's dialect has sections (ADR 0026): it needs no package for that.
    pub(super) fn document_has_sections(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.viewer.ed.view.sections_on())
    }

    /// Insert Block…: the same library, for the caret wherever it is.
    pub(super) fn open_block_library_by_command(&mut self, ctx: &Context) -> bool {
        self.block_library_available() && self.open_block_library(ctx, Opened::Command, String::new())
    }

    /// A block can be moved now. A form's changed draft holds its block by marks a move would end, and
    /// it is not lost silently (ADR 0020), so it comes first.
    fn block_move_allowed(&self) -> bool {
        self.block_package().is_some()
            && !self.block_draft_changed()
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.viewer.ed.session.read_only().is_none() && d.viewer.card_edit().is_none())
    }

    /// Move Block Up or Down is possible for the caret.
    pub(super) fn block_move_available(&self, up: bool) -> bool {
        self.block_move_allowed() && self.doc.as_ref().is_some_and(|d| d.viewer.ed.can_move_block(up))
    }

    /// Move Block Up or Down.
    pub(super) fn move_block_at_caret(&mut self, up: bool) -> bool {
        if !self.block_move_allowed() || self.keyboard_elsewhere() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let result = d.viewer.ed.move_block(up, in_source);
        if in_source {
            d.source_ui.find_reveal = true;
        } else {
            d.viewer.find_reveal = true;
        }
        self.block_move_done(result)
    }

    fn block_move_done(&mut self, result: Result<bool, markise_engine::editor::MoveRefused>) -> bool {
        match result {
            Ok(moved) => {
                self.remember_block_views();
                moved
            }
            Err(why) => {
                self.banner = Some(super::Banner::Error(format!("The block was not moved: {why}.")));
                false
            }
        }
    }

    /// The document as it stands.
    pub(super) fn block_edit_origin(&self) -> Option<(u64, u64)> {
        let d = self.doc.as_ref()?;
        Some((d.id, d.viewer.ed.session.revision().counter))
    }

    /// What the drag of a block came to.
    fn block_drop(&mut self) {
        let Some(((opener, line), origin)) = self.block_library.requested_move.take() else {
            return;
        };
        // named from the last frame: in another document or revision it means nothing
        if self.block_edit_origin() != Some(origin) {
            return;
        }
        if self.block_draft_changed() {
            self.block_form_message("Save or cancel this draft before moving a block.");
            return;
        }
        if !self.block_move_allowed() {
            return;
        }
        let Some(d) = &mut self.doc else { return };
        let result = d.viewer.ed.move_block_to(opener, line);
        self.block_move_done(result);
    }

    /// The keyboard is in a field of something else, such as Find: what it types and its commands are not
    /// the document's.
    fn keyboard_elsewhere(&self) -> bool {
        let (Some(ctx), Some(d)) = (&self.ctx, &self.doc) else {
            return false;
        };
        let Some(focused) = ctx.memory(|m| m.focused()) else {
            return false;
        };
        // the widget itself: full Source mode counts as the source's whoever has the keyboard
        d.viewer.surface_id() != Some(focused) && !d.source_ui.has_focus(ctx)
    }

    /// A block can be deleted now: as for a move, but no package is needed, and a draft is asked about.
    fn block_delete_allowed(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.viewer.ed.session.read_only().is_none() && d.viewer.card_edit().is_none())
    }

    /// Delete Block is possible for the caret.
    pub(super) fn block_delete_available(&self) -> bool {
        self.block_delete_allowed()
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.viewer.ed.block_opener_at_caret().is_some())
    }

    /// Delete Block.
    pub(super) fn delete_block_at_caret(&mut self) -> bool {
        if !self.block_delete_allowed() || self.keyboard_elsewhere() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let Some(opener) = d.viewer.ed.block_opener_in_pane(in_source) else {
            return false;
        };
        self.delete_block_asking(opener)
    }

    /// Deletes the block with this opener, once a changed draft of it, or of a block inside it, is
    /// answered for. A form of that block with nothing changed closes as it does when its block goes.
    pub(super) fn delete_block_asking(&mut self, opener: usize) -> bool {
        let Some(d) = &self.doc else { return false };
        let Some(lines) = d.viewer.ed.block_lines(opener) else {
            return false;
        };
        if let Some(open) = self.block_form()
            && open.doc() == d.id
            && open.is_dirty()
            && lines.contains(&open.key())
        {
            self.set_ask(Ask::BlockDraft(DraftNext::DeleteBlock(opener)));
            return true;
        }
        self.delete_block_now(opener)
    }

    /// The draft is answered for, or there was none.
    pub(super) fn delete_block_now(&mut self, opener: usize) -> bool {
        if !self.block_delete_allowed() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let result = d.viewer.ed.delete_block(opener, in_source);
        if in_source {
            d.source_ui.find_reveal = true;
        } else {
            d.viewer.request_refocus();
        }
        match result {
            Ok(done) => {
                if done {
                    self.remember_block_views();
                }
                done
            }
            Err(why) => {
                self.banner = Some(super::Banner::Error(format!("The block was not deleted: {why}.")));
                false
            }
        }
    }

    /// What a block's delete button asked for in the last frame.
    fn block_delete_pressed(&mut self) {
        let Some((opener, origin)) = self.block_library.requested_delete.take() else {
            return;
        };
        if self.block_edit_origin() == Some(origin) {
            self.delete_block_asking(opener);
        }
    }

    /// A section can be changed now: the document has them and is whole, as the last section of a part is cut
    /// short (ADR 0026), and nothing is being edited in a field of its own.
    pub(super) fn section_edit_allowed(&self) -> bool {
        self.progressive.is_none()
            && self.doc.as_ref().is_some_and(|d| {
                d.viewer.ed.view.sections_on()
                    && d.viewer.ed.session.read_only().is_none()
                    && d.viewer.card_edit().is_none()
            })
    }

    /// Move Section Up or Down is possible for the caret. A form's changed draft holds its block by marks a
    /// move would end, and it is not lost silently, as for a block's move.
    pub(super) fn section_move_available(&self, up: bool) -> bool {
        self.section_edit_allowed()
            && !self.block_draft_changed()
            && (self.doc.as_ref()).is_some_and(|d| d.viewer.ed.can_move_section(d.viewer.ed.section_at_caret(), up))
    }

    /// Delete Section is possible for the caret.
    pub(super) fn section_delete_available(&self) -> bool {
        self.section_edit_allowed()
            && (self.doc.as_ref()).is_some_and(|d| d.viewer.ed.can_delete_section(d.viewer.ed.section_at_caret()))
    }

    /// Move Section Up or Down, for the section holding the caret.
    pub(super) fn move_section_at_caret(&mut self, up: bool) -> bool {
        if self.keyboard_elsewhere() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let index = d.viewer.ed.section_in_pane(in_source);
        self.move_section(index, up)
    }

    fn move_section(&mut self, index: usize, up: bool) -> bool {
        let to = if up { index.checked_sub(1) } else { Some(index + 2) };
        to.is_some_and(|to| self.move_section_to(index, to))
    }

    /// Before section `to`: a step or a drop.
    fn move_section_to(&mut self, index: usize, to: usize) -> bool {
        if !self.section_edit_allowed() {
            return false;
        }
        if self.block_draft_changed() {
            self.block_form_message("Save or cancel this draft before moving a section.");
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        // the title goes with the section holding it, or from the top when another one comes first
        let ed = &d.viewer.ed;
        // the walk over the document only if the sections it concerns hold a `#` at all
        let concerned = ed.section_has_title(index) || (to == 0 && ed.section_has_title(0));
        let title = (concerned && ed.can_move_section_to(index, to)).then(|| ed.only_title_section());
        let warning = title.flatten().and_then(|t| title_warning(index, to, t));
        let hollow = ed.move_passes_hollow(index, to).then_some(HOLLOW_HINT);
        let warning = match (warning, hollow) {
            (Some(w), Some(h)) => Some(format!("{w} {h}")),
            (w, h) => w.or(h.map(str::to_string)),
        };
        let result = d.viewer.ed.move_section_to(index, to, in_source);
        if in_source {
            d.source_ui.find_reveal = true;
        } else {
            d.viewer.find_reveal = true;
        }
        match result {
            Ok(moved) => {
                self.remember_block_views();
                if let (true, Some(text)) = (moved, warning) {
                    self.undo_notice(text);
                }
                moved
            }
            Err(why) => {
                self.banner = Some(super::Banner::Error(format!("Not moved: {why}.")));
                false
            }
        }
    }

    /// Says what was just done, with Undo, unless something that asks for an answer is showing.
    pub(super) fn undo_notice(&mut self, text: String) {
        use super::Banner;
        if matches!(self.banner, Some(Banner::Conflict(_) | Banner::SaveConflict { .. })) {
            return;
        }
        let (Some(origin), Some(ctx)) = (self.block_edit_origin(), &self.ctx) else {
            return;
        };
        let until = ctx.input(|i| i.time) + super::UNDO_NOTICE_SECONDS;
        self.banner = Some(Banner::Undo { text, origin, until });
    }

    /// Duplicate on a section's row: right after it, one undo step.
    fn duplicate_section(&mut self, index: usize) -> bool {
        if !self.section_edit_allowed() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let result = d.viewer.ed.duplicate_section(index, in_source);
        if in_source {
            d.source_ui.find_reveal = true;
        } else {
            d.viewer.find_reveal = true;
        }
        match result {
            Ok(done) => {
                self.remember_block_views();
                done
            }
            Err(why) => {
                self.banner = Some(super::Banner::Error(format!("Not duplicated: {why}.")));
                false
            }
        }
    }

    /// Delete Section, for the section holding the caret.
    pub(super) fn delete_section_at_caret(&mut self) -> bool {
        if self.keyboard_elsewhere() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let index = d.viewer.ed.section_in_pane(in_source);
        self.delete_section_asking(index)
    }

    /// Deletes this section, once a changed draft of a block in it is answered for.
    fn delete_section_asking(&mut self, index: usize) -> bool {
        let Some(d) = &self.doc else { return false };
        let Some(lines) = d.viewer.ed.section_lines(index) else {
            return false;
        };
        if let Some(open) = self.block_form()
            && open.doc() == d.id
            && open.is_dirty()
            && lines.contains(&open.key())
        {
            self.set_ask(Ask::BlockDraft(DraftNext::DeleteSection(index)));
            return true;
        }
        self.delete_section_now(index)
    }

    /// The draft is answered for, or there was none.
    pub(super) fn delete_section_now(&mut self, index: usize) -> bool {
        if !self.section_edit_allowed() {
            return false;
        }
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        let name = d.viewer.ed.view.section_name(index);
        let result = d.viewer.ed.delete_section(index, in_source);
        if in_source {
            d.source_ui.find_reveal = true;
        } else {
            d.viewer.request_refocus();
        }
        match result {
            Ok(done) => {
                if done {
                    self.remember_block_views();
                    self.undo_notice(format!("Deleted: {name}"));
                }
                done
            }
            Err(why) => {
                self.banner = Some(super::Banner::Error(format!("Not deleted: {why}.")));
                false
            }
        }
    }

    /// What a control on a section's divider asked for in the last frame.
    fn section_control_pressed(&mut self) {
        let Some((act, origin)) = self.block_library.requested_section.take() else {
            return;
        };
        // named from the last frame: in another document or revision it means nothing
        if self.block_edit_origin() != Some(origin) {
            return;
        }
        match act {
            SectionAct::MoveUp(index) => self.move_section(index, true),
            SectionAct::MoveDown(index) => self.move_section(index, false),
            SectionAct::Delete(index) => self.delete_section_asking(index),
            SectionAct::MoveTo { index, to } => self.move_section_to(index, to),
            SectionAct::Duplicate(index) => self.duplicate_section(index),
            SectionAct::InsertAfter(index) => {
                let in_source = self.source_focused();
                let ctx = self.ctx.clone();
                ctx.is_some_and(|ctx| self.open_section_form_after(&ctx, index, in_source))
            }
        };
    }

    /// Inserts the preset at this place in the package: one undo step, shown natively, the caret in its
    /// first field. Package data is not trusted: a preset that is not what it says is refused here.
    pub fn insert_block_preset(&mut self, preset: usize) -> bool {
        let Some(package) = self.block_package().cloned() else {
            return false;
        };
        let Some(preset) = package.index.presets().get(preset) else {
            return false;
        };
        let in_source = self.source_focused();
        let Some(d) = &mut self.doc else { return false };
        match d.viewer.ed.insert_block(&preset.block, &preset.markdown, in_source) {
            Ok(_) => {
                if in_source {
                    d.source_ui.find_reveal = true;
                } else {
                    d.viewer.find_reveal = true;
                }
                self.remember_block_views();
                true
            }
            Err(why) => {
                let language = package_language(d.viewer.ed.session.doc());
                let label = preset.label.as_ref().map_or(preset.id.as_str(), |l| l.get(&language));
                // a package wrote the label: it stays one line, and nothing in it hides or turns the text around
                let label = markise_package::printable(label);
                self.banner = Some(super::Banner::Error(format!("“{label}” was not inserted: {why}.")));
                false
            }
        }
    }

    /// Closes it, the keyboard going back where it was: Close and Quit go on from there.
    pub(super) fn block_library_close(&mut self, ctx: &Context) {
        if let Some(id) = self.block_library.open.take().and_then(|o| o.previous_focus) {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }

    fn open_block_library(&mut self, ctx: &Context, opened: Opened, query: String) -> bool {
        let package = self.block_package().cloned();
        let Some(d) = &self.doc else { return false };
        let language = package_language(d.viewer.ed.session.doc());
        let mut library = package
            .as_ref()
            .map_or_else(Default::default, |p| p.index.library(&language));
        if d.viewer.ed.view.sections_on() {
            library.add_section(package.as_ref().and_then(|p| p.index.sections()), &language);
        }
        if library.is_empty() {
            return false;
        }
        let origin = (d.id, d.viewer.ed.session.revision().counter);
        let anchor = if self.source_focused() {
            None
        } else {
            d.viewer.caret_screen
        };
        if let Some(package) = package {
            self.block_form.assets.use_package(package);
        }
        self.block_library.open = Some(OpenLibrary::new(
            origin,
            library,
            opened,
            query,
            anchor,
            ctx.memory(|m| m.focused()),
        ));
        true
    }

    /// Per frame, before the document is drawn, so that the keys it takes do not reach it.
    pub(super) fn block_library_window(&mut self, ctx: &Context) {
        self.block_drop();
        self.block_delete_pressed();
        self.section_control_pressed();
        if let Some(query) = self.block_library.requested.take() {
            // the rest of a large file is on its way: an id in it could not be seen, so it is typing
            if self.progressive.is_none() {
                self.open_block_library(ctx, Opened::Typed, query);
            } else if let Some(d) = &mut self.doc {
                d.viewer.type_after_library(format!("/{query}"));
                ctx.request_repaint();
            }
        }
        let Some(open) = &mut self.block_library.open else {
            return;
        };
        // anything changing under it ends it: what it would insert has no place any more
        let current = self
            .doc
            .as_ref()
            .map(|d| (d.id, d.viewer.ed.session.revision().counter));
        let offers = matches!(self.package.status, crate::package_ui::PackageStatus::Ready(_))
            || self.doc.as_ref().is_some_and(|d| d.viewer.ed.view.sections_on());
        if current != Some(open.origin) || !offers {
            self.block_library.open = None;
            return;
        }
        let t = self.theme.clone();
        let act = open.ui(
            ctx,
            &style_for(&t),
            color(t.chrome.sidebar_bg),
            &mut self.block_form.assets,
        );
        let Some(act) = act else { return };
        let Some(open) = self.block_library.open.take() else {
            return;
        };
        // the keyboard goes back in this very frame: what the field took is not the document's
        ctx.input_mut(|i| {
            i.events
                .retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(_)));
        });
        if act != LibraryAct::Dismiss
            && let Some(id) = open.previous_focus
        {
            ctx.memory_mut(|m| m.request_focus(id));
        }
        match act {
            LibraryAct::Choose(LibraryItem::Preset(preset)) => {
                self.insert_block_preset(preset);
                ctx.request_repaint();
            }
            LibraryAct::Choose(LibraryItem::Section) => {
                let in_source = self.source_focused();
                self.open_section_form(ctx, in_source);
                ctx.request_repaint();
            }
            LibraryAct::Type(text) => {
                if let Some(d) = &mut self.doc {
                    d.viewer.type_after_library(text);
                }
                ctx.request_repaint();
            }
            LibraryAct::Close | LibraryAct::Dismiss => {}
        }
    }
}
