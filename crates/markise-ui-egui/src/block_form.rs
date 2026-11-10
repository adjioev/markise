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
use egui_schema_form::{FileRequest, FormHost, SchemaForm, Style, Thumbnail};
use markise_doc::{Affinity, DocPos};
use markise_doc::{FormatDoc, NamedContainer};
use markise_engine::block_fields::{BlockFields, FieldValue, FieldsError, ItemId};
pub use markise_engine::block_page::package_language;
use markise_engine::block_view::read_form;
use markise_engine::editor::{EditorSession, MarkId};
use markise_layout::block::{ImageState, Images};
use markise_md::ParseOptions;
use markise_package::{BlockEntry, InstalledPackage, PackageIndex, printable};
use schema_form_core::{Control, FieldSpec, FormSpec};
use serde_json::Value;

use crate::images::TextureStore;

/// What undo calls a saved form.
const UNDO_LABEL: &str = "Edit Block";
const SAVE_KEY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
/// How long the shown part of something outside the fields gets.
const MAX_EXTRA_CHARS: usize = 80;
const FOOTER_HEIGHT: f32 = 44.0;
const BUTTON_ROW: f32 = 32.0;
/// Below this much free room in the footer, its note about the keys goes.
const KEYS_NOTE_ROOM: f32 = 180.0;
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
    /// Which item of the model each row of the form was when the form last read the model, by the
    /// form's row keys: a row is its item, whatever it says (ADR 0020).
    rows: RowItems,
}

/// By field: the row keys in the order of the model's items, and the item each is.
type RowItems = HashMap<String, Vec<(u64, ItemId)>>;

/// The form's rows as the model's items, row for item in order, right after the form took its values.
fn row_items(model: &BlockFields, form: &mut SchemaForm) -> RowItems {
    let mut out = HashMap::new();
    for f in model.fields() {
        let FieldValue::Items(items) = &f.value else { continue };
        let Some(keys) = form.row_keys(&f.name).filter(|k| k.len() == items.len()) else {
            continue;
        };
        out.insert(
            f.name.clone(),
            keys.into_iter().zip(items.iter().map(|i| i.id)).collect(),
        );
    }
    out
}

/// A text field holding a code block shows read-only: a value line written anew would be escaped and
/// lose its indent, so the code is edited in the Markdown (issue #231). The engine refuses such an edit
/// in any case; this is so nobody types what cannot be saved.
fn keep_code(model: &BlockFields, form: &mut SchemaForm) {
    for f in model.fields() {
        if let FieldValue::Text(_) = f.value {
            form.set_read_only(&f.name, model.holds_code(&f.name).then(|| CODE_KEPT.to_string()));
        }
    }
}

/// What each row of the form is now: its item, or none for a new row.
fn rows_now(rows: &RowItems, form: &mut SchemaForm) -> HashMap<String, Vec<Option<ItemId>>> {
    (rows.iter())
        .filter_map(|(name, items)| {
            let keys = form.row_keys(name)?;
            let item = |k: &u64| items.iter().find(|(x, _)| x == k).map(|(_, id)| *id);
            Some((name.clone(), keys.iter().map(item).collect()))
        })
        .collect()
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
    /// Live marks at the block's two ends: edits move them, and only edits do. A block with the
    /// same text elsewhere is another block.
    marks: [MarkId; 2],
    name: String,
    args: String,
    /// What the package's text in it is in.
    language: String,
    title: String,
    /// The variant in words, or the arguments as they stand.
    note: String,
    /// A file of the package for the block's icon, if it declares one.
    icon: Option<String>,
    content: Content,
    /// The text revision the panel last looked at (a reload moves it too).
    changes: u64,
    /// What a refused Save or a refused file has to say.
    message: Option<String>,
    picking: Vec<FileRequest>,
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

/// The form's title and note in `language`: the block's label, and its variant in words or the
/// arguments as they stand. A package or the document wrote them: each stays one line, and nothing in
/// it hides or turns the text around.
fn heading(entry: &BlockEntry<'_>, name: &str, args: &str, language: &str) -> (String, String) {
    let schema = entry.schema;
    let title = printable(schema.label.as_ref().map_or(name, |l| l.get(language)));
    let named = !args.is_empty() && schema.variants().any(|v| v == args);
    let variant_label = schema.variant_label(if named { args } else { "" }, language);
    // with a field of their own the arguments need no note
    let args_note = if schema.raw.get("argument").is_some() { "" } else { args };
    (title, printable(variant_label.unwrap_or(args_note)))
}

/// The form as the panel shows it: every word a package wrote for a field (its label, help, the add
/// button's label and the options' labels) stays one line, with nothing in it that hides or turns the text
/// around. Keys and values are the package's data and stay as they are.
fn shown_spec(mut spec: FormSpec) -> FormSpec {
    fn escape(fields: &mut [FieldSpec]) {
        for f in fields {
            f.label = printable(&f.label);
            f.help = f.help.as_deref().map(printable);
            f.add_label = f.add_label.as_deref().map(printable);
            match &mut f.control {
                Control::Array { fields: inner } | Control::Group { fields: inner } => escape(inner),
                Control::ChoiceButtons { options, .. } => {
                    for o in options {
                        o.label = printable(&o.label);
                    }
                }
                _ => {}
            }
        }
    }
    escape(&mut spec.fields);
    spec
}

/// The form's words for what is wrong with a value: some quote the package (a pattern, an option's value)
/// or the document (an id), and those stay one line, with nothing hidden.
fn form_texts() -> egui_schema_form::Texts {
    egui_schema_form::Texts {
        message: std::sync::Arc::new(|kind| printable(&kind.to_string())),
        ..Default::default()
    }
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
        session: &mut EditorSession,
        index: &PackageIndex,
        language: &str,
    ) -> Option<OpenForm> {
        let changes = session.revision().counter;
        let entry = index.block(&c.name)?;
        let schema = entry.schema;
        let args = c.args.to_string();
        let variant = Some(args.as_str()).filter(|a| !a.is_empty() && schema.variants().any(|v| v == *a));
        let (title, note) = heading(&entry, &c.name, &args, language);
        // a name; a package gives it meaning with a file of that name, or leaves it a name
        let icon_name =
            (entry.view.and_then(|v| v.icon.as_deref())).or_else(|| entry.palette.and_then(|p| p.icon.as_deref()));
        let icon = icon_name.map(|n| format!("assets/{n}.svg"));
        let mut block_reasons = Vec::new();
        // arguments of a block with variants name one; anywhere else they are not ours to judge
        if variant.is_none() && !args.is_empty() && schema.variants().next().is_some() {
            block_reasons.push(format!("The block has no variant `{args}`."));
        }
        let range = block_range_at_container(c);
        let read = || -> Result<(BlockFields, FormSpec), String> {
            let (spec, decls) = read_form(index, &c.name, language, variant)?;
            let spec = shown_spec(spec);
            let source = session
                .text()
                .get(range.clone())
                .ok_or("the block is not where it was")?;
            let model = BlockFields::decode(source, range.start, &decls, &ParseOptions::default())
                .map_err(|e| e.to_string())?;
            Ok((model, spec))
        };
        let content = match read() {
            Ok((model, spec)) => {
                let baseline = model.values();
                add_extras(&model, &mut block_reasons);
                let mut form = SchemaForm::new(spec, baseline.clone(), ("block-form", doc_id, range.start))
                    .with_texts(form_texts());
                form.request_focus();
                let rows = row_items(&model, &mut form);
                keep_code(&model, &mut form);
                Content::Form(Box::new(Ready {
                    rows,
                    model,
                    form,
                    baseline,
                    track: Track::Live,
                    block_reasons,
                }))
            }
            Err(why) => Content::Refused(why),
        };
        let marks = [
            session.add_mark(DocPos::new(range.start)),
            session.add_mark(DocPos {
                affinity: Affinity::Upstream,
                ..DocPos::new(range.end)
            }),
        ];
        Some(OpenForm {
            doc: doc_id,
            key: range.start,
            marks,
            name: c.name.to_string(),
            args,
            language: language.to_string(),
            title,
            note,
            icon,
            content,
            changes,
            message: None,
            picking: Vec::new(),
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

    /// The block's label.
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn args(&self) -> &str {
        &self.args
    }

    /// The draft holds something the document does not.
    pub fn is_dirty(&self) -> bool {
        match &self.content {
            // rows that read the same and changed places are a change too: what is nested under them goes along
            Content::Form(r) => {
                r.form.values() != &r.baseline
                    || r.rows.iter().any(|(name, items)| {
                        r.form
                            .row_keys_now(name)
                            .is_some_and(|keys| !keys.iter().eq(items.iter().map(|(k, _)| k)))
                    })
            }
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

    /// A composition is going on in a field: its text is not final, so nothing acts on the draft.
    pub fn is_composing(&self) -> bool {
        matches!(&self.content, Content::Form(r) if r.form.is_composing())
    }

    /// Gives back what the form holds in the session of its document.
    pub fn release(&self, session: &mut EditorSession) {
        for m in self.marks {
            session.remove_mark(m);
        }
    }

    /// The keyboard is in the panel.
    pub fn is_focused(&self, ctx: &Context) -> bool {
        let Some(rect) = self.rect else { return false };
        ctx.memory(|m| m.focused())
            .and_then(|id| ctx.read_response(id))
            .is_some_and(|r| rect.intersects(r.rect))
    }

    /// Follows the document through its edits. Returns false when the panel has nothing left to
    /// show. Costs a comparison unless the text changed.
    pub fn follow(&mut self, session: &EditorSession) -> bool {
        let revision = session.revision().counter;
        if revision == self.changes {
            return true;
        }
        self.changes = revision;
        let (Some(start), Some(end)) = (session.mark(self.marks[0]), session.mark(self.marks[1])) else {
            return false;
        };
        let (start, end) = (start.offset, end.offset);
        self.key = start;
        // still a block from one mark to the other, of the same name
        let here = session.doc().named_containers_at(start);
        let same = (here.iter()).find(|c| c.opener.start == start && c.closer.end == end && *c.name == *self.name);
        let dirty = self.is_dirty();
        let Content::Form(r) = &mut self.content else {
            return same.is_some();
        };
        let Some(c) = same else {
            if dirty {
                r.track = Track::Gone;
            }
            return dirty;
        };
        let text = session.text();
        let source = text.get(start..end).unwrap_or("");
        if source == r.model.source() {
            match r.model.refreshed(source, start) {
                Ok(m) => r.model = m,
                Err(_) => return dirty,
            }
            if r.track == Track::Stale {
                r.track = Track::Live;
            }
            return true;
        }
        if dirty {
            r.track = Track::Stale;
            return true;
        }
        let Ok(m) = r.model.refreshed(source, start) else {
            return false;
        };
        if *c.args == *self.args {
            let values = m.values();
            r.model = m;
            r.block_reasons.retain(|x| !x.starts_with(IN_NO_FIELD));
            add_extras(&r.model, &mut r.block_reasons);
            r.form.set_values(values.clone());
            r.rows = row_items(&r.model, &mut r.form);
            keep_code(&r.model, &mut r.form);
            r.baseline = values;
        } else {
            // another variant has another form: the caller opens it anew
            r.track = Track::Stale;
        }
        true
    }

    /// The document it belongs to is gone. A changed draft stays to be read and copied, and has
    /// nowhere to go; returns false when there is nothing to keep.
    pub fn orphan(&mut self) -> bool {
        let dirty = self.is_dirty();
        if dirty && let Content::Form(r) = &mut self.content {
            r.track = Track::Gone;
        }
        dirty
    }

    /// The document's language changed: the same draft, focus and all, in the package's words for it.
    /// A composition goes on first. Costs a comparison unless the language changed.
    pub fn set_language(&mut self, index: &PackageIndex, language: &str) {
        if self.language == language || self.is_composing() {
            return;
        }
        self.language = language.to_string();
        let Some(entry) = index.block(&self.name) else { return };
        (self.title, self.note) = heading(&entry, &self.name, &self.args, language);
        let Content::Form(r) = &mut self.content else { return };
        let variant = Some(self.args.as_str()).filter(|a| !a.is_empty() && entry.schema.variants().any(|v| v == *a));
        // the same package, so the same fields: only its words differ
        if let Some(Ok(input)) = index.form_input(&self.name, language)
            && let Ok(spec) = FormSpec::parse(&input.schema, &input.hints, variant)
        {
            r.form.set_spec(shown_spec(spec));
        }
    }

    /// The arguments in the document differ from what this form was built for, with no draft to
    /// lose: it has to be opened anew.
    pub fn needs_reopen(&self) -> bool {
        matches!(&self.content, Content::Form(r) if r.track == Track::Stale && !self.is_dirty())
    }

    /// Writes the draft, as one undo step. Refuses a block that changed.
    pub fn save(&mut self, session: &mut EditorSession) -> Result<(), String> {
        // where the block stands right now: a change can land between the last frame and this one
        if !self.follow(session) {
            return Ok(());
        }
        let dirty = self.is_dirty();
        let Content::Form(r) = &mut self.content else {
            return Err("This block has no form.".to_string());
        };
        if r.form.is_composing() {
            return Err("Finish the text being composed first.".to_string());
        }
        if !dirty {
            return Ok(());
        }
        match r.track {
            Track::Live => {}
            Track::Stale => return Err(STALE.to_string()),
            Track::Gone => return Err(GONE.to_string()),
        }
        let rows = rows_now(&r.rows, &mut r.form);
        let edits = (r.model.edits_from_rows(r.form.values(), &rows)).map_err(|e| e.to_string())?;
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
        if self.is_composing() {
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
        let st = host.style;
        let spacing = ui.spacing().item_spacing.y;
        let full = ui.available_rect_before_wrap();
        self.rect = Some(full);
        let footer_h = FOOTER_HEIGHT;
        let body = egui::Rect::from_min_max(full.min, egui::pos2(full.right(), full.bottom() - footer_h));
        let foot = egui::Rect::from_min_max(egui::pos2(full.left(), body.bottom()), full.max);

        let mut body_ui = ui.new_child(UiBuilder::new().id_salt("markise-block-form-body").max_rect(body));
        let picture = (self.icon.as_deref()).map(|name| host.assets.svg(&ctx, name));
        body_ui.horizontal(|ui| {
            match picture {
                Some(Thumbnail::Mask { texture, .. }) => paint_title_icon(ui, texture, st.muted),
                Some(Thumbnail::Ready { texture, .. }) => paint_title_icon(ui, texture, egui::Color32::WHITE),
                _ => {}
            }
            ui.add(egui::Label::new(RichText::new(&self.title).size(15.0).strong().color(st.fg)).wrap());
        });
        if !self.note.is_empty() {
            body_ui.label(RichText::new(&self.note).size(11.5).color(st.muted));
        }
        body_ui.add_space(spacing);
        if let Some(m) = &self.message {
            warn(&mut body_ui, m, st);
        }
        let mut can_save = true;
        match &mut self.content {
            Content::Refused(why) => {
                can_save = false;
                warn(&mut body_ui, &format!("This block has no form: {why}"), st);
            }
            Content::Form(r) => {
                match r.track {
                    Track::Live => {}
                    Track::Stale => {
                        warn(&mut body_ui, STALE, st);
                        if egui_schema_form::button(&mut body_ui, "Read the Block Again", false, true, st).clicked() {
                            action = Some(PanelAction::Reload);
                        }
                    }
                    Track::Gone => {
                        can_save = false;
                        warn(&mut body_ui, GONE, st);
                    }
                }
                if !r.form.errors().is_empty() || !r.block_reasons.is_empty() {
                    body_ui.label(
                        RichText::new("This block does not fit its schema. You can still save it.")
                            .size(12.0)
                            .color(st.danger),
                    );
                    // they quote the document's arguments and text: each stays one line, nothing in it hides
                    // or turns the text around
                    for reason in &r.block_reasons {
                        body_ui.label(RichText::new(printable(reason)).size(11.5).color(st.muted));
                    }
                }
                if r.model.has_nested() {
                    body_ui.label(RichText::new(NESTED_KEPT).size(11.5).color(st.muted));
                }
                body_ui.add_space(spacing);
                r.form.set_style(st);
                let mut host = FrameHost {
                    host,
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
        // the footer: right-aligned, Cancel then Save in the order Tab reaches them
        let mut foot_ui = ui.new_child(UiBuilder::new().id_salt("markise-block-form-foot").max_rect(foot));
        foot_ui.add_space(FOOTER_HEIGHT - BUTTON_ROW);
        foot_ui.horizontal(|ui| {
            let widths = |label: &str| {
                let font = egui::FontId::proportional(13.5);
                ui.painter().layout_no_wrap(label.to_string(), font, st.fg).size().x + 24.0
            };
            let total = widths("Cancel") + widths("Save") + ui.spacing().item_spacing.x;
            let room = ui.available_width() - total;
            if room > KEYS_NOTE_ROOM {
                let keys = if cfg!(target_os = "macos") {
                    "Esc cancels, ⌘↩ saves"
                } else {
                    "Esc cancels, Ctrl+Enter saves"
                };
                let note = ui.add(egui::Label::new(RichText::new(keys).size(11.5).color(st.muted)).selectable(false));
                ui.add_space((room - note.rect.width() - ui.spacing().item_spacing.x).max(0.0));
            } else {
                ui.add_space(room.max(0.0));
            }
            // ids of their own: the note before them comes and goes with the width, and the
            // focus must stay on the button it is on
            let mut footer = |ui: &mut Ui, label: &str, primary: bool, enabled: bool, then: PanelAction| {
                let scope = UiBuilder::new().scope_id(egui::Id::unique(("markise-block-form-button", label)));
                let r = ui.scope_builder(scope, |ui| egui_schema_form::button(ui, label, primary, enabled, st));
                if r.inner.clicked() {
                    action = Some(then);
                }
            };
            footer(ui, "Cancel", false, true, PanelAction::Cancel);
            footer(ui, "Save", true, can_save, PanelAction::Save);
        });
        ui.advance_cursor_after_rect(full);
        self.had_focus = self.is_focused(&ctx);
        action
    }
}

const IN_NO_FIELD: &str = "In no field, kept as it is";
/// Markdown nested under an item is the item's: the form keeps it and the Markdown is where it is edited.
pub(crate) const NESTED_KEPT: &str = "Markdown nested under an item is kept with it. Edit it in the block's \
                                      Markdown or in Source.";
/// A text field with a code block in it is shown, not edited: the code keeps its bytes.
pub(crate) const CODE_KEPT: &str = "This field holds a code block, which is kept as written. Edit it in the \
                                    block's Markdown or in Source.";
const STALE: &str = "The block changed in the document, so the draft was not saved. Read the block again \
                     to lose the draft, or Cancel.";
const GONE: &str =
    "The block is no longer in the document, so there is nothing to save to. You can still copy the text.";

fn paint_title_icon(ui: &mut Ui, texture: egui::TextureId, tint: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(texture, r, uv, tint);
}

/// What a block or its package says went wrong can quote them: it stays on its lines, with nothing in
/// it that hides or turns the text around.
fn warn(ui: &mut Ui, text: &str, st: Style) {
    let text = &printable(text);
    let r = ui.add(egui::Label::new(RichText::new(text).size(12.5).color(st.danger)).wrap());
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

/// The form's colours from the theme: its light or dark base, with the theme's accent.
pub fn style_for(theme: &markise_theme::Theme) -> Style {
    let base = if theme.dark { Style::DARK } else { Style::LIGHT };
    base.with_accent(markise_layout::block::color(theme.chrome.accent))
}

/// The width a panel of this window may have.
pub fn panel_widths(window: f32) -> (f32, f32, f32) {
    let max = (window * 0.6).max(PANEL_MIN);
    (PANEL_MIN, PANEL_DEFAULT.min(max), max)
}

/// What the app gives a form to draw with, for one frame.
pub struct PanelHost<'a> {
    pub style: Style,
    pub ctx: &'a Context,
    pub images: &'a mut TextureStore,
    pub assets: &'a mut FormAssets,
    /// The folder of the document, which image paths are relative to.
    pub folder: Option<&'a Path>,
    pub root_url: Option<&'a str>,
    /// Whether a block can be drawn as a picture here: then a path that picture cannot reach is
    /// worth a word, though the form and the editor show the image.
    pub site_pictures: bool,
}

struct FrameHost<'a, 'b> {
    host: &'a mut PanelHost<'b>,
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
        let (ctx, root) = (self.host.ctx, self.host.root_url);
        match self.host.assets.resolve(ctx, folder, root, path) {
            None => Thumbnail::Loading,
            Some(None) => Thumbnail::None,
            Some(Some(file)) => match self.host.picture(&file) {
                // a path as the author wrote it: where it resolves to is the same to them
                Thumbnail::Failed(why) => Thumbnail::Failed(printable(&format!("{why}: {path}"))),
                other => other,
            },
        }
    }

    fn path_note(&mut self, path: &str) -> Option<String> {
        if !self.host.site_pictures {
            return None;
        }
        let why = markise_engine::block_page::unreachable_image(path)?;
        Some(format!("The block's picture cannot use this file. {why}"))
    }

    fn icon(&mut self, name: &str) -> Thumbnail {
        let icon = if name.ends_with(".svg") {
            self.host.assets.svg(self.host.ctx, name)
        } else {
            match self.host.assets.asset_path(name) {
                Ok(path) => self.host.picture(&path.to_string_lossy()),
                Err(why) => Thumbnail::Failed(why),
            }
        };
        // the package names the icon, and why it failed can quote that name: on hover it stays one line
        match icon {
            Thumbnail::Failed(why) => Thumbnail::Failed(printable(&why)),
            other => other,
        }
    }
}

/// Where a chosen file goes: relative to the document's folder, the way documents refer to
/// their pictures. A file elsewhere has no such path.
pub fn relative_to_folder(file: &Path, folder: &Path) -> Option<String> {
    markise_engine::attach::path_below(file, folder)
}

/// Biggest icon file read. Icons are small; this one is data from a package.
const MAX_ICON_BYTES: u64 = 256 << 10;
/// The side of an icon's bitmap; zoomed up it stays sharp enough for a button.
const ICON_PIXELS: u32 = 128;

/// Paths remembered before starting over: one for each thing typed into a path field.
const MAX_PATHS: usize = 4096;

enum IconEntry {
    Loading,
    Mask(TextureHandle),
    Colored(TextureHandle),
    Failed(String),
}

enum Job {
    Icon {
        name: String,
        file: PathBuf,
    },
    /// What a reference in the document means on this disk: it looks at the disk, so not here.
    Resolve {
        key: PathKey,
        src: String,
    },
}

enum Done {
    Icon(String, Result<(ColorImage, bool), String>),
    Resolved(PathKey, Option<String>),
}

/// The folder and root a reference is read against, and the reference.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PathKey {
    folder: PathBuf,
    root_url: Option<String>,
    src: String,
}

/// What forms need from the disk: a package's SVG icons and where image references lead. It lives
/// as long as the session, with one worker working through one queue in turn, so opening a form
/// again costs nothing and never adds to what is running. An idle worker waits without waking.
#[derive(Default)]
pub struct FormAssets {
    package: Option<InstalledPackage>,
    icons: HashMap<String, IconEntry>,
    /// `None`: asked, no answer yet. Then the file, if the reference is a file at all.
    paths: HashMap<PathKey, Option<Option<String>>>,
    jobs: Option<Sender<Job>>,
    done: Option<Receiver<Done>>,
    /// Jobs not yet taken back.
    pending: usize,
}

impl FormAssets {
    /// The package icons come from from now on. Another one starts over.
    pub fn use_package(&mut self, package: InstalledPackage) {
        if self.package.as_ref().is_none_or(|p| p.sha256 != package.sha256) {
            self.icons.clear();
            // the old queue runs out, and its answers go nowhere
            self.jobs = None;
            self.done = None;
            self.pending = 0;
        }
        self.package = Some(package);
    }

    /// A form opens: what the disk said about paths may be out of date by now.
    pub fn forget_paths(&mut self) {
        self.paths.clear();
    }

    pub fn is_idle(&self) -> bool {
        self.pending == 0
    }

    fn asset_path(&self, name: &str) -> Result<PathBuf, String> {
        match &self.package {
            Some(p) => p.asset_path(name),
            None => Err("no package in use".to_string()),
        }
    }

    fn send(&mut self, ctx: &Context, job: Job) {
        if self.jobs.is_none() {
            let (jobs, queue) = channel::<Job>();
            let (answers, done) = channel();
            let wake = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("markise-block-form".into())
                .spawn(move || {
                    while let Ok(job) = queue.recv() {
                        let answer = run_job(job);
                        let published = crate::wake_track::publishing();
                        if answers.send(answer).is_err() {
                            return;
                        }
                        wake.request_repaint();
                        drop(published);
                    }
                });
            if spawned.is_err() {
                eprintln!("markise: no thread for block form files");
                return;
            }
            self.jobs = Some(jobs);
            self.done = Some(done);
        }
        if self.jobs.as_ref().is_some_and(|j| j.send(job).is_ok()) {
            self.pending += 1;
        }
    }

    /// Takes in what the worker finished.
    pub fn poll(&mut self, ctx: &Context) {
        let Some(rx) = &self.done else { return };
        while let Ok(answer) = rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            match answer {
                Done::Icon(name, Ok((image, mask))) => {
                    let t = ctx.load_texture(format!("block-icon-{name}"), image, egui::TextureOptions::LINEAR);
                    let entry = if mask {
                        IconEntry::Mask(t)
                    } else {
                        IconEntry::Colored(t)
                    };
                    self.icons.insert(name, entry);
                }
                Done::Icon(name, Err(why)) => {
                    self.icons.insert(name, IconEntry::Failed(why));
                }
                Done::Resolved(key, file) => {
                    self.paths.insert(key, Some(file));
                }
            }
        }
    }

    /// Where `src` leads: `None` while the worker looks, then the file, if it is a file at all.
    /// Asked once per reference; every frame after that is a lookup.
    fn resolve(&mut self, ctx: &Context, folder: &Path, root_url: Option<&str>, src: &str) -> Option<Option<String>> {
        let key = PathKey {
            folder: folder.to_path_buf(),
            root_url: root_url.map(str::to_string),
            src: src.to_string(),
        };
        if let Some(known) = self.paths.get(&key) {
            return known.clone();
        }
        if self.paths.len() >= MAX_PATHS {
            self.paths.clear();
        }
        self.paths.insert(key.clone(), None);
        let src = src.to_string();
        self.send(ctx, Job::Resolve { key, src });
        None
    }

    pub(crate) fn svg(&mut self, ctx: &Context, name: &str) -> Thumbnail {
        let size = vec2(ICON_PIXELS as f32, ICON_PIXELS as f32);
        match self.icons.get(name) {
            Some(IconEntry::Loading) => Thumbnail::Loading,
            Some(IconEntry::Mask(t)) => Thumbnail::Mask { texture: t.id(), size },
            Some(IconEntry::Colored(t)) => Thumbnail::Ready { texture: t.id(), size },
            Some(IconEntry::Failed(why)) => Thumbnail::Failed(why.clone()),
            None => {
                match self.asset_path(name) {
                    Ok(file) => {
                        self.icons.insert(name.to_string(), IconEntry::Loading);
                        let name = name.to_string();
                        self.send(ctx, Job::Icon { name, file });
                    }
                    Err(why) => {
                        self.icons.insert(name.to_string(), IconEntry::Failed(why));
                    }
                }
                Thumbnail::Loading
            }
        }
    }
}

fn run_job(job: Job) -> Done {
    match job {
        // whatever this file does to the renderer, the queue goes on and the icon has its reason
        Job::Icon { name, file } => {
            let drawn = std::panic::catch_unwind(|| rasterize_svg(&file));
            Done::Icon(
                name,
                drawn.unwrap_or_else(|_| Err("the icon could not be drawn".to_string())),
            )
        }
        Job::Resolve { key, src } => {
            let file = markise_doc::resource::local_file(&key.folder, key.root_url.as_deref(), &src);
            Done::Resolved(key, file.map(|l| l.path.to_string_lossy().into_owned()))
        }
    }
}

const NO_FILTER: &str = "an icon may not use a filter";
const NO_MASK: &str = "an icon may not use a mask";
const NO_IMAGE: &str = "an icon may not hold an image";
const NO_PATTERN: &str = "an icon may not use a pattern";
const NO_DASH: &str = "an icon may not use a dashed stroke";
const TOO_DEEP: &str = "an icon may not nest so deep";
const TOO_MANY: &str = "an icon may not hold so many shapes";
const TOO_LARGE: &str = "an icon may not draw a layer that large";
/// The widest layer in pixels, before the renderer cuts it to size. Its own sums overflow far beyond
/// this, so it must not get there.
const MAX_ICON_LAYER: f32 = 1_000_000.0;
/// Elements of the XML the scan reads at all; the file's size bounds them, this says it earlier.
const MAX_ICON_ELEMENTS: u32 = 1 << 16;
/// Parts of the parsed picture. Each can cost a layer's worth of blending, so the file's size is
/// not bound enough.
const MAX_ICON_PARTS: usize = 4096;
/// Nested layers are alive together, each up to the renderer's limit of five times the canvas each
/// way: 1.6 MB here.
const MAX_ICON_DEPTH: usize = 16;

/// The source as written: no DTD, so no entity can multiply it, and none of the elements, attributes
/// or CSS whose cost the picture's size would not bound. `use` and `marker` copy their content once
/// per reference, so a small file parses into something of any size; `pattern` renders a whole
/// tile whatever its size; filters are `filter`, `fe*`, the attribute and the CSS property.
fn refuse_in_source(text: &str) -> Result<(), String> {
    let options = resvg::usvg::roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_ICON_ELEMENTS,
    };
    let doc = resvg::usvg::roxmltree::Document::parse_with_options(text, options).map_err(|e| e.to_string())?;
    for node in doc.descendants() {
        if node.is_text() {
            // the text of a `style` element, CDATA included
            if node.parent().is_some_and(|p| p.tag_name().name() == "style")
                && node.text().is_some_and(|s| s.to_ascii_lowercase().contains("filter"))
            {
                return Err(NO_FILTER.to_string());
            }
            continue;
        }
        if !node.is_element() {
            continue;
        }
        let name = node.tag_name().name();
        let style = node
            .attribute("style")
            .is_some_and(|s| s.to_ascii_lowercase().contains("filter"));
        if name == "filter" || name.starts_with("fe") || style || node.attributes().any(|a| a.name() == "filter") {
            return Err(NO_FILTER.to_string());
        }
        if name == "pattern" {
            return Err(NO_PATTERN.to_string());
        }
        if name == "use" || name == "marker" {
            return Err(format!("an icon may not use `{name}`"));
        }
    }
    Ok(())
}

/// The parsed picture, everywhere the renderer would go: groups, clip paths and their chains, the
/// paint of shapes, the shapes of text. The tree's own list of filters does not follow a chain of
/// masks, and the renderer does. So every mask is refused, not just a mask holding a filter.
fn refuse_in_tree(root: &resvg::usvg::Group, to_pixels: resvg::tiny_skia::Transform) -> Result<(), String> {
    let mut parts = 0;
    walk_tree(root, to_pixels, 0, &mut parts)
}

fn walk_tree(
    group: &resvg::usvg::Group,
    outer: resvg::tiny_skia::Transform,
    depth: usize,
    parts: &mut usize,
) -> Result<(), String> {
    use resvg::usvg::{Node, Paint};
    let transform = outer.pre_concat(group.transform());
    // the sum the renderer does for a layer, before it cuts the layer to size
    if group.should_isolate()
        && let Some(b) = group.layer_bounding_box().transform(transform)
        && [b.x(), b.y(), b.width(), b.height()]
            .iter()
            .any(|v| v.abs() > MAX_ICON_LAYER)
    {
        return Err(TOO_LARGE.to_string());
    }
    if depth > MAX_ICON_DEPTH {
        return Err(TOO_DEEP.to_string());
    }
    *parts += 1;
    if *parts > MAX_ICON_PARTS {
        return Err(TOO_MANY.to_string());
    }
    if !group.filters().is_empty() {
        return Err(NO_FILTER.to_string());
    }
    if group.mask().is_some() {
        return Err(NO_MASK.to_string());
    }
    let mut clip = group.clip_path();
    let mut chain = depth;
    while let Some(c) = clip {
        chain += 1;
        walk_tree(c.root(), transform.pre_concat(c.transform()), chain, parts)?;
        clip = c.clip_path();
    }
    for node in group.children() {
        match node {
            Node::Group(g) => walk_tree(g, transform, depth + 1, parts)?,
            Node::Text(t) => walk_tree(t.flattened(), transform, depth + 1, parts)?,
            Node::Image(_) => return Err(NO_IMAGE.to_string()),
            Node::Path(p) => {
                *parts += 1;
                if *parts > MAX_ICON_PARTS {
                    return Err(TOO_MANY.to_string());
                }
                // the dashes of a stroke are as many as its length allows, up to a million a path
                if p.stroke().is_some_and(|s| s.dasharray().is_some()) {
                    return Err(NO_DASH.to_string());
                }
                let fill = p.fill().map(|f| f.paint());
                let stroke = p.stroke().map(|s| s.paint());
                if [fill, stroke]
                    .into_iter()
                    .flatten()
                    .any(|x| matches!(x, Paint::Pattern(_)))
                {
                    return Err(NO_PATTERN.to_string());
                }
            }
        }
    }
    Ok(())
}

/// An SVG file to pixels. `currentColor` becomes white and the result a mask that takes the colour
/// of the text around it; an icon with colours of its own stays as it is. Nothing outside the file
/// is read: images are not loaded, this build of the parser has no text and so no fonts, and it
/// follows no reference outside the file. Whatever costs more than the 128 by 128 output would
/// bound is refused by name ([`refuse_in_source`], [`refuse_in_tree`]); an icon has no need of it.
fn rasterize_svg(path: &Path) -> Result<(ColorImage, bool), String> {
    use resvg::{tiny_skia, usvg};
    let bytes = markise_doc::resource::read_regular(path, MAX_ICON_BYTES).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "the icon is not text".to_string())?;
    refuse_in_source(&text)?;
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
    let size = tree.size();
    let scale = ICON_PIXELS as f32 / size.width().max(size.height()).max(1.0);
    let to_pixels = tiny_skia::Transform::from_scale(scale, scale);
    refuse_in_tree(tree.root(), to_pixels)?;
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
    fn the_example_package_s_icons_are_masks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/block-package/package/assets");
        for name in ["basket", "bulb", "clock"] {
            let path = root.join(format!("{name}.svg"));
            let (_, mask) = rasterize_svg(&path).unwrap();
            assert!(mask, "{name} follows the theme");
        }
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
    fn a_filter_is_refused_before_it_can_run() {
        let (_d, path) = svg(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><filter id="f"><feTurbulence numOctaves="1000000000" baseFrequency="1"/></filter><rect width="10" height="10" filter="url(#f)"/></svg>"##,
        );
        assert!(rasterize_svg(&path).unwrap_err().contains("filter"));
    }

    #[test]
    fn a_dtd_is_refused_so_no_entity_can_hide_anything() {
        let (_d, path) = svg(
            r#"<!DOCTYPE svg [<!ENTITY s "filter: blur(5px)">]><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="9" height="9" style="&s;"/></svg>"#,
        );
        assert!(rasterize_svg(&path).is_err());
    }

    fn refused_with(body: &str) -> String {
        let head = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#;
        let (_d, path) = svg(&format!("{head}{body}</svg>"));
        rasterize_svg(&path).expect_err(body)
    }

    const TURBULENCE: &str = r#"<filter id="f"><feTurbulence numOctaves="1000000000" baseFrequency="1"/></filter>"#;

    #[test]
    fn a_filter_at_the_end_of_a_mask_chain_is_refused() {
        let why = refused_with(&format!(
            r#"{TURBULENCE}<mask id="c"><rect width="10" height="10" fill="white" filter="url(#f)"/></mask><mask id="b" mask="url(#c)"><rect width="10" height="10" fill="white"/></mask><mask id="a" mask="url(#b)"><rect width="10" height="10" fill="white"/></mask><rect width="10" height="10" mask="url(#a)"/>"#
        ));
        assert!(why.contains("filter"), "{why}");
    }

    #[test]
    fn what_an_icon_has_no_need_of_is_refused_by_name() {
        let rect = r#"<rect width="10" height="10"/>"#;
        for (body, word) in [
            (
                format!(r#"<mask id="m">{rect}</mask><rect width="9" height="9" mask="url(#m)"/>"#),
                "mask",
            ),
            (TURBULENCE.to_string(), "filter"),
            (r#"<feTurbulence/>"#.to_string(), "filter"),
            (
                r#"<style>rect { filter: blur(5px) }</style><rect width="9" height="9"/>"#.to_string(),
                "filter",
            ),
            (
                r#"<rect width="9" height="9" style="filter: blur(5px)"/>"#.to_string(),
                "filter",
            ),
            (
                r#"<rect width="9" height="9" filter="blur(5px)"/>"#.to_string(),
                "filter",
            ),
        ] {
            let why = refused_with(&body);
            assert!(why.contains(word), "{body}: {why}");
        }
    }

    #[test]
    fn a_plain_icon_with_a_clip_still_draws() {
        let head = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#;
        let body = r##"<clipPath id="c"><rect width="10" height="10"/></clipPath><g clip-path="url(#c)"><rect width="10" height="10" fill="#ff0000"/></g>"##;
        let (_d, path) = svg(&format!("{head}{body}</svg>"));
        let (image, _) = rasterize_svg(&path).unwrap();
        assert!(image.pixels.iter().any(|p| *p != egui::Color32::TRANSPARENT));
    }

    /// A tile this size would ask the renderer for tens of gigabytes: refused without drawing.
    const HUGE_TILE: &str = r##"<pattern id="p" width="100000" height="100000" patternUnits="userSpaceOnUse"><rect width="5" height="5" fill="#ff0000"/></pattern>"##;

    #[test]
    fn a_pattern_is_refused_in_the_source_and_in_the_tree() {
        let drawn = |paint: &str| format!(r#"{HUGE_TILE}<rect width="9" height="9" {paint}/>"#);
        for paint in [r#"fill="url(#p)""#, r##"stroke="url(#p)" fill="none""##] {
            assert!(refused_with(&drawn(paint)).contains("pattern"), "{paint}");
            // the tree too, should anything ever get past the scan
            let text = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">{}</svg>"#,
                drawn(paint)
            );
            let tree = resvg::usvg::Tree::from_str(&text, &Default::default()).unwrap();
            let to_pixels = resvg::tiny_skia::Transform::identity();
            assert!(
                refuse_in_tree(tree.root(), to_pixels).unwrap_err().contains("pattern"),
                "{paint}"
            );
        }
    }

    #[test]
    fn a_small_file_cannot_grow_when_parsed() {
        let rect = r#"<rect id="r" width="9" height="9"/>"#;
        assert!(refused_with(&format!(r##"{rect}<use href="#r"/>"##)).contains("`use`"));
        assert!(refused_with(r#"<marker id="m"/>"#).contains("`marker`"));
        let bomb = r#"<!DOCTYPE svg [<!ENTITY a "<g/><g/>"><!ENTITY b "&a;&a;">]><svg xmlns="http://www.w3.org/2000/svg">&b;</svg>"#;
        let (_d, path) = svg(bomb);
        assert!(rasterize_svg(&path).is_err());
    }

    #[test]
    fn what_a_file_can_repeat_has_a_limit() {
        let dashed = r#"<path d="M0 0H9" stroke="red" stroke-dasharray="0.001"/>"#;
        assert!(refused_with(dashed).contains("dashed"));
        let deep = format!(
            "{}{}{}",
            r#"<g opacity="0.5">"#.repeat(MAX_ICON_DEPTH + 2),
            r#"<rect width="9" height="9"/>"#,
            "</g>".repeat(MAX_ICON_DEPTH + 2)
        );
        assert!(refused_with(&deep).contains("deep"));
        let many = r#"<rect width="9" height="9"/>"#.repeat(MAX_ICON_PARTS + 1);
        assert!(refused_with(&many).contains("many"));
    }

    #[test]
    fn a_layer_beyond_the_renderers_sums_is_refused() {
        let why =
            refused_with(r##"<g opacity="0.5"><rect width="1000000000" height="1000000000" fill="#ff0000"/></g>"##);
        assert!(why.contains("large"), "{why}");
        let why = refused_with(
            r##"<g opacity="0.5" transform="translate(-1000000 0)"><rect width="9" height="9"/><rect x="1" width="9" height="9"/></g>"##,
        );
        assert!(why.contains("large"), "{why}");
    }

    #[test]
    fn what_a_file_declares_does_not_size_the_output() {
        let rect = r##"<rect width="10" height="10" fill="#ff0000"/>"##;
        let ns = r#"xmlns="http://www.w3.org/2000/svg""#;
        for body in [
            format!(r#"<svg {ns} width="1000000000" height="1000000000">{rect}</svg>"#),
            format!(r#"<svg {ns} width="1000000000" height="1" viewBox="0 0 10 10">{rect}</svg>"#),
            format!(r#"<svg {ns} width="0.001" height="0.001" viewBox="0 0 1000000000 1000000000">{rect}</svg>"#),
            format!(r#"<svg {ns} width="1000000" height="1000000" viewBox="0 0 0.001 0.001">{rect}</svg>"#),
            // a nested picture and an isolated layer, both declaring far more than the canvas
            format!(r#"<svg {ns} viewBox="0 0 10 10"><svg width="1000000000" height="1000000000">{rect}</svg></svg>"#),
            format!(
                r##"<svg {ns} viewBox="0 0 10 10"><g opacity="0.5"><rect width="1000000000" height="1000000000" fill="#ff0000"/></g></svg>"##
            ),
            // strokes and clips far larger than the canvas
            format!(
                r##"<svg {ns} viewBox="0 0 10 10"><path d="M-1000000 -1000000L1000000 1000000" stroke="#ff0000" stroke-width="1000000"/></svg>"##
            ),
            format!(
                r##"<svg {ns} viewBox="0 0 10 10"><clipPath id="c"><rect width="1000000000" height="1000000000"/></clipPath><g clip-path="url(#c)">{rect}</g></svg>"##
            ),
        ] {
            let (_d, path) = svg(&body);
            // refusing is fine; what it may not do is draw anything bigger, or fall over
            if let Ok((image, _)) = rasterize_svg(&path) {
                let side = ICON_PIXELS as usize;
                assert_eq!(image.size, [side, side], "{body}");
            }
        }
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
    fn data_images_fonts_and_references_outside_load_nothing() {
        let head = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10">"#;
        let nested = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 1'%3E%3Crect width='1' height='1' fill='red'/%3E%3C/svg%3E";
        for body in [
            format!(r#"<image width="10" height="10" xlink:href="{nested}"/>"#),
            r#"<style>@import url("other.css"); @font-face { src: url("other.ttf") }</style>"#.to_string(),
            r#"<text x="1" y="9" font-family="Other">T</text>"#.to_string(),
        ] {
            let (_d, path) = svg(&format!("{head}{body}</svg>"));
            let (image, _) = rasterize_svg(&path).unwrap_or_else(|e| panic!("{body}: {e}"));
            assert!(image.pixels.iter().all(|p| *p == egui::Color32::TRANSPARENT), "{body}");
        }
    }

    /// The worker's answers for `assets`, on a context of its own.
    fn run_until_idle(assets: &mut FormAssets, ctx: &Context) {
        let started = std::time::Instant::now();
        while !assets.is_idle() {
            assert!(started.elapsed().as_secs() < 180, "the worker did not answer");
            assets.poll(ctx);
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_reference_is_resolved_off_the_frame_and_only_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.png"), "x").unwrap();
        let ctx = Context::default();
        let mut assets = FormAssets::default();
        assert_eq!(
            assets.resolve(&ctx, dir.path(), None, "a%20b.png"),
            None,
            "no answer on the frame"
        );
        assert_eq!(assets.pending, 1);
        assert_eq!(assets.resolve(&ctx, dir.path(), None, "a%20b.png"), None);
        assert_eq!(assets.pending, 1, "asked once");
        run_until_idle(&mut assets, &ctx);
        let file = assets.resolve(&ctx, dir.path(), None, "a%20b.png").unwrap().unwrap();
        assert!(file.ends_with("a b.png"), "{file}");
        assert_eq!(assets.pending, 0, "known from now on");
        // not a local file at all
        assert_eq!(
            assets.resolve(&ctx, dir.path(), None, "https://example.org/a.png"),
            None
        );
        run_until_idle(&mut assets, &ctx);
        assert_eq!(
            assets.resolve(&ctx, dir.path(), None, "https://example.org/a.png"),
            Some(None)
        );
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
