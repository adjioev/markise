//! The Preferences window (issue #50; spec 04 §2.1, spec 05 §4): ⌘, on macOS (application
//! menu), Ctrl+, elsewhere (Edit menu).
//!
//! One movable window in the shared dialog style ([`crate::dialog`]): a search box on top, the
//! section list on the left and the section's settings on the right. It only shows settings
//! the app already has — the settings file's model (`markise_engine::settings`) and the
//! menus' toggles — and changes them through the same setters as the menus, so both stay in
//! sync and every change is applied at once and persisted like a menu toggle.
//!
//! The window is drawn from a snapshot of the current values ([`Values`]) and reports what the
//! user changed as [`Change`]s; the app applies them (`app/prefs.rs`). Rows are a static table
//! ([`ROWS`]) so the search can filter them by label without drawing anything.

use std::path::PathBuf;

use egui::{Context, RichText, Role, Ui, WidgetInfo};
use markise_engine::settings::{Appearance, EditingSettings, SourceSettings, ToolbarPosition};
use markise_theme::Overrides;
use markise_theme::catalog::{StyleId, ThemeId};

use crate::dialog::{self, Look};

/// The window's sections, in list order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Section {
    #[default]
    General,
    Editor,
    Markdown,
    Appearance,
    Source,
    Files,
    Export,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::General,
        Section::Editor,
        Section::Markdown,
        Section::Appearance,
        Section::Source,
        Section::Files,
        Section::Export,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Editor => "Editor",
            Section::Markdown => "Markdown",
            Section::Appearance => "Appearance",
            Section::Source => "Source Mode",
            Section::Files => "Files & Images",
            Section::Export => "Export",
        }
    }

    /// Whether "Reset to Defaults" applies (the section has settings with defaults).
    pub fn resettable(self) -> bool {
        matches!(
            self,
            Section::Editor | Section::Markdown | Section::Appearance | Section::Source
        )
    }
}

/// One setting (or one control group) in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowId {
    SettingsFile,
    KeyBindings,
    AutoPairBrackets,
    AutoPairMarkdown,
    SmartPunctuation,
    SmartDashesEm,
    Diagrams,
    Wikilinks,
    HighlightCode,
    Theme,
    Style,
    Colors,
    Appearance,
    FontAndSize,
    Toolbar,
    ToolbarPosition,
    ShowRecent,
    Zoom,
    LineNumbers,
    SoftWrap,
    CurrentLine,
    Whitespace,
    ThemeFiles,
}

/// A row of the table: where it lives, its label (also its accessible name and what the
/// search matches) and extra search words.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub id: RowId,
    pub section: Section,
    /// Sub-heading inside the section (rows of one group are adjacent).
    pub group: &'static str,
    pub label: &'static str,
    pub keywords: &'static str,
}

const fn row(id: RowId, section: Section, group: &'static str, label: &'static str, keywords: &'static str) -> Row {
    Row {
        id,
        section,
        group,
        label,
        keywords,
    }
}

/// Every row, in display order.
pub const ROWS: &[Row] = &[
    row(
        RowId::KeyBindings,
        Section::General,
        "Keyboard",
        "Custom key bindings",
        "shortcuts keyboard keybinding accelerator",
    ),
    row(
        RowId::AutoPairBrackets,
        Section::Editor,
        "Typing",
        "Auto-pair brackets and quotes",
        "autopair match close",
    ),
    row(
        RowId::AutoPairMarkdown,
        Section::Editor,
        "Typing",
        "Auto-pair Markdown syntax",
        "autopair emphasis asterisk underscore backtick",
    ),
    row(
        RowId::SmartPunctuation,
        Section::Editor,
        "Typing",
        "Smart punctuation (convert on input)",
        "quotes typography curly",
    ),
    row(
        RowId::SmartDashesEm,
        Section::Editor,
        "Typing",
        "Smart dashes: -- as em dash",
        "hyphen en dash typography",
    ),
    row(
        RowId::Diagrams,
        Section::Markdown,
        "Extensions",
        "Render diagrams",
        "mermaid flowchart sequence",
    ),
    row(
        RowId::Wikilinks,
        Section::Markdown,
        "Extensions",
        "Wikilinks [[Note]]",
        "links vault obsidian",
    ),
    row(
        RowId::HighlightCode,
        Section::Markdown,
        "Code",
        "Highlight code (this session)",
        "syntax fences colors",
    ),
    row(RowId::Theme, Section::Appearance, "Theme", "Complete theme", "css"),
    row(RowId::Style, Section::Appearance, "Theme", "Style", "theme layout"),
    row(
        RowId::Colors,
        Section::Appearance,
        "Theme",
        "Colors",
        "theme palette colours",
    ),
    row(
        RowId::Appearance,
        Section::Appearance,
        "Theme",
        "Light or dark",
        "appearance dark mode light mode system auto",
    ),
    row(
        RowId::ThemeFiles,
        Section::Appearance,
        "Theme",
        "Theme files",
        "import css open theme folder report coverage",
    ),
    row(
        RowId::FontAndSize,
        Section::Appearance,
        "Font & size",
        "Font & size",
        "body font size line height content width typography",
    ),
    row(
        RowId::Toolbar,
        Section::Appearance,
        "Window",
        "Show toolbar",
        "formatting toolbar",
    ),
    row(
        RowId::ToolbarPosition,
        Section::Appearance,
        "Window",
        "Toolbar position",
        "top bottom",
    ),
    row(
        RowId::ShowRecent,
        Section::Appearance,
        "Window",
        "Show recent files in the sidebar",
        "recent sidebar history",
    ),
    row(RowId::Zoom, Section::Appearance, "Window", "Zoom", "scale size magnify"),
    row(RowId::LineNumbers, Section::Source, "Display", "Line numbers", "gutter"),
    row(RowId::SoftWrap, Section::Source, "Display", "Soft wrap", "wrap lines"),
    row(
        RowId::CurrentLine,
        Section::Source,
        "Display",
        "Highlight current line",
        "caret cursor line",
    ),
    row(
        RowId::Whitespace,
        Section::Source,
        "Display",
        "Show whitespace",
        "spaces tabs invisibles marks",
    ),
    row(
        RowId::SettingsFile,
        Section::Files,
        "Locations",
        "Settings file",
        "json config location reveal themes folder",
    ),
];

/// Whether `row` matches the search `query` (case-insensitive, by label or search words;
/// an empty query matches every row).
pub fn matches(row: &Row, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || row.label.to_lowercase().contains(&q) || row.keywords.contains(&q)
}

/// The rows a search shows.
pub fn search(query: &str) -> Vec<&'static Row> {
    ROWS.iter().filter(|r| matches(r, query)).collect()
}

/// Window state.
#[derive(Default)]
pub struct PrefsState {
    open: bool,
    pub section: Section,
    pub query: String,
    /// The search box takes the keyboard focus at the next frame (just opened).
    focus_search: bool,
}

impl PrefsState {
    pub fn open(&mut self) {
        if !self.open {
            self.focus_search = true;
        }
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// The style's own typography (shown while not overridden).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontDefaults {
    pub body_font: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub content_width: f32,
}

/// The current values the window shows.
#[derive(Clone, Debug, Default)]
pub struct Values {
    pub editing: EditingSettings,
    pub source: SourceSettings,
    pub diagrams: bool,
    pub wikilinks: bool,
    pub highlight_code: bool,
    /// Complete themes (id, label) and the selected one (`None`: style and colors mixed).
    pub themes: Vec<(ThemeId, String)>,
    pub theme: Option<ThemeId>,
    pub styles: Vec<(StyleId, String)>,
    pub style: Option<StyleId>,
    pub colors: Vec<(ThemeId, String)>,
    pub color: Option<ThemeId>,
    pub appearance: Appearance,
    pub overrides: Overrides,
    pub font_defaults: FontDefaults,
    pub show_toolbar: bool,
    pub toolbar_position: ToolbarPosition,
    pub show_recent: bool,
    pub zoom: f32,
    pub zoom_range: (f32, f32),
    pub settings_file: Option<PathBuf>,
    pub theme_folder: Option<PathBuf>,
    /// The session has a user theme folder (Import Theme, Open Theme Folder).
    pub has_theme_folder: bool,
    pub key_bindings: Vec<(String, String)>,
}

/// What the user changed in the window this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Editing(EditingSettings),
    Source(SourceSettings),
    Diagrams(bool),
    Wikilinks(bool),
    HighlightCode(bool),
    Theme(ThemeId),
    Style(StyleId),
    Colors(ThemeId),
    Appearance(Appearance),
    Overrides(Overrides),
    ShowToolbar(bool),
    ToolbarPosition(ToolbarPosition),
    ShowRecent(bool),
    Zoom(f32),
    RevealSettingsFile,
    OpenThemeFolder,
    ImportTheme,
    ThemeReport,
    /// "Reset to Defaults" for a section: asks first (confirm seam).
    AskReset(Section),
}

/// The window's width in points: wide, but never wider than the window (zoomed in).
fn width(ctx: &Context) -> f32 {
    (ctx.content_rect().width() - 48.0).clamp(300.0, 680.0)
}

/// Height of the settings pane (scrolls beyond).
fn pane_height(ctx: &Context) -> f32 {
    (ctx.content_rect().height() - 240.0).clamp(120.0, 420.0)
}

/// Gives `r` the accessible name `label` (a control drawn next to its own label).
fn name(r: &egui::Response, kind: Role, label: &str) {
    let label = label.to_string();
    let enabled = r.enabled();
    r.widget_info(move || WidgetInfo::labeled(kind, enabled, &label));
}

/// Draws the window; returns false when it should close.
pub fn show(ctx: &Context, st: &mut PrefsState, v: &Values, out: &mut Vec<Change>) -> bool {
    if !st.open {
        return false;
    }
    let keep = dialog::window_with(
        ctx,
        "markise-preferences",
        "Preferences",
        width(ctx),
        false,
        |ui, look| body(ui, look, st, v, out),
    );
    if !keep {
        st.open = false;
    }
    keep
}

fn body(ui: &mut Ui, look: &Look, st: &mut PrefsState, v: &Values, out: &mut Vec<Change>) -> bool {
    let search = ui.add(
        egui::TextEdit::singleline(&mut st.query)
            .hint_text("Search settings")
            .desired_width(f32::INFINITY),
    );
    name(&search, Role::TextInput, "Search settings");
    if std::mem::take(&mut st.focus_search) {
        search.request_focus();
    }
    ui.add_space(10.0);
    let searching = !st.query.trim().is_empty();
    let hits = search_hits(&st.query);
    let h = pane_height(ui.ctx());
    ui.horizontal_top(|ui| {
        // the section list
        ui.vertical(|ui| {
            ui.set_width(140.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            for s in Section::ALL {
                let hit = hits.contains(&s);
                let mut text = RichText::new(s.label());
                if searching && !hit {
                    text = text.color(look.weak);
                }
                let selected = !searching && st.section == s;
                let r = ui.add_sized([140.0, 24.0], egui::Button::selectable(selected, text));
                if r.clicked() {
                    st.section = s;
                    st.query.clear();
                }
            }
        });
        // a divider exactly as tall as the pane (a separator would take all the height the
        // window had last frame and make it grow)
        let (line, _) = ui.allocate_exact_size(egui::vec2(9.0, h), egui::Sense::hover());
        ui.painter()
            .vline(line.center().x, line.y_range(), egui::Stroke::new(1.0, look.border));
        // the pane keeps the window's width: long values wrap instead of widening it
        let pane_w = ui.available_width();
        ui.vertical(|ui| {
            ui.set_width(pane_w);
            ui.set_max_width(pane_w);
            egui::ScrollArea::vertical()
                .id_salt("markise-preferences-pane")
                .max_height(h)
                .min_scrolled_height(h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if searching {
                        if hits.is_empty() {
                            ui.label(RichText::new("No settings match.").color(look.weak));
                        }
                        for s in Section::ALL.into_iter().filter(|s| hits.contains(s)) {
                            section_rows(ui, look, s, &st.query, v, out);
                            ui.add_space(12.0);
                        }
                    } else {
                        section_rows(ui, look, st.section, "", v, out);
                        section_note(ui, look, st.section);
                    }
                });
        });
    });
    dialog::footer(ui, |ui| {
        let done = dialog::primary(ui, look, "Done").clicked();
        if !searching && st.section.resettable() {
            let at_defaults = section_at_defaults(st.section, v);
            let r = ui
                .add_enabled_ui(!at_defaults, |ui| dialog::secondary(ui, look, "Reset to Defaults"))
                .inner;
            if r.clicked() {
                out.push(Change::AskReset(st.section));
            }
        }
        done
    })
}

/// Sections with rows matching `query`.
fn search_hits(query: &str) -> Vec<Section> {
    let mut v: Vec<Section> = search(query).into_iter().map(|r| r.section).collect();
    v.dedup();
    v
}

/// Explanations shown under a section's rows (not settings; not searched).
fn section_note(ui: &mut Ui, look: &Look, s: Section) {
    let note = match s {
        Section::Files => {
            group_heading(ui, look, "Images");
            "Options for pasted and dropped images will appear here."
        }
        Section::Export => "Export formats use their built-in options. Pandoc is found automatically on the PATH.",
        _ => return,
    };
    ui.label(RichText::new(note).color(look.weak));
}

fn group_heading(ui: &mut Ui, look: &Look, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(12.0).strong().color(look.weak));
    ui.add_space(2.0);
}

/// The rows of section `s` matching `query`, under the section title and group headings.
fn section_rows(ui: &mut Ui, look: &Look, s: Section, query: &str, v: &Values, out: &mut Vec<Change>) {
    ui.label(RichText::new(s.label()).size(14.0).strong().color(look.text));
    ui.add_space(4.0);
    let mut group = "";
    for r in ROWS.iter().filter(|r| r.section == s && matches(r, query)) {
        if r.group != group {
            group = r.group;
            group_heading(ui, look, group);
        }
        row_ui(ui, look, r, v, out);
        ui.add_space(4.0);
    }
}

fn check(ui: &mut Ui, label: &str, on: bool, enabled: bool) -> Option<bool> {
    let mut v = on;
    let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut v, label));
    r.changed().then_some(v)
}

fn path_line(ui: &mut Ui, look: &Look, p: Option<&std::path::Path>, none: &str) {
    match p {
        Some(p) => {
            ui.add(
                egui::Label::new(RichText::new(p.display().to_string()).monospace().color(look.weak))
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
        None => {
            ui.label(RichText::new(none).color(look.weak));
        }
    }
}

/// A combo box with the label on its left.
fn combo<T: Clone + PartialEq>(
    ui: &mut Ui,
    label: &str,
    current: Option<&T>,
    none_text: &str,
    items: &[(T, String)],
) -> Option<T> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        let l = ui.add_sized([120.0, 20.0], egui::Label::new(label));
        let text = current
            .and_then(|c| items.iter().find(|(id, _)| id == c))
            .map_or(none_text.to_string(), |(_, n)| n.clone());
        let r = egui::ComboBox::from_id_salt(("markise-pref-combo", label))
            .selected_text(text)
            .width(220.0)
            .show_ui(ui, |ui| {
                for (id, n) in items {
                    if ui.selectable_label(current == Some(id), n).clicked() {
                        chosen = Some(id.clone());
                    }
                }
            });
        name(&r.response, Role::ComboBox, label);
        // the label beside it names it (screen readers do not read it twice)
        let _ = r.response.labelled_by(l.id);
    });
    chosen
}

fn row_ui(ui: &mut Ui, look: &Look, r: &Row, v: &Values, out: &mut Vec<Change>) {
    let e = v.editing;
    let s = v.source;
    match r.id {
        RowId::SettingsFile => {
            ui.label(r.label);
            path_line(ui, look, v.settings_file.as_deref(), "Not saved in this session");
            if v.settings_file.is_some() && dialog::secondary(ui, look, crate::sidebar::reveal_label()).clicked() {
                out.push(Change::RevealSettingsFile);
            }
            if let Some(d) = &v.theme_folder {
                ui.label(RichText::new("User themes").color(look.text));
                path_line(ui, look, Some(d), "");
            }
            ui.label(
                RichText::new("Hand edits to settings.json are read at the next start.")
                    .size(12.0)
                    .color(look.weak),
            );
        }
        RowId::KeyBindings => {
            ui.label(r.label);
            if v.key_bindings.is_empty() {
                ui.label(
                    RichText::new("None. Add a \"keyBinding\" object to settings.json to change shortcuts.")
                        .color(look.weak),
                );
            } else {
                egui::Grid::new("markise-pref-keys")
                    .num_columns(2)
                    .spacing([16.0, 2.0])
                    .show(ui, |ui| {
                        for (cmd, accel) in &v.key_bindings {
                            ui.label(RichText::new(cmd).color(look.text));
                            let a = if accel.is_empty() { "(unbound)" } else { accel.as_str() };
                            ui.label(RichText::new(a).monospace().color(look.weak));
                            ui.end_row();
                        }
                    });
            }
        }
        RowId::AutoPairBrackets => {
            if let Some(on) = check(ui, r.label, e.auto_pair_brackets, true) {
                out.push(Change::Editing(EditingSettings {
                    auto_pair_brackets: on,
                    ..e
                }));
            }
        }
        RowId::AutoPairMarkdown => {
            if let Some(on) = check(ui, r.label, e.auto_pair_markdown, true) {
                out.push(Change::Editing(EditingSettings {
                    auto_pair_markdown: on,
                    ..e
                }));
            }
        }
        RowId::SmartPunctuation => {
            if let Some(on) = check(ui, r.label, e.smart_punctuation, true) {
                out.push(Change::Editing(EditingSettings {
                    smart_punctuation: on,
                    ..e
                }));
            }
        }
        RowId::SmartDashesEm => {
            // only with smart punctuation on (as the Edit menu had it)
            if let Some(on) = check(ui, r.label, e.smart_dashes_em, e.smart_punctuation) {
                out.push(Change::Editing(EditingSettings {
                    smart_dashes_em: on,
                    ..e
                }));
            }
        }
        RowId::Diagrams => {
            if let Some(on) = check(ui, r.label, v.diagrams, true) {
                out.push(Change::Diagrams(on));
            }
        }
        RowId::Wikilinks => {
            if let Some(on) = check(ui, r.label, v.wikilinks, true) {
                out.push(Change::Wikilinks(on));
            }
        }
        RowId::HighlightCode => {
            if let Some(on) = check(ui, r.label, v.highlight_code, true) {
                out.push(Change::HighlightCode(on));
            }
        }
        RowId::Theme => {
            if let Some(t) = combo(ui, r.label, v.theme.as_ref(), "Custom (style and colors)", &v.themes) {
                out.push(Change::Theme(t));
            }
        }
        RowId::Style => {
            if let Some(t) = combo(ui, r.label, v.style.as_ref(), "", &v.styles) {
                out.push(Change::Style(t));
            }
        }
        RowId::Colors => {
            if let Some(t) = combo(ui, r.label, v.color.as_ref(), "", &v.colors) {
                out.push(Change::Colors(t));
            }
        }
        RowId::Appearance => {
            ui.horizontal(|ui| {
                ui.add_sized([120.0, 20.0], egui::Label::new(r.label));
                for (a, n) in [
                    (Appearance::System, "Follow system"),
                    (Appearance::Light, "Light"),
                    (Appearance::Dark, "Dark"),
                ] {
                    if ui.radio(v.appearance == a, n).clicked() && v.appearance != a {
                        out.push(Change::Appearance(a));
                    }
                }
            });
        }
        RowId::FontAndSize => {
            let mut o = v.overrides.clone();
            font_controls(ui, &mut o, &v.font_defaults);
            ui.add_space(4.0);
            if ui
                .add_enabled_ui(!o.is_empty(), |ui| {
                    dialog::secondary(ui, look, "Use the Style’s Values")
                })
                .inner
                .clicked()
            {
                o = Overrides::default();
            }
            if o != v.overrides {
                out.push(Change::Overrides(o));
            }
        }
        RowId::Toolbar => {
            if let Some(on) = check(ui, r.label, v.show_toolbar, true) {
                out.push(Change::ShowToolbar(on));
            }
        }
        RowId::ToolbarPosition => {
            ui.horizontal(|ui| {
                ui.add_sized([120.0, 20.0], egui::Label::new(r.label));
                for (p, n) in [(ToolbarPosition::Top, "Top"), (ToolbarPosition::Bottom, "Bottom")] {
                    if ui.radio(v.toolbar_position == p, n).clicked() && v.toolbar_position != p {
                        out.push(Change::ToolbarPosition(p));
                    }
                }
            });
        }
        RowId::ShowRecent => {
            if let Some(on) = check(ui, r.label, v.show_recent, true) {
                out.push(Change::ShowRecent(on));
            }
        }
        RowId::Zoom => {
            ui.horizontal(|ui| {
                ui.add_sized([120.0, 20.0], egui::Label::new(r.label));
                let step = |z: f32, d: i32| ((z * 10.0).round() as i32 + d) as f32 / 10.0;
                let (lo, hi) = v.zoom_range;
                if ui
                    .add_enabled(v.zoom > lo + 1e-3, egui::Button::new("Zoom Out"))
                    .clicked()
                {
                    out.push(Change::Zoom(step(v.zoom, -1)));
                }
                ui.label(RichText::new(format!("{:.0} %", v.zoom * 100.0)).color(look.text));
                if ui
                    .add_enabled(v.zoom < hi - 1e-3, egui::Button::new("Zoom In"))
                    .clicked()
                {
                    out.push(Change::Zoom(step(v.zoom, 1)));
                }
                if ui
                    .add_enabled((v.zoom - 1.0).abs() > 1e-3, egui::Button::new("Actual Size"))
                    .clicked()
                {
                    out.push(Change::Zoom(1.0));
                }
            });
        }
        RowId::LineNumbers => {
            if let Some(on) = check(ui, r.label, s.line_numbers, true) {
                out.push(Change::Source(SourceSettings { line_numbers: on, ..s }));
            }
        }
        RowId::SoftWrap => {
            if let Some(on) = check(ui, r.label, s.soft_wrap, true) {
                out.push(Change::Source(SourceSettings { soft_wrap: on, ..s }));
            }
        }
        RowId::CurrentLine => {
            if let Some(on) = check(ui, r.label, s.current_line, true) {
                out.push(Change::Source(SourceSettings { current_line: on, ..s }));
            }
        }
        RowId::Whitespace => {
            if let Some(on) = check(ui, r.label, s.whitespace_marks, true) {
                out.push(Change::Source(SourceSettings {
                    whitespace_marks: on,
                    ..s
                }));
            }
        }
        RowId::ThemeFiles => {
            ui.horizontal(|ui| {
                ui.add_sized([120.0, 20.0], egui::Label::new(r.label));
                let has = v.has_theme_folder;
                if ui
                    .add_enabled_ui(has, |ui| dialog::secondary(ui, look, "Import Theme…"))
                    .inner
                    .clicked()
                {
                    out.push(Change::ImportTheme);
                }
                if ui
                    .add_enabled_ui(has, |ui| dialog::secondary(ui, look, "Open Theme Folder"))
                    .inner
                    .clicked()
                {
                    out.push(Change::OpenThemeFolder);
                }
                if dialog::secondary(ui, look, "Theme Report…").clicked() {
                    out.push(Change::ThemeReport);
                }
            });
        }
    }
}

/// The "Font & size" controls (body font, base size, line height, content width): shared by
/// the Font & size window and the Appearance section. Edits `o` in place.
pub fn font_controls(ui: &mut Ui, o: &mut Overrides, d: &FontDefaults) {
    egui::Grid::new("font-overrides")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            let l = ui.label("Body font");
            let mut font = o.body_font.clone().unwrap_or_default();
            let hint = d.body_font.clone().unwrap_or_else(|| "style default".into());
            let r = ui.add(
                egui::TextEdit::singleline(&mut font)
                    .hint_text(hint)
                    .desired_width(200.0),
            );
            name(&r, Role::TextInput, "Body font");
            let r = r.labelled_by(l.id);
            if r.changed() {
                o.body_font = (!font.trim().is_empty()).then_some(font);
            }
            ui.end_row();
            let row = |ui: &mut Ui, label: &str, v: &mut Option<f32>, current: f32, (lo, hi): (f32, f32), step: f64| {
                let l = ui.label(label);
                ui.horizontal(|ui| {
                    let mut on = v.is_some();
                    let c = ui.checkbox(&mut on, "").on_hover_text("Override the style");
                    name(&c, Role::CheckBox, &format!("Override {}", label.to_lowercase()));
                    if c.changed() {
                        *v = on.then_some(current);
                    }
                    let mut x = v.unwrap_or(current);
                    let s = ui.add_enabled(on, egui::Slider::new(&mut x, lo..=hi).step_by(step));
                    name(&s, Role::Slider, label);
                    let s = s.labelled_by(l.id);
                    if s.changed() {
                        *v = Some(x);
                    }
                });
                ui.end_row();
            };
            row(
                ui,
                "Base size",
                &mut o.font_size,
                d.font_size,
                Overrides::FONT_SIZE,
                0.5,
            );
            row(
                ui,
                "Line height",
                &mut o.line_height,
                d.line_height,
                Overrides::LINE_HEIGHT,
                0.05,
            );
            row(
                ui,
                "Content width",
                &mut o.content_width,
                d.content_width,
                (Overrides::CONTENT_WIDTH.0, 1600.0),
                10.0,
            );
        });
}

/// Whether every setting of `s` has its default value (Reset to Defaults is then disabled).
pub fn section_at_defaults(s: Section, v: &Values) -> bool {
    match s {
        Section::Editor => v.editing == EditingSettings::default(),
        Section::Source => v.source == SourceSettings::default(),
        Section::Markdown => v.diagrams && v.wikilinks && v.highlight_code,
        Section::Appearance => {
            let sel = markise_theme::catalog::Selection::default();
            v.style.as_ref() == Some(&sel.style)
                && v.color.as_ref() == Some(&sel.colors)
                && v.appearance == Appearance::System
                && v.overrides.is_empty()
                && v.show_toolbar
                && v.toolbar_position == ToolbarPosition::Top
                && v.show_recent
                && (v.zoom - 1.0).abs() < 1e-3
        }
        Section::General | Section::Files | Section::Export => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_but_export_has_rows_and_rows_are_grouped() {
        for s in Section::ALL {
            let n = ROWS.iter().filter(|r| r.section == s).count();
            assert_eq!(n == 0, s == Section::Export, "{s:?}");
        }
        // rows of one section are adjacent, and so are the rows of one group
        let mut seen: Vec<(Section, &str)> = Vec::new();
        for r in ROWS {
            let key = (r.section, r.group);
            if seen.last() != Some(&key) {
                assert!(!seen.contains(&key), "{key:?} split");
                seen.push(key);
            }
        }
    }

    #[test]
    fn search_matches_labels_and_words_case_insensitively() {
        let ids = |q: &str| search(q).iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids("soft WRAP"), [RowId::SoftWrap]);
        assert!(ids("dark").contains(&RowId::Appearance));
        assert_eq!(ids("").len(), ROWS.len());
        assert!(ids("no such setting").is_empty());
        assert_eq!(search_hits("auto-pair"), [Section::Editor]);
    }
}
