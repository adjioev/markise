//! Renderer setup (issue #86): wgpu (Metal on macOS, Vulkan / DX12 / GL elsewhere) by default,
//! glow (OpenGL) as the fallback.
//!
//! - wgpu presents with `AutoVsync` and a maximum frame latency of 1: smooth scrolling asks for
//!   a frame every display refresh while the content moves (120 Hz on ProMotion displays), the
//!   present call paces it, and the frame shown is the newest one. The low-power adapter is
//!   preferred (integrated GPU on dual-GPU machines).
//! - If wgpu cannot start (no usable adapter or surface), the app starts again with glow in the
//!   same process; the reason goes to the diagnostics log. The app is not created before the
//!   renderer is ready, so nothing is lost.
//!
//! Environment (diagnostics):
//! - `MARKISE_RENDERER=glow|wgpu`: force a renderer (`glow` also skips the wgpu attempt).
//! - `MARKISE_PRESENT=fifo|mailbox|immediate|autovsync|autonovsync`: wgpu present mode.
//! - `MARKISE_FRAME_LATENCY=<n>`: wgpu maximum frame latency.
//! - wgpu's own `WGPU_BACKEND`, `WGPU_POWER_PREF`, `WGPU_ADAPTER_NAME` apply as well
//!   (`WGPU_BACKEND=vulkan` on macOS has no adapter: a way to try the glow fallback).

use eframe::egui_wgpu::WgpuSetup;
use eframe::wgpu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    Wgpu,
    Glow,
}

impl Renderer {
    /// The renderer to try first: wgpu unless `MARKISE_RENDERER=glow`.
    pub fn from_env() -> Self {
        Self::parse(std::env::var("MARKISE_RENDERER").ok().as_deref())
    }

    fn parse(v: Option<&str>) -> Self {
        match v {
            Some("glow") => Renderer::Glow,
            _ => Renderer::Wgpu,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Renderer::Wgpu => "wgpu",
            Renderer::Glow => "glow",
        }
    }
}

/// Puts `renderer` and its settings into `native`.
pub fn configure(native: &mut eframe::NativeOptions, renderer: Renderer) {
    native.renderer = match renderer {
        Renderer::Wgpu => eframe::Renderer::Wgpu,
        Renderer::Glow => eframe::Renderer::Glow,
    };
    let surface = &mut native.wgpu_options.surface;
    surface.present_mode = present_mode(std::env::var("MARKISE_PRESENT").ok().as_deref());
    surface.desired_maximum_frame_latency = Some(
        std::env::var("MARKISE_FRAME_LATENCY")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1),
    );
    if let WgpuSetup::CreateNew(setup) = &mut native.wgpu_options.wgpu_setup {
        // an editor needs no discrete GPU (battery); WGPU_POWER_PREF still overrides it
        setup.power_preference = wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower);
    }
}

fn present_mode(v: Option<&str>) -> wgpu::PresentMode {
    use wgpu::PresentMode as P;
    match v {
        Some("fifo") => P::Fifo,
        Some("mailbox") => P::Mailbox,
        Some("immediate") => P::Immediate,
        Some("autonovsync") => P::AutoNoVsync,
        _ => P::AutoVsync,
    }
}

/// One line describing the renderer that started (diagnostics log, timing, frame log).
pub fn describe(native_renderer: Renderer, cc: &eframe::CreationContext<'_>) -> String {
    match (native_renderer, cc.wgpu_render_state.as_ref()) {
        (Renderer::Wgpu, Some(rs)) => {
            let info = rs.adapter.get_info();
            format!("wgpu {:?} ({}, {:?})", info.backend, info.name, info.device_type)
        }
        (r, _) => r.name().to_string(),
    }
}

/// Error messages logged (through the `log` crate) while the renderer starts, so a failed wgpu
/// start says why in the diagnostics log. Only error-level records, and only until the app is
/// created: wgpu's own errors while running stay out of the log file.
pub mod startup_log {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CAPTURING: AtomicBool = AtomicBool::new(true);
    static ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;

    impl log::Log for Capture {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Error && CAPTURING.load(Ordering::Relaxed)
        }

        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                let mut e = ERRORS.lock().unwrap_or_else(|e| e.into_inner());
                if e.len() < 8 {
                    e.push(format!("{}: {}", r.target(), r.args()));
                }
            }
        }

        fn flush(&self) {}
    }

    static LOGGER: Capture = Capture;

    /// Installs the capture (once, before the event loop; a logger set elsewhere wins).
    pub fn install() {
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Error);
        }
    }

    /// The app was created: stop capturing.
    pub fn stop() {
        CAPTURING.store(false, Ordering::Relaxed);
        log::set_max_level(log::LevelFilter::Off);
    }

    /// The errors captured so far, joined (empty when none).
    pub fn take() -> String {
        std::mem::take(&mut *ERRORS.lock().unwrap_or_else(|e| e.into_inner())).join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wgpu_is_the_default_glow_on_request() {
        assert_eq!(Renderer::parse(None), Renderer::Wgpu);
        assert_eq!(Renderer::parse(Some("wgpu")), Renderer::Wgpu);
        assert_eq!(Renderer::parse(Some("glow")), Renderer::Glow);
        assert_eq!(Renderer::parse(Some("other")), Renderer::Wgpu);
    }

    #[test]
    fn wgpu_presents_with_vsync_and_one_frame_of_latency() {
        let mut native = eframe::NativeOptions::default();
        configure(&mut native, Renderer::Wgpu);
        assert_eq!(native.renderer, eframe::Renderer::Wgpu);
        // (unless the environment overrides them)
        if std::env::var_os("MARKISE_PRESENT").is_none() {
            assert_eq!(native.wgpu_options.surface.present_mode, wgpu::PresentMode::AutoVsync);
        }
        if std::env::var_os("MARKISE_FRAME_LATENCY").is_none() {
            assert_eq!(native.wgpu_options.surface.desired_maximum_frame_latency, Some(1));
        }
        configure(&mut native, Renderer::Glow);
        assert_eq!(native.renderer, eframe::Renderer::Glow);
        assert_eq!(present_mode(Some("mailbox")), wgpu::PresentMode::Mailbox);
    }
}
