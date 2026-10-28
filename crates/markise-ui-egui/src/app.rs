//! The application shell: menu bar, sidebar, viewer / source editor, file I/O wiring.
//!
//! `MarkiseApp::ui` is toolkit-glue free (plain `egui::Context`), so the desktop app (eframe)
//! and headless tests / measurements drive the same code.
//!
//! Document state machine (see `docs/adr/0002-ui-layout.md`, "Document lifecycle"):
//! - **Text is the source of truth.** Once source mode was entered, the editor buffer is the
//!   authoritative text of the document and survives mode switches; every change bumps
//!   `text_rev`. The view model is *derived*: background rebuilds bring `model_rev` up to
//!   `text_rev`; a failed rebuild only leaves the preview behind — the text is never lost.
//! - **Saving writes a text snapshot** `(text, rev)` (applied to the document as minimal
//!   patches and serialized in the worker). Success sets `saved_rev = rev` and the new
//!   [`DiskVersion`]; the document is dirty while `text_rev != saved_rev`.
//! - **Continuations carry their preconditions.** Save-then-Close / Install / Open run only if
//!   the save covered the current text (`text_rev == saved rev`); otherwise the question is
//!   asked again. An overwrite authorization (expected disk version) travels with its save
//!   request. Discard performs the action without touching the document: if opening the
//!   other file fails, the edited document stays as it was (dirty).
//! - Every load and save records the file's disk version; saves refuse to overwrite external
//!   changes (see `markise_engine::save`). Watcher events always trigger a reload check; a
//!   reload equal to the recorded version (our own save) is ignored; events during a load or
//!   save are queued. Loads run on one cancellable loader worker (latest request wins).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Context, RichText, ThemePreference};
use markise_engine::Wake;
use markise_engine::editor::SaveSnapshot;
use markise_engine::jobs::{Hooks, Rebuilt, SaveText, Saved, spawn_rebuild, spawn_save};
use markise_engine::load::{Loaded, Loader, StageHook};
use markise_engine::save::{DiskVersion, Expect, SaveError};
use markise_engine::settings::Appearance;
use markise_engine::tree::{MARKDOWN_EXTENSIONS, TEXT_EXTENSIONS, is_markdown};
use markise_engine::watch::{FileWatcher, WatchEvent};
use markise_layout::{DocModel, block::color};
use markise_md::{Document, MarkdownFormat, ParseOptions};
use markise_theme::Overrides;
use markise_theme::Theme;
use markise_theme::catalog::{StyleId, ThemeId};

use crate::images::TextureStore;
use crate::side_by_side::{DEFAULT_SPLIT, SideBySide};
use crate::sidebar::{FileTree, Tab, outline};
use crate::source::SourceState;
use crate::themes::{ConfigDir, InstallDone, ThemeEvent, ThemeManager};
use crate::viewer::DocViewer;

mod menus;
pub use menus::MenuHost;

/// The three mutually exclusive view modes (spec 04 §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Rendered,
    Source,
    SideBySide,
}

/// Light/dark choice; `System` follows the OS (default). Picks the selected theme's light or
/// dark variant (ADR 0004).
pub type ThemeChoice = Appearance;

/// How long the first frame may wait for the selected theme's import (no flash of the
/// default theme at startup; a typical import takes 1–5 ms).
const FIRST_THEME_WAIT: Duration = Duration::from_millis(300);

/// Startup options.
#[derive(Clone)]
pub struct AppOptions {
    /// Paths from the command line (`markise <file|folder>…`).
    pub open: Vec<PathBuf>,
    /// Print the timing log (`MARKISE_TIMING=1`).
    pub timing: bool,
    /// With `timing`: quit after the first document frame (or the first frame without a
    /// document) plus this many idle seconds, logging repaint counts (`MARKISE_TIMING_EXIT`).
    pub exit_after_idle: Option<f32>,
    /// Process start, for the startup measurement.
    pub start: Instant,
    /// Use system fonts (off in tests for determinism).
    pub system_fonts: bool,
    /// Worker hooks (tests inject delays / failures deterministically).
    pub hooks: Hooks,
    /// Loader stage hook (tests).
    pub load_hook: Option<StageHook>,
    /// Settings file, user themes (`ConfigDir::None` in tests: nothing is read or written in
    /// the user's config folder).
    pub config: ConfigDir,
    /// Markdown parse options for opened documents.
    pub parse: ParseOptions,
    /// Syntax-highlight fenced code (View menu "Highlight Code"; session setting for now).
    pub highlight_code: bool,
    /// Text backend for document text (`MARKISE_TEXT=parley` in the app; ADR 0005 T2 developer
    /// switch, default epaint).
    pub text_backend: crate::text_paint::TextBackend,
    /// Short git commit of the build (About window), if known.
    pub commit: Option<String>,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            open: Vec::new(),
            timing: false,
            exit_after_idle: None,
            start: Instant::now(),
            system_fonts: false,
            hooks: Hooks::default(),
            load_hook: None,
            config: ConfigDir::None,
            parse: ParseOptions::default(),
            highlight_code: true,
            text_backend: crate::text_paint::TextBackend::Epaint,
            commit: None,
        }
    }
}

/// What a link click should do (only http/https/mailto open without asking).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    /// `http`, `https`, `mailto`: open in the browser / mail client.
    Open(String),
    /// Any other scheme (`file:`, `vscode:`, …): ask before handing it to the OS.
    Confirm(String),
    /// `#anchor` inside the document.
    Anchor(String),
    /// A Markdown file next to the document: open it in Markise.
    LocalDoc(PathBuf),
    /// Another local path: not opened (reported).
    LocalOther(PathBuf),
}

fn scheme(target: &str) -> Option<&str> {
    let (s, _) = target.split_once(':')?;
    // a single letter is a Windows drive (`C:\…`), not a scheme
    (s.len() >= 2
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.')))
    .then_some(s)
}

pub fn classify_link(target: &str, base_dir: Option<&Path>) -> LinkAction {
    let t = target.trim();
    if t.starts_with('#') {
        return LinkAction::Anchor(t.to_string());
    }
    if let Some(s) = scheme(t) {
        return match s.to_ascii_lowercase().as_str() {
            "http" | "https" | "mailto" => LinkAction::Open(t.to_string()),
            _ => LinkAction::Confirm(t.to_string()),
        };
    }
    let file = t.split('#').next().unwrap_or("");
    let p = base_dir.map_or_else(|| PathBuf::from(file), |d| d.join(file));
    if is_markdown(&p) {
        LinkAction::LocalDoc(p)
    } else {
        LinkAction::LocalOther(p)
    }
}

struct DocState {
    /// Unique per opened document (stale job results are matched against it).
    id: u64,
    /// The `keyBinding` settings value last applied to this document's keymap.
    key_bindings_applied: Option<Vec<(String, String)>>,
    path: PathBuf,
    viewer: DocViewer,
    lossy: bool,
    deleted: bool,
    /// The disk content the saved revision corresponds to (loaded or last saved).
    disk: Option<DiskVersion>,
    /// Editor buffer; once created it is the authoritative text until the document is
    /// replaced or reloaded (it survives switching back to the preview).
    buffer: Option<SourceState>,
    /// Revision of the authoritative text.
    text_rev: u64,
    /// Revision the view model reflects (`<= text_rev`).
    model_rev: u64,
    /// Revision known to be on disk.
    saved_rev: u64,
    /// Revision whose rebuild failed (not retried automatically).
    failed_rev: Option<u64>,
    /// Editor session counter folded into `text_rev` (rich-mode edits).
    seen_counter: u64,
    /// Session state each recently installed source-mode revision produced (newest last), so
    /// a save of that revision marks it saved whichever worker finishes first.
    installed: Vec<(u64, markise_engine::editor::StateId)>,
    /// Source-buffer texts saved before the session reached them (revision, base document,
    /// buffer text): materialised as their own "Source Edit" boundary when a later revision
    /// is installed, so every saved state stays reachable by undo (ADR 0006 §4.3).
    saved_texts: Vec<(u64, Arc<dyn markise_doc::FormatDoc>, String)>,
    /// Revision whose incremental Side by Side update failed: the background rebuild takes
    /// over for it (once), and the incremental path is not retried until the text changes.
    incr_failed_rev: Option<u64>,
    /// Content identity of the text the saved revision holds (`saved_rev`): undoing back to
    /// it reads clean although the revision moved on (ADR 0006 §4.3 intent).
    saved_fp: Option<TextFp>,
    /// Content identity of the current text, cached per `text_rev`.
    fp_cache: std::cell::Cell<Option<((u64, u64), TextFp)>>,
}

/// Content identity of a document text: length and hash.
type TextFp = (usize, u64);

fn text_fp(text: &str) -> TextFp {
    use std::hash::{Hash, Hasher};
    let mut h = ahash::AHasher::default();
    text.hash(&mut h);
    (text.len(), h.finish())
}

impl DocState {
    /// Folds pending buffer changes and rich-mode edits into `text_rev`. A rich-mode edit
    /// updates the model synchronously, so it advances `model_rev` too.
    fn sync(&mut self) {
        if let Some(b) = &mut self.buffer
            && b.dirty
        {
            b.dirty = false;
            self.text_rev += 1;
        }
        let c = self.viewer.ed.session.revision().counter;
        if c != self.seen_counter {
            self.seen_counter = c;
            if self.text_rev == self.model_rev {
                self.text_rev += 1;
                self.model_rev = self.text_rev;
            }
        }
    }

    /// The source buffer holds text the session does not have yet (a rebuild is pending).
    fn buffer_ahead(&self) -> bool {
        self.text_rev != self.model_rev || self.buffer.as_ref().is_some_and(|b| b.dirty)
    }

    fn dirty(&self) -> bool {
        if self.buffer.as_ref().is_some_and(|b| b.dirty) {
            return true;
        }
        if self.buffer_ahead() {
            if self.text_rev == self.saved_rev {
                return false;
            }
        } else if !self.viewer.ed.session.is_dirty() {
            // the saved state itself (StateId, ADR 0006 §4.3)
            return false;
        } else if !self.viewer.ed.session.hints().is_empty() {
            // a hinted state is never the saved one (saving would materialise escapes)
            return true;
        }
        // another state with exactly the saved text (e.g. edited back in Side by Side)
        let Some(saved) = self.saved_fp else { return true };
        if !self.buffer_ahead() && self.viewer.ed.session.text().len() != saved.0 {
            return true; // no hashing while the length differs (typing)
        }
        self.current_fp() != saved
    }

    /// Content identity of the current text (hashed once per revision; only when the length
    /// matches the saved text's).
    fn current_fp(&self) -> TextFp {
        let key = (self.text_rev, self.viewer.ed.session.revision().counter);
        if let Some((k, fp)) = self.fp_cache.get()
            && k == key
        {
            return fp;
        }
        let fp = match &self.buffer {
            Some(b) if self.buffer_ahead() => text_fp(&b.doc_text()),
            _ => text_fp(self.viewer.ed.session.text()),
        };
        self.fp_cache.set(Some((key, fp)));
        fp
    }

    /// What a save writes now, its revision and (for a session snapshot) the session state
    /// it records as saved. Literal hints are materialised first (ADR 0006 §2.6).
    fn snapshot(&mut self) -> (SaveText, u64, Option<SaveSnapshot>) {
        if self.buffer_ahead()
            && let Some(b) = &self.buffer
        {
            let base = self.viewer.ed.session.doc_arc().clone();
            return (
                SaveText::Buffer {
                    base,
                    text: b.doc_text(),
                },
                self.text_rev,
                None,
            );
        }
        match self.viewer.ed.edit(|s| s.prepare_save()) {
            Ok(snap) => {
                self.sync();
                (SaveText::Exact(snap.text.clone()), self.text_rev, Some(snap))
            }
            Err(_) => (
                SaveText::Exact(self.viewer.ed.session.doc().snapshot()),
                self.text_rev,
                None,
            ),
        }
    }

    /// The Markdown document (source mode works on the concrete type).
    fn md(&self) -> &Document {
        self.viewer
            .ed
            .session
            .doc()
            .as_any()
            .downcast_ref::<Document>()
            .expect("a Markdown document")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadReason {
    Open,
    /// Open after the user chose Discard: installs without asking again.
    OpenDiscarding,
    /// Watcher event: check for an external change.
    Reload,
    /// The user chose "reload from disk": replace the document even with local edits.
    AcceptReload,
}

struct LoadInfo {
    path: PathBuf,
    started: Instant,
    reason: LoadReason,
    generation: u64,
}

struct Rebuild {
    doc_id: u64,
    rev: u64,
    rx: Receiver<Result<Rebuilt, String>>,
}

/// What to do after a successful save — only if the save covered the current text.
enum Then {
    Nothing,
    Close,
    Open(PathBuf),
    Install(Box<Loaded>),
}

struct SaveRequest {
    /// Overwrite authorization: the disk version the user saw (None = the recorded version).
    expect: Option<Expect>,
    then: Then,
}

struct Saving {
    doc_id: u64,
    /// Content identity of the text being saved.
    fp: TextFp,
    rx: Receiver<Saved>,
    then: Then,
    /// The session state the save writes (marked saved on success).
    snap: Option<SaveSnapshot>,
}

/// A question waiting for the user.
enum Ask {
    /// Unsaved changes: Save / Discard / Cancel, then continue with the action.
    Unsaved(Then),
    /// A link with a non-web scheme.
    Link(String),
    /// "Import theme…": a theme of that name exists; replace it?
    ReplaceTheme { src: PathBuf, existing: Vec<PathBuf> },
}

enum Banner {
    Error(String),
    Info(String),
    /// External change while there are unsaved local edits.
    Conflict(Box<Loaded>),
    /// Save refused: the file changed on disk (`found` = what is there now).
    SaveConflict {
        found: Option<DiskVersion>,
    },
}

/// Measurement log state (env-gated).
struct Timing {
    enabled: bool,
    start: Instant,
    frames: u64,
    first_frame_logged: bool,
    open_started: Option<(Instant, usize)>,
    exit_after_idle: Option<f32>,
    idle_from: Option<(Instant, u64)>,
    idle_input_frames: u64,
    idle_unprompted_frames: u64,
}

impl Timing {
    fn log(&self, msg: &str) {
        if self.enabled {
            eprintln!(
                "markise-timing: t={:.1}ms {msg}",
                self.start.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}

pub struct MarkiseApp {
    /// Keymap for host commands while no document is open (registry defaults + keyBinding).
    host_keymap: markise_engine::editor::keymap::Keymap,
    host_bindings_applied: Option<Vec<(String, String)>>,
    /// Test hook replacing the settings file's `keyBinding`.
    key_bindings_override: Option<Vec<(String, String)>>,
    ctx: Option<Context>,
    wake: Option<Wake>,
    hooks: Hooks,
    load_hook: Option<StageHook>,
    doc: Option<DocState>,
    next_doc_id: u64,
    loader: Option<Loader>,
    load: Option<LoadInfo>,
    /// A watcher event arrived while a load / save was running: check again afterwards.
    reload_pending: bool,
    rebuild: Option<Rebuild>,
    saving: Option<Saving>,
    queued_save: Option<SaveRequest>,
    ask: Option<Ask>,
    allow_close: bool,
    source_visible: bool,
    /// Side by Side mode (spec 04 §3.3; exclusive with `source_visible`).
    side_by_side: bool,
    /// The mode ⌃⌘/ returns to from Side by Side: Source (`true`) or Rendered.
    sbs_return_source: bool,
    sbs: SideBySide,
    /// Editor ids of replaced buffers: their `TextEdit` state (undo history) is dropped.
    retired_editors: Vec<egui::Id>,
    /// Colour the Side by Side source pane (below the size limit, 99 C58).
    source_highlight: bool,
    source_highlighter: crate::source_highlight::SourceHighlighter,
    /// Keystroke → model update times in Side by Side (ms; measurements, tests).
    pub sbs_apply_ms: Vec<f64>,
    /// Leaving source mode: scroll the preview to this offset once the model is current.
    pending_scroll: Option<usize>,
    sidebar_visible: bool,
    /// Formatting toolbar state (spec 04 §3.2; shown per the `showToolbar` setting).
    toolbar: crate::toolbar::Toolbar,
    tab: Tab,
    tree: Option<FileTree>,
    themes: ThemeManager,
    /// The theme report window is open.
    theme_report: bool,
    /// The "Font & size" window is open.
    font_window: bool,
    /// Summary of the last "Import theme…" (shown in the report window).
    installed: Option<InstallDone>,
    /// Fingerprint of the theme the egui visuals were built from.
    visuals_fp: Option<u64>,
    /// [`ThemeManager::generation`] of `theme`.
    theme_gen: u64,
    /// New font definitions were handed to egui; they apply at the next pass, so the font
    /// generation (part of every layout cache key) is bumped only then — otherwise layouts
    /// shaped with the old fonts would be cached under the new generation.
    fonts_pending: bool,
    parse: ParseOptions,
    watcher: Option<FileWatcher>,
    images: TextureStore,
    /// Code highlighter (created with the first frame's repaint callback; its worker starts
    /// on the first code block shown).
    highlighter: Option<Arc<crate::highlight::CodeHighlighter>>,
    highlight_code: bool,
    /// Diagram renderer (created with the first frame's repaint callback; its worker starts on
    /// the first diagram shown) and the Diagrams preference (spec 01 §4.6, default off).
    diagram_renderer: Option<Arc<crate::diagram::DiagramRenderer>>,
    diagrams: bool,
    text_backend: crate::text_paint::TextBackend,
    /// The parley shaper and glyph atlas (only with [`crate::TextBackend::Parley`]; created
    /// with the first font definitions).
    parley: Option<crate::text_paint::ParleyText>,
    banner: Option<Banner>,
    title: String,
    pending_open: Vec<PathBuf>,
    timing: Timing,
    font_gen: u64,
    theme: Theme,
    /// File → Export state (running job, previous export, result message).
    pub(crate) export: crate::export_ui::ExportUi,
    /// Number of frames run (for tests and the idle-repaint measurement).
    pub frames: u64,
    /// Native menu bar (macOS, Windows); `None`: the egui menu bar is shown.
    menu_host: Option<Box<dyn MenuHost>>,
    /// Platform whose menus and shortcuts are shown.
    menu_platform: markise_engine::editor::keymap::Platform,
    /// About Markise window.
    about: crate::about::About,
    commit: Option<String>,
}

impl MarkiseApp {
    pub fn new(opts: AppOptions) -> Self {
        let themes = ThemeManager::new(opts.config, opts.system_fonts);
        let diagrams = themes.diagrams();
        MarkiseApp {
            host_keymap: markise_engine::editor::keymap::Keymap::new(
                &markise_engine::editor::command::Registry::for_format(&markise_md::MarkdownFormat::default()),
                markise_engine::editor::keymap::Platform::current(),
            ),
            host_bindings_applied: None,
            key_bindings_override: None,
            ctx: None,
            wake: None,
            hooks: opts.hooks,
            load_hook: opts.load_hook,
            doc: None,
            next_doc_id: 1,
            loader: None,
            load: None,
            reload_pending: false,
            rebuild: None,
            saving: None,
            queued_save: None,
            ask: None,
            allow_close: false,
            source_visible: false,
            side_by_side: false,
            sbs_return_source: false,
            sbs: SideBySide::default(),
            retired_editors: Vec::new(),
            source_highlight: true,
            source_highlighter: Default::default(),
            sbs_apply_ms: Vec::new(),
            pending_scroll: None,
            sidebar_visible: true,
            toolbar: Default::default(),
            tab: Tab::Outline,
            tree: None,
            themes,
            theme_report: false,
            font_window: false,
            installed: None,
            visuals_fp: None,
            theme_gen: 0,
            fonts_pending: false,
            parse: opts.parse,
            watcher: None,
            images: TextureStore::default(),
            highlighter: None,
            highlight_code: opts.highlight_code,
            diagram_renderer: None,
            diagrams,
            text_backend: opts.text_backend,
            parley: None,
            banner: None,
            title: String::new(),
            pending_open: opts.open,
            timing: Timing {
                enabled: opts.timing,
                start: opts.start,
                frames: 0,
                first_frame_logged: false,
                open_started: None,
                exit_after_idle: opts.exit_after_idle,
                idle_from: None,
                idle_input_frames: 0,
                idle_unprompted_frames: 0,
            },
            font_gen: 0,
            theme: Theme::light(),
            export: Default::default(),
            frames: 0,
            menu_host: None,
            menu_platform: markise_engine::editor::keymap::Platform::current(),
            about: Default::default(),
            commit: opts.commit,
        }
    }

    // ------------------------------------------------------------------ accessors (tests)

    /// Repaint callback for workers (a no-op before the first frame).
    pub(crate) fn wake_handle(&self) -> Wake {
        self.wake.clone().unwrap_or_else(|| Arc::new(|| {}))
    }

    pub fn document_path(&self) -> Option<&Path> {
        self.doc.as_ref().map(|d| d.path.as_path())
    }

    pub fn model(&self) -> Option<&Arc<DocModel>> {
        self.doc.as_ref().map(|d| d.viewer.model())
    }

    /// The text of the document the preview shows (the model's document).
    pub fn viewed_text(&self) -> Option<&str> {
        self.doc.as_ref().map(|d| d.viewer.ed.session.text())
    }

    pub fn is_loading(&self) -> bool {
        self.load.is_some()
    }

    /// A load, rebuild or save job is running.
    pub fn is_busy(&self) -> bool {
        self.rebuild.is_some() || self.saving.is_some() || self.load.is_some()
    }

    /// A save job is running.
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    pub fn in_source_mode(&self) -> bool {
        self.source_visible
    }

    /// The current view mode (spec 04 §3.3).
    pub fn view_mode(&self) -> ViewMode {
        if self.side_by_side {
            ViewMode::SideBySide
        } else if self.source_visible {
            ViewMode::Source
        } else {
            ViewMode::Rendered
        }
    }

    /// Vertical scroll offset of the source editor (points), if it exists.
    pub fn source_scroll_offset(&self) -> Option<f32> {
        self.doc.as_ref().and_then(|d| d.buffer.as_ref()).map(|b| b.offset)
    }

    /// Source line (fractional) at the top of the source editor's viewport.
    pub fn source_top_line(&self) -> Option<f32> {
        self.doc
            .as_ref()
            .and_then(|d| d.buffer.as_ref())
            .map(|b| b.line_at_y(b.offset))
    }

    /// Syntax colouring of the Side by Side source pane (on by default; measurements).
    pub fn set_source_highlighting(&mut self, on: bool) {
        self.source_highlight = on;
    }

    /// Side by Side divider position (fraction of the width for the source pane).
    pub fn side_by_side_split(&self) -> f32 {
        self.themes.side_by_side_split().unwrap_or(DEFAULT_SPLIT)
    }

    pub fn set_side_by_side_split(&mut self, v: f32) {
        self.themes.set_side_by_side_split(v);
    }

    pub fn viewer(&mut self) -> Option<&mut DocViewer> {
        self.doc.as_mut().map(|d| &mut d.viewer)
    }

    /// The source editor's text, if a buffer exists (tests).
    pub fn source_buffer(&self) -> Option<&str> {
        self.doc
            .as_ref()
            .and_then(|d| d.buffer.as_ref())
            .map(|b| b.buffer.as_str())
    }

    /// The editor buffer, marked as changed (tests type through this).
    pub fn source_buffer_mut(&mut self) -> Option<&mut String> {
        self.doc.as_mut().and_then(|d| d.buffer.as_mut()).map(|b| {
            b.dirty = true;
            &mut b.buffer
        })
    }

    /// The authoritative text of the document (buffer or model).
    pub fn document_text(&self) -> Option<String> {
        let d = self.doc.as_ref()?;
        Some(match &d.buffer {
            Some(b) if d.buffer_ahead() => b.doc_text(),
            _ => d.viewer.ed.session.text().to_string(),
        })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Unsaved edits exist.
    pub fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty())
    }

    /// Text of the banner, if any (tests).
    pub fn banner_text(&self) -> Option<String> {
        self.banner.as_ref().map(|b| match b {
            Banner::Error(e) => format!("error: {e}"),
            Banner::Info(m) => m.clone(),
            Banner::Conflict(_) => "conflict: changed on disk".into(),
            Banner::SaveConflict { .. } => "save-conflict".into(),
        })
    }

    /// A Save / Discard / Cancel (or link) question is open.
    pub fn is_asking(&self) -> bool {
        self.ask.is_some()
    }

    pub fn set_theme_choice(&mut self, c: ThemeChoice) {
        self.themes.set_appearance(c);
    }

    /// Orderly shutdown: writes pending settings.
    pub fn on_exit(&mut self) {
        self.themes.flush();
    }

    /// Selects a style, keeping the colors (menu, tests).
    pub fn select_style(&mut self, id: StyleId) {
        self.themes.select_style(id);
    }

    /// Selects a color family, keeping the style (menu, tests).
    pub fn select_colors(&mut self, id: ThemeId) {
        self.themes.select_colors(id);
    }

    /// Sets the typography overrides (menu, tests).
    pub fn set_overrides(&mut self, o: Overrides) {
        self.themes.set_overrides(o);
    }

    /// Selects a complete theme: its style and colors (menu, tests).
    pub fn select_theme(&mut self, id: ThemeId) {
        self.themes.select(id);
    }

    /// The theme currently rendered.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn theme_manager(&self) -> &ThemeManager {
        &self.themes
    }

    /// Names in the file tree as last shown, top to bottom (tests: wait for the listing).
    pub fn file_tree_rows(&self) -> Vec<String> {
        self.tree.as_ref().map(|t| t.row_names()).unwrap_or_default()
    }

    /// Shows the Files (`true`) or Outline tab of the sidebar.
    pub fn show_files_tab(&mut self, files: bool) {
        self.tab = if files { Tab::Files } else { Tab::Outline };
    }

    /// Expands or collapses a folder of the file tree.
    pub fn toggle_folder(&mut self, dir: &Path) {
        if let Some(t) = self.tree.as_mut() {
            t.toggle(dir);
        }
    }

    /// "Import theme…" with a known file (tests; the menu asks with a file dialog).
    pub fn import_theme(&mut self, css: PathBuf) {
        self.themes.install(css);
    }

    // ------------------------------------------------------------------ opening

    /// Opens a file (background load) or mounts a folder; asks first if the current
    /// document has unsaved edits.
    pub fn open(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path.is_dir() {
            if let Some(w) = self.watcher.as_mut() {
                w.watch_folder(&path);
            }
            self.tree = Some(FileTree::new(path));
            self.tab = Tab::Files;
            self.sidebar_visible = true;
            return;
        }
        if self.doc.as_ref().is_some_and(|d| d.path == path) {
            return;
        }
        if self.is_dirty() {
            self.ask = Some(Ask::Unsaved(Then::Open(path)));
            return;
        }
        self.open_now(path, LoadReason::Open);
    }

    fn open_now(&mut self, path: PathBuf, reason: LoadReason) {
        if self.tree.is_none()
            && let Some(parent) = path.parent()
        {
            // Mount the file's folder (lazy listing, decision M1-1).
            self.tree = Some(FileTree::new(parent.to_path_buf()));
            if let Some(w) = self.watcher.as_mut() {
                w.watch_folder(parent);
            }
        }
        self.start_load(path, reason);
    }

    fn start_load(&mut self, path: PathBuf, reason: LoadReason) {
        let Some(wake) = self.wake.clone() else {
            self.pending_open.push(path);
            return;
        };
        if self.loader.is_none() {
            match Loader::new(self.parse.clone(), wake, self.load_hook.clone()) {
                Ok(l) => self.loader = Some(l),
                Err(e) => {
                    self.banner = Some(Banner::Error(e));
                    return;
                }
            }
        }
        if matches!(reason, LoadReason::Open | LoadReason::OpenDiscarding) {
            let size = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
            self.timing.open_started = Some((Instant::now(), size));
            self.timing
                .log(&format!("open-start path={} bytes={size}", path.display()));
        }
        // the single loader worker cancels whatever it was doing for an older request
        let generation = self.loader.as_ref().expect("loader").request(path.clone());
        self.load = Some(LoadInfo {
            path,
            started: Instant::now(),
            reason,
            generation,
        });
    }

    fn poll_loading(&mut self) {
        let Some(loader) = &self.loader else { return };
        let mut got = None;
        while let Some((generation, r)) = loader.try_recv() {
            if self.load.as_ref().is_some_and(|l| l.generation == generation) {
                got = Some(r);
            } // else: superseded
        }
        let Some(r) = got else { return };
        let l = self.load.take().expect("load");
        match r {
            Err(e) => self.banner = Some(Banner::Error(e)),
            Ok(loaded) => {
                let t = loaded.timings;
                self.timing.log(&format!(
                    "loaded path={} bytes={} items={} read={:.1}ms parse={:.1}ms model={:.1}ms wall={:.1}ms",
                    loaded.path.display(),
                    loaded.bytes,
                    loaded.model.items.len(),
                    t.read.as_secs_f64() * 1e3,
                    t.parse.as_secs_f64() * 1e3,
                    t.model.as_secs_f64() * 1e3,
                    l.started.elapsed().as_secs_f64() * 1e3
                ));
                match l.reason {
                    LoadReason::Open => {
                        if self.is_dirty() {
                            // edits were made while the open was pending
                            self.ask = Some(Ask::Unsaved(Then::Install(Box::new(loaded))));
                        } else {
                            self.install(loaded);
                        }
                    }
                    LoadReason::OpenDiscarding => self.install(loaded),
                    LoadReason::Reload => self.external_change(loaded),
                    LoadReason::AcceptReload => self.accept_reload(loaded),
                }
            }
        }
        if std::mem::take(&mut self.reload_pending) {
            self.request_reload();
        }
    }

    /// The highlight source views use: the highlighter when highlighting is on.
    fn view_highlights(&self) -> Arc<dyn markise_layout::Highlights + Send + Sync> {
        match (&self.highlighter, self.highlight_code) {
            (Some(h), true) => h.clone(),
            _ => Arc::new(markise_layout::NoHighlights),
        }
    }

    /// The diagram source views use: the renderer when diagrams are on.
    fn view_diagrams(&self) -> Arc<dyn markise_layout::Diagrams + Send + Sync> {
        match (&self.diagram_renderer, self.diagrams) {
            (Some(d), true) => d.clone(),
            _ => Arc::new(markise_layout::NoDiagrams),
        }
    }

    /// A new viewer was created for a new document.
    fn document_changed(&mut self, viewer: &mut DocViewer) {
        if let Some(h) = &self.highlighter {
            h.document_changed();
        }
        if let Some(d) = &self.diagram_renderer {
            d.document_changed();
        }
        viewer.ed.view.set_highlights(self.view_highlights());
        viewer.ed.view.set_diagrams(self.view_diagrams());
    }

    /// Switches diagram rendering on or off (View menu "Diagrams"; spec 01 §4.6 preference,
    /// persisted in the settings file, default on — DG-2/DG-7).
    pub fn set_diagrams(&mut self, on: bool) {
        self.themes.set_diagrams(on);
        self.apply_diagrams(on);
    }

    /// Shows diagrams rendered or as source in the open document (no settings write).
    fn apply_diagrams(&mut self, on: bool) {
        self.diagrams = on;
        let d = self.view_diagrams();
        if let Some(doc) = self.doc.as_mut() {
            doc.viewer.ed.view.set_diagrams(d);
        }
    }

    /// Whether diagrams are rendered.
    pub fn diagrams(&self) -> bool {
        self.diagrams
    }

    /// The diagram renderer, once the first frame created it (tests).
    pub fn diagram_renderer(&self) -> Option<&Arc<crate::diagram::DiagramRenderer>> {
        self.diagram_renderer.as_ref()
    }

    /// Switches code highlighting on or off (View menu).
    pub fn set_highlight_code(&mut self, on: bool) {
        self.highlight_code = on;
        let h = self.view_highlights();
        if let Some(d) = self.doc.as_mut() {
            d.viewer.ed.view.set_highlights(h);
        }
    }

    /// Whether the formatting toolbar is shown (View → Toolbar, persisted as `showToolbar`;
    /// spec 04 §3.2). In this version it is shown over the rendered document only (Rendered,
    /// and the rendered pane of Side by Side); source mode has no toolbar yet.
    pub fn show_toolbar(&self) -> bool {
        self.themes.show_toolbar()
    }

    /// View → Toolbar.
    pub fn set_show_toolbar(&mut self, on: bool) {
        self.themes.set_show_toolbar(on);
        if !on {
            self.toolbar.close();
        }
    }

    /// The toolbar's controls as shown now (tests): `None` while no toolbar is visible.
    pub fn toolbar_state(&self) -> Option<crate::toolbar::Model> {
        self.toolbar_visible().then(|| self.toolbar_model(!self.side_by_side))
    }

    /// Toolbar drop-down open / groups collapsed into More (tests).
    pub fn toolbar(&self) -> &crate::toolbar::Toolbar {
        &self.toolbar
    }

    /// The toolbar is drawn this frame: enabled, a document is open and the rendered document
    /// is visible (not in source mode).
    fn toolbar_visible(&self) -> bool {
        self.show_toolbar() && self.doc.is_some() && !self.source_visible
    }

    /// Whether code highlighting is on.
    pub fn highlight_code(&self) -> bool {
        self.highlight_code
    }

    /// The code highlighter, once the first frame created it (tests).
    pub fn highlighter(&self) -> Option<&Arc<crate::highlight::CodeHighlighter>> {
        self.highlighter.as_ref()
    }

    fn install(&mut self, loaded: Loaded) {
        let same = self.doc.as_ref().is_some_and(|d| d.path == loaded.path);
        if !same {
            self.images.clear();
        }
        self.source_visible = false;
        self.rebuild = None;
        self.pending_scroll = None;
        self.sbs = SideBySide::default();
        self.sbs.sync_source = true;
        self.source_highlighter = Default::default();
        self.banner = loaded.lossy.then(|| {
            Banner::Info("The file is not valid UTF-8; it was decoded lossily and saving is disabled.".into())
        });
        if let Some(w) = self.watcher.as_mut() {
            w.watch_file(&loaded.path);
        }
        let format: Arc<dyn markise_doc::DocumentFormat> = Arc::new(MarkdownFormat {
            opts: self.parse.clone(),
        });
        let mut viewer = DocViewer::new(format, loaded.doc, loaded.model, &self.theme, Some(loaded.path.clone()));
        let saved_fp = Some(text_fp(viewer.doc().text()));
        viewer.set_font_generation(self.font_gen);
        self.document_changed(&mut viewer);
        let id = self.next_doc_id;
        self.next_doc_id += 1;
        if let Some(old) = self.doc.take()
            && let Some(b) = old.buffer
        {
            self.retired_editors.push(b.id);
        }
        self.doc = Some(DocState {
            key_bindings_applied: None,
            id,
            path: loaded.path,
            viewer,
            lossy: loaded.lossy,
            deleted: false,
            disk: Some(loaded.version),
            buffer: None,
            text_rev: 0,
            model_rev: 0,
            saved_rev: 0,
            failed_rev: None,
            seen_counter: 0,
            installed: Vec::new(),
            saved_texts: Vec::new(),
            incr_failed_rev: None,
            saved_fp,
            fp_cache: std::cell::Cell::new(None),
        });
    }

    // ------------------------------------------------------------------ external changes

    /// Watcher event for the open file (spec 04 §11).
    fn request_reload(&mut self) {
        if self.load.is_some() || self.saving.is_some() {
            self.reload_pending = true;
            return;
        }
        if let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) {
            self.start_load(path, LoadReason::Reload);
        }
    }

    fn external_change(&mut self, loaded: Loaded) {
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.path != loaded.path {
            return;
        }
        if doc.disk.is_some_and(|d| d.same_content(&loaded.version)) {
            doc.deleted = false;
            return; // our own save, or a touch without a content change
        }
        if self.saving.is_some() {
            self.reload_pending = true; // decide after our save finished
            return;
        }
        if self.is_dirty() {
            if let Some(Banner::SaveConflict { found }) = &mut self.banner {
                // keep the save-conflict choices; "Overwrite" now targets the newest version
                *found = Some(loaded.version);
            } else {
                self.banner = Some(Banner::Conflict(Box::new(loaded)));
            }
            return;
        }
        self.accept_reload(loaded);
    }

    /// Replaces the document's text with `loaded` (explicit decision, or a clean document).
    fn accept_reload(&mut self, loaded: Loaded) {
        let visible = self.source_visible;
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.path != loaded.path {
            return;
        }
        // one undoable "Reload from Disk" transaction (ADR 0006 §4.3); disk is now current
        let _ = doc.viewer.install(loaded.doc, loaded.model, "Reload from Disk", false);
        doc.viewer.ed.session.mark_current_saved();
        doc.seen_counter = doc.viewer.ed.session.revision().counter;
        doc.saved_texts.clear(); // the buffer they came from is dropped below
        if let Some(h) = &self.highlighter {
            h.document_changed();
        }
        if let Some(d) = &self.diagram_renderer {
            d.document_changed();
        }
        doc.disk = Some(loaded.version);
        doc.lossy = loaded.lossy;
        doc.deleted = false;
        let rev = doc.text_rev.max(doc.model_rev) + 1;
        doc.text_rev = rev;
        doc.model_rev = rev;
        doc.saved_rev = rev;
        doc.failed_rev = None;
        doc.incr_failed_rev = None;
        doc.saved_fp = Some(text_fp(doc.viewer.doc().text()));
        // the buffer belonged to the dropped edits; a visible editor shows the new text
        let line = doc.buffer.as_ref().map_or(0, |b| b.middle_line());
        if let Some(b) = doc.buffer.take() {
            self.retired_editors.push(b.id);
        }
        doc.buffer = visible.then(|| SourceState::new(doc.md(), line));
        self.rebuild = None; // its revision is older than the new text
        if matches!(self.banner, Some(Banner::Conflict(_) | Banner::SaveConflict { .. })) {
            self.banner = None;
        }
    }

    fn poll_watcher(&mut self) {
        let mut events = Vec::new();
        if let Some(w) = &self.watcher {
            while let Some(e) = w.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            match e {
                WatchEvent::FileChanged(p) => {
                    if self.doc.as_ref().is_some_and(|d| same_file(&d.path, &p)) {
                        self.request_reload();
                    }
                }
                WatchEvent::FileRemoved(p) => {
                    if let Some(doc) = &mut self.doc
                        && same_file(&doc.path, &p)
                    {
                        // C18: keep the content, mark it; saving recreates the file.
                        doc.deleted = true;
                        self.banner = Some(Banner::Info(
                            "The file was deleted or moved on disk. Save to write it again.".into(),
                        ));
                    }
                }
                WatchEvent::DirChanged(d) => {
                    if let (Some(t), Some(w)) = (self.tree.as_mut(), self.wake.as_ref()) {
                        t.invalidate(&d, w);
                    }
                }
                // live reload: re-import the affected theme, re-render (new fingerprint)
                WatchEvent::ThemeChanged(p) => self.themes.file_changed(&p),
            }
        }
    }

    // ------------------------------------------------------------------ derived model

    /// Brings the preview model up to the text revision (one rebuild at a time).
    fn ensure_model(&mut self) {
        if self.rebuild.is_some() {
            return;
        }
        let (Some(doc), Some(wake)) = (self.doc.as_mut(), self.wake.clone()) else {
            return;
        };
        doc.sync();
        if doc.model_rev == doc.text_rev || doc.failed_rev == Some(doc.text_rev) {
            return;
        }
        let Some(text) = doc.buffer.as_ref().map(|b| b.doc_text()) else {
            return;
        };
        let rev = doc.text_rev;
        let rx = spawn_rebuild(
            doc.viewer.doc().clone(),
            doc.viewer.model().clone(),
            text,
            Some(doc.path.clone()),
            rev,
            self.hooks.clone(),
            wake,
        );
        self.rebuild = Some(Rebuild {
            doc_id: doc.id,
            rev,
            rx,
        });
    }

    fn poll_rebuild(&mut self) {
        let Some(r) = &self.rebuild else { return };
        let res = match r.rx.try_recv() {
            Ok(x) => x,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            // the worker died (panicked) without answering
            Err(_) => Err("the rebuild worker stopped unexpectedly".into()),
        };
        let r = self.rebuild.take().expect("rebuild");
        let Some(doc) = self.doc.as_mut().filter(|d| d.id == r.doc_id) else {
            return;
        };
        match res {
            Ok(done) if done.rev > doc.model_rev && done.rev <= doc.text_rev => {
                // the source edit joins the open "Source Edit" transaction (one per source
                // session segment between saves, ADR 0006 §4.3) — not while a save is in
                // flight: the state it may mark saved must stay a transaction boundary
                let mut join = self.saving.is_none();
                // saved source texts older than this revision first, each its own boundary
                let saved_texts = std::mem::take(&mut doc.saved_texts);
                for (rev, base, text) in saved_texts {
                    if rev > done.rev {
                        doc.saved_texts.push((rev, base, text));
                        continue;
                    }
                    if rev == done.rev {
                        continue; // this install is that text
                    }
                    let mut d = base.clone_box();
                    markise_engine::edit::apply_text_doc(&mut *d, &text);
                    let d: Arc<dyn markise_doc::FormatDoc> = Arc::from(d);
                    doc.viewer.ed.release();
                    let _ =
                        doc.viewer
                            .ed
                            .session
                            .replace_doc(d, "Source Edit", markise_engine::editor::TxKind::Bulk, join);
                    let state = doc.viewer.ed.session.state_id();
                    doc.installed.push((rev, state));
                    if doc.saved_rev == rev {
                        doc.viewer.ed.session.mark_current_saved();
                    }
                    join = false;
                }
                let _ = doc.viewer.install(done.doc, done.model, "Source Edit", join);
                let state = doc.viewer.ed.session.state_id();
                doc.installed.push((done.rev, state));
                if doc.installed.len() > 16 {
                    doc.installed.remove(0);
                }
                if doc.saved_rev == done.rev {
                    doc.viewer.ed.session.mark_current_saved();
                }
                doc.seen_counter = doc.viewer.ed.session.revision().counter;
                if let Some(h) = &self.highlighter {
                    h.document_changed();
                }
                if let Some(d) = &self.diagram_renderer {
                    d.document_changed();
                }
                doc.model_rev = done.rev;
                if doc.model_rev == doc.text_rev
                    && let Some(offset) = self.pending_scroll.take()
                {
                    let len = doc.viewer.ed.session.text().len();
                    let item = doc.viewer.model().item_at_offset(offset.min(len));
                    doc.viewer.ed.view.scroll_to_item(item.saturating_sub(2));
                }
            }
            Ok(_) => {} // stale (reloaded meanwhile)
            Err(e) => {
                // the text stays in the buffer; only the preview is behind
                doc.failed_rev = Some(r.rev);
                self.banner = Some(Banner::Error(format!(
                    "The preview could not be updated ({e}). Your text is kept; switch to source mode to continue editing."
                )));
            }
        }
    }

    // ------------------------------------------------------------------ saving

    fn start_save(&mut self, req: SaveRequest) {
        if self.saving.is_some() {
            // latest request wins, but a pending continuation is not forgotten
            let then = match (&req.then, self.queued_save.take()) {
                (Then::Nothing, Some(q)) => q.then,
                _ => req.then,
            };
            self.queued_save = Some(SaveRequest {
                expect: req.expect,
                then,
            });
            return;
        }
        let (Some(doc), Some(wake)) = (self.doc.as_mut(), self.wake.clone()) else {
            return;
        };
        doc.sync();
        if doc.lossy {
            self.banner = Some(Banner::Error("Saving is disabled for a lossily decoded file.".into()));
            return;
        }
        if !doc.dirty() && !doc.deleted && req.expect.is_none() {
            // the file already holds exactly this text
            self.finish_then(req.then);
            return;
        }
        let expect = req.expect.unwrap_or(match (doc.disk, doc.deleted) {
            (Some(v), false) => Expect::Version(v),
            (Some(v), true) => Expect::VersionOrMissing(v),
            (None, _) => Expect::Missing,
        });
        let (text, rev, snap) = doc.snapshot();
        if let SaveText::Buffer { base, text } = &text
            && !doc.saved_texts.iter().any(|(r, _, _)| *r == rev)
        {
            doc.saved_texts.push((rev, base.clone(), text.clone()));
        }
        let fp = match &text {
            SaveText::Exact(t) => text_fp(t),
            SaveText::Buffer { text, .. } => text_fp(text),
        };
        let rx = spawn_save(text, doc.path.clone(), expect, rev, self.hooks.clone(), wake);
        self.saving = Some(Saving {
            doc_id: doc.id,
            fp,
            rx,
            then: req.then,
            snap,
        });
    }

    /// Saves the document (Cmd/Ctrl+S).
    pub fn save(&mut self) {
        self.start_save(SaveRequest {
            expect: None,
            then: Then::Nothing,
        });
    }

    fn poll_save(&mut self) {
        let Some(s) = &self.saving else { return };
        let saved = match s.rx.try_recv() {
            Ok(x) => x,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Saved {
                rev: 0,
                result: Err(SaveError::Io("the save worker stopped unexpectedly".into())),
                bytes: 0,
            },
        };
        let s = self.saving.take().expect("saving");
        let current = self.doc.as_ref().is_some_and(|d| d.id == s.doc_id);
        if current {
            let doc = self.doc.as_mut().expect("doc");
            match saved.result {
                Ok(out) => {
                    doc.disk = Some(out.version);
                    doc.deleted = false;
                    if saved.rev >= doc.saved_rev {
                        doc.saved_rev = saved.rev;
                        doc.saved_fp = Some(s.fp);
                    }
                    if let Some(snap) = &s.snap {
                        doc.viewer.ed.session.mark_saved(snap);
                    } else if let Some(&(_, state)) = doc.installed.iter().rev().find(|(r, _)| *r == saved.rev) {
                        // a source buffer save whose rebuild was installed first
                        doc.viewer.ed.session.mark_saved_state(state);
                    } else {
                        // the saved text is not a session state yet (still in the source
                        // buffer): no history state is on disk until it is installed
                        doc.viewer.ed.session.clear_saved_state();
                    }
                    self.timing.log(&format!("saved bytes={}", saved.bytes));
                    self.banner = out.kept_backup.map(|b| {
                        Banner::Info(format!(
                            "Saved. Another program changed the file while saving; its version was kept in {}",
                            b.display()
                        ))
                    });
                    self.finish_then(s.then);
                }
                Err(SaveError::Conflict { found }) => {
                    self.queued_save = None;
                    self.banner = Some(Banner::SaveConflict { found });
                }
                Err(SaveError::Io(e)) => {
                    // spec 04 §10: content stays in the editor, still unsaved; report it
                    self.queued_save = None;
                    self.banner = Some(Banner::Error(format!("Save failed: {e}")));
                }
            }
        }
        if let Some(q) = self.queued_save.take() {
            self.start_save(q);
        }
        if std::mem::take(&mut self.reload_pending) {
            self.request_reload();
        }
    }

    /// Runs a save continuation if its precondition holds: the saved revision covers the
    /// current text. Otherwise (the user kept typing) the question is asked again.
    fn finish_then(&mut self, then: Then) {
        if matches!(then, Then::Nothing) {
            return;
        }
        if let Some(d) = &mut self.doc {
            d.sync();
        }
        if self.is_dirty() {
            self.ask = Some(Ask::Unsaved(then));
        } else {
            self.perform(then);
        }
    }

    /// Performs an action without saving (Discard, or after a covering save). Discard does
    /// not modify the document: if the action fails (e.g. the other file cannot be opened),
    /// the edited document stays as it was.
    fn perform(&mut self, then: Then) {
        match then {
            Then::Nothing => {}
            Then::Open(path) => self.open_now(path, LoadReason::OpenDiscarding),
            Then::Install(loaded) => self.install(*loaded),
            Then::Close => {
                self.allow_close = true;
                if let Some(c) = &self.ctx {
                    c.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// The window is about to close; returns `true` if it may (no unsaved edits). Otherwise
    /// the Save / Discard / Cancel question is shown.
    pub fn request_close(&mut self) -> bool {
        if let Some(d) = &mut self.doc {
            d.sync();
        }
        if self.allow_close || !self.is_dirty() {
            return true;
        }
        self.ask = Some(Ask::Unsaved(Then::Close));
        false
    }

    /// Toggles source-code mode, mapping the viewport middle in both directions. The editor
    /// buffer is kept when leaving, so re-entering never starts from a stale model.
    pub fn toggle_source(&mut self) {
        if self.side_by_side {
            // ⌘/ from Side by Side → Source, full width (spec 04 §3.3)
            self.set_view_mode(ViewMode::Source);
            return;
        }
        let Some(doc) = self.doc.as_mut() else { return };
        doc.sync();
        if !self.source_visible {
            // literal hints expire on a mode switch (ADR 0006 §2.6)
            let _ = doc.viewer.ed.edit(|s| s.expire_hints());
            doc.sync();
            // a buffer the session has caught up with is recreated from the session (the
            // rich editor may have changed the text since)
            Self::refresh_stale_buffer(doc, &mut self.retired_editors);
            if doc.buffer.is_none() {
                let (item, _) = doc.viewer.ed.view.anchor();
                let pos = doc.viewer.model().items.start(item).unwrap_or(0);
                let text = doc.md();
                let line = text.text().as_bytes()[..pos].iter().filter(|&&b| b == b'\n').count();
                doc.buffer = Some(SourceState::new(text, line + 8));
            }
            self.source_visible = true;
        } else {
            self.source_visible = false;
            doc.viewer.request_refocus();
            if let Some(b) = &doc.buffer {
                let offset = b.line_to_doc_offset(b.middle_line());
                if doc.model_rev == doc.text_rev {
                    let len = doc.viewer.ed.session.text().len();
                    let item = doc.viewer.model().item_at_offset(offset.min(len));
                    doc.viewer.ed.view.scroll_to_item(item.saturating_sub(2));
                } else {
                    self.pending_scroll = Some(offset);
                }
            }
        }
    }

    /// ⌃⌘/ (spec 04 §3.3): Rendered or Source → Side by Side, remembering that mode; again →
    /// back to the remembered mode.
    pub fn toggle_side_by_side(&mut self) {
        if self.side_by_side {
            let back = if self.sbs_return_source {
                ViewMode::Source
            } else {
                ViewMode::Rendered
            };
            self.set_view_mode(back);
        } else {
            self.sbs_return_source = self.source_visible;
            self.enter_side_by_side();
        }
    }

    /// Sets the view mode directly (View menu radio items); Rendered and Source become the
    /// mode ⌃⌘/ returns to.
    pub fn set_view_mode(&mut self, m: ViewMode) {
        if self.doc.is_none() {
            return;
        }
        match m {
            ViewMode::SideBySide => {
                if !self.side_by_side {
                    // like ⌃⌘/: leaving Side by Side returns to the mode it was entered from
                    self.sbs_return_source = self.source_visible;
                    self.enter_side_by_side();
                }
            }
            ViewMode::Source | ViewMode::Rendered => {
                self.sbs_return_source = m == ViewMode::Source;
                if self.side_by_side {
                    // the panes already show the same place: keep the positions
                    self.side_by_side = false;
                    self.source_visible = m == ViewMode::Source;
                    if let (ViewMode::Rendered, Some(doc)) = (m, self.doc.as_mut()) {
                        doc.viewer.request_refocus();
                    }
                } else if (m == ViewMode::Source) != self.source_visible {
                    self.toggle_source();
                }
            }
        }
    }

    fn enter_side_by_side(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.sync();
        let from_source = self.source_visible;
        self.source_visible = false;
        self.side_by_side = true;
        let _ = doc.viewer.ed.edit(|s| s.expire_hints());
        doc.sync();
        Self::refresh_stale_buffer(doc, &mut self.retired_editors);
        if doc.buffer.is_none() {
            let (item, _) = doc.viewer.ed.view.anchor();
            let pos = doc.viewer.model().items.start(item).unwrap_or(0);
            let text = doc.md();
            let line = text.text().as_bytes()[..pos].iter().filter(|&&b| b == b'\n').count();
            doc.buffer = Some(SourceState::new(text, line));
        }
        // the pane the user was looking at leads; the other follows on the first frame
        self.sbs.sync_rendered = from_source;
        self.sbs.sync_source = !from_source;
    }

    /// Drops a source buffer the rich editor has moved past (its text is no longer the
    /// session's), so the source editor shows the current text; a buffer that is ahead of
    /// the session (rebuild pending) or equal to it is kept with its undo history.
    fn refresh_stale_buffer(doc: &mut DocState, retired: &mut Vec<egui::Id>) {
        let stale = doc
            .buffer
            .as_ref()
            .is_some_and(|b| !doc.buffer_ahead() && b.doc_text() != doc.viewer.ed.session.text());
        if stale && let Some(b) = doc.buffer.take() {
            retired.push(b.id);
        }
    }

    fn open_dialog(&mut self) {
        let mut exts: Vec<&str> = MARKDOWN_EXTENSIONS.to_vec();
        exts.extend_from_slice(TEXT_EXTENSIONS);
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Markdown", MARKDOWN_EXTENSIONS)
            .add_filter("Plain text", TEXT_EXTENSIONS)
            .add_filter("All supported", &exts)
            .pick_file()
        {
            self.open(p);
        }
    }

    fn open_folder_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().pick_folder() {
            self.open(p);
        }
    }

    fn wants_dark(&self, ctx: &Context) -> bool {
        appearance_dark(ctx, self.themes.appearance())
    }

    fn resolve_theme(&mut self, ctx: &Context) {
        let dark = self.wants_dark(ctx);
        self.themes.update(dark);
        if self.theme_gen != self.themes.generation() {
            self.theme_gen = self.themes.generation();
            self.theme = self.themes.theme().clone();
            if let Some(d) = &self.diagram_renderer {
                d.set_font_size(self.theme.font_size);
            }
        }
        let fp = self.theme.fingerprint();
        if self.visuals_fp != Some(fp) {
            self.visuals_fp = Some(fp);
            ctx.set_visuals(visuals_for(&self.theme));
        }
    }

    fn poll_themes(&mut self, ctx: &Context) {
        for e in self.themes.poll() {
            self.theme_event(ctx, e);
        }
        // a debounced settings write is due later: wake up for it
        if let Some(t) = self.themes.save_deadline() {
            ctx.request_repaint_after(t.saturating_duration_since(std::time::Instant::now()));
        }
    }

    fn theme_event(&mut self, ctx: &Context, e: ThemeEvent) {
        match e {
            ThemeEvent::Fonts(f) => {
                self.install_fonts(ctx, crate::fonts::definitions_with(&f.defaults, &f.set));
                self.fonts_pending = true;
                ctx.request_repaint();
            }
            ThemeEvent::Installed(done) => {
                self.installed = Some(done);
                self.theme_report = true;
            }
            ThemeEvent::InstallCollision { src, existing } => {
                self.ask = Some(Ask::ReplaceTheme { src, existing });
            }
            ThemeEvent::Error(e) => self.banner = Some(Banner::Error(e)),
        }
    }

    /// Hands font definitions to egui (and to the parley backend when it is selected).
    fn install_fonts(&mut self, ctx: &Context, defs: egui::FontDefinitions) {
        if self.text_backend == crate::text_paint::TextBackend::Parley {
            match &mut self.parley {
                Some(p) => p.set_fonts(&defs),
                None => self.parley = Some(crate::text_paint::ParleyText::new(&defs)),
            }
        }
        ctx.set_fonts(defs);
    }

    fn import_theme_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Import Theme")
            .add_filter("CSS theme", &["css"])
            .pick_file()
        {
            self.themes.install(p);
        }
    }

    fn open_theme_folder(&mut self) {
        match self.themes.user_dir().map(Path::to_path_buf) {
            Some(d) => {
                if let Err(e) = crate::themes::open_folder(&d) {
                    self.banner = Some(Banner::Error(format!("Cannot open {}: {e}", d.display())));
                }
            }
            None => self.banner = Some(Banner::Info("No user theme folder in this session.".into())),
        }
    }

    fn update_title(&mut self, ctx: &Context) {
        let title = match &self.doc {
            Some(d) => {
                let name = d
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{name}{} — Markise", if self.is_dirty() { " •" } else { "" })
            }
            None => "Markise".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn open_link(&mut self, ctx: &Context, target: &str) {
        let base = self.doc.as_ref().and_then(|d| d.path.parent().map(Path::to_path_buf));
        match classify_link(target, base.as_deref()) {
            LinkAction::Open(u) => ctx.open_url(egui::OpenUrl::new_tab(u)),
            LinkAction::Confirm(u) => self.ask = Some(Ask::Link(u)),
            LinkAction::Anchor(a) => {
                if let Some(d) = &mut self.doc {
                    d.viewer.scroll_to_anchor(&a);
                }
            }
            LinkAction::LocalDoc(p) if p.is_file() => self.open(p),
            LinkAction::LocalDoc(p) | LinkAction::LocalOther(p) => {
                self.banner = Some(Banner::Info(format!("Not opened: {}", p.display())));
            }
        }
    }

    /// Runs one frame in the root `Ui` of a pass (`Context::run_ui`, `eframe::App::ui`).
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        if self.ctx.is_none() {
            let c = ctx.clone();
            self.wake = Some(Arc::new(move || c.request_repaint()));
            self.ctx = Some(ctx.clone());
            if let Some(w) = self.wake.clone() {
                self.highlighter = Some(Arc::new(crate::highlight::CodeHighlighter::new(w.clone())));
                self.diagram_renderer = Some(Arc::new(crate::diagram::DiagramRenderer::new(w, self.theme.font_size)));
                let h = self.view_highlights();
                let dg = self.view_diagrams();
                if let Some(d) = self.doc.as_mut() {
                    d.viewer.ed.view.set_highlights(h);
                    d.viewer.ed.view.set_diagrams(dg);
                }
            }
            if let Some(w) = self.wake.clone() {
                self.watcher = FileWatcher::new(w.clone()).ok();
                self.themes.start(w);
            }
            // first frame: wait briefly for the settings and the selected theme (no flash of
            // the default theme)
            let c = ctx.clone();
            for e in self.themes.wait_ready(&|a| appearance_dark(&c, a), FIRST_THEME_WAIT) {
                self.theme_event(ctx, e);
            }
        }
        // (the user theme folder is created, canonicalized and watched by the theme worker)
        if !crate::fonts::installed(ctx) {
            // faces were loaded by the theme worker (no font file access on the UI thread);
            // until they arrive egui's bundled fonts are used
            let defs = match &self.themes.last_fonts {
                Some(f) => crate::fonts::definitions_with(&f.defaults, &f.set),
                None => crate::fonts::definitions_with(&Default::default(), &Default::default()),
            };
            self.install_fonts(ctx, defs);
            self.font_gen += 1;
            ctx.request_repaint();
            return; // fonts apply at the next pass
        }
        if std::mem::take(&mut self.fonts_pending) {
            // the definitions set last pass are active from this pass on
            self.font_gen += 1;
        }
        self.frames += 1;
        self.timing.frames += 1;
        if ctx.input(|i| i.viewport().close_requested()) && !self.request_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        for p in std::mem::take(&mut self.pending_open) {
            self.open(p);
        }
        self.poll_loading();
        for id in std::mem::take(&mut self.retired_editors) {
            ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(id));
        }
        self.poll_rebuild();
        self.poll_save();
        self.poll_watcher();
        self.poll_themes(ctx);
        self.export.poll();
        if let Some(d) = &mut self.doc {
            d.sync();
        }
        // Side by Side updates the model synchronously and incrementally (side_by_side.rs);
        // only a failed update falls back to the background rebuild (which reports the error)
        if !self.source_visible
            && (!self.side_by_side || self.doc.as_ref().is_some_and(|d| d.incr_failed_rev == Some(d.text_rev)))
        {
            self.ensure_model();
        }
        if self.images.poll(ctx)
            && let Some(d) = &mut self.doc
        {
            d.viewer.ed.view.images_changed();
        }
        self.images.ensure_math(ctx);
        if self.images.poll_math()
            && let Some(d) = &mut self.doc
        {
            d.viewer.ed.view.math_changed();
        }
        if let (Some(t), Some(w)) = (&mut self.tree, &self.wake) {
            t.poll(w);
        }
        self.resolve_theme(ctx);
        // the settings file (read in the background) may switch diagrams
        if self.themes.diagrams() != self.diagrams {
            self.apply_diagrams(self.themes.diagrams());
        }
        self.apply_key_bindings();
        if let Some(d) = self.doc.as_mut() {
            let p = self.themes.editing().to_prefs();
            if *d.viewer.ed.session.prefs() != p {
                d.viewer.ed.session.set_prefs(p);
            }
        }

        // ---- shortcuts (consumed before widgets see them). The application commands' own
        // chords first (Open Folder ⇧⌘O before ⌘O, Side by Side ⌃⌘/ before ⌘/): they carry more
        // modifiers than the registry host commands below.
        self.app_shortcuts(ctx);
        // host commands (Save, Open…, Source Code Mode, Toggle Sidebar) go through the effective
        // keymap, so `keyBinding` overrides rebind, unbind or take over their chords
        // (performed through the menu dispatch, exactly like choosing the menu item)
        for id in self.take_host_shortcuts(ctx) {
            self.menu_command(ctx, id, crate::menu::Trigger::Shortcut);
        }

        self.menus_ui(ui);
        self.status_bar(ui);
        if self.sidebar_visible {
            self.sidebar(ui);
        }
        self.central(ui);
        self.theme_report_window(ctx);
        self.font_window(ctx);
        self.about.show(ctx, self.commit.as_deref());
        self.dialogs(ctx);
        self.export_dialogs(ctx);
        self.update_title(ctx);
        self.sync_native_menu();
        self.after_frame(ctx);
    }

    fn dialogs(&mut self, ctx: &Context) {
        let Some(p) = &self.ask else { return };
        let name = self
            .doc
            .as_ref()
            .and_then(|d| d.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("markise-confirm")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            match p {
                Ask::ReplaceTheme { src, existing } => {
                    let name = src
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.label(RichText::new(format!("A theme named “{name}” is already installed.")).strong());
                    ui.label("Replace it? These will be replaced:");
                    for p in existing {
                        ui.weak(
                            p.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Replace").clicked() {
                            choice = Some("replace");
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some("cancel");
                        }
                    });
                }
                Ask::Link(url) => {
                    ui.label(RichText::new("Open this link with another application?").strong());
                    ui.label(url);
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            choice = Some("open");
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some("cancel");
                        }
                    });
                }
                _ => {
                    ui.label(RichText::new(format!("“{name}” has unsaved changes.")).strong());
                    ui.label("Save them before continuing?");
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            choice = Some("save");
                        }
                        if ui.button("Discard").clicked() {
                            choice = Some("discard");
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some("cancel");
                        }
                    });
                }
            }
        });
        match (choice, self.ask.take()) {
            (Some("replace"), Some(Ask::ReplaceTheme { src, .. })) => self.themes.install_replacing(src),
            (Some("open"), Some(Ask::Link(url))) => {
                if let Some(c) = &self.ctx {
                    c.open_url(egui::OpenUrl::new_tab(url));
                }
            }
            (Some("discard"), Some(Ask::Unsaved(then))) => self.perform(then),
            (Some("save"), Some(Ask::Unsaved(then))) => self.start_save(SaveRequest { expect: None, then }),
            (Some("cancel"), _) => {}
            (_, ask) => self.ask = ask,
        }
    }

    /// Consumes this frame's key presses bound to host commands in the effective keymap (the
    /// document's, else the app's) and returns those command ids.
    fn take_host_shortcuts(&mut self, ctx: &Context) -> Vec<&'static str> {
        let platform = markise_engine::editor::keymap::Platform::current();
        let km = match &self.doc {
            Some(d) => d.viewer.keymap(),
            None => &self.host_keymap,
        };
        let registry = markise_engine::editor::command::Registry::new();
        let mut out = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = e
                else {
                    return true;
                };
                let Some(chord) = crate::viewer::chord_of(*key, *modifiers, platform) else {
                    return true;
                };
                let Some(&id) = km.lookup(&chord).first() else {
                    return true;
                };
                let host = registry
                    .get(id)
                    .is_some_and(|c| matches!(c.run, markise_engine::editor::command::Run::Host));
                if host {
                    out.push(id);
                }
                !host
            });
        });
        out
    }

    /// The keymap in effect (the document's, else the app's): shortcuts and menu chords.
    pub(crate) fn effective_keymap(&self) -> &markise_engine::editor::keymap::Keymap {
        match &self.doc {
            Some(d) => d.viewer.keymap(),
            None => &self.host_keymap,
        }
    }

    /// Test hook: `keyBinding` overrides used instead of the settings file's.
    pub fn set_key_bindings_override(&mut self, b: Option<Vec<(String, String)>>) {
        self.key_bindings_override = b;
    }

    /// Applies the settings file's `keyBinding` overrides to the app's and the open document's
    /// keymaps when they (or the document) changed; problems go to the log.
    fn apply_key_bindings(&mut self) {
        let cur = self
            .key_bindings_override
            .clone()
            .unwrap_or_else(|| self.themes.key_bindings());
        if self.host_bindings_applied.as_ref() != Some(&cur) {
            let platform = markise_engine::editor::keymap::Platform::current();
            let reg = markise_engine::editor::command::Registry::for_format(&markise_md::MarkdownFormat::default());
            let mut km = markise_engine::editor::keymap::Keymap::new(&reg, platform);
            let _ = km.apply_overrides(&reg, &cur, platform);
            self.host_keymap = km;
            self.host_bindings_applied = Some(cur.clone());
        }
        let Some(d) = self.doc.as_mut() else { return };
        if d.key_bindings_applied.as_ref() == Some(&cur) {
            return;
        }
        for p in d.viewer.set_key_bindings(&cur) {
            eprintln!("markise: {p}");
        }
        d.key_bindings_applied = Some(cur);
    }

    /// Runs registry command `id` on the document (menus; tests). Not while the source
    /// buffer is ahead of the session or in source mode. In Side by Side the left pane shows
    /// the session's text, so a command that changed it rewrites the source buffer (its undo
    /// history is kept) — both panes stay equal.
    pub fn run_command(&mut self, ctx: &Context, id: &str) -> bool {
        let Some(d) = self.doc.as_mut() else { return false };
        if d.buffer_ahead() || self.source_visible {
            return false;
        }
        let ok = d
            .viewer
            .run(ctx, self.parley.as_mut(), id, &markise_doc::Params::new())
            .is_ok();
        d.sync();
        if self.side_by_side
            && let Some(b) = d.buffer.as_mut()
            && b.doc_text() != d.viewer.ed.session.text()
        {
            b.set_doc_text(d.viewer.ed.session.text());
        }
        ok
    }

    /// "Font & size": user overrides of body font, base size, line height and content width
    /// on top of the selected style; applied live and persisted.
    fn font_window(&mut self, ctx: &Context) {
        if !self.font_window {
            return;
        }
        let mut open = true;
        let mut o = self.themes.overrides().clone();
        let before = o.clone();
        let t = &self.theme;
        // the style's own values (shown while not overridden)
        let style_font = t.fonts.body.iter().find(|f| Some(*f) != o.body_font.as_ref()).cloned();
        egui::Window::new("Font & size")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("font-overrides")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Body font");
                        let mut name = o.body_font.clone().unwrap_or_default();
                        let hint = style_font.clone().unwrap_or_else(|| "style default".into());
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut name)
                                    .hint_text(hint)
                                    .desired_width(200.0),
                            )
                            .changed()
                        {
                            o.body_font = (!name.trim().is_empty()).then_some(name);
                        }
                        ui.end_row();
                        let row = |ui: &mut egui::Ui,
                                   label: &str,
                                   v: &mut Option<f32>,
                                   current: f32,
                                   (lo, hi): (f32, f32),
                                   step: f64| {
                            ui.label(label);
                            ui.horizontal(|ui| {
                                let mut on = v.is_some();
                                if ui.checkbox(&mut on, "").on_hover_text("Override the style").changed() {
                                    *v = on.then_some(current);
                                }
                                let mut x = v.unwrap_or(current);
                                if ui
                                    .add_enabled(on, egui::Slider::new(&mut x, lo..=hi).step_by(step))
                                    .changed()
                                {
                                    *v = Some(x);
                                }
                            });
                            ui.end_row();
                        };
                        row(
                            ui,
                            "Base size",
                            &mut o.font_size,
                            t.font_size,
                            Overrides::FONT_SIZE,
                            0.5,
                        );
                        row(
                            ui,
                            "Line height",
                            &mut o.line_height,
                            t.line_height,
                            Overrides::LINE_HEIGHT,
                            0.05,
                        );
                        row(
                            ui,
                            "Content width",
                            &mut o.content_width,
                            t.column_max_width,
                            (Overrides::CONTENT_WIDTH.0, 1600.0),
                            10.0,
                        );
                    });
                ui.separator();
                if ui
                    .add_enabled(!o.is_empty(), egui::Button::new("Use the style's values"))
                    .clicked()
                {
                    o = Overrides::default();
                }
            });
        if o != before {
            self.themes.set_overrides(o);
        }
        if !open {
            self.font_window = false;
        }
    }

    /// The import report of the active (or just imported) theme: coverage, unsupported
    /// features, fonts.
    fn theme_report_window(&mut self, ctx: &Context) {
        if !self.theme_report {
            return;
        }
        let mut open = true;
        let installed = self.installed.as_ref();
        let current = self.themes.current_import().cloned();
        let fonts = self.themes.last_fonts.clone();
        let import_error = self.themes.import_error().map(str::to_string);
        let theme_name = self.theme.name.clone();
        let theme = &self.theme;
        let skipped_content = ctx.fonts_mut(|f| markise_layout::block::undrawable_generated(theme, f));
        egui::Window::new("Theme report")
            .open(&mut open)
            .default_width(460.0)
            .default_height(420.0)
            .vscroll(true)
            .show(ctx, |ui| {
                if let Some(done) = installed {
                    ui.label(
                        RichText::new(format!(
                            "Imported “{}”",
                            done.css.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                        ))
                        .strong(),
                    );
                    ui.label(format!(
                        "{} file{} copied to the theme folder{}.",
                        done.copied,
                        if done.copied == 1 { "" } else { "s" },
                        if done.replaced {
                            " (replaced the previous version)"
                        } else {
                            ""
                        }
                    ));
                    for (p, why) in done.skipped.iter().take(8) {
                        ui.weak(format!(
                            "not copied: {} — {why}",
                            p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                        ));
                    }
                    ui.separator();
                    report_ui(ui, &done.import);
                } else if let Some(r) = &current {
                    ui.label(RichText::new(&theme_name).strong());
                    report_ui(ui, r);
                } else if let Some(e) = import_error {
                    ui.colored_label(egui::Color32::from_rgb(0xc0, 0x39, 0x2b), e);
                } else {
                    ui.label(format!("{theme_name}: a native Markise theme (no CSS import)."));
                }
                if !skipped_content.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Generated content not drawn").strong());
                    ui.weak("no glyph in the theme's fonts (or an emoji, which egui cannot draw in color):");
                    for (at, text) in &skipped_content {
                        ui.label(format!("{at}: “{text}”"));
                    }
                }
                if let Some(f) = fonts {
                    ui.separator();
                    ui.label(RichText::new("Fonts").strong());
                    for (role, used) in &f.chosen {
                        ui.label(format!(
                            "{role}: {}",
                            if used.is_empty() {
                                "default".to_string()
                            } else {
                                used.join(", ")
                            }
                        ));
                    }
                    if !f.missing.is_empty() {
                        ui.weak(format!("not installed: {}", f.missing.join(", ")));
                    }
                }
            });
        if !open {
            self.theme_report = false;
            self.installed = None;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("markise-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(l) = &self.load {
                    ui.spinner();
                    ui.label(format!(
                        "Loading {}…",
                        l.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                    ));
                }
                if self.saving.is_some() {
                    ui.label("Saving…");
                }
                self.export.status(ui);
                if let Some(d) = &self.doc {
                    let m = d.viewer.model();
                    ui.weak(format!(
                        "{} · {} KB · {} blocks",
                        match self.view_mode() {
                            ViewMode::Source => "Source",
                            ViewMode::SideBySide => "Side by Side",
                            ViewMode::Rendered => "Preview",
                        },
                        d.viewer.ed.session.text().len() / 1024,
                        m.items.len()
                    ));
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let t = self.theme.clone();
        egui::Panel::left("markise-sidebar")
            .resizable(true)
            .default_size(240.0)
            .frame(egui::Frame::new().fill(color(t.chrome.sidebar_bg)).inner_margin(8.0))
            .show(ui, |ui| {
                let look = crate::sidebar::SideLook::new(&t);
                ui.horizontal(|ui| {
                    for (tab, label) in [(Tab::Files, "Files"), (Tab::Outline, "Outline")] {
                        let on = self.tab == tab;
                        let text = RichText::new(label)
                            .font(if on { look.bold_font.clone() } else { look.font.clone() })
                            .color(if on { look.active_text } else { look.text });
                        let r = ui.add(egui::Button::new(text).frame(false));
                        if on {
                            let y = r.rect.bottom() + 1.0;
                            ui.painter().line_segment(
                                [egui::pos2(r.rect.left(), y), egui::pos2(r.rect.right(), y)],
                                egui::Stroke::new(2.0, look.outline_active_text),
                            );
                        }
                        if r.clicked() {
                            self.tab = tab;
                        }
                    }
                });
                ui.add_space(2.0);
                ui.separator();
                match self.tab {
                    Tab::Files => {
                        let current = self.doc.as_ref().map(|d| d.path.clone());
                        let mut clicked = None;
                        if let (Some(tree), Some(wake)) = (self.tree.as_mut(), self.wake.as_ref()) {
                            clicked = tree.show(ui, current.as_deref(), wake, &look);
                        } else {
                            ui.weak("No folder open");
                            if ui.button("Open Folder…").clicked() {
                                self.open_folder_dialog();
                            }
                        }
                        if let Some(p) = clicked {
                            self.open(p);
                        }
                    }
                    Tab::Outline => {
                        if let Some(d) = &mut self.doc {
                            let model = d.viewer.model().clone();
                            let cur = model.current_heading(d.viewer.ed.view.anchor().0);
                            if let Some(item) = outline(ui, &model.outline, cur, &look) {
                                d.viewer.ed.view.scroll_to_item(item);
                                // Side by Side: the source pane follows the rendered pane
                                self.sbs.sync_source = true;
                                if self.source_visible {
                                    // leave source mode at the heading (preview shows it)
                                    self.toggle_source();
                                    if let Some(d) = &mut self.doc {
                                        d.viewer.ed.view.scroll_to_item(item);
                                    }
                                }
                            }
                        } else {
                            ui.weak("No document");
                        }
                    }
                }
            });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let t = self.theme.clone();
        // formatting toolbar (spec 04 §3.2): its drop-down keys are taken before the document
        // surface sees them; the bar itself is drawn on top afterwards
        let toolbar = self.toolbar_visible().then(|| self.toolbar_model(!self.side_by_side));
        let mut tool_cmd = None;
        match &toolbar {
            Some(m) => tool_cmd = self.toolbar.pre_input(ctx, m),
            None => self.toolbar.close(),
        }
        if let Some(id) = tool_cmd.take() {
            self.menu_command(ctx, id, crate::menu::Trigger::Click);
            ctx.request_repaint();
        }
        let mut tool_area = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(color(t.page_bg)))
            .show(ui, |ui| {
                self.banner_ui(ui, &t);
                if self.side_by_side && self.doc.is_some() {
                    tool_area = self.side_by_side_ui(ui, &t);
                    return;
                }
                tool_area = Some(ui.available_rect_before_wrap());
                if toolbar.is_some() && self.doc.is_some() && !self.source_visible {
                    ui.add_space(crate::toolbar::RESERVED_H);
                }
                if self.source_visible
                    && let Some(s) = self.doc.as_mut().and_then(|d| d.buffer.as_mut())
                {
                    if s.readonly {
                        ui.weak("Large file: source view is read-only in this version (M1).");
                    }
                    s.show(ui);
                    return;
                }
                match &mut self.doc {
                    Some(d) => {
                        d.viewer.set_font_generation(self.font_gen);
                        // the rich editor accepts edits once it has the buffer's text
                        let editable = !d.buffer_ahead();
                        let out = d.viewer.show(ui, &mut self.images, &t, self.parley.as_mut(), editable);
                        if out.edited {
                            d.sync();
                        }
                        if let Some(ms) = out.input_ms {
                            self.timing.log(&format!("keystroke handle+layout+paint={ms:.2}ms"));
                        }
                        // lazy fallback fonts: loaded (on the worker) only when the document
                        // first needs a glyph the primary fonts lack; the affected items are
                        // laid out again when they arrive (font generation bump)
                        if d.viewer.ed.view.missing_glyphs() && !self.themes.fallback_requested() {
                            self.themes.request_fallback();
                            ctx.request_repaint();
                        }
                        if let Some(link) = out.link {
                            self.open_link(ctx, &link);
                        }
                        if let Some((start, bytes)) = self.timing.open_started.take() {
                            self.timing.log(&format!(
                                "first-document-frame bytes={bytes} open_to_frame={:.1}ms painted={} laid_out={} rss_mb={}",
                                start.elapsed().as_secs_f64() * 1e3,
                                out.painted,
                                out.laid_out,
                                rss_mb()
                            ));
                            self.timing.idle_from = Some((Instant::now(), self.timing.frames));
                        }
                    }
                    None => {
                        tool_area = None;
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                // plain-text shortcuts: egui's default fonts lack ⇧
                                RichText::new(if self.menu_platform == markise_engine::editor::keymap::Platform::Mac {
                                    "Open a Markdown file (Cmd+O) or a folder (Shift+Cmd+O)"
                                } else {
                                    "Open a Markdown file (Ctrl+O) or a folder (Ctrl+Shift+O)"
                                })
                                    .color(color(t.meta_text))
                                    .size(16.0),
                            );
                        });
                    }
                }
            });
        if let (Some(m), Some(area)) = (&toolbar, tool_area) {
            let look = crate::toolbar::Look::new(&t);
            tool_cmd = self.toolbar.show(ui, area, m, &look);
        }
        if let Some(id) = tool_cmd {
            // exactly like the menu item (and its shortcut)
            self.menu_command(ctx, id, crate::menu::Trigger::Click);
            ctx.request_repaint();
        }
    }

    /// Side by Side panes (spec 04 §3.3, interim: the rendered pane is read-only). Returns the
    /// rendered pane's rectangle.
    fn side_by_side_ui(&mut self, ui: &mut egui::Ui, t: &Theme) -> Option<egui::Rect> {
        let ctx = ui.ctx().clone();
        let split = self.side_by_side_split();
        let show_toolbar = self.toolbar_visible();
        let doc = self.doc.as_mut()?;
        if doc.buffer.is_none() {
            // a new or reloaded document: the editor starts at the rendered position
            doc.buffer = Some(SourceState::new(doc.md(), 0));
            self.sbs.sync_source = true;
        }
        doc.viewer.set_font_generation(self.font_gen);
        let incremental = doc.incr_failed_rev != Some(doc.text_rev) && doc.failed_rev != Some(doc.text_rev);
        let behind = doc.model_rev != doc.text_rev && incremental;
        let (doc_id, text_rev) = (doc.id, doc.text_rev);
        let buffer = doc.buffer.as_mut().expect("buffer");
        let out = self.sbs.show(
            ui,
            buffer,
            &mut doc.viewer,
            crate::side_by_side::Frame {
                split,
                doc_id,
                text_rev,
                behind,
                incremental,
                images: &mut self.images,
                theme: t,
                parley: self.parley.as_mut(),
                highlighter: self.source_highlight.then_some(&mut self.source_highlighter),
                top_reserve: if show_toolbar { crate::toolbar::RESERVED_H } else { 0.0 },
            },
        );
        if out.typed {
            doc.text_rev += 1;
        }
        match out.applied {
            Some(Ok(_)) => {
                doc.model_rev = doc.text_rev;
                if let Some(ms) = out.apply_ms
                    && out.typed
                {
                    self.sbs_apply_ms.push(ms);
                }
            }
            Some(Err(e)) => {
                // the background rebuild takes over for this revision (ensure_model); it
                // reports an error itself if it fails too
                eprintln!("markise: incremental preview update failed ({e}); rebuilding");
                doc.incr_failed_rev = Some(doc.text_rev);
                ctx.request_repaint();
            }
            None => {}
        }
        if doc.viewer.ed.view.missing_glyphs() && !self.themes.fallback_requested() {
            self.themes.request_fallback();
            ctx.request_repaint();
        }
        if let Some(v) = out.split {
            self.themes.set_side_by_side_split(v);
        }
        if let Some(link) = out.link {
            self.open_link(&ctx, &link);
        }
        out.right
    }

    fn banner_ui(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let Some(b) = &self.banner else { return };
        enum Act {
            Dismiss,
            Reload,
            Keep,
            Overwrite,
            ReloadDiscard,
        }
        let mut act = None;
        egui::Frame::new()
            .fill(color(t.chrome.active_bg))
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| match b {
                    Banner::Error(e) => {
                        ui.colored_label(color(t.alert[4]), e);
                        if ui.button("Dismiss").clicked() {
                            act = Some(Act::Dismiss);
                        }
                    }
                    Banner::Info(m) => {
                        ui.label(m);
                        if ui.button("Dismiss").clicked() {
                            act = Some(Act::Dismiss);
                        }
                    }
                    Banner::Conflict(_) => {
                        ui.label("The file changed on disk and you have unsaved edits.");
                        if ui.button("Reload from disk").clicked() {
                            act = Some(Act::Reload);
                        }
                        if ui.button("Keep my edits").clicked() {
                            act = Some(Act::Keep);
                        }
                    }
                    Banner::SaveConflict { .. } => {
                        ui.label("Not saved: the file was changed on disk by another program.");
                        if ui.button("Overwrite").clicked() {
                            act = Some(Act::Overwrite);
                        }
                        if ui.button("Reload from disk (discard my edits)").clicked() {
                            act = Some(Act::ReloadDiscard);
                        }
                        if ui.button("Cancel").clicked() {
                            act = Some(Act::Dismiss);
                        }
                    }
                });
            });
        match act {
            Some(Act::Dismiss) | Some(Act::Keep) => self.banner = None,
            Some(Act::Reload) => {
                if let Some(Banner::Conflict(loaded)) = self.banner.take() {
                    self.accept_reload(*loaded);
                }
            }
            Some(Act::Overwrite) => {
                if let Some(Banner::SaveConflict { found }) = self.banner.take() {
                    let expect = found.map_or(Expect::Missing, Expect::Version);
                    self.start_save(SaveRequest {
                        expect: Some(expect),
                        then: Then::Nothing,
                    });
                }
            }
            Some(Act::ReloadDiscard) => {
                self.banner = None;
                if let Some(p) = self.doc.as_ref().map(|d| d.path.clone()) {
                    self.start_load(p, LoadReason::AcceptReload);
                }
            }
            None => {}
        }
    }

    /// "Reload from disk" in the conflict banner (tests).
    pub fn resolve_conflict_reload(&mut self) {
        if let Some(Banner::Conflict(loaded)) = self.banner.take() {
            self.accept_reload(*loaded);
        }
    }

    /// "Replace" in the replace-theme question (tests).
    pub fn confirm_replace_theme(&mut self) {
        if let Some(Ask::ReplaceTheme { src, .. }) = self.ask.take() {
            self.themes.install_replacing(src);
        }
    }

    /// "Overwrite" in the save-conflict banner (tests).
    pub fn resolve_save_conflict_overwrite(&mut self) {
        if let Some(Banner::SaveConflict { found }) = self.banner.take() {
            self.start_save(SaveRequest {
                expect: Some(found.map_or(Expect::Missing, Expect::Version)),
                then: Then::Nothing,
            });
        }
    }

    fn after_frame(&mut self, ctx: &Context) {
        if !self.timing.enabled {
            return;
        }
        if std::env::var_os("MARKISE_REPAINT_DEBUG").is_some() {
            let ev = ctx.input(|i| i.events.len());
            self.timing.log(&format!(
                "frame={} events={ev} causes={:?}",
                self.timing.frames,
                ctx.repaint_causes()
            ));
        }
        if !self.timing.first_frame_logged {
            self.timing.first_frame_logged = true;
            self.timing.log(&format!("first-frame-built rss_mb={}", rss_mb()));
            if self.pending_open.is_empty() && self.load.is_none() && self.doc.is_none() {
                self.timing.idle_from = Some((Instant::now(), self.timing.frames));
            }
        }
        if let (Some(secs), Some((since, frames))) = (self.timing.exit_after_idle, self.timing.idle_from) {
            let idle = since.elapsed().as_secs_f32();
            // frames in the first 0.5 s are the window settling and not counted
            if self.timing.frames > frames && idle > 0.5 {
                let had_input = ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving());
                if had_input {
                    self.timing.idle_input_frames += 1;
                } else {
                    self.timing.idle_unprompted_frames += 1;
                }
            }
            if idle + 0.03 >= secs {
                self.timing.log(&format!(
                    "idle-window secs={idle:.2} frames_total={} counted_after_0.5s: with_input={} unprompted={} (incl. the 1 scheduled exit wake-up) rss_mb={}",
                    self.timing.frames - frames,
                    self.timing.idle_input_frames,
                    self.timing.idle_unprompted_frames,
                    rss_mb()
                ));
                self.timing.exit_after_idle = None;
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f32(secs - idle));
            }
        }
    }
}

/// Sets egui's theme preference for the appearance and returns whether the result is dark.
fn appearance_dark(ctx: &Context, a: Appearance) -> bool {
    let pref = match a {
        Appearance::System => ThemePreference::System,
        Appearance::Light => ThemePreference::Light,
        Appearance::Dark => ThemePreference::Dark,
    };
    if ctx.options(|o| o.theme_preference) != pref {
        ctx.set_theme(pref);
    }
    ctx.theme() == egui::Theme::Dark
}

/// The import report summary (spec 03 §2.4 step 4).
fn report_ui(ui: &mut egui::Ui, r: &markise_theme::ImportResult) {
    let rep = &r.report;
    ui.label(format!(
        "Coverage {:.1}% — {} of {} declarations on modelled elements applied, {} deliberately ignored",
        rep.coverage() * 100.0,
        rep.applied,
        rep.content_declarations,
        rep.ignored
    ));
    ui.label(format!(
        "{} rules, {} selectors ({} matched) · {}",
        rep.rules,
        rep.selectors,
        rep.selectors_matched,
        match (rep.scheme_aware, r.theme.dark) {
            (true, _) => "light and dark variants (prefers-color-scheme)",
            (false, true) => "dark theme",
            (false, false) => "light theme",
        }
    ));
    if rep.font_faces > 0 {
        ui.weak(format!(
            "{} @font-face rule{}: font files are not loaded yet (installed fonts with the same family name are used)",
            rep.font_faces,
            if rep.font_faces == 1 { "" } else { "s" }
        ));
    }
    let list = |ui: &mut egui::Ui, title: &str, items: &[markise_theme::Issue]| {
        if items.is_empty() {
            return;
        }
        ui.label(RichText::new(title).strong());
        for i in items.iter().take(8) {
            let at = i.locations.first().map(|l| rep.describe(*l)).unwrap_or_default();
            ui.label(format!("{}× {}  {at}", i.count, i.name));
        }
        if items.len() > 8 {
            ui.weak(format!("… and {} more", items.len() - 8));
        }
    };
    list(ui, "Unsupported properties", &rep.unsupported_properties);
    list(ui, "Unsupported values", &rep.unsupported_values);
    list(ui, "Unsupported selectors", &rep.unsupported_selectors);
    list(ui, "Skipped at-rules", &rep.unsupported_at_rules);
    for w in rep.warnings.iter().take(8) {
        ui.colored_label(egui::Color32::from_rgb(0xb0, 0x7a, 0x12), w);
    }
}

/// egui visuals following the theme's chrome (spec 03 §2.2: colors only).
fn visuals_for(t: &Theme) -> egui::Visuals {
    let mut v = if t.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let chrome = &t.chrome;
    v.panel_fill = color(chrome.sidebar_bg);
    v.extreme_bg_color = color(t.page_bg);
    v.hyperlink_color = color(t.link);
    v.selection.bg_fill = color(chrome.selection_bg);
    if let Some(c) = t.selection_text {
        v.selection.stroke.color = color(c);
    }
    v.widgets.noninteractive.fg_stroke.color = color(chrome.control_text);
    v.widgets.inactive.fg_stroke.color = color(chrome.control_text);
    v.widgets.hovered.weak_bg_fill = color(chrome.active_bg);
    v.widgets.active.weak_bg_fill = color(chrome.active_bg);
    v
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

/// Resident memory of this process in MB (0 if unavailable).
pub fn rss_mb() -> usize {
    memory_stats::memory_stats().map_or(0, |m| m.physical_mem / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_only_open_web_schemes_without_asking() {
        let base = Path::new("/docs");
        assert_eq!(
            classify_link("https://example.com/x", Some(base)),
            LinkAction::Open("https://example.com/x".into())
        );
        assert_eq!(
            classify_link("HTTP://a", Some(base)),
            LinkAction::Open("HTTP://a".into())
        );
        assert_eq!(
            classify_link("mailto:a@b.c", Some(base)),
            LinkAction::Open("mailto:a@b.c".into())
        );
        for t in [
            "vscode://file/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "smb://host/share",
        ] {
            assert_eq!(classify_link(t, Some(base)), LinkAction::Confirm(t.into()), "{t}");
        }
        assert_eq!(classify_link("#intro", Some(base)), LinkAction::Anchor("#intro".into()));
        assert_eq!(
            classify_link("other.md#sec", Some(base)),
            LinkAction::LocalDoc(PathBuf::from("/docs/other.md"))
        );
        assert_eq!(
            classify_link("tool.sh", Some(base)),
            LinkAction::LocalOther(PathBuf::from("/docs/tool.sh"))
        );
        // a Windows drive letter is a path, not a scheme
        assert!(matches!(classify_link("C:/x/readme.md", None), LinkAction::LocalDoc(_)));
    }
}
