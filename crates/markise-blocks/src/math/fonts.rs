//! The bundled KaTeX fonts (OFL-1.1, unmodified; `assets/fonts/katex/SOURCE.md`) and glyph
//! outlines read from them with `skrifa`.
//!
//! The fonts are embedded (490 KiB): the RaTeX layout uses the KaTeX metrics, so outlines must
//! come from exactly these files. Outlines are converted once per (face, char) into em-unit path
//! commands (y down, origin at the glyph's baseline origin) and cached for the process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ratex_types::path_command::PathCommand;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};

macro_rules! faces {
    ($($variant:ident => $name:literal, $file:literal;)*) => {
        /// One of the bundled KaTeX faces.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum MathFace { $($variant,)* }

        impl MathFace {
            pub const ALL: &[MathFace] = &[$(MathFace::$variant,)*];

            /// RaTeX's name for the face (`"Main-Regular"`).
            pub fn ratex_name(self) -> &'static str {
                match self { $(MathFace::$variant => $name,)* }
            }

            pub fn from_ratex_name(s: &str) -> Option<MathFace> {
                match s { $($name => Some(MathFace::$variant),)* _ => None }
            }

            /// The font file name in `assets/fonts/katex/`.
            pub fn file_name(self) -> &'static str {
                match self { $(MathFace::$variant => $file,)* }
            }

            /// The embedded font bytes.
            pub fn data(self) -> &'static [u8] {
                match self {
                    $(MathFace::$variant => include_bytes!(concat!("../../../../assets/fonts/katex/", $file)),)*
                }
            }
        }
    };
}

faces! {
    AmsRegular => "AMS-Regular", "KaTeX_AMS-Regular.ttf";
    CaligraphicRegular => "Caligraphic-Regular", "KaTeX_Caligraphic-Regular.ttf";
    FrakturRegular => "Fraktur-Regular", "KaTeX_Fraktur-Regular.ttf";
    FrakturBold => "Fraktur-Bold", "KaTeX_Fraktur-Bold.ttf";
    MainBold => "Main-Bold", "KaTeX_Main-Bold.ttf";
    MainBoldItalic => "Main-BoldItalic", "KaTeX_Main-BoldItalic.ttf";
    MainItalic => "Main-Italic", "KaTeX_Main-Italic.ttf";
    MainRegular => "Main-Regular", "KaTeX_Main-Regular.ttf";
    MathBoldItalic => "Math-BoldItalic", "KaTeX_Math-BoldItalic.ttf";
    MathItalic => "Math-Italic", "KaTeX_Math-Italic.ttf";
    SansSerifBold => "SansSerif-Bold", "KaTeX_SansSerif-Bold.ttf";
    SansSerifItalic => "SansSerif-Italic", "KaTeX_SansSerif-Italic.ttf";
    SansSerifRegular => "SansSerif-Regular", "KaTeX_SansSerif-Regular.ttf";
    ScriptRegular => "Script-Regular", "KaTeX_Script-Regular.ttf";
    Size1Regular => "Size1-Regular", "KaTeX_Size1-Regular.ttf";
    Size2Regular => "Size2-Regular", "KaTeX_Size2-Regular.ttf";
    Size3Regular => "Size3-Regular", "KaTeX_Size3-Regular.ttf";
    Size4Regular => "Size4-Regular", "KaTeX_Size4-Regular.ttf";
    TypewriterRegular => "Typewriter-Regular", "KaTeX_Typewriter-Regular.ttf";
}

impl MathFace {
    fn font(self) -> Option<FontRef<'static>> {
        FontRef::new(self.data()).ok()
    }

    fn ratex_id(self) -> Option<ratex_font::FontId> {
        ratex_font::FontId::parse(self.ratex_name())
    }
}

/// The character to look up in the face's cmap for a display-list char code (KaTeX fonts key
/// mathematical alphanumerics by their ASCII letters).
pub fn ttf_char(face: MathFace, char_code: u32) -> char {
    match face.ratex_id() {
        Some(id) => ratex_font::katex_ttf_glyph_char(id, char_code),
        None => char::from_u32(char_code).unwrap_or('\u{fffd}'),
    }
}

/// Whether the face has a glyph for `ch`.
pub fn has_glyph(face: MathFace, ch: char) -> bool {
    face.font()
        .and_then(|f| f.charmap().map(ch))
        .is_some_and(|g| g.to_u32() != 0)
}

/// A glyph outline in em units: y grows downwards, (0, 0) is the glyph origin on the baseline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphOutline {
    pub commands: Vec<PathCommand>,
    /// Ink bounds `[x0, y0, x1, y1]` in em (y down); all zero for an empty glyph.
    pub bounds: [f64; 4],
}

struct Pen {
    scale: f64,
    out: Vec<PathCommand>,
    b: [f64; 4],
}

impl Pen {
    fn pt(&mut self, x: f32, y: f32) -> (f64, f64) {
        let (x, y) = (x as f64 * self.scale, -(y as f64) * self.scale);
        self.b = [self.b[0].min(x), self.b[1].min(y), self.b[2].max(x), self.b[3].max(y)];
        (x, y)
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.pt(x, y);
        self.out.push(PathCommand::MoveTo { x, y });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.pt(x, y);
        self.out.push(PathCommand::LineTo { x, y });
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (x1, y1) = self.pt(cx0, cy0);
        let (x, y) = self.pt(x, y);
        self.out.push(PathCommand::QuadTo { x1, y1, x, y });
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (x1, y1) = self.pt(cx0, cy0);
        let (x2, y2) = self.pt(cx1, cy1);
        let (x, y) = self.pt(x, y);
        self.out.push(PathCommand::CubicTo { x1, y1, x2, y2, x, y });
    }
    fn close(&mut self) {
        self.out.push(PathCommand::Close);
    }
}

type OutlineCache = Mutex<HashMap<(MathFace, char), Option<Arc<GlyphOutline>>>>;

static OUTLINES: OnceLock<OutlineCache> = OnceLock::new();

fn draw(face: MathFace, ch: char) -> Option<GlyphOutline> {
    let font = face.font()?;
    let gid = font.charmap().map(ch)?;
    if gid.to_u32() == 0 {
        return None;
    }
    let upem = font.metrics(Size::unscaled(), LocationRef::default()).units_per_em;
    if upem == 0 {
        return None;
    }
    let glyph = font.outline_glyphs().get(gid)?;
    let mut pen = Pen {
        scale: 1.0 / upem as f64,
        out: Vec::new(),
        b: [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
    };
    glyph
        .draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut pen,
        )
        .ok()?;
    let bounds = if pen.out.is_empty() { [0.0; 4] } else { pen.b };
    Some(GlyphOutline {
        commands: pen.out,
        bounds,
    })
}

/// The outline of `ch` in `face` (cached), or `None` when the face has no such glyph.
pub fn glyph_outline(face: MathFace, ch: char) -> Option<Arc<GlyphOutline>> {
    let cache = OUTLINES.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&(face, ch)) {
        return hit.clone();
    }
    let outline = draw(face, ch).map(Arc::new);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((face, ch), outline.clone());
    outline
}

/// Total size of the embedded font files.
pub fn embedded_bytes() -> usize {
    MathFace::ALL.iter().map(|f| f.data().len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_parses_and_maps_to_ratex() {
        for &f in MathFace::ALL {
            assert!(f.font().is_some(), "{f:?}");
            assert_eq!(MathFace::from_ratex_name(f.ratex_name()), Some(f));
            assert!(f.ratex_id().is_some(), "{f:?}");
        }
        assert!(embedded_bytes() > 400_000 && embedded_bytes() < 700_000);
    }

    #[test]
    fn outline_is_em_scaled_and_flipped() {
        let o = glyph_outline(MathFace::MainRegular, 'x').expect("x");
        assert!(!o.commands.is_empty());
        // 'x' sits on the baseline and reaches ~0.43 em up (y negative = above the baseline)
        assert!(o.bounds[1] < -0.3 && o.bounds[1] > -0.6, "{:?}", o.bounds);
        assert!(o.bounds[3].abs() < 0.05, "{:?}", o.bounds);
        assert!(glyph_outline(MathFace::MainRegular, 'Ж').is_none());
    }
}
