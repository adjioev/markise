//! The format seam (ADR 0006 §1.2): `DocumentFormat` (factory + metadata), `FormatDoc` (a
//! parsed document) and `EditBehavior` (format-specific planning on the concrete document, so
//! planners never downcast).

use std::any::Any;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::{CommandDescriptor, PlannerId};
use crate::hints::{HintSet, LiteralHint};
use crate::owner::{Adjacent, BlockOwner, BlockPart, ContainerAt};
use crate::patch::PatchSet;
use crate::plan::{Plan, PlanError, PlanRequest, TypingPlan, plain_typing};
use crate::pos::{Affinity, DocPos, VirtualSlot};
use crate::source::SourceToken;
use crate::text::{LineEnding, Span};
use crate::view::{FlatEvent, HeadingInfo, InlineSource, LeafView, RichInline, TableView};

/// Format-neutral options for opening a document. Format-specific settings (e.g. Markdown's
/// parse options) live in the `DocumentFormat` instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatOptions {
    /// Line ending for documents that contain none (R2.2).
    pub default_eol: LineEnding,
}

/// What one text/hint change did to the document: enough for incremental consumers
/// (`markise-layout`'s `DocModel::apply`) to update only what changed (ADR 0006 §2.1, §7.2.6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditSummary {
    /// Pre-edit byte range covering every change (empty for hint-only changes at a point).
    pub changed: Range<usize>,
    /// Length of the text now occupying `changed`.
    pub new_len: usize,
    /// Byte range that was re-parsed (post-edit coordinates).
    pub reparsed: Range<usize>,
    /// Net length change.
    pub delta: isize,
    /// Top-level blocks replaced (indices before the edit) …
    pub replaced_top: Range<usize>,
    /// … and the top-level blocks that replaced them (indices after the edit). Blocks after
    /// them are unchanged apart from a shift by `delta`.
    pub new_top: Range<usize>,
    /// Document-level dependencies whose value changed (ADR 0006 §7.4).
    pub deps: DepChange,
    /// Intra-line fast path (ADR 0006 §7.2.1): only the content of the leaf starting at this
    /// (absolute, unchanged) offset changed; the block structure is the same, everything after
    /// the edit shifted by `delta`. `replaced_top`/`new_top`/`reparsed` still describe the
    /// enclosing top-level block, so consumers that ignore this field stay correct.
    pub inline_leaf: Option<usize>,
    /// Nested restart (ADR 0006 §7.2.3): inside top-level block `new_top.start`, only the
    /// leaves from this (absolute, unchanged) offset — the start of the first re-parsed child of
    /// a container — up to `reparsed.end` changed; the containers around them kept their
    /// computed state (list tightness, callout kind); `reparsed.end` is then the end of that
    /// region, everything after it shifted by `delta`. (When this is `None`, `reparsed` covers
    /// whole top-level blocks.)
    pub nested_from: Option<usize>,
}

impl EditSummary {
    /// A summary for "everything may have changed" (full re-parse) of a document that had
    /// `old_top` top-level blocks and now has `new_top`.
    pub fn full(old_top: usize, new_top: usize, old_len: usize, new_len: usize) -> Self {
        EditSummary {
            changed: 0..old_len,
            new_len,
            reparsed: 0..new_len,
            delta: new_len as isize - old_len as isize,
            replaced_top: 0..old_top,
            new_top: 0..new_top,
            deps: DepChange::all(),
            inline_leaf: None,
            nested_from: None,
        }
    }

    /// Combines `self` with `next`, a change applied right after it, into one summary of
    /// both (in `self`'s before and `next`'s after coordinates).
    pub fn then(self, next: EditSummary) -> EditSummary {
        if next.replaced_top.is_empty() && next.new_top.is_empty() && next.changed.is_empty() && next.delta == 0 {
            let mut s = self;
            s.deps.merge(next.deps);
            return s;
        }
        if self.replaced_top.is_empty() && self.new_top.is_empty() && self.changed.is_empty() && self.delta == 0 {
            let mut n = next;
            n.deps.merge(self.deps);
            return n;
        }
        // block ranges: union in the intermediate coordinates, then mapped back/forward
        let (s1, r1, n1) = (self.replaced_top.start, self.replaced_top.len(), self.new_top.len());
        let (s2, r2, n2) = (next.replaced_top.start, next.replaced_top.len(), next.new_top.len());
        let (replaced_top, new_top) = if r1 == 0 && n1 == 0 {
            (next.replaced_top.clone(), next.new_top.clone())
        } else if r2 == 0 && n2 == 0 {
            (self.replaced_top.clone(), self.new_top.clone())
        } else {
            let a = s1.min(s2);
            let b = (s1 + n1).max(s2 + r2);
            (a..b + r1 - n1, a..b + n2 - r2)
        };
        // byte ranges: `changed` in the original coordinates
        let self_noop = self.changed.is_empty() && self.new_len == 0 && self.delta == 0;
        let (changed, new_len) = if self_noop {
            (next.changed.clone(), next.new_len)
        } else {
            let mid_lo = self.changed.start.min(next.changed.start);
            let mid_hi = (self.changed.start + self.new_len).max(next.changed.end);
            (
                mid_lo..(mid_hi as isize - self.delta) as usize,
                (mid_hi as isize - mid_lo as isize + next.delta) as usize,
            )
        };
        let shift = |r: &Range<usize>| -> Range<usize> {
            let f = |x: usize| {
                if x >= next.changed.end {
                    (x as isize + next.delta) as usize
                } else {
                    x.min(next.changed.start + next.new_len)
                }
            };
            f(r.start)..f(r.end)
        };
        let r1 = shift(&self.reparsed);
        let reparsed = r1.start.min(next.reparsed.start)..r1.end.max(next.reparsed.end);
        let mut deps = self.deps;
        deps.merge(next.deps);
        EditSummary {
            changed,
            new_len,
            reparsed,
            delta: self.delta + next.delta,
            replaced_top,
            new_top,
            deps,
            inline_leaf: None,
            nested_from: None,
        }
    }
}

/// Document-level dependencies changed by an edit (ADR 0006 §7.4): only items that depend on
/// them need new cache keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DepChange {
    /// Reference labels (format-normalised, hashed with [`label_key`]) whose definition was
    /// added, removed or changed — including labels that were looked up without success.
    pub labels: Vec<u64>,
    /// The resource root (e.g. `typora-root-url`) changed: image paths resolve differently.
    pub resources: bool,
    /// Everything may have changed (full re-parse, option change).
    pub all: bool,
}

impl DepChange {
    pub fn all() -> Self {
        DepChange {
            all: true,
            ..Default::default()
        }
    }
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && !self.resources && !self.all
    }
    pub fn merge(&mut self, o: DepChange) {
        self.labels.extend(o.labels);
        self.labels.sort_unstable();
        self.labels.dedup();
        self.resources |= o.resources;
        self.all |= o.all;
    }
}

/// Stable 64-bit key of a normalised reference label (FNV-1a; stable across runs).
pub fn label_key(normalized: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in normalized.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Factory and metadata for one file type ("markdown"; later "org").
pub trait DocumentFormat: Send + Sync {
    fn id(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn parse(&self, text: String, opts: &FormatOptions) -> Box<dyn FormatDoc>;
    /// Commands this format adds (pure data; the engine makes them executable).
    fn commands(&self) -> &'static [CommandDescriptor] {
        &[]
    }
}

/// A parsed document. Text is the source of truth; every mutation is a patch set or a hint
/// change. `Sync` so that a parsed snapshot can be shared with workers (`Arc<dyn FormatDoc>`).
pub trait FormatDoc: EditBehavior + Send + Sync {
    fn format_id(&self) -> &'static str;
    fn text(&self) -> &str;
    fn line_ending(&self) -> LineEnding;
    /// Applies one patch set: one text change, one re-parse. Hints are transformed like live
    /// marks (see [`HintSet::transform`]).
    ///
    /// # Panics
    /// When `patches` is not valid for the current text ([`PatchSet::validate`]).
    fn apply(&mut self, patches: &PatchSet) -> EditSummary;
    /// Applies `patches` and then makes `hints` (when given) the complete hint set, as one
    /// change; the summary covers both (text and re-projection). Formats should install the
    /// hints of the edited lines before re-parsing (the default applies and then re-projects,
    /// i.e. two parses).
    ///
    /// # Panics
    /// When `patches` is not valid for the current text.
    fn apply_with_hints(&mut self, patches: &PatchSet, hints: Option<&HintSet>) -> EditSummary {
        let s = self.apply(patches);
        if let Some(h) = hints
            && h != self.hints()
        {
            let s2 = self.set_hints(h);
            return s.then(s2);
        }
        s
    }
    fn hints(&self) -> &HintSet;
    /// Replaces the hint set and re-projects the lines whose hint changed (no text change).
    fn set_hints(&mut self, hints: &HintSet) -> EditSummary;
    /// The leaf block containing (or nearest before) byte offset `at`.
    fn leaf(&self, at: usize) -> Option<LeafView>;
    /// The block next to `leaf` in document order (after it with `forward`, else before it),
    /// for edits that join neighbouring blocks: `None` when there is no such block,
    /// `Some(None)` when there is one but no text-holding leaf begins (`forward`) or ends
    /// (backward) at its edge — an empty container, a block the format does not model as a
    /// leaf — and `Some(Some(leaf))` otherwise. A join must never reach past this neighbour.
    /// [`FormatDoc::adjacent_block`] from the leaf's first byte.
    fn adjacent_leaf(&self, leaf: &LeafView, forward: bool) -> Option<Option<LeafView>> {
        self.adjacent_block(leaf.span.start, forward).map(|a| a.leaf)
    }

    // ---- block ownership (ADR 0011, `crate::owner`) ------------------------------------

    /// **The** position → block ownership query: the leaf that owns offset `at` and the part
    /// of it `at` is in, by the rule documented in [`crate::owner`]; `None` between blocks.
    /// Formats with a block tree answer from it in O(log n); the default uses
    /// [`FormatDoc::leaf`] plus a scan of `at`'s own line.
    fn block_owner(&self, at: usize) -> Option<BlockOwner> {
        crate::owner::owner_by_lines(self.text(), at, &|p| self.leaf(p))
    }
    /// [`FormatDoc::block_owner`], with offsets between blocks resolved to the leaf before
    /// (`Upstream`) or after (`Downstream`) them, part [`BlockPart::Gap`] — never past an
    /// intervening block without a text leaf (then `None`).
    fn block_owner_biased(&self, at: usize, bias: Affinity) -> Option<BlockOwner> {
        if let Some(o) = self.block_owner(at) {
            return Some(o);
        }
        let leaf = self.adjacent_block(at, bias == Affinity::Downstream)?.leaf?;
        let edge = match bias {
            Affinity::Upstream => leaf.span.end,
            Affinity::Downstream => leaf.span.start,
        };
        let mut o = BlockOwner::resolve(self.text(), leaf, edge)?;
        o.at = at;
        o.part = BlockPart::Gap;
        Some(o)
    }
    /// The block next to offset `at` in document order (after it with `forward`, else before
    /// it): the sibling of the innermost block holding `at` — or, between blocks, the nearest
    /// block of the innermost container holding `at` — climbing out of containers that end
    /// (start) there. Never skips a block: see [`Adjacent`]. The default scans lines
    /// conservatively (the first non-blank line must start / end inside a leaf).
    fn adjacent_block(&self, at: usize, forward: bool) -> Option<Adjacent> {
        let text = self.text();
        let b = text.as_bytes();
        let here = self.leaf(at);
        if forward {
            let mut p = here.as_ref().map_or(at, |l| l.span.end.max(at));
            while p < b.len() && !matches!(b[p], b'\n' | b'\r') {
                p += 1;
            }
            while p < b.len() && b[p].is_ascii_whitespace() {
                p += 1;
            }
            if p >= b.len() {
                return None;
            }
            let leaf = self.leaf(p).filter(|n| n.span.start > at && n.span.start <= p);
            Some(Adjacent {
                block: leaf.as_ref().map_or(Span::new(p, p), |l| l.span),
                leaf,
            })
        } else {
            let mut p = here.as_ref().map_or(at, |l| l.span.start.min(at));
            while p > 0 && !matches!(b[p - 1], b'\n' | b'\r') {
                p -= 1;
            }
            while p > 0 && b[p - 1].is_ascii_whitespace() {
                p -= 1;
            }
            if p == 0 {
                return None;
            }
            let leaf = self.leaf(p - 1).filter(|n| n.span.end < at && n.span.end >= p);
            Some(Adjacent {
                block: leaf.as_ref().map_or(Span::new(p, p), |l| l.span),
                leaf,
            })
        }
    }
    /// The containers holding offset `at`, outermost first, with their spans (also between
    /// blocks, e.g. on an empty list item). The default knows only the kinds of the leaf at
    /// `at` and reports the leaf's span for each.
    fn containers_at(&self, at: usize) -> Vec<ContainerAt> {
        self.leaf(at).map_or_else(Vec::new, |l| {
            l.containers
                .iter()
                .map(|k| ContainerAt {
                    kind: k.clone(),
                    span: l.span,
                })
                .collect()
        })
    }
    /// The neutral inline tree of `leaf`, offsets relative to its first content byte.
    fn inlines(&self, leaf: &LeafView) -> RichInline {
        match leaf.lines.first() {
            Some(first) => self.inline(InlineSource::Lines(&leaf.lines), first.start),
            None => RichInline::default(),
        }
    }
    /// The neutral inline tree (with rendered values) of `src`; offsets relative to `base`
    /// (absolute; usually the item's start, so source maps are item-relative). Inline
    /// markup of the format applies only to leaves that have it (paragraphs, headings,
    /// cells); the caller decides which content to ask for.
    fn inline(&self, src: InlineSource<'_>, base: usize) -> RichInline;

    // ---- layout views (ADR 0006 §1.4) --------------------------------------------------

    /// Number of top-level blocks.
    fn top_count(&self) -> usize;
    /// Emits the leaves of top-level blocks `tops` with their container chains, in order.
    fn flatten(&self, tops: Range<usize>, sink: &mut dyn FnMut(FlatEvent<'_>));
    /// Emits the events needed for the leaves of top-level blocks `tops` that start in
    /// `from..to` (absolute): `Resume` events for the containers enclosing `from`, then the
    /// blocks from `from` on. Leaves outside the region may be emitted too (callers filter by
    /// position). Default: [`FormatDoc::flatten`] of `tops`.
    fn flatten_region(&self, tops: Range<usize>, from: usize, to: usize, sink: &mut dyn FnMut(FlatEvent<'_>)) {
        let _ = (from, to);
        self.flatten(tops, sink);
    }
    /// The table leaf starting at absolute offset `at` (its `FlatLeaf::abs_span().start`).
    fn table(&self, at: usize) -> Option<TableView>;
    /// Cell content spans (absolute, trimmed) of one table row (absolute span of the row).
    fn table_row_cells(&self, row: Span) -> Vec<Span>;
    /// Outline contribution of the heading leaf starting at absolute offset `at`.
    fn heading(&self, at: usize) -> Option<HeadingInfo>;
    /// The anchor id of a heading whose key is shared by `n_before` earlier anchors.
    fn anchor_id(&self, key: &str, n_before: usize) -> String {
        if n_before == 0 {
            key.to_string()
        } else {
            format!("{key}-{}", n_before + 1)
        }
    }
    /// Appends to `out` the reference labels a leaf with these content lines (absolute) may
    /// look up — a superset of what its inline parse attempts, resolved or not ([`label_key`]s,
    /// sorted and deduplicated within the appended part; ADR 0006 §7.4).
    fn reference_candidates(&self, lines: &[Span], out: &mut Vec<u64>) {
        let _ = (lines, out);
    }
    /// Fingerprint of the current definition of reference label `key` (`None`: undefined).
    fn reference_fingerprint(&self, key: u64) -> Option<u64> {
        let _ = key;
        None
    }
    /// One combined fingerprint of the definitions of `keys` (in order).
    fn reference_fingerprints(&self, keys: &[u64]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for k in keys {
            h = (h ^ k ^ self.reference_fingerprint(*k).unwrap_or(0x9e37_79b9)).wrapping_mul(0x0100_0000_01b3);
        }
        h
    }
    /// Source-mode tokens (spec 02 §12.1.1) of the text in `range` (absolute): ascending,
    /// non-overlapping runs clipped to `range` are appended to `out`; bytes without a run are
    /// plain. Computed from the format's parse; only the blocks overlapping `range` are
    /// visited, so the cost follows the range, not the document. Default: no tokens.
    fn source_tokens(&self, range: Range<usize>, out: &mut Vec<SourceToken>) {
        let _ = (range, out);
    }
    /// Root for absolute resource paths (Markdown: front matter `typora-root-url`).
    fn resource_root(&self) -> Option<String> {
        None
    }
    /// Resolves an image source against the document folder and [`FormatDoc::resource_root`].
    fn resolve_resource(&self, doc_dir: &Path, src: &str) -> PathBuf {
        crate::resource::resolve(doc_dir, self.resource_root().as_deref(), src)
    }
    /// The concrete document, for format-specific consumers above the seam (exporters,
    /// per-format tools in the app). Never needed by layout, commands or the editor.
    fn as_any(&self) -> &dyn Any;
    /// Compact projected block tree (tests assert bytes + tree + selection).
    fn debug_tree(&self) -> String;
    fn snapshot(&self) -> Arc<str> {
        Arc::from(self.text())
    }
    fn clone_box(&self) -> Box<dyn FormatDoc>;
    /// Canonical forward patches (ADR 0006 §2.1): newly typed line breaks converted to the
    /// document's line ending; every patch marked verbatim.
    fn canonicalize(&self, patches: &PatchSet) -> PatchSet {
        patches.canonical_eol(self.line_ending())
    }
    /// `true` when `offset` starts a physical line in the current text.
    fn is_line_start(&self, offset: usize) -> bool {
        crate::text::is_line_start(self.text(), offset)
    }
}

/// Format-specific editing. All methods are planners or queries: nothing mutates.
///
/// Deferred to later milestones (ADR 0006): `line_signature` / `opener_unchanged_proof` (§7.2,
/// fast path), `reveal` (§2.4), `clip` (§6.1).
pub trait EditBehavior {
    /// Runs planner `id`. Default: `Unsupported`.
    fn plan(&self, id: PlannerId, req: &PlanRequest) -> Result<Plan, PlanError> {
        let _ = (id, req);
        Err(PlanError::Unsupported)
    }
    /// Cheap applicability check (used for `enabled`).
    fn can_plan(&self, id: PlannerId, req: &PlanRequest) -> bool {
        self.plan(id, req).is_ok()
    }
    /// Whether toggle command `id` is active at the selection (menu check marks, toolbar
    /// state: e.g. the selection is already strong). Default: never.
    fn plan_checked(&self, id: PlannerId, req: &PlanRequest) -> bool {
        let _ = (id, req);
        false
    }
    /// Typing pipeline step 4–5 (ADR 0006 §5.1): the keystroke step (with any `EnterPending`
    /// hint) and an immediate rule completed by it.
    fn on_text(&self, req: &PlanRequest, typed: &str) -> TypingPlan {
        plain_typing(req, typed)
    }
    /// Escape patch making a hinted line parse as what the user saw (ADR 0006 §2.6). `None`
    /// when the construct cannot be escaped (the hint is dropped).
    fn materialize_hint(&self, hint: &LiteralHint) -> Option<PatchSet> {
        let _ = hint;
        None
    }
    /// Re-resolves a virtual live mark after its defining structure changed (§2.2). Default:
    /// the format defines no virtual slots.
    fn resolve_virtual(&self, slot: &VirtualSlot) -> Option<DocPos> {
        let _ = slot;
        None
    }
    /// Container prefix to repeat on a new line at `at` (e.g. `> `).
    fn continuation_prefix(&self, at: usize) -> String {
        let _ = at;
        String::new()
    }
    /// Auto-pair characters (spec 02 §4).
    fn pairs(&self) -> &'static [(char, char)] {
        &[]
    }
}
