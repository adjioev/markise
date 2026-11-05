//! The block library's glue (ADR 0022): when it opens and closes and where the keyboard goes. See
//! [`crate::block_library`].

use egui::Context;
use markise_layout::block::color;

use super::MarkiseApp;
use crate::block_form::{LANGUAGE, style_for};
use crate::block_library::{LibraryAct, OpenLibrary, Opened};

/// The library of the session.
#[derive(Default)]
pub(super) struct BlockLibraryState {
    open: Option<OpenLibrary>,
    /// `/` was typed: opens on the next frame, with what was typed after it.
    requested: Option<String>,
    /// A block was dropped: its opener and the line it goes to.
    requested_move: Option<(usize, usize)>,
}

impl BlockLibraryState {
    pub(super) fn request(&mut self, query: Option<String>) {
        if query.is_some() {
            self.requested = query;
        }
    }

    pub(super) fn request_move(&mut self, drop: Option<(usize, usize)>) {
        if drop.is_some() {
            self.requested_move = drop;
        }
    }
}

impl MarkiseApp {
    /// The open library.
    pub fn block_library(&self) -> Option<&OpenLibrary> {
        self.block_library.open.as_ref()
    }

    /// There is something to offer and the document can take it: the Insert Block command.
    pub(super) fn block_library_available(&self) -> bool {
        self.block_package().is_some_and(|p| !p.index.presets().is_empty())
            && self.progressive.is_none()
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.viewer.ed.session.read_only().is_none() && d.viewer.card_edit().is_none())
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
        if !self.block_move_allowed() {
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

    /// What the drag of a block came to.
    fn block_drop(&mut self) {
        let Some((opener, line)) = self.block_library.requested_move.take() else {
            return;
        };
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
                let label = preset.label.as_ref().map_or(preset.id.as_str(), |l| l.get(LANGUAGE));
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
        let Some(package) = self.block_package().cloned() else {
            return false;
        };
        let Some(d) = &self.doc else { return false };
        let library = package.index.library(LANGUAGE);
        if library.is_empty() {
            return false;
        }
        let origin = (d.id, d.viewer.ed.session.revision().counter);
        let anchor = if self.source_focused() {
            None
        } else {
            d.viewer.caret_screen
        };
        self.block_form.assets.use_package(package);
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
        if current != Some(open.origin) || !matches!(self.package.status, crate::package_ui::PackageStatus::Ready(_)) {
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
            LibraryAct::Choose(preset) => {
                self.insert_block_preset(preset);
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
