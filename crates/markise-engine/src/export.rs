//! Export jobs (spec 04 §8): HTML, HTML without styles, PDF, PNG, Pandoc formats and custom
//! commands, run on a worker thread with progress and cancellation ([`JobControl`]).
//!
//! The UI builds an [`ExportRequest`] from the current document snapshot, calls
//! [`spawn_export`] and polls the returned [`ExportJob`]; nothing here touches the UI thread.
//! "Export with Previous" re-runs the last request ([`ExportRequest::again`]).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use markise_export::CommentExport;
use markise_export::job::JobControl;
use markise_export::model::{ExDoc, ModelOptions};
use markise_export::pandoc::{self, PandocFormat, PandocOptions};
use markise_export::vars::{self, Vars};
use markise_export::{CodeHighlight, HtmlOptions, css};
use markise_md::{Document, ParseOptions};
use markise_pdf::{PdfOptions, PngOptions};
use markise_text::TextEngine;
use markise_theme::Theme;

use crate::Wake;

/// What to export to.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportKind {
    /// Self-contained HTML styled with CSS generated from the theme.
    Html,
    /// Bare semantic HTML (no stylesheet).
    HtmlPlain,
    Pdf,
    /// PNG image of the whole document.
    Png,
    Pandoc(PandocFormat),
    /// A user command; `${outputPath}` etc. are substituted (spec 04 §8.3). The command
    /// receives the Markdown file path as `${currentPath}`.
    Custom {
        name: String,
        command: String,
        extension: String,
    },
}

impl ExportKind {
    pub fn label(&self) -> String {
        match self {
            ExportKind::Html => "HTML".into(),
            ExportKind::HtmlPlain => "HTML (without styles)".into(),
            ExportKind::Pdf => "PDF".into(),
            ExportKind::Png => "Image (PNG)".into(),
            ExportKind::Pandoc(f) => f.label().into(),
            ExportKind::Custom { name, .. } => name.clone(),
        }
    }

    pub fn extension(&self) -> String {
        match self {
            ExportKind::Html | ExportKind::HtmlPlain => "html".into(),
            ExportKind::Pdf => "pdf".into(),
            ExportKind::Png => "png".into(),
            ExportKind::Pandoc(f) => f.extension().into(),
            ExportKind::Custom { extension, .. } => extension.clone(),
        }
    }
}

/// One export to run.
#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub kind: ExportKind,
    /// Markdown source snapshot.
    pub text: String,
    /// The document file, when saved (images and relative links resolve against its folder).
    pub doc_path: Option<PathBuf>,
    pub output: PathBuf,
    /// The active theme (HTML CSS, PDF/PNG styling).
    pub theme: Theme,
    pub opened_folder: Option<PathBuf>,
    /// Explicit Pandoc executable (else auto-detected).
    pub pandoc_path: Option<PathBuf>,
    /// Command run after a successful export (variables substituted, shell-quoted).
    pub after_command: Option<String>,
    /// Read export settings from the document's front matter (spec 04 §8.3).
    pub front_matter_overrides: bool,
    /// Timeout for Pandoc and custom commands.
    pub timeout: Duration,
    /// Parse wikilinks (the `markdown.wikilinks` setting, spec 06 V2.1).
    pub wikilinks: bool,
    /// The vault's files: HTML export writes resolved wikilinks as relative links to the
    /// linked files (spec 06 Q-V11); without it they are plain text.
    pub vault: Option<std::sync::Arc<crate::vault::VaultIndex>>,
    /// Parse CriticMarkup comments and suggestions (the `markdown.criticMarkup` setting,
    /// ADR 0012).
    pub critic_markup: bool,
    /// Keep or strip review comments in the output (setting `export.comments`, ADR 0012).
    pub comments: CommentExport,
}

impl ExportRequest {
    pub fn new(kind: ExportKind, text: String, doc_path: Option<PathBuf>, output: PathBuf, theme: Theme) -> Self {
        ExportRequest {
            kind,
            text,
            doc_path,
            output,
            theme,
            opened_folder: None,
            pandoc_path: None,
            after_command: None,
            front_matter_overrides: true,
            timeout: Duration::from_secs(120),
            wikilinks: true,
            vault: None,
            critic_markup: true,
            comments: CommentExport::Keep,
        }
    }

    /// The same export for a new document snapshot ("Export with Previous").
    pub fn again(&self, text: String, doc_path: Option<PathBuf>, theme: Theme) -> Self {
        ExportRequest {
            text,
            doc_path,
            theme,
            ..self.clone()
        }
    }
}

/// A finished export.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportDone {
    pub outputs: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub elapsed: Duration,
    /// Output of the custom/after-export command, if any.
    pub command_output: Option<String>,
}

/// A running export.
pub struct ExportJob {
    pub control: JobControl,
    pub rx: Receiver<Result<ExportDone, String>>,
    pub label: String,
    pub output: PathBuf,
}

impl ExportJob {
    pub fn cancel(&self) {
        self.control.cancel();
    }
    pub fn progress(&self) -> f32 {
        self.control.progress()
    }
}

/// Starts `req` on a worker thread.
pub fn spawn_export(req: ExportRequest, wake: Wake) -> ExportJob {
    let control = JobControl::new();
    let (tx, rx) = channel();
    let label = req.kind.label();
    let output = req.output.clone();
    let c2 = control.clone();
    let tx2 = tx.clone();
    let r = std::thread::Builder::new()
        .name("markise-export".into())
        .spawn(move || {
            let res = run_export(&req, &c2);
            crate::wake_track::publishing();
            let _ = tx.send(res);
            wake();
            crate::wake_track::woken();
        });
    if let Err(e) = r {
        let _ = tx2.send(Err(format!("cannot start export worker: {e}")));
    }
    ExportJob {
        control,
        rx,
        label,
        output,
    }
}

/// The shared text engine with system fonts (font discovery runs once per process).
fn with_engine<T>(f: impl FnOnce(&mut TextEngine) -> T) -> T {
    static ENGINE: OnceLock<Mutex<TextEngine>> = OnceLock::new();
    let m = ENGINE.get_or_init(|| Mutex::new(markise_pdf::system_engine(Duration::from_secs(20))));
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

use markise_export::output::{publish, publish_with};

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Runs `req` synchronously (the worker body; also used by tests and the CLI).
pub fn run_export(req: &ExportRequest, control: &JobControl) -> Result<ExportDone, String> {
    let t0 = Instant::now();
    control.check().map_err(|e| e.to_string())?;
    let doc = Document::parse(
        req.text.clone(),
        ParseOptions {
            wikilinks: req.wikilinks,
            critic_markup: req.critic_markup,
            ..ParseOptions::default()
        },
    );
    let base_dir = req.doc_path.as_deref().and_then(Path::parent).map(Path::to_path_buf);
    let fallback = req.doc_path.as_deref().map(stem).unwrap_or_else(|| stem(&req.output));
    let today = vars::today();
    let fm_yaml = |ex: &ExDoc| ex.meta_yaml.clone();
    let mut warnings = Vec::new();
    let mut outputs = vec![req.output.clone()];
    let mut command_output = None;
    match &req.kind {
        ExportKind::Html | ExportKind::HtmlPlain => {
            let plain = req.kind == ExportKind::HtmlPlain;
            let fm = markise_md::frontmatter::front_matter(&doc);
            let get = |k: &str| {
                markise_md::frontmatter::get(&fm, k)
                    .filter(|v| !v.trim().is_empty() && *v != "unset")
                    .map(str::to_string)
            };
            let opts = HtmlOptions {
                fallback_title: Some(fallback),
                css: (!plain).then(|| css::theme_css(&req.theme)),
                highlight: if plain {
                    CodeHighlight::Off
                } else {
                    CodeHighlight::Classes
                },
                no_style: plain,
                head_extra: req.front_matter_overrides.then(|| get("append-head")).flatten(),
                body_extra: req.front_matter_overrides.then(|| get("append-body")).flatten(),
                images: html_images(&doc, base_dir.as_deref(), &req.output, plain),
                control: Some(control.clone()),
                wikilinks: wikilink_hrefs(req),
                comments: req.comments,
                ..Default::default()
            };
            control.set_progress(0.3);
            let (html, html_warnings) = markise_export::render_document_report(&doc, &opts);
            warnings.extend(html_warnings);
            control.check().map_err(|e| e.to_string())?;
            publish(&req.output, html.as_bytes(), control)?;
        }
        ExportKind::Pdf => {
            let ex = ExDoc::build(
                &doc,
                &ModelOptions {
                    highlight: true,
                    comments: req.comments,
                },
            );
            let mut opts = PdfOptions {
                theme: req.theme.clone(),
                base_dir: base_dir.clone(),
                fallback_title: fallback,
                ..PdfOptions::default()
            };
            if req.front_matter_overrides {
                opts.apply_front_matter(&ex);
            }
            let vars = Vars::for_export(
                req.doc_path.as_deref(),
                &req.output,
                req.opened_folder.as_deref(),
                &today,
            )
            .with_front_matter(&fm_yaml(&ex));
            let out = with_engine(|eng| markise_pdf::export_pdf(eng, &ex, &opts, &vars, control))
                .map_err(|e| e.to_string())?;
            warnings.extend(out.warnings);
            publish(&req.output, &out.files[0], control)?;
        }
        ExportKind::Png => {
            let ex = ExDoc::build(
                &doc,
                &ModelOptions {
                    highlight: true,
                    comments: req.comments,
                },
            );
            let opts = PngOptions {
                theme: req.theme.clone(),
                base_dir: base_dir.clone(),
                ..PngOptions::default()
            };
            let out =
                with_engine(|eng| markise_pdf::export_png(eng, &ex, &opts, control)).map_err(|e| e.to_string())?;
            warnings.extend(out.warnings);
            // every output path is decided before anything is written
            let paths = tile_paths(&req.output, out.files.len())?;
            // extra tiles first, never overwriting (they were not approved in the dialog);
            // on failure the ones already written are removed and the main file is untouched
            let mut written: Vec<&Path> = Vec::new();
            for (path, f) in paths.iter().zip(&out.files).skip(1) {
                if let Err(e) = publish_with(path, f, control, false, &|| {}) {
                    for w in written {
                        let _ = std::fs::remove_file(w);
                    }
                    return Err(e);
                }
                written.push(path);
            }
            if let Err(e) = publish(&paths[0], &out.files[0], control) {
                for w in written {
                    let _ = std::fs::remove_file(w);
                }
                return Err(e);
            }
            outputs = paths;
            if out.files.len() > 1 {
                warnings.push(format!("the image was split into {} tiles", out.files.len()));
            }
        }
        ExportKind::Pandoc(format) => {
            let exe = pandoc::find_pandoc(req.pandoc_path.as_deref())
                .ok_or_else(|| pandoc::PandocError::NotFound.to_string())?;
            let info = pandoc::probe(&exe, control).map_err(|e| e.to_string())?;
            control.set_progress(0.1);
            let ex = ExDoc::build(
                &doc,
                &ModelOptions {
                    comments: req.comments,
                    ..Default::default()
                },
            );
            let mut opts = PandocOptions::new(*format);
            opts.resource_dir = base_dir.clone();
            opts.timeout = req.timeout;
            pandoc::export(&info, &ex, &req.output, &opts, control).map_err(|e| e.to_string())?;
        }
        ExportKind::Custom { command, .. } => {
            let ex = ExDoc::build(
                &doc,
                &ModelOptions {
                    comments: req.comments,
                    ..Default::default()
                },
            );
            let vars = Vars::for_export(
                req.doc_path.as_deref(),
                &req.output,
                req.opened_folder.as_deref(),
                &today,
            )
            .with_front_matter(&fm_yaml(&ex));
            command_output = Some(run_command(command, &vars, base_dir.as_deref(), req.timeout, control)?);
        }
    }
    if let Some(after) = &req.after_command {
        let ex = ExDoc::build(
            &doc,
            &ModelOptions {
                comments: req.comments,
                ..Default::default()
            },
        );
        let vars = Vars::for_export(
            req.doc_path.as_deref(),
            &req.output,
            req.opened_folder.as_deref(),
            &today,
        )
        .with_front_matter(&ex.meta_yaml);
        let out = run_command(after, &vars, base_dir.as_deref(), req.timeout, control)?;
        command_output = Some(command_output.map(|c| c + &out).unwrap_or(out));
    }
    control.set_progress(1.0);
    Ok(ExportDone {
        outputs,
        warnings,
        elapsed: t0.elapsed(),
        command_output,
    })
}

/// Largest image embedded into a self-contained HTML export (larger ones stay links).
pub const MAX_EMBEDDED_IMAGE: u64 = 10 << 20;
/// Total embedded image output (base64) of one HTML export; further images stay links.
pub const EMBED_BUDGET: usize = 64 << 20;

/// How the HTML export writes local images: embedded (styled, self-contained) or relative to
/// the exported file (without styles); unchanged for an unsaved document.
fn html_images(doc: &Document, doc_dir: Option<&Path>, output: &Path, plain: bool) -> markise_export::ImageLinks {
    use markise_export::{Assets, ImageLinks};
    let Some(doc_dir) = doc_dir else {
        return ImageLinks::Keep;
    };
    let assets = Assets {
        doc_dir: doc_dir.to_path_buf(),
        root_url: markise_md::frontmatter::resource_root(doc),
        out_dir: output
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf(),
    };
    if plain {
        ImageLinks::Rebase { assets }
    } else {
        ImageLinks::Embed {
            assets,
            max_bytes: MAX_EMBEDDED_IMAGE,
            budget: EMBED_BUDGET,
        }
    }
}

/// Runs a command template without a shell: it is split into arguments first and variables
/// are substituted inside each argument, so document values cannot inject commands (see
/// `markise_export::process`). Returns stdout; a non-zero exit is an error with stderr.
pub fn run_command(
    template: &str,
    vars: &Vars,
    cwd: Option<&Path>,
    timeout: Duration,
    control: &JobControl,
) -> Result<String, String> {
    use markise_export::process::{ProcError, run_template};
    run_template(template, vars, cwd, timeout, control).map_err(|e| match e {
        ProcError::Failed { code, stderr } => format!(
            "command failed (exit code {}): {}",
            code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            stderr.trim()
        ),
        e => format!("command: {e}"),
    })
}

/// Output paths of a PNG export split into `n` tiles: the chosen file, then `name-2.png`…
/// When any of the extra names already exists, a numbered variant (`name.1-2.png`, …) is
/// chosen for the whole set so no unrelated file is overwritten.
pub fn tile_paths(output: &Path, n: usize) -> Result<Vec<PathBuf>, String> {
    let ext = output
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "png".into());
    let base = stem(output);
    let extra = |prefix: &str| -> Vec<PathBuf> {
        (2..=n)
            .map(|i| output.with_file_name(format!("{prefix}-{i}.{ext}")))
            .collect()
    };
    let mut rest = extra(&base);
    let mut k = 1;
    while rest.iter().any(|p| p.exists()) {
        if k > 999 {
            return Err(format!(
                "cannot find free file names for the {} extra image tiles next to {}",
                n - 1,
                output.display()
            ));
        }
        rest = extra(&format!("{base}.{k}"));
        k += 1;
    }
    Ok(std::iter::once(output.to_path_buf()).chain(rest).collect())
}

/// Wikilink `href`s of an HTML export: the linked file relative to the output folder,
/// percent-encoded (V2.6.3), with the heading's slug as fragment.
fn wikilink_hrefs(req: &ExportRequest) -> markise_export::WikiLinkHrefs {
    let Some(ix) = req.vault.clone() else {
        return markise_export::WikiLinkHrefs(None);
    };
    let from = req.doc_path.as_deref().and_then(|p| ix.rel_of(p));
    let out_dir = req.output.parent().map(Path::to_path_buf).unwrap_or_default();
    markise_export::WikiLinkHrefs(Some(std::sync::Arc::new(move |target: &str, sub: Option<&str>| {
        let f = ix.resolve_wikilink(target, from.as_deref())?;
        let rel = crate::vault::relative_path(&out_dir, &ix.abs(&f.rel))?;
        let mut href = crate::vault::percent_encode_path(&rel);
        if let Some(h) = sub
            .filter(|s| !s.starts_with('^'))
            .and_then(|s| s.split('#').map(str::trim).rfind(|x| !x.is_empty()))
        {
            href.push('#');
            href.push_str(&markise_md::slug::slug(h));
        }
        Some(href)
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: ExportKind, dir: &Path, name: &str) -> ExportRequest {
        ExportRequest::new(
            kind,
            "---\ntitle: T\n---\n# Hello\n\nSome *text* and `code`.\n\n```rust\nfn a() {}\n```\n".into(),
            Some(dir.join("doc.md")),
            dir.join(name),
            Theme::light(),
        )
    }

    /// Spec 06 Q-V11: resolved wikilinks become relative links to the linked file, unresolved
    /// ones plain text; with wikilinks off the brackets stay text.
    #[test]
    fn html_wikilinks_link_to_resolved_notes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let ix = crate::vault::VaultIndex::from_rels(
            root.clone(),
            ["doc.md".to_string(), "Notes/Other note.md".to_string()],
            1,
        );
        let text = "See [[Other note#Part Two|the other]] and [[Missing]].\n";
        let mut r = ExportRequest::new(
            ExportKind::HtmlPlain,
            text.into(),
            Some(root.join("doc.md")),
            root.join("out.html"),
            Theme::light(),
        );
        r.vault = Some(std::sync::Arc::new(ix));
        let c = JobControl::new();
        let out = run_export(&r, &c).unwrap();
        let s = std::fs::read_to_string(&out.outputs[0]).unwrap();
        assert!(
            s.contains("<a href=\"Notes/Other%20note.md#part-two\">the other</a>"),
            "{s}"
        );
        assert!(s.contains(" and Missing."), "{s}");
        r.wikilinks = false;
        let out = run_export(&r, &c).unwrap();
        let s = std::fs::read_to_string(&out.outputs[0]).unwrap();
        assert!(s.contains("[[Missing]]"), "{s}");
    }

    #[test]
    fn html_styled_and_plain() {
        let d = tempfile::tempdir().unwrap();
        let c = JobControl::new();
        let r = run_export(&req(ExportKind::Html, d.path(), "a.html"), &c).unwrap();
        let s = std::fs::read_to_string(&r.outputs[0]).unwrap();
        assert!(s.contains("<style>") && s.contains("#write h1 {") && s.contains("cm-s-inner"));
        let r = run_export(&req(ExportKind::HtmlPlain, d.path(), "b.html"), &c).unwrap();
        let s = std::fs::read_to_string(&r.outputs[0]).unwrap();
        assert!(!s.contains("<style"), "{s}");
        assert!(s.contains("<h1 id='hello'>Hello</h1>"));
        assert!(s.contains("<title>T</title>"));
    }

    fn png_bytes() -> Vec<u8> {
        // 1×1 PNG
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0,
            1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 13, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
            0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E,
            0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    /// Percent-decodes an `src` attribute value for resolving it as a path in tests.
    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn src_values(html: &str) -> Vec<String> {
        html.split("<img src=\"")
            .skip(1)
            .map(|r| r.split('"').next().unwrap().to_string())
            .collect()
    }

    // Codex round 6: export to another folder keeps local images working.
    #[test]
    fn html_export_to_another_folder_keeps_images() {
        let d = tempfile::tempdir().unwrap();
        let docs = d.path().join("docs");
        let pics = docs.join("мои картинки");
        std::fs::create_dir_all(&pics).unwrap();
        std::fs::create_dir_all(docs.join("images")).unwrap();
        std::fs::write(docs.join("images/a.png"), png_bytes()).unwrap();
        std::fs::write(pics.join("рис 1.png"), png_bytes()).unwrap();
        let exports = d.path().join("exports");
        std::fs::create_dir_all(&exports).unwrap();
        let text = "# Pics\n\n![a](images/a.png) ![b](<мои картинки/рис 1.png>) ![r](https://example.org/r.png)\n";
        let doc_path = docs.join("note.md");
        // without styles: links relative to the exported file
        let r = ExportRequest::new(
            ExportKind::HtmlPlain,
            text.into(),
            Some(doc_path.clone()),
            exports.join("note.html"),
            Theme::light(),
        );
        run_export(&r, &JobControl::new()).unwrap();
        let html = std::fs::read_to_string(exports.join("note.html")).unwrap();
        let srcs = src_values(&html);
        assert_eq!(srcs.len(), 3, "{html}");
        assert_eq!(srcs[0], "../docs/images/a.png");
        assert_eq!(srcs[2], "https://example.org/r.png", "remote URLs untouched");
        for s in &srcs[..2] {
            assert!(
                exports.join(decode(s)).exists(),
                "{s} does not resolve from the export folder"
            );
        }
        // styled (self-contained): local raster images embedded, remote kept
        let mut r = r.clone();
        r.kind = ExportKind::Html;
        r.output = exports.join("styled.html");
        let done = run_export(&r, &JobControl::new()).unwrap();
        assert!(done.warnings.is_empty(), "{:?}", done.warnings);
        let html = std::fs::read_to_string(exports.join("styled.html")).unwrap();
        let srcs = src_values(&html);
        assert!(srcs[0].starts_with("data:image/png;base64,iVBOR"), "{}", srcs[0]);
        assert!(srcs[1].starts_with("data:image/png;base64,"));
        assert_eq!(srcs[2], "https://example.org/r.png");
    }

    #[test]
    fn cancelled_before_start() {
        let d = tempfile::tempdir().unwrap();
        let c = JobControl::new();
        c.cancel();
        assert!(run_export(&req(ExportKind::Html, d.path(), "a.html"), &c).is_err());
        assert!(!d.path().join("a.html").exists());
    }

    #[cfg(unix)]
    #[test]
    fn custom_command_substitutes_variables() {
        let d = tempfile::tempdir().unwrap();
        let kind = ExportKind::Custom {
            name: "copy".into(),
            command: "cp ${currentPath} ${outputPath}".into(),
            extension: "md".into(),
        };
        let r = req(kind, d.path(), "out copy.md");
        std::fs::write(d.path().join("doc.md"), &r.text).unwrap();
        let done = run_export(&r, &JobControl::new()).unwrap();
        assert_eq!(done.command_output.as_deref(), Some(""));
        assert!(d.path().join("out copy.md").exists());
        // failing command reports its output
        let mut bad = r.clone();
        bad.kind = ExportKind::Custom {
            name: "x".into(),
            command: "/bin/sh -c 'echo oops >&2; exit 3'".into(),
            extension: "x".into(),
        };
        let e = run_export(&bad, &JobControl::new()).unwrap_err();
        assert!(e.contains("exit code 3") && e.contains("oops"), "{e}");
    }

    // Codex review P1: front matter cannot inject commands.
    #[cfg(unix)]
    #[test]
    fn custom_command_values_are_not_interpreted() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("pwned");
        let title = format!("$(touch {m}) `touch {m}` \"; touch {m}; ' %PATH%", m = marker.display());
        let text = format!("---\ntitle: {}\n---\n# x\n", title.replace('"', "\\\""));
        let mut r = ExportRequest::new(
            ExportKind::Custom {
                name: "echo".into(),
                command: "echo \"${title}\" '${title}' ${title}".into(),
                extension: "txt".into(),
            },
            text,
            Some(d.path().join("doc.md")),
            d.path().join("out.txt"),
            Theme::light(),
        );
        r.after_command = Some("echo ${title}".into());
        let done = run_export(&r, &JobControl::new()).unwrap();
        assert!(!marker.exists(), "a substituted value was executed");
        let out = done.command_output.unwrap();
        assert!(out.contains("$(touch"), "{out}");
    }

    #[test]
    fn tile_paths_avoid_existing_files() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("r.png");
        assert_eq!(tile_paths(&out, 1).unwrap(), vec![out.clone()]);
        let p = tile_paths(&out, 3).unwrap();
        assert_eq!(p[1], d.path().join("r-2.png"));
        std::fs::write(d.path().join("r-3.png"), b"unrelated").unwrap();
        let p = tile_paths(&out, 3).unwrap();
        assert_eq!(p[0], out);
        assert_eq!(p[1], d.path().join("r.1-2.png"));
        assert_eq!(p[2], d.path().join("r.1-3.png"));
    }

    #[test]
    fn tile_names_exhausted_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("r.png");
        std::fs::write(d.path().join("r-2.png"), b"x").unwrap();
        for k in 1..=999 {
            std::fs::write(d.path().join(format!("r.{k}-2.png")), b"x").unwrap();
        }
        assert!(tile_paths(&out, 2).is_err());
    }

    #[test]
    fn nothing_is_published_after_cancel() {
        let d = tempfile::tempdir().unwrap();
        let c = JobControl::new();
        c.cancel();
        assert!(publish(&d.path().join("x"), b"x", &c).is_err());
        assert!(!d.path().join("x").exists());
    }
}
