//! `markise-ui-egui` — the egui front end (layer L6): document viewer over
//! `markise-layout`, sidebar (file tree, outline), source-code mode, app shell.

pub mod about;
pub mod app;
pub mod block_edit;
pub mod block_snap;
pub mod clipboard;
pub mod confirm;
pub mod diagram;
pub mod dialog;
pub mod export_ui;
pub mod fonts;
pub mod highlight;
pub mod icons;
pub mod images;
pub mod menu;
pub mod menu_sync;
pub mod prefs;
pub mod side_by_side;
pub mod sidebar;
pub mod source;
pub mod text_paint;
pub mod themes;
pub mod toolbar;
pub mod viewer;
#[doc(hidden)]
pub mod wake_track;
pub mod word_count;

pub use app::{AppOptions, MarkiseApp, MenuHost, ThemeChoice, ViewMode, rss_mb};
pub use markise_engine::settings::Appearance;
pub use text_paint::TextBackend;
pub use themes::ConfigDir;
