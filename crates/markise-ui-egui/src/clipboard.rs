//! Clipboard seam for pastes egui does not deliver itself.
//!
//! Text and image data arrive as egui events ([`egui::Event::Paste`],
//! [`egui::Event::PasteImage`]; the backend reads the system clipboard). A file copied in a
//! file manager (Finder ⌘C) arrives only as its name in a text paste, so on a paste the app
//! asks this seam for the copied files first: the desktop app reads the system clipboard's file
//! list, tests script it, and without one ([`NoFiles`]) a paste is plain text.

use std::path::PathBuf;
use std::sync::Mutex;

/// The files on the clipboard (a file manager copy).
pub trait ClipboardFiles: Send + Sync {
    /// Absolute paths of the copied files; empty when the clipboard holds no files.
    fn files(&self) -> Vec<PathBuf>;
}

/// No file list (tests, platforms without one): pastes are text or image data only.
#[derive(Default)]
pub struct NoFiles;

impl ClipboardFiles for NoFiles {
    fn files(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// A scripted clipboard file list (tests): what [`Scripted::set`] put there.
#[derive(Default)]
pub struct Scripted(Mutex<Vec<PathBuf>>);

impl Scripted {
    pub fn set(&self, files: Vec<PathBuf>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = files;
    }
}

impl ClipboardFiles for Scripted {
    fn files(&self) -> Vec<PathBuf> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
