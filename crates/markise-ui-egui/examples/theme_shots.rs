//! Headless screenshots of the real app UI in every built-in theme (light and dark), for
//! visual checks of the theme mapping (ADR 0004).
//!
//! `cargo run -p markise-ui-egui --release --example theme_shots [-- <out-dir> [<doc.md>]]`
//!
//! Drives `MarkiseApp` on a plain `egui::Context` (no window, no GPU), tessellates the final
//! frame with epaint and rasterises the triangles in software (premultiplied alpha, textures
//! sampled bilinearly). Uses system fonts, a temporary config folder and the sample document
//! below unless one is given. Default output: `<repo>/.screenshots/` (gitignored).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, ImageData, TextureId};
use markise_theme::catalog::{Selection, StyleId, ThemeId};
use markise_ui_egui::{AppOptions, Appearance, ConfigDir, MarkiseApp};

const SAMPLE: &str = r#"# Field Notes on Lighthouses

A lighthouse is a tower that guides ships at night. This page exercises **strong text**,
*emphasis*, `inline code`, ~~struck words~~, ==highlighted words== and a [link to the
harbour office](https://example.org/harbour).

## Keepers and Lamps

> The lamp was trimmed at dusk and the log written at dawn.
> — from an imaginary keeper's diary

### Duties

- Trim the wick and polish the lens
- Wind the clockwork
  - every four hours
  - more often in a storm
- [x] Record the weather
- [ ] Repaint the gallery rail

1. Light the lamp
2. Check the fog signal
3. Write the log

#### Lamp types

| Lamp | Fuel | Range (nmi) |
|:-----|:----:|------------:|
| Argand | colza oil | 12 |
| Pressure | kerosene | 18 |
| Electric | mains | 25 |

```rust
fn beam(angle: f32) -> bool {
    // one flash every ten seconds
    (angle % 360.0) < 12.0
}
```

---

##### A smaller heading

###### The smallest heading

Final paragraph with a footnote-free ending and some more words so that the line wraps
nicely across the column at the default window size.
"#;

/// Code blocks in the languages the highlighter must cover (`MARKISE_SHOTS=code`).
const SAMPLE_CODE: &str = r#"## Code samples

```rust
/// A lamp with a flash pattern.
#[derive(Debug, Clone)]
pub struct Lamp<'a> { name: &'a str, period: f32 }

impl Lamp<'_> {
    pub fn is_lit(&self, t: f32) -> bool {
        (t % self.period) < 0.5 // short flash
    }
}
```

```ts
interface Beacon { id: number; label?: string }
export async function load(id: number): Promise<Beacon> {
  const res = await fetch(`/beacons/${id}`);
  return (await res.json()) as Beacon; // trust the server
}
```

```python
import math

class Tower:
    """A lighthouse tower."""
    def range_nmi(self, height_m: float) -> float:
        # geographic range for a 5 m eye height
        return 2.08 * (math.sqrt(height_m) + math.sqrt(5.0))
```

```json
{"name": "Fastnet", "lit": 1904, "range": 27.5, "automated": true, "keeper": null}
```

```bash
#!/usr/bin/env bash
for f in logs/*.txt; do
  if grep -q "fog" "$f"; then echo "fog in ${f##*/}"; fi
done
```

```yaml
lighthouse:
  name: Fastnet   # Ireland
  height: 54
  lamps: [main, reserve]
```
"#;

/// Cyrillic text with long inline code spans (justification and code-box spacing check).
const SAMPLE_RU: &str = r#"## Часть 3. Техническая проверка

Не для методиста, делается один раз.

- **Миграция.** В релизе `add_reference :trail_node_games, :linked_source` + колонка `linked_at`. Убедиться, что деплой её прогнал: без неё страница узла падает на первом обращении к `linked?`.
- **Импорт реестра.** Прогнать `games:import` на подтеме со связанным вариантом. Ожидание — поимённый отказ: «связан с B, B1, реестр сюда не пишет». Если импорт молча перезапишет, следующий синк снесёт связь.
- **Дампер контента.** Связанный вариант отдаёт текст источника, но со своей категорией; в дампе поле `linked_license`. Проверить, что next-server это не ломает.
- **Порядок выката.** Гейт потолка схемы (`MAX_KNOWN_SCHEMA_VERSION` в `apps/next-server/lib/content-sync/worker.ts`) поднят до `20260825120000` тем же PR. На прод next-server должен уехать **раньше** Rails: иначе первый же publish уронит синк, и ученики останутся на старом контенте при зелёной публикации.
- **Новые колонки в синк не едут намеренно.** `linked_source_id/linked_at` — админские; дампер разрешает связь ДО записи, поэтому в `academy_games.json` у связанного варианта уже лежат контент и статус источника. Единственное новое поле дампа — информационное `linked_license`, адресованное `content:load` на стороне Rails, а не раннеру.

Обычный абзац с выравниванием по ширине: строки внутри абзаца растягиваются только за счёт пробелов между словами, последняя строка остаётся неровной.
"#;

struct Raster {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>, // premultiplied, gamma space, 0..1
}

struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

impl Tex {
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let g = |x: usize, y: usize| {
            let c = self.px[y * self.w + x];
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
        };
        let (a, b, c, d) = (g(x0, y0), g(x1, y0), g(x0, y1), g(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            out[i] = (top + (bot - top) * fy) / 255.0;
        }
        out
    }
}

/// Applies (and consumes) a frame's texture updates (frees are ignored: a short-lived run).
fn apply_all(texs: &mut HashMap<TextureId, Tex>, delta: &mut egui::TexturesDelta) {
    for (id, deltas) in &delta.set {
        for d in deltas {
            apply_delta(texs, *id, d);
        }
    }
    delta.clear();
}

fn apply_delta(texs: &mut HashMap<TextureId, Tex>, id: TextureId, delta: &egui::epaint::ImageDelta) {
    let ImageData::Color(img) = &delta.image;
    let [w, h] = img.size;
    match delta.pos {
        None => {
            texs.insert(
                id,
                Tex {
                    w,
                    h,
                    px: img.pixels.clone(),
                },
            );
        }
        Some([x0, y0]) => {
            if let Some(t) = texs.get_mut(&id) {
                for y in 0..h {
                    for x in 0..w {
                        if x0 + x < t.w && y0 + y < t.h {
                            t.px[(y0 + y) * t.w + x0 + x] = img.pixels[y * w + x];
                        }
                    }
                }
            }
        }
    }
}

fn rasterize(prims: &[ClippedPrimitive], texs: &HashMap<TextureId, Tex>, ppp: f32, w: usize, h: usize) -> Raster {
    let mut r = Raster {
        w,
        h,
        px: vec![[0.0, 0.0, 0.0, 1.0]; w * h],
    };
    for cp in prims {
        let Primitive::Mesh(mesh) = &cp.primitive else { continue };
        let Some(tex) = texs.get(&mesh.texture_id) else {
            continue;
        };
        let clip = cp.clip_rect;
        let (cx0, cy0) = ((clip.min.x * ppp).max(0.0), (clip.min.y * ppp).max(0.0));
        let (cx1, cy1) = ((clip.max.x * ppp).min(w as f32), (clip.max.y * ppp).min(h as f32));
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let p: Vec<(f32, f32)> = v.iter().map(|v| (v.pos.x * ppp, v.pos.y * ppp)).collect();
            let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).max(cx0).floor() as i32;
            let maxx = p.iter().map(|q| q.0).fold(f32::MIN, f32::max).min(cx1).ceil() as i32;
            let miny = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).max(cy0).floor() as i32;
            let maxy = p.iter().map(|q| q.1).fold(f32::MIN, f32::max).min(cy1).ceil() as i32;
            for y in miny.max(0)..maxy.min(h as i32) {
                for x in minx.max(0)..maxx.min(w as i32) {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5);
                    if sx < cx0 || sx >= cx1 || sy < cy0 || sy >= cy1 {
                        continue;
                    }
                    let w0 = ((p[1].0 - sx) * (p[2].1 - sy) - (p[2].0 - sx) * (p[1].1 - sy)) / area;
                    let w1 = ((p[2].0 - sx) * (p[0].1 - sy) - (p[0].0 - sx) * (p[2].1 - sy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let t = tex.sample(u, vv);
                    let mut src = [0.0f32; 4];
                    for (i, s) in src.iter_mut().enumerate() {
                        let c = |k: usize| v[k].color.to_array()[i] as f32 / 255.0;
                        *s = (w0 * c(0) + w1 * c(1) + w2 * c(2)) * t[i];
                    }
                    let d = &mut r.px[y as usize * w + x as usize];
                    for i in 0..4 {
                        d[i] = src[i] + d[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    r
}

fn save_png(r: &Raster, path: &Path) {
    let mut img = image::RgbaImage::new(r.w as u32, r.h as u32);
    for (i, p) in r.px.iter().enumerate() {
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        img.put_pixel(
            (i % r.w) as u32,
            (i / r.w) as u32,
            image::Rgba([to8(p[0]), to8(p[1]), to8(p[2]), 255]),
        );
    }
    img.save(path).expect("write png");
}

fn shoot(doc: &Path, config: &Path, id: ThemeId, appearance: Appearance, out: &Path) {
    shoot_with(doc, config, Selection::complete(id), appearance, out, None);
}

/// `files_tab`: the second shot shows the sidebar's Files tab with this folder expanded
/// (instead of the end of the document).
fn shoot_with(doc: &Path, config: &Path, sel: Selection, appearance: Appearance, out: &Path, files_tab: Option<&Path>) {
    let (w_pt, h_pt, ppp) = (1080.0f32, 820.0f32, 2.0f32);
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![doc.to_path_buf()],
        system_fonts: true,
        // `MARKISE_TEXT=parley`: shots of the parley text path (ADR 0005 T2)
        text_backend: markise_ui_egui::TextBackend::from_env(),
        config: ConfigDir::Dir(config.to_path_buf()),
        // `==mark==` on, so the theme's highlight style is visible; diagrams on (ADR 0009)
        parse: markise_md::ParseOptions {
            highlight: true,
            diagrams: true,
            ..Default::default()
        },
        ..Default::default()
    });
    app.select_style(sel.style);
    app.select_colors(sel.colors);
    app.set_theme_choice(appearance);
    // `MARKISE_SHOT_TOOLBAR=1`: with the formatting toolbar (spec 04 §3.2); `=narrow` also
    // renders a narrow window, where groups collapse into More
    let toolbar = std::env::var("MARKISE_SHOT_TOOLBAR").unwrap_or_default();
    if !toolbar.is_empty() {
        app.set_show_toolbar(true);
    }
    let w_pt = if toolbar == "narrow" { 560.0 } else { w_pt };
    let ctx = egui::Context::default();
    // `MARKISE_SHOT_MODE=sbs|source`: shots of the Side by Side / Source view mode
    let mode = std::env::var("MARKISE_SHOT_MODE").unwrap_or_default();
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
        if app.model().is_some() && app.view_mode() == markise_ui_egui::ViewMode::Rendered {
            match mode.as_str() {
                "sbs" => app.set_view_mode(markise_ui_egui::ViewMode::SideBySide),
                "source" => app.set_view_mode(markise_ui_egui::ViewMode::Source),
                _ => {}
            }
        }
        let ready = app.model().is_some()
            && !app.is_busy()
            && !app.highlighter().is_some_and(|h| h.busy())
            && !app.diagram_renderer().is_some_and(|d| d.busy())
            && app.theme_manager().last_fonts.is_some()
            && app.frames > 3;
        if ready {
            stable += 1;
        }
        if (ready && stable >= 4) || start.elapsed() > Duration::from_secs(20) {
            let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
            let r = rasterize(&prims, &texs, ppp, (w_pt * ppp) as usize, (h_pt * ppp) as usize);
            save_png(&r, out);
            // second shot: the end of the document, or the Files tab
            if let Some(dir) = files_tab {
                app.show_files_tab(true);
                app.toggle_folder(dir);
                for _ in 0..30 {
                    let mut full = ctx.run_ui(input(), |ui| app.ui(ui));
                    apply_all(&mut texs, &mut full.textures_delta);
                    std::thread::sleep(Duration::from_millis(10));
                }
            } else if let Some(v) = app.viewer() {
                v.ed.view.scroll_to_y(f64::MAX);
            }
            let mut full = ctx.run_ui(input(), |ui| app.ui(ui));
            for _ in 0..3 {
                apply_all(&mut texs, &mut full.textures_delta);
                full = ctx.run_ui(input(), |ui| app.ui(ui));
            }
            apply_all(&mut texs, &mut full.textures_delta);
            let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
            let r = rasterize(&prims, &texs, ppp, (w_pt * ppp) as usize, (h_pt * ppp) as usize);
            let suffix = if files_tab.is_some() { "files" } else { "end" };
            save_png(
                &r,
                &out.with_file_name(format!("{}-{suffix}.png", out.file_stem().unwrap().to_string_lossy())),
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    println!("{} ({:?}) -> {}", app.theme().name, appearance, out.display());
    let skipped = ctx.fonts_mut(|f| markise_layout::block::undrawable_generated(app.theme(), f));
    if !skipped.is_empty() {
        println!("  generated content not drawn: {skipped:?}");
    }
    if let Some(f) = &app.theme_manager().last_fonts {
        println!("  fonts: {:?}; not installed: {:?}", f.chosen, f.missing);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out_dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join(".screenshots"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let work = std::env::temp_dir().join(format!("markise-theme-shots-{}", std::process::id()));
    let config = work.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let doc = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => {
            let p = work.join("lighthouses.md");
            std::fs::write(&p, SAMPLE).unwrap();
            p
        }
    };
    let themes = [
        (ThemeId::Markise, "markise"),
        (ThemeId::Builtin("drake".into()), "drake"),
        (ThemeId::Builtin("lapis".into()), "lapis"),
        (ThemeId::Builtin("phycat".into()), "phycat"),
    ];
    // MARKISE_SHOTS=palettes renders only the palette pack, MARKISE_SHOTS=css only the rest
    let which = std::env::var("MARKISE_SHOTS").unwrap_or_default();
    if which.is_empty() || which == "css" {
        for (id, name) in themes {
            for (a, suffix) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
                shoot(
                    &doc,
                    &config,
                    id.clone(),
                    a,
                    &out_dir.join(format!("{name}-{suffix}.png")),
                );
            }
        }
    }
    if which == "sidebar" || which.is_empty() {
        // a small folder: subfolder, Markdown and other text files
        let root = work.join("Lighthouse Notes");
        let guides = root.join("Keeper Guides");
        std::fs::create_dir_all(&guides).unwrap();
        std::fs::create_dir_all(root.join("Archive")).unwrap();
        let doc = root.join("lighthouses.md");
        std::fs::write(&doc, SAMPLE).unwrap();
        for f in ["Lamp maintenance.md", "Fog signals.md"] {
            std::fs::write(
                guides.join(f),
                "# x
",
            )
            .unwrap();
        }
        std::fs::write(
            root.join("README.md"),
            "# readme
",
        )
        .unwrap();
        std::fs::write(
            root.join("logbook.txt"),
            "log
",
        )
        .unwrap();
        let guides = std::fs::canonicalize(&guides).unwrap();
        for (id, a, name) in [
            (ThemeId::Markise, Appearance::Light, "markise-light"),
            (ThemeId::Markise, Appearance::Dark, "markise-dark"),
            (ThemeId::Builtin("drake".into()), Appearance::Light, "drake-light"),
            (ThemeId::Builtin("lapis".into()), Appearance::Light, "lapis-light"),
            (ThemeId::Palette("tokyo-night".into()), Appearance::Dark, "tokyo-night"),
            (
                ThemeId::Palette("catppuccin".into()),
                Appearance::Light,
                "catppuccin-latte",
            ),
        ] {
            shoot_with(
                &doc,
                &config,
                Selection::complete(id),
                a,
                &out_dir.join(format!("sidebar-{name}.png")),
                Some(&guides),
            );
        }
    }
    if which == "ru" || which.is_empty() {
        let ru = work.join("proverka.md");
        std::fs::write(&ru, SAMPLE_RU).unwrap();
        for (id, name) in [
            (ThemeId::Builtin("lapis".into()), "lapis"),
            (ThemeId::Builtin("drake".into()), "drake"),
        ] {
            for (a, suffix) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
                shoot(
                    &ru,
                    &config,
                    id.clone(),
                    a,
                    &out_dir.join(format!("ru-{name}-{suffix}.png")),
                );
            }
        }
    }
    if which.is_empty() || which == "code" {
        let code = work.join("code.md");
        std::fs::write(&code, SAMPLE_CODE).unwrap();
        for (id, a, name) in [
            (ThemeId::Markise, Appearance::Light, "markise-light"),
            (ThemeId::Markise, Appearance::Dark, "markise-dark"),
            (ThemeId::Palette("tokyo-night".into()), Appearance::Dark, "tokyo-night"),
            (ThemeId::Palette("gruvbox".into()), Appearance::Light, "gruvbox-light"),
            (ThemeId::Builtin("lapis".into()), Appearance::Light, "lapis-light"),
        ] {
            shoot(&code, &config, id, a, &out_dir.join(format!("code-{name}.png")));
        }
    }
    if which.is_empty() || which == "palettes" {
        // palette pack and Markise Night: one shot per palette theme
        let mut families = vec![markise_theme::catalog::markise_night_family()];
        families.extend(markise_theme::catalog::palette_families());
        for f in families {
            for (a, v) in [(Appearance::Light, &f.light), (Appearance::Dark, &f.dark)] {
                if let Some(markise_theme::catalog::VariantSource::Native { id }) = v {
                    shoot(
                        &doc,
                        &config,
                        f.id.clone(),
                        a,
                        &out_dir.join(format!("palette-{id}.png")),
                    );
                }
            }
        }
    }
    if which.is_empty() || which == "grid" {
        // ADR 0007: styles × color schemes, plus complete themes decomposed
        let styles = ["classic", "modern", "technical", "compact"];
        let colors = [
            (ThemeId::Markise, Appearance::Light, "markise-light"),
            (ThemeId::Palette("nord".into()), Appearance::Dark, "nord"),
            (ThemeId::Palette("gruvbox".into()), Appearance::Light, "gruvbox-light"),
        ];
        for style in styles {
            for (c, a, cname) in &colors {
                let sel = Selection {
                    style: StyleId::Builtin(style.into()),
                    colors: c.clone(),
                };
                shoot_with(
                    &doc,
                    &config,
                    sel,
                    *a,
                    &out_dir.join(format!("grid-{style}-{cname}.png")),
                    None,
                );
            }
        }
        for (sel, a, name) in [
            (
                Selection {
                    style: StyleId::Theme(ThemeId::Builtin("lapis".into())),
                    colors: ThemeId::Palette("tokyo-night".into()),
                },
                Appearance::Dark,
                "lapis-style-tokyo-night",
            ),
            (
                Selection {
                    style: StyleId::Builtin("modern".into()),
                    colors: ThemeId::Builtin("lapis".into()),
                },
                Appearance::Light,
                "modern-lapis-colors",
            ),
        ] {
            shoot_with(&doc, &config, sel, a, &out_dir.join(format!("grid-{name}.png")), None);
        }
    }
    let _ = std::fs::remove_dir_all(&work);
}
