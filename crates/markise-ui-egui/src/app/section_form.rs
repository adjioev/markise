//! The small form behind the library's section item (ADR 0026): a title, and metadata only if the site
//! declares any. Nothing is written until Save. See [`markise_engine::editor::NewSection`].

use egui::accesskit::Role;
use egui::{Context, Id, Key, RichText, TextEdit};
use markise_engine::editor::NewSection;
use markise_package::{DEFAULT_SECTION_LABEL, SectionConfig, is_section_value};

use super::MarkiseApp;
use crate::block_form::package_language;
use crate::dialog::{Width, footer, modal, primary, secondary, title};

const TITLE_ID: &str = "markise-section-form-title";

/// One metadata key the site declares, in the document's language.
struct FormField {
    key: String,
    label: String,
    /// The values to choose from with what they say; none: any one line.
    choices: Vec<(String, String)>,
    value: String,
}

/// The open form.
pub struct SectionForm {
    /// The document and revision it was opened for: another one closes it, with nothing written.
    origin: (u64, u64),
    in_source: bool,
    heading: String,
    title: String,
    fields: Vec<FormField>,
    focus: bool,
    previous_focus: Option<Id>,
}

impl SectionForm {
    /// What its title says: the site's word for a section, or the default.
    pub fn heading(&self) -> &str {
        &self.heading
    }

    /// The keys it asks for, in order.
    pub fn keys(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.key.as_str()).collect()
    }

    /// Save can go on: a heading needs some text, and what is filled in has to be a metadata value.
    pub fn can_save(&self) -> bool {
        !self.title.trim().is_empty() && self.fields.iter().all(FormField::fits)
    }

    fn metadata(&self) -> Vec<(String, String)> {
        (self.fields.iter())
            .map(|f| (f.key.clone(), f.written().to_string()))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}

impl FormField {
    /// What goes in the file: a value is literal, and only what is around it goes, as a reader takes that off.
    fn written(&self) -> &str {
        self.value.trim()
    }

    /// Empty, which is not written, or a value by the grammar of ADR 0026.
    fn fits(&self) -> bool {
        self.written().is_empty() || is_section_value(self.written())
    }
}

fn shown(text: Option<&markise_package::Localized>, language: &str, default: &str) -> String {
    text.map(|l| l.get(language).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn form_fields(config: Option<&SectionConfig>, language: &str) -> Vec<FormField> {
    (config.iter().flat_map(|c| &c.fields))
        .map(|f| FormField {
            key: f.key.clone(),
            label: shown(f.label.as_ref(), language, &f.key),
            choices: (f.values.iter())
                .map(|v| (v.value.clone(), shown(v.label.as_ref(), language, &v.value)))
                .collect(),
            value: String::new(),
        })
        .collect()
}

impl MarkiseApp {
    /// The open form of a new section.
    pub fn section_form(&self) -> Option<&SectionForm> {
        self.section_form.as_ref()
    }

    /// Sets what the open form's title field holds (tests).
    pub fn set_section_form_title(&mut self, text: &str) {
        if let Some(f) = &mut self.section_form {
            f.title = text.to_string();
        }
    }

    /// Sets what the open form holds for this key (tests).
    pub fn set_section_form_value(&mut self, key: &str, value: &str) {
        if let Some(f) = (self.section_form.as_mut()).and_then(|o| o.fields.iter_mut().find(|f| f.key == key)) {
            f.value = value.to_string();
        }
    }

    /// The library's section item was chosen.
    pub(super) fn open_section_form(&mut self, ctx: &Context, in_source: bool) -> bool {
        let Some(d) = &self.doc else { return false };
        if d.viewer.ed.session.read_only().is_some() || !d.viewer.ed.view.sections_on() {
            return false;
        }
        let language = package_language(d.viewer.ed.session.doc());
        let config = self.block_package().and_then(|p| p.index.sections());
        self.section_form = Some(SectionForm {
            origin: (d.id, d.viewer.ed.session.revision().counter),
            in_source,
            heading: shown(config.and_then(|c| c.label.as_ref()), &language, DEFAULT_SECTION_LABEL),
            title: String::new(),
            fields: form_fields(config, &language),
            focus: true,
            previous_focus: ctx.memory(|m| m.focused()),
        });
        true
    }

    /// Closes it with nothing written, the keyboard going back where it was.
    pub fn cancel_section_form(&mut self, ctx: &Context) {
        if let Some(id) = self.section_form.take().and_then(|f| f.previous_focus) {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }

    /// Writes the section of the open form and closes it. `false`: nothing was written.
    pub fn save_section_form(&mut self, ctx: &Context) -> bool {
        if self.section_form.as_ref().is_none_or(|f| !f.can_save()) {
            return false;
        }
        let Some(form) = self.section_form.take() else {
            return false;
        };
        if let Some(id) = form.previous_focus {
            ctx.memory_mut(|m| m.request_focus(id));
        }
        let Some(d) = &mut self.doc else { return false };
        let new = NewSection {
            title: form.title.clone(),
            id: d.viewer.ed.free_section_id(),
            metadata: form.metadata(),
        };
        let result = d.viewer.ed.insert_section(&new, form.in_source);
        ctx.request_repaint();
        match result {
            Ok(_) => {
                if form.in_source {
                    d.source_ui.find_reveal = true;
                } else {
                    d.viewer.find_reveal = true;
                }
                self.remember_block_views();
                true
            }
            Err(why) => {
                self.banner = Some(super::Banner::Error(format!(
                    "“{}” was not inserted: {why}.",
                    form.heading
                )));
                false
            }
        }
    }

    /// Per frame. Another document, an edit or Read ends it, with nothing written.
    pub(super) fn section_form_window(&mut self, ctx: &Context) {
        let Some(form) = &mut self.section_form else { return };
        let current = self.doc.as_ref().and_then(|d| {
            (d.viewer.ed.session.read_only().is_none()).then(|| (d.id, d.viewer.ed.session.revision().counter))
        });
        if current != Some(form.origin) {
            self.cancel_section_form(ctx);
            return;
        }
        let (act, dismissed) = modal(ctx, "markise-section-form", Width::Narrow, |ui, look| {
            title(ui, look, &form.heading);
            ui.label(RichText::new("Title").color(look.weak));
            let id = Id::unique(TITLE_ID);
            let edit = TextEdit::singleline(&mut form.title)
                .id(id)
                .desired_width(f32::INFINITY);
            ui.add(edit)
                .widget_info(|| egui::WidgetInfo::labeled(Role::TextInput, true, "Title"));
            if std::mem::take(&mut form.focus) {
                ui.memory_mut(|m| m.request_focus(id));
            }
            // a site can declare many: the fields scroll, and the buttons stay
            let height = (ui.ctx().content_rect().height() - 200.0).max(80.0);
            egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for field in &mut form.fields {
                        ui.add_space(8.0);
                        let named = ui.label(RichText::new(&field.label).color(look.weak));
                        if field.choices.is_empty() {
                            let edit = TextEdit::singleline(&mut field.value).desired_width(f32::INFINITY);
                            let label = field.label.clone();
                            ui.add(edit)
                                .widget_info(|| egui::WidgetInfo::labeled(Role::TextInput, true, &label));
                        } else {
                            let chosen = (field.choices.iter())
                                .find(|(v, _)| *v == field.value)
                                .map_or(String::new(), |(_, l)| l.clone());
                            egui::ComboBox::from_id_salt(("markise-section-field", &field.key))
                                .selected_text(chosen)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut field.value, String::new(), "");
                                    for (value, label) in &field.choices {
                                        ui.selectable_value(&mut field.value, value.clone(), label);
                                    }
                                })
                                .response
                                .labelled_by(named.id);
                        }
                        if !field.fits() {
                            ui.label(
                                RichText::new("One line, up to 200 characters.")
                                    .color(look.danger)
                                    .small(),
                            );
                        }
                    }
                });
            let can_save = form.can_save();
            let mut act = None;
            footer(ui, |ui| {
                if ui
                    .add_enabled_ui(can_save, |ui| primary(ui, look, "Save"))
                    .inner
                    .clicked()
                {
                    act = Some(true);
                }
                if secondary(ui, look, "Cancel").clicked() {
                    act = Some(false);
                }
            });
            if can_save && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                act = Some(true);
            }
            act
        });
        match act {
            Some(true) => {
                self.save_section_form(ctx);
            }
            Some(false) => self.cancel_section_form(ctx),
            None if dismissed => self.cancel_section_form(ctx),
            None => {}
        }
    }
}
