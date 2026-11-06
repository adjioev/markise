//! The integration tests of `markise-engine`, one test binary (issue #116): every file in this
//! folder is a module here. Add new integration tests as a module (`tests/it/<topic>.rs`
//! plus a `mod` line below), not as a new `tests/*.rs` file: each of those builds and
//! links its own executable.

mod block_check;
mod block_fields;
mod block_forms;
mod block_gaps;
mod block_inplace;
mod block_insert;
mod block_move;
mod block_ownership;
mod block_page;
mod block_place;
mod block_views;
mod callouts;
mod common;
mod directive_edit;
mod editor;
mod editor_props;
mod graphemes;
mod image_edit;
mod input_rules;
mod list_props;
mod lists;
mod paging;
mod practice;
mod progressive;
mod section_edit;
mod source;
mod structure;
mod surface;
mod table_operations;
