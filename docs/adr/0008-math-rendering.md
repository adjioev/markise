# ADR 0008 — TeX math rendering

Status: accepted for the `math` branch (2026-10-27): engine, renderers, HTML export and viewer
integration implemented; PDF/PNG export waits for PR #11. Scope: `crates/markise-blocks`
(`math`), `crates/markise-export` (HTML), `crates/markise-layout` (`math`, block/view),
`crates/markise-ui-egui` (`images`, `viewer`), `assets/fonts/katex/`.

## Context

Spec 01 §3.7 / §4.1–4.2 define math blocks (`$$`, `\[…\]`, ```` ```math ````) and inline math
(`$…$`, `\(…\)`, `` $`…`$ ``, inline `$$…$$`), "rendered with MathJax-compatible TeX"; spec 04
asks for HTML export "math as rendered SVG or as LaTeX source" and (P2) copy as SVG / MathML /
TeX. `markise-md` already parses all of these (T15–T17); until now the viewer and export showed
the TeX source (C36, M1-3). The owner lifted the "math postponed" decision on 2026-10-27.

Constraints (AGENTS.md, spec README): pure Rust at run time — no JavaScript engine, webview or
external process; permissive crate licenses; heavy rendering off the UI thread, results cached
and computed lazily for visible items only; a math-heavy 1 MB document must still open within
NFR-2.

## Options surveyed (crates.io / GitHub, 2026-10-27)

| Candidate | License | Activity | Output | Notes |
|---|---|---|---|---|
| **RaTeX** (`ratex-parser`, `ratex-layout`, `ratex-types`, `ratex-font` 0.1.15) | MIT | very active (1.5k stars, release 2026-10-16, ~155k downloads) | renderer-neutral `DisplayList` (glyphs by font + char code, rules, rects, SVG-style paths) in em, with width/height/depth | Rust port of KaTeX's parser and layout; KaTeX font metrics embedded (no font needed for layout); macro expansion and output budgets (`validate_layout`), stack-safety guards; mhchem `\ce`. Glyph outlines need the KaTeX fonts (OFL-1.1). |
| **latex-rust** 3.0.1 | MIT/Apache-2.0 (code); embeds STIX Two Math (OFL-1.1) and an LPPL-1.3c color table | new (first release 2026-09, 2 stars, single author) | box tree → SVG / PNG / egui shapes (egui feature pinned to 0.36) | TeX Appendix G layout on exact rationals; no `align*`, `smallmatrix`, `\bmod`, `\ce` in our corpus; LPPL data is not on the allowlist. |
| typst (+ `mitex` LaTeX→Typst) | Apache-2.0 | very active | full Typst document → SVG/PDF | Excellent math, but needs the whole Typst compiler (hundreds of crates, a `World`, its own fonts) and a lossy LaTeX→Typst translation per formula; far too heavy for an inline-formula renderer. |
| ReX | MIT | dead since 2020 | — | Unmaintained. |
| `pulldown-latex`, `math-core`, `latex2mathml` | MIT | active / active / stale | MathML only | No layout: useful only for "copy as MathML" (P2). |
| `katex` / `katex-gdef-v8` | MIT | — | HTML | JavaScript engine (QuickJS/V8) — excluded. |
| `katex-rs` 0.3 | MIT | active | KaTeX HTML/MathML markup | Needs a browser's CSS layout to display — not native. |
| `iced_math`, `leaf-math`, `mathtex`, `fmd-math`, `tex2math` | various | new | widget / SVG | iced-bound, thin RaTeX wrapper, experimental XeTeX port, non-standard license, LGPL — not usable. |

## Spike (`spikes/adr-0008-math/`, release build, Apple M-series)

56-formula corpus (`cmp/src/corpus.txt`): fractions, roots, scripts, big operators, Greek, all
delimiter kinds, `\left…\right`, matrices (`pmatrix`, `bmatrix`, `vmatrix`, `smallmatrix`,
`array`), `aligned`, `align`, `align*`, `gather`, `cases`, `equation`, `\text`, font commands,
accents, braces, `\binom`, `\cfrac`, `\operatorname`, relations, arrows, `\xrightarrow`,
`\overset`, `\pmod`, `\color`, `\boxed`, `\tag`, `\substack`, `\not`, `\ce`, plus 3 invalid inputs.

| | RaTeX | latex-rust |
|---|---|---|
| Valid formulas accepted | **53 / 53** | 49 / 53 (`align*`, `\bmod`, `smallmatrix`, `\ce`) |
| Invalid formulas rejected with a message | 3 / 3 (with byte position) | 3 / 3 |
| Parse + layout, sum over corpus | **6.1 ms** (median 17 µs/formula, max 1.9 ms — `\bmod` expands `\mskip`, compiled regex per use) | 18.1 ms (max 5.0 ms) |
| Release binary delta over a hello-world | +4.0 MB (parser pulls `regex`, `fancy-regex` 0.14, `regex-lite`) | +2.1 MB (includes the 839 KB font) |
| Font needed | KaTeX TTFs for outlines only (19 files, 490 KiB) | STIX Two Math (839 KB) |

Measured in the product (`cargo bench -p markise-blocks --bench math`): first layout 0.9 ms
(lazy tables), first raster 2.0 ms (font parse + outlines); layout of a typical formula 30 µs;
SVG ≈ 105 µs; raster at 24 px/em ≈ 50 µs. The 1 MB math-heavy corpus has 6 117 display formulas:
laying out **all** of them takes 204 ms (33 µs each) and layout + raster ≈ 118 µs each — so the
viewer must stay lazy (it only renders the ~10–50 formulas on screen, < 6 ms of worker time), and
document open does no math work at all.

Visual check: `cargo run -p markise-blocks --example math_render -- - out < formulas.txt` writes
`out.png` (raster path) and `out.html` (SVG path); KaTeX-quality output for the whole corpus.

## Decision 1 — RaTeX for parsing and layout

RaTeX is the only candidate that is pure Rust, permissively licensed, actively maintained,
KaTeX-compatible (the de-facto "MathJax-compatible TeX" subset users write in Markdown) and
produces a renderer-neutral display list. Its layout needs no font file, so sizes (for line
layout and height estimates) come without touching fonts.

`markise_blocks::math::layout(tex, mode) -> Result<MathLayout, MathError>` wraps it:

- `MathLayout { mode, width, height, depth, items }` in em, y down, baseline at `y = height`;
  items are `Glyph { face, ch, x, y, scale, color }`, `Missing` (a char no bundled face has),
  `Rect` (rules, `\colorbox`; dashed flag) and `Path` (radicals, stretchy delimiters, braces).
- `color: None` means "current text color" (a sentinel layout color maps back to `None`), so a
  theme change does not need a new layout; explicit `\color`/`\textcolor` keep their RGBA.
- Inline math uses TeX text style, blocks display style.
- Guards: input over 64 KiB, more than 65 536 items, non-finite metrics → error; RaTeX runs
  under `catch_unwind` (third-party code on untrusted text must not kill a worker or an export).
- Errors carry RaTeX's message and byte offset; callers show the source with an error style.

## Decision 2 — bundle the KaTeX fonts (OFL-1.1, owner-approved for font files)

The RaTeX metrics are the KaTeX font metrics, so the outlines must come from those exact files.
The 19 KaTeX TTFs used by RaTeX (`KaTeX_Caligraphic-Bold` is never referenced and not shipped)
are committed **unmodified** under `assets/fonts/katex/` from the npm release `katex@0.16.22`
(byte-identical to the copies inside `ratex-katex-fonts`), with `OFL.txt`, `SOURCE.md`
(SHA-256 per file) and a `NOTICE` entry. Every family is a Reserved Font Name, so no
subsetting (a subset is a modified font and would have to be renamed); total 490 KiB, embedded
with `include_bytes!`. Owner decision 2026-10-27: OFL-1.1 allowed for bundled font files only.

Glyph outlines are read with `skrifa` (already in the tree) into em-unit path commands and
cached per (face, char) for the process.

## Decision 3 — two renderers, one display list

- **SVG** (`math::svg::to_svg`): glyphs as outline `<path>`s (no font embedding, no `<text>`
  except the `Missing` fallback), sized in `em` × `DEFAULT_MATH_SCALE` (1.21: the KaTeX fonts'
  x-height is small next to body fonts; same factor as KaTeX's default) with
  `vertical-align:-depth` so the baseline lines up; `fill="currentColor"` for inherited color;
  `role="img"` + `aria-label` with the TeX source; display mode adds `display:block`.
- **Raster** (`math::raster::rasterize`): `vello_cpu` (already in the tree) fills the same
  paths into a straight-alpha RGBA bitmap at a given px/em and text color, with 2 px padding and
  the baseline position; capped at 4096 px per side / 8 Mpx (scale reduced, reported back).
  egui cannot fill non-convex paths, so the viewer draws math as textures.
- **PDF** (PR #11, `export-m3`, not merged): walk `MathLayout` and emit `fonts::glyph_outline`
  paths + rects as vector graphics (or embed the KaTeX TTFs and emit glyph runs). PNG export can
  use `rasterize` directly. Integration is left to after #11 merges (see "Integration notes").

## Decision 4 — background service with a byte-bounded cache

`MathService` (worker thread, default 1) renders `MathRequest { tex, mode, px_per_em, color }`
to `MathImageLookup::{Ready(Arc<MathImage>), Empty, Failed(Arc<MathError>), Pending}`; the key
hashes all four fields. The queue is LIFO (formulas asked for last are on screen) and bounded
(512; the oldest fall off and are re-requested when visible); `clear_queue` drops queued work
on document switch; results (errors included) live in a 64 MiB LRU cache; the wake callback
requests a repaint after each job. Nothing math-related runs at document open or on the UI
thread.

## Decision 5 — HTML export

`HtmlOptions::math: MathExport::{Svg (default), Tex}`. SVG output keeps the spec 03 containers
and wraps the `<svg>` in `mjx-container class='MathJax' jax='SVG'` (`display='true'` for
blocks) — the element themes target (spec 03 §4.3/§4.4). `Tex` keeps the C36 placeholder form
(`\(…\)` / `\[…\]`) for a client-side renderer. Invalid TeX falls back to the source with class
`md-math-error` and the message in `title`. The untrusted-export fuzz test now also feeds math;
the only SVG it accepts is the generated one directly inside `mjx-container`.

MathML output is not produced (spec 04 copy-as-MathML is P2; `pulldown-latex`/`math-core`
(MIT) would be the cheap route when needed).

## Decision 6 — viewer integration on the `FormatDoc` seam (after PR #13)

`markise-layout` still does not depend on `markise-md` or `markise-blocks`:

- `markise_layout::math` defines `MathQuery { tex, display, px_per_em, color }`, `MathBox`
  (bitmap size, baseline, padding, width/ascent/descent in pixels) and `MathState::{Unavailable,
  Pending, Empty, Failed, Ready}`. The UI's `Images` provider answers `Images::math` (default
  method: `Unavailable`, so tests and headless tools keep the source rendering).
- Math blocks (`LeafKind::Math` and ```` ```math ```` fences, whole blocks only — a block split
  into segments keeps its source) become a centred `Prim::Math` with half an em above and below,
  scaled down when wider than the column. Inline math (`InlineKind::Math`, TeX = the node's
  rendered value) reserves its width with two invisible no-break spaces: a 1 pt *strut* whose
  line height lifts the row for the formula's ascent, then a body-size space carrying the
  formula width as leading space (no line break is possible between them, so the leading space
  is never dropped at a row start); the bitmap is placed over the pair on the text baseline, and
  the source map makes the formula one atom. Justification never stretches the pair.
- Pending/unavailable formulas show the styled source as before (M1-3); invalid TeX shows the
  source in the caution color.
- `Item::has_math` (math blocks, or source containing `$` / `\(`; over-approximated) puts the
  view's math epoch into the item's cache key; `View::math_changed` bumps it. The UI
  (`TextureStore`) owns the `MathService` (started on the first frame), sets a flag from the
  worker's wake callback, calls `math_changed` at most once per frame, uploads bitmaps on first
  paint into a 32 MiB texture LRU and snaps them to the pixel grid (they are rendered 1:1 for
  `font size × 1.21 × pixels per point`, so zoom/scale changes re-render).
- Measured (`cargo bench -p markise-layout --bench layout -- math`, 1 MB math-heavy corpus,
  2× scale): first screen 0.75 ms warm (formulas pending); cold first screen 8.9 ms and all 11
  visible formulas rendered and laid out again 0.3 ms later. Document open does no math work.

Review fixes (PR #15, Codex):
- **Bounded sizes.** `layout` refuses boxes over `MAX_EM` (2000 em) and any non-finite or far-off
  coordinate (`\rule{1e20em}{1em}` is an error); `rasterize` sizes in f64 with the same bounds
  and caps; the SVG renderer refuses unbounded input too. Every service job runs under
  `catch_unwind` (a panic is an error result), and a worker thread that dies anyway marks its
  job failed, wakes the UI and is replaced (`MathServiceStats::{panicked, respawned}`).
- **Descent reserved (MA-9).** The strut adds room above the text; when a formula is also
  deeper than the text's descent, the paragraph's runs are centred in their rows and the strut
  grows by twice the larger excess, so the row gains room above *and* below while the formula
  stays on the text baseline. Rows without a formula are unaffected (their glyphs are as tall as
  the row). Cost: such a row is symmetric, so it may be taller than strictly necessary.
- **Rendered at the shown size, bounded uploads.** Geometry is kept apart from pixels: the
  service keeps each formula's natural box (or error) in a small metrics map keyed by
  (TeX, mode) that is bounded by entry count and never evicted with bitmaps. A request carries
  the column (or cell) width; a formula that fits is rendered at natural size under a key that
  does not depend on the column, a wider one at the column width rounded down to a 32 px
  bucket. A full-size bitmap of a formula shown narrower is never rendered, so huge formulas
  cannot push each other out of the bitmap cache in a loop. Table natural-width passes only
  *measure* (`MathQuery::measure` → `MathService::measure`, metrics only); measured rows are
  rebuilt for drawing. The UI uploads
  at most 8 formula textures / 4 MiB per frame (one larger bitmap alone) and repaints for the
  rest.
- **Round 2.** Strut heights are set when the paragraph's job ends, after its alignment is
  known (a tall-but-shallow formula next to a deep one gets the centred-mode height too).
  Table formulas are sized to their cell's content width (rows whose pre-shaped cells do not
  fit are rebuilt with cell-sized formulas) and `Prim::Math::clip` keeps them inside the
  cell.

The parley developer backend places the reserved box by its own inline-box rules (not
verified). The caret treats a rendered formula as one atom; revealing its source when the caret
enters it (spec 02 §8) is editor work (M2).

## Integration notes (pending)

- **PDF/PNG (PR #11)** — see Decision 3: walk `MathLayout` with `fonts::glyph_outline` for
  vector PDF output; PNG export can call `raster::rasterize` (or reuse the `MathService`).
- **Copy as SVG/TeX** (spec 04, P2) — `to_svg` exists; MathML would need a new dependency.

## Consequences

- Release binary: see "Size" below; the fonts add 490 KiB, RaTeX's regex dependencies are
  partly shared with syntect (`regex`; `fancy-regex` 0.14 vs 0.16 is a duplicate).
- Characters outside the KaTeX fonts (e.g. Cyrillic or CJK inside `\text{}`) get KaTeX
  fallback metrics and are drawn as a placeholder box in the raster path and as `<text>` in SVG
  (MA-3).
- RaTeX is 0.1.x: pinned through `Cargo.lock`; upgrades must re-run the math tests and the spike
  corpus.

## Size

Release binaries, `origin/main` (1de3691) vs this branch, same machine (macOS arm64):

| Binary | main | math branch | Delta |
|---|---|---|---|
| `markise-cli` (HTML export renders math) | 5 492 560 B | 8 991 104 B | **+3.5 MB** (RaTeX code and tables, regex crates, 490 KiB fonts, vello_cpu/skrifa paths) |
| `markise` (measured before the viewer integration) | 23 042 640 B | 23 025 008 B | ±0 |

With the viewer integration (branch merged with main after PR #13): `markise` = 25 976 400 B, **+2.9 MB** over the main build above (that delta also contains PR #13's own changes, so math costs at most that; the app already linked vello_cpu, skrifa and regex). No start-up cost: fonts are `include_bytes!` data,
parsed on first use; RaTeX tables are lazy statics (first layout 0.9 ms, on the worker).
