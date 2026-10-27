//! Theme discovery (spec 03 §1.1–§1.3, B3): list the themes in a theme folder.
//!
//! Every top-level `*.css` file except `*.user.css` is one theme; subfolders hold assets and
//! are not listed. The display name follows A20 ([`crate::import::display_name`]); the dark
//! flag comes from importing the theme (spec 03 §1.4).

use std::path::{Path, PathBuf};

use crate::import::{ImportOptions, display_name, import_theme_file};

/// Where a theme was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// `<config>/markise/themes/`
    User,
    /// A third-party theme folder imported read-only (B3).
    Legacy,
}

/// One theme in a theme folder.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeEntry {
    /// Menu label (A20).
    pub name: String,
    /// File stem (the stored setting value).
    pub stem: String,
    pub path: PathBuf,
    /// Dark chrome (luma rule) for the theme's default variant.
    pub dark: bool,
    /// Has light and dark variants (`prefers-color-scheme`).
    pub scheme_aware: bool,
    pub source: ThemeSource,
}

/// The per-user configuration directory (`~/Library/Application Support` on macOS,
/// `%APPDATA%` on Windows, `$XDG_CONFIG_HOME` or `~/.config` elsewhere).
pub fn config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".config")))
    }
}

/// `<config>/markise/themes/`
pub fn user_theme_dir() -> Option<PathBuf> {
    config_dir().map(|c| c.join("markise").join("themes"))
}

/// The third-party theme folders of B3 for this platform (read-only import).
pub fn legacy_theme_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(c) = config_dir() {
        if cfg!(target_os = "macos") {
            v.push(c.join("abnerworks.Typora").join("themes"));
        } else {
            v.push(c.join("Typora").join("themes"));
        }
    }
    v
}

/// List the themes in `dir`, sorted by display name. Missing or unreadable folders yield an
/// empty list. Each theme is imported once to determine its dark flag.
pub fn discover_themes(dir: &Path, source: ThemeSource) -> Vec<ThemeEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let opts = ImportOptions::default();
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = file_name.to_ascii_lowercase();
        if !lower.ends_with(".css") || lower.ends_with(".user.css") || !path.is_file() {
            continue;
        }
        let stem = file_name[..file_name.len() - 4].to_string();
        let (dark, scheme_aware) = match import_theme_file(&path, &opts) {
            Ok(r) => (r.theme.dark, r.theme.scheme_aware),
            Err(_) => continue,
        };
        out.push(ThemeEntry {
            name: display_name(&stem),
            stem,
            path,
            dark,
            scheme_aware,
            source,
        });
        if out.len() >= 1000 {
            break;
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Themes from the user folder, plus the legacy folders when `include_legacy`.
pub fn discover_all(include_legacy: bool) -> Vec<ThemeEntry> {
    let mut v = Vec::new();
    if let Some(d) = user_theme_dir() {
        v.extend(discover_themes(&d, ThemeSource::User));
    }
    if include_legacy {
        for d in legacy_theme_dirs() {
            v.extend(discover_themes(&d, ThemeSource::Legacy));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_top_level_css_except_user_css() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("solar-flare.css"), "body { background: #111; color: #eee }").unwrap();
        std::fs::write(d.join("paper_white.css"), "body { background: #fff }").unwrap();
        std::fs::write(d.join("base.user.css"), "p { color: red }").unwrap();
        std::fs::write(d.join("paper_white.user.css"), "p { color: red }").unwrap();
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        std::fs::create_dir(d.join("solar-flare")).unwrap();
        std::fs::write(d.join("solar-flare").join("extra.css"), "").unwrap();
        let v = discover_themes(d, ThemeSource::User);
        let names: Vec<_> = v.iter().map(|e| (e.name.as_str(), e.dark)).collect();
        assert_eq!(names, vec![("Paper White", false), ("Solar Flare", true)]);
        assert_eq!(v[1].stem, "solar-flare");
        assert!(discover_themes(&d.join("missing"), ThemeSource::User).is_empty());
    }
}
