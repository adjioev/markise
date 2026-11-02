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
chunks of at most a page of whole top-level blocks (not ones holding the caret, a selection
end or the focused item). Materializing the page under the scroll
anchor keeps the anchor's relative position; pages above the anchor only shift indices, so the
content on screen never moves. Jumps (outline, anchors, wikilink headings, find, Side by Side,
comments, history) materialize their target first. While the scrollbar is dragged its scale is
frozen; its bottom always means the end of the document.

**Edits.** `DocModel::apply` is unchanged for edits inside real items (the caret's page is
always real). An edit whose re-parsed range reaches a placeholder (an unclosed fence, Replace
All, a large undo far from the view), or that inserts more blocks than two pages hold (a large
paste), re-pages only the region it reaches (`DocModel::apply_region`, phase 5): the items and
placeholders the re-parsed blocks overlap (whole placeholders) are replaced by fresh
placeholders for the region's top-level blocks, built a page at a time; everything after
shifts in O(chunks). The region's outline entries and disclosure markers are replaced (the
anchors of affected keys renumbered), real `[toc]` blocks re-keyed, and the view takes the
result as a new model that keeps its scroll position through the edit (`ModelChange::moved`);
it then materializes the pages near the viewport and the caret as usual. Fresh pages hold at
most a page of blocks, so pages never grow without bound; a page left smaller by an edit stays
smaller until it is evicted and re-cut. An edit whose region spans more than half of the
document (Replace All), or one the format reports as changing everything (`DepChange::all`),
rebuilds the model in one streaming pass instead (cheaper for that much). When the region's
outline change re-segments real `[toc]`s or the resource root changed, the view takes a new
model; otherwise it splices the region like any edit. `[toc]` segment keys are cached until the outline
changes, so materializing pages holding repeated TOCs no longer re-hashes the outline.

**Consumers that walked every item** must not: they see placeholders (`Item::is_page`). The
word count now counts the document's top-level blocks from the edits' summaries
(`EditorSession::take_log`, `markise-engine/src/count.rs`), independent of the view model;
Source mode's code highlighting reads a placeholder's code items without materializing it
(`DocModel::page_items`).

**Switch.** Paging is the default. `MARKISE_FULL_VIEW=1` builds full models (comparison
during the transition); `MARKISE_PAGE_TOPS=n` sets the page size (the suites can run with
tiny pages). A format without `FormatDoc::top_span` / `top_at` gets full models.

**Cross-block facts (phase 3).** Most multi-line constructs (lists, quotes, callouts,
tables, code, math, front matter, HTML blocks) belong to one parsed top-level block, which a
page never splits. What does cross top-level blocks is kept document-wide in
`CrossBlockIndex` (`model/cross.rs`), built during the model's streaming pass and kept in step
by `DocModel::apply` from the replaced items of each edit:

- `<details>` / `<summary>` / `</details>` written as separate raw blocks: every marker with its
  container scope, paired document-wide (nested, quoted, malformed and separate-summary
  disclosures as a full model pairs them; a summary counts only as the next block in the
  same container). Markers live in a chunked relative-offset list and groups refer to them
  by index: an edit touching no marker (ordinary typing) only shifts them in O(chunks), and
  the view's derived state follows the splices without a derivation. The
  reader's open/closed choice lives on the start marker, so it survives eviction, edits
  elsewhere and a re-parse of its container; a full rebuild carries it over for markers
  outside the edit. The view derives
  its item-index state (hidden ranges, controls, titles, depths) from the index after every
  change; a placeholder wholly inside a closed disclosure has zero height and is not
  materialized, one holding a start or end counts as visible until it is materialized.
- Dependency generations of the document-level inputs: the outline (`[toc]` content and
  length), the resource root (images), reference definitions and the disclosures' pairing
  and state (a page measured while a disclosure in it was closed). A placeholder's measured
  height records the generations; it is used only while those its blocks depend on are
  unchanged, and the view re-estimates placeholders when a generation changes.

Headings, duplicate slugs and anchors are the outline (complete in every model); reference
definitions (first one wins, also inside containers) and footnote labels are resolved by the
format's document-wide context; heading texts in placeholders are refreshed from their pages
when a definition they may use changes (`PageStats::heading_refs`). Comments' presence scans
the document, independent of items; CriticMarkup does not cross blocks (ADR 0012).

**No item index survives a frame (phase 4).** Whatever is kept across frames names an item by
source position (its start and its ordinal among the items starting there): the scroll
position smooth scrolling follows (`View::source_anchor`), the last frame's visible items used
for hit tests, a pressed task checkbox, navigation history, Side by Side targets
(`Target::At { start, nth, frac }`, the counterpart's page materialized before scrolling to
it). Comment cards come from the visible items' layouts every frame; comment jumps go through
the caret (its reach is materialized); the gutter's presence is the document scanner's.
Copy and cut take the session's text of the selection, independent of items. The word count
sums a selection's fully selected middle exactly from cached totals of runs of blocks.

## Known limits

- A placeholder partly inside a closed disclosure is estimated as if all of it were shown
  until it is materialized (it is near the viewport whenever that matters: closed content has
  no height).
- There is no rich (HTML) clipboard yet; copy is source text (not a paging limit).
