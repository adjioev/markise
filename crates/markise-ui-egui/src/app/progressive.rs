//! Progressive open (issue #126, ADR 0017): a large file is shown from its first part while the
//! loader's rest thread parses the whole text; the whole document then replaces the part
//! without moving the view, with the edits made meanwhile carried over.
//!
//! While the rest loads the document is *partial* ([`DocState::partial`]): reading, scrolling
//! and editing work on the part shown; outline, find, Go to End and `[toc]` cover that part
//! (the find bar and the outline say so); the word count waits for the whole document; a save
//! (also Save on Close, Save As, autosave) waits and runs when the whole document is in
//! ("saving when done" in the status bar); draft backups wait; exports, the clean copy,
//! Replace All and Accept/Reject All are unavailable (they would cover only the part).
//! The loading state is shown only in the status bar, never above the document.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use markise_engine::load::{Loaded, Partial, Rest};

use super::{Ask, Banner, LoadReason, MarkiseApp, SaveRequest, Then, text_fp};

/// A progressive open whose whole document has not arrived yet.
pub(super) struct Progressive {
    /// The document showing the first part (`None`: not shown — the open found unsaved edits
    /// in the current document; the whole document then goes through the usual question).
    pub shown: Option<u64>,
    /// Length of the part (bytes of the file's decoded text).
    pub cut: usize,
    pub rest: Rest,
    pub path: PathBuf,
    pub reason: LoadReason,
    /// When the open was requested (timing log).
    pub started: Instant,
    /// A save requested meanwhile: runs once the whole document is in.
    pub save: Option<SaveRequest>,
    /// A re-parse with new parse options requested meanwhile (its label).
    pub reparse: Option<&'static str>,
}

impl MarkiseApp {
    /// The open document shows only the first part of its file (the rest is loading, or
    /// failed to load).
    pub fn is_partial(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.partial)
    }

    /// A save is waiting for the rest of the file (status bar, tests).
    pub fn save_waits_for_rest(&self) -> bool {
        self.progressive.as_ref().is_some_and(|p| p.save.is_some())
    }

    /// The first part of a progressive open arrived (the load request is done; the whole
    /// document follows through `part.rest`).
    pub(super) fn partial_arrived(&mut self, part: Partial, reason: LoadReason, started: Instant) {
        let Partial { loaded, cut, rest } = part;
        let t = loaded.timings;
        self.timing.log(&format!(
            "partial path={} bytes={} cut={cut} items={} read={:.1}ms parse={:.1}ms model={:.1}ms wall={:.1}ms",
            loaded.path.display(),
            loaded.bytes,
            loaded.model.items.len(),
            t.read.as_secs_f64() * 1e3,
            t.parse.as_secs_f64() * 1e3,
            t.model.as_secs_f64() * 1e3,
            started.elapsed().as_secs_f64() * 1e3
        ));
        let path = loaded.path.clone();
        // an open over unsaved edits asks first (with the whole document, as a normal open)
        let show = match reason {
            LoadReason::Open => !self.is_dirty(),
            LoadReason::OpenDiscarding => true,
            LoadReason::Reload | LoadReason::AcceptReload => false,
        };
        let mut p = Progressive {
            shown: None,
            cut,
            rest,
            path,
            reason,
            started,
            save: None,
            reparse: None,
        };
        if show {
            // replaces the current document (and with it its own continuation, if any)
            self.install_partial(loaded);
            p.shown = self.doc.as_ref().map(|d| d.id);
            self.progressive = Some(p);
        } else {
            // the current document stays, its own continuation too (issue #126 review: a
            // partial document with unsaved edits must still complete)
            self.progressive_waiting = Some(p);
        }
    }

    /// Takes the whole document of a progressive open when it has arrived.
    pub(super) fn poll_progressive(&mut self) {
        let mut any = false;
        if let Some(r) = self.progressive.as_ref().and_then(|p| p.rest.try_recv()) {
            let p = self.progressive.take().expect("progressive");
            any = true;
            match (r, p.shown) {
                (Ok(full), Some(id)) => self.complete(p, full, id),
                (Err(e), _) => {
                    // the part stays; it can never be saved
                    if p.shown.is_some() && self.doc.as_ref().map(|d| d.id) == p.shown {
                        self.banner = Some(Banner::Error(format!(
                            "The rest of the file could not be loaded ({e}); only the first part is shown and saving is disabled."
                        )));
                    }
                }
                (Ok(_), None) => {}
            }
        }
        if let Some(r) = self.progressive_waiting.as_ref().and_then(|p| p.rest.try_recv()) {
            let p = self.progressive_waiting.take().expect("waiting");
            any = true;
            match r {
                Err(e) => {
                    self.side.forget(&p.path);
                    self.banner = Some(Banner::Error(e));
                }
                Ok(mut full) => {
                    // (renamed meanwhile: `p.path` followed the file)
                    full.path = p.path.clone();
                    self.log_loaded(&full, p.started);
                    if p.reason == LoadReason::Open && self.is_dirty() {
                        // installing it (Discard, or after a covering save) replaces the
                        // current document and only then drops its continuation
                        self.set_ask(Ask::Unsaved(Then::Install(Box::new(full))));
                    } else {
                        self.install(full);
                    }
                }
            }
        }
        if any && std::mem::take(&mut self.reload_pending) {
            self.request_reload();
        }
    }

    /// Installs the whole document `full` in place of the part document `id` shows.
    fn complete(&mut self, p: Progressive, full: Loaded, id: u64) {
        let Some(d) = self.doc.as_mut().filter(|d| d.id == id) else {
            return;
        };
        let wall = p.started.elapsed();
        // the identity of the file's text as read (the saved state's text): known from the
        // whole file's version when the part was installed, except for a lossily decoded
        // file (hashed now, as a normal open does); a recovered draft has none
        let saved_fp = (d.saved_fp.is_none() && full.lossy && !d.untitled).then(|| text_fp(full.doc.text()));
        let from = d.viewer.ed.session.revision().counter;
        let doc: Arc<dyn markise_doc::FormatDoc> = full.doc;
        let t = full.timings;
        match d.viewer.continue_with(doc, full.model, p.cut) {
            Ok(c) => {
                self.timing.log(&format!(
                    "loaded-rest path={} bytes={} parse={:.1}ms model={:.1}ms edited={} merge={:.1}ms install={:.1}ms wall={:.1}ms",
                    p.path.display(),
                    full.bytes,
                    t.parse.as_secs_f64() * 1e3,
                    t.model.as_secs_f64() * 1e3,
                    c.edited,
                    c.merge.as_secs_f64() * 1e3,
                    c.total.as_secs_f64() * 1e3,
                    wall.as_secs_f64() * 1e3
                ));
            }
            Err(e) => {
                self.banner = Some(Banner::Error(format!(
                    "The rest of the file could not be added ({e}); only the first part is shown and saving is disabled."
                )));
                return;
            }
        }
        d.partial = false;
        d.viewer.partial = false;
        if saved_fp.is_some() {
            d.saved_fp = saved_fp;
        }
        d.fp_cache.set(None);
        let to = d.viewer.ed.session.revision().counter;
        self.attach_continued(id, from, to);
        if let Some(label) = p.reparse {
            self.reparse_document(label);
        }
        if let Some(req) = p.save {
            self.start_save(req);
        }
    }

    /// A save request while the document is partial: kept (the latest wins, a continuation
    /// such as Close is not forgotten) and run when the whole document is in. `false`: the
    /// document is not partial-and-loading.
    pub(super) fn defer_save(&mut self, req: SaveRequest) -> Result<(), SaveRequest> {
        let id = self.doc.as_ref().filter(|d| d.partial).map(|d| d.id);
        match self.progressive.as_mut() {
            Some(p) if id.is_some() && p.shown == id => {
                let then = match (&req.then, p.save.take()) {
                    (Then::Nothing, Some(q)) => q.then,
                    _ => req.then,
                };
                p.save = Some(SaveRequest {
                    expect: req.expect,
                    target: req.target,
                    then,
                });
                Ok(())
            }
            _ => Err(req),
        }
    }

    /// A re-parse with new options while the document is partial waits for the whole one.
    pub(super) fn defer_reparse(&mut self, label: &'static str) -> bool {
        let id = self.doc.as_ref().filter(|d| d.partial).map(|d| d.id);
        match self.progressive.as_mut() {
            Some(p) if id.is_some() && p.shown == id => {
                p.reparse = Some(label);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use egui::Context;
    use markise_engine::load::{ProgressiveOpts, Stage, StageHook};

    use super::super::*;

    /// Holds the rest thread before it parses until released; records the rest's stages.
    #[derive(Default)]
    struct Gate {
        held: Mutex<bool>,
        cv: Condvar,
        rest_stages: Mutex<Vec<Stage>>,
    }

    impl Gate {
        fn hook(self: &Arc<Self>) -> StageHook {
            let g = self.clone();
            Arc::new(move |st, _| {
                if std::thread::current().name() != Some("markise-load-rest") {
                    return;
                }
                g.rest_stages.lock().unwrap().push(st);
                let mut held = g.held.lock().unwrap();
                while *held && st == Stage::Parse {
                    held = g.cv.wait(held).unwrap();
                }
            })
        }
        fn hold(&self) {
            *self.held.lock().unwrap() = true;
        }
        fn release(&self) {
            *self.held.lock().unwrap() = false;
            self.cv.notify_all();
        }
    }

    fn app_with(gate: &Arc<Gate>) -> MarkiseApp {
        let mut app = MarkiseApp::new(AppOptions {
            progressive: Some(ProgressiveOpts {
                min_bytes: 16 << 10,
                prefix_bytes: 4 << 10,
            }),
            load_hook: Some(gate.hook()),
            ..Default::default()
        });
        app.wake = Some(Arc::new(|| {}));
        app.workflow.started = true;
        app
    }

    /// Runs the polls a frame runs (without drawing) until `f` holds.
    fn pump(app: &mut MarkiseApp, what: &str, f: impl Fn(&MarkiseApp) -> bool) {
        let start = Instant::now();
        let ctx = Context::default();
        while !f(app) {
            assert!(start.elapsed() < Duration::from_secs(20), "waited too long for: {what}");
            app.poll_loading();
            app.poll_progressive();
            app.poll_save();
            app.workflow_poll(&ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn big_file(dir: &Path, name: &str) -> (PathBuf, String) {
        let p = dir.join(name);
        let text = markise_corpus::generate(64 << 10, 11);
        std::fs::write(&p, &text).unwrap();
        (std::fs::canonicalize(&p).unwrap(), text)
    }

    fn open_partial(app: &mut MarkiseApp, p: &Path) {
        app.open(p.to_path_buf());
        pump(app, "the first part", |a| a.is_partial());
        assert!(app.is_loading());
        let shown = app.document_text().unwrap();
        assert!(shown.len() < 16 << 10, "only the first part: {}", shown.len());
    }

    fn type_at(app: &mut MarkiseApp, at: usize, s: &str) {
        let d = app.doc.as_mut().unwrap();
        d.viewer.ed.set_caret(markise_doc::DocPos::new(at), false);
        d.viewer.ed.edit(|s2| s2.insert_text(s)).unwrap();
    }

    #[test]
    fn saves_backups_and_autosave_wait_for_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let drafts = tempfile::tempdir().unwrap();
        let (p, text) = big_file(dir.path(), "big.md");
        let gate = Arc::new(Gate::default());
        gate.hold();
        let mut app = app_with(&gate);
        app.workflow.worker =
            Some(markise_engine::recovery::Worker::new(drafts.path().to_owned(), Arc::new(|| {})).unwrap());
        let mut prefs = app.themes.file_settings();
        prefs.autosave = true;
        prefs.backups = true;
        app.themes.set_file_settings(prefs);
        open_partial(&mut app, &p);
        type_at(&mut app, 0, "EDIT ");
        assert!(app.is_dirty());
        // an explicit save, autosave and a draft backup are all due
        app.save();
        assert!(app.save_waits_for_rest());
        assert!(app.saving.is_none());
        app.workflow.save_at = Instant::now() - Duration::from_secs(3600);
        app.workflow.backup_at = Instant::now() - Duration::from_secs(3600);
        app.workflow_poll(&Context::default());
        assert!(app.saving.is_none(), "autosave never writes the first part");
        assert!(app.workflow.pending.is_none(), "no backup of the first part");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text, "nothing written yet");
        gate.release();
        pump(&mut app, "loaded and saved", |a| !a.is_loading() && a.saving.is_none());
        let want = format!("EDIT {text}");
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            want,
            "the whole edited file was saved"
        );
        assert!(!app.is_dirty());
        assert!(!app.is_partial());
        assert_eq!(app.document_text().unwrap(), want);
        // undo reaches the file as opened
        let d = app.doc.as_mut().unwrap();
        while d.viewer.ed.edit(|s| s.undo()) {}
        assert_eq!(app.document_text().unwrap(), text);
    }

    #[test]
    fn external_change_while_loading_is_checked_when_complete() {
        let dir = tempfile::tempdir().unwrap();
        let (p, _) = big_file(dir.path(), "big.md");
        let gate = Arc::new(Gate::default());
        // clean document: the new content is taken
        gate.hold();
        let mut app = app_with(&gate);
        open_partial(&mut app, &p);
        let changed = format!("# Changed\n\n{}", markise_corpus::generate(40 << 10, 12));
        std::fs::write(&p, &changed).unwrap();
        app.request_reload(); // the watcher's event
        assert!(app.reload_pending, "checked after the load");
        gate.release();
        pump(&mut app, "reloaded", |a| {
            !a.is_loading() && a.document_text().as_deref() == Some(changed.as_str())
        });
        assert!(!app.is_dirty());
        // edited document: a conflict, the edits and the whole old file kept
        gate.hold();
        let (q, text) = big_file(dir.path(), "other.md");
        app.open(q.clone());
        pump(&mut app, "the first part", |a| a.is_partial());
        type_at(&mut app, 0, "MINE ");
        std::fs::write(&q, "external").unwrap();
        app.request_reload();
        gate.release();
        pump(&mut app, "conflict", |a| {
            !a.is_loading() && !a.reload_pending && matches!(a.banner, Some(Banner::Conflict(_)))
        });
        assert_eq!(app.document_text().unwrap(), format!("MINE {text}"));
        assert!(app.is_dirty());
    }

    #[test]
    fn opening_another_file_while_loading() {
        let dir = tempfile::tempdir().unwrap();
        let (a, text) = big_file(dir.path(), "a.md");
        let gate = Arc::new(Gate::default());
        gate.hold();
        let mut app = app_with(&gate);
        open_partial(&mut app, &a);
        // a file that cannot be opened: the first file stays and completes
        app.open(dir.path().join("missing.md"));
        pump(&mut app, "the failed open", |a| a.load.is_none());
        assert!(matches!(app.banner, Some(Banner::Error(_))));
        assert!(app.is_partial());
        gate.release();
        pump(&mut app, "complete", |a| !a.is_loading());
        assert_eq!(app.document_text().unwrap(), text);
        // another file opened while the first loads replaces it; the rest is dropped
        gate.hold();
        gate.rest_stages.lock().unwrap().clear();
        let (b, _) = big_file(dir.path(), "b.md");
        app.open(b.clone());
        pump(&mut app, "b partial", |a| a.is_partial());
        let small = dir.path().join("small.md");
        std::fs::write(&small, "# Small\n").unwrap();
        app.open(small.clone());
        pump(&mut app, "small open", |a| !a.is_loading());
        assert_eq!(app.document_text().unwrap(), "# Small\n");
        assert!(!app.is_partial());
        // (the rest thread was held at its parse stage; released, it finds itself cancelled)
        gate.release();
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !gate.rest_stages.lock().unwrap().contains(&Stage::Model),
            "the dropped rest stops before building its model"
        );
        app.poll_progressive();
        assert_eq!(app.document_text().unwrap(), "# Small\n");
    }

    #[test]
    fn close_while_loading() {
        let dir = tempfile::tempdir().unwrap();
        let (p, text) = big_file(dir.path(), "big.md");
        let gate = Arc::new(Gate::default());
        gate.hold();
        let mut app = app_with(&gate);
        open_partial(&mut app, &p);
        assert!(app.request_close(), "nothing to save: closes at once");
        // with edits and Save on Close: the save waits for the rest, then the window closes
        let mut app = app_with(&gate);
        let mut prefs = app.themes.file_settings();
        prefs.save_on_close = true;
        app.themes.set_file_settings(prefs);
        open_partial(&mut app, &p);
        type_at(&mut app, 0, "X");
        assert!(!app.request_close());
        assert!(app.save_waits_for_rest());
        gate.release();
        pump(&mut app, "saved and closing", |a| a.allow_close);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("X{text}"));
    }

    #[test]
    fn recovered_drafts_and_lossy_files_open_progressively() {
        let dir = tempfile::tempdir().unwrap();
        let (p, text) = big_file(dir.path(), "draft.md");
        let gate = Arc::new(Gate::default());
        let mut app = app_with(&gate);
        app.workflow.drafts.push(p.clone());
        gate.hold();
        app.open_known_file(p.clone());
        pump(&mut app, "the first part", |a| a.is_partial());
        assert!(
            app.is_untitled() && app.is_dirty(),
            "a recovered draft is an unsaved copy"
        );
        gate.release();
        pump(&mut app, "complete", |a| !a.is_loading());
        assert_eq!(app.document_text().unwrap(), text);
        assert!(app.is_dirty());
        // a lossily decoded file: saving stays disabled after it is complete
        let lossy = dir.path().join("lossy.md");
        let mut bytes = text.clone().into_bytes();
        bytes.extend(b"\n\nbad \xff byte\n");
        std::fs::write(&lossy, &bytes).unwrap();
        app.doc = None;
        gate.hold();
        app.open(lossy.clone());
        pump(&mut app, "the first part", |a| a.is_partial());
        app.save();
        gate.release();
        pump(&mut app, "complete", |a| !a.is_loading() && a.saving.is_none());
        assert!(app.doc.as_ref().unwrap().lossy);
        assert_eq!(std::fs::read(&lossy).unwrap(), bytes, "never written");
        assert_eq!(app.document_text().unwrap(), String::from_utf8_lossy(&bytes));
        assert!(matches!(app.banner, Some(Banner::Error(_))), "the save was refused");
    }

    /// Review of #152, P1: a second large open that cannot be shown (the current partial
    /// document got edits before its first part arrived) must not drop the current document's
    /// continuation; it only goes when the other file actually replaces it.
    #[test]
    fn a_waiting_open_keeps_the_current_documents_rest() {
        use crate::confirm::Choice;
        let dir = tempfile::tempdir().unwrap();
        let (a, text_a) = big_file(dir.path(), "a.md");
        let (b, text_b) = big_file(dir.path(), "b.md");
        let gate = Arc::new(Gate::default());
        gate.hold();
        let mut app = app_with(&gate);
        open_partial(&mut app, &a);
        // B is requested while A is clean; A is edited before B's first part is handled
        app.open(b.clone());
        type_at(&mut app, 0, "MINE ");
        pump(&mut app, "b's first part, not shown", |a| {
            a.progressive_waiting.is_some()
        });
        assert!(app.is_partial() && app.progressive.is_some(), "A keeps its rest");
        gate.release();
        pump(&mut app, "both complete", |a| {
            a.progressive.is_none() && a.progressive_waiting.is_none()
        });
        assert!(!app.is_partial(), "A is complete");
        assert_eq!(app.document_text().unwrap(), format!("MINE {text_a}"));
        assert!(matches!(app.ask, Some(Ask::Unsaved(Then::Install(_)))), "B asks first");
        // Cancel: A stays and can be saved
        app.answer_question(Choice::Cancel);
        app.save();
        pump(&mut app, "saved", |a| a.saving.is_none() && !a.is_dirty());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), format!("MINE {text_a}"));
        // and the other way: B replaces an edited partial A after Don't Save
        gate.hold();
        let (c, _) = big_file(dir.path(), "c.md");
        app.open(c.clone());
        pump(&mut app, "c partial", |a| a.is_partial());
        app.open(b.clone());
        type_at(&mut app, 0, "X");
        pump(&mut app, "b waiting", |a| a.progressive_waiting.is_some());
        gate.release();
        pump(&mut app, "question", |a| {
            a.progressive_waiting.is_none() && a.ask.is_some()
        });
        app.answer_question(Choice::DontSave);
        assert_eq!(app.document_text().unwrap(), text_b);
        assert!(!app.is_partial() && app.progressive.is_none());
    }

    /// Review of #152 round 2: a waiting open follows a rename of its file or of its folder,
    /// before its whole document arrives and while its question is open.
    #[test]
    fn a_waiting_open_follows_renames() {
        use crate::confirm::Choice;
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = big_file(dir.path(), "a.md");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let (b, text_b) = big_file(&sub, "b.md");
        let gate = Arc::new(Gate::default());
        gate.hold();
        let mut app = app_with(&gate);
        open_partial(&mut app, &a);
        app.open(b.clone());
        type_at(&mut app, 0, "MINE ");
        pump(&mut app, "b waiting", |a| a.progressive_waiting.is_some());
        // the file is renamed while its rest loads …
        let c = sub.join("c.md");
        std::fs::rename(&b, &c).unwrap();
        app.renamed(&b, &c);
        gate.release();
        pump(&mut app, "question", |a| {
            a.progressive_waiting.is_none() && a.ask.is_some()
        });
        // … and its folder while the question is open
        let moved = dir.path().join("moved");
        std::fs::rename(&sub, &moved).unwrap();
        app.renamed(&sub, &moved);
        app.answer_question(Choice::DontSave);
        let want = moved.join("c.md");
        assert_eq!(app.document_path(), Some(want.as_path()));
        assert_eq!(app.document_text().unwrap(), text_b);
        type_at(&mut app, 0, "Z");
        app.save();
        pump(&mut app, "saved", |a| a.saving.is_none() && !a.is_dirty());
        assert_eq!(std::fs::read_to_string(&want).unwrap(), format!("Z{text_b}"));
        assert!(!b.exists() && !c.exists(), "nothing written at the old paths");
    }

    /// Review of #152, P2: a clean-copy question asked about one document never writes
    /// another (the first part of a file opened meanwhile); every write path refuses a partial
    /// document.
    #[test]
    fn writes_never_take_the_first_part() {
        use crate::confirm::Choice;
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.md");
        std::fs::write(&small, "Text {++added++} here.\n").unwrap();
        let (big, _) = big_file(dir.path(), "big.md");
        let gate = Arc::new(Gate::default());
        gate.hold();
        let mut app = app_with(&gate);
        app.open(small.clone());
        pump(&mut app, "small", |a| a.doc.is_some() && !a.is_loading());
        app.save_copy_without_comments();
        assert!(matches!(app.ask, Some(Ask::CleanCopy { .. })));
        // a large file is opened before the question is answered: the question goes
        app.open(big.clone());
        pump(&mut app, "big partial", |a| a.is_partial());
        assert!(app.ask.is_none(), "the question went with its document");
        let copy = dir.path().join("copy.md");
        app.queue_save_dialog_answer(Some(copy.clone()));
        app.answer_question(Choice::AcceptAll);
        app.write_clean_copy(false);
        assert_eq!(app.files.save_answers.len(), 1, "no copy of the first part was started");
        // exports refuse it too
        let req = markise_engine::export::ExportRequest::new(
            markise_engine::export::ExportKind::Html,
            app.document_text().unwrap(),
            Some(big.clone()),
            dir.path().join("out.html"),
            app.theme().clone(),
        );
        app.start_export(req);
        assert!(!app.export.is_running());
        gate.release();
        pump(&mut app, "complete", |a| !a.is_loading());
        assert!(!copy.exists() && !dir.path().join("out.html").exists());
    }

    #[test]
    fn small_files_open_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("small.md");
        std::fs::write(&p, "# Title\n\n".repeat(500)).unwrap();
        let gate = Arc::new(Gate::default());
        let mut app = app_with(&gate);
        app.open(p);
        pump(&mut app, "open", |a| a.doc.is_some());
        assert!(!app.is_partial() && !app.is_loading());
        assert!(gate.rest_stages.lock().unwrap().is_empty(), "no rest thread");
    }
}
