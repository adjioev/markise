//! File commands (spec 04 §2 File menu, §4.1 sidebar file items): File → New (an untitled
//! document in this window), Save As…, and the file tree's New File / New Folder / Rename /
//! Move to Trash / Reveal / Copy Path. The disk work is in `markise_engine::fsops`; this
//! module wires it to the document, the watcher and the file tree. Rename, New File and New
//! Folder edit the name inline in the tree row ([`crate::sidebar::InlineEdit`]); Move to Trash
//! asks through [`crate::confirm`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::Context;
use markise_engine::fsops;
use markise_engine::load::{LoadTimings, Loaded};
use markise_engine::save::{DiskVersion, Expect};
use markise_engine::tree::{MARKDOWN_EXTENSIONS, TEXT_EXTENSIONS};
use markise_layout::DocModel;
use markise_md::Document;

use super::{Ask, Banner, MarkiseApp, SaveRequest, Then};
use crate::sidebar::{EditEnd, InlineEdit, Tab, TreeAction};

/// State of the file operations: the inline rename and the test hook that answers the save
/// dialog.
#[derive(Default)]
pub(crate) struct FileOps {
    /// Answers handed out instead of showing the native save dialog (tests).
    save_answers: VecDeque<Option<PathBuf>>,
    /// The tree row whose name is being edited.
    pub(crate) rename: Option<InlineEdit>,
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
            self.set_ask(Ask::Unsaved(Then::New));
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
        self.side.opened(path);
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
            TreeAction::Delete(p) => {
                if self.ask.is_none() {
                    self.set_ask(Ask::Trash(p));
                }
            }
            TreeAction::EndRename(end) => self.end_rename(end),
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

    /// Starts editing the name of `path` inline in its tree row (the Files tab is shown).
    pub fn start_rename(&mut self, path: PathBuf) {
        self.sidebar_visible = true;
        self.tab = Tab::Files;
        if let Some(t) = self.tree.as_mut() {
            t.expand_to(&path);
        }
        self.files.rename = Some(InlineEdit::new(path));
    }

    /// The inline rename: the item and the name typed so far (tests).
    pub fn rename_state(&self) -> Option<(&Path, &str)> {
        self.files.rename.as_ref().map(|r| (r.path.as_path(), r.name.as_str()))
    }

    /// Types `name` into the inline rename field (tests).
    pub fn set_rename_text(&mut self, name: &str) {
        if let Some(r) = self.files.rename.as_mut() {
            r.name = name.to_string();
        }
    }

    /// The error shown under the renamed row (tests).
    pub fn rename_error(&self) -> Option<&str> {
        self.files.rename.as_ref().and_then(|r| r.error.as_deref())
    }

    /// The inline rename's "click elsewhere" rule (as in Finder): wheel or trackpad scrolling
    /// keeps editing; any pointer press outside the field — the tree's scroll bar included,
    /// and wherever the field is, drawn or scrolled out of view — ends it like a click
    /// elsewhere (renames if the name is accepted, otherwise keeps the old name and says why
    /// in the banner). Losing the focus without a press (Tab) ends it in the row itself.
    pub(super) fn rename_blur(&mut self, ctx: &Context) {
        let Some(r) = self.files.rename.as_ref() else { return };
        if !r.had_focus {
            return; // not started yet (its row was never shown)
        }
        let id = crate::sidebar::rename_field_id();
        let focused = ctx.memory(|m| m.has_focus(id));
        // where this frame's presses started (not where the pointer is now)
        let presses: Vec<egui::Pos2> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerButton { pos, pressed: true, .. } => Some(*pos),
                    _ => None,
                })
                .collect()
        });
        let field = r.drawn.then(|| ctx.read_response(id).map(|f| f.rect)).flatten();
        if presses.iter().any(|p| !field.is_some_and(|f| f.contains(*p))) {
            self.end_rename(EditEnd::CommitOrCancel);
        } else if !focused {
            match ctx.memory(|m| m.focused()) {
                // another widget took the focus (Tab while the row was scrolled away): a
                // focus loss, like Tab in the row
                Some(other) if other != id => self.end_rename(EditEnd::CommitOrCancel),
                // the focus went with the row (scrolled out of view): the field takes it
                // back when it is drawn again, unless something else has it by then
                _ => {
                    if let Some(r) = self.files.rename.as_mut() {
                        r.refocus = true;
                    }
                }
            }
        }
    }

    /// Enter in the inline rename: performs it, or keeps editing with the reason it was
    /// refused (an existing name is never replaced).
    pub fn confirm_rename(&mut self) {
        self.end_rename(EditEnd::Commit);
    }

    /// Esc in the inline rename: keeps the old name (tests).
    pub fn cancel_rename(&mut self) {
        self.end_rename(EditEnd::Cancel);
    }

    fn end_rename(&mut self, end: EditEnd) {
        let Some(mut r) = self.files.rename.take() else { return };
        if end == EditEnd::Cancel {
            return;
        }
        let result = if self.saving.is_some() {
            // the running save writes the old path; renaming now could mark text clean
            // that the new file does not hold
            Err("A save is in progress. Try again when it has finished.".to_string())
        } else {
            fsops::rename(&r.path, r.name.trim_end_matches(['\r', '\n']))
        };
        match result {
            Ok(to) => self.renamed(&r.path, &to),
            Err(e) if end == EditEnd::Commit => {
                r.error = Some(e);
                r.select = true; // focus the field again
                self.files.rename = Some(r);
            }
            Err(e) => {
                // clicked elsewhere with a refused name: the old name stays (say why, if a
                // new one was typed)
                if r.path.file_name().is_none_or(|n| n.to_string_lossy() != r.name) {
                    self.banner = Some(Banner::Info(format!("Not renamed: {e}")));
                }
            }
        }
    }

    /// `from` was renamed to `to`: the document (in it, or it) follows.
    fn renamed(&mut self, from: &Path, to: &Path) {
        self.side.renamed(from, to);
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

    /// "Move to Trash" for the open Move to Trash question (tests; the app answers it
    /// through [`crate::confirm`]).
    pub fn confirm_delete(&mut self) {
        if matches!(self.ask, Some(Ask::Trash(_))) {
            self.answer_question(crate::confirm::Choice::MoveToTrash);
        }
    }

    pub(super) fn move_to_trash(&mut self, p: &Path) {
        match fsops::move_to_trash(p) {
            Ok(()) => {
                self.side.removed(p);
                if let (Some(t), Some(w)) = (self.tree.as_mut(), self.wake.as_ref()) {
                    t.renamed(p, p, w); // forget its listing; list the parent again
                }
            }
            Err(e) => self.banner = Some(Banner::Error(e)),
        }
    }

    /// The Move to Trash question is open for this item (tests).
    pub fn delete_pending(&self) -> Option<&Path> {
        match &self.ask {
            Some(Ask::Trash(p)) => Some(p),
            _ => None,
        }
    }
}
