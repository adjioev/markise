//! `cargo xtask assets`: every committed asset must have a license/attribution record.
//!
//! Derived from WordCraft's `xtask/src/assets.rs` (https://github.com/storytold/wordcraft, rev
//! 6a2b520fa49aa0d0595cea36589aaf16ca371cd3, MIT OR Apache-2.0, Copyright (c) 2026 ArtCraft Team
//! and the WordCraft contributors; see NOTICE). Changes for Markise: instead of one
//! ATTRIBUTION.md table, an asset is attributed by a `SOURCE.md`/`SOURCES.md` record in its
//! folder or a parent folder that names it, or by a `NOTICE` entry naming the file or one of its
//! folders. A record names a path only as a complete delimited path (a backticked span, a
//! Markdown link target, or a whitespace-separated word), never as part of a longer name.
//!
//! Assets are every git-tracked (and untracked, not ignored) file under `assets/`, `site/assets/`
//! and test-fixture folders (`tests/fixtures/`, `testdata/`), plus every file with an image,
//! font, media or document extension anywhere. License and record documents themselves
//! (`LICENSE`, `LICENSE-MIT`, `COPYING.txt`, `NOTICE.md`, `SOURCE.md`, `SOURCES.md`,
//! `README.md`; never a file with a media extension such as `notice.svg`) are not assets.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const ASSET_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "tif", "tiff", "ico", "icns", "svg", "pdf", "ai", "eps", "psd",
    "icc", "icm", "ttf", "otf", "ttc", "woff", "woff2", "docx", "odt", "rtf", "epub", "mp4", "webm", "wav", "mp3",
    "ogg",
];
const ASSET_DIRS: &[&str] = &["assets/", "site/assets/"];
const FIXTURE_DIRS: &[&str] = &["tests/fixtures/", "testdata/"];
/// Folders never scanned: throwaway spikes are separate workspaces and ship nothing.
const SKIP_DIRS: &[&str] = &["spikes/", "target/"];
const RECORD_FILES: &[&str] = &["SOURCE.md", "SOURCES.md"];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

/// License and attribution documents: `SOURCE.md`, `SOURCES.md`, `README.md`, and
/// `LICENSE`/`LICENCE`/`COPYING`/`NOTICE`, optionally with a `-`/`_` suffix naming the license
/// (`LICENSE-MIT`, `OFL_LICENSE`… is not one) and a text extension (`.txt`, `.md`). Anything with
/// another extension (`notice.svg`, `license-screen.png`) is not a document.
fn is_meta(path: &str) -> bool {
    let name = file_name(path);
    if RECORD_FILES.contains(&name) || name == "README.md" {
        return true;
    }
    let stem = match extension(name).as_deref() {
        None => name,
        Some("txt" | "md") => Path::new(name)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or(name),
        Some(_) => return false,
    };
    let upper = stem.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "NOTICE"].iter().any(|p| {
        upper.strip_prefix(p).is_some_and(|rest| {
            rest.is_empty()
                || (rest.starts_with(['-', '_'])
                    && rest[1..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.')))
        })
    })
}

pub fn is_asset(path: &str) -> bool {
    if is_meta(path) || SKIP_DIRS.iter().any(|d| path.starts_with(d)) {
        return false;
    }
    let ext = extension(path);
    ASSET_DIRS.iter().any(|d| path.starts_with(d))
        || FIXTURE_DIRS
            .iter()
            .any(|d| path.starts_with(d) || path.contains(&format!("/{d}")))
        || ext.is_some_and(|e| ASSET_EXT.contains(&e.as_str()))
}

/// The complete paths a record names: every code span (CommonMark: a run of n backticks up to
/// the next run of exactly n, so ``` ``a b`` ``` is one span), every Markdown link target
/// (`[text](path)`), and every whitespace-separated word of the remaining text with surrounding
/// brackets/quotes and trailing punctuation removed. A path with spaces is only named when it is
/// backticked or linked as a whole, so `` `large cat.png` `` does not name `cat.png`.
pub fn named_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        if b[i] == b'`' {
            // A code span (CommonMark 6.1): an opening run of n backticks closes at the next run
            // of exactly n backticks; without one the run is literal text.
            let n = b[i..].iter().take_while(|&&c| c == b'`').count();
            match closing_backtick_run(text, i + n, n) {
                Some(close) => {
                    out.push(code_span_content(&text[i + n..close]));
                    rest.push(' ');
                    i = close + n;
                }
                None => {
                    rest.push_str(&text[i..i + n]);
                    i += n;
                }
            }
            continue;
        }
        if b[i] == b']'
            && text[i + 1..].starts_with('(')
            && let Some(j) = text[i + 2..].find(')')
        {
            out.push(text[i + 2..i + 2 + j].trim().to_owned());
            rest.push(' ');
            i += 2 + j + 1;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or_default();
        rest.push(ch);
        i += ch.len_utf8();
    }
    for word in rest.split_whitespace() {
        let word = word
            .trim_start_matches(['(', '[', '<', '"', '\'', '*', '|'])
            .trim_end_matches([')', ']', '>', '"', '\'', '*', '|', ',', ';', ':', '!', '?', '.']);
        if !word.is_empty() {
            out.push(word.to_owned());
        }
    }
    out
}

/// Start of the first run of exactly `n` backticks at or after `from`.
fn closing_backtick_run(text: &str, from: usize, n: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = from;
    while i < b.len() {
        if b[i] == b'`' {
            let run = b[i..].iter().take_while(|&&c| c == b'`').count();
            if run == n {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Code span content per CommonMark: line endings become spaces, and one leading and one
/// trailing space are removed when both are present and the content is not all spaces.
fn code_span_content(raw: &str) -> String {
    let c = raw.replace("\r\n", " ").replace(['\n', '\r'], " ");
    if c.len() >= 2 && c.starts_with(' ') && c.ends_with(' ') && c.bytes().any(|x| x != b' ') {
        c[1..c.len() - 1].to_owned()
    } else {
        c
    }
}

/// `needle` is one of the complete paths `hay` names (see [`named_paths`]).
pub fn mentions(hay: &str, needle: &str) -> bool {
    !needle.is_empty() && named_paths(hay).iter().any(|p| p == needle)
}

/// Parent folders of `path`, nearest first, each with a trailing `/` (`a/b/c.png` → `a/b/`, `a/`).
fn parents(path: &str) -> impl Iterator<Item = &str> {
    path.rmatch_indices('/').map(move |(i, _)| &path[..=i])
}

/// The record that attributes `path`, if any. `records` maps a folder (with trailing `/`, or ""
/// for the root) to the text of the `SOURCE.md`/`SOURCES.md` files in it.
pub fn attribution(path: &str, notice: &str, records: &BTreeMap<String, String>) -> Option<String> {
    if mentions(notice, path) {
        return Some("NOTICE".into());
    }
    // A folder entry in NOTICE covers its files; at least two levels deep (`assets/brand/`), so a
    // passing mention of `assets/` attributes nothing.
    for dir in parents(path).filter(|d| d.matches('/').count() >= 2) {
        if mentions(notice, dir) {
            return Some(format!("NOTICE ({dir})"));
        }
    }
    for dir in parents(path).chain(std::iter::once("")) {
        if let Some(text) = records.get(dir)
            && mentions(text, &path[dir.len()..])
        {
            return Some(format!("{dir}SOURCE.md"));
        }
    }
    None
}

/// Asset paths that no record attributes.
pub fn missing(files: &[String], notice: &str, records: &BTreeMap<String, String>) -> Vec<String> {
    files
        .iter()
        .filter(|f| is_asset(f) && attribution(f, notice, records).is_none())
        .cloned()
        .collect()
}

/// The paths in `git ls-files -z` output: NUL-separated and never quoted, so a name with
/// non-ASCII letters or a newline is still checked.
fn git_paths(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

pub fn run(root: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    // A failed listing is no listing: read as empty, it would pass with nothing checked.
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let files: Vec<String> = git_paths(&out.stdout)
        .into_iter()
        .filter(|l| root.join(l).is_file())
        .collect();
    let notice = std::fs::read_to_string(root.join("NOTICE")).map_err(|e| format!("NOTICE: {e}"))?;
    let mut records: BTreeMap<String, String> = BTreeMap::new();
    for f in files.iter().filter(|f| RECORD_FILES.contains(&file_name(f))) {
        let text = std::fs::read_to_string(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
        let dir = &f[..f.len() - file_name(f).len()];
        records.entry(dir.to_owned()).or_default().push_str(&text);
    }
    let count = files.iter().filter(|f| is_asset(f)).count();
    let miss = missing(&files, &notice, &records);
    if miss.is_empty() {
        println!("assets: all {count} asset files have an attribution record (SOURCE.md or NOTICE)");
        Ok(())
    } else {
        Err(format!(
            "{} asset file(s) lack an attribution record; name each file in a SOURCE.md next to it \
             (or in a parent folder) or its folder/file in NOTICE (origin, author, license):\n  {}",
            miss.len(),
            miss.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(d, t)| ((*d).to_owned(), (*t).to_owned()))
            .collect()
    }

    #[test]
    fn classifies_assets() {
        for a in [
            "assets/themes/x/x.css",
            "assets/brand/tools/build.sh",
            "site/assets/hero.jpg",
            "docs/images/a.PNG",
            "crates/markise-md/tests/fixtures/sample.md",
            "testdata/a.txt",
            "fonts/Inter.woff2",
        ] {
            assert!(is_asset(a), "{a}");
        }
        for n in [
            "crates/x/src/lib.rs",
            "site/index.html",
            "assets/themes/x/LICENSE",
            "assets/icons/lucide/SOURCE.md",
            "assets/brand/README.md",
            "spikes/adr-0005/x/logo.png",
            "crates/markise-theme/tests/theme-corpus.tsv",
        ] {
            assert!(!is_asset(n), "{n}");
        }
    }

    #[test]
    fn token_matching() {
        assert!(mentions("Files: `folder.svg`, `file.svg`.", "file.svg"));
        assert!(mentions("see assets/brand/): x", "assets/brand/"));
        assert!(mentions("ends with file.svg.", "file.svg"));
        assert!(mentions("file.svg", "file.svg"));
        assert!(!mentions("`my-file.svg`", "file.svg"));
        assert!(!mentions("`file.svg.bak`", "file.svg"));
        assert!(!mentions("assets/themes/palettes/SOURCES.md", "assets/"));
        assert!(!mentions("x/assets/brand/ y", "assets/brand/"));
        assert!(!mentions("anything", ""));
        // Paths with spaces: only the complete delimited path counts.
        assert!(!mentions("| `large cat.png` | photo |", "cat.png"));
        assert!(mentions("| `large cat.png` | photo |", "large cat.png"));
        assert!(!mentions("a large cat.png here", "large cat.png"));
        assert!(!mentions("[photo](img/large cat.png)", "cat.png"));
        assert!(mentions("see [logo](img/logo.png).", "img/logo.png"));
        // Code spans with longer backtick runs (CommonMark): the whole span is one path.
        assert!(!mentions("| ``large cat.png`` | photo |", "cat.png"));
        assert!(mentions("| ``large cat.png`` | photo |", "large cat.png"));
        assert!(!mentions("```large cat.png```", "cat.png"));
        assert!(mentions("```large cat.png```", "large cat.png"));
        assert!(mentions("`` odd`name.png ``", "odd`name.png"));
        // A run only closes on a run of the same length.
        assert!(!mentions("``large ` cat.png``", "cat.png"));
        // An unmatched run is literal text and stays attached to its word.
        assert!(!mentions("``a.png and b.png", "a.png"));
        assert!(mentions("``a.png and b.png", "b.png"));
        assert!(mentions("- \"quoted/path.svg\",", "quoted/path.svg"));
    }

    #[test]
    fn only_license_documents_are_exempt() {
        for doc in [
            "assets/themes/x/LICENSE",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "assets/fonts/LICENSE.txt",
            "assets/x/COPYING",
            "assets/x/NOTICE.md",
            "assets/x/licence_ofl-1.1.txt",
            "assets/x/SOURCE.md",
            "assets/x/README.md",
        ] {
            assert!(!is_asset(doc), "{doc}");
        }
        for asset in [
            "assets/icons/notice.svg",
            "assets/license-screen.png",
            "assets/x/LICENSE.svg",
            "assets/x/COPYING.pdf",
            "assets/x/noticeboard.txt",
            "assets/x/licenses.css",
        ] {
            assert!(is_asset(asset), "{asset}");
        }
        let files = vec![
            "assets/icons/notice.svg".to_string(),
            "assets/license-screen.png".into(),
        ];
        assert_eq!(missing(&files, "", &BTreeMap::new()), files);
    }

    #[test]
    fn a_name_with_spaces_does_not_attribute_its_last_word() {
        for record in ["| `large cat.png` | CC0 |", "| ``large cat.png`` | CC0 |"] {
            let recs = records(&[("assets/photos/", record)]);
            let files = vec![
                "assets/photos/large cat.png".to_string(),
                "assets/photos/cat.png".into(),
            ];
            assert_eq!(
                missing(&files, "", &recs),
                vec!["assets/photos/cat.png".to_string()],
                "{record}"
            );
        }
    }

    #[test]
    fn unattributed_assets_fail() {
        let files: Vec<String> = [
            "assets/icons/lucide/folder.svg",
            "assets/icons/lucide/file.svg",
            "assets/icons/lucide/new.svg",
            "assets/themes/drake/drake/font.css",
            "assets/brand/png/markise-16.png",
            "site/assets/shots/a.jpg",
            "site/assets/hero.jpg",
            "docs/images/b.png",
            "crates/x/src/lib.rs",
        ]
        .map(String::from)
        .to_vec();
        let notice = "Brand artwork (assets/brand/): owner. Mentions assets/ in passing.";
        let recs = records(&[
            ("assets/icons/lucide/", "Files: `folder.svg`, `file.svg`."),
            ("assets/themes/drake/", "| `drake/font.css` | none |"),
            ("site/assets/", "| `hero.jpg` | owner |"),
        ]);
        assert_eq!(
            missing(&files, notice, &recs),
            vec![
                "assets/icons/lucide/new.svg",
                "site/assets/shots/a.jpg",
                "docs/images/b.png"
            ]
        );
        assert_eq!(
            attribution("assets/brand/png/markise-16.png", notice, &recs).as_deref(),
            Some("NOTICE (assets/brand/)")
        );
        assert_eq!(
            attribution("assets/themes/drake/drake/font.css", notice, &recs).as_deref(),
            Some("assets/themes/drake/SOURCE.md")
        );
    }

    #[test]
    fn a_record_in_a_parent_folder_names_the_relative_path() {
        let recs = records(&[("site/assets/", "| `shots/a.jpg` | screenshot |")]);
        assert!(attribution("site/assets/shots/a.jpg", "", &recs).is_some());
        // The bare file name does not attribute a file in a subfolder.
        let recs = records(&[("site/assets/", "| `a.jpg` | screenshot |")]);
        assert!(attribution("site/assets/shots/a.jpg", "", &recs).is_none());
    }

    #[test]
    fn reads_raw_nul_separated_paths() {
        assert_eq!(
            git_paths(b"assets/\xc3\xa9 b.png\0docs/images/x.svg\0"),
            vec!["assets/é b.png".to_string(), "docs/images/x.svg".into()]
        );
        assert!(git_paths(b"").is_empty());
    }

    #[test]
    fn a_failed_file_listing_fails_the_check() {
        // Outside a git checkout `git ls-files` fails; its empty output must not pass the check.
        let dir = std::env::temp_dir().join(format!("markise-xtask-assets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("NOTICE"), "").unwrap();
        let inside_git = Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "--git-dir"])
            .output()
            .is_ok_and(|o| o.status.success());
        let result = run(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        if inside_git {
            return; // the temporary folder is in a checkout here: nothing to test
        }
        assert!(
            result.as_ref().is_err_and(|e| e.starts_with("git ls-files failed")),
            "{result:?}"
        );
    }
}
