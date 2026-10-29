//! Headless screenshots of the app's dialogs (issue #35) and the Preferences window (issue #50): each scene opens one dialog in the
//! real app UI and saves a PNG, in the light and the dark appearance.
//!
//! `cargo run -p markise-ui-egui --release --example dialog_shots [-- <out-dir> [<prefix>]]`
//!
//! Drives `MarkiseApp` on a plain `egui::Context` like `theme_shots` (no window, no GPU).
//! Yes/no questions are native OS alerts in the desktop app; here they are answered by the
//! in-app fallback (shown where no native alert is available), so that is what the shots
//! show. Default output: `<repo>/.screenshots/dialogs/` (gitignored).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, TextureId};
use markise_engine::export::{ExportKind, ExportRequest};
use markise_ui_egui::export_ui::ExportAction;
use markise_ui_egui::menu::MenuAction;
use markise_ui_egui::prefs::{Change, Section};
use markise_ui_egui::sidebar::TreeAction;
use markise_ui_egui::{AppOptions, Appearance, ConfigDir, MarkiseApp};

#[path = "support/raster.rs"]
mod raster;
use raster::{Tex, apply_all, rasterize, save_png};

const DOC: &str =
    "# Harbour log\n\nThe lamp was trimmed at dusk and the log written at dawn.\n\n- wind: NW 4\n- visibility: good\n";

type Setup = fn(&mut MarkiseApp, &Context, &Path);

fn scenes() -> Vec<(&'static str, Setup)> {
    vec![
        ("unsaved-changes", |app, _, _| {
            app.edit_source_text(|t| t.push_str("\nAn unsaved line.\n"));
            app.request_close();
        }),
        ("move-to-trash", |app, ctx, dir| {
            app.tree_action(ctx, TreeAction::Delete(dir.join("2026-05-30.md")));
        }),
        ("rename", |app, ctx, dir| {
            app.tree_action(ctx, TreeAction::Rename(dir.join("2026-05-30.md")));
        }),
        ("rename-error", |app, ctx, dir| {
            app.tree_action(ctx, TreeAction::Rename(dir.join("2026-05-30.md")));
            app.set_rename_text("tides.md");
            app.confirm_rename();
        }),
        ("new-file", |app, ctx, dir| {
            app.tree_action(ctx, TreeAction::NewFile(dir.to_path_buf()));
        }),
        ("new-folder", |app, ctx, dir| {
            app.tree_action(ctx, TreeAction::NewFolder(dir.to_path_buf()));
        }),
        ("export-overwrite", |app, _, dir| {
            let req = ExportRequest::new(
                ExportKind::Html,
                app.document_text().unwrap_or_default(),
                Some(dir.join("log.md")),
                dir.join("log.html"),
                app.theme().clone(),
            );
            // the question is asked once the export finished (see `shoot`)
            app.start_export(req);
        }),
        ("about", |app, ctx, _| {
            app.menu_action(ctx, &MenuAction::Command("app.about"));
        }),
        ("font-and-size", |app, ctx, _| {
            app.menu_action(ctx, &MenuAction::Command("theme.fontAndSize"));
        }),
        ("theme-report", |app, ctx, _| {
            app.menu_action(ctx, &MenuAction::Command("theme.report"));
        }),
        // the Preferences window (issue #50)
        ("preferences-general", |app, ctx, _| {
            app.menu_action(ctx, &MenuAction::Command("app.preferences"));
        }),
        ("preferences-editor", |app, ctx, _| {
            app.open_preferences_at(Section::Editor);
            app.apply_preference(
                ctx,
                Change::Editing(markise_engine::settings::EditingSettings {
                    smart_punctuation: true,
                    ..Default::default()
                }),
            );
        }),
        ("preferences-markdown", |app, _, _| {
            app.open_preferences_at(Section::Markdown)
        }),
        ("preferences-appearance", |app, _, _| {
            app.open_preferences_at(Section::Appearance)
        }),
        ("preferences-source", |app, _, _| {
            app.open_preferences_at(Section::Source)
        }),
        ("preferences-files", |app, _, _| app.open_preferences_at(Section::Files)),
        ("preferences-export", |app, _, _| {
            app.open_preferences_at(Section::Export)
        }),
        ("preferences-search", |app, _, _| {
            app.open_preferences_at(Section::General);
            app.preferences().query = "line".into();
        }),
        ("preferences-reset", |app, ctx, _| {
            app.open_preferences_at(Section::Editor);
            app.apply_preference(
                ctx,
                Change::Editing(markise_engine::settings::EditingSettings {
                    auto_pair_markdown: true,
                    ..Default::default()
                }),
            );
            app.apply_preference(ctx, Change::AskReset(Section::Editor));
        }),
        ("preferences-zoom-150", |app, _, _| {
            app.set_zoom(1.5);
            app.open_preferences_at(Section::Appearance);
        }),
        // the sidebar itself (issue #57): a nested folder, a plain-text file, a long name
        ("sidebar", |app, ctx, dir| {
            app.toggle_folder(&dir.join("drafts"));
            // a second file: the Recent section lists both
            app.tree_action(ctx, TreeAction::Open(dir.join("tides.md")));
        }),
        ("sidebar-outline", |app, _, _| {
            app.show_files_tab(false);
        }),
        // the same with a large document font size: the sidebar keeps its UI size
        ("sidebar-large-font", |app, _, dir| {
            app.toggle_folder(&dir.join("drafts"));
            app.set_overrides(markise_theme::Overrides {
                font_size: Some(17.5),
                ..Default::default()
            });
            app.set_view_mode(markise_ui_egui::ViewMode::Rendered);
        }),
    ]
}

fn shoot(name: &str, setup: Setup, appearance: Appearance, out: &Path) {
    let (w_pt, h_pt, ppp) = (900.0f32, 600.0f32, 1.0f32);
    let work = std::env::temp_dir().join(format!("markise-dialog-shots-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let dir = work.join("harbour");
    std::fs::create_dir_all(dir.join("drafts")).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    std::fs::write(dir.join("log.md"), DOC).unwrap();
    std::fs::write(dir.join("2026-05-30.md"), "# 30 May\n").unwrap();
    std::fs::write(dir.join("tides.md"), "# Tides\n").unwrap();
    std::fs::write(dir.join("crew list.txt"), "Ann, Bo\n").unwrap();
    std::fs::write(
        dir.join("a rather long note title that does not fit the sidebar.md"),
        "# Long\n",
    )
    .unwrap();
    std::fs::write(dir.join("drafts").join("storm warning.md"), "# Storm\n").unwrap();
    std::fs::create_dir_all(dir.join("drafts").join("old")).unwrap();
    let config = work.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let mut app = MarkiseApp::new(AppOptions {
        open: vec![dir.join("log.md")],
        system_fonts: true,
        config: ConfigDir::Dir(config),
        ..Default::default()
    });
    app.set_theme_choice(appearance);
    app.set_view_mode(markise_ui_egui::ViewMode::Source);
    app.show_files_tab(true);
    let ctx = Context::default();
    let mut texs: HashMap<TextureId, Tex> = HashMap::new();
    // the sidebar scenes point at a row (hover highlight)
    let hover = name.starts_with("sidebar").then_some(egui::pos2(150.0, 230.0));
    let input = || {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w_pt, h_pt))),
            events: hover.map(egui::Event::PointerMoved).into_iter().collect(),
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
    let mut done = false;
    loop {
        let mut full = ctx.run_ui(input(), |ui| app.ui(ui));
        apply_all(&mut texs, &mut full.textures_delta);
        let ready = app.model().is_some()
            && !app.is_busy()
            && app.theme_manager().last_fonts.is_some()
            && app.frames > 3
            && app.file_tree_rows().len() >= 3;
        if ready && !done && app.view_mode() != markise_ui_egui::ViewMode::Source {
            // the source editor takes the focus when it opens: switch first, set up later
            app.set_view_mode(markise_ui_egui::ViewMode::Source);
        } else if ready && !done {
            setup(&mut app, &ctx, &dir);
            done = true;
        } else if done {
            if name == "export-overwrite" && stable == 0 && app.export_state().is_running() {
                std::thread::sleep(Duration::from_millis(15));
                continue;
            }
            if name == "export-overwrite" && stable == 0 {
                app.export_action_for_test(ExportAction::OverwritePrevious);
            }
            stable += 1;
        }
        if stable >= 12 || start.elapsed() > Duration::from_secs(20) {
            let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
            let r = rasterize(&prims, &texs, ppp, (w_pt * ppp) as usize, (h_pt * ppp) as usize);
            save_png(&r, out);
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    println!("{name} ({appearance:?}) -> {}", out.display());
    let _ = std::fs::remove_dir_all(&work);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out_dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join(".screenshots/dialogs"));
    let prefix = args.get(1).cloned().unwrap_or_else(|| "after".into());
    std::fs::create_dir_all(&out_dir).unwrap();
    let only = std::env::var("MARKISE_SHOTS").unwrap_or_default();
    for (name, setup) in scenes() {
        if !only.is_empty() && !only.split(',').any(|o| o == name) {
            continue;
        }
        for (a, suffix) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
            shoot(name, setup, a, &out_dir.join(format!("{prefix}-{name}-{suffix}.png")));
        }
    }
}
