//! `markise-layout` — block layout, layout cache and viewport virtualisation (layer L2).
//!
//! Design (see `docs/adr/0002-ui-layout.md`):
//! - Layout depends only on the format-neutral seam of `markise-doc` (ADR 0006 §1): the
//!   document is read through `&dyn FormatDoc` (flatten, inline, table and heading views),
//!   never through a format's own types.
//! - A document is flattened into leaf [`Item`]s ([`model`]); each is laid out independently
//!   into a [`BlockLayout`] of positioned primitives ([`block`]); text is shaped through the
//!   backend-neutral [`TextShaper`] / [`TextBlock`] seam ([`text`], ADR 0005 T2): epaint galleys
//!   by default, parley (`markise-text`) as the developer-switch alternative.
//! - Layouts are cached per item, keyed by (item content hash, document context hash, width,
//!   theme fingerprint, pixels-per-point, …) ([`cache`]).
//! - [`View`] keeps the scroll anchor and a Fenwick tree of item heights (estimated until laid
//!   out) and lays out only the items that intersect the viewport plus a margin ([`view`]).

pub mod block;
pub mod cache;
pub mod caret;
pub mod diagram;
pub mod fenwick;
pub mod highlight;
pub mod links;
pub mod math;
pub mod model;
pub mod source;
pub mod text;
pub mod view;

pub use block::{BlockLayout, FontSet, ImageState, Images, ItemFocus, LinkHit, Prim, RoleFaces, reveal_set};
pub use cache::{CacheKey, LayoutCache};
pub use diagram::{DiagramQuery, DiagramState, Diagrams, NoDiagrams};
pub use highlight::{CodeQuery, Highlights, NoHighlights};
pub use links::{AllResolved, LinkTargets};
pub use math::{MATH_SCALE, MathBox, MathQuery, MathState};
pub use model::{CodeRef, Container, DocModel, Gap, Item, ItemKind, Items, Marker, ModelChange, OutlineEntry};
pub use source::{SourceCtx, SourceLayout, SourceLine, SourceParams, SourceView, SourceVisible};
pub use text::{
    EpaintShaper, FontRole, FragmentId, FragmentKind, MapPiece, ParleyShaper, PieceKind, SourceMap, StyledText,
    TextBlock, TextShaper, TextStyle,
};
pub use view::{View, Visible};

/// Font family names the layout uses; the UI must register all of them
/// (see [`block::register_families`]).
pub const FAMILY_BODY: &str = "markise-body";
pub const FAMILY_BOLD: &str = "markise-bold";
pub const FAMILY_HEADING: &str = "markise-heading";
pub const FAMILY_HEADING_BOLD: &str = "markise-heading-bold";
pub const FAMILY_MONO: &str = "markise-mono";
pub const FAMILY_MONO_BOLD: &str = "markise-mono-bold";
/// Sidebar items (UI font unless the theme styles the sidebar font).
pub const FAMILY_SIDEBAR: &str = "markise-sidebar";
pub const FAMILY_SIDEBAR_BOLD: &str = "markise-sidebar-bold";

/// Helpers for tests and headless measurements: a CPU-only epaint font system with the
/// Markise families registered (egui's bundled fonts only), and a parley shaper over them.
pub mod testing {
    use std::sync::Arc;

    use epaint::text::{FontDefinitions, Fonts, TextOptions};
    use markise_doc::FormatDoc;
    use markise_theme::Theme;

    use crate::{DocModel, View};

    /// A document with its view model (tests, headless tools). Dereferences to the model.
    #[derive(Clone)]
    pub struct Shown {
        pub doc: Arc<dyn FormatDoc>,
        pub model: Arc<DocModel>,
    }

    impl Shown {
        pub fn new(doc: Arc<dyn FormatDoc>) -> Self {
            let model = DocModel::build(&*doc, None);
            Shown { doc, model }
        }

        /// A view of the document.
        pub fn view(&self, theme: Theme, width: f32, ppp: f32) -> View {
            View::new(self.doc.clone(), self.model.clone(), theme, width, ppp)
        }
    }

    impl std::ops::Deref for Shown {
        type Target = DocModel;
        fn deref(&self) -> &DocModel {
            &self.model
        }
    }

    /// The text options of [`fonts`] (pass them to `Fonts::begin_pass`).
    pub fn text_options() -> TextOptions {
        TextOptions {
            max_texture_side: 8192,
            ..Default::default()
        }
    }

    /// egui's bundled fonts with the Markise families registered.
    pub fn font_definitions() -> FontDefinitions {
        let mut defs = FontDefinitions::default();
        crate::block::register_families(&mut defs, &crate::block::FontSet::default());
        defs
    }

    pub fn fonts() -> Fonts {
        Fonts::new(text_options(), font_definitions())
    }

    /// A parley shaper over the same bundled fonts (no system fonts: deterministic).
    pub fn parley() -> crate::ParleyShaper {
        crate::ParleyShaper::from_font_definitions(&font_definitions())
    }
}
