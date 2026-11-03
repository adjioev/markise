//! Background draft backups, autosave deadlines and persisted recent paths.
use super::*;
use markise_engine::recovery::{Event, Request, Worker};

pub(super) struct Workflow {
    pub worker: Option<Worker>,
    pub(super) started: bool,
    pub recent: Vec<PathBuf>,
    pub drafts: Vec<PathBuf>,
    pub show_recovery: bool,
    pub recovered: Option<PathBuf>,
    pub(super) backup_at: Instant,
    pub(super) save_at: Instant,
    backed: Option<(u64, u64)>,
    pub(super) pending: Option<(u64, u64)>,
}
impl Default for Workflow {
    fn default() -> Self {
        Self {
            worker: None,
            started: false,
            recent: Vec::new(),
            drafts: Vec::new(),
            show_recovery: false,
            recovered: None,
            backup_at: Instant::now(),
            save_at: Instant::now(),
            backed: None,
            pending: None,
        }
    }
}
/// A backup of the document as the user sees it: its text with the literal-hint escapes and a
/// comment draft open in a card applied (the draft's patches join the escape patches, so the
/// open document and its undo history are not touched; Codex review of #158).
fn backup_request(id: u64, name: String, viewer: &crate::viewer::DocViewer) -> Request {
    let mut req = Request::backup(id, name, viewer.ed.session.doc());
    if let (Request::Backup { literal, .. }, Some(draft)) = (&mut req, viewer.card_draft_patches()) {
        // escapes inside the draft's range are replaced by it
        literal.retain(|p| {
            draft
                .iter()
                .all(|d| p.range.end <= d.range.start || p.range.start >= d.range.end)
        });
        literal.extend(draft);
        literal.sort_by_key(|p| (p.range.start, p.range.end));
    }
    req
}

impl MarkiseApp {
    pub(super) fn remember_path(&mut self, path: &Path) {
        if !self.themes.file_settings().record_recent {
            return;
        }
        self.workflow.recent.retain(|p| p != path);
        self.workflow.recent.insert(0, path.to_owned());
        self.workflow.recent.truncate(30);
        if let Some(w) = &self.workflow.worker {
            let _ = w.tx.send(Request::Recent(path.to_owned()));
        }
    }
    pub(super) fn clear_recent_paths(&mut self) {
        self.workflow.recent.clear();
        for p in self.side.recent().to_vec() {
            self.side.forget(&p);
        }
        if let Some(w) = &self.workflow.worker {
            let _ = w.tx.send(Request::ClearRecent);
        }
    }
    /// Called only when a replacement really installs, a save covers all edits, or Close
    /// was explicitly accepted. A failed Open must keep the old document's backup.
    pub(super) fn forget_backup(&mut self) {
        if let Some(w) = &self.workflow.worker {
            if let Some(d) = &self.doc {
                let _ = w.tx.send(Request::Remove(d.id));
            }
            if let Some(p) = self.workflow.recovered.take() {
                let _ = w.tx.send(Request::RemoveRecovered(p.clone()));
                self.workflow.drafts.retain(|v| v != &p);
            }
        }
        self.workflow.backed = None;
    }
    pub(super) fn workflow_poll(&mut self, ctx: &Context) {
        if !self.workflow.started {
            self.workflow.started = true;
            if let Some(p) = self
                .themes
                .settings_file()
                .and_then(|p| p.parent().map(|p| p.join("recovery")))
            {
                match Worker::new(p, self.wake_handle()) {
                    Ok(w) => self.workflow.worker = Some(w),
                    Err(e) => self.banner = Some(Banner::Error(format!("Cannot start recovery: {e}"))),
                }
            }
        }
        let events: Vec<_> = self
            .workflow
            .worker
            .as_ref()
            .map(|w| w.rx.try_iter().collect())
            .unwrap_or_default();
        for e in events {
            match e {
                Event::Ready {
                    drafts,
                    recent,
                    recent_files,
                } => {
                    self.workflow.drafts = drafts;
                    self.workflow.show_recovery = !self.workflow.drafts.is_empty();
                    if self.themes.file_settings().record_recent {
                        let current = self.side.recent().to_vec();
                        for p in recent_files.into_iter().rev() {
                            self.side.opened(&p);
                        }
                        for p in current.into_iter().rev() {
                            self.side.opened(&p);
                        }
                        if let Some(w) = &self.workflow.worker {
                            for p in self.workflow.recent.iter().rev() {
                                let _ = w.tx.send(Request::Recent(p.clone()));
                            }
                        }
                    }
                    for p in recent {
                        if !self.workflow.recent.contains(&p) {
                            self.workflow.recent.push(p);
                        }
                    }
                }
                Event::BackedUp(id) => {
                    if self.workflow.pending.is_some_and(|p| p.0 == id) {
                        let key = self.workflow.pending.take();
                        if self.doc.as_ref().is_some_and(|d| d.id == id) {
                            self.workflow.backed = key;
                        }
                    }
                }
                Event::Error(e) => {
                    self.workflow.pending = None;
                    self.banner = Some(Banner::Error(e));
                }
            }
        }
        let prefs = self.themes.file_settings();
        // (a progressively opened document holding only the first part of its file is neither
        // backed up nor autosaved: the timers run on once the whole document is in, issue #126)
        if self.is_partial() {
            return;
        }
        if self.doc.as_ref().is_some_and(|d| d.dirty() && !d.lossy) {
            let now = Instant::now();
            let backup_delay = Duration::from_secs(30);
            if prefs.backups
                && let Some(worker) = &self.workflow.worker
            {
                if now.duration_since(self.workflow.backup_at) >= backup_delay && self.workflow.pending.is_none() {
                    let d = self.doc.as_ref().unwrap();
                    // (a comment draft open in a card changes the key as it is typed)
                    let key = (d.id, d.viewer.ed.session.revision().counter ^ d.viewer.card_draft_key());
                    if self.workflow.backed != Some(key) {
                        // a text snapshot, not a copy of the parsed document (issue #29)
                        let req = backup_request(d.id, d.display_name(), &d.viewer);
                        if worker.tx.send(req).is_ok() {
                            self.workflow.pending = Some(key);
                        }
                    }
                    self.workflow.backup_at = now;
                }
                ctx.request_repaint_after(backup_delay.saturating_sub(now.duration_since(self.workflow.backup_at)));
            }
            let interval = Duration::from_secs(prefs.autosave_minutes.clamp(1, 1440) as u64 * 60);
            if prefs.autosave && self.doc.as_ref().is_some_and(|d| !d.untitled && !d.deleted) {
                if now.duration_since(self.workflow.save_at) >= interval {
                    // Conflicts require a decision; never continually retry or replace that banner.
                    if self.saving.is_none()
                        && self.ask.is_none()
                        && !matches!(self.banner, Some(Banner::Conflict(_) | Banner::SaveConflict { .. }))
                    {
                        self.autosave_now();
                    }
                    self.workflow.save_at = now;
                }
                ctx.request_repaint_after(interval.saturating_sub(now.duration_since(self.workflow.save_at)));
            }
        } else {
            if self.workflow.backed.is_some() {
                self.forget_backup();
            }
            self.workflow.backup_at = Instant::now();
            self.workflow.save_at = Instant::now();
        }
    }
    pub(super) fn recovery_window(&mut self, ctx: &Context) {
        if !self.workflow.show_recovery {
            return;
        }
        let mut open = true;
        let mut restore = None;
        egui::Window::new("Recover unsaved drafts")
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .default_width(560.0)
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                if !self.workflow.drafts.is_empty() {
                    ui.label("Drafts from a previous session are available. Each opens as an unsaved copy.");
                }
                if self.workflow.drafts.is_empty() {
                    ui.label("No drafts to recover.");
                }
                for p in &self.workflow.drafts {
                    ui.horizontal(|ui| {
                        ui.label(p.file_name().unwrap_or_default().to_string_lossy());
                        if ui.button("Recover").clicked() {
                            restore = Some(p.clone());
                        }
                    });
                }
                ui.label("You can return here from the File menu: Recover Unsaved Drafts.");
            });
        self.workflow.show_recovery = open;
        if let Some(p) = restore {
            self.workflow.show_recovery = false;
            self.open_known_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(app: &mut MarkiseApp, f: impl Fn(&MarkiseApp) -> bool) {
        let start = Instant::now();
        let ctx = Context::default();
        while !f(app) {
            assert!(start.elapsed() < Duration::from_secs(5));
            app.workflow_poll(&ctx);
            app.poll_save();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn save_on_close_cancel_keeps_draft_then_success_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("saved.md");
        let mut app = MarkiseApp::new(AppOptions::default());
        app.workflow.started = true;
        app.wake = Some(Arc::new(|| {}));
        app.new_document_now();
        app.doc
            .as_mut()
            .unwrap()
            .viewer
            .ed
            .session
            .insert_text("my draft")
            .unwrap();
        let mut prefs = app.themes.file_settings();
        prefs.save_on_close = true;
        app.themes.set_file_settings(prefs);
        app.queue_save_dialog_answer(None);
        assert!(!app.request_close());
        assert!(app.is_dirty());
        assert!(!app.allow_close);
        app.queue_save_dialog_answer(Some(target.clone()));
        assert!(!app.request_close());
        wait(&mut app, |a| a.allow_close);
        assert_eq!(std::fs::read_to_string(target).unwrap(), "my draft");
        assert!(!app.is_dirty());
    }

    #[test]
    fn recovery_is_dirty_copy_and_cleanup_waits_for_covering_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.md");
        std::fs::write(&path, "unsaved draft").unwrap();
        let mut app = MarkiseApp::new(AppOptions::default());
        app.workflow.started = true;
        app.workflow.drafts.push(path.clone());
        let loaded = markise_engine::load::load_file(&path, Default::default()).unwrap();
        app.install_doc(loaded, false);
        assert!(app.is_dirty());
        assert!(app.doc.as_ref().unwrap().untitled);
        assert!(app.document_path().is_none());
        assert_eq!(app.document_text().unwrap(), "unsaved draft");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "unsaved draft");
    }
    #[test]
    fn autosave_refuses_external_changes_and_keeps_unsaved_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "original").unwrap();
        let mut app = MarkiseApp::new(AppOptions::default());
        app.wake = Some(Arc::new(|| {}));
        app.workflow.started = true;
        app.install_doc(
            markise_engine::load::load_file(&path, Default::default()).unwrap(),
            false,
        );
        app.doc
            .as_mut()
            .unwrap()
            .viewer
            .ed
            .session
            .insert_text("mine ")
            .unwrap();
        let mut prefs = app.themes.file_settings();
        prefs.autosave = true;
        app.themes.set_file_settings(prefs);
        std::fs::write(&path, "external").unwrap();
        app.workflow.save_at = Instant::now() - Duration::from_secs(301);
        app.workflow_poll(&Context::default());
        wait(&mut app, |a| a.saving.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        assert!(app.is_dirty());
        assert!(matches!(app.banner, Some(Banner::SaveConflict { .. })));
    }
    #[test]
    fn pending_backup_cannot_resurrect_a_discarded_document() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = MarkiseApp::new(AppOptions::default());
        app.workflow.started = true;
        app.workflow.worker = Some(Worker::new(dir.path().to_owned(), Arc::new(|| {})).unwrap());
        app.new_document_now();
        app.doc
            .as_mut()
            .unwrap()
            .viewer
            .ed
            .session
            .insert_text("draft")
            .unwrap();
        app.workflow.backup_at = Instant::now() - Duration::from_secs(31);
        app.workflow_poll(&Context::default());
        app.new_document_now(); // install follows the owner's explicit Discard decision
        wait(&mut app, |a| a.workflow.pending.is_none());
        // A worker barrier: all previous queued writes/removals are now complete.
        let w = app.workflow.worker.as_ref().unwrap();
        w.tx.send(Request::backup(
            999,
            "barrier".into(),
            &markise_md::Document::parse("barrier", Default::default()),
        ))
        .unwrap();
        loop {
            if matches!(w.rx.recv_timeout(Duration::from_secs(5)).unwrap(), Event::BackedUp(999)) {
                break;
            }
        }
        let drafts: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .flat_map(|e| std::fs::read_dir(e.path()).unwrap().flatten())
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .collect();
        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].file_name().to_string_lossy().contains("barrier"));
    }

    /// Codex review of #158: a comment draft open in a card makes the document dirty and is
    /// in the crash-recovery backup (the open document and its history stay untouched), and
    /// a changed draft is backed up again.
    #[test]
    fn backup_includes_an_open_card_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = MarkiseApp::new(AppOptions::default());
        app.workflow.started = true;
        app.workflow.worker = Some(Worker::new(dir.path().to_owned(), Arc::new(|| {})).unwrap());
        app.install_untitled("Note {>>old<<} end.\n".into(), "note.md");
        assert!(!app.is_dirty());
        let backup = |app: &mut MarkiseApp| -> String {
            app.workflow.backup_at = Instant::now() - Duration::from_secs(31);
            app.workflow_poll(&Context::default());
            assert!(app.workflow.pending.is_some(), "a backup was requested");
            wait(app, |a| a.workflow.pending.is_none());
            let files: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .filter(|e| e.path().is_dir())
                .flat_map(|e| std::fs::read_dir(e.path()).unwrap().flatten())
                .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                .collect();
            assert_eq!(files.len(), 1);
            std::fs::read_to_string(files[0].path()).unwrap()
        };
        let v = &mut app.doc.as_mut().unwrap().viewer;
        v.edit_card_for_test(5, "new text");
        assert!(app.is_dirty(), "an open draft is unsaved");
        assert_eq!(backup(&mut app), "Note {>>new text<<} end.\n");
        let v = &mut app.doc.as_mut().unwrap().viewer;
        assert_eq!(
            v.ed.session.text(),
            "Note {>>old<<} end.\n",
            "the document is untouched"
        );
        assert!(!v.ed.session.history().can_undo());
        v.edit_card_for_test(5, "newer");
        assert_eq!(backup(&mut app), "Note {>>newer<<} end.\n");
    }
}
