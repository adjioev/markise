//! The block form's glue (ADR 0020): when a form opens and closes, which keys reach it, and the
//! file dialog it asks for. See [`crate::block_form`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::Context;
use egui_schema_form::Thumbnail;
use markise_layout::BlockViewMode;
use markise_layout::block::color;

use super::{MarkiseApp, Then};
use crate::block_form::{FormAssets, OpenForm, PanelAction, PanelHost, block_at, panel_widths, style_for};

/// Chooses a file, starting in the given folder.
pub type ImagePicker = Arc<dyn Fn(&Path) -> Option<PathBuf> + Send + Sync>;

/// What waits for a changed draft to be saved or discarded.
pub(super) enum DraftNext {
    Then(Then),
    /// Stop Using This Package.
    StopPackage,
    /// Delete the block with this opener.
    DeleteBlock(usize),
}

/// The form of the session.
pub(super) struct BlockFormState {
    pub(super) open: Option<OpenForm>,
    /// How many forms were opened: names the one open, as its file requests are numbered by itself.
    pub(super) opened: u64,
    /// Asked for by the document's drawing: opens on the next frame.
    requested: Option<usize>,
    /// A block's view button was pressed: switches on the next frame.
    switch_requested: Option<usize>,
    pub(super) picker: ImagePicker,
    pub(super) assets: FormAssets,
    /// How each document showed its blocks, for the session.
    view_states: HashMap<PathBuf, ViewState>,
}

type ViewState = (BlockViewMode, Vec<(usize, BlockViewMode)>);

impl BlockFormState {
    pub(super) fn new(picker: Option<ImagePicker>) -> BlockFormState {
        BlockFormState {
            open: None,
            opened: 0,
            requested: None,
            switch_requested: None,
            assets: FormAssets::default(),
            view_states: HashMap::new(),
            picker: picker.unwrap_or_else(|| {
                Arc::new(|folder: &Path| {
                    rfd::FileDialog::new()
                        .set_directory(folder)
                        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"])
                        .pick_file()
                })
            }),
        }
    }
}

impl BlockFormState {
    /// The worker has nothing outstanding (tests: [`MarkiseApp::background_settled`]).
    pub(super) fn is_idle(&self) -> bool {
        self.assets.is_idle()
    }

    pub(super) fn request(&mut self, at: Option<usize>) {
        if at.is_some() {
            self.requested = at;
        }
    }

    pub(super) fn request_switch(&mut self, opener: Option<usize>) {
        if opener.is_some() {
            self.switch_requested = opener;
        }
    }
}

impl MarkiseApp {
    /// A form is open.
    pub fn block_form(&self) -> Option<&OpenForm> {
        self.block_form.open.as_ref()
    }

    /// The caret is in a block with a form.
    pub(super) fn block_form_available(&self) -> bool {
        let (Some(p), Some(d)) = (self.block_package(), &self.doc) else {
            return false;
        };
        let s = &d.viewer.ed.session;
        block_at(s.doc(), s.selection().primary().focus.offset, &p.index).is_some()
    }

    /// A form holds changes the document does not.
    pub(super) fn block_draft_changed(&self) -> bool {
        self.block_form.open.as_ref().is_some_and(OpenForm::is_dirty)
    }

    /// Something for the open form to say.
    pub(super) fn block_form_message(&mut self, text: &str) {
        if let Some(open) = &mut self.block_form.open {
            open.set_message(text);
        }
    }

    /// The block at the caret has a view to switch from or to.
    pub(super) fn block_view_available(&self) -> bool {
        self.block_with_view_at_caret().is_some()
    }

    /// The block around the caret, when it has more than one thing to show.
    fn block_with_view_at_caret(&self) -> Option<usize> {
        let (package, d) = (self.block_package()?, self.doc.as_ref()?);
        let s = &d.viewer.ed.session;
        let c = block_at(s.doc(), s.selection().primary().focus.offset, &package.index)?;
        let has_view = package.index.block(&c.name)?.view.is_some();
        (has_view || d.viewer.ed.view.can_draw_site()).then_some(c.opener.start)
    }

    pub(super) fn switch_block_view_at_caret(&mut self) -> bool {
        let Some(opener) = self.block_with_view_at_caret() else {
            return false;
        };
        let Some(d) = &mut self.doc else { return false };
        d.viewer.ed.switch_block_view(opener);
        self.remember_block_views();
        true
    }

    pub(super) fn switch_all_block_views(&mut self) -> bool {
        let Some(d) = &mut self.doc else { return false };
        d.viewer.ed.switch_all_block_views();
        self.remember_block_views();
        true
    }

    /// The package has blocks that can be practiced (ADR 0024).
    pub(super) fn block_practice_available(&self) -> bool {
        (self.block_package()).is_some_and(|p| {
            (p.index.block_names()).any(|n| {
                p.index
                    .block(n)
                    .is_some_and(|e| e.view.is_some_and(|v| v.practice.is_some()))
            })
        })
    }

    /// Practice for the whole document, or out of it.
    pub(super) fn toggle_practice_all(&mut self) -> bool {
        if !self.block_practice_available() {
            return false;
        }
        let Some(d) = &mut self.doc else { return false };
        d.viewer.ed.toggle_practice_all();
        self.remember_block_views();
        true
    }

    /// Keeps how this document shows its blocks, for the session (ADR 0021).
    pub(super) fn remember_block_views(&mut self) {
        if let Some(d) = &self.doc
            && !d.untitled
        {
            let state = d.viewer.ed.block_view_state();
            self.block_form.view_states.insert(d.path.clone(), state);
        }
    }

    /// What was kept for this document, in its new viewer.
    pub(super) fn bring_back_block_views(&mut self) {
        let Some(d) = &mut self.doc else { return };
        if let Some((mode, own)) = self.block_form.view_states.get(&d.path) {
            d.viewer.ed.restore_block_view_state(*mode, own);
        }
    }

    /// The keyboard is in the form.
    pub(super) fn block_form_focused(&self, ctx: &Context) -> bool {
        self.block_form.open.as_ref().is_some_and(|f| f.is_focused(ctx))
    }

    pub(super) fn open_block_form_at_caret(&mut self) -> bool {
        let Some(d) = &self.doc else { return false };
        let at = d.viewer.ed.session.selection().primary().focus.offset;
        self.open_block_form(at)
    }

    /// Opens the form of the block around `at`. A draft of another block is not thrown away for it.
    pub fn open_block_form(&mut self, at: usize) -> bool {
        let Some(package) = self.block_package().cloned() else {
            return false;
        };
        let Some(d) = &mut self.doc else { return false };
        // a block being practiced has no commands (ADR 0024)
        if d.viewer.ed.in_practice(at) {
            return false;
        }
        let Some(c) = block_at(d.viewer.ed.session.doc(), at, &package.index) else {
            return false;
        };
        // a draft of a document that is gone has nowhere to go, and only Cancel can end it
        if let Some(open) = &mut self.block_form.open
            && open.doc() != d.id
            && open.is_dirty()
        {
            open.set_message("Copy what you need and Cancel this draft before opening another block.");
            return false;
        }
        if let Some(open) = &mut self.block_form.open
            && open.doc() == d.id
        {
            if open.key() == c.opener.start {
                return true;
            }
            if open.is_dirty() {
                open.set_message("Save or cancel this draft before opening another block.");
                return false;
            }
            open.release(&mut d.viewer.ed.session);
        }
        let index = package.index.clone();
        self.block_form.assets.use_package(package);
        self.block_form.assets.forget_paths();
        self.block_form.open = OpenForm::open(d.id, &c, &mut d.viewer.ed.session, &index);
        self.block_form.opened += 1;
        self.block_form.open.is_some()
    }

    /// Drops the draft that was answered for: the document or package goes next.
    pub(super) fn block_form_discard(&mut self) {
        self.block_form_close();
    }

    /// Closes the form, giving back what it holds in its document's session.
    fn block_form_close(&mut self) {
        if let Some(open) = self.block_form.open.take()
            && let Some(d) = self.doc.as_mut()
            && d.id == open.doc()
        {
            open.release(&mut d.viewer.ed.session);
        }
    }

    /// Cancels the draft (the Cancel button and its keys).
    pub fn block_form_cancel(&mut self) {
        if self.block_form.open.is_some() {
            self.block_form_close();
            if let Some(d) = &mut self.doc {
                d.viewer.request_refocus();
            }
        }
    }

    /// Saves the draft: one undo step, then the panel closes and the keyboard goes back to the
    /// document. A refusal stays in the panel with its reason.
    pub fn block_form_save(&mut self, _ctx: &Context) {
        self.block_form_write();
    }

    /// [`Self::block_form_save`]. A draft of another document, one that was replaced without a
    /// question (the panel may not have noticed yet), goes nowhere and stays readable.
    pub(super) fn block_form_write(&mut self) {
        let Some(open) = self.block_form.open.as_mut() else {
            return;
        };
        let Some(d) = self.doc.as_mut().filter(|d| d.id == open.doc()) else {
            if !open.orphan() {
                self.block_form.open = None;
            }
            return;
        };
        match d.viewer.ed.edit(|s| open.save(s)) {
            Ok(()) => {
                self.block_form_close();
                if let Some(d) = &mut self.doc {
                    d.viewer.request_refocus();
                }
            }
            Err(why) => open.set_message(why),
        }
    }

    /// The keyboard is in a form with a composition going on.
    pub(super) fn block_form_composing(&self) -> bool {
        self.block_form.open.as_ref().is_some_and(OpenForm::is_composing)
    }

    /// Reads the block again, losing the draft.
    fn block_form_reload(&mut self) {
        let Some(key) = self.block_form.open.as_ref().map(OpenForm::key) else {
            return;
        };
        self.block_form_close();
        self.open_block_form(key);
    }

    /// Per frame, before the document is drawn. Closes what lost its document, package or block.
    pub(super) fn block_form_panel(&mut self, ui: &mut egui::Ui) {
        let index = self.block_package().map(|p| p.index.clone());
        let package = self.block_package().cloned();
        if let Some(d) = self.doc.as_mut() {
            d.viewer.set_block_index(index.clone());
            d.viewer
                .set_block_site(package.as_ref(), self.web_helpers.as_ref(), ui.ctx());
        }
        self.block_form.assets.poll(ui.ctx());
        self.block_icons_update(ui.ctx());
        if let Some(at) = self.block_form.requested.take() {
            self.open_block_form(at);
        }
        if let Some(opener) = self.block_form.switch_requested.take()
            && let Some(d) = &mut self.doc
        {
            d.viewer.ed.switch_block_view(opener);
            self.remember_block_views();
        }
        if self.block_form.open.is_none() {
            return;
        }
        // Anything that can be cancelled asked before getting here. What is left cannot be: a
        // lookup that lost the package (Save As into a folder without it) keeps the draft and its
        // Save, a document replaced without a question keeps the draft to read and copy.
        let keep = match (self.doc.as_mut(), self.block_form.open.as_mut(), index) {
            (Some(d), Some(open), index) if open.doc() == d.id => {
                (index.is_some() || open.is_dirty()) && open.follow(&d.viewer.ed.session)
            }
            (_, Some(open), _) => open.orphan(),
            _ => false,
        };
        if !keep {
            self.block_form_close();
            return;
        }
        if self.block_form.open.as_ref().is_some_and(OpenForm::needs_reopen) {
            self.block_form_reload();
        }
        let t = self.theme.clone();
        let (min, default, max) = panel_widths(ui.available_width());
        let mut action = None;
        egui::Panel::right("markise-block-form")
            .resizable(true)
            .default_size(default)
            .size_range(min..=max)
            .frame(
                egui::Frame::new()
                    .fill(color(t.chrome.sidebar_bg))
                    .inner_margin(egui::Margin::symmetric(14, 12)),
            )
            .show(ui, |ui| {
                let Some(open) = self.block_form.open.as_mut() else {
                    return;
                };
                let ctx = ui.ctx().clone();
                let path = self.doc.as_ref().and_then(|d| d.file().map(Path::to_path_buf));
                let folder = path.as_deref().and_then(Path::parent);
                let root_url = self.doc.as_ref().and_then(|d| d.viewer.model().root_url.clone());
                let site_pictures = self.web_helpers.is_some();
                let mut host = PanelHost {
                    style: style_for(&t),
                    ctx: &ctx,
                    images: &mut self.images,
                    assets: &mut self.block_form.assets,
                    folder,
                    root_url: root_url.as_deref(),
                    site_pictures,
                };
                action = open.ui(ui, &mut host);
            });
        self.block_form_files();
        match action {
            Some(PanelAction::Save) => {
                let ctx = ui.ctx().clone();
                self.block_form_save(&ctx);
            }
            Some(PanelAction::Cancel) => self.block_form_cancel(),
            Some(PanelAction::Reload) => self.block_form_reload(),
            None => {}
        }
    }

    /// The block icons the document's layout has asked for, through the form's worker. Costs a lock
    /// and a look at an empty set when nothing is on its way.
    fn block_icons_update(&mut self, ctx: &Context) {
        let Some(d) = &self.doc else { return };
        let board = d.viewer.block_icons().clone();
        if !board.is_waiting() {
            return;
        }
        let Some(package) = self.block_package().cloned() else {
            return;
        };
        let assets = &mut self.block_form.assets;
        assets.use_package(package);
        let arrived = board.take_answers(|name| match assets.svg(ctx, name) {
            Thumbnail::Mask { texture, .. } => Some(Some((texture, true))),
            Thumbnail::Ready { texture, .. } => Some(Some((texture, false))),
            Thumbnail::Loading => None,
            Thumbnail::Failed(_) | Thumbnail::None => Some(None),
        });
        if arrived {
            ctx.request_repaint();
        }
    }

    /// The files the form asked for: the image picker, between frames as the app's other windows.
    fn block_form_files(&mut self) {
        let Some(open) = self.block_form.open.as_mut() else {
            return;
        };
        let requests = open.take_picking();
        if requests.is_empty() {
            return;
        }
        // one window at a time; another click asks again
        for request in requests {
            self.pick_for_form(request);
        }
    }
}
