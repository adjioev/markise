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
    /// The preset last chosen.
    chosen: Option<usize>,
}

impl BlockLibraryState {
    pub(super) fn request(&mut self, query: Option<String>) {
        if query.is_some() {
            self.requested = query;
        }
    }
}

impl MarkiseApp {
    /// The open library.
    pub fn block_library(&self) -> Option<&OpenLibrary> {
        self.block_library.open.as_ref()
    }

    /// The preset last chosen in the library, by its place in the package.
    pub fn block_library_chosen(&self) -> Option<usize> {
        self.block_library.chosen
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
        if let Some(query) = self.block_library.requested.take() {
            self.open_block_library(ctx, Opened::Typed, query);
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
            LibraryAct::Choose(preset) => self.block_library.chosen = Some(preset),
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
