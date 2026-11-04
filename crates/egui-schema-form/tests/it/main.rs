//! The integration tests of `egui-schema-form`, one test binary (issue #116). All headless:
//! `egui_kittest` drives the form through its accessibility tree, so what a screen reader finds
//! is also what the tests find.

mod common;
mod form;
mod media;
