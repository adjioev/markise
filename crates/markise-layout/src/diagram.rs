//! Diagram input for diagram items (spec 01 §4.6, ADR 0009), behind a trait so that layout does
//! not own the renderer's worker; the UI adapts `markise_blocks::diagram::DiagramService` to
//! [`Diagrams`].
//!
//! Cache contract: the layout of a diagram item depends on the diagram's [`DiagramState`]
//! (off / pending / ready / failed) and on [`Diagrams::generation`] (on/off, label font size,
//! text metrics). Both enter the item's [`CacheKey`](crate::CacheKey); the scene itself is a
//! pure function of the block content (`CodeRef::key`) and the generation, and its colors of
//! the theme, which the key covers already. So a diagram is laid out as source first and once
//! more when its scene arrives.

use std::sync::Arc;

use markise_blocks::diagram::{DiagramLang, DiagramResult, TableMeasure};
use markise_doc::FormatDoc;

use crate::model::CodeRef;

/// One diagram item's question.
#[derive(Clone, Copy)]
pub struct DiagramQuery<'a> {
    pub lang: DiagramLang,
    /// Content key and length of the block.
    pub code: CodeRef,
    pub doc: &'a Arc<dyn FormatDoc>,
    /// Absolute start of the block.
    pub start: usize,
    pub item: usize,
}

impl std::fmt::Debug for DiagramQuery<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagramQuery")
            .field("lang", &self.lang)
            .field("code", &self.code)
            .field("start", &self.start)
            .field("item", &self.item)
            .finish()
    }
}

impl DiagramQuery<'_> {
    /// A `'static` builder of the block content (lines joined with `\n`) that shares the
    /// document (O(1) to create); call it on a worker thread.
    pub fn source(&self) -> impl FnOnce() -> String + Send + 'static {
        let doc = self.doc.clone();
        let start = self.start;
        let len = self.code.len;
        move || {
            let lines = doc.leaf(start).map(|l| l.lines).unwrap_or_default();
            let text = doc.text();
            let mut s = String::with_capacity(len);
            for l in &lines {
                s.push_str(l.slice(text));
                s.push('\n');
            }
            s
        }
    }
}

/// What a diagram item shows.
#[derive(Clone, Debug)]
pub enum DiagramState {
    /// Diagrams are off: styled source.
    Off,
    /// Rendering: styled source until the scene arrives.
    Pending,
    /// Rendered: the scene, or why it is not renderable (syntax error, unsupported type, too
    /// large) — then the source is shown with the error as a notice.
    Done(DiagramResult),
}

impl DiagramState {
    /// Distinguishes the states in cache keys (0 off, 1 pending, 2 ready, 3 failed).
    pub fn tag(&self) -> u64 {
        match self {
            DiagramState::Off => 0,
            DiagramState::Pending => 1,
            DiagramState::Done(r) if r.is_ok() => 2,
            DiagramState::Done(_) => 3,
        }
    }
}

/// Source of diagram scenes.
pub trait Diagrams {
    /// Changes when diagrams are switched on/off or the label metrics change (part of keys).
    fn generation(&self) -> u64;
    /// The diagram's current state (no side effects).
    fn state(&self, q: &DiagramQuery) -> DiagramState;
    /// Asks for the diagram to be rendered (cheap no-op when cached or already requested).
    fn want(&self, q: &DiagramQuery);
    /// Whether the source lacks label metrics for `fonts` (a fingerprint of theme and font
    /// faces); layout then measures them with its text stack and calls
    /// [`Diagrams::set_metrics`] (ADR 0009 §4, DG-6).
    fn needs_metrics(&self, _fonts: u64) -> bool {
        false
    }
    /// Label metrics measured with the layout's fonts (changes [`Diagrams::generation`]).
    fn set_metrics(&self, _fonts: u64, _metrics: TableMeasure) {}
}

/// Diagrams off: diagram fences show their source.
pub struct NoDiagrams;

impl Diagrams for NoDiagrams {
    fn generation(&self) -> u64 {
        0
    }

    fn state(&self, _q: &DiagramQuery) -> DiagramState {
        DiagramState::Off
    }

    fn want(&self, _q: &DiagramQuery) {}
}
