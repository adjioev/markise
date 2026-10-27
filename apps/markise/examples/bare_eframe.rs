//! Baseline for NFR-1/NFR-6: an empty eframe window with default settings, timed the same way
//! as the app (`cargo run -p markise --release --example bare_eframe`). Prints the time to
//! the creator callback and to the first frame plus RSS, then exits after 1 s.

use std::time::Instant;

fn main() -> eframe::Result {
    let start = Instant::now();
    let mut first = true;
    eframe::run_ui_native("bare", eframe::NativeOptions::default(), move |ui, _| {
        eframe::egui::CentralPanel::default().show(ui, |ui| ui.label("bare"));
        if first {
            first = false;
            let rss = memory_stats::memory_stats().map_or(0, |m| m.physical_mem >> 20);
            eprintln!(
                "bare-eframe: first-frame-built t={:.1}ms rss_mb={rss}",
                start.elapsed().as_secs_f64() * 1e3
            );
            let c = ui.ctx().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                c.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
                c.request_repaint();
            });
        }
    })
}
