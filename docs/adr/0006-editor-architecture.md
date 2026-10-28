# ADR 0006 — Editor architecture for M2: format seam, patch-based editing, command registry, undo

Status: **proposed** (2026-10-27, revision 4 after three Codex review passes the same day; the
review items and where each is addressed are listed in §12). Docs only; no production code is
changed by this ADR.
Scope: new `crates/markise-doc` (L1), `crates/markise-md` (L1), `crates/markise-layout` (L2),
`crates/markise-engine` (L4), `crates/markise-ui-egui` (L6), a test-only format crate, later
`markise-mcp` (L5) and `apps/markise-cli`. Depends on ADR 0001 (accepted), ADR 0002 (accepted)
and ADR 0005 (**proposed**; every statement that relies on parley, `DocPos`/`FragmentId` or ADR
0005's composition state is marked *[0005]* and must be revisited if that ADR changes).

Inputs: `docs/spec/02-editing-behavior.md` ("spec 02"), `docs/spec/01-markdown-dialect.md`
("spec 01"), `docs/spec/04-app-features.md` ("spec 04"), `docs/spec/99-open-questions.md` (C-,
M1-items), `docs/spec/README.md` (NFRs, MVP scope), `docs/research/wordcraft-reuse.md` ("research
note", §2 commands, §3 undo, §5 IME, §6 clipboard, §8 find). Claims about current code cite files
at `origin/main` 569f376. Anything not verified in code or spec is marked **(assumption)**; every
performance figure that is not a measurement is a **hypothesis** to be confirmed by the gates of §7.

## Decision summary

1. **Format seam before editor work.** A new GUI-free crate `markise-doc` (L1, below
   `markise-md`) owns format-neutral *data and traits only*: `Span`, `Patch`/`PatchSet`,
   `DocPos`, `Selection`, marks and transforms, the neutral block view, the neutral inline view
   (styled runs with marker ranges and a source map), **command descriptors** (pure data) and the
   `DocumentFormat` / `FormatDoc` traits. Planning methods live **on the concrete document**
   (`FormatDoc: EditBehavior`), so no downcasts are needed. Executable commands are assembled in
   the engine (L4). A tiny second format (`markise-testfmt`) proves the seam and the layering (§1).
2. **Text is the source of truth; the projection is text + literal-line hints.** Every edit is a
   patch set applied through the format document. Input-rule undo and "paragraph until Enter"
   (S-2.1) are expressed as *literal-line hints* — editor-only interpretation state that is part of
   transactions, expires at defined points and is materialised as Markdown escapes so the saved
   bytes always parse to what the user saw (§2.6).
3. **Carets are source offsets + affinity** (ADR 0005 §4.3 *[0005]*); fragment source maps are
   item-relative; empty fragments, padded table cells and capped long cells get explicit caret
   anchors (§2.2–§2.3).
4. **Marker reveal is a layout input of the focused item only** (cache key `extra`) (§2.4).
5. **One command registry** in `markise-engine` built from format/engine descriptors; WordCraft's
   design adapted (research note §2); `keyBinding` overrides by label or id (§3).
6. **Undo = transactions of ordered steps** (canonical forward patches, verbatim inverses, hint
   changes), immutable per-transaction selections, a journal for live marks, deferred external
   effects, rollback, byte-bounded history, a `counter`/`StateId` revision split (§4).
7. **NFR-4 needs a parse/model plan, not only span shifting:** an intra-line fast path guarded by a
   format line signature, measured gates G1–G3,
   failed-opener restarts that never change parse semantics, synchronous large reparses with an
   optional strictly-ordered asynchronous mode behind gate G4; dependency-scoped invalidation
   including negative reference lookups and HTML-anchor/slug propagation; find with bounded/unbounded query classes (§6.2, §7).
8. **Milestones M2a–M2e** with a corrected feature matrix; development on the epaint shaper behind
   ADR 0005 T2; ADR 0005's IME behaviour and real-IME acceptance gate are kept unchanged (§6.3, §8).

---

## 1. "Format as plugin" audit

AGENTS.md (M2) requires that "the layout model, caches, caret/selection model and editing
commands must not depend on Markdown-specific types" and that Markdown behaviour lives behind a
format trait, so that a later `markise-org` parser can produce the same block model.

### 1.1 Findings: coupling points in the current code

Caret/selection model and editing commands **do not exist yet** (verified: no caret, selection or
command types in any crate), so they must be built neutral from the start. The coupling is in
layout, the engine jobs and two UI files:

| # | Where | Markdown-specific dependency | Severity |
|---|---|---|---|
| C-1 | `crates/markise-layout/Cargo.toml` | `markise-md` is a normal dependency of L2 layout | structural |
| C-2 | `crates/markise-layout/src/model.rs:11-13,185` | `DocModel` **owns** `pub doc: markise_md::Document`; imports `Block`, `BlockKind`, `AlertKind`, `InlineContext`, `parse_inlines`, `plain_text`, `SlugTracker` | high |
| C-3 | `model.rs` `Flattener` (from `:370`), `leaf_kind` (`:686`) | flattening matches `BlockKind::{BlockQuote, List, ListItem, FootnoteDef, Table{aligns}, …}` | high (this *is* the format→model mapping; it belongs to the format) |
| C-4 | `model.rs:17-31` | `Container::Quote { alert: Option<markise_md::AlertKind> }` | low |
| C-5 | `model.rs:103-130,149-169,619` | `Item.span`, `Segment::Lines.spans`, `split_units` use `markise_md::Span` | low |
| C-6 | `model.rs` outline in `DocModel::build` | heading text via `parse_inlines` + `plain_text`, anchors via `SlugTracker` (spec 01 §7) | medium |
| C-7 | `model.rs:263-268` | `typora-root-url` via `markise_md::frontmatter`; `ctx_hash` hashes Markdown link definitions and footnote labels | medium |
| C-8 | `crates/markise-layout/src/block.rs:12-13,282-490` | `InlineBuilder::node` walks `markise_md::inline::InlineKind`, `decode_entity`; `layout_item` re-runs `parse_inlines` per paragraph/heading/cell (`:606`, `:643`, `:1121`) | high (the renderer parses Markdown) |
| C-9 | `block.rs:1076-1121` `layout_table` | matches `BlockKind::Table { aligns }`, `markise_md::table::row_cells`; cells capped at `MAX_CELL_BYTES` = 2 KiB (`model.rs:649`, `cap_cell`) | medium |
| C-10 | `block.rs:319`, `:491` | `markise_md::frontmatter::resolve_image_src`; `alert_index(markise_md::AlertKind)` | low |
| C-11 | `block.rs` `layout_item` (`:503`) | resolves the leaf through `model.block(idx)` → `markise_md::Block`, reads `block.lines` / `block.content(text)` | high |
| C-12 | `model.rs:149-155` | `Item.path` = absolute block indices into the Markdown tree (also a perf issue, §7) | medium |
| C-13 | `crates/markise-engine/src/jobs.rs:57-59`, `load.rs:72-77,174` | rebuild/load call `Document::parse`, `base.doc.clone()`, `edit::apply_text`, `DocModel::build` | medium |
| C-14 | `crates/markise-engine/src/edit.rs:15-25` | `Patch` lives in L4 and applies to `markise_md::Document` | low |
| C-15 | `crates/markise-ui-egui/src/app.rs:37,474`; `source.rs:6` | `ParseOptions::default()` hard-coded; source mode reads `markise_md::Document` / `LineEnding` | low |
| C-16 | `crates/markise-export/Cargo.toml` | HTML export renders the Markdown tree directly | acceptable (exporters are per format; Q6) |
| C-17 | `crates/markise-theme/src/lib.rs:240-274` | slots named after Markdown/DOM concepts (`headings`, `quote_bar`, `alert[5]` in `AlertKind` order) | acceptable (semantic slots; index by `CalloutKind`) |
| C-18 | `model.rs:103-111,600` | `Segment::Lines` holds **absolute** spans and derives `Hash`; `Flattener::push` hashes the whole segment, so a segmented block's item hash changes when text above it changes | medium (cache reuse, §7.3) |

Neutral already: `cache.rs` (`CacheKey` holds hashes and numbers), `fenwick.rs`, `view.rs` (only
through `DocModel`/`Item`), `crates/markise-blocks`, and ADR 0002's document lifecycle in `app.rs`.

**Conclusion.** The *shape* of the view model is close to neutral, but the code that **builds** it
and the code that **renders inline content** parse Markdown inside L2. Both move behind the seam
before the editor adds source maps and reveal on top of them.

### 1.2 The seam (no dependency cycles)

Layering rule: `markise-doc` (L1) contains only data types and traits that mention data types.
Nothing in it references `EditorSession`, `CommandSpec`, egui or any L2+ type.

```text
markise-doc (L1)
├── text.rs      Span, LineEnding, Patch { range, insert, verbatim }, PatchSet (sorted,
│                non-overlapping, char-boundary validated), apply helpers
├── pos.rs       DocPos { offset, affinity }, Selection { anchor, focus, goal_x }, Mark sets,
│                transform(&mut marks, &PatchSet)
├── view.rs      ItemKind, Container, CalloutKind, Marker, Gap, Segment (moved from layout),
│                LeafView, TableView, CellAnchor, OutlineEntry
├── rich.rs      RichInline (neutral inline tree, §1.3), SourceMap pieces, DepSet (§7.4)
├── hints.rs     LiteralHint (editor-only interpretation state, §2.6)
├── command.rs   CommandDescriptor, PlannerId, ParamSchema, Scope, Effect, DefaultKey (data)
├── plan.rs      Plan { steps: Vec<PatchSet>, hints: HintDelta, selection_after }, PlanRequest
└── format.rs    trait DocumentFormat, trait FormatDoc, trait EditBehavior, EditSummary
```

```rust
/// Factory and metadata for one file type ("markdown"; later "org").
pub trait DocumentFormat: Send + Sync {
    fn id(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn parse(&self, text: String, opts: &FormatOptions) -> Box<dyn FormatDoc>;
    /// Commands this format adds (pure data; the engine makes them executable).
    fn commands(&self) -> &'static [CommandDescriptor] { &[] }
}

/// A parsed document. Planning lives on the document itself (supertrait), so a planner
/// always has its concrete type: no `Any` downcasts.
pub trait FormatDoc: EditBehavior + Send {
    fn text(&self) -> &str;
    fn line_ending(&self) -> LineEnding;
    fn apply(&mut self, steps: &PatchSet) -> EditSummary;        // one text change, one reparse
    fn set_hints(&mut self, hints: &HintSet) -> EditSummary;     // re-projects hinted lines (§2.6)
    fn flatten(&self, top_level: Range<usize>, out: &mut ItemSink);
    fn flatten_source(&self, lines: Range<usize>, out: &mut ItemSink); // source mode (§5.5)
    fn leaf(&self, at: usize) -> Option<LeafView<'_>>;
    fn inlines(&self, leaf: &LeafView, ctx: &DocContext) -> RichInline;  // records DepSet
    fn table(&self, leaf: &LeafView) -> Option<TableView>;
    fn context(&self) -> DocContext;
    fn outline(&self, leaf: &LeafView, ctx: &DocContext) -> Option<HeadingInfo>; // text + base slug
    fn snapshot(&self) -> Arc<str>;
    fn clone_box(&self) -> Box<dyn FormatDoc>;
}

/// Format-specific editing. All methods are planners: `&self` + request → `Plan` (or a typed
/// error); nothing mutates.
pub trait EditBehavior {
    fn plan(&self, id: PlannerId, req: &PlanRequest) -> Result<Plan, PlanError>;
    fn on_text(&self, req: &PlanRequest, typed: &str) -> TypingPlan;           // §5.1
    fn line_signature(&self, line_start: usize) -> u64;                       // §7.2 fast path
    fn opener_unchanged_proof(&self, opener: &FailedOpener, changed_lines: &[Range<usize>]) -> bool; // §7.2
    fn reveal(&self, rich: &RichInline, leaf: &LeafView, caret: usize, prefs: &EditPrefs) -> Reveal;
    fn materialize_hint(&self, hint: &LiteralHint) -> Option<PatchSet>;       // §2.6
    fn resolve_virtual(&self, slot: &VirtualSlot, e: &EditSummary) -> (DocPos, VirtualSlot); // §2.2
    fn clip(&self, sel: Range<usize>) -> ClipFragment;                         // §6.1
    fn continuation_prefix(&self, at: usize) -> String;
    fn pairs(&self, ctx: PairContext) -> &[(char, char)];
}
```

`PlanRequest` carries the selection, the editing context (block kind at the caret, scope), typed
params, the write preferences and a read-only view of the current hints. Generic planners
(`edit.newline`, `edit.deleteBackward`, `format.strong`, `block.heading2`, `table.addRowBelow`, …)
have well-known `PlannerId`s defined in `markise-doc`; a format answers `PlanError::Unsupported` for
those it does not implement (the engine then disables the command). Format-specific commands use
format-scoped ids (`markdown.insertLinkRef`).

**Engine assembly (L4).** `markise-engine` holds `CommandSpec { desc: &CommandDescriptor, run }`
where `run` is one of a few engine-defined adapters: `RunPlanner(PlannerId)` → `session.apply_plan(
doc.plan(id, &req)?)`; `RunEngine(fn(&mut EditorSession, &Params) -> CmdResult)` for engine-only
commands (undo, save, find, caret moves that need layout, view toggles). Format descriptors are
merged into the registry at document open; the dependency direction is L4 → L1 only.

**Second format.** `crates/markise-testfmt` (classified *Testkit* by the `xtask layers` check,
research note §1): a line-based toy format (`= Heading`, `* item`, paragraphs, `*strong*`, one
immediate input rule `* ` → item, one Enter rule `= x` → heading, a lossless serializer).
`markise-layout` and `markise-engine` tests run against it in addition to Markdown, proving that
layout, caret, commands, undo and hints work without `markise-md`.

### 1.3 Neutral inline view (`RichInline`)

Each node: `kind`, `source: Range<u32>` (relative to the leaf's first content byte), `content`
range and `markers: SmallVec<[Range<u32>; 2]>` (e.g. `**`…`**`; for a link `[` and `](url "t")`).
Kinds: `Text, Emph, Strong, Underline, Strike, Highlight, Sub, Sup, Code, Link{target, title},
Image{src, alt, title}, Math{tex}, FootnoteRef{label}, RawInline{lang}, Break{hard},
Atom{display}` (atomic substitution: entity, escape, emoji, render-time smart punctuation — one
caret stop covering its whole source range). Container prefixes between content lines are not part
of any node; `LeafView` carries the content-line table. Building a `RichInline` also yields its
`DepSet` (§7.4). `markise-md` builds it from its `Inline` tree (`inline.rs:37-95`); Org maps
naturally (`*b*`, `/i/`, `=v=`, `~c~`, `[[target][desc]]`).

### 1.4 Neutral block view

Keep `ItemKind`/`Container` with: `AlertKind` → `CalloutKind { Note, Tip, Important, Warning,
Caution, Other(Box<str>) }`; `FrontMatter` → `Metadata`; `Html` → `RawBlock { lang }`. `Item.path`
(C-12) is dropped (leaf by offset). `Segment` hashes become offset-independent (C-18, §7.3). Org
items to check in P2: heading badges (TODO/priority/tags), drawers → `Hidden`/`Metadata`, tables
without alignment row.

### 1.5 Changes required before editor work (= milestone M2a)

1. Create `markise-doc`; move `Span`, `LineEnding` (re-exported by `markise-md`), `Patch` (from
   `markise-engine/src/edit.rs`), `ItemKind`/`Container`/`Segment`.
2. Move flattening and outline building into `markise-md` (`impl FormatDoc for Document`);
   `DocModel` keeps items, outline, context and dependency indexes, **not** the document.
3. `InlineBuilder` and `layout_table` consume `RichInline` / `TableView`.
4. `markise-layout` drops `markise-md` (dev-dependency for tests only); add `markise-testfmt`.
5. Engine/UI select the format through a `FormatRegistry`; `ParseOptions` become Markdown's
   `FormatOptions` filled from settings (C-15).
6. `xtask layers` (research note §1) with rules: `markise-doc` depends on no workspace crate;
   `markise-layout`, `markise-engine` (except the registry module's wiring in `apps/*`) and
   `markise-ui-egui` do not depend on `markise-md`.
7. Normalise segment hashes (C-18).

Acceptance: viewer kittest screenshots and `cargo bench -p markise-ui-egui --bench viewer`
unchanged (±10 %); `cargo tree -p markise-layout` shows no `markise-md`; layout tests pass on both
formats; inserting text above an unchanged segmented block leaves its item hash and cached layout
reusable (full and incremental model agree).

---

## 2. Editing model

### 2.1 Text is the source of truth; every edit is a patch

- `FormatDoc::apply(&PatchSet) -> EditSummary` replaces one-range `Document::edit`
  (`markise-md/src/lib.rs:175`) as the only text mutation. Patches are in pre-edit coordinates,
  sorted, non-overlapping; one new text, one incremental reparse over the union of patched ranges
  (subject to §7.2), one span shift. `EditSummary { replaced_top, new_top, reparsed, delta, deps }`.
- Batch apply exists because `ops::insert_list_item` renumbers with one `doc.edit` per marker
  (`ops.rs:158`): one reparse and one O(n) `replace_range` per item. Replace All has the same shape.
- **Planners.** `ops::*` (`toggle_task`, `insert_list_item`, `insert_block_after`,
  `set_code_content`, `set_front_matter`, `set_table_cell`, `replace_block`) become
  `plan_*(&Document, …) -> Result<Plan, PlanError>`. A `Plan` may contain several **ordered**
  steps (`Vec<PatchSet>`, each in the coordinates produced by the previous step) when a later
  step depends on an earlier one; most plans have one step. Generators in `write.rs` stay (R1.3).
- **Canonical patches.** Before a step is applied and recorded, newly typed line breaks are
  converted to the document's line ending and continuation prefixes are added; the recorded forward
  patch is the resulting exact byte string marked `verbatim`. Redo therefore reproduces the bytes
  exactly, and mixed-ending files (C23) survive undo/redo.

### 2.2 Positions, selections, marks

- `DocPos { offset, affinity, virt: u16 }` — byte offset into the source, on a `char` boundary,
  normalised to a caret stop; `offset` + `affinity` are ADR 0005 §4.3's canonical position
  *[0005]*. `virt` is a **format-neutral virtual-position discriminator**: 0 for every position that
  exists in the source; *k* > 0 for the *k*-th editor-only slot the format defines at that same
  offset (missing table cells, the virtual empty paragraph). Positions order by `(offset, virt)`.
  No item indices, block paths or rendered offsets in carets. (Proposed amendment to ADR 0005 §4.3:
  `DocPos` gains `virt`; Q10.)
- **Virtual positions through edits and history.** `virt` is a derived value: it depends on the
  structure that defines the slot (for a missing table cell: the row's real-cell count, the table's
  column count, the row and the table existing). Every live mark with `virt > 0` therefore also
  stores its **intended slot** in structure terms — `VirtualSlot::TableCell { table_start: mark,
  row: u32, col: u32 }` (`table_start` is itself a live position; `row`/`col` are the intended
  logical cell) or `VirtualSlot::EmptyParagraph { gap: mark }`.
  - After every applied patch set, a virtual mark is **re-resolved** by the format
    (`EditBehavior::resolve_virtual(&slot, &EditSummary) -> (DocPos, VirtualSlot)`) whenever its
    defining structure may have changed: the patch touches its offset, **or** the edit's replaced
    range intersects the defining row or table (any cell inserted/removed in that row, a column
    added/removed, the header or delimiter row edited, the row or table removed or reparsed). The
    format recomputes `virt` from the intended `(row, col)` against the new structure:
    - the intended column now has a real cell → `DocPos` at that cell's content (`virt = 0`);
    - it is still missing → `virt = col − r′ + 1` with the new real-cell count `r′`, offset = the
      new row end;
    - the column no longer exists (table narrowed) → the last column of the row (real or virtual);
    - the row was removed → same column in the nearest following row, else the preceding row;
    - the table was removed → collapse to `virt = 0` at the position where the table started.
  - Marks whose defining structure is untouched only shift (their `virt` stays valid by
    construction). The editing caret after a materialising edit is set by the plan. Historical
    selections keep `(offset, virt)` and their slot verbatim — they are restored in their own state
    (§4.1).
- `Selection { anchor, focus, goal_x }`; no multi-range selections in M2 (C8).
- **Live marks** — the current selection, the virtual block (below), literal hints (§2.6), find
  results (§6.2), the composition anchor (§6.3), the last-edit position — are transformed through
  every applied patch set: before a patch unchanged; after it shifted by the delta; inside a replaced
  range collapsed to the patch start (Upstream) or to the end of the insert (Downstream). The
  command sets the editing caret explicitly (`Plan::selection_after`).
- **Historical selections are never transformed.** `sel_before`/`sel_after` of a transaction are
  valid in that transaction's before/after states; undo/redo return the document to exactly those
  states, so they are restored verbatim (§4.1). Test: select text, delete a range containing the
  selection's interior in a later transaction, undo both → both selections exact.
- **Caret stops (live mode).** Not stops: container prefixes, block markers, atomic substitutions,
  collapsed inline markers; they normalise to the nearest stop with the affinity of the side they
  came from. Revealed markers are stops (spec 02 §3.2). With "display source for simple blocks on
  focus" (spec 02 §1.5) the focused heading's `#` markers become stops.
- **Explicit anchors** where no rendered text exists:
  - *Empty fragments* (empty heading `#`, empty item `- `, empty code block, empty cell, empty
    front matter): the fragment carries `empty_anchor: offset` (item-relative) and a caret rect
    from its style's line metrics — not a snap to a neighbour.
  - *Virtual empty paragraph*: Markdown cannot represent an empty paragraph (spec 01 R3.1.1; R6.3
    only says Enter creates one). Proposal **(assumption, Q1)**: Enter at the end of a block
    creates an editor-only `VirtualBlock` anchored at a source gap (a live mark); the first typed
    character materialises it with the R6.2 separator in the same patch; moving away drops it with
    no source change. Byte-identical round-trip is unaffected.
    The virtual paragraph's caret is `DocPos { offset: gap, virt: 1 }`.
  - *Table cells*: `TableView.cells[row][col]` is `CellAnchor::Span(range)` (possibly empty, e.g.
    `| |`) or `CellAnchor::Missing { row_end, ordinal }` for cells GFM pads because the row is
    short (C31). A row with *r* real cells in an *n*-column table has missing columns *r…n−1*;
    column *c* is the caret position `DocPos { offset: row_end, virt: c − r + 1 }`, so several
    missing cells of one row are distinct, ordered stops (left/right and Tab move between them;
    selection endpoints can sit in them). Typing into missing column *c* is planned as one patch at
    `row_end` that materialises the empty cells *r…c−1* and cell *c* with the typed text, in the
    row's existing pipe/spacing style (outer pipes or not); `selection_after` is inside the new cell
    *c* with `virt = 0`.
  - Test: 4-column header, body row `| a |` (one real cell); place the caret in column 4
    (`virt = 3`), type `x` → row `| a |  |  | x |` (style per A8 rules), caret after `x`; Undo →
    original bytes and the caret back at `{row_end, virt: 3}`; Redo → same as after typing; Tab/
    Shift+Tab across the three missing cells visits `virt` 1, 2, 3 in order.
  - Structure-change tests (caret parked in missing column 4 of `| a |`, `virt = 3`, while the
    edit is made elsewhere, e.g. by a find/replace or a second editing step): insert ` b |` after
    `a` (row now `| a | b |`) → caret re-resolves to column 4 with `virt = 2`; insert two more
    cells → column 4 is real, caret inside it with `virt = 0`; remove `b` again → `virt = 2`;
    delete the column 3 via `table.deleteColumn` (table narrowed to 3 columns) → caret in column 3
    (`virt = 1`); delete the row → same column in the next row; delete the table → `virt = 0` at
    the former table start. Each step also checked with incremental == full model and Undo
    restoring the exact earlier caret.

### 2.3 Rendered positions ↔ source offsets

- Each text fragment (ADR 0005 `FragmentId` *[0005]*; on epaint one `Prim::Text`) carries a
  **source map**: sorted pieces `(rendered, source, kind: Visible | Marker | Atom | Prefix |
  Synthetic | Suppressed)` with source ranges **relative to the item start**. Item-relative maps
  plus offset-independent item hashes (which requires the C-18 fix: today `Segment::Lines` hashes
  absolute spans) keep cached layouts valid when text above changes. ADR 0005 `FragmentId.block`
  should be item-relative too **(proposed amendment to 0005 §4.3, Q10)**.
- `Visible` maps byte for byte; `Marker` exists only when revealed; `Atom` maps a rendered cluster
  to its source range; `Prefix` is never rendered; `Synthetic` (bullets, alert titles, TOC entries,
  padding, IME preedit) has no source and snaps to the adjacent stop or the fragment's
  `empty_anchor`; `Suppressed` (selection replaced by an IME composition, §6.3) is laid out as
  hidden but keeps its mapping.
- Rendered → source: hit test (epaint `cursor_from_pos` now; parley `Cursor` later *[0005]*) →
  piece → item-relative offset + item start. A click on collapsed styled text maps to the content
  side of the markers. Source → rendered: binary search; positions inside hidden markers map to the
  content boundary (opening → content start, Downstream; closing → content end, Upstream).
- **Long table cells.** The viewer caps cells at 2 KiB (`cap_cell`, C-9). The focused cell is laid
  out **uncapped** as its own fragment (row height follows); unfocused cells keep the cap. Caret
  navigation past 2 KiB uses the focused cell's uncapped layout.
- Pre-T7 caret stepping (epaint): grapheme boundaries on the rendered text with
  `unicode-segmentation` (MIT/Apache), mapped through the source map; epaint `char` cursors only for
  point→index. Bidi and complex scripts wait for T7 *[0005]*.

### 2.4 Marker reveal (spec 02 §1, §3.2)

- `EditBehavior::reveal` returns the reveal set: inline nodes whose source range contains or touches
  the caret (all nesting levels), the block-marker flag (spec 02 §1.5) and "image source above the
  image" (spec 02 §9). With a non-empty selection only the focus end reveals **(assumption)**.
- Part of the focused item's cache key (`CacheKey.extra`, `cache.rs`): caret moves with an
  unchanged reveal set cost no layout; entering a span costs one layout of the focused item (one
  segment for segmented blocks); leaving restores the cached collapsed layout; other items never
  re-layout.
- Reveal height changes update that item's height; the scroll anchor is pinned to the **caret
  line**. Reveal is frozen during IME composition and pointer-drag selection.
- Segmented paragraphs: spans crossing segment borders are not matched (ADR 0002 approximation).

### 2.5 Where state lives

```text
markise-engine::editor::EditorSession          (L4; UI-free, headless-testable)
  format: Arc<dyn DocumentFormat>, doc: Box<dyn FormatDoc>, model: DocModel
  sel: Selection, marks: LiveMarks (virtual block, hints, find results, composition anchor)
  history: History (§4), composition: Option<Composition> (§6.3 [0005] §4.4)
  rev: TextRev { counter, state }, jobs: JobGenerations (monotonic), prefs: EditPrefs
markise-layout::View                           (L2) heights, anchor, cache; CaretGeometry queries
markise-ui-egui::DocEditor                     (L6) egui events → EditorInput; paints caret/selection
```

`DocModel` does not own the document (C-2); the session owns the mutable `FormatDoc` on the UI
thread. Workers receive `Arc<str>` snapshots taken **at job start** (save, autosave, search
restart), never as a side effect of a keystroke (§6.2, §7.5). Vertical movement and visual line
start/end use `CaretGeometry` from `View`; headless sessions implement it with the CPU test font
system (`markise_layout::testing::fonts`).

### 2.6 Literal-line hints: input-rule undo and "paragraph until Enter"

**Problem.** The projection of plain bytes cannot express two spec behaviours: (a) after typing
`- ` the bytes already parse as a list, so "Undo restores the literal `- ` paragraph text" (spec 02
§11) is not a text change at all; (b) S-2.1 requires `## Plan` to stay a visible paragraph until
Enter, but the parser recognises an ATX heading (even `#Title` in non-strict mode, R3.2.1) as soon
as it is typed; typing ```` ``` ```` would open an unclosed fence that swallows the rest of the
container (and forces a huge reparse per keystroke).

**Mechanism.** A `LiteralHint { line: live mark at a line start, kind: Literal, origin:
EnterPending | RuleTrigger }` tells the format to parse that physical line as **paragraph text**
(markers literal and visible) regardless of the block syntax it starts with. Hints are few (normally
zero or one), live in the session's marks, are passed to the format (`FormatDoc::set_hints`) and
consulted by the block parser at line starts; changing the hint set re-parses only the hinted
line's region (an edit with no text change). The hint set is part of the item hash of the hinted
item.

- **Enter-only syntaxes** (spec 02 §2.2: ATX/setext headings, fences, `$$`, `[toc]`, thematic
  breaks, pipe header rows, footnote/link definitions, front matter, plus HTML-block openers
  **(assumption)**): when typing makes a paragraph line start one of them, the typing pipeline adds
  an `EnterPending` hint in the same step. Enter runs the §2.2 rule: the planner removes the hint
  and applies any generator patches (closing fence, delimiter row) as one `Structural` transaction.
- **Immediate rules** (spec 02 §2.1: quote, bullets, tasks, ordered, callout) — one consistent
  history boundary. When a keystroke completes a trigger, the input is handled as **two
  transactions applied back to back before any frame is drawn**:
  1. the typing transaction `T1` (state `S0 → S1`) whose last step inserts the keystroke **and adds
     a literal hint** on that line — so `S1` = bytes `- ` + hint = a paragraph showing `- `;
  2. the conversion transaction `T2` (`kind: Auto`, `S1 → S2`) whose only step **removes the hint**
     (Markdown needs no text patch: the bytes already parse as a list; a format may add patches).

  The model/layout update of `S1` may be coalesced away because no frame separates `T1` and `T2`.
  Every state is recorded exactly: Undo of `T2` restores `S1` (bytes `- `, hint present, paragraph,
  `sel_before` of `T2`); Undo of `T1` restores `S0` (no space, no hint); Redo walks forward to `S1`
  then `S2`. Undo never synthesises state; it only inverts recorded steps (§4.1). The same pattern
  covers callout conversion (`[!NOTE]`) and the auto-pair cleanup of spec 02 §2.1.
- **Enter-only syntaxes, same contract.** The typing step that makes the line Enter-only syntax adds
  the `EnterPending` hint inside the typing transaction; the Enter transaction removes it together
  with its generator patches. Undo of Enter restores the hinted paragraph `## Plan`; undo of the
  typing removes text and hint together.
- **Dirty identity.** `StateId` identifies (bytes, hint set) (§4.3). `S1` and `S2` have equal bytes
  but different ids, so: type `- `, save (clean at `S2`), Undo → `S1` is dirty (saving it would
  materialise `\- `), Redo → `S2` clean again. A save snapshot never contains hints, so
  `saved_state` is always a hint-free state.
- **Expiry and materialisation.** A hint expires when the caret leaves the hinted block, before a
  save snapshot, on a mode switch, and on document close. On expiry the format's
  `materialize_hint` returns a minimal escape patch so the bytes parse as what the user saw
  (Markdown: `\#`, `\-`, `\>`, ``\` ``, `\~`, `\$`, `1\.`, `\|`, `\[`, `\<` — all in the R4.1.1
  escape set). The materialisation patch together with the hint removal is one step that joins the
  open transaction if one is open (it closes at the same moment), otherwise it is its own `Auto`
  transaction "Escape Literal Syntax" — so after Undo-to-`S1` and leaving the block, a further Undo
  returns exactly to `S1`. If a format
  cannot escape a construct, the hint is dropped and the block takes its parsed form. Whether
  leaving a not-yet-Entered `## Plan` should escape it or let it become a heading is an owner
  decision (**Q11**; proposal: escape, i.e. "what you saw is what is saved").
- **Undo/redo/rollback.** Hint additions/removals are recorded in transaction steps (§4.1) and
  journaled; undoing a transaction restores the exact hint set of its before state.
- **Reload** drops all hints (disk bytes are authoritative); **save** materialises them first, so a
  saved file never depends on editor-only state.
- Tests assert **bytes, projected block type (`debug_tree`) and selection** for: typing `## Plan`
  (paragraph until Enter, heading after; Undo of Enter → hinted paragraph; Undo again → text gone);
  `- ` + Undo (paragraph `- `, bytes `- `) + Redo (list); repeated Undo/Redo/Undo/Redo cycles over
  `S0 ⇄ S1 ⇄ S2` with `StateId` and dirty flag checked at each state; save at `S2`, Undo, Redo →
  clean; `- ` + Undo + caret away (bytes `\- `, own transaction) + Undo (back to `S1`); typing
  ```` ``` ```` in the middle of a document (no swallowing, no huge reparse); hints across rollback,
  save and reload.

---

## 3. Command architecture

### 3.1 Descriptors and executable commands (adapted from WordCraft, research note §2)

WordCraft routes menus, shortcuts, palette, CLI scripts and MCP through one `CommandSpec { id,
label, location, shortcut, params, mutates, enabled, run }` registry (research note §2,
`wordcraft/crates/engine/src/lib.rs`, rev 6a2b520f). We adapt the design, not the code (nothing to
add to `NOTICE`), split into a data part (L1) and an executable part (L4):

```rust
// markise-doc (L1): pure data
pub struct CommandDescriptor {
    pub id: &'static str,              // "format.strong", "table.addRowBelow", "markdown.insertLinkRef"
    pub label: &'static str,           // menu title; stable (keyBinding, macOS App Shortcuts)
    pub menu: Option<MenuPath>,
    pub keys: &'static [DefaultKey],   // per-platform defaults (§3.2)
    pub scope: Scope,                  // Global | Editor | Table | Code | Source | Find
    pub effect: Effect,                // Pure | View | Mutates { group: UndoGroup }
    pub params: ParamSchema,           // typed; JSON schema generated for MCP/CLI
    pub planner: Option<PlannerId>,    // format-implemented commands
}
// markise-engine (L4): executable
pub struct CommandSpec {
    pub desc: &'static CommandDescriptor,
    pub enabled: fn(&Ctx) -> Result<(), Disabled>,   // generic: scope/read-only/selection checks
    pub run: Run,                                    // RunPlanner(PlannerId) | RunEngine(fn …)
}
```

For planner commands `enabled` additionally asks the document (`plan` in dry-run mode returning
`Unsupported`/`NotApplicable` cheaply). Differences from WordCraft, each deliberate: typed params
with a generated schema instead of free-form `serde_json::Value` (research note §2's weakness);
scopes for context-dependent keys; the **session**, not the command, records undo, opens/closes
groups, bumps revisions and defers external effects (§4.4).

`Registry::describe()` → MCP `list_commands` (M3) and `markise-cli commands`; `markise-cli run
<file> --script job.json` runs command lists headlessly (research note §14) and drives scenario
tests (§8.3).

### 3.2 Default shortcuts and key routing

`DefaultKey { mac: Option<&str>, other: Option<&str> }` in canonical chord text (`"Cmd+B"`,
``"Ctrl+Shift+`"``). Rule from spec 02 §15.2: Windows/Linux default = macOS with Cmd→Ctrl, Opt→Alt,
unless §15.2 lists an explicit binding or the mapped chord collides (then `other: None`, Q4).
Editing subset (full lists in spec 02 §15):

| Command id | Label | macOS | Windows / Linux | Scope |
|---|---|---|---|---|
| `edit.undo` / `edit.redo` | Undo / Redo | Cmd+Z / Shift+Cmd+Z | Ctrl+Z / Ctrl+Shift+Z, Ctrl+Y **(assumption: platform convention)** | Global |
| `edit.copyMarkdown` | Copy as Markdown | Shift+Cmd+C | Ctrl+Shift+C | Editor |
| `edit.pastePlain` | Paste as Plain Text | Shift+Cmd+V | Ctrl+Shift+V | Editor |
| `edit.moveBlockUp/Down` | Move Row Up/Down | Opt+↑/↓ | Alt+↑/↓ | Editor, Table (A4) |
| `select.word` / `select.line` / `select.scope` / `select.block` | Select Word / Line / Styled Scope / Paragraph | Cmd+D / Cmd+L / Cmd+E / Opt+Cmd+P | Ctrl+D / Ctrl+L / Ctrl+E / Ctrl+Alt+P | Editor (L, E in table: row/cell) |
| `delete.word` / `delete.line` / `delete.scope` / `delete.block` | Delete Word / Line / Styled Scope / Paragraph | Shift+Cmd+D / Shift+Opt+Cmd+L / Shift+Opt+Cmd+E / Shift+Opt+Cmd+P | Ctrl+Shift+D / Ctrl+Alt+Shift+L / Ctrl+Alt+Shift+E / Ctrl+Alt+Shift+P | Editor |
| `find.open` / `find.next` / `find.prev` / `find.replace` | Find… / Find Next / Find Previous / Find and Replace… | Cmd+F / Cmd+G / Shift+Cmd+G / Opt+Cmd+F | Ctrl+F / F3 / Shift+F3 / Ctrl+H | Global |
| `nav.top` / `nav.bottom` / `nav.selection` | Jump to Top / Bottom / Selection | Cmd+↑ / Cmd+↓ / Cmd+J | Ctrl+Home / Ctrl+End / Ctrl+J | Editor |
| `block.paragraph`, `block.heading1…6` | Paragraph, Heading 1–6 | Cmd+0, Cmd+1…6 | Ctrl+0, Ctrl+1…6 | Editor |
| `block.headingUp` / `block.headingDown` | Increase / Decrease Heading Level | Cmd+= / Cmd+- | Ctrl+= / Ctrl+- | Editor |
| `insert.table` / `block.codeFence` / `block.mathBlock` / `block.quote` | Insert Table / Code Fences / Math Block / Quote | Opt+Cmd+T / Opt+Cmd+C / Opt+Cmd+B / Opt+Cmd+Q | Ctrl+T / Ctrl+Shift+K / Ctrl+Shift+M / Ctrl+Shift+Q | Editor |
| `block.orderedList` / `block.bulletList` / `block.taskList` | Ordered / Unordered / Task List | Opt+Cmd+O / Opt+Cmd+U / Opt+Cmd+X | Ctrl+Shift+[ / Ctrl+Shift+] / Ctrl+Alt+X | Editor |
| `block.toggleTask` | Toggle Task Status | Ctrl+X | — (collides with Cut; Q4) | Editor |
| `block.indent` / `block.outdent` | Indent / Outdent | Cmd+] / Cmd+[ | Ctrl+] / Ctrl+[ (direction to verify, spec 02 §15.2) | Editor |
| `markdown.insertLinkRef` / `markdown.insertFootnote` / `insert.rule` | Link Reference / Footnotes / Horizontal Line | Opt+Cmd+L / Opt+Cmd+R / Opt+Cmd+- | Ctrl+Alt+L / Ctrl+Alt+R / Ctrl+Alt+- | Editor |
| `format.strong` / `format.emph` / `format.underline` | Strong / Emphasis / Underline | Cmd+B / Cmd+I / Cmd+U | Ctrl+B / Ctrl+I / Ctrl+U | Editor |
| `format.code` / `format.strike` / `format.highlight` | Code / Strike / Highlight | Ctrl+\` / Ctrl+Shift+\` / Shift+Cmd+H | Ctrl+Shift+\` / Alt+Shift+5 / Ctrl+Shift+H | Editor |
| `format.inlineMath` / `format.comment` | Inline Math / Comment | Ctrl+M / Ctrl+- | — / — (Q4) | Editor |
| `format.link` / `format.image` / `format.clear` | Hyperlink / Image / Clear Format | Cmd+K / Ctrl+Cmd+I / Cmd+\\ | Ctrl+K / Ctrl+Shift+I / Ctrl+\\ | Editor |
| `table.addRowBelow` / `table.deleteRow` | Add Row Below / Delete Row | Cmd+Return / Shift+Cmd+Backspace | Ctrl+Enter / Ctrl+Shift+Backspace | Table |
| `table.moveRowUp/Down`, `table.moveColLeft/Right` | Move Table Row / Column | Ctrl+Cmd+↑/↓/←/→ | — (Q4) | Table |
| `code.exitBelow` | (Cmd+Enter in code) | Cmd+Return | Ctrl+Enter | Code |
| `edit.openLine` | (Open Line) | Ctrl+O | — (macOS only) | Editor |
| `view.sourceMode` / `view.focus` / `view.typewriter` | Source Code Mode / Focus Mode / Typewriter Mode | Cmd+/ / F8 / F9 | Ctrl+/ / F8 / F9 | Global |

Context-dependent keys (Enter, Shift+Enter, Tab, Shift+Tab, Backspace, Delete, arrows, Esc) are
commands (`edit.newline`, `edit.softBreak`, `edit.indentOrNextCell`, `edit.deleteBackward`,
`caret.left`, …). Routing: chord → candidate ids ordered by scope specificity (Table/Code before
Editor before Global); the first candidate whose `enabled` passes runs (Cmd+Return:
`table.addRowBelow` in a table, `code.exitBelow` in code). Printable text is `edit.insertText
{ text }` through the typing pipeline (§5.1), so MCP and scripts can type.

egui specifics **(assumptions to verify in M2b)**: the document surface sets egui's focus-lock event
filter for Tab, arrows and Escape; `Modifiers::COMMAND` maps Cmd/Ctrl; letter shortcuts on non-Latin
layouts need egui patches E7/E19 (research note §21) — a recorded limitation until then.

### 3.3 Keymap customisation (`keyBinding`)

- Spec 05 §4.7 / B2: `conf.user.json` `keyBinding` maps **menu title → accelerator** (Windows/Linux,
  P1). Keys may be a command `label` (interop) or `id`; value an accelerator or `""` to unbind.
- Unknown labels, unparsable accelerators and conflicts (same chord, overlapping scope) are
  reported, never fatal; the later entry wins. Reserved OS chords cannot be rebound **(design
  choice)**. Applied at start and on config reload.
- macOS: stable menu titles serve OS "App Shortcuts" once a native menu exists (spec 02 §15.1); the
  in-window egui menu cannot be rebound by the OS (limitation until a native menu, research note §19).
- `craft-keymap` (research note §19a) stays an M3 spike; M2's model (chords, scopes, overrides,
  conflicts) is compatible.

---

## 4. Undo / redo

### 4.1 Representation

```rust
struct Transaction {
    label: &'static str,              // "Typing", "Strong", "Paste", "Replace All", "Source Edit"
    steps: Vec<Step>,                 // applied in order; undo walks them in reverse
    sel_before: Selection,            // immutable, valid in state_before
    sel_after: Selection,             // immutable, valid in state_after
    kind: TxKind,                     // Typing{leaf_start, last_at, dir} | Auto | Structural | Bulk
    state_before: StateId, state_after: StateId,
}
struct Step {
    forward: PatchSet,                // canonical: exact bytes inserted (verbatim), coords = state before this step
    removed: Vec<Arc<str>>,           // verbatim bytes each patch replaced
    hints: HintDelta,                 // literal-line hints added/removed (§2.6)
}
```

- **Undo** applies, for each step in reverse, the inverse patch set (insert ranges ← verbatim
  removed bytes, in that step's after-coordinates) and the inverse hint delta, then restores
  `sel_before` verbatim. **Redo** re-applies steps forward and restores `sel_after`. Both go through
  `FormatDoc::apply`/`set_hints`, so reparse, model update and live-mark transforms happen as for any
  edit. A new edit clears the redo stack.
- Steps are never merged across a dependency: a step that edits text inserted by an earlier step of
  the same transaction stays a separate step. **Typing coalescing** is the one composition: a
  keystroke patch adjacent to the last step's last insert (append) or removal (prepend, Backspace;
  append, Delete) extends that patch in place; anything else appends a new step.
- No document snapshots are stored (research note §3).

### 4.2 Grouping rules

A typing group coalesces consecutive `edit.insertText` / `edit.deleteBackward` /
`edit.deleteForward` keystrokes into one transaction. A new group starts on:

| Trigger | Source |
|---|---|
| pause > 1 s between keystrokes | C13 |
| caret jump: any selection change not produced by the typing itself | C13, spec 02 §11 |
| typing moves into a different leaf block | spec 02 §11 "within one block" |
| switching between inserting and deleting | (design choice, refines A3) |
| typing whitespace (space, tab, soft line break) when the group's previously typed character is not whitespace — the whitespace opens the new group; consecutive whitespace stays together; an IME commit is never split (it stays one transaction even with internal whitespace) | C56 (amendment 2026-10-27, A3 "word/run groups") |
| Enter / structural split | spec 02 §11 |
| block-type change (input rule, menu, Backspace conversion) | C13, spec 02 §11 |
| formatting command, paste, drop | C13 |
| IME composition commit: boundary before and after | spec 02 §11, ADR 0005 §4.4 *[0005]* |
| undo, redo, save (a save closes the open group so the saved state is a transaction boundary) | (design choice) |

**Amendment (owner, 2026-10-27; `docs/research/marktext-comparison.md` R15 → C56):** the
word-boundary trigger above was added so that typing `foo bar` undoes as `foo` + ` bar`, as A3's
"word/run groups" asks; the pause rule (C13) alone grouped whole sentences. Tests: the C56
scenarios in `99-open-questions.md` (injected clock, no pauses). A whitespace keystroke that
completes an immediate input rule (`- `) therefore opens a new typing group; the two-transaction
pattern of §2.6 is unchanged (Undo of the conversion → hinted `- `; Undo again → `-`).

Special transactions:

- **Input-rule conversions are their own step** (spec 02 §11; WordCraft "auto-rewrite = separate
  step", research note §3). Immediate rules: the typing transaction ends with the triggering
  keystroke **and a literal hint**, then the `Auto` conversion transaction removes the hint (plus
  any format patches), both before the next frame (§2.6); Undo of the conversion restores the
  recorded hinted state exactly. Enter rules: one `Structural` transaction (hint removal + generator
  patches).
- **Smart punctuation** (convert on input, spec 02 §13): a typed `“` is part of the typing group; a
  rewrite of previously typed characters (`--` → `–`) is an `Auto` step **(design choice, Q9)**.
- **Auto-pairs** (spec 02 §4) are part of the typing group.
- **Replace All**: one transaction (spec 02 §11); the find panel's Undo targets its id.
- **Joins** (WordCraft `join_next_undo` / `merge_undo_since`, research note §3): multi-frame drags
  (row/column/block reorder) and the table realign at cell exit (§5.3) join the open transaction.
- **Hint materialisation** at expiry (§2.6) joins the transaction that is being closed.

### 4.3 Revisions, dirty state, save, reload, source mode

ADR 0002's lifecycle (`app.rs`: `text_rev`, `model_rev`, `saved_rev`) assumes revisions only grow.
Undo can return to the saved *content*, which must read as clean:

- `TextRev.counter` — monotonic; bumped by every applied step, undo, redo and rollback. Keeps ADR
  0002's ordering checks (stale job results, save continuations) valid.
- `StateId` — identity of a text+hint state: fresh per new transaction; undo/redo set the recorded
  ids. `saved_state` is set on save success; **dirty = `state != saved_state`**. A save snapshot
  carries `(counter, state)`; hints are materialised before the snapshot (§2.6).
- **External reload** (ADR 0002 "Disk changes"; `app.rs` `accept_reload`): one undoable `Bulk`
  transaction "Reload from Disk" from the existing minimal diff (`markise-engine/src/edit.rs`
  `patches`), hints dropped, live marks transformed, `saved_state` = new state. If its patches alone
  exceed the history budget, history is cleared with a notice **(design choice)**.
- **Source mode** (spec 02 §11: fresh history for the source editor): its own `History`; on leaving,
  the session's net change is pushed to the live history. To keep `saved_state` reachable when the
  user saved *during* the source session, the net change is split at every saved state: the live
  history receives one "Source Edit" transaction per segment `entry → save₁ → … → exit` (at most
  saves + 1 transactions), each carrying the source session's `StateId`s for its endpoints. Test:
  edit in source mode, save, edit more, leave, undo once → clean.

### 4.4 Atomic multi-step commands, journal, rollback

`session.transact(label, |tx| { … })`:

- Planners run against the current document; `tx.apply(plan)` applies each step immediately (later
  planners see earlier results); every applied step is appended to the transaction.
- A **journal** taken at `transact` start holds the live marks (selection, virtual block, hints,
  composition anchor, find-result set *generation*), the reveal state and `StateId`.
- **External effects are deferred until commit**: no autosave scheduling, no worker (re)starts, no
  MCP/automation notifications, no `saved_state` changes, no clipboard writes; they are queued in
  the transaction and run (or are discarded) at the end.
- **On error**: inverse steps are applied in reverse, the journal is restored (marks collapsed by the
  failed steps come back from the journal, not from inverse transforms), find results are
  invalidated and re-requested, `counter` is bumped (never reused, so results from workers started
  before the failure are discarded by stamp), and the error is returned. Bytes, hints, selection and
  `StateId` equal the pre-transaction values (property test).
- On success the steps form one transaction; queued effects run once.
- MCP `batch` (M3) maps to one `transact`.

Tests: overlapping sequential steps (step 2 edits step 1's insert) undo/redo byte-exact; mixed line
endings across undo/redo; failed transaction with find results and a composition anchor inside a
deleted range; a worker result arriving after rollback is dropped.

### 4.5 Memory bounds (10 MB documents, NFR-7)

- Stored bytes per transaction ≈ inserted + removed text; coalesced typing ≈ 2× what was typed.
- ≤ 500 transactions (WordCraft's `MAX_UNDO`) **and** ≤ 32 MB (default, 10 % of NFR-7's 300 MB,
  configurable); oldest dropped first; the newest transaction is always kept (Replace All on 10 MB
  stays undoable once).
- `removed` uses `Arc<str>`; a bulk step removing > 8 MB may reference a slice of the pre-edit
  snapshot instead of copying per patch **(optimisation, measure first)**.
- Source-mode history: own 8 MB slice, discarded on leaving (after the split of §4.3).

---

## 5. Input rules and per-block behaviours as commands

### 5.1 Typing pipeline (`edit.insertText`)

For each text event (not during IME composition, §6.3):

1. Read-only mode (spec 02 §12.4) → reject.
2. Auto-pair (spec 02 §4; `pairs()`; Markdown-syntax pairs when that preference is on).
3. Smart punctuation, convert on input (spec 02 §13; OS quote preference on macOS, locale default
   elsewhere **(assumption)**).
4. Insert as a canonical patch replacing the selection.
5. `on_text`: Enter-only syntax just formed at a block start → add an `EnterPending` hint in the
   same step (§2.6); immediate rule completed (spec 02 §2.1, C9: never on a soft-broken line; strict
   mode; 10-level quote guard) → close the typing group and push the `Auto` transaction.

Enter (`edit.newline`) first runs the §2.2 rules for a hinted line (`# Title`, fences, `$$`,
`[toc]`, rules, `---` as first block → front matter, pipe header, `[^id]:`, `[id]:`), then the
per-block table below.

### 5.2 Per-block keys (spec 02 §5) → planners

| Spec 02 rule | Command | Markdown planner |
|---|---|---|
| §5.1 paragraph middle / end | `edit.newline` | split with style preservation (§3.4, S-3.1: close/reopen spans crossing the caret using `RichInline` markers) / virtual empty paragraph (§2.2) |
| §5.1 heading end / middle / start | `edit.newline` | C7 |
| §5.1 list item; empty item | `edit.newline` | new item with the list's pattern, C11 renumbering; empty → outdent or leave (S-5.1, S-5.2) |
| §5.1 quote paragraph; empty one | `edit.newline` | new paragraph inside; empty leaves the quote |
| §5.1 empty link/footnote definition | `edit.newline` | back to empty paragraph |
| §5.1 front matter, code, math, table | `edit.newline` | newline in content / exit rules; code auto-indent; table: cell below (§5.3) |
| §5.1 Shift+Enter | `edit.softBreak` | soft break + continuation prefix; table cell: `<br/>` |
| §5.1 Cmd/Ctrl+Enter | `table.addRowBelow` / `code.exitBelow` | scope routing |
| §5.1 macOS Ctrl+O | `edit.openLine` | split, caret stays |
| §5.2 Backspace at block start, rules 1–6 | `edit.deleteBackward` | the six rules in spec order (S-5.3, S-5.4) |
| §5.2 Cmd+Backspace | `delete.toLineStart` | visual line start (`CaretGeometry`) |
| §5.3 Tab / Shift+Tab | `edit.indentOrNextCell` / `edit.outdentOrPrevCell` | list indent/outdent ("Indent Size"), next/prev cell (+ new row, S-6.1), definition fields, leading-space removal, literal tab, code indent |
| §5.4 arrows, Cmd+↑/↓, Ctrl+A/E, Cmd+J | `caret.*`, `nav.*` | `CaretGeometry`; code exits; goal x across fragments *[0005] §4.3* |
| §5.5 Esc | `edit.assist` | autocomplete (emoji, code language list) |
| §10, C8 | `select.*`, `delete.*` | words by UAX #29; styled scope from `RichInline`; block by `LeafView` |
| §15.1 heading semantics | `block.headingUp/Down`, `block.headingN` | incl. setext H1/H2 preservation; same level → paragraph |
| §3.3 | `format.*` | toggle / empty pair; link/image prefill from clipboard |
| §9 checkbox click | `block.toggleTask` | `ops::toggle_task` as planner |

**Lists (C11, A7).** Every list-changing planner renumbers following items only if the list was
sequential before the edit; fixed-number and non-sequential lists keep their numbers; renumbering is
part of the same patch set (one reparse; cost bounded by §7.2's list case).

### 5.3 Tables (spec 02 §6, A8)

- Caret in a cell anchor (§2.2). Typing is a minimal patch in the cell; `|` inserted escaped;
  Shift+Enter → `<br/>`; pasted line breaks → `<br/>` **(assumption)**; missing cells materialise
  their separators on first input.
- **Alignment (A8).** Non-pretty tables: only the edited row changes. Pretty tables are re-aligned
  when the caret leaves the cell or table, on Tab/Enter navigation and before save, joined into the
  open typing transaction (not per keystroke as `ops::set_table_cell` does today, `ops.rs:340`).
  The saved file is aligned; while typing only the edited row may be unaligned (invisible in live
  mode). Q7.
- Structure commands (add/delete/move row/column, alignment, delete table, insert table with the C2
  dialog, drag reorder of rows/columns, **Copy Table** = HTML with simple borders + Markdown, spec 02
  §6) are planners regenerating the table or rewriting rows, one transaction each.
- Segmented tables (`Segment::Rows`): column widths from header + first 64 rows; editing later rows
  never re-measures widths.

### 5.4 Code, math, HTML, front matter

- Code blocks edit content lines in place in the **Code** scope: Enter auto-indents, Tab/Shift+Tab
  indent by 4, auto-pairs without Markdown rules, Left at 0 / Right at end exit, Backspace on an empty
  or single-line block at 0 converts to paragraph. A typed line that would close the fence lengthens
  opener and closer in the same step (`fence_for_pattern`, R3.5.3/T13); this reparses the whole block
  (accepted, §7.2).
- **Syntax highlighting (P0, spec README MVP "fenced code with highlighting")**: not implemented in
  M1 (`markise-blocks` has image decoding only). M2d adds a highlighter in `markise-blocks` behind a
  `Highlighter` trait. **Decided (owner/lead, 2026-10-27; Q13): `syntect`** (MIT) with its
  pure-Rust `fancy-regex` backend (no Oniguruma C library). Still to do in M2d and record in
  `NOTICE`: the licenses of the bundled syntax definitions and themes in the syntax set we ship
  (only permissive ones; drop or replace others), binary-size delta, coverage of spec 01 Appendix A
  languages, and highlight latency per segment. Run per code segment off the UI thread, results cached by (segment content hash,
  language, theme), applied as styled runs; the edited segment is re-highlighted synchronously only
  if it fits the keystroke budget, otherwise shown with the previous/plain style until the worker
  returns (a bounded one-frame lag, never a block).
  **Viewer part implemented (2026-10-27, `syntax-highlight` branch):** `markise-blocks::highlight`
  (two-face syntax set, fancy-regex, Appendix A alias table, scope → `TokenKind` rules, one bounded
  background worker with content-keyed byte-bounded cache, per-chunk work bound, generation
  cancellation), `markise-layout::Highlights` (layout keys carry highlighter generation + ready
  bit; colors from `Theme::token_look`), HTML export spans. Licenses, coverage and throughput:
  NOTICE and `docs/spec/99-open-questions.md` HL-1…HL-7.
- Language box (spec 02 §7): an egui field editing the fence `info` span; one transaction per change.
- Math/diagram/HTML blocks (§8): styled source placeholders (M1-3; math postponed per AGENTS.md);
  editing state = same content with the caret inside. Front matter: content edit + §5.1 exit rule.

### 5.5 Source mode, focus and typewriter (spec 02 §12)

- (As delivered in M2d slice 1 the source view is a sibling of `View` rather than a
  `flatten_source` of it — see §16.) M2d replaces the `TextEdit` source mode (`source.rs`, 2 MB
  limit, M1-4) by the same editor with `flatten_source`: items are runs of **physical source lines** (segmented like code), covering every
  byte except the BOM (C35): blank lines, hidden link/footnote definitions, front matter, the final
  line terminator. Monospace, no reveal, Markdown colouring from the block tree, virtualised by `View`.
  Enter continues lists/quotes, Tab indents list items (`PlanRequest.source = true`).
- Position mapping on entry/exit (spec 02 §12.1): carets are source offsets already; the **viewport
  centre** maps through the source offset of the item at the vertical middle (entering: that line is
  centred; leaving: the item containing it is centred). Test both directions on a 10 MB document.
- Focus mode: dims non-focused items (C4) at paint time. Typewriter mode: caret line centred, half a
  viewport of padding.

---

## 6. Clipboard, find & replace, IME

### 6.1 Clipboard (spec 02 §14, spec 04 §9)

- Platform access: vendor and trim WordCraft's `clipboard.rs` + platform modules with `NOTICE`
  attribution, adding bounded reads (research note §6).
- **Clipping.** `EditBehavior::clip(range)` returns a `ClipFragment`: the covered leaves, partial
  leaves clipped **inside the existing semantic tree** with their ancestor context kept — enclosing
  inline styles and links (`bold` selected inside `**bold**` stays strong; link text stays a link),
  code context (a selection inside a code block or code span stays code, `*x*` there is not
  emphasis), enclosing containers (quote, list item with its marker kind) and table rows/columns
  covered. From it the format synthesises **clipped Markdown** (selected source plus re-added
  ancestor markers/fences/prefixes, markers never cut in half) — used for Copy as Markdown and as
  the input to `markise-export` for the HTML flavour (spec 03 classes; theme CSS optional per "Copy
  without Theme Styling").
- **Plain-text flavour default conflict:** spec 02 §14 says Copy = HTML + plain text with a
  preference switching plain text *to* Markdown (so default plain = rendered text); spec 04 §9 says
  the default plain-text flavour *is* Markdown. Proposal: follow spec 04 §9 (Markdown), keep the
  preference; **owner/spec decision Q12**.
- Cut = copy + delete (one transaction). Paste (P0 text): insert as Markdown source, line endings
  converted, continuation prefixes; literal in code; in cells pipes escaped and breaks → `<br/>`;
  pastes > 1 MB are bulk edits (§7.6).
- Smart paste HTML → Markdown (P1, M3; research note §7). Paste as Plain Text (P0).
- **Images:** drag & drop of image *files* is **P0** (spec 04 §7.1): M2e inserts `![](path)` using
  the "When Insert" preferences (copy/move to `./assets` etc., relative path if possible, A13 name
  collision rule). Bitmap paste from the clipboard is P1 (spec 04 §7.1 note) → M3.
- Tests: partial emphasis, link text, inside code block and code span, nested quote/list, partial
  table, Copy Table, mixed line endings.

### 6.2 Find & replace (spec 04 §2.3 P0; spec 02 §11)

- Matching on the **source text** (Q3), options case / whole word / regex (`regex`, linear time,
  pattern caps per research note §8). Matches inside hidden markup reveal their span when selected.
- **Query classes.** *Bounded* queries — literals, and regexes that cannot match a line terminator
  and have a finite maximum match length (checked on the parsed pattern with `regex-syntax`) — get
  synchronous local re-scanning after an edit: the window is the edited range extended by the maximum
  match length on both sides (and to line boundaries for anchors). *Unbounded* queries (e.g.
  `(?s)start.*end`, `\s+` across lines, unbounded repetition) restart a **cancellable worker search**
  on a fresh snapshot, debounced (≈150 ms after the last keystroke **(assumption, tune)**).
- **Stamps.** Every result set carries `(counter, query_generation)`; results from older stamps are
  shown as stale (dimmed count) and are not used for navigation edits. **Replace All is disabled
  while results are incomplete or stale**; when invoked it runs a full search of the current revision
  on the worker and applies the replacements as one `PatchSet` only if the revision is unchanged at
  completion (otherwise it re-runs). Replace next re-validates the current match first.
- **Bounded storage and per-keystroke cost.** At most 100 000 stored matches (UI shows "100 000+");
  matches are stored in chunks with a per-chunk lazy offset delta, so transforming them through an
  edit is O(log chunks + chunk), not O(matches).
- Snapshots are taken when a worker search starts (at most once per debounce window), not per
  keystroke; a 10 MB `Arc<str>` copy is ~1–3 ms **(hypothesis, measure)**; a rope (§7.6) would make
  it O(1).
- Bench: typing 200 keystrokes in the 10 MB corpus with a 100k-match bounded query and with an
  unbounded regex active; p99 must stay within NFR-4.

### 6.3 IME integration (ADR 0005 §4.4, unchanged)

- The session holds ADR 0005's `Composition { anchor, replaced, preedit, cursor, phase }` and its
  rules: start, update, clearing-is-not-cancelling, commit, final cancel, focus/document/mode
  switch. While composing, IME-consumed keys are not commands; typing pipeline, input rules,
  auto-pairs and reveal changes are suspended.
- **Backend-independent display contract**: the neutral layout input of each visible fragment
  touched by `replaced` carries `suppressed: Vec<Range>` (source-map kind `Suppressed`), and the
  focused fragment gets the preedit inserted at `anchor` (underlined `Synthetic` piece). The epaint
  path implements this by splicing sections into the item's job; the parley path by ADR 0005's
  `StyledText` suppressed ranges. `PlatformOutput::ime` (candidate window rect and caret rect) is set
  every composing frame on both paths.
- Commit = one transaction with boundaries before and after (§4.2), replacing `replaced` or inserting
  at `anchor`; then input rules run on the committed text (e.g. `》` + space with remapping, A22/C26).
  Cancel restores the display only: no transaction.
- **Acceptance stays as ADR 0005 states it**: the injected-event tests of ADR 0005 §4.4 (including a
  selection spanning two paragraphs and a table cell) on **both** shaper paths, and the manual
  real-IME gate (macOS Kotoeri Japanese, Simplified Chinese Pinyin, Korean) **before M2
  acceptance**; Windows/Linux IMEs before release. Developing on epaint does not relax the gate; any
  relaxation requires an explicit amendment of ADR 0005.
- Limits that remain egui's (ADR 0005 R4, research note §5): no preedit cursor range in egui 0.33
  (egui 0.37, used since 2026-10-27, provides `active_range_chars`);
  delete-surrounding/surrounding text on desktop still need egui patches E21/P7b.

---

## 7. Performance plan (NFR-4: keystroke p99 < 16 ms in a 10 MB document)

### 7.1 What the current code does per text change (verified)

| Step | Where | Cost on 10 MB | On the path today? |
|---|---|---|---|
| `String::replace_range` | `markise-md/src/lib.rs:222` | O(n) memmove | yes (M0 criterion `md-edit` ≈ 2.4 ms per insert+delete pair incl. reparse + shift, ADR 0001 — a mean on one machine, **not** a per-stage p99) |
| reparse from the last **top-level** clean block before the edit until resync | `lib.rs:195-205` | O(size of the enclosing top-level block): a keystroke inside a 2 MB list, quote or code block reparses all of it | yes |
| failed-opener restart: `failed_openers.iter().find(\|f\| f < start)` picks the **earliest** failed opener in the document | `lib.rs:199-201` | one unclosed `$$`/front-matter opener near the top makes every later edit reparse from it | yes |
| shift spans of all blocks after the edit | `lib.rs:266-270`, `markise-md/src/block.rs` `Block::shift` | O(spans after) | yes |
| whole-text diff of the source buffer | `markise-engine/src/edit.rs` `patches` via `jobs.rs:58` | O(n) scan + Myers in hunks | source mode (M1) |
| clone of the whole `Document` | `jobs.rs:57` | O(n) copy + allocations | rebuild worker |
| `DocModel::build` (flatten, hash every source byte, outline inline parse) | `markise-layout/src/model.rs:207` | "tens of ms" (its doc comment) | rebuild worker |
| `View::set_model` → `reset_heights` | `view.rs:93,179` | O(items) | yes |
| global `ctx_hash` in every `CacheKey` | `view.rs:243`, `model.rs:268` | any link-def/footnote/heading(+`[toc]`)/front-matter edit invalidates every layout and height | yes |
| segment hashes include absolute spans | `model.rs:600` (C-18) | edits above a segmented block invalidate its cached layouts | yes |

### 7.2 Parse and model: the actual keystroke hazards

Relative spans or a rope (§7.6) address memmove and shifting, **not** reparse size. Remedies, in
order:

1. **Intra-line fast path.** If a patch set touches a single physical line, inserts/removes no line
   terminator, and the format's `line_signature` of that line is unchanged (Markdown: container
   prefix, leading marker class and width, blank/non-blank, setext/thematic/delimiter-row candidacy,
   pipe count for table candidacy, fence-character run and closing candidacy, HTML-start candidacy,
   `$$`/`\[`/`\]`/front-matter-closer candidacy, hard-break trailing spaces), the block structure cannot change: spans are
   shifted, the leaf's inline cache and the containing item/segment hash are refreshed, no block
   reparse runs. A debug assertion and a property test compare the result with a full reparse after
   every fast-path edit. Expected to cover the vast majority of keystrokes (letters inside lines).
2. **Failed-opener restarts — semantics first.** A failed opener (unclosed `$$`, `\[`, front matter)
   is a look-ahead whose result depends on the text from the opener to the end of its search window
   (the closing-line search stops where the enclosing container ends, C34; for front matter, R3.10).
   The parser records, per failed opener, its window end *including the line that ended the
   container*. Rule:
   - An opener is **affected** if the edit (old coordinates) intersects or abuts its window.
   - An affected opener may be **skipped only with a proof** that its outcome is unchanged:
     every changed line of the *resulting* text (lines touched by a patch, after joins/splits, so a
     deletion turning `x\]` into `\]` is seen) contains no closer candidate for that opener's
     delimiter kind, and every changed line keeps its container-prefix signature (so the window end
     cannot move). Fast-path edits (1.) satisfy the proof by construction (closer candidacy is part
     of the line signature).
   - Otherwise the restart anchor is the **earliest affected, unproven** opener — not the nearest.
     Example (LaTeX delimiters on): two unmatched `\[` lines, then a `\]` line is added below →
     both openers are affected and unproven → reparse from the **first** `\[`, whose block now
     ends at the new `\]` and contains the second `\[` as content; restarting at the nearest
     opener would produce a different tree.
   - Performance never changes parse semantics: if the resulting reparse region is large it is
     handled by 4.; the current code's behaviour (earliest failed opener before the edit,
     `lib.rs:199-201`) is the correct-but-slow baseline the optimisation must match exactly.
   - Tests (incremental == full): the two-`\[` example; deletion creating a closer (`x\]` → `\]`);
     a closer created by joining two lines; a quote whose prefix removal extends a window; an
     unclosed `$$` near the top with fast-path edits far below (skipped by proof) and with a
     structural edit far below (restart from the opener → large region → 4.).
3. **Nested restart checkpoints (gate G2 remedy).** For structural edits inside a large container,
   restart at the nearest clean child boundary (list item / quote line start) with the container
   stack reconstructed from the block path, and resync at the next unchanged sibling. Parser change in
   `markise-md`; implemented only if G2 fails.
4. **Large reparses: synchronous by default; asynchronous mode is an explicit correctness gate.**
   M2 applies every edit synchronously, including large reparses (correct, possibly a slow frame;
   such edits are expected to be rare enough to stay outside p99 — measured by G2/G3). Only if
   measurements show such edits above 100 ms on the 10 MB corpus is the **asynchronous mode**
   implemented, and only with this contract (gate **G4**, must pass before it is enabled):
   - *Trigger*: an edit whose reparse region (from the restart anchor of 2./3. to the end of the
     enclosing top-level block) exceeds a threshold (256 KiB, hypothesis).
   - *Freeze*: the triggering edit's planned patches are handed to the worker together with an
     `Arc<str>` snapshot and the old blocks of the region; the session's document is **not**
     modified yet. The screen keeps the last installed state with an "updating…" indicator after
     100 ms; the view stays scrollable, but no edit applies.
   - *Strict ordering*: from the trigger on, **every** input event (characters, IME events, keys,
     commands, pointer edits, paste, save, reload, close) is appended to one FIFO queue; nothing
     bypasses it, no fast path runs while a job is pending. So `Enter` then `a` always executes as
     Enter, then `a`.
   - *No rebase needed*: because nothing applies while pending, the revision cannot change; the
     worker result (new text + reparsed blocks + flattened items for revision `r+1`) is installed
     atomically when it arrives, then the queue is replayed in order on the UI thread; a replayed
     event may trigger another asynchronous job, which re-freezes with the rest of the queue intact.
   - *Failure/cancellation*: a worker error or panic falls back to applying the same edit
     synchronously (correct, slow); document close waits for the job (bounded by the synchronous
     cost) before the normal close/dirty flow; save and reload are ordinary queued events. Queue
     length is bounded (10 000 events, then input is refused with a beep and a notice).
   - Tests: Enter + `a` ordering; IME commit queued behind a pending job; nested triggers during
     replay; worker failure → synchronous result identical; close/save/reload while pending.
5. **Accepted block-local costs**, measured: fence extension (opener + closer patches → whole code
   block reparsed), list renumbering (patches across the list → whole list), table realign (whole
   table). Each is O(block), occurs at most once per specific action, and falls under the async
   fallback above the threshold.
6. **Model.** `DocModel::apply(&EditSummary)` re-flattens only the replaced top-level blocks — and for
   a fast-path edit only the affected item/segment; re-hashing is bounded to those items. Item hashes
   use item-relative data only (C-18 fix).

Tests (incremental == full parse and model, plus timing in the bench): a 2 MB list with edits in its
middle (fast path and structural); a 10k-line code block (typing, fence extension); an unclosed `$$`
near the top followed by edits far below; renumbering a 10k-item ordered list; a table realign of a
5k-row pretty table.

### 7.3 Keystroke budget (hypothesis until G1–G3)

Synchronous on the UI thread:

| Stage | Work | Target p99 (10 MB, reference machine) |
|---|---|---|
| input → command → plan | typing pipeline, rule check on the focused leaf | 0.3 ms |
| `FormatDoc::apply` | memmove + fast path or bounded reparse + span shift | 3 ms |
| `DocModel::apply` | affected items only; dependency propagation (§7.4) | 1 ms |
| `View::apply` | heights splice, anchor on the caret line | 0.5 ms |
| marks, history | selection, hints, chunked find results; typing coalescing | 0.2 ms |
| layout of the edited item | inline view + shaping of one item/segment | 3 ms |
| frame | paint + tessellate visible items, caret, selection | 4 ms |

The ≈12 ms CPU sum is an allocation to test against, not a guarantee: the M0 figure is a criterion
mean, and headless tessellation time is not keystroke-to-pixel latency (GPU upload, compositor and
present are outside it). Allowed per keystroke: linear passes with tiny constants over arrays of
numbers (memmove, offset shifts, a Fenwick rebuild), each measured. Forbidden: parsing, hashing or
copying text outside the affected region, inline-parsing or shaping other items, cloning the
document, whole-text diffs, invalidating unrelated cache entries.

### 7.4 Dependency-scoped invalidation

The single `ctx_hash` is replaced by explicit dependencies:

- **Attempted lookups, including negative ones.** Building a `RichInline` records every reference
  lookup the inline parser *attempted* — `[label]`, `[text][label]`, `[text][]`, `![…][label]`,
  `[^label]` — by normalised label hash, whether it resolved or not (an unresolved `[label]` stays
  text today, so success-only tracking would miss a later definition). This needs a small
  `markise-md` change: the inline parser reports lookups. The `DepSet` also records image use
  (`has_image` exists) and TOC use.
- `DocModel` keeps a reverse index `label hash → items`. `EditSummary.deps` lists labels whose
  definitions were added, removed or changed (old and new definition blocks of replaced top-level
  blocks); only items in the reverse index for those labels get new keys and height estimates.
- **Anchors (spec 01 §7).** A heading's id depends on its own rendered text (R7.1) **and on every
  earlier anchor with the same slug**, where anchors are headings *and* `<a name="…">` elements
  found in inline or block HTML (R7.2). The format reports, per leaf, its anchor contributions
  (`HeadingInfo` for headings; `HtmlAnchor { name }` for each `<a name>` in raw HTML), recorded in
  the leaf's `DepSet`. Keys are compared case-insensitively (R7.2); an HTML `name` is used as
  written, lower-cased, without re-slugging **(assumption)**.
  - Numbering per R7.2: if *n* earlier anchors share the key, the heading id is `key-(n+1)` —
    `intro`, `intro-2`, `intro-3`; R7.3: a heading whose id would be `write` gets `write-1`.
  - `DocModel` keeps an **anchor index** `key → ordered list of (offset, Heading | Html)`. When an
    anchor is inserted, removed or changes key (a heading's rendered text changes — including
    through a reference-definition dependency — or an `<a name>` is added/edited/removed in any
    HTML block or inline HTML), the ids of **headings after that position in the old and the new
    key lists** are recomputed. Cost O(anchors sharing those keys); worst case O(headings), measured
    in the bench. Changed heading ids update the outline entries, invalidate TOC items (dep `toc`)
    and the sidebar outline; `#anchor` links resolve at click time and need no invalidation.
- Front matter `typora-root-url` and the document folder form the `images` dependency.
- Tests (incremental vs full model, ids compared for every heading): adding/removing a definition for
  an unresolved reference; editing a definition used inside a heading (outline text and id, TOC);
  three headings `Intro` → `intro`, `intro-2`, `intro-3`, then editing the first (later ids shift
  down); inserting `<a name="intro">` in an HTML block before them (all shift up: `intro-2`,
  `intro-3`, `intro-4`) and removing it again; an inline `<a name="Intro">` inside a paragraph
  (case-insensitive); renaming a heading to collide with another; a heading `Write` → `write-1`.

### 7.5 Other required changes

1. No document in `DocModel`; no clone per edit (§2.5). Full rebuilds (reload, option change) stay on
   the rebuild worker with ADR 0002's revision checks.
2. Batch apply (§2.1) for renumbering, Replace All, table realign.
3. Source mode without whole-text diffs (the editor emits patches itself).
4. `Item.path` removed (C-12).

### 7.6 Gates

- **G1 — text and offsets** (`editor-keystroke` bench, API level): memmove + span shift p99 ≤ 1.5 ms,
  items/heights splice p99 ≤ 1.5 ms at start/middle/end of the 10 MB corpus. If shifting fails:
  **relative spans** (top-level blocks and items in chunks of ≤ 256 with lazy base deltas; heights in
  the same chunks). If memmove fails: a **rope** (`crop` or `ropey`, license check then) with the
  parser reading a contiguous copy of the reparse window.
- **G2 — parse and model**: `FormatDoc::apply` + `DocModel::apply` p99 ≤ 4 ms for the §7.2 test
  cases (fast path, structural edit in a 2 MB list, failed opener, renumbering). If it fails: nested
  restart checkpoints (§7.2.3); large reparses otherwise stay synchronous unless G4's asynchronous
  mode (§7.2.4) is implemented and passes its correctness tests.
- **G3 — end to end**: `type-10mb` (headless, event → tessellated output) p99 ≤ 12 ms, and the
  desktop `MARKISE_TIMING=1` keystroke-to-present measurement p99 < 16 ms (manual, reported).
- **G4 — asynchronous reparse mode** (only if built, §7.2.4): the ordering, replay, failure and
  close/save/reload tests pass; until then large reparses are synchronous.
- **Bulk edits** (Replace All with many matches, paste > 1 MB, select-all + type, reload, option
  change) are not keystrokes: target < 100 ms on 10 MB, otherwise they use the asynchronous mode of §7.2.4
  (only if implemented and G4 passed; else they stay synchronous).

### 7.7 Measurement harness

- `crates/markise-engine/benches/editor.rs` (`editor-keystroke`): headless `EditorSession` on the
  10 MB corpus; caret at start/middle/end, inside a 2 MB list, a 10k-item ordered list, a segmented
  table, a 10k-line code block, a segmented paragraph, below an unclosed `$$`; 200-keystroke scripts
  (letters, Enter, Backspace, an input rule, Cmd+B); p50/p99/max per stage and total; also with a
  find query active (§6.2).
- `crates/markise-ui-egui/benches/viewer.rs` gains `type-10mb` (injected key events through the real
  app on an `egui::Context`).
- Desktop: `MARKISE_TIMING=1` logs per-keystroke stage timings and frame present time.
- CI: spec README regression rule (> 10 % fails); numbers in each milestone's final commit.

---

## 8. Milestones, dependencies and tests

### 8.1 Feature-to-milestone matrix

Each milestone keeps `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test
--workspace` green and adds benches for what it introduces. Creation scenarios are assigned to the
milestone that can also *edit* the created block.

| # | Scope | Acceptance tests | Depends on |
|---|---|---|---|
| **M2a** — seam + edit core (no typing UI) | §1.5 (incl. `markise-testfmt`, C-18 fix); `PatchSet` + batch apply; planners for existing `ops`; literal-hint plumbing in the parser (§2.6); intra-line fast path + proof-based failed-opener restarts (§7.2.1–2); `DocModel::apply`, `View::apply`; dependency-scoped invalidation with negative lookups and slug propagation (§7.4); `EditorSession` skeleton with steps, journal, `transact`/rollback, `History`; `xtask layers` | viewer screenshots + viewer bench unchanged; seam tests on both formats; incremental == full (parse, model, outline, slugs) property tests incl. §7.2/§7.4 cases; rollback/journal tests (§4.4); G1 and G2 reported | ADR 0005 **T2** touches `block.rs`/`Prim`: land T2 first or in the same series |
| **M2b** — caret, selection, plain typing | `DocPos`/marks/transforms, caret stops and explicit anchors, source maps (epaint), hit testing, caret moves, mouse selection, caret/selection painting, reveal (§2.4), insert/delete, paragraph split, virtual empty paragraph, undo/redo with grouping (§4.2), `StateId` dirty/save/reload, registry + keymap + menus for implemented commands | S-3.1; §3.2 reveal/step-through; §11 undo cases incl. immutable-selection test; C13 pause (injected clock); undo-to-clean; reload transaction; G3 headless | M2a; epaint |
| **M2c** — paragraphs, headings, lists, quotes, inline | immediate rules §2.1 incl. callouts; Enter rules §2.2 for headings, rules, `[toc]`, footnote/link definitions; hints + materialisation (§2.6); per-block keys §5.1–§5.3 for paragraphs, headings, lists, quotes, definitions; C11; heading commands; `format.*`; auto-pair; smart punctuation; selection/delete commands §10; Move Row (A4, blocks); `keyBinding` | S-2.1, S-2.2, S-2.3, S-3.1, S-5.1–S-5.4; one scenario per §2.1 row and per §2.2 row listed here; §2.6 hint tests (bytes + block type + selection) | M2b |
| **M2d** — tables, code, special blocks, modes | Enter rules creating code (S-2.5), table (S-2.6), front matter (S-2.4), math, HTML; table cell editing/anchors/navigation/toolbar/ops/drag reorder/Copy Table, A8 realign; code blocks incl. **syntax highlighting** and language box; math/HTML/front-matter editing; new source mode (removes M1-4 limit) with viewport-centre mapping and its history split (§4.3); focus/typewriter | S-2.4, S-2.5, S-2.6, S-6.1, S-6.2; padded/missing cell typing, > 2 KiB cell navigation, empty code/cell anchors; highlighting of the spec 01 Appendix A core languages with an off-thread cache bench; Copy Table; source-mode round-trip of mixed endings (C23) and viewport-centre preservation both ways; source-mode save identity test | M2c |
| **M2e** — clipboard, find, IME, files, autosave | clipping copy (HTML + plain), Copy as Markdown, cut, paste text, paste as plain; **image-file drag & drop (P0)**; find/replace with query classes, stamps, Replace All gating; IME per §6.3 on the default shaper; autosave/recovery (research note §15); final NFR-4/8 report | §6.1 clip tests; image drop with "When Insert" prefs and A13; §6.2 tests + find-active typing bench; ADR 0005 §4.4 injected-event tests on both shaper paths; **real-IME gate (macOS Japanese, Chinese, Korean) before M2 acceptance** | M2b (find, clipboard), M2c/M2d (paste into structure) |

### 8.2 What can start on epaint vs what waits for parley *[0005]*

Can start now (epaint behind the T2 seam): everything defined on source offsets — seam, patches,
planners, hints, undo, commands, keymap, reveal, input rules, tables, code blocks, source mode, find,
clipboard, the IME state machine and its suppression/preedit display contract. Caret stepping uses
grapheme boundaries on rendered text (§2.3), so Latin, CJK and most emoji behave correctly.

Waits for T7 (parley editing): grapheme-correct hit testing inside ligatures, bidi visual movement and
selection rects, complex-script caret stops, parley-path IME metrics. T8: accessibility. M2 acceptance
requires the real-IME gate on whichever shaper is the default at that time (§6.3); if T7 has not
landed, the gate is run on the epaint path. None of this changes the editing model.

### 8.3 Test strategy

- **Scenario tests from spec 02 as data** (`crates/markise-engine/tests/scenarios/*.scn`), run
  headlessly against `EditorSession` with the CPU test font system; each names its spec ID. `‸` marks
  the caret, `«…»` a selection (escaped as `\‸`), a caret on its own line after the last block means
  the virtual empty paragraph; keys resolve through the *default keymap* of the given platform:

  ```text
  id: S-2.1 heading (spec 02 §2.3)
  given: "‸"
  keys: type "## Plan"
  expect: "## Plan‸"
  expect_tree: 'para "## Plan"'      # paragraph until Enter (literal hint)
  then_keys: Enter
  then_expect: |
    ## Plan

    ‸
  then_tree: 'h2 "Plan"'
  ---
  id: §11 input-rule undo
  given: "‸"
  keys: type "- " | Cmd+Z
  expect: "- ‸"
  expect_tree: 'para "- "'           # bytes unchanged, projection literal
  ```

  Every scenario asserts bytes, projected block tree and selection; optional `expect_undo: n` checks
  that n undos return to `given` byte-identically (with its tree and selection) and redo returns.
- **Property tests**: random edit/command sequences on both formats — undo all → original bytes,
  hints and selection; redo all → final; incremental == full parse, model, outline and slugs;
  fast-path == full reparse; rollback == no-op incl. marks; every live `DocPos` on a char boundary and
  a caret stop; patches never touch bytes outside the addressed blocks (R1.2/R1.3).
- **kittest interaction tests** for key routing, focus lock (Tab), menus enabled/disabled, IME event
  injection, image drop.
- **Benches** per §7.7 with CI regression gates.

---

## 9. Alternatives considered

- **Edit a rich tree and serialize**: violates spec 01 R1.2/R1.3 and ADR 0001; rejected.
- **Snapshot undo** (WordCraft's storage): up to 10 MB per step with our `String`; rejected.
- **Transient block-type state without a parser hint** (overlay the projection after parsing): an
  unclosed fence would still swallow the rest of the document and force huge reparses; rejected in
  favour of parser-level literal-line hints.
- **`Any` downcasts from `&dyn FormatDoc`** for planners: works but moves type errors to run time;
  rejected in favour of planning methods on the document (`FormatDoc: EditBehavior`).
- **Format-specific editors**: duplicates caret, layout, commands; contradicts AGENTS.md. Rejected.
- **Generic static dispatch** (`EditorSession<F>`): spreads a type parameter through engine/UI/MCP for
  no measurable gain. Rejected for now.
- **Rope now**: large parser change, does not address reparse size; deferred behind G1.

## 10. Open questions for the owner

| # | Question | Proposal |
|---|---|---|
| Q1 | Empty paragraphs: representation/persistence (spec gap → section A) | Virtual, not saved; materialised on first character (§2.2) |
| Q2 | Accept `counter` + `StateId` split (amends ADR 0002)? | Yes (§4.3) |
| Q3 | Find on source text or rendered text? (spec 04 §2.3 silent) | Source text (§6.2) |
| Q4 | Windows/Linux defaults for colliding chords: Toggle Task (Ctrl+X = Cut), Comment (Ctrl+- = Decrease Heading), Inline Math (Ctrl+M), Move Table Row/Column | No default until decided; bindable via `keyBinding` |
| Q5 | Copy as Markdown of partial/multi-container selections | Clipped Markdown with ancestor markers (§6.1) |
| Q6 | Export for future formats: per-format exporter or HTML from the neutral model? | Per-format for now (C-16) |
| Q7 | Pretty-table realign at cell exit/navigation/save instead of per keystroke (A8) | Yes (§5.3) |
| Q8 | Undo budget 32 MB / 500 steps vs NFR-7 | Yes, configurable (§4.5) |
| Q9 | Smart-punctuation rewrites as separate undo step | Yes (§4.2) |
| Q10 | ADR 0005 amendments: item-relative `FragmentId` / source maps; `DocPos.virt` discriminator | Yes (§2.2, §2.3) |
| Q11 | Leaving a not-yet-Entered `## Plan` (or an undone `- `): escape it or let it take its parsed form? (spec gap → section A) | Escape (`\## Plan`, `\- `): "what you saw is what is saved" (§2.6) |
| Q12 | Default plain-text clipboard flavour: spec 02 §14 (rendered text, pref → Markdown) vs spec 04 §9 (Markdown by default) | Spec 04 §9: Markdown (§6.1); spec team to reconcile |
| Q13 | Syntax highlighter for P0 code highlighting | **Decided (owner/lead, 2026-10-27): `syntect` with the pure-Rust `fancy-regex` backend** (§5.4); syntax-set licenses, size and coverage checked in M2d |

## 11. Proposed entries for `99-open-questions.md` (to add when accepted)

Section A (spec team): Q1 empty paragraphs; Q3 find scope; Q11 un-Entered block syntax on leave;
Q12 plain-text clipboard flavour. Section C / M2-n (implementers): M2-1 revision split (Q2); M2-2
colliding Windows/Linux chords (Q4); M2-3 clipped Copy as Markdown (Q5); M2-4 table realign timing
(Q7); M2-5 undo bounds (Q8); M2-6 smart-punctuation undo step (Q9); M2-7 reveal only at the focus end
(§2.4); M2-8 insert/delete switch opens a new undo group (§4.2); M2-9 find debounce and 100k result
cap (§6.2); M2-10 asynchronous reparse mode threshold 256 KiB and its G4 contract (§7.2.4).

## 12. Review log (revision 2)

Codex review of revision 1 (`238599d`), REQUEST CHANGES, 12 items:

| # | Item | Addressed in |
|---|---|---|
| 1 | Seam dependency cycle; planner access to concrete doc; second format | §1.2 (descriptors in L1, `CommandSpec` in L4, `FormatDoc: EditBehavior`, `markise-testfmt`), §3.1 |
| 2 | Transient interpretation state for input-rule undo / paragraph-until-Enter | §2.6, §4.1 (`Step.hints`), §4.2, §5.1, §8.3 |
| 3 | Historical selections must not be transformed | §2.2, §4.1 |
| 4 | Ordered steps, journal, deferred effects, canonical/verbatim patches | §2.1, §4.1, §4.4 |
| 5 | Parse/model gate beyond span shifting; budget as hypothesis | §7.1, §7.2, §7.3, §7.6 (G2) |
| 6 | Segment hashes not offset-independent (C-18) | §1.1, §1.5, §2.3, §7.2.6 |
| 7 | Negative reference lookups; heading/slug/TOC propagation | §7.4 |
| 8 | Find: bounded vs unbounded queries, stamps, Replace All gating, bounded results, bench | §6.2 |
| 9 | HTML copy clipping with ancestor context; plain-text flavour conflict | §6.1, Q12 |
| 10 | Caret anchors for missing/padded cells, empty fragments, > 2 KiB cells; source-mode coverage | §2.2, §2.3, §5.5 |
| 11 | IME suppression on both backends; real-IME gate unchanged | §6.3, §8.2 |
| 12 | Feature-to-milestone matrix; image-file drop P0; code highlighting; Copy Table; source-mode viewport; save identity across source sessions | §8.1, §5.3, §5.4, §5.5, §6.1, §4.3 |

Codex pass 2 on revision 2 (`73e70a5`): items 1, 3, 4, 6, 8, 9, 11, 12 accepted; five blockers,
addressed in revision 3:

| Pass-2 item | Fix | Where |
|---|---|---|
| Input-rule undo contradicted exact state restoration | typing transaction ends **with** a literal hint, conversion removes it before display; undo only inverts recorded steps; repeated undo/redo and dirty identity tests | §2.6, §4.2 |
| Failed-opener restart could change the parse | affected = edit intersects/abuts the recorded window; skip only with a proof on the resulting changed lines; otherwise earliest affected opener; two-`\[` and deletion examples as tests | §7.2.2 |
| Async fallback had no ordering/completion contract | synchronous by default; optional asynchronous mode behind gate G4 with freeze, single FIFO for all input, no rebase (nothing applies while pending), failure → synchronous | §7.2.4, §7.6 |
| Anchors omitted `<a name>` and used wrong suffixes | anchor index over headings and HTML anchors, R7.2 numbering (`intro`, `intro-2`), R7.3 `write-1`, tests | §7.4 |
| Missing table cells collapsed to one position | `DocPos.virt` discriminator, preserved in history, re-resolved on patches at its offset, materialisation plan, multi-missing-cell test | §2.2 |

Codex pass 3 on revision 3 (`535478f`): one blocker — `virt` depends on the row's real-cell count,
so edits elsewhere in the row or table left stale virtual marks. Revision 4: virtual marks store
their intended slot (`table_start`, `row`, `col`) and are re-resolved whenever the defining row or
table structure changes (cell count, table width, row/table removal), with tests (§2.2).

Also recorded: Q13 decided — `syntect` with the pure-Rust `fancy-regex` backend (§5.4).

## 13. Implementation status: M2a part 2 (2026-10-27)

Branch `m2a-layout`. What landed, against the coupling points of §1.1 and the gates of §7.6:

- **G1 remedy (§7.6):** top-level blocks (`markise-md`, `tops.rs`) and layout items
  (`markise-layout`, `Items`) live in `markise_doc::chunked::RelChunks`: each element relative to
  its own origin, origins relative to chunk bases (≤ 256 per chunk). An edit re-chunks the touched
  chunk group and adds the delta to later bases. `Document::blocks()` remains as an absolute
  compatibility view, materialised lazily after an edit (exporters, tests).
- **Layout on the seam (C-1…C-12, C-18):** `markise-layout` depends on `markise-doc` only
  (`markise-md` is a dev-dependency; enforced by `[format_seam]` in `layers.toml`). It reads
  documents through `FormatDoc::flatten` (item-relative `FlatEvent`s), `inline` (`RichInline` with
  rendered values; paragraph lines, segments, capped table cells), `table`, `table_row_cells`,
  `heading`, `anchor_id`, `resource_root`. `DocModel` no longer owns the document (C-2); segment
  spans are item-relative (C-18). Text blocks carry item-relative `SourceMap`s
  (`TextBlock::source_map`), `FragmentId.block` is item-relative for cells and code chunks.
- **Incremental model (§7.2.6):** `DocModel::apply(&dyn FormatDoc, &EditSummary)` rebuilds the items
  of the re-parsed top-level blocks (or of one leaf for an intra-line edit), shifts the rest, renumbers
  anchors per affected key and re-segments `[toc]`s when the outline changed (a `[toc]` that changes
  its number of segments marks the change `full`: the view resets its heights); `View::apply`
  splices heights. Cached layouts are keyed by everything they hold, including item-relative
  source positions (segmented tables hash their rows' offsets in the table). `EditSummary` gained `replaced_top`/`new_top`/`deps`/`inline_leaf` and `then` (combined
  summary of `apply_with_hints`). Property tests compare apply with a full build after random edits.
- **Intra-line fast path (§7.2.1):** implemented with a conservative line signature (one content
  line of a paragraph or heading that starts with a letter, edit after that letter, no pipe, `#`,
  backslash or line break in the line or edit, trailing blanks kept); property-tested against full
  parses.
- **Dependencies (§7.4):** reference definitions enter only the cache keys of items that may use
  them: the format reports a superset of the labels an item's inline parse may look up (every
  innermost bracket text, resolved or not, except certain inline links), the view folds their
  definition fingerprints into the key (memoised per item hash and pruned when it outgrows the
  model, so building the model needs no label scan; footnote candidates `[^x](…)` and the
  `\|`-unescaped form of table-cell labels included); edits report the labels of the definitions among the replaced and new blocks.
  Anchors: renumbered per affected key. Images: the resource root/document folder enter only keys of
  items with images; a resource-root change reports those items so the view demotes their heights.
  (The view's context key has no other model-side input: references are demoted per label,
  highlighting changes colours only.) `[toc]`: keyed by the outline. Not yet: HTML `<a name>` anchors (the viewer did
  not use them before either), a reverse label → items index (definition edits scan the headings and
  the view's memo).
- **Remaining coupling:** the engine/UI viewer lifecycle (C-13, C-15) still uses `markise_md::Document`
  concretely (`Loaded`/`Rebuilt` carry `Arc<Document>`; source mode, saving); export renders the
  Markdown tree (C-16, by design); `FormatRegistry` wiring is not done.
- **Gates:** G1 memmove + span shift p50 ≈ memmove alone (0.22 ms at the start of 10 MB; p99
  noise-bound on the shared machine); items/heights splice p99 ≤ 0.05 ms. G2: intra-line edits in a
  2 MB list ≈ 1.4 ms p99, edits below an unclosed `$$` ≈ 0.1 ms; a structural edit (Enter) in a 2 MB
  list still re-parses and re-flattens the whole list (≈ 35–50 ms): §7.2.3 nested restart
  checkpoints and a container-local re-flatten are the remaining G2 work (done: §14).

## 14. Implementation status: gate G2 (2026-10-27)

Branch `m2a-g2`. §7.2.3 is implemented as designed; measurements did not call for another
design (the residual cost is a shift of the container's later children, ≈ 0.5 ms for 25k
siblings, see below).

- **Nested restart (`markise-md`, `nested.rs`, `parser::prepare_nested`/`parse_nested`).** For a
  structural edit inside a top-level container (list, quote, footnote, nested lists) the parser
  picks the deepest container on the path whose child starts on a later line than the
  container and ends before the edited line; it reconstructs the container stack at that
  child's line (prefixes, list item content columns, lazy-continuation state), parses from
  there and re-synchronises at the first line that starts an *old* sibling of the same kind
  (same marker shape and content column for list items) after the edit, with hints unchanged on
  that line. A restart is refused where the reconstructed state could differ from a full
  parse: a child directly after a sibling that may absorb the line lazily (paragraph, table,
  link definition) outside a list, the first line of a container, edits touching the BOM, a
  failed opener before the edit (it stays the restart anchor, §7.2.2, and lies outside the
  container). If the container closes before a re-sync, the output continues as an ordinary
  top-level re-parse that re-syncs at a clean top-level block.
- **Splice.** The re-parsed children replace the old ones in place (spans relative to the
  top-level block's origin), later siblings and the ancestors' tails shift; list tightness is
  recomputed only from the windows around the spliced children when the list was tight or the
  splice made it loose (`parser::refinish_spliced`; a loose list whose looseness might be gone
  is rescanned up to its first loose spot). When a container's computed state changes (list
  tightness, callout kind) the edit reports the whole top-level block as re-parsed, because the
  rendering of every item changes.
- **Model.** `EditSummary::nested_from` (the start of the first re-parsed child) lets
  `DocModel::apply` re-flatten only the leaves from there to the re-sync point:
  `FormatDoc::flatten_region` emits `FlatEvent::Resume` for the enclosing containers (their
  first-leaf decorations are spent) and then the region. A format without the method falls
  back to flattening the whole top-level block. Intra-line edits in a *segmented* block (code,
  long paragraph) rebuild only the edited segment; the others shift and are re-keyed from a
  stored per-segment text hash.
- **Fast path.** The intra-line fast path descends to the leaf by binary search (it was a linear
  scan of a list's items) and shifts later siblings as a whole.
- **Fence completion.** The keystroke completing ```` ``` ```` applies text and the final hint
  set in one `apply_with_hints` (§2.6), so the suffix is never re-parsed as code first; the
  session bench covers it.
- **Neighbour-dependent rules (review round 1).** A re-sync (nested or top-level) is valid only
  where the old sibling's fate does not depend on what precedes it. Audited: adjacent HTML
  blocks merge when the later one closes (R3.14.2) — no nested re-sync on an HTML block right
  after an HTML sibling, and the top-level restart moves before an adjacent HTML predecessor
  (this top-level case predates the branch); a block right after a link reference definition is
  the rest of the paragraph the definition was extracted from (it opened earlier, so e.g. its
  table/setext fate differs) — not a re-sync point. Lazy continuation, setext underlines, tables
  after paragraphs and interrupting list items are decided on the line itself from the open
  paragraph, so a block opened at the line in the new parse matches the old one; adjacent lists
  merge into the container being re-parsed (its identity is checked). The region flatten emits
  `Resume` ("decorations spent") only when the innermost container produced an item before the
  region and no list opened after it; otherwise (e.g. `- >` with an empty quote) it flattens the
  whole block.
- **Tests.** `markise-md/tests/nested.rs` (incremental == fresh parse and lossless round-trip on
  container-heavy documents, 10k cases; deterministic Enter/typing/Backspace edits that must
  take the nested path; tightness transitions), `markise-layout/tests/incremental.rs`
  (`DocModel::apply` == full build in containers, 10k cases; locality of the model update in a
  list, a quote and a nested list). The generators include empty containers (`- >`, `> -`,
  `> >`, `-`), adjacent HTML blocks, setext/lazy/definition/table/adjacent-list cases. The
  fast-path, hint, round-trip and editor property suites were run with 10k cases as well; CI
  runs the incremental suites in release mode with 3000 cases (Linux).

**Measurements** (`cargo bench -p markise-engine --bench editor`, release, aarch64 laptop shared
with other work; `FormatDoc::apply` + `DocModel::apply`, p50 / p99 in ms; budget p99 ≤ 4):

| Case (2 MB container, `[middle]`; ranges over two runs) | main before | this branch |
|---|---|---|
| Enter in a flat list | 33.1 / 46.1 | 0.66–0.90 / 1.04–1.70 |
| typing / Backspace in a flat list | 0.90 / 1.54 (typing) | 0.62–0.79 / 1.17–1.75 |
| Enter / typing / Backspace in a nested list | (not measured) | 0.57–0.94 / 0.96–2.06 |
| Enter / typing / Backspace in a block quote | (not measured) | ≤ 0.20 / ≤ 0.58 |
| the same at the container's end | (not measured) | ≤ 0.01 / ≤ 0.07 |
| typing / Backspace in a 2 MB code block | (not measured) | 0.61–0.66 / 1.00–2.05 |
| typing / Backspace in a 512 KB table | (not measured) | 1.22–1.36 / 1.96–2.34 |
| edits below an unclosed `$$` (10 MB corpus) | 0.08 / 0.22 | 0.05–0.11 / 0.12–1.52 |

G2 worst p99 over all cases: 2.1–2.8 ms across three runs (PASS; main: 46 ms). Session keystroke
completing ```` ``` ````: 0.29 ms p99 at a paragraph start of the 10 MB corpus, 1.5 ms p99 at an
item start of a 2 MB list. G1 is unchanged (items/heights splice p99 ≤ 0.25 ms; memmove + shift
p99 1.3–2.3 ms tracks the memmove-only p99 of the same run, i.e. machine noise, p50 0.22 ms
as before). Viewer (`markise-ui-egui` bench): open 1 MB 17.9 ms, 10 MB 77.7 ms median (main:
19.2 / 79.0), scroll p99 1.30 ms (1.26), no regression.

**Residual costs (accepted, §7.2.5).** (1) An edit that flips a 2 MB list between tight and loose
(a blank line between two items, or removing the only one) changes every item's rendering: the
parse stays nested (≈ 2.5–4.4 ms p99) but the model re-flattens the list, 18–28 ms p99 — one
slow frame for a one-off structural change; an in-place update of the items' tightness flag is
the remedy if it shows up in G3. (2) Every nested edit still shifts the spans of the container's
later children (≈ 0.5 ms per 25k siblings); relative child coordinates (as for top-level blocks,
§13) would remove it if containers far beyond 2 MB matter.

## 14. Implementation notes: M2b — caret, selection, plain typing (2026-10-27)

Branch `m2b`. What landed against the M2b row of §8.1 (decisions numbered M2b-n in
`99-open-questions.md`):

- **Caret geometry** (`markise-layout::caret`): positions are `DocPos` source offsets; a caret
  stop is any offset a rendered grapheme boundary of a text fragment maps to through its
  item-relative source map, biased at piece boundaries (moving right lands Upstream after the
  skipped character, left Downstream before it), so hidden markers, container prefixes and atoms
  are never stops and revealed markers are stepped through (spec 02 §3.2). Hit testing (epaint
  `TextBlock::hit`, snapped to grapheme stops), caret and selection rectangles, grapheme / word (UAX #29) / visual row /
  row start-end with soft-wrap affinity / page / document moves; vertical moves keep `goal_x`
  across items and skip items without stops (rules, images). Empty paragraphs/headings carry an
  explicit `empty_anchor` (§2.2).
- **Reveal (§2.4)**: `ItemFocus { reveal, virtual_after }` is a layout input of the focused item
  only (`View::set_focus`, part of that item's cache key); the reveal set is computed from the
  neutral `RichInline` (nodes with markers containing or touching the caret, all levels), revealed
  markers are laid out as `Marker` pieces in the theme's meta colour. Caret moves with an
  unchanged reveal set cost no layout. Frozen (kept as shown) during a pointer drag (M2b-6).
- **Session** (`markise-engine::editor`): owns `Arc<dyn FormatDoc>` edited in place and reports
  `DocChange` (combined `EditSummary`s) for the view; the editing surface `EditorView` (session +
  `View` + registry) releases the view's reference before mutating commands, applies the summary
  with `DocModel::apply` on the view's own model (`View::model_mut` + `View::apply_change`) and
  refreshes the focus (M2b-8). Stop-aware Backspace/Delete and the M2b block rules (M2b-2/3);
  Enter over a selection is one transaction; `replace_doc` records reload / source edits as one
  undoable transaction of the minimal diff (M2b-9); `StateId` dirty tracking, undo to the saved
  state reads clean.
- **Commands and keys**: caret, selection, word-delete, delete-to-line-start, select-all and host
  (`file.save`, `file.open`, `view.sourceMode`, `view.toggleSidebar`) commands in the registry
  (`Run::Caret` needs a `CaretGeometry`, `Run::Host` is performed by the app);
  `editor::keymap` parses the descriptors' per-platform chords (Cmd→Ctrl rule of §3.2) into a
  chord → command map; the egui surface routes key events through it. Edit and Paragraph menus
  in the egui menu bar list registry commands with their chords and enabled state (native menus:
  `native-menus` branch, 99 NM-1…NM-4: one menu model from the registry, shown natively on macOS and
  Windows and as the egui menu bar on Linux). Heading/paragraph styles (Cmd+0…6) come from the Markdown planners. Formatting
  toggles (Cmd+B/I/…) are not wired yet: they must follow spec 02 §3.3 and C49 exactly (the
  toolbar of spec 04 §3.2 calls the same commands) and are left to M2c.
- **Markdown planners**: Enter splits a paragraph with style preservation (S-3.1: spans crossing
  the split are closed and reopened, whitespace at the split dropped, a soft line break at the
  split replaced), splits a heading into heading + paragraph (C7), creates a sibling list item
  from an item's first paragraph (M2b-4), inserts a line break with the container prefix in
  code/math/HTML/front matter, does nothing in tables; at the end of a paragraph or heading it
  puts the caret on the virtual empty paragraph, which typing materialises (M2b-1).
- **UI** (`markise-ui-egui::viewer`): the rich view is the editor — focus on click, blinking caret
  with a 10 s blink timeout (M2b-5), click / shift-click / drag / double-click (word) /
  triple-click (block) selection with drag auto-scroll, caret follow, Cmd/Ctrl+click opens links
  (plain click places the caret, spec 02 §3.2), plain-text copy/cut/paste placeholder (M2b-7).
  App lifecycle: the session is the authoritative text in the rich view; source mode keeps its
  buffer + rebuild worker, and the rebuilt document is installed into the session as a "Source
  Edit" step; saving writes the session's hint-free snapshot (or a source buffer through the
  format-neutral `SaveText::Buffer` path); reload is an undoable transaction. Save and rebuild
  jobs no longer need the concrete Markdown type (C-13 partly resolved).
- **Measured** (`cargo bench -p markise-engine --bench typing`, headless G3: command → session
  edit → model/view update → focus → caret follow → viewport layout, CPU test fonts; Apple
  M-series under heavy parallel load (load average ~50), so thread CPU time is reported next to
  wall time; after merging G2). 263-keystroke script (letters, Backspace, Enter, caret moves,
  Undo) in the middle of the corpus:

  | corpus | p50 | p99 (wall) | p99 (thread CPU) | max | worst kind (p99) |
  |---|---|---|---|---|---|
  | 1 MB | 0.07 ms | 0.43 ms | 0.31 ms | 0.91 ms | letter 0.44 ms |
  | 10 MB | 0.14 ms | 2.0 ms | 2.0 ms | 3.4 ms | letter 2.5 ms; Enter 1.3 ms |

  Budget: G3 headless p99 ≤ 12 ms (NFR-4 16 ms keystroke-to-pixel): met with a wide margin. No
  document copies during the script. Earlier runs under higher load showed wall-clock outliers up
  to ~90 ms with sub-millisecond thread CPU (scheduling, not work). Paint,
  tessellation and present are not included; the desktop `MARKISE_TIMING=1` log now prints
  "keystroke handle+layout+paint" per input frame.
- **Not in M2b** (→ M2c and later): input-rule conversions beyond the M2a plumbing, the remaining
  Enter/Backspace/Tab rules of spec 02 §5, formatting commands, `keyBinding` overrides, IME
  beyond commits (§6.3: M2b inserts `ImeEvent::Commit` as one transaction and places the
  candidate window at the caret; preedit display, suppression and the real-IME gate are M2e), clipboard clipping, reveal in table cells, image
  source reveal, C53 (arrow past the last block), typewriter/focus modes.

## 15. Implementation notes: M2c — smart editing in the rich view (2026-10-28)

Branch `m2c` (PR #35). What landed against the M2c row of §8.1 (decisions numbered M2c-n, listed
below and proposed for `99-open-questions.md`):

- **Planners** (`markise-md/src/edit/`): `inline_fmt` (format commands), `rules` (typing
  pipeline: auto-pair, smart punctuation, hints, immediate rules), `blocks` (Enter / Backspace /
  Delete / Tab / Shift+Enter for lists, quotes, headings; C11 renumbering), `select` (§10),
  `structure` (Move Row, quote/list toggles, Horizontal Line, quote Backspace). All return
  minimal patch sets on the marker lines; untouched bytes stay identical. `Plan` gained a
  `label` (key planners: `Some` = own structural transaction, `None` = a keystroke of the typing
  group); `PlanRequest` carries `EditPrefs`; `EditBehavior::plan_checked` gives menu/toolbar check
  state; the engine asks the format's `edit.deleteBackward` / `edit.deleteForward` / `edit.softBreak`
  / Tab planners before its generic M2b rules.
- **Hints (§2.6) refined**: a `LiteralHint` has a column (`col`): the container prefix before it
  keeps its meaning and no block opens at or after it, so rules typed inside quotes and list items
  keep their containers (M2c-1). Hinted lines also suppress task boxes, alert tags and link
  reference definitions. A second origin `ForceBlock` makes an empty list item nested by Tab under
  a paragraph (CommonMark reads `  - ` there as a setext underline or paragraph text) an item while
  the caret is in it; left empty, its line is removed on expiry (M2c-2).
- **Virtual paragraph levels**: `DocPos.virt = 1 + n` puts the virtual paragraph after the leaf
  *outside* its `n` innermost containers (a list counts as list + item). Enter on the empty last item
  of a top-level list removes it and puts the caret there (S-5.1); Enter on a virtual paragraph moves
  it out of one more container (an empty paragraph in a quote leaves the quote). Materialising writes
  the outer containers' prefix; the layout draws the caret at that indentation (`ItemFocus::
  virtual_exit`) (M2c-3).
- **Empty items** get a caret anchor (the flatten `Empty` span runs to the end of the marker line);
  an empty document draws its caret at the column top.
- **Settings**: `editing` (auto-pair brackets/Markdown syntax, smart punctuation, dash style; Edit
  menu) and `keyBinding` (label or id → accelerator, `""` unbinds; conflicts/unknown names logged,
  the later entry wins) in `settings.json` (§3.3; spec 05 §4.7 names `conf.user.json` — the same
  object is read from the Markise settings file until an advanced-settings file exists).
- **Menus**: Format menu (check marks), Paragraph menu (heading level, quote/list toggles, task,
  indent/outdent, Horizontal Line), Edit menu (selection/delete/move commands, editing prefs).
- **Fixed on the way**: MD-INC-1 — the nested incremental restart rebuilt a list opened on a task
  item's marker line without consuming the task box (property test at 30 000 cases).
- **Tests**: Markdown planner tests (`markise-md/tests/edit_format.rs`), engine surface suites with
  caret-drawable, on-stops and incremental == full checks after every key (`lists.rs`,
  `input_rules.rs`, `structure.rs`), the property test extended with the new commands and rule
  triggers, hint property test with column / ForceBlock hints, and an `egui_kittest` end-to-end list
  suite driving the real app with platform-like key events (`markise-ui-egui/tests/lists_e2e.rs`).
- **Measured** (`cargo bench -p markise-engine --bench typing`, script extended with bullet rule,
  list Enter/Tab/Shift+Tab/exit, heading rule, auto-pair, Select Word + Strong; headless G3, CPU test
  fonts, Apple M-series under parallel load):

  | corpus | p50 | p99 (thread CPU) | max | worst kind |
  |---|---|---|---|---|
  | 1 MB | 0.07 ms | 0.40 ms | 0.99 ms | list p99 0.67 ms |
  | 10 MB | 0.15 ms | 1.39 ms | 2.24 ms | rule max 2.2 ms |

  Budget 12 ms (NFR-4 16 ms keystroke-to-pixel): met for typing; Enter with renumbering in a very
  long ordered list is a known exception (MD-PERF-1 below).
- **Decisions** (M2c-n, proposed for `99-open-questions.md`):
  - M2c-1 hint column; M2c-2 ForceBlock for empty nested items; M2c-3 virtual paragraph levels.
  - M2c-4: Enter on an empty first item of a longer list removes it and opens the virtual paragraph
    after the block before the list; with nothing before the list Enter does nothing.
  - M2c-5: Tab on the first item / Shift+Tab on a top-level item: no change (no literal tab); Tab
    elsewhere inserts a tab except at a block's text start or a line start (it would open indented
    code).
  - M2c-6: a format command at a caret inside an empty pair of its markers removes the pair; a
    selection cutting a styled span is widened to the whole span; same-style spans inside or
    adjacent merge; Hyperlink: caret in `[]` with no selection, in `()` after wrapping, a selection
    inside a link removes it; Clear Format removes emphasis, strong, underline, strike, highlight,
    sub/superscript and code.
  - M2c-7: Enter on a hinted heading / rule / `[toc]` / footnote definition drops the hint and puts
    the caret on the virtual paragraph (no blank line written); C43 removes an empty heading with its
    separator.
  - M2c-8: callout rule: completing `[!KIND]` on a quote's first line (also by stepping over an
    auto-paired `]`) converts and opens a virtual body paragraph.
  - M2c-9: smart quotes use “ ” ‘ ’ until the OS preference is read; dashes never convert where the
    line holds only dashes/markers so far.
  - M2c-10: word / line deletes at an item boundary fall back to Backspace / Delete and their rules.
  - M2c-11: Shift+Enter at the very end of a paragraph acts as Enter (an empty trailing soft line
    cannot be stored); Shift+Enter is its own transaction.
  - M2c-12: Move Row keeps numbers and bullets with the positions; top-level blocks swap whole.
- **Codex review of PR #35 (fixed):** Clear Format removes only parsed `<u>` nodes (literal code
  content survives); a marker whose width changes (renumbering `9.`→`10.`, list-kind toggle, Move
  Row, outdent into an ordered list) moves the item's continuation lines with the content column,
  and Enter at the end of an item with children inserts the new item after the whole item (no child
  detached; property test over 8–11 and 97–101 item lists); leaving a list from after a nested item
  exits relative to the actual anchor leaf; typing looks up only the caret's item (no sibling
  scan); leaving the middle of a quote writes the C45 separator at once; C48 Delete before a rule,
  a quote's or a list item's first paragraph; host shortcuts (Save, Open…, Source Code Mode,
  Toggle Sidebar) go through the effective keymap (`keyBinding` rebinds/unbinds them, another
  command may take their chord); unknown key names in `keyBinding` are reported and ignored.
  Round 2: Move Row maps a caret on a continuation line line by line (Unicode-safe); the engine
  snaps planner selections to char boundaries (debug builds assert); C48 merges drop the next
  block's heading syntax and keep this block's (setext underline, ATX closing), also in quotes and
  list items.
- **Known exception to NFR-4 (MD-PERF-1, issue #37; beta follow-up):** Enter or Backspace that renumbers a long sequential
  ordered list re-parses the list from the edit to its last renumbered marker. Typing bench in one
  2 MB ordered list: letters p99 0.64 ms, but Enter (split + renumbering ~15 000 following items)
  p50 27 ms / max 43 ms — over the 12 ms budget. Follow-up: a marker-renumbering fast path in
  `markise-md` (digits-only marker patches update numbers and spans without a re-parse).
- **Not in M2c** (→ M2d/M2e/later): ordered task items (MT-1), table/code/math/front-matter Enter
  rules (S-2.4–S-2.6), the Cmd+K clipboard prefill (needs clipboard reading), C47 image atomic
  deletion, C53, C54, the formatting toolbar (spec 04 §3.2, P1), Select Line by sentence, in-app
  key-binding editor, OS quote preference, source-mode rules (M2d).

## 16. Implementation notes: M2d slice 1 — the source editor core (2026-10-28)

Branch `m2d-source-editor`. Full Source mode and the left pane of Side by Side are a source
editor on the editing engine; the `TextEdit` source path, its 2 MB edit limit (M1-4), the
128 KB colouring cap and the source-buffer rebuild/install machinery are gone. Decisions are
numbered M2d-n (proposed for `99-open-questions.md`).

- **M2d-1 — a sibling of `View`, not `flatten_source`.** §5.5 planned the source view as the
  rendered `View` over a "source flatten" of the model. `View` is built around rendered items
  (containers, segments, reveal focus, block-level cache keys), none of which a code view
  needs, so the source view is its own type, `markise-layout::source::SourceView`, reusing the
  same pieces: `Fenwick` heights with an anchor scroll position, lazy layout of the viewport
  plus a margin, the backend-neutral `TextShaper`, item-relative `SourceMap`s and the
  `Highlights` seam. It is format-neutral (it reads the text and `FormatDoc::source_tokens`).
- **M2d-2 — items.** An item is a run of physical lines (≤ 64 lines, ≈ 16 KiB); a line longer
  than 4 KiB is cut into pieces of its own items (a piece starts a new visual row), so a file
  without line breaks still lays out piece by piece. Per-item byte and newline counts live in
  two more Fenwick trees: line ↔ offset, gutter numbers and scrolling to the end need no walk
  over the document. An edit re-chunks only the items it touches (a small last item merges
  with the next); chunk boundaries are an implementation detail, so "incremental == full
  rebuild" is defined on what users see — the physical lines, the line ↔ offset mapping and
  the shaped text and styles of every line (property test `markise-layout/tests/source_view.rs`).
  Tab stops are measured from the start of the line, or of the piece of an over-long line.
- **M2d-3 — layout per line, cached by content.** Each unit is shaped on its own; shaped lines
  are cached by (text, token runs, code runs, wrap width, tab width, theme, fonts), so a
  keystroke re-shapes only the edited line. Item layouts far from the viewport are dropped
  (bounded memory); their heights stay exact.
- **M2d-4 — tokens from the parse.** `FormatDoc::source_tokens(range)` (new, `markise-doc`)
  reports neutral classes (`SourceTok`: headings 1–6, quote, emphasis, strong, strike, mark,
  link, URL, code, code block, metadata, marker); Markdown implements it from the CST,
  visiting only the top-level blocks — and inside large containers, code blocks and tables
  only the children and lines — overlapping the range. A paragraph over 64 KiB is parsed
  inline line by line, and a single line over 64 KiB only in a 4 KiB window around the range.
  Fenced code takes the language colours of the rendered view's background highlighter:
  the source view asks the rendered model's code items (`CodeQuery`) and maps their content
  offsets back to source bytes; a pending highlight re-lays out the item when it arrives.
  Invalidation: items in the edit's re-parsed range lose their tokens; a change of reference
  definitions (or any document-wide dependency) re-tokenises every item lazily.
- **M2d-5 — history.** Side by Side edits are ordinary transactions of the live history (typing
  groups as in the rendered view). Full Source mode opens a *source session* on the
  `EditorSession`: undo stops at the entry state, the rendered view's redo stack is set aside;
  on exit the session's transactions are concatenated into "Source Edit" transactions split
  after every state that was ever saved (§4.3: undo reaches every saved state, which reads
  clean). A session without edits leaves the history and the set-aside redo stack untouched.
  Source edits update the rendered model incrementally too (no rebuild on leaving). A reload
  from disk in source mode restarts the session around the reload transaction.
- **M2d-6 — verbatim edits (C58).** Typing, paste and IME commits insert the bytes as given;
  Backspace/Delete remove graphemes (CRLF is one), words, or up to the visual row start. A
  typed line break repeats the terminator of the line it splits (CRLF on a CRLF line), the last
  line without one takes the document's line ending; in a uniformly CRLF document this is C58's
  rule, in a mixed one it differs (C58 kept typed breaks as typed — see the open questions).
  Tab inserts the indent unit (4 spaces) or, over several lines, indents every touched line;
  Shift+Tab outdents touched lines (list-aware rules are slice 2).
- **M2d-7 — commands and focus.** The source pane keeps its own selection and runs caret and
  selection commands through the shared registry with the source view as `CaretGeometry`;
  Undo/Redo use the session; format and block commands are disabled while the source pane has
  the focus (menus, toolbar and shortcuts; slice 2 adds them). In Side by Side the pane with
  the keyboard focus receives menu commands; the source pane takes the focus when Source or
  Side by Side opens. The formatting toolbar stays hidden in full Source mode and inactive over
  the read-only rendered pane, honouring the top/bottom reservation.
- **M2d-8 — display preferences.** Settings key `source` (`line_numbers` false, `soft_wrap`
  true, `current_line` true, `whitespace_marks` false), View → Source Editor. Line numbers are
  a gutter numbering every line; whitespace marks show `·` for spaces and an arrow for tabs,
  and a faint return arrow after two or more trailing spaces before a line break is always
  shown (SE-2), all at paint time; the
  current-line band covers the caret's physical line (or piece). Without soft wrap the view
  scrolls horizontally (wheel / caret follow; no horizontal scrollbar yet).
- **M2d-9 — position mapping.** Entering source mode centres the source line of the block at
  the rendered viewport's middle (plus its share of a tall block's lines) and collapses the
  selection to its focus; leaving centres the block holding the source viewport's middle line
  and gives the rendered view the source caret (moved to its nearest caret stop). Outline
  clicks in source mode scroll the source to the heading's line. Side by Side scroll sync maps
  through the source view's line index (`side_by_side::LineMap`).
- **Selections (review of PR #23).** The source pane's selection follows every document change
  made elsewhere — mapped through the change summary for rendered-pane edits and menu
  Undo/Redo, through the diff (session live marks) for a reload — and every selection the pane
  takes is snapped to a caret position (char boundary, not inside a CRLF). Grapheme steps widen
  their segmentation context until the boundary is real, so long clusters are never split.
- **Owner decisions SE-1…SE-10** (private spec): M2d-6 line endings kept (SE-1); the hard-break
  mark is always shown faintly, spaces/tabs only with Show Whitespace (SE-2); the indent unit
  becomes the `codeIndentSize` preference in slice 2 (SE-4); smart Home in slice 2, read-only
  mode and IME preedit in M2e (SE-10).
- **Removed safety net.** The interim Side by Side caught a panicking incremental update and
  fell back to a background rebuild of the buffer. With the session as the only text there is
  no buffer to rebuild from; source edits now behave like rendered-view edits (no catch).
- **Tests.** `markise-md` source tokens (range == clipped document), `markise-layout` source
  view unit and property tests (incremental == full, 10 MB locality), `markise-engine/tests/
  source.rs` (verbatim typing, CRLF, caret commands, Tab, disabled format commands, source
  session collapse with saved boundaries, Side by Side live history, 300 random keystrokes
  with incremental == full checks of both views), real-app kittests
  `markise-ui-egui/tests/source_editor.rs` (typing and saved bytes, keyboard/mouse selection,
  wrap and gutter toggles, undo across mode switches, mapping in/out, 10 MB open/scroll/edit)
  and the ported Side by Side, lifecycle, safety and smoke suites.
- **Measured** (release, Apple M-series under heavy parallel load — load average ≈ 115 on 10
  cores, so thread CPU is the signal):

  | bench | case | p50 | p99 (thread CPU) | budget |
  |---|---|---|---|---|
  | `markise-engine --bench source` (headless, wrap on) | type 1 MB | 0.21 ms | 0.68 ms | 12 ms |
  | | type 10 MB | 0.21 ms | 0.97 ms | 12 ms |
  | | source view build 10 MB / first viewport | 6.2 ms / 4.6 ms | | |
  | | scroll to end with wrap, 10 MB | 12 ms (wall) | | 16 ms |
  | | one 10 MB line, typing | 8.5 ms | 12.7 ms | 12 ms (exceeded) |
  | `markise-ui-egui --bench source_editor` (whole frame, wrap + gutter) | Source 10 MB | 0.77 ms | 1.80 ms | 16 ms |
  | | Side by Side 10 MB | 0.92 ms | 2.05 ms | 16 ms |
  | | open 10 MB → first source frame | 184 ms + 71 ms | | 1 s |

  Known exception: typing in a single 10 MB line costs ≈ 8 ms per keystroke (the rendered
  model's handling of one huge paragraph), p99 just over the headless budget; ordinary
  documents stay far below it.
- **Not in slice 1** (→ slice 2: source-mode smart editing): list/quote continuation, list-aware
  Tab/Shift+Tab, renumbering, auto-pair and tag closing, marker Backspace, format commands and
  the toolbar in the source pane, URL-over-selection paste, table navigation and formatting,
  matching brackets, Move Line, Delete Line, smart Home; (→ slice 3) rich-view tables and
  special blocks with both Side by Side panes editable; later: IME preedit display, multiple
  carets, folding, a horizontal scrollbar.

## Consequences

- One new L1 crate (`markise-doc`) and one Testkit crate (`markise-testfmt`); new third-party
  dependencies: `unicode-segmentation` (M2b), `regex`/`regex-syntax` (M2e), `syntect` with `fancy-regex` (M2d, decided Q13; syntax-set licenses in `NOTICE`),
  vendored clipboard code with a `NOTICE` entry (M2e).
- `markise-layout` becomes format-agnostic; the Markdown → view mapping, inline rendering input,
  literal-line hints, reference-lookup reporting and line signatures live in `markise-md`. A future
  `markise-org` implements `DocumentFormat`/`FormatDoc` without touching layout, caret, commands or
  undo.
- `ops` signatures change to planners; their tests carry over.
- ADR 0002's lifecycle is amended (revision split, editing on the UI thread, async reparse fallback);
  ADR 0005 gets the item-relative `FragmentId` note; its IME gate is unchanged.
- Risks: the keystroke budget is a hypothesis until G1–G3; literal-line hints add parser state that
  must stay consistent with incremental reparse (covered by the incremental == full property tests
  with hints).
