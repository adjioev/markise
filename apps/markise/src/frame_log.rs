//! `MARKISE_FRAME_LOG=1` (or a file path) writes one CSV line per frame — timing, wheel input
//! received and applied, repaint request — to `markise-frames-<pid>.csv` in the temporary folder
//! (or the path given), for checking frame pacing and scroll smoothness on a real display
//! (issue #86). Off by default; see `docs/diagnostics.md` for the columns.

use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

struct Log {
    out: std::io::BufWriter<std::fs::File>,
    t0: Instant,
    last: Option<Instant>,
    frame: u64,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);
static CONFIG: Mutex<String> = Mutex::new(String::new());
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The renderer description for the log header.
pub fn set_config(c: String) {
    *lock(&CONFIG) = c;
}

/// The log file, when `MARKISE_FRAME_LOG` asks for one.
fn path() -> Option<std::path::PathBuf> {
    let v = std::env::var_os("MARKISE_FRAME_LOG")?;
    if v.is_empty() || v == "0" {
        None
    } else if v == "1" {
        Some(std::env::temp_dir().join(format!("markise-frames-{}.csv", std::process::id())))
    } else {
        Some(v.into())
    }
}

/// CPU time of the calling thread (ms; NaN where not measured).
fn thread_cpu_ms() -> f64 {
    #[cfg(unix)]
    {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid pointer to a timespec; the clock exists on macOS and Linux.
        let r = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        if r == 0 {
            return ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 / 1e6;
        }
    }
    f64::NAN
}

/// Refresh rate the main screen supports (macOS: `NSScreen.maximumFramesPerSecond`; 0 where
/// unknown).
fn display_fps() -> isize {
    #[cfg(target_os = "macos")]
    {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        // SAFETY: called on the main thread (the UI); `mainScreen` may be nil.
        unsafe {
            let s: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
            if s.is_null() {
                return 0;
            }
            msg_send![s, maximumFramesPerSecond]
        }
    }
    #[cfg(not(target_os = "macos"))]
    0
}

/// A frame being logged.
pub struct Frame {
    start: Instant,
    cpu0: f64,
    input_events: usize,
    predicted_dt: f32,
    prev_cpu: Option<f32>,
}

/// Before the app's frame (`None` when logging is off).
pub fn start(ui: &eframe::egui::Ui, frame: &eframe::Frame) -> Option<Frame> {
    if FAILED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut g = lock(&LOG);
    if g.is_none() {
        let path = path()?;
        let Ok(file) = std::fs::File::create(&path) else {
            eprintln!("markise: cannot write the frame log {}", path.display());
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            return None;
        };
        let mut out = std::io::BufWriter::new(file);
        let _ = writeln!(
            out,
            "# markise frame log: {} | display max fps {} | ppp {} | MARKISE_SCROLL_SMOOTH={:?} MARKISE_SCROLL_SNAP={:?}",
            lock(&CONFIG),
            display_fps(),
            ui.ctx().pixels_per_point(),
            std::env::var("MARKISE_SCROLL_SMOOTH").ok(),
            std::env::var("MARKISE_SCROLL_SNAP").ok(),
        );
        let _ = writeln!(
            out,
            "frame,t_ms,interval_ms,ui_wall_ms,ui_cpu_ms,prev_frame_cpu_ms,predicted_dt_ms,input_events,wheel_events,momentum_events,phases,delta_raw_pt,delta_egui_smooth_pt,delta_applied_pt,pending_pt,scroll_y,total_h,repaint_requested,input_driven"
        );
        let _ = out.flush();
        eprintln!("markise: frame log -> {}", path.display());
        markise_ui_egui::smooth_scroll::enable_diag();
        *g = Some(Log {
            out,
            t0: Instant::now(),
            last: None,
            frame: 0,
        });
    }
    let (input_events, predicted_dt) = ui.input(|i| (i.raw.events.len(), i.predicted_dt));
    Some(Frame {
        start: Instant::now(),
        cpu0: thread_cpu_ms(),
        input_events,
        predicted_dt,
        prev_cpu: frame.info().cpu_usage,
    })
}

impl Frame {
    /// After the app's frame: one CSV line.
    pub fn finish(self, ui: &eframe::egui::Ui) {
        let wall = self.start.elapsed().as_secs_f64() * 1e3;
        let cpu = thread_cpu_ms() - self.cpu0;
        let repaint = ui.ctx().has_requested_repaint();
        let s = markise_ui_egui::smooth_scroll::take_frame_diag();
        let mut g = lock(&LOG);
        let Some(l) = g.as_mut() else { return };
        let t = self.start.duration_since(l.t0).as_secs_f64() * 1e3;
        let interval = l.last.map_or(0.0, |p| self.start.duration_since(p).as_secs_f64() * 1e3);
        l.last = Some(self.start);
        l.frame += 1;
        let _ = writeln!(
            l.out,
            "{},{t:.3},{interval:.3},{wall:.3},{cpu:.3},{:.3},{:.3},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.2},{:.0},{},{}",
            l.frame,
            self.prev_cpu.map_or(f32::NAN, |c| c * 1e3),
            self.predicted_dt * 1e3,
            self.input_events,
            s.events,
            s.momentum,
            if s.phases.is_empty() { "-" } else { &s.phases },
            s.raw,
            s.smooth,
            s.applied,
            s.pending,
            s.scroll_y,
            s.total,
            repaint,
            self.input_events > 0
        );
        // flushed per frame: the app may exit without dropping the writer
        let _ = l.out.flush();
    }
}
