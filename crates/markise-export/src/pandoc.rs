//! Pandoc bridge (spec 04 §8.2): the export model ([`ExDoc`]) is serialized to Pandoc's JSON
//! AST and piped into `pandoc -f json -t <format>`, so the exported structure matches the
//! editor (we never hand our Markdown dialect to Pandoc's Markdown reader).
//!
//! Mapping of Markise extensions: highlight → `Span` with class `mark`, `<kbd>` → `Span` class
//! `kbd`, underline → `Underline`, alerts → `Div` with the alert kind as class plus a `title`
//! `Div` (the shape of Pandoc's own GFM alert reader), task items → `☐`/`☒` prefixes (Pandoc's
//! task-list representation), display math → `Math DisplayMath`, diagrams → `CodeBlock` with the
//! diagram language class, `[toc]` → omitted from the body and `--toc` passed instead, front
//! matter → document metadata (`title`, `author`, other keys as strings), footnotes → `Note`.
//!
//! Requires Pandoc ≥ 2.10 (pandoc-types API 1.21: `Underline`, the new `Table` shape). The API
//! version is taken from the installed Pandoc itself (`pandoc -t json` of an empty document).

use std::path::{Path, PathBuf};
use std::time::Duration;

use markise_md::{AlertKind, Align};
use serde_json::{Value, json};

use crate::job::JobControl;
use crate::model::{ExBlock, ExDoc, ExInline, ExItem};

/// Output formats offered through Pandoc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PandocFormat {
    Docx,
    Odt,
    Rtf,
    Epub,
    Latex,
    MediaWiki,
    Rst,
    Textile,
    Opml,
}

impl PandocFormat {
    pub const ALL: [PandocFormat; 9] = [
        PandocFormat::Docx,
        PandocFormat::Odt,
        PandocFormat::Rtf,
        PandocFormat::Epub,
        PandocFormat::Latex,
        PandocFormat::MediaWiki,
        PandocFormat::Rst,
        PandocFormat::Textile,
        PandocFormat::Opml,
    ];

    /// Pandoc writer name (`-t`).
    pub fn writer(self) -> &'static str {
        match self {
            PandocFormat::Docx => "docx",
            PandocFormat::Odt => "odt",
            PandocFormat::Rtf => "rtf",
            PandocFormat::Epub => "epub",
            PandocFormat::Latex => "latex",
            PandocFormat::MediaWiki => "mediawiki",
            PandocFormat::Rst => "rst",
            PandocFormat::Textile => "textile",
            PandocFormat::Opml => "opml",
        }
    }

    /// Default file extension (without dot).
    pub fn extension(self) -> &'static str {
        match self {
            PandocFormat::Docx => "docx",
            PandocFormat::Odt => "odt",
            PandocFormat::Rtf => "rtf",
            PandocFormat::Epub => "epub",
            PandocFormat::Latex => "tex",
            PandocFormat::MediaWiki => "wiki",
            PandocFormat::Rst => "rst",
            PandocFormat::Textile => "textile",
            PandocFormat::Opml => "opml",
        }
    }

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            PandocFormat::Docx => "Word (.docx)",
            PandocFormat::Odt => "OpenDocument (.odt)",
            PandocFormat::Rtf => "RTF",
            PandocFormat::Epub => "EPUB",
            PandocFormat::Latex => "LaTeX",
            PandocFormat::MediaWiki => "MediaWiki",
            PandocFormat::Rst => "reStructuredText",
            PandocFormat::Textile => "Textile",
            PandocFormat::Opml => "OPML",
        }
    }

    /// Parses a writer name or extension.
    pub fn from_name(s: &str) -> Option<PandocFormat> {
        let s = s.to_ascii_lowercase();
        PandocFormat::ALL
            .into_iter()
            .find(|f| f.writer() == s || f.extension() == s)
    }
}

/// Pandoc-types API version, e.g. `[1, 23, 1]`.
pub type ApiVersion = Vec<u64>;

/// The API version used when the installed Pandoc cannot be asked (tests, previews).
pub fn default_api_version() -> ApiVersion {
    vec![1, 23, 1]
}

// ---------------------------------------------------------------------------------------------
// AST generation

fn attr(id: &str, classes: &[&str]) -> Value {
    json!([id, classes, []])
}

fn node(t: &str, c: Value) -> Value {
    json!({"t": t, "c": c})
}

fn tag(t: &str) -> Value {
    json!({ "t": t })
}

/// Text → `Str`/`Space` sequence (whitespace runs become one `Space`).
fn push_text(s: &str, out: &mut Vec<Value>) {
    let mut word = String::new();
    let mut pending_space = false;
    for ch in s.chars() {
        if ch == ' ' || ch == '\t' || ch == '\n' {
            if !word.is_empty() {
                out.push(node("Str", json!(word)));
                word.clear();
            }
            pending_space = true;
        } else {
            if pending_space {
                out.push(tag("Space"));
                pending_space = false;
            }
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(node("Str", json!(word)));
    }
    if pending_space {
        out.push(tag("Space"));
    }
}

/// Footnotes nest at most this deep (a note inside a note…); deeper or cyclic references
/// become plain superscript numbers.
const MAX_NOTE_DEPTH: usize = 4;

struct AstWriter<'a> {
    doc: &'a ExDoc,
    /// Footnotes being expanded (cycle detection).
    active: std::cell::RefCell<Vec<usize>>,
    /// Document folder for resolving local images (none: sources stay as written).
    doc_dir: Option<&'a Path>,
}

impl AstWriter<'_> {
    fn inlines(&self, nodes: &[ExInline]) -> Vec<Value> {
        let mut out = Vec::new();
        for n in nodes {
            self.inline(n, &mut out);
        }
        out
    }

    fn inline(&self, n: &ExInline, out: &mut Vec<Value>) {
        match n {
            ExInline::Text(t) => push_text(t, out),
            ExInline::Code(c) => out.push(node("Code", json!([attr("", &[]), c]))),
            ExInline::Emph(c) => out.push(node("Emph", json!(self.inlines(c)))),
            ExInline::Strong(c) => out.push(node("Strong", json!(self.inlines(c)))),
            ExInline::Strike(c) => out.push(node("Strikeout", json!(self.inlines(c)))),
            ExInline::Underline(c) => out.push(node("Underline", json!(self.inlines(c)))),
            ExInline::Sub(c) => out.push(node("Subscript", json!(self.inlines(c)))),
            ExInline::Sup(c) => out.push(node("Superscript", json!(self.inlines(c)))),
            ExInline::Highlight(c) => out.push(node("Span", json!([attr("", &["mark"]), self.inlines(c)]))),
            ExInline::Kbd(c) => out.push(node("Span", json!([attr("", &["kbd"]), self.inlines(c)]))),
            ExInline::Link { url, title, children } => out.push(node(
                "Link",
                json!([
                    attr("", &[]),
                    self.inlines(children),
                    [url, title.clone().unwrap_or_default()]
                ]),
            )),
            ExInline::Image { src, alt, title } => {
                let mut alt_nodes = Vec::new();
                push_text(alt, &mut alt_nodes);
                // local images resolve like the viewer (document folder, resource root) and are
                // handed to Pandoc as paths; remote URLs stay as written
                let resolved = self.doc_dir.and_then(|d| {
                    markise_doc::resource::local_file(d, self.doc.resource_root(), src)
                        .map(|l| l.path.to_string_lossy().into_owned())
                });
                let src = resolved.as_deref().unwrap_or(src);
                out.push(node(
                    "Image",
                    json!([attr("", &[]), alt_nodes, [src, title.clone().unwrap_or_default()]]),
                ));
            }
            ExInline::Math(t) => out.push(node("Math", json!([tag("InlineMath"), t]))),
            ExInline::FootnoteRef(num) => {
                let cyclic = {
                    let a = self.active.borrow();
                    a.contains(num) || a.len() >= MAX_NOTE_DEPTH
                };
                if cyclic {
                    out.push(node("Superscript", json!([node("Str", json!(num.to_string()))])));
                    return;
                }
                self.active.borrow_mut().push(*num);
                let blocks = self
                    .doc
                    .footnotes
                    .iter()
                    .find(|(k, _)| k == num)
                    .map(|(_, b)| self.blocks(b))
                    .unwrap_or_default();
                self.active.borrow_mut().pop();
                out.push(node("Note", json!(blocks)));
            }
            ExInline::SoftBreak => out.push(tag("SoftBreak")),
            ExInline::HardBreak => out.push(tag("LineBreak")),
            ExInline::RawHtml(h) => out.push(node("RawInline", json!(["html", h]))),
        }
    }

    fn blocks(&self, blocks: &[ExBlock]) -> Vec<Value> {
        let mut out = Vec::new();
        for b in blocks {
            self.block(b, &mut out, false);
        }
        out
    }

    fn block(&self, b: &ExBlock, out: &mut Vec<Value>, tight: bool) {
        match b {
            ExBlock::Heading { level, id, content } => {
                out.push(node("Header", json!([level, attr(id, &[]), self.inlines(content)])))
            }
            ExBlock::Paragraph(c) => out.push(node(if tight { "Plain" } else { "Para" }, json!(self.inlines(c)))),
            ExBlock::Code { lang, text, .. } => {
                let classes: Vec<&str> = if lang.is_empty() { vec![] } else { vec![lang.as_str()] };
                let body = text.strip_suffix('\n').unwrap_or(text);
                out.push(node("CodeBlock", json!([attr("", &classes), body])));
            }
            ExBlock::Math(t) => out.push(node("Para", json!([node("Math", json!([tag("DisplayMath"), t]))]))),
            ExBlock::Diagram { lang, source } => out.push(node("CodeBlock", json!([attr("", &[lang]), source]))),
            ExBlock::RawHtml(h) => out.push(node("RawBlock", json!(["html", h]))),
            ExBlock::Quote(bs) => out.push(node("BlockQuote", json!(self.blocks(bs)))),
            ExBlock::Alert { kind, blocks } => {
                let k = kind.as_str();
                let title = alert_title(*kind);
                let mut inner = vec![node(
                    "Div",
                    json!([attr("", &["title"]), [node("Para", json!([node("Str", json!(title))]))]]),
                )];
                inner.extend(self.blocks(blocks));
                out.push(node("Div", json!([attr("", &[k]), inner])));
            }
            ExBlock::List {
                ordered,
                start,
                tight,
                items,
            } => {
                let items: Vec<Value> = items.iter().map(|it| json!(self.item(it, *tight))).collect();
                if *ordered {
                    out.push(node(
                        "OrderedList",
                        json!([[start, tag("Decimal"), tag("Period")], items]),
                    ));
                } else {
                    out.push(node("BulletList", json!(items)));
                }
            }
            ExBlock::Table { aligns, header, rows } => out.push(self.table(aligns, header, rows)),
            ExBlock::Rule => out.push(tag("HorizontalRule")),
            ExBlock::Toc => {} // replaced by `--toc`
        }
    }

    fn item(&self, it: &ExItem, tight: bool) -> Vec<Value> {
        let mut out = Vec::new();
        for (i, b) in it.blocks.iter().enumerate() {
            if i == 0
                && let Some(checked) = it.task
                && let ExBlock::Paragraph(c) = b
            {
                let mut inl = vec![node("Str", json!(if checked { "☒" } else { "☐" })), tag("Space")];
                inl.extend(self.inlines(c));
                out.push(node(if tight { "Plain" } else { "Para" }, json!(inl)));
                continue;
            }
            self.block(b, &mut out, tight);
        }
        if out.is_empty() && it.task.is_some() {
            let mark = if it.task == Some(true) { "☒" } else { "☐" };
            out.push(node("Plain", json!([node("Str", json!(mark))])));
        }
        out
    }

    fn table(&self, aligns: &[Align], header: &[Vec<ExInline>], rows: &[Vec<Vec<ExInline>>]) -> Value {
        let align = |a: &Align| {
            tag(match a {
                Align::Left => "AlignLeft",
                Align::Center => "AlignCenter",
                Align::Right => "AlignRight",
                Align::None => "AlignDefault",
            })
        };
        let colspecs: Vec<Value> = aligns
            .iter()
            .map(|a| json!([align(a), tag("ColWidthDefault")]))
            .collect();
        let row = |cells: &[Vec<ExInline>]| {
            let cells: Vec<Value> = aligns
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let content = cells.get(i).map(|c| self.inlines(c)).unwrap_or_default();
                    let blocks = if content.is_empty() {
                        json!([])
                    } else {
                        json!([node("Plain", json!(content))])
                    };
                    json!([attr("", &[]), tag("AlignDefault"), 1, 1, blocks])
                })
                .collect();
            json!([attr("", &[]), cells])
        };
        let head = json!([attr("", &[]), [row(header)]]);
        let body_rows: Vec<Value> = rows.iter().map(|r| row(r)).collect();
        let body = json!([attr("", &[]), 0, [], body_rows]);
        node(
            "Table",
            json!([attr("", &[]), [null, []], colspecs, head, [body], [attr("", &[]), []]]),
        )
    }
}

fn alert_title(k: AlertKind) -> &'static str {
    match k {
        AlertKind::Note => "Note",
        AlertKind::Tip => "Tip",
        AlertKind::Important => "Important",
        AlertKind::Warning => "Warning",
        AlertKind::Caution => "Caution",
    }
}

fn meta_inlines(s: &str) -> Value {
    let mut v = Vec::new();
    push_text(s, &mut v);
    node("MetaInlines", json!(v))
}

/// The Pandoc JSON AST of `doc` for pandoc-types API `api`.
pub fn to_pandoc_json(doc: &ExDoc, api: &[u64]) -> Value {
    to_pandoc_json_in(doc, api, None)
}

/// [`to_pandoc_json`] with local image sources resolved against `doc_dir` and the document's
/// resource root (the shared resolver of the viewer and the other exports).
pub fn to_pandoc_json_in(doc: &ExDoc, api: &[u64], doc_dir: Option<&Path>) -> Value {
    let w = AstWriter {
        doc,
        active: Default::default(),
        doc_dir,
    };
    let mut meta = serde_json::Map::new();
    for (k, v) in &doc.meta {
        if v.trim().is_empty() || v == "unset" {
            continue;
        }
        let key = k.to_ascii_lowercase();
        let value = if key == "author" && v.starts_with('[') && v.ends_with(']') {
            let list: Vec<Value> = v[1..v.len() - 1]
                .split(',')
                .map(|a| meta_inlines(a.trim().trim_matches(['"', '\''])))
                .collect();
            node("MetaList", json!(list))
        } else if matches!(
            key.as_str(),
            "title" | "author" | "subtitle" | "date" | "subject" | "description"
        ) {
            meta_inlines(v)
        } else {
            node("MetaString", json!(v))
        };
        meta.insert(key, value);
    }
    if !meta.contains_key("title")
        && let Some(h) = doc.headings.first()
    {
        // Pandoc warns (and some writers need a title) when it is missing.
        meta.insert("pagetitle".into(), node("MetaString", json!(h.text)));
    }
    json!({
        "pandoc-api-version": api,
        "meta": Value::Object(meta),
        "blocks": w.blocks(&doc.blocks),
    })
}

// ---------------------------------------------------------------------------------------------
// Running Pandoc

/// Errors of a Pandoc export.
#[derive(Debug, Clone, PartialEq)]
pub enum PandocError {
    /// No `pandoc` executable found.
    NotFound,
    /// The installed Pandoc is older than 2.10.
    TooOld(String),
    /// Starting or talking to the process failed.
    Io(String),
    /// Pandoc exited with an error.
    Failed {
        code: Option<i32>,
        stderr: String,
    },
    Timeout(Duration),
    Cancelled,
}

impl std::fmt::Display for PandocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PandocError::NotFound => f.write_str(
                "Pandoc was not found. Install Pandoc 2.10 or newer (https://pandoc.org/installing.html) \
                 or set its path in the export settings.",
            ),
            PandocError::TooOld(v) => write!(
                f,
                "Pandoc {v} is too old: Markise needs Pandoc 2.10 or newer (https://pandoc.org/installing.html)."
            ),
            PandocError::Io(e) => write!(f, "cannot run Pandoc: {e}"),
            PandocError::Failed { code, stderr } => {
                let msg = stderr.trim();
                match code {
                    Some(c) => write!(f, "Pandoc failed (exit code {c}): {msg}"),
                    None => write!(f, "Pandoc was terminated: {msg}"),
                }
            }
            PandocError::Timeout(d) => write!(f, "Pandoc did not finish within {} s", d.as_secs()),
            PandocError::Cancelled => f.write_str("export cancelled"),
        }
    }
}

impl std::error::Error for PandocError {}

/// Locates `pandoc`: an explicit path, then `PATH`, then common install folders (GUI apps on
/// macOS do not inherit the login shell's `PATH`).
pub fn find_pandoc(custom: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = custom {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exe = if cfg!(windows) { "pandoc.exe" } else { "pandoc" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(windows) {
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(l).join("Pandoc"));
        }
        dirs.push(PathBuf::from(r"C:\Program Files\Pandoc"));
    } else {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/opt/local/bin"] {
            dirs.push(PathBuf::from(d));
        }
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&h).join(".local/bin"));
            dirs.push(PathBuf::from(&h).join(".cabal/bin"));
        }
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// Facts about an installed Pandoc.
#[derive(Clone, Debug, PartialEq)]
pub struct PandocInfo {
    pub path: PathBuf,
    /// e.g. "3.8.3".
    pub version: String,
    pub api: ApiVersion,
}

/// Parses the first line of `pandoc --version` ("pandoc 3.8.3" / "pandoc.exe 2.19.2").
pub fn parse_version(output: &str) -> Option<Vec<u64>> {
    let first = output.lines().next()?;
    let v = first.split_whitespace().nth(1)?;
    let parts: Vec<u64> = v.split('.').map_while(|p| p.parse().ok()).collect();
    (!parts.is_empty()).then_some(parts)
}

/// Runs `pandoc --version` and asks Pandoc for its AST API version.
pub fn probe(path: &Path, control: &JobControl) -> Result<PandocInfo, PandocError> {
    let out = run(path, &["--version"], None, b"", Duration::from_secs(20), control)?;
    let text = String::from_utf8_lossy(&out);
    let version = parse_version(&text).ok_or_else(|| PandocError::Io(format!("unexpected version output: {text}")))?;
    let vs = version.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
    if version.as_slice() < [2, 10].as_slice() {
        return Err(PandocError::TooOld(vs));
    }
    let json_out = run(
        path,
        &["-f", "markdown", "-t", "json"],
        None,
        b"",
        Duration::from_secs(20),
        control,
    )?;
    let v: Value =
        serde_json::from_slice(&json_out).map_err(|e| PandocError::Io(format!("cannot read Pandoc JSON: {e}")))?;
    let api: ApiVersion = v["pandoc-api-version"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .filter(|a: &Vec<u64>| a.len() >= 2)
        .ok_or_else(|| PandocError::Io("Pandoc JSON has no API version".into()))?;
    if api.as_slice() < [1, 21].as_slice() {
        return Err(PandocError::TooOld(vs));
    }
    if api[0] != 1 {
        // a future major AST revision may change node shapes: refuse instead of guessing
        return Err(PandocError::Io(format!(
            "Pandoc {vs} uses AST API {api:?}, which this version of Markise does not support"
        )));
    }
    Ok(PandocInfo {
        path: path.to_path_buf(),
        version: vs,
        api,
    })
}

/// Runs `exe args…` (no shell) with `input` on stdin; returns stdout. Cancellation and the
/// timeout end the process tree; output capture is bounded (see [`crate::process`]).
pub fn run(
    exe: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    input: &[u8],
    timeout: Duration,
    control: &JobControl,
) -> Result<Vec<u8>, PandocError> {
    run_opts(
        exe,
        args,
        cwd,
        input,
        &crate::process::RunOptions::new(timeout),
        control,
    )
}

fn run_opts(
    exe: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    input: &[u8],
    opts: &crate::process::RunOptions,
    control: &JobControl,
) -> Result<Vec<u8>, PandocError> {
    use crate::process::ProcError;
    crate::process::run_opts(exe, args, cwd, input, opts, control).map_err(|e| match e {
        ProcError::NotFound(_) => PandocError::NotFound,
        ProcError::Io(e) => PandocError::Io(e),
        ProcError::Failed { code, stderr } => PandocError::Failed { code, stderr },
        ProcError::Timeout(d) => PandocError::Timeout(d),
        ProcError::Cancelled => PandocError::Cancelled,
        e @ ProcError::Shutdown(_) => PandocError::Io(e.to_string()),
    })
}

/// Options of [`export`].
#[derive(Clone, Debug)]
pub struct PandocOptions {
    pub format: PandocFormat,
    /// Folder relative image paths are resolved against (the document's folder).
    pub resource_dir: Option<PathBuf>,
    /// Extra user arguments.
    pub extra_args: Vec<String>,
    pub timeout: Duration,
    /// Process-tree shutdown deadline (see [`crate::process::RunOptions`]); its `timeout` is
    /// replaced by [`PandocOptions::timeout`].
    pub run: crate::process::RunOptions,
}

impl PandocOptions {
    pub fn new(format: PandocFormat) -> Self {
        PandocOptions {
            format,
            resource_dir: None,
            extra_args: Vec::new(),
            timeout: Duration::from_secs(120),
            run: crate::process::RunOptions::new(Duration::from_secs(120)),
        }
    }
}

/// Exports `doc` to `output` through Pandoc.
pub fn export(
    info: &PandocInfo,
    doc: &ExDoc,
    output: &Path,
    opts: &PandocOptions,
    control: &JobControl,
) -> Result<(), PandocError> {
    control.check().map_err(|_| PandocError::Cancelled)?;
    let json = to_pandoc_json_in(doc, &info.api, opts.resource_dir.as_deref()).to_string();
    // Pandoc writes a temporary file next to the destination; it replaces the destination
    // only after Pandoc succeeded and the export was not cancelled (crate::output).
    let tmp = crate::output::temp_path_for(output).map_err(PandocError::Io)?;
    let out = tmp.to_string_lossy().into_owned();
    let mut args: Vec<String> = vec![
        "-f".into(),
        "json".into(),
        "-t".into(),
        opts.format.writer().into(),
        "-s".into(),
        "-o".into(),
        out,
    ];
    if doc.blocks.iter().any(|b| matches!(b, ExBlock::Toc)) {
        args.push("--toc".into());
    }
    if let Some(d) = &opts.resource_dir {
        args.push(format!("--resource-path={}", d.to_string_lossy()));
    }
    args.extend(opts.extra_args.iter().cloned());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    control.set_progress(0.3);
    let run_opts_ = crate::process::RunOptions {
        timeout: opts.timeout,
        ..opts.run.clone()
    };
    if let Err(e) = run_opts(
        &info.path,
        &argv,
        opts.resource_dir.as_deref(),
        json.as_bytes(),
        &run_opts_,
        control,
    ) {
        // failed, cancelled, timed out or not shut down: the output is never published; it is
        // removed explicitly and a cleanup failure is reported instead of lost
        return Err(match crate::output::discard(tmp) {
            Ok(()) => e,
            Err(c) => PandocError::Io(format!("{e}; {c}")),
        });
    }
    // only reached when the whole process tree is gone (run_opts succeeded); persist()
    // discards the temporary file itself on cancellation or a rename failure
    crate::output::persist(tmp, output, control, true).map_err(|e| {
        if e == crate::output::CANCELLED {
            PandocError::Cancelled
        } else {
            PandocError::Io(e)
        }
    })?;
    control.set_progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExDoc, ModelOptions};
    use markise_md::parse;

    fn ast(md: &str) -> Value {
        to_pandoc_json(
            &ExDoc::build(&parse(md), &ModelOptions::default()),
            &default_api_version(),
        )
    }

    #[test]
    fn text_splits_into_str_and_space() {
        let mut v = Vec::new();
        push_text("a  b\nc", &mut v);
        assert_eq!(
            v,
            vec![
                node("Str", json!("a")),
                tag("Space"),
                node("Str", json!("b")),
                tag("Space"),
                node("Str", json!("c"))
            ]
        );
    }

    #[test]
    fn extensions_mapping() {
        let a = ast(
            "---\ntitle: T\nfoo: bar\n---\n# H\n\n<mark>m</mark> <u>u</u>\n\n> [!TIP]\n> x\n\n- [x] done\n\n[toc]\n",
        );
        assert_eq!(a["meta"]["title"]["t"], "MetaInlines");
        assert_eq!(a["meta"]["foo"], node("MetaString", json!("bar")));
        let blocks = a["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["t"], "Header");
        assert_eq!(blocks[0]["c"][1][0], "h");
        let para = &blocks[1]["c"];
        assert_eq!(para[0]["t"], "Span");
        assert_eq!(para[0]["c"][0][1][0], "mark");
        assert_eq!(para[2]["t"], "Underline");
        assert_eq!(blocks[2]["t"], "Div");
        assert_eq!(blocks[2]["c"][0][1][0], "tip");
        assert_eq!(blocks[3]["t"], "BulletList");
        assert_eq!(blocks[3]["c"][0][0]["c"][0]["c"], "☒");
        assert_eq!(blocks.len(), 4, "toc omitted from the body");
    }

    #[test]
    fn cyclic_footnotes_terminate() {
        // self reference and mutual references (accepted by the parser)
        for md in [
            "See[^a].\n\n[^a]: again[^a]\n",
            "x[^a]\n\n[^a]: to b[^b]\n\n[^b]: to a[^a]\n",
        ] {
            let v = ast(md);
            let s = v.to_string();
            assert!(s.contains("\"Note\""), "{s}");
            assert!(s.contains("\"Superscript\""), "cycle cut: {s}");
            assert!(s.len() < 10_000);
        }
        // a long chain is bounded by the nesting depth
        let mut md = String::from("x[^n0]\n\n");
        for i in 0..50 {
            md.push_str(&format!("[^n{i}]: note[^n{}]\n\n", i + 1));
        }
        let s = ast(&md).to_string();
        assert_eq!(s.matches("\"Note\"").count(), MAX_NOTE_DEPTH);
    }

    /// A fake Pandoc that writes part of its output, then `then` (Unix shell / cmd syntax).
    fn fake_pandoc(dir: &Path, then: &str) -> PandocInfo {
        let p = if cfg!(windows) {
            let p = dir.join(format!("fake-pandoc-{}.cmd", then.len()));
            std::fs::write(
                &p,
                format!("@echo off\r\n:loop\r\nif \"%~1\"==\"-o\" goto found\r\nif \"%~1\"==\"\" exit /b 2\r\nshift\r\ngoto loop\r\n:found\r\necho partial> \"%~2\"\r\n{then}\r\n"),
            )
            .unwrap();
            p
        } else {
            let p = dir.join(format!("fake-pandoc-{}", then.len()));
            std::fs::write(
                &p,
                format!("#!/bin/sh\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nprintf partial > \"$2\"\n{then}\n"),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            p
        };
        PandocInfo {
            path: p,
            version: "3.0".into(),
            api: default_api_version(),
        }
    }

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".markise-export-"))
            .collect()
    }

    // Codex round 5: a tree that does not empty in time is never published; a rename failure
    // removes the temporary file.
    #[test]
    fn stuck_tree_and_rename_failure_never_publish() {
        let d = tempfile::tempdir().unwrap();
        let doc = ExDoc::build(&parse("# x\n"), &ModelOptions::default());
        let ok = fake_pandoc(d.path(), "exit 0");
        let dest = d.path().join("out.docx");
        std::fs::write(&dest, b"previous export").unwrap();
        let mut opts = PandocOptions::new(PandocFormat::Docx);
        opts.run.shutdown = Duration::from_millis(300);
        opts.run.simulate_stuck_tree = Some(Duration::from_secs(60));
        let e = export(&ok, &doc, &dest, &opts, &JobControl::new()).unwrap_err();
        assert!(e.to_string().contains("could not stop"), "{e}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous export");
        assert!(temp_leftovers(d.path()).is_empty());
        // cancelled while the (successful) tree is shut down
        let mut opts = PandocOptions::new(PandocFormat::Docx);
        opts.run.simulate_stuck_tree = Some(Duration::from_millis(800));
        let c = JobControl::new();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel();
        });
        let e = export(&ok, &doc, &dest, &opts, &c).unwrap_err();
        assert_eq!(e, PandocError::Cancelled);
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous export");
        assert!(temp_leftovers(d.path()).is_empty());
        // the rename fails (the destination is a folder): reported, temporary file removed
        let folder = d.path().join("taken.docx");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inside"), b"x").unwrap();
        let e = export(
            &ok,
            &doc,
            &folder,
            &PandocOptions::new(PandocFormat::Docx),
            &JobControl::new(),
        )
        .unwrap_err();
        assert!(matches!(e, PandocError::Io(_)), "{e:?}");
        assert!(folder.join("inside").exists());
        assert!(temp_leftovers(d.path()).is_empty());
    }

    // Codex round 3: a cancelled or failing Pandoc never touches the destination.
    #[cfg(unix)]
    #[test]
    fn cancelled_or_failed_pandoc_keeps_the_destination() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("out.docx");
        std::fs::write(&dest, b"previous export").unwrap();
        let doc = ExDoc::build(&parse("# x\n"), &ModelOptions::default());
        let c = JobControl::new();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel();
        });
        let e = export(
            &fake_pandoc(d.path(), "sleep 30"),
            &doc,
            &dest,
            &PandocOptions::new(PandocFormat::Docx),
            &c,
        )
        .unwrap_err();
        assert_eq!(e, PandocError::Cancelled);
        let failing = fake_pandoc(d.path(), "exit 3");
        let e = export(
            &failing,
            &doc,
            &dest,
            &PandocOptions::new(PandocFormat::Docx),
            &JobControl::new(),
        )
        .unwrap_err();
        assert!(matches!(e, PandocError::Failed { code: Some(3), .. }), "{e:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous export");
        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".markise-export-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        // success replaces it
        let ok = fake_pandoc(d.path(), "exit 0");
        export(
            &ok,
            &doc,
            &dest,
            &PandocOptions::new(PandocFormat::Docx),
            &JobControl::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"partial");
    }

    // Codex round 4: a descendant holding the output open is gone before cleanup; no temporary
    // file remains right after cancellation (no grace sleep).
    #[cfg(windows)]
    #[test]
    fn windows_cancel_removes_temp_held_by_descendant() {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("fake-pandoc.cmd");
        std::fs::write(
            &script,
            "@echo off\r\n:loop\r\nif \"%~1\"==\"-o\" goto found\r\nif \"%~1\"==\"\" exit /b 2\r\nshift\r\ngoto loop\r\n:found\r\nset \"OUT=%~2\"\r\necho partial> \"%OUT%\"\r\nstart /b cmd /c ping -n 30 127.0.0.1 ^>^> \"%OUT%\"\r\nping -n 30 127.0.0.1 >nul\r\n",
        )
        .unwrap();
        let info = PandocInfo {
            path: script,
            version: "3.0".into(),
            api: default_api_version(),
        };
        let dest = d.path().join("out.docx");
        std::fs::write(&dest, b"previous export").unwrap();
        let doc = ExDoc::build(&parse("# x\n"), &ModelOptions::default());
        let c = JobControl::new();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            c2.cancel();
        });
        let e = export(&info, &doc, &dest, &PandocOptions::new(PandocFormat::Docx), &c).unwrap_err();
        assert_eq!(e, PandocError::Cancelled, "{e}");
        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".markise-export-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous export");
    }

    // Codex round 9 self-review: Pandoc gets images resolved with the resource root too.
    #[test]
    fn pandoc_images_use_the_shared_resolver() {
        let doc = ExDoc::build(
            &parse(
                "---\ntypora-root-url: ./media\n---\n![p](/photo.png) ![r](https://e.org/r.png) ![q](a%20b.png#x)\n",
            ),
            &ModelOptions::default(),
        );
        let v = to_pandoc_json_in(&doc, &default_api_version(), Some(Path::new("/docs")));
        let s = v.to_string();
        let media = Path::new("/docs").join("media").join("photo.png");
        assert!(
            s.contains(&serde_json::to_string(&media.to_string_lossy()).unwrap()),
            "{s}"
        );
        assert!(s.contains("\"https://e.org/r.png\""));
        let ab = Path::new("/docs").join("a b.png");
        assert!(
            s.contains(&serde_json::to_string(&ab.to_string_lossy()).unwrap()),
            "{s}"
        );
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("pandoc 3.8.3\nFeatures: +server"), Some(vec![3, 8, 3]));
        assert_eq!(parse_version("pandoc.exe 2.19.2"), Some(vec![2, 19, 2]));
        assert_eq!(parse_version("garbage"), None);
        assert_eq!(PandocFormat::from_name("tex"), Some(PandocFormat::Latex));
    }

    #[test]
    fn missing_executable_is_not_found() {
        let r = run(
            Path::new("/nonexistent/pandoc-xyz"),
            &[],
            None,
            b"",
            Duration::from_secs(1),
            &JobControl::new(),
        );
        assert_eq!(r, Err(PandocError::NotFound));
        assert!(PandocError::NotFound.to_string().contains("pandoc.org"));
    }
}
