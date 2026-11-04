//! What the form asks of its host. The form never opens a dialog, reads a file or decodes an
//! image: it asks, and goes on drawing. The host decides how and answers later.

use egui::{Id, TextureId, Ui, Vec2};
use serde_json::Value;

/// Identifies a [`FormHost::request_file`]; give it back to [`crate::SchemaForm::deliver_file`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileRequest(pub(crate) u64);

/// A thumbnail as far as the host has got.
#[derive(Clone, Debug, PartialEq)]
pub enum Thumbnail {
    /// No picture to show.
    None,
    /// On its way. The host repaints when it arrives.
    Loading,
    Ready {
        texture: TextureId,
        size: Vec2,
    },
    /// Why there is no picture.
    Failed(String),
}

/// The host's side of files and thumbnails. Nothing here may block a frame.
pub trait FormHost {
    /// Whether the form offers a file picker.
    fn can_pick_files(&self) -> bool {
        true
    }
    /// Asks for a file, in the host's own dialog, off the frame. The answer goes to
    /// [`crate::SchemaForm::deliver_file`], with `None` if the person cancelled.
    fn request_file(&mut self, request: FileRequest, field_label: &str);
    /// The thumbnail of `path`, called every frame for every image with a path, so it must be
    /// cheap: start the loading once, answer [`Thumbnail::Loading`] meanwhile.
    fn thumbnail(&mut self, path: &str) -> Thumbnail;
}

/// A host with no files: no picker, no thumbnails.
pub struct NoHost;

impl FormHost for NoHost {
    fn can_pick_files(&self) -> bool {
        false
    }
    fn request_file(&mut self, _: FileRequest, _: &str) {}
    fn thumbnail(&mut self, _: &str) -> Thumbnail {
        Thumbnail::None
    }
}

/// What a host widget gets to work with.
pub struct WidgetContext<'a> {
    /// The field's value. A key missing from the values reads as `null`.
    pub value: &'a mut Value,
    pub label: &'a str,
    /// Derive ids from this; it stays with the field through reorders.
    pub id: Id,
    /// The field's reasons, already in words. The form shows them as well.
    pub errors: &'a [String],
}

/// A widget the host registers under a name ([`crate::SchemaForm::register_widget`]), the form's
/// only extension point. Hints refer to it by that name; the form knows nothing else about it.
pub trait HostWidget {
    /// Draws the field and returns whether it changed the value.
    fn ui(&mut self, ui: &mut Ui, cx: WidgetContext<'_>) -> bool;
}
