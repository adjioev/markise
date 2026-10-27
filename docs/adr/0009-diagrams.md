# ADR 0009 — Diagrams: own parsers and display list, dagre-compatible layout through `dugong`

Status: accepted for the `diagrams` branch (2026-10-27). Scope: `crates/markise-blocks`
(`diagram`), `crates/markise-export` (HTML), later `markise-layout` / `markise-ui-egui`
(viewer) and PDF/PNG export. Supersedes the "diagrams postponed" note of the M3 milestone
(owner lifted it 2026-10-27) and, for diagrams, the source placeholders of C36 / M1-3.

## Context

Spec 01 R3.5.6 / §4.6: with the Diagrams extension on, fences `mermaid`, `sequence`
(js-sequence-diagrams syntax) and `flow` (flowchart.js syntax) render as diagrams; Mermaid
types follow Mermaid 11.x. Spec 02 §8: an empty diagram shows "Empty Diagram", syntax errors
show inline. Spec 03: export DOM `pre.md-fences.md-diagram > .md-diagram-panel` with an SVG.

Constraints (owner): pure Rust at run time — no JavaScript engine, no browser/webview, no
external process (`mmdc`); permissive licenses; the app's own text stack (no new fonts);
NFRs: lazy (visible blocks only), cached, off the UI thread, bounded for pathological input;
documents are untrusted (no panics, bounded memory/stack/time).

## Survey (crates.io / GitHub, 2026-10-27)

| Candidate | License | Activity | Coverage | Output | Notes |
|---|---|---|---|---|---|
| `merman` 0.9.2 (+ `merman-core`, `merman-render`, `dugong`) | MIT OR Apache-2.0; default features add EPL-2.0 ELK and OFL math fonts | very active (release 2026-10-21, ~170k downloads, used by Zed) | ~37 Mermaid types, follows Mermaid 12.1 | Mermaid-structured SVG (CSS `<style>` block, optional HTML labels / `foreignObject`), layout JSON, PNG/PDF via resvg | cancellation, deadlines, resource policies, `TextMeasurer` trait. ~400k lines of Rust. |
| `mermaid-rs-renderer` 0.3.1 | MIT | active (2026-07) | 23 types | SVG; PNG via resvg; optional "scene" draw list | text shaped into outlines through usvg/fontdb (a second text stack); no documented limits for untrusted input. ~58k lines. |
| `selkie-rs` 0.3.0, `mermaid-render` 0.10, `mermaid-core` 0.1.5 | MIT / MIT OR Apache-2.0 | young or quiet | partial | SVG | small user base. |
| `flowmaid` 0.27 | **GPL-3.0** | — | several | — | excluded (license). |
| `mermaid-text`, `mmdflux`, `figurehead`, `graphs-tui` | MIT | — | flowchart/sequence | terminal text (mmdflux also SVG) | not a GUI renderer. |
| `dugong` 0.9.2 (+ `dugong-graphlib`) | MIT OR Apache-2.0 (dagre-derived, MIT) | same repo as merman | layered layout only | node/edge coordinates | port of dagre 2.0.2 — the layout Mermaid flowcharts use; compound graphs (subgraphs), self loops, edge-label ranks, work-unit control. ~17k lines. |
| `rust-sugiyama` 0.4 | MIT | 2025-09 | layered layout | coordinates | petgraph-based; no compound graphs or edge labels. |
| `dagre` 0.1, `dagre-dgl-rs`, `rusty-mermaid-dagre`, `elkrs` | Apache/MIT | young | layout | coordinates | `elkrs` is a large ELK port. |

No crate implements js-sequence-diagrams or flowchart.js syntax.

## Spike (`spikes/adr-0009/`, release build, Apple M1 Max)

`merman` 0.9.2 with only `svg` + flowchart, sequence, class, state, ER, pie, gantt (no ELK,
no math, no PNG):

- clean release build of the crate graph: **4 min 48 s**; 142 crates; stand-alone binary
  **13.1 MB**. Licenses all permissive (MPL-2.0 cssparser/selectors file-level, BSD-3
  `lol_html`, Unicode-3.0 ICU4X, `ryu-js` Apache-2.0 OR BSL-1.0).
- render time (deterministic text measurement): small flowchart 2.2 ms (34.6 ms with the
  "resvg-safe" post-processing pipeline that turns HTML labels into SVG text), 200-node
  flowchart 133 ms, 2000 nodes ~1 s (deadline enforced correctly at 2 s), sequence 0.5 ms.
- 3000 random mutations of samples: 0 panics.
- Output: SVG whose styling lives in a large CSS `<style>` block with attribute and
  structural selectors (`[data-look="neo"]…`, `:nth-of-type`), markers, filters and a
  `"Recursive Variable"` font default. Painting it natively needs an SVG + CSS engine
  (usvg/resvg) and that engine's text stack (fontdb/rustybuzz) — i.e. a second font system
  next to parley — or a rasterised texture per diagram and theme.

`dugong` alone (layout of synthetic graphs, label-sized edges): 200 nodes **6.8 ms**,
1000 nodes 32 ms, 2000 nodes 111 ms; 2000 random compound graphs with self loops and
multi-edges: 0 panics; clean build of `dugong` + graphlib ~12 s.

## Decision

1. **Own parsers, own model, own display list; layout of graph diagrams through `dugong`.**
   - Parsers (in `markise_blocks::diagram`): Mermaid `flowchart`/`graph`, Mermaid
     `sequenceDiagram`, js-sequence-diagrams (`sequence`), flowchart.js (`flow`) — written from
     the public syntax documentation of Mermaid, js-sequence-diagrams and flowchart.js
     (third-party open-source projects, not the reference product).
   - Two models: `graph::Graph` (Mermaid flowcharts and flowchart.js map onto it) and
     `sequence::Sequence` (both sequence syntaxes map onto it).
   - Layout: graphs through `dugong` (pinned `=0.9.2`: upstream says its low-level API is not
     a separately supported surface; upgrades are deliberate and re-run the fuzz tests);
     sequence diagrams through an own column/row layout (no graph layout needed).
   - Output: a renderer-neutral `Scene` — rectangles, ellipses, paths (move/line/cubic),
     single-line text runs — with **color roles** (`Role::NodeFill`, …) instead of colors.
   - Why not adopt `merman`: its size and build cost (13 MB, ~5 min clean release build —
     the whole Markise app is smaller), and its SVG-with-CSS output, which forces a second
     SVG/CSS/text engine into the viewer or rasterised textures keyed by theme and zoom.
     Its fidelity across 37 types is the price we pay; see "Later" for an opt-in path.
   - Why not `mermaid-rs-renderer`: text through its own font stack, no documented bounds.
2. **Mermaid types in v1:** `flowchart` / `graph` and `sequenceDiagram`. Every other Mermaid
   type (`classDiagram`, `stateDiagram(-v2)`, `erDiagram`, `gantt`, `pie`, `journey`,
   `gitGraph`, `mindmap`, `timeline`, `zenuml`, C4, `quadrantChart`, `requirementDiagram`,
   `sankey`, `xychart`, `block`, `packet`, `kanban`, `architecture`, `radar`, `treemap`) is
   recognised and reported as **unsupported**: the block shows its source with the notice
   "Mermaid diagram type "…" is not supported yet; showing the source". Next candidates on the
   same engine: `stateDiagram` and `classDiagram` (graph layout), `erDiagram`, `pie`.
3. **Painting.** One `Scene`, several painters: `diagram::svg::to_svg` (HTML export; only
   `svg`/`rect`/`ellipse`/`path`/`text` with presentation attributes — no scripts, links,
   `foreignObject` or CSS, so it is inert whatever the source); the egui front end
   (epaint shapes + the app's text stack); PDF/PNG export later. Colors come from
   `DiagramPalette::from_theme` — derived from the color scheme part of the composed theme
   (ADR 0007): node fill = accent tinted over the page, text = body text, notes = warning
   alert tint, etc. (WCAG AA for label text tested in light and dark). Explicit `style` /
   `classDef` / `linkStyle` colors in the source are honoured as `Paint::Custom`. Layout depends
   only on the label font size (from the style) and the text measurer, so switching colors never
   re-lays out a diagram.
4. **Text.** Layout measures labels through the `TextMeasure` trait. The viewer measures the
   body font's per-character advances (printable ASCII, regular and bold) with its own text
   stack once per theme/font change (`markise_layout::block::label_metrics`) and hands the
   resulting `TableMeasure` to the worker (DG-6), so boxes fit the faces painted — monospace
   styles included; a label that still comes out wider is shrunk to fit. The measurement shapes ~190 glyphs:
   ~0.7 ms cold on the UI thread (`cargo bench -p markise-layout --bench layout --
   diagram-label`, M1 Max), once per theme/font change. Export and tests use
   `ApproxMeasure` (font-independent width classes). SVG text uses the theme's body font stack
   with `dominant-baseline="central"`, so small width differences only change the slack inside
   boxes. No fonts are added.
5. **Bounds for untrusted input** (`Limits`, defaults): source ≤ 256 KiB; ≤ 1000 nodes
   (participants ≤ 200); ≤ 2000 edges (≤ 4000 sequence statements); labels ≤ 1000 characters and
   40 lines (truncated with "…"); subgraph/block nesting ≤ 24; parse + layout ≤ 2 s wall clock.
   Parsers are iterative (no recursion on input) and charge work units for every scanned byte
   and created item (hard cap 20 M units, deadline/cancellation checked every 64 Ki units); class
   names are interned and bounded (DG-8). Scenes over 30 000 pt on a side, 30 000 shapes or 8 000
   text runs are refused, and the viewer expands dashed strokes only inside the clip rect
   (≤ 20 000 dashes per item and frame). dugong runs under a `WorkControl` that checks
   cancellation and the deadline every 20 000 work units, and as defence in depth inside
   `catch_unwind`. Exceeding a bound gives `DiagramError::TooLarge` / `Timeout` and the source is
   shown. Property tests (`crates/markise-blocks/tests/diagram_fuzz.rs`): mutated samples and
   arbitrary text in all three languages, deep nesting, huge labels/lines, fan-out explosions
   (`a & b & … --> c & d & …`).
6. **Scheduling and caching** (`DiagramService`): one worker thread, LIFO queue of ≤ 64 jobs (the
   most recently requested — visible — diagram first, the oldest dropped), LRU cache bounded
   to 32 MiB of scenes, keyed by language + source + font size + measurer id; errors cached (a
   broken diagram is not re-parsed per frame), cancellations not; `set_generation` drops queued
   jobs of an older document and cancels the running one; `wake` (repaint) after each result.
   Width is not part of the key: the viewer scales a scene down to the column width at paint
   time (Mermaid's `useMaxWidth` behaviour), which costs nothing.
7. **Preference.** Owner decision 2026-10-27 (DG-2): Diagrams are **on** by default (a Markise
   deviation from the reference default off; spec 01 §4.6/§5, spec 02 §16, spec 05 updated). View →
   Diagrams switches them and is persisted as settings key `diagrams` (DG-7). Off shows the
   fences as source.

## Measurements (release, Apple M1 Max, `cargo bench -p markise-blocks --bench diagram`)

| Input | parse + layout |
|---|---|
| flowchart, 10 labelled edges | ~1.2 ms |
| flowchart, 50 nodes / 50 labelled edges | 1.3 ms |
| flowchart, 200 nodes / 200 labelled edges | 5.9 ms |
| sequence, 50 messages + notes | 41 µs |
| sequence, 500 messages + notes | 0.42 ms |

Typical diagrams render well inside one frame even synchronously; the worker keeps large ones
off the UI thread. Cost when the extension is off or no diagram is visible: none (the worker
thread is idle; nothing is parsed).

## Consequences

- New dependency: `dugong` + `dugong-graphlib` (MIT OR Apache-2.0; pulls `serde`/`serde_json`,
  `hashbrown`, `rustc-hash`). Not vendored, so no NOTICE entry is required by AGENTS.md; dagre's
  MIT notice travels with the crate.
- Visual fidelity differs from Mermaid's own renderer (shapes, spacing, curve style follow it
  closely; themes are Markise's). Subgraph titles are drawn in a band added above the cluster
  box that dagre computed, so an edge entering a cluster from above can cross its title; edges
  to a subgraph id attach to the subgraph's first node (Mermaid clips them at the cluster
  border). Positional hints of flowchart.js (`right`, `left`, …) are ignored.
- `click`, links and icons in diagrams are ignored (security and simplicity).

## Later

- More Mermaid types on the same model/painters (state, class, ER, pie first).
- No `merman` backend (owner, DG-4: EPL-2.0 ELK, size); the remaining Mermaid types are
  written in-house on the same model and painters.
- PDF/PNG export (PR #11, not merged yet) paints the same `Scene` (rect / ellipse / path / text
  map directly onto its page primitives).

## Viewer integration (done on this branch, after PR #13)

- `markise-layout` reads diagrams through its own trait `Diagrams` (`diagram.rs`: generation,
  state, want), like `Highlights`; it depends on `markise-blocks` only for the `Scene` type
  (allowed by the L2 chain) and never on `markise-md` (format seam unchanged).
- Diagram fence items carry the whole block's content key (`CodeRef`) and are segmented like
  code blocks, so the source fallback (off / pending / error) shapes only the visible segments;
  once the scene is ready the first segment draws it and later segments are empty (zero height). The item's state (off / pending / scene / error) and the
  renderer generation are part of its cache key; layout requests the scene only for items it
  lays out (visible ones), shows the source while pending, and draws the scene scaled down to
  the column (centered) with the theme's colors as rect / path / text primitives (`Prim::Path`
  is new; the egui viewer paints it, dashed strokes included). Errors show the notice above the
  source.
- `markise-ui-egui::diagram::DiagramRenderer` adapts `DiagramService` (lazy worker start, document
  generations, label size from the theme, label metrics from layout). View → Diagrams switches
  rendering (default on, persisted, DG-2/DG-7). Cancellation of a superseded job wakes the UI and
  a re-request for the current document replaces a pending request of an older one.
