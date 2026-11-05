//! What the block library offers (ADR 0022): a package's presets in its groups and order, with what
//! a query looks at. Plain data: this crate draws nothing and keeps no words of its own.

use crate::index::PackageIndex;
use crate::palette::Preset;
use crate::value::Localized;

/// One row: a preset of the package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryEntry {
    /// The preset's place in [`PackageIndex::presets`].
    pub preset: usize,
    /// The block's name.
    pub block: String,
    pub title: String,
    pub detail: Option<String>,
    /// An icon name; the package supplies what it means.
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
    /// and a query matches every language the package has.
    pub fn library(&self, language: &str) -> Library {
        let mut library = Library::default();
        for (g, group) in self.groups().iter().enumerate() {
            let label = group.label.as_ref().map_or(group.id.as_str(), |l| l.get(language));
            library.groups.push(label.to_string());
            for block in &group.blocks {
                self.push_block(&mut library, block, Some(g), language);
            }
        }
        let grouped = |name: &str| self.groups().iter().any(|g| g.blocks.iter().any(|b| b == name));
        for (i, preset) in self.presets().iter().enumerate() {
            if !grouped(&preset.block) {
                self.push_preset(&mut library, i, preset, None, language);
            }
        }
        library
    }

    fn push_block(&self, library: &mut Library, block: &str, group: Option<usize>, language: &str) {
        for (i, preset) in self.presets().iter().enumerate() {
            if preset.block == block {
                self.push_preset(library, i, preset, group, language);
            }
        }
    }

    fn push_preset(&self, library: &mut Library, at: usize, preset: &Preset, group: Option<usize>, language: &str) {
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
            preset: at,
            block: block.to_string(),
            title,
            detail,
            icon: entry.palette.and_then(|p| p.icon.clone()),
            group,
            search: fold(&search),
        });
    }
}

impl Library {
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
