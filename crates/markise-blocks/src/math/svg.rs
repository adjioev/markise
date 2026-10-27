//! [`MathLayout`] → self-contained SVG (glyphs as outline paths, no font embedding).
//!
//! The SVG is sized in `em` of the surrounding text (times [`SvgOptions::scale`]) and shifted
//! with `vertical-align` so its baseline sits on the text baseline. Items without an explicit
//! TeX color are painted with `currentColor`, so the exporting theme's text color applies.

use std::fmt::Write;

use super::fonts::glyph_outline;
use super::{MathItem, MathLayout, MathMode, PathCommand, Rgba8};

/// User units per em in the SVG coordinate system.
const UNITS: f64 = 1000.0;

#[derive(Clone, Debug)]
pub struct SvgOptions {
    /// Size of one math em relative to the surrounding text em.
    pub scale: f32,
    /// Accessible label (normally the TeX source); emitted as `aria-label`.
    pub label: Option<String>,
    /// Keep a display-style formula in the text flow (Markdown `$$…$$` inside a paragraph);
    /// otherwise display mode is a centred block.
    pub inline_position: bool,
}

impl Default for SvgOptions {
    fn default() -> Self {
        SvgOptions {
            scale: super::DEFAULT_MATH_SCALE,
            label: None,
            inline_position: false,
        }
    }
}

fn num(out: &mut String, v: f64) {
    let r = (v * 10.0).round() / 10.0;
    if r == r.trunc() {
        let _ = write!(out, "{}", r as i64);
    } else {
        let _ = write!(out, "{r:.1}");
    }
}

fn em(out: &mut String, v: f64) {
    let r = (v * 10_000.0).round() / 10_000.0;
    let _ = write!(out, "{r}em");
}

fn paint(out: &mut String, attr: &str, c: &Option<Rgba8>) {
    if let Some([r, g, b, a]) = c {
        let _ = write!(out, " {attr}=\"#{r:02x}{g:02x}{b:02x}\"");
        if *a != 255 {
            let _ = write!(out, " {attr}-opacity=\"{:.3}\"", *a as f32 / 255.0);
        }
    }
}

fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// Appends path data for `cmds`, placed at (`ox`, `oy`) em and scaled by `s`.
fn path_data(out: &mut String, cmds: &[PathCommand], ox: f64, oy: f64, s: f64) {
    let p = |out: &mut String, x: f64, y: f64| {
        num(out, (ox + x * s) * UNITS);
        out.push(' ');
        num(out, (oy + y * s) * UNITS);
    };
    for c in cmds {
        match *c {
            PathCommand::MoveTo { x, y } => {
                out.push('M');
                p(out, x, y);
            }
            PathCommand::LineTo { x, y } => {
                out.push('L');
                p(out, x, y);
            }
            PathCommand::QuadTo { x1, y1, x, y } => {
                out.push('Q');
                p(out, x1, y1);
                out.push(' ');
                p(out, x, y);
            }
            PathCommand::CubicTo { x1, y1, x2, y2, x, y } => {
                out.push('C');
                p(out, x1, y1);
                out.push(' ');
                p(out, x2, y2);
                out.push(' ');
                p(out, x, y);
            }
            PathCommand::Close => out.push('Z'),
        }
    }
}

/// Renders `layout` as an `<svg>` element.
pub fn to_svg(layout: &MathLayout, opts: &SvgOptions) -> String {
    let s = opts.scale as f64;
    let sane = |v: f64| v.is_finite() && v.abs() <= super::MAX_EM;
    if !(sane(layout.width) && sane(layout.height) && sane(layout.depth)) {
        // never produced by `layout` (it refuses such formulas); defensive for hand-made input
        return "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\"></svg>".into();
    }
    let (w, h, d) = (layout.width.max(0.0), layout.height.max(0.0), layout.depth.max(0.0));
    let mut out = String::with_capacity(256 + layout.items.len() * 200);
    out.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"");
    em(&mut out, w * s);
    out.push_str("\" height=\"");
    em(&mut out, (h + d) * s);
    out.push_str("\" viewBox=\"0 0 ");
    num(&mut out, w * UNITS);
    out.push(' ');
    num(&mut out, (h + d) * UNITS);
    out.push_str("\" style=\"vertical-align:");
    em(&mut out, -d * s);
    if layout.mode == MathMode::Display && !opts.inline_position {
        out.push_str(";display:block;margin:0 auto");
    }
    out.push_str(";overflow:visible\" role=\"img\" focusable=\"false\"");
    if let Some(label) = &opts.label {
        out.push_str(" aria-label=\"");
        escape_attr(label, &mut out);
        out.push('"');
    }
    out.push_str("><g fill=\"currentColor\" stroke=\"none\">");
    for item in &layout.items {
        match item {
            MathItem::Glyph {
                x,
                y,
                scale,
                face,
                ch,
                color,
            } => {
                let Some(o) = glyph_outline(*face, *ch) else { continue };
                if o.commands.is_empty() {
                    continue;
                }
                out.push_str("<path d=\"");
                path_data(&mut out, &o.commands, *x, *y, *scale);
                out.push('"');
                paint(&mut out, "fill", color);
                out.push_str("/>");
            }
            MathItem::Missing { x, y, scale, ch, color } => {
                // the viewer's fonts draw it (best effort; the layout reserved KaTeX metrics)
                out.push_str("<text x=\"");
                num(&mut out, x * UNITS);
                out.push_str("\" y=\"");
                num(&mut out, y * UNITS);
                out.push_str("\" font-size=\"");
                num(&mut out, scale * UNITS);
                out.push('"');
                paint(&mut out, "fill", color);
                out.push('>');
                let mut b = [0u8; 4];
                escape_attr(ch.encode_utf8(&mut b), &mut out);
                out.push_str("</text>");
            }
            MathItem::Rect {
                x,
                y,
                w,
                h,
                color,
                dashed,
            } => {
                if *dashed {
                    out.push_str("<line x1=\"");
                    num(&mut out, x * UNITS);
                    out.push_str("\" x2=\"");
                    num(&mut out, (x + w) * UNITS);
                    out.push_str("\" y1=\"");
                    num(&mut out, (y + h / 2.0) * UNITS);
                    out.push_str("\" y2=\"");
                    num(&mut out, (y + h / 2.0) * UNITS);
                    out.push_str("\" stroke-width=\"");
                    num(&mut out, h * UNITS);
                    out.push_str("\" stroke-dasharray=\"");
                    num(&mut out, h * 3.0 * UNITS);
                    out.push('"');
                    if color.is_some() {
                        paint(&mut out, "stroke", color);
                    } else {
                        out.push_str(" stroke=\"currentColor\"");
                    }
                    out.push_str("/>");
                } else {
                    out.push_str("<rect x=\"");
                    num(&mut out, x * UNITS);
                    out.push_str("\" y=\"");
                    num(&mut out, y * UNITS);
                    out.push_str("\" width=\"");
                    num(&mut out, w * UNITS);
                    out.push_str("\" height=\"");
                    num(&mut out, h * UNITS);
                    out.push('"');
                    paint(&mut out, "fill", color);
                    out.push_str("/>");
                }
            }
            MathItem::Path {
                x,
                y,
                commands,
                fill,
                color,
            } => {
                out.push_str("<path d=\"");
                path_data(&mut out, commands, *x, *y, 1.0);
                out.push('"');
                if *fill {
                    paint(&mut out, "fill", color);
                } else {
                    out.push_str(" fill=\"none\" stroke-width=\"40\"");
                    if color.is_some() {
                        paint(&mut out, "stroke", color);
                    } else {
                        out.push_str(" stroke=\"currentColor\"");
                    }
                }
                out.push_str("/>");
            }
        }
    }
    out.push_str("</g></svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::super::layout;
    use super::*;

    #[test]
    fn svg_is_sized_in_em_with_baseline_shift() {
        let l = layout(r"\frac{a}{b}", MathMode::Inline).unwrap();
        let svg = to_svg(
            &l,
            &SvgOptions {
                scale: 1.0,
                label: Some(r"\frac{a}{b} <&>".into()),
                inline_position: false,
            },
        );
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\""));
        assert!(svg.contains("vertical-align:-0."), "{svg}");
        assert!(svg.contains("aria-label=\"\\frac{a}{b} &lt;&amp;&gt;\""));
        assert!(svg.contains("<rect"), "fraction bar");
        assert_eq!(svg.matches("<path").count(), 2, "{svg}");
        assert!(svg.ends_with("</g></svg>"));
        assert!(!svg.contains("NaN") && !svg.contains("inf"));
    }

    #[test]
    fn explicit_color_is_kept_and_default_is_current_color() {
        let l = layout(r"\textcolor{#ff0000}{x}y", MathMode::Inline).unwrap();
        let svg = to_svg(&l, &SvgOptions::default());
        assert!(svg.contains("fill=\"#ff0000\""));
        assert!(svg.contains("<g fill=\"currentColor\""));
    }

    #[test]
    fn oversized_layout_gives_an_empty_svg() {
        let mut l = layout("x", MathMode::Inline).unwrap();
        l.height = f64::INFINITY;
        let svg = to_svg(&l, &SvgOptions::default());
        assert!(!svg.contains("inf") && !svg.contains("NaN") && svg.contains("width=\"0\""));
    }

    #[test]
    fn inline_display_svg_stays_in_the_text_flow() {
        let l = layout(r"\sum_{i=1}^n x_i", MathMode::Display).unwrap();
        let opts = SvgOptions {
            inline_position: true,
            ..Default::default()
        };
        let svg = to_svg(&l, &opts);
        assert!(
            !svg.contains("display:block") && svg.contains("vertical-align:-"),
            "{svg}"
        );
    }

    #[test]
    fn display_svg_is_a_centred_block() {
        let l = layout(r"x", MathMode::Display).unwrap();
        assert!(to_svg(&l, &SvgOptions::default()).contains("display:block"));
    }

    #[test]
    fn missing_glyph_falls_back_to_text() {
        let l = layout(r"\text{я}", MathMode::Inline).unwrap();
        assert!(to_svg(&l, &SvgOptions::default()).contains(">я</text>"));
    }
}
