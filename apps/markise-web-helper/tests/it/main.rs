//! The real helper with the real web engine. macOS only: nothing to draw with elsewhere. No window
//! shows: the helper never orders one in.

#[cfg(target_os = "macos")]
mod hostile;
#[cfg(target_os = "macos")]
mod render;
#[cfg(target_os = "macos")]
mod support;
