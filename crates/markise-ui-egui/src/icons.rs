//! Sidebar icons: a few Lucide icons (ISC license, `assets/icons/lucide/`, see `NOTICE`),
//! embedded as SVG source and drawn as egui strokes. Only what those icons use is supported:
//! `<path d="…">` with M/L/H/V/A/C/Z commands (absolute and relative); arcs and curves are
//! flattened to polylines once and cached.

use std::sync::OnceLock;

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Folder,
    FolderOpen,
    FileText,
    File,
}

const SOURCES: [(Icon, &str); 4] = [
    (Icon::Folder, include_str!("../../../assets/icons/lucide/folder.svg")),
    (
        Icon::FolderOpen,
        include_str!("../../../assets/icons/lucide/folder-open.svg"),
    ),
    (
        Icon::FileText,
        include_str!("../../../assets/icons/lucide/file-text.svg"),
    ),
    (Icon::File, include_str!("../../../assets/icons/lucide/file.svg")),
];

/// Polylines in the 24×24 icon box.
pub(crate) type Lines = Vec<Vec<Pos2>>;

fn cache() -> &'static [(Icon, Lines)] {
    static C: OnceLock<Vec<(Icon, Lines)>> = OnceLock::new();
    C.get_or_init(|| SOURCES.iter().map(|(i, svg)| (*i, parse_svg(svg))).collect())
}

/// Draws `icon` into `rect` (square) with the given color; stroke width scales with the size.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let Some((_, lines)) = cache().iter().find(|(i, _)| *i == icon) else {
        return;
    };
    let s = rect.width().min(rect.height()) / 24.0;
    let stroke = Stroke::new((2.0 * s).max(1.0), color);
    for l in lines {
        let pts: Vec<Pos2> = l.iter().map(|p| rect.min + egui::vec2(p.x * s, p.y * s)).collect();
        painter.add(egui::Shape::line(pts, stroke));
    }
}

/// The `d` attributes of every `<path>` in an SVG document.
fn path_data(svg: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find("<path") {
        rest = &rest[i..];
        let Some(d) = rest.find(" d=\"") else { break };
        let body = &rest[d + 4..];
        let Some(end) = body.find('"') else { break };
        out.push(&body[..end]);
        rest = &body[end..];
    }
    out
}

pub(crate) fn parse_svg(svg: &str) -> Lines {
    path_data(svg).into_iter().flat_map(parse_path).collect()
}

/// Tokenizer: command letters and numbers (handles `-` and `.` separators, `1.5.5`).
fn tokens(d: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() {
            out.push(Tok::Cmd(c));
            i += 1;
        } else if c == '-' || c == '.' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            let mut dot = c == '.';
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !dot {
                    dot = true;
                    i += 1;
                } else if (ch == 'e' || ch == 'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = d[start..i].parse::<f32>() {
                out.push(Tok::Num(v));
            }
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Tok {
    Cmd(char),
    Num(f32),
}

pub(crate) fn parse_path(d: &str) -> Lines {
    let toks = tokens(d);
    let mut lines: Lines = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    let mut p = pos2(0.0, 0.0);
    let mut start = p;
    let mut cmd = 'M';
    let mut i = 0;
    let num = |i: &mut usize| -> Option<f32> {
        match toks.get(*i) {
            Some(Tok::Num(v)) => {
                *i += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                cur.push(start);
                p = start;
                lines.push(std::mem::take(&mut cur));
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { p } else { pos2(0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else {
                    break;
                };
                if cur.len() > 1 {
                    lines.push(std::mem::take(&mut cur));
                }
                cur.clear();
                p = pos2(base.x + x, base.y + y);
                start = p;
                cur.push(p);
                // further pairs are implicit line-tos
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else {
                    break;
                };
                p = pos2(base.x + x, base.y + y);
                cur.push(p);
            }
            'H' => {
                let Some(x) = num(&mut i) else { break };
                p = pos2(if rel { p.x + x } else { x }, p.y);
                cur.push(p);
            }
            'V' => {
                let Some(y) = num(&mut i) else { break };
                p = pos2(p.x, if rel { p.y + y } else { y });
                cur.push(p);
            }
            'C' => {
                let v: Option<Vec<f32>> = (0..6).map(|_| num(&mut i)).collect();
                let Some(v) = v else { break };
                let c1 = pos2(base.x + v[0], base.y + v[1]);
                let c2 = pos2(base.x + v[2], base.y + v[3]);
                let e = pos2(base.x + v[4], base.y + v[5]);
                for k in 1..=8 {
                    let t = k as f32 / 8.0;
                    let mt = 1.0 - t;
                    let x = mt * mt * mt * p.x + 3.0 * mt * mt * t * c1.x + 3.0 * mt * t * t * c2.x + t * t * t * e.x;
                    let y = mt * mt * mt * p.y + 3.0 * mt * mt * t * c1.y + 3.0 * mt * t * t * c2.y + t * t * t * e.y;
                    cur.push(pos2(x, y));
                }
                p = e;
            }
            'A' => {
                let v: Option<Vec<f32>> = (0..7).map(|_| num(&mut i)).collect();
                let Some(v) = v else { break };
                let e = pos2(base.x + v[5], base.y + v[6]);
                arc(&mut cur, p, v[0], v[1], v[2], v[3] != 0.0, v[4] != 0.0, e);
                p = e;
            }
            _ => {
                i += 1; // unsupported command: skip a token
            }
        }
    }
    if cur.len() > 1 {
        lines.push(cur);
    }
    lines
}

/// SVG elliptical arc (endpoint parametrization, SVG 1.1 F.6.5) flattened into `out`.
#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<Pos2>, p0: Pos2, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: Pos2) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || p0 == p1 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sp, cp) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lam = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if lam > 1.0 {
        rx *= lam.sqrt();
        ry *= lam.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = (num / den).max(0.0).sqrt();
    if large == sweep {
        co = -co;
    }
    let cx1 = co * rx * y1 / ry;
    let cy1 = -co * ry * x1 / rx;
    let cx = cp * cx1 - sp * cy1 + (p0.x + p1.x) / 2.0;
    let cy = sp * cx1 + cp * cy1 + (p0.y + p1.y) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let d = (ux * vx + uy * vy) / ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt());
        let a = d.clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 {
        dt -= std::f32::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f32::consts::TAU;
    }
    let steps = ((dt.abs() / (std::f32::consts::PI / 8.0)).ceil() as usize).clamp(2, 32);
    for k in 1..=steps {
        let t = t1 + dt * k as f32 / steps as f32;
        let (st, ct) = t.sin_cos();
        out.push(pos2(cx + cp * rx * ct - sp * ry * st, cy + sp * rx * ct + cp * ry * st));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_parse_into_polylines_inside_the_box() {
        for (icon, svg) in SOURCES {
            let lines = parse_svg(svg);
            assert!(!lines.is_empty(), "{icon:?}");
            for l in &lines {
                assert!(l.len() >= 2, "{icon:?}");
                for p in l {
                    assert!(
                        (0.0..=24.0).contains(&p.x) && (0.0..=24.0).contains(&p.y),
                        "{icon:?}: {p:?}"
                    );
                }
            }
        }
        // the folder outline is one closed path that ends where it starts
        let f = parse_svg(SOURCES[0].1);
        assert_eq!(f.len(), 1);
        let (a, b) = (f[0][0], *f[0].last().unwrap());
        assert!((a - b).length() < 0.01);
    }

    #[test]
    fn arcs_end_on_their_endpoint() {
        let mut v = Vec::new();
        arc(&mut v, pos2(2.0, 0.0), 2.0, 2.0, 0.0, false, true, pos2(0.0, 2.0));
        let last = *v.last().unwrap();
        assert!((last - pos2(0.0, 2.0)).length() < 1e-3);
    }
}
