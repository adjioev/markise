//! File commands (spec 04 §2 File menu, §4.1 sidebar file items): File → New (an untitled
//! document in this window), Save As…, and the file tree's New File / New Folder / Rename /
//! Move to Trash / Reveal / Copy Path. The disk work is in `markise_engine::fsops`; this
//! module wires it to the document, the watcher and the file tree, and owns the rename and
//! delete questions.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Context, RichText};
use markise_engine::fsops;
use markise_engine::load::{LoadTimings, Loaded};
use markise_engine::save::{DiskVersion, Expect};
use markise_engine::tree::{MARKDOWN_EXTENSIONS, TEXT_EXTENSIONS};
use markise_layout::DocModel;
use markise_md::Document;

use super::{Ask, Banner, MarkiseApp, SaveRequest, Then};
use crate::sidebar::TreeAction;

/// State of the file operations: the open rename / delete question and the test hook that
/// answers the save dialog.
#[derive(Default)]
pub(crate) struct FileOps {
    /// Answers handed out instead of showing the native save dialog (tests).
    save_answers: VecDeque<Option<PathBuf>>,
    rename: Option<Rename>,
    delete: Option<PathBuf>,
}

struct Rename {
    path: PathBuf,
    name: String,
    error: Option<String>,
    /// First frame: focus the field and select the name without its extension.
    fresh: bool,
}

/// `p` with the default extension when it has none (spec 05: `md`; returns whether it was
/// added), its folder canonical (the file tree and the watcher use canonical paths).
fn normalize_target(mut p: PathBuf) -> (PathBuf, bool) {
    let added = p.extension().is_none();
    if added {
        p.set_extension(fsops::DEFAULT_EXTENSION);
    }
    let p = match (p.parent().and_then(|d| std::fs::canonicalize(d).ok()), p.file_name()) {
        (Some(d), Some(n)) => d.join(n),
        _ => p,
    };
    (p, added)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Shows `path` in the platform's file manager (selected where the platform supports it).
fn reveal(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        c.arg(arg);
        c
    } else {
        // no portable "select this file": open the folder that holds it
        let mut c = Command::new("xdg-open");
        c.arg(if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        });
        c
    };
    cmd.spawn().map(|_| ())
}

impl MarkiseApp {
    // ------------------------------------------------------------------ New

    /// File → New (spec 04 §2): an untitled document in this window (asks to save unsaved
    /// edits first). New Window is not implemented yet: the app has one window.
    pub fn new_document(&mut self) {
        if let Some(d) = &mut self.doc {
            d.sync();
        }
        if self.is_dirty() {
            self.ask = Some(Ask::Unsaved(Then::New));
            return;
        }
        self.new_document_now();
    }

    /// The folder Save As proposes for an untitled document (and that its relative links
    /// resolve against): the mounted folder, else the current document's, else home.
    fn untitled_dir(&self) -> PathBuf {
        self.tree
            .as_ref()
            .map(|t| t.root.clone())
            .or_else(|| self.doc.as_ref().and_then(|d| d.path.parent().map(Path::to_path_buf)))
            .or_else(home_dir)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    pub(super) fn new_document_now(&mut self) {
        // a load still running would replace the new document when it lands
        if let Some(l) = &self.loader {
            l.cancel();
        }
        self.load = None;
        let placeholder = self
            .untitled_dir()
            .join(format!("{}.{}", fsops::UNTITLED, fsops::DEFAULT_EXTENSION));
        // empty text: new lines are written with LF (spec 05 default line ending)
        let doc = Document::parse(String::new(), self.parse.clone());
        let model = DocModel::build(&doc, Some(&placeholder));
        let loaded = Loaded {
            path: placeholder,
            doc: Arc::new(doc),
            model,
            timings: LoadTimings::default(),
            mtime: None,
            version: DiskVersion::of_bytes(b"", None),
            lossy: false,
            bytes: 0,
        };
        self.install_doc(loaded, true);
    }

    // ------------------------------------------------------------------ Save As

    /// File → Save As… (spec 04 §2): asks for a file and saves the document there; the
    /// document then lives in that file.
    pub fn save_as(&mut self) {
        if self.doc.is_none() {
            return;
        }
        if let Some((p, expect)) = self.pick_save_target() {
            self.start_save(SaveRequest {
                expect: Some(expect),
                target: Some(p),
                then: Then::Nothing,
            });
        }
    }

    /// Asks for the Save As file and returns it with what the save expects to find there:
    /// the version of the file the dialog confirmed replacing (captured now, so a change
    /// made afterwards is still a conflict and the replaced content keeps its recovery
    /// backup), or no file. `None`: cancelled, or refused with a banner.
    pub(super) fn pick_save_target(&mut self) -> Option<(PathBuf, Expect)> {
        let (p, added) = normalize_target(self.pick_save_path()?);
        if added && std::fs::symlink_metadata(&p).is_ok() {
            // the dialog confirmed the name without the extension, not this file
            self.banner = Some(Banner::Error(format!(
                "Not saved: “{}” already exists. Choose it in Save As to replace it.",
                p.display()
            )));
            return None;
        }
        match markise_engine::save::disk_version(&p) {
            Ok(Some(v)) => Some((p, Expect::Version(v))),
            Ok(None) => Some((p, Expect::Missing)),
            Err(e) => {
                self.banner = Some(Banner::Error(format!("Cannot save to {}: {e}", p.display())));
                None
            }
        }
    }

    /// Test hook: the next save dialog returns `answer` (`None`: Cancel) instead of opening.
    pub fn queue_save_dialog_answer(&mut self, answer: Option<PathBuf>) {
        self.files.save_answers.push_back(answer);
    }

    /// The native save dialog (default name `Untitled.md` or the current name; the Markdown
    /// filter comes first so the platform adds `.md` and confirms replacing the final name).
    fn pick_save_path(&mut self) -> Option<PathBuf> {
        if let Some(a) = self.files.save_answers.pop_front() {
            return a;
        }
        let (dir, name) = match &self.doc {
            Some(d) if !d.untitled => (
                d.path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.untitled_dir()),
                d.display_name(),
            ),
            _ => (
                self.untitled_dir(),
                format!("{}.{}", fsops::UNTITLED, fsops::DEFAULT_EXTENSION),
            ),
        };
        rfd::FileDialog::new()
            .set_title("Save As")
            .set_directory(dir)
            .set_file_name(name)
            .add_filter("Markdown", MARKDOWN_EXTENSIONS)
            .add_filter("Plain text", TEXT_EXTENSIONS)
            .save_file()
    }

    /// The document's file is now `path` (Save As, rename): watch it, resolve relative
    /// references against it, show its folder in the sidebar. `version`: the disk version the
    /// document now corresponds to (what Save As wrote; for a rename the unchanged content),
    /// the watcher's baseline for the new path (ADR 0010).
    pub(super) fn document_moved(&mut self, path: &Path, version: Option<DiskVersion>) {
        if let Some(w) = self.watcher.as_mut() {
            w.watch_file(path, version);
        }
        if let Some(d) = self.doc.as_mut() {
            d.viewer.ed.set_path(Some(path.to_path_buf()));
        }
        let Some(parent) = path.parent() else { return };
        match (&mut self.tree, &self.wake) {
            (None, _) => {
                self.tree = Some(crate::sidebar::FileTree::new(parent.to_path_buf()));
                if let Some(w) = self.watcher.as_mut() {
                    w.watch_folder(parent);
                }
            }
            (Some(t), Some(wake)) if parent.starts_with(&t.root) => t.reveal_dir(parent, wake),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ file tree

    /// Performs a file tree click or context-menu item (also the test entry point).
    pub fn tree_action(&mut self, ctx: &Context, a: TreeAction) {
        match a {
            TreeAction::Open(p) => self.open(p),
            TreeAction::NewFile(dir) => match fsops::create_file(&dir) {
                Ok(p) => {
                    self.show_in_tree(&dir);
                    self.open(p.clone());
                    self.start_rename(p);
                }
                Err(e) => self.banner = Some(Banner::Error(format!("Cannot create a file in {}: {e}", dir.display()))),
            },
            TreeAction::NewFolder(dir) => match fsops::create_folder(&dir) {
                Ok(p) => {
                    self.show_in_tree(&dir);
                    self.start_rename(p);
                }
                Err(e) => {
                    self.banner = Some(Banner::Error(format!(
                        "Cannot create a folder in {}: {e}",
                        dir.display()
                    )))
                }
            },
            TreeAction::Rename(p) => self.start_rename(p),
            TreeAction::Delete(p) => self.files.delete = Some(p),
            TreeAction::Reveal(p) => {
                if let Err(e) = reveal(&p) {
                    self.banner = Some(Banner::Error(format!("Cannot show {}: {e}", p.display())));
                }
            }
            TreeAction::CopyPath(p) => ctx.copy_text(p.display().to_string()),
        }
        ctx.request_repaint();
    }

    fn show_in_tree(&mut self, dir: &Path) {
        if let (Some(t), Some(w)) = (self.tree.as_mut(), self.wake.as_ref()) {
            t.reveal_dir(dir, w);
        }
    }

    /// Opens the rename question for `path`.
    pub fn start_rename(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.files.rename = Some(Rename {
            path,
            name,
            error: None,
            fresh: true,
        });
    }

    /// The rename question: the item and the name typed so far (tests).
    pub fn rename_state(&self) -> Option<(&Path, &str)> {
        self.files.rename.as_ref().map(|r| (r.path.as_path(), r.name.as_str()))
    }

    /// Types `name` into the rename question (tests).
    pub fn set_rename_text(&mut self, name: &str) {
        if let Some(r) = self.files.rename.as_mut() {
            r.name = name.to_string();
        }
    }

    /// The error shown in the rename question (tests).
    pub fn rename_error(&self) -> Option<&str> {
        self.files.rename.as_ref().and_then(|r| r.error.as_deref())
    }

    /// "Rename" in the rename question: performs it, or keeps the question open with the
    /// reason it was refused (an existing name is never replaced).
    pub fn confirm_rename(&mut self) {
        let Some(mut r) = self.files.rename.take() else { return };
        if self.saving.is_some() {
            // the running save writes the old path; renaming now could mark text clean
            // that the new file does not hold
            r.error = Some("A save is in progress. Try again when it has finished.".into());
            self.files.rename = Some(r);
            return;
        }
        match fsops::rename(&r.path, &r.name) {
            Ok(to) => self.renamed(&r.path, &to),
            Err(e) => {
                r.error = Some(e);
                self.files.rename = Some(r);
            }
        }
    }

    /// `from` was renamed to `to`: the document (in it, or it) follows.
    fn renamed(&mut self, from: &Path, to: &Path) {
        if let (Some(t), Some(w)) = (self.tree.as_mut(), self.wake.as_ref()) {
            t.renamed(from, to, w);
        }
        // a load of the old path still running: load the new one instead
        if let Some(l) = &self.load
            && let Some(p) = fsops::rebase(&l.path, from, to)
        {
            let reason = l.reason;
            self.start_load(p, reason);
        }
        let moved = self
            .doc
            .as_ref()
            .and_then(|d| d.file())
            .and_then(|f| fsops::rebase(f, from, to));
        if let Some(p) = moved {
            let mut version = None;
            if let Some(d) = self.doc.as_mut() {
                d.path = p.clone();
                version = d.disk; // a rename keeps the content
            }
            self.document_moved(&p, version);
        }
    }

    /// "Move to Trash" in the delete question (tests).
    pub fn confirm_delete(&mut self) {
        let Some(p) = self.files.delete.take() else { return };
        match fsops::move_to_trash(&p) {
            Ok(()) => {
                if let (Some(t), Some(w)) = (self.tree.as_mut(), self.wake.as_ref()) {
                    t.renamed(&p, &p, w); // forget its listing; list the parent again
                }
            }
            Err(e) => self.banner = Some(Banner::Error(e)),
        }
    }

    /// The delete question is open (tests).
    pub fn delete_pending(&self) -> Option<&Path> {
        self.files.delete.as_deref()
    }

    /// The rename and delete questions (after any Save / Discard question).
    pub(super) fn file_dialogs(&mut self, ctx: &Context) {
        if self.ask.is_some() {
            return;
        }
        if let Some(p) = self.files.delete.clone() {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("markise-delete")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(RichText::new(format!("Move “{name}” to the Trash?")).strong());
                ui.horizontal(|ui| {
                    if ui.button("Move to Trash").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        choice = Some(false);
                    }
                });
            });
            match choice {
                Some(true) => self.confirm_delete(),
                Some(false) => self.files.delete = None,
                None => {}
            }
            return;
        }
        let Some(r) = self.files.rename.as_mut() else { return };
        let mut choice = None;
        let field = egui::Id::unique("markise-rename-field");
        egui::Modal::new(egui::Id::unique("markise-rename")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let old = r
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.label(RichText::new(format!("Rename “{old}”")).strong());
            let out = egui::TextEdit::singleline(&mut r.name)
                .id(field)
                .desired_width(380.0)
                .show(ui);
            if r.fresh {
                r.fresh = false;
                out.response.request_focus();
                // select the name without its extension (folders: all of it)
                let stem = if r.path.is_dir() {
                    r.name.chars().count()
                } else {
                    r.name
                        .rfind('.')
                        .map_or(r.name.chars().count(), |i| r.name[..i].chars().count())
                };
                let mut st = out.state;
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(stem),
                )));
                st.store(ui.ctx(), field);
            }
            if out.response.changed() {
                r.error = None;
            }
            if let Some(e) = &r.error {
                ui.colored_label(egui::Color32::from_rgb(0xc0, 0x39, 0x2b), e);
            }
            let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if ui.button("Rename").clicked() || enter {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => self.confirm_rename(),
            Some(false) => self.files.rename = None,
            None => {}
        }
    }
}
