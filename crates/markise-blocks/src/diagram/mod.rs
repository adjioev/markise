//! Diagram rendering for fenced blocks `mermaid`, `sequence` and `flow` (spec 01 R3.5.6,
//! §4.6; ADR 0009).
//!
//! Pipeline: source → parser (own, per syntax) → diagram model ([`graph::Graph`] or
//! [`sequence::Sequence`]) → layout (layered graph layout through `dugong`, a Rust port of
//! dagre, or the sequence layout) → a renderer-neutral [`Scene`] (display list with color
//! roles). Painting: [`svg::to_svg`] for HTML export, the egui front end for the editor.
//!
//! Supported in v1:
//! - `mermaid`: `flowchart` / `graph` and `sequenceDiagram`. Every other Mermaid diagram type
//!   is reported as [`DiagramError::Unsupported`]; callers show the source with a notice.
//! - `sequence`: js-sequence-diagrams syntax (mapped onto the sequence model).
//! - `flow`: flowchart.js syntax (mapped onto the graph model).
//!
//! Documents are untrusted: parsers are iterative (no recursion on input nesting), every count
//! is bounded by [`Limits`], and layout runs under a time budget with cooperative
//! cancellation ([`Cancel`]). Nothing here panics on any input (property tests in `tests/`).

pub mod flowjs;
pub mod graph;
mod graph_layout;
pub mod measure;
mod mermaid_flow;
mod mermaid_seq;
pub mod palette;
pub mod scene;
pub mod sequence;
mod sequence_js;
mod service;
pub mod svg;
mod text;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use measure::{ApproxMeasure, TableMeasure, TextMeasure};
pub use palette::DiagramPalette;
pub use scene::{Anchor, Item, Paint, Role, Scene, Seg, Stroke};
pub use service::{DiagramKey, DiagramLookup, DiagramResult, DiagramService, DiagramStats, diagram_key};

/// Fence languages rendered as diagrams (spec 01 R3.5.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagramLang {
    Mermaid,
    /// js-sequence-diagrams syntax.
    Sequence,
    /// flowchart.js syntax.
    Flow,
}

impl DiagramLang {
    /// The diagram language of a fence info string's first word (case-insensitive), if any.
    pub fn from_info(lang: &str) -> Option<DiagramLang> {
        let l = lang.trim();
        if l.eq_ignore_ascii_case("mermaid") {
            Some(DiagramLang::Mermaid)
        } else if l.eq_ignore_ascii_case("sequence") {
            Some(DiagramLang::Sequence)
        } else if l.eq_ignore_ascii_case("flow") {
            Some(DiagramLang::Flow)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DiagramLang::Mermaid => "mermaid",
            DiagramLang::Sequence => "sequence",
            DiagramLang::Flow => "flow",
        }
    }
}

/// Bounds applied to every diagram (untrusted input).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_source_bytes: usize,
    /// Graph nodes, sequence participants included.
    pub max_nodes: usize,
    /// Graph edges / sequence messages and notes.
    pub max_edges: usize,
    /// Characters per label (longer labels are truncated with an ellipsis).
    pub max_label_chars: usize,
    /// Nesting of subgraphs / sequence blocks.
    pub max_depth: usize,
    /// Wall-clock budget for parse + layout.
    pub time_budget: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_source_bytes: 256 * 1024,
            max_nodes: 1000,
            max_edges: 2000,
            max_label_chars: 1000,
            max_depth: 24,
            time_budget: Duration::from_secs(2),
        }
    }
}

/// Cooperative cancellation flag shared with a running render.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Why a diagram was not rendered. Callers show the source together with [`fmt::Display`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagramError {
    /// No content (spec 02 §8: placeholder "Empty Diagram").
    Empty,
    /// Syntax error at a 1-based source line.
    Syntax {
        line: usize,
        message: String,
    },
    /// A Mermaid diagram type Markise does not render yet (e.g. `classDiagram`).
    Unsupported {
        kind: String,
    },
    /// A bound of [`Limits`] was exceeded.
    TooLarge(String),
    /// The time budget ran out.
    Timeout,
    Cancelled,
}

impl fmt::Display for DiagramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagramError::Empty => f.write_str("Empty Diagram"),
            DiagramError::Syntax { line, message } => write!(f, "Diagram syntax error (line {line}): {message}"),
            DiagramError::Unsupported { kind } => {
                write!(
                    f,
                    "Mermaid diagram type \u{201c}{kind}\u{201d} is not supported yet; showing the source"
                )
            }
            DiagramError::TooLarge(what) => write!(f, "Diagram too large to render ({what}); showing the source"),
            DiagramError::Timeout => f.write_str("Diagram took too long to render; showing the source"),
            DiagramError::Cancelled => f.write_str("Diagram rendering was cancelled"),
        }
    }
}

impl std::error::Error for DiagramError {}

/// Inputs of one render.
pub struct RenderCtx<'a> {
    pub measure: &'a dyn TextMeasure,
    /// Base font size of labels in points (from the active style).
    pub font_size: f32,
    pub limits: Limits,
    pub cancel: Option<&'a Cancel>,
    deadline: Instant,
}

impl<'a> RenderCtx<'a> {
    pub fn new(measure: &'a dyn TextMeasure, font_size: f32) -> Self {
        let limits = Limits::default();
        RenderCtx {
            measure,
            font_size: sane_font_size(font_size),
            limits,
            cancel: None,
            deadline: Instant::now() + limits.time_budget,
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self.deadline = Instant::now() + limits.time_budget;
        self
    }

    pub fn with_cancel(mut self, cancel: &'a Cancel) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// `Err` when cancelled or over the time budget.
    pub fn check(&self) -> Result<(), DiagramError> {
        if self.cancel.is_some_and(Cancel::is_cancelled) {
            return Err(DiagramError::Cancelled);
        }
        if Instant::now() > self.deadline {
            return Err(DiagramError::Timeout);
        }
        Ok(())
    }
}

/// Hard cap on parser work units per diagram (independent of the time budget, so a parse is
/// bounded even without a deadline).
pub(crate) const MAX_PARSE_WORK: usize = 20_000_000;

/// Parser work accounting: every parser charges units for what it scans or creates; the
/// render's cancellation flag and deadline are checked every 64 Ki units, and the total is
/// capped by [`MAX_PARSE_WORK`].
pub(crate) struct Work<'a> {
    ctx: Option<&'a RenderCtx<'a>>,
    units: usize,
    next: usize,
}

impl<'a> Work<'a> {
    pub fn new(ctx: &'a RenderCtx<'a>) -> Self {
        Work {
            ctx: Some(ctx),
            units: 0,
            next: 1 << 16,
        }
    }

    /// No deadline (tests, direct parser use); the hard cap still applies.
    pub fn unbounded() -> Work<'static> {
        Work {
            ctx: None,
            units: 0,
            next: 1 << 16,
        }
    }

    pub fn charge(&mut self, n: usize) -> Result<(), DiagramError> {
        self.units = self.units.saturating_add(n);
        if self.units > MAX_PARSE_WORK {
            return Err(DiagramError::TooLarge("too much work to parse".into()));
        }
        if self.units >= self.next {
            self.next = self.units.saturating_add(1 << 16);
            if let Some(c) = self.ctx {
                c.check()?;
            }
        }
        Ok(())
    }
}

fn sane_font_size(s: f32) -> f32 {
    if s.is_finite() { s.clamp(6.0, 72.0) } else { 14.0 }
}

/// Parses and lays out one diagram fence.
pub fn render(lang: DiagramLang, source: &str, ctx: &RenderCtx) -> Result<Scene, DiagramError> {
    if source.len() > ctx.limits.max_source_bytes {
        return Err(DiagramError::TooLarge(format!("{} KB source", source.len() / 1024)));
    }
    if source.trim().is_empty() {
        return Err(DiagramError::Empty);
    }
    ctx.check()?;
    let scene = match lang {
        DiagramLang::Mermaid => match mermaid_kind(source) {
            MermaidKind::Flowchart => {
                graph_layout::layout(&mermaid_flow::parse(source, &ctx.limits, &mut Work::new(ctx))?, ctx)?
            }
            MermaidKind::Sequence => {
                sequence::layout(&mermaid_seq::parse(source, &ctx.limits, &mut Work::new(ctx))?, ctx)?
            }
            MermaidKind::Other(kind) => return Err(DiagramError::Unsupported { kind }),
            MermaidKind::Unknown(line, word) => {
                return Err(DiagramError::Syntax {
                    line,
                    message: format!("unknown diagram type \u{201c}{word}\u{201d}"),
                });
            }
            MermaidKind::Empty => return Err(DiagramError::Empty),
        },
        DiagramLang::Sequence => sequence::layout(&sequence_js::parse(source, &ctx.limits, &mut Work::new(ctx))?, ctx)?,
        DiagramLang::Flow => graph_layout::layout(&flowjs::parse_with(source, &ctx.limits, &mut Work::new(ctx))?, ctx)?,
    };
    debug_assert!(scene.is_well_formed());
    check_scene_size(&scene)?;
    Ok(scene)
}

/// Largest scene a painter is asked to show (the viewer lays a diagram out as one item; a
/// bigger drawing is shown as source with a notice instead).
pub const MAX_SCENE_SIDE: f32 = 30_000.0;
pub const MAX_SCENE_ITEMS: usize = 30_000;
pub const MAX_SCENE_TEXTS: usize = 8_000;

fn check_scene_size(s: &Scene) -> Result<(), DiagramError> {
    let texts = s.items.iter().filter(|i| matches!(i, Item::Text { .. })).count();
    if s.width > MAX_SCENE_SIDE
        || s.height > MAX_SCENE_SIDE
        || s.items.len() > MAX_SCENE_ITEMS
        || texts > MAX_SCENE_TEXTS
    {
        return Err(DiagramError::TooLarge(format!(
            "{:.0} × {:.0} pt, {} shapes",
            s.width,
            s.height,
            s.items.len()
        )));
    }
    Ok(())
}

enum MermaidKind {
    Flowchart,
    Sequence,
    Other(String),
    /// 1-based line and first word.
    Unknown(usize, String),
    Empty,
}

/// Mermaid diagram types known to exist (rendered or not), by their header keyword.
const MERMAID_OTHER_TYPES: &[&str] = &[
    "classDiagram",
    "classDiagram-v2",
    "stateDiagram",
    "stateDiagram-v2",
    "erDiagram",
    "journey",
    "gantt",
    "pie",
    "quadrantChart",
    "requirementDiagram",
    "gitGraph",
    "C4Context",
    "C4Container",
    "C4Component",
    "C4Dynamic",
    "C4Deployment",
    "mindmap",
    "timeline",
    "zenuml",
    "sankey-beta",
    "sankey",
    "xychart-beta",
    "xychart",
    "block-beta",
    "block",
    "packet-beta",
    "packet",
    "kanban",
    "architecture-beta",
    "architecture",
    "radar-beta",
    "treemap-beta",
    "treemap",
    "info",
];

/// Lines of a Mermaid source after the optional YAML front matter, with 1-based numbers;
/// `%%` comment lines and blank lines are skipped by callers.
pub(crate) fn mermaid_body(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut lines = source.lines().enumerate().map(|(i, l)| (i + 1, l)).peekable();
    // front matter: `---` on the first non-blank line … `---`
    let mut skip_to = 0usize;
    let first = source.lines().enumerate().find(|(_, l)| !l.trim().is_empty());
    if let Some((i, l)) = first
        && l.trim() == "---"
        && let Some((j, _)) = source.lines().enumerate().skip(i + 1).find(|(_, l)| l.trim() == "---")
    {
        skip_to = j + 1;
    }
    while lines.peek().is_some_and(|(n, _)| *n <= skip_to) {
        lines.next();
    }
    lines
}

/// Whether a Mermaid line carries no statement (blank, `%%` comment or `%%{…}%%` directive).
pub(crate) fn is_mermaid_noise(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with("%%")
}

fn mermaid_kind(source: &str) -> MermaidKind {
    for (n, line) in mermaid_body(source) {
        if is_mermaid_noise(line) {
            continue;
        }
        let word = line
            .split(|c: char| c.is_whitespace() || c == ';' || c == ':')
            .find(|w| !w.is_empty())
            .unwrap_or("");
        let lower = word.to_ascii_lowercase();
        if lower == "flowchart" || lower == "graph" || lower == "flowchart-elk" {
            return MermaidKind::Flowchart;
        }
        if word == "sequenceDiagram" {
            return MermaidKind::Sequence;
        }
        if let Some(k) = MERMAID_OTHER_TYPES.iter().find(|k| k.eq_ignore_ascii_case(word)) {
            return MermaidKind::Other((*k).to_string());
        }
        let shown: String = word.chars().take(40).collect();
        return MermaidKind::Unknown(n, shown);
    }
    MermaidKind::Empty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RenderCtx<'static> {
        RenderCtx::new(&ApproxMeasure, 14.0)
    }

    #[test]
    fn r3_5_6_fence_languages() {
        assert_eq!(DiagramLang::from_info("Mermaid"), Some(DiagramLang::Mermaid));
        assert_eq!(DiagramLang::from_info("SEQUENCE"), Some(DiagramLang::Sequence));
        assert_eq!(DiagramLang::from_info("flow"), Some(DiagramLang::Flow));
        assert_eq!(DiagramLang::from_info("math"), None);
        assert_eq!(DiagramLang::from_info("rust"), None);
    }

    #[test]
    fn empty_diagram_placeholder() {
        // spec 02 §8: empty diagram shows "Empty Diagram"
        for lang in [DiagramLang::Mermaid, DiagramLang::Sequence, DiagramLang::Flow] {
            let e = render(lang, "  \n\n", &ctx()).unwrap_err();
            assert_eq!(e, DiagramError::Empty);
            assert_eq!(e.to_string(), "Empty Diagram");
        }
        assert_eq!(
            render(DiagramLang::Mermaid, "%% only a comment\n", &ctx()),
            Err(DiagramError::Empty)
        );
    }

    #[test]
    fn unsupported_mermaid_types_are_reported() {
        let e = render(DiagramLang::Mermaid, "classDiagram\n  A <|-- B\n", &ctx()).unwrap_err();
        assert_eq!(
            e,
            DiagramError::Unsupported {
                kind: "classDiagram".into()
            }
        );
        assert!(e.to_string().contains("not supported"));
        let e = render(DiagramLang::Mermaid, "---\ntitle: x\n---\npie\n \"a\": 1\n", &ctx()).unwrap_err();
        assert_eq!(e, DiagramError::Unsupported { kind: "pie".into() });
    }

    #[test]
    fn unknown_mermaid_type_is_a_syntax_error() {
        let e = render(DiagramLang::Mermaid, "\nnonsense here\n", &ctx()).unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { line: 2, .. }), "{e:?}");
    }

    #[test]
    fn oversized_source_is_refused() {
        let src = format!("graph TD\n{}", "A-->B\n".repeat(60_000));
        assert!(matches!(
            render(DiagramLang::Mermaid, &src, &ctx()),
            Err(DiagramError::TooLarge(_))
        ));
    }

    #[test]
    fn cancelled_render_stops() {
        let c = Cancel::new();
        c.cancel();
        let ctx = ctx().with_cancel(&c);
        assert_eq!(
            render(DiagramLang::Mermaid, "graph TD\nA-->B\n", &ctx),
            Err(DiagramError::Cancelled)
        );
    }
}
