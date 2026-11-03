use super::*;
use markise_engine::quick_open::{Query, Results, Search};
#[derive(Default)]
pub(super) struct QuickOpen {
    pub open: bool,
    search: Option<Search>,
    query: String,
    generation: u64,
    results: Option<Results>,
    selected: usize,
    focus: bool,
    previous_focus: Option<egui::Id>,
    root: Option<PathBuf>,
}
impl MarkiseApp {
    pub(super) fn open_quick(&mut self, ctx: &Context) {
        let mut roots: Vec<_> = self.tree.as_ref().map(|t| vec![t.root.clone()]).unwrap_or_default();
        if let Some(p) = self.document_path().and_then(Path::parent)
            && !roots.iter().any(|r| p.starts_with(r))
        {
            roots.push(p.to_owned());
        }
        let root = roots.first().cloned();
        let search = match Search::new(roots, self.workflow.recent.clone(), self.wake_handle()) {
            Ok(s) => s,
            Err(e) => {
                self.banner = Some(Banner::Error(e.to_string()));
                return;
            }
        };
        self.quick = QuickOpen {
            open: true,
            root,
            search: Some(search),
            focus: true,
            previous_focus: ctx.memory(|m| m.focused()),
            ..Default::default()
        };
        self.quick_query();
    }
    fn quick_query(&mut self) {
        self.quick.generation += 1;
        self.quick.selected = 0;
        self.quick.results = None;
        if let Some(s) = &self.quick.search {
            let _ = s.tx.send(Query {
                generation: self.quick.generation,
                text: self.quick.query.clone(),
            });
        }
    }
    pub(super) fn quick_window(&mut self, ctx: &Context) {
        if !self.quick.open {
            return;
        }
        if let Some(s) = &self.quick.search {
            for r in s.rx.try_iter() {
                if r.generation == self.quick.generation {
                    self.quick.results = Some(r);
                }
            }
        }
        let mut open = true;
        let mut choose = None;
        let mut changed = false;
        // Consume navigation before the editor sees it; the text field retains typing focus.
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let len = self
            .quick
            .results
            .as_ref()
            .map_or(0, |r| r.recent.len() + r.files.len());
        if up {
            self.quick.selected = self.quick.selected.saturating_sub(1);
        }
        if down {
            self.quick.selected = (self.quick.selected + 1).min(len.saturating_sub(1));
        }
        egui::Window::new("Quick Open")
            .id(egui::Id::unique("quick-open"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .default_width((ctx.content_rect().width() - 48.0).clamp(240.0, 560.0))
            .show(ctx, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.quick.query)
                        .hint_text("Find a file by name…")
                        .desired_width(f32::INFINITY),
                );
                if self.quick.focus {
                    field.request_focus();
                    self.quick.focus = false;
                }
                changed = field.changed();
                if let Some(r) = &self.quick.results {
                    let mut index = 0;
                    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                        for (label, paths) in [("Recent Files", &r.recent), ("File Results", &r.files)] {
                            if !paths.is_empty() {
                                ui.add_space(6.0);
                                ui.weak(label);
                            }
                            for p in paths {
                                let selected = index == self.quick.selected;
                                let response = ui.selectable_label(
                                    selected,
                                    format!(
                                        "{}\n{}",
                                        p.file_name().unwrap_or_default().to_string_lossy(),
                                        location(p, self.quick.root.as_deref())
                                    ),
                                );
                                let response = response.on_hover_text(p.display().to_string());
                                if selected && (up || down) {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() || (selected && enter && !changed) {
                                    choose = Some(p.clone());
                                }
                                index += 1;
                            }
                        }
                    });
                    if len == 0 {
                        ui.label("No matching files.");
                    }
                    if r.limited {
                        ui.label("Folder is large; results cover the first 100,000 entries.");
                    }
                    if r.unreadable {
                        ui.label("Some folders could not be read.");
                    }
                } else {
                    ui.label("Searching…");
                }
                ui.separator();
                ui.weak("Arrow keys: select    Enter: open    Esc: close");
            });
        if changed {
            self.quick_query();
        }
        if !open || escape || choose.is_some() {
            self.quick.open = false;
            self.quick.search = None;
            if let Some(id) = self.quick.previous_focus.take() {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        if let Some(p) = choose {
            self.open_known_file(p);
        }
    }
}

fn location(path: &Path, root: Option<&Path>) -> String {
    let parent = path.parent().unwrap_or(path);
    if let Some(root) = root
        && let Ok(rel) = parent.strip_prefix(root)
    {
        let base = root.file_name().unwrap_or_default().to_string_lossy();
        return if rel.as_os_str().is_empty() {
            base.into_owned()
        } else {
            format!("{base}/{}", rel.display())
        };
    }
    let parts: Vec<_> = parent.components().rev().take(3).collect();
    let suffix: PathBuf = parts.into_iter().rev().collect();
    format!("…/{}", suffix.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    fn wait(h: &mut Harness<'static, MarkiseApp>, check: impl Fn(&MarkiseApp) -> bool) {
        let start = Instant::now();
        while !check(h.state()) {
            assert!(start.elapsed() < Duration::from_secs(5));
            h.step();
            std::thread::sleep(Duration::from_millis(2));
        }
        h.step();
    }
    #[test]
    fn palette_typing_escape_and_unsaved_open_guard() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let a = root.join("alpha.md");
        let b = root.join("beta.md");
        std::fs::write(&a, "alpha").unwrap();
        std::fs::write(&b, "beta").unwrap();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1000.0, 700.0))
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, app: &mut MarkiseApp| app.ui(ui),
                MarkiseApp::new(AppOptions {
                    open: vec![a.clone()],
                    ..Default::default()
                }),
            );
        wait(&mut h, |a| a.doc.is_some() && a.load.is_none());
        h.run_steps(3);
        let ctx = h.ctx.clone();
        let surface = h.state_mut().viewer().unwrap().surface_id().unwrap();
        ctx.memory_mut(|m| m.request_focus(surface));
        h.state_mut().open_quick(&ctx);
        h.run_steps(2);
        h.event(egui::Event::Text("bt".into()));
        h.run_steps(2);
        wait(&mut h, |a| a.quick.results.is_some());
        assert_eq!(h.state().quick.query, "bt");
        assert_eq!(h.state().document_text().unwrap(), "alpha");
        assert_eq!(h.state().quick.results.as_ref().unwrap().files, vec![b.clone()]);
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(!h.state().quick.open);
        assert!(ctx.memory(|m| m.has_focus(surface)));
        h.state_mut()
            .doc
            .as_mut()
            .unwrap()
            .viewer
            .ed
            .session
            .insert_text("edit ")
            .unwrap();
        h.state_mut().open_quick(&ctx);
        h.run_steps(2);
        h.event(egui::Event::Text("bt".into()));
        h.run_steps(2);
        wait(&mut h, |a| a.quick.results.is_some());
        h.key_press(egui::Key::Enter);
        h.run_steps(2);
        assert!(h.state().is_asking());
        assert_eq!(h.state().document_path(), Some(a.as_path()));
    }
}
