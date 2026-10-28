//! File → Export submenu and the export job status (spec 04 §8): the menu picks a format, a
//! save dialog picks the file, the export runs on a worker ([`markise_engine::export`]) and the
//! status bar shows progress with Cancel, then the result or the error.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use markise_engine::export::{ExportDone, ExportJob, ExportKind, ExportRequest, spawn_export};

use crate::app::MarkiseApp;

/// What the Export menu asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportAction {
    /// Ask for a file, then export.
    Export(ExportKind),
    /// The previous export again, asking for the file (pre-filled).
    Previous,
    /// The previous export again to the same file, without asking.
    OverwritePrevious,
}

/// Outcome shown in the status bar.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportMessage {
    Done { text: String, at: Instant },
    Failed(String),
}

/// Export state of the window.
#[derive(Default)]
pub struct ExportUi {
    job: Option<ExportJob>,
    last: Option<ExportRequest>,
    message: Option<ExportMessage>,
    /// "Export and Overwrite with Previous" waiting for its one-time confirmation.
    pending_overwrite: Option<ExportRequest>,
    /// The (document, destination) pair for which overwriting without the dialog was
    /// confirmed; any other document or destination asks again (spec 04: warns once).
    overwrite_ok_for: Option<(Option<PathBuf>, PathBuf)>,
}

impl ExportUi {
    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    pub fn message(&self) -> Option<&ExportMessage> {
        self.message.as_ref()
    }

    /// An overwrite confirmation is waiting.
    pub fn is_asking_overwrite(&self) -> bool {
        self.pending_overwrite.is_some()
    }

    /// The export waiting for its overwrite confirmation.
    pub(crate) fn pending_overwrite(&self) -> Option<&ExportRequest> {
        self.pending_overwrite.as_ref()
    }

    pub(crate) fn answer_overwrite(&mut self, overwrite: bool) -> Option<ExportRequest> {
        let req = self.pending_overwrite.take()?;
        if !overwrite {
            return None;
        }
        self.overwrite_ok_for = Some((req.doc_path.clone(), req.output.clone()));
        Some(req)
    }

    /// Format label of the previous export, if any ("Export with Previous (PDF)").
    pub fn previous_label(&self) -> Option<String> {
        self.last.as_ref().map(|r| r.kind.label())
    }

    pub fn has_previous(&self) -> bool {
        self.last.is_some()
    }

    /// Polls the running job; returns true when it finished this frame.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.job else { return false };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the export worker stopped unexpectedly".into()),
        };
        let label = job.label.clone();
        self.job = None;
        self.message = Some(match res {
            Ok(done) => ExportMessage::Done {
                text: done_text(&label, &done),
                at: Instant::now(),
            },
            Err(e) => ExportMessage::Failed(format!("{label} export failed: {e}")),
        });
        true
    }

    /// Status bar part: progress + Cancel while running, else the last result.
    pub fn status(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            ui.spinner();
            let name = job.output.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            ui.label(format!("Exporting {} ({:.0}%)", name, job.progress() * 100.0));
            if ui.small_button("Cancel").clicked() {
                job.cancel();
            }
            ui.ctx().request_repaint_after(Duration::from_millis(200));
            return;
        }
        match &self.message {
            Some(ExportMessage::Done { text, at }) => {
                ui.weak(text);
                // the success note fades after a while
                let left = Duration::from_secs(8).saturating_sub(at.elapsed());
                if left.is_zero() {
                    self.message = None;
                } else {
                    ui.ctx().request_repaint_after(left);
                }
            }
            Some(ExportMessage::Failed(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e)
                    .on_hover_text(e.clone());
                if ui.small_button("✕").clicked() {
                    self.message = None;
                }
            }
            None => {}
        }
    }
}

fn done_text(label: &str, d: &ExportDone) -> String {
    let name = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let files = d.outputs.iter().map(name).collect::<Vec<_>>().join(", ");
    let mut s = format!("Exported {label}: {files} ({:.1} s)", d.elapsed.as_secs_f32());
    for w in &d.warnings {
        s.push_str(" · ");
        s.push_str(w);
    }
    s
}

fn default_output(doc: Option<&Path>, ext: &str) -> PathBuf {
    match doc {
        Some(p) => p.with_extension(ext),
        None => PathBuf::from(format!("Untitled.{ext}")),
    }
}

impl MarkiseApp {
    /// Runs a menu action (save dialog on the UI thread, export on a worker).
    pub(crate) fn export_action(&mut self, a: ExportAction) {
        let Some(text) = self.document_text() else { return };
        let doc_path = self.document_path().map(Path::to_path_buf);
        let theme = self.theme().clone();
        let req = match a {
            ExportAction::Export(kind) => {
                let ext = kind.extension();
                let proposal = default_output(doc_path.as_deref(), &ext);
                let Some(output) = save_dialog(&proposal, &kind.label(), &ext) else {
                    return;
                };
                ExportRequest::new(kind, text, doc_path, output, theme)
            }
            ExportAction::Previous => {
                let Some(prev) = self.export.last.clone() else { return };
                let ext = prev.kind.extension();
                let proposal = match &prev.output.parent() {
                    Some(dir) => dir.join(
                        default_output(doc_path.as_deref(), &ext)
                            .file_name()
                            .unwrap_or_default(),
                    ),
                    None => default_output(doc_path.as_deref(), &ext),
                };
                let Some(output) = save_dialog(&proposal, &prev.kind.label(), &ext) else {
                    return;
                };
                let mut r = prev.again(text, doc_path, theme);
                r.output = output;
                r
            }
            ExportAction::OverwritePrevious => {
                let Some(prev) = self.export.last.clone() else { return };
                let req = prev.again(text, doc_path, theme);
                let confirmed =
                    self.export.overwrite_ok_for.as_ref() == Some(&(req.doc_path.clone(), req.output.clone()));
                if !confirmed {
                    self.export.pending_overwrite = Some(req);
                    return;
                }
                req
            }
        };
        self.start_export(req);
    }

    /// Starts an export without a dialog (menu, tests).
    pub fn start_export(&mut self, req: ExportRequest) {
        if self.export.job.is_some() {
            return;
        }
        let wake = self.wake_handle();
        self.export.message = None;
        self.export.last = Some(req.clone());
        self.export.job = Some(spawn_export(req, wake));
    }

    /// Runs an action that needs no dialog ("Export and Overwrite with Previous"; tests).
    pub fn export_action_for_test(&mut self, a: ExportAction) {
        if a == ExportAction::OverwritePrevious {
            self.export_action(a);
        }
    }

    /// Answers the overwrite confirmation directly (tests; the app asks through
    /// [`crate::confirm`]).
    pub fn answer_overwrite_for_test(&mut self, overwrite: bool) {
        if let Some(r) = self.export.answer_overwrite(overwrite) {
            self.start_export(r);
        }
    }

    /// Export state (tests).
    pub fn export_state(&self) -> &ExportUi {
        &self.export
    }
}

fn save_dialog(proposal: &Path, label: &str, ext: &str) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().add_filter(label, &[ext]);
    if let Some(dir) = proposal.parent().filter(|d| !d.as_os_str().is_empty()) {
        d = d.set_directory(dir);
    }
    if let Some(name) = proposal.file_name() {
        d = d.set_file_name(name.to_string_lossy());
    }
    d.save_file()
}
