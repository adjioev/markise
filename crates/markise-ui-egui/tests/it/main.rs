//! The integration tests of `markise-ui-egui`, one test binary (issue #116): every file in this
//! folder is a module here. Add new integration tests as a module (`tests/it/<topic>.rs`
//! plus a `mod` line below), not as a new `tests/*.rs` file: each of those builds and
//! links its own executable.

mod block_form;
mod block_gaps;
mod block_ids;
mod block_library;
mod block_packages;
mod block_views;
mod code_tools;
mod common;
mod common_html;
mod diagrams;
mod dialogs;
mod editing;
mod export;
mod files;
mod find;
mod full_view;
mod help;
mod highlight;
mod idle;
mod image_edit;
mod images_paste;
mod keyboard_zoom;
mod lifecycle;
mod lists_e2e;
mod menus;
mod open_latency;
mod preferences;
mod progressive_open;
mod qa_sweep;
mod review;
mod safety;
mod select_all;
mod side_by_side;
mod sidebar;
mod site_blocks;
mod smoke;
mod source_editor;
mod tables;
mod tasks_e2e;
mod themes;
mod toolbar;
mod updates;
mod vault;
mod watch_gaps;
mod word_count;
mod zoom;
