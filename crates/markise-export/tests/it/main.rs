//! The integration tests of `markise-export`, one test binary (issue #116): every file in this
//! folder is a module here. Add new integration tests as a module (`tests/it/<topic>.rs`
//! plus a `mod` line below), not as a new `tests/*.rs` file: each of those builds and
//! links its own executable.

mod adversarial;
mod conformance;
mod critic;
mod diagrams;
mod fuzz;
mod highlight;
mod html_export;
mod pandoc;
