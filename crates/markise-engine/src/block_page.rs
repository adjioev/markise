//! A block as its site's code expects it (ADR 0023): generic markup from its fields, the package's
//! files by their conventional names, and nothing else on disk. Everything here runs on the web
//! client's thread. Nothing of a document reaches a page except through this.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use markise_doc::FormatDoc;
use markise_doc::resource::{has_scheme, is_absolute_like};
use markise_export::{HtmlOptions, escape_html, render_body};
use markise_md::{Document, ParseOptions};
use markise_package::InstalledPackage;
use markise_web_render::proto::{self, Area};
use markise_web_render::{JobSource, Prepared};
use schema_form_core::{Control, FormSpec, Selection};

use crate::block_fields::{BlockFields, FieldKind, FieldValue, ItemData};
use crate::block_view::PackageViews;

/// The kinds of file a block's own images can be.
const IMAGE_TYPES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"];
/// How many images one note names.
const NOTE_NAMES: usize = 3;
/// How much of one path a note shows.
const NOTE_PATH_CHARS: usize = 200;

/// One block, ready to be drawn.
pub struct BlockPage {
    pub views: Arc<PackageViews>,
    pub package: InstalledPackage,
    /// The document's folder: `None` for a document with no file.
    pub folder: Option<PathBuf>,
    pub name: String,
    pub args: String,
    /// From its opener's first colon to its closer's end.
    pub source: String,
    /// The images its markup holds, by decoded relative path: all a page gets of the document.
    images: Mutex<HashSet<String>>,
}

impl BlockPage {
    pub fn new(
        views: Arc<PackageViews>,
        package: InstalledPackage,
        folder: Option<PathBuf>,
        name: &str,
        args: &str,
        source: &str,
    ) -> BlockPage {
        BlockPage {
            views,
            package,
            folder,
            name: name.to_string(),
            args: args.to_string(),
            source: source.to_string(),
            images: Mutex::default(),
        }
    }

    fn package_file(&self, name: &str) -> Option<String> {
        self.package.asset_path(name).ok()?.is_file().then(|| name.to_string())
    }
}

impl JobSource for BlockPage {
    fn prepare(&self) -> Result<Prepared, String> {
        let (fields, spec) = self
            .views
            .block_model(&self.name, &self.args, &self.source)
            .ok_or("The block cannot be read.")?;
        let schema = self
            .package
            .index
            .block(&self.name)
            .ok_or("The package has no such block.")?;
        let variant = schema.schema.variants().any(|v| v == self.args);
        let argument = schema.schema.raw.get("argument").is_some();
        let html = markup(&fields, &spec, variant, argument);
        let images = images_in(&html);
        // a page's address would lose the dots and could name another file: such a path never gets there
        // (these are decoded already)
        let refused: Vec<String> = sorted(&images).iter().filter_map(|p| unreachable_path(p)).collect();
        if !refused.is_empty() {
            return Err(joined(refused));
        }
        *self.images.lock().unwrap_or_else(|e| e.into_inner()) = images;
        let shared = self.package_file("styles/styles.css");
        let own = self.package_file(&format!("components/{0}/{0}.css", self.name));
        Ok(Prepared {
            html,
            styles: shared.into_iter().chain(own).collect(),
            module: self.package_file(&format!("components/{0}/{0}.js", self.name)),
        })
    }

    fn file(&self, area: Area, path: &str) -> Option<Vec<u8>> {
        match area {
            Area::Package => {
                let full = self.package.asset_path(path).ok()?;
                let relative = full.strip_prefix(&self.package.root).ok()?;
                read_within(&self.package.root, relative)
            }
            Area::Document => {
                let path = clean_dots(path);
                let listed = self.images.lock().unwrap_or_else(|e| e.into_inner()).contains(&path);
                if !listed || !is_image(&path) {
                    return None;
                }
                read_within(self.folder.as_ref()?, Path::new(&path))
            }
        }
    }

    fn images_note(&self) -> Option<String> {
        let images = self.images.lock().unwrap_or_else(|e| e.into_inner());
        let notes: Vec<String> = sorted(&images)
            .iter()
            .filter(|p| !has_scheme(p))
            .filter_map(|p| self.why_not_read(p))
            .collect();
        (!notes.is_empty()).then(|| joined(notes))
    }
}

impl BlockPage {
    /// Why this image of the block's own list cannot be read, in plain words, with its path.
    fn why_not_read(&self, path: &str) -> Option<String> {
        let shown = clipped(path);
        if !is_image(path) {
            return Some(format!("Not a picture file: {shown}"));
        }
        let Some(folder) = &self.folder else {
            return Some(format!("The document is not saved in a folder yet: {shown}"));
        };
        let dir = Dir::open_ambient_dir(folder, ambient_authority()).ok()?;
        let file = match dir.open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(format!("File not found: {shown}")),
            // the system saying no has a number; the handle refusing a link that leads out has none
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && e.raw_os_error().is_some() => {
                return Some(format!("Cannot read: {shown}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Some(outside(&shown)),
            Err(_) => return Some(format!("This file cannot be opened: {shown}")),
        };
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() {
            return Some(format!("Not a file: {shown}"));
        }
        (metadata.len() > proto::MAX_FILE as u64).then(|| format!("File too large: {shown}"))
    }
}

fn outside(shown: &str) -> String {
    format!("Outside the document's folder: {shown}")
}

fn clipped(path: &str) -> String {
    if path.chars().count() <= NOTE_PATH_CHARS {
        return path.to_string();
    }
    let mut out: String = path.chars().take(NOTE_PATH_CHARS).collect();
    out.push('…');
    out
}

fn sorted(images: &HashSet<String>) -> Vec<&String> {
    let mut all: Vec<&String> = images.iter().collect();
    all.sort();
    all
}

/// Some reasons as one note, of a length to read.
fn joined(notes: Vec<String>) -> String {
    let more = notes.len().saturating_sub(NOTE_NAMES);
    let mut note = notes.into_iter().take(NOTE_NAMES).collect::<Vec<_>>().join("; ");
    if more > 0 {
        note.push_str(&format!("; and {more} more"));
    }
    note
}

/// Why a picture of a block can never have the image at this path, whatever is on disk: a page gets
/// nothing but files inside the document's folder (ADR 0023). `None` for one that may be there. The
/// editor's own display of an image goes by its own rule and shows all of these it can read. Cheap,
/// and touches no disk.
pub fn unreachable_image(src: &str) -> Option<String> {
    if has_scheme(src) {
        return unreachable_path(src);
    }
    let path = src.split(['?', '#']).next().unwrap_or("");
    unreachable_path(&percent_decoded(path))
}

/// [`unreachable_image`] of a path decoded already, as a page's address comes to one.
fn unreachable_path(path: &str) -> Option<String> {
    let shown = clipped(path);
    if has_scheme(path) {
        let own = ["data:", "blob:"]
            .iter()
            .any(|s| path.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s)));
        return (!own).then(|| format!("Not a file in the document's folder: {shown}"));
    }
    if is_absolute_like(path) {
        return Some(outside(&shown));
    }
    let mut depth = 0i32;
    let mut dots = false;
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                dots = true;
                depth -= 1;
                if depth < 0 {
                    return Some(outside(&shown));
                }
            }
            _ => depth += 1,
        }
    }
    dots.then(|| format!("Not a plain path in the document's folder: {shown}"))
}

/// Through a handle on the folder: whatever a path or a link says, nothing outside it opens, also if
/// something changes between a check and the read.
fn read_within(folder: &Path, relative: &Path) -> Option<Vec<u8>> {
    let dir = Dir::open_ambient_dir(folder, ambient_authority()).ok()?;
    let mut file = dir.open(relative).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(proto::MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= proto::MAX_FILE).then_some(bytes)
}

fn is_image(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    IMAGE_TYPES.contains(&ext.as_str())
}

fn attr(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    escape_html(value, out);
    out.push('"');
}

/// A piece of Markdown as HTML, through the export and its sanitizer.
fn blocks(text: &str) -> String {
    let doc = Document::parse(text, ParseOptions::default());
    render_body(&doc, &HtmlOptions::default())
}

/// The same when it is one line of text: without the paragraph around it.
fn inline(text: &str) -> String {
    let html = blocks(text);
    let trimmed = html.trim();
    if let Some(rest) = trimmed.strip_prefix("<p")
        && let Some(inner) = rest.strip_suffix("</p>")
        && let Some((_, inner)) = inner.split_once('>')
        && !inner.contains("<p")
    {
        return inner.to_string();
    }
    html
}

/// The generic markup of ADR 0023, by field type alone.
pub fn markup(fields: &BlockFields, spec: &FormSpec, variant: bool, argument: bool) -> String {
    let mut out = String::from("<section");
    attr(&mut out, "data-component", fields.name());
    attr(&mut out, "class", fields.name());
    if !fields.args().is_empty() {
        if variant {
            attr(&mut out, "data-variant", fields.args());
        }
        if argument {
            attr(&mut out, "data-argument", fields.args());
        }
    }
    out.push('>');
    for field in fields.fields() {
        match (&field.kind, &field.value) {
            // it is `data-argument` above
            (FieldKind::Argument, _) => {}
            (_, FieldValue::Text(text)) if text.trim().is_empty() => {}
            (_, FieldValue::Items(items)) if items.is_empty() => {}
            (FieldKind::Heading { level }, FieldValue::Text(text)) => {
                let level = level.unwrap_or(2).clamp(1, 6);
                out.push_str(&format!("<h{level}"));
                attr(&mut out, "data-field", &field.name);
                out.push('>');
                out.push_str(&inline(text));
                out.push_str(&format!("</h{level}>"));
            }
            (FieldKind::Quote, FieldValue::Text(text)) => {
                out.push_str("<blockquote");
                attr(&mut out, "data-field", &field.name);
                out.push('>');
                out.push_str(&blocks(text));
                out.push_str("</blockquote>");
            }
            (_, FieldValue::Text(text)) => {
                out.push_str("<div");
                attr(&mut out, "data-field", &field.name);
                out.push('>');
                out.push_str(&blocks(text));
                out.push_str("</div>");
            }
            (FieldKind::Image, FieldValue::Items(items)) => {
                out.push_str("<div");
                attr(&mut out, "data-field", &field.name);
                out.push('>');
                for item in items {
                    if let ItemData::Image { alt, path } = &item.data {
                        out.push_str("<img");
                        attr(&mut out, "src", path);
                        attr(&mut out, "alt", alt);
                        out.push('>');
                    }
                }
                out.push_str("</div>");
            }
            (_, FieldValue::Items(items)) => {
                out.push_str("<ul");
                attr(&mut out, "data-field", &field.name);
                if field.kind == FieldKind::Choices {
                    attr(&mut out, "data-select", selection_of(spec, &field.name));
                }
                out.push('>');
                for item in items {
                    out.push_str("<li data-item");
                    let text = match &item.data {
                        ItemData::Text(text) => text,
                        ItemData::Choice { anchor, text, correct } => {
                            if let Some(id) = anchor {
                                attr(&mut out, "data-option-id", id);
                            }
                            attr(&mut out, "data-correct", if *correct { "true" } else { "false" });
                            text
                        }
                        ItemData::Image { alt, .. } => alt,
                    };
                    out.push('>');
                    out.push_str(&inline(text));
                    out.push_str("</li>");
                }
                out.push_str("</ul>");
            }
        }
    }
    out.push_str("</section>");
    out
}

fn selection_of(spec: &FormSpec, key: &str) -> &'static str {
    let single = spec.fields.iter().any(|f| {
        f.key == key
            && matches!(
                f.control,
                Control::Choices {
                    selection: Selection::Single,
                    ..
                }
            )
    });
    if single { "single" } else { "multiple" }
}

/// `./a/./b` as `a/b`, as a URL resolves it: a page never asks for the dots.
fn clean_dots(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| *p != ".").collect();
    parts.join("/")
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `src` of every `<img` the markup holds. It is our own markup, through a sanitizer: attributes
/// are quoted.
fn images_in(html: &str) -> HashSet<String> {
    let mut found = HashSet::new();
    let mut rest = html;
    while let Some(at) = rest.find("<img") {
        rest = &rest[at + 4..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        for quote in ['"', '\''] {
            if let Some(from) = tag.find(&format!("src={quote}")) {
                let value = &tag[from + 5..];
                let value = value.split(quote).next().unwrap_or("");
                let value = value
                    .replace("&amp;", "&")
                    .replace("&#39;", "'")
                    .replace("&quot;", "\"");
                // as a page's URL comes to a file: no query or fragment, then decoded
                let path = value.split(['?', '#']).next().unwrap_or("");
                found.insert(clean_dots(&percent_decoded(path)));
                break;
            }
        }
    }
    found
}

/// A document's language, from its front matter: what its blocks' pages say they are in, and what
/// its package's text is shown in. Empty when it names none, or names something that is no tag: this
/// goes into a page as it stands. The document keeps the answer until an edit reaches its first block.
pub fn document_language(doc: &dyn FormatDoc) -> String {
    let value = doc.language().unwrap_or_default();
    if is_language_tag(&value) { value } else { String::new() }
}

/// Whether `value` can be a language tag; anything else counts as no language.
pub fn is_language_tag(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The language to show a package's text in for this document: English when it names none. The
/// app's own words are English always.
pub fn package_language(doc: &dyn FormatDoc) -> String {
    let language = document_language(doc);
    if language.is_empty() {
        "en".to_string()
    } else {
        language
    }
}
