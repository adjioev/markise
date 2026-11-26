//! The tags of a vault (spec 06 §6, §10; ADR 0027): which notes carry which tags.
//!
//! The scanner reads each note once in the background ([`read_note_tags`]) and keeps the
//! answers per note; a [`TagIndex`] is the immutable summary a [`super::VaultIndex`] snapshot
//! carries for the UI: every tag with the notes that have it, nested tags included in their
//! parents (`#a/b` counts for `a` too, V10.3). Tags compare without regard to case; a tag is
//! shown with the first casing seen in vault-path order.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

/// Notes larger than this are not read for tags (a tag index is no reason to read huge files).
pub const MAX_NOTE_BYTES: u64 = 4 << 20;

/// One tag of the vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEntry {
    /// The full tag as first written in the vault (`Area/health`), without `#`.
    pub name: String,
    /// Number of `/`-separated segments minus one (0 for a top-level tag).
    pub depth: usize,
    /// Vault paths of the notes with this tag or one nested below it, in path order.
    pub notes: Vec<String>,
}

impl TagEntry {
    /// The last segment of the tag (`health` of `area/health`).
    pub fn leaf(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }
}

/// Every tag of a vault snapshot.
#[derive(Clone, Debug, Default)]
pub struct TagIndex {
    /// Sorted by segments (`a`, `a/b`, `a/b/c`, `a-c`, `b`), compared case-insensitively.
    entries: Vec<TagEntry>,
    by_key: HashMap<String, usize>,
    /// Tags per note (vault path → tags as written in the note, front matter first).
    by_note: BTreeMap<String, Arc<[String]>>,
    /// Every note has been read (`false` while the first reading is still running).
    complete: bool,
}

/// The tag a search query names: `tag:x`, `tag:#x`, `#x` or `x` (any letter case), without a
/// trailing `/`; `None` when it names none.
pub fn query_tag(q: &str) -> Option<&str> {
    let q = q.trim();
    let q = match q.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("tag:") => q[4..].trim_start(),
        _ => q,
    };
    let q = q.strip_prefix('#').unwrap_or(q).trim_end_matches('/');
    (!q.is_empty() && !q.contains(char::is_whitespace)).then_some(q)
}

impl TagIndex {
    /// The index of `notes` (vault path → its tags). `complete`: every note was read.
    pub fn build(notes: BTreeMap<String, Arc<[String]>>, complete: bool) -> TagIndex {
        let mut entries: Vec<TagEntry> = Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for (rel, tags) in &notes {
            for t in tags.iter() {
                // the tag and each parent it implies
                let mut end = 0;
                for (depth, seg) in t.split('/').enumerate() {
                    end += seg.len() + usize::from(depth > 0);
                    let name = &t[..end];
                    let key = markise_md::tags::fold(name);
                    let i = *by_key.entry(key).or_insert_with(|| {
                        entries.push(TagEntry {
                            name: name.to_string(),
                            depth,
                            notes: Vec::new(),
                        });
                        entries.len() - 1
                    });
                    let e = &mut entries[i];
                    if e.notes.last() != Some(rel) {
                        e.notes.push(rel.clone());
                    }
                }
            }
        }
        entries.sort_by_cached_key(|e| {
            markise_md::tags::fold(&e.name)
                .split('/')
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
        let by_key = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (markise_md::tags::fold(&e.name), i))
            .collect();
        TagIndex {
            entries,
            by_key,
            by_note: notes,
            complete,
        }
    }

    /// Every tag, parents before the tags nested in them, otherwise by name.
    pub fn entries(&self) -> &[TagEntry] {
        &self.entries
    }

    /// The tag `name` (without `#`, any letter case).
    pub fn get(&self, name: &str) -> Option<&TagEntry> {
        let name = name.strip_prefix('#').unwrap_or(name).trim_end_matches('/');
        self.by_key
            .get(&markise_md::tags::fold(name))
            .map(|&i| &self.entries[i])
    }

    /// The notes a search query finds (`tag:x`, `#x` or `x`): those with tag `x` or a tag
    /// nested below it (V12.2: `x/y`, never `y/x`), in path order. Empty when no note has it.
    pub fn search(&self, query: &str) -> &[String] {
        query_tag(query)
            .and_then(|t| self.get(t))
            .map_or(&[], |e| e.notes.as_slice())
    }

    /// The tags of the note at vault path `rel`, as written in it.
    pub fn tags_of(&self, rel: &str) -> &[String] {
        self.by_note.get(rel).map_or(&[], |t| t)
    }

    /// Every note has been read for tags.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Reads the tags of the note at `path` (file I/O: worker threads only). Not a regular file,
/// unreadable or larger than [`MAX_NOTE_BYTES`]: no tags. The text is parsed with the default
/// Markdown options, so the index finds tags where the editor shows them.
pub fn read_note_tags(path: &Path) -> Vec<String> {
    let Ok(m) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if !m.is_file() || m.len() > MAX_NOTE_BYTES {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    note_tags(&String::from_utf8_lossy(&bytes))
}

/// The tags of a note's text (front matter, then `#tags`).
pub fn note_tags(text: &str) -> Vec<String> {
    let body = text.trim_start_matches('\u{FEFF}');
    // nothing to find: no `#` and no front matter
    if !text.contains('#') && !body.starts_with("---") {
        return Vec::new();
    }
    let doc = markise_md::Document::parse(text, markise_md::ParseOptions::default());
    markise_md::tags::document_tags(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(notes: &[(&str, &[&str])]) -> TagIndex {
        TagIndex::build(
            notes
                .iter()
                .map(|(r, t)| {
                    (
                        r.to_string(),
                        t.iter().map(|s| s.to_string()).collect::<Arc<[String]>>(),
                    )
                })
                .collect(),
            true,
        )
    }

    #[test]
    fn nested_tags_count_for_their_parents() {
        let ix = index(&[
            ("a.md", &["area/health/sleep"]),
            ("b.md", &["area", "Idea"]),
            ("c/d.md", &["idea", "x/area"]),
        ]);
        let names: Vec<&str> = ix.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["area", "area/health", "area/health/sleep", "Idea", "x", "x/area"]
        );
        assert_eq!(ix.search("tag:#area"), ["a.md", "b.md"]);
        assert_eq!(ix.search("#Area/Health"), ["a.md"]);
        assert_eq!(ix.search("idea"), ["b.md", "c/d.md"]);
        // G2: one tag, the first casing in path order
        assert_eq!(ix.get("IDEA").unwrap().name, "Idea");
        assert!(ix.search("health").is_empty());
        assert!(ix.search("").is_empty());
        assert_eq!(ix.get("area/health").unwrap().depth, 1);
        assert_eq!(ix.get("area/health").unwrap().leaf(), "health");
        assert_eq!(ix.tags_of("b.md"), ["area", "Idea"]);
    }

    #[test]
    fn sorted_by_segments() {
        let ix = index(&[("n.md", &["a-c", "a/b", "b", "a"])]);
        let names: Vec<&str> = ix.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a", "a/b", "a-c", "b"]);
    }

    #[test]
    fn queries() {
        assert_eq!(query_tag(" tag:#a/b/ "), Some("a/b"));
        assert_eq!(query_tag("TAG: x"), Some("x"));
        assert_eq!(query_tag("#x"), Some("x"));
        assert_eq!(query_tag("x y"), None);
        assert_eq!(query_tag("tag:"), None);
    }

    #[test]
    fn note_text() {
        assert!(note_tags("plain text").is_empty());
        assert_eq!(note_tags("---\ntags: a\n---\nsee #b\n"), ["a", "b"]);
        assert_eq!(note_tags("\u{FEFF}---\ntags: [a]\n---\n"), ["a"]);
    }
}
