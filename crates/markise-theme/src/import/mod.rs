//! CSS theme importer (spec 03 §2): a tolerant CSS-subset reader that turns a theme written
//! against the spec 03 DOM contract into a native [`Theme`].
//!
//! Pipeline: parse (cssparser) → inline `@import`s, evaluate `@media`, collect `@font-face`
//! ([`sheet`]) → match selectors against a synthetic editor DOM ([`dom`], [`selector`]) →
//! cascade, custom properties, `var()`, inheritance ([`cascade`]) → read native slots from the
//! computed styles ([`map`]). Layering (spec 03 §1.2): built-in base → `{name}.css` (with its
//! imports inlined) → `base.user.css` → `{name}.user.css`. See `docs/adr/0003-theme-import.md`.
//!
//! The importer never panics on arbitrary input and bounds its work: file size, total size,
//! import depth and file count, rule and declaration counts, selector and `var()` nesting,
//! substituted value length (see [`ImportOptions`]).

mod cascade;
mod dom;
mod map;
mod report;
mod selector;
mod sheet;
mod value;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use report::{ImportReport, Issue, Location, MAX_LOCATIONS};

use crate::Theme;
use cascade::{Outcome, Variant};
use report::ReportBuilder;
use sheet::{Cond, Loader};
use value::{PropClass, classify};

/// Importer options and work limits.
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// Theme display name; defaults to the file-name rule of A20, or "Imported Theme".
    pub name: Option<String>,
    /// Nominal viewport for `@media (min-width…)`, `vw`/`vh` and percentages (CSS px).
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// For `-webkit-min-device-pixel-ratio` / `min-resolution` queries.
    pub device_pixel_ratio: f32,
    /// Largest single CSS file read (bytes); larger files are skipped (larger strings truncated).
    pub max_file_bytes: usize,
    /// Total CSS bytes across the theme, its imports and user layers.
    pub max_total_bytes: usize,
    /// Maximum `@import` nesting (spec 03 §1.5: ≤ 10).
    pub max_import_depth: u32,
    /// Maximum number of files (theme + imports + user layers).
    pub max_files: usize,
    /// Maximum style rules kept.
    pub max_rules: usize,
    /// Maximum declarations kept.
    pub max_declarations: usize,
    /// Maximum selectors kept (each is matched against every modelled element).
    pub max_selectors: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            name: None,
            viewport_width: 1100.0,
            viewport_height: 780.0,
            device_pixel_ratio: 2.0,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_bytes: 8 * 1024 * 1024,
            max_import_depth: 10,
            max_files: 64,
            max_rules: 20_000,
            max_declarations: 100_000,
            max_selectors: 50_000,
        }
    }
}

/// Result of an import.
#[derive(Clone, Debug)]
pub struct ImportResult {
    /// The theme as shown without an OS preference: for scheme-aware themes the light
    /// variant, otherwise the only variant (dark flag by the luma rule, spec 03 §1.4).
    pub theme: Theme,
    /// Dark variant of a scheme-aware theme (`@media (prefers-color-scheme: dark)`).
    pub dark_variant: Option<Theme>,
    /// `@media print` blocks, verbatim, kept for PDF export (spec 03 §6).
    pub print_css: Vec<String>,
    /// `@include-when-export` URLs (spec 03 §1.5), for HTML/PDF export.
    pub include_when_export: Vec<String>,
    pub report: ImportReport,
}

impl ImportResult {
    /// The variant to use for the given OS appearance.
    pub fn variant(&self, os_dark: bool) -> &Theme {
        match (&self.dark_variant, os_dark) {
            (Some(d), true) => d,
            _ => &self.theme,
        }
    }
}

/// One stylesheet layer.
struct Layer {
    css: String,
    /// Path of the file (for imports, cycle detection and reporting); `None` for strings.
    path: Option<PathBuf>,
    base_dir: Option<PathBuf>,
}

/// Import a theme from a CSS string. `base_dir` resolves relative `@import`s and font URLs.
pub fn import_css(css: &str, base_dir: &Path, opts: &ImportOptions) -> ImportResult {
    let name = opts.name.clone().unwrap_or_else(|| "Imported Theme".to_string());
    import_layers(
        vec![Layer {
            css: css.to_string(),
            path: None,
            base_dir: Some(base_dir.to_path_buf()),
        }],
        name,
        opts,
    )
}

/// Import a theme file with its user layers (spec 03 §1.2): `{name}.css`, then
/// `base.user.css` and `{name}.user.css` from the same folder when present. The display name
/// follows A20 unless `opts.name` is set.
pub fn import_theme_file(path: &Path, opts: &ImportOptions) -> std::io::Result<ImportResult> {
    let meta = std::fs::metadata(path)?;
    if meta.len() as usize > opts.max_file_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: larger than the {} byte limit", path.display(), opts.max_file_bytes),
        ));
    }
    let css =
        String::from_utf8_lossy(&markise_doc::resource::read_regular(path, opts.max_file_bytes as u64)?).into_owned();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut layers = vec![Layer {
        css,
        path: Some(path.to_path_buf()),
        base_dir: Some(dir.clone()),
    }];
    for user in ["base.user.css".to_string(), format!("{stem}.user.css")] {
        let p = dir.join(&user);
        if p.is_file()
            && let Ok(meta) = std::fs::metadata(&p)
            && (meta.len() as usize) <= opts.max_file_bytes
            && let Ok(bytes) = markise_doc::resource::read_regular(&p, opts.max_file_bytes as u64)
        {
            layers.push(Layer {
                css: String::from_utf8_lossy(&bytes).into_owned(),
                path: Some(p),
                base_dir: Some(dir.clone()),
            });
        }
    }
    let name = opts.name.clone().unwrap_or_else(|| display_name(&stem));
    Ok(import_layers(layers, name, opts))
}

/// Menu label from a theme file stem (A20): split on `-`/`_`, capitalise each word.
pub fn display_name(stem: &str) -> String {
    stem.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Built-in base layer (original): the metric defaults a theme can rely on, like a browser's
/// UA stylesheet, so that inherited font sizes and weights do not flatten headings. Heading
/// sizes are the built-in themes' ratios (32/25/20.5/17.5/16/15 px at a 16 px body). Colors
/// are not set here: the built-in light/dark theme supplies them.
const BASE_CSS: &str = "
h1 { font-size: 2em; font-weight: bold }
h2 { font-size: 1.5625em; font-weight: bold }
h3 { font-size: 1.28125em; font-weight: bold }
h4 { font-size: 1.09375em; font-weight: bold }
h5 { font-size: 1em; font-weight: bold }
h6 { font-size: 0.9375em; font-weight: bold }
";

/// File index used for locations in the built-in base layer.
const BASE_FILE: usize = usize::MAX;

fn import_layers(layers: Vec<Layer>, name: String, opts: &ImportOptions) -> ImportResult {
    let mut rb = ReportBuilder::default();
    let (sheet, base_rules) = {
        let mut loader = Loader::new(opts, &mut rb);
        loader.load_text(BASE_CSS, None, None, BASE_FILE, 0, Cond::Always);
        let base_rules = loader.sheet.rules.len();
        loader.base_rules = base_rules;
        let base_selectors = loader.sheet.selectors;
        let base_decls = loader.sheet.declarations;
        loader.sheet.selectors -= base_selectors;
        loader.sheet.declarations -= base_decls;
        // TI-9: the theme folder is the folder of the root theme CSS (the first layer)
        loader.root_dir = layers
            .first()
            .and_then(|l| l.base_dir.as_ref())
            .and_then(|d| d.canonicalize().ok());
        for layer in &layers {
            let idx = loader.add_file(layer.path.clone().unwrap_or_else(|| PathBuf::from("<input>")));
            loader.note_bytes(layer.css.len());
            loader.load_text(
                &layer.css,
                layer.base_dir.clone(),
                layer.path.as_deref(),
                idx,
                0,
                Cond::Always,
            );
        }
        (loader.sheet, base_rules)
    };
    let dom = dom::Dom::build();
    let matches = cascade::match_rules(&sheet, &dom, base_rules);
    let mut outcomes: HashMap<(u32, u32), Outcome> = HashMap::new();

    let (theme, dark_variant) = if sheet.scheme_aware {
        let light = cascade::compute(&sheet, &dom, &matches, Variant::Light, opts, &mut outcomes);
        let dark = cascade::compute(&sheet, &dom, &matches, Variant::Dark, opts, &mut outcomes);
        let lt = map::build_theme(&light, &dom, &sheet, &name, Some(Variant::Light));
        let dt = map::build_theme(&dark, &dom, &sheet, &name, Some(Variant::Dark));
        (lt, Some(dt))
    } else {
        let c = cascade::compute(&sheet, &dom, &matches, Variant::Light, opts, &mut outcomes);
        (map::build_theme(&c, &dom, &sheet, &name, None), None)
    };

    // report
    let mut report = ImportReport {
        files: std::mem::take(&mut rb.files),
        scheme_aware: sheet.scheme_aware,
        dark: theme.dark,
        rules: sheet.rules.len() - base_rules,
        selectors: sheet.selectors,
        selectors_matched: matches.selectors_matched,
        declarations: sheet.declarations,
        font_faces: sheet.faces.len(),
        ..ImportReport::default()
    };
    for (ri, rule) in sheet.rules.iter().enumerate().skip(base_rules) {
        if !matches.rule_matched[ri] {
            continue;
        }
        for (di, d) in rule.decls.iter().enumerate() {
            if d.name.starts_with("--") {
                continue; // definitions, not visual declarations
            }
            report.content_declarations += 1;
            match classify(&d.name) {
                PropClass::Supported => match outcomes.get(&(ri as u32, di as u32)) {
                    Some(Outcome::Applied) | None => report.applied += 1,
                    Some(Outcome::BadValue) => rb.values.add(&format!("{}: {}", d.name, short(&d.value)), d.loc),
                    Some(Outcome::Ignored) => {
                        report.ignored += 1;
                        rb.ignored.add(&d.name, d.loc);
                    }
                },
                PropClass::Ignored => {
                    report.ignored += 1;
                    rb.ignored.add(&d.name, d.loc);
                }
                PropClass::Unsupported => rb.properties.add(&d.name, d.loc),
            }
        }
    }
    report.unsupported_selectors = std::mem::take(&mut rb.selectors).finish();
    report.unsupported_properties = std::mem::take(&mut rb.properties).finish();
    report.unsupported_values = std::mem::take(&mut rb.values).finish();
    report.unsupported_at_rules = std::mem::take(&mut rb.at_rules).finish();
    report.ignored_properties = std::mem::take(&mut rb.ignored).finish();
    report.warnings = std::mem::take(&mut rb.warnings);
    report.attempted = std::mem::take(&mut rb.attempted);

    ImportResult {
        theme,
        dark_variant,
        print_css: sheet.print_css,
        include_when_export: sheet.include_when_export,
        report,
    }
}

fn short(v: &str) -> String {
    let v = v.trim();
    if v.chars().count() > 60 {
        let s: String = v.chars().take(57).collect();
        format!("{s}...")
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests;
