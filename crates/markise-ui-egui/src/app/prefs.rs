//! The Preferences window's glue (issue #50): builds the [`Values`] it shows from the same
//! state the menus read, and applies its [`Change`]s through the same setters the menu items
//! use (so menus and window stay in sync, and every change is applied and persisted at once).

use std::path::Path;

use egui::Context;
use markise_engine::settings::{Appearance, EditingSettings, SourceSettings, ToolbarPosition, ZOOM_RANGE};
use markise_theme::Overrides;
use markise_theme::catalog::{Origin, Selection, ThemeFamily, ThemeId};

use super::{Ask, Banner, MarkiseApp};
use crate::prefs::{Change, FontDefaults, Section, Swatch, ThemeCard, Values};

/// The family's name as the Themes menu shows it.
fn family_label(f: &ThemeFamily) -> String {
    match (&f.light, &f.dark) {
        (Some(_), Some(_)) => f.name.clone(),
        (None, Some(_)) => format!("{} (dark only)", f.name),
        _ => format!("{} (light only)", f.name),
    }
}

impl MarkiseApp {
    /// Opens the Preferences window (⌘, / Ctrl+,; brings it to the front when open).
    pub fn open_preferences(&mut self) {
        self.prefs.open();
        if let Some(c) = &self.ctx {
            c.move_to_top(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::unique("markise-preferences"),
            ));
        }
    }

    /// Opens the Preferences window at `section` (View → Theme → Theme Settings…).
    pub fn open_preferences_at(&mut self, section: Section) {
        self.prefs.section = section;
        self.prefs.query.clear();
        self.open_preferences();
    }

    /// The Preferences window is open.
    pub fn is_preferences_open(&self) -> bool {
        self.prefs.is_open()
    }

    /// The Preferences window's state: section, search text (tests, screenshots).
    pub fn preferences(&mut self) -> &mut crate::prefs::PrefsState {
        &mut self.prefs
    }

    /// The style's own typography, shown by the font controls while not overridden.
    pub(super) fn font_defaults(&self) -> FontDefaults {
        let (th, o) = (&self.theme, self.themes.overrides());
        FontDefaults {
            body_font: th.fonts.body.iter().find(|f| Some(*f) != o.body_font.as_ref()).cloned(),
            font_size: th.font_size,
            line_height: th.line_height,
            content_width: th.column_max_width,
        }
    }

    /// The values the Preferences window shows.
    pub fn preference_values(&self) -> Values {
        let t = &self.themes;
        let sel = t.selection();
        let families = t.families();
        let o = t.overrides();
        Values {
            editing: t.editing(),
            source: t.source_prefs(),
            diagrams: self.diagrams,
            wikilinks: t.wikilinks(),
            critic_markup: t.critic_markup(),
            comment_display: t.comment_display(),
            export_comments: t.export_comments(),
            highlight_code: self.highlight_code,
            themes: families
                .iter()
                .filter(|f| f.id == ThemeId::Markise || matches!(f.origin, Origin::Builtin | Origin::User))
                .map(|f| ThemeCard {
                    id: f.id.clone(),
                    name: family_label(f),
                    swatch: self.swatches.iter().find(|(id, _)| *id == f.id).and_then(|(_, s)| *s),
                })
                .collect(),
            theme: sel.complete_theme().cloned(),
            styles: t.style_choices().into_iter().map(|(id, n, _)| (id, n)).collect(),
            style: Some(sel.style.clone()),
            colors: families.iter().map(|f| (f.id.clone(), family_label(f))).collect(),
            color: Some(sel.colors.clone()),
            appearance: t.appearance(),
            overrides: o.clone(),
            font_defaults: self.font_defaults(),
            show_toolbar: self.show_toolbar(),
            toolbar_position: self.toolbar_position(),
            show_recent: self.show_recent(),
            zoom: self.zoom(),
            zoom_range: ZOOM_RANGE,
            settings_file: t.settings_file(),
            theme_folder: t.user_dir().map(Path::to_path_buf),
            has_theme_folder: t.has_user_dir(),
            key_bindings: self.key_bindings_override.clone().unwrap_or_else(|| t.key_bindings()),
            images: t.image_settings().clone(),
        }
    }

    /// Draws the Preferences window and applies what changed in it.
    pub(super) fn preferences_window(&mut self, ctx: &Context) {
        if !self.prefs.is_open() {
            return;
        }
        self.update_swatches();
        let values = self.preference_values();
        let mut changes = Vec::new();
        crate::prefs::show(ctx, &mut self.prefs, &values, &mut changes);
        for c in changes {
            self.apply_preference(ctx, c);
        }
    }

    /// Colours of the theme preview cards in the current appearance (CSS themes are imported
    /// on the theme worker the first time; their card fills in when the import arrives).
    fn update_swatches(&mut self) {
        let dark = self.theme.dark;
        let ids: Vec<ThemeId> = self
            .themes
            .families()
            .iter()
            .filter(|f| f.id == ThemeId::Markise || matches!(f.origin, Origin::Builtin | Origin::User))
            .map(|f| f.id.clone())
            .collect();
        let swatches: Vec<(ThemeId, Option<Swatch>)> = ids
            .into_iter()
            .map(|id| {
                let s = self.themes.preview(&id, dark);
                (id, s)
            })
            .collect();
        self.swatches = swatches;
    }

    /// Applies one change from the Preferences window exactly as the menu item would.
    pub fn apply_preference(&mut self, ctx: &Context, c: Change) {
        match c {
            Change::Editing(e) => self.themes.set_editing(e),
            Change::Source(s) => self.themes.set_source_prefs(s),
            Change::Diagrams(on) => self.set_diagrams(on),
            Change::Wikilinks(on) => self.set_wikilinks(on),
            Change::CriticMarkup(on) => self.set_critic_markup(on),
            Change::CommentDisplay(d) => self.themes.set_comment_display(d),
            Change::ExportComments(on) => self.set_export_comments(on),
            Change::HighlightCode(on) => self.set_highlight_code(on),
            Change::Theme(id) => self.themes.select(id),
            Change::Style(id) => self.themes.select_style(id),
            Change::Colors(id) => self.themes.select_colors(id),
            Change::Appearance(a) => {
                if a != self.themes.appearance() {
                    self.themes.set_appearance(a);
                }
            }
            Change::Overrides(o) => self.themes.set_overrides(o),
            Change::ShowToolbar(on) => self.set_show_toolbar(on),
            Change::ToolbarPosition(p) => self.set_toolbar_position(p),
            Change::ShowRecent(on) => self.set_show_recent(on),
            Change::Zoom(z) => self.set_zoom(z),
            Change::Images(s) => self.themes.set_image_settings(s),
            Change::RevealSettingsFile => {
                let Some(f) = self.themes.settings_file() else { return };
                if f.exists() {
                    self.tree_action(ctx, crate::sidebar::TreeAction::Reveal(f));
                } else if let Some(d) = f.parent() {
                    // nothing written yet: show the folder it will be written to
                    if let Err(e) = std::fs::create_dir_all(d).and_then(|_| crate::themes::open_folder(d)) {
                        self.banner = Some(Banner::Error(format!("Cannot open {}: {e}", d.display())));
                    }
                }
            }
            Change::OpenThemeFolder => self.open_theme_folder(),
            Change::ImportTheme => self.import_theme_dialog(),
            Change::ThemeReport => {
                self.installed = None;
                self.theme_report = true;
            }
            Change::AskReset(s) => self.set_ask(Ask::ResetPrefs(s)),
        }
        ctx.request_repaint();
    }

    /// Puts every setting of Preferences section `s` back to its default (after the user
    /// confirmed "Reset to Defaults").
    pub fn reset_preferences(&mut self, s: Section) {
        match s {
            Section::Editor => self.themes.set_editing(EditingSettings::default()),
            Section::Source => self.themes.set_source_prefs(SourceSettings::default()),
            Section::Markdown => {
                self.set_diagrams(true);
                self.set_wikilinks(true);
                self.set_critic_markup(true);
                self.themes.set_comment_display(Default::default());
                self.set_export_comments(true);
                self.set_highlight_code(true);
            }
            Section::Appearance => {
                self.themes.set_selection(Selection::default());
                self.themes.set_appearance(Appearance::System);
                self.themes.set_overrides(Overrides::default());
                self.set_show_toolbar(true);
                self.set_toolbar_position(ToolbarPosition::Top);
                self.set_show_recent(true);
                if (self.zoom() - 1.0).abs() > 1e-4 {
                    self.set_zoom(1.0);
                }
            }
            Section::Files => self
                .themes
                .set_image_settings(markise_engine::settings::ImageSettings::default()),
            Section::General | Section::Export => {}
        }
        if let Some(c) = &self.ctx {
            c.request_repaint();
        }
    }
}
