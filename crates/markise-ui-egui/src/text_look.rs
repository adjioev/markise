//! How the epaint glyph atlas turns outlines into pixels (issue #172): hinting and the
//! coverage-to-alpha curve, per theme brightness and display density.
//!
//! egui hints every glyph with the font's own instructions and only then rasterises it. At 2×
//! that adds nothing but distortion: Arial's horizontal strokes (2.3 px at 16 pt) are rounded up
//! to 3 px while the vertical ones stay at 2.8 px, and the glyphs lose up to 2 px of height, so
//! stroke-rich text such as Cyrillic looks squat and heavy. macOS and browsers do not do this.
//! On high-density displays Markise therefore rasterises the plain outline; low-density displays
//! keep egui's hinting, where it is what keeps text sharp.
//!
//! Without the rounding, dark-on-light text would be thinner than it was and further from the
//! system renderer, so light themes use a mild coverage curve there that restores the previous
//! overall ink. Dark themes keep egui's curve. Measurements: `docs/diagnostics.md`.

use egui::epaint::FontColorTransferFunction as Transfer;
use egui::epaint::text::TextOptions;

/// Pixels per point from which a display counts as high density (zoom included, so that zooming
/// a 2× display out does not bring the distortion back).
pub const HIGH_DENSITY_PPP: f32 = 1.5;

/// Coverage exponent for light themes on high-density displays.
const LIGHT_THEME_GAMMA: f32 = 0.7;

/// The coverage curve, `MARKISE_TEXT_GAMMA`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Curve {
    /// Markise's choice per theme brightness and density.
    #[default]
    Default,
    /// egui's stock curve for the theme's brightness.
    Stock,
    /// This curve in every theme.
    Fixed(Transfer),
}

/// Diagnostic overrides of the defaults, read once at start (see `docs/diagnostics.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextTuning {
    pub curve: Curve,
    /// Replaces hinting on every display, `MARKISE_TEXT_HINTING`.
    pub hinting: Option<bool>,
}

impl TextTuning {
    pub fn from_env() -> Self {
        Self::parse(
            std::env::var("MARKISE_TEXT_GAMMA").ok().as_deref(),
            std::env::var("MARKISE_TEXT_HINTING").ok().as_deref(),
        )
    }

    /// `gamma`: `egui`, `off` (alpha = coverage) or an exponent; `hinting`: `1` or `0`. Anything
    /// else keeps the default.
    fn parse(gamma: Option<&str>, hinting: Option<&str>) -> Self {
        let curve = match gamma {
            Some("egui") => Curve::Stock,
            Some("off") => Curve::Fixed(Transfer::Off),
            Some(n) => n
                .parse::<f32>()
                .ok()
                .filter(|e| e.is_finite() && *e > 0.0)
                .map_or(Curve::Default, |e| Curve::Fixed(Transfer::Gamma(e))),
            None => Curve::Default,
        };
        let hinting = match hinting {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        };
        TextTuning { curve, hinting }
    }

    /// Adjusts `options`, holding egui's stock values for a theme of this brightness, to this
    /// display density (pixels per point, zoom included).
    pub fn apply(self, options: &mut TextOptions, dark: bool, ppp: f32) {
        let high_density = is_high_density(ppp);
        options.font_hinting = self.hinting.unwrap_or(!high_density);
        match self.curve {
            Curve::Default if high_density && !dark => {
                options.color_transfer_function = Transfer::Gamma(LIGHT_THEME_GAMMA);
            }
            Curve::Default | Curve::Stock => {}
            Curve::Fixed(t) => options.color_transfer_function = t,
        }
    }
}

/// Whether `ppp` counts as a high-density display.
pub fn is_high_density(ppp: f32) -> bool {
    ppp >= HIGH_DENSITY_PPP
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(t: TextTuning, dark: bool, ppp: f32) -> TextOptions {
        let mut o = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        }
        .text_options;
        t.apply(&mut o, dark, ppp);
        o
    }

    #[test]
    fn defaults_per_theme_brightness_and_density() {
        let d = TextTuning::default();
        let dark_2x = options(d, true, 2.0);
        assert!(!dark_2x.font_hinting);
        assert_eq!(dark_2x.color_transfer_function, Transfer::DARK_MODE_DEFAULT);
        let light_2x = options(d, false, 2.0);
        assert!(!light_2x.font_hinting);
        assert_eq!(light_2x.color_transfer_function, Transfer::Gamma(0.7));
        // low density: egui's stock settings, unchanged
        for dark in [true, false] {
            let stock = if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            }
            .text_options;
            assert_eq!(options(d, dark, 1.0), stock);
            assert!(stock.font_hinting);
        }
    }

    #[test]
    fn the_stock_curve_and_hinting_restore_egui_at_2x() {
        let t = TextTuning::parse(Some("egui"), Some("1"));
        for dark in [true, false] {
            let stock = if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            }
            .text_options;
            assert_eq!(options(t, dark, 2.0), stock);
        }
    }

    #[test]
    fn switch_values() {
        assert_eq!(TextTuning::parse(None, None), TextTuning::default());
        assert_eq!(TextTuning::parse(Some("off"), None).curve, Curve::Fixed(Transfer::Off));
        assert_eq!(
            TextTuning::parse(Some("0.8"), Some("0")),
            TextTuning {
                curve: Curve::Fixed(Transfer::Gamma(0.8)),
                hinting: Some(false),
            }
        );
        for bad in ["", "x", "-1", "0", "NaN", "inf"] {
            assert_eq!(TextTuning::parse(Some(bad), None), TextTuning::default(), "{bad:?}");
        }
        for bad in ["", "x", "2"] {
            assert_eq!(TextTuning::parse(None, Some(bad)), TextTuning::default(), "{bad:?}");
        }
    }

    #[test]
    fn dark_theme_curve_lifts_partial_coverage_and_light_theme_curve_less_so() {
        let alpha = |dark, c| {
            options(TextTuning::default(), dark, 2.0)
                .color_transfer_function
                .alpha_from_coverage(c)
        };
        assert!((alpha(true, 0.5) - 0.75).abs() < 1e-4);
        assert!((alpha(false, 0.5) - 0.5f32.powf(0.7)).abs() < 1e-4);
        assert!(alpha(false, 0.5) < alpha(true, 0.5));
        for dark in [true, false] {
            assert_eq!(alpha(dark, 0.0), 0.0);
            assert_eq!(alpha(dark, 1.0), 1.0);
        }
    }
}
