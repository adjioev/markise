//! Headless screenshots of one bilingual paragraph at 2× for comparing text weight against the
//! system renderer (issue #172), in the Markise light and dark themes.
//!
//! `cargo run -p markise-ui-egui --release --example text_shots [-- <out-dir>]`
//!
//! Drives `MarkiseApp` on a plain `egui::Context` like `theme_shots` (no window, no GPU, the
//! same gamma-space blending as the wgpu and glow painters). Each line is its own paragraph so a
//! reference render can use the same strings without depending on line breaking. Honours the
//! `MARKISE_TEXT_*` switches of `docs/diagnostics.md`. Default output: `<repo>/.screenshots/text/`
//! (gitignored).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::TextureId;
use markise_ui_egui::{AppOptions, Appearance, ConfigDir, MarkiseApp};

#[path = "support/raster.rs"]
mod raster;
use raster::{Tex, apply_all, rasterize, save_png};

const DOC: &str = "Съешь же ещё этих мягких французских булок, да выпей чаю.\n\n\
The quick brown fox jumps over the lazy dog, then naps at noon.\n\n\
Широкая электрификация южных губерний даст мощный толчок.\n\n\
Pack my box with five dozen liquor jugs and some cold water.\n\n\
нннннннннннннннннннннннннннннннн\n\n\
llllllllllllllllllllllllllllllllllllllll\n";

fn shoot(doc: &Path, config: &Path, appearance: Appearance, out: &Path) {
    let (w_pt, h_pt, ppp) = (1100.0f32, 520.0f32, 2.0f32);
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![doc.to_path_buf()],
        system_fonts: true,
        text_backend: markise_ui_egui::TextBackend::from_env(),
        text_tuning: markise_ui_egui::TextTuning::from_env(),
        config: ConfigDir::Dir(config.to_path_buf()),
        ..Default::default()
    });
    app.set_theme_choice(appearance);
    let ctx = egui::Context::default();
    let mut texs: HashMap<TextureId, Tex> = HashMap::new();
    let input = || {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w_pt, h_pt))),
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        raw
    };
    let start = Instant::now();
    let mut stable = 0;
    loop {
        let mut full = ctx.run_ui(input(), |ui| app.ui(ui));
        apply_all(&mut texs, &mut full.textures_delta);
        if app.model().is_some() && !app.is_busy() && app.theme_manager().last_fonts.is_some() && app.frames > 3 {
            stable += 1;
        }
        if stable >= 4 || start.elapsed() > Duration::from_secs(20) {
            let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
            save_png(
                &rasterize(&prims, &texs, ppp, (w_pt * ppp) as usize, (h_pt * ppp) as usize),
                out,
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let t = app.theme();
    println!(
        "{} -> {} (page {:?}, text {:?}, {} pt)",
        t.name,
        out.display(),
        t.page_bg,
        t.text,
        t.font_size
    );
    if let Some(f) = &app.theme_manager().last_fonts {
        println!("  fonts: {:?}; not installed: {:?}", f.chosen, f.missing);
    }
}

fn main() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join(".screenshots/text"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let work = std::env::temp_dir().join(format!("markise-text-shots-{}", std::process::id()));
    let config = work.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let doc = work.join("text.md");
    std::fs::write(&doc, DOC).unwrap();
    for (a, name) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
        shoot(&doc, &config, a, &out_dir.join(format!("{name}.png")));
    }
    let _ = std::fs::remove_dir_all(&work);
}
