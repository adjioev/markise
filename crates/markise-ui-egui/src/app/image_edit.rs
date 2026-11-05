//! Changing an image from its buttons in the rendered view: Replace… chooses another file, which takes
//! the path the attachment rules give it, the same as a drop; Edit… is a small window with the description
//! and the path. One undo step each. The image's title and attributes stay.

use std::ops::Range;
use std::path::{Path, PathBuf};

use egui::{Context, Key, Modifiers};
use markise_engine::attach::{self, Item, Outcome};
use markise_engine::editor::image_dest_for;

use super::attach::Stage;
use super::{Banner, MarkiseApp};

/// A document and its revision.
type Origin = (u64, u64);

/// What a button asked for in the last frame.
enum Asked {
    Replace(Range<usize>),
    Edit(Range<usize>),
}

/// A chosen file on its way to a path.
struct Copying {
    origin: Origin,
    at: Range<usize>,
    stage: Stage,
}

/// The small window of Edit….
struct EditWindow {
    origin: Origin,
    at: Range<usize>,
    alt: String,
    dest: String,
    message: Option<String>,
    focus: bool,
}

#[derive(Default)]
pub(super) struct ImageEditState {
    asked: Option<(Asked, Origin)>,
    copying: Option<Copying>,
    window: Option<EditWindow>,
}

impl ImageEditState {
    pub(super) fn request(
        &mut self,
        replace: Option<Range<usize>>,
        edit: Option<Range<usize>>,
        origin: Option<Origin>,
    ) {
        let asked = replace.map(Asked::Replace).or(edit.map(Asked::Edit));
        if let (Some(asked), Some(origin)) = (asked, origin) {
            self.asked = Some((asked, origin));
        }
    }

    /// Nothing is on its way.
    pub(super) fn is_settled(&self) -> bool {
        self.asked.is_none() && self.copying.is_none()
    }
}

impl MarkiseApp {
    /// Per frame, before the document is drawn.
    pub(super) fn image_edit_frame(&mut self, ctx: &Context) {
        if let Some((asked, origin)) = self.image_edit.asked.take()
            && self.block_edit_origin() == Some(origin)
        {
            match asked {
                Asked::Replace(at) => self.replace_image(at),
                Asked::Edit(at) => self.open_image_edit(at),
            }
        }
        self.image_copy_poll();
        self.image_edit_window(ctx);
    }

    /// Why no image can change now, as the banner says it.
    fn image_edit_refused(&self) -> Option<&'static str> {
        let d = self.doc.as_ref()?;
        if d.viewer.review_preview() {
            return Some("The document is read-only while comments show as No Markup or Original (View → Comments).");
        }
        d.viewer
            .ed
            .session
            .read_only()
            .is_some()
            .then_some("The document is read-only.")
    }

    /// Replace…: asks for a file, then gives the image at `at` the path it gets.
    pub fn replace_image(&mut self, at: Range<usize>) {
        if let Some(why) = self.image_edit_refused() {
            self.banner = Some(Banner::Info(why.into()));
            return;
        }
        let folder = (self.doc.as_ref())
            .and_then(|d| d.file().and_then(Path::parent).map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let picker = self.block_form.picker.clone();
        let Some(file) = picker(&folder) else { return };
        self.replace_image_with(at, file);
    }

    /// Gives the image at `at` this file, copied by the attachment rules on a worker.
    pub(super) fn replace_image_with(&mut self, at: Range<usize>, file: PathBuf) {
        let (Some(origin), Some(req)) = (self.block_edit_origin(), self.attach_request()) else {
            return;
        };
        let stage = self.job(req, vec![Item::File(file)]);
        self.image_edit.copying = Some(Copying { origin, at, stage });
    }

    fn image_copy_poll(&mut self) {
        let Some(mut copying) = self.image_edit.copying.take() else {
            return;
        };
        let Some(out) = self.job_outcome(&mut copying.stage) else {
            self.image_edit.copying = Some(copying);
            return;
        };
        if self.block_edit_origin() != Some(copying.origin) {
            self.banner = Some(Banner::Info(
                "The document changed while the file was copied, so the image was not replaced.".into(),
            ));
            return;
        }
        self.image_take_outcome(copying.at, out);
    }

    fn image_take_outcome(&mut self, at: Range<usize>, out: Outcome) {
        if let Some(why) = out.errors.first() {
            self.banner = Some(Banner::Error(format!("The image was not replaced: {why}")));
            return;
        }
        let Some(target) = out.targets.first() else { return };
        let relative = self.themes.image_settings().relative_paths;
        let Some(d) = &mut self.doc else { return };
        let note_dir = d.file().and_then(Path::parent).map(Path::to_path_buf);
        let root = (d.viewer.ed.session.doc().resource_root())
            .and_then(|r| attach::resource_root_dir(note_dir.as_deref(), &r));
        let dest = attach::destination(
            note_dir.as_deref(),
            root.as_deref(),
            target,
            relative && note_dir.is_some(),
        );
        if let Err(why) = d.viewer.ed.replace_image_dest(at, &dest) {
            self.banner = Some(Banner::Error(format!("The image was not replaced: {why}.")));
        }
    }

    /// Edit…: the description and the path of the image at `at`, as written.
    pub fn open_image_edit(&mut self, at: Range<usize>) {
        if let Some(why) = self.image_edit_refused() {
            self.banner = Some(Banner::Info(why.into()));
            return;
        }
        let Some(origin) = self.block_edit_origin() else { return };
        let Some(d) = &self.doc else { return };
        let Some(parts) = d.viewer.ed.image_parts(at.clone()) else {
            return;
        };
        let text = d.viewer.ed.session.text();
        let dest = &text[parts.dest.clone()];
        let dest = dest.strip_prefix('<').and_then(|t| t.strip_suffix('>')).unwrap_or(dest);
        self.image_edit.window = Some(EditWindow {
            origin,
            at,
            alt: text[parts.alt].to_string(),
            dest: dest.to_string(),
            message: None,
            focus: true,
        });
    }

    /// The Edit… window is open.
    pub fn image_edit_is_open(&self) -> bool {
        self.image_edit.window.is_some()
    }

    fn image_edit_window(&mut self, ctx: &Context) {
        let Some(mut w) = self.image_edit.window.take() else {
            return;
        };
        // what it holds a place in is another text by now
        if self.block_edit_origin() != Some(w.origin) {
            return;
        }
        let mut save = false;
        let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        egui::Window::new("Edit Image")
            .id(egui::Id::unique("markise-image-edit"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .show(ctx, |ui| {
                ui.set_width(360.0);
                let label = ui.label("Description");
                let alt = ui.add(
                    egui::TextEdit::singleline(&mut w.alt)
                        .desired_width(f32::INFINITY)
                        .id_salt("markise-image-edit-alt"),
                );
                let alt = alt.labelled_by(label.id);
                if std::mem::take(&mut w.focus) {
                    alt.request_focus();
                }
                let label = ui.label("Path");
                let dest = ui.add(
                    egui::TextEdit::singleline(&mut w.dest)
                        .desired_width(f32::INFINITY)
                        .id_salt("markise-image-edit-path"),
                );
                let dest = dest.labelled_by(label.id);
                if (alt.lost_focus() || dest.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter)) {
                    save = true;
                }
                if let Some(m) = &w.message {
                    ui.colored_label(ui.visuals().warn_fg_color, m);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            if let Some(d) = &mut self.doc {
                d.viewer.request_refocus();
            }
            return;
        }
        if save {
            let Some(d) = &mut self.doc else { return };
            let Some(dest) = image_dest_for(&w.dest) else {
                w.message = Some("A path cannot hold that.".into());
                self.image_edit.window = Some(w);
                return;
            };
            match d.viewer.ed.edit_image(w.at.clone(), &w.alt, &dest) {
                Ok(_) => {
                    d.viewer.request_refocus();
                    return;
                }
                Err(why) => w.message = Some(format!("It was not changed: {why}.")),
            }
        }
        self.image_edit.window = Some(w);
    }
}
