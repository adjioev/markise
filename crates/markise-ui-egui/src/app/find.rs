//! In-document search UI; source chunks are bounded and never share the editable document.
use super::*;
use markise_doc::Selection;
use markise_engine::search::{Completion, Key, Query, Replacement, Results, SNAPSHOT_CHUNK, SearchWorker};

#[derive(Default)]
pub(super) struct FindState {
    pub open: bool,
    replace: bool,
    focus: bool,
    query: Query,
    replacement: String,
    generation: u64,
    key: Option<Key>,
    worker: Option<SearchWorker>,
    chunks: Vec<String>,
    copied: usize,
    submitted: bool,
    results: Option<Results>,
    active: Option<usize>,
    operation: Option<Replacement>,
    /// The search looks at everything the source holds: Source and Side by Side show it all, the
    /// rendered view leaves out what it hides (ADR 0018).
    all_of_source: bool,
    navigation: Option<bool>,
    query_id: Option<egui::Id>,
    /// Only the exact post-Replace-All revision can be undone by the panel button.
    undo: Option<(u64, u64)>,
    error: Option<String>,
    focus_before: Option<egui::Id>,
}
impl MarkiseApp {
    /// Opens the shared in-document find bar. Matching is against document source in all modes.
    pub fn open_find(&mut self, replace: bool) {
        if self.doc.is_none() {
            return;
        }
        if !self.find.open {
            self.find.focus_before = self.ctx.as_ref().and_then(|c| c.memory(|m| m.focused()));
        }
        self.find.open = true;
        self.find.replace = replace;
        self.find.focus = true;
    }
    pub fn set_find_query(&mut self, query: Query) {
        self.find.query = query;
        self.find_invalidate();
    }
    pub fn find_results(&self) -> Option<&Results> {
        self.find.results.as_ref().filter(|r| self.find_current(r.key))
    }
    pub fn find_active(&self) -> Option<usize> {
        self.find.active
    }
    fn find_current(&self, key: Key) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.id == key.document && d.viewer.ed.session.revision().counter == key.revision)
            && self.find.generation == key.generation
    }
    fn find_invalidate(&mut self) {
        self.find.generation += 1;
        self.find.key = None;
        self.find.results = None;
        self.find.active = None;
        self.find.chunks.clear();
        self.find.copied = 0;
        self.find.submitted = false;
        self.find.error = None;
        self.find.operation = None;
        if let Some(w) = &self.find.worker {
            w.cancel(self.find.generation);
        }
    }
    pub fn close_find(&mut self) {
        self.find.open = false;
        if self.find.operation.is_some() {
            self.find_invalidate();
        }
        if let Some(ctx) = &self.ctx {
            if let Some(id) = self.find.focus_before.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            } else if let Some(d) = &mut self.doc {
                if self.source_visible || self.side_by_side {
                    d.source_ui.request_focus();
                } else {
                    d.viewer.request_refocus();
                }
            }
        }
    }
    pub fn find_next(&mut self, backwards: bool) {
        if !self.find.open {
            self.open_find(false);
        }
        let Some(results) = self.find_results() else {
            self.find.navigation = Some(backwards);
            return;
        };
        let d = self.doc.as_ref().unwrap();
        let source = self.source_visible || (self.side_by_side && self.word_count_source_pane);
        let caret = if source {
            d.viewer.ed.source.as_ref().map(|p| p.sel.primary().focus.offset)
        } else {
            None
        }
        .unwrap_or_else(|| d.viewer.ed.session.selection().primary().focus.offset);
        let Some(i) = results.next(self.find.active, caret, backwards) else {
            return;
        };
        let range = results.matches[i].clone();
        self.find.active = Some(i);
        let d = self.doc.as_mut().unwrap();
        // Session selection does not materialise literal hints or alter document bytes.
        d.viewer
            .ed
            .session
            .set_selection(Selection::range(range.start, range.end));
        d.viewer.ed.source_selection_from_session();
        d.viewer.ed.reveal = true;
        d.viewer.ed.refresh_focus();
        // (also when the caret was on this item already)
        d.viewer.ed.view.reveal_offset(range.start);
        d.viewer.find_reveal = true;
        d.source_ui.find_reveal = true;
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
    /// Builds replacement patches in the search worker, then applies one validated transaction.
    pub(super) fn replace_find_next(&mut self) {
        if !self.find.open {
            self.open_find(true);
        }
        self.replace_find(self.find.replacement.clone(), false);
    }
    pub fn replace_find(&mut self, replacement: String, all: bool) {
        if all && self.is_partial() {
            return; // Replace All waits for the whole file (progressive open, issue #126)
        }
        // a read-only document (the clean review previews, issue #88): no replacement starts;
        // one already queued is refused by the session when it completes
        if let Some(why) = self.doc.as_ref().and_then(|d| d.viewer.ed.session.read_only()) {
            self.find.error = Some(format!("Read-only: {why}."));
            return;
        }
        if !self
            .find_results()
            .is_some_and(|r| r.error.is_none() && !r.matches.is_empty())
        {
            return;
        }
        let only = if all {
            None
        } else {
            let Some(i) = self.find.active else {
                self.find_next(false);
                self.replace_find(replacement, false);
                return;
            };
            self.find_results().map(|r| r.search_index(i))
        };
        let skip = self.find_results().map_or_else(Default::default, |r| r.dropped.clone());
        self.find_invalidate();
        self.find.operation = Some(Replacement {
            text: replacement,
            only,
            skip,
        });
    }
    pub fn undo_replace_all(&mut self) -> bool {
        let Some((id, revision)) = self.find.undo.take() else {
            return false;
        };
        let Some(d) = self.doc.as_mut() else { return false };
        if d.id != id || d.viewer.ed.session.revision().counter != revision {
            return false;
        }
        let done = d.viewer.ed.edit(|s| s.undo());
        self.find_invalidate();
        done
    }
    pub(super) fn find_update(&mut self, ctx: &Context) {
        if !self.find.open {
            self.find_publish();
            return;
        }
        let Some(d) = &self.doc else {
            self.close_find();
            return;
        };
        let identity = (d.id, d.viewer.ed.session.revision().counter);
        let all_of_source = self.source_visible || self.side_by_side;
        if all_of_source != self.find.all_of_source {
            self.find.all_of_source = all_of_source;
            self.find_invalidate();
        }
        if self.find.key.is_some_and(|k| (k.document, k.revision) != identity) {
            self.find_invalidate();
        }
        if self.find.key.is_none() {
            self.find.key = Some(Key {
                document: identity.0,
                revision: identity.1,
                generation: self.find.generation,
            });
        }
        if self.find.worker.is_none() {
            self.find.worker = Some(SearchWorker::new(self.wake_handle()));
        }
        while let Some(c) = self.find.worker.as_ref().and_then(SearchWorker::poll) {
            match c {
                Completion::Found(mut r) if self.find_current(r.key) => {
                    if !all_of_source && let Some(d) = &self.doc {
                        r.drop_hidden(d.viewer.ed.session.doc());
                    }
                    self.find.error = r.error.clone();
                    self.find.results = Some(r);
                }
                Completion::Replaced(k, patches) if self.find_current(k) => {
                    match patches {
                        Ok(patches) if !patches.is_empty() => {
                            let d = self.doc.as_mut().unwrap();
                            let all = self.find.operation.as_ref().is_some_and(|o| o.only.is_none());
                            match d.viewer.ed.edit(|s| {
                                s.apply_external(&patches, if all { "Replace All" } else { "Replace" }, false)
                            }) {
                                Ok(()) => {
                                    if all {
                                        self.find.undo = Some((d.id, d.viewer.ed.session.revision().counter));
                                    }
                                    self.find_invalidate();
                                }
                                Err(e) => self.find.error = Some(e.to_string()),
                            }
                        }
                        Ok(_) => {
                            self.find_invalidate();
                        }
                        Err(e) => {
                            self.find.operation = None;
                            self.find.error = Some(e);
                        }
                    }
                }
                _ => {}
            }
        }
        // A completed replacement invalidated the pre-edit key earlier in this frame.
        if self.find.key.is_none() {
            let d = self.doc.as_ref().unwrap();
            self.find.key = Some(Key {
                document: d.id,
                revision: d.viewer.ed.session.revision().counter,
                generation: self.find.generation,
            });
        }
        if self.find.results.is_some()
            && let Some(backwards) = self.find.navigation.take()
        {
            self.find_next(backwards);
        }
        if !self.find.submitted && !self.find.query.text.is_empty() {
            let text = self.doc.as_ref().unwrap().viewer.ed.session.text();
            // 1 MiB/frame, in independently allocated 64 KiB chunks. Concatenation runs on
            // the worker; no growing String or FormatDoc Arc can trigger a full UI copy.
            for _ in 0..16 {
                let start = self.find.copied;
                if start == text.len() {
                    break;
                }
                let mut end = (start + SNAPSHOT_CHUNK).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                self.find.chunks.push(text[start..end].to_owned());
                self.find.copied = end;
            }
            if self.find.copied == text.len() {
                self.find.worker.as_ref().unwrap().submit(
                    self.find.key.unwrap(),
                    std::mem::take(&mut self.find.chunks),
                    self.find.query.clone(),
                    self.find.operation.clone(),
                );
                self.find.submitted = true;
            } else {
                ctx.request_repaint();
            }
        }
        self.find_publish();
    }
    fn find_publish(&mut self) {
        let current = self
            .find
            .results
            .as_ref()
            .filter(|r| self.find.open && self.find_current(r.key));
        if let Some(d) = &mut self.doc {
            d.viewer.find_matches = current.map(|r| (r.key.revision, r.matches.clone(), self.find.active));
        }
    }
    pub(super) fn find_ui(&mut self, ui: &mut egui::Ui) {
        if !self.find.open {
            return;
        }
        let old = self.find.query.clone();
        let mut close = false;
        let mut next = None;
        let mut replace = None;
        let mut undo = false;
        let mut show_source = false;
        if self.find.query_id.is_some_and(|id| ui.memory(|m| m.has_focus(id))) {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)) {
                next = Some(true);
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                next = Some(false);
            }
        }
        let ready = self.find_results().is_some_and(|r| !r.matches.is_empty());
        // progressive open (issue #126): results cover the part loaded so far; Replace All
        // would leave the rest unchanged
        let partial = self.is_partial();
        let ready_all = ready && !partial;
        let all_tip = if partial {
            "Available once the whole file has loaded"
        } else {
            "Replace every match"
        };
        let status = if self.find.query.text.is_empty() {
            String::new()
        } else if self.find.error.is_some() {
            "Cannot search".into()
        } else if let Some(r) = self.find_results() {
            match (r.matches.len(), self.find.active) {
                (0, _) => "No matches".into(),
                (n, Some(i)) => format!("{} of {n}", i + 1),
                (1, None) => "1 match".into(),
                (n, None) => format!("{n} matches"),
            }
        } else if self.find.operation.is_some() {
            "Replacing…".into()
        } else {
            "Searching…".into()
        };
        let can_undo = self.find.undo.is_some_and(|(id, rev)| {
            self.doc
                .as_ref()
                .is_some_and(|d| d.id == id && d.viewer.ed.session.revision().counter == rev)
        });
        egui::Panel::top("document-find-bar")
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                let narrow = ui.available_width() < 580.0;
                ui.horizontal(|ui| {
                    let label = ui.label("Find");
                    let reserve = if narrow { 34.0 } else { 272.0 };
                    let width = (ui.available_width() - reserve).max(80.0);
                    let response = ui.add_sized(
                        [width, 24.0],
                        egui::TextEdit::singleline(&mut self.find.query.text)
                            .id_salt("document-find-query")
                            .hint_text("Find in this document"),
                    ).labelled_by(label.id);
                    self.find.query_id = Some(response.id);
                    if std::mem::take(&mut self.find.focus) {
                        response.request_focus();
                        let mut state = egui::TextEdit::load_state(ui.ctx(), response.id).unwrap_or_default();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(self.find.query.text.chars().count()),
                        )));
                        state.store(ui.ctx(), response.id);
                    }
                    if !narrow {
                        ui.add_sized([82.0, 24.0], egui::Label::new(&status));
                        if ui.add_enabled(ready, egui::Button::new("Previous")).on_hover_text("Previous match · Shift+Enter").clicked() {
                            next = Some(true);
                        }
                        if ui.add_enabled(ready, egui::Button::new("Next")).on_hover_text("Next match · Enter").clicked() {
                            next = Some(false);
                        }
                    }
                    if ui.button("×").on_hover_text("Close Find · Esc").clicked() {
                        close = true;
                    }
                });
                if narrow {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(&status);
                        if ui.add_enabled(ready, egui::Button::new("Previous")).clicked() { next = Some(true); }
                        if ui.add_enabled(ready, egui::Button::new("Next")).clicked() { next = Some(false); }
                    });
                }
                if self.find.replace {
                    ui.horizontal(|ui| {
                        let label = ui.label("Replace with");
                        let reserve = if narrow { 0.0 } else { 162.0 };
                        let width = (ui.available_width() - reserve).max(80.0);
                        ui.add_sized([width, 24.0], egui::TextEdit::singleline(&mut self.find.replacement)
                            .id_salt("document-find-replacement")
                            .hint_text("Replacement text"))
                            .labelled_by(label.id)
                            .on_hover_text(if self.find.query.regex { "Regex captures: $1, ${name}. Use $$ for a literal $." } else { "Leave empty to delete matching text." });
                        if !narrow {
                            if ui.add_enabled(ready, egui::Button::new("Replace")).clicked() { replace = Some(false); }
                            if ui.add_enabled(ready_all, egui::Button::new("Replace All")).on_hover_text(all_tip).on_disabled_hover_text(all_tip).clicked() { replace = Some(true); }
                        }
                    });
                    if narrow {
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(ready, egui::Button::new("Replace")).clicked() { replace = Some(false); }
                            if ui.add_enabled(ready_all, egui::Button::new("Replace All")).on_hover_text(all_tip).on_disabled_hover_text(all_tip).clicked() { replace = Some(true); }
                        });
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.toggle_value(&mut self.find.query.case_sensitive, "Aa").on_hover_text("Match case");
                    ui.toggle_value(&mut self.find.query.whole_word, "Whole word").on_hover_text("Match whole words only");
                    ui.toggle_value(&mut self.find.query.regex, ".*").on_hover_text("Use a regular expression");
                    ui.separator();
                    ui.toggle_value(&mut self.find.replace, "Replace…").on_hover_text("Show or hide replacement controls");
                    if partial {
                        ui.weak("Loaded part").on_hover_text("The rest of the file is still loading: matches cover the part loaded so far and are updated when the whole file is in.");
                    } else {
                        ui.weak("Current document").on_hover_text("Searches this file’s Markdown source, including formatting and link URLs. Other files are not searched.");
                    }
                    if self.find.active.is_some() && !self.source_visible && ui.small_button("Show in Source").on_hover_text("Inspect a match hidden by Markdown formatting").clicked() {
                        show_source = true;
                    }
                    if can_undo && ui.small_button("Undo Replace All").clicked() { undo = true; }
                });
                if let Some(error) = &self.find.error {
                    let message = error.lines().rev().find(|s| !s.trim().is_empty()).unwrap_or(error);
                    ui.colored_label(ui.visuals().error_fg_color, message).on_hover_text(error);
                }
            });
        if old != self.find.query {
            self.find_invalidate();
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            close = true;
        }
        if close {
            self.close_find();
        } else if show_source {
            let range = self
                .find_results()
                .and_then(|r| self.find.active.and_then(|i| r.matches.get(i).cloned()));
            self.set_view_mode(ViewMode::Source);
            if let (Some(range), Some(d)) = (range, &mut self.doc) {
                d.viewer
                    .ed
                    .source_set_selection(Selection::range(range.start, range.end));
                d.source_ui.find_reveal = true;
            }
        } else if undo {
            self.undo_replace_all();
        } else if let Some(all) = replace {
            self.replace_find(self.find.replacement.clone(), all);
        } else if let Some(backwards) = next {
            self.find_next(backwards);
        }
        self.find_publish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_snapshot_restart_and_document_identity_reject_stale_completions() {
        let mut app = MarkiseApp::new(AppOptions::default());
        app.new_document_now();
        app.open_find(false);
        app.set_find_query(Query {
            text: "x".into(),
            ..Default::default()
        });
        let ctx = Context::default();
        // Simulate a partially fed snapshot without a multi-MB UI/layout fixture.
        let d = app.doc.as_ref().unwrap();
        let key = Key {
            document: d.id,
            revision: d.viewer.ed.session.revision().counter,
            generation: app.find.generation,
        };
        app.find.key = Some(key);
        app.find.copied = 7;
        app.find.chunks.push("partial".into());
        let d = app.doc.as_mut().unwrap();
        let copies = d.viewer.ed.session.copies;
        d.viewer
            .ed
            .edit(|s| s.apply_external(&markise_doc::PatchSet::single_verbatim(0..0, "x"), "test", false))
            .unwrap();
        assert_eq!(
            d.viewer.ed.session.copies, copies,
            "snapshot chunks must not share the FormatDoc"
        );
        app.find_update(&ctx);
        assert!(!app.find_current(key));
        assert!(app.find.chunks.is_empty()); // The new one-byte snapshot was submitted.
        assert_ne!(app.find.key.unwrap().revision, key.revision);
        let stale = app.find.key.unwrap();
        // Make an actual replacement completion ready before switching files, but do not
        // let the app poll it until the new document has been installed.
        let (tx, rx) = std::sync::mpsc::channel();
        app.find.worker = Some(SearchWorker::new(Arc::new(move || {
            let _ = tx.send(());
        })));
        app.find.worker.as_ref().unwrap().submit(
            stale,
            vec!["x".into()],
            app.find.query.clone(),
            Some(Replacement {
                text: "bad".into(),
                ..Default::default()
            }),
        );
        rx.recv_timeout(Duration::from_secs(10))
            .expect("replacement completion");
        app.new_document_now();
        app.find_update(&ctx);
        assert_eq!(
            app.viewed_text(),
            Some(""),
            "late replacement must not edit the new document"
        );
        assert!(
            !app.find_current(stale),
            "a new document can reuse a revision but never identity"
        );
        app.find.results = Some(Results {
            key: stale,
            matches: Arc::new(std::iter::once(0..1).collect()),
            error: None,
            candidates: Vec::new(),
            dropped: Arc::default(),
        });
        app.find_next(false);
        app.replace_find("bad".into(), true);
        assert_eq!(app.viewed_text(), Some(""));
    }
}
