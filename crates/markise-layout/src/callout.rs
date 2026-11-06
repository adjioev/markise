//! How a callout's header looks (ADR 0025): the accent colour from the theme and the icon, drawn
//! from primitives so it follows the theme and needs no asset. The syntax is the format's: layout
//! reads [`CalloutView`] through the seam.

use epaint::{Color32, Pos2, Stroke, pos2};
use markise_doc::{CalloutAccent, CalloutIcon};
use markise_theme::{Rgba, Theme};

use crate::block::Prim;

/// Width of the icon's slot in a header.
pub(crate) const ICON_SLOT: f32 = 20.0;

/// The theme colour of an accent: the theme's alert colours, or its quote bar.
pub(crate) fn accent_rgba(t: &Theme, a: CalloutAccent) -> Rgba {
    match a {
        CalloutAccent::Note => t.alert[0],
        CalloutAccent::Tip => t.alert[1],
        CalloutAccent::Important => t.alert[2],
        CalloutAccent::Warning => t.alert[3],
        CalloutAccent::Caution => t.alert[4],
        CalloutAccent::Quote => t.quote_bar,
    }
}

/// The primitives of `icon` around `center`, in a box `size` points wide.
pub(crate) fn icon_prims(icon: CalloutIcon, center: Pos2, size: f32, color: Color32) -> Vec<Prim> {
    let r = size / 2.0;
    let at = |x: f32, y: f32| pos2(center.x + x * r, center.y + y * r);
    let stroke = Stroke::new((size / 9.0).max(1.0), color);
    let line = |pts: &[(f32, f32)]| Prim::Path {
        points: pts.iter().map(|&(x, y)| at(x, y)).collect(),
        closed: false,
        fill: Color32::TRANSPARENT,
        stroke,
        dash: None,
    };
    let filled = |pts: &[(f32, f32)]| Prim::Path {
        points: pts.iter().map(|&(x, y)| at(x, y)).collect(),
        closed: true,
        fill: color,
        stroke: Stroke::NONE,
        dash: None,
    };
    let ring = |x: f32, y: f32, radius: f32| Prim::Circle {
        center: at(x, y),
        radius: radius * r,
        fill: Color32::TRANSPARENT,
        stroke,
    };
    let dot = |x: f32, y: f32, radius: f32| Prim::Circle {
        center: at(x, y),
        radius: radius * r,
        fill: color,
        stroke: Stroke::NONE,
    };
    match icon {
        CalloutIcon::Info => vec![
            ring(0.0, 0.0, 0.9),
            dot(0.0, -0.42, 0.13),
            line(&[(0.0, -0.1), (0.0, 0.5)]),
        ],
        CalloutIcon::Question => vec![
            ring(0.0, 0.0, 0.9),
            line(&[
                (-0.32, -0.3),
                (-0.2, -0.5),
                (0.05, -0.55),
                (0.3, -0.4),
                (0.3, -0.15),
                (0.0, 0.05),
                (0.0, 0.2),
            ]),
            dot(0.0, 0.5, 0.12),
        ],
        CalloutIcon::Warning => vec![
            Prim::Path {
                points: vec![at(0.0, -0.9), at(0.95, 0.8), at(-0.95, 0.8)],
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke,
                dash: None,
            },
            line(&[(0.0, -0.3), (0.0, 0.2)]),
            dot(0.0, 0.48, 0.11),
        ],
        CalloutIcon::Check => vec![line(&[(-0.8, 0.05), (-0.25, 0.6), (0.85, -0.6)])],
        CalloutIcon::Cross => vec![
            line(&[(-0.65, -0.65), (0.65, 0.65)]),
            line(&[(0.65, -0.65), (-0.65, 0.65)]),
        ],
        CalloutIcon::Bolt => vec![
            filled(&[(0.2, -0.95), (-0.55, 0.15), (-0.05, 0.15), (-0.2, 0.95)]),
            filled(&[(0.2, -0.95), (0.55, -0.15), (0.05, -0.15), (-0.2, 0.95)]),
        ],
        CalloutIcon::Bulb => vec![
            ring(0.0, -0.25, 0.62),
            line(&[(-0.3, 0.55), (0.3, 0.55)]),
            line(&[(-0.18, 0.85), (0.18, 0.85)]),
        ],
        CalloutIcon::Star => {
            let mut prims = Vec::with_capacity(5);
            let point = |k: f32, radius: f32| {
                let a = (k * 72.0 - 90.0).to_radians();
                (radius * a.cos(), radius * a.sin())
            };
            for k in 0..5 {
                let k = k as f32;
                prims.push(filled(&[
                    (0.0, 0.0),
                    point(k - 0.5, 0.38),
                    point(k, 0.98),
                    point(k + 0.5, 0.38),
                ]));
            }
            prims
        }
        CalloutIcon::Pencil => vec![
            line(&[
                (-0.75, 0.75),
                (-0.6, 0.2),
                (0.35, -0.75),
                (0.75, -0.35),
                (-0.2, 0.6),
                (-0.75, 0.75),
            ]),
            line(&[(0.15, -0.55), (0.55, -0.15)]),
        ],
        CalloutIcon::Bug => vec![
            ring(0.0, 0.15, 0.55),
            dot(0.0, -0.55, 0.22),
            line(&[(-0.55, 0.15), (-0.95, 0.15)]),
            line(&[(0.55, 0.15), (0.95, 0.15)]),
            line(&[(-0.45, -0.25), (-0.85, -0.55)]),
            line(&[(0.45, -0.25), (0.85, -0.55)]),
            line(&[(-0.45, 0.55), (-0.85, 0.85)]),
            line(&[(0.45, 0.55), (0.85, 0.85)]),
        ],
        CalloutIcon::List => [-0.6, 0.0, 0.6]
            .into_iter()
            .flat_map(|y| [dot(-0.75, y, 0.12), line(&[(-0.35, y), (0.9, y)])])
            .collect(),
        CalloutIcon::Quote => [-0.5, 0.35]
            .into_iter()
            .map(|x| {
                filled(&[
                    (x, -0.55),
                    (x + 0.5, -0.55),
                    (x + 0.5, 0.0),
                    (x + 0.15, 0.6),
                    (x - 0.1, 0.6),
                    (x + 0.1, 0.0),
                    (x, 0.0),
                ])
            })
            .collect(),
        CalloutIcon::Lines => vec![
            line(&[(-0.85, -0.6), (0.85, -0.6)]),
            line(&[(-0.85, -0.2), (0.85, -0.2)]),
            line(&[(-0.85, 0.2), (0.85, 0.2)]),
            line(&[(-0.85, 0.6), (0.2, 0.6)]),
        ],
        CalloutIcon::Task => vec![
            Prim::Path {
                points: vec![at(-0.8, -0.8), at(0.8, -0.8), at(0.8, 0.8), at(-0.8, 0.8)],
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke,
                dash: None,
            },
            line(&[(-0.42, 0.02), (-0.1, 0.35), (0.45, -0.32)]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use epaint::vec2;

    #[test]
    fn every_icon_stays_in_its_box() {
        for icon in [
            CalloutIcon::Pencil,
            CalloutIcon::Info,
            CalloutIcon::Bulb,
            CalloutIcon::Star,
            CalloutIcon::Check,
            CalloutIcon::Question,
            CalloutIcon::Warning,
            CalloutIcon::Cross,
            CalloutIcon::Bolt,
            CalloutIcon::Bug,
            CalloutIcon::List,
            CalloutIcon::Quote,
            CalloutIcon::Lines,
            CalloutIcon::Task,
        ] {
            let prims = icon_prims(icon, pos2(10.0, 10.0), 14.0, Color32::RED);
            assert!(!prims.is_empty());
            for p in prims {
                let points = match p {
                    Prim::Path { points, .. } => points,
                    Prim::Circle { center, radius, .. } => {
                        vec![center + vec2(radius, radius), center - vec2(radius, radius)]
                    }
                    other => panic!("{other:?}"),
                };
                for q in points {
                    assert!((q.x - 10.0).abs() <= 7.5 && (q.y - 10.0).abs() <= 7.5, "{icon:?} {q:?}");
                }
            }
        }
    }
}
