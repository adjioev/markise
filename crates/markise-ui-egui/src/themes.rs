//! Theme selection for the app (ADR 0004, ADR 0007): the catalog (Markise, built-in, user themes),
//! the selected style and color family, user typography overrides and the light/dark appearance
//! (persisted in the settings file), background
//! imports with a result cache, "Import theme…", live reload of user themes and the theme's
//! font resolution.
//!
//! All file work (extracting built-ins, discovery, imports, installs, font scans, settings
//! writes) runs on one worker thread; the UI thread only polls a channel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use markise_engine::Wake;
use markise_engine::settings::{
    Appearance, FontOverrides, OverridesPatch, Settings, SettingsPatch, apply_pending, is_future_error, settings_path,
    update_file, write_pending,
};
use markise_engine::watch::{FileWatcher, WatchEvent};
use markise_theme::catalog::{
    Origin, Selection, StyleId, ThemeFamily, ThemeId, VariantSource, builtin_families, markise_family,
    markise_night_family, palette_families, user_families,
};
use markise_theme::discover::{ThemeSource, discover_themes};
use markise_theme::install::{InstallError, InstallLimits, install_theme};
use markise_theme::style::{BUILTIN_STYLES, DEFAULT_STYLE, builtin_style, composed_name, native_colors};
use markise_theme::{
    ColorScheme, ImportOptions, ImportResult, Overrides, Style, Theme, builtin, compose_with, import_theme_file,
};

use crate::fonts::{FontRequest, ResolvedFonts};
use crate::prefs::Swatch;

/// Where the app keeps its configuration.
#[derive(Clone, Debug, Default)]
pub enum ConfigDir {
    /// No settings file, no user themes (tests, measurements). Built-in themes are extracted
    /// to the temp folder.
    #[default]
    None,
    /// The platform config folder (or a test folder): `<dir>/markise/settings.json`,
    /// `<dir>/markise/themes/`, `<dir>/markise/builtin-themes/`.
    Dir(PathBuf),
}

enum Job {
    /// Load the settings and create/canonicalize the user folder (fast), then extract the
    /// built-ins and discover user themes.
    Init,
    Discover,
    /// Import a theme file; the generation travels with the result.
    Import(PathBuf, u64),
    /// Copy a theme into the user folder; `true` = replace an existing theme of that name.
    Install(PathBuf, bool),
    /// Resolve fonts; `true` = also load the fallback fonts (CJK).
    Fonts(u64, FontRequest, bool),
    /// Merge the changed settings into the file (locked read–merge–write); the id comes back
    /// with the result.
    Save(u64, SettingsPatch),
    /// Register the watcher on the user theme folder.
    Watch,
    /// Import a CSS theme only for its preview colours (Preferences' theme cards): the
    /// import is dropped on the worker, only the [`Swatch`] comes back; generation as Import.
    Preview(PathBuf, bool, u64),
}

/// Outcome of "Import theme…".
pub struct InstallDone {
    pub css: PathBuf,
    pub copied: usize,
    pub skipped: Vec<(PathBuf, String)>,
    pub replaced: bool,
    pub import: Arc<ImportResult>,
}

enum InstallFail {
    Collision(Vec<PathBuf>),
    Other(String),
}

enum Done {
    /// Settings file contents and the canonical user theme folder.
    Settings(Settings, Option<PathBuf>),
    /// A watcher registered on the user theme folder (created on the worker).
    Watcher(Option<FileWatcher>),
    Families(Result<Vec<ThemeFamily>, String>),
    Imported {
        path: PathBuf,
        generation: u64,
        result: Result<Arc<ImportResult>, String>,
        /// Files the import depends on, canonical: what it read plus the user layers
        /// (`base.user.css`, `{name}.user.css`) whether they exist or not.
        deps: Vec<PathBuf>,
    },
    Installed(PathBuf, Result<(InstallDone, Vec<ThemeFamily>), InstallFail>),
    Fonts(u64, Arc<ResolvedFonts>),
    /// Result of `Job::Save` by id; an error says whether retrying can help.
    Saved(u64, Result<(), (String, bool)>),
    /// Preview colours of a CSS theme variant (`None`: the import failed).
    Preview(PathBuf, bool, u64, Option<Swatch>),
}

struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Done>,
}

/// Dependencies of an import: the files it read and the user layers of its folder.
fn import_deps(path: &Path, result: Option<&ImportResult>) -> Vec<PathBuf> {
    // canonical where it exists; a missing file by its canonical folder (or as written)
    let canon = |p: &Path| {
        std::fs::canonicalize(p).unwrap_or_else(|_| match (p.parent(), p.file_name()) {
            (Some(d), Some(n)) => std::fs::canonicalize(d).map_or_else(|_| p.to_path_buf(), |d| d.join(n)),
            _ => p.to_path_buf(),
        })
    };
    let mut deps = vec![canon(path)];
    if let Some(r) = result {
        deps.extend(r.report.files.iter().map(|f| canon(f)));
        // imports that failed (e.g. missing) still count: their appearance reloads the theme
        deps.extend(r.report.attempted.iter().map(|f| canon(f)));
    }
    if let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) {
        let dir = canon(dir);
        deps.push(dir.join("base.user.css"));
        deps.push(dir.join(format!("{}.user.css", stem.to_string_lossy())));
    }
    deps.sort();
    deps.dedup();
    deps
}

fn spawn_worker(builtin_root: PathBuf, user_dir: Option<PathBuf>, settings: Option<PathBuf>, wake: Wake) -> Worker {
    let (jtx, jrx) = channel::<Job>();
    let (dtx, drx) = channel::<Done>();
    std::thread::Builder::new()
        .name("markise-themes".into())
        .spawn(move || {
            let mut user_dir = user_dir;
            let discover = |user_dir: &Option<PathBuf>| -> Vec<ThemeFamily> {
                user_dir
                    .as_ref()
                    .map(|d| user_families(&discover_themes(d, ThemeSource::User)))
                    .unwrap_or_default()
            };
            while let Ok(job) = jrx.recv() {
                let done = match job {
                    Job::Init => {
                        // changes an earlier session could not write at shutdown
                        if let Some(p) = &settings
                            && let Err(e) = apply_pending(p)
                        {
                            eprintln!("markise: pending settings not applied: {e}");
                        }
                        let loaded = settings.as_deref().map(Settings::load).unwrap_or_default();
                        if let Some(d) = &user_dir {
                            let _ = std::fs::create_dir_all(d);
                            user_dir = Some(std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()));
                        }
                        let published = crate::wake_track::publishing();
                        if dtx.send(Done::Settings(loaded, user_dir.clone())).is_err() {
                            drop(published);
                            break;
                        }
                        wake();
                        drop(published);
                        let r = builtin::extract(&builtin_root).map_err(|e| format!("built-in themes: {e}"));
                        match r {
                            Ok(_) => Done::Families(Ok(discover(&user_dir))),
                            Err(e) => Done::Families(Err(e)),
                        }
                    }
                    Job::Watch => {
                        // live reload (registering can take a moment: queued after the first
                        // theme import)
                        let watcher = user_dir.as_ref().and_then(|d| {
                            let mut w = FileWatcher::new(wake.clone()).ok()?;
                            w.watch_themes(d);
                            // registered in the background (never blocks this worker); once
                            // in effect, the folder is reported as changed (catch-up)
                            Some(w)
                        });
                        Done::Watcher(watcher)
                    }
                    Job::Discover => Done::Families(Ok(discover(&user_dir))),
                    Job::Import(path, generation) => {
                        let result = import_theme_file(&path, &ImportOptions::default())
                            .map(Arc::new)
                            .map_err(|e| e.to_string());
                        let deps = import_deps(&path, result.as_ref().ok().map(|r| &**r));
                        Done::Imported {
                            path,
                            generation,
                            result,
                            deps,
                        }
                    }
                    Job::Preview(path, dark_variant, generation) => {
                        let swatch = import_theme_file(&path, &ImportOptions::default())
                            .ok()
                            .map(|r| Swatch::of(r.variant(dark_variant)));
                        Done::Preview(path, dark_variant, generation, swatch)
                    }
                    Job::Install(src, replace) => match &user_dir {
                        None => Done::Installed(
                            src,
                            Err(InstallFail::Other("no user theme folder in this session".into())),
                        ),
                        Some(d) => {
                            let r = match install_theme(&src, d, &InstallLimits::default(), replace) {
                                Ok(i) => Ok((
                                    InstallDone {
                                        css: i.css,
                                        copied: i.copied.len(),
                                        skipped: i.skipped,
                                        replaced: i.replaced,
                                        import: Arc::new(i.import),
                                    },
                                    discover(&user_dir),
                                )),
                                Err(InstallError::Collision(p)) => Err(InstallFail::Collision(p)),
                                Err(e) => Err(InstallFail::Other(e.to_string())),
                            };
                            Done::Installed(src, r)
                        }
                    },
                    Job::Fonts(key, req, fallback) => Done::Fonts(key, Arc::new(crate::fonts::resolve(&req, fallback))),
                    Job::Save(id, patch) => Done::Saved(
                        id,
                        match &settings {
                            Some(p) => update_file(p, &patch)
                                .map(|_| ())
                                .map_err(|e| (format!("settings: {e}"), !is_future_error(&e))),
                            None => Ok(()),
                        },
                    ),
                };
                // a `Done` may be applied before `wake` runs: counted until it returned (tests)
                let published = crate::wake_track::publishing();
                if dtx.send(done).is_err() {
                    drop(published);
                    break;
                }
                wake();
                drop(published);
            }
        })
        .expect("spawn theme worker");
    Worker { tx: jtx, rx: drx }
}

/// Where the active style comes from.
#[derive(Clone, Debug, PartialEq)]
enum StyleSrc {
    Builtin(String),
    Variant(VariantSource),
}

/// What the active theme was composed from.
#[derive(Clone, Debug, PartialEq)]
struct Composed {
    style: StyleSrc,
    colors: VariantSource,
    overrides: Overrides,
}

/// Settings form of the overrides and back.
pub fn overrides_from_settings(o: &FontOverrides) -> Overrides {
    Overrides {
        body_font: o.font_family.clone(),
        font_size: o.font_size,
        line_height: o.line_height,
        content_width: o.content_width,
    }
}

pub fn overrides_to_settings(o: &Overrides) -> FontOverrides {
    FontOverrides {
        font_family: o.body_font.clone(),
        font_size: o.font_size,
        line_height: o.line_height,
        content_width: o.content_width,
    }
}

/// Settings are written this long after the last change.
pub const SAVE_DELAY: Duration = Duration::from_millis(300);
/// Shutdown waits this long for queued settings writes.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

impl Drop for ThemeManager {
    fn drop(&mut self) {
        self.flush();
    }
}

struct Cached {
    result: Arc<ImportResult>,
    deps: Vec<PathBuf>,
}

/// Something the UI should show after polling.
pub enum ThemeEvent {
    /// New fonts for the active theme (install with `fonts::definitions_with`).
    Fonts(Arc<ResolvedFonts>),
    Installed(InstallDone),
    /// "Import theme…" found a theme of the same name: ask, then
    /// [`ThemeManager::install_replacing`].
    InstallCollision {
        src: PathBuf,
        existing: Vec<PathBuf>,
    },
    Error(String),
}

pub struct ThemeManager {
    config: ConfigDir,
    /// Canonical user theme folder (known once the worker reported it).
    user_dir: Option<PathBuf>,
    builtin_root: PathBuf,
    settings: Settings,
    settings_loaded: bool,
    /// Choices made before the settings file was read win over it, each on its own.
    chose_style: bool,
    chose_colors: bool,
    chose_appearance: bool,
    /// The Side by Side divider was moved before the settings file arrived.
    chose_split: bool,
    /// The window was zoomed before the settings file arrived.
    chose_zoom: bool,
    /// Diagrams switched (Preferences) before the settings file was read.
    chose_diagrams: bool,
    /// View → Toolbar toggled before the settings file was read.
    chose_toolbar: bool,
    chose_recent: bool,
    chose_word_count: bool,
    chose_count_mode: bool,
    chose_toolbar_position: bool,
    chose_editing: bool,
    chose_source: bool,
    chose_images: bool,
    /// Wikilinks switched (Preferences) before the settings file was read.
    chose_markdown: bool,
    /// Settings changed and not yet handed to the worker (coalesced, debounced).
    dirty: SettingsPatch,
    save_at: Option<Instant>,
    /// Override members changed before the settings file was read (merged into the file's).
    early_overrides: OverridesPatch,
    /// Saves handed to the worker and not answered yet (shutdown waits for them).
    in_flight: Vec<(u64, SettingsPatch)>,
    next_save: u64,
    /// Saves that failed in a way a retry may fix; recorded for the next start at shutdown.
    unsaved: SettingsPatch,
    /// A save failed (e.g. a settings file of a newer version): reported once.
    save_error_shown: bool,
    selection: Selection,
    overrides: Overrides,
    families: Vec<ThemeFamily>,
    worker: Option<Worker>,
    cache: HashMap<PathBuf, Cached>,
    /// Import generation per path: bumped on every invalidation; results of older
    /// generations are dropped.
    gens: HashMap<PathBuf, u64>,
    /// Imports requested and not answered yet.
    requested: Vec<PathBuf>,
    /// Failed imports: error and dependencies (a change to any of them retries).
    failed: HashMap<PathBuf, (String, Vec<PathBuf>)>,
    current: Theme,
    current_src: Option<Composed>,
    /// Bumped whenever `current` changes.
    generation: u64,
    /// Font request key of the fonts installed (or being resolved).
    font_key: Option<u64>,
    system_fonts: bool,
    discover_pending: bool,
    /// The family to select once a pending install's discovery arrives.
    select_after_install: Option<PathBuf>,
    pub last_fonts: Option<Arc<ResolvedFonts>>,
    /// Watches the user theme folder (live reload); registered by the worker.
    watcher: Option<FileWatcher>,
    watch_requested: bool,
    /// Fallback fonts (CJK) were asked for by the document; loaded with the next font job.
    want_fallback: bool,
    /// Preview colours of theme variants (Preferences' theme cards): a few colours per
    /// variant, kept apart from the full imports (`None`: the import failed).
    previews: Vec<(VariantSource, Option<Swatch>)>,
    /// Preview imports handed to the worker and not answered yet.
    previews_pending: Vec<VariantSource>,
    /// Jobs handed to the worker whose answer was not applied yet (tests: "background
    /// settled", [`ThemeManager::is_idle`]).
    jobs_out: std::cell::Cell<usize>,
}

impl ThemeManager {
    /// No file access here: the settings file is read and the user folder created by the
    /// worker ([`ThemeManager::start`]).
    pub fn new(config: ConfigDir, system_fonts: bool) -> Self {
        let builtin_root = match &config {
            ConfigDir::None => std::env::temp_dir().join("markise-builtin-themes"),
            ConfigDir::Dir(d) => d.join("markise").join("builtin-themes"),
        };
        let mut families = vec![markise_family(), markise_night_family()];
        families.extend(builtin_families(
            &builtin_root.join(format!("{:016x}", builtin::content_hash())),
        ));
        families.extend(palette_families());
        ThemeManager {
            config,
            user_dir: None,
            builtin_root,
            settings: Settings::default(),
            settings_loaded: false,
            chose_style: false,
            chose_colors: false,
            chose_appearance: false,
            chose_split: false,
            chose_zoom: false,
            chose_diagrams: false,
            chose_toolbar: false,
            chose_recent: false,
            chose_word_count: false,
            chose_count_mode: false,
            chose_toolbar_position: false,
            chose_editing: false,
            chose_source: false,
            chose_images: false,
            chose_markdown: false,
            dirty: SettingsPatch::default(),
            save_at: None,
            early_overrides: OverridesPatch::default(),
            in_flight: Vec::new(),
            next_save: 0,
            unsaved: SettingsPatch::default(),
            save_error_shown: false,
            selection: Selection::default(),
            overrides: Overrides::default(),
            families,
            worker: None,
            cache: HashMap::new(),
            gens: HashMap::new(),
            requested: Vec::new(),
            failed: HashMap::new(),
            current: Theme::light(),
            current_src: None,
            generation: 0,
            font_key: None,
            system_fonts,
            discover_pending: false,
            select_after_install: None,
            last_fonts: None,
            watcher: None,
            watch_requested: false,
            want_fallback: false,
            previews: Vec::new(),
            previews_pending: Vec::new(),
            jobs_out: std::cell::Cell::new(0),
        }
    }

    /// Starts the worker (settings, built-ins, user themes, then the selected theme).
    pub fn start(&mut self, wake: Wake) {
        if self.worker.is_some() {
            return;
        }
        let (settings, user_dir) = match &self.config {
            ConfigDir::Dir(d) => (Some(settings_path(d)), Some(d.join("markise").join("themes"))),
            ConfigDir::None => (None, None),
        };
        let w = spawn_worker(self.builtin_root.clone(), user_dir, settings, wake);
        if w.tx.send(Job::Init).is_ok() {
            self.jobs_out.set(self.jobs_out.get() + 1);
        }
        self.discover_pending = true;
        self.worker = Some(w);
    }

    fn send(&self, job: Job) {
        if let Some(w) = &self.worker
            && w.tx.send(job).is_ok()
        {
            self.jobs_out.set(self.jobs_out.get() + 1);
        }
    }

    /// Test hook: the worker has no unanswered jobs, no debounced settings write is pending
    /// and the folder watch (once requested) has been handed back. Nothing changes the theme
    /// or wakes the UI from here on until something new is asked.
    #[doc(hidden)]
    pub fn is_idle(&self) -> bool {
        let watch_due = !self.watch_requested && self.settings_loaded && self.has_user_dir() && self.worker.is_some();
        self.jobs_out.get() == 0
            && self.save_at.is_none()
            && !watch_due
            && self.watcher.as_ref().is_none_or(FileWatcher::is_synced)
    }

    /// Test hook: blocks (at most `timeout`) until the theme folder watch has applied every
    /// request, so its catch-up event is queued for the next [`ThemeManager::poll`] (never on
    /// a real UI thread). `None`: no answer in time; `Err` names a watch that is not in effect.
    #[doc(hidden)]
    pub fn sync_watch(&self, timeout: Duration) -> Option<Result<(), String>> {
        self.watcher
            .as_ref()
            .map_or(Some(Ok(())), |w| w.sync_within(Some(timeout)))
    }

    // ---------------------------------------------------------------- accessors

    pub fn families(&self) -> &[ThemeFamily] {
        &self.families
    }

    /// The selected style and color family.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Whether the style and the colors were chosen in this session (they then win over the
    /// settings file when it arrives late).
    pub fn chosen_parts(&self) -> (bool, bool) {
        (self.chose_style, self.chose_colors)
    }

    /// The user's typography overrides.
    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// Styles for the Style menu: the built-in styles, then the styles of the complete (CSS)
    /// themes, with their labels.
    pub fn style_choices(&self) -> Vec<(StyleId, String, bool)> {
        let mut v: Vec<(StyleId, String, bool)> = BUILTIN_STYLES
            .iter()
            .map(|s| (StyleId::Builtin(s.id.into()), s.name.to_string(), false))
            .collect();
        v.extend(
            self.families
                .iter()
                .filter(|f| matches!(f.origin, Origin::Builtin | Origin::User))
                .map(|f| (StyleId::Theme(f.id.clone()), f.name.clone(), true)),
        );
        v
    }

    fn family(&self, id: &ThemeId) -> Option<&ThemeFamily> {
        self.families.iter().find(|f| f.id == *id)
    }

    pub fn appearance(&self) -> Appearance {
        self.settings.appearance
    }

    /// The canonical user theme folder, once the worker has created it.
    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }

    /// The settings file of this session (`None`: settings are not persisted, e.g. tests).
    pub fn settings_file(&self) -> Option<PathBuf> {
        match &self.config {
            ConfigDir::Dir(d) => Some(settings_path(d)),
            ConfigDir::None => None,
        }
    }

    /// The session has a user theme folder (it may not be known yet).
    pub fn has_user_dir(&self) -> bool {
        matches!(self.config, ConfigDir::Dir(_))
    }

    /// The selected color family.
    pub fn selected_family(&self) -> &ThemeFamily {
        self.family(&self.selection.colors).unwrap_or(&self.families[0])
    }

    /// CSS files the active theme is composed from (colors first, then style).
    fn current_css(&self) -> Vec<&Path> {
        let Some(c) = &self.current_src else { return Vec::new() };
        let mut v = Vec::new();
        if let VariantSource::Css { path, .. } = &c.colors {
            v.push(path.as_path());
        }
        if let StyleSrc::Variant(VariantSource::Css { path, .. }) = &c.style
            && !v.contains(&path.as_path())
        {
            v.push(path.as_path());
        }
        v
    }

    /// Import result of the active colors' (else style's) CSS theme, for the theme report.
    pub fn current_import(&self) -> Option<&Arc<ImportResult>> {
        self.current_css()
            .into_iter()
            .find_map(|p| self.cache.get(p).map(|c| &c.result))
    }

    pub fn import_error(&self) -> Option<&str> {
        self.current_css()
            .into_iter()
            .find_map(|p| self.failed.get(p).map(|f| f.0.as_str()))
    }

    /// The theme to render.
    pub fn theme(&self) -> &Theme {
        &self.current
    }

    /// Changes whenever [`ThemeManager::theme`] changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    // ---------------------------------------------------------------- changes

    /// Selects a complete theme: its own style and colors (both count as chosen).
    pub fn select(&mut self, id: ThemeId) {
        self.set_selection_parts(Selection::complete(id), true);
    }

    /// Selects a style, keeping the colors.
    pub fn select_style(&mut self, style: StyleId) {
        let colors = self.selection.colors.clone();
        self.set_selection(Selection { style, colors });
    }

    /// Selects a color family, keeping the style.
    pub fn select_colors(&mut self, colors: ThemeId) {
        let style = self.selection.style.clone();
        self.set_selection(Selection { style, colors });
    }

    /// Selects style and colors. Only the parts that change count as chosen: a choice made
    /// before the settings file was read replaces that part of the file, not the other one.
    /// (Selecting a complete theme sets both parts, so both count.)
    pub fn set_selection(&mut self, sel: Selection) {
        self.set_selection_parts(sel, false);
    }

    fn set_selection_parts(&mut self, sel: Selection, both: bool) {
        let style_changed = both || sel.style != self.selection.style;
        let colors_changed = both || sel.colors != self.selection.colors;
        if style_changed {
            self.chose_style = true;
            self.settings.style = sel.style.to_setting();
            self.dirty.style = Some(self.settings.style.clone());
        }
        if colors_changed {
            self.chose_colors = true;
            self.settings.colors = sel.colors.to_setting();
            self.dirty.colors = Some(self.settings.colors.clone());
        }
        if style_changed || colors_changed {
            self.selection = sel;
            self.schedule_save();
        }
    }

    /// Replaces the typography overrides ("Font & size…").
    pub fn set_overrides(&mut self, o: Overrides) {
        if o != self.overrides {
            self.overrides = o;
            let before = std::mem::replace(&mut self.settings.overrides, overrides_to_settings(&self.overrides));
            let diff = OverridesPatch::diff(&before, &self.settings.overrides);
            if !self.settings_loaded {
                self.early_overrides.merge(diff.clone());
            }
            self.dirty.overrides.merge(diff);
            self.schedule_save();
        }
    }

    /// `keyBinding` overrides from the settings file (spec 05 §4.7; object command label or
    /// id → accelerator, `""` unbinds), in file order.
    pub fn key_bindings(&self) -> Vec<(String, String)> {
        self.settings.key_bindings()
    }

    /// Editing preferences (settings key `editing`; Preferences → Editor).
    pub fn editing(&self) -> markise_engine::settings::EditingSettings {
        self.settings.editing
    }

    /// Changes the editing preferences; persisted like the other settings.
    pub fn set_editing(&mut self, e: markise_engine::settings::EditingSettings) {
        self.chose_editing = true;
        if e != self.settings.editing {
            self.settings.editing = e;
            self.dirty.editing = Some(e);
            self.schedule_save();
        }
    }

    /// Source editor display preferences (settings key `source`; Preferences → Source Mode).
    pub fn source_prefs(&self) -> markise_engine::settings::SourceSettings {
        self.settings.source
    }

    /// Changes the source editor display preferences; persisted like the other settings.
    pub fn set_source_prefs(&mut self, p: markise_engine::settings::SourceSettings) {
        self.chose_source = true;
        if p != self.settings.source {
            self.settings.source = p;
            self.dirty.source = Some(p);
            self.schedule_save();
        }
    }

    /// Pasted and dropped images (settings key `images`; Preferences → Files & Images).
    pub fn image_settings(&self) -> &markise_engine::settings::ImageSettings {
        &self.settings.images
    }

    /// Changes the image settings; persisted like the other settings.
    pub fn set_image_settings(&mut self, s: markise_engine::settings::ImageSettings) {
        self.chose_images = true;
        if s != self.settings.images {
            self.settings.images = s.clone();
            self.dirty.images = Some(s);
            self.schedule_save();
        }
    }

    /// Whether wikilinks are parsed (settings key `markdown.wikilinks`, default on).
    pub fn wikilinks(&self) -> bool {
        self.settings.markdown.wikilinks
    }

    /// Preferences → Markdown → Wikilinks; persisted like the other settings.
    pub fn set_wikilinks(&mut self, on: bool) {
        self.chose_markdown = true;
        if on != self.settings.markdown.wikilinks {
            self.settings.markdown.wikilinks = on;
            self.dirty.markdown = Some(self.settings.markdown);
            self.schedule_save();
        }
    }

    /// Whether CriticMarkup comments and suggestions are parsed (settings key
    /// `markdown.criticMarkup`, default on; ADR 0012).
    pub fn critic_markup(&self) -> bool {
        self.settings.markdown.critic_markup
    }

    /// View → Comments & Suggestions; persisted like the other settings.
    pub fn set_critic_markup(&mut self, on: bool) {
        self.chose_markdown = true;
        if on != self.settings.markdown.critic_markup {
            self.settings.markdown.critic_markup = on;
            self.dirty.markdown = Some(self.settings.markdown);
            self.schedule_save();
        }
    }

    /// How comments are shown (settings key `markdown.commentDisplay`).
    pub fn comment_display(&self) -> markise_engine::settings::CommentDisplay {
        self.settings.markdown.comment_display
    }

    /// Preferences → Markdown → Show comments, and the comments panel's toggle.
    pub fn set_comment_display(&mut self, d: markise_engine::settings::CommentDisplay) {
        self.chose_markdown = true;
        if d != self.settings.markdown.comment_display {
            self.settings.markdown.comment_display = d;
            self.dirty.markdown = Some(self.settings.markdown);
            self.schedule_save();
        }
    }

    /// Whether exports keep review comments (settings key `markdown.exportComments`).
    pub fn export_comments(&self) -> bool {
        self.settings.markdown.export_comments
    }

    /// File → Export → Include Comments; persisted like the other settings.
    pub fn set_export_comments(&mut self, on: bool) {
        self.chose_markdown = true;
        if on != self.settings.markdown.export_comments {
            self.settings.markdown.export_comments = on;
            self.dirty.markdown = Some(self.settings.markdown);
            self.schedule_save();
        }
    }

    /// Whether diagram fences are rendered (settings key `diagrams`, default on).
    pub fn diagrams(&self) -> bool {
        self.settings.diagrams
    }

    /// Preferences → Markdown → Render diagrams; persisted like the other settings (DG-7).
    pub fn set_diagrams(&mut self, on: bool) {
        self.chose_diagrams = true;
        if on != self.settings.diagrams {
            self.settings.diagrams = on;
            self.dirty.diagrams = Some(on);
            self.schedule_save();
        }
    }

    /// Whether the formatting toolbar is shown (settings key `showToolbar`, spec 04 §3.2;
    /// spec 05 default on).
    pub fn show_toolbar(&self) -> bool {
        self.settings.show_toolbar
    }

    /// View → Toolbar; persisted like the other settings.
    pub fn set_show_toolbar(&mut self, on: bool) {
        self.chose_toolbar = true;
        if on != self.settings.show_toolbar {
            self.settings.show_toolbar = on;
            self.dirty.show_toolbar = Some(on);
            self.schedule_save();
        }
    }

    /// Whether the sidebar shows its Recent section (settings key `sidebar.showRecent`,
    /// default on).
    pub fn show_recent(&self) -> bool {
        self.settings.show_recent
    }

    /// View → Show Recent Files; persisted like the other settings.
    pub fn set_show_recent(&mut self, on: bool) {
        self.chose_recent = true;
        if on != self.settings.show_recent {
            self.settings.show_recent = on;
            self.dirty.show_recent = Some(on);
            self.schedule_save();
        }
    }

    /// Whether the status bar shows the word count (settings key `statusBar.showWordCount`,
    /// default on; issue #53).
    pub fn show_word_count(&self) -> bool {
        self.settings.show_word_count
    }

    /// Preferences → General → Show word count; persisted like the other settings.
    pub fn set_show_word_count(&mut self, on: bool) {
        self.chose_word_count = true;
        if on != self.settings.show_word_count {
            self.settings.show_word_count = on;
            self.dirty.show_word_count = Some(on);
            self.schedule_save();
        }
    }

    /// The count the status bar shows (settings key `statusBar.count`, default words).
    pub fn count_mode(&self) -> markise_engine::settings::CountMode {
        self.settings.count_mode
    }

    /// A click on the status bar's count cycles it; persisted like the other settings.
    pub fn set_count_mode(&mut self, mode: markise_engine::settings::CountMode) {
        self.chose_count_mode = true;
        if mode != self.settings.count_mode {
            self.settings.count_mode = mode;
            self.dirty.count_mode = Some(mode);
            self.schedule_save();
        }
    }

    /// Formatting toolbar placement (settings key `toolbarPosition`, default top).
    pub fn toolbar_position(&self) -> markise_engine::settings::ToolbarPosition {
        self.settings.toolbar_position
    }

    /// Preferences → Appearance → Toolbar position; persisted like the other settings.
    pub fn set_toolbar_position(&mut self, p: markise_engine::settings::ToolbarPosition) {
        self.chose_toolbar_position = true;
        if p != self.settings.toolbar_position {
            self.settings.toolbar_position = p;
            self.dirty.toolbar_position = Some(p);
            self.schedule_save();
        }
    }

    pub fn set_appearance(&mut self, a: Appearance) {
        self.chose_appearance = true;
        if a != self.settings.appearance {
            self.settings.appearance = a;
            self.dirty.appearance = Some(a);
            self.schedule_save();
        }
    }

    /// PROTOTYPE: appearance for this run only (not saved; `MARKISE_PROTO_THEME`).
    pub fn set_appearance_transient(&mut self, a: Appearance) {
        self.chose_appearance = true;
        self.settings.appearance = a;
    }

    /// Whole-window zoom factor (View → Zoom; 1.0 until zoomed).
    pub fn zoom(&self) -> f32 {
        self.settings.zoom.unwrap_or(1.0)
    }

    /// Sets the whole-window zoom (clamped to 50–300 %; persisted, written debounced).
    pub fn set_zoom(&mut self, v: f32) {
        if !v.is_finite() {
            return;
        }
        let (lo, hi) = markise_engine::settings::ZOOM_RANGE;
        let v = v.clamp(lo, hi);
        self.chose_zoom = true;
        if self.settings.zoom.unwrap_or(1.0) != v {
            self.settings.zoom = Some(v);
            self.dirty.zoom = Some(v);
            self.schedule_save();
        }
    }

    /// Side by Side divider position (fraction of the width given to the source pane), if set.
    pub fn side_by_side_split(&self) -> Option<f32> {
        self.settings.side_by_side_split
    }

    /// Moves the Side by Side divider (persisted, spec 04 §3.3; written debounced).
    pub fn set_side_by_side_split(&mut self, v: f32) {
        let (lo, hi) = markise_engine::settings::SPLIT_RANGE;
        let v = v.clamp(lo, hi);
        self.chose_split = true;
        if self.settings.side_by_side_split != Some(v) {
            self.settings.side_by_side_split = Some(v);
            self.dirty.side_by_side_split = Some(v);
            self.schedule_save();
        }
    }

    /// Settings are written [`SAVE_DELAY`] after the last change (sliders and typing produce
    /// one write); the theme itself updates at once.
    fn schedule_save(&mut self) {
        self.save_at = Some(Instant::now() + SAVE_DELAY);
    }

    /// When the pending settings write is due (the UI repaints then).
    pub fn save_deadline(&self) -> Option<Instant> {
        self.save_at
    }

    fn save_if_due(&mut self, now: Instant) {
        if self.save_at.is_some_and(|t| now >= t) {
            self.save_at = None;
            self.send_save();
        }
    }

    fn send_save(&mut self) {
        let patch = std::mem::take(&mut self.dirty);
        if patch.is_empty() {
            return;
        }
        if self.worker.is_some() {
            self.next_save += 1;
            self.in_flight.push((self.next_save, patch.clone()));
            self.send(Job::Save(self.next_save, patch));
        } else {
            // no worker (tests, no session): written at shutdown
            self.unsaved.merge(patch);
        }
    }

    /// Orderly shutdown: hands pending settings to the worker and waits until every queued
    /// save is answered (up to [`FLUSH_TIMEOUT`]). Whatever is not written then — the worker is
    /// busy or blocked, a write failed, or there is no worker — is written directly or, if
    /// that fails, recorded in a per-instance `settings.pending.<pid>-<random>.json` and
    /// applied at the next start.
    pub fn flush(&mut self) {
        self.save_at = None;
        self.send_save();
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        while !self.in_flight.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(Ok(d)) = self.worker.as_ref().map(|w| w.rx.recv_timeout(left)) else {
                break; // timeout, or the worker is gone
            };
            self.apply(d, &mut Vec::new());
        }
        let mut rest = std::mem::take(&mut self.unsaved);
        let timed_out = !self.in_flight.is_empty();
        for (_, p) in self.in_flight.drain(..) {
            rest.merge(p);
        }
        if rest.is_empty() {
            return;
        }
        let ConfigDir::Dir(d) = &self.config else { return };
        let path = settings_path(d);
        // a timed-out worker may still hold the lock: do not wait for it, record the patch
        let direct = if timed_out {
            None
        } else {
            Some(update_file(&path, &rest))
        };
        match direct {
            Some(Ok(_)) => {}
            Some(Err(e)) if is_future_error(&e) => {}
            other => {
                let why = match other {
                    Some(Err(e)) => e.to_string(),
                    _ => "settings writes still queued at shutdown".into(),
                };
                match write_pending(&path, &rest) {
                    Ok(_) => eprintln!("markise: settings kept for the next start ({why})"),
                    Err(e) => eprintln!("markise: settings changes lost ({why}; {e})"),
                }
            }
        }
    }

    /// The document needs glyphs the loaded fonts lack: load the fallback fonts (CJK) on the
    /// worker; they arrive as [`ThemeEvent::Fonts`]. Asked once per session.
    pub fn request_fallback(&mut self) {
        self.want_fallback = true;
    }

    /// Fallback fonts were requested.
    pub fn fallback_requested(&self) -> bool {
        self.want_fallback
    }

    /// Copies a theme into the user folder (async); the result arrives as
    /// [`ThemeEvent::Installed`] (the theme is selected) or [`ThemeEvent::InstallCollision`].
    pub fn install(&mut self, src: PathBuf) {
        self.send(Job::Install(src, false));
    }

    /// Retries an install after the user agreed to replace the existing theme.
    pub fn install_replacing(&mut self, src: PathBuf) {
        self.send(Job::Install(src, true));
    }

    fn invalidate(&mut self, key: &Path) {
        *self.gens.entry(key.to_path_buf()).or_default() += 1;
        self.cache.remove(key);
        self.failed.remove(key);
        self.requested.retain(|r| r != key);
    }

    /// A file in the user theme folder changed (watcher, canonical path): drop affected
    /// imports — cached, failed (a repaired theme re-imports) and in flight — and rescan.
    pub fn file_changed(&mut self, path: &Path) {
        // a changed folder (created, replaced, renamed) affects every dependency beneath it
        let hit = |k: &PathBuf, deps: &[PathBuf]| k.starts_with(path) || deps.iter().any(|d| d.starts_with(path));
        let mut stale: Vec<PathBuf> = self
            .cache
            .iter()
            .filter(|(k, c)| hit(k, &c.deps))
            .map(|(k, _)| k.clone())
            .collect();
        stale.extend(self.failed.iter().filter(|(k, f)| hit(k, &f.1)).map(|(k, _)| k.clone()));
        // in-flight imports may have read the old file: any change in the user folder makes
        // the user themes being imported stale
        if let Some(d) = &self.user_dir
            && path.starts_with(d)
        {
            stale.extend(self.requested.iter().filter(|r| r.starts_with(d)).cloned());
        }
        for k in stale {
            self.invalidate(&k);
        }
        // preview colours of themes under the changed path are read again (a pending one
        // is answered for an older generation and dropped)
        let under = |v: &VariantSource| matches!(v, VariantSource::Css { path: p, .. } if p.starts_with(path));
        let mut bumped = Vec::new();
        for v in self.previews.iter().map(|(v, _)| v).chain(&self.previews_pending) {
            if let VariantSource::Css { path: p, .. } = v
                && p.starts_with(path)
            {
                bumped.push(p.clone());
            }
        }
        bumped.sort();
        bumped.dedup();
        for p in bumped {
            *self.gens.entry(p).or_default() += 1;
        }
        self.previews.retain(|(v, _)| !under(v));
        self.previews_pending.retain(|v| !under(v));
        // a theme file added, removed or renamed at the top level changes the catalog (the
        // folder itself: the watch restarted, anything may have changed)
        let top_level = self
            .user_dir
            .as_ref()
            .is_some_and(|d| path.parent() == Some(d.as_path()) || path == d.as_path());
        if top_level && !self.discover_pending {
            self.discover_pending = true;
            self.send(Job::Discover);
        }
    }

    /// Polls worker results; returns events for the UI.
    pub fn poll(&mut self) -> Vec<ThemeEvent> {
        let mut out = Vec::new();
        self.request_watch();
        self.save_if_due(Instant::now());
        let Some(w) = &self.worker else { return out };
        let mut done = Vec::new();
        while let Ok(d) = w.rx.try_recv() {
            done.push(d);
        }
        for d in done {
            self.apply(d, &mut out);
        }
        // live reload of user themes
        let mut changed = Vec::new();
        if let Some(w) = &self.watcher {
            while let Some(e) = w.try_recv() {
                if let WatchEvent::ThemeChanged(p) = e {
                    changed.push(p);
                }
            }
        }
        for p in changed {
            self.file_changed(&p);
        }
        out
    }

    fn apply(&mut self, d: Done, out: &mut Vec<ThemeEvent>) {
        // every job is answered by exactly one `Done`; `Init` also sends `Settings` first
        if !matches!(d, Done::Settings(..)) {
            self.jobs_out.set(self.jobs_out.get().saturating_sub(1));
        }
        match d {
            Done::Watcher(w) => self.watcher = w,
            Done::Settings(loaded, user_dir) => {
                self.user_dir = user_dir;
                let file_sel = Selection::from_settings(&loaded.style, &loaded.colors);
                // each part chosen before the file arrived wins; the others come from the file
                // (the chosen parts are already pending as a patch: only they are written)
                let chosen = self.settings.clone();
                self.settings = loaded;
                if self.chose_style {
                    self.settings.style = chosen.style;
                } else {
                    self.selection.style = file_sel.style;
                }
                if self.chose_colors {
                    self.settings.colors = chosen.colors;
                } else {
                    self.selection.colors = file_sel.colors;
                }
                if self.chose_appearance {
                    self.settings.appearance = chosen.appearance;
                }
                if self.chose_split {
                    self.settings.side_by_side_split = chosen.side_by_side_split;
                }
                if self.chose_zoom {
                    self.settings.zoom = chosen.zoom;
                }
                if self.chose_diagrams {
                    self.settings.diagrams = chosen.diagrams;
                }
                if self.chose_toolbar {
                    self.settings.show_toolbar = chosen.show_toolbar;
                }
                if self.chose_recent {
                    self.settings.show_recent = chosen.show_recent;
                }
                if self.chose_word_count {
                    self.settings.show_word_count = chosen.show_word_count;
                }
                if self.chose_count_mode {
                    self.settings.count_mode = chosen.count_mode;
                }
                if self.chose_toolbar_position {
                    self.settings.toolbar_position = chosen.toolbar_position;
                }
                if self.chose_editing {
                    self.settings.editing = chosen.editing;
                }
                if self.chose_source {
                    self.settings.source = chosen.source;
                }
                if self.chose_images {
                    self.settings.images = chosen.images.clone();
                }
                if self.chose_markdown {
                    self.settings.markdown = chosen.markdown;
                }
                // overrides member by member: the ones changed early win
                self.early_overrides.apply(&mut self.settings.overrides);
                self.overrides = overrides_from_settings(&self.settings.overrides);
                self.settings_loaded = true;
            }
            Done::Families(r) => {
                self.discover_pending = false;
                match r {
                    Ok(user) => self.set_user_families(user),
                    Err(e) => out.push(ThemeEvent::Error(e)),
                }
            }
            Done::Imported {
                path,
                generation,
                result,
                deps,
            } => {
                if self.gens.get(&path).copied().unwrap_or(0) != generation {
                    return; // obsolete: the file changed while it was being imported
                }
                self.requested.retain(|p| *p != path);
                match result {
                    Ok(result) => {
                        self.failed.remove(&path);
                        self.cache.insert(path, Cached { result, deps });
                    }
                    Err(e) => {
                        self.failed.insert(path, (e, deps));
                    }
                }
            }
            Done::Installed(src, r) => match r {
                Ok((done, user)) => {
                    let css = done.css.clone();
                    // a re-import of an installed file must not show a stale result
                    self.invalidate(&css);
                    self.set_user_families(user);
                    match self.family_of(&css) {
                        Some(id) => self.select(id),
                        None => self.select_after_install = Some(css),
                    }
                    out.push(ThemeEvent::Installed(done));
                }
                Err(InstallFail::Collision(existing)) => out.push(ThemeEvent::InstallCollision { src, existing }),
                Err(InstallFail::Other(e)) => out.push(ThemeEvent::Error(format!("Theme not imported: {e}"))),
            },
            Done::Fonts(key, fonts) => {
                if self.font_key == Some(key) {
                    self.last_fonts = Some(fonts.clone());
                    out.push(ThemeEvent::Fonts(fonts));
                }
            }
            Done::Preview(path, dark_variant, generation, swatch) => {
                if self.gens.get(&path).copied().unwrap_or(0) != generation {
                    return; // the file changed meanwhile
                }
                let src = VariantSource::Css { path, dark_variant };
                self.previews_pending.retain(|v| *v != src);
                self.previews.retain(|(v, _)| *v != src);
                self.previews.push((src, swatch));
            }
            Done::Saved(id, r) => {
                let patch = self
                    .in_flight
                    .iter()
                    .position(|(i, _)| *i == id)
                    .map(|i| self.in_flight.remove(i).1);
                // a successful save supersedes the same fields of older failed ones
                if r.is_ok()
                    && let Some(p) = &patch
                {
                    self.unsaved.forget(p);
                }
                if let Err((e, retry)) = r {
                    if retry && let Some(p) = patch {
                        self.unsaved.merge(p);
                    }
                    if !self.save_error_shown {
                        self.save_error_shown = true;
                        out.push(ThemeEvent::Error(e));
                    }
                }
            }
        }
    }

    fn set_user_families(&mut self, user: Vec<ThemeFamily>) {
        self.families.retain(|f| f.origin != Origin::User);
        self.families.extend(user);
        if let Some(css) = self.select_after_install.take()
            && let Some(id) = self.family_of(&css)
        {
            self.select(id);
        }
    }

    /// The family that contains the CSS file `css` (paths from the worker are canonical).
    fn family_of(&self, css: &Path) -> Option<ThemeId> {
        self.families
            .iter()
            .find(|f| f.css_paths().contains(&css))
            .map(|f| f.id.clone())
    }

    /// The sources the selection asks for in this appearance.
    fn wanted(&self, dark: bool) -> (StyleSrc, VariantSource) {
        let colors = self.selected_family().pick(dark).clone();
        let default = || StyleSrc::Builtin(DEFAULT_STYLE.into());
        let style = match &self.selection.style {
            StyleId::Builtin(id) if BUILTIN_STYLES.iter().any(|s| s.id == id) => StyleSrc::Builtin(id.clone()),
            StyleId::Builtin(_) => default(),
            StyleId::Theme(t) => match self.family(t) {
                Some(f) => StyleSrc::Variant(f.pick(dark).clone()),
                None => default(),
            },
        };
        (style, colors)
    }

    /// The theme of a variant: native at once, CSS from the cache (requesting the import when
    /// missing). `None` while an import is pending; Markise when it failed.
    fn variant_theme(&mut self, src: &VariantSource, dark: bool) -> Option<Theme> {
        let fallback = || if dark { Theme::dark() } else { Theme::light() };
        match src {
            VariantSource::Native { id } => Some(Theme::native(id).unwrap_or_else(fallback)),
            VariantSource::Css { path, dark_variant } => match self.cache.get(path) {
                Some(c) => Some(c.result.variant(*dark_variant).clone()),
                None => {
                    if !self.requested.contains(path) && !self.failed.contains_key(path) && self.worker.is_some() {
                        self.requested.push(path.clone());
                        let generation = self.gens.get(path).copied().unwrap_or(0);
                        self.send(Job::Import(path.clone(), generation));
                    }
                    // a failed import shows Markise in the wanted appearance; a pending one
                    // keeps the previous theme until it arrives
                    self.failed.contains_key(path).then(fallback)
                }
            },
        }
    }

    fn resolve_style(&mut self, src: &StyleSrc, dark: bool) -> Option<Style> {
        match src {
            StyleSrc::Builtin(id) => builtin_style(id),
            StyleSrc::Variant(v) => self.variant_theme(v, dark).map(|t| t.style()),
        }
    }

    fn resolve_colors(&mut self, src: &VariantSource, dark: bool) -> Option<ColorScheme> {
        match src {
            VariantSource::Native { id } => {
                Some(native_colors(id).unwrap_or_else(|| if dark { Theme::dark() } else { Theme::light() }.colors()))
            }
            css => self.variant_theme(css, dark).map(|t| t.colors()),
        }
    }

    /// Number of full theme imports held (tests: previews must not add any).
    pub fn cached_imports(&self) -> usize {
        self.cache.len()
    }

    /// Preview colours of theme family `id` in the wanted appearance (Preferences' theme
    /// cards). Native themes and CSS themes already imported for use answer at once; other CSS
    /// themes are imported on the worker for their colours only (the full import is not kept),
    /// so `None` until that arrives, or when it failed.
    pub fn preview(&mut self, id: &ThemeId, dark: bool) -> Option<Swatch> {
        let src = self.family(id)?.pick(dark).clone();
        if let Some((_, s)) = self.previews.iter().find(|(v, _)| *v == src) {
            return *s;
        }
        let swatch = match &src {
            VariantSource::Native { id } => Some(Swatch::of(
                &Theme::native(id).unwrap_or_else(|| if dark { Theme::dark() } else { Theme::light() }),
            )),
            VariantSource::Css { path, dark_variant } => match self.cache.get(path) {
                Some(c) => Some(Swatch::of(c.result.variant(*dark_variant))),
                None => {
                    if !self.previews_pending.contains(&src) && self.worker.is_some() {
                        self.previews_pending.push(src.clone());
                        let generation = self.gens.get(path).copied().unwrap_or(0);
                        self.send(Job::Preview(path.clone(), *dark_variant, generation));
                    }
                    return None;
                }
            },
        };
        self.previews.push((src, swatch));
        swatch
    }

    /// Menu label of the selected style.
    fn style_label(&self) -> String {
        match &self.selection.style {
            StyleId::Builtin(id) => BUILTIN_STYLES
                .iter()
                .find(|s| s.id == id)
                .unwrap_or(&BUILTIN_STYLES[0])
                .name
                .to_string(),
            StyleId::Theme(t) => self
                .family(t)
                .map_or_else(|| BUILTIN_STYLES[0].name.to_string(), |f| f.name.clone()),
        }
    }

    /// Recomputes the active theme (selected style + colors + overrides) for the wanted
    /// appearance; requests imports and fonts as needed. Returns `true` when the active theme
    /// changed.
    pub fn update(&mut self, dark: bool) -> bool {
        let (style_src, colors_src) = self.wanted(dark);
        // request both imports before waiting on either
        let style = self.resolve_style(&style_src, dark);
        let colors = self.resolve_colors(&colors_src, dark);
        let theme = match (style, colors) {
            (Some(style), Some(colors)) => {
                let mut t = compose_with(&style, &colors, &self.overrides);
                if self.selection.complete_theme().is_none() {
                    let default = matches!(&style_src, StyleSrc::Builtin(id) if id == DEFAULT_STYLE);
                    t.name = composed_name(&self.style_label(), &colors.name, default);
                }
                Some(t)
            }
            _ => None,
        };
        let key = Composed {
            style: style_src,
            colors: colors_src,
            overrides: self.overrides.clone(),
        };
        let mut changed = false;
        if let Some(t) = theme
            && (self.current_src.as_ref() != Some(&key) || t != self.current)
        {
            self.current = t;
            self.current_src = Some(key);
            self.generation += 1;
            changed = true;
        }
        // fonts of the active theme
        if self.system_fonts {
            let req = FontRequest::of(&self.current);
            let key = req.key() ^ u64::from(self.want_fallback);
            if self.font_key != Some(key) && self.worker.is_some() {
                self.font_key = Some(key);
                self.send(Job::Fonts(key, req, self.want_fallback));
            }
        }
        changed
    }

    /// Blocks up to `timeout` until the settings are read and the selected theme is imported
    /// (first frame only), so the window does not flash the default theme. `dark` is asked
    /// again after the settings (appearance) arrive.
    pub fn wait_ready(&mut self, dark: &dyn Fn(Appearance) -> bool, timeout: Duration) -> Vec<ThemeEvent> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let d = dark(self.appearance());
            if self.settings_loaded || self.worker.is_none() {
                self.update(d);
            }
            let sel = &self.selection;
            let known = self.family(&sel.colors).is_some()
                && match &sel.style {
                    StyleId::Theme(t) => self.family(t).is_some(),
                    StyleId::Builtin(_) => true,
                };
            let ready = (self.settings_loaded || self.worker.is_none())
                && (known || !self.discover_pending)
                // system fonts come from the worker too: wait for them (no font flash)
                && (!self.system_fonts || self.last_fonts.is_some() || self.worker.is_none())
                && match self.current_src.as_ref() {
                    Some(c) => (c.style.clone(), c.colors.clone()) == self.wanted(d),
                    None => false,
                };
            if ready {
                break;
            }
            let now = Instant::now();
            let Some(w) = &self.worker else { break };
            if now >= deadline {
                break;
            }
            match w.rx.recv_timeout(deadline - now) {
                Ok(done) => self.apply(done, &mut out),
                Err(_) => break,
            }
        }
        self.update(dark(self.appearance()));
        self.request_watch();
        out
    }

    /// Asks the worker to watch the user theme folder once it is known (after the first theme
    /// import was requested, so that import is not delayed).
    fn request_watch(&mut self) {
        if !self.watch_requested && self.settings_loaded && self.has_user_dir() && self.worker.is_some() {
            self.watch_requested = true;
            self.send(Job::Watch);
        }
    }
}

/// Opens a folder in the platform file manager (not waited for).
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(dir).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager_with_user_dir(dir: &Path) -> ThemeManager {
        let mut m = ThemeManager::new(ConfigDir::None, false);
        m.user_dir = Some(dir.to_path_buf());
        m
    }

    fn result_for(path: &Path) -> Arc<ImportResult> {
        Arc::new(import_theme_file(path, &ImportOptions::default()).unwrap())
    }

    #[test]
    fn a_result_from_before_a_change_is_dropped_and_requested_again() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(tmp.path()).unwrap();
        let css = dir.join("t.css");
        std::fs::write(&css, "body { background: #111 }").unwrap();
        let mut m = manager_with_user_dir(&dir);
        m.requested.push(css.clone());
        let stale = result_for(&css);
        // the file changes while the worker is importing it
        std::fs::write(&css, "body { background: #eee }").unwrap();
        m.file_changed(&css);
        let mut out = Vec::new();
        m.apply(
            Done::Imported {
                path: css.clone(),
                generation: 0,
                result: Ok(stale),
                deps: import_deps(&css, None),
            },
            &mut out,
        );
        assert!(!m.cache.contains_key(&css), "obsolete result not installed");
        assert!(!m.requested.contains(&css), "the next update requests it again");
        assert_eq!(m.gens.get(&css), Some(&1));
    }

    #[test]
    fn a_new_user_layer_invalidates_the_cached_import() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(tmp.path()).unwrap();
        let css = dir.join("t.css");
        std::fs::write(&css, "body { background: #111 }").unwrap();
        let mut m = manager_with_user_dir(&dir);
        let r = result_for(&css);
        let deps = import_deps(&css, Some(&r));
        assert!(deps.contains(&dir.join("base.user.css")) && deps.contains(&dir.join("t.user.css")));
        m.cache.insert(css.clone(), Cached { result: r, deps });
        // base.user.css did not exist at import time; now it appears
        std::fs::write(dir.join("base.user.css"), "p { color: red }").unwrap();
        m.file_changed(&dir.join("base.user.css"));
        assert!(!m.cache.contains_key(&css));
    }

    #[test]
    fn a_missing_import_that_appears_and_a_replaced_folder_invalidate_the_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(tmp.path()).unwrap();
        let css = dir.join("t.css");
        std::fs::write(&css, "@import 'sub/colors.css'; body { background: #111 }").unwrap();
        let mut m = manager_with_user_dir(&dir);
        let r = result_for(&css);
        let deps = import_deps(&css, Some(&r));
        assert!(deps.contains(&dir.join("sub").join("colors.css")), "{deps:?}");
        m.cache.insert(
            css.clone(),
            Cached {
                result: r,
                deps: deps.clone(),
            },
        );
        // the folder appears (one event for the folder) with the missing stylesheet in it
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("colors.css"), "body { background: #222 }").unwrap();
        m.file_changed(&dir.join("sub"));
        assert!(
            !m.cache.contains_key(&css),
            "a changed folder invalidates dependencies beneath it"
        );
        // and the exact file event works too
        m.cache.insert(
            css.clone(),
            Cached {
                result: result_for(&css),
                deps,
            },
        );
        m.file_changed(&dir.join("sub").join("colors.css"));
        assert!(!m.cache.contains_key(&css));
    }

    #[test]
    fn a_repaired_theme_is_retried_after_a_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(tmp.path()).unwrap();
        let css = dir.join("t.css");
        let mut m = manager_with_user_dir(&dir);
        let mut out = Vec::new();
        m.apply(
            Done::Imported {
                path: css.clone(),
                generation: 0,
                result: Err("too large".into()),
                deps: import_deps(&css, None),
            },
            &mut out,
        );
        assert!(m.failed.contains_key(&css));
        std::fs::write(&css, "body {}").unwrap();
        m.file_changed(&css);
        assert!(
            !m.failed.contains_key(&css),
            "failure cleared: the next update re-imports"
        );
    }

    #[test]
    fn changing_style_colors_or_overrides_recomposes_the_theme() {
        let mut m = ThemeManager::new(ConfigDir::None, false);
        m.update(false);
        assert_eq!(m.theme().name, "Markise Light");
        let fp = |m: &ThemeManager| m.theme().fingerprint();
        let (g0, f0) = (m.generation(), fp(&m));
        m.select_style(StyleId::Builtin("technical".into()));
        assert!(m.update(false));
        assert!(m.generation() > g0 && fp(&m) != f0, "style change invalidates");
        assert_eq!(m.theme().font_size, markise_theme::style::technical().font_size);
        assert_eq!(m.theme().name, "Markise Light · Technical");
        let f1 = fp(&m);
        m.select_colors(ThemeId::Palette("nord".into()));
        assert!(m.update(true));
        assert_ne!(fp(&m), f1, "colors change invalidates");
        assert_eq!(m.theme().name, "Nord · Technical");
        assert_eq!(
            m.theme().font_size,
            markise_theme::style::technical().font_size,
            "style kept"
        );
        let f2 = fp(&m);
        m.set_overrides(Overrides {
            content_width: Some(1000.0),
            ..Default::default()
        });
        assert!(m.update(true));
        assert_ne!(fp(&m), f2, "override change invalidates");
        assert_eq!(m.theme().column_max_width, 1000.0);
        assert!(!m.update(true), "nothing changed");
        // a complete theme again: its own name, the default style
        m.select(ThemeId::Palette("nord".into()));
        assert!(m.update(true));
        assert_eq!(m.theme().name, "Nord");
        assert_eq!(m.theme().font_size, Theme::light().font_size);
    }

    #[test]
    fn one_part_chosen_before_a_late_settings_load_keeps_the_other_part_from_the_file() {
        let file = Settings {
            style: "style:compact".into(),
            colors: "palette:nord".into(),
            ..Default::default()
        };
        // only the style is changed before the file arrives
        let mut m = ThemeManager::new(ConfigDir::None, false);
        m.select_style(StyleId::Builtin("classic".into()));
        m.apply(Done::Settings(file.clone(), None), &mut Vec::new());
        assert_eq!(
            m.selection(),
            &Selection {
                style: StyleId::Builtin("classic".into()),
                colors: ThemeId::Palette("nord".into()),
            }
        );
        assert_eq!(
            m.dirty,
            SettingsPatch {
                style: Some("style:classic".into()),
                ..Default::default()
            },
            "only the chosen part is written"
        );
        // only the colors
        let mut m = ThemeManager::new(ConfigDir::None, false);
        m.select_colors(ThemeId::Palette("dracula".into()));
        m.apply(Done::Settings(file.clone(), None), &mut Vec::new());
        assert_eq!(m.selection().style, StyleId::Builtin("compact".into()));
        assert_eq!(m.selection().colors, ThemeId::Palette("dracula".into()));
        // a complete theme sets both, even the part that equals the current one
        let mut m = ThemeManager::new(ConfigDir::None, false);
        m.select(ThemeId::Palette("rose-pine".into()));
        m.apply(Done::Settings(file, None), &mut Vec::new());
        assert_eq!(
            m.selection(),
            &Selection::complete(ThemeId::Palette("rose-pine".into()))
        );
        assert_eq!(m.dirty.style.as_deref(), Some("style:markise"));
    }

    #[test]
    fn settings_writes_are_coalesced_and_debounced() {
        let mut m = ThemeManager::new(ConfigDir::None, false);
        for lh in [1.5, 1.6, 1.7, 1.8] {
            m.set_overrides(Overrides {
                line_height: Some(lh),
                ..Default::default()
            });
        }
        m.set_appearance(Appearance::Dark);
        assert!(m.update(true), "the preview applies at once");
        assert_eq!(m.theme().line_height, 1.8);
        let due = m.save_deadline().expect("a write is pending");
        // nothing is written before the delay; one coalesced patch afterwards
        m.save_if_due(due - Duration::from_millis(1));
        assert!(m.save_deadline().is_some() && !m.dirty.is_empty());
        let patch = m.dirty.clone();
        assert_eq!(patch.overrides.line_height, Some(Some(1.8)));
        assert_eq!(patch.overrides.font_size, None, "untouched member not patched");
        assert_eq!(patch.appearance, Some(Appearance::Dark));
        assert_eq!((patch.style.clone(), patch.colors.clone()), (None, None));
        m.save_if_due(due);
        assert!(m.save_deadline().is_none() && m.dirty.is_empty());
        assert_eq!(
            m.unsaved.overrides.line_height,
            Some(Some(1.8)),
            "kept for shutdown (no worker)"
        );
    }

    #[test]
    fn shutdown_waits_for_queued_saves_and_records_what_it_cannot_write() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().to_path_buf();
        let path = settings_path(&cfg);
        // a save already handed to a (busy) worker: shutdown waits for its answer
        let (jtx, jrx) = channel::<Job>();
        let (dtx, drx) = channel::<Done>();
        let worker = std::thread::spawn({
            let path = path.clone();
            move || {
                while let Ok(job) = jrx.recv() {
                    if let Job::Save(id, p) = job {
                        std::thread::sleep(Duration::from_millis(200)); // busy
                        let r = update_file(&path, &p).map(|_| ()).map_err(|e| (e.to_string(), true));
                        let _ = dtx.send(Done::Saved(id, r));
                    }
                }
            }
        });
        let mut m = ThemeManager::new(ConfigDir::Dir(cfg.clone()), false);
        m.worker = Some(Worker { tx: jtx, rx: drx });
        m.select_colors(ThemeId::Palette("nord".into()));
        m.save_if_due(Instant::now() + SAVE_DELAY); // queued, dirty now empty
        assert!(m.dirty.is_empty() && m.in_flight.len() == 1);
        m.set_appearance(Appearance::Dark); // and a fresh change
        m.flush();
        assert!(m.in_flight.is_empty());
        let s = Settings::load(&path);
        assert_eq!((s.colors.as_str(), s.appearance), ("palette:nord", Appearance::Dark));
        drop(m);
        worker.join().unwrap();

        // a worker that never answers: the patch goes to the pending file
        let (jtx, _jrx) = channel::<Job>();
        let (_dtx, drx) = channel::<Done>();
        let mut m = ThemeManager::new(ConfigDir::Dir(cfg.clone()), false);
        m.worker = Some(Worker { tx: jtx, rx: drx });
        m.select_style(StyleId::Builtin("compact".into()));
        let t = Instant::now();
        m.flush();
        assert!(t.elapsed() >= FLUSH_TIMEOUT);
        assert_eq!(Settings::load(&path).style, "style:markise", "not written directly");
        assert_eq!(markise_engine::settings::pending_files(&path).len(), 1);
        assert!(apply_pending(&path).unwrap());
        assert_eq!(
            Settings::load(&path).style,
            "style:compact",
            "applied at the next start"
        );
    }

    #[test]
    fn a_successful_save_supersedes_an_older_failed_one() {
        let mut m = ThemeManager::new(ConfigDir::None, false);
        let size = |v: f32| SettingsPatch {
            overrides: OverridesPatch {
                font_size: Some(Some(v)),
                ..Default::default()
            },
            colors: (v == 18.0).then(|| "palette:nord".to_string()),
            ..Default::default()
        };
        m.in_flight = vec![(1, size(18.0)), (2, size(20.0))];
        let mut out = Vec::new();
        m.apply(Done::Saved(1, Err(("disk full".into(), true))), &mut out);
        assert_eq!(m.unsaved.overrides.font_size, Some(Some(18.0)));
        m.apply(Done::Saved(2, Ok(())), &mut out);
        assert_eq!(m.unsaved.overrides.font_size, None, "18 must not be re-applied over 20");
        assert_eq!(
            m.unsaved.colors.as_deref(),
            Some("palette:nord"),
            "unrelated field kept"
        );
        assert!(m.in_flight.is_empty());
    }

    #[test]
    fn flush_writes_pending_changes_without_a_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = ThemeManager::new(ConfigDir::Dir(tmp.path().to_path_buf()), false);
        m.select_colors(ThemeId::Palette("nord".into()));
        drop(m); // orderly shutdown flushes
        let s = Settings::load(&settings_path(tmp.path()));
        assert_eq!(s.colors, "palette:nord");
    }

    #[test]
    fn choices_made_before_the_settings_arrive_win() {
        let mut m = ThemeManager::new(ConfigDir::None, false);
        m.select(ThemeId::Palette("nord".into()));
        let mut out = Vec::new();
        let loaded = Settings {
            style: "theme:builtin:drake".into(),
            colors: "builtin:drake".into(),
            appearance: Appearance::Dark,
            overrides: FontOverrides {
                line_height: Some(2.0),
                ..Default::default()
            },
            ..Default::default()
        };
        m.apply(Done::Settings(loaded, None), &mut out);
        assert_eq!(m.selection(), &Selection::complete(ThemeId::Palette("nord".into())));
        assert_eq!(
            m.overrides().line_height,
            Some(2.0),
            "overrides from the file (not chosen)"
        );
        // an override member changed before the file arrives merges with the file's members
        let mut m2 = ThemeManager::new(ConfigDir::None, false);
        m2.set_overrides(Overrides {
            font_size: Some(18.0),
            ..Default::default()
        });
        let loaded = Settings {
            overrides: FontOverrides {
                line_height: Some(2.0),
                ..Default::default()
            },
            ..Default::default()
        };
        m2.apply(Done::Settings(loaded, None), &mut Vec::new());
        assert_eq!(
            (m2.overrides().font_size, m2.overrides().line_height),
            (Some(18.0), Some(2.0))
        );
        assert_eq!(
            m.appearance(),
            Appearance::Dark,
            "appearance from the file (not chosen)"
        );
    }
}
