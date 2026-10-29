//! Vault file-name map and link resolution (spec 06 §1–§3, owner decisions §17.1).
//!
//! The vault is the opened folder (the sidebar's mounted folder). [`VaultScanner`] keeps an
//! in-memory list of its files on a background thread — a full walk when a folder is mounted,
//! then one directory at a time from the file watcher's events — and publishes immutable
//! [`VaultIndex`] snapshots. Nothing is stored on disk and nothing in the vault is written.
//!
//! [`VaultIndex`] answers format-neutral questions on the UI thread without I/O:
//! - wikilink targets (V2.3, V2.4, tie-break Q-V2: exact case → same folder as the linking
//!   note → fewest folder levels → path order);
//! - Markdown link paths (V2.6: `%20` and `<…>` forms, relative first, then root, then name);
//! - the shortest unique link text for a file (V14.3 "shortest");
//! - fuzzy note-name matching for link autocomplete (V3.3).
//!
//! Vault paths use `/` separators and NFC; names compare case-insensitively (lower-cased NFC,
//! an approximation of simple case folding).
//!
//! # Threat model for notes created from links
//!
//! A link names a note that does not exist; confirming creates it (spec 06 V2.8). Link text
//! and the folder's contents are untrusted: another program (a sync client, a script, another
//! user with write access to a folder *inside* the vault) may change the tree at any moment,
//! including swapping a folder for a symbolic link while a note is being created.
//!
//! **Guarantee:** a note is only ever created inside the vault root directory *object* that
//! the scanner opened when the vault was mounted ([`VaultIndex::root_dir`]), never outside
//! it and never inside a hidden folder such as `.obsidian/` (Q-V1). Concretely:
//! - the root path is resolved once, at mount; creations use that directory handle and never
//!   resolve the root path again;
//! - every step is relative to the parent's open handle (`cap-std`): each folder is created
//!   and then entered with a no-follow open, and the file is created exclusively without
//!   following a link, so a component swapped for a symbolic link at any moment makes the
//!   creation fail instead of redirecting it;
//! - names are checked first ([`check_new_note`]): portable on every supported platform, no
//!   hidden components, no `..`.
//!
//! We never resolve link text through symlinks or outside paths, and never create through a
//! symlinked component.
//!
//! **Out of scope:**
//! - Replacing the vault root directory itself after it was mounted (whoever can do that
//!   already controls every note in it); creations then still go to the directory that was
//!   opened, or fail.
//! - Concurrent renames of directories inside the vault by other processes (equivalent to
//!   moving the note after creation, which no editor can prevent). As a cheap mitigation,
//!   after creating the file we walk the same components again from the root handle
//!   (no-follow) and compare the file's identity; if a folder was moved meanwhile, the user
//!   is told and the note is not opened (nothing is deleted).
//!
//! Reading follows links as the file tree does (the scanner lists a linked folder once, with
//! a cycle guard).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use unicode_normalization::UnicodeNormalization;

use crate::Wake;

/// NFC form of `s`.
pub fn nfc(s: &str) -> String {
    if s.is_ascii() { s.to_string() } else { s.nfc().collect() }
}

/// Case-insensitive comparison key: NFC, lower case.
pub fn fold(s: &str) -> String {
    nfc(s).to_lowercase()
}

/// `true` for a vault file that is a note (`.md`, any case).
pub fn is_note(name: &str) -> bool {
    let n = name.len();
    n > 3 && name.is_char_boundary(n - 3) && name[n - 3..].eq_ignore_ascii_case(".md")
}

/// One file of the vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultFile {
    /// Vault path (`Projects/Plan.md`), NFC: the lookup form.
    pub rel: String,
    /// The same path as spelled on disk (possibly NFD): the form files are opened with.
    pub disk: String,
    fold_rel: String,
    fold_name: String,
    /// Number of `/`-separated segments (1 for a file in the root).
    pub depth: usize,
}

impl VaultFile {
    /// The basename (`Plan.md`).
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// The basename without a trailing `.md` (`Plan`).
    pub fn stem(&self) -> &str {
        markise_md::wikilink::strip_md(self.name())
    }

    /// The folder part (`Projects`; empty in the root).
    pub fn folder(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(d, _)| d)
    }

    pub fn is_note(&self) -> bool {
        is_note(&self.rel)
    }
}

/// Path form of inserted links (spec 06 V14.3, `newLinkFormat`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkFormat {
    /// The name when unique, else the vault path.
    #[default]
    Shortest,
    /// Relative to the linking note's folder.
    Relative,
    /// The full vault path.
    Absolute,
}

/// Where notes created from links go (spec 06 V2.8, `newFileLocation`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NewNoteLocation {
    /// The vault root.
    #[default]
    Root,
    /// The linking note's folder.
    Current,
    /// This vault folder.
    Folder(String),
}

/// The vault's "Files and links" settings (spec 06 §14): Obsidian's (`.obsidian/app.json`,
/// read only, never written — Q-V1) or Markise's plain-folder defaults (Q-V15, Q-V18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultSettings {
    /// The folder has a `.obsidian/` directory.
    pub obsidian: bool,
    /// Insert wikilinks (`false`: Markdown links).
    pub wikilinks: bool,
    pub link_format: LinkFormat,
    pub new_notes: NewNoteLocation,
    /// Obsidian's "Default location for new attachments" (`attachmentFolderPath`, V14.2) as
    /// written, when the vault sets it: `/` = vault root, `./` = the note's folder, `./name` =
    /// a subfolder of the note's folder, anything else = that folder from the vault root
    /// (interpreted by `crate::attach::image_folder`).
    pub attachment_folder: Option<String>,
}

impl Default for VaultSettings {
    fn default() -> Self {
        VaultSettings::plain()
    }
}

impl VaultSettings {
    /// A folder without `.obsidian/`: Markdown links with relative paths (Typora-readable,
    /// Q-V15), new notes next to the linking note (Q-V18).
    pub fn plain() -> Self {
        VaultSettings {
            obsidian: false,
            wikilinks: false,
            link_format: LinkFormat::Relative,
            new_notes: NewNoteLocation::Current,
            attachment_folder: None,
        }
    }

    /// An Obsidian vault: wikilinks and shortest paths (Obsidian's defaults); new notes next
    /// to the linking note (owner revision of Q-V18, 2026-10-28: the same default as plain
    /// folders). Keys of `app_json` that are present and understood override these (key
    /// names inferred, Q-V3; anything else is ignored) — `newFileLocation` only when set
    /// explicitly.
    pub fn obsidian(app_json: Option<&str>) -> Self {
        let mut s = VaultSettings {
            obsidian: true,
            wikilinks: true,
            link_format: LinkFormat::Shortest,
            new_notes: NewNoteLocation::Current,
            attachment_folder: None,
        };
        let Some(serde_json::Value::Object(m)) = app_json.and_then(|t| serde_json::from_str(t).ok()) else {
            return s;
        };
        if let Some(b) = m.get("useMarkdownLinks").and_then(serde_json::Value::as_bool) {
            s.wikilinks = !b;
        }
        match m.get("newLinkFormat").and_then(serde_json::Value::as_str) {
            Some("shortest") => s.link_format = LinkFormat::Shortest,
            Some("relative") => s.link_format = LinkFormat::Relative,
            Some("absolute") => s.link_format = LinkFormat::Absolute,
            _ => {}
        }
        s.attachment_folder = m
            .get("attachmentFolderPath")
            .and_then(serde_json::Value::as_str)
            .map(|p| nfc(p.trim()))
            .filter(|p| !p.is_empty());
        match m.get("newFileLocation").and_then(serde_json::Value::as_str) {
            Some("root") => s.new_notes = NewNoteLocation::Root,
            Some("current") => s.new_notes = NewNoteLocation::Current,
            Some("folder") => {
                let f = m
                    .get("newFileFolderPath")
                    .and_then(serde_json::Value::as_str)
                    .map(|p| nfc(p.trim().trim_matches('/')))
                    .unwrap_or_default();
                s.new_notes = if f.is_empty() {
                    NewNoteLocation::Root
                } else {
                    NewNoteLocation::Folder(f)
                };
            }
            _ => {}
        }
        s
    }

    /// Reads the settings of the vault at `root` (file I/O: worker threads only).
    pub fn read(root: &Path) -> Self {
        let dir = root.join(".obsidian");
        if !dir.is_dir() {
            return VaultSettings::plain();
        }
        // a regular file of sane size only: a FIFO or device would block the open or read
        let app = dir.join("app.json");
        let regular = std::fs::symlink_metadata(&app).is_ok_and(|m| m.is_file() && m.len() <= APP_JSON_MAX);
        let text = regular.then(|| std::fs::File::open(&app).ok()).flatten().and_then(|f| {
            use std::io::Read;
            let mut t = String::new();
            f.take(APP_JSON_MAX).read_to_string(&mut t).ok().map(|_| t)
        });
        VaultSettings::obsidian(text.as_deref())
    }
}

/// Largest `.obsidian/app.json` read (a settings file is a few hundred bytes).
const APP_JSON_MAX: u64 = 1 << 20;

/// An immutable snapshot of the vault's files.
#[derive(Clone, Debug, Default)]
pub struct VaultIndex {
    root: PathBuf,
    /// The vault root directory opened once when the scanner mounted it: the only capability
    /// notes are created through (see the threat model in the module docs).
    root_dir: Option<Arc<cap_std::fs::Dir>>,
    generation: u64,
    settings: VaultSettings,
    /// Sorted by vault path (code-point order).
    files: Vec<VaultFile>,
    by_name: HashMap<String, Vec<u32>>,
    by_rel: HashMap<String, Vec<u32>>,
}

/// Folder part of a vault path.
fn folder_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Joins `base` (a vault folder) and `rel`, resolving `.` and `..`; `None` when the path
/// leaves the vault.
fn join_rel(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// Percent-decodes `s` (UTF-8; invalid sequences stay as written).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-encodes a path for a Markdown link destination (V2.6.3): space, `%`, `#`, `(`,
/// `)`, `<`, `>`, `^`, `|`, `[`, `]`, backslash and control characters; other Unicode stays
/// literal.
pub fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control()
            || matches!(
                c,
                ' ' | '%' | '#' | '(' | ')' | '<' | '>' | '^' | '|' | '[' | ']' | '\\'
            )
        {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A target that can be written inside `[[…]]` (V2.1.2 rule 5, for the whole path): none of
/// `# | ^ : [ ]` or `%%`, no surrounding whitespace (it would be trimmed).
fn wiki_target_ok(t: &str) -> bool {
    !t.is_empty() && !t.contains(['#', '|', '^', ':', '[', ']', '\n']) && !t.contains("%%") && t.trim() == t
}

/// A link label with the Markdown syntax characters backslash-escaped.
fn escape_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '>' | '!' | '&' | '$' | '~' | '=' | '^'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A URL scheme at the start of `s` (`https:`), not a Windows drive letter.
fn has_scheme(s: &str) -> bool {
    s.split_once(':').is_some_and(|(sch, _)| {
        sch.len() >= 2
            && sch.starts_with(|c: char| c.is_ascii_alphabetic())
            && sch
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
    })
}

/// How a link was matched: a file and the fragment / subpath it carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// Vault path of the target file.
    pub rel: String,
    /// `#fragment` of a Markdown link (percent-decoded) — `None` for wikilinks, whose subpath
    /// the caller already has.
    pub fragment: Option<String>,
}

impl VaultIndex {
    /// An index of `rels` (vault paths as spelled on disk, any order) under `root` (tests,
    /// benchmarks, scanner).
    pub fn from_rels(root: PathBuf, rels: impl IntoIterator<Item = String>, generation: u64) -> VaultIndex {
        let mut files: Vec<VaultFile> = rels
            .into_iter()
            .map(|disk| {
                let rel = nfc(&disk);
                let fold_rel = rel.to_lowercase();
                let fold_name = fold_rel.rsplit('/').next().unwrap_or(&fold_rel).to_string();
                let depth = rel.split('/').count();
                VaultFile {
                    rel,
                    disk,
                    fold_rel,
                    fold_name,
                    depth,
                }
            })
            .collect();
        files.sort_by(|a, b| a.rel.cmp(&b.rel));
        files.dedup_by(|a, b| a.rel == b.rel);
        let mut by_name: HashMap<String, Vec<u32>> = HashMap::with_capacity(files.len());
        let mut by_rel: HashMap<String, Vec<u32>> = HashMap::with_capacity(files.len());
        for (i, f) in files.iter().enumerate() {
            by_name.entry(f.fold_name.clone()).or_default().push(i as u32);
            by_rel.entry(f.fold_rel.clone()).or_default().push(i as u32);
        }
        VaultIndex {
            root,
            root_dir: None,
            generation,
            settings: VaultSettings::plain(),
            files,
            by_name,
            by_rel,
        }
    }

    /// The same index with the vault root's directory capability.
    pub fn with_root_dir(mut self, dir: Option<Arc<cap_std::fs::Dir>>) -> Self {
        self.root_dir = dir;
        self
    }

    /// The vault root directory opened at mount time (notes are created through it).
    pub fn root_dir(&self) -> Option<&Arc<cap_std::fs::Dir>> {
        self.root_dir.as_ref()
    }

    /// The same index with `settings`.
    pub fn with_settings(mut self, settings: VaultSettings) -> Self {
        self.settings = settings;
        self
    }

    /// The vault's "Files and links" settings.
    pub fn settings(&self) -> &VaultSettings {
        &self.settings
    }

    /// The link written for `f` in the note at `from` (spec 06 V14.3, V2.6.3): a wikilink when
    /// `wikilink` and the name allows one (V2.1.2 rule 5), else a Markdown link with an
    /// encoded path including the extension; the path form follows the vault's settings.
    pub fn link_for(&self, f: &VaultFile, from: Option<&str>, wikilink: bool) -> String {
        let from_dir = from.map(folder_of).unwrap_or("");
        let relative = relative_rel(from_dir, &f.rel);
        let primary = match self.settings.link_format {
            LinkFormat::Shortest => {
                let unique = self.by_name.get(&f.fold_name).is_none_or(|v| v.len() <= 1);
                if unique { f.name().to_string() } else { f.rel.clone() }
            }
            LinkFormat::Relative => relative.clone(),
            LinkFormat::Absolute => f.rel.clone(),
        };
        // the preferred form first, then longer ones: whichever resolves back to `f` wins
        // (a shorter form may name another file, e.g. `Sub/Note` from `A/` when a root
        // `Sub/Note.md` exists)
        let mut paths = vec![primary];
        if !relative.starts_with("../") {
            paths.push(format!("./{relative}"));
        }
        paths.push(f.rel.clone());
        paths.push(format!("/{}", f.rel));
        paths.dedup();
        if wikilink {
            for p in &paths {
                let t = if f.is_note() {
                    markise_md::wikilink::strip_md(p)
                } else {
                    p.as_str()
                };
                if wiki_target_ok(t) && self.resolve_wikilink(t, from).is_some_and(|g| g.rel == f.rel) {
                    return format!("[[{t}]]");
                }
            }
        }
        let label = escape_label(f.stem());
        for p in &paths {
            let enc = percent_encode_path(p);
            if self.resolve_markdown(&enc, from).is_some_and(|r| r.rel == f.rel) {
                return format!("[{label}]({enc})");
            }
        }
        format!("[{label}]({})", percent_encode_path(&format!("/{}", f.rel)))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Changes with every new snapshot.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn files(&self) -> &[VaultFile] {
        &self.files
    }

    /// The file at vault path `rel` (exact).
    pub fn file(&self, rel: &str) -> Option<&VaultFile> {
        self.files
            .binary_search_by(|f| f.rel.as_str().cmp(rel))
            .ok()
            .map(|i| &self.files[i])
    }

    /// The vault path of an absolute path inside the vault.
    pub fn rel_of(&self, abs: &Path) -> Option<String> {
        rel_path(&self.root, abs)
    }

    /// The absolute path of a vault path, spelled as on disk when the file is known.
    pub fn abs(&self, rel: &str) -> PathBuf {
        let disk = self.file(rel).map_or(rel, |f| f.disk.as_str());
        let mut p = self.root.clone();
        for s in disk.split('/').filter(|s| !s.is_empty()) {
            p.push(s);
        }
        p
    }

    fn by_rel_candidates(&self, rel: &str) -> Vec<u32> {
        self.by_rel.get(&fold(rel)).cloned().unwrap_or_default()
    }

    /// Candidates for path `p` (no leading `/`) with the extension rule (V2.3.2).
    fn rel_with_ext(&self, p: &str) -> (Vec<u32>, String) {
        if is_note(p) {
            return (self.by_rel_candidates(p), p.to_string());
        }
        let md = format!("{p}.md");
        let c = self.by_rel_candidates(&md);
        if !c.is_empty() {
            return (c, md);
        }
        (self.by_rel_candidates(p), p.to_string())
    }

    /// Picks among `cands` (V2.4.4): exact case of `wanted` (compared with the whole vault path
    /// when `whole`, else with the name) → same folder as `from` → fewest segments → path.
    fn pick(&self, cands: &[u32], wanted: &str, whole: bool, from: Option<&str>) -> Option<&VaultFile> {
        let from_dir = from.map(folder_of);
        cands.iter().map(|&i| &self.files[i as usize]).min_by(|a, b| {
            let exact = |f: &VaultFile| {
                let s = if whole { f.rel.as_str() } else { f.name() };
                s != wanted
            };
            let same = |f: &VaultFile| from_dir.is_none_or(|d| f.folder() != d);
            exact(a)
                .cmp(&exact(b))
                .then_with(|| same(a).cmp(&same(b)))
                .then_with(|| a.depth.cmp(&b.depth))
                .then_with(|| a.rel.cmp(&b.rel))
        })
    }

    /// Resolves a wikilink target as written (V2.3, V2.4). `from`: the linking note's vault
    /// path (relative forms and the same-folder preference need it). An empty target is the
    /// linking note itself.
    pub fn resolve_wikilink(&self, target: &str, from: Option<&str>) -> Option<&VaultFile> {
        // V2.3.1: trim, NFC, collapse repeated `/` (a backslash is an ordinary character)
        let mut t = nfc(target.trim());
        while t.contains("//") {
            t = t.replace("//", "/");
        }
        if t.is_empty() {
            return from.and_then(|f| self.file(f));
        }
        if !t.contains('/') {
            // V2.4.2: basename form, any folder
            let (cands, wanted) = if is_note(&t) {
                (
                    self.by_name.get(&t.to_lowercase()).cloned().unwrap_or_default(),
                    t.clone(),
                )
            } else {
                let md = format!("{t}.md");
                match self.by_name.get(&md.to_lowercase()) {
                    Some(c) if !c.is_empty() => (c.clone(), md),
                    _ => (
                        self.by_name.get(&t.to_lowercase()).cloned().unwrap_or_default(),
                        t.clone(),
                    ),
                }
            };
            return self.pick(&cands, &wanted, false, from);
        }
        let from_dir = from.map(folder_of).unwrap_or("");
        // V2.4.3 (1): `./`, `../` only relative to the linking note
        if t.starts_with("./") || t.starts_with("../") {
            let p = join_rel(from_dir, &t)?;
            let (c, w) = self.rel_with_ext(&p);
            return self.pick(&c, &w, true, from);
        }
        // (2): `/` only from the vault root
        if let Some(rest) = t.strip_prefix('/') {
            let (c, w) = self.rel_with_ext(rest);
            return self.pick(&c, &w, true, from);
        }
        // (3a) from the root, (3b) relative to the note, (3c) suffix match
        let (c, w) = self.rel_with_ext(&t);
        if !c.is_empty() {
            return self.pick(&c, &w, true, from);
        }
        if let Some(p) = join_rel(from_dir, &t) {
            let (c, w) = self.rel_with_ext(&p);
            if !c.is_empty() {
                return self.pick(&c, &w, true, from);
            }
        }
        let last = t.rsplit('/').next().unwrap_or(&t);
        for (name, suffix) in if is_note(&t) {
            vec![(last.to_string(), t.clone())]
        } else {
            vec![(format!("{last}.md"), format!("{t}.md")), (last.to_string(), t.clone())]
        } {
            let want = format!("/{}", suffix.to_lowercase());
            let cands: Vec<u32> = self
                .by_name
                .get(&name.to_lowercase())
                .map(|v| {
                    v.iter()
                        .copied()
                        .filter(|&i| self.files[i as usize].fold_rel.ends_with(&want))
                        .collect()
                })
                .unwrap_or_default();
            if !cands.is_empty() {
                return self.pick(&cands, &name, false, from);
            }
        }
        None
    }

    /// Resolves a Markdown link destination (V2.6.1, V2.6.2): `None` for URLs, same-document
    /// `#fragment` links and paths that resolve to no vault file or leave the vault.
    pub fn resolve_markdown(&self, dest: &str, from: Option<&str>) -> Option<Resolved> {
        let (path, fragment) = split_markdown_dest(dest)?;
        let from_dir = from.map(folder_of).unwrap_or("");
        let found = |rel: &str| -> Option<&VaultFile> {
            let (c, w) = self.rel_with_ext(rel);
            self.pick(&c, &w, true, from)
        };
        let file = if let Some(rest) = path.strip_prefix('/') {
            found(rest)
        } else {
            // leaving the vault: unresolved, never searched elsewhere
            let r = join_rel(from_dir, &path)?;
            found(&r).or_else(|| found(&path)).or_else(|| {
                (!path.contains('/'))
                    .then(|| self.resolve_wikilink(&path, from))
                    .flatten()
            })
        }?;
        Some(Resolved {
            rel: file.rel.clone(),
            fragment,
        })
    }

    /// The link text written for `f` with the "shortest" format (V14.3): the stem (notes) or
    /// name (attachments) when no other file has the same name, else the full vault path
    /// (without `.md` for notes).
    pub fn shortest_link(&self, f: &VaultFile) -> String {
        let unique = self.by_name.get(&f.fold_name).is_none_or(|v| v.len() <= 1);
        let shown = if unique { f.name() } else { f.rel.as_str() };
        if f.is_note() {
            markise_md::wikilink::strip_md(shown).to_string()
        } else {
            shown.to_string()
        }
    }

    /// Notes matching `query` for link autocomplete (V3.3): fuzzy subsequence match on the
    /// stem, then on the vault path; ranking exact stem > prefix > word-start subsequence >
    /// other subsequence > path-only match; ties: shorter path, then path order. At most
    /// `limit` results.
    pub fn suggest(&self, query: &str, limit: usize) -> Vec<&VaultFile> {
        let q = fold(query.trim());
        let mut scored: Vec<(u8, usize, &str, &VaultFile)> = Vec::new();
        for f in self.files.iter().filter(|f| f.is_note()) {
            let stem = markise_md::wikilink::strip_md(&f.fold_name);
            let rank = if q.is_empty() {
                Some(4)
            } else if stem == q {
                Some(0)
            } else if stem.starts_with(&q) {
                Some(1)
            } else if let Some(word_start) = subsequence(stem, &q) {
                Some(if word_start { 2 } else { 3 })
            } else if subsequence(&f.fold_rel, &q).is_some() {
                Some(4)
            } else {
                None
            };
            if let Some(r) = rank {
                scored.push((r, f.rel.len(), &f.rel, f));
            }
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(b.2)));
        scored.into_iter().take(limit).map(|x| x.3).collect()
    }
}

/// Whether `q` is a subsequence of `s`; `Some(true)` when every matched character starts a
/// word (or the match is contiguous from a word start).
fn subsequence(s: &str, q: &str) -> Option<bool> {
    let mut it = q.chars().peekable();
    let mut prev: Option<char> = None;
    let mut prev_matched = false;
    let mut word_starts = true;
    for c in s.chars() {
        let Some(&want) = it.peek() else { break };
        if c == want {
            let at_start = prev.is_none_or(|p| !p.is_alphanumeric());
            if !at_start && !prev_matched {
                word_starts = false;
            }
            it.next();
            prev_matched = true;
        } else {
            prev_matched = false;
        }
        prev = Some(c);
    }
    it.peek().is_none().then_some(word_starts)
}

/// The path and fragment of a Markdown link destination (V2.6.1): `None` for URLs and
/// same-document `#…` links.
pub fn split_markdown_dest(dest: &str) -> Option<(String, Option<String>)> {
    let d = dest.trim();
    let d = d.strip_prefix('<').and_then(|x| x.strip_suffix('>')).unwrap_or(d);
    if d.is_empty() || d.starts_with('#') || has_scheme(d) {
        return None;
    }
    let (p, frag) = match d.find('#') {
        Some(i) => (&d[..i], Some(percent_decode(&d[i + 1..]))),
        None => (d, None),
    };
    let mut path = nfc(&percent_decode(p).replace('\\', "/"));
    while path.contains("//") {
        path = path.replace("//", "/");
    }
    if path.is_empty() {
        return None;
    }
    Some((path, frag.filter(|f| !f.is_empty())))
}

/// Vault path `to` relative to vault folder `from_dir` (`../x/y.md`).
fn relative_rel(from_dir: &str, to: &str) -> String {
    let a: Vec<&str> = if from_dir.is_empty() {
        Vec::new()
    } else {
        from_dir.split('/').collect()
    };
    let b: Vec<&str> = to.split('/').collect();
    let common = a
        .iter()
        .zip(&b[..b.len().saturating_sub(1)])
        .take_while(|(x, y)| x == y)
        .count();
    let mut parts: Vec<&str> = vec![".."; a.len() - common];
    parts.extend(&b[common..]);
    parts.join("/")
}

/// The vault path of `abs` under `root` (`None` outside it).
pub fn rel_path(root: &Path, abs: &Path) -> Option<String> {
    let r = abs.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in r.components() {
        match c {
            Component::Normal(s) => parts.push(nfc(&s.to_string_lossy())),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The vault path of `abs` under `root` as spelled on disk (no normalisation).
fn disk_rel_path(root: &Path, abs: &Path) -> Option<String> {
    let r = abs.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in r.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Windows device names, reserved in every folder with any extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why the vault path `rel` cannot name a new note (spec 06 V2.8 rule 4: names valid on every
/// supported platform; V1.3/V1.4: never inside a hidden folder such as `.obsidian/`).
pub fn check_new_note(rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        return Err("The name is empty.".into());
    }
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err("The name has an empty or relative folder part.".into());
        }
        if seg.starts_with('.') {
            return Err("Notes cannot be created in hidden folders or as hidden files.".into());
        }
        if let Some(c) = seg
            .chars()
            .find(|c| matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
        {
            return Err(format!("Note names cannot contain “{}”.", c.escape_default()));
        }
        if seg.ends_with('.') || seg.ends_with(' ') || seg.starts_with(' ') {
            return Err("Names cannot start with a space or end with a dot or a space.".into());
        }
        let base = seg.split('.').next().unwrap_or(seg).trim_end().to_ascii_uppercase();
        if RESERVED.contains(&base.as_str()) {
            return Err(format!("“{seg}” is a reserved name on Windows."));
        }
    }
    Ok(())
}

/// Creates the empty note `rel` in the vault whose root directory `root_dir` was opened at
/// mount time (file I/O: worker threads only) and returns its path below `root_path` (the
/// path that was mounted; used only to name the result, never resolved). An existing file is
/// returned as is. Every step works relative to an open directory handle (`cap-std`): folders
/// are created and entered without following symbolic links, and the file is created
/// exclusively without following one — see the threat model in the module docs. Existing
/// folders keep their disk spelling (NFD stays NFD).
pub fn create_note(root_dir: &cap_std::fs::Dir, root_path: &Path, rel: &str) -> Result<PathBuf, String> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    check_new_note(rel)?;
    let mut dir = root_dir.try_clone().map_err(|e| format!("the vault folder: {e}"))?;
    let segs: Vec<&str> = rel.split('/').collect();
    let (last, folders) = segs.split_last().ok_or("The name is empty.")?;
    let mut path = root_path.to_path_buf();
    let mut so_far = String::new();
    let mut disk_folders: Vec<String> = Vec::new();
    let refuse = |seg: &str, e: std::io::Error| -> String {
        match dir_is_link(&e) {
            true => format!("“{seg}” is a symbolic link; notes are not created through links."),
            false => format!("“{seg}”: {e}"),
        }
    };
    for seg in folders {
        let seg = disk_name(&dir, seg);
        if !so_far.is_empty() {
            so_far.push('/');
        }
        so_far.push_str(&seg);
        step_hook("create_dir", &so_far);
        match dir.create_dir(&seg) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("“{seg}”: {e}")),
        }
        step_hook("open_dir", &so_far);
        dir = dir.open_dir_nofollow(&seg).map_err(|e| refuse(&seg, e))?;
        path.push(&seg);
        disk_folders.push(seg);
    }
    let last = disk_name(&dir, last);
    let mut opts = cap_std::fs::OpenOptions::new();
    opts.write(true).create_new(true).follow(FollowSymlinks::No);
    step_hook("create_file", &so_far);
    let created = match dir.open_with(&last, &opts) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let m = dir.symlink_metadata(&last).map_err(|e| format!("“{last}”: {e}"))?;
            if m.file_type().is_symlink() {
                return Err(format!(
                    "“{last}” is a symbolic link; notes are not created through links."
                ));
            }
            if !m.is_file() {
                return Err(format!("“{last}” exists and is not a file."));
            }
            let mut ro = cap_std::fs::OpenOptions::new();
            ro.read(true).follow(FollowSymlinks::No);
            dir.open_with(&last, &ro).map_err(|e| refuse(&last, e))?
        }
        Err(e) => return Err(refuse(&last, e)),
    };
    step_hook("verify", &so_far);
    // a folder renamed away by another process meanwhile (out of scope, see the threat
    // model) is at least reported: the note must still be reachable from the root
    if !reachable(root_dir, &disk_folders, &last, &created) {
        return Err("A folder was moved while the note was being created; the note was not opened.".into());
    }
    path.push(&last);
    Ok(path)
}

/// Whether `file` is still `folders/…/name` below `root`, walking without following links
/// (the same file: device and inode on Unix; the same kind of entry elsewhere).
fn reachable(root: &cap_std::fs::Dir, folders: &[String], name: &str, file: &cap_std::fs::File) -> bool {
    use cap_fs_ext::DirExt;
    let Ok(mut d) = root.try_clone() else { return false };
    for seg in folders {
        match d.open_dir_nofollow(seg) {
            Ok(n) => d = n,
            Err(_) => return false,
        }
    }
    let (Ok(found), Ok(held)) = (d.symlink_metadata(name), file.metadata()) else {
        return false;
    };
    if found.file_type().is_symlink() || !found.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use cap_fs_ext::OsMetadataExt;
        found.dev() == held.dev() && found.ino() == held.ino()
    }
    #[cfg(not(unix))]
    {
        held.is_file()
    }
}

/// A [`create_note`] step hook: step name, vault path of the folder concerned.
#[cfg(test)]
pub(crate) type CreateHook = Box<dyn FnMut(&str, &str)>;

#[cfg(test)]
thread_local! {
    /// Test hook run at each step boundary of [`create_note`] with the step name and the vault
    /// path of the folder concerned (controlled-interleaving tests).
    pub(crate) static CREATE_HOOK: std::cell::RefCell<Option<CreateHook>> =
        std::cell::RefCell::new(None);
}

#[inline]
fn step_hook(_step: &str, _folder: &str) {
    #[cfg(test)]
    CREATE_HOOK.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f(_step, _folder);
        }
    });
}

/// Opens the vault root directory for the capability held from mount. On Windows the handle
/// is opened sharing delete access (cap-std's own directory opens do not), so the user can
/// still rename or move the vault folder while Markise has it open; the handle then keeps
/// pointing at the moved folder, as the threat model says.
pub fn open_root(root: &Path) -> std::io::Result<cap_std::fs::Dir> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        const FILE_SHARE_DELETE: u32 = 0x4;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root)?;
        if !f.metadata()?.is_dir() {
            return Err(std::io::Error::other("not a folder"));
        }
        Ok(cap_std::fs::Dir::from_std_file(f))
    }
    #[cfg(not(windows))]
    {
        cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())
    }
}

/// Whether an error from a no-follow open means "this is a symbolic link" (`ELOOP` on Unix,
/// cap-std's own sandbox errors elsewhere).
fn dir_is_link(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ELOOP) || e.raw_os_error() == Some(libc::ENOTDIR) {
        return true;
    }
    e.kind() == std::io::ErrorKind::PermissionDenied || e.to_string().contains("symlink")
}

/// The spelling of `seg` in `dir` on disk: an existing entry whose name has the same NFC form
/// (an NFD folder stays NFD), else `seg` itself.
fn disk_name(dir: &cap_std::fs::Dir, seg: &str) -> String {
    // listed, not probed: a normalisation-insensitive file system answers a probe for the
    // NFC spelling of an NFD entry, but only the listing has the disk spelling
    let want = nfc(seg);
    let names: Vec<String> = dir
        .entries()
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if names.iter().any(|n| n == seg) {
        return seg.to_string();
    }
    names
        .into_iter()
        .find(|n| nfc(n) == want)
        .unwrap_or_else(|| seg.to_string())
}

/// [`create_note`] through `index`'s root capability on a short-lived worker thread; the
/// result arrives on the receiver (the caller is woken).
pub fn spawn_create_note(index: Arc<VaultIndex>, rel: String, wake: Wake) -> Receiver<Result<PathBuf, String>> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("markise-new-note".into()).spawn({
        let tx = tx.clone();
        move || {
            let r = match index.root_dir() {
                Some(dir) => create_note(dir, index.root(), &rel),
                None => Err("the vault folder could not be opened".into()),
            };
            let _ = tx.send(r);
            wake();
        }
    });
    if let Err(e) = spawned {
        let _ = tx.send(Err(format!("cannot start a thread: {e}")));
    }
    rx
}

/// A relative path from folder `from_dir` to `to` (both absolute), `/`-separated (export
/// links). `None` when no relative path exists (another drive).
pub fn relative_path(from_dir: &Path, to: &Path) -> Option<String> {
    let a: Vec<Component> = from_dir.components().collect();
    let b: Vec<Component> = to.components().collect();
    if a.first() != b.first() {
        return None;
    }
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..a.len() {
        parts.push("..".into());
    }
    for c in &b[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    Some(parts.join("/"))
}

// ------------------------------------------------------------------ background scanner

enum Cmd {
    Mount(PathBuf),
    Changed(PathBuf),
    Rescan,
}

/// Keeps a [`VaultIndex`] of the mounted folder current on a background thread.
pub struct VaultScanner {
    /// Set on drop: the worker stops at the next directory entry.
    cancel: Arc<std::sync::atomic::AtomicBool>,
    tx: Option<Sender<Cmd>>,
    rx: Receiver<Arc<VaultIndex>>,
    /// Joined on drop: the worker holds the vault root's directory handle, which must be
    /// closed when the scanner goes (on Windows an open handle keeps a deleted folder
    /// pending, so the same path cannot be created again).
    worker: Option<std::thread::JoinHandle<()>>,
    /// Commands sent and not fully processed yet (tests: [`VaultScanner::is_idle`]).
    pending: Arc<std::sync::atomic::AtomicUsize>,
    /// A snapshot taken off the channel by [`VaultScanner::is_idle`], handed out next.
    stash: std::sync::Mutex<Option<Arc<VaultIndex>>>,
}

/// How long dropping a scanner waits for its worker (on the UI thread).
const DROP_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

impl Drop for VaultScanner {
    fn drop(&mut self) {
        // cancel: the worker stops between directory entries and discards queued commands;
        // closing the command channel ends its loop
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.tx = None;
        let Some(w) = self.worker.take() else { return };
        let t = std::time::Instant::now();
        while !w.is_finished() && t.elapsed() < DROP_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if w.is_finished() {
            let _ = w.join();
        }
        // else: detached — it exits on its own once its blocking call returns; its root handle
        // does not keep the folder from being renamed, deleted or re-created (Unix; Windows
        // opens it sharing delete access, see `open_root`)
    }
}

/// Test hook run before each folder the scanner walks, with the vault root (a test blocks a
/// worker of one root without touching other tests' scanners).
#[cfg(test)]
pub(crate) type ScanHook = Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
pub(crate) static SCAN_HOOK: std::sync::Mutex<Option<ScanHook>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn scan_hook(root: &Path) {
    let h = SCAN_HOOK.lock().ok().and_then(|g| g.clone());
    if let Some(h) = h {
        h(root);
    }
}

/// Directory entries the scanner follows at most (cycle and runaway guard).
const MAX_DEPTH: usize = 64;

struct ScanState {
    root: PathBuf,
    files: BTreeSet<String>,
    dirs: BTreeSet<String>,
    generation: u64,
    settings: VaultSettings,
    /// Something a snapshot shows changed since the last one.
    dirty: bool,
    /// The root directory opened once at mount (see the threat model).
    root_dir: Option<Arc<cap_std::fs::Dir>>,
    /// The owning scanner was dropped: stop as soon as possible.
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl ScanState {
    /// Lists `dir` (vault path, `""` = root): its files and sub-folders (hidden entries
    /// skipped, V1.3).
    fn list(&self, dir: &str) -> Option<(Vec<String>, Vec<String>)> {
        let abs = if dir.is_empty() {
            self.root.clone()
        } else {
            let mut p = self.root.clone();
            p.extend(dir.split('/'));
            p
        };
        let rd = std::fs::read_dir(&abs).ok()?;
        let (mut files, mut dirs) = (Vec::new(), Vec::new());
        for e in rd.flatten() {
            if self.cancelled() {
                return None;
            }
            // the disk spelling (an NFD name must be listed and opened as NFD)
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = if dir.is_empty() { name } else { format!("{dir}/{name}") };
            match crate::tree::entry_is_dir(&e) {
                Some(true) => dirs.push(rel),
                Some(false) => files.push(rel),
                None => {}
            }
        }
        Some((files, dirs))
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn walk(&mut self, dir: &str, depth: usize, seen: &mut HashSet<PathBuf>) {
        if depth > MAX_DEPTH || self.cancelled() {
            return;
        }
        #[cfg(test)]
        scan_hook(&self.root);
        let abs = self.root.join(dir);
        if let Ok(c) = std::fs::canonicalize(&abs)
            && !seen.insert(c)
        {
            return; // a symlink back into a folder already walked
        }
        let Some((files, dirs)) = self.list(dir) else { return };
        self.dirs.insert(dir.to_string());
        self.files.extend(files);
        for d in dirs {
            self.walk(&d, depth + 1, seen);
        }
    }

    fn remove_subtree(&mut self, dir: &str) {
        let prefix = format!("{dir}/");
        self.files.retain(|f| !f.starts_with(&prefix));
        self.dirs.retain(|d| d != dir && !d.starts_with(&prefix));
    }

    fn mount(&mut self, root: PathBuf) {
        self.dirty = true;
        // the root path is resolved here, once; later creations use this handle only
        self.root_dir = open_root(&root).ok().map(Arc::new);
        self.settings = VaultSettings::read(&root);
        self.root = root;
        self.files.clear();
        self.dirs.clear();
        let mut seen = HashSet::new();
        self.walk("", 0, &mut seen);
    }

    /// Walks the mounted root again (the watch restarted), keeping its directory handle;
    /// publishes only if something differs.
    fn rescan(&mut self) {
        let (files, dirs) = (std::mem::take(&mut self.files), std::mem::take(&mut self.dirs));
        let settings = VaultSettings::read(&self.root);
        let mut seen = HashSet::new();
        self.walk("", 0, &mut seen);
        if self.files != files || self.dirs != dirs || settings != self.settings {
            self.settings = settings;
            self.dirty = true;
        }
    }

    /// The indexed files and folders below `dir` (`""` = all), for change detection.
    fn subtree(&self, dir: &str) -> (Vec<String>, Vec<String>) {
        if dir.is_empty() {
            return (
                self.files.iter().cloned().collect(),
                self.dirs.iter().cloned().collect(),
            );
        }
        let prefix = format!("{dir}/");
        let under = |set: &BTreeSet<String>| -> Vec<String> {
            // everything starting with `dir` is contiguous; `dir b.md` may sit between `dir`
            // and `dir/…`, so filter inside that run
            set.range(dir.to_string()..)
                .take_while(|p| p.starts_with(dir))
                .filter(|p| p.as_str() == dir || p.starts_with(&prefix))
                .cloned()
                .collect()
        };
        (under(&self.files), under(&self.dirs))
    }

    /// `abs` changed (a directory, or something inside one): list its directory again.
    fn changed(&mut self, abs: &Path) {
        let mut dir = if abs == self.root {
            String::new()
        } else {
            match disk_rel_path(&self.root, abs) {
                Some(r) => r,
                None => return,
            }
        };
        if !dir.is_empty() && self.files.contains(&dir) {
            dir = folder_of(&dir).to_string();
        }
        // Obsidian's settings may have changed (or the config folder appeared / went away);
        // a snapshot is published only when they did
        if dir.is_empty() || dir == ".obsidian" || dir.starts_with(".obsidian/") {
            let s = VaultSettings::read(&self.root);
            if s != self.settings {
                self.settings = s;
                self.dirty = true;
            }
        }
        if dir.split('/').any(|s| s.starts_with('.')) {
            return;
        }
        // publish only if the indexed contents below `dir` really change (duplicate events,
        // an existing note replaced atomically, hidden files: no new snapshot)
        let before = self.subtree(&dir);
        self.relist(&dir);
        if self.subtree(&dir) != before {
            self.dirty = true;
        }
    }

    fn relist(&mut self, dir: &str) {
        let dir = dir.to_string();
        let Some((files, dirs)) = self.list(&dir) else {
            // gone: drop it and everything below
            if dir.is_empty() {
                self.files.clear();
                self.dirs.clear();
            } else {
                self.files.remove(&dir);
                self.remove_subtree(&dir);
            }
            return;
        };
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let direct = |p: &str| p.starts_with(&prefix) && !p[prefix.len()..].contains('/');
        self.files.retain(|f| !direct(f));
        self.files.extend(files);
        let known: Vec<String> = self
            .dirs
            .iter()
            .filter(|d| !d.is_empty() && direct(d))
            .cloned()
            .collect();
        let now: HashSet<&String> = dirs.iter().collect();
        for d in known {
            if !now.contains(&d) {
                self.remove_subtree(&d);
            }
        }
        self.dirs.insert(dir.clone());
        for d in dirs {
            if !self.dirs.contains(&d) {
                let mut seen = HashSet::new();
                self.walk(&d, d.split('/').count(), &mut seen);
            }
        }
    }

    fn snapshot(&mut self) -> Arc<VaultIndex> {
        self.generation += 1;
        Arc::new(
            VaultIndex::from_rels(self.root.clone(), self.files.iter().cloned(), self.generation)
                .with_settings(self.settings.clone())
                .with_root_dir(self.root_dir.clone()),
        )
    }
}

impl VaultScanner {
    pub fn new(wake: Wake) -> Result<VaultScanner, String> {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (res_tx, rx) = channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_pending = pending.clone();
        let worker = std::thread::Builder::new()
            .name("markise-vault".into())
            .spawn(move || {
                let mut st = ScanState {
                    root: PathBuf::new(),
                    files: BTreeSet::new(),
                    dirs: BTreeSet::new(),
                    generation: 0,
                    settings: VaultSettings::plain(),
                    dirty: false,
                    root_dir: None,
                    cancel: worker_cancel,
                };
                while let Ok(first) = cmd_rx.recv() {
                    if st.cancelled() {
                        break; // queued commands are discarded
                    }
                    // coalesce a burst into one snapshot
                    let mut batch = vec![first];
                    while let Ok(c) = cmd_rx.try_recv() {
                        batch.push(c);
                    }
                    // processed once its snapshot (if any) is sent
                    let n = batch.len();
                    let done = || {
                        worker_pending.fetch_sub(n, std::sync::atomic::Ordering::AcqRel);
                    };
                    // the last mount wins; changes before it are moot
                    if let Some(i) = batch.iter().rposition(|c| matches!(c, Cmd::Mount(_))) {
                        batch.drain(..i);
                    }
                    let mut mounted = !st.root.as_os_str().is_empty();
                    for c in batch {
                        match c {
                            Cmd::Mount(r) => {
                                st.mount(r);
                                mounted = true;
                            }
                            Cmd::Changed(p) if mounted => st.changed(&p),
                            Cmd::Rescan if mounted => st.rescan(),
                            _ => {}
                        }
                    }
                    if st.cancelled() {
                        break;
                    }
                    // nothing visible changed (e.g. Obsidian rewrote app.json unchanged): no new
                    // snapshot, so layouts and autocomplete keep their state
                    if !mounted || !std::mem::take(&mut st.dirty) {
                        done();
                        continue;
                    }
                    if res_tx.send(st.snapshot()).is_err() {
                        break;
                    }
                    done();
                    wake();
                }
            })
            .map_err(|e| format!("cannot start vault thread: {e}"))?;
        Ok(VaultScanner {
            cancel,
            tx: Some(tx),
            rx,
            worker: Some(worker),
            pending,
            stash: Default::default(),
        })
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.tx {
            self.pending.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            if tx.send(c).is_err() {
                self.pending.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            }
        }
    }

    /// Test hook: every command sent so far is processed and its snapshot (if any) was taken
    /// with [`VaultScanner::try_recv`]; the scanner publishes nothing more until asked again.
    #[doc(hidden)]
    pub fn is_idle(&self) -> bool {
        // the worker sends a snapshot before it counts the command as processed: once the
        // count is zero, every snapshot is already in the channel
        if self.pending.load(std::sync::atomic::Ordering::Acquire) != 0 {
            return false;
        }
        let mut stash = self.stash.lock().unwrap_or_else(|e| e.into_inner());
        while let Ok(i) = self.rx.try_recv() {
            *stash = Some(i);
        }
        stash.is_none()
    }

    /// Scans `root` from scratch (a folder was mounted).
    pub fn mount(&self, root: PathBuf) {
        self.send(Cmd::Mount(root));
    }

    /// Something changed in (or at) `path` (watcher event).
    pub fn changed(&self, path: PathBuf) {
        self.send(Cmd::Changed(path));
    }

    /// Walks the mounted folder again (the folder watch restarted).
    pub fn rescan(&self) {
        self.send(Cmd::Rescan);
    }

    /// The newest snapshot published since the last call, if any.
    pub fn try_recv(&self) -> Option<Arc<VaultIndex>> {
        let mut last = self.stash.lock().unwrap_or_else(|e| e.into_inner()).take();
        while let Ok(i) = self.rx.try_recv() {
            last = Some(i);
        }
        last
    }

    /// Waits up to `timeout` for a snapshot (tests).
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<Arc<VaultIndex>> {
        if let Some(i) = self.stash.lock().unwrap_or_else(|e| e.into_inner()).take() {
            return Some(i);
        }
        self.rx.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests;
