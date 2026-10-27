//! Layered layout of [`Graph`] diagrams through `dugong` (a Rust port of dagre, the layout
//! Mermaid flowcharts use) and drawing into a [`Scene`].

use std::panic::{AssertUnwindSafe, catch_unwind};

use dugong::graphlib::{Graph as DGraph, GraphOptions};
use dugong::{EdgeLabel, GraphLabel, NodeLabel, RankDir, WorkControl, WorkError};

use super::graph::{Direction, Graph, Head, LineKind, Shape};
use super::measure::label_size;
use super::scene::{Anchor, Canvas, Paint, Role, Scene, Seg, Stroke};
use super::sequence::{ArrowHead, arrow_head};
use super::{DiagramError, RenderCtx};

/// Charges dugong's work units against the render's cancellation flag and time budget.
struct Budget<'a, 'b> {
    ctx: &'a RenderCtx<'b>,
    units: usize,
    next: usize,
    stop: Option<DiagramError>,
}

impl WorkControl for Budget<'_, '_> {
    fn charge(&mut self, units: usize) -> Result<(), WorkError> {
        self.units = self.units.saturating_add(units);
        if self.units >= self.next {
            self.next = self.units.saturating_add(20_000);
            if let Err(e) = self.ctx.check() {
                self.stop = Some(e);
                return Err(WorkError::Interrupted);
            }
        }
        Ok(())
    }
}

fn node_key(i: usize) -> String {
    format!("n{i}")
}

fn cluster_key(i: usize) -> String {
    format!("c{i}")
}

/// Node box size for a label of `(w, h)`.
fn shape_size(shape: Shape, (w, h): (f32, f32), fs: f32) -> (f32, f32) {
    let px = fs * 1.1;
    let py = fs * 0.65;
    match shape {
        Shape::Rect | Shape::Round => (w + 2.0 * px, h + 2.0 * py),
        Shape::Subroutine => (w + 2.0 * px + fs, h + 2.0 * py),
        Shape::Asymmetric => (w + 2.0 * px + (h + 2.0 * py) / 2.0, h + 2.0 * py),
        Shape::Stadium => (w + px + h + 2.0 * py, h + 2.0 * py),
        Shape::Circle => {
            let d = w.max(h) + 2.0 * py;
            (d, d)
        }
        Shape::DoubleCircle => {
            let d = w.max(h) + 2.0 * py + fs * 0.7;
            (d, d)
        }
        Shape::Diamond => {
            // the label rectangle must fit inside the rhombus: w/W + h/H <= 1
            let ww = if w <= 2.5 * h { w + h } else { 1.6 * w };
            let ww = ww.max(1.0);
            let hh = h / (1.0 - (w / ww).min(0.9));
            (ww + 2.0 * py, hh + 2.0 * py)
        }
        Shape::Hexagon | Shape::Parallelogram | Shape::ParallelogramAlt | Shape::Trapezoid | Shape::TrapezoidAlt => {
            (w + 2.0 * px + h + 2.0 * py, h + 2.0 * py)
        }
        Shape::Cylinder => {
            let ry = cylinder_ry(w + 2.0 * px);
            (w + 2.0 * px, h + 2.0 * py + 2.0 * ry)
        }
    }
}

fn cylinder_ry(w: f32) -> f32 {
    (w / 2.0 / (2.5 + w / 50.0)).clamp(3.0, 12.0)
}

/// Outline polygon (for polygonal shapes) centered on `c`.
fn outline(shape: Shape, c: [f32; 2], w: f32, h: f32) -> Option<Vec<[f32; 2]>> {
    let (x0, y0, x1, y1) = (c[0] - w / 2.0, c[1] - h / 2.0, c[0] + w / 2.0, c[1] + h / 2.0);
    let k = h / 2.0; // slant / point depth
    Some(match shape {
        Shape::Diamond => vec![[c[0], y0], [x1, c[1]], [c[0], y1], [x0, c[1]]],
        Shape::Hexagon => vec![
            [x0 + k, y0],
            [x1 - k, y0],
            [x1, c[1]],
            [x1 - k, y1],
            [x0 + k, y1],
            [x0, c[1]],
        ],
        Shape::Parallelogram => vec![[x0 + k, y0], [x1, y0], [x1 - k, y1], [x0, y1]],
        Shape::ParallelogramAlt => vec![[x0, y0], [x1 - k, y0], [x1, y1], [x0 + k, y1]],
        Shape::Trapezoid => vec![[x0 + k, y0], [x1 - k, y0], [x1, y1], [x0, y1]],
        Shape::TrapezoidAlt => vec![[x0, y0], [x1, y0], [x1 - k, y1], [x0 + k, y1]],
        Shape::Asymmetric => vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0 + k, c[1]]],
        _ => return None,
    })
}

/// Where the segment from the node center `c` towards `p` leaves the node outline.
fn boundary(shape: Shape, c: [f32; 2], w: f32, h: f32, p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
    if dx.abs() < 1e-4 && dy.abs() < 1e-4 {
        return c;
    }
    match shape {
        Shape::Circle | Shape::DoubleCircle => {
            let r = w / 2.0;
            let l = (dx * dx + dy * dy).sqrt();
            [c[0] + dx / l * r, c[1] + dy / l * r]
        }
        _ => {
            if let Some(poly) = outline(shape, c, w, h)
                && let Some(q) = ray_polygon(c, p, &poly)
            {
                return q;
            }
            // bounding box
            let (hw, hh) = (w / 2.0, h / 2.0);
            let t = if dx.abs() * hh > dy.abs() * hw {
                hw / dx.abs()
            } else {
                hh / dy.abs()
            };
            [c[0] + dx * t, c[1] + dy * t]
        }
    }
}

/// First intersection of the ray `c → p` with the polygon edges (smallest positive parameter).
fn ray_polygon(c: [f32; 2], p: [f32; 2], poly: &[[f32; 2]]) -> Option<[f32; 2]> {
    let d = [p[0] - c[0], p[1] - c[1]];
    let mut best: Option<f32> = None;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let e = [b[0] - a[0], b[1] - a[1]];
        let den = d[0] * e[1] - d[1] * e[0];
        if den.abs() < 1e-9 {
            continue;
        }
        let ac = [a[0] - c[0], a[1] - c[1]];
        let t = (ac[0] * e[1] - ac[1] * e[0]) / den;
        let u = (ac[0] * d[1] - ac[1] * d[0]) / den;
        if t > 0.0 && (-1e-4..=1.0 + 1e-4).contains(&u) {
            best = Some(best.map_or(t, |b: f32| b.min(t)));
        }
    }
    best.map(|t| [c[0] + d[0] * t, c[1] + d[1] * t])
}

/// d3's basis spline through `pts` as path segments (starts at the first point, ends at the
/// last).
fn basis(pts: &[[f32; 2]]) -> Vec<Seg> {
    let mut segs = Vec::with_capacity(pts.len() + 2);
    if pts.is_empty() {
        return segs;
    }
    segs.push(Seg::MoveTo(pts[0]));
    if pts.len() == 2 {
        segs.push(Seg::LineTo(pts[1]));
        return segs;
    }
    let mut p0 = pts[0];
    let mut p1 = pts[0];
    for (k, &p) in pts.iter().enumerate().skip(1) {
        if k == 1 {
            p1 = p;
            continue;
        }
        if k == 2 {
            segs.push(Seg::LineTo([(5.0 * p0[0] + p1[0]) / 6.0, (5.0 * p0[1] + p1[1]) / 6.0]));
        }
        segs.push(Seg::CubicTo(
            [(2.0 * p0[0] + p1[0]) / 3.0, (2.0 * p0[1] + p1[1]) / 3.0],
            [(p0[0] + 2.0 * p1[0]) / 3.0, (p0[1] + 2.0 * p1[1]) / 3.0],
            [(p0[0] + 4.0 * p1[0] + p[0]) / 6.0, (p0[1] + 4.0 * p1[1] + p[1]) / 6.0],
        ));
        p0 = p1;
        p1 = p;
    }
    // line end: one more basis point at the last point, then a line to it
    segs.push(Seg::CubicTo(
        [(2.0 * p0[0] + p1[0]) / 3.0, (2.0 * p0[1] + p1[1]) / 3.0],
        [(p0[0] + 2.0 * p1[0]) / 3.0, (p0[1] + 2.0 * p1[1]) / 3.0],
        [(p0[0] + 5.0 * p1[0]) / 6.0, (p0[1] + 5.0 * p1[1]) / 6.0],
    ));
    segs.push(Seg::LineTo(p1));
    segs
}

pub(crate) fn layout(g: &Graph, ctx: &RenderCtx) -> Result<Scene, DiagramError> {
    let m = ctx.measure;
    let fs = ctx.font_size;
    ctx.check()?;

    // a node whose id names a subgraph stands for that subgraph: edges go to its first node
    let mut alias: Vec<Option<usize>> = vec![None; g.nodes.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        if let Some(ci) = g.clusters.iter().position(|c| c.id == n.id)
            && let Some(first) = first_node_in(g, ci)
            && first != i
        {
            alias[i] = Some(first);
        }
    }
    let resolve = |i: usize| alias[i].unwrap_or(i);

    let mut dg: DGraph<NodeLabel, EdgeLabel, GraphLabel> = DGraph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: !g.clusters.is_empty(),
    });
    dg.set_graph(GraphLabel {
        rankdir: match g.direction {
            Direction::TopDown => RankDir::TB,
            Direction::BottomUp => RankDir::BT,
            Direction::LeftRight => RankDir::LR,
            Direction::RightLeft => RankDir::RL,
        },
        nodesep: (fs * 3.0) as f64,
        ranksep: (fs * 3.5) as f64,
        edgesep: fs as f64,
        marginx: 0.0,
        marginy: 0.0,
        ..Default::default()
    });
    dg.set_default_node_label(NodeLabel::default);
    dg.set_default_edge_label(|| EdgeLabel {
        minlen: 1,
        weight: 1.0,
        ..Default::default()
    });

    let sizes: Vec<(f32, f32)> = g
        .nodes
        .iter()
        .map(|n| shape_size(n.shape, label_size(m, &n.label, fs, false), fs))
        .collect();
    for (i, n) in g.nodes.iter().enumerate() {
        if alias[i].is_some() {
            continue;
        }
        dg.set_node(
            node_key(i),
            NodeLabel {
                width: sizes[i].0 as f64,
                height: sizes[i].1 as f64,
                ..Default::default()
            },
        );
        let _ = n;
    }
    let mut cluster_used = vec![false; g.clusters.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        if alias[i].is_some() {
            continue;
        }
        let mut c = n.cluster;
        while let Some(ci) = c {
            if cluster_used[ci] {
                break;
            }
            cluster_used[ci] = true;
            c = g.clusters[ci].parent;
        }
    }
    for (ci, c) in g.clusters.iter().enumerate() {
        if !cluster_used[ci] {
            continue;
        }
        let (tw, th) = label_size(m, &c.title, fs, false);
        dg.set_node(
            cluster_key(ci),
            NodeLabel {
                width: tw as f64 + 2.0 * fs as f64,
                height: th as f64,
                ..Default::default()
            },
        );
    }
    for (ci, c) in g.clusters.iter().enumerate() {
        if cluster_used[ci]
            && let Some(p) = c.parent
        {
            dg.set_parent(cluster_key(ci), cluster_key(p));
        }
    }
    for (i, n) in g.nodes.iter().enumerate() {
        if alias[i].is_none()
            && let Some(c) = n.cluster
        {
            dg.set_parent(node_key(i), cluster_key(c));
        }
    }
    for (k, e) in g.edges.iter().enumerate() {
        let (lw, lh) = if e.label.is_empty() {
            (0.0, 0.0)
        } else {
            label_size(m, &e.label, fs * 0.92, false)
        };
        let pad = if e.label.is_empty() { 0.0 } else { fs * 1.0 };
        dg.set_edge_named(
            node_key(resolve(e.from)),
            node_key(resolve(e.to)),
            Some(format!("e{k}")),
            Some(EdgeLabel {
                minlen: e.minlen.clamp(1, 16),
                weight: 1.0,
                width: (lw + pad) as f64,
                height: (lh + pad * 0.6) as f64,
                ..Default::default()
            }),
        );
    }

    let mut budget = Budget {
        ctx,
        units: 0,
        next: 20_000,
        stop: None,
    };
    let res = catch_unwind(AssertUnwindSafe(|| dugong::layout_controlled(&mut dg, &mut budget)));
    match res {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return Err(budget
                .stop
                .take()
                .unwrap_or_else(|| DiagramError::TooLarge("layout failed".into())));
        }
        Err(_) => return Err(DiagramError::TooLarge("layout failed".into())),
    }
    ctx.check()?;

    let pos = |key: &str| -> Option<[f32; 2]> {
        let n = dg.node(key)?;
        Some([n.x? as f32, n.y? as f32])
    };
    let mut c = Canvas::new(m);

    // clusters, outermost first
    let mut order: Vec<usize> = (0..g.clusters.len()).filter(|&ci| cluster_used[ci]).collect();
    let depth = |mut ci: usize| {
        let mut d = 0;
        while let Some(p) = g.clusters[ci].parent {
            d += 1;
            ci = p;
            if d > 64 {
                break;
            }
        }
        d
    };
    order.sort_by_key(|&ci| depth(ci));
    let lh = m.line_height(fs);
    // content extents per cluster (descendant nodes), so the title gets its own band above the
    // content instead of above the box dagre computed (which edges may cross)
    let mut content: Vec<[f32; 4]> =
        vec![[f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY]; g.clusters.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        if alias[i].is_some() {
            continue;
        }
        let Some(p) = pos(&node_key(i)) else { continue };
        let (hw, hh) = (sizes[i].0 / 2.0, sizes[i].1 / 2.0);
        let mut cl = n.cluster;
        let mut steps = 0;
        while let Some(ci) = cl {
            let r = &mut content[ci];
            r[0] = r[0].min(p[0] - hw);
            r[1] = r[1].min(p[1] - hh);
            r[2] = r[2].max(p[0] + hw);
            r[3] = r[3].max(p[1] + hh);
            cl = g.clusters[ci].parent;
            steps += 1;
            if steps > 64 {
                break;
            }
        }
    }
    let mut rects: Vec<Option<[f32; 4]>> = vec![None; g.clusters.len()];
    // innermost first, so a parent can enclose its children's final boxes
    for &ci in order.iter().rev() {
        let key = cluster_key(ci);
        let Some(n) = dg.node(&key) else { continue };
        let (Some(x), Some(y)) = (n.x, n.y) else { continue };
        let (w, h) = (n.width as f32, n.height as f32);
        let (mut x0, mut y0, mut x1, mut y1) = (
            x as f32 - w / 2.0,
            y as f32 - h / 2.0,
            x as f32 + w / 2.0,
            y as f32 + h / 2.0,
        );
        let mut ct = content[ci];
        for (cj, c2) in g.clusters.iter().enumerate() {
            if c2.parent == Some(ci)
                && let Some(r) = rects[cj]
            {
                ct = [
                    ct[0].min(r[0]),
                    ct[1].min(r[1]),
                    ct[2].max(r[0] + r[2]),
                    ct[3].max(r[1] + r[3]),
                ];
            }
        }
        let title = &g.clusters[ci].title;
        let title_h = lh * title.len() as f32 + fs * 0.6;
        let (tw, _) = label_size(m, title, fs, false);
        if ct[0].is_finite() {
            x0 = x0.min(ct[0] - fs * 0.6);
            x1 = x1.max(ct[2] + fs * 0.6);
            y0 = y0.min(ct[1] - title_h - fs * 0.3);
            y1 = y1.max(ct[3] + fs * 0.6);
        }
        if x1 - x0 < tw + 2.0 * fs {
            let cx = (x0 + x1) / 2.0;
            x0 = cx - tw / 2.0 - fs;
            x1 = cx + tw / 2.0 + fs;
        }
        rects[ci] = Some([x0, y0, x1 - x0, y1 - y0]);
    }
    for &ci in &order {
        let Some(rect) = rects[ci] else { continue };
        let title_h = lh * g.clusters[ci].title.len() as f32 + fs * 0.6;
        c.rect(
            rect,
            4.0,
            Some(Role::ClusterFill.into()),
            Some(Stroke::solid(1.0, Role::ClusterStroke)),
        );
        c.text_block(
            [rect[0] + rect[2] / 2.0, rect[1] + title_h / 2.0 + fs * 0.1],
            &g.clusters[ci].title,
            fs,
            false,
            Role::ClusterText.into(),
        );
    }

    // edges
    let mut labels = Vec::new();
    for (k, e) in g.edges.iter().enumerate() {
        if k % 128 == 0 {
            ctx.check()?;
        }
        let (a, b) = (resolve(e.from), resolve(e.to));
        let Some(lab) = dg.edge(&node_key(a), &node_key(b), Some(&format!("e{k}"))) else {
            continue;
        };
        let mut pts: Vec<[f32; 2]> = lab
            .points
            .iter()
            .map(|p| [p.x as f32, p.y as f32])
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .collect();
        if let (Some(ca), Some(cb)) = (pos(&node_key(a)), pos(&node_key(b))) {
            if pts.is_empty() {
                pts = vec![ca, cb];
            }
            // clip both ends to the actual node outlines
            let first_toward = if pts.len() > 1 { pts[1] } else { cb };
            pts[0] = boundary(g.nodes[a].shape, ca, sizes[a].0, sizes[a].1, first_toward);
            let n = pts.len();
            let last_from = if n > 1 { pts[n - 2] } else { ca };
            pts[n - 1] = boundary(g.nodes[b].shape, cb, sizes[b].0, sizes[b].1, last_from);
        }
        if pts.len() < 2 {
            continue;
        }
        if let (Some(x), Some(y)) = (lab.x, lab.y)
            && !e.label.is_empty()
        {
            labels.push(([x as f32, y as f32], k));
        }
        if e.line == LineKind::Invisible {
            continue;
        }
        let paint: Paint = e.stroke.map_or(Role::EdgeStroke.into(), Paint::Custom);
        let stroke = match e.line {
            LineKind::Thick => Stroke {
                width: 3.2,
                paint,
                dash: None,
            },
            LineKind::Dotted => Stroke {
                width: 1.5,
                paint,
                dash: Some([3.0, 3.0]),
            },
            _ => Stroke {
                width: 1.5,
                paint,
                dash: None,
            },
        };
        let tip_end = pts[pts.len() - 1];
        let tip_start = pts[0];
        let shorten = |pts: &mut Vec<[f32; 2]>, at_end: bool, head: Head| {
            if head == Head::None {
                return;
            }
            let n = pts.len();
            let (i, j) = if at_end { (n - 1, n - 2) } else { (0, 1) };
            let (dx, dy) = (pts[i][0] - pts[j][0], pts[i][1] - pts[j][1]);
            let l = (dx * dx + dy * dy).sqrt();
            let cut = match head {
                Head::Arrow => 8.0,
                Head::Circle => 8.0,
                _ => 0.0,
            };
            if l > cut * 1.5 {
                pts[i] = [pts[i][0] - dx / l * cut, pts[i][1] - dy / l * cut];
            }
        };
        shorten(&mut pts, true, e.head);
        shorten(&mut pts, false, e.tail);
        let segs = basis(&pts);
        c.path(segs, None, Some(stroke));
        let n = pts.len();
        draw_head(&mut c, pts[n - 1], tip_end, e.head, paint, stroke.width);
        draw_head(&mut c, pts[0], tip_start, e.tail, paint, stroke.width);
    }

    // nodes
    for (i, n) in g.nodes.iter().enumerate() {
        if alias[i].is_some() {
            continue;
        }
        let Some(ctr) = pos(&node_key(i)) else { continue };
        let fill: Paint = n.style.fill.map_or(Role::NodeFill.into(), Paint::Custom);
        let sp: Paint = n.style.stroke.map_or(Role::NodeStroke.into(), Paint::Custom);
        let stroke = if n.style.dashed {
            Stroke::dashed(1.3, sp, [4.0, 3.0])
        } else {
            Stroke::solid(1.3, sp)
        };
        draw_node(&mut c, n.shape, ctr, sizes[i], fill, stroke, fs);
        let tp: Paint = n.style.text.map_or(Role::NodeText.into(), Paint::Custom);
        c.text_block(ctr, &n.label, fs, false, tp);
    }

    // edge labels on top
    for (p, k) in labels {
        let e = &g.edges[k];
        let (w, h) = label_size(m, &e.label, fs * 0.92, false);
        c.rect(
            [p[0] - w / 2.0 - 4.0, p[1] - h / 2.0 - 2.0, w + 8.0, h + 4.0],
            2.0,
            Some(Role::EdgeLabelFill.into()),
            None,
        );
        c.text_block(p, &e.label, fs * 0.92, false, Role::EdgeText.into());
    }

    if let Some(t) = &g.title {
        // title above the drawing (canvas grows upwards)
        let top = c_items_top(&c);
        c.text(
            [0.0, top - fs * 1.2],
            t,
            fs * 1.15,
            Anchor::Start,
            true,
            Role::Text.into(),
        );
    }
    Ok(c.finish(fs * 0.5))
}

fn c_items_top(c: &Canvas) -> f32 {
    use super::scene::Item;
    c.items
        .iter()
        .filter_map(|it| match it {
            Item::Rect { rect, .. } => Some(rect[1]),
            Item::Ellipse { center, radii, .. } => Some(center[1] - radii[1]),
            Item::Text { pos, .. } => Some(pos[1]),
            Item::Path { segs, .. } => segs
                .iter()
                .filter_map(|s| match s {
                    Seg::MoveTo(p) | Seg::LineTo(p) | Seg::CubicTo(_, _, p) => Some(p[1]),
                    Seg::Close => None,
                })
                .reduce(f32::min),
        })
        .fold(0.0, f32::min)
}

fn first_node_in(g: &Graph, ci: usize) -> Option<usize> {
    g.nodes.iter().position(|n| {
        let mut c = n.cluster;
        let mut steps = 0;
        while let Some(x) = c {
            if x == ci {
                return true;
            }
            steps += 1;
            if steps > 64 {
                return false;
            }
            c = g.clusters[x].parent;
        }
        false
    })
}

fn draw_head(c: &mut Canvas, from: [f32; 2], tip: [f32; 2], head: Head, paint: Paint, width: f32) {
    match head {
        Head::None => {}
        Head::Arrow => {
            let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
            let l = (dx * dx + dy * dy).sqrt();
            if l < 1e-3 {
                return;
            }
            let (ux, uy) = (dx / l, dy / l);
            let (px, py) = (-uy, ux);
            let len = 9.0 + width;
            let hw = 4.5 + width * 0.5;
            let base = [tip[0] - ux * len, tip[1] - uy * len];
            c.polygon(
                &[
                    tip,
                    [base[0] + px * hw, base[1] + py * hw],
                    [base[0] - px * hw, base[1] - py * hw],
                ],
                Some(paint),
                None,
            );
        }
        Head::Circle => {
            let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-3);
            let r = 4.0;
            c.ellipse([tip[0] - dx / l * r, tip[1] - dy / l * r], [r, r], Some(paint), None);
        }
        Head::Cross => arrow_head(c, from, tip, ArrowHead::Cross),
    }
}

fn draw_node(c: &mut Canvas, shape: Shape, ctr: [f32; 2], (w, h): (f32, f32), fill: Paint, stroke: Stroke, fs: f32) {
    let (x0, y0) = (ctr[0] - w / 2.0, ctr[1] - h / 2.0);
    let f = Some(fill);
    let s = Some(stroke);
    match shape {
        Shape::Rect => c.rect([x0, y0, w, h], 2.0, f, s),
        Shape::Round => c.rect([x0, y0, w, h], (fs * 0.5).min(h / 2.0), f, s),
        Shape::Stadium => c.rect([x0, y0, w, h], h / 2.0, f, s),
        Shape::Subroutine => {
            c.rect([x0, y0, w, h], 0.0, f, s);
            let d = fs * 0.5;
            c.line([x0 + d, y0], [x0 + d, y0 + h], stroke);
            c.line([x0 + w - d, y0], [x0 + w - d, y0 + h], stroke);
        }
        Shape::Circle => c.ellipse(ctr, [w / 2.0, h / 2.0], f, s),
        Shape::DoubleCircle => {
            c.ellipse(ctr, [w / 2.0, h / 2.0], f, s);
            let g = fs * 0.3;
            c.ellipse(ctr, [w / 2.0 - g, h / 2.0 - g], None, s);
        }
        Shape::Cylinder => {
            let ry = cylinder_ry(w);
            let rx = w / 2.0;
            let (top, bot) = (y0 + ry, y0 + h - ry);
            let k = 0.5523; // circle-to-Bézier constant
            // body: left side down, bottom front arc, right side up, top back arc
            let body = vec![
                Seg::MoveTo([x0, top]),
                Seg::LineTo([x0, bot]),
                Seg::CubicTo([x0, bot + ry * k], [ctr[0] - rx * k, bot + ry], [ctr[0], bot + ry]),
                Seg::CubicTo([ctr[0] + rx * k, bot + ry], [x0 + w, bot + ry * k], [x0 + w, bot]),
                Seg::LineTo([x0 + w, top]),
                Seg::CubicTo([x0 + w, top - ry * k], [ctr[0] + rx * k, top - ry], [ctr[0], top - ry]),
                Seg::CubicTo([ctr[0] - rx * k, top - ry], [x0, top - ry * k], [x0, top]),
                Seg::Close,
            ];
            c.path(body, f, s);
            // front of the top ellipse
            let rim = vec![
                Seg::MoveTo([x0, top]),
                Seg::CubicTo([x0, top + ry * k], [ctr[0] - rx * k, top + ry], [ctr[0], top + ry]),
                Seg::CubicTo([ctr[0] + rx * k, top + ry], [x0 + w, top + ry * k], [x0 + w, top]),
            ];
            c.path(rim, None, s);
        }
        _ => {
            if let Some(poly) = outline(shape, ctr, w, h) {
                c.polygon(&poly, f, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::{ApproxMeasure, DiagramLang, render};

    fn ctx() -> RenderCtx<'static> {
        RenderCtx::new(&ApproxMeasure, 14.0)
    }

    #[test]
    fn basis_endpoints() {
        let segs = basis(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        assert_eq!(segs.first(), Some(&Seg::MoveTo([0.0, 0.0])));
        assert_eq!(segs.last(), Some(&Seg::LineTo([10.0, 10.0])));
    }

    #[test]
    fn boundary_hits_outlines() {
        let p = boundary(Shape::Rect, [0.0, 0.0], 20.0, 10.0, [100.0, 0.0]);
        assert!((p[0] - 10.0).abs() < 1e-3 && p[1].abs() < 1e-3);
        let p = boundary(Shape::Diamond, [0.0, 0.0], 20.0, 20.0, [100.0, 100.0]);
        assert!((p[0] - 5.0).abs() < 1e-3 && (p[1] - 5.0).abs() < 1e-3, "{p:?}");
        let p = boundary(Shape::Circle, [0.0, 0.0], 20.0, 20.0, [0.0, -50.0]);
        assert!((p[1] + 10.0).abs() < 1e-3);
    }

    #[test]
    fn flowchart_scene_is_well_formed() {
        let s = render(
            DiagramLang::Mermaid,
            "flowchart LR\nA[Start] -->|go| B{OK?}\nB -- yes --> C((Done))\nB -. no .-> A\nsubgraph S [Group]\nC\nend",
            &ctx(),
        )
        .unwrap();
        assert!(s.is_well_formed());
        assert!(s.width > 100.0 && s.height > 20.0);
        let texts: Vec<&str> = s
            .items
            .iter()
            .filter_map(|i| {
                if let super::super::Item::Text { text, .. } = i {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        for t in ["Start", "OK?", "Done", "go", "yes", "no", "Group"] {
            assert!(texts.contains(&t), "{t} missing in {texts:?}");
        }
        // LR: B is to the right of A
        let x_of = |t: &str| {
            s.items.iter().find_map(|i| match i {
                super::super::Item::Text { text, pos, .. } if text == t => Some(pos[0]),
                _ => None,
            })
        };
        assert!(x_of("OK?") > x_of("Start"));
    }

    #[test]
    fn edge_to_subgraph_goes_to_its_first_node() {
        let s = render(
            DiagramLang::Mermaid,
            "graph TD\nsubgraph one\na1-->a2\nend\nx-->one",
            &ctx(),
        )
        .unwrap();
        let texts = s
            .items
            .iter()
            .filter(|i| matches!(i, super::super::Item::Text { text, .. } if text == "one"))
            .count();
        assert_eq!(texts, 1, "subgraph id must not become a separate node");
    }

    #[test]
    fn flow_js_renders() {
        let s = render(
            DiagramLang::Flow,
            "st=>start: Start\ncond=>condition: OK?\ne=>end: End\nst->cond\ncond(yes)->e\ncond(no)->st",
            &ctx(),
        )
        .unwrap();
        assert!(s.is_well_formed());
    }

    #[test]
    fn time_budget_stops_huge_layouts() {
        let src = format!(
            "graph TD\n{}",
            (0..900)
                .map(|i| format!("n{i}-->n{}\n", (i * 7 + 3) % 900))
                .collect::<String>()
        );
        let ctx = ctx().with_limits(crate::diagram::Limits {
            time_budget: std::time::Duration::from_millis(1),
            ..Default::default()
        });
        assert_eq!(render(DiagramLang::Mermaid, &src, &ctx), Err(DiagramError::Timeout));
    }
}
