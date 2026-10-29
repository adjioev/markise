//! Pasting and dropping images and files into the document (spec 04 §7.1–§7.3, spec 05 §4.4,
//! spec 06 §14; issue #51).
//!
//! Before the document surface sees the frame's input, [`MarkiseApp::attach_pre_input`] takes:
//! - clipboard image data ([`egui::Event::PasteImage`], e.g. a screenshot) while the document
//!   (rendered view or source editor) has the keyboard focus;
//! - a text paste, when the clipboard seam may hold files ([`crate::clipboard`]: a file
//!   manager copy arrives only as the file's name): the paste waits while a worker reads the
//!   file list (never on the UI thread; at most [`CLIPBOARD_TIMEOUT`]), then the files go in
//!   like a drop at the paste position, or else the text is pasted at the selection it was
//!   made at — one undo step either way, cancelled when the document changed meanwhile;
//! - a pasted image URL (setting `images.paste_urls`): inserted as `![](url)` at once (no I/O);
//! - dropped files (anywhere in the window while a document is open): inserted at the drop
//!   point when it is over the rendered document, else at the caret. Without a document, a
//!   dropped Markdown file is opened.
//!
//! The files are written (or copied) by `markise_engine::attach` on a worker thread; when the
//! result arrives the Markdown is inserted as one undo step — at the remembered position when
//! the document did not change meanwhile, else at the caret. Undo removes the text, never the
//! file. A never-saved document's pasted images go to the app-data folder for user images
//! (`<config>/markise/user-images/`) with absolute paths; without a settings folder (tests that
//! do not set one) the user is asked to save first.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Context, Event};
use markise_doc::Selection;
use markise_engine::attach::{self, Item, Outcome, Request};

use super::{Banner, MarkiseApp};

/// An attachment job waiting for its worker.
struct Pending {
    doc_id: u64,
    /// The session revision counter when the job started.
    counter: u64,
    /// Where to insert (drop point) while the document is unchanged; `None`: at the caret.
    at: Option<usize>,
    rx: Receiver<Outcome>,
}

/// How long a paste waits for the clipboard's file list before it goes in as text.
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(1);

/// A text paste waiting for the clipboard's file list (read on a worker).
struct PendingPaste {
    doc_id: u64,
    /// The session revision counter at paste time: if the document changed meanwhile, the paste
    /// is cancelled (its selection would no longer be where the user pasted).
    counter: u64,
    /// The selection at paste time (the source pane's in Source / Side by Side).
    sel: Selection,
    source: bool,
    text: String,
    started: Instant,
    rx: Receiver<Vec<PathBuf>>,
}

impl PendingPaste {
    fn new(d: &super::DocState, sel: Selection, source: bool, text: String, rx: Receiver<Vec<PathBuf>>) -> Self {
        PendingPaste {
            doc_id: d.id,
            counter: d.viewer.ed.session.revision().counter,
            sel,
            source,
            text,
            started: Instant::now(),
            rx,
        }
    }
}

pub(crate) struct AttachState {
    clipboard: Arc<dyn crate::clipboard::ClipboardFiles>,
    pending: Vec<Pending>,
    pastes: VecDeque<PendingPaste>,
    /// Fixed `YYYYMMDDHHMMSS` for pasted image names (tests).
    stamp: Option<String>,
}

impl AttachState {
    pub(crate) fn new(clipboard: Arc<dyn crate::clipboard::ClipboardFiles>) -> Self {
        AttachState {
            clipboard,
            pending: Vec::new(),
            pastes: VecDeque::new(),
            stamp: None,
        }
    }
}

impl MarkiseApp {
    /// Uses `stamp` (`YYYYMMDDHHMMSS`) instead of the clock for pasted image names (tests).
    pub fn set_paste_stamp(&mut self, stamp: Option<String>) {
        self.attach.stamp = stamp;
    }

    /// A paste waits for the clipboard's file list, or pasted or dropped files are still
    /// being written.
    pub fn is_attaching(&self) -> bool {
        !self.attach.pending.is_empty() || !self.attach.pastes.is_empty()
    }

    /// The app-data folder for images pasted into never-saved documents (`None` without a
    /// settings folder).
    pub fn user_images_folder(&self) -> Option<PathBuf> {
        self.themes
            .settings_file()
            .and_then(|f| f.parent().map(|d| d.join("user-images")))
    }

    /// Whether the document (rendered surface or source editor) has the keyboard focus.
    fn document_focused(&self, ctx: &Context) -> bool {
        let Some(d) = &self.doc else { return false };
        if d.viewer.ed.source.is_some() && d.source_ui.has_focus(ctx) {
            return true;
        }
        !self.side_by_side
            && !self.source_visible
            && d.viewer.surface_id().is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
    }

    /// Takes this frame's image pastes, file pastes, image URL pastes and dropped files before
    /// the document surface handles the input (see the module docs).
    pub(super) fn attach_pre_input(&mut self, ctx: &Context) {
        if self.is_asking() {
            return;
        }
        let dropped: Vec<PathBuf> = ctx.input_mut(|i| {
            if i.raw.dropped_files.is_empty() {
                return Vec::new();
            }
            std::mem::take(&mut i.raw.dropped_files)
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.drop_files(ctx, dropped);
        }
        if self.doc.is_none() || !self.document_focused(ctx) {
            return;
        }
        let (has_image, has_text) = ctx.input(|i| {
            (
                i.events.iter().any(|e| matches!(e, Event::PasteImage(_))),
                i.events.iter().any(|e| matches!(e, Event::Paste(_))),
            )
        });
        if !has_image && !has_text {
            return;
        }
        let paste_urls = self.themes.image_settings().paste_urls;
        // a text paste may stand for a file manager copy: its file list is read on a worker
        // (a slow clipboard owner must not freeze the window), the paste waits for it
        let ask_files = has_text && self.attach.clipboard.may_have_files();
        let mut images = Vec::new();
        let mut urls = Vec::new();
        let mut texts = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::PasteImage(img) => {
                    images.push(Item::Image(img.clone()));
                    false
                }
                Event::Paste(t) if ask_files => {
                    texts.push(t.clone());
                    false
                }
                Event::Paste(t) if paste_urls && attach::is_image_url(t) => {
                    urls.push(format!("![]({})", t.trim()));
                    false
                }
                _ => true,
            })
        });
        for md in urls {
            self.insert_attachment_text(ctx, &md, None);
        }
        if !images.is_empty() {
            self.start_attach(images, None);
        }
        if !texts.is_empty() {
            self.start_paste(texts.concat());
        }
    }

    /// Starts reading the clipboard's file list for the text paste `text` on a worker; the paste
    /// is applied by [`Self::attach_poll`] at the selection it had now.
    fn start_paste(&mut self, text: String) {
        let source = self.source_visible || self.side_by_side;
        let Some(d) = &self.doc else { return };
        let sel = match (&d.viewer.ed.source, source) {
            (Some(p), true) => p.sel.clone(),
            _ => d.viewer.ed.session.selection().clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let clipboard = self.attach.clipboard.clone();
        let wake = self.wake_handle();
        let spawned = std::thread::Builder::new()
            .name("markise-clipboard".into())
            .spawn(move || {
                let _ = tx.send(clipboard.files());
                wake();
            });
        if spawned.is_err() {
            // no thread: the paste is plain text (nothing was read)
            drop(rx);
            let (tx, rx2) = std::sync::mpsc::channel();
            let _ = tx.send(Vec::new());
            return self.queue_paste(PendingPaste::new(d, sel, source, text, rx2));
        }
        let p = PendingPaste::new(d, sel, source, text, rx);
        self.queue_paste(p);
    }

    fn queue_paste(&mut self, p: PendingPaste) {
        self.attach.pastes.push_back(p);
    }

    /// Files dropped on the window: inserted into the open document (at the drop point over the
    /// rendered view), or the first Markdown file opened when there is no document.
    fn drop_files(&mut self, ctx: &Context, files: Vec<PathBuf>) {
        if self.doc.is_none() {
            if let Some(p) = files.into_iter().find(|p| markise_engine::tree::is_markdown(p)) {
                self.open(p);
            }
            return;
        }
        let pos = ctx.input(|i| i.pointer.latest_pos());
        let mut at = None;
        if !self.source_visible
            && !self.side_by_side
            && let (Some(p), Some(d)) = (pos, self.doc.as_mut())
        {
            at = d.viewer.offset_at_screen(ctx, self.parley.as_mut(), &self.images, p);
            if let Some(o) = at {
                // the caret goes to the drop point (and the surface takes the focus)
                d.viewer.ed.session.set_selection(Selection::caret(o));
                d.viewer.ed.refresh_focus();
                d.viewer.request_refocus();
            }
        }
        self.start_attach(files.into_iter().map(Item::File).collect(), at);
    }

    /// Starts writing `items` for the open document on a worker.
    fn start_attach(&mut self, items: Vec<Item>, at: Option<usize>) {
        let Some(d) = &self.doc else { return };
        let note = d.file().map(|p| p.to_path_buf());
        let index = self.vault_index().cloned();
        let vault = match &index {
            Some(ix) => Some((ix.root().to_path_buf(), ix.root_dir().cloned())),
            None => self.tree.as_ref().map(|t| (t.root.clone(), None)),
        };
        let obsidian_folder = index
            .as_ref()
            .filter(|ix| ix.settings().obsidian)
            .and_then(|ix| ix.settings().attachment_folder.clone());
        let req = Request {
            items,
            settings: self.themes.image_settings().clone(),
            note,
            vault,
            obsidian_folder,
            user_images: self.user_images_folder(),
            stamp: self.attach.stamp.clone().unwrap_or_else(attach::stamp_now),
        };
        let rx = attach::spawn(req, self.wake_handle());
        self.attach.pending.push(Pending {
            doc_id: d.id,
            counter: d.viewer.ed.session.revision().counter,
            at,
            rx,
        });
    }

    /// Per frame: inserts the Markdown of finished attachment jobs (in start order) and reports
    /// failures.
    pub(super) fn attach_poll(&mut self) {
        self.paste_poll();
        while let Some(p) = self.attach.pending.first() {
            let out = match p.rx.try_recv() {
                Ok(o) => o,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Outcome {
                    errors: vec!["the worker stopped".into()],
                    ..Default::default()
                },
            };
            let p = self.attach.pending.remove(0);
            for f in &out.written {
                if let Some(dir) = f.parent() {
                    self.vault_changed(dir);
                }
            }
            let same_doc = self.doc.as_ref().is_some_and(|d| d.id == p.doc_id);
            if same_doc && !out.inserts.is_empty() {
                let unchanged = self
                    .doc
                    .as_ref()
                    .is_some_and(|d| d.viewer.ed.session.revision().counter == p.counter);
                let at = p.at.filter(|_| unchanged);
                if let Some(ctx) = self.ctx.clone() {
                    self.insert_attachment_text(&ctx, &out.text(), at);
                }
            }
            if !out.errors.is_empty() {
                self.banner = Some(Banner::Error(out.errors.join(" ")));
            }
        }
    }

    /// Applies the text pastes whose clipboard file list arrived (in paste order): the files
    /// go in like a drop at the paste position, otherwise the text is pasted (an image URL as
    /// an image). Cancelled when the document changed meanwhile; a file list that takes longer
    /// than [`CLIPBOARD_TIMEOUT`] is not waited for (the text goes in, a late list is ignored).
    fn paste_poll(&mut self) {
        while let Some(p) = self.attach.pastes.front() {
            let files = match p.rx.try_recv() {
                Ok(f) => f,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let waited = p.started.elapsed();
                    if waited < CLIPBOARD_TIMEOUT {
                        if let Some(c) = &self.ctx {
                            c.request_repaint_after(CLIPBOARD_TIMEOUT - waited);
                        }
                        return;
                    }
                    Vec::new()
                }
            };
            let Some(p) = self.attach.pastes.pop_front() else {
                return;
            };
            let Some(ctx) = self.ctx.clone() else { return };
            let current = self
                .doc
                .as_ref()
                .is_some_and(|d| d.id == p.doc_id && d.viewer.ed.session.revision().counter == p.counter);
            if !current {
                if self.doc.as_ref().is_some_and(|d| d.id == p.doc_id) {
                    self.banner = Some(Banner::Info(
                        "The paste was cancelled: the document changed while the clipboard was read.".into(),
                    ));
                }
                continue;
            }
            if files.is_empty() {
                let text = if self.themes.image_settings().paste_urls && attach::is_image_url(&p.text) {
                    format!("![]({})", p.text.trim())
                } else {
                    p.text.clone()
                };
                self.restore_paste_selection(&p);
                self.insert_attachment_text(&ctx, &text, None);
            } else {
                let at = p.sel.primary().start().offset;
                self.restore_paste_selection(&p);
                self.start_attach(files.into_iter().map(Item::File).collect(), Some(at));
            }
        }
    }

    /// Puts the selection a paste was made at back (it may have moved without an edit).
    fn restore_paste_selection(&mut self, p: &PendingPaste) {
        let Some(d) = self.doc.as_mut() else { return };
        if p.source && d.viewer.ed.source.is_some() {
            d.viewer.ed.source_set_selection(p.sel.clone());
        } else {
            d.viewer.ed.session.set_selection(p.sel.clone());
            d.viewer.ed.refresh_focus();
        }
    }

    /// Inserts `md` as one undo step: in the source editor when it has the focus, else in the
    /// rendered document at `at` (or the current selection).
    fn insert_attachment_text(&mut self, ctx: &Context, md: &str, at: Option<usize>) {
        // the source editor is where edits go in Source and Side by Side (whose rendered pane
        // is read-only)
        let source = self.source_visible || self.side_by_side;
        let Some(d) = self.doc.as_mut() else { return };
        if source && d.viewer.ed.source.is_some() {
            if let Some(o) = at {
                let o = o.min(d.viewer.ed.session.text().len());
                d.viewer.ed.source_set_selection(Selection::caret(o));
            }
            let _ = d.viewer.ed.source_insert(md, true);
        } else {
            d.viewer
                .ed
                .edit(|s| {
                    s.close_typing();
                    if let Some(o) = at {
                        s.set_selection(Selection::caret(o.min(s.text().len())));
                    }
                    let r = s.insert_text(md);
                    s.close_typing();
                    r
                })
                .ok();
            d.viewer.ed.refresh_focus();
        }
        d.sync();
        ctx.request_repaint();
    }
}
