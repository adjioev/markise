//! PROTOTYPE (#86): `MARKISE_FRAME_LOG=1` (or a path) writes one CSV line per frame — timing,
//! scroll input received and applied, repaint cause — to `/tmp/markise-frames-<pid>.csv` (or
//! the path given), for frame pacing analysis on the real display.

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

pub fn set_config(c: String) {
    *CONFIG.lock().unwrap() = c;
}

fn enabled() -> Option<String> {
    let v = std::env::var("MARKISE_FRAME_LOG").ok()?;
    match v.as_str() {
        "" | "0" => None,
        "1" => Some(format!("/tmp/markise-frames-{}.csv", std::process::id())),
        p => Some(p.to_string()),
    }
}

fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec; the clock is supported on macOS and Linux.
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 / 1e6
}

/// Refresh rate the main screen supports (macOS: `NSScreen.maximumFramesPerSecond`).
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
            let n: isize = msg_send![s, maximumFramesPerSecond];
            n
        }
    }
    #[cfg(not(target_os = "macos"))]
    0
}

pub struct Frame {
    start: Instant,
    cpu0: f64,
    input_events: usize,
    predicted_dt: f32,
    prev_cpu_usage: Option<f32>,
}

/// Before the app's frame (`None` when logging is off).
pub fn start(ui: &eframe::egui::Ui, frame: &eframe::Frame) -> Option<Frame> {
    let mut g = LOG.lock().unwrap();
    if g.is_none() {
        let path = enabled()?;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).ok()?);
        let _ = writeln!(
            out,
            "# markise frame log: {} | display max fps {} | ppp {} | MARKISE_SCROLL_SNAP={:?} MARKISE_SCROLL_RAW={:?} MARKISE_DISPLAY_LINK={:?} MARKISE_LAZY_VIEW={:?}",
            CONFIG.lock().unwrap(),
            display_fps(),
            ui.ctx().pixels_per_point(),
            std::env::var("MARKISE_SCROLL_SNAP").ok(),
            std::env::var("MARKISE_SCROLL_RAW").ok(),
            std::env::var("MARKISE_DISPLAY_LINK").ok(),
            std::env::var("MARKISE_LAZY_VIEW").ok(),
        );
        let _ = writeln!(
            out,
            "frame,t_ms,interval_ms,ui_wall_ms,ui_cpu_ms,prev_frame_cpu_ms,predicted_dt_ms,input_events,wheel_events,momentum_events,phases,delta_raw_pt,delta_egui_smooth_pt,delta_applied_pt,pending_pt,scroll_y,total_h,repaint_requested,input_driven"
        );
        let _ = out.flush();
        eprintln!("markise: frame log -> {path}");
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
        prev_cpu_usage: frame.info().cpu_usage,
    })
}

impl Frame {
    /// After the app's frame: one CSV line.
    pub fn finish(self, ui: &eframe::egui::Ui) {
        let wall = self.start.elapsed().as_secs_f64() * 1e3;
        let cpu = thread_cpu_ms() - self.cpu0;
        let repaint = ui.ctx().has_requested_repaint();
        let s = markise_ui_egui::viewer::take_frame_scroll();
        let mut g = LOG.lock().unwrap();
        let Some(l) = g.as_mut() else { return };
        let t = self.start.duration_since(l.t0).as_secs_f64() * 1e3;
        let interval = l.last.map_or(0.0, |p| self.start.duration_since(p).as_secs_f64() * 1e3);
        l.last = Some(self.start);
        l.frame += 1;
        let _ = writeln!(
            l.out,
            "{},{t:.3},{interval:.3},{wall:.3},{cpu:.3},{:.3},{:.3},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.2},{:.0},{},{}",
            l.frame,
            self.prev_cpu_usage.map_or(f32::NAN, |c| c * 1e3),
            self.predicted_dt * 1e3,
            self.input_events,
            s.events,
            s.momentum,
            if s.phases.is_empty() { "-".into() } else { s.phases },
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
