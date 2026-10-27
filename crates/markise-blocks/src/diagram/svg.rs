//! [`Scene`] → standalone inline SVG (HTML export, spec 03 `.md-diagram-panel`).
//!
//! The output only contains `svg`, `g`, `rect`, `ellipse`, `path` and `text` elements with
//! presentation attributes: no scripts, links, `foreignObject`, external references or CSS, so
//! it is safe to inline in an exported document regardless of the diagram source.

use std::fmt::Write as _;

use markise_theme::Rgba;

use super::palette::DiagramPalette;
use super::scene::{Anchor, Item, Paint, Scene, Seg, Stroke};

fn num(out: &mut String, v: f32) {
    let v = if v.is_finite() { v } else { 0.0 };
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        let _ = write!(out, "{}", r as i64);
    } else {
        let _ = write!(out, "{r}");
    }
}

fn color(out: &mut String, attr: &str, c: Rgba) {
    let [r, g, b, a] = c.0;
    let _ = write!(out, " {attr}=\"#{r:02x}{g:02x}{b:02x}\"");
    if a < 255 {
        let _ = write!(out, " {attr}-opacity=\"{:.3}\"", a as f32 / 255.0);
    }
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if (c as u32) < 0x20 && c != '\t' => out.push(' '),
            c => out.push(c),
        }
    }
}

fn paint_attrs(out: &mut String, pal: &DiagramPalette, fill: Option<Paint>, stroke: Option<Stroke>) {
    match fill {
        Some(p) => color(out, "fill", pal.resolve(p)),
        None => out.push_str(" fill=\"none\""),
    }
    if let Some(s) = stroke {
        color(out, "stroke", pal.resolve(s.paint));
        out.push_str(" stroke-width=\"");
        num(out, s.width);
        out.push('"');
        if let Some([d, g]) = s.dash {
            out.push_str(" stroke-dasharray=\"");
            num(out, d);
            out.push(' ');
            num(out, g);
            out.push('"');
        }
    }
}

/// Renders `scene` as an `<svg>` element. `font_family` is a CSS font-family list (e.g. the
/// theme's body stack, already joined); it is escaped.
pub fn to_svg(scene: &Scene, pal: &DiagramPalette, font_family: &str) -> String {
    let mut o = String::with_capacity(256 + scene.items.len() * 96);
    o.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"");
    num(&mut o, scene.width);
    o.push_str("\" height=\"");
    num(&mut o, scene.height);
    o.push_str("\" viewBox=\"0 0 ");
    num(&mut o, scene.width);
    o.push(' ');
    num(&mut o, scene.height);
    o.push_str("\" style=\"max-width:100%;height:auto\" role=\"img\" font-family=\"");
    escape(font_family, &mut o);
    o.push_str("\">");
    for it in &scene.items {
        match it {
            Item::Rect {
                rect,
                radius,
                fill,
                stroke,
            } => {
                o.push_str("<rect x=\"");
                num(&mut o, rect[0]);
                o.push_str("\" y=\"");
                num(&mut o, rect[1]);
                o.push_str("\" width=\"");
                num(&mut o, rect[2].max(0.0));
                o.push_str("\" height=\"");
                num(&mut o, rect[3].max(0.0));
                o.push('"');
                if *radius > 0.0 {
                    o.push_str(" rx=\"");
                    num(&mut o, *radius);
                    o.push('"');
                }
                paint_attrs(&mut o, pal, *fill, *stroke);
                o.push_str("/>");
            }
            Item::Ellipse {
                center,
                radii,
                fill,
                stroke,
            } => {
                o.push_str("<ellipse cx=\"");
                num(&mut o, center[0]);
                o.push_str("\" cy=\"");
                num(&mut o, center[1]);
                o.push_str("\" rx=\"");
                num(&mut o, radii[0].max(0.0));
                o.push_str("\" ry=\"");
                num(&mut o, radii[1].max(0.0));
                o.push('"');
                paint_attrs(&mut o, pal, *fill, *stroke);
                o.push_str("/>");
            }
            Item::Path { segs, fill, stroke } => {
                o.push_str("<path d=\"");
                for s in segs {
                    match *s {
                        Seg::MoveTo(p) => {
                            o.push('M');
                            num(&mut o, p[0]);
                            o.push(' ');
                            num(&mut o, p[1]);
                        }
                        Seg::LineTo(p) => {
                            o.push('L');
                            num(&mut o, p[0]);
                            o.push(' ');
                            num(&mut o, p[1]);
                        }
                        Seg::CubicTo(a, b, c) => {
                            o.push('C');
                            for (i, p) in [a, b, c].iter().enumerate() {
                                if i > 0 {
                                    o.push(' ');
                                }
                                num(&mut o, p[0]);
                                o.push(' ');
                                num(&mut o, p[1]);
                            }
                        }
                        Seg::Close => o.push('Z'),
                    }
                }
                o.push('"');
                paint_attrs(&mut o, pal, *fill, *stroke);
                if stroke.is_some() {
                    o.push_str(" stroke-linejoin=\"round\"");
                }
                o.push_str("/>");
            }
            Item::Text {
                pos,
                text,
                size,
                anchor,
                bold,
                paint,
                ..
            } => {
                o.push_str("<text x=\"");
                num(&mut o, pos[0]);
                o.push_str("\" y=\"");
                num(&mut o, pos[1]);
                o.push_str("\" font-size=\"");
                num(&mut o, *size);
                o.push('"');
                match anchor {
                    Anchor::Start => {}
                    Anchor::Middle => o.push_str(" text-anchor=\"middle\""),
                    Anchor::End => o.push_str(" text-anchor=\"end\""),
                }
                o.push_str(" dominant-baseline=\"central\"");
                if *bold {
                    o.push_str(" font-weight=\"bold\"");
                }
                color(&mut o, "fill", pal.resolve(*paint));
                o.push('>');
                escape(text, &mut o);
                o.push_str("</text>");
            }
        }
    }
    o.push_str("</svg>");
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::scene::Role;
    use markise_theme::Theme;

    #[test]
    fn escapes_text_and_font() {
        let scene = Scene {
            width: 10.0,
            height: 10.0,
            items: vec![Item::Text {
                pos: [1.0, 2.5],
                text: "<script>&\"".into(),
                size: 12.0,
                width: 30.0,
                anchor: Anchor::Middle,
                bold: false,
                paint: Role::Text.into(),
            }],
        };
        let svg = to_svg(&scene, &DiagramPalette::from_theme(&Theme::light()), "a\"><script>");
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;&amp;&quot;"));
        assert!(svg.contains("x=\"1\" y=\"2.5\""));
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.ends_with("</svg>"));
    }
}
