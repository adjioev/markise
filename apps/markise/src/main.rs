//! Markise desktop app: `markise [<file|folder>…]`.
//!
//! Files opened from the OS file manager arrive as arguments on Windows and Linux; on macOS
//! Finder sends an Apple Event instead (see `macos_open`).
//!
//! Environment (measurement only):
//! - `MARKISE_TIMING=1` prints a timing log to stderr (startup, open-to-first-frame, RSS).
//! - `MARKISE_TIMING_EXIT=<secs>` (with timing) quits after the first document frame (or the
//!   first frame without a document) plus `<secs>` idle seconds and logs how many frames ran
//!   while idle (NFR-10).
//!
//! Diagnostics: start, panics and fatal errors are appended to a small log file
//! (`markise.log`; folder per platform in `diag`).
//!
//! Developer switch:
//! - `MARKISE_TEXT=parley` lays out and paints document text with parley (`markise-text`) and a
//!   vello_cpu glyph atlas instead of epaint (ADR 0005 T2/T3; interim quality, default off).

// Release builds on Windows are GUI-subsystem executables (no console window); terminal
// launches re-attach to the parent console (`diag::attach_parent_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod diag;
#[cfg(target_os = "macos")]
mod macos_open;
#[cfg(target_os = "macos")]
mod menu_tracking;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native_menu;

use std::path::PathBuf;
use std::time::Instant;

use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp};

/// Application id: Wayland app id / X11 class, matching the Linux desktop entry
/// (`packaging/linux/io.github.adjioev.markise.desktop`) so shells show the right icon.
const APP_ID: &str = "io.github.adjioev.markise";

/// Window/taskbar icon (the macOS .app bundle uses `assets/brand/markise.icns` instead).
const ICON_PNG: &[u8] = include_bytes!("../../../assets/brand/png/markise-256.png");

fn app_icon() -> Option<eframe::egui::IconData> {
    eframe::icon_data::from_png_bytes(ICON_PNG).ok()
}

struct App(MarkiseApp);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        for path in macos_open::take_pending() {
            self.0.open(path);
        }
        self.0.ui(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // pending (debounced) settings
        self.0.on_exit();
    }
}

/// The system clipboard's file list (a Finder / Explorer / file manager copy), read on paste.
struct SystemClipboardFiles;

impl markise_ui_egui::clipboard::ClipboardFiles for SystemClipboardFiles {
    fn files(&self) -> Vec<PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            arboard::Clipboard::new()
                .and_then(|mut c| c.get().file_list())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.is_absolute())
                .collect()
        }
        #[cfg(target_arch = "wasm32")]
        Vec::new()
    }
}

fn main() -> eframe::Result {
    let start = Instant::now();
    #[cfg(windows)]
    diag::attach_parent_console();
    diag::install_panic_hook();
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.iter().any(|a| a.as_os_str() == "--help" || a.as_os_str() == "-h") {
        println!("usage: markise [<file|folder>…]");
        return Ok(());
    }
    let timing = std::env::var_os("MARKISE_TIMING").is_some_and(|v| v != "0");
    let exit_after_idle = std::env::var("MARKISE_TIMING_EXIT").ok().and_then(|v| v.parse().ok());
    let opts = AppOptions {
        open: args,
        timing,
        exit_after_idle: if timing { exit_after_idle } else { None },
        start,
        system_fonts: true,
        text_backend: markise_ui_egui::TextBackend::from_env(),
        commit: option_env!("MARKISE_COMMIT").map(String::from),
        // settings file and user themes in the platform config folder (ADR 0004)
        config: markise_theme::discover::config_dir().map_or(ConfigDir::None, ConfigDir::Dir),
        // yes/no questions as the platform's alerts (issue #35)
        confirm: markise_ui_egui::confirm::native(),
        // a file manager copy pasted into the document (issue #51)
        clipboard_files: std::sync::Arc::new(SystemClipboardFiles),
        ..Default::default()
    };
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Markise")
        .with_app_id(APP_ID)
        .with_inner_size([1100.0, 780.0])
        .with_min_inner_size([420.0, 300.0]);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native = eframe::NativeOptions {
        viewport,
        // repaint only on events (no continuous rendering), NFR-10
        run_and_return: false,
        ..Default::default()
    };
    // fonts are loaded by the app's theme worker (off the UI thread)
    let creator: eframe::AppCreator<'static> = Box::new(move |cc| {
        if timing {
            eprintln!(
                "markise-timing: t={:.1}ms creator-called (window + GL context ready)",
                start.elapsed().as_secs_f64() * 1e3
            );
        }
        #[cfg(target_os = "macos")]
        macos_open::set_context(cc.egui_ctx.clone());
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = cc;
        #[allow(unused_mut)]
        let mut app = MarkiseApp::new(opts);
        // native menu bar on macOS and Windows; elsewhere the app shows its egui menu bar
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(menu) = native_menu::NativeMenu::new(cc) {
            app.set_menu_host(Box::new(menu));
        }
        Ok(Box::new(App(app)))
    });
    diag::log(&format!(
        "markise {} start ({}/{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let result = run(native, creator);
    if let Err(e) = &result {
        // (main's Err return prints it to stderr as well)
        diag::log(&format!("fatal: {e}"));
    }
    result
}

#[cfg(not(target_os = "macos"))]
fn run(native: eframe::NativeOptions, creator: eframe::AppCreator<'static>) -> eframe::Result {
    eframe::run_native("Markise", native, creator)
}

/// macOS: the event loop is created here (not inside `run_native`) so the "open documents"
/// handler can be added to winit's application delegate before the app finishes launching.
#[cfg(target_os = "macos")]
fn run(native: eframe::NativeOptions, creator: eframe::AppCreator<'static>) -> eframe::Result {
    let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    if !macos_open::install() {
        eprintln!("markise: could not install the macOS open-documents handler");
    }
    let mut app = eframe::create_native("Markise", native, creator, &event_loop);
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_icon_decodes() {
        let icon = super::app_icon().expect("embedded icon PNG decodes");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
