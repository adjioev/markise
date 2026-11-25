//! The Tags view of the sidebar (spec 06 V10.4, V10.5; ADR 0027): every tag of the opened
//! folder with its number of notes, a filter that also takes a search such as `tag:x` or `#x`,
//! and the notes of a chosen tag. The list comes from the vault index ([`TagIndex`]), built in
//! the background; nothing here reads files.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{RichText, Ui};
use markise_engine::vault::tags::{TagIndex, query_tag};

use super::MarkiseApp;
use crate::sidebar::{RowKind, SideLook, Tab, virtual_rows};

/// State of the Tags view.
#[derive(Default)]
pub(crate) struct TagsView {
    /// The filter / search field.
    pub query: String,
    /// The tag whose notes are listed (as its key is written by the user or the index).
    pub selected: Option<String>,
    /// Focus the filter field next frame.
    focus: bool,
}

/// A row of the tag list.
struct TagRow {
    name: String,
    leaf: String,
    depth: usize,
    count: usize,
}

/// The tags `query` keeps (all when it is empty): by part of the name, `tag:` and `#` removed.
fn filtered(tags: &TagIndex, query: &str) -> Vec<TagRow> {
    let q = query_tag(query).map(markise_md::tags::fold);
    tags.entries()
        .iter()
        .filter(|e| {
            q.as_ref()
                .is_none_or(|q| markise_md::tags::fold(&e.name).contains(q.as_str()))
        })
        .map(|e| TagRow {
            // filtered: the full name (parents may be hidden); else the tree's last segment
            leaf: if q.is_some() {
                e.name.clone()
            } else {
                e.leaf().to_string()
            },
            depth: if q.is_some() { 0 } else { e.depth },
            name: e.name.clone(),
            count: e.notes.len(),
        })
        .collect()
}

impl MarkiseApp {
    /// Shows the notes with tag `name` in the sidebar's Tags view (a click on a tag in the
    /// document, a search).
    pub fn show_tag(&mut self, name: &str) {
        let name = name.strip_prefix('#').unwrap_or(name).trim_end_matches('/');
        self.tags_view.selected = Some(name.to_string());
        self.tags_view.query.clear();
        self.tab = Tab::Tags;
        self.sidebar_visible = true;
    }

    /// The Tags view (tests): the selected tag and the filter text.
    pub fn tags_view_state(&self) -> (Option<&str>, &str) {
        (self.tags_view.selected.as_deref(), &self.tags_view.query)
    }

    /// The sidebar's Tags view.
    pub(super) fn tags_panel(&mut self, ui: &mut Ui, look: &SideLook) {
        ui.label(RichText::new("Tags").font(look.bold_font.clone()).color(look.text));
        let Some(ix) = self.vault_index().cloned() else {
            ui.weak(if self.tree.is_some() {
                "Indexing the folder…"
            } else {
                "No folder open"
            });
            return;
        };
        if !self.parse.tags {
            ui.weak("#tags in text are off (Preferences → Markdown); front-matter tags are listed.");
        }
        let tags = ix.tags().clone();
        ui.add_space(4.0);
        let field = egui::TextEdit::singleline(&mut self.tags_view.query)
            .hint_text("Filter, or tag:name")
            .desired_width(f32::INFINITY);
        let resp = ui.add(field);
        if std::mem::take(&mut self.tags_view.focus) {
            resp.request_focus();
        }
        if resp.changed() {
            self.tags_view.selected = None;
        }
        let rows = filtered(&tags, &self.tags_view.query);
        // Enter: the notes of the tag the search names, else of the first match
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let exact = query_tag(&self.tags_view.query)
                .and_then(|q| tags.get(q))
                .map(|e| e.name.clone());
            if let Some(name) = exact.or_else(|| rows.first().map(|r| r.name.clone())) {
                self.tags_view.selected = Some(name);
            }
        }
        ui.add_space(2.0);
        if !tags.is_complete() {
            ui.label(
                RichText::new("Reading notes…")
                    .font(look.small_font.clone())
                    .color(look.muted),
            );
        }
        match self.tags_view.selected.clone() {
            Some(sel) => self.tag_notes(ui, look, &ix, &tags, &sel),
            None => self.tag_list(ui, look, rows, tags.is_empty()),
        }
    }

    fn tag_list(&mut self, ui: &mut Ui, look: &SideLook, rows: Vec<TagRow>, none: bool) {
        if rows.is_empty() {
            let note = if none {
                "No tags in this folder"
            } else {
                "No matching tags"
            };
            ui.label(RichText::new(note).font(look.font.clone()).color(look.icon));
            return;
        }
        let mut picked = None;
        virtual_rows(ui, "markise-tags", look.row_h, rows.len(), None, |ui, range| {
            for r in &rows[range] {
                let full = format!("#{} ({} note{})", r.name, r.count, if r.count == 1 { "" } else { "s" });
                let resp = look.row_named(ui, r.depth, RowKind::File { markdown: true }, &r.leaf, &full, false);
                count_badge(ui, look, resp.rect, r.count);
                if resp.clicked() {
                    picked = Some(r.name.clone());
                }
            }
        });
        if let Some(p) = picked {
            self.tags_view.selected = Some(p);
        }
    }

    fn tag_notes(
        &mut self,
        ui: &mut Ui,
        look: &SideLook,
        ix: &Arc<markise_engine::vault::VaultIndex>,
        tags: &TagIndex,
        sel: &str,
    ) {
        if ui.small_button("← All tags").clicked() {
            self.tags_view.selected = None;
            return;
        }
        let (name, notes) = match tags.get(sel) {
            Some(e) => (e.name.clone(), e.notes.clone()),
            None => (sel.to_string(), Vec::new()),
        };
        ui.label(
            RichText::new(format!(
                "#{name} · {} note{}",
                notes.len(),
                if notes.len() == 1 { "" } else { "s" }
            ))
            .font(look.small_font.clone())
            .color(look.muted),
        );
        if notes.is_empty() {
            let note = if tags.is_complete() {
                "No notes have this tag"
            } else {
                "Reading notes…"
            };
            ui.label(RichText::new(note).font(look.font.clone()).color(look.icon));
            return;
        }
        let current = self.current_rel();
        let mut open: Option<PathBuf> = None;
        virtual_rows(ui, "markise-tag-notes", look.row_h, notes.len(), None, |ui, range| {
            for rel in &notes[range] {
                let f = ix.file(rel);
                let stem = f.map_or(rel.as_str(), |f| f.stem());
                let resp = look.row_named(
                    ui,
                    0,
                    RowKind::File { markdown: true },
                    stem,
                    rel,
                    current.as_deref() == Some(rel.as_str()),
                );
                if resp.clicked() {
                    open = Some(ix.abs(rel));
                }
            }
        });
        if let Some(p) = open {
            self.open_known_file(p);
        }
    }
}

/// The number of notes at the right end of a tag row, muted.
fn count_badge(ui: &Ui, look: &SideLook, rect: egui::Rect, count: usize) {
    let p = ui.painter_at(rect);
    p.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        look.font.clone(),
        look.muted,
    );
}
