//! `markise-text` (L2): GUI-free text shaping and paragraph layout for Markise (ADR 0005).
//!
//! - [`FontStore`]: fontique collection (system fonts discovered on a worker thread), theme
//!   `@font-face` faces ([`FaceRule`]) loaded from files (confined, size-capped, WOFF/WOFF2
//!   unwrapped), `local()` via the adapter-owned [`FaceNameIndex`], font generation.
//! - CSS Fonts 4 face matching: face-set selection on descriptors ([`matching`]) and per-grapheme
//!   cluster matching with `unicode-range`, NFC and variation-selector rules ([`coverage`]).
//! - [`TextEngine::layout`] → [`TextLayout`]: parley shaping, UAX #14 breaking, bidi, line
//!   metrics, glyph runs for an atlas, grapheme-correct carets ([`LocalPos`]), hit testing both
//!   ways, selection rectangles, visual/logical/word/line moves.
//!
//! No egui types: rendering (glyph atlas) belongs to `markise-ui-egui` (ADR 0005 §3, T3).

pub mod coverage;
pub mod face;
mod layout;
pub mod matching;
pub mod names;
mod store;
pub mod style;
pub mod styled;
#[doc(hidden)]
pub mod testing;

pub use face::{FaceError, FaceRule, FaceSource, LoadPolicy, SourceError};
pub use layout::{
    Affinity, ClusterInfo, FontHandle, Glyph, GlyphRun, LineInfo, LocalPos, Rect, StyleBrush, TextEngine, TextLayout,
};
pub use matching::{FaceDescriptors, FaceSlope};
pub use names::{FaceLocation, FaceNameIndex};
pub use store::{FaceId, FontStore};
pub use style::{Family, Feature, Generic, Slope, TextStyle};
pub use styled::{BaseDir, InlineObject, StyleId, StyledText};
