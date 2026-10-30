//! PROTOTYPE (branch `proto/block-snapshots`, not product code).
//!
//! `markise-web-render`: a long-lived helper process that renders one HTML block at a time
//! with the system WebKit (a hidden `NSWindow` holding one `WKWebView`) and answers with a PNG
//! snapshot and the block's height. Markise never hosts a web view itself.
//!
//! Protocol (stdin → requests, stdout → responses; stdout carries nothing else): each frame is
//! `u32 LE length + JSON` followed by `u32 LE length + binary payload` (empty for requests).
//!
//! Request JSON: `{"id": u64, "width": f64, "scale": f64, "html": str, "css": str, "js": str,
//! "theme": "dark"|"light", "vars": str}` (theme → `<html data-theme>` + `color-scheme`, vars →
//! `<html style>` custom properties).
//! Response JSON: `{"id", "ok", "height", "px_w", "px_h", "backing", "decorated",
//! "ms_queue", "ms_js", "ms_snap", "ms_png", "ms_total", "error"}` + PNG bytes.
//! The first frame the helper writes is `{"ready": true, "ms_startup": f64}`.
//!
//! `markise-web-render --selftest <out.png> [width]` renders a fixed HTML sample and exits
//! (the feasibility gate of the prototype brief).

mod proto;

#[cfg(target_os = "macos")]
mod editor;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("markise-web-render: this prototype runs on macOS only");
    std::process::exit(2);
}
