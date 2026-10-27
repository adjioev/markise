//! TeX math rendering (spec 01 §3.7, §4.1–4.2; ADR 0008).
//!
//! The pipeline is GUI-free and renderer-neutral:
//!
//! 1. [`layout`] parses TeX with RaTeX (`ratex-parser`, MIT) and lays it out with its KaTeX-style
//!    engine (`ratex-layout`, which carries the KaTeX font metrics). The result is a
//!    [`MathLayout`]: a flat display list of glyphs, rules and paths in em units with the
//!    baseline, height and depth — no font file is needed for this step.
//! 2. Renderers turn a [`MathLayout`] into output using the glyph outlines of the bundled KaTeX
//!    fonts ([`fonts`], OFL-1.1, `assets/fonts/katex/`):
//!    - [`svg::to_svg`] — a self-contained SVG string (HTML export, copy as SVG);
//!    - [`raster::rasterize`] — an RGBA bitmap through `vello_cpu` (the egui viewer, PNG export);
//!    - PDF export can walk the same display list with [`fonts::glyph_outline`] (vector paths).
//! 3. [`MathService`] runs layout + rasterisation on a background thread with a byte-bounded
//!    result cache keyed by source, mode, size and color (NFR: never on the UI thread).
//!
//! Invalid TeX gives a [`MathError`] (message + byte offset when known); callers show the source
//! with an error style (spec 04: no fake render).

pub mod fonts;
pub mod raster;
mod service;
pub mod svg;

use std::panic::{AssertUnwindSafe, catch_unwind};

use ratex_types::color::Color;
use ratex_types::display_item::DisplayItem;

pub use raster::MathImage;
pub use ratex_types::path_command::PathCommand;
pub use service::{
    MathImageLookup, MathKey, MathMeasure, MathRequest, MathService, MathServiceStats, key_of, width_bucket,
};

/// Inline (`$…$`, text style) or display (`$$…$$` blocks, display style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MathMode {
    Inline,
    Display,
}

/// Formulas longer than this are not laid out (error result); keeps a pathological paste from
/// stalling the worker. Real formulas are far below it.
pub const MAX_TEX_BYTES: usize = 64 * 1024;

/// Display-list items above this are refused (RaTeX's own output budget is 65 536).
pub const MAX_ITEMS: usize = 65_536;

/// Largest formula box side in em (a 1000-row matrix is ~1500 em; `\rule{1e20em}{1em}` is
/// refused): keeps every renderer's arithmetic finite and bounded.
pub const MAX_EM: f64 = 2_000.0;

/// Math is drawn this much larger than the surrounding text (the KaTeX fonts have a small
/// x-height compared with typical UI/body fonts; KaTeX's own default is the same factor).
pub const DEFAULT_MATH_SCALE: f32 = 1.21;

/// An RGBA color in 0–255, straight alpha.
pub type Rgba8 = [u8; 4];

/// One drawing item of a laid-out formula. Coordinates are in em, y grows downwards, the origin
/// is the top-left corner of the formula box (the baseline is at `y = height`).
#[derive(Clone, Debug, PartialEq)]
pub enum MathItem {
    /// A glyph of a bundled KaTeX face, its baseline origin at (`x`, `y`), drawn at `scale` em.
    Glyph {
        x: f64,
        y: f64,
        scale: f64,
        face: fonts::MathFace,
        ch: char,
        color: Option<Rgba8>,
    },
    /// A character none of the bundled faces has (e.g. Cyrillic inside `\text{}`); renderers
    /// draw a placeholder box of the given advance (system-font fallback: ADR 0008 open item).
    Missing {
        x: f64,
        y: f64,
        scale: f64,
        ch: char,
        color: Option<Rgba8>,
    },
    /// A filled axis-aligned rectangle (fraction bars, rules, `\colorbox` backgrounds).
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        color: Option<Rgba8>,
        dashed: bool,
    },
    /// A path relative to (`x`, `y`) in em (radical signs, stretchy delimiters and braces).
    Path {
        x: f64,
        y: f64,
        commands: Vec<PathCommand>,
        fill: bool,
        color: Option<Rgba8>,
    },
}

/// A laid-out formula. `color: None` on an item means "the current text color".
#[derive(Clone, Debug, PartialEq)]
pub struct MathLayout {
    pub mode: MathMode,
    /// Advance width in em.
    pub width: f64,
    /// Ascent above the baseline in em.
    pub height: f64,
    /// Descent below the baseline in em.
    pub depth: f64,
    pub items: Vec<MathItem>,
}

impl MathLayout {
    /// Total height (ascent + descent) in em.
    pub fn total_height(&self) -> f64 {
        self.height + self.depth
    }

    /// Rough heap size, for cache budgets.
    pub fn approx_bytes(&self) -> usize {
        let paths: usize = self
            .items
            .iter()
            .map(|i| match i {
                MathItem::Path { commands, .. } => commands.len() * std::mem::size_of::<PathCommand>(),
                _ => 0,
            })
            .sum();
        std::mem::size_of::<Self>() + self.items.len() * std::mem::size_of::<MathItem>() + paths
    }
}

/// Invalid or unsupported TeX.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathError {
    pub message: String,
    /// Byte offset into the TeX source, when the parser knows it.
    pub offset: Option<usize>,
}

impl std::fmt::Display for MathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.offset {
            Some(o) => write!(f, "{} (at byte {o})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for MathError {}

fn err(message: impl Into<String>) -> MathError {
    MathError {
        message: message.into(),
        offset: None,
    }
}

/// The layout color that stands for "current text color". RaTeX needs a concrete color; this
/// value is not producible from TeX color syntax in practice (`\color` takes names, `#rrggbb`
/// or 8-bit-per-channel models), so a match maps back to `None`.
const INHERIT: Color = Color {
    r: 0.012_345_67,
    g: 0.023_456_78,
    b: 0.034_567_89,
    a: 1.0,
};

fn map_color(c: &Color) -> Option<Rgba8> {
    if *c == INHERIT {
        return None;
    }
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some([q(c.r), q(c.g), q(c.b), q(c.a)])
}

/// Parses and lays out `tex`. Pure function of its inputs (cache-friendly); typical formulas
/// take 5–300 µs in release builds (ADR 0008).
pub fn layout(tex: &str, mode: MathMode) -> Result<MathLayout, MathError> {
    if tex.len() > MAX_TEX_BYTES {
        return Err(err(format!(
            "formula too long ({} bytes; limit {MAX_TEX_BYTES})",
            tex.len()
        )));
    }
    if tex.trim().is_empty() {
        return Ok(MathLayout {
            mode,
            width: 0.0,
            height: 0.0,
            depth: 0.0,
            items: Vec::new(),
        });
    }
    // RaTeX is third-party code fed with untrusted document text: a panic there must not take
    // the worker (or an export) down.
    let res = catch_unwind(AssertUnwindSafe(|| layout_inner(tex, mode)));
    res.unwrap_or_else(|_| Err(err("internal error in the math layout engine")))
}

fn layout_inner(tex: &str, mode: MathMode) -> Result<MathLayout, MathError> {
    let ast = ratex_parser::parse(tex).map_err(|e| MathError {
        message: e.message.clone(),
        offset: e.loc.as_ref().map(|l| l.start).filter(|&o| o <= tex.len()),
    })?;
    let opts = ratex_layout::LayoutOptions {
        style: match mode {
            MathMode::Inline => ratex_types::math_style::MathStyle::Text,
            MathMode::Display => ratex_types::math_style::MathStyle::Display,
        },
        color: INHERIT,
        ..Default::default()
    };
    let lbox = ratex_layout::layout(&ast, &opts);
    ratex_layout::validate_layout(&lbox).map_err(err)?;
    let dl = ratex_layout::to_display_list(&lbox);
    if dl.items.len() > MAX_ITEMS {
        return Err(err("formula too complex"));
    }
    let ok = |v: f64| v.is_finite() && v.abs() <= MAX_EM;
    if !(ok(dl.width) && ok(dl.height) && ok(dl.depth) && ok(dl.height + dl.depth)) {
        return Err(err(format!("formula too large (limit {MAX_EM} em)")));
    }
    let mut items = Vec::with_capacity(dl.items.len());
    for it in dl.items {
        items.push(match it {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font,
                char_code,
                color,
            } => {
                let color = map_color(&color);
                match fonts::MathFace::from_ratex_name(&font) {
                    Some(face) => {
                        let ch = fonts::ttf_char(face, char_code);
                        if fonts::has_glyph(face, ch) {
                            MathItem::Glyph {
                                x,
                                y,
                                scale,
                                face,
                                ch,
                                color,
                            }
                        } else {
                            MathItem::Missing { x, y, scale, ch, color }
                        }
                    }
                    None => MathItem::Missing {
                        x,
                        y,
                        scale,
                        ch: char::from_u32(char_code).unwrap_or('\u{fffd}'),
                        color,
                    },
                }
            }
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                color,
                dashed,
            } => {
                // `y` is the centre line of the rule
                MathItem::Rect {
                    x,
                    y: y - thickness / 2.0,
                    w: width,
                    h: thickness,
                    color: map_color(&color),
                    dashed,
                }
            }
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                color,
            } => MathItem::Rect {
                x,
                y,
                w: width,
                h: height,
                color: map_color(&color),
                dashed: false,
            },
            DisplayItem::Path {
                x,
                y,
                commands,
                fill,
                color,
            } => MathItem::Path {
                x,
                y,
                commands,
                fill,
                color: map_color(&color),
            },
        });
    }
    // every coordinate finite and near the box (ink may stick out a little, never by 1e20 em)
    let near = |v: f64| v.is_finite() && v.abs() <= 4.0 * MAX_EM;
    let path_ok = |cmds: &[PathCommand]| {
        cmds.iter().all(|c| match *c {
            PathCommand::MoveTo { x, y } | PathCommand::LineTo { x, y } => near(x) && near(y),
            PathCommand::QuadTo { x1, y1, x, y } => near(x1) && near(y1) && near(x) && near(y),
            PathCommand::CubicTo { x1, y1, x2, y2, x, y } => {
                near(x1) && near(y1) && near(x2) && near(y2) && near(x) && near(y)
            }
            PathCommand::Close => true,
        })
    };
    let items_ok = items.iter().all(|i| match i {
        MathItem::Glyph { x, y, scale, .. } | MathItem::Missing { x, y, scale, .. } => {
            near(*x) && near(*y) && scale.is_finite() && *scale >= 0.0 && *scale <= 100.0
        }
        MathItem::Rect { x, y, w, h, .. } => near(*x) && near(*y) && near(*w) && near(*h),
        MathItem::Path { x, y, commands, .. } => near(*x) && near(*y) && path_ok(commands),
    });
    if !items_ok {
        return Err(err(format!("formula too large (limit {MAX_EM} em)")));
    }
    Ok(MathLayout {
        mode,
        width: dl.width,
        height: dl.height,
        depth: dl.depth,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_formula_has_glyphs_and_metrics() {
        let l = layout("x^2", MathMode::Inline).unwrap();
        assert!(l.width > 0.5 && l.height > 0.5, "{l:?}");
        let glyphs = l.items.iter().filter(|i| matches!(i, MathItem::Glyph { .. })).count();
        assert_eq!(glyphs, 2);
        assert!(l.items.iter().all(|i| match i {
            MathItem::Glyph { color, .. } => color.is_none(),
            _ => true,
        }));
    }

    #[test]
    fn display_mode_is_larger_than_inline() {
        let i = layout(r"\sum_{n=1}^\infty \frac{1}{n^2}", MathMode::Inline).unwrap();
        let d = layout(r"\sum_{n=1}^\infty \frac{1}{n^2}", MathMode::Display).unwrap();
        assert!(d.total_height() > i.total_height());
    }

    #[test]
    fn fraction_has_a_rule_and_sqrt_a_path_or_glyph() {
        let l = layout(r"\frac{a}{b}", MathMode::Display).unwrap();
        assert!(l.items.iter().any(|i| matches!(i, MathItem::Rect { .. })));
        let s = layout(r"\sqrt{\frac{a}{b}}", MathMode::Display).unwrap();
        assert!(s.items.len() > l.items.len());
    }

    #[test]
    fn color_command_yields_explicit_color() {
        let l = layout(r"\textcolor{red}{x} y", MathMode::Inline).unwrap();
        let colors: Vec<_> = l
            .items
            .iter()
            .filter_map(|i| match i {
                MathItem::Glyph { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(colors.len(), 2);
        assert!(colors.contains(&None));
        assert!(colors.iter().any(|c| matches!(c, Some([r, 0, 0, 255]) if *r > 200)));
    }

    #[test]
    fn invalid_tex_is_an_error_with_offset() {
        let e = layout(r"\frac{1}{2", MathMode::Inline).unwrap_err();
        assert!(!e.message.is_empty());
        let e = layout(r"a + \foo{x}", MathMode::Inline).unwrap_err();
        assert!(e.message.contains("foo"), "{e}");
        assert_eq!(e.offset, Some(4));
    }

    #[test]
    fn empty_and_oversized_input() {
        let l = layout("  ", MathMode::Display).unwrap();
        assert!(l.items.is_empty() && l.width == 0.0);
        let big = "x+".repeat(MAX_TEX_BYTES);
        assert!(layout(&big, MathMode::Inline).is_err());
    }

    #[test]
    fn common_latex_coverage() {
        // ADR 0008 coverage list: everything here must lay out without error.
        for tex in [
            r"\frac{a}{b}",
            r"\sqrt[3]{x}",
            r"a_{i,j}^{(k)}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}",
            r"\begin{aligned} f(x) &= x^2 \\ &= y \end{aligned}",
            r"\begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
            r"\begin{align*} a &= b \end{align*}",
            r"\text{if } x \in \mathbb{R}",
            r"\alpha\beta\Gamma\Omega",
            r"\sum_{i=1}^n \int_0^1 \lim_{x\to 0} \prod",
            r"\left( \frac{a}{b} \right] \left\{ x \right.",
            r"\hat{x} \vec{v} \overline{AB} \underbrace{x+y}_{2}",
            r"\operatorname{arg\,max} \mathcal{L} \mathfrak{g} \mathbf{v}",
            r"\binom{n}{k} \xrightarrow{f} \boxed{E=mc^2}",
            r"a \equiv b \pmod{n}",
        ] {
            assert!(layout(tex, MathMode::Display).is_ok(), "{tex}");
        }
    }

    #[test]
    fn cyrillic_text_is_reported_as_missing_glyph() {
        let l = layout(r"\text{да}", MathMode::Inline).unwrap();
        assert!(l.items.iter().any(|i| matches!(i, MathItem::Missing { .. })), "{l:?}");
    }

    #[test]
    fn huge_dimensions_are_an_error() {
        // review P2 #1: a 1e20 em rule used to overflow the rasteriser and kill the worker
        for tex in [
            r"\rule{100000000000000000000em}{1em}",
            r"\rule{1em}{100000000000000000000em}",
            r"\rule{3000em}{1em}",
            r"\kern{100000000000000000000em}x",
            r"x\raisebox{100000000000000000000em}{y}",
        ] {
            match layout(tex, MathMode::Display) {
                Err(e) => assert!(!e.message.is_empty(), "{tex}"),
                Ok(l) => panic!("{tex} laid out: {} × {}", l.width, l.total_height()),
            }
        }
        assert!(layout(r"\rule{10em}{1em}", MathMode::Display).is_ok());
    }

    #[test]
    fn adversarial_nesting_does_not_crash() {
        let deep = format!("{}x{}", "{".repeat(5000), "}".repeat(5000));
        let _ = layout(&deep, MathMode::Inline);
        let fr = format!("{}x{}", r"\frac{".repeat(500), "}{y}".repeat(500));
        let _ = layout(&fr, MathMode::Inline);
        let mac = r"\def\a{\a\a}\a";
        assert!(layout(mac, MathMode::Inline).is_err());
    }
}
