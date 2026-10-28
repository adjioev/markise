//! App settings file (ADR 0004 §Settings, ADR 0007): `<config>/markise/settings.json`.
//!
//! Format — a JSON object, UTF-8, written pretty-printed:
//!
//! ```json
//! {
//!   "version": 2,
//!   "style": "style:classic",
//!   "colors": "palette:nord",
//!   "appearance": "system",
//!   "overrides": { "font_family": "Georgia", "font_size": 17, "line_height": 1.7, "content_width": 860 }
//! }
//! ```
//!
//! | Key | Values | Default |
//! |---|---|---|
//! | `version` | `2` (files with `1` are migrated) | `2` |
//! | `style` | `"style:<id>"` (built-in style: `markise`, `classic`, `modern`, `technical`, `compact`) or `"theme:<theme id>"` (the style of a complete theme) | `"style:markise"` |
//! | `colors` | theme family id: `"markise"`, `"palette:<id>"`, `"builtin:<id>"` (`drake`, `lapis`, `phycat`), `"user:<key>"` (user theme family: lowercase file stem without a `-dark`/`-light` suffix) | `"markise"` |
//! | `appearance` | `"system"`, `"light"`, `"dark"` | `"system"` |
//! | `diagrams` | `true` / `false`: render `mermaid`/`sequence`/`flow` fences (spec 01 §4.6; View → Diagrams) | `true` |
//! | `showToolbar` | `true` / `false`: show the formatting toolbar (spec 04 §3.2, spec 05; View → Toolbar) | `false` |
//! | `consumed_pending` | names of recovery files already merged into this file (written with the merge; see [`update_file`]); omitted when empty | none |
//! | `overrides` | object with optional `font_family` (string), `font_size`, `line_height`, `content_width` (numbers); omitted when empty | none |
//! | `editing` | object: `auto_pair_brackets` (default `true`), `auto_pair_markdown` (`false`), `smart_punctuation` (convert on input, `false`), `smart_dashes_em` (`--` → em dash, `false`) (spec 02 §4, §13, §16; Edit menu) | defaults |
//! | `source` | object: `line_numbers` (default `false`), `soft_wrap` (`true`), `current_line` (highlight the caret's line, `true`), `whitespace_marks` (`false`) — source editor display (spec 02 §12.1.1; View menu) | defaults |
//! | `keyBinding` | object: command label (menu title) or id → accelerator (`"Ctrl+Shift+K"`), `""` unbinds (spec 05 §4.7); read only, kept verbatim | none |
//! | `side_by_side_split` | Side by Side divider position: fraction of the editor width given to the source pane, `0.15`–`0.85` (spec 04 §3.3); omitted until the divider is moved | `0.5` |
//!
//! **Versions.** A version-1 file (or one without `version`) stores one `theme` id; it migrates
//! to a complete theme: CSS themes (`builtin:`/`user:`) to `style: "theme:<id>"` +
//! `colors: "<id>"`, native ones (`markise`, `palette:`) to the default style + `colors: "<id>"`.
//! In a version-1 file, `style`/`colors` left over from a newer build are stale (an older build
//! changed `theme` after them) and are ignored. From version 2 on `style`/`colors` are
//! authoritative and `theme` is ignored.
//!
//! **Downgrade policy.** Version 2 also writes `theme` (= the `colors` family) so that an older
//! build shows the same colors as a complete theme (with that theme's own style; it falls back
//! to Markise for families it does not know). When the older build changes the theme it writes
//! `version: 1`, and the next newer build migrates that `theme` again.
//!
//! **Newer versions.** A file with a `version` above [`VERSION`] is read best-effort but never
//! written: saves fail with an error (the UI reports it once), so a newer build's file is not
//! downgraded.
//!
//! Unknown keys — top-level and inside `overrides` — are preserved when the file is rewritten
//! (newer versions, hand edits); a missing, unreadable or malformed file means defaults (a
//! malformed file is not overwritten until a setting changes).
//!
//! **Writes** ([`update_file`]) are a read–merge–write of only the changed fields under an
//! advisory lock (`.settings.lock` next to the file), so two Markise instances do not overwrite
//! each other's unrelated changes; the file itself is replaced through an exclusively created,
//! randomly named temp file renamed over the old one.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Light/dark appearance of the selected theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Follow the OS setting.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub fn as_str(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Option<Appearance> {
        match s {
            "system" => Some(Appearance::System),
            "light" => Some(Appearance::Light),
            "dark" => Some(Appearance::Dark),
            _ => None,
        }
    }
}

/// Typography overrides on top of the selected style ("Font & size…"); `None` = the style's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontOverrides {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    pub content_width: Option<f32>,
}

impl FontOverrides {
    pub fn is_empty(&self) -> bool {
        *self == FontOverrides::default()
    }

    /// Recognized members of the `overrides` object; the rest is returned separately.
    fn split_json(v: Value) -> (FontOverrides, Map<String, Value>) {
        let Value::Object(mut m) = v else {
            return (FontOverrides::default(), Map::new());
        };
        let o = FontOverrides::from_json(&Value::Object(m.clone()));
        for k in Self::KEYS {
            m.remove(k);
        }
        (o, m)
    }

    const KEYS: [&'static str; 4] = ["font_family", "font_size", "line_height", "content_width"];

    fn from_json(v: &Value) -> FontOverrides {
        let num = |k: &str| {
            v.get(k)
                .and_then(Value::as_f64)
                .map(|n| n as f32)
                .filter(|n| n.is_finite() && *n > 0.0)
        };
        FontOverrides {
            font_family: v
                .get("font_family")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 200)
                .map(str::to_string),
            font_size: num("font_size"),
            line_height: num("line_height"),
            content_width: num("content_width"),
        }
    }

    fn to_json(&self, extra: &Map<String, Value>) -> Value {
        let mut m = extra.clone();
        if let Some(f) = &self.font_family {
            m.insert("font_family".into(), Value::from(f.clone()));
        }
        for (k, v) in [
            ("font_size", self.font_size),
            ("line_height", self.line_height),
            ("content_width", self.content_width),
        ] {
            if let Some(v) = v {
                // round to 2 decimals so the file stays readable
                m.insert(k.into(), Value::from(((v as f64) * 100.0).round() / 100.0));
            }
        }
        Value::Object(m)
    }
}

/// Editing preferences (spec 02 §4, §13, §16), settings key `editing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditingSettings {
    pub auto_pair_brackets: bool,
    pub auto_pair_markdown: bool,
    /// Smart punctuation "Convert on Input".
    pub smart_punctuation: bool,
    /// `--` → em dash (instead of en dash, `---` → em dash).
    pub smart_dashes_em: bool,
}

impl Default for EditingSettings {
    fn default() -> Self {
        EditingSettings {
            auto_pair_brackets: true,
            auto_pair_markdown: false,
            smart_punctuation: false,
            smart_dashes_em: false,
        }
    }
}

impl EditingSettings {
    /// The planners' editing preferences.
    pub fn to_prefs(self) -> markise_doc::EditPrefs {
        markise_doc::EditPrefs {
            auto_pair_brackets: self.auto_pair_brackets,
            auto_pair_markdown: self.auto_pair_markdown,
            smart_punctuation: if self.smart_punctuation {
                markise_doc::SmartPunctuation::ConvertOnInput
            } else {
                markise_doc::SmartPunctuation::Off
            },
            smart_dashes: if self.smart_dashes_em {
                markise_doc::SmartDashes::Em
            } else {
                markise_doc::SmartDashes::EnEm
            },
            ..Default::default()
        }
    }
    fn to_json(self) -> Value {
        let mut m = Map::new();
        m.insert("auto_pair_brackets".into(), Value::from(self.auto_pair_brackets));
        m.insert("auto_pair_markdown".into(), Value::from(self.auto_pair_markdown));
        m.insert("smart_punctuation".into(), Value::from(self.smart_punctuation));
        m.insert("smart_dashes_em".into(), Value::from(self.smart_dashes_em));
        Value::Object(m)
    }
    fn from_json(v: &Value) -> Option<Self> {
        let Value::Object(m) = v else { return None };
        let d = EditingSettings::default();
        let b = |k: &str, def: bool| m.get(k).and_then(Value::as_bool).unwrap_or(def);
        Some(EditingSettings {
            auto_pair_brackets: b("auto_pair_brackets", d.auto_pair_brackets),
            auto_pair_markdown: b("auto_pair_markdown", d.auto_pair_markdown),
            smart_punctuation: b("smart_punctuation", d.smart_punctuation),
            smart_dashes_em: b("smart_dashes_em", d.smart_dashes_em),
        })
    }
}

/// Source editor display preferences (spec 02 §12.1.1 display defaults), settings key
/// `source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSettings {
    pub line_numbers: bool,
    pub soft_wrap: bool,
    pub current_line: bool,
    pub whitespace_marks: bool,
}

impl Default for SourceSettings {
    fn default() -> Self {
        SourceSettings {
            line_numbers: false,
            soft_wrap: true,
            current_line: true,
            whitespace_marks: false,
        }
    }
}

impl SourceSettings {
    fn to_json(self) -> Value {
        let mut m = Map::new();
        m.insert("line_numbers".into(), Value::from(self.line_numbers));
        m.insert("soft_wrap".into(), Value::from(self.soft_wrap));
        m.insert("current_line".into(), Value::from(self.current_line));
        m.insert("whitespace_marks".into(), Value::from(self.whitespace_marks));
        Value::Object(m)
    }
    fn from_json(v: &Value) -> Option<Self> {
        let Value::Object(m) = v else { return None };
        let d = SourceSettings::default();
        let b = |k: &str, def: bool| m.get(k).and_then(Value::as_bool).unwrap_or(def);
        Some(SourceSettings {
            line_numbers: b("line_numbers", d.line_numbers),
            soft_wrap: b("soft_wrap", d.soft_wrap),
            current_line: b("current_line", d.current_line),
            whitespace_marks: b("whitespace_marks", d.whitespace_marks),
        })
    }
}

/// The settings the app persists.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Selected style (`markise_theme::catalog::StyleId::to_setting`).
    pub style: String,
    /// Selected color family (`markise_theme::catalog::ThemeId::to_setting`).
    pub colors: String,
    pub appearance: Appearance,
    /// Render diagram fences (spec 01 §4.6, DG-2/DG-7; owner default on).
    pub diagrams: bool,
    /// Show the formatting toolbar (key `showToolbar`, spec 04 §3.2; spec 05 default off).
    pub show_toolbar: bool,
    /// Editing preferences (key `editing`).
    pub editing: EditingSettings,
    /// Source editor display preferences (key `source`).
    pub source: SourceSettings,
    pub overrides: FontOverrides,
    /// Members of `overrides` this version does not know, kept verbatim.
    pub overrides_extra: Map<String, Value>,
    /// Names of recovery files already merged into this file (`consumed_pending`): written in
    /// the same atomic write as the merged values, so a crash before their deletion cannot
    /// replay them; such files are skipped and deleted later.
    pub consumed_pending: Vec<String>,
    /// Side by Side divider position (fraction of the width given to the source pane).
    pub side_by_side_split: Option<f32>,
    /// The file's schema version (written back as [`VERSION`]; a newer file is not written).
    pub version: u64,
    /// Keys this version does not know, kept verbatim.
    pub extra: Map<String, Value>,
}

impl Settings {
    /// `keyBinding` overrides (spec 05 §4.7, ADR 0006 §3.3): the object's string members in
    /// file order (command label or id → accelerator; `""` unbinds). Kept verbatim in
    /// `extra` (the settings UI does not write it).
    pub fn key_bindings(&self) -> Vec<(String, String)> {
        match self.extra.get("keyBinding") {
            Some(Value::Object(m)) => m
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect(),
            _ => Vec::new(),
        }
    }
}

pub const DEFAULT_STYLE: &str = "style:markise";
pub const VERSION: u64 = 2;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            style: DEFAULT_STYLE.into(),
            colors: "markise".into(),
            appearance: Appearance::System,
            diagrams: true,
            show_toolbar: false,
            editing: EditingSettings::default(),
            source: SourceSettings::default(),
            overrides: FontOverrides::default(),
            overrides_extra: Map::new(),
            consumed_pending: Vec::new(),
            side_by_side_split: None,
            version: VERSION,
            extra: Map::new(),
        }
    }
}

/// Override members a settings change touches: outer `None` = unchanged, `Some(None)` =
/// cleared (back to the style's value), `Some(Some(v))` = set. Members are patched one by one,
/// so two instances editing different members do not overwrite each other.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverridesPatch {
    pub font_family: Option<Option<String>>,
    pub font_size: Option<Option<f32>>,
    pub line_height: Option<Option<f32>>,
    pub content_width: Option<Option<f32>>,
}

impl OverridesPatch {
    /// The members that differ between `old` and `new`.
    pub fn diff(old: &FontOverrides, new: &FontOverrides) -> OverridesPatch {
        fn d<T: Clone + PartialEq>(a: &Option<T>, b: &Option<T>) -> Option<Option<T>> {
            (a != b).then(|| b.clone())
        }
        OverridesPatch {
            font_family: d(&old.font_family, &new.font_family),
            font_size: d(&old.font_size, &new.font_size),
            line_height: d(&old.line_height, &new.line_height),
            content_width: d(&old.content_width, &new.content_width),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == OverridesPatch::default()
    }

    /// `later` wins member by member.
    pub fn merge(&mut self, later: OverridesPatch) {
        if later.font_family.is_some() {
            self.font_family = later.font_family;
        }
        if later.font_size.is_some() {
            self.font_size = later.font_size;
        }
        if later.line_height.is_some() {
            self.line_height = later.line_height;
        }
        if later.content_width.is_some() {
            self.content_width = later.content_width;
        }
    }

    pub fn apply(&self, o: &mut FontOverrides) {
        if let Some(v) = &self.font_family {
            o.font_family = v.clone();
        }
        if let Some(v) = self.font_size {
            o.font_size = v;
        }
        if let Some(v) = self.line_height {
            o.line_height = v;
        }
        if let Some(v) = self.content_width {
            o.content_width = v;
        }
    }
}

/// The fields a settings change touches; [`update_file`] merges them into the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsPatch {
    pub style: Option<String>,
    pub colors: Option<String>,
    pub appearance: Option<Appearance>,
    pub diagrams: Option<bool>,
    pub show_toolbar: Option<bool>,
    pub editing: Option<EditingSettings>,
    pub source: Option<SourceSettings>,
    pub overrides: OverridesPatch,
    pub side_by_side_split: Option<f32>,
}

/// Valid divider positions (spec 04 §3.3): both panes stay usable.
pub const SPLIT_RANGE: (f32, f32) = (0.15, 0.85);

fn valid_split(v: &Value) -> Option<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .map(|n| n.clamp(SPLIT_RANGE.0, SPLIT_RANGE.1))
}

fn round_split(v: f32) -> Value {
    Value::from(((v as f64) * 1000.0).round() / 1000.0)
}

impl SettingsPatch {
    pub fn is_empty(&self) -> bool {
        *self == SettingsPatch::default()
    }

    pub fn apply(&self, s: &mut Settings) {
        if let Some(v) = &self.style {
            s.style = v.clone();
        }
        if let Some(v) = &self.colors {
            s.colors = v.clone();
        }
        if let Some(v) = self.appearance {
            s.appearance = v;
        }
        if let Some(v) = self.diagrams {
            s.diagrams = v;
        }
        if let Some(v) = self.show_toolbar {
            s.show_toolbar = v;
        }
        if let Some(v) = self.editing {
            s.editing = v;
        }
        if let Some(v) = self.source {
            s.source = v;
        }
        self.overrides.apply(&mut s.overrides);
        if let Some(v) = self.side_by_side_split {
            s.side_by_side_split = Some(v);
        }
    }

    /// Forgets the fields `saved` sets (they were written later): an older failed change must
    /// not be re-applied over them. Unrelated fields stay.
    pub fn forget(&mut self, saved: &SettingsPatch) {
        if saved.style.is_some() {
            self.style = None;
        }
        if saved.colors.is_some() {
            self.colors = None;
        }
        if saved.appearance.is_some() {
            self.appearance = None;
        }
        if saved.side_by_side_split.is_some() {
            self.side_by_side_split = None;
        }
        if saved.diagrams.is_some() {
            self.diagrams = None;
        }
        if saved.show_toolbar.is_some() {
            self.show_toolbar = None;
        }
        if saved.editing.is_some() {
            self.editing = None;
        }
        if saved.source.is_some() {
            self.source = None;
        }
        let (o, so) = (&mut self.overrides, &saved.overrides);
        if so.font_family.is_some() {
            o.font_family = None;
        }
        if so.font_size.is_some() {
            o.font_size = None;
        }
        if so.line_height.is_some() {
            o.line_height = None;
        }
        if so.content_width.is_some() {
            o.content_width = None;
        }
    }

    /// `later` wins field by field.
    pub fn merge(&mut self, later: SettingsPatch) {
        if later.style.is_some() {
            self.style = later.style;
        }
        if later.colors.is_some() {
            self.colors = later.colors;
        }
        if later.appearance.is_some() {
            self.appearance = later.appearance;
        }
        if later.diagrams.is_some() {
            self.diagrams = later.diagrams;
        }
        if later.show_toolbar.is_some() {
            self.show_toolbar = later.show_toolbar;
        }
        if later.editing.is_some() {
            self.editing = later.editing;
        }
        if later.source.is_some() {
            self.source = later.source;
        }
        self.overrides.merge(later.overrides);
        if later.side_by_side_split.is_some() {
            self.side_by_side_split = later.side_by_side_split;
        }
    }

    /// JSON form (recovery file): absent = unchanged, `null` = cleared override.
    pub fn to_json(&self) -> String {
        let mut m = Map::new();
        if let Some(v) = &self.style {
            m.insert("style".into(), Value::from(v.clone()));
        }
        if let Some(v) = &self.colors {
            m.insert("colors".into(), Value::from(v.clone()));
        }
        if let Some(v) = self.appearance {
            m.insert("appearance".into(), Value::from(v.as_str()));
        }
        if let Some(v) = self.side_by_side_split {
            m.insert("side_by_side_split".into(), round_split(v));
        }
        if let Some(v) = self.diagrams {
            m.insert("diagrams".into(), Value::from(v));
        }
        if let Some(v) = self.show_toolbar {
            m.insert("showToolbar".into(), Value::from(v));
        }
        if let Some(v) = self.editing {
            m.insert("editing".into(), v.to_json());
        }
        if let Some(v) = self.source {
            m.insert("source".into(), v.to_json());
        }
        let mut o = Map::new();
        let op = &self.overrides;
        if let Some(v) = &op.font_family {
            o.insert("font_family".into(), v.clone().map_or(Value::Null, Value::from));
        }
        for (k, v) in [
            ("font_size", op.font_size),
            ("line_height", op.line_height),
            ("content_width", op.content_width),
        ] {
            if let Some(v) = v {
                o.insert(k.into(), v.map_or(Value::Null, |n| Value::from(n as f64)));
            }
        }
        if !o.is_empty() {
            m.insert("overrides".into(), Value::Object(o));
        }
        Value::Object(m).to_string()
    }

    pub fn from_json(text: &str) -> Option<SettingsPatch> {
        let Value::Object(m) = serde_json::from_str::<Value>(text).ok()? else {
            return None;
        };
        let st = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let mut p = SettingsPatch {
            style: st("style"),
            colors: st("colors"),
            appearance: m.get("appearance").and_then(Value::as_str).and_then(Appearance::parse),
            side_by_side_split: m.get("side_by_side_split").and_then(valid_split),
            diagrams: m.get("diagrams").and_then(Value::as_bool),
            show_toolbar: m.get("showToolbar").and_then(Value::as_bool),
            editing: m.get("editing").and_then(EditingSettings::from_json),
            source: m.get("source").and_then(SourceSettings::from_json),
            ..Default::default()
        };
        if let Some(Value::Object(o)) = m.get("overrides") {
            let num = |k: &str| {
                o.get(k)
                    .map(|v| v.as_f64().map(|n| n as f32).filter(|n| n.is_finite() && *n > 0.0))
            };
            p.overrides = OverridesPatch {
                font_family: o.get("font_family").map(|v| v.as_str().map(str::to_string)),
                font_size: num("font_size"),
                line_height: num("line_height"),
                content_width: num("content_width"),
            };
        }
        Some(p)
    }
}

/// Version 1 `theme` → (`style`, `colors`) of version 2.
pub fn migrate_theme(theme: &str) -> (String, String) {
    let theme = theme.trim();
    if theme.starts_with("builtin:") || theme.starts_with("user:") {
        (format!("theme:{theme}"), theme.to_string())
    } else {
        (DEFAULT_STYLE.to_string(), theme.to_string())
    }
}

pub const FILE_NAME: &str = "settings.json";
const MAX_BYTES: u64 = 1 << 20;

impl Settings {
    /// Parses settings JSON; invalid values fall back to their defaults.
    pub fn from_json(text: &str) -> Option<Settings> {
        let Value::Object(mut map) = serde_json::from_str::<Value>(text).ok()? else {
            return None;
        };
        let mut s = Settings::default();
        let valid = |v: Option<Value>| match v {
            Some(Value::String(t)) if !t.trim().is_empty() && t.len() <= 300 => Some(t),
            _ => None,
        };
        // files without a version predate it: version 1
        s.version = map.remove("version").and_then(|v| v.as_u64()).unwrap_or(1);
        let theme = valid(map.remove("theme"));
        let (style, colors) = (valid(map.remove("style")), valid(map.remove("colors")));
        match theme {
            // version 1: `theme` is authoritative; split fields are stale leftovers
            Some(t) if s.version <= 1 => (s.style, s.colors) = migrate_theme(&t),
            _ => {
                if let Some(t) = style {
                    s.style = t;
                }
                if let Some(t) = colors {
                    s.colors = t;
                }
            }
        }
        if let Some(o) = map.remove("overrides") {
            (s.overrides, s.overrides_extra) = FontOverrides::split_json(o);
        }
        if let Some(Value::Array(v)) = map.remove("consumed_pending") {
            s.consumed_pending = v
                .iter()
                .filter_map(Value::as_str)
                .filter(|n| n.starts_with(PENDING_PREFIX) && !n.contains(['/', '\\']))
                .take(1000)
                .map(str::to_string)
                .collect();
        }
        if let Some(a) = map
            .remove("appearance")
            .as_ref()
            .and_then(Value::as_str)
            .and_then(Appearance::parse)
        {
            s.appearance = a;
        }
        s.side_by_side_split = map.remove("side_by_side_split").as_ref().and_then(valid_split);
        if let Some(Value::Bool(b)) = map.remove("diagrams") {
            s.diagrams = b;
        }
        if let Some(Value::Bool(b)) = map.remove("showToolbar") {
            s.show_toolbar = b;
        }
        if let Some(e) = map.remove("editing").as_ref().and_then(EditingSettings::from_json) {
            s.editing = e;
        }
        if let Some(e) = map.remove("source").as_ref().and_then(SourceSettings::from_json) {
            s.source = e;
        }
        s.extra = map;
        Some(s)
    }

    pub fn to_json(&self) -> String {
        let mut map = self.extra.clone();
        map.insert("version".into(), Value::from(VERSION));
        map.insert("style".into(), Value::from(self.style.clone()));
        map.insert("colors".into(), Value::from(self.colors.clone()));
        // for older builds (downgrade policy): the color family as their complete theme
        map.insert("theme".into(), Value::from(self.colors.clone()));
        map.insert("appearance".into(), Value::from(self.appearance.as_str()));
        map.insert("diagrams".into(), Value::from(self.diagrams));
        map.insert("showToolbar".into(), Value::from(self.show_toolbar));
        map.insert("editing".into(), self.editing.to_json());
        map.insert("source".into(), self.source.to_json());
        if !self.consumed_pending.is_empty() {
            map.insert("consumed_pending".into(), Value::from(self.consumed_pending.clone()));
        }
        if let Some(v) = self.side_by_side_split {
            map.insert("side_by_side_split".into(), round_split(v));
        }
        let o = self.overrides.to_json(&self.overrides_extra);
        if o.as_object().is_some_and(|m| !m.is_empty()) {
            map.insert("overrides".into(), o);
        }
        let mut out = serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_else(|_| "{}".into());
        out.push('\n');
        out
    }

    /// Loads `path`; defaults when missing or malformed.
    pub fn load(path: &Path) -> Settings {
        let Ok(meta) = std::fs::metadata(path) else {
            return Settings::default();
        };
        if !meta.is_file() || meta.len() > MAX_BYTES {
            return Settings::default();
        }
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| Settings::from_json(&t))
            .unwrap_or_default()
    }

    /// The file was written by a newer Markise (not written by this version).
    pub fn is_future(&self) -> bool {
        self.version > VERSION
    }

    /// Writes `path` (creating its folder) via a temp file + rename. Refuses to replace a
    /// settings file of a newer version. Prefer [`update_file`], which merges.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.is_future() {
            return Err(future_error(self.version));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // exclusively created, randomly named temp file next to the target, then renamed
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::Builder::new().prefix(".settings-").tempfile_in(dir)?;
        std::io::Write::write_all(&mut tmp, self.to_json().as_bytes())?;
        tmp.persist(path).map(|_| ()).map_err(|e| e.error)
    }
}

fn future_error(v: u64) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("settings.json was written by a newer Markise (version {v}); changes are not saved"),
    )
}

/// A save failed because the file is from a newer version (retrying cannot help).
pub fn is_future_error(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Unsupported
}

/// Changes that could not be written at shutdown: one file per exiting instance,
/// `settings.pending.<pid>-<random>.json` next to the settings file, created exclusively (never
/// read-merged-replaced), applied and removed by [`apply_pending`] at the next start.
pub const PENDING_PREFIX: &str = "settings.pending.";

/// The pending files next to `settings`, oldest first (modification time, then name).
pub fn pending_files(settings: &Path) -> Vec<PathBuf> {
    let dir = settings.parent().unwrap_or(Path::new("."));
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with(PENDING_PREFIX) && n.ends_with(".json")
        })
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            (t, e.path())
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

/// Records `patch` for the next start in a new per-instance pending file: written to a hidden
/// temp file, then published under a unique name without replacing anything, so concurrent
/// writers never overwrite each other and readers never see a partial file.
pub fn write_pending(settings: &Path, patch: &SettingsPatch) -> std::io::Result<PathBuf> {
    let dir = settings.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new().prefix(".settings-pending-").tempfile_in(dir)?;
    std::io::Write::write_all(&mut tmp, patch.to_json().as_bytes())?;
    let random: String = tmp
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().trim_start_matches(".settings-pending-").to_string())
        .unwrap_or_default();
    let mut n = 0u32;
    loop {
        let name = format!("{PENDING_PREFIX}{}-{random}{n}.json", std::process::id());
        match tmp.persist_noclobber(dir.join(&name)) {
            Ok(_) => return Ok(dir.join(name)),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => {
                tmp = e.file;
                n += 1;
            }
            Err(e) => return Err(e.error),
        }
    }
}

/// Applies the pending files ([`write_pending`]) under the settings lock, oldest first, each as
/// a field patch, and removes exactly the files it applied (a file that appears meanwhile stays
/// for the next start). Returns whether any patch was applied.
pub fn apply_pending(settings: &Path) -> std::io::Result<bool> {
    apply_pending_with(settings, || {})
}

fn apply_pending_with(settings: &Path, after_listing: impl FnOnce()) -> std::io::Result<bool> {
    if pending_files(settings).is_empty() {
        return Ok(false);
    }
    let _lock = lock_settings(settings)?;
    let files = pending_files(settings);
    after_listing();
    let mut s = Settings::load(settings);
    if s.is_future() {
        // a newer build owns the file: these changes cannot apply; drop them
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
        return Ok(false);
    }
    let applied = consume(&files, &mut s);
    if applied {
        s.save(settings)?;
    }
    remove_consumed(&files);
    Ok(applied)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Merges the pending `files` into `s`, oldest first, skipping files `s` already lists as
/// consumed, and records all of them as consumed (names of files that no longer exist drop
/// out of the list). Returns whether a patch was applied.
fn consume(files: &[PathBuf], s: &mut Settings) -> bool {
    let mut applied = false;
    for f in files {
        if s.consumed_pending.contains(&file_name(f)) {
            continue; // merged by an earlier write whose cleanup did not finish
        }
        if let Some(p) = std::fs::read_to_string(f)
            .ok()
            .and_then(|t| SettingsPatch::from_json(&t))
            && !p.is_empty()
        {
            p.apply(s);
            applied = true;
        }
    }
    s.consumed_pending = files.iter().map(|f| file_name(f)).collect();
    applied
}

/// Deletes consumed recovery files; a failure is logged and retried by the next write.
fn remove_consumed(files: &[PathBuf]) {
    for f in files {
        if let Err(e) = std::fs::remove_file(f)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "markise: could not remove {}: {e} (skipped and retried later)",
                f.display()
            );
        }
    }
}

fn lock_settings(settings: &Path) -> std::io::Result<std::fs::File> {
    let dir = settings.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".settings.lock"))?;
    lock.lock()?; // released when the file is dropped
    Ok(lock)
}

/// Applies `patch` to the settings file: under the advisory lock `.settings.lock`, reads the
/// current file, applies (and removes) pending recovery files first ([`write_pending`]; they are
/// older than this save), then changes only the patched fields, writes it back (temp file +
/// rename) and returns the merged settings. Fails without writing for a file of a newer version.
pub fn update_file(path: &Path, patch: &SettingsPatch) -> std::io::Result<Settings> {
    update_file_with(path, patch, remove_consumed)
}

fn update_file_with(path: &Path, patch: &SettingsPatch, cleanup: impl FnOnce(&[PathBuf])) -> std::io::Result<Settings> {
    let _lock = lock_settings(path)?;
    let mut s = Settings::load(path);
    if s.is_future() {
        return Err(future_error(s.version));
    }
    // pending changes of instances that exited earlier are older than this save: apply them
    // first, so they can never be restored over it later, and consume them
    // (their names go into the same atomic write, so they are never replayed)
    let pending = pending_files(path);
    consume(&pending, &mut s);
    patch.apply(&mut s);
    s.save(path)?;
    cleanup(&pending);
    Ok(s)
}

/// `<config>/markise/settings.json` for a config folder (`markise_theme::discover::config_dir`).
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("markise").join(FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DG-2/DG-7: `diagrams` defaults to on, round-trips, and patches carry it.
    #[test]
    fn source_settings_default_and_round_trip() {
        let d = Settings::default();
        assert!(!d.source.line_numbers && d.source.soft_wrap && d.source.current_line && !d.source.whitespace_marks);
        let s = Settings::from_json(r#"{"version": 2, "source": {"line_numbers": true}}"#).unwrap();
        assert!(s.source.line_numbers && s.source.soft_wrap);
        assert_eq!(Settings::from_json(&s.to_json()).unwrap().source, s.source);
        let p = SettingsPatch {
            source: Some(SourceSettings {
                whitespace_marks: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(SettingsPatch::from_json(&p.to_json()).unwrap().source, p.source);
    }

    #[test]
    fn editing_settings_default_and_round_trip() {
        let d = Settings::default();
        assert!(d.editing.auto_pair_brackets && !d.editing.auto_pair_markdown);
        let s = Settings::from_json(r#"{"version": 2, "editing": {"auto_pair_markdown": true}}"#).unwrap();
        assert!(s.editing.auto_pair_markdown && s.editing.auto_pair_brackets);
        assert_eq!(Settings::from_json(&s.to_json()).unwrap().editing, s.editing);
        let p = SettingsPatch {
            editing: Some(EditingSettings {
                smart_punctuation: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(SettingsPatch::from_json(&p.to_json()).unwrap().editing, p.editing);
    }

    /// Spec 04 §3.2 / spec 05: `showToolbar` defaults to off, round-trips, and patches carry it.
    #[test]
    fn show_toolbar_setting_defaults_off_and_round_trips() {
        assert!(!Settings::default().show_toolbar);
        assert!(!Settings::from_json(r#"{"version": 2}"#).unwrap().show_toolbar);
        let on = Settings::from_json(r#"{"version": 2, "showToolbar": true}"#).unwrap();
        assert!(on.show_toolbar);
        assert!(on.to_json().contains("\"showToolbar\": true"));
        assert!(Settings::from_json(&on.to_json()).unwrap().show_toolbar);
        assert!(
            !Settings::from_json(r#"{"version": 2, "showToolbar": 1}"#)
                .unwrap()
                .show_toolbar
        );
        let p = SettingsPatch {
            show_toolbar: Some(true),
            ..Default::default()
        };
        assert_eq!(SettingsPatch::from_json(&p.to_json()).unwrap().show_toolbar, Some(true));
        let mut s = Settings::default();
        p.apply(&mut s);
        assert!(s.show_toolbar);
    }

    #[test]
    fn diagrams_setting_defaults_on_and_round_trips() {
        assert!(Settings::default().diagrams);
        assert!(Settings::from_json(r#"{"version": 2}"#).unwrap().diagrams);
        let off = Settings::from_json(r#"{"version": 2, "diagrams": false}"#).unwrap();
        assert!(!off.diagrams);
        assert!(!Settings::from_json(&off.to_json()).unwrap().diagrams);
        assert!(
            Settings::from_json(r#"{"version": 2, "diagrams": "no"}"#)
                .unwrap()
                .diagrams,
            "invalid → default"
        );
        let p = SettingsPatch {
            diagrams: Some(false),
            ..Default::default()
        };
        let back = SettingsPatch::from_json(&p.to_json()).unwrap();
        assert_eq!(back.diagrams, Some(false));
        let mut s = Settings::default();
        back.apply(&mut s);
        assert!(!s.diagrams);
    }

    #[test]
    fn side_by_side_split_round_trips_and_is_clamped() {
        // spec 04 §3.3: the divider position is persisted
        let s = Settings {
            side_by_side_split: Some(0.3333),
            ..Settings::default()
        };
        let back = Settings::from_json(&s.to_json()).unwrap();
        assert_eq!(back.side_by_side_split, Some(0.333));
        assert!(!Settings::default().to_json().contains("side_by_side_split"));
        let wild = Settings::from_json(r#"{"version": 2, "side_by_side_split": 7}"#).unwrap();
        assert_eq!(wild.side_by_side_split, Some(SPLIT_RANGE.1));
        let bad = Settings::from_json(r#"{"version": 2, "side_by_side_split": "x"}"#).unwrap();
        assert_eq!(bad.side_by_side_split, None);
        assert!(!bad.extra.contains_key("side_by_side_split"));
        // patches: merge, forget, recovery JSON
        let mut p = SettingsPatch {
            side_by_side_split: Some(0.4),
            ..Default::default()
        };
        assert_eq!(SettingsPatch::from_json(&p.to_json()).unwrap(), p);
        let mut t = Settings::default();
        p.apply(&mut t);
        assert_eq!(t.side_by_side_split, Some(0.4));
        p.forget(&p.clone());
        assert!(p.is_empty());
    }

    #[test]
    fn round_trip_preserves_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("markise-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = settings_path(&dir);
        // missing file: defaults
        assert_eq!(Settings::load(&path), Settings::default());
        let mut s = Settings {
            style: "style:classic".into(),
            colors: "builtin:phycat".into(),
            appearance: Appearance::Dark,
            overrides: FontOverrides {
                font_family: Some("Georgia".into()),
                font_size: Some(17.5),
                line_height: None,
                content_width: Some(900.0),
            },
            ..Default::default()
        };
        s.extra.insert("future".into(), Value::from(true));
        s.save(&path).unwrap();
        let back = Settings::load(&path);
        assert_eq!(back, s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"version\": 2") && text.contains("\"future\": true"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_values_fall_back() {
        assert_eq!(Settings::from_json("[1,2]"), None);
        assert_eq!(Settings::from_json("not json"), None);
        let s =
            Settings::from_json(r#"{"version": 2, "style": 5, "colors": "", "appearance": "purple", "overrides": 3}"#)
                .unwrap();
        assert_eq!(s, Settings::default());
        let s = Settings::from_json(r#"{"overrides": {"font_size": "big", "line_height": -1, "font_family": " "}}"#)
            .unwrap();
        assert!(s.overrides.is_empty());
    }

    #[test]
    fn version_2_then_an_older_build_edits_theme_then_version_2_again() {
        // a v2 file as this build writes it
        let v2 = Settings {
            style: "style:classic".into(),
            colors: "palette:nord".into(),
            ..Default::default()
        }
        .to_json();
        assert!(
            v2.contains("\"theme\": \"palette:nord\""),
            "compat key for older builds: {v2}"
        );
        // an older build keeps style/colors as unknown keys, changes `theme`, writes version 1
        let mut old: Map<String, Value> = serde_json::from_str(&v2).unwrap();
        old.insert("version".into(), Value::from(1));
        old.insert("theme".into(), Value::from("builtin:drake"));
        let s = Settings::from_json(&Value::Object(old).to_string()).unwrap();
        assert_eq!(
            (s.style.as_str(), s.colors.as_str()),
            ("theme:builtin:drake", "builtin:drake"),
            "the older build's latest choice wins over stale split fields"
        );
        // and a v2 file ignores a `theme` that disagrees with its split fields
        let s = Settings::from_json(
            r#"{"version": 2, "theme": "builtin:drake", "style": "style:compact", "colors": "markise"}"#,
        )
        .unwrap();
        assert_eq!((s.style.as_str(), s.colors.as_str()), ("style:compact", "markise"));
    }

    #[test]
    fn unknown_top_level_and_nested_fields_survive_an_unrelated_change() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        std::fs::write(
            &path,
            r#"{"version": 2, "style": "style:modern", "colors": "markise", "future": [1, 2],
                "overrides": {"font_size": 18, "letter_spacing": 0.5}}"#,
        )
        .unwrap();
        let s = update_file(
            &path,
            &SettingsPatch {
                appearance: Some(Appearance::Dark),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.overrides.font_size, Some(18.0));
        let text = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["future"], serde_json::json!([1, 2]));
        assert_eq!(v["overrides"]["letter_spacing"], serde_json::json!(0.5));
        assert_eq!(v["overrides"]["font_size"], serde_json::json!(18.0));
        assert_eq!(v["appearance"], "dark");
        assert_eq!(v["style"], "style:modern");
        // clearing the known overrides keeps the unknown member
        update_file(
            &path,
            &SettingsPatch {
                overrides: OverridesPatch::diff(
                    &FontOverrides {
                        font_size: Some(18.0),
                        ..Default::default()
                    },
                    &FontOverrides::default(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["overrides"], serde_json::json!({"letter_spacing": 0.5}));
    }

    #[test]
    fn a_newer_version_is_read_but_never_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let text = r#"{"version": 3, "style": "style:compact", "colors": "palette:nord", "shape": "new"}"#;
        std::fs::write(&path, text).unwrap();
        let s = Settings::load(&path);
        assert!(s.is_future());
        assert_eq!((s.style.as_str(), s.colors.as_str()), ("style:compact", "palette:nord"));
        let err = update_file(
            &path,
            &SettingsPatch {
                colors: Some("markise".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("newer Markise"), "{err}");
        assert!(s.save(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "untouched");
    }

    #[test]
    fn two_instances_editing_different_override_members_keep_both() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let empty = FontOverrides::default();
        // both instances started with empty overrides
        let a = SettingsPatch {
            overrides: OverridesPatch::diff(
                &empty,
                &FontOverrides {
                    font_size: Some(18.0),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let b = SettingsPatch {
            overrides: OverridesPatch::diff(
                &empty,
                &FontOverrides {
                    line_height: Some(1.8),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        assert_eq!(b.overrides.font_size, None, "font_size untouched by B");
        update_file(&path, &a).unwrap();
        let s = update_file(&path, &b).unwrap();
        assert_eq!(
            (s.overrides.font_size, s.overrides.line_height),
            (Some(18.0), Some(1.8))
        );
        // an explicit clear removes only that member
        let c = SettingsPatch {
            overrides: OverridesPatch {
                font_size: Some(None),
                ..Default::default()
            },
            ..Default::default()
        };
        let s = update_file(&path, &c).unwrap();
        assert_eq!((s.overrides.font_size, s.overrides.line_height), (None, Some(1.8)));
    }

    #[test]
    fn pending_files_of_two_instances_are_all_applied_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        assert!(!apply_pending(&path).unwrap(), "nothing pending");
        // two exiting instances, concurrently
        let patches = [
            SettingsPatch {
                colors: Some("palette:nord".into()),
                overrides: OverridesPatch {
                    font_size: Some(Some(18.0)),
                    ..Default::default()
                },
                ..Default::default()
            },
            SettingsPatch {
                appearance: Some(Appearance::Dark),
                overrides: OverridesPatch {
                    line_height: Some(Some(1.8)),
                    ..Default::default()
                },
                ..Default::default()
            },
        ];
        let threads: Vec<_> = patches
            .into_iter()
            .map(|p| {
                let path = path.clone();
                std::thread::spawn(move || write_pending(&path, &p).unwrap())
            })
            .collect();
        let files: Vec<PathBuf> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_ne!(files[0], files[1], "one file per instance");
        assert_eq!(pending_files(&path).len(), 2);
        // a later file of the same instance wins on a shared field
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_pending(
            &path,
            &SettingsPatch {
                colors: Some("palette:dracula".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(apply_pending(&path).unwrap());
        let s = Settings::load(&path);
        assert_eq!((s.colors.as_str(), s.appearance), ("palette:dracula", Appearance::Dark));
        assert_eq!(
            (s.overrides.font_size, s.overrides.line_height),
            (Some(18.0), Some(1.8))
        );
        assert!(pending_files(&path).is_empty());
        assert!(!apply_pending(&path).unwrap());
    }

    #[test]
    fn a_pending_file_that_appears_during_apply_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        write_pending(
            &path,
            &SettingsPatch {
                colors: Some("palette:nord".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let late = SettingsPatch {
            style: Some("style:compact".into()),
            ..Default::default()
        };
        let mut late_file = None;
        assert!(apply_pending_with(&path, || late_file = Some(write_pending(&path, &late).unwrap())).unwrap());
        let late_file = late_file.unwrap();
        assert!(late_file.is_file(), "not applied, not deleted");
        assert_eq!(Settings::load(&path).colors, "palette:nord");
        assert_eq!(Settings::load(&path).style, DEFAULT_STYLE);
        // the next start applies it
        assert!(apply_pending(&path).unwrap());
        assert_eq!(Settings::load(&path).style, "style:compact");
    }

    #[test]
    fn a_later_save_wins_over_an_older_pending_file() {
        // A exits with font size 18 pending; running B then saves 20
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        write_pending(
            &path,
            &SettingsPatch {
                colors: Some("palette:nord".into()),
                overrides: OverridesPatch {
                    font_size: Some(Some(18.0)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        update_file(
            &path,
            &SettingsPatch {
                overrides: OverridesPatch {
                    font_size: Some(Some(20.0)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert!(pending_files(&path).is_empty(), "consumed by the save");
        // the next launch restores nothing older
        assert!(!apply_pending(&path).unwrap());
        let s = Settings::load(&path);
        assert_eq!(s.overrides.font_size, Some(20.0));
        assert_eq!(s.colors, "palette:nord", "unrelated pending field preserved");
    }

    #[test]
    fn a_crash_before_deleting_consumed_files_does_not_replay_them() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let stale = write_pending(
            &path,
            &SettingsPatch {
                colors: Some("palette:nord".into()),
                overrides: OverridesPatch {
                    font_size: Some(Some(18.0)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        // the save merges the pending file, then "crashes" before deleting it
        update_file_with(
            &path,
            &SettingsPatch {
                overrides: OverridesPatch {
                    font_size: Some(Some(20.0)),
                    ..Default::default()
                },
                ..Default::default()
            },
            |_| {},
        )
        .unwrap();
        assert!(stale.is_file());
        assert_eq!(Settings::load(&path).consumed_pending, vec![file_name(&stale)]);
        // the next start skips it (no replay of 18) and removes it
        assert!(!apply_pending(&path).unwrap());
        assert!(!stale.is_file());
        let s = Settings::load(&path);
        assert_eq!(s.overrides.font_size, Some(20.0));
        assert_eq!(s.colors, "palette:nord");
        // the list drops names of files that are gone on the next write
        let s = update_file(
            &path,
            &SettingsPatch {
                appearance: Some(Appearance::Dark),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(s.consumed_pending.is_empty());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("consumed_pending"));
        // a later save also skips a listed file left behind
        let stale = write_pending(
            &path,
            &SettingsPatch {
                appearance: Some(Appearance::Light),
                ..Default::default()
            },
        )
        .unwrap();
        update_file_with(&path, &SettingsPatch::default(), |_| {}).unwrap();
        let s = update_file(&path, &SettingsPatch::default()).unwrap();
        assert_eq!(s.appearance, Appearance::Light, "applied once");
        assert!(!stale.is_file());
    }

    #[test]
    fn forgetting_saved_fields_keeps_unrelated_ones() {
        let mut failed = SettingsPatch {
            colors: Some("palette:nord".into()),
            overrides: OverridesPatch {
                font_size: Some(Some(18.0)),
                line_height: Some(Some(1.5)),
                ..Default::default()
            },
            ..Default::default()
        };
        failed.forget(&SettingsPatch {
            overrides: OverridesPatch {
                font_size: Some(Some(20.0)),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(failed.overrides.font_size, None);
        assert_eq!(failed.overrides.line_height, Some(Some(1.5)));
        assert_eq!(failed.colors.as_deref(), Some("palette:nord"));
    }

    #[test]
    fn patches_merge_and_round_trip_through_json() {
        let mut p = SettingsPatch {
            style: Some("style:classic".into()),
            overrides: OverridesPatch {
                font_size: Some(Some(18.0)),
                font_family: Some(Some("Georgia".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        p.merge(SettingsPatch {
            colors: Some("palette:nord".into()),
            appearance: Some(Appearance::Dark),
            overrides: OverridesPatch {
                font_size: Some(None),
                line_height: Some(Some(1.5)),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(p.style.as_deref(), Some("style:classic"));
        assert_eq!(p.overrides.font_size, Some(None), "later clear wins");
        assert_eq!(p.overrides.font_family, Some(Some("Georgia".into())));
        assert_eq!(SettingsPatch::from_json(&p.to_json()), Some(p));
        assert_eq!(SettingsPatch::from_json("{}"), Some(SettingsPatch::default()));
        assert_eq!(SettingsPatch::from_json("[]"), None);
    }

    #[test]
    fn updates_merge_only_the_changed_fields() {
        // two instances: each saves only what it changed
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let a = SettingsPatch {
            colors: Some("palette:nord".into()),
            ..Default::default()
        };
        let b = SettingsPatch {
            style: Some("style:technical".into()),
            appearance: Some(Appearance::Light),
            ..Default::default()
        };
        let threads: Vec<_> = [a, b]
            .into_iter()
            .map(|p| {
                let path = path.clone();
                std::thread::spawn(move || update_file(&path, &p).unwrap())
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let s = Settings::load(&path);
        assert_eq!(
            (s.style.as_str(), s.colors.as_str(), s.appearance),
            ("style:technical", "palette:nord", Appearance::Light)
        );
    }

    #[test]
    fn version_1_theme_migrates_to_style_and_colors() {
        let s = Settings::from_json(r#"{"version": 1, "theme": "user:solar", "appearance": "light"}"#).unwrap();
        assert_eq!(
            (s.style.as_str(), s.colors.as_str(), s.appearance),
            ("theme:user:solar", "user:solar", Appearance::Light)
        );
        let s = Settings::from_json(r#"{"version": 1, "theme": "builtin:drake"}"#).unwrap();
        assert_eq!(
            (s.style.as_str(), s.colors.as_str()),
            ("theme:builtin:drake", "builtin:drake")
        );
        for native in ["markise", "palette:nord"] {
            let s = Settings::from_json(&format!(r#"{{"version": 1, "theme": "{native}"}}"#)).unwrap();
            assert_eq!((s.style.as_str(), s.colors.as_str()), (DEFAULT_STYLE, native));
        }
        // the old key is not written back; the new ones are
        let text = Settings::from_json(r#"{"version": 1, "theme": "palette:nord"}"#)
            .unwrap()
            .to_json();
        assert!(
            text.contains("\"version\": 2") && text.contains("\"colors\": \"palette:nord\""),
            "{text}"
        );
        assert!(!text.contains("overrides"), "empty overrides omitted: {text}");
    }
}
