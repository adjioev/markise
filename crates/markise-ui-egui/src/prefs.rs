//! The Preferences window (issue #50; spec 04 §2.1, spec 05 §4): ⌘, on macOS (application
//! menu), Ctrl+, elsewhere (Edit menu).
//!
//! One movable window in the shared dialog style ([`crate::dialog`]), laid out like a system
//! settings pane: a sidebar with a search field and the sections (each with a small icon drawn
//! in code, the selected one in an accent pill), and on the right the section's settings in
//! rounded cards — one row per setting, the label on the left and the control right-aligned on
//! a common grid. Booleans are switches, light/dark and the toolbar position are segmented
//! controls, the complete themes are preview cards drawn from each theme's colours. Changes
//! apply at once (no Done button: × or Esc closes); each section ends with a subtle "Reset to
//! Defaults" that asks first.
//!
//! It only shows settings the app already has — the settings file's model
//! (`markise_engine::settings`) and the menus' toggles — and changes them through the same
//! setters as the menus, so both stay in sync and every change is persisted like a menu toggle.
//! The window is drawn from a snapshot of the current values ([`Values`]) and reports what the
//! user changed as [`Change`]s; the app applies them (`app/prefs.rs`). Rows are a static table
//! ([`ROWS`]) so the search can filter them by label without drawing anything.

use std::path::PathBuf;

use egui::{
    Align, Align2, Color32, Context, CornerRadius, FontId, Layout, Margin, Rect, Response, RichText, Role, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo, pos2, vec2,
};
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

    /// The tile colour behind the section's icon.
    fn tint(self) -> Color32 {
        match self {
            Section::General => Color32::from_rgb(0x8e, 0x8e, 0x93),
            Section::Editor => Color32::from_rgb(0xf0, 0x9a, 0x37),
            Section::Markdown => Color32::from_rgb(0x58, 0x56, 0xd6),
            Section::Appearance => Color32::from_rgb(0x2f, 0x7c, 0xf6),
            Section::Source => Color32::from_rgb(0x4a, 0x55, 0x68),
            Section::Files => Color32::from_rgb(0x3b, 0xa5, 0xa0),
            Section::Export => Color32::from_rgb(0x34, 0xa8, 0x53),
        }
    }
}

/// One setting (or one control group) in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowId {
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
    ThemeFiles,
    BodyFont,
    FontSize,
    LineHeight,
    ContentWidth,
    Toolbar,
    ToolbarPosition,
    ShowRecent,
    Zoom,
    LineNumbers,
    SoftWrap,
    CurrentLine,
    Whitespace,
    SettingsFile,
    ThemeFolder,
}

/// A row of the table: where it lives, its label (also its accessible name and what the
/// search matches) and extra search words.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub id: RowId,
    pub section: Section,
    /// The card the row sits in (rows of one card are adjacent); its title is shown above it.
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

use RowId as R;
use Section as S;

/// Every row, in display order.
pub const ROWS: &[Row] = &[
    row(
        R::KeyBindings,
        S::General,
        "Keyboard",
        "Custom key bindings",
        "shortcuts keyboard keybinding accelerator",
    ),
    row(
        R::AutoPairBrackets,
        S::Editor,
        "Typing",
        "Auto-pair brackets and quotes",
        "autopair match close",
    ),
    row(
        R::AutoPairMarkdown,
        S::Editor,
        "Typing",
        "Auto-pair Markdown syntax",
        "autopair emphasis asterisk underscore backtick",
    ),
    row(
        R::SmartPunctuation,
        S::Editor,
        "Typography",
        "Smart punctuation (convert on input)",
        "quotes typography curly",
    ),
    row(
        R::SmartDashesEm,
        S::Editor,
        "Typography",
        "Smart dashes: -- as em dash",
        "hyphen en dash typography",
    ),
    row(
        R::Diagrams,
        S::Markdown,
        "Extensions",
        "Render diagrams",
        "mermaid flowchart sequence",
    ),
    row(
        R::Wikilinks,
        S::Markdown,
        "Extensions",
        "Wikilinks [[Note]]",
        "links vault obsidian",
    ),
    row(
        R::HighlightCode,
        S::Markdown,
        "Code",
        "Highlight code (this session)",
        "syntax fences colors",
    ),
    row(R::Theme, S::Appearance, "Theme", "Theme", "complete theme css preview"),
    row(R::Style, S::Appearance, "Style & colors", "Style", "theme layout"),
    row(
        R::Colors,
        S::Appearance,
        "Style & colors",
        "Colors",
        "theme palette colours",
    ),
    row(
        R::Appearance,
        S::Appearance,
        "Style & colors",
        "Light or dark",
        "appearance dark mode light mode system auto",
    ),
    row(
        R::ThemeFiles,
        S::Appearance,
        "Style & colors",
        "Theme files",
        "import css open theme folder report coverage",
    ),
    row(
        R::BodyFont,
        S::Appearance,
        "Font & size",
        "Body font",
        "typography family",
    ),
    row(
        R::FontSize,
        S::Appearance,
        "Font & size",
        "Base size",
        "font size typography",
    ),
    row(
        R::LineHeight,
        S::Appearance,
        "Font & size",
        "Line height",
        "spacing typography",
    ),
    row(
        R::ContentWidth,
        S::Appearance,
        "Font & size",
        "Content width",
        "column width",
    ),
    row(
        R::Toolbar,
        S::Appearance,
        "Window",
        "Show toolbar",
        "formatting toolbar",
    ),
    row(
        R::ToolbarPosition,
        S::Appearance,
        "Window",
        "Toolbar position",
        "top bottom",
    ),
    row(
        R::ShowRecent,
        S::Appearance,
        "Window",
        "Show recent files in the sidebar",
        "recent sidebar history",
    ),
    row(R::Zoom, S::Appearance, "Window", "Zoom", "scale size magnify"),
    row(R::LineNumbers, S::Source, "Display", "Line numbers", "gutter"),
    row(R::SoftWrap, S::Source, "Display", "Soft wrap", "wrap lines"),
    row(
        R::CurrentLine,
        S::Source,
        "Display",
        "Highlight current line",
        "caret cursor line",
    ),
    row(
        R::Whitespace,
        S::Source,
        "Display",
        "Show whitespace",
        "spaces tabs invisibles marks",
    ),
    row(
        R::SettingsFile,
        S::Files,
        "Locations",
        "Settings file",
        "json config location reveal",
    ),
    row(
        R::ThemeFolder,
        S::Files,
        "Locations",
        "User themes",
        "themes folder location css",
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
    /// The search field takes the keyboard focus at the next frame (just opened).
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

/// The colours a theme preview card is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    pub page: Color32,
    pub text: Color32,
    pub heading: Color32,
    pub link: Color32,
    pub code: Color32,
}

impl Swatch {
    pub fn of(t: &markise_theme::Theme) -> Swatch {
        let c = markise_layout::block::color;
        Swatch {
            page: c(t.page_bg),
            text: c(t.text),
            heading: c(t.headings[0].color),
            link: c(t.link),
            code: c(t.code_block.background),
        }
    }
}

/// A complete theme as a preview card.
#[derive(Clone, Debug)]
pub struct ThemeCard {
    pub id: ThemeId,
    pub name: String,
    /// `None` while the theme's import is still running (a neutral card is drawn).
    pub swatch: Option<Swatch>,
}

/// The current values the window shows.
#[derive(Clone, Debug, Default)]
pub struct Values {
    pub editing: EditingSettings,
    pub source: SourceSettings,
    pub diagrams: bool,
    pub wikilinks: bool,
    pub highlight_code: bool,
    /// Complete themes and the selected one (`None`: style and colors mixed).
    pub themes: Vec<ThemeCard>,
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

// ------------------------------------------------------------------------------ metrics

/// Sidebar width (points).
const SIDEBAR_W: f32 = 184.0;
/// Height of a sidebar entry and of a one-line settings row.
const SIDE_ROW_H: f32 = 30.0;
const ROW_H: f32 = 38.0;
/// Horizontal padding inside a card.
const CARD_PAD: f32 = 14.0;
const LABEL_SIZE: f32 = 13.5;

/// The window's width in points: roomy, but never wider than the window (zoomed in).
fn width(ctx: &Context) -> f32 {
    (ctx.content_rect().width() - 48.0).clamp(320.0, 780.0)
}

/// Height of the body (sidebar and settings pane; the pane scrolls beyond).
fn body_height(ctx: &Context) -> f32 {
    (ctx.content_rect().height() - 140.0).clamp(160.0, 660.0)
}

/// Colours of the window's surfaces, derived from the dialog look.
#[derive(Clone, Copy)]
struct Palette {
    look: Look,
    /// Cards on the pane.
    card: Color32,
    /// Separators between rows, card borders.
    line: Color32,
    /// The search field and the segmented controls' track.
    well: Color32,
}

impl Palette {
    fn new(look: Look) -> Palette {
        let (card, well) = if look.dark {
            (
                look.fill.lerp_to_gamma(Color32::WHITE, 0.06),
                look.fill.lerp_to_gamma(Color32::BLACK, 0.25),
            )
        } else {
            (
                look.fill.lerp_to_gamma(Color32::WHITE, 0.75),
                look.fill.lerp_to_gamma(Color32::BLACK, 0.06),
            )
        };
        Palette {
            look,
            card,
            line: look.text.lerp_to_gamma(card, 0.88),
            well,
        }
    }
}

/// Gives `r` the accessible name `label`.
fn name(r: &Response, kind: Role, label: &str) {
    let label = label.to_string();
    let enabled = r.enabled();
    r.widget_info(move || WidgetInfo::labeled(kind, enabled, &label));
}

/// Gives `r` a selectable role (switch, radio segment, card) with its state.
fn name_selected(r: &Response, kind: Role, selected: bool, label: &str) {
    let label = label.to_string();
    let enabled = r.enabled();
    r.widget_info(move || WidgetInfo::selected(kind, enabled, selected, &label));
}

/// The keyboard focus ring around `rect`.
fn focus_ring(ui: &Ui, r: &Response, rect: Rect, radius: u8, p: &Palette) {
    if r.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(radius.saturating_add(2)),
            Stroke::new(2.0, p.look.accent.gamma_multiply(0.7)),
            StrokeKind::Outside,
        );
    }
}

// ------------------------------------------------------------------------------ window

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
        |ui, look| {
            body(ui, &Palette::new(*look), st, v, out);
            false
        },
    );
    if !keep {
        st.open = false;
    }
    keep
}

fn body(ui: &mut Ui, p: &Palette, st: &mut PrefsState, v: &Values, out: &mut Vec<Change>) {
    let h = body_height(ui.ctx());
    let searching = !st.query.trim().is_empty();
    let hits = search_hits(&st.query);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // ---- sidebar
        ui.allocate_ui_with_layout(vec2(SIDEBAR_W, h), Layout::top_down(Align::Min), |ui| {
            ui.set_width(SIDEBAR_W);
            search_field(ui, p, st);
            ui.add_space(12.0);
            for s in Section::ALL {
                let dim = searching && !hits.contains(&s);
                if side_entry(ui, p, s, !searching && st.section == s, dim).clicked() {
                    st.section = s;
                    st.query.clear();
                }
                ui.add_space(2.0);
            }
        });
        ui.add_space(18.0);
        // ---- settings pane: keeps the window's width (long values wrap instead of widening it)
        let pane_w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(pane_w, h), Layout::top_down(Align::Min), |ui| {
            ui.set_width(pane_w);
            egui::ScrollArea::vertical()
                .id_salt("markise-preferences-pane")
                .max_height(h)
                .min_scrolled_height(h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(pane_w - 14.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if searching {
                        if hits.is_empty() {
                            ui.add_space(6.0);
                            ui.label(RichText::new("No settings match.").color(p.look.weak));
                        }
                        for s in Section::ALL.into_iter().filter(|s| hits.contains(s)) {
                            section_ui(ui, p, s, &st.query, v, out);
                            ui.add_space(8.0);
                        }
                    } else {
                        section_ui(ui, p, st.section, "", v, out);
                        section_note(ui, p, st.section);
                        if st.section.resettable() {
                            reset_link(ui, p, st.section, v, out);
                        }
                    }
                    ui.add_space(6.0);
                });
        });
    });
}

/// Sections with rows matching `query`.
fn search_hits(query: &str) -> Vec<Section> {
    let mut v: Vec<Section> = search(query).into_iter().map(|r| r.section).collect();
    v.dedup();
    v
}

/// The rounded search field with a magnifier.
fn search_field(ui: &mut Ui, p: &Palette, st: &mut PrefsState) {
    let (rect, _) = ui.allocate_exact_size(vec2(SIDEBAR_W, 30.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), p.well);
    // magnifier
    let c = rect.left_center() + vec2(15.0, -1.0);
    let s = Stroke::new(1.5, p.look.weak);
    ui.painter().circle_stroke(c, 4.5, s);
    ui.painter().line_segment([c + vec2(3.3, 3.3), c + vec2(6.8, 6.8)], s);
    let edit_rect = Rect::from_min_max(
        pos2(rect.left() + 28.0, rect.top()),
        pos2(rect.right() - 8.0, rect.bottom()),
    );
    let r = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(edit_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.query)
                        .hint_text(RichText::new("Search settings").color(p.look.weak))
                        .frame(egui::Frame::NONE)
                        .text_color(p.look.text)
                        .desired_width(edit_rect.width()),
                )
            },
        )
        .inner;
    name(&r, Role::TextInput, "Search settings");
    if r.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.5, p.look.accent.gamma_multiply(0.8)),
            StrokeKind::Inside,
        );
    }
    if std::mem::take(&mut st.focus_search) {
        r.request_focus();
    }
}

/// A sidebar entry: icon tile, left-aligned label, the selected one in an accent pill.
fn side_entry(ui: &mut Ui, p: &Palette, s: Section, selected: bool, dim: bool) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(SIDEBAR_W, SIDE_ROW_H), Sense::click());
    name_selected(&r, Role::Button, selected, s.label());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(7), p.look.accent);
    } else if r.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), p.well);
    }
    focus_ring(ui, &r, rect, 7, p);
    let tile = Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), vec2(20.0, 20.0));
    painter.rect_filled(tile, CornerRadius::same(5), s.tint());
    paint_icon(painter, s, tile.shrink(4.0), Color32::WHITE);
    let fg = if selected {
        p.look.on_accent
    } else if dim {
        p.look.weak.gamma_multiply(0.7)
    } else {
        p.look.text
    };
    painter.text(
        rect.left_center() + vec2(36.0, 0.0),
        Align2::LEFT_CENTER,
        s.label(),
        FontId::proportional(LABEL_SIZE),
        fg,
    );
    r
}

/// The section icons (12×12, white on the tile), drawn with strokes.
fn paint_icon(painter: &egui::Painter, s: Section, r: Rect, c: Color32) {
    let st = Stroke::new(1.4, c);
    let at = |x: f32, y: f32| pos2(r.left() + x * r.width(), r.top() + y * r.height());
    match s {
        Section::General => {
            // a gear: ring and six teeth
            let ctr = r.center();
            painter.circle_stroke(ctr, r.width() * 0.26, st);
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([ctr + d * r.width() * 0.34, ctr + d * r.width() * 0.5], st);
            }
        }
        Section::Editor => {
            // a pencil
            painter.line_segment([at(0.2, 0.8), at(0.8, 0.2)], Stroke::new(2.4, c));
            painter.line_segment([at(0.08, 0.92), at(0.2, 0.8)], Stroke::new(1.2, c));
        }
        Section::Markdown => {
            // "M" and a down arrow
            painter.line_segment([at(0.05, 0.8), at(0.05, 0.2)], st);
            painter.line_segment([at(0.05, 0.2), at(0.28, 0.5)], st);
            painter.line_segment([at(0.28, 0.5), at(0.5, 0.2)], st);
            painter.line_segment([at(0.5, 0.2), at(0.5, 0.8)], st);
            painter.line_segment([at(0.8, 0.2), at(0.8, 0.8)], st);
            painter.line_segment([at(0.66, 0.62), at(0.8, 0.8)], st);
            painter.line_segment([at(0.94, 0.62), at(0.8, 0.8)], st);
        }
        Section::Appearance => {
            // contrast: a ring with its left half filled
            let ctr = r.center();
            let rad = r.width() * 0.46;
            painter.circle_stroke(ctr, rad, st);
            let pts: Vec<_> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 16.0;
                    ctr + vec2(a.cos(), a.sin()) * rad
                })
                .collect();
            painter.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
        }
        Section::Source => {
            // </>
            painter.line_segment([at(0.3, 0.2), at(0.02, 0.5)], st);
            painter.line_segment([at(0.02, 0.5), at(0.3, 0.8)], st);
            painter.line_segment([at(0.7, 0.2), at(0.98, 0.5)], st);
            painter.line_segment([at(0.98, 0.5), at(0.7, 0.8)], st);
            painter.line_segment([at(0.58, 0.12), at(0.42, 0.88)], st);
        }
        Section::Files => crate::icons::paint(painter, r.expand(1.0), crate::icons::Icon::Folder, c),
        Section::Export => {
            // a tray with an arrow up out of it
            painter.line_segment([at(0.5, 0.05), at(0.5, 0.62)], st);
            painter.line_segment([at(0.28, 0.27), at(0.5, 0.05)], st);
            painter.line_segment([at(0.72, 0.27), at(0.5, 0.05)], st);
            painter.line_segment([at(0.1, 0.55), at(0.1, 0.92)], st);
            painter.line_segment([at(0.1, 0.92), at(0.9, 0.92)], st);
            painter.line_segment([at(0.9, 0.92), at(0.9, 0.55)], st);
        }
    }
}

/// The section's title and its cards (rows matching `query`).
fn section_ui(ui: &mut Ui, p: &Palette, s: Section, query: &str, v: &Values, out: &mut Vec<Change>) {
    ui.label(RichText::new(s.label()).size(19.0).strong().color(p.look.text));
    ui.add_space(12.0);
    let rows: Vec<&Row> = ROWS.iter().filter(|r| r.section == s && matches(r, query)).collect();
    let mut i = 0;
    while i < rows.len() {
        let group = rows[i].group;
        let n = rows[i..].iter().take_while(|r| r.group == group).count();
        card(ui, p, group, |ui| {
            for (k, r) in rows[i..i + n].iter().enumerate() {
                if k > 0 {
                    separator(ui, p);
                }
                row_ui(ui, p, r, v, out);
            }
        });
        i += n;
    }
}

/// A group: small muted title, then a rounded card.
fn card(ui: &mut Ui, p: &Palette, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(11.5).strong().color(p.look.weak));
    });
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(CARD_PAD as i8, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    ui.add_space(18.0);
}

/// A 1px line between two rows of a card.
fn separator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.line));
}

/// Paints a row label at the left of `rect`.
fn paint_label(ui: &Ui, p: &Palette, rect: Rect, text: &str, enabled: bool) {
    let c = if enabled { p.look.text } else { p.look.weak };
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(LABEL_SIZE),
        c,
    );
}

/// A one-line row: the label on the left, `control` right-aligned. Returns its result.
fn control_row<R>(ui: &mut Ui, p: &Palette, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    paint_label(ui, p, rect, label, true);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            control(ui)
        },
    )
    .inner
}

/// A row whose label is a text node of its own (rows without a named control: locations,
/// key bindings), `control` right-aligned.
fn info_row(ui: &mut Ui, p: &Palette, label: &str, control: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.label(RichText::new(label).size(LABEL_SIZE).color(p.look.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                control(ui)
            });
        },
    );
}

/// A switch row: the whole row toggles; the switch is drawn on the right.
fn switch_row(ui: &mut Ui, p: &Palette, label: &str, on: bool, enabled: bool) -> Option<bool> {
    // disabled through egui, so the switch is reported as disabled (and takes no input)
    let w = ui.available_width();
    let (rect, r) = ui
        .add_enabled_ui(enabled, |ui| ui.allocate_exact_size(vec2(w, ROW_H), Sense::click()))
        .inner;
    name_selected(&r, Role::Switch, on, label);
    paint_label(ui, p, rect, label, enabled);
    let sw = Rect::from_min_size(pos2(rect.right() - 38.0, rect.center().y - 11.0), vec2(38.0, 22.0));
    paint_switch(ui, p, &r, sw, on, enabled);
    focus_ring(ui, &r, sw, 11, p);
    (enabled && r.clicked()).then_some(!on)
}

/// The pill switch: accent track and the knob on the right when on.
fn paint_switch(ui: &Ui, p: &Palette, r: &Response, rect: Rect, on: bool, enabled: bool) {
    let t = ui.ctx().animate_bool_with_time(r.id, on, 0.12);
    let off_track = if p.look.dark {
        p.look.fill.lerp_to_gamma(Color32::WHITE, 0.22)
    } else {
        p.look.fill.lerp_to_gamma(Color32::BLACK, 0.16)
    };
    let mut track = off_track.lerp_to_gamma(p.look.accent, t);
    if !enabled {
        track = track.gamma_multiply(0.45);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(11), track);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    let knob = pos2(x, rect.center().y);
    painter.circle_filled(knob + vec2(0.0, 0.6), 9.0, Color32::from_black_alpha(40));
    painter.circle_filled(
        knob,
        9.0,
        if enabled {
            Color32::WHITE
        } else {
            Color32::from_gray(225)
        },
    );
}

/// A segmented control (`[System | Light | Dark]`); returns the segment clicked.
fn segmented<T: Copy + PartialEq>(ui: &mut Ui, p: &Palette, items: &[(T, &str)], current: T) -> Option<T> {
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = items
        .iter()
        .map(|(_, n)| {
            let g = ui.painter().layout_no_wrap(n.to_string(), font.clone(), p.look.text);
            g.size().x + 26.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(7), p.well);
    let mut x = rect.left() + 2.0;
    let mut chosen = None;
    for ((value, label), w) in items.iter().zip(widths) {
        let seg = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        x += w;
        let r = ui.interact(seg, egui::Id::unique(("markise-segment", *label)), Sense::click());
        let on = *value == current;
        name_selected(&r, Role::RadioButton, on, label);
        let painter = ui.painter();
        if on {
            painter.rect_filled(seg, CornerRadius::same(5), p.look.accent);
        } else if r.hovered() {
            painter.rect_filled(seg, CornerRadius::same(5), p.card.gamma_multiply(0.6));
        }
        focus_ring(ui, &r, seg, 5, p);
        let fg = if on { p.look.on_accent } else { p.look.text };
        painter.text(seg.center(), Align2::CENTER_CENTER, *label, font.clone(), fg);
        if r.clicked() && !on {
            chosen = Some(*value);
        }
    }
    chosen
}

/// A combo box (Style, Colors), named by its row label.
fn combo<T: Clone + PartialEq>(ui: &mut Ui, label: &str, current: Option<&T>, items: &[(T, String)]) -> Option<T> {
    let mut chosen = None;
    let text = current
        .and_then(|c| items.iter().find(|(id, _)| id == c))
        .map_or(String::new(), |(_, n)| n.clone());
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
    chosen
}

/// A subtle button (text in the accent colour, no frame).
fn text_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let c = if enabled {
        p.look.accent
    } else {
        p.look.weak.gamma_multiply(0.7)
    };
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(c)).frame(false))
}

/// A small framed button in the card style.
fn card_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).size(12.5).color(p.look.text))
            .fill(p.well)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(6))
            .min_size(vec2(0.0, 24.0)),
    )
}

/// The ↺ that puts an overridden typography value back to the style's.
fn reset_icon(ui: &mut Ui, p: &Palette, what: &str) -> bool {
    let (rect, r) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    name(&r, Role::Button, &format!("Use the style’s {}", what.to_lowercase()));
    let r = r.on_hover_text("Use the style’s value");
    let c = if r.hovered() { p.look.accent } else { p.look.weak };
    let ctr = rect.center();
    let rad = 5.5;
    let pts: Vec<_> = (0..=20)
        .map(|i| {
            let a = -2.4 + i as f32 * 4.6 / 20.0;
            ctr + vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    let start = pts[0];
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.5, c)));
    // arrow head at the start of the arc
    ui.painter()
        .line_segment([start, start + vec2(-0.5, -3.5)], Stroke::new(1.5, c));
    ui.painter()
        .line_segment([start, start + vec2(3.2, -0.8)], Stroke::new(1.5, c));
    focus_ring(ui, &r, rect, 10, p);
    r.clicked()
}

/// A slider row: value on the right, ↺ while it overrides the style's value.
fn slider_row(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    value: &mut Option<f32>,
    default: f32,
    ((lo, hi), step, decimals): ((f32, f32), f64, usize),
) {
    control_row(ui, p, label, |ui| {
        // overridden: the style's value is not in effect (`default` is the composed theme's,
        // which already carries the override)
        let overridden = value.is_some();
        // the ↺ column is always reserved, so the sliders line up
        if overridden {
            if reset_icon(ui, p, label) {
                *value = None;
            }
        } else {
            ui.add_space(20.0);
        }
        let mut x = value.unwrap_or(default);
        ui.add_sized(
            [44.0, 20.0],
            egui::Label::new(
                RichText::new(format!("{x:.decimals$}"))
                    .monospace()
                    .color(if overridden { p.look.text } else { p.look.weak }),
            ),
        );
        ui.spacing_mut().slider_width = 170.0;
        let s = ui.add(egui::Slider::new(&mut x, lo..=hi).step_by(step).show_value(false));
        name(&s, Role::Slider, label);
        if s.changed() {
            *value = Some(x);
        }
    });
}

fn row_ui(ui: &mut Ui, p: &Palette, r: &Row, v: &Values, out: &mut Vec<Change>) {
    let e = v.editing;
    let s = v.source;
    let switch = |ui: &mut Ui, on: bool| switch_row(ui, p, r.label, on, true);
    match r.id {
        R::KeyBindings => {
            if v.key_bindings.is_empty() {
                info_row(ui, p, r.label, |ui| {
                    ui.label(RichText::new("None").color(p.look.weak));
                });
                ui.label(
                    RichText::new("Add a \"keyBinding\" object to settings.json to change shortcuts.")
                        .size(12.0)
                        .color(p.look.weak),
                );
                ui.add_space(10.0);
            } else {
                info_row(ui, p, r.label, |_| {});
                egui::Grid::new("markise-pref-keys")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for (cmd, accel) in &v.key_bindings {
                            ui.label(RichText::new(cmd).color(p.look.text));
                            let a = if accel.is_empty() { "(unbound)" } else { accel.as_str() };
                            ui.label(RichText::new(a).monospace().color(p.look.weak));
                            ui.end_row();
                        }
                    });
                ui.add_space(10.0);
            }
        }
        R::AutoPairBrackets => {
            if let Some(on) = switch(ui, e.auto_pair_brackets) {
                out.push(Change::Editing(EditingSettings {
                    auto_pair_brackets: on,
                    ..e
                }));
            }
        }
        R::AutoPairMarkdown => {
            if let Some(on) = switch(ui, e.auto_pair_markdown) {
                out.push(Change::Editing(EditingSettings {
                    auto_pair_markdown: on,
                    ..e
                }));
            }
        }
        R::SmartPunctuation => {
            if let Some(on) = switch(ui, e.smart_punctuation) {
                out.push(Change::Editing(EditingSettings {
                    smart_punctuation: on,
                    ..e
                }));
            }
        }
        R::SmartDashesEm => {
            // only with smart punctuation on (as the Edit menu had it)
            if let Some(on) = switch_row(ui, p, r.label, e.smart_dashes_em, e.smart_punctuation) {
                out.push(Change::Editing(EditingSettings {
                    smart_dashes_em: on,
                    ..e
                }));
            }
        }
        R::Diagrams => {
            if let Some(on) = switch(ui, v.diagrams) {
                out.push(Change::Diagrams(on));
            }
        }
        R::Wikilinks => {
            if let Some(on) = switch(ui, v.wikilinks) {
                out.push(Change::Wikilinks(on));
            }
        }
        R::HighlightCode => {
            if let Some(on) = switch(ui, v.highlight_code) {
                out.push(Change::HighlightCode(on));
            }
        }
        R::Theme => {
            ui.add_space(12.0);
            if let Some(t) = theme_cards(ui, p, v) {
                out.push(Change::Theme(t));
            }
            if v.theme.is_none() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Custom: the style and colors below come from different themes.")
                        .size(12.0)
                        .color(p.look.weak),
                );
            }
            ui.add_space(12.0);
        }
        R::Style => {
            if let Some(t) = control_row(ui, p, r.label, |ui| combo(ui, r.label, v.style.as_ref(), &v.styles)) {
                out.push(Change::Style(t));
            }
        }
        R::Colors => {
            if let Some(t) = control_row(ui, p, r.label, |ui| combo(ui, r.label, v.color.as_ref(), &v.colors)) {
                out.push(Change::Colors(t));
            }
        }
        R::Appearance => {
            let items = [
                (Appearance::System, "System"),
                (Appearance::Light, "Light"),
                (Appearance::Dark, "Dark"),
            ];
            let picked = control_row(ui, p, r.label, |ui| segmented(ui, p, &items, v.appearance));
            if let Some(a) = picked {
                out.push(Change::Appearance(a));
            }
        }
        R::ThemeFiles => {
            control_row(ui, p, r.label, |ui| {
                if card_button(ui, p, "Theme Report…", true).clicked() {
                    out.push(Change::ThemeReport);
                }
                if card_button(ui, p, "Open Theme Folder", v.has_theme_folder).clicked() {
                    out.push(Change::OpenThemeFolder);
                }
                if card_button(ui, p, "Import Theme…", v.has_theme_folder).clicked() {
                    out.push(Change::ImportTheme);
                }
            });
        }
        R::BodyFont => {
            let mut o = v.overrides.clone();
            control_row(ui, p, r.label, |ui| {
                if o.body_font.is_some() {
                    if reset_icon(ui, p, r.label) {
                        o.body_font = None;
                    }
                } else {
                    ui.add_space(20.0);
                }
                let mut font = o.body_font.clone().unwrap_or_default();
                let hint = v
                    .font_defaults
                    .body_font
                    .clone()
                    .unwrap_or_else(|| "Style default".into());
                let t = ui.add(
                    egui::TextEdit::singleline(&mut font)
                        .hint_text(hint)
                        .desired_width(222.0),
                );
                name(&t, Role::TextInput, r.label);
                if t.changed() {
                    o.body_font = (!font.trim().is_empty()).then_some(font);
                }
            });
            if o != v.overrides {
                out.push(Change::Overrides(o));
            }
        }
        R::FontSize | R::LineHeight | R::ContentWidth => {
            let mut o = v.overrides.clone();
            let d = &v.font_defaults;
            match r.id {
                R::FontSize => slider_row(
                    ui,
                    p,
                    r.label,
                    &mut o.font_size,
                    d.font_size,
                    (Overrides::FONT_SIZE, 0.5, 1),
                ),
                R::LineHeight => slider_row(
                    ui,
                    p,
                    r.label,
                    &mut o.line_height,
                    d.line_height,
                    (Overrides::LINE_HEIGHT, 0.05, 2),
                ),
                _ => slider_row(
                    ui,
                    p,
                    r.label,
                    &mut o.content_width,
                    d.content_width,
                    ((Overrides::CONTENT_WIDTH.0, 1600.0), 10.0, 0),
                ),
            }
            if o != v.overrides {
                out.push(Change::Overrides(o));
            }
        }
        R::Toolbar => {
            if let Some(on) = switch(ui, v.show_toolbar) {
                out.push(Change::ShowToolbar(on));
            }
        }
        R::ToolbarPosition => {
            let items = [(ToolbarPosition::Top, "Top"), (ToolbarPosition::Bottom, "Bottom")];
            let picked = control_row(ui, p, r.label, |ui| segmented(ui, p, &items, v.toolbar_position));
            if let Some(t) = picked {
                out.push(Change::ToolbarPosition(t));
            }
        }
        R::ShowRecent => {
            if let Some(on) = switch(ui, v.show_recent) {
                out.push(Change::ShowRecent(on));
            }
        }
        R::Zoom => {
            let step = |z: f32, d: i32| ((z * 10.0).round() as i32 + d) as f32 / 10.0;
            let (lo, hi) = v.zoom_range;
            control_row(ui, p, r.label, |ui| {
                if card_button(ui, p, "Actual Size", (v.zoom - 1.0).abs() > 1e-3).clicked() {
                    out.push(Change::Zoom(1.0));
                }
                if card_button(ui, p, "Zoom In", v.zoom < hi - 1e-3).clicked() {
                    out.push(Change::Zoom(step(v.zoom, 1)));
                }
                ui.add_sized(
                    [48.0, 20.0],
                    egui::Label::new(RichText::new(format!("{:.0} %", v.zoom * 100.0)).color(p.look.text)),
                );
                if card_button(ui, p, "Zoom Out", v.zoom > lo + 1e-3).clicked() {
                    out.push(Change::Zoom(step(v.zoom, -1)));
                }
            });
        }
        R::LineNumbers => {
            if let Some(on) = switch(ui, s.line_numbers) {
                out.push(Change::Source(SourceSettings { line_numbers: on, ..s }));
            }
        }
        R::SoftWrap => {
            if let Some(on) = switch(ui, s.soft_wrap) {
                out.push(Change::Source(SourceSettings { soft_wrap: on, ..s }));
            }
        }
        R::CurrentLine => {
            if let Some(on) = switch(ui, s.current_line) {
                out.push(Change::Source(SourceSettings { current_line: on, ..s }));
            }
        }
        R::Whitespace => {
            if let Some(on) = switch(ui, s.whitespace_marks) {
                out.push(Change::Source(SourceSettings {
                    whitespace_marks: on,
                    ..s
                }));
            }
        }
        R::SettingsFile => {
            info_row(ui, p, r.label, |ui| {
                if v.settings_file.is_some() && card_button(ui, p, crate::sidebar::reveal_label(), true).clicked() {
                    out.push(Change::RevealSettingsFile);
                }
            });
            path_text(ui, p, v.settings_file.as_deref(), "Not saved in this session");
            ui.label(
                RichText::new("Hand edits to settings.json are read at the next start.")
                    .size(12.0)
                    .color(p.look.weak),
            );
            ui.add_space(10.0);
        }
        R::ThemeFolder => {
            info_row(ui, p, r.label, |_| {});
            path_text(ui, p, v.theme_folder.as_deref(), "No user theme folder in this session");
            ui.add_space(10.0);
        }
    }
}

/// A path under a row, monospace, wrapping, selectable.
fn path_text(ui: &mut Ui, p: &Palette, path: Option<&std::path::Path>, none: &str) {
    let text = match path {
        Some(path) => RichText::new(path.display().to_string()).monospace().size(12.0),
        None => RichText::new(none),
    };
    ui.add(
        egui::Label::new(text.color(p.look.weak))
            .selectable(path.is_some())
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
    ui.add_space(4.0);
}

/// The complete themes as preview cards in a wrapped grid; returns the one clicked.
fn theme_cards(ui: &mut Ui, p: &Palette, v: &Values) -> Option<ThemeId> {
    const W: f32 = 112.0;
    const H: f32 = 108.0;
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for t in &v.themes {
            let (rect, r) = ui.allocate_exact_size(vec2(W, H), Sense::click());
            let selected = v.theme.as_ref() == Some(&t.id);
            name_selected(&r, Role::RadioButton, selected, &t.name);
            let thumb = Rect::from_min_size(rect.min, vec2(W, 76.0));
            paint_thumbnail(ui, p, thumb, t.swatch.as_ref());
            let ring = if selected {
                Stroke::new(2.5, p.look.accent)
            } else if r.hovered() {
                Stroke::new(1.5, p.look.accent.gamma_multiply(0.5))
            } else {
                Stroke::new(1.0, p.line)
            };
            ui.painter()
                .rect_stroke(thumb, CornerRadius::same(8), ring, StrokeKind::Outside);
            focus_ring(ui, &r, thumb, 8, p);
            let name_c = if selected { p.look.accent } else { p.look.text };
            let label = ui
                .painter()
                .layout(t.name.clone(), FontId::proportional(12.5), name_c, W);
            let at = pos2(rect.center().x - label.size().x / 2.0, thumb.bottom() + 8.0);
            ui.painter().galley(at, label, name_c);
            if r.clicked() {
                chosen = Some(t.id.clone());
            }
        }
    });
    chosen
}

/// A miniature document in the theme's colours: page, heading, text lines, a link, a code
/// block. A neutral card while the theme is still loading.
fn paint_thumbnail(ui: &Ui, p: &Palette, rect: Rect, sw: Option<&Swatch>) {
    let painter = ui.painter();
    let Some(sw) = sw else {
        painter.rect_filled(rect, CornerRadius::same(8), p.well);
        return;
    };
    painter.rect_filled(rect, CornerRadius::same(8), sw.page);
    let x0 = rect.left() + 12.0;
    let w = rect.width() - 24.0;
    let bar = |y: f32, frac: f32, h: f32, c: Color32| {
        painter.rect_filled(
            Rect::from_min_size(pos2(x0, rect.top() + y), vec2(w * frac, h)),
            CornerRadius::same(2),
            c,
        );
    };
    bar(11.0, 0.55, 6.0, sw.heading);
    let line = sw.text.gamma_multiply(0.55);
    bar(24.0, 0.92, 3.0, line);
    bar(31.0, 0.78, 3.0, line);
    // a link inside the third line
    bar(38.0, 0.30, 3.0, line);
    painter.rect_filled(
        Rect::from_min_size(pos2(x0 + w * 0.34, rect.top() + 38.0), vec2(w * 0.36, 3.0)),
        CornerRadius::same(2),
        sw.link,
    );
    let code = Rect::from_min_size(pos2(x0, rect.top() + 48.0), vec2(w, 18.0));
    painter.rect_filled(code, CornerRadius::same(3), sw.code);
    painter.rect_filled(
        Rect::from_min_size(code.min + vec2(6.0, 7.5), vec2(w * 0.5, 3.0)),
        CornerRadius::same(2),
        sw.text.gamma_multiply(0.45),
    );
}

/// Explanations shown under a section's cards (not settings; not searched).
fn section_note(ui: &mut Ui, p: &Palette, s: Section) {
    let (title, note) = match s {
        Section::Files => ("Images", "Options for pasted and dropped images will appear here."),
        Section::Export => (
            "Export",
            "Export formats use their built-in options. Pandoc is found automatically on the PATH.",
        ),
        _ => return,
    };
    card(ui, p, title, |ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(note).color(p.look.weak));
        ui.add_space(12.0);
    });
}

/// "Reset to Defaults" at the bottom of a section: a subtle text button (asks first).
fn reset_link(ui: &mut Ui, p: &Palette, s: Section, v: &Values, out: &mut Vec<Change>) {
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        if text_button(ui, p, "Reset to Defaults", !section_at_defaults(s, v)).clicked() {
            out.push(Change::AskReset(s));
        }
    });
}

/// The "Font & size" controls (body font, base size, line height, content width) of the
/// Font & size window. Edits `o` in place.
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
                    // keep the checked state in the accessible info (a plain name would drop it)
                    name_selected(&c, Role::CheckBox, on, &format!("Override {}", label.to_lowercase()));
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
        // rows of one section are adjacent, and so are the rows of one card
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
