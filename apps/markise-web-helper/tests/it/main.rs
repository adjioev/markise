//! The real helper with the real web engine. macOS only: nothing to draw with elsewhere. No window
//! shows: the helper never orders one in. The example package is checked everywhere and drawn on macOS.

#[cfg(target_os = "macos")]
mod client;
mod example_package;
#[cfg(target_os = "macos")]
mod hostile;
#[cfg(target_os = "macos")]
mod render;
#[cfg(target_os = "macos")]
mod support;
