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

    pub fn toggle(&mut self, dir: &Path) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rows_dirty = true;
    }

    /// Shows the tree; returns a file the user clicked.
    pub fn show(&mut self, ui: &mut Ui, current: Option<&Path>, wake: &Wake, look: &SideLook) -> Option<PathBuf> {
        let name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        ui.label(RichText::new(name).strong().color(look.active_text));
        let n = self.rows(wake).len();
        let mut clicked = None;
        let mut toggle = None;
        virtual_rows(ui, "markise-tree", look.row_h, n, |ui, range| {
            for r in &self.rows[range] {
                if let Some(t) = &r.note {
                    look.row(ui, r.depth, RowKind::Note, t, false);
                    continue;
                }
                if r.is_dir {
                    if look
                        .row(ui, r.depth, RowKind::Dir { open: r.expanded }, &r.name, false)
                        .clicked()
                    {
                        toggle = Some(r.path.clone());
                    }
                } else {
                    let selected = current.is_some_and(|c| c == r.path);
                    let md = markise_engine::tree::is_markdown(&r.path);
                    if look
                        .row(ui, r.depth, RowKind::File { markdown: md }, &r.name, selected)
                        .clicked()
                    {
                        clicked = Some(r.path.clone());
                    }
                }
            }
        });
        if let Some(d) = toggle {
            self.toggle(&d);
        }
        clicked
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
                crate::icons::paint(&p, r, icon, self.icon);
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
                crate::icons::paint(&p, r, icon, if active { self.active_text } else { self.icon });
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
}

/// A virtualised list of `n` rows of exactly `row_h` points. `show_rows` computes offsets
/// with the item spacing current when it is called, so the zero spacing the rows are painted
/// with is set in a scope *around* it.
fn virtual_rows(
    ui: &mut Ui,
    id: &str,
    row_h: f32,
    n: usize,
    add: impl FnOnce(&mut Ui, std::ops::Range<usize>),
) -> egui::scroll_area::ScrollAreaOutput<()> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical()
            .id_salt(id)
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
    virtual_rows(ui, "markise-outline", look.row_h, entries.len(), |ui, range| {
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
                let out = virtual_rows(ui, "t", 24.0, 100, |ui, range| {
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
