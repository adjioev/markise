# ADR 0002 — Text layout for the egui front end: epaint galleys for M1, custom shaping later

Status: accepted for M1 (2026-10-26). Scope: `crates/markise-layout` (L2), `crates/markise-ui-egui`
(L6), `apps/markise`.

## Context

M1 renders a Markdown document read-only in an egui window and must meet NFR-1/2/3/5/6/7/10
(start, open 1 MB / 10 MB, 60–120 fps scrolling, memory, idle CPU). M2 adds live-preview
editing with keystroke latency p99 < 16 ms in a 10 MB document (NFR-4) and needs hit testing,
caret placement and selection inside rendered text. Two ways to turn styled text into pixels:

1. **epaint text layout** — egui's own `LayoutJob` → `Galley` (ab_glyph rasterisation, simple
   per-`char` advance + kerning, greedy line breaking, font fallback by glyph availability,
   faux italics, `valign` for sub/superscript, a galley cache keyed by the job hash).
2. **Custom shaping** — `harfrust` (HarfBuzz port) or `swash` shaping, `parley`/own line
   breaking (UAX #14), `skrifa`/`swash` scaling, own glyph atlas, painted as egui meshes or
   through a `vello_cpu` raster (the WordCraft reference architecture does the latter).

## What epaint gives and lacks (concrete)

| Need | epaint 0.33 | custom (harfrust/swash/parley) |
|---|---|---|
| Latin/CJK paragraphs, mixed sizes, colours, background, strike | yes | yes |
| Bold | only via a separate bold font family (no synthetic bold) | variable fonts / weights |
| Italic | faux (skew) only | real italic faces |
| Complex scripts (Arabic, Indic), ligatures, RTL/bidi | **no** (no shaping, no bidi) | yes |
| Line breaking | greedy, whitespace/CJK-aware, not UAX #14 | UAX #14, hyphenation possible |
| Font fallback | ordered list per family | per-script / per-cluster |
| Hit testing / caret | `cursor_from_pos`, `pos_from_cursor` on `char` indices | must be written (clusters) |
| Integration cost | none: same atlas, same painter, same accessibility | own atlas, own mesh/texture path, own caret/selection code |
| Work to reach M1 | days | weeks |

Measured with epaint (M1, release, Apple M-series): first screen of the 10 MB corpus shaped
from scratch in ~1.3 ms (`cargo bench -p markise-layout`, `first-screen-cold-10mb`); a screen
of new blocks during a scrollbar jump costs p50 ~0.35 ms / p99 ~1 ms of CPU per frame (large blocks are split into segment items, see below); wheel scrolling over
cached blocks p99 ~0.6 ms (`cargo bench -p markise-ui-egui --bench viewer`). Text shaping is
therefore not the bottleneck for M1's budgets; window/GPU start-up is (see "Measurements").

## Decision

- **M1 uses epaint galleys**, behind a seam: `markise_layout::block::Shaper` (`fn layout(&mut
  self, LayoutJob) -> Arc<Galley>`). Layout produces positioned primitives (`Prim`) per block,
  so the painter and the virtualisation/caching layers do not depend on how text was shaped.
  *(2026-10-27, ADR 0005 T2: the seam is now `markise_layout::text::{TextShaper, TextBlock}`
  over a backend-neutral `StyledText`; epaint galleys are its default implementation, unchanged
  in output.)*
- Bold comes from a system bold face (memory-mapped at run time; nothing bundled), italics are
  epaint's skew, CJK via a memory-mapped system fallback font.
- Galleys are cached inside per-block layouts (key: block content hash, document context
  hash, width, theme fingerprint, pixels-per-point, font generation, image epoch). Because an
  egui font-atlas rebuild invalidates glyph UVs, the painter re-requests each visible galley
  from egui's galley cache every frame (a hash lookup on a hit), so a cached layout can never
  paint stale glyphs.
- Large blocks are split into *segment* items at model-build time — code/math/HTML blocks and
  paragraphs by lines (over-long lines split into 4 KiB pieces), tables by row groups (column
  widths from the header and the first 64 body rows, so every segment agrees), `[toc]` by
  entry groups — so layout, caching and painting stay bounded per item even for a single
  10 MB block. Inline markup spanning a segment border of a huge paragraph is not matched
  (documented approximation). Within an item, code and TOC galleys are further chunked by
  48 lines for paint culling.
- **Revisit before or during M2** (owner: whoever starts M2 layout): move to custom shaping
  (`harfrust` + `skrifa`/`swash`, UAX #14 breaking, own atlas painted as egui meshes) when any
  of these becomes a requirement — complex scripts / RTL, real bold/italic faces from imported
  themes (spec 03 §2 `@font-face`), ligatures, or caret movement by grapheme cluster. The
  `Shaper` seam plus `Prim` output keep that change inside `markise-layout`.

## Consequences

- No bidi/complex-script support in M1 (listed as an open issue). Faux italics only.
- Hit testing uses epaint's `char`-based cursor API (`BlockLayout::link_at`); M2's caret
  mapping can start from it but will have to move to clusters with custom shaping.
- epaint underlines sit at the bottom of the line box (1.6× line height), far below the
  baseline, so links are shown by colour only.
- Memory: galleys carry their glyph meshes; the layout cache is bounded by entry count and by
  estimated bytes (16 MB default).

## Document lifecycle (M1 review follow-up)

The UI keeps one authoritative state per open document (`crates/markise-ui-egui/src/app.rs`):

- **Text is the source of truth.** Once source mode was entered, the editor buffer is the
  document's text and survives mode switches; each change bumps `text_rev`. The view model
  is derived: a background rebuild brings `model_rev` up to `text_rev`. A failed or crashed
  rebuild only leaves the preview behind (error banner); the text is never consumed or lost.
- **Saved state is a pair** `(saved_rev, disk version)`. A save writes a snapshot `(text,
  rev)` — applied to the document as minimal patches and serialized in the worker — and on
  success sets `saved_rev = rev` and records the new disk version (size, mtime, hash). The
  document is dirty while `text_rev != saved_rev`.
- **Continuations carry their preconditions** and are re-checked when the save completes:
  Save-then-Close / Open / Install run only if the saved revision covers the current text;
  if the user kept typing, the Save / Discard / Cancel question is asked again. An overwrite
  authorization (the disk version the user saw in a conflict) travels with its save request.
  Discard performs the action without modifying the document: if the other file then fails
  to open, the edited document stays dirty.
- **Disk changes.** Saves use the protocol in `markise_engine::save` (check → write and
  fsync a temp file → re-check → hard-link backup of the current file → rename → keep the
  backup if it no longer matches the expected version). This is not an atomic compare-and-
  swap: a writer that replaces the file by rename in the instant between the backup and the
  rename is overwritten; every other concurrent write is either detected as a conflict or
  preserved in a reported `.<name>.markise-backup-*` file. Watcher events always trigger a
  reload check on one cancellable loader worker (latest request wins); a reload equal to the
  recorded disk version (our own save) is ignored; events during a load or save are queued.

Deterministic tests inject delays and failures through worker hooks (`JobHooks`,
`SaveHooks`, the loader `StageHook`) instead of sleeping.

## Measurements (M1)

Headless (`cargo bench -p markise-ui-egui --bench viewer`, real app UI on an `egui::Context`,
tessellation included, no window/GPU), Apple M-series, release:

| Metric | Result | Budget |
|---|---|---|
| Open 1 MB → first rendered frame (read+parse+model+frame) | ~12–14 ms | NFR-2 < 200 ms |
| Open 10 MB → first rendered frame | ~83–89 ms | NFR-3 < 1 s |
| Wheel scroll 10 MB, CPU per frame | p50 0.10 ms, p99 ~0.6 ms | NFR-5 8.3 ms (120 fps) |
| Scrollbar jump 10 MB (new screen laid out) | p50 0.35 ms, p99 1.0 ms, max 1.0 ms | NFR-5 |
| RSS, 1 MB open (+ scrolled) / 10 MB open (+ scrolled), headless | 23 (45) / 90 (109) MB | — |
| Repaints requested when idle | 0 | NFR-10 |

Desktop app (`MARKISE_TIMING=1`, eframe + glow, window 1100×780):

| Metric | Result | Budget |
|---|---|---|
| Process start → first frame built | 158–245 ms, typically ~165–190 ms (bare eframe window: ~180 ms; eframe+wgpu: ~170–180 ms) | NFR-1 < 150 ms — **over** |
| Open (CLI arg) → first document frame, 1 MB / 10 MB | 31–38 ms / 87–100 ms | NFR-2 / NFR-3 — ok |
| RSS with no document / 1 MB / 10 MB | ~96 / ~118 / ~186 MB (bare eframe window: ~89 MB) | NFR-6 < 80 MB — **over**; NFR-7 < 300 MB — ok |
| Frames during 2.5 s idle with a document | 0 unprompted (only the measurement's own exit wake-up) | NFR-10 |

NFR-1 and NFR-6 are dominated by the eframe/winit/OpenGL (or wgpu) window baseline, which
already exceeds both budgets on this machine before any Markise code runs. Options for later
milestones: lazy GPU init behind an early native window, a lighter renderer, or renegotiating
the budgets for the egui stack.
