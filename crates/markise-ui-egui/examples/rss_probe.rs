//! Headless RSS probe: opens a document in the real app with system fonts and reports the
//! resident memory once the document and its fonts (incl. lazily loaded fallbacks) settled.
//!
//! `cargo run -p markise-ui-egui --release --example rss_probe -- <doc.md>`

use std::time::{Duration, Instant};

use markise_ui_egui::{AppOptions, ConfigDir, MarkiseApp, rss_mb};

fn main() {
    let doc = std::env::args().nth(1).expect("usage: rss_probe <doc.md>");
    let config = std::env::temp_dir().join(format!("markise-rss-probe-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    let before = rss_mb();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![doc.into()],
        system_fonts: true,
        config: ConfigDir::Dir(config.clone()),
        ..Default::default()
    });
    let ctx = egui::Context::default();
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 780.0))),
        ..Default::default()
    };
    let start = Instant::now();
    let mut stable = 0;
    let mut last_fonts = 0usize;
    while start.elapsed() < Duration::from_secs(20) {
        ctx.run_ui(input(), |ui| app.ui(ui)).drop_without_applying_deltas();
        let fonts = markise_ui_egui::fonts::retained_bytes();
        let ready = app.model().is_some() && !app.is_busy() && app.theme_manager().last_fonts.is_some();
        if ready && fonts == last_fonts {
            stable += 1;
            if stable > 40 {
                break;
            }
        } else {
            stable = 0;
        }
        last_fonts = fonts;
        std::thread::sleep(Duration::from_millis(10));
    }
    println!(
        "rss_mb={} (process start {before}) font_bytes_mb={:.1} fallback_requested={}",
        rss_mb(),
        markise_ui_egui::fonts::retained_bytes() as f64 / (1 << 20) as f64,
        app.theme_manager().fallback_requested()
    );
    let _ = std::fs::remove_dir_all(&config);
}
