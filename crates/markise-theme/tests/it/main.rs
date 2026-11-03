//! The integration tests of `markise-theme`, one test binary (issue #116): every file in this
//! folder is a module here. Add new integration tests as a module (`tests/it/<topic>.rs`
//! plus a `mod` line below), not as a new `tests/*.rs` file: each of those builds and
//! links its own executable.

mod builtin;
mod corpus;
