//! Spike S1 (ADR 0005 §6): a theme-font paragraph end to end.
//!
//! Theme family "Spike Theme" = three WOFF2 faces written from macOS system fonts (never
//! committed): Georgia for U+0000–002F, U+003A–00FF (Latin without digits), Courier New for the
//! digits U+0030–0039 (a unicode-range split), Georgia Bold at weight 700. Shaped by
//! markise-text (parley), rasterised into the vello_cpu glyph atlas, uploaded as egui textures
//! and painted as meshes; system font discovery runs on a worker meanwhile.
//!
//! Commands:
//!   s1 prepare <dir>                 write the three .woff2 files
//!   s1 window parley|epaint <dir>    eframe window: start → first frame, paragraph painted,
//!                                    RSS after 3 s, then exits (prints one result line)
//!   s1 shots <dir> <out>             headless screenshots, parley and epaint, 1×/2×, light/dark
//!                                    (software raster of egui's tessellated output)

mod paint;
mod raster;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, pos2};
use markise_text::{
    FaceRule, FaceSource, Family, FontStore, LoadPolicy, StyledText, TextEngine, TextLayout, TextStyle,
};

const SUPP: &str = "/System/Library/Fonts/Supplemental";
const TEXT: &str =
    "Theme fonts in 2026: Georgia letters, Courier digits 0123456789, and bold words — all shaped by parley.";
const BOLD: &str = "bold words";

fn woff2(sfnt: &[u8]) -> Vec<u8> {
    markise_text::testing::woff2(sfnt, |d| {
        let mut out = Vec::new();
        brotli::BrotliCompress(&mut &d[..], &mut out, &brotli::enc::BrotliEncoderParams::default()).unwrap();
        out
    })
}

fn prepare(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for (src, dst) in [
        ("Georgia.ttf", "theme-regular.woff2"),
        ("Courier New.ttf", "theme-digits.woff2"),
        ("Georgia Bold.ttf", "theme-bold.woff2"),
    ] {
        let b = std::fs::read(Path::new(SUPP).join(src)).expect("system font");
        let w = woff2(&b);
        std::fs::write(dir.join(dst), &w).unwrap();
        println!("{dst}: {} B (from {} B)", w.len(), b.len());
    }
}

fn rules() -> Vec<FaceRule> {
    vec![
        FaceRule::new("Spike Theme", vec![FaceSource::File("theme-regular.woff2".into())]).unicode_range([
            0x0000..=0x002F,
            0x003A..=0x00FF,
            0x2000..=0x206F,
        ]),
        FaceRule::new("Spike Theme", vec![FaceSource::File("theme-digits.woff2".into())])
            .unicode_range([0x0030..=0x0039]),
        FaceRule::new("Spike Theme", vec![FaceSource::File("theme-bold.woff2".into())]).weight(700.0, 700.0),
    ]
}

fn styled(size: f32) -> StyledText {
    let fam = Family::parse_list("\"Spike Theme\", serif");
    let mut t = StyledText::plain(TEXT, TextStyle::new(fam.clone(), size));
    let b = TEXT.find(BOLD).unwrap();
    t.push_run(b..b + BOLD.len(), TextStyle::new(fam, size).weight(700.0));
    t
}

fn rss_kib() -> u64 {
    let pid = std::process::id().to_string();
    let o = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0)
}

/// Parley path state.
struct ParleyText {
    engine: TextEngine,
    painter: paint::TextPainter,
    layout: Option<(f32, TextLayout)>,
    faces_ms: f64,
}

impl ParleyText {
    fn new(dir: &Path, discover: bool) -> Self {
        let t = Instant::now();
        let mut store = FontStore::new();
        if discover {
            store.start_system_discovery(); // worker thread
        }
        store.set_policy(LoadPolicy {
            root: Some(dir.to_path_buf()),
            max_bytes: 16 << 20,
        });
        for r in rules() {
            store.add_face(r).expect("theme face");
        }
        ParleyText {
            engine: TextEngine::new(store),
            painter: paint::TextPainter::default(),
            layout: None,
            faces_ms: t.elapsed().as_secs_f64() * 1e3,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, color: Color32) {
        self.engine.fonts.poll();
        let ppp = ui.ctx().pixels_per_point();
        let width = 520.0;
        if self.layout.as_ref().is_none_or(|(p, _)| *p != ppp) {
            self.layout = Some((ppp, self.engine.layout(&styled(18.0), Some(width), ppp)));
        }
        let (_, l) = self.layout.as_ref().unwrap();
        let (w, h) = l.size();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w / ppp, h / ppp), egui::Sense::hover());
        let origin = rect.min;
        self.painter.paint(ui.ctx(), ui.painter(), l, origin, ppp, color);
    }
}

fn epaint_fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, file) in [("georgia", "Georgia.ttf"), ("georgia-bold", "Georgia Bold.ttf")] {
        let b = std::fs::read(Path::new(SUPP).join(file)).unwrap();
        defs.font_data.insert(name.into(), Arc::new(FontData::from_owned(b)));
    }
    defs.families
        .insert(FontFamily::Name("theme".into()), vec!["georgia".into()]);
    defs.families
        .insert(FontFamily::Name("theme-bold".into()), vec!["georgia-bold".into()]);
    defs
}

fn epaint_ui(ui: &mut egui::Ui, color: Color32) {
    // M1-style: Georgia family + a separate bold family; no unicode-range representation
    let b = TEXT.find(BOLD).unwrap();
    let mut job = egui::text::LayoutJob::default();
    let fmt = |fam: &str| egui::TextFormat {
        font_id: FontId::new(18.0, FontFamily::Name(fam.into())),
        color,
        ..Default::default()
    };
    job.append(&TEXT[..b], 0.0, fmt("theme"));
    job.append(BOLD, 0.0, fmt("theme-bold"));
    job.append(&TEXT[b + BOLD.len()..], 0.0, fmt("theme"));
    job.wrap.max_width = 520.0;
    ui.label(job);
}

struct WindowApp {
    mode: String,
    start: Instant,
    parley: Option<ParleyText>,
    first_frame: Option<Duration>,
    painted: Option<Duration>,
    created: Duration,
    frames: u32,
    rss_at_start: u64,
}

impl eframe::App for WindowApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames += 1;
        if self.first_frame.is_none() {
            self.first_frame = Some(self.start.elapsed());
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label(RichText::new(format!("S1 — {} path", self.mode)).small());
            let color = ui.visuals().text_color();
            match &mut self.parley {
                Some(p) => p.ui(ui, color),
                None => epaint_ui(ui, color),
            }
        });
        if self.painted.is_none() {
            self.painted = Some(self.start.elapsed());
        }
        let el = self.start.elapsed();
        if el > Duration::from_secs(3) {
            let sys = self.parley.as_ref().map(|p| {
                (
                    p.engine.fonts.system_ready(),
                    p.engine.fonts.discovery_time(),
                    p.faces_ms,
                )
            });
            println!(
                "RESULT mode={} app_created_ms={:.1} first_frame_ms={:.1} paragraph_painted_ms={:.1} frames={} rss_start_kib={} rss_3s_kib={} parley(system_ready, discovery, theme_faces_ms)={:?} atlas_pages={:?} upload_bytes={:?}",
                self.mode,
                self.created.as_secs_f64() * 1e3,
                self.first_frame.unwrap().as_secs_f64() * 1e3,
                self.painted.unwrap().as_secs_f64() * 1e3,
                self.frames,
                self.rss_at_start,
                rss_kib(),
                sys,
                self.parley.as_ref().map(|p| p.painter.atlas.pages.len()),
                self.parley.as_ref().map(|p| p.painter.upload_bytes),
            );
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint();
    }
}

fn window(mode: &str, dir: PathBuf) {
    let start = Instant::now();
    let rss_at_start = rss_kib();
    let mode = mode.to_string();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([640.0, 260.0]),
        ..Default::default()
    };
    eframe::run_native(
        "markise S1",
        opts,
        Box::new(move |cc| {
            let discover = std::env::var_os("S1_NO_DISCOVERY").is_none();
            let parley = if mode == "parley" {
                let mut p = ParleyText::new(&dir, discover);
                if let Some(sz) = std::env::var("S1_PAGE").ok().and_then(|v| v.parse().ok()) {
                    p.painter.atlas = atlas::Atlas::new(sz);
                }
                Some(p)
            } else {
                cc.egui_ctx.set_fonts(epaint_fonts());
                None
            };
            Ok(Box::new(WindowApp {
                mode,
                start,
                parley,
                first_frame: None,
                painted: None,
                created: start.elapsed(),
                frames: 0,
                rss_at_start,
            }))
        }),
    )
    .unwrap();
}

fn shots(dir: PathBuf, out: PathBuf) {
    std::fs::create_dir_all(&out).unwrap();
    for mode in ["parley", "epaint"] {
        for ppp in [1.0f32, 2.0] {
            for dark in [false, true] {
                let ctx = egui::Context::default();
                ctx.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                let mut parley = (mode == "parley").then(|| ParleyText::new(&dir, false));
                if parley.is_none() {
                    ctx.set_fonts(epaint_fonts());
                }
                let (w, h) = (600.0, 140.0);
                let input = || egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(w, h))),
                    viewports: std::iter::once((
                        egui::ViewportId::ROOT,
                        egui::ViewportInfo {
                            native_pixels_per_point: Some(ppp),
                            ..Default::default()
                        },
                    ))
                    .collect(),
                    ..Default::default()
                };
                let mut texs = std::collections::HashMap::new();
                let mut last = None;
                for _ in 0..3 {
                    let mut o = ctx.run_ui(input(), |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            let color = ui.visuals().text_color();
                            match &mut parley {
                                Some(p) => p.ui(ui, color),
                                None => epaint_ui(ui, color),
                            }
                        });
                    });
                    raster::apply_all(&mut texs, &mut o.textures_delta);
                    last = Some(ctx.tessellate(o.shapes, o.pixels_per_point));
                }
                let prims = last.unwrap();
                let r = raster::rasterize(&prims, &texs, ppp, (w * ppp) as usize, (h * ppp) as usize);
                let name = format!("s1-{mode}-{}x-{}.png", ppp as u32, if dark { "dark" } else { "light" });
                raster::save_png(&r, &out.join(&name));
                if let Some(p) = &parley {
                    let l = p.layout.as_ref().unwrap().1.glyph_runs();
                    let mut faces: Vec<(u64, usize)> = Vec::new();
                    for run in &l {
                        match faces.iter_mut().find(|f| f.0 == run.font.id()) {
                            Some(f) => f.1 += run.glyphs.len(),
                            None => faces.push((run.font.id(), run.glyphs.len())),
                        }
                    }
                    let names: Vec<String> = faces
                        .iter()
                        .map(|(id, n)| {
                            let which = (0..3)
                                .find(|&k| p.engine.fonts.face_blob_id(k) == *id)
                                .map_or("other".to_string(), |k| ["regular", "digits", "bold"][k].to_string());
                            format!("{which}:{n}")
                        })
                        .collect();
                    println!("{name}: glyphs per theme face {names:?}");
                } else {
                    println!("{name}");
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("prepare") => prepare(Path::new(&args[2])),
        Some("window") => window(&args[2], PathBuf::from(&args[3])),
        Some("shots") => shots(PathBuf::from(&args[2]), PathBuf::from(&args[3])),
        _ => eprintln!("usage: s1 prepare <dir> | window parley|epaint <dir> | shots <dir> <out>"),
    }
}
