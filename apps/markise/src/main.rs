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
//!
//! Renderer: wgpu with a glow fallback (`renderer`; `MARKISE_RENDERER=glow` forces glow).
//! Frame pacing diagnostics: `MARKISE_FRAME_LOG` (`frame_log`, `docs/diagnostics.md`).

// Release builds on Windows are GUI-subsystem executables (no console window); terminal
// launches re-attach to the parent console (`diag::attach_parent_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod diag;
mod frame_log;
#[cfg(target_os = "macos")]
mod macos_open;
#[cfg(target_os = "macos")]
mod menu_tracking;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native_menu;
mod renderer;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use renderer::Renderer;

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
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        for path in macos_open::take_pending() {
            self.0.open(path);
        }
        let log = frame_log::start(ui, frame);
        self.0.ui(ui);
        if let Some(l) = log {
            l.finish(ui);
        }
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
        // the welcome tutorial at the first launch (issue #63)
        welcome: true,
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
    let native = |r: Renderer| {
        let mut native = eframe::NativeOptions {
            viewport: viewport.clone(),
            // repaint only on events (no continuous rendering), NFR-10
            run_and_return: false,
            ..Default::default()
        };
        renderer::configure(&mut native, r);
        native
    };
    // the app is created once, by whichever renderer starts (see `run`)
    let opts = Rc::new(RefCell::new(Some(opts)));
    let created = Rc::new(Cell::new(false));
    let creator = |r: Renderer| -> eframe::AppCreator<'static> {
        let (opts, created) = (opts.clone(), created.clone());
        let surface = {
            let n = native(r);
            format!(
                "present={:?} frame_latency={:?}",
                n.wgpu_options.surface.present_mode, n.wgpu_options.surface.desired_maximum_frame_latency
            )
        };
        // fonts are loaded by the app's theme worker (off the UI thread)
        Box::new(move |cc| {
            created.set(true);
            renderer::startup_log::stop();
            let desc = renderer::describe(r, cc);
            diag::log(&format!("renderer: {desc}"));
            frame_log::set_config(format!("renderer={desc} {surface}"));
            if timing {
                eprintln!(
                    "markise-timing: t={:.1}ms creator-called (window + renderer ready: {desc})",
                    start.elapsed().as_secs_f64() * 1e3
                );
            }
            #[cfg(target_os = "macos")]
            macos_open::set_context(cc.egui_ctx.clone());
            let opts = opts.borrow_mut().take().ok_or("the app was already created")?;
            #[allow(unused_mut)]
            let mut app = MarkiseApp::new(opts);
            // native menu bar on macOS and Windows; elsewhere the app shows its egui menu bar
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some(menu) = native_menu::NativeMenu::new(cc) {
                app.set_menu_host(Box::new(menu));
            }
            Ok(Box::new(App(app)))
        })
    };
    diag::log(&format!(
        "markise {} start ({}/{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    renderer::startup_log::install();
    let result = run(Renderer::from_env(), &native, &creator, &created);
    if let Err(e) = &result {
        // (main's Err return prints it to stderr as well)
        diag::log(&format!("fatal: {e}"));
    }
    result
}

/// Runs the app with renderer `first`; when wgpu fails to start (the app was never created),
/// once more with glow on the same event loop.
///
/// The event loop is created here rather than inside `run_native`: on macOS so the "open
/// documents" handler can be added to winit's application delegate before the app finishes
/// launching, everywhere so a second run can reuse it (winit allows one event loop per process).
fn run(
    first: Renderer,
    native: &dyn Fn(Renderer) -> eframe::NativeOptions,
    creator: &dyn Fn(Renderer) -> eframe::AppCreator<'static>,
    created: &Cell<bool>,
) -> eframe::Result {
    use winit::platform::run_on_demand::EventLoopExtRunOnDemand as _;

    let mut event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    #[cfg(target_os = "macos")]
    if !macos_open::install() {
        eprintln!("markise: could not install the macOS open-documents handler");
    }
    let mut r = first;
    loop {
        {
            let mut app = eframe::create_native("Markise", native(r), creator(r), &event_loop);
            event_loop.run_app_on_demand(&mut app)?;
        }
        if created.get() {
            return Ok(());
        }
        let why = renderer::startup_log::take();
        let why = if why.is_empty() {
            "no error reported".to_string()
        } else {
            why
        };
        if r == Renderer::Wgpu {
            let msg = format!("renderer: wgpu could not start ({why}); falling back to glow");
            eprintln!("markise: {msg}");
            diag::log(&msg);
            r = Renderer::Glow;
            continue;
        }
        return Err(eframe::Error::AppCreation(
            format!("the {} renderer could not start: {why}", r.name()).into(),
        ));
    }
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
