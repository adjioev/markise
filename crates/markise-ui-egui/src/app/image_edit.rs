//! Changing an image from its buttons in the rendered view: Replace… chooses another file in the image
//! picker, which takes the path the attachment rules give it, the same as a drop; Edit… is a small window
//! with the description and the path. One undo step each. The image's title and attributes stay. The picker
//! also serves a form's Choose file.

use std::ops::Range;
use std::path::{Path, PathBuf};

use egui::{Context, Key, Modifiers};
use egui_schema_form::FileRequest;
use markise_engine::attach::{self, Item, Outcome};
use markise_engine::editor::image_dest_for;

use super::attach::Stage;
use super::{Banner, MarkiseApp};
use crate::block_form::relative_to_folder;
use crate::image_picker::{ImagePicker, PickerAct, is_droppable};

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

/// Who a choice in the picker is for.
#[derive(Clone)]
enum PickFor {
    Image(Range<usize>),
    Form(FileRequest),
}

/// The open picker.
struct OpenPicker {
    picker: ImagePicker,
    pick_for: PickFor,
    origin: Origin,
    folder: PathBuf,
    /// Dropped files being copied by the attachment rules.
    adding: Vec<Stage>,
}

#[derive(Default)]
pub(super) struct ImageEditState {
    asked: Option<(Asked, Origin)>,
    copying: Option<Copying>,
    window: Option<EditWindow>,
    picker: Option<OpenPicker>,
    /// A file from outside on its way into the folder, for a form.
    form_copying: Option<(FileRequest, PathBuf, Stage)>,
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
        self.asked.is_none()
            && self.copying.is_none()
            && self.form_copying.is_none()
            && self
                .picker
                .as_ref()
                .is_none_or(|p| !p.picker.is_busy() && p.adding.is_empty())
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
        self.image_form_copy_poll();
        self.image_edit_window(ctx);
        self.image_picker_window(ctx);
    }

    /// The open image picker (tests).
    pub fn image_picker(&self) -> Option<&ImagePicker> {
        self.image_edit.picker.as_ref().map(|p| &p.picker)
    }

    fn open_image_picker(&mut self, pick_for: PickFor, current: Option<String>) -> bool {
        let Some(origin) = self.block_edit_origin() else {
            return false;
        };
        let wake = self.wake_handle();
        let Some(d) = &self.doc else { return false };
        let Some(folder) = d.file().and_then(Path::parent).map(Path::to_path_buf) else {
            return false;
        };
        let text = d.viewer.ed.session.text();
        let picker = ImagePicker::open(&folder, text, current.as_deref(), wake);
        self.image_edit.picker = Some(OpenPicker {
            picker,
            pick_for,
            origin,
            folder,
            adding: Vec::new(),
        });
        true
    }

    /// A form's Choose file: the picker, or nothing for a document with no folder.
    pub(super) fn pick_for_form(&mut self, request: FileRequest) {
        if !self.open_image_picker(PickFor::Form(request), None)
            && let Some(open) = self.block_form.open.as_mut()
        {
            open.deliver_file(request, None);
        }
    }

    /// Takes the frame's dropped files while the picker is open: they are added to it, not to the document.
    pub(super) fn image_picker_takes_drops(&mut self, files: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        if self.image_edit.picker.is_none() {
            return Some(files);
        }
        let (images, others): (Vec<_>, Vec<_>) = files.into_iter().partition(|p| is_droppable(p));
        if !others.is_empty()
            && let Some(p) = &mut self.image_edit.picker
        {
            p.picker.set_note("Only image files can be added here.".into());
        }
        if !images.is_empty()
            && let Some(req) = self.attach_request()
        {
            let stage = self.job(req, images.into_iter().map(Item::File).collect());
            if let Some(p) = &mut self.image_edit.picker {
                p.adding.push(stage);
            }
        }
        None
    }

    fn image_picker_window(&mut self, ctx: &Context) {
        let Some(mut open) = self.image_edit.picker.take() else {
            return;
        };
        // what it was opened for is another text by now, unless a form's draft is what it serves
        if matches!(open.pick_for, PickFor::Image(_)) && self.block_edit_origin() != Some(open.origin) {
            return;
        }
        let mut still = Vec::new();
        for mut stage in std::mem::take(&mut open.adding) {
            let Some(out) = self.job_outcome(&mut stage) else {
                still.push(stage);
                continue;
            };
            if let Some(why) = out.errors.first() {
                open.picker.set_note(why.clone());
            }
            for t in out.targets {
                let rel = relative_to_folder(&t, &open.folder).unwrap_or_else(|| t.display().to_string());
                open.picker.add(t, rel);
            }
        }
        open.adding = still;
        let act = open.picker.show(ctx);
        let file = match act {
            None => {
                self.image_edit.picker = Some(open);
                return;
            }
            Some(PickerAct::Close) => None,
            Some(PickerAct::Choose(file)) => Some(file),
            Some(PickerAct::Browse) => {
                let picker = self.block_form.picker.clone();
                match picker(&open.folder) {
                    Some(file) => Some(file),
                    None => {
                        self.image_edit.picker = Some(open);
                        return;
                    }
                }
            }
        };
        match (open.pick_for.clone(), file) {
            (PickFor::Image(at), Some(file)) => self.replace_image_with(at, file),
            (PickFor::Image(_), None) => {}
            (PickFor::Form(request), file) => self.image_for_form(request, file, &open.folder),
        }
        if let Some(d) = &mut self.doc
            && matches!(open.pick_for, PickFor::Image(_))
        {
            d.viewer.request_refocus();
        }
    }

    /// What was chosen for a form: a file from outside its folder is copied by the attachment rules first.
    fn image_for_form(&mut self, request: FileRequest, file: Option<PathBuf>, folder: &Path) {
        let Some(file) = file else {
            if let Some(open) = self.block_form.open.as_mut() {
                open.deliver_file(request, None);
            }
            return;
        };
        if let Some(rel) = relative_to_folder(&file, folder) {
            if let Some(open) = self.block_form.open.as_mut() {
                open.deliver_file(request, Some(rel));
            }
            return;
        }
        let Some(req) = self.attach_request() else { return };
        let stage = self.job(req, vec![Item::File(file)]);
        self.image_edit.form_copying = Some((request, folder.to_path_buf(), stage));
    }

    fn image_form_copy_poll(&mut self) {
        let Some((request, folder, mut stage)) = self.image_edit.form_copying.take() else {
            return;
        };
        let Some(out) = self.job_outcome(&mut stage) else {
            self.image_edit.form_copying = Some((request, folder, stage));
            return;
        };
        let target = out.targets.first();
        let rel = target.and_then(|t| relative_to_folder(t, &folder));
        let Some(open) = self.block_form.open.as_mut() else {
            return;
        };
        if let Some(why) = out.errors.first() {
            open.set_message(why.clone());
        } else if let (Some(t), None) = (target, &rel) {
            open.set_message(format!(
                "{} is not inside the document's folder, so it was not used.",
                t.display()
            ));
        }
        open.deliver_file(request, rel);
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
        // what it is now, to show it; a document with no folder yet has only the system's dialog
        let current = self.doc.as_ref().and_then(|d| {
            let parts = d.viewer.ed.image_parts(at.clone())?;
            let dest = &d.viewer.ed.session.text()[parts.dest];
            Some(
                dest.strip_prefix('<')
                    .and_then(|t| t.strip_suffix('>'))
                    .unwrap_or(dest)
                    .to_string(),
            )
        });
        if self.open_image_picker(PickFor::Image(at.clone()), current) {
            return;
        }
        let picker = self.block_form.picker.clone();
        let Some(file) = picker(Path::new(".")) else { return };
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
        // it may have become read-only meanwhile, a review mode in Side by Side as anywhere
        if let Some(why) = self.image_edit_refused() {
            self.banner = Some(Banner::Info(why.into()));
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
        // nor is it to be saved once the document has become read-only
        if let Some(why) = self.image_edit_refused() {
            self.banner = Some(Banner::Info(why.into()));
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
