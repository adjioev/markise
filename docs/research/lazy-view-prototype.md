# Lazy ("paged") view model — prototype report (issue #29)

Branch `proto/lazy-view` (throwaway prototype, not for merge), 2026-11-02. Question: is a lazy
view model feasible for 100 MB documents, and what does it buy? Short answer: **yes, and it
pays off — go**, with a production estimate of 4–6 weeks.

## What was built

Switch: `MARKISE_LAZY_VIEW=1` (read once at model build). Without it nothing changes (all
workspace tests pass in normal mode).

- `DocModel` in paged mode (`crates/markise-layout/src/model/paged.rs`) holds **one placeholder
  item (`ItemKind::Page`) per page of 128 top-level blocks**, built by one streaming flatten
  pass that keeps no items. A placeholder lives in the same `RelChunks` list as real items, so
  its source bounds shift with edits for free (the design's `ProjectionIndex`), and carries
  aggregate statistics for a height estimate plus the measured height of its items when it was
  evicted. The outline stays complete (headings inside a placeholder point at it).
- `View` (`view.rs`): each frame first evicts runs of items more than 4 viewports away (cut at
  top-level block boundaries, ≤ 128 blocks per page, never the first/last page or the caret's
  page), then materializes every placeholder within one viewport above / two below. One height
  per placeholder sits in the existing Fenwick tree; materializing the page under the anchor
  keeps the anchor's relative position in the page; pages above the anchor only shift indices.
- Materialize/evict are ordinary `ModelChange` splices, so the view, caret geometry, editor and
  viewer keep working with global item indices. Caret moves (`EditorView::sync`,
  `refresh_focus`, `scroll_to_caret`) materialize the caret's page and its neighbours.
- Jumps: outline click (`View::ensure_heading`), anchor links, find next, Cmd+Up/Down.
- `[TOC]`: only exists in materialized pages (segmented and keyed there), so the 2,841 repeated
  TOCs of the mixed corpus cost nothing while off screen.
- Edits use the normal incremental `DocModel::apply`; an edit whose range touches a placeholder
  (rare: an unclosed fence re-parsing to the end, replace-all, a large undo) rebuilds the paged
  model (~0.3 s on 100 MB, on top of the parse that happens anyway).
- Scrollbar: its scale is frozen while dragging.
- `FormatDoc::top_at(offset)` (default `None`, Markdown implements it) maps a source position
  to its top-level block.

Deviation from the reviewer's design (`PagedView` with a new API and source-position anchors):
the global item-index contract is used by caret geometry, editor, viewer, comments, Side by
Side, code tools and tests; replacing it was not doable in a few days. Keeping it, with
placeholders as items, let the whole app run unchanged and turned out to be enough: indices
change only by reported splices, and the index anchor is transformed through them. Everything
else follows the design (128-block pages, `RelChunks` bounds, one estimate per page refined by
layout, materialize the target before a jump, bounded projection memory).

## Numbers (normal vs lazy)

Headless real app (`benches/lazy.rs`), release, M-series Mac, fresh process per run, `main`
3f42af7 (before the #119 memory compaction). Corpora: `markise-corpus --huge` (100 MiB each),
plus `mixed-1toc` = mixed with all but the first `[TOC]` turned into a paragraph. "Steady" =
after the first jumps (the loader's transient ~90 MB is gone by then). Live = counting
allocator; footprint = macOS physical footprint.

| | mixed (2,841 TOCs) | mixed, 1 TOC | prose | log |
|---|---|---|---|---|
| Items in the model | 1,989,079 → **4,561** | 757,288 → 4,461 | 549,215 → 4,319 | 84,131 → 697 |
| Live heap, steady | 1,047 → **476 MB** | 705 → 476 MB | 386 → 235 MB | 202 → 179 MB |
| Footprint, steady | 1,194 → 693 MB | 935 → 691 MB | 701 → 436 MB | 323 → 293 MB |
| Live / footprint after traversing the whole file | 1,125 / 1,256 → **536 / 582 MB** | 799 / 1,057 → 575 / 642 MB | 445 / 770 → 294 / 360 MB | 221 / 349 → 196 / 320 MB |
| RSS after traversal | 1,373 → 643 MB | 1,075 → 681 MB | 788 → 398 MB | 364 → 336 MB |
| Open → first painted frame | 1,232 → **743 ms** | 902 → 753 ms | 626 → 602 ms | 301 → 295 ms |
| Install + first frame (UI thread) | 332 → 6 ms | 111 → 8 ms | 86 → 6 ms | 15 → 5 ms |
| Wheel scroll p50 / p99, cold | 2.4 / 3.9 → 0.5 / 2.3 ms | 2.4 / 4.9 → 0.6 / 1.3 ms | 2.3 / 2.9 → 0.7 / 1.8 ms | 2.1 / 3.0 → 0.9 / 1.9 ms |
| Wheel scroll p50 / p99, warm | 2.3 / 4.6 → 0.5 / 1.5 ms | 2.3 / 3.6 → 0.4 / 0.8 ms | 2.1 / 2.5 → 0.7 / 2.7 ms | 2.2 / 4.3 → 0.8 / 3.6 ms |
| Scrollbar click jump p50 / max | 6.9 / 12.9 → 6.2 / 8.1 ms | 3.8 / 5.3 → 2.8 / 5.4 ms | 3.8 / 4.8 → 2.6 / 5.2 ms | 3.0 / 3.3 → 3.0 / 3.5 ms |
| Outline jump p50 / max (cold) | 5.3 / 5.8 → 4.2 / 5.2 ms | 4.8 / 5.3 → 3.8 / 3.9 ms | 4.2 / 7.0 → 5.6 / 8.2 ms | 3.0 / 3.6 → 2.4 / 3.0 ms |
| Find-next jump p50 / max (cold) | 4.5 / 8.0 → 3.1 / 6.3 ms | 4.5 / 8.7 → 3.2 / 6.7 ms | 4.8 / 4.9 → 3.2 / 4.6 ms | 2.1 / 3.0 → 0.7 / 2.5 ms |
| Cmd+Down/Up p50 / max | 4.1 / 4.4 → 1.2 / 1.7 ms | 3.9 / 5.8 → 1.2 / 2.0 ms | 3.6 / 4.0 → 1.4 / 2.7 ms | 2.7 / 2.8 → 1.3 / 2.2 ms |
| Typing (letters/BS/Enter) p50 / p99 | 2.2 / 2.6 → 0.35 / 0.66 ms | 2.3 / 3.1 → 0.37 / 0.81 ms | 2.2 / 8.3 → 0.51 / 1.4 ms | 1.9 / 2.8 → 0.37 / 0.61 ms |
| Backspace/Enter at block starts p99 | 2.9 → 1.2 ms | 2.8 → 1.2 ms | 4.5 → 2.4 ms | 5.0 → 0.7 ms |
| Traversal frame p99 / max (3,000 scrollbar steps) | 6.8 / 14 → 7.2 / 51 ms | 5.6 / 12.5 → 4.8 / 9.4 ms | 9.1 / 34 → 7.4 / 18 ms | 5.0 / 50 → 3.4 / 10 ms |
| Scrollbar thumb drift after a jump (max) | 0.58 → 0.00 px | 0 → 0 px | 0 → 0.01 px | 0 → 0 px |
| Document height: change after full traversal | +6.2 % | +2.0 % → −5.4 % | +3.6 % → −3.0 % | +0.2 % → −1.2 % |

Further observations:

- **Retained memory after traversal is bounded**: the lazy model ends with 3–7 k items after
  materializing 1.8–3.7 k pages and evicting as many; live heap grows only by what the normal
  mode also grows by (layout cache, egui caches).
- Materialization costs ~1 ms per page on average (2.2 s over 2,273 pages on mixed; the
  51 ms traversal maximum is a frame that materialized pages holding repeated TOCs — each
  re-hashes the 29 k-entry outline for its 454 segments; a cached per-segment key fixes it).
- The incremental update stays correct: after all edits and the traversal, every materialized
  item equals a full build's item at the same position, placeholders cover exactly the full
  model's items, outlines agree (`DocModel::check_paged`, checked on all four corpora).
- Thumb drift is zero because estimates are refined per page and the anchor keeps its content;
  the "max anchor jump" counter (1–6 k px on a 2·10⁹ px document) is the change of the absolute
  scroll position when the page under a scrollbar target materializes — invisible, the page was
  never shown.
- **Finding for normal mode**: its `[TOC]` segment estimate is one line per 64-entry segment,
  so on the repeated-TOC corpus the normal document height is 1.0·10⁸ px against a real
  ~2.1·10⁹ px (20× too short); the scrollbar maps to the wrong content. The lazy page estimate
  counts entries. Small independent fix worth doing on main.
- Caveat on per-frame times: word count is off in lazy mode and was on in normal mode; part
  of the ~2 ms normal-mode baseline per frame is O(items) work outside the view model. The
  memory and open numbers are not affected by this.

What the numbers mean against the reviewer's go gate: bounded projection memory — yes;
≥ 250 MB live reduction on mixed — 571 MB against main (the view model, its heights and the
word-count cache are what disappears; against #119 the saving would be roughly 300–390 MB);
ordinary scroll / typing p99 near 16 ms — all ≤ 3 ms, maxima ≤ 9 ms outside traversal.

Remaining floor on mixed in lazy mode: text 100 MB + parsed document ~300 MB (before #119;
~170 MB with its block-record compaction) + outline and caches. **Lazy view + #119 should land
mixed near 3.4× live, prose near 2×** — the lazy view alone cannot reach 3× footprint for
structured content; the parsed document is the next target.

## Tried by hand

Not done in the GUI: the agent session had no way to drive or watch the desktop app. Feel was
approximated headlessly through the real UI code (scrollbar clicks and drags, wheel, keys,
outline, find, edits) as above. The owner should try it (commands below), in particular:
scrollbar drag across repeated-TOC regions, outline clicks into far sections, typing at page
boundaries.

## Stubbed or approximate in lazy mode

- **Word count**: off (it walks every item).
- **Disclosure (`<details>`)**: the index sees only materialized items; collapsing across
  pages / offscreen state is undefined. Not tested.
- **Side by Side / source ↔ rendered sync, comment jumps, wikilink heading jumps**: they map an
  offset to an item and scroll there; on a placeholder they land at the proportional position
  in the page (the next frame materializes it). Not tested.
- **Rich clipboard / select-all**: untouched (session-based); select-all paints only visible
  items. Not tested.
- **Heading text after a reference-definition change**: not refreshed for headings in
  placeholders.
- **Outline count changes**: placeholders holding TOCs keep their old height estimate until the
  next re-estimate.
- **Hit testing next to items without caret stops** may walk into a placeholder (empty layout,
  harmless; 26 times in the prose run).
- **Edits touching a placeholder**: full paged rebuild instead of an incremental update.

## Production estimate

Keep the placeholder approach (it proved the rest of the app can stay index-based) rather than
a new `PagedView` API with source anchors; ~4–6 weeks for one implementer:

| Work | Estimate |
|---|---|
| Paging on by default (switch only for tests), cleanup of `View` paging code, pin/keep policy tuned (memory vs. re-materialization) | 3–4 d |
| Word count from the document with page aggregates; partial selections count boundary pages | 1 w |
| Disclosure index on source positions (offscreen open/closed ranges, closed ranges contribute zero height) | 1 w |
| Offset-based jumps everywhere (Side by Side, comments, wikilinks, review) via `ensure_offset` | 2–3 d |
| Edits touching placeholders: re-page the affected region instead of a rebuild; outline/TOC re-estimates; cached TOC segment keys | 3–4 d |
| Differential property tests (incremental vs full build, `check_paged`), page-boundary navigation and editing tests, heavy-suite scenario, bench in `benches/` | 1–1.5 w |
| Normal-mode TOC estimate fix (independent, small) | 0.5 d |

Risks: disclosure ranges crossing pages; any future consumer that iterates all items (it would
silently see placeholders — make `Items` iteration explicit about them); behaviour while a
background job holds the model (`Arc::make_mut` would copy it; small in paged mode).

## Recommendation

**Go.** The prototype shows the lazy view is feasible without redesigning the editor: memory
after open drops by 150–570 MB on structured 100 MiB documents and stays bounded after a full
traversal, open-to-first-frame is faster (UI-thread install from 100–330 ms to ~6 ms), and
scrolling, jumps and typing are as fast or faster with a stable scrollbar. Do #119 first (it
is independent and also shrinks the parsed document), then the lazy view as above.

## Running it

```sh
# corpus (~400 MB; delete afterwards)
cargo run --release -p markise-corpus -- /tmp/markise-corpus --huge
# the app, lazy vs normal
MARKISE_LAZY_VIEW=1 cargo run --release -p markise -- /tmp/markise-corpus/huge-mixed.md
cargo run --release -p markise -- /tmp/markise-corpus/huge-mixed.md
# measurements (each file × mode in a fresh process)
MARKISE_BENCH_LAZY=1 MARKISE_LAZY_FILES=/tmp/markise-corpus/huge-mixed.md,/tmp/markise-corpus/huge-prose.md \
  cargo bench -p markise-ui-egui --bench lazy
```

## Follow-ups after the owner tried it (2026-11-02)

### Scrollbar drag did not reach the end (lazy mode)

Reproduced headlessly (`MARKISE_LAZY_END=1` probe in `benches/lazy.rs`); two causes:

1. The pointer was mapped over the whole bar height while the thumb travels only bar − thumb,
   and the grab offset was ignored: with the thumb visually at the bottom the scroll fraction
   was ~0.969 — on the lazy model's correct height (~2.1·10⁹ px on mixed) about 65 M px,
   thousands of screens, before the end. Lazy mode now maps the pointer at its grab point over
   the thumb's travel (a press outside the thumb centres it). **Normal mode has the same bug**
   (fraction 0.969, ~3 M px short on its 1·10⁸ px estimate) and keeps it on this branch; worth
   fixing on main.
2. The scale frozen during a drag: fraction 1.0 mapped to the frozen total − viewport while
   pages loaded during the drag had grown the document (last block ~80 px below the viewport).
   Fraction 1.0 now pins the end.

Wheel, Cmd+Down and End reached the end in both modes.

### Text quality while scrolling (#86) — switches, all modes

| Switch | What it does |
|---|---|
| `MARKISE_SCROLL_SNAP=1` | render at the scroll offset rounded to device pixels, item origins and column edge rounded |
| `MARKISE_SCROLL_RAW=1` | trackpad / momentum wheel deltas applied exactly (egui smooths ≥ 8 pt deltas after `TouchPhase::End` into a delayed tail) |
| `MARKISE_SCROLL_TILES=1` | (implies snap) per-item tessellated meshes cached and composited at the snapped offset |

Simulated 120 Hz trackpad gesture with momentum, ppp 2 (`MARKISE_LAZY_SCROLLQ=1`):

| | mixed normal | mixed lazy | prose normal | prose lazy |
|---|---|---|---|---|
| Line wobble frames (neighbouring items' rendered spacing changed), off → snap | 78 → 0 | 0 → 0 (inside a TOC) | 148 → 0 | 124 → 0 |
| Momentum tracking error, off → raw | 19.4 % → 0 % | 19.4 % → 0 % | 19.4 % → 0 % | 19.4 % → 0 % |
| Frame time mean / sd / p99, snap+raw | 2.30 / 0.32 / 3.9 ms | 0.55 / 0.16 / 1.3 ms | 2.18 / 0.27 / 3.0 ms | 0.42 / 0.09 / 0.8 ms |
| Frames over 8.3 ms (120 Hz) | 0 of 888 | 0 | 0 | 0 |
| Tiles+raw: frame mean / sd, mesh memory | 2.23 / 0.31 ms, 0.1 MB | 0.54 / 0.13 ms, 0.1 MB | 2.24 / 0.43 ms, < 0.1 MB | 0.42 / 0.09 ms, < 0.1 MB |

Epaint rounds each galley to a device pixel on its own, so at fractional offsets neighbouring
lines crossed a pixel boundary on different frames — a 1 px relative wobble; snapping removes
it. Mesh tiles bring no measurable gain: once the offset is snapped, glyph quads from the atlas
already composite pixel-identically, and raster-texture tiles would need a new BlockLayout
rasterizer for the same pixels. Recommendation: snap + raw, no tiles. Headless frame times are
CPU only; real 60/120 Hz pacing and the visual result need checking on the device.

### Progressive open (`MARKISE_PROGRESSIVE_OPEN=1`, lazy view only)

Files ≥ 4 MiB: the loader parses the first ~200 KB (cut at a blank line outside fences / `$$`)
and shows it at once, then parses the whole file and swaps it in without moving the view.
While loading: reading, scrolling and editing work on the part shown; outline, find, Cmd+Down
and `[TOC]` cover that part; a banner says the rest is loading; save is refused until the
whole file is in. Edits made meanwhile are carried over as one diff patch on the full document
(undo history, selection kept). Measured (`MARKISE_LAZY_OPEN=1`):

| | mixed | prose | log |
|---|---|---|---|
| Open → first painted frame: normal / lazy / lazy+progressive | 1,389 / 867 / **47 ms** | 570 / 458 / **50 ms** | 287 / 277 / **35 ms** |
| Open → fully loaded, lazy+progressive | 782 ms | 503 ms | 328 ms |
| Slowest UI frame while loading (install with an edit included) | 32 ms | 29 ms | 32 ms |
| Edit during loading kept, view unchanged at install, model consistent | yes | yes | yes |

The read itself is ~40 ms; the lag was parse + model on 100 MB.

### Touchpad smoothness, round 2 (owner: RAW broke smoothness; SNAP alone not as smooth as Zed)

- **Why RAW was worse:** applying deltas in the frame they arrive stutters when trackpad events
  do not arrive one per frame (0 in one frame, 2 in the next; frames drawn for other reasons
  get none). Simulated with uneven delivery (`MARKISE_LAZY_SQ_CLUSTER=1`): stalled frames —
  exact raw 232 of ~300 moving, egui smoothing 148 (it passes finger-phase deltas through),
  **resampled raw 4**. `MARKISE_SCROLL_RAW=1` now drains delivered deltas with a one-frame
  half-life (`MARKISE_SCROLL_HALFLIFE_MS`, default 8, total preserved); `=exact` keeps the
  old behaviour.
- **Zed / GPUI** (read for ideas, Apache-2.0, `crates/gpui_macos`): frames are driven by a
  `CAMetalDisplayLink` (macOS 14+; `CVDisplayLink` before) — one drawable per refresh, Metal
  rendering; precise deltas become `ScrollDelta::Pixels` and are applied as delivered, so each
  frame shows everything since the last refresh. Markise renders with glow (OpenGL), likely
  capped at 60 Hz on ProMotion. Closest analogue here: `MARKISE_RENDERER=wgpu` (Metal) +
  `MARKISE_DISPLAY_LINK=1` (a frame per refresh while scrolling, present blocks on vsync) +
  resampled raw. A real display-link driver would need wgpu/winit changes.
- **`MARKISE_FRAME_LOG=1`** writes `/tmp/markise-frames-<pid>.csv` (per frame: interval, UI
  wall/CPU, previous frame CPU, wheel events / momentum / phases, raw / egui / applied /
  pending deltas, scroll offset, repaint requested, input-driven; header: renderer, present
  mode, display max fps) for checking pacing on the real display.
- Progressive open: the loading note is in the status bar now (no banner moving the text); a
  save requested while loading runs when the file is complete.
