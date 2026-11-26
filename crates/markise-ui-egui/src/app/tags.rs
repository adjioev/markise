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
    /// The rows of the last filter: the index generation and query it was computed for, and
    /// the positions of the kept tags in [`TagIndex::entries`] (recomputed only when either
    /// changes, not per frame).
    rows: Option<(u64, String, Arc<[usize]>)>,
}

impl TagsView {
    /// The tags the query keeps, from the cache when the index and the query are unchanged.
    fn rows(&mut self, generation: u64, tags: &TagIndex) -> Arc<[usize]> {
        match &self.rows {
            Some((g, q, rows)) if *g == generation && *q == self.query => rows.clone(),
            _ => {
                let rows: Arc<[usize]> = filtered(tags, &self.query).into();
                self.rows = Some((generation, self.query.clone(), rows.clone()));
                rows
            }
        }
    }
}

/// The positions in [`TagIndex::entries`] of the tags `query` keeps (all when it names none):
/// by part of the name, `tag:` and `#` removed.
fn filtered(tags: &TagIndex, query: &str) -> Vec<usize> {
    let q = query_tag(query).map(markise_md::tags::fold);
    tags.entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            q.as_ref()
                .is_none_or(|q| markise_md::tags::fold(&e.name).contains(q.as_str()))
        })
        .map(|(i, _)| i)
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
        let query = &mut self.tags_view.query;
        let resp = ui
            .horizontal(|ui| {
                let l = ui.label(RichText::new("Find").font(look.font.clone()).color(look.muted));
                let field = egui::TextEdit::singleline(query)
                    .id_salt("markise-tags-search")
                    .hint_text("name, #name or tag:name")
                    .desired_width(f32::INFINITY);
                ui.add(field).labelled_by(l.id)
            })
            .inner;
        if resp.changed() {
            self.tags_view.selected = None;
        }
        let rows = self.tags_view.rows(ix.generation(), &tags);
        let filtering = query_tag(&self.tags_view.query).is_some();
        // Enter: the notes of the tag the search names, else of the first match
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let exact = query_tag(&self.tags_view.query)
                .and_then(|q| tags.get(q))
                .map(|e| e.name.clone());
            let first = || rows.first().map(|&i| tags.entries()[i].name.clone());
            if let Some(name) = exact.or_else(first) {
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
            None => self.tag_list(ui, look, &tags, &rows, filtering),
        }
    }

    /// `rows`: positions in `tags.entries()`; `filtering`: a query narrows the list (full names,
    /// no tree indentation, since parents may be left out).
    fn tag_list(&mut self, ui: &mut Ui, look: &SideLook, tags: &TagIndex, rows: &[usize], filtering: bool) {
        if rows.is_empty() {
            let note = if tags.is_empty() {
                "No tags in this folder"
            } else {
                "No matching tags"
            };
            ui.label(RichText::new(note).font(look.font.clone()).color(look.icon));
            return;
        }
        let mut picked = None;
        virtual_rows(ui, "markise-tags", look.row_h, rows.len(), None, |ui, range| {
            // only the visible rows are built
            for &i in &rows[range] {
                let e = &tags.entries()[i];
                let count = e.notes.len();
                let full = format!("#{} ({count} note{})", e.name, if count == 1 { "" } else { "s" });
                let (shown, depth) = if filtering {
                    (e.name.as_str(), 0)
                } else {
                    (e.leaf(), e.depth)
                };
                let resp = look.row_named(ui, depth, RowKind::File { markdown: true }, shown, &full, false);
                count_badge(ui, look, resp.rect, count);
                if resp.clicked() {
                    picked = Some(e.name.clone());
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
        // borrowed from the index: nothing is copied per frame
        let (name, notes): (&str, &[String]) = match tags.get(sel) {
            Some(e) => (&e.name, &e.notes),
            None => (sel, &[]),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Review of #270: the filter runs when the index or the query changes, not per frame.
    #[test]
    fn filter_rows_are_cached_by_generation_and_query() {
        let notes: BTreeMap<String, Arc<[String]>> = [
            (
                "a.md".to_string(),
                vec!["idea".to_string(), "area/x".to_string()].into(),
            ),
            ("b.md".to_string(), vec!["Idea".to_string()].into()),
        ]
        .into_iter()
        .collect();
        let tags = TagIndex::build(notes, true);
        let mut v = TagsView::default();
        let all = v.rows(1, &tags);
        assert_eq!(all.len(), 3);
        assert!(Arc::ptr_eq(&all, &v.rows(1, &tags)), "same frame inputs: cached");
        v.query = "tag:#ide".into();
        let some = v.rows(1, &tags);
        assert_eq!(
            some.iter()
                .map(|&i| tags.entries()[i].name.as_str())
                .collect::<Vec<_>>(),
            ["idea"]
        );
        assert!(!Arc::ptr_eq(&some, &v.rows(2, &tags)), "a new index: computed again");
    }
}
