//! The integration tests of `markise-md`, one test binary (issue #116): every file in this
//! folder is a module here. Add new integration tests as a module (`tests/it/<topic>.rs`
//! plus a `mod` line below), not as a new `tests/*.rs` file: each of those builds and
//! links its own executable.

mod adversarial;
mod api_fuzz;
mod block_owner;
mod code_fence;
mod critic;
mod critic_edit;
mod directive;
mod edit_format;
mod fastpath;
mod hints;
mod language;
mod nested;
mod regressions;
mod roundtrip;
mod spec01;
mod wikilinks;
