//! Graph diagram model shared by Mermaid flowcharts and flowchart.js (`flow`) diagrams.

use std::collections::HashMap;

use markise_theme::Rgba;

use super::{DiagramError, Limits};

/// Layout direction (`TD`/`TB`, `BT`, `LR`, `RL`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

impl Direction {
    pub fn parse(s: &str) -> Option<Direction> {
        Some(match s.to_ascii_uppercase().as_str() {
            "TD" | "TB" | "V" | "^" => Direction::TopDown,
            "BT" => Direction::BottomUp,
            "LR" | ">" => Direction::LeftRight,
            "RL" | "<" => Direction::RightLeft,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    DoubleCircle,
    /// `>text]`
    Asymmetric,
    Diamond,
    Hexagon,
    /// `[/text/]`
    Parallelogram,
    /// `[\text\]`
    ParallelogramAlt,
    /// `[/text\]`
    Trapezoid,
    /// `[\text/]`
    TrapezoidAlt,
}

/// Explicit colors from `style` / `classDef` (only what Markise paints).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeStyle {
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    pub text: Option<Rgba>,
    pub dashed: bool,
}

impl NodeStyle {
    /// Applies `other`'s set fields over `self`.
    pub fn merge(&mut self, other: &NodeStyle) {
        self.fill = other.fill.or(self.fill);
        self.stroke = other.stroke.or(self.stroke);
        self.text = other.text.or(self.text);
        self.dashed |= other.dashed;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: Vec<String>,
    pub shape: Shape,
    pub style: NodeStyle,
    /// Innermost cluster containing the node.
    pub cluster: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineKind {
    #[default]
    Solid,
    Dotted,
    Thick,
    Invisible,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Head {
    #[default]
    None,
    Arrow,
    Circle,
    Cross,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Vec<String>,
    pub line: LineKind,
    /// Marker at `to`.
    pub head: Head,
    /// Marker at `from`.
    pub tail: Head,
    /// Rank distance (longer arrows such as `--->` span more ranks).
    pub minlen: usize,
    pub stroke: Option<Rgba>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub id: String,
    pub title: Vec<String>,
    pub parent: Option<usize>,
}

/// A parsed graph diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub clusters: Vec<Cluster>,
    pub title: Option<String>,
}

/// Incremental builder enforcing [`Limits`].
pub(crate) struct GraphBuilder<'l> {
    pub graph: Graph,
    pub index: HashMap<String, usize>,
    pub limits: &'l Limits,
}

impl<'l> GraphBuilder<'l> {
    pub fn new(limits: &'l Limits) -> Self {
        GraphBuilder {
            graph: Graph::default(),
            index: HashMap::new(),
            limits,
        }
    }

    /// The node `id`, created (label = id) inside `cluster` if new.
    pub fn node(&mut self, id: &str, cluster: Option<usize>) -> Result<usize, DiagramError> {
        if let Some(&i) = self.index.get(id) {
            return Ok(i);
        }
        if self.graph.nodes.len() + self.graph.clusters.len() >= self.limits.max_nodes {
            return Err(DiagramError::TooLarge(format!(
                "more than {} nodes",
                self.limits.max_nodes
            )));
        }
        let i = self.graph.nodes.len();
        let label = super::text::label_lines(id, false, self.limits.max_label_chars);
        self.graph.nodes.push(Node {
            id: id.to_string(),
            label,
            shape: Shape::Rect,
            style: NodeStyle::default(),
            cluster,
        });
        self.index.insert(id.to_string(), i);
        Ok(i)
    }

    pub fn edge(&mut self, e: Edge) -> Result<(), DiagramError> {
        if self.graph.edges.len() >= self.limits.max_edges {
            return Err(DiagramError::TooLarge(format!(
                "more than {} edges",
                self.limits.max_edges
            )));
        }
        self.graph.edges.push(e);
        Ok(())
    }

    pub fn cluster(&mut self, id: String, title: Vec<String>, parent: Option<usize>) -> Result<usize, DiagramError> {
        if self.graph.nodes.len() + self.graph.clusters.len() >= self.limits.max_nodes {
            return Err(DiagramError::TooLarge(format!(
                "more than {} nodes",
                self.limits.max_nodes
            )));
        }
        self.graph.clusters.push(Cluster { id, title, parent });
        Ok(self.graph.clusters.len() - 1)
    }

    pub fn finish(self) -> Graph {
        self.graph
    }
}

/// Parses a CSS color as used in Mermaid `style`/`classDef`: `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb()`/`rgba()` and a small set of names. Anything else is `None` (ignored).
pub fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim().trim_end_matches("!important").trim();
    if let Some(h) = s.strip_prefix('#') {
        let v = |i: usize, n: usize| u8::from_str_radix(h.get(i..i + n)?, 16).ok();
        return match h.len() {
            3 => Some(Rgba([v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17, 255])),
            4 => Some(Rgba([v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17, v(3, 1)? * 17])),
            6 => Some(Rgba([v(0, 2)?, v(2, 2)?, v(4, 2)?, 255])),
            8 => Some(Rgba([v(0, 2)?, v(2, 2)?, v(4, 2)?, v(6, 2)?])),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    if let Some(args) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<&str> = args.split([',', ' ', '/']).filter(|p| !p.is_empty()).collect();
        if parts.len() < 3 || parts.len() > 4 {
            return None;
        }
        let ch = |p: &str| -> Option<u8> {
            if let Some(pc) = p.strip_suffix('%') {
                let f: f32 = pc.parse().ok()?;
                f.is_finite().then(|| (f.clamp(0.0, 100.0) * 2.55).round() as u8)
            } else {
                let f: f32 = p.parse().ok()?;
                f.is_finite().then(|| f.clamp(0.0, 255.0).round() as u8)
            }
        };
        let a = match parts.get(3) {
            Some(p) => {
                let f: f32 = p.trim_end_matches('%').parse().ok()?;
                let f = if p.ends_with('%') { f / 100.0 } else { f };
                if !f.is_finite() {
                    return None;
                }
                (f.clamp(0.0, 1.0) * 255.0).round() as u8
            }
            None => 255,
        };
        return Some(Rgba([ch(parts[0])?, ch(parts[1])?, ch(parts[2])?, a]));
    }
    Some(match lower.as_str() {
        "black" => Rgba::hex(0x000000),
        "white" => Rgba::hex(0xffffff),
        "red" => Rgba::hex(0xff0000),
        "green" => Rgba::hex(0x008000),
        "blue" => Rgba::hex(0x0000ff),
        "yellow" => Rgba::hex(0xffff00),
        "orange" => Rgba::hex(0xffa500),
        "purple" => Rgba::hex(0x800080),
        "gray" | "grey" => Rgba::hex(0x808080),
        "lightgray" | "lightgrey" => Rgba::hex(0xd3d3d3),
        "pink" => Rgba::hex(0xffc0cb),
        "cyan" | "aqua" => Rgba::hex(0x00ffff),
        "magenta" | "fuchsia" => Rgba::hex(0xff00ff),
        "lime" => Rgba::hex(0x00ff00),
        "navy" => Rgba::hex(0x000080),
        "teal" => Rgba::hex(0x008080),
        "brown" => Rgba::hex(0xa52a2a),
        "transparent" | "none" => Rgba::TRANSPARENT,
        _ => return None,
    })
}

/// Parses a Mermaid style list `fill:#f9f,stroke:#333,stroke-width:4px,color:#fff`.
pub fn parse_style(list: &str) -> NodeStyle {
    let mut st = NodeStyle::default();
    // split on `,`/`;` outside parentheses (`rgb(1,2,3)` stays whole; `\,` is Mermaid's escape)
    let list = list.replace("\\,", ",");
    let mut decls = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in list.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' | ';' if depth == 0 => {
                decls.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    decls.push(&list[start..]);
    for decl in decls {
        let Some((k, v)) = decl.split_once(':') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "fill" => st.fill = parse_color(v),
            "stroke" => st.stroke = parse_color(v),
            "color" => st.text = parse_color(v),
            "stroke-dasharray" => st.dashed = !v.trim().is_empty() && v.trim() != "0",
            _ => {}
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#f9f"), Some(Rgba([255, 153, 255, 255])));
        assert_eq!(parse_color("#336699"), Some(Rgba::hex(0x336699)));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Rgba([1, 2, 3, 255])));
        assert_eq!(parse_color("rgba(1,2,3,0.5)"), Some(Rgba([1, 2, 3, 128])));
        assert_eq!(parse_color("Red"), Some(Rgba::hex(0xff0000)));
        assert_eq!(parse_color("#12"), None);
        assert_eq!(parse_color("url(x)"), None);
        assert_eq!(parse_color("rgb(NaN,1,1)"), None);
    }

    #[test]
    fn styles() {
        let s = parse_style("fill:#f9f,stroke:#333,stroke-width:4px,color:#fff,stroke-dasharray: 5 5");
        assert_eq!(s.fill, Some(Rgba([255, 153, 255, 255])));
        assert_eq!(s.stroke, Some(Rgba([51, 51, 51, 255])));
        assert_eq!(s.text, Some(Rgba::hex(0xffffff)));
        assert!(s.dashed);
    }
}
