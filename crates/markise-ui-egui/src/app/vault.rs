//! Vault quick wins (spec 06 §2–§3): wikilinks resolved against the opened folder, Markdown
//! links with encoded paths, creating a note from an unresolved link, Back / Forward between
//! opened notes, revealing the opened note in the file tree and `[[` link autocomplete.
//!
//! The vault is the sidebar's mounted folder (the opened folder, or the document's folder).
//! Its file-name map ([`VaultIndex`]) is built and kept current by a background scanner fed by
//! the file watcher's events; the UI thread only reads snapshots (no I/O per frame).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egui::{Context, Key};
use markise_doc::{FormatDoc, Selection};
use markise_engine::vault::{VaultIndex, VaultScanner};
use markise_layout::{DocModel, LinkTargets};
use markise_md::wikilink::WikiParts;
use markise_md::{Document, MarkdownFormat};

use super::{Ask, Banner, MarkiseApp};

/// History entries kept per window.
const HISTORY_MAX: usize = 200;
/// Rows of the autocomplete popup (V3.3: at most 50).
const SUGGEST_MAX: usize = 50;

/// One opened file and where the reader was in it when leaving it.
#[derive(Clone, Debug)]
pub struct NavEntry {
    pub path: PathBuf,
    /// Viewport anchor (item, offset into it).
    anchor: Option<(usize, f32)>,
    selection: Option<Selection>,
}

/// Back / Forward history of opened files (per window).
#[derive(Clone, Debug, Default)]
pub struct NavHistory {
    entries: Vec<NavEntry>,
    /// The entry of the current document (when it is in the history).
    index: usize,
}

impl NavHistory {
    /// The files in the history, oldest first, and the current position (tests).
    pub fn paths(&self) -> (Vec<PathBuf>, usize) {
        (self.entries.iter().map(|e| e.path.clone()).collect(), self.index)
    }
}

/// A row of the link autocomplete popup.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// The note's stem (what is shown).
    pub label: String,
    /// Its folder (shown dimmed; empty in the vault root).
    pub folder: String,
    /// Vault path of the note.
    rel: String,
}

/// The open link autocomplete popup (V3.1–V3.4, file stage only).
#[derive(Clone, Debug)]
pub struct Completion {
    /// Offset of the `[[` that opened it.
    anchor: usize,
    /// The text typed after `[[`.
    pub query: String,
    pub items: Vec<Suggestion>,
    pub selected: usize,
    /// Generation of the index the items were computed from.
    index_gen: u64,
}

/// A re-parse of the open document with new parse options (a dialect extension switched:
/// wikilinks, CriticMarkup).
struct Reparse {
    doc_id: u64,
    counter: u64,
    rx: Receiver<(Arc<Document>, Arc<DocModel>)>,
    /// What switched (the view's label for the install).
    label: &'static str,
}

/// Vault state of the application.
#[derive(Default)]
pub(crate) struct VaultState {
    scanner: Option<VaultScanner>,
    scanner_failed: bool,
    index: Option<Arc<VaultIndex>>,
    /// The root the scanner was last asked to scan.
    mounted: Option<PathBuf>,
    history: NavHistory,
    /// A Back / Forward target being opened (its history index).
    pending_nav: Option<usize>,
    /// Scroll to this heading path once that file is shown (wikilink `#Heading`, Markdown
    /// `#fragment`).
    pending_subpath: Option<(PathBuf, Vec<String>)>,
    complete: Option<Completion>,
    /// `[[` the user dismissed with Esc (its offset): not reopened there.
    dismissed: Option<usize>,
    /// Session revision counter seen by the last autocomplete update.
    seen_counter: Option<(u64, u64)>,
    reparse: Option<Reparse>,
    /// A note being created from an unresolved link.
    creating: Option<Receiver<Result<PathBuf, String>>>,
    /// An IME composition is in progress: the popup leaves the keys to it.
    ime_composing: bool,
    /// Enter / Tab chose the selected row this frame (accepted after the surface's input).
    accept_pending: bool,
    /// Wikilinks may be on at all (the startup parse options allow them).
    pub(crate) allowed: bool,
}

impl VaultState {
    /// `allowed`: the startup parse options allow wikilinks.
    pub(crate) fn new(allowed: bool) -> Self {
        VaultState {
            allowed,
            ..Default::default()
        }
    }
}

/// Which wikilink targets exist, for the layout of one document.
#[derive(Debug)]
struct VaultLinks {
    index: Option<Arc<VaultIndex>>,
    /// The document's vault path.
    from: Option<String>,
    generation: u64,
}

impl LinkTargets for VaultLinks {
    fn generation(&self) -> u64 {
        self.generation
    }

    fn resolves(&self, target: &str) -> bool {
        match &self.index {
            None => true,
            Some(_) if target.trim().is_empty() => true,
            Some(ix) => ix.resolve_wikilink(target, self.from.as_deref()).is_some(),
        }
    }
}

/// File extensions that never get `.md` appended when a note is created from a link (they
/// name attachments, V1.5; spec 06 V2.8 rule 3).
const ATTACHMENT_EXTS: &[&str] = &[
    "avif", "bmp", "gif", "jpeg", "jpg", "png", "svg", "webp", "flac", "m4a", "mp3", "ogg", "wav", "webm", "3gp",
    "mkv", "mov", "mp4", "ogv", "pdf", "canvas", "base", "json", "txt", "csv", "html", "zip",
];

fn attachment_ext(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| ATTACHMENT_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Heading text as wikilink subpaths match it (V2.5.1): `# | ^ : [ ] %` as spaces, runs of
/// whitespace collapsed, lower case.
fn heading_key(s: &str) -> String {
    let replaced: String = s
        .chars()
        .map(|c| {
            if matches!(c, '#' | '|' | '^' | ':' | '[' | ']' | '%') {
                ' '
            } else {
                c
            }
        })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The outline entry a heading path names (V2.5.1, V2.5.2): the last segment's heading whose
/// ancestors include the earlier segments in order, else the first heading matching the last
/// segment, else (Markdown fragments, V2.6.4) the heading whose slug anchor it is.
pub fn find_heading(model: &DocModel, segs: &[String]) -> Option<usize> {
    let last = segs.last()?;
    let want = heading_key(last);
    let outline = &model.outline;
    let keys: Vec<String> = outline.iter().map(|o| heading_key(&o.text)).collect();
    let ancestors_ok = |k: usize| {
        let mut need = segs.len() - 1;
        let mut level = outline[k].level;
        let mut j = k;
        while need > 0 && j > 0 {
            j -= 1;
            if outline[j].level < level {
                level = outline[j].level;
                if keys[j] == heading_key(&segs[need - 1]) {
                    need -= 1;
                }
            }
        }
        need == 0
    };
    if let Some(k) = (0..outline.len()).find(|&k| keys[k] == want && ancestors_ok(k)) {
        return Some(outline[k].item);
    }
    if let Some(k) = (0..outline.len()).find(|&k| keys[k] == want) {
        return Some(outline[k].item);
    }
    let anchor = markise_md::slug::slug(last);
    model
        .heading_by_anchor(&anchor)
        .or_else(|| model.heading_by_anchor(last))
        .map(|e| e.item)
}

impl MarkiseApp {
    // ------------------------------------------------------------------ index

    /// The vault root: the sidebar's mounted folder.
    fn vault_root(&self) -> Option<PathBuf> {
        self.tree.as_ref().map(|t| t.root.clone())
    }

    /// The vault index, once the first scan finished (tests).
    pub fn vault_index(&self) -> Option<&Arc<VaultIndex>> {
        self.vault.index.as_ref()
    }

    /// Screen centre of a visible link in the rendered document (tests).
    pub fn link_center(&self, target: &str) -> Option<egui::Pos2> {
        self.doc.as_ref().and_then(|d| d.viewer.link_center(target))
    }

    /// Back / Forward history (tests).
    pub fn nav_history(&self) -> &NavHistory {
        &self.vault.history
    }

    /// The file tree (tests).
    pub fn file_tree(&self) -> Option<&crate::sidebar::FileTree> {
        self.tree.as_ref()
    }

    /// A file-system change reported by the watcher (the scanner lists that folder again).
    pub(super) fn vault_changed(&mut self, dir: &Path) {
        if let Some(s) = &self.vault.scanner {
            s.changed(dir.to_path_buf());
        }
    }

    /// The folder watch restarted: walk the vault again.
    pub(super) fn vault_rescan(&mut self) {
        if let Some(s) = &self.vault.scanner {
            s.rescan();
        }
    }

    /// Per frame: settings-driven wikilinks switch, scanner mount and snapshots, re-parse
    /// results. Cheap when nothing changed.
    pub(super) fn vault_poll(&mut self) {
        let want = self.vault.allowed && self.themes.wikilinks();
        if want != self.parse.wikilinks {
            self.apply_wikilinks(want);
        }
        self.review_poll_setting();
        self.poll_reparse();
        self.poll_create();
        let root = self.vault_root();
        if root != self.vault.mounted {
            self.vault.mounted = root.clone();
            self.vault.index = None;
            self.set_doc_links();
            if let Some(r) = root {
                if self.vault.scanner.is_none()
                    && !self.vault.scanner_failed
                    && let Some(w) = self.wake.clone()
                {
                    match VaultScanner::new(w) {
                        Ok(s) => self.vault.scanner = Some(s),
                        Err(e) => {
                            self.vault.scanner_failed = true;
                            self.timing.log(&format!("vault-scanner-error {e}"));
                        }
                    }
                }
                if let Some(s) = &self.vault.scanner {
                    s.mount(r);
                }
            }
        }
        if let Some(ix) = self.vault.scanner.as_ref().and_then(VaultScanner::try_recv)
            && Some(ix.root()) == self.vault.mounted.as_deref()
        {
            self.vault.index = Some(ix);
            self.set_doc_links();
        }
    }

    /// Gives the open document's view the current link targets.
    pub(super) fn set_doc_links(&mut self) {
        let index = self.vault.index.clone();
        let Some(d) = self.doc.as_mut() else { return };
        let from = index.as_ref().and_then(|ix| d.file().and_then(|p| ix.rel_of(p)));
        // the wikilinks switch is part of it: layouts cached with the other parse options of
        // the same source must not be reused
        let base = index.as_ref().map_or(0, |ix| ix.generation().wrapping_add(1));
        // the CriticMarkup switch likewise (ADR 0012)
        let generation =
            (base << 3) | ((self.parse.critic_markup as u64) << 2) | ((self.parse.wikilinks as u64) << 1) | 1;
        d.viewer.ed.view.set_links(Arc::new(VaultLinks {
            index,
            from,
            generation,
        }));
    }

    /// The open document's vault path.
    fn current_rel(&self) -> Option<String> {
        let ix = self.vault.index.as_ref()?;
        self.doc.as_ref().and_then(|d| d.file()).and_then(|p| ix.rel_of(p))
    }

    // ------------------------------------------------------------------ wikilinks switch

    /// Whether wikilinks are parsed (Preferences → Markdown, setting `markdown.wikilinks`).
    pub fn wikilinks(&self) -> bool {
        self.parse.wikilinks
    }

    /// Preferences → Markdown → Wikilinks: switches the dialect extension (persisted) and re-parses the open
    /// document in the background.
    pub fn set_wikilinks(&mut self, on: bool) {
        self.themes.set_wikilinks(on);
        let on = on && self.vault.allowed;
        if on != self.parse.wikilinks {
            self.apply_wikilinks(on);
        }
    }

    fn apply_wikilinks(&mut self, on: bool) {
        self.parse.wikilinks = on;
        self.vault.complete = None;
        self.reparse_document("Wikilinks");
    }

    /// Re-parses the open document with the current parse options in the background (a
    /// dialect extension was switched); later loads parse with them too. The text is not
    /// changed.
    pub(super) fn reparse_document(&mut self, label: &'static str) {
        // later loads parse with the new options
        self.loader = None;
        let wake = self.wake_handle();
        let Some(d) = self.doc.as_mut() else { return };
        let format: Arc<dyn markise_doc::DocumentFormat> = Arc::new(MarkdownFormat {
            opts: self.parse.clone(),
        });
        d.viewer.ed.session.set_format(format);
        let text = d.viewer.ed.session.text().to_string();
        let opts = self.parse.clone();
        let path = d.file().map(Path::to_path_buf);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("markise-reparse".into())
            .spawn(move || {
                let doc = Document::parse(text, opts);
                let model = DocModel::build(&doc, path.as_deref());
                let _ = tx.send((Arc::new(doc), model));
                wake();
            });
        if spawned.is_ok() {
            self.vault.reparse = Some(Reparse {
                doc_id: d.id,
                counter: d.viewer.ed.session.revision().counter,
                rx,
                label,
            });
        }
    }

    fn poll_reparse(&mut self) {
        let Some(r) = &self.vault.reparse else { return };
        let Ok((doc, model)) = r.rx.try_recv() else { return };
        let (doc_id, counter, label) = (r.doc_id, r.counter, r.label);
        self.vault.reparse = None;
        let Some(d) = self.doc.as_mut() else { return };
        if d.id != doc_id {
            return;
        }
        if d.viewer.ed.session.revision().counter != counter {
            // edited meanwhile: parse the current text again
            self.reparse_document(label);
            return;
        }
        let doc: Arc<dyn FormatDoc> = doc;
        let _ = d.viewer.install(doc, model, label, false);
        self.set_doc_links();
    }

    /// A re-parse after switching a dialect extension is still running (tests).
    pub fn is_reparsing(&self) -> bool {
        self.vault.reparse.is_some()
    }

    // ------------------------------------------------------------------ following links

    /// Handles a clicked link that points into the vault: wikilinks (`[[…]]`, as the layout
    /// reports them) and Markdown links to local files with `%20` / `<…>` forms. `false`:
    /// not handled here (the caller classifies it).
    pub(super) fn follow_vault_link(&mut self, target: &str) -> bool {
        if let Some(w) = markise_md::wikilink::parse_link_string(target) {
            self.follow_wikilink(&w);
            return true;
        }
        let Some((path, fragment)) = markise_engine::vault::split_markdown_dest(target) else {
            return false;
        };
        let segs: Vec<String> = fragment.into_iter().collect();
        if let Some(ix) = self.vault.index.clone()
            && let Some(r) = ix.resolve_markdown(target, self.current_rel().as_deref())
        {
            let abs = ix.abs(&r.rel);
            if markise_engine::tree::is_markdown(&abs) {
                self.open_note(abs, segs, true);
                return true;
            }
            return false;
        }
        // no index (yet): relative to the document's folder, decoded (the loader reports a
        // missing file; no file-system access here)
        if self.vault.index.is_some() {
            return false;
        }
        let base = self.doc.as_ref().and_then(|d| d.path.parent().map(Path::to_path_buf));
        let p = base.map_or_else(|| PathBuf::from(&path), |b| b.join(&path));
        if markise_engine::tree::is_markdown(&p) {
            self.open_note(p, segs, false);
            return true;
        }
        false
    }

    /// Opens a note (or scrolls the open one) and scrolls to `segs` when given. `known`: the
    /// path comes from the vault index (canonical, a file): opened without file-system
    /// checks on the UI thread.
    fn open_note(&mut self, abs: PathBuf, segs: Vec<String>, known: bool) {
        if self
            .doc
            .as_ref()
            .and_then(|d| d.file())
            .is_some_and(|f| f == abs.as_path())
        {
            self.scroll_to_heading(&segs);
            return;
        }
        self.vault.pending_subpath = (!segs.is_empty()).then(|| (abs.clone(), segs));
        if known {
            self.open_known_file(abs);
        } else {
            self.open(abs);
        }
    }

    fn scroll_to_heading(&mut self, segs: &[String]) {
        if segs.is_empty() {
            return;
        }
        if let Some(d) = self.doc.as_mut()
            && let Some(i) = find_heading(d.viewer.model(), segs)
        {
            d.viewer.ed.view.scroll_to_item(i);
        }
    }

    fn follow_wikilink(&mut self, w: &WikiParts) {
        let segs: Vec<String> = w.headings().into_iter().map(str::to_string).collect();
        if w.target.is_empty() {
            self.scroll_to_heading(&segs);
            return;
        }
        let from = self.current_rel();
        if let Some(ix) = self.vault.index.clone() {
            if let Some(f) = ix.resolve_wikilink(&w.target, from.as_deref()) {
                let abs = ix.abs(&f.rel);
                if markise_engine::tree::is_markdown(&abs) {
                    self.open_note(abs, segs, true);
                } else {
                    // an attachment: the OS opens it, after asking (like other non-web links)
                    self.set_ask(Ask::Link(file_url(&abs)));
                }
                return;
            }
            self.offer_create(&w.target, from.as_deref());
        } else {
            // the folder is still being scanned (no file-system probing on the UI thread)
            self.banner = Some(Banner::Info("The folder is still being indexed; try again.".into()));
        }
    }

    /// Unresolved link (V2.8): asks to create the note — at the target's path from the vault
    /// root when it has a folder part, else in the default location for new notes (Q-V18).
    fn offer_create(&mut self, target: &str, from: Option<&str>) {
        if self.vault_root().is_none() {
            return;
        }
        let t = markise_engine::vault::nfc(target.trim());
        let last = t.rsplit('/').next().unwrap_or(&t);
        if attachment_ext(last) {
            self.banner = Some(Banner::Info("Only notes can be created from links.".into()));
            return;
        }
        let rel = if t.starts_with("./") || t.starts_with("../") {
            let dir = from.map(|f| f.rsplit_once('/').map_or("", |(d, _)| d)).unwrap_or("");
            match join_vault(dir, &t) {
                Some(r) => r,
                None => {
                    self.banner = Some(Banner::Info("The link points outside the folder.".into()));
                    return;
                }
            }
        } else if t.contains('/') {
            // V2.8 rule 1: a folder part is taken from the vault root
            t.trim_start_matches('/').to_string()
        } else {
            // rule 2: the default location for new notes (§14, Q-V18 as revised by the owner
            // 2026-10-28): the linking note's folder, unless app.json sets `newFileLocation`
            use markise_engine::vault::NewNoteLocation;
            let loc = self
                .vault
                .index
                .as_ref()
                .map_or(NewNoteLocation::Current, |ix| ix.settings().new_notes.clone());
            let dir = match loc {
                NewNoteLocation::Root => String::new(),
                NewNoteLocation::Folder(f) => f,
                NewNoteLocation::Current => from
                    .map(|f| f.rsplit_once('/').map_or("", |(d, _)| d).to_string())
                    .unwrap_or_default(),
            };
            if dir.is_empty() {
                t.clone()
            } else {
                format!("{dir}/{t}")
            }
        };
        let rel = if markise_engine::vault::is_note(&rel) {
            rel
        } else {
            format!("{rel}.md")
        };
        if let Err(why) = markise_engine::vault::check_new_note(&rel) {
            self.banner = Some(Banner::Info(format!("Cannot create “{rel}”: {why}")));
            return;
        }
        self.set_ask(Ask::CreateNote { rel });
    }

    /// The user confirmed creating the note `rel` in the vault at `root`: created on a worker
    /// (containment checked there, `markise_engine::vault::create_note`), then opened.
    pub(super) fn create_note(&mut self, rel: String) {
        // through the vault root's directory handle opened at mount (threat model in
        // `markise_engine::vault`); the root path is not resolved again
        let Some(ix) = self.vault.index.clone() else {
            self.banner = Some(Banner::Info("The folder is still being indexed; try again.".into()));
            return;
        };
        self.vault.creating = Some(markise_engine::vault::spawn_create_note(ix, rel, self.wake_handle()));
    }

    /// A note creation finished: open the note, or report why not.
    fn poll_create(&mut self) {
        let Some(rx) = &self.vault.creating else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("note creation stopped".into()),
        };
        self.vault.creating = None;
        match r {
            Ok(path) => {
                // the scanner lists the folder again; the tree follows the watcher's event
                if let Some(parent) = path.parent() {
                    self.vault_changed(parent);
                }
                self.open_known_file(path);
            }
            Err(e) => self.banner = Some(Banner::Error(format!("Cannot create the note: {e}"))),
        }
    }

    /// A note is being created (tests).
    pub fn is_creating_note(&self) -> bool {
        self.vault.creating.is_some()
    }

    // ------------------------------------------------------------------ history

    /// Before the document is replaced: remember where the reader was in it.
    pub(super) fn nav_leave(&mut self) {
        let Some(d) = self.doc.as_ref() else { return };
        let Some(p) = d.file() else { return };
        let h = &mut self.vault.history;
        if let Some(e) = h.entries.get_mut(h.index)
            && e.path == p
        {
            e.anchor = Some(d.viewer.ed.view.anchor());
            e.selection = Some(d.viewer.ed.session.selection().clone());
        }
    }

    /// A document was installed: record it, restore a Back / Forward position, scroll to a
    /// pending heading, reveal it in the file tree, hand it the link targets.
    pub(super) fn nav_installed(&mut self, untitled: bool) {
        self.vault.complete = None;
        self.vault.dismissed = None;
        self.vault.reparse = None;
        self.set_doc_links();
        if untitled {
            return;
        }
        let Some(path) = self.document_path().map(Path::to_path_buf) else {
            return;
        };
        let h = &mut self.vault.history;
        let mut restore = None;
        match self.vault.pending_nav.take() {
            Some(i) if h.entries.get(i).is_some_and(|e| e.path == path) => {
                h.index = i;
                restore = Some(h.entries[i].clone());
            }
            _ => {
                let same = h.entries.get(h.index).is_some_and(|e| e.path == path);
                if !same {
                    if !h.entries.is_empty() {
                        h.entries.truncate(h.index + 1);
                    }
                    h.entries.push(NavEntry {
                        path: path.clone(),
                        anchor: None,
                        selection: None,
                    });
                    if h.entries.len() > HISTORY_MAX {
                        h.entries.remove(0);
                    }
                    h.index = h.entries.len() - 1;
                }
            }
        }
        if let Some(e) = restore
            && let Some(d) = self.doc.as_mut()
        {
            if let Some((i, off)) = e.anchor {
                d.viewer.ed.view.scroll_to_item(i);
                d.viewer.ed.view.scroll_by(off);
            }
            if let Some(sel) = e.selection
                && sel.primary().end().offset <= d.viewer.ed.session.text().len()
            {
                d.viewer.ed.edit(|s| s.set_selection(sel));
            }
        }
        if let Some((p, segs)) = self.vault.pending_subpath.take()
            && p == path
        {
            self.scroll_to_heading(&segs);
        }
        if let Some(t) = self.tree.as_mut() {
            t.reveal(&path);
        }
    }

    /// The history index Back would open.
    fn back_target(&self) -> Option<usize> {
        let h = &self.vault.history;
        if h.entries.is_empty() {
            return None;
        }
        let at_current = self
            .document_path()
            .is_some_and(|p| h.entries.get(h.index).is_some_and(|e| e.path == p));
        if at_current {
            h.index.checked_sub(1)
        } else {
            Some(h.index)
        }
    }

    fn forward_target(&self) -> Option<usize> {
        let h = &self.vault.history;
        let at_current = self
            .document_path()
            .is_some_and(|p| h.entries.get(h.index).is_some_and(|e| e.path == p));
        (at_current && h.index + 1 < h.entries.len()).then_some(h.index + 1)
    }

    pub fn can_go_back(&self) -> bool {
        self.back_target().is_some()
    }

    pub fn can_go_forward(&self) -> bool {
        self.forward_target().is_some()
    }

    /// Go → Back.
    pub fn go_back(&mut self) {
        if let Some(i) = self.back_target() {
            self.go_to(i);
        }
    }

    /// Go → Forward.
    pub fn go_forward(&mut self) {
        if let Some(i) = self.forward_target() {
            self.go_to(i);
        }
    }

    fn go_to(&mut self, i: usize) {
        let path = self.vault.history.entries[i].path.clone();
        // gone from the vault (known from the index; files outside it are left to the loader)
        let gone = self
            .vault
            .index
            .as_ref()
            .is_some_and(|ix| ix.rel_of(&path).is_some_and(|r| ix.file(&r).is_none()));
        if gone {
            self.vault.history.entries.remove(i);
            let h = &mut self.vault.history;
            if h.index >= i && h.index > 0 {
                h.index -= 1;
            }
            self.banner = Some(Banner::Info(format!("{} no longer exists.", path.display())));
            return;
        }
        self.nav_leave();
        self.vault.pending_nav = Some(i);
        self.vault.pending_subpath = None;
        self.open_known_file(path);
    }

    // ------------------------------------------------------------------ autocomplete

    /// The open autocomplete popup (tests).
    pub fn completion(&self) -> Option<&Completion> {
        self.vault.complete.as_ref()
    }

    /// Before the document surface sees this frame's keys: ↑ / ↓ move the selection, Enter or
    /// Tab accept, Esc closes (V3.4). Only while the popup shows rows.
    pub(super) fn complete_pre_input(&mut self, ctx: &Context) {
        if self.is_asking() {
            return;
        }
        let Some(c) = self.vault.complete.as_ref() else {
            self.track_ime(ctx);
            return;
        };
        if c.items.is_empty() {
            self.track_ime(ctx);
            return;
        }
        let n = c.items.len();
        // keys in event order; while an IME composition is active (Preedit … Commit) the keys
        // belong to it (e.g. Enter confirming a candidate) and are left alone
        let mut composing = self.vault.ime_composing;
        let (mut down, mut up, mut enter, mut tab, mut esc) = (false, false, false, false, false);
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Ime(ime) => {
                    composing = ime_step(composing, ime);
                    true
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !composing && modifiers.is_none() => {
                    let flag = match key {
                        Key::ArrowDown => &mut down,
                        Key::ArrowUp => &mut up,
                        Key::Enter => &mut enter,
                        Key::Tab => &mut tab,
                        Key::Escape => &mut esc,
                        _ => return true,
                    };
                    *flag = true;
                    false
                }
                _ => true,
            });
        });
        self.vault.ime_composing = composing;
        let Some(c) = self.vault.complete.as_mut() else { return };
        if down {
            c.selected = (c.selected + 1) % n;
        }
        if up {
            c.selected = (c.selected + n - 1) % n;
        }
        if esc {
            self.vault.dismissed = Some(c.anchor);
            self.vault.complete = None;
            ctx.request_repaint();
            return;
        }
        if enter || tab {
            // accepted after the surface handled this frame's text (an IME commit or typing
            // before the key must be part of the query)
            self.vault.accept_pending = true;
            ctx.request_repaint();
        }
    }

    /// Follows the IME composition state from this frame's events (no keys taken).
    fn track_ime(&mut self, ctx: &Context) {
        let mut composing = self.vault.ime_composing;
        ctx.input(|i| {
            for e in &i.events {
                if let egui::Event::Ime(ime) = e {
                    composing = ime_step(composing, ime);
                }
            }
        });
        self.vault.ime_composing = composing;
    }

    /// Accepts row `i`: replaces `[[query` (and an auto-paired `]]` after the caret) with the
    /// link in the vault's link syntax and path form (V14.3, Q-V15). A wikilink leaves the
    /// caret before `]]` (V3.4), a Markdown link after it. A name that cannot be a wikilink
    /// (V2.1.2 rule 5) becomes a Markdown link with an encoded path (spec 06 E6).
    pub fn accept_completion(&mut self, i: usize) {
        let Some(c) = self.vault.complete.take() else { return };
        let Some(s) = c.items.get(i) else { return };
        let Some(ix) = self.vault.index.clone() else { return };
        let Some(f) = ix.file(&s.rel) else { return };
        // link syntax and path form per the vault's settings (spec 06 §14, Q-V15); Markdown
        // links while wikilinks are switched off (V3.1)
        let wikilink = ix.settings().wikilinks && self.parse.wikilinks;
        let insert = ix.link_for(f, self.current_rel().as_deref(), wikilink);
        let Some(d) = self.doc.as_mut() else { return };
        let caret = d.viewer.ed.session.selection().primary().focus.offset;
        let text = d.viewer.ed.session.text();
        if c.anchor > caret || !text[c.anchor..].starts_with("[[") {
            return;
        }
        let end = if text[caret..].starts_with("]]") {
            caret + 2
        } else {
            caret
        };
        d.viewer
            .ed
            .edit(|s| {
                s.close_typing();
                s.set_selection(Selection::range(c.anchor, end));
                let r = s.insert_text(&insert);
                s.close_typing();
                // V3.4: a wikilink leaves the caret before its `]]` (ready for `#` / `|`)
                if insert.starts_with("[[") && insert.ends_with("]]") {
                    s.set_selection(Selection::caret(c.anchor + insert.len() - 2));
                }
                r
            })
            .ok();
        d.viewer.ed.refresh_focus();
        self.vault.dismissed = None;
    }

    /// After the document surface handled input: opens the popup right after `[[` was typed,
    /// follows the query while the caret stays in it, closes it otherwise.
    pub(super) fn complete_update(&mut self) {
        self.complete_refresh();
        if std::mem::take(&mut self.vault.accept_pending)
            && let Some(c) = &self.vault.complete
            && !c.items.is_empty()
        {
            let i = c.selected.min(c.items.len() - 1);
            self.accept_completion(i);
        }
    }

    fn complete_refresh(&mut self) {
        // V3.1: the list opens also while wikilinks are off (it then inserts Markdown links)
        let live = !self.source_visible && !self.side_by_side && !self.is_asking();
        let Some(d) = self.doc.as_ref().filter(|_| live) else {
            self.vault.complete = None;
            return;
        };
        let s = &d.viewer.ed.session;
        let sel = s.selection().primary();
        let counter = (d.id, s.revision().counter);
        let changed = self.vault.seen_counter != Some(counter);
        self.vault.seen_counter = Some(counter);
        if !sel.is_caret() {
            self.vault.complete = None;
            return;
        }
        let caret = sel.focus.offset;
        let text = s.text();
        let anchor = match &self.vault.complete {
            Some(c) => Some(c.anchor),
            None if changed
                && caret >= 2
                && text.is_char_boundary(caret)
                && text[..caret].ends_with("[[")
                && !text[..caret - 2].ends_with(['\\', '!'])
                && self.vault.dismissed != Some(caret - 2) =>
            {
                Some(caret - 2)
            }
            None => None,
        };
        let Some(anchor) = anchor else { return };
        let query = (anchor + 2 <= caret && text.get(anchor..anchor + 2) == Some("[["))
            .then(|| text.get(anchor + 2..caret))
            .flatten()
            .filter(|q| q.len() <= 200 && !q.contains(['[', ']', '|', '#', '\n']));
        let Some(query) = query.map(str::to_string) else {
            self.vault.complete = None;
            return;
        };
        let index_gen = self.vault.index.as_ref().map_or(0, |ix| ix.generation());
        if let Some(c) = &self.vault.complete
            && c.query == query
            && c.index_gen == index_gen
        {
            return;
        }
        let items: Vec<Suggestion> = self
            .vault
            .index
            .as_ref()
            .map(|ix| {
                ix.suggest(&query, SUGGEST_MAX)
                    .into_iter()
                    .map(|f| Suggestion {
                        label: f.stem().to_string(),
                        folder: f.folder().to_string(),
                        rel: f.rel.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.vault.complete = Some(Completion {
            anchor,
            query,
            items,
            selected: 0,
            index_gen,
        });
    }

    /// Draws the popup under the caret; a click accepts a row.
    pub(super) fn complete_ui(&mut self, ctx: &Context) {
        let Some(c) = &self.vault.complete else { return };
        if c.items.is_empty() {
            return;
        }
        let Some(at) = self.doc.as_ref().and_then(|d| d.viewer.caret_screen) else {
            return;
        };
        let t = &self.theme;
        let look = crate::sidebar::SideLook::new(t);
        let fill = markise_layout::block::color(t.page_bg);
        let text = markise_layout::block::color(t.text);
        let dim = markise_layout::block::color(t.meta_text);
        let sel_fill = look.active_bg;
        let mut clicked = None;
        let selected = c.selected;
        egui::Area::new(egui::Id::unique("markise-link-complete"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(at.left(), at.bottom() + 4.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(fill).show(ui, |ui| {
                    ui.set_max_width(420.0);
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (i, s) in c.items.iter().enumerate() {
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                &s.label,
                                0.0,
                                egui::TextFormat {
                                    color: text,
                                    ..Default::default()
                                },
                            );
                            if !s.folder.is_empty() {
                                job.append(
                                    &format!("  {}", s.folder),
                                    0.0,
                                    egui::TextFormat {
                                        color: dim,
                                        ..Default::default()
                                    },
                                );
                            }
                            let r = ui.add(egui::Button::selectable(i == selected, job).fill(if i == selected {
                                sel_fill
                            } else {
                                egui::Color32::TRANSPARENT
                            }));
                            if i == selected {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
                });
            });
        if let Some(i) = clicked {
            self.accept_completion(i);
            if let Some(d) = self.doc.as_mut() {
                d.viewer.request_refocus();
            }
        }
    }
}

/// The IME composition state after `ime`: a non-empty preedit is composing, a commit or an
/// empty preedit ends it.
fn ime_step(composing: bool, ime: &egui::ImeEvent) -> bool {
    match ime {
        egui::ImeEvent::Preedit { text, .. } => !text.is_empty(),
        egui::ImeEvent::Commit(_) => false,
        _ => composing,
    }
}

/// `dir` joined with a `./` / `../` path (vault paths); `None` when it leaves the vault.
fn join_vault(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for s in rel.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// A `file://` URL for a local file (opened by the OS after the Open Link question).
fn file_url(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    format!("file://{}", markise_engine::vault::percent_encode_path(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_keys_v2_5_1() {
        assert_eq!(heading_key("Q: Scope|Size"), "q scope size");
        assert_eq!(heading_key("  Next   steps "), "next steps");
    }

    #[test]
    fn create_name_rules_v2_8() {
        assert!(attachment_ext("Missing.png"), "E5");
        assert!(!attachment_ext("Report v1.8"));
    }

    #[test]
    fn file_urls_are_encoded() {
        assert_eq!(file_url(Path::new("/a b/c#d.png")), "file:///a%20b/c%23d.png");
    }
}
