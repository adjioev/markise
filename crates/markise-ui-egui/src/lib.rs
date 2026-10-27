//! `markise-ui-egui` — the egui front end (layer L6): document viewer over
//! `markise-layout`, sidebar (file tree, outline), source-code mode, app shell.

pub mod about;
pub mod app;
pub mod diagram;
pub mod export_ui;
pub mod fonts;
pub mod highlight;
pub mod icons;
pub mod images;
pub mod menu;
pub mod side_by_side;
#[cfg(test)]
mod side_by_side_tests;
pub mod sidebar;
pub mod source;
pub mod source_highlight;
pub mod text_paint;
pub mod themes;
pub mod viewer;

pub use app::{AppOptions, MarkiseApp, MenuHost, ThemeChoice, ViewMode, rss_mb};
pub use markise_engine::settings::Appearance;
pub use text_paint::TextBackend;
pub use themes::ConfigDir;
