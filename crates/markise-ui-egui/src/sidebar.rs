//! Sidebar: lazy file tree of the mounted folder and the document outline (spec 04 §4.1).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Ui};
use markise_engine::Wake;
use markise_engine::tree::{Entry, Lister};
use markise_layout::OutlineEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Files,
    Outline,
}

/// What the user asked for in the file tree: a click, or a context-menu item (spec 04 §4.1
/// sidebar file items). Performed by the application (`app/files.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeAction {
    /// A file row was clicked.
    Open(PathBuf),
    /// New File in this folder.
    NewFile(PathBuf),
    /// New Folder in this folder.
    NewFolder(PathBuf),
    Rename(PathBuf),
    /// Move to Trash.
    Delete(PathBuf),
    /// Reveal in Finder / Explorer / the file manager.
    Reveal(PathBuf),
    CopyPath(PathBuf),
    /// The inline name editor finished (see [`InlineEdit`]).
    EndRename(EditEnd),
}

/// The inline name editor of a file tree row (Rename, and the new item after New File / New
/// Folder): the row turns into a text field with the name.
#[derive(Clone, Debug)]
pub struct InlineEdit {
    pub path: PathBuf,
    pub name: String,
    /// Why the last attempt was refused (shown under the row while editing goes on).
    pub error: Option<String>,
    /// Focus the field and select the name without its extension at the next frame the row
    /// is shown (the start, and after a refused Enter).
    pub select: bool,
    /// The field has had the keyboard focus (editing really started).
    pub had_focus: bool,
    /// Give the field the focus back at the next frame it is drawn, keeping the selection
    /// (its row was scrolled out of view, which took the focus).
    pub refocus: bool,
    /// The row was drawn this frame (reset by the app before the sidebar).
    pub drawn: bool,
}

impl InlineEdit {
    pub fn new(path: PathBuf) -> InlineEdit {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        InlineEdit {
            path,
            name,
            error: None,
            select: true,
            had_focus: false,
            refocus: false,
            drawn: false,
        }
    }

    /// Characters of the name that start out selected: the stem of a file (the extension is
    /// kept), all of a folder's name (or of a name without a stem, like `.env`).
    pub fn stem_chars(&self) -> usize {
        let all = self.name.chars().count();
        if self.path.is_dir() {
            return all;
        }
        match self.name.rfind('.') {
            Some(i) if i > 0 => self.name[..i].chars().count(),
            _ => all,
        }
    }
}

/// How the inline name editor finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditEnd {
    /// Enter: rename; a refused name keeps the editor open with the reason.
    Commit,
    /// The field lost the focus another way (a click elsewhere): rename if the name is
    /// accepted, otherwise give up.
    CommitOrCancel,
    /// Esc: keep the old name.
    Cancel,
}

/// Id of the inline name field (tests focus and type into it).
pub fn rename_field_id() -> egui::Id {
    egui::Id::unique("markise-rename-field")
}

/// Label of the "reveal in the file manager" item on this platform.
pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Show in File Manager"
    }
}

/// The context menu of a file tree row (`dir`: the row is a folder) or of the folder itself
/// (`root`: no Rename / Delete).
fn row_menu(ui: &mut Ui, path: &Path, dir: bool, root: bool, out: &mut Option<TreeAction>) {
    let folder = if dir {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    };
    let mut pick = |ui: &mut Ui, label: &str, a: TreeAction| {
        if ui.button(label).clicked() {
            *out = Some(a);
            ui.close();
        }
    };
    if !dir {
        pick(ui, "Open", TreeAction::Open(path.to_path_buf()));
        ui.separator();
    }
    pick(ui, "New File", TreeAction::NewFile(folder.clone()));
    pick(ui, "New Folder", TreeAction::NewFolder(folder));
    ui.separator();
    if !root {
        pick(ui, "Rename", TreeAction::Rename(path.to_path_buf()));
        pick(ui, "Move to Trash", TreeAction::Delete(path.to_path_buf()));
        ui.separator();
    }
    pick(ui, reveal_label(), TreeAction::Reveal(path.to_path_buf()));
    pick(ui, "Copy Path", TreeAction::CopyPath(path.to_path_buf()));
}

/// Listing state of one directory.
#[derive(Default)]
struct DirState {
    entries: Option<Vec<Entry>>,
    error: Option<String>,
    /// A listing request is running.
    in_flight: bool,
    /// A change arrived while listing: list again when the running request finishes.
    pending: bool,
    generation: u64,
}

/// One visible row of the flattened tree.
#[derive(Clone, Debug)]
pub struct Row {
    pub depth: usize,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    /// Placeholder row ("…" while listing, or an error).
    pub note: Option<String>,
}

/// The mounted folder. Directories are listed lazily by one background worker; change
/// bursts are coalesced per directory (one listing in flight + one pending); the expanded
/// tree is flattened into a cached row list and only visible rows are drawn.
pub struct FileTree {
    pub root: PathBuf,
    dirs: HashMap<PathBuf, DirState>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    rows_dirty: bool,
    lister: Option<Lister>,
    lister_error: Option<String>,
    /// Number of listing requests issued (tests).
    pub requests: u64,
    /// Rows drawn by the last frame (the virtualised range).
    shown: std::ops::Range<usize>,
    /// A file to scroll into view once its row is listed ([`FileTree::reveal`]).
    pending_reveal: Option<PathBuf>,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        FileTree {
            root,
            dirs: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: true,
            lister: None,
            lister_error: None,
            requests: 0,
            shown: 0..0,
            pending_reveal: None,
        }
    }

    fn issue(&mut self, dir: &Path, wake: &Wake) {
        if self.lister.is_none() && self.lister_error.is_none() {
            match Lister::new(wake.clone()) {
                Ok(l) => self.lister = Some(l),
                Err(e) => self.lister_error = Some(e),
            }
        }
        let st = self.dirs.entry(dir.to_path_buf()).or_default();
        st.generation += 1;
        st.in_flight = true;
        st.pending = false;
        self.requests += 1;
        match &self.lister {
            Some(l) if l.request(dir.to_path_buf(), st.generation) => {}
            _ => {
                st.in_flight = false;
                st.error = Some(
                    self.lister_error
                        .clone()
                        .unwrap_or_else(|| "listing unavailable".into()),
                );
            }
        }
    }

    fn ensure(&mut self, dir: &Path, wake: &Wake) {
        if !self.dirs.contains_key(dir) {
            self.issue(dir, wake);
        }
    }

    /// A directory changed on disk (watcher): list it again, coalescing bursts.
    pub fn invalidate(&mut self, dir: &Path, wake: &Wake) {
        let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let key = if self.dirs.contains_key(&canon) {
            Some(canon)
        } else {
            self.dirs
                .keys()
                .find(|k| std::fs::canonicalize(k).is_ok_and(|c| c == canon))
                .cloned()
        };
        let Some(k) = key else { return };
        let st = self.dirs.get_mut(&k).expect("key exists");
        if st.in_flight {
            st.pending = true;
        } else {
            self.issue(&k, wake);
        }
    }

    /// Lists every known directory again (the folder watch restarted: changes made before it
    /// took effect have no events).
    pub fn invalidate_all(&mut self, wake: &Wake) {
        let dirs: Vec<PathBuf> = self.dirs.keys().cloned().collect();
        for d in dirs {
            let st = self.dirs.get_mut(&d).expect("key exists");
            if st.in_flight {
                st.pending = true;
            } else {
                self.issue(&d, wake);
            }
        }
    }

    /// Collects finished listings; returns `true` if any arrived.
    pub fn poll(&mut self, wake: &Wake) -> bool {
        let mut got = false;
        let mut again = Vec::new();
        while let Some((dir, generation, r)) = self.lister.as_ref().and_then(|l| l.try_recv()) {
            let Some(st) = self.dirs.get_mut(&dir) else { continue };
            if generation != st.generation {
                continue; // superseded
            }
            st.in_flight = false;
            match r {
                Ok(v) => {
                    st.entries = Some(v);
                    st.error = None;
                }
                Err(e) => st.error = Some(e),
            }
            if st.pending {
                again.push(dir);
            }
            got = true;
        }
        for d in again {
            self.issue(&d, wake);
        }
        if got {
            self.rows_dirty = true;
        }
        got
    }

    fn rebuild_rows(&mut self, wake: &Wake) {
        let mut rows = Vec::new();
        let root = self.root.clone();
        self.walk(&root, 0, wake, &mut rows);
        self.rows = rows;
        self.rows_dirty = false;
    }

    fn walk(&mut self, dir: &Path, depth: usize, wake: &Wake, rows: &mut Vec<Row>) {
        self.ensure(dir, wake);
        let st = &self.dirs[dir];
        let note = |t: &str| Row {
            depth,
            name: String::new(),
            path: dir.to_path_buf(),
            is_dir: false,
            expanded: false,
            note: Some(t.to_string()),
        };
        let entries = match (&st.entries, &st.error) {
            (Some(v), _) => v.clone(),
            (None, Some(e)) => {
                rows.push(note(&format!("cannot list: {e}")));
                return;
            }
            (None, None) => {
                rows.push(note("…"));
                return;
            }
        };
        if entries.is_empty() {
            rows.push(note("(no Markdown files)"));
        }
        for e in entries {
            let expanded = e.is_dir && self.expanded.contains(&e.path);
            rows.push(Row {
                depth,
                name: e.name,
                path: e.path.clone(),
                is_dir: e.is_dir,
                expanded,
                note: None,
            });
            if expanded {
                self.walk(&e.path, depth + 1, wake, rows);
            }
        }
    }

    /// Flattened visible rows (rebuilt only when listings or expansion change).
    pub fn rows(&mut self, wake: &Wake) -> &[Row] {
        if self.rows_dirty {
            self.rebuild_rows(wake);
        }
        &self.rows
    }

    /// Names of the listed entries as last shown (placeholder rows excluded), top to bottom.
    pub fn row_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| r.note.is_none())
            .map(|r| r.name.clone())
            .collect()
    }

    /// Shows the contents of `dir` (and lists it again: a file operation changed it).
    pub fn reveal_dir(&mut self, dir: &Path, wake: &Wake) {
        if dir != self.root {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rows_dirty = true;
        if self.dirs.contains_key(dir) {
            self.invalidate(dir, wake);
        }
    }

    /// Expands every folder between the root and `path` (so its row is shown).
    pub fn expand_to(&mut self, path: &Path) {
        if !path.starts_with(&self.root) {
            return;
        }
        for a in path.ancestors().skip(1) {
            if a == self.root || !a.starts_with(&self.root) {
                break;
            }
            self.expanded.insert(a.to_path_buf());
        }
        self.rows_dirty = true;
    }

    /// Expands the folders down to `path` and scrolls its row into view once listed (a note
    /// was opened through a link, Back / Forward). Paths outside the tree are ignored.
    pub fn reveal(&mut self, path: &Path) {
        if !path.starts_with(&self.root) || path == self.root {
            return;
        }
        self.expand_to(path);
        self.pending_reveal = Some(path.to_path_buf());
    }

    /// The file waiting to be scrolled into view (tests).
    pub fn pending_reveal(&self) -> Option<&Path> {
        self.pending_reveal.as_deref()
    }

    /// Whether `dir` is expanded (tests).
    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// `from` was renamed to `to`: expanded folders follow, the parent is listed again.
    pub fn renamed(&mut self, from: &Path, to: &Path, wake: &Wake) {
        let moved: Vec<PathBuf> = self.expanded.iter().filter(|p| p.starts_with(from)).cloned().collect();
        for p in moved {
            self.expanded.remove(&p);
            if let Some(n) = markise_engine::fsops::rebase(&p, from, to) {
                self.expanded.insert(n);
            }
        }
        self.dirs.retain(|k, _| !k.starts_with(from));
        self.rows_dirty = true;
        if let Some(parent) = to.parent() {
            self.invalidate(parent, wake);
        }
    }

    pub fn toggle(&mut self, dir: &Path) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rows_dirty = true;
    }

    /// Shows the tree; returns what the user clicked or chose in a context menu, and how the
    /// inline name editor (`edit`, drawn in its row) finished — that one first.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        current: Option<&Path>,
        wake: &Wake,
        look: &SideLook,
        mut edit: Option<&mut InlineEdit>,
    ) -> Vec<TreeAction> {
        let name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        let mut action = None;
        let mut end = None;
        let root = self.root.clone();
        ui.add(
            egui::Label::new(RichText::new(name).strong().color(look.active_text))
                .selectable(false)
                .sense(egui::Sense::click()),
        )
        .context_menu(|ui| row_menu(ui, &root, true, true, &mut action));
        let n = self.rows(wake).len();
        // a new editor whose row is not drawn: scroll it into view
        let mut scroll_to = edit
            .as_deref()
            .filter(|e| e.select)
            .and_then(|e| self.rows.iter().position(|r| r.note.is_none() && r.path == e.path))
            .filter(|i| !self.shown.contains(i))
            .map(|i| (i as f32 - 2.0).max(0.0) * look.row_h);
        // a revealed file: scroll once its row is listed (until then its folders are listing)
        if let Some(p) = self.pending_reveal.clone()
            && let Some(i) = self.rows.iter().position(|r| r.note.is_none() && r.path == p)
        {
            self.pending_reveal = None;
            // fully visible rows only: the first and last drawn rows may be cut off
            let inside = self.shown.start < i && i + 1 < self.shown.end;
            if scroll_to.is_none() && !inside {
                scroll_to = Some((i as f32 - 2.0).max(0.0) * look.row_h);
            }
        }
        // an item outside the mounted folder (File → Rename of such a document): its editor
        // sits above the tree
        if let Some(e) = edit.as_deref_mut().filter(|e| !e.path.starts_with(&self.root)) {
            let kind = if e.path.is_dir() {
                RowKind::Dir { open: false }
            } else {
                RowKind::File {
                    markdown: markise_engine::tree::is_markdown(&e.path),
                }
            };
            end = look.edit_row(ui, 0, kind, e);
        }
        let mut toggle = None;
        let mut shown = 0..0;
        let out = virtual_rows(ui, "markise-tree", look.row_h, n, scroll_to, |ui, range| {
            shown = range.clone();
            for r in &self.rows[range] {
                if let Some(t) = &r.note {
                    look.row(ui, r.depth, RowKind::Note, t, false);
                    continue;
                }
                let kind = if r.is_dir {
                    RowKind::Dir { open: r.expanded }
                } else {
                    RowKind::File {
                        markdown: markise_engine::tree::is_markdown(&r.path),
                    }
                };
                if let Some(e) = edit.as_deref_mut().filter(|e| e.path == r.path) {
                    if let Some(x) = look.edit_row(ui, r.depth, kind, e) {
                        end = Some(x);
                    }
                    continue;
                }
                if r.is_dir {
                    let resp = look.row(ui, r.depth, kind, &r.name, false);
                    if resp.clicked() {
                        toggle = Some(r.path.clone());
                    }
                    resp.context_menu(|ui| row_menu(ui, &r.path, true, false, &mut action));
                } else {
                    let selected = current.is_some_and(|c| c == r.path);
                    let resp = look.row(ui, r.depth, kind, &r.name, selected);
                    if resp.clicked() {
                        action = Some(TreeAction::Open(r.path.clone()));
                    }
                    resp.context_menu(|ui| row_menu(ui, &r.path, false, false, &mut action));
                }
            }
        });
        self.shown = shown;
        // the empty space below the rows: the mounted folder's menu
        let below = egui::Rect::from_min_max(
            egui::pos2(out.inner_rect.left(), out.inner_rect.top() + out.content_size.y),
            out.inner_rect.max,
        );
        if below.height() > 0.0 {
            ui.interact(below, ui.scope_id().with("markise-tree-empty"), egui::Sense::click())
                .context_menu(|ui| row_menu(ui, &root, true, true, &mut action));
        }
        if let Some(d) = toggle {
            self.toggle(&d);
        }
        end.map(TreeAction::EndRename).into_iter().chain(action).collect()
    }
}

/// What a sidebar row shows in front of its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Dir {
        open: bool,
    },
    File {
        markdown: bool,
    },
    /// Outline heading of this level (1-based, relative to the shallowest).
    Heading {
        level: u8,
    },
    /// A placeholder line ("…", errors).
    Note,
}

/// Resolved sidebar style: the theme's sidebar slots with UI defaults from the chrome colors.
#[derive(Clone, Debug)]
pub struct SideLook {
    pub font: egui::FontId,
    pub bold_font: egui::FontId,
    pub text: Color32,
    pub active_text: Color32,
    pub active_bg: Color32,
    pub hover_text: Color32,
    pub hover_bg: Color32,
    pub outline_active_text: Color32,
    pub outline_active_bg: Color32,
    pub icon: Color32,
    pub row_h: f32,
    pub pad_x: f32,
    pub indent: f32,
    pub radius: f32,
}

fn c32(c: markise_theme::Rgba) -> Color32 {
    markise_layout::block::color(c)
}

impl SideLook {
    pub fn new(t: &markise_theme::Theme) -> SideLook {
        let s = &t.sidebar;
        let ch = &t.chrome;
        let size = s.font_size.unwrap_or(13.5);
        let (fam, bold_fam) = (markise_layout::FAMILY_SIDEBAR, markise_layout::FAMILY_SIDEBAR_BOLD);
        let bold = s.bold.unwrap_or(false);
        let font = egui::FontId::new(size, egui::FontFamily::Name((if bold { bold_fam } else { fam }).into()));
        let bold_font = egui::FontId::new(size, egui::FontFamily::Name(bold_fam.into()));
        let text = s.text.map_or(c32(ch.control_text), c32);
        let active_bg = s.active_bg.map_or(c32(ch.active_bg), c32);
        let blend = |a: Color32, b: Color32, f: f32| a.lerp_to_gamma(b, f);
        SideLook {
            font,
            bold_font,
            text,
            active_text: s.active_text.map_or(c32(t.text), c32),
            active_bg,
            hover_text: s.hover_text.map_or(text, c32),
            hover_bg: s.hover_bg.map_or(blend(c32(ch.sidebar_bg), active_bg, 0.5), c32),
            outline_active_text: s.outline_active_text.map_or(c32(ch.accent), c32),
            outline_active_bg: s.outline_active_bg.map_or(Color32::TRANSPARENT, c32),
            icon: s.icon.map_or(blend(text, c32(ch.sidebar_bg), 0.35), c32),
            row_h: s.row_height.unwrap_or(24.0).max(size + 6.0),
            pad_x: s.padding_x.unwrap_or(6.0),
            indent: s.indent.unwrap_or(14.0),
            radius: s.radius.unwrap_or(4.0),
        }
    }

    /// One full-width row: background (active / hover), chevron, icon or level marker, label.
    pub fn row(&self, ui: &mut Ui, depth: usize, kind: RowKind, label: &str, active: bool) -> egui::Response {
        let w = ui.available_width();
        let sense = if kind == RowKind::Note {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        };
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, self.row_h), sense);
        // accessibility (and UI tests): the row is a labelled button
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), active, label));
        let p = ui.painter_at(rect);
        let outline = matches!(kind, RowKind::Heading { .. });
        let hovered = resp.hovered() && kind != RowKind::Note;
        let bg = if active {
            if outline {
                self.outline_active_bg
            } else {
                self.active_bg
            }
        } else if hovered {
            self.hover_bg
        } else {
            Color32::TRANSPARENT
        };
        if bg.a() > 0 {
            p.rect_filled(rect.shrink2(egui::vec2(1.0, 1.0)), self.radius, bg);
        }
        let color = match (active, outline, hovered) {
            (true, true, _) => self.outline_active_text,
            (true, false, _) => self.active_text,
            (false, _, true) => self.hover_text,
            _ => self.text,
        };
        let x = self.lead(&p, rect, depth, kind, active);
        let cy = rect.center().y;
        let font = self.font.clone();
        let color = match kind {
            RowKind::Note => self.icon,
            // top-level headings stand out by color (the UI font may have no bold face)
            RowKind::Heading { level: 1 } if !active && !hovered => self.active_text,
            _ => color,
        };
        let galley = ui.painter().layout(label.to_string(), font, color, f32::INFINITY);
        let gpos = egui::pos2(x, cy - galley.size().y * 0.5);
        let clip = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(rect.right() - 4.0, rect.bottom()));
        ui.painter_at(clip).galley(gpos, galley, color);
        resp.on_hover_text_at_pointer(label)
    }

    /// Paints what a row shows in front of its label; returns where the label starts.
    fn lead(&self, p: &egui::Painter, rect: egui::Rect, depth: usize, kind: RowKind, active: bool) -> f32 {
        let cy = rect.center().y;
        let mut x = rect.left() + self.pad_x + depth as f32 * self.indent;
        let icon_s = (self.row_h * 0.62).clamp(12.0, 18.0);
        match kind {
            RowKind::Dir { open } => {
                // disclosure chevron
                let c = egui::pos2(x + 4.0, cy);
                let pts = if open {
                    vec![
                        c + egui::vec2(-3.5, -1.8),
                        c + egui::vec2(0.0, 2.2),
                        c + egui::vec2(3.5, -1.8),
                    ]
                } else {
                    vec![
                        c + egui::vec2(-1.8, -3.5),
                        c + egui::vec2(2.2, 0.0),
                        c + egui::vec2(-1.8, 3.5),
                    ]
                };
                p.add(egui::Shape::line(pts, egui::Stroke::new(1.4, self.icon)));
                x += 12.0;
                let r = egui::Rect::from_center_size(egui::pos2(x + icon_s * 0.5, cy), egui::vec2(icon_s, icon_s));
                let icon = if open {
                    crate::icons::Icon::FolderOpen
                } else {
                    crate::icons::Icon::Folder
                };
                crate::icons::paint(p, r, icon, self.icon);
                x += icon_s + 6.0;
            }
            RowKind::File { markdown } => {
                x += 12.0;
                let r = egui::Rect::from_center_size(egui::pos2(x + icon_s * 0.5, cy), egui::vec2(icon_s, icon_s));
                let icon = if markdown {
                    crate::icons::Icon::FileText
                } else {
                    crate::icons::Icon::File
                };
                crate::icons::paint(p, r, icon, if active { self.active_text } else { self.icon });
                x += icon_s + 6.0;
            }
            RowKind::Heading { level } => {
                // subtle level marker: a short tick, longer for deeper levels
                if level > 1 {
                    let len = 3.0 + level.min(4) as f32;
                    p.line_segment(
                        [egui::pos2(x, cy), egui::pos2(x + len, cy)],
                        egui::Stroke::new(
                            1.2,
                            if active {
                                self.outline_active_text
                            } else {
                                self.icon.gamma_multiply(0.6)
                            },
                        ),
                    );
                    x += len + 5.0;
                }
            }
            RowKind::Note => {
                x += 12.0;
            }
        }
        x
    }

    /// The row of the inline name editor: the row's icon and a text field with the name;
    /// a refused name is shown under the row. Returns how editing finished, if it did.
    pub fn edit_row(&self, ui: &mut Ui, depth: usize, kind: RowKind, edit: &mut InlineEdit) -> Option<EditEnd> {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, self.row_h), egui::Sense::hover());
        let x = self.lead(&ui.painter_at(rect), rect, depth, kind, false);
        let field = egui::Rect::from_min_max(
            egui::pos2(x - 4.0, rect.top() + 1.0),
            egui::pos2(rect.right() - 4.0, rect.bottom() - 1.0),
        );
        let id = rename_field_id();
        // a child Ui: the field must not move the next row (fixed row pitch)
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let out = egui::TextEdit::singleline(&mut edit.name)
            .id(id)
            .font(self.font.clone())
            .margin(egui::Margin::symmetric(3, 1))
            .desired_width(field.width() - 6.0)
            .show(&mut child);
        if edit.select {
            edit.select = false;
            out.response.request_focus();
            out.response.scroll_to_me(None);
            let mut st = out.state.clone();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(edit.stem_chars()),
            )));
            st.store(ui.ctx(), id);
        }
        if edit.refocus {
            edit.refocus = false;
            // never take the focus from another widget
            if ui.memory(|m| m.focused()).is_none_or(|f| f == id) {
                out.response.request_focus();
            }
        }
        edit.drawn = true;
        if out.response.has_focus() {
            edit.had_focus = true;
        }
        if out.response.changed() {
            edit.error = None;
        }
        if let Some(e) = &edit.error {
            let dark = ui.visuals().dark_mode;
            let (bg, fg) = if dark {
                (Color32::from_rgb(0x5c, 0x1f, 0x1c), Color32::from_rgb(0xff, 0xd9, 0xd6))
            } else {
                (Color32::from_rgb(0xfd, 0xec, 0xea), Color32::from_rgb(0xb3, 0x26, 0x1e))
            };
            egui::Area::new(egui::Id::unique("markise-rename-error"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(field.left(), rect.bottom() + 1.0))
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(bg)
                        .stroke(egui::Stroke::new(1.0, fg.gamma_multiply(0.5)))
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            ui.set_max_width(field.width().max(160.0));
                            ui.add(egui::Label::new(RichText::new(e).color(fg).size(12.0)).wrap());
                        });
                });
        }
        if out.response.lost_focus() {
            let (esc, enter) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
            return Some(if esc {
                EditEnd::Cancel
            } else if enter {
                EditEnd::Commit
            } else {
                EditEnd::CommitOrCancel
            });
        }
        None
    }
}

/// A virtualised list of `n` rows of exactly `row_h` points. `show_rows` computes offsets
/// with the item spacing current when it is called, so the zero spacing the rows are painted
/// with is set in a scope *around* it.
fn virtual_rows(
    ui: &mut Ui,
    id: &str,
    row_h: f32,
    n: usize,
    scroll_to: Option<f32>,
    add: impl FnOnce(&mut Ui, std::ops::Range<usize>),
) -> egui::scroll_area::ScrollAreaOutput<()> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical();
        if let Some(y) = scroll_to {
            area = area.vertical_scroll_offset(y);
        }
        area.id_salt(id)
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_rows(ui, row_h, n, add)
    })
    .inner
}

pub fn outline(ui: &mut Ui, entries: &[OutlineEntry], current: Option<usize>, look: &SideLook) -> Option<usize> {
    if entries.is_empty() {
        ui.colored_label(look.icon, "No headings");
        return None;
    }
    let min_level = entries.iter().map(|e| e.level).min().unwrap_or(1);
    let mut clicked = None;
    virtual_rows(ui, "markise-outline", look.row_h, entries.len(), None, |ui, range| {
        for i in range {
            let e = &entries[i];
            let level = e.level - min_level + 1;
            let text = if e.text.is_empty() { "(empty heading)" } else { &e.text };
            let r = look.row(
                ui,
                (level - 1) as usize,
                RowKind::Heading { level },
                text,
                current == Some(i),
            );
            if r.clicked() {
                clicked = Some(e.item);
            }
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_rows_use_the_painted_row_pitch() {
        let ctx = egui::Context::default();
        let mut content_h = 0.0;
        let mut painted = Vec::new();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0; // a non-zero default must not leak in
                let out = virtual_rows(ui, "t", 24.0, 100, None, |ui, range| {
                    for _ in range {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 24.0), egui::Sense::hover());
                        painted.push(r.top());
                    }
                });
                content_h = out.content_size.y;
            });
        })
        .drop_without_applying_deltas();
        assert!((content_h - 2400.0).abs() < 0.5, "content height {content_h}");
        for w in painted.windows(2) {
            assert!((w[1] - w[0] - 24.0).abs() < 0.01);
        }
    }
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn settle(t: &mut FileTree, wake: &Wake) {
        let start = Instant::now();
        while t.dirs.values().any(|d| d.in_flight) {
            t.poll(wake);
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn change_bursts_are_coalesced_and_the_last_listing_wins() {
        // review item 15
        let d = std::env::temp_dir().join(format!("markise-tree-burst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let d = std::fs::canonicalize(&d).unwrap();
        let wake: Wake = Arc::new(|| {});
        let mut t = FileTree::new(d.clone());
        t.rows(&wake);
        settle(&mut t, &wake);
        assert_eq!(t.requests, 1);
        for i in 0..200 {
            std::fs::write(d.join(format!("f{i}.md")), "x").unwrap();
            t.invalidate(&d, &wake);
        }
        settle(&mut t, &wake);
        assert!(t.requests <= 3, "200 events → {} listings", t.requests);
        let files = t.rows(&wake).iter().filter(|r| !r.is_dir && r.note.is_none()).count();
        assert_eq!(files, 200, "the final listing reflects every file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rows_are_a_cached_flattened_model() {
        // review item 16
        let d = std::env::temp_dir().join(format!("markise-tree-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub").join("deep.md"), "x").unwrap();
        for i in 0..5_000 {
            std::fs::write(d.join(format!("n{i}.md")), "").unwrap();
        }
        let d = std::fs::canonicalize(&d).unwrap();
        let wake: Wake = Arc::new(|| {});
        let mut t = FileTree::new(d.clone());
        t.rows(&wake);
        settle(&mut t, &wake);
        assert_eq!(t.rows(&wake).len(), 5_001);
        assert_eq!(t.rows(&wake)[0].name, "sub", "folders first");
        assert_eq!(t.rows(&wake)[1].name, "n0.md");
        assert_eq!(t.rows(&wake)[3].name, "n2.md", "natural order");
        // rows are reused while nothing changes
        let p = t.rows(&wake).as_ptr();
        assert_eq!(t.rows(&wake).as_ptr(), p);
        t.toggle(&d.join("sub"));
        t.rows(&wake);
        settle(&mut t, &wake);
        let rows = t.rows(&wake);
        assert_eq!(rows[1].name, "deep.md");
        assert_eq!(rows[1].depth, 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
