//! The integration tests of `markise-layout`, one test binary (issue #116): every file in this
//! folder is a module here. Add new integration tests as a module (`tests/it/<topic>.rs`
//! plus a `mod` line below), not as a new `tests/*.rs` file: each of those builds and
//! links its own executable.

mod common_html;
mod diagram;
mod font_fallback;
mod highlight;
mod incremental;
mod math;
mod native;
mod paging;
mod seam;
mod sections;
mod segments;
mod site;
mod source_view;
mod theme;
mod view;
