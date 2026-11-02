# ADR 0016: paged view model

Status: accepted for issue #125 (owner decision 2026-11-02: before the beta). Phase 1 of the
production work; the prototype and its measurements are on branch `proto/lazy-view`
(`docs/research/lazy-view-prototype.md` there).

## Context

The view model ([`DocModel`], `crates/markise-layout/src/model.rs`) held one item per leaf
block (or segment of one) of the whole document: about 2 M items on the 100 MiB mixed corpus,
most of the memory after open beside the text and the parsed document, and O(items) work in
places that walk every item. Only the items near the viewport are ever laid out.

## Decision

**Placeholders in the global item list.** A paged model holds one placeholder item
(`ItemKind::Page`) per page of `PAGE_TOPS` = 128 top-level blocks; only pages near the
viewport and the caret are *materialized* (replaced by their real items), pages far away are
*evicted* back to a placeholder. Materialization and eviction are ordinary splices reported as
`ModelChange`s, so the existing global item-index contract stays: the view, caret geometry,
the editor, the viewer and their tests keep working with item indices (a placeholder is one
item). A placeholder keeps its source bounds in the same chunked relative-offset list as real
items (edits above it shift it in O(chunks)), aggregate statistics for a height estimate
(`PageStats`), and the measured height of its items when it was evicted. The outline stays
complete; entries of headings inside a placeholder point at the placeholder.

The reviewer's original design (a separate `PagedView` API with source-position anchors) was
not followed: the item-index contract is used by caret geometry, editing, the viewer, comments,
Side by Side, code tools and many tests, and the prototype showed that placeholders as items
are enough (indices change only by reported splices; anchors are transformed through them).
Code that needs a position that survives paging uses source positions
(`View::source_anchor` / `View::scroll_to_source`, `View::ensure_offset`,
`View::ensure_heading`).

**Invariant.** Every real item of a paged model equals the item a full build has at the same
source position; placeholders cover exactly the full build's items between their neighbours;
the outline and the `[toc]` runs agree (`DocModel::check_against_full`, differential tests
after random edits, materializations and evictions: `crates/markise-layout/tests/paging.rs`,
editing at page edges against a full model: `crates/markise-engine/tests/paging.rs`).

**Who decides what is materialized.** The view, before each frame (`view/paging.rs`): the
first and the last page stay (document edges; a document of at most two pages is a full
model), the caret's page and the items around it stay, placeholders within one viewport above
and two below are materialized, runs of items more than four viewports away are evicted in
chunks of at most a page of whole top-level blocks (not ones holding the caret, the focused
item or a disclosure marker paired outside the chunk). Materializing the page under the scroll
anchor keeps the anchor's relative position; pages above the anchor only shift indices, so the
content on screen never moves. Jumps (outline, anchors, wikilink headings, find, Side by Side,
comments, history) materialize their target first. While the scrollbar is dragged its scale is
frozen; its bottom always means the end of the document.

**Edits.** `DocModel::apply` is unchanged for edits inside real items (the caret's page is
always real). An edit whose re-parsed range reaches a placeholder (an unclosed fence, Replace
All, a large undo) rebuilds the paged model (one streaming pass, about 0.3 s on 100 MiB on top
of the parse that happens anyway); phase 5 re-pages only the affected region.

**Consumers that walked every item** must not: they see placeholders (`Item::is_page`). The
word count now counts the document's top-level blocks from the edits' summaries
(`EditorSession::take_log`, `markise-engine/src/count.rs`), independent of the view model;
Source mode's code highlighting reads a placeholder's code items without materializing it
(`DocModel::page_items`).

**Switch.** Paging is the default. `MARKISE_FULL_VIEW=1` builds full models (comparison
during the transition); `MARKISE_PAGE_TOPS=n` sets the page size (the suites can run with
tiny pages). A format without `FormatDoc::top_span` / `top_at` gets full models.

## Known limits (later phases of #125)

- Selections over more than 4,096 blocks are estimated from their byte share (as before);
  page totals make them exact (phase 2).
- `<details>` pairs are indexed over real items only: a disclosure whose markers are in
  different pages is paired once both are materialized (phase 3: an index on source
  positions).
- Side by Side keeps item targets within one frame; positions between pages are interpolated
  per item (phase 4: offset-based targets).
- Heading texts of placeholders are not refreshed when a reference definition changes; a
  placeholder's height estimate does not follow outline count changes until it is
  re-estimated; materializing a page holding repeated `[toc]`s re-hashes the outline per
  segment (phase 5).
