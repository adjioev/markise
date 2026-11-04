//! The form of a block in a side panel (ADR 0020, issue #178): a draft of one block's values next
//! to the document, with Save and Cancel. The rules live below: `markise_engine::block_fields`
//! reads and writes the Markdown, `egui_schema_form` draws the fields, `markise_package` says what
//! a block has. This is what is left: which block, what becomes of the draft when the document
//! changes under it, and the files and pictures a form asks its host for. The app glue is
//! `app/block_form.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{ColorImage, Context, Key, KeyboardShortcut, Modifiers, RichText, TextureHandle, Ui, UiBuilder, vec2};
use egui_schema_form::{FileRequest, FormHost, SchemaForm, Thumbnail};
use markise_doc::{FormatDoc, NamedContainer};
use markise_engine::block_fields::{BlockFields, FieldsError, decls_from_spec};
use markise_layout::block::{ImageState, Images};
use markise_md::ParseOptions;
use markise_package::{InstalledPackage, PackageIndex};
use schema_form_core::FormSpec;
use serde_json::Value;

use crate::images::TextureStore;

/// Texts of a package have no other language to go by yet: the app's words are English.
pub const LANGUAGE: &str = "en";
/// What undo calls a saved form.
const UNDO_LABEL: &str = "Edit Block";
const SAVE_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
/// How long the shown part of something outside the fields gets.
const MAX_EXTRA_CHARS: usize = 80;
const PANEL_MIN: f32 = 280.0;
const PANEL_DEFAULT: f32 = 420.0;

/// What the person did in the panel this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    Save,
    Cancel,
    /// Throw the draft away and read the block again.
    Reload,
}

/// How the draft stands to the document.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Track {
    /// The block is as it was read.
    Live,
    /// The block changed in the document while the draft held changes.
    Stale,
    /// The block is no longer there, and the draft held changes.
    Gone,
}

struct Ready {
    model: BlockFields,
    form: SchemaForm,
    /// The values as read: a draft differing from them has something to lose.
    baseline: Value,
    track: Track,
    /// What does not fit the schema and is not by a field.
    block_reasons: Vec<String>,
}

enum Content {
    /// The package says something a form cannot honour.
    Refused(String),
    Form(Box<Ready>),
}

/// The open panel.
pub struct OpenForm {
    /// The document it belongs to.
    doc: u64,
    /// The block's opener in the document, kept current.
    key: usize,
    name: String,
    args: String,
    title: String,
    content: Content,
    /// The text revision the panel last looked at (a reload moves it too).
    changes: u64,
    /// What a refused Save or a refused file has to say.
    message: Option<String>,
    picking: Vec<FileRequest>,
    icons: IconCache,
    /// The panel's rectangle in the last frame: the keyboard is in it when the focused widget is.
    rect: Option<egui::Rect>,
    /// The keyboard was in the panel at the end of the last frame. Esc takes the focus away
    /// before anything is drawn, so at that point the panel can only remember it.
    had_focus: bool,
}

/// The block's name, arguments and range for a form at `at`: the innermost one the package knows.
pub fn block_at(doc: &dyn FormatDoc, at: usize, index: &PackageIndex) -> Option<NamedContainer> {
    doc.named_containers_at(at)
        .into_iter()
        .rev()
        .find(|c| index.block(&c.name).is_some())
}

fn clip(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut out: String = line.chars().take(MAX_EXTRA_CHARS).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

impl OpenForm {
    /// Reads the block `c` of the document. `None`: the package does not know it.
    pub fn open(
        doc_id: u64,
        c: &NamedContainer,
        text: &str,
        index: &PackageIndex,
        package: InstalledPackage,
        changes: u64,
    ) -> Option<OpenForm> {
        let entry = index.block(&c.name)?;
        let schema = entry.schema;
        let args = c.args.to_string();
        let label = schema.label.as_ref().map_or(c.name.as_ref(), |l| l.get(LANGUAGE));
        let variant = Some(args.as_str()).filter(|a| !a.is_empty() && schema.variants().any(|v| v == *a));
        let variant_label = schema.variant_label(variant.unwrap_or(""), LANGUAGE);
        let mut title = label.to_string();
        if let Some(v) = variant_label {
            title.push_str(" · ");
            title.push_str(v);
        }
        let mut block_reasons = Vec::new();
        // arguments of a block with variants name one; anywhere else they are not ours to judge
        if variant.is_none() && !args.is_empty() && schema.variants().next().is_some() {
            block_reasons.push(format!("The block has no variant `{args}`."));
        }
        let range = block_range_at_container(c);
        let read = || -> Result<(BlockFields, FormSpec), String> {
            let input = index
                .form_input(&c.name, LANGUAGE)
                .ok_or("the package has no schema for it")?
                .map_err(|e| e.to_string())?;
            let spec = FormSpec::parse(&input.schema, &input.hints, variant).map_err(|e| e.to_string())?;
            let decls = decls_from_spec(&spec).map_err(|f| format!("field `{f}` has no place in the block"))?;
            let source = text.get(range.clone()).ok_or("the block is not where it was")?;
            let model = BlockFields::decode(source, range.start, &decls, &ParseOptions::default())
                .map_err(|e| e.to_string())?;
            Ok((model, spec))
        };
        let content = match read() {
            Ok((model, spec)) => {
                let baseline = model.values();
                add_extras(&model, &mut block_reasons);
                let mut form = SchemaForm::new(spec, baseline.clone(), ("block-form", doc_id, range.start));
                form.request_focus();
                Content::Form(Box::new(Ready {
                    model,
                    form,
                    baseline,
                    track: Track::Live,
                    block_reasons,
                }))
            }
            Err(why) => Content::Refused(why),
        };
        Some(OpenForm {
            doc: doc_id,
            key: range.start,
            name: c.name.to_string(),
            args,
            title,
            content,
            changes,
            message: None,
            picking: Vec::new(),
            icons: IconCache::new(package),
            rect: None,
            had_focus: false,
        })
    }

    pub fn doc(&self) -> u64 {
        self.doc
    }

    pub fn key(&self) -> usize {
        self.key
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn args(&self) -> &str {
        &self.args
    }

    /// The draft holds something the document does not.
    pub fn is_dirty(&self) -> bool {
        match &self.content {
            Content::Form(r) => r.form.values() != &r.baseline,
            Content::Refused(_) => false,
        }
    }

    /// The draft as it stands, for tests and a host of its own.
    pub fn values(&self) -> Option<&Value> {
        match &self.content {
            Content::Form(r) => Some(r.form.values()),
            Content::Refused(_) => None,
        }
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// The keyboard is in the panel.
    pub fn is_focused(&self, ctx: &Context) -> bool {
        let Some(rect) = self.rect else { return false };
        ctx.memory(|m| m.focused())
            .and_then(|id| ctx.read_response(id))
            .is_some_and(|r| rect.intersects(r.rect))
    }

    /// Follows the document. Returns false when the panel has nothing left to show. Costs a
    /// comparison unless the document changed; then the text is searched for the block only if
    /// it is not where it was.
    pub fn follow(&mut self, doc: &dyn FormatDoc, text: &str, changes: u64) -> bool {
        if changes == self.changes {
            return true;
        }
        self.changes = changes;
        let dirty = self.is_dirty();
        let Content::Form(r) = &mut self.content else {
            let here = doc.named_containers_at(self.key);
            return here.iter().any(|c| c.opener.start == self.key && *c.name == *self.name);
        };
        let source = r.model.source().to_string();
        let base = r.model.base();
        if text.get(base..base + source.len()) == Some(source.as_str()) {
            return true;
        }
        // moved, unchanged: the nearest place that holds it
        let nearest = (text.match_indices(source.as_str()).map(|(i, _)| i)).min_by_key(|i| i.abs_diff(base));
        if let Some(at) = nearest
            && let Ok(m) = r.model.refreshed(&source, at)
        {
            r.model = m;
            self.key = at;
            return true;
        }
        // changed where it stands: the same opener at the same place
        let here = doc.named_containers_at(self.key);
        let same = here
            .iter()
            .find(|c| c.opener.start == self.key && *c.name == *self.name);
        match same {
            None if dirty => {
                r.track = Track::Gone;
                true
            }
            None => false,
            Some(_) if dirty => {
                r.track = Track::Stale;
                true
            }
            Some(c) => {
                let range = block_range_at_container(c);
                let same_args = *c.args == *self.args;
                match text.get(range.clone()).map(|s| r.model.refreshed(s, range.start)) {
                    Some(Ok(m)) if same_args => {
                        let values = m.values();
                        r.model = m;
                        r.block_reasons.retain(|x| !x.starts_with(IN_NO_FIELD));
                        add_extras(&r.model, &mut r.block_reasons);
                        r.form.set_values(values.clone());
                        r.baseline = values;
                        r.track = Track::Live;
                        true
                    }
                    // another variant has another form: the caller opens it anew
                    Some(Ok(_)) => {
                        r.track = Track::Stale;
                        self.message = None;
                        true
                    }
                    _ => false,
                }
            }
        }
    }

    /// The arguments in the document differ from what this form was built for, with no draft to
    /// lose: it has to be opened anew.
    pub fn needs_reopen(&self) -> bool {
        matches!(&self.content, Content::Form(r) if r.track == Track::Stale && !self.is_dirty())
    }

    /// Writes the draft, as one undo step. Refuses a block that changed.
    pub fn save(&mut self, session: &mut markise_engine::editor::EditorSession) -> Result<(), String> {
        let Content::Form(r) = &mut self.content else {
            return Err("This block has no form.".to_string());
        };
        if r.form.is_composing() {
            return Err("Finish the text being composed first.".to_string());
        }
        match r.track {
            Track::Live => {}
            Track::Stale => return Err(STALE.to_string()),
            Track::Gone => return Err(GONE.to_string()),
        }
        let edits = r.model.edits_from_values(r.form.values()).map_err(|e| e.to_string())?;
        if edits.is_empty() {
            return Ok(());
        }
        match r.model.apply_to_session(session, &edits, UNDO_LABEL) {
            Ok(_) => Ok(()),
            Err(FieldsError::Stale) => {
                r.track = Track::Stale;
                Err(STALE.to_string())
            }
            Err(e) => Err(format!("The block was not changed: {e}")),
        }
    }

    pub fn take_picking(&mut self) -> Vec<FileRequest> {
        std::mem::take(&mut self.picking)
    }

    pub fn deliver_file(&mut self, request: FileRequest, path: Option<String>) {
        if let Content::Form(r) = &mut self.content {
            r.form.deliver_file(request, path);
        }
    }

    /// Takes Save and Cancel from the input before the fields see them. Only while the keyboard
    /// is in the panel, and not in the middle of a composition.
    fn keys(&self, ctx: &Context) -> Option<PanelAction> {
        if !self.had_focus && !self.is_focused(ctx) {
            return None;
        }
        if matches!(&self.content, Content::Form(r) if r.form.is_composing()) {
            return None;
        }
        ctx.input_mut(|i| {
            if i.consume_shortcut(&SAVE_KEY) {
                Some(PanelAction::Save)
            } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                Some(PanelAction::Cancel)
            } else {
                None
            }
        })
    }

    /// Draws the panel into `ui`, a right-hand panel of the window.
    pub fn ui(&mut self, ui: &mut Ui, host: &mut PanelHost<'_>) -> Option<PanelAction> {
        let ctx = ui.ctx().clone();
        let mut action = self.keys(&ctx);
        let spacing = ui.spacing().item_spacing.y;
        let full = ui.available_rect_before_wrap();
        self.rect = Some(full);
        self.icons.poll(&ctx);
        let footer_h = ui.spacing().interact_size.y + spacing * 2.0;
        let body = egui::Rect::from_min_max(full.min, egui::pos2(full.right(), full.bottom() - footer_h));
        let foot = egui::Rect::from_min_max(egui::pos2(full.left(), body.bottom()), full.max);

        let mut body_ui = ui.new_child(UiBuilder::new().id_salt("markise-block-form-body").max_rect(body));
        body_ui.label(RichText::new(&self.title).strong());
        if !self.args.is_empty() {
            body_ui.small(&self.args);
        }
        body_ui.add_space(spacing);
        if let Some(m) = &self.message {
            warn(&mut body_ui, m);
        }
        let mut can_save = true;
        match &mut self.content {
            Content::Refused(why) => {
                can_save = false;
                warn(&mut body_ui, &format!("This block has no form: {why}"));
            }
            Content::Form(r) => {
                match r.track {
                    Track::Live => {}
                    Track::Stale => {
                        warn(&mut body_ui, STALE);
                        if body_ui.button("Read the Block Again").clicked() {
                            action = Some(PanelAction::Reload);
                        }
                    }
                    Track::Gone => {
                        can_save = false;
                        warn(&mut body_ui, GONE);
                    }
                }
                if !r.form.errors().is_empty() || !r.block_reasons.is_empty() {
                    body_ui.label(
                        RichText::new("This block does not fit its schema. You can still save it.")
                            .color(body_ui.visuals().warn_fg_color),
                    );
                    for reason in &r.block_reasons {
                        body_ui.small(reason);
                    }
                }
                body_ui.separator();
                let mut host = FrameHost {
                    host,
                    icons: &mut self.icons,
                    picking: &mut self.picking,
                };
                egui::ScrollArea::vertical()
                    .id_salt("markise-block-form-scroll")
                    .auto_shrink([false, false])
                    .show(&mut body_ui, |ui| {
                        // room for the scroll bar, which would cover the buttons at the edge
                        ui.set_max_width(ui.available_width() - ui.spacing().scroll.bar_width);
                        r.form.ui_with(ui, &mut host);
                    });
            }
        }
        let mut foot_ui = ui.new_child(UiBuilder::new().id_salt("markise-block-form-foot").max_rect(foot));
        foot_ui.add_space(spacing);
        foot_ui.horizontal(|ui| {
            if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() {
                action = Some(PanelAction::Save);
            }
            if ui.button("Cancel").clicked() {
                action = Some(PanelAction::Cancel);
            }
        });
        ui.advance_cursor_after_rect(full);
        self.had_focus = self.is_focused(&ctx);
        action
    }
}

const IN_NO_FIELD: &str = "In no field, kept as it is";
const STALE: &str = "The block changed in the document, so the draft was not saved. Read the block again \
                     to lose the draft, or Cancel.";
const GONE: &str = "The block is no longer in the document, so there is nothing to save to.";

fn warn(ui: &mut Ui, text: &str) {
    let color = ui.visuals().warn_fg_color;
    let r = ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.clear_value();
        n.set_label(text);
        n.set_role(egui::accesskit::Role::Alert);
        n.set_live(egui::accesskit::Live::Polite);
    });
}

fn add_extras(model: &BlockFields, reasons: &mut Vec<String>) {
    for extra in model.extras() {
        let piece = model.source().get(extra).unwrap_or("");
        reasons.push(format!("{IN_NO_FIELD}: {}", clip(piece)));
    }
}

fn block_range_at_container(c: &NamedContainer) -> std::ops::Range<usize> {
    c.opener.start..c.closer.end
}

/// The width a panel of this window may have.
pub fn panel_widths(window: f32) -> (f32, f32, f32) {
    let max = (window * 0.6).max(PANEL_MIN);
    (PANEL_MIN, PANEL_DEFAULT.min(max), max)
}

/// What the app gives a form to draw with, for one frame.
pub struct PanelHost<'a> {
    pub ctx: &'a Context,
    pub images: &'a mut TextureStore,
    /// The folder of the document, which image paths are relative to.
    pub folder: Option<&'a Path>,
    pub root_url: Option<&'a str>,
}

struct FrameHost<'a, 'b> {
    host: &'a mut PanelHost<'b>,
    icons: &'a mut IconCache,
    picking: &'a mut Vec<FileRequest>,
}

impl PanelHost<'_> {
    fn picture(&mut self, src: &str) -> Thumbnail {
        self.images.request(self.ctx, src);
        match self.images.state(src) {
            ImageState::Pending => Thumbnail::Loading,
            ImageState::Failed(why) => Thumbnail::Failed(why),
            ImageState::Ready { size } => match self.images.texture(src) {
                Some(t) => Thumbnail::Ready {
                    texture: t.id(),
                    size: vec2(size[0] as f32, size[1] as f32),
                },
                // evicted just now; it comes back with the next request
                None => Thumbnail::Loading,
            },
        }
    }
}

impl FormHost for FrameHost<'_, '_> {
    fn can_pick_files(&self) -> bool {
        self.host.folder.is_some()
    }

    fn request_file(&mut self, request: FileRequest, _field_label: &str) {
        self.picking.push(request);
    }

    fn thumbnail(&mut self, path: &str) -> Thumbnail {
        let Some(folder) = self.host.folder else {
            return Thumbnail::None;
        };
        match markise_doc::resource::local_file(folder, self.host.root_url, path) {
            Some(l) => self.host.picture(&l.path.to_string_lossy()),
            None => Thumbnail::None,
        }
    }

    fn icon(&mut self, name: &str) -> Thumbnail {
        if name.ends_with(".svg") {
            return self.icons.svg(self.host.ctx, name);
        }
        match self.icons.package.asset_path(name) {
            Ok(path) => self.host.picture(&path.to_string_lossy()),
            Err(why) => Thumbnail::Failed(why),
        }
    }
}

/// Where a chosen file goes: relative to the document's folder, the way documents refer to
/// their pictures. A file elsewhere has no such path.
pub fn relative_to_folder(file: &Path, folder: &Path) -> Option<String> {
    let file = std::fs::canonicalize(file).ok()?;
    let folder = std::fs::canonicalize(folder).ok()?;
    let rel = file.strip_prefix(folder).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

/// Biggest icon file read. Icons are small; this one is data from a package.
const MAX_ICON_BYTES: u64 = 256 << 10;
/// The side of an icon's bitmap; zoomed up it stays sharp enough for a button.
const ICON_PIXELS: u32 = 128;

enum IconEntry {
    Loading,
    Mask(TextureHandle),
    Colored(TextureHandle),
    Failed(String),
}

/// An icon's name and its bitmap, with whether that is a mask.
type IconResult = (String, Result<(ColorImage, bool), String>);

/// A package's SVG icons, drawn on a worker thread so that no frame waits for them.
struct IconCache {
    package: InstalledPackage,
    entries: HashMap<String, IconEntry>,
    tx: Sender<IconResult>,
    rx: Receiver<IconResult>,
}

impl IconCache {
    fn new(package: InstalledPackage) -> IconCache {
        let (tx, rx) = channel();
        IconCache {
            package,
            entries: HashMap::new(),
            tx,
            rx,
        }
    }

    fn poll(&mut self, ctx: &Context) {
        while let Ok((name, result)) = self.rx.try_recv() {
            let entry = match result {
                Ok((image, mask)) => {
                    let t = ctx.load_texture(format!("block-icon-{name}"), image, egui::TextureOptions::LINEAR);
                    if mask {
                        IconEntry::Mask(t)
                    } else {
                        IconEntry::Colored(t)
                    }
                }
                Err(why) => IconEntry::Failed(why),
            };
            self.entries.insert(name, entry);
        }
    }

    fn svg(&mut self, ctx: &Context, name: &str) -> Thumbnail {
        let size = vec2(ICON_PIXELS as f32, ICON_PIXELS as f32);
        match self.entries.get(name) {
            Some(IconEntry::Loading) => Thumbnail::Loading,
            Some(IconEntry::Mask(t)) => Thumbnail::Mask { texture: t.id(), size },
            Some(IconEntry::Colored(t)) => Thumbnail::Ready { texture: t.id(), size },
            Some(IconEntry::Failed(why)) => Thumbnail::Failed(why.clone()),
            None => {
                self.entries.insert(name.to_string(), IconEntry::Loading);
                match self.package.asset_path(name) {
                    Ok(path) => self.start(ctx, name, path),
                    Err(why) => {
                        self.entries.insert(name.to_string(), IconEntry::Failed(why));
                    }
                }
                Thumbnail::Loading
            }
        }
    }

    fn start(&self, ctx: &Context, name: &str, path: PathBuf) {
        let (tx, ctx, name) = (self.tx.clone(), ctx.clone(), name.to_string());
        let spawned = std::thread::Builder::new()
            .name("markise-block-icon".into())
            .spawn(move || {
                let _ = tx.send((name, rasterize_svg(&path)));
                ctx.request_repaint();
            });
        if spawned.is_err() {
            eprintln!("markise: no thread for a block icon");
        }
    }
}

/// An SVG file to pixels. `currentColor` becomes white and the result a mask that takes the colour
/// of the text around it; an icon with colours of its own stays as it is. Nothing outside the file
/// is read: linked images are refused.
fn rasterize_svg(path: &Path) -> Result<(ColorImage, bool), String> {
    use resvg::{tiny_skia, usvg};
    let bytes = markise_doc::resource::read_regular(path, MAX_ICON_BYTES).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "the icon is not text".to_string())?;
    let mask = text.contains("currentColor");
    let text = text.replace("currentColor", "#ffffff");
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&text, &options).map_err(|e| e.to_string())?;
    let mut pixmap = tiny_skia::Pixmap::new(ICON_PIXELS, ICON_PIXELS).ok_or("no room for the icon")?;
    let size = tree.size();
    let scale = ICON_PIXELS as f32 / size.width().max(size.height()).max(1.0);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let side = ICON_PIXELS as usize;
    Ok((ColorImage::from_rgba_premultiplied([side, side], pixmap.data()), mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.svg");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn current_color_makes_a_mask_and_colours_stay() {
        let head = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#;
        let (_d, path) = svg(&format!(
            r#"{head}<rect width="10" height="10" fill="currentColor"/></svg>"#
        ));
        let (image, mask) = rasterize_svg(&path).unwrap();
        assert!(mask);
        assert_eq!(image.pixels[0], egui::Color32::WHITE);

        let (_d, path) = svg(&format!(
            r##"{head}<rect width="10" height="10" fill="#ff0000"/></svg>"##
        ));
        let (image, mask) = rasterize_svg(&path).unwrap();
        assert!(!mask);
        assert_eq!(image.pixels[0], egui::Color32::RED);
    }

    #[test]
    fn what_is_not_an_icon_is_a_reason() {
        let (_d, path) = svg("not svg");
        assert!(rasterize_svg(&path).is_err());
        let (_d, path) = svg(&"x".repeat(MAX_ICON_BYTES as usize + 1));
        assert!(rasterize_svg(&path).is_err());
        assert!(rasterize_svg(Path::new("/nonexistent/icon.svg")).is_err());
    }

    #[test]
    fn an_icon_reads_nothing_outside_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.svg");
        std::fs::write(
            &other,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect width="1" height="1" fill="#ff0000"/></svg>"##,
        )
        .unwrap();
        let path = dir.path().join("icon.svg");
        let body = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10"><image width="10" height="10" xlink:href="{}"/></svg>"#,
            other.display()
        );
        std::fs::write(&path, body).unwrap();
        let (image, _) = rasterize_svg(&path).unwrap();
        assert!(image.pixels.iter().all(|p| *p == egui::Color32::TRANSPARENT));
    }

    #[test]
    fn a_chosen_file_is_relative_to_the_folder_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("doc/images");
        std::fs::create_dir_all(&sub).unwrap();
        let inside = sub.join("a b.png");
        std::fs::write(&inside, "x").unwrap();
        let folder = dir.path().join("doc");
        assert_eq!(relative_to_folder(&inside, &folder).as_deref(), Some("images/a b.png"));
        let outside = dir.path().join("c.png");
        std::fs::write(&outside, "x").unwrap();
        assert_eq!(relative_to_folder(&outside, &folder), None);
    }
}
