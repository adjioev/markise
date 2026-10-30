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
//! - a pasted image URL (setting `images.paste_urls`): inserted as `![](url)` (no I/O);
//! - dropped files (anywhere in the window while a document is open): inserted at the drop
//!   point when it is over the rendered document, else at the caret. Without a document, a
//!   dropped Markdown file is opened.
//!
//! Every paste and drop becomes one entry of a single ordered queue. Entries resolve in parallel
//! (the clipboard's file list, writing or copying files and encoding images by
//! `markise_engine::attach`, all on worker threads), but they are inserted strictly in queue
//! order: an entry waits until every earlier one was inserted or cancelled. Each insertion is
//! one undo step at the selection captured when the entry was made, re-based through the
//! insertions of earlier entries; when the user changed the document meanwhile, a text paste is
//! cancelled and written files are linked at the caret instead. Undo removes the text, never the
//! file. A never-saved document's pasted images go to the app-data folder for user images
//! (`<config>/markise/user-images/`) with absolute paths; without a settings folder (tests that
//! do not set one) the user is asked to save first.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Context, Event};
use markise_doc::Selection;
use markise_engine::attach::{self, Item, Outcome, Request};

use super::{Banner, MarkiseApp};

/// How long a paste waits for the clipboard's file list before it goes in as text.
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(1);
/// How long an attachment job (copying, encoding) may take before it is given up: the queue
/// moves on with a message and a late result is ignored.
const JOB_TIMEOUT: Duration = Duration::from_secs(30);

/// Test hook run on the job worker before the job (e.g. to stall it).
pub type JobHook = Arc<dyn Fn() + Send + Sync>;

/// What an entry still waits for, or what it inserts.
enum Stage {
    /// A text paste whose clipboard file list is being read.
    Clipboard { text: String, rx: Receiver<Vec<PathBuf>> },
    /// Files or image data being written (since `Instant`).
    Job(Receiver<Outcome>, Instant),
    /// Text to paste (cancelled when the user changed the document meanwhile).
    Text(String),
    /// A finished job: its Markdown goes in (at the caret when the document changed).
    Done(Outcome),
}

/// One paste or drop, in queue order.
struct Entry {
    doc_id: u64,
    /// The session revision counter when the entry was made.
    counter: u64,
    /// The selection then (the source pane's in Source / Side by Side; a caret at the drop
    /// point for drops).
    sel: Selection,
    source: bool,
    started: Instant,
    stage: Stage,
    /// Where files of this entry go, captured when it was made (a clipboard file list that
    /// arrives later must not use another document's folder).
    dest: Option<Request>,
}

/// An insertion made by this module (not by the user): queued entries captured before it are
/// re-based through it instead of being cancelled.
#[derive(Clone, Copy, Debug)]
struct OwnEdit {
    doc_id: u64,
    /// Session revision counters before and after the insertion.
    from: u64,
    to: u64,
    /// The replaced range's start, its length and the inserted length.
    pos: usize,
    removed: usize,
    inserted: usize,
}

impl OwnEdit {
    /// `o` after this edit (a position at the insertion point moves past the inserted text, so
    /// queued entries stay in order).
    fn map(&self, o: usize) -> usize {
        if o < self.pos {
            o
        } else if o >= self.pos + self.removed {
            o - self.removed + self.inserted
        } else {
            self.pos + self.inserted
        }
    }
}

pub(crate) struct AttachState {
    clipboard: Arc<dyn crate::clipboard::ClipboardFiles>,
    queue: VecDeque<Entry>,
    /// Our own insertions since the oldest queued entry was made.
    own_edits: Vec<OwnEdit>,
    /// Fixed `YYYYMMDDHHMMSS` for pasted image names (tests).
    stamp: Option<String>,
    job_timeout: Duration,
    job_hook: Option<JobHook>,
}

impl AttachState {
    pub(crate) fn new(clipboard: Arc<dyn crate::clipboard::ClipboardFiles>) -> Self {
        AttachState {
            clipboard,
            queue: VecDeque::new(),
            own_edits: Vec::new(),
            stamp: None,
            job_timeout: JOB_TIMEOUT,
            job_hook: None,
        }
    }
}

impl MarkiseApp {
    /// Test hook ([`MarkiseApp::background_settled`]): no paste or drop is being resolved.
    pub(super) fn attach_settled(&self) -> bool {
        self.attach.queue.is_empty()
    }

    /// Uses `stamp` (`YYYYMMDDHHMMSS`) instead of the clock for pasted image names (tests).
    pub fn set_paste_stamp(&mut self, stamp: Option<String>) {
        self.attach.stamp = stamp;
    }

    /// Test seams: the attachment job deadline and a hook run on the job worker before each job.
    pub fn set_attach_job_test(&mut self, timeout: Duration, hook: Option<JobHook>) {
        self.attach.job_timeout = timeout;
        self.attach.job_hook = hook;
    }

    /// A paste or drop is still resolving or waiting for its turn.
    pub fn is_attaching(&self) -> bool {
        !self.attach.queue.is_empty()
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
        // (a slow clipboard owner must not freeze the window), the paste waits for it; while
        // earlier entries are queued, every paste queues behind them (order)
        let ask_files = has_text && self.attach.clipboard.may_have_files();
        let queued = !self.attach.queue.is_empty();
        // in event order
        let mut taken: Vec<Event> = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::PasteImage(_) => {
                    taken.push(e.clone());
                    false
                }
                Event::Paste(t) if ask_files || queued || (paste_urls && attach::is_image_url(t)) => {
                    taken.push(e.clone());
                    false
                }
                _ => true,
            })
        });
        for e in taken {
            match e {
                Event::PasteImage(img) => {
                    let stage = self.attach_request().map(|r| self.job(r, vec![Item::Image(img)]));
                    self.enqueue(None, stage);
                }
                Event::Paste(t) if ask_files => {
                    let rx = self.spawn_clipboard();
                    self.enqueue(None, Some(Stage::Clipboard { text: t, rx }));
                }
                Event::Paste(t) => {
                    let text = self.paste_text(&t);
                    self.enqueue(None, Some(Stage::Text(text)));
                }
                _ => {}
            }
        }
        self.attach_poll();
    }

    /// The text a paste inserts: an image URL as an image (setting), else the text.
    fn paste_text(&self, t: &str) -> String {
        if self.themes.image_settings().paste_urls && attach::is_image_url(t) {
            format!("![]({})", t.trim())
        } else {
            t.to_string()
        }
    }

    /// Reads the clipboard's file list on a worker.
    fn spawn_clipboard(&self) -> Receiver<Vec<PathBuf>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let clipboard = self.attach.clipboard.clone();
        let wake = self.wake_handle();
        let failed = std::thread::Builder::new()
            .name("markise-clipboard".into())
            .spawn({
                let tx = tx.clone();
                move || {
                    let files = clipboard.files();
                    crate::wake_track::publishing();
                    let _ = tx.send(files);
                    wake();
                    crate::wake_track::woken();
                }
            })
            .is_err();
        if failed {
            let _ = tx.send(Vec::new()); // no thread: the paste is plain text
        }
        rx
    }

    /// Appends an entry for the open document at `at` (a drop point) or its current selection.
    fn enqueue(&mut self, at: Option<usize>, stage: Option<Stage>) {
        let Some(stage) = stage else { return };
        let source = self.source_visible || self.side_by_side;
        let dest = self.attach_request();
        let Some(d) = &self.doc else { return };
        let sel = match (at, &d.viewer.ed.source, source) {
            (Some(o), _, _) => Selection::caret(o),
            (None, Some(p), true) => p.sel.clone(),
            _ => d.viewer.ed.session.selection().clone(),
        };
        self.attach.queue.push_back(Entry {
            doc_id: d.id,
            counter: d.viewer.ed.session.revision().counter,
            sel,
            source,
            started: Instant::now(),
            stage,
            dest,
        });
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
        let stage = self
            .attach_request()
            .map(|r| self.job(r, files.into_iter().map(Item::File).collect()));
        self.enqueue(at, stage);
    }

    /// The destination context of the open document (no items yet): note, vault root handle,
    /// settings, resource root.
    fn attach_request(&self) -> Option<Request> {
        let d = self.doc.as_ref()?;
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
        Some(Request {
            items: Vec::new(),
            settings: self.themes.image_settings().clone(),
            note,
            vault,
            obsidian_folder,
            user_images: self.user_images_folder(),
            stamp: self.attach.stamp.clone().unwrap_or_else(attach::stamp_now),
            resource_root: d.viewer.ed.session.doc().resource_root(),
        })
    }

    /// Starts writing `items` to the destination `req` on a worker.
    fn job(&self, mut req: Request, items: Vec<Item>) -> Stage {
        req.items = items;
        let (tx, rx) = std::sync::mpsc::channel();
        let (wake, hook) = (self.wake_handle(), self.attach.job_hook.clone());
        let spawned = std::thread::Builder::new()
            .name("markise-attach".into())
            .spawn(move || {
                if let Some(h) = hook {
                    h();
                }
                let out = attach::run(&req);
                crate::wake_track::publishing();
                let _ = tx.send(out); // ignored when the entry timed out
                wake();
                crate::wake_track::woken();
            });
        match spawned {
            Ok(_) => Stage::Job(rx, Instant::now()),
            Err(e) => Stage::Done(Outcome {
                errors: vec![format!("cannot start a thread: {e}")],
                ..Default::default()
            }),
        }
    }

    /// Per frame: advances every queued entry (all may resolve at once), then inserts finished
    /// entries from the front, strictly in queue order. Cheap when the queue is empty.
    pub(super) fn attach_poll(&mut self) {
        if self.attach.queue.is_empty() {
            self.attach.own_edits.clear();
            return;
        }
        // entries of a document that is no longer open are cancelled at once (nothing copied)
        let current = self.doc.as_ref().map(|d| d.id);
        self.attach.queue.retain(|e| Some(e.doc_id) == current);
        // resolve
        let mut start_jobs = Vec::new();
        let mut wait: Option<Duration> = None;
        let job_timeout = self.attach.job_timeout;
        for (i, e) in self.attach.queue.iter_mut().enumerate() {
            match &e.stage {
                Stage::Clipboard { text, rx } => {
                    let files = match rx.try_recv() {
                        Ok(f) => Some(f),
                        Err(TryRecvError::Disconnected) => Some(Vec::new()),
                        Err(TryRecvError::Empty) => {
                            let waited = e.started.elapsed();
                            if waited >= CLIPBOARD_TIMEOUT {
                                // a late list is not waited for: the text goes in
                                Some(Vec::new())
                            } else {
                                let left = CLIPBOARD_TIMEOUT - waited;
                                wait = Some(wait.map_or(left, |w| w.min(left)));
                                None
                            }
                        }
                    };
                    match files {
                        Some(f) if f.is_empty() => {
                            let t = text.clone();
                            e.stage = Stage::Text(t); // made final below
                            start_jobs.push((i, None));
                        }
                        Some(f) => start_jobs.push((i, Some(f))),
                        None => {}
                    }
                }
                Stage::Job(rx, since) => match rx.try_recv() {
                    Ok(o) => e.stage = Stage::Done(o),
                    Err(TryRecvError::Disconnected) => {
                        e.stage = Stage::Done(Outcome {
                            errors: vec!["the worker stopped".into()],
                            ..Default::default()
                        })
                    }
                    Err(TryRecvError::Empty) => {
                        let waited = since.elapsed();
                        if waited >= job_timeout {
                            // given up: the queue moves on, a late result is dropped with the
                            // receiver
                            e.stage = Stage::Done(Outcome {
                                errors: vec![format!(
                                    "Writing the pasted or dropped files took longer than {} s and was given up (files may still appear in the image folder).",
                                    job_timeout.as_secs_f32().round()
                                )],
                                ..Default::default()
                            });
                        } else {
                            let left = job_timeout - waited;
                            wait = Some(wait.map_or(left, |w| w.min(left)));
                        }
                    }
                },
                Stage::Text(_) | Stage::Done(_) => {}
            }
        }
        for (i, files) in start_jobs {
            match files {
                None => {
                    if let Stage::Text(t) = &self.attach.queue[i].stage {
                        let t = self.paste_text(t);
                        self.attach.queue[i].stage = Stage::Text(t);
                    }
                }
                Some(f) => {
                    // the destination captured with the paste (the document is still the same:
                    // entries of other documents were removed above)
                    let stage = match self.attach.queue[i].dest.clone() {
                        Some(req) => self.job(req, f.into_iter().map(Item::File).collect()),
                        None => Stage::Done(Outcome::default()),
                    };
                    self.attach.queue[i].stage = stage;
                }
            }
        }
        // insert in order
        while matches!(
            self.attach.queue.front().map(|e| &e.stage),
            Some(Stage::Text(_) | Stage::Done(_))
        ) {
            let Some(e) = self.attach.queue.pop_front() else { break };
            self.insert_entry(e);
        }
        if let (Some(w), Some(c)) = (wait, &self.ctx) {
            c.request_repaint_after(w);
        }
    }

    /// Inserts a finished entry at its selection re-based through our earlier insertions, or
    /// handles a change by the user meanwhile (text: cancelled; files: at the caret).
    fn insert_entry(&mut self, mut e: Entry) {
        let Some(ctx) = self.ctx.clone() else { return };
        let (mut counter, mut a, mut f) = (e.counter, e.sel.primary().anchor.offset, e.sel.primary().focus.offset);
        for o in self.attach.own_edits.iter().filter(|o| o.doc_id == e.doc_id) {
            if o.from == counter {
                (counter, a, f) = (o.to, o.map(a), o.map(f));
            }
        }
        e.sel = Selection::range(a, f);
        let same_doc = self.doc.as_ref().is_some_and(|d| d.id == e.doc_id);
        let unchanged = same_doc
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.viewer.ed.session.revision().counter == counter);
        match e.stage {
            Stage::Text(text) => {
                if !unchanged {
                    if same_doc {
                        self.banner = Some(Banner::Info(
                            "The paste was cancelled: the document changed while the clipboard was read.".into(),
                        ));
                    }
                    return;
                }
                self.restore_selection(&e.sel, e.source);
                self.insert_attachment_text(&ctx, &text);
            }
            Stage::Done(out) => {
                for f in &out.written {
                    if let Some(dir) = f.parent() {
                        self.vault_changed(dir);
                    }
                }
                if same_doc && !out.inserts.is_empty() {
                    if unchanged {
                        self.restore_selection(&e.sel, e.source);
                    }
                    // links generated again for where the note is now (Save As may have moved it
                    // while the job ran); the files stay where they were written
                    let md = self.render_for_current_note(&out);
                    self.insert_attachment_text(&ctx, &md);
                }
                if !out.errors.is_empty() {
                    self.banner = Some(Banner::Error(out.errors.join(" ")));
                }
            }
            Stage::Clipboard { .. } | Stage::Job(..) => {}
        }
    }

    /// `out`'s Markdown for the open document's current file, resource root and relative-paths
    /// setting.
    fn render_for_current_note(&self, out: &Outcome) -> String {
        let Some(d) = &self.doc else { return out.text() };
        let note_dir = d.file().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let root = d
            .viewer
            .ed
            .session
            .doc()
            .resource_root()
            .and_then(|r| attach::resource_root_dir(note_dir.as_deref(), &r));
        out.render(
            note_dir.as_deref(),
            root.as_deref(),
            self.themes.image_settings().relative_paths,
        )
    }

    /// Puts a captured selection back (it may have moved without an edit).
    fn restore_selection(&mut self, sel: &Selection, source: bool) {
        let Some(d) = self.doc.as_mut() else { return };
        let len = d.viewer.ed.session.text().len();
        let p = sel.primary();
        let sel = Selection::range(p.anchor.offset.min(len), p.focus.offset.min(len));
        if source && d.viewer.ed.source.is_some() {
            d.viewer.ed.source_set_selection(sel);
        } else {
            d.viewer.ed.session.set_selection(sel);
            d.viewer.ed.refresh_focus();
        }
    }

    /// Inserts `md` at the current selection as one undo step (in the source editor in Source and
    /// Side by Side, whose rendered pane is read-only) and records it as our own edit.
    fn insert_attachment_text(&mut self, ctx: &Context, md: &str) {
        let source = self.source_visible || self.side_by_side;
        let Some(d) = self.doc.as_mut() else { return };
        let (from, old_len) = (d.viewer.ed.session.revision().counter, d.viewer.ed.session.text().len());
        let range;
        if source && d.viewer.ed.source.is_some() {
            range = d
                .viewer
                .ed
                .source
                .as_ref()
                .map(|p| p.sel.primary().range())
                .unwrap_or(0..0);
            let _ = d.viewer.ed.source_insert(md, true);
        } else {
            let mut r0 = 0..0;
            d.viewer
                .ed
                .edit(|s| {
                    s.close_typing();
                    r0 = s.selection().primary().range();
                    let r = s.insert_text(md);
                    s.close_typing();
                    r
                })
                .ok();
            range = r0;
            d.viewer.ed.refresh_focus();
        }
        d.sync();
        let (to, new_len) = (d.viewer.ed.session.revision().counter, d.viewer.ed.session.text().len());
        if to != from {
            let removed = range.len();
            self.attach.own_edits.push(OwnEdit {
                doc_id: d.id,
                from,
                to,
                pos: range.start,
                removed,
                inserted: (new_len + removed).saturating_sub(old_len),
            });
        }
        ctx.request_repaint();
    }
}
