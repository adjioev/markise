//! What the block library offers (ADR 0022): a package's presets in its groups and order, with what
//! a query looks at. Plain data: this crate draws nothing and keeps no words of its own.

use std::collections::BTreeMap;

use crate::index::PackageIndex;
use crate::sections::SectionConfig;
use crate::value::Localized;

/// What the library's item for a section says, when the package does not (ADR 0026).
pub const DEFAULT_SECTION_LABEL: &str = "Section";

/// What a row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryItem {
    /// A preset, by its place in [`PackageIndex::presets`].
    Preset(usize),
    /// A new section of a document that has sections, which is no block and no preset (ADR 0026).
    Section,
}

/// One row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryEntry {
    pub item: LibraryItem,
    /// The block's name, empty for a section.
    pub block: String,
    pub title: String,
    pub detail: Option<String>,
    /// The icon's file in the package, if the block names one: `assets/<name>.svg` (ADR 0019). It may
    /// not be there.
    pub icon: Option<String>,
    /// The group's place in [`Library::groups`]; `None` for a block in no group.
    pub group: Option<usize>,
    /// Folded words of every language, for [`Library::filter`].
    search: String,
}

/// A package's presets, ready to be shown and filtered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    groups: Vec<String>,
    entries: Vec<LibraryEntry>,
}

/// Lowercase, and `ё` as `е`, since writers of Russian use the two as one letter.
fn fold(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

fn push_all(search: &mut String, text: Option<&Localized>) {
    match text {
        Some(Localized::Plain(s)) => {
            search.push_str(s);
            search.push('\n');
        }
        Some(Localized::ByLanguage(m)) => {
            for s in m.values() {
                search.push_str(s);
                search.push('\n');
            }
        }
        None => {}
    }
}

impl PackageIndex {
    /// The library in the package's order: its groups, the blocks of each, the presets of each. A
    /// block in no group comes last, so that a preset cannot be out of reach. Text shows in `language`
    /// and a query matches every language the package has. A block has one place, the first it is
    /// listed at, so there is at most one row per preset, whatever the groups say.
    pub fn library(&self, language: &str) -> Library {
        let mut library = Library::default();
        let mut by_block: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, preset) in self.presets().iter().enumerate() {
            by_block.entry(preset.block.as_str()).or_default().push(i);
        }
        let mut rest = by_block.clone();
        for (g, group) in self.groups().iter().enumerate() {
            let label = group.label.as_ref().map_or(group.id.as_str(), |l| l.get(language));
            library.groups.push(label.to_string());
            for block in &group.blocks {
                for i in rest.remove(block.as_str()).unwrap_or_default() {
                    self.push_preset(&mut library, i, Some(g), language);
                }
            }
        }
        // the palette's order, not the blocks'
        let mut ungrouped: Vec<usize> = rest.into_values().flatten().collect();
        ungrouped.sort_unstable();
        for i in ungrouped {
            self.push_preset(&mut library, i, None, language);
        }
        library
    }

    fn push_preset(&self, library: &mut Library, at: usize, group: Option<usize>, language: &str) {
        let Some(preset) = self.presets().get(at) else { return };
        let block = preset.block.as_str();
        let Some(entry) = self.block(block) else { return };
        let block_label = entry.schema.label.as_ref();
        let description = entry.palette.and_then(|p| p.description.as_ref());
        let shown = |l: Option<&'_ Localized>| l.map(|l| l.get(language).to_string()).filter(|s| !s.is_empty());
        let name = shown(block_label).unwrap_or_else(|| block.to_string());
        let title = shown(preset.label.as_ref()).unwrap_or_else(|| name.clone());
        // rows of one block differ by their titles: say whose they are
        let several = self.presets_of(block).nth(1).is_some();
        let detail = match (several && title != name, shown(description)) {
            (true, Some(about)) => Some(format!("{name} · {about}")),
            (true, None) => Some(name),
            (false, about) => about,
        };
        let mut search = String::new();
        push_all(&mut search, preset.label.as_ref());
        push_all(&mut search, block_label);
        push_all(&mut search, description);
        search.push_str(block);
        search.push('\n');
        search.push_str(preset.keywords.as_deref().unwrap_or(""));
        library.entries.push(LibraryEntry {
            item: LibraryItem::Preset(at),
            block: block.to_string(),
            title,
            detail,
            icon: (entry.palette.and_then(|p| p.icon.as_deref()))
                .or_else(|| entry.view.and_then(|v| v.icon.as_deref()))
                .map(|n| format!("assets/{n}.svg")),
            group,
            search: fold(&search),
        });
    }
}

impl Library {
    /// Adds the item for a new section, first and in no group: it is no block of any group, and a list
    /// that scrolls does not hide it. A document whose dialect has sections wants it; none other does.
    pub fn add_section(&mut self, config: Option<&SectionConfig>, language: &str) {
        let shown = |l: Option<&'_ Localized>| l.map(|l| l.get(language).to_string()).filter(|s| !s.is_empty());
        let label = config.and_then(|c| c.label.as_ref());
        let description = config.and_then(|c| c.description.as_ref());
        let mut search = String::new();
        push_all(&mut search, label);
        push_all(&mut search, description);
        search.push_str(DEFAULT_SECTION_LABEL);
        search.push('\n');
        search.push_str(config.and_then(|c| c.keywords.as_deref()).unwrap_or(""));
        self.entries.insert(
            0,
            LibraryEntry {
                item: LibraryItem::Section,
                block: String::new(),
                title: shown(label).unwrap_or_else(|| DEFAULT_SECTION_LABEL.to_string()),
                detail: shown(description),
                icon: config.and_then(|c| c.icon.as_ref()).map(|n| format!("assets/{n}.svg")),
                group: None,
                search: fold(&search),
            },
        );
    }

    /// The groups' labels; [`LibraryEntry::group`] points here.
    pub fn groups(&self) -> &[String] {
        &self.groups
    }

    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The places of the entries with every word of `query`, each anywhere in a label, a
    /// description, a keyword or the block's name, in the package's order.
    pub fn filter(&self, query: &str) -> Vec<usize> {
        let query = fold(query);
        let words: Vec<&str> = query.split_whitespace().collect();
        (self.entries.iter().enumerate())
            .filter(|(_, e)| words.iter().all(|w| e.search.contains(w)))
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// What validation refuses cannot make rows either, should an index ever come another way.
    #[test]
    fn a_block_listed_many_times_has_one_place() {
        let manifest = json!({"format": crate::FORMAT, "library": "s", "version": "1", "blocks": ["b"]});
        let schema = json!({"fields": [{"name": "t", "type": "text"}]});
        let palette = json!({
            "groups": [{"id": "g", "blocks": ["b"]}, {"id": "h", "blocks": []}],
            "presets": [
                {"id": "p", "block": "b", "markdown": ":::b\n\nx\n\n:::\n"},
                {"id": "q", "block": "b", "markdown": ":::b\n\ny\n\n:::\n"}
            ]
        });
        let files = [
            ("manifest.json", manifest),
            ("components/b/b.json", schema),
            ("editor/palette.json", palette),
        ]
        .map(|(path, v)| (path.to_string(), v.to_string().into_bytes()));
        let mut index = PackageIndex::from_files(&files.into_iter().collect()).unwrap();
        for g in index.groups_for_test() {
            g.blocks = vec!["b".to_string(); 10_000];
        }
        let library = index.library("en");
        assert_eq!(library.entries().len(), 2, "one row per preset");
        assert!(library.entries().iter().all(|e| e.group == Some(0)));
    }
}
