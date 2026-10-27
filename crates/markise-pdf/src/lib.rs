//! `markise-pdf` (L3): paginated layout of the export model and its PDF and PNG back ends
//! (spec 04 §8.1).
//!
//! - [`layout`]: blocks → pages of positioned items (shaping by `markise-text`, ADR 0005).
//! - [`pdf`]: Krilla back end (selectable text, links, outline, tagged PDF).
//! - [`png`]: vello_cpu back end for "Export → Image".
//!
//! Math and diagrams are drawn as styled source (rendering postponed by the owner).

pub mod image;
pub mod layout;
pub mod pdf;
pub mod png;
pub mod samples;

use std::path::PathBuf;
use std::time::Duration;

use markise_export::job::JobControl;
use markise_export::model::ExDoc;
use markise_export::vars::Vars;
use markise_text::{FontStore, TextEngine};
use markise_theme::Theme;

pub use layout::{Laid, LayoutOptions};
pub use pdf::{PdfError, PdfMeta};

/// Millimetres → points.
pub const MM: f32 = 72.0 / 25.4;

/// Paper sizes of spec 04 §8.1 (portrait, points).
pub fn paper_size(name: &str) -> Option<(f32, f32)> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
    Some(match n.as_str() {
        "a3" => (297.0 * MM, 420.0 * MM),
        "a4" => (210.0 * MM, 297.0 * MM),
        "a5" => (148.0 * MM, 210.0 * MM),
        "b5" => (176.0 * MM, 250.0 * MM),
        "letter" | "usletter" => (612.0, 792.0),
        "legal" | "uslegal" => (612.0, 1008.0),
        "tabloid" => (792.0, 1224.0),
        _ => {
            // custom "W H" / "WxH" with units
            let parts: Vec<&str> = name
                .split(|c: char| c.is_whitespace() || c == 'x' || c == '×' || c == ',')
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() == 2 {
                let w = parse_length(parts[0])?;
                let h = parse_length(parts[1])?;
                if w > 36.0 && h > 36.0 && w < 14_400.0 && h < 14_400.0 {
                    return Some((w, h));
                }
            }
            return None;
        }
    })
}

/// Parses a CSS-like length into points: `mm`, `cm`, `in`, `pt`, `px` (bare numbers are px,
/// open question C58).
pub fn parse_length(s: &str) -> Option<f32> {
    let s = s.trim().to_ascii_lowercase();
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s.as_str(), "px"),
    };
    let v: f32 = num.trim().parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    Some(match unit {
        "mm" => v * MM,
        "cm" => v * 10.0 * MM,
        "in" => v * 72.0,
        "pt" => v,
        "px" => v * 0.75,
        _ => return None,
    })
}

/// PDF export settings (spec 04 §8.1; front-matter overrides spec 05 §3).
#[derive(Clone, Debug)]
pub struct PdfOptions {
    /// Page size in points (portrait).
    pub paper: (f32, f32),
    /// Margins in points: top, right, bottom, left.
    pub margins: [f32; 4],
    pub header: Option<String>,
    pub footer: Option<String>,
    pub break_before_h1: bool,
    pub theme: Theme,
    pub base_dir: Option<PathBuf>,
    /// Write a tagged (accessible) PDF.
    pub tagged: bool,
    /// Title when the document has neither a `title` key nor a heading (the file name).
    pub fallback_title: String,
    pub language: Option<String>,
}

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions {
            paper: paper_size("a4").unwrap_or((595.0, 842.0)),
            margins: [18.0 * MM, 16.0 * MM, 18.0 * MM, 16.0 * MM],
            header: None,
            footer: None,
            break_before_h1: false,
            theme: Theme::light(),
            base_dir: None,
            tagged: true,
            fallback_title: String::new(),
            language: None,
        }
    }
}

impl PdfOptions {
    /// Applies the export front-matter keys: `papersize`, `margin`, `margin-left|right|top|
    /// bottom`, `header`, `footer` (spec 05 §3; `unset` means "not set").
    pub fn apply_front_matter(&mut self, doc: &ExDoc) {
        if let Some(p) = doc.meta_value("papersize").and_then(paper_size) {
            self.paper = p;
        }
        if let Some(m) = doc.meta_value("margin") {
            let v: Vec<f32> = m.split_whitespace().filter_map(parse_length).collect();
            match v.as_slice() {
                [a] => self.margins = [*a; 4],
                [a, b] => self.margins = [*a, *b, *a, *b],
                [a, b, c] => self.margins = [*a, *b, *c, *b],
                [a, b, c, d] => self.margins = [*a, *b, *c, *d],
                _ => {}
            }
        }
        for (key, idx) in [
            ("margin-top", 0),
            ("margin-right", 1),
            ("margin-bottom", 2),
            ("margin-left", 3),
        ] {
            if let Some(v) = doc.meta_value(key).and_then(parse_length) {
                self.margins[idx] = v;
            }
        }
        if let Some(h) = doc.meta_value("header") {
            self.header = Some(h.to_string());
        }
        if let Some(f) = doc.meta_value("footer") {
            self.footer = Some(f.to_string());
        }
    }

    /// Margins clamped so at least a third of the page stays for content.
    fn clamped_margins(&self) -> [f32; 4] {
        let (w, h) = self.paper;
        let mut m = self.margins;
        let hx = (m[1] + m[3]).max(1.0);
        if hx > w * 0.66 {
            m[1] *= w * 0.66 / hx;
            m[3] *= w * 0.66 / hx;
        }
        let vy = (m[0] + m[2]).max(1.0);
        if vy > h * 0.66 {
            m[0] *= h * 0.66 / vy;
            m[2] *= h * 0.66 / vy;
        }
        m
    }
}

/// A text engine with the system fonts (blocks up to `timeout` for discovery; call it on a
/// worker thread).
pub fn system_engine(timeout: Duration) -> TextEngine {
    let mut store = FontStore::new();
    store.start_system_discovery();
    store.wait_for_system(timeout);
    TextEngine::new(store)
}

/// Export result.
#[derive(Debug)]
pub struct Output {
    /// One file (PDF) or one PNG per tile.
    pub files: Vec<Vec<u8>>,
    pub pages: usize,
    pub warnings: Vec<String>,
}

/// Errors of [`export_pdf`] / [`export_png`].
#[derive(Debug)]
pub enum ExportError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Cancelled => f.write_str("export cancelled"),
            ExportError::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ExportError {}

/// The theme used for print: dark themes are not printed (spec 04 §8.1); the light built-in
/// replaces them.
pub fn print_theme(theme: &Theme) -> (Theme, Option<String>) {
    if theme.dark {
        (
            Theme::light(),
            Some(format!(
                "\"{}\" is a dark theme; the PDF uses the default light theme",
                theme.name
            )),
        )
    } else {
        (theme.clone(), None)
    }
}

/// Lays out and writes `doc` as PDF. `vars` supplies header/footer variables (file paths,
/// front matter); `title`, `author`, `today` are added here when missing.
pub fn export_pdf(
    eng: &mut TextEngine,
    doc: &ExDoc,
    opts: &PdfOptions,
    vars: &Vars,
    control: &JobControl,
) -> Result<Output, ExportError> {
    let (theme, warn) = print_theme(&opts.theme);
    let title = doc.title(&opts.fallback_title);
    let author = doc.author().map(str::to_string);
    let mut vars = vars.clone();
    if vars.get("title").is_none() {
        vars.set("title", title.clone());
    }
    if vars.get("author").is_none() {
        vars.set("author", author.clone().unwrap_or_default());
    }
    let today = vars.get("today").unwrap_or_else(markise_export::vars::today);
    vars.set("today", today.clone());
    let lo = LayoutOptions {
        page_width: opts.paper.0,
        page_height: opts.paper.1,
        margins: opts.clamped_margins(),
        unit: 0.75,
        theme,
        base_dir: opts.base_dir.clone(),
        break_before_h1: opts.break_before_h1,
        header: opts.header.clone(),
        footer: opts.footer.clone(),
        vars,
    };
    control.set_progress(0.05);
    let laid = layout::layout(eng, doc, &lo, control).map_err(|_| ExportError::Cancelled)?;
    let meta = PdfMeta {
        title: Some(title).filter(|t| !t.is_empty()),
        author,
        subject: doc.meta_value("subject").map(str::to_string),
        language: opts
            .language
            .clone()
            .or_else(|| doc.meta_value("lang").map(str::to_string)),
        date: Some(today),
    };
    let bytes = pdf::write_pdf(&laid, &meta, opts.tagged, control).map_err(|e| match e {
        PdfError::Cancelled => ExportError::Cancelled,
        e => ExportError::Failed(e.to_string()),
    })?;
    let mut warnings = laid.warnings.clone();
    warnings.extend(warn);
    control.set_progress(1.0);
    Ok(Output {
        files: vec![bytes],
        pages: laid.pages.len(),
        warnings,
    })
}

/// PNG export settings (spec 04 §8.1 "Image").
#[derive(Clone, Debug)]
pub struct PngOptions {
    /// Image width in CSS pixels (default 640).
    pub width: f32,
    /// Device pixels per CSS pixel (quality: 1 medium, 2 high, 3 best).
    pub scale: f32,
    pub theme: Theme,
    pub base_dir: Option<PathBuf>,
}

impl Default for PngOptions {
    fn default() -> Self {
        PngOptions {
            width: 640.0,
            scale: 2.0,
            theme: Theme::light(),
            base_dir: None,
        }
    }
}

/// Renders `doc` as one continuous image (split into tiles when very tall).
pub fn export_png(
    eng: &mut TextEngine,
    doc: &ExDoc,
    opts: &PngOptions,
    control: &JobControl,
) -> Result<Output, ExportError> {
    let t = &opts.theme;
    let pad_x = t.column_padding_x.min(opts.width / 6.0);
    let lo = LayoutOptions {
        page_width: opts.width.max(64.0),
        page_height: f32::INFINITY,
        margins: [t.column_padding_y, pad_x, t.column_padding_y, pad_x],
        unit: 1.0,
        theme: t.clone(),
        base_dir: opts.base_dir.clone(),
        break_before_h1: false,
        header: None,
        footer: None,
        vars: Vars::new(),
    };
    control.set_progress(0.05);
    let laid = layout::layout(eng, doc, &lo, control).map_err(|_| ExportError::Cancelled)?;
    let scale = opts.scale.clamp(0.5, 4.0);
    let (files, omitted) = png::render_png(&laid, scale, control).map_err(|e| {
        if e.contains("cancelled") {
            ExportError::Cancelled
        } else {
            ExportError::Failed(e)
        }
    })?;
    control.set_progress(1.0);
    Ok(Output {
        pages: files.len(),
        files,
        warnings: laid.warnings.into_iter().chain(omitted).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_and_papers() {
        assert!((parse_length("10mm").unwrap() - 28.346).abs() < 0.01);
        assert_eq!(parse_length("1in"), Some(72.0));
        assert_eq!(parse_length("96px"), Some(72.0));
        assert_eq!(parse_length("96"), Some(72.0));
        assert_eq!(parse_length("x"), None);
        assert_eq!(paper_size("Letter"), Some((612.0, 792.0)));
        assert_eq!(paper_size("US Legal"), Some((612.0, 1008.0)));
        let (w, h) = paper_size("100mm 200mm").unwrap();
        assert!((w - 283.46).abs() < 0.1 && (h - 566.93).abs() < 0.1);
        assert_eq!(paper_size("nonsense"), None);
    }

    #[test]
    fn front_matter_overrides() {
        let doc = ExDoc::build(
            &markise_md::parse(
                "---\npapersize: letter\nmargin: 1in 2in\nmargin-left: 72pt\nfooter: ${pageNo}\nheader: unset\n---\n# T\n",
            ),
            &Default::default(),
        );
        let mut o = PdfOptions::default();
        o.apply_front_matter(&doc);
        assert_eq!(o.paper, (612.0, 792.0));
        assert_eq!(o.margins, [72.0, 144.0, 72.0, 72.0]);
        assert_eq!(o.footer.as_deref(), Some("${pageNo}"));
        assert_eq!(o.header, None);
    }
}
