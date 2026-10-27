//! Renderer-neutral display list of a laid-out diagram (ADR 0009 §3).
//!
//! A [`Scene`] is produced once per (source, language, text metrics) and painted many times: by
//! the egui front end (shapes + the app's own text stack), by the HTML export ([`super::svg`])
//! and, later, by PDF/PNG export. Geometry is in logical points with the origin at the top
//! left; colors are *roles* resolved at paint time from the active color scheme
//! ([`super::DiagramPalette`]), so switching colors never re-lays out a diagram (ADR 0007 split:
//! the style's font size drives layout, the color scheme only painting).

use markise_theme::Rgba;

/// Semantic color role of a stroke, fill or text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    NodeFill,
    NodeStroke,
    NodeText,
    EdgeStroke,
    EdgeText,
    /// Background behind edge labels (so the edge does not run through the text).
    EdgeLabelFill,
    ClusterFill,
    ClusterStroke,
    ClusterText,
    NoteFill,
    NoteStroke,
    NoteText,
    ActorFill,
    ActorStroke,
    ActorText,
    LifeLine,
    ActivationFill,
    /// Frames of sequence blocks (`loop`, `alt`, …).
    FrameStroke,
    FrameLabelFill,
    FrameText,
    /// Titles and other free text.
    Text,
    /// The page/background color (e.g. holes in markers).
    Background,
}

/// A color: a palette role, or an explicit color from the diagram source (`style`/`classDef`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Paint {
    Role(Role),
    Custom(Rgba),
}

impl From<Role> for Paint {
    fn from(r: Role) -> Self {
        Paint::Role(r)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub paint: Paint,
    /// Dash and gap length; `None` = solid.
    pub dash: Option<[f32; 2]>,
}

impl Stroke {
    pub fn solid(width: f32, paint: impl Into<Paint>) -> Self {
        Stroke {
            width,
            paint: paint.into(),
            dash: None,
        }
    }

    pub fn dashed(width: f32, paint: impl Into<Paint>, dash: [f32; 2]) -> Self {
        Stroke {
            width,
            paint: paint.into(),
            dash: Some(dash),
        }
    }
}

/// One path segment; all coordinates absolute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    MoveTo([f32; 2]),
    LineTo([f32; 2]),
    /// Cubic Bézier: two control points, end point.
    CubicTo([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// Horizontal anchor of a text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Rect {
        /// `[x, y, width, height]`.
        rect: [f32; 4],
        radius: f32,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    },
    Ellipse {
        center: [f32; 2],
        radii: [f32; 2],
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    },
    Path {
        segs: Vec<Seg>,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    },
    /// One line of text. `pos` is the anchor point on the line's vertical center; `width` is
    /// the advance the layout measured (painters with other fonts may shrink to fit it).
    Text {
        pos: [f32; 2],
        text: String,
        size: f32,
        width: f32,
        anchor: Anchor,
        bold: bool,
        paint: Paint,
    },
}

/// A laid-out diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
}

impl Scene {
    /// Translates every item by `d` (used to apply margins after layout).
    pub fn translate(&mut self, d: [f32; 2]) {
        let mv = |p: &mut [f32; 2]| {
            p[0] += d[0];
            p[1] += d[1];
        };
        for it in &mut self.items {
            match it {
                Item::Rect { rect, .. } => {
                    rect[0] += d[0];
                    rect[1] += d[1];
                }
                Item::Ellipse { center, .. } => mv(center),
                Item::Path { segs, .. } => {
                    for s in segs {
                        match s {
                            Seg::MoveTo(p) | Seg::LineTo(p) => mv(p),
                            Seg::CubicTo(a, b, c) => {
                                mv(a);
                                mv(b);
                                mv(c);
                            }
                            Seg::Close => {}
                        }
                    }
                }
                Item::Text { pos, .. } => mv(pos),
            }
        }
    }

    /// Approximate heap size in bytes (cache accounting).
    pub fn heap_size(&self) -> usize {
        let mut n = self.items.capacity() * std::mem::size_of::<Item>();
        for it in &self.items {
            match it {
                Item::Path { segs, .. } => n += segs.capacity() * std::mem::size_of::<Seg>(),
                Item::Text { text, .. } => n += text.capacity(),
                _ => {}
            }
        }
        n
    }

    /// Every coordinate is finite and inside the scene bounds (with a small tolerance).
    pub fn is_well_formed(&self) -> bool {
        let (w, h) = (self.width, self.height);
        if !(w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0) {
            return false;
        }
        let tol = 2.0;
        let ok = |p: [f32; 2]| {
            p[0].is_finite() && p[1].is_finite() && p[0] >= -tol && p[1] >= -tol && p[0] <= w + tol && p[1] <= h + tol
        };
        self.items.iter().all(|it| match it {
            Item::Rect { rect, .. } => ok([rect[0], rect[1]]) && ok([rect[0] + rect[2], rect[1] + rect[3]]),
            Item::Ellipse { center, radii, .. } => {
                ok([center[0] - radii[0], center[1] - radii[1]]) && ok([center[0] + radii[0], center[1] + radii[1]])
            }
            Item::Path { segs, .. } => segs.iter().all(|s| match s {
                Seg::MoveTo(p) | Seg::LineTo(p) => ok(*p),
                Seg::CubicTo(a, b, c) => ok(*a) && ok(*b) && ok(*c),
                Seg::Close => true,
            }),
            Item::Text { pos, size, .. } => ok(*pos) && size.is_finite(),
        })
    }
}

/// Scene builder that tracks the bounding box of everything drawn.
pub(crate) struct Canvas<'m> {
    pub measure: &'m dyn super::TextMeasure,
    pub items: Vec<Item>,
    min: [f32; 2],
    max: [f32; 2],
}

impl<'m> Canvas<'m> {
    pub fn new(measure: &'m dyn super::TextMeasure) -> Self {
        Canvas {
            measure,
            items: Vec::new(),
            min: [f32::INFINITY; 2],
            max: [f32::NEG_INFINITY; 2],
        }
    }

    fn grow(&mut self, p: [f32; 2], pad: f32) {
        if !(p[0].is_finite() && p[1].is_finite()) {
            return;
        }
        self.min[0] = self.min[0].min(p[0] - pad);
        self.min[1] = self.min[1].min(p[1] - pad);
        self.max[0] = self.max[0].max(p[0] + pad);
        self.max[1] = self.max[1].max(p[1] + pad);
    }

    pub fn rect(&mut self, rect: [f32; 4], radius: f32, fill: Option<Paint>, stroke: Option<Stroke>) {
        let pad = stroke.map_or(0.0, |s| s.width / 2.0);
        self.grow([rect[0], rect[1]], pad);
        self.grow([rect[0] + rect[2], rect[1] + rect[3]], pad);
        self.items.push(Item::Rect {
            rect,
            radius,
            fill,
            stroke,
        });
    }

    pub fn ellipse(&mut self, center: [f32; 2], radii: [f32; 2], fill: Option<Paint>, stroke: Option<Stroke>) {
        let pad = stroke.map_or(0.0, |s| s.width / 2.0);
        self.grow([center[0] - radii[0], center[1] - radii[1]], pad);
        self.grow([center[0] + radii[0], center[1] + radii[1]], pad);
        self.items.push(Item::Ellipse {
            center,
            radii,
            fill,
            stroke,
        });
    }

    pub fn path(&mut self, segs: Vec<Seg>, fill: Option<Paint>, stroke: Option<Stroke>) {
        let pad = stroke.map_or(0.0, |s| s.width / 2.0);
        for s in &segs {
            match *s {
                Seg::MoveTo(p) | Seg::LineTo(p) => self.grow(p, pad),
                Seg::CubicTo(a, b, c) => {
                    self.grow(a, pad);
                    self.grow(b, pad);
                    self.grow(c, pad);
                }
                Seg::Close => {}
            }
        }
        self.items.push(Item::Path { segs, fill, stroke });
    }

    pub fn polygon(&mut self, pts: &[[f32; 2]], fill: Option<Paint>, stroke: Option<Stroke>) {
        let mut segs = Vec::with_capacity(pts.len() + 1);
        for (i, p) in pts.iter().enumerate() {
            segs.push(if i == 0 { Seg::MoveTo(*p) } else { Seg::LineTo(*p) });
        }
        segs.push(Seg::Close);
        self.path(segs, fill, stroke);
    }

    pub fn line(&mut self, a: [f32; 2], b: [f32; 2], stroke: Stroke) {
        self.path(vec![Seg::MoveTo(a), Seg::LineTo(b)], None, Some(stroke));
    }

    /// One text line; `pos` is the anchor point on the vertical center.
    pub fn text(&mut self, pos: [f32; 2], text: &str, size: f32, anchor: Anchor, bold: bool, paint: Paint) {
        if text.is_empty() {
            return;
        }
        let w = self.measure.width(text, size, bold);
        let x0 = match anchor {
            Anchor::Start => pos[0],
            Anchor::Middle => pos[0] - w / 2.0,
            Anchor::End => pos[0] - w,
        };
        let h = self.measure.line_height(size) / 2.0;
        self.grow([x0, pos[1] - h], 0.0);
        self.grow([x0 + w, pos[1] + h], 0.0);
        self.items.push(Item::Text {
            pos,
            text: text.to_string(),
            size,
            width: w,
            anchor,
            bold,
            paint,
        });
    }

    /// Lines of text centered on `center` (vertically and horizontally).
    pub fn text_block(&mut self, center: [f32; 2], lines: &[String], size: f32, bold: bool, paint: Paint) {
        let lh = self.measure.line_height(size);
        let top = center[1] - lh * lines.len() as f32 / 2.0;
        for (i, l) in lines.iter().enumerate() {
            self.text(
                [center[0], top + lh * (i as f32 + 0.5)],
                l,
                size,
                Anchor::Middle,
                bold,
                paint,
            );
        }
    }

    /// Appends `other`'s items (painted after this canvas's) and bounds.
    pub fn append(&mut self, other: Canvas) {
        self.items.extend(other.items);
        for i in 0..2 {
            self.min[i] = self.min[i].min(other.min[i]);
            self.max[i] = self.max[i].max(other.max[i]);
        }
    }

    /// The scene, translated so the drawing starts at `margin` from the top left.
    pub fn finish(self, margin: f32) -> Scene {
        if self.items.is_empty() || !self.min[0].is_finite() {
            return Scene {
                width: 2.0 * margin,
                height: 2.0 * margin,
                items: self.items,
            };
        }
        let mut s = Scene {
            width: self.max[0] - self.min[0] + 2.0 * margin,
            height: self.max[1] - self.min[1] + 2.0 * margin,
            items: self.items,
        };
        s.translate([margin - self.min[0], margin - self.min[1]]);
        s
    }
}
