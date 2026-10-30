//! `markise-engine` — document session services (layer L4) used by the front end:
//! the headless editing session with transactions, undo and the command registry ([`editor`],
//! ADR 0006), background file read + parse + view-model build ([`load`]), atomic save ([`save`]),
//! lazy folder listing with natural sort ([`tree`]), file and folder operations of the
//! sidebar ([`fsops`]), file watching ([`watch`]), the app
//! settings file ([`settings`]) and export jobs ([`export`]).
//! Nothing here runs on the UI thread except cheap channel polls.

pub mod attach;
pub mod edit;
pub mod editor;
pub mod export;
pub mod fsops;
pub mod jobs;
pub mod load;
pub mod save;
pub mod settings;
pub mod tree;
pub mod vault;
#[doc(hidden)]
pub mod wake_track;
pub mod watch;

/// Callback that wakes the UI (e.g. `egui::Context::request_repaint`).
pub type Wake = std::sync::Arc<dyn Fn() + Send + Sync>;
