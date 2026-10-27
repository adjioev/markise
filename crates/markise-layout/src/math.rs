//! Rendered math as far as layout is concerned (ADR 0008).
//!
//! Layout does not depend on the math renderer (`markise-blocks`): it asks the UI's
//! [`Images`](crate::Images) provider through [`Images::math`](crate::Images::math) with a
//! [`MathQuery`] (TeX, inline/display, pixel size, color) and gets a [`MathState`]. A ready
//! formula is placed as a [`Prim::Math`](crate::Prim::Math) bitmap; a pending, failed or
//! unavailable one keeps the styled-source rendering (failed: in the error color).
//!
//! Cache contract: items that may contain math ([`Item::has_math`](crate::Item::has_math)) carry
//! the view's math epoch in their cache key; the UI calls
//! [`View::math_changed`](crate::View::math_changed) when results arrive, so a formula is laid
//! out as source first and once more when its bitmap is ready.

use std::sync::Arc;

use epaint::Color32;

/// Math is drawn this much larger than the surrounding text (the KaTeX fonts have a small
/// x-height; `markise_blocks::math::DEFAULT_MATH_SCALE` has the same value).
pub const MATH_SCALE: f32 = 1.21;

/// One formula to show.
#[derive(Clone, Debug, PartialEq)]
pub struct MathQuery {
    pub tex: Arc<str>,
    /// Display style (math blocks) or text style (inline math).
    pub display: bool,
    /// Pixels per math em: font size × [`MATH_SCALE`] × pixels per point.
    pub px_per_em: f32,
    /// Color of the parts without an explicit TeX color.
    pub color: Color32,
    /// The widest the formula may be shown, in pixels (its column or table cell): the provider
    /// renders a wider formula scaled down to it (never at full size and shrunk afterwards) and
    /// must not make the bitmap of a formula that fits depend on this value.
    pub max_width: Option<f32>,
    /// Only the shown size is needed (natural-width measuring, e.g. of table columns): the
    /// provider answers from the formula's metrics without rendering pixels; the returned
    /// [`MathBox`] has no bitmap (`key` 0, `size` 0) and is never painted.
    pub measure: bool,
}

/// A rendered formula's bitmap geometry, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathBox {
    /// Identifies the bitmap for the painter (the renderer's cache key).
    pub key: u64,
    /// Bitmap size, including `pad` on every side.
    pub size: [u32; 2],
    /// Top of the bitmap to the baseline.
    pub baseline: f32,
    pub pad: f32,
    /// Advance width, ascent and descent of the formula box (without padding).
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
}

/// Render state of a [`MathQuery`].
#[derive(Clone, Debug, PartialEq)]
pub enum MathState {
    /// No math renderer: show the source.
    Unavailable,
    /// Queued or rendering: show the source for now.
    Pending,
    /// Nothing to draw (empty formula).
    Empty,
    /// Invalid TeX: show the source in the error style.
    Failed(String),
    Ready(MathBox),
}
