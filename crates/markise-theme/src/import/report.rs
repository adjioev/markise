//! The import report (spec 03 §2.4 step 4): what was applied, what was not, and where.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A source position: index into [`ImportReport::files`] and a 1-based line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub file: usize,
    pub line: u32,
}

/// One kind of unsupported item with its occurrence count and (up to
/// [`MAX_LOCATIONS`]) locations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub name: String,
    pub count: usize,
    pub locations: Vec<Location>,
}

/// Locations kept per issue.
pub const MAX_LOCATIONS: usize = 8;

/// What the importer did with a theme.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    /// Files read, in load order (theme, its `@import`s, user layers). Index 0 is the theme
    /// itself; when importing from a string with no path it is `<input>`.
    pub files: Vec<PathBuf>,
    /// The theme contains `@media (prefers-color-scheme: …)` (spec 03 §1.4).
    pub scheme_aware: bool,
    /// The resulting (default) theme has dark chrome.
    pub dark: bool,
    /// Style rules kept (after dropping invalid ones and `@media print`).
    pub rules: usize,
    pub selectors: usize,
    /// Selectors that match an element of the native model (content or chrome color slots).
    pub selectors_matched: usize,
    pub declarations: usize,
    /// Declarations in rules that match a modelled element.
    pub content_declarations: usize,
    /// Of those: understood (supported property, valid value).
    pub applied: usize,
    /// Of those: known but deliberately ignored (layout/animation/vendor properties).
    pub ignored: usize,
    pub font_faces: usize,
    /// Selectors the importer could not evaluate (invalid ones drop their whole rule).
    pub unsupported_selectors: Vec<Issue>,
    /// Unsupported properties in rules that match a modelled element.
    pub unsupported_properties: Vec<Issue>,
    /// Supported properties with values the importer could not parse (`calc()`, …).
    pub unsupported_values: Vec<Issue>,
    /// Known properties deliberately ignored (layout, animation, vendor-prefixed, …) in rules
    /// that match a modelled element, for frequency analysis (C6).
    pub ignored_properties: Vec<Issue>,
    /// At-rules skipped (`@keyframes`, `@page`, remote `@import`, unknown media features, …).
    pub unsupported_at_rules: Vec<Issue>,
    /// Non-fatal problems: missing import files, limits hit, cycles.
    pub warnings: Vec<String>,
    /// Local `@import` targets the importer tried to read, existing or not (resolved against
    /// the importing file's folder, not canonicalized): live reload watches them so a
    /// missing import that appears later reloads the theme.
    pub attempted: Vec<PathBuf>,
}

impl ImportReport {
    /// Share of understood declarations among those that target modelled elements, excluding
    /// deliberately ignored ones: `applied / (applied + unsupported properties + values)`.
    pub fn coverage(&self) -> f64 {
        let bad: usize = self
            .unsupported_properties
            .iter()
            .chain(&self.unsupported_values)
            .map(|i| i.count)
            .sum();
        let denom = self.applied + bad;
        if denom == 0 {
            1.0
        } else {
            self.applied as f64 / denom as f64
        }
    }

    /// `file:line` for a location.
    pub fn describe(&self, loc: Location) -> String {
        let f = self
            .files
            .get(loc.file)
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if loc.file == usize::MAX {
                    "<base>".into()
                } else {
                    "<input>".into()
                }
            });
        format!("{f}:{}", loc.line)
    }
}

/// Accumulates issues by name.
#[derive(Default)]
pub(crate) struct Issues(BTreeMap<String, (usize, Vec<Location>)>);

impl Issues {
    pub fn add(&mut self, name: &str, loc: Location) {
        if self.0.len() >= 4096 && !self.0.contains_key(name) {
            return; // bound memory on adversarial input
        }
        let e = self.0.entry(name.to_string()).or_default();
        e.0 += 1;
        if e.1.len() < MAX_LOCATIONS {
            e.1.push(loc);
        }
    }

    pub fn finish(self) -> Vec<Issue> {
        let mut v: Vec<Issue> = self
            .0
            .into_iter()
            .map(|(name, (count, locations))| Issue { name, count, locations })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        v
    }
}

#[derive(Default)]
pub(crate) struct ReportBuilder {
    pub files: Vec<PathBuf>,
    pub selectors: Issues,
    pub properties: Issues,
    pub values: Issues,
    pub ignored: Issues,
    pub at_rules: Issues,
    pub warnings: Vec<String>,
    pub attempted: Vec<PathBuf>,
}

impl ReportBuilder {
    pub fn warn(&mut self, msg: String) {
        if self.warnings.len() < 256 && !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }
}
