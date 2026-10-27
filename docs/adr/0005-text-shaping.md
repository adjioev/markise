# ADR 0005 — Text shaping and glyph rendering for M2: parley + fontique behind `markise-text`, glyph atlas in egui

Status: **proposed** (2026-10-26; revised after two review passes the same day; **T0 spikes
S1–S5 and T1 done 2026-10-27**, results in §7 and recommended changes marked *[T0]*; **T2
seam implemented 2026-10-27**, see *[T2]* notes in §4.1 and §5). Supersedes
the "revisit before M2" item of ADR 0002 once accepted. Scope: new `crates/markise-text` (L2),
`crates/markise-layout` (L2), `crates/markise-ui-egui` (L6). T1 added `crates/markise-text`
(not wired into the app); spikes are isolated under `spikes/adr-0005/` (see its README).

## Context

M1 shapes text with epaint galleys behind `markise_layout::block::Shaper`
(`fn layout(&mut self, LayoutJob) -> Arc<Galley>`, ADR 0002). Layout emits `Prim::Text { pos,
galley }`; the painter re-requests each visible galley from egui's galley cache every frame
because an atlas rebuild invalidates glyph UVs; layouts are cached per item (`CacheKey`:
item hash, document context, width, theme, ppp, font generation, extra) under a 16 MB / 4096
entry LRU. Blocks above 128 lines or 32 KiB are split into segment items of up to 64 lines /
16 KiB; a single line over 4 KiB is cut into pieces at a UTF-8 character boundary, preferring a
space in the second half (`model::split_units`).

epaint is the wrong base for M2 and for imported themes:

- no OpenType shaping (no ligatures, no Arabic joining, no Indic/USE reordering), no bidi;
- `char`-indexed cursors (`cursor_from_pos` / `pos_from_cursor`), not grapheme clusters, so
  caret steps can land inside a ZWJ emoji or a combining sequence;
- no automatic face matching: a real italic or bold file can be registered as its own epaint
  family and selected by our code (M1 does this for bold), but there is no style/weight
  matching, no synthetic bold, only faux (skewed) italics, and only the default instance of a
  variable font (TH-2); `@font-face` files with weight ranges and `unicode-range` splits (C3,
  TH-1) have no representation;
- greedy, non-UAX #14 line breaking.

**Re-check against egui 0.37 (2026-10-27, toolchain bump to Rust 1.99.1).** The list above
describes epaint 0.33. epaint 0.37 replaced ab_glyph with skrifa (parsing, hinting) and
harfrust (OpenType shaping): default features such as ligatures and kerning are now applied
(e.g. `->` renders as an arrow in a ligature code font) and cannot be switched off per section,
so R10's "code configurable" is still unmet. Shaping runs per font-face run, not per script run,
and there is still no bidi. Cursors remain `char`-indexed: a ligature or multi-character cluster
gets zero-width continuation glyphs so that `Row::glyphs.len()` equals the character count (caret
stops can still land inside a cluster). Glyphs are now centred vertically in an explicit
`line_height` (CSS half-leading) instead of sitting at the top of the line box. Face matching,
`@font-face`/`unicode-range`, synthetic bold and UAX #14 breaking are unchanged. The decision
below is unaffected; the IME facts are updated in R4 and §4.4.

**Is staying on epaint's shaping viable for M2? (T0, 2026-10-27: no.)** Evidence from the
epaint 0.37 sources and `spikes/adr-0005/t0/epaint037-check`: shaping is per *font-face* run
with `UnicodeBuffer::guess_segment_properties()` (direction guessed from the run's first
script), there is no UAX #9 analysis or reordering (`text/unicode.rs` still carries
"TODO(emilk): heed bidi characters"; `segment_into_runs` documents that script-aware splitting is
needed "once RTL/bidi support is added"): `abc אבג def` is drawn with א leftmost, and
`אב (abc) ג` produced 19 row glyphs for 10 characters. Cursors are still `CCursor` (char index).
What *did* change and matters for us: epaint 0.37 rasterises glyphs with **vello_cpu 0.2** and,
with its `color_fonts` feature, draws COLR (incl. v1, via glifo) and sbix/CBDT glyphs — the
rasteriser of §3 can be the same crate. Verdict: epaint remains the rollback path, not the M2
base; parley is still required for R1, R2, R5 and R8.

The `Shaper` seam is narrower than ADR 0002 hoped: its signature and `Prim::Text` are epaint
types (`LayoutJob`, `Galley`), and `BlockLayout::link_at`/`cost` read galley internals. Moving
off epaint changes the seam itself, not only its implementation (§4.1).

## 1. Requirements

| # | Requirement | Source | Notes for the decision |
|---|---|---|---|
| R1 | Caret positions and selection ends on **grapheme cluster** boundaries; Backspace deletes by cluster (or by code point for some scripts), arrows step by cluster, word moves by UAX #29 words | spec 02 §3–5, §10 | cluster map text↔glyphs |
| R2 | **Hit testing both directions**: point → (position, affinity) and position → caret rect; selection → rects split at bidi boundaries | spec 02 §5.4, §10; link click (M1) | per-line visual cluster boxes |
| R3 | **Up/Down by visual line with goal x**, across blocks, segments, table cells and code chunks; line start/end, Cmd+Backspace to visual line start | spec 02 §5.4, §6 | document-level caret model (§4.3) |
| R4 | **IME preedit** inline at the caret, replacing the selection only on commit, cancellable, candidate window at the caret; composition end is an undo boundary | spec 02 §11 | egui 0.33: `ImeEvent::Preedit(String)` without a cursor range; egui 0.37: `Preedit { text, active_range_chars }` carries it, but there is still no surrounding-text output and egui-winit emits no delete-surrounding (§4.4) |
| R5 | **Bidi** (UAX #9) per logical paragraph: RTL paragraphs and runs, per-line reordering, mirrored brackets, visual vs logical arrows | product | must survive render chunking (§4.2) |
| R6 | **Complex scripts** (Arabic joining, Indic, Thai, Myanmar, Khmer) via OpenType shaping per script run; dictionary word/line segmentation for Thai, Lao, Khmer, Myanmar | product | parley `complex-scripts` feature (§2.2) |
| R7 | **CJK line breaking** (UAX #14 incl. ideographic opportunities), one CJK face per run where possible | bench "CJK" corpus | |
| R8 | **Theme fonts** per CSS Fonts 4: `@font-face` (local files ttf/otf/woff/woff2, `local()`, `src` fallback), weight/style/stretch descriptors incl. ranges, `unicode-range` composite faces (C3, TH-1); path confinement like TI-9 | spec 03 §1.5/§2, C3, TI-8/9 | importer records `FontFace { sources, weight, italic, unicode_range }` |
| R9 | **Real bold/italic faces and variable-font instances** (wght/ital/slnt), synthesis only as a fallback | TH-2 | |
| R10 | **Ligatures** and OpenType features (prose on; code configurable) | themes using ligature code fonts | |
| R11 | **Colour emoji**: sbix (macOS), COLRv0 and COLRv1 (Windows 11, newer Noto), CBDT (Linux); `monocolorEmoji` (B2) | spec 01 emoji, 03 `monocolor-emoji` | COLRv1 needs a second rasteriser path (§3) |
| R12 | **Fallback chains**: CSS stack → theme faces → per-script system fallback → last resort; deterministic per run | 03 §2.1, ADR 0004 dec. 5 | |
| R13 | **Performance**: NFR-1 (start < 250 ms), NFR-3, NFR-4 (keystroke p99 < 16 ms in 10 MB), NFR-5 (60/120 fps), NFR-10 | spec README | per-item shaping, cached; no font I/O blocking the UI thread |
| R14 | **Memory**: NFR-6 < 130 MB with 1 MB open, NFR-7 < 300 MB with 10 MB | spec README, ADR 0002 | M1 desktop measured ~118 MB at 1 MB (window baseline ~89–96 MB): **~12 MB headroom** (§3.1) |
| R15 | **wasm**: no system fonts, no mmap; fonts arrive as bytes; browser IME via egui's text agent | spec README | |
| R16 | **Licenses** per AGENTS.md; nothing bundled unless owner-approved (M1-5 still open for egui's OFL/Ubuntu font data) | AGENTS.md, NOTICE | applies to test fixtures too |
| R17 | **Accessibility** of the document surface (screen readers: text, caret, selection, navigation) | product | egui's built-in text a11y does not cover custom-painted text (§4.5) |

## 2. Candidates

### 2.1 `wordcraft-fonts` (storytold/wordcraft `crates/fonts`)

Read at rev `6a2b520fa49aa0d0595cea36589aaf16ca371cd3` (2026-10-26). MIT OR Apache-2.0.

**What it is.** ~6.9 kLOC: `fontdb.rs` (font database), `fontdb/user.rs` (user font files,
WOFF/WOFF2 unwrapping with `wuff` + `brotli-decompressor`, size caps), `lib.rs` (`shape`,
`shape_run`: harfrust shaping per script run, direction runs, glyphs in *logical* order with byte
clusters), `script.rs` (script itemisation, per-script fallback families), `color.rs`
(COLRv0/v1 paint graphs → flat `ColorOp` list; sbix/CBDT bitmaps; hardened caps), `math.rs`
(OpenType MATH table), `word.rs` (Word font substitutes and line metrics), `ui_fallback.rs` +
`CraftFont` (WordCraft's UI CJK fonts). Dependencies: skrifa 0.47, harfrust 0.13, wuff,
brotli-decompressor, unicode-bidi, unicode-script, kurbo, log — all crates.io, inherited through
`workspace = true`.

**API surface.** `FontDb` (`global()` singleton or `with_font_dirs`; `face(family, style)`,
`styles`, `axes`, `add_font(Vec<u8>)`, `fallback_for(char, exclude)`, `fallback_for_run`,
`outline(face, gid)`, background catalog scan), `FontFace` (metrics, coverage, skrifa/harfrust
refs, variable coords, `color_glyph(gid, ppem)`), `FaceRef` (`&'static` handle: faces are leaked,
never unloaded), `shape(face, text, features, map) -> Vec<ShapedGlyph>` (font units; no sizes, no
line breaking).

**How WordCraft uses it.** `crates/layout/src/para.rs` (3.5 kLOC) resolves runs against the Word
document model, shapes per style run, groups glyphs into grapheme `Cluster`s, breaks lines
first-fit with `unicode-linebreak` (+ hyphenation), then `reorder_line` runs UAX #9 per line and
stores visual cluster boxes (`Line::vis`). `crates/layout/src/hit.rs` hit-tests on pages
(`Page`, `Placed`, `StoryRef`). `crates/render` fills each glyph outline with `vello_cpu` into a
page pixmap (no glyph cache); the egui canvas uploads pages (or 1024-px tiles) as textures and
draws caret, selection and IME preedit on top.

**As a git dependency** (spike `spikes/adr-0005/wordcraft-fonts`, source-verified facts and one
macOS run):

- Resolves and builds with Rust 1.89: the crate does not inherit `rust-version` (its workspace
  says 1.95, therefore not enforced); the workspace `[patch]` section does not apply to a consumer
  and is not needed by this crate. **Nothing guarantees this stays true.**
- `include_bytes!("../../../assets/fonts/…")` embeds **13 OFL font files (~4.3 MB)
  unconditionally**. OFL is not on our license list (M1-5) and Markise bundles no fonts.
- `#![forbid(unsafe_code)]`: font files are read into owned heap memory (`std::fs::read`), never
  mapped, never unloaded. Measured: after five system fallbacks (Arabic → Geeza Pro, 日 → Hiragino
  Sans, 한 → Apple SD Gothic Neo, 👩 → Apple Color Emoji, क → Kohinoor Devanagari — whole families
  load) the database held **303 MiB of owned font bytes**; process RSS went from 5.0 MiB to
  314 MiB. This is a font-ownership result, not a rendering-memory result.
- Shaping alone: median 54 µs/KiB on the common input (README).
- Churn (one `git log` reading): 98 commits touched `crates/fonts` in 30 days, 47 in 7 days; the
  API follows WordCraft's word-processor needs, not a CSS font model.

**What we would still write ourselves** with it: CSS font matching, line breaking, per-line bidi,
cluster mapping, hit testing, caret geometry and movement, IME layout, rasterisation and caching.
`para.rs`/`hit.rs` are welded to the Word model and pages: references only.

**Verdict:** do not depend on `wordcraft-fonts` (git or whole-crate vendoring). Vendor, with
attribution in `NOTICE`, only self-contained pieces when needed: `decode_font_file` (WOFF/WOFF2
with its checks) for `@font-face` files, later `math.rs` (MATH table) and possibly `color.rs`
(COLRv1 → ops) for the COLRv1 path.

### 2.2 parley + fontique (linebender)

parley 0.12 / fontique 0.12, Apache-2.0 OR MIT, MSRV 1.88 (manifest-verified; builds on 1.89).
Internally harfrust 0.12 (MIT) + skrifa 0.44 + ICU4X segmenter/properties (Unicode-3.0 data).

- **Layout**: style-run/tree builders, itemisation (script, bidi level, font), shaping, UAX #14
  line breaking incl. CJK, per-line bidi reordering, alignment incl. justify, inline boxes, line
  metrics, re-breaking at a new width without reshaping.
- **Feature policy**: default features are `system` only. Without **`complex-scripts`** the
  engine uses ICU4X's non-dictionary word/line segmenters, so Thai, Lao, Khmer and Myanmar get no
  dictionary word breaks (shaping itself is unaffected). Decision: enable `complex-scripts`
  (adds dictionary data — measure binary-size cost in T1) with Thai/Khmer break tests; if the
  size is unacceptable, make it a build feature and record the limitation.
- **Editing primitives**: `Cursor` (byte index + affinity, `from_point`, `geometry`, visual and
  logical word moves), `Selection` (`word_from_point`, `line_from_point`, `move_lines` with goal
  x, `line_start/end`, `geometry` → rects, extend/shift-click), `PlainEditor` with IME compose.
  These work **within one `Layout`**; the document-level model is ours (§4.3).
- **Fonts (fontique)**: system fonts via CoreText / DirectWrite / fontconfig, memory-mapped,
  lazily; per-script+locale fallback; `register_fonts(Blob, FontInfoOverride { family_name,
  weight, style, width, axes })`. `weight` is a single value and `axes` are default coordinates:
  **not** a CSS descriptor-range contract; fontique matches attributes *within* a family and walks
  families in order. Hence CSS matching is the adapter's job (§4.1). No `unicode-range`, no
  WOFF/WOFF2. wasm: dummy system backend, fonts registered as blobs.
- **Variable fonts / synthesis**: explicit `FontVariations` per run; runs carry `Synthesis`
  (embolden/skew) that the renderer applies.
- Spike observations (one macOS run; README): build+shape+break median round average 105
  µs/KiB vs epaint's 58 µs/KiB `layout_job` on galley-cache misses with a warm glyph atlas, same
  input (different scopes; hypothesis only: cost per shaped item about 2× M1);
  `FontContext::new()` with system discovery 85–154 ms; shape-only RSS 17.39 → 19.94 MiB across
  Arabic, Japanese, Korean, emoji and Devanagari text (no glyph decoding); no `.notdef` in the
  first mixed layout (the second, Devanagari/Korean layout was not inspected); one arrow over
  👩‍👩‍👧 moves the whole 18-byte grapheme.
- Risks: pre-1.0, breaking releases (0.12 split out `parley_engine`/`parlance`); Linux system
  fonts via fontconfig (C library, §4.6).

### 2.3 cosmic-text

cosmic-text 0.19 (MIT OR Apache-2.0, MSRV 1.89): fontdb (memory-mapped by default: its `std`
feature enables `fontdb/memmap`) + harfrust 0.5 + swash + `unicode-linebreak` + `unicode-bidi`,
built-in `Editor`, `SwashCache`, IME-friendly buffer. Mature (COSMIC, iced, Bevy). Fit problem: its
`Buffer` owns the text and is line-oriented (plain-text editors, labels), while our styled
block-flow text and source↔rich mapping live in `markise-layout`/`markise-doc`; font matching is
fontdb's, not CSS-like; older harfrust/skrifa pins. Viable fallback for *shaping/layout* if
parley stalls; it does not fix egui-level IME limitations (§4.4).

### 2.4 harfrust (or rustybuzz) + skrifa directly

Maximum control, smallest dependency set; we would rewrite most of parley (itemisation, fallback,
breaking, bidi, clusters, caret). Only if parley's model blocks us.

### 2.5 Stay on epaint galleys

Zero work; fails R1, R5, R6, R8, R9 (beyond hand-registered bold/italic families), R10, R11
(monochrome Noto Emoji only). Remains the M1 path and the rollback until T8 (§5).

### 2.6 Feature / risk matrix

| | epaint 0.33 (0.37: see Context) | wordcraft-fonts (+ own layout) | parley + fontique | cosmic-text | harfrust + skrifa (own) |
|---|---|---|---|---|---|
| Grapheme caret / selection (R1) | no (`char`) | own (reference in para.rs) | **yes** (per layout) | yes | own |
| Hit test both ways (R2) | char-based | own | **yes** (per layout) | yes | own |
| Cross-surface caret (R3) | own | own | own (§4.3) | own | own |
| IME (R4) | via TextEdit only | own | `set_compose` / own | yes | own |
| Bidi (R5) | no | shaping per run; reorder own | **yes** | yes | own |
| Complex scripts (R6) | no | shaping yes; segmentation own | shaping yes; dictionary breaks with `complex-scripts` | yes | own |
| UAX #14 + CJK (R7) | no | own | **yes** | yes | own |
| CSS face matching (R8) | no | no | no → adapter (§4.1) | no → adapter | own |
| `unicode-range` (R8) | no | no | no → adapter | no → adapter | own |
| WOFF/WOFF2 | no | **yes** | no → vendored | no | own |
| Real styles / variable (R9) | separate families, manual | instances, no synthesis | **yes** (+ `Synthesis`) | partial | own |
| Ligatures (R10) | no | yes | **yes** | yes | yes |
| Colour emoji (R11) | mono only | `color.rs` incl. COLRv1 | renderer's job | swash: sbix, CBDT, COLRv0 | renderer's job |
| Font memory (R14) | per-family files | **owned copies** (303 MiB after 5 fallbacks) | **mmap** | mmap (default) | ours |
| Rust 1.89 | yes | yes today, unenforced | yes (MSRV 1.88) | yes (MSRV 1.89) | yes |
| wasm | yes | yes | yes (blobs) | yes | yes |
| License | MIT/Apache (+ OFL font data, M1-5) | MIT/Apache + **4.3 MB OFL fonts embedded** | Apache/MIT (+ harfrust MIT, ICU Unicode-3.0) | MIT/Apache | MIT / MIT-Apache |
| API stability | stable | **low** (app-driven churn) | medium (pre-1.0, versioned) | medium | high |
| Our remaining work | none | large | **medium** | medium + model mismatch | very large |

## 3. Rendering options

Shaped output is positioned glyph ids per font/size/variation; it must become pixels in the egui
frame next to the existing `Prim::Rect/Line/Image` painting.

**(a) Outlines → egui meshes.** skrifa outlines tessellated (epaint `PathShape` or lyon). No
hinting, no gamma-correct coverage, epaint feathering is poor at 12–16 px, large meshes.
**Rejected** for body text (possible for very large display sizes).

**(b) Glyph atlas: rasterise each (face, glyph, size px, subpixel-x bin, variation coords,
synthesis) once into an egui texture, draw textured quads** — epaint's own approach.
- Rasterisers behind a `GlyphRasterizer` trait: `swash::scale` for masks (subpixel offsets,
  optional TrueType hinting), sbix/CBDT bitmaps and COLRv0 layers; **swash 0.2.10 does not
  implement COLRv1 paint graphs**, so a second path is required, not optional: skrifa's colour
  painter (or WordCraft's `color.rs` ops) rendered with `vello_cpu` into the atlas cell.
  *[T0] Superseded:* **one rasteriser, vello_cpu 0.2** (already in the tree through epaint 0.37,
  which uses it for its own glyphs) with its `text` and `png` features: `glyph_run().fill_glyphs`
  draws outlines, sbix/CBDT bitmaps and COLRv0/**v1** (glifo's COLR painter). S4 drew a synthetic
  COLRv1-only font (PaintGlyph → PaintLinearGradient) in colour and Apple Color Emoji (sbix);
  11.8 µs per glyph at 1×, 26 µs at 2×. swash is not needed; hinting is not available (off on
  macOS/HiDPI anyway, ADR text below).
- COLRv1 / `monocolorEmoji` behaviour (needed before R11 is "done"): with colour enabled, a COLRv1
  glyph uses the vello_cpu path; if that fails (caps, malformed graph), draw the glyph's plain
  outline when the font has one, else take the next fallback face with sbix/CBDT/outline glyphs.
  With `monocolorEmoji`, the fallback chain prefers a monochrome emoji face and colour glyphs are
  drawn as plain outlines in the text colour.
- Quality: 4 horizontal subpixel bins, vertical pixel snapping, sizes in physical pixels (ppp is a
  cache-key input already), grayscale AA (egui composites premultiplied RGBA). Hinting off on
  macOS/HiDPI; light hinting optional at 1×.
- Dark mode / gamma: coverage → alpha with egui's own curve (`Visuals::text_alpha_from_coverage`)
  so theme text matches M1; optional stem darkening for light-on-dark.
- Performance: one raster per glyph key; per frame only quads for visible lines (cached mesh per
  text prim keyed by atlas generation replaces today's per-frame galley re-request).
- wasm: identical.

**(c) vello_cpu rasterises whole blocks into textures (WordCraft).** Best AA, COLRv1, export
parity; but every visible block is a texture (a 1100×780 window at 2× ≈ 13 MB RGBA per screen plus
scroll margin), each keystroke re-rasterises and re-uploads the focused block, every
width/theme/ppp change re-rasterises everything visible, caret/selection/IME still need an
overlay. **Rejected** for the live surface; reasonable for PNG export/thumbnails.

**Decision: (b)**, atlas owned by `markise-ui-egui`, fed by glyph runs from `markise-text`.

### 3.1 Memory budget (NFR-6)

M1 desktop: ~118 MB RSS with a 1 MB document against a 130 MB budget; the bare eframe window is
~89–96 MB. The text subsystem change therefore gets **≤ 10 MB net** at 1 MB, measured as an RSS
delta against the epaint path on the same document and window (T3 gate). Planned allocation:

| Item | Budget | Notes |
|---|---|---|
| Monochrome glyph atlas | 1024² RGBA = 4 MiB GPU + ≤ 4 MiB CPU staging. *[T0]* **start at 512²** (1 + 1 MiB), grow 512² → 1024² → 2048² on demand | egui textures are RGBA; LRU eviction by page first, growth to 2048² (16 + 16 MiB) only if eviction thrashes, and only within the §3.1 delta; CPU staging limited to dirty regions. S1 measured the 1024² page alone at ≈ 7–11 MB RSS (fails the budget with discovery: Δ +13.6…17.4 MB); with 512² Δ = +6.0…7.5 MB. S4: the whole 10 MB corpus used 6.3 % of a 1024² page at 1× (26 % at 2×) |
| Colour emoji atlas | 512² RGBA = 1 MiB GPU + 1 MiB staging, max 1024² | emoji capped at 2× the text size in px |
| Font data | mapped, not owned | only touched pages count; fallback faces mapped on demand |
| Shaped layouts (`TextBlock`s) | inside the existing 16 MB layout-cache bound | parley layouts may cost more per char than galleys: measure in T1, adjust the bound |
| epaint's own font atlas | shrinks | document text leaves it; chrome text stays |

On Apple unified memory GPU textures count towards process memory; the T3 gate measures RSS, not
estimates. *[T0] S1 attribution (eframe window, one paragraph, macOS):* atlas page 1024² ≈ 7–11
MB, system discovery (fontique collection + face-name index scan) ≈ 2–4 MB, parley/fontique/ICU
code and data + three decoded WOFF2 faces ≈ 3.5 MB. If the budget cannot be met, the atlas shrinks first (smaller pages, more eviction),
then the layout cache.

## 4. Decision (proposed)

1. **Shaping, fonts and paragraph layout: parley + fontique** (with `complex-scripts`, subject to
   the T1 size check) in a new GUI-free crate **`markise-text`** (L2, below `markise-layout`;
   depends on parley, fontique, skrifa, `wuff`, `memmap2`; no egui). No dependency on
   `wordcraft-fonts`; vendor its WOFF/WOFF2 unwrapping (and later MATH/COLRv1 helpers) with
   attribution in `NOTICE`.
2. **Rendering: glyph atlas in `markise-ui-egui`** (`text_paint` module), swash + vello_cpu
   rasterisers behind `GlyphRasterizer` (*[T0]*: vello_cpu alone, §3).
3. **epaint galleys stay as the implementation and runtime rollback** of the new seam until the
   gates of T8 pass (editing/IME prototype, accessibility, platforms, memory, scrolling).
   Window chrome keeps egui text (TH-9).
4. When implemented, update `99-open-questions.md`: **C3** changes from "declaration order" to
   CSS Fonts 4 semantics (§4.1); TH-1/TH-2 closed by T4; new C-items for the Linux font policy
   (§4.6) and `complex-scripts`.

### 4.1 Adapter design (`markise-text`)

```text
markise-text
├── store.rs     FontStore: fontique Collection (built on a worker), theme faces, generation;
│                bounded font-miss worker (see "font misses")
├── face.rs      @font-face loading: src list tried in order, first that loads wins —
│                local("…") by full/PostScript name via the adapter's FaceNameIndex (below);
│                file: confined (TI-9), size-capped,
│                ttf/otf/ttc/woff/woff2; data: later; remote never. Each loaded face registered
│                under a private name "mk-ff-<n>" (so fontique never matches it implicitly)
├── matching.rs  CSS Fonts 4 §5 face matching per (CSS family, requested stretch/style/weight)
├── style.rs     TextStyle { stack, size, weight, italic, color: u32, decorations,
│                letter_spacing, features, valign }  (no egui types)
├── styled.rs    StyledText { text, runs: Vec<(Range, StyleId)>, inline_boxes, links,
│                base_dir, context: ChunkContext (§4.2) }
├── itemize.rs   resolves, per grapheme cluster, the face list for each run (see below) and emits
│                parley style runs with explicit family lists + FontVariations
├── layout.rs    TextLayout (Arc): wraps parley::Layout<StyleId>; lines, metrics, glyph_runs()
│                → GlyphRun { font: FontKey, size, synthesis, coords, glyphs, style }; decorations
├── caret.rs     LocalPos { byte, affinity }; hit, caret_rect, selection_rects, moves, line_at,
│                x_on_line, hit_line_at_x(first|last)
└── preedit.rs   composition overlay (§4.4)
```

**CSS font matching (replaces the earlier "one synthetic family per face" idea).** For each style
run and each family name of its CSS stack, in order:

1. Collect the `@font-face` rules of that family (theme faces), else the system family of that
   name (fontique), else a generic mapping (`serif`, `monospace`, …).
2. **Face-set selection** by CSS Fonts 4 §5.2 on descriptors only, never on coverage: stretch,
   then style (`italic` → `oblique` → normal, per spec), then weight with the range-aware rules
   (requested weight inside a face's `font-weight` range matches exactly; otherwise the spec's
   400/500 and lighter/heavier search). Result: the set of rules sharing the winning descriptor
   values (the unicode-range composite face).
3. **Candidate order within that set**: rules whose `unicode-range` includes the cluster's
   first character, checked in **reverse declaration order** (CSS: for overlapping ranges with
   equal descriptors, the last-defined rule is checked first); then the next family in the stack;
   then system fallback (fontique per-script); then last resort.
4. **Cluster matching per CSS Fonts 4 §5.3**, per grapheme cluster, over that candidate order:
   (a) the first face that *supports the whole cluster* wins — supports means: a glyph for every
   character, or for the cluster's canonically equivalent composed form (NFC) when that differs;
   a variation-selector sequence counts as supported if the face maps the sequence (cmap format
   14) or, failing that in every candidate, the base character alone (the selector is then
   ignored for matching; VS16/VS15 additionally bias emoji vs text presentation per R11);
   (b) if no candidate supports the whole cluster, find the face supporting the **longest
   leading sequence** of the cluster (base plus as many following characters as possible, again
   counting canonical equivalents), render that sequence with it, then match each remaining
   combining character separately (CSS Fonts 4 §5.3): every such mark **re-runs full
   single-character matching** — steps 1–3 again, i.e. family stack, face-set selection by
   stretch/style/weight, and `unicode-range` filtering against **that mark's** code point (not
   the base character's), then system fallback;
   (c) characters no candidate supports go to the last resort. A font change inside a cluster
   (only in case b) splits the shaping run but **never the grapheme**: caret stops, hit testing
   and selection stay on grapheme-cluster boundaries, with the cluster's glyphs from several runs
   treated as one cluster in the caret map. Otherwise face changes split runs only at cluster
   boundaries; adjacent
   clusters with the same face stay in one run (Arabic joining, kerning). NFC substitution is a
   matching decision only: the text and its offsets are never rewritten — the shaper's own
   normalisation (harfrust composes/decomposes for the chosen face) does the rest. Gated in T4
   by the tests below; parts parley/fontique cannot express (e.g. cmap-14 checks) are done on
   skrifa charmaps in the adapter.
5. The run gets an explicit family list ("mk-ff-7, mk-ff-3, <system fallback families>") plus
   `FontVariations` clamping `wght` (and `slnt`/`ital`) into the matched face's descriptor range
   for variable faces; synthesis only when the matched set has no face of the requested style.

Tests (T4): two faces (regular, bold) of one family covering identical characters → bold text
uses the bold face, no synthesis; overlapping `unicode-range` faces → last-declared wins;
variable face with `font-weight: 100 900` plus a static 700 face → at 650 only the variable
face matches exactly (instance `wght` 650); at 700 both match and the later-declared rule is
checked first (assert the spec's outcome, not a preference); `src: local(missing), url(ok.woff2)`
→ woff2 loaded; `src: local("<installed full name>")` → that exact face, by full name and by
PostScript name, including a non-regular face of a family; confined-path rejection; two faces of
one family with **disjoint** `unicode-range`s — A covers only the base character's range, B only
the combining mark's range — → the base from face A, the mark from face B via the mark's own
single-character matching (rule 4b), one caret stop for the whole cluster; decomposed
`e` + U+0301 in a face that only has `é` → supported via NFC; emoji + VS16 prefers the colour face,
+ VS15 the text face; Arabic word whose letters all fall in one range stays one run.

*[T1, 2026-10-27] Implemented in `crates/markise-text`* (`store.rs`, `matching.rs`,
`coverage.rs`, `face.rs`, `names.rs`, `layout.rs`): steps 1–5 including the reverse-declaration
candidate order, NFC support, cmap-14 sequences and VS15/VS16 presentation bias, and rule 4b.
Rule 4b needs one trick: parley picks one font per *cluster* using the cluster's first
character's style, so a mark split off a grapheme gets its own style range with a private,
shaper-ignored feature tag (`mksp`), which forces a shaping-item boundary; caret stops stay on
grapheme boundaries because `TextLayout` snaps every parley cursor to them. System families that
precede a theme family in a stack are coverage-checked through fontique (cached); after the last
theme family the rest of the stack is handed to parley unchanged (equivalent and faster). Not yet
implemented: the bounded font-miss worker (fontique lookups for families not in the first theme
pass can still happen on the calling thread) and `data:` sources. A `local()` hit inside a
TrueType collection is copied into a standalone sfnt (fontique registers every face of a blob
under the override name), which owns that face's bytes. Theme-face loading must move to a
worker in T4 (S1: three WOFF2 decodes took 9.2 ms on the UI thread).

**FaceNameIndex (adapter-owned).** fontique 0.12 looks up *families*, not individual faces by
full or PostScript name, so `local()` needs our own index: name-table entries (full name ID 4,
PostScript name ID 6, typographic family/subfamily) → (file path, face index), built off the UI
thread from the platform font folders — the same scan ADR 0004's index already performs — and
memory-mapped on use. On macOS/Windows it may lag fonts installed while the app runs (rescan on
theme reload). Gated in T4 by the successful-`local()` tests. *[T1]* Implemented with skrifa's
name table reader (full name, PostScript name, typographic family + subfamily); it duplicates
the scan of ADR 0004's `markise_ui_egui::fonts::SystemFonts` (L6, ttf-parser) — move that index
down into `markise-text` in T4 so there is one scan.

**Seam in `markise-layout`** (replaces the epaint-typed `Shaper`): `trait TextShaper { fn
shape(&mut self, text: &StyledText, width: Option<f32>) -> Arc<dyn TextBlock>; fn
generation(&self) -> u64; }`, `trait TextBlock { size, lines, hit, caret_rect, selection_rects,
cost, as_any }`, `Prim::Text { pos, frag: FragmentId, text: Arc<dyn TextBlock> }`.
Implementations: `EpaintShaper` (wraps galleys) and `ParleyShaper`. `link_at`/`cost` go through
`TextBlock`. `InlineBuilder` builds `StyledText` instead of `LayoutJob`. `CacheKey` already has
the font generation; `BlockLayout::cost` uses `TextBlock::cost`. The atlas has its own bound.

*[T2, 2026-10-27] Implemented* in `crates/markise-layout/src/text/` with these refinements:
- The shaping input is a layout-level `StyledText` (text, contiguous runs of `TextStyle` =
  size, line height, `FontRole` body/heading/mono × regular/bold, italic, colour, background,
  underline/strike, `valign`, letter spacing, plus a per-run *lead* = extra space before the run,
  which inline-box padding, TOC indentation and justification use), `halign` and
  `break_on_newline`; `shape(&StyledText, width: Option<f32>)`. Backends map it to their own
  input (epaint `LayoutJob`; `markise_text::StyledText` with leads as inline objects). Keeping
  the role abstraction (not CSS stacks) at this level is what makes the epaint conversion exact.
- `TextBlock` (object-safe, `Send + Sync`): `text`, `size`, `rect` (epaint's anchor convention:
  centred rows straddle x = 0), `line_count`, `lines` (byte range, row origin/size, baseline,
  hard break), `first_line_height`, `half_leading`, `clusters` (visual order, byte, first char,
  x, advance — justification works on these, backend-neutrally), `ink_spans(range)` (geometry of
  decoration boxes/strike/underline), `hit`, `char_at` (link hits), `caret_rect`,
  `selection_rects(LocalPos, LocalPos)`, `source_map()` hook (always `None` until T7), `cost`,
  `as_any`. `TextShaper` also has `has_glyphs(style, s)`, `lacks_glyphs(block)` and
  `generation()` (mixed into the cache key; 0 for epaint, salted for parley).
- `Prim::Text { pos, frag: FragmentId, text: Arc<dyn TextBlock> }`; `FragmentId { block (item-
  relative offset, 0 until T7), kind: Para | Cell{row, col} | CodeChunk(n) | Run(n) | Label,
  offset (row start inside a justified paragraph) }` (Q10 adopted).
- `EpaintShaper` is egui's `FontsView` itself (`impl TextShaper for FontsView`), so callers pass
  the same object as before. The painter downcasts to `EpaintBlock` and re-requests the galley
  exactly as M1 did.
- `ParleyShaper` (part of T3 pulled forward to prove the seam end to end): shapes with
  `markise-text` over the *same faces* as the epaint families (each egui face registered as its
  own fontique family, no byte copy, epaint's fallback order), at scale = pixels per point;
  `markise-ui-egui::text_paint` paints its glyph runs through a vello_cpu atlas of 512² pages
  (ported from the T0 spike). Switch: `MARKISE_TEXT=parley` (`AppOptions::text_backend`, default
  epaint). Gaps until T3–T5: no `valign` raise, no run backgrounds, epaint `FontTweak`s ignored,
  no pixel snapping of rows, no atlas eviction, synthetic italics look rough, no CSS face matching
  (T4).
- *Review round 1 (Codex, PR #10):* justification sorts the space offsets it collects (clusters
  arrive in visual order, so RTL/mixed rows produced descending cuts and a slice panic on the
  parley path); `EpaintBlock` keeps soft-wrap affinity both ways (`hit` reports epaint's
  `prefer_next_row` as downstream/upstream, `caret_rect`/`selection_rects` honour it); the parley
  font registration keeps the face index of a collection file (the selected face is copied out
  into a standalone sfnt instead of registering every member under one alias);
  `ParleyBlock::cost` counts glyph runs (an estimate until they are built on first paint).
  Seam tests now also cover soft-wrap affinity (both backends), justified RTL and mixed-bidi
  paragraphs, bidi carets/selection, ligatures, control characters, inline boxes, multiline
  alignment and collection faces.
- *Decisions (owner/coordinator, 2026-10-27):* parley stays linked into the default binary
  (+4.9 MB; no epaint-only build has been asked for and T3 needs it). The atlas has no eviction
  or growth bound yet: **eviction and the §3.1 RSS gate are T3 acceptance criteria** before the
  parley path can be promoted beyond the developer switch.

**Font misses.** A fallback face that is not mapped yet, or a family not yet indexed, must not
block the UI thread (async collection construction alone does not prevent later lazy lookups,
mmaps and page faults). The shaper consults only faces already available; a miss enqueues the
lookup/mmap on one bounded worker (queue ≤ 64, newest wins), the run is shaped with the last
resort meanwhile, and completion bumps the font generation, relayouting the visible items.
Prefetch: theme faces and the per-script fallbacks of the document's scripts after load.

### 4.2 Logical paragraphs vs render chunks

Today a paragraph line over 4 KiB is cut at a char boundary (`split_units`), which can split a
combining sequence, a ZWJ emoji or an Arabic word, and each piece is analysed alone. The new path
separates two notions and keeps paragraph-level analysis paragraph-wide:

- **Logical paragraph** = for bidi, a hard line of a Markdown block (hard and soft line breaks
  are rendered as line breaks, M1-2, and a newline is a UAX #9 paragraph separator, class B), a
  table cell, or a code line. The Markdown block's base direction (first strong character of the
  block, or a future explicit direction) is stored in the item and applies to all its lines.
- **Render chunk** = the existing segment item (64 lines / 16 KiB groups of **whole lines**) and,
  only for single lines over 4 KiB, a *piece* of a line. Segment items never cut a logical
  paragraph, so for them parley's per-layout analysis is exact. Only pieces need more.
- **Pieces — target design.** For a line over 4 KiB the adapter runs the paragraph-level
  analyses **once over the whole line** (bidi levels incl. bracket pairs and isolates via
  `unicode-bidi`; script, grapheme, word and line-break boundaries; face itemisation, §4.1), at
  model build or on edit of that line, and caches them with the line (O(line length)). Each piece
  is then laid out from that analysis rather than re-analysed: its slice of resolved levels, and
  shaping with **pre/post context** (the neighbouring characters, as HarfBuzz/harfrust buffers
  support via `set_pre_context` / `set_post_context`) so joining and contextual forms continue
  across the cut. A cut is a forced visual line end; reordering (UAX #9 L1–L4) is per visual line
  and needs only that line's levels.
- **Feasibility gate (unresolved).** parley 0.12's builder takes a base direction but **no
  externally resolved levels and no shaping context**. Options, to be decided by spike **S5**:
  (a) contribute a "precomputed analysis / context" input to parley upstream; (b) an **unproven
  experiment**: approximate the resolved levels inside each piece with inserted directional
  controls (isolates or embeddings, invisible, offsets remapped). Naively wrapping maximal
  same-level runs in sibling isolates is *not* equivalent — it loses nesting (levels 1, 2, 1
  reorder differently under UAX #9 L2) — so a correct encoding must rebuild the nesting, and
  shaping context would still stop at the cut; S5 decides whether it works at all; (c) lay out pieces with our own line assembly on top of
  harfrust shaping (with context) for this path only. Until S5 passes, pieces are an explicit
  **degraded mode**: each piece is analysed alone with the block's base direction.
- *[T0] S5 result (200 KiB generated line, 50 pieces, normal and forced cuts; README; oracle
  revised after review):* option (b) is feasible in parley 0.12 on this input — RLE/LRE steps
  plus a final RLO/LRO per run of equal level, base-level runs opened by LRM/RLM, encoding the
  piece's **line** levels (whole-line UAX #9 levels with the L1 line-end reset applied by the
  adapter; encoding pre-L1 paragraph levels left 34 mismatches at piece ends) — gave **0**
  visual-order mismatches against an independent L1+L2 reference over 123,132 source graphemes
  and **0** mirroring mismatches, for both cut sets (degraded mode: 64 / 26 mismatches, all
  within 256 B of a cut). One input and one font: feasibility evidence, not proof of
  equivalence. Shaping context is still lost where a cut splits a joining word: at forced cuts
  9/49 straddling Arabic words kept the unchunked layout's glyph ids in degraded mode and,
  measured separately, with (b); harfrust with `set_pre_context`/`set_post_context` (option c)
  kept 49/49 (vs harfrust on the whole word). Whole-line re-analysis took **10.3–10.6 ms**
  (bidi 6.5, graphemes 2.2, line breaks 1.2–1.4, scripts 0.3), five times the 2 ms gate. **The
  S5 gate fails as specified.** Proposed design instead:
  1. levels for pieces via option (b) with L1 applied per piece by the adapter (no parley
     change needed); to be confirmed by the §4.2 boundary tests in T5;
  2. normal cuts at spaces outside isolates/brackets (they never split a joining word, so no
     context is needed); forced cuts (no break opportunity within 64 KiB) keep the documented
     shaping-context loss, until parley grows a pre/post-context input (option a, upstream
     proposal) — option (c) only if a real document needs it;
  3. the over-long line's analysis runs off the UI thread after an edit (pieces shaped in
     degraded mode until it lands, then the visible pieces are reshaped), or incrementally
     per paragraph-level isolating run sequence; recorded as TS-1 in `99-open-questions.md`.
- **Cut positions.** Preferred: a grapheme-cluster boundary that is a UAX #14 break opportunity
  after a space, outside explicit embeddings/isolates (`LRE…PDF`, `LRI…PDI`), outside bracket
  pairs that contain strong characters, and outside inline-markup spans, searched within 4–8 KiB.
  If none exists the line is not cut until **64 KiB**, where a **forced cut** at a
  grapheme-cluster boundary is made. A forced cut *can* split an Arabic joining sequence, a
  nested isolate or a bracket pair; with the S5 design that only costs a visual line end (levels
  and shaping context are preserved), in degraded mode it also changes joining forms, isolate
  levels and bracket resolution next to the cut. Such lines (64 KiB without a space or break
  opportunity outside isolates) are pathological (minified or encoded data); the trade-off bounds
  per-item layout cost and is documented, not hidden.
- **Bounded invalidation**: an edit re-shapes only its chunk, unless it (a) changes the block's
  first strong character → all chunks of the block (the base direction is part of the item hash),
  or (b) touches an over-long line → the line's analysis is recomputed (O(line length): a cost
  risk for NFR-4 on multi-MB single lines, measured in S5) and only pieces whose levels,
  boundaries or context windows changed are re-shaped.
- Boundary tests (T5, both for the S5 design and for degraded mode where they document the
  degradation): combining sequence, ZWJ family emoji, regional-indicator flag and an Arabic
  joining word straddling 4096 bytes and the 64 KiB forced cut; nested isolates
  (`RLI … LRI … PDI … PDI`) spanning a cut; bracket pairs `( … )` whose opening and closing sides
  fall in different pieces (UAX #9 N0); an RTL block whose later pieces start with neutrals or
  digits; an edit that flips the first strong character; inline markup spanning a cut (existing
  approximation, ADR 0002).

### 4.3 Document caret model (M2)

- **Canonical position**: `DocPos { source: usize, affinity, virt: u16 }` — a byte offset into the
  Markdown source, stable across model rebuilds and transformed by edits (`markise-doc`), plus
  ADR 0006's format-neutral virtual-slot discriminator (`virt` = 0 for source positions, *k* > 0
  for the *k*-th editor-only slot at that offset: missing table cells, the virtual empty
  paragraph; ADR 0006 §2.2, Q10 — adopted here). Item indices are never stored in carets.
  `markise-text` itself only knows `LocalPos { byte, affinity }` within one layout.
- **Fragments**: every `Prim::Text` carries a `FragmentId { block: offset of the block start
  **relative to its item** (ADR 0006 §2.3, Q10: item-relative source maps keep cached layouts
  valid when text above changes), kind: Para | Cell{row, col} | CodeChunk(n) | Run(n) (text between inline images) |
  Label }` and a source map (fragment byte ↔ source byte, with hidden/revealed markers). A
  `BlockLayout` lists its fragments in reading order (tables row-major, spec 02 §6).
  `DocPos → (item, fragment, LocalPos)` and back go through the model's offset index
  (`item_at_offset`) plus the fragment source maps; positions inside hidden markers map to the
  nearest visible boundary with the affinity of the side they came from.
- **Goal x** in document content coordinates: fragment x + item origin + container indent −
  horizontal scroll of a code block. Up/Down: line above/below in the fragment at goal x; at the
  fragment's first/last line, the previous/next fragment in reading order (laid out on demand;
  bounded by segmentation), hitting its last/first line at goal x. Left/Right at fragment edges
  cross to the neighbour (spec 02 §5.4 code-block exits; table cells per §6).
- **Ambiguity**: hit/caret round trips are exact except at bidi run boundaries and soft-wrap
  ends, where one offset has two visual positions; affinity disambiguates and tests assert the
  pair, not a single rect.
- Tests (T7): empty paragraph and empty cell; cell-to-cell and leaving the table; code chunk
  boundary at line 48; text–image–text runs; caret entering a styled span (marker reveal shifts
  layout but not `DocPos`); model rebuild/resegmentation keeps the caret; selection across an
  image, a table and a code block; goal x preserved across an indented list and a horizontally
  scrolled code block; wrap-end and bidi-boundary affinity.

### 4.4 IME composition

State on the focused document: `Composition { anchor: DocPos, replaced: Option<Range<DocPos>>,
preedit: String, cursor: Option<Range<usize>>, phase: Composing | Ending }`.

- **Start**: the first non-empty `Preedit` (or `Enabled` followed by one) creates the state. A
  non-empty selection becomes `replaced`; it is **not** deleted. Every fragment that the range
  touches renders its part as suppressed (each visible fragment's `StyledText` carries the
  suppressed byte ranges; off-screen fragments are not laid out); block structure is unchanged
  until commit, so a selection spanning several blocks shows the preedit at `anchor` in the first
  fragment and the remaining selected text hidden in place (an approximation of the post-commit
  look, documented).
- **Update**: the focused fragment's `StyledText` is re-shaped with the preedit inserted at
  `anchor` (underlined run; an offset map keeps positions outside it unchanged).
  `PlatformOutput::ime = IMEOutput { rect, cursor_rect }` every frame while composing. egui 0.33
  provides no cursor range, so `cursor` is `None` and the caret sits at the end of the preedit.
  *Re-checked 2026-10-27:* stock egui 0.37 has `ImeEvent::Preedit { text, active_range_chars }`;
  egui-winit 0.37 converts winit's byte range to characters (an invalid range is dropped, and on
  Windows an empty range at the start is dropped as a workaround for the built-in Korean IME), so
  `cursor` can come from `active_range_chars` once Markise is on 0.37. `IMEOutput` also gained
  `purpose` and `should_interrupt_composition` (egui-winit interrupts by toggling
  `set_ime_allowed`).
- **Clearing the display is not cancelling.** winit documents an empty preedit immediately
  before a commit, and egui-winit 0.33.3 translates `Preedit("", None)` into
  `ImeEvent::Disabled`; egui-winit 0.37 passes it through as `Preedit { text: "", .. }` and no
  longer emits `Enabled`/`Disabled` (both deprecated). So an empty `Preedit` (or, on 0.33,
  `Disabled`) only removes the preedit from the display and moves the state to `Ending`;
  `anchor` and `replaced` are kept.
- **Commit**: `Commit(text)` in `Composing` or `Ending` → one edit replacing `replaced` (or
  inserting at `anchor`), undo boundaries before and after (spec 02 §11); state cleared.
- **Final cancel** happens only when the state is `Ending` and one of these occurs before any
  `Commit`: a non-IME key or pointer event, the end of a frame that delivered no IME event after
  the `Ending` transition (assumption: a commit arrives in the same event batch as its clearing
  `Preedit`/`Disabled`; unverified — S2 logs the real sequences, and if commits can arrive later
  the window is widened to the next input event), focus loss, document switch or mode switch. Cancel restores the display of
  `replaced`; no edit, no undo step. A `Preedit` with text in `Ending` resumes `Composing`.
- Key events consumed by the IME are not interpreted as editing commands while composing.
- *[T0] S2:* the state machine above, an inline preedit as its own underlined parley style run,
  suppression of the replaced selection, `IMEOutput` at the caret every composing frame and the
  0.37 active range as the preedit caret pass 6/6 injected-event tests through a real
  `egui::Context` (`spikes/adr-0005/t0/s2-ime-log -- selftest`). egui 0.37 also has
  `ImeEvent::DeleteSurrounding`, but egui-winit 0.37 never emits it (winit 0.30 has no such
  event), so the E21/P7b limitation stands. **Real-IME sequences are not recorded yet** (owner
  run of `s2 window` with Kotoeri/Pinyin/Korean; the log answers whether `Commit` arrives in the
  same frame as the clearing `Preedit("")`, which the cancel rule assumes).
- **Tests** (injected events, T7): `Preedit("か") → Preedit("") → Disabled → Commit("家")` with a
  selection → the selection is replaced by 家 in one undo step; `Preedit → Preedit("") → Disabled`
  then a frame without IME events → cancel, selection text intact; `Ending` then `Preedit("x")`
  resumes; focus loss and document switch while composing; a selection spanning two paragraphs
  and a table cell (suppressed in all three fragments, single edit on commit).
- **Gate**: those tests plus **manual real-IME tests before M2 acceptance**: macOS Japanese
  (Kotoeri), Simplified Chinese Pinyin, Korean; Windows (MS-IME) and Linux (IBus or Fcitx) before
  release. The web build's text agent is checked in the wasm smoke (T6).

### 4.5 Accessibility

Custom-painted text is invisible to AccessKit unless we publish it. Milestone T8: one node per
visible text fragment (role paragraph/heading/list item/cell, text value, character bounds),
caret and selection exposed as text selection, actions (set selection, scroll into view), node
ids derived from `FragmentId` (stable across frames and relayout), virtualised navigation
(off-screen blocks represented by placeholder nodes or exposed on demand when the reader moves
past the viewport). Tested with VoiceOver; NVDA and Orca before release.

### 4.6 Platform policy

- **macOS / Windows**: fontique system backends (CoreText / DirectWrite — OS APIs, like winit's).
- **Linux (decision needed, recorded as a C-item)**: proposal — build fontique without `system`
  on Linux and feed it from our own memory-mapped font-folder index (pure Rust, shared with
  ADR 0004's index), with an explicit generic-family table and a per-script fallback table
  (Noto/DejaVu/WenQuanYi/… families, locale for Han unification); `fontconfig-dlopen` stays an
  opt-in feature for users who need fontconfig rules. fontconfig would still be a C library at
  run time, which is why it is not the default.
- **wasm**: fonts provided as bytes (theme files + optional approved bundled faces); a smoke test
  in a headless browser that loads fonts, shapes, paints through the atlas and exercises egui's
  IME text agent — compiling is not enough.

## 5. Migration plan (small mergeable milestones)

Each milestone keeps `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace` green,
adds benches for what it introduces, and reports numbers against the NFRs.

| # | Milestone | Tests | Acceptance |
|---|---|---|---|
| T0 | Spikes S0–S5 (§6); update this ADR; owner decision | spike README per spike | thresholds of §6 met or ADR revised/rejected |
| T1 | `markise-text`: `FontStore`, matching, `StyledText`, `TextLayout`, `hit`/`caret_rect`/`selection_rects`. **Test fonts**: synthetic fonts generated in tests with `write-fonts` (fontations, Apache/MIT) for metrics, ranges, weights, ligature lookups; real-script tests use system fonts when present and are skipped (reported) otherwise; `epaint_default_fonts` only after M1-5 is approved | breaks at fixed widths; grapheme-only caret stops; hit↔caret round-trip property test with the affinity rule of §4.3; ligature on/off; Thai/Khmer breaks if `complex-scripts` | builds on 1.89 and `wasm32-unknown-unknown`; committed bench harness (S0 inputs) with raw numbers; binary-size delta reported |
| | *[T1 status 2026-10-27]* | done except: wasm build checked (`cargo check --target wasm32-unknown-unknown`), not executed; `complex-scripts` costs **+3.1 MB** binary (5.63 vs 2.54 MB spike binary): kept as a default-on crate feature, owner to confirm (TS-2); Linux uses fontique `system` + `fontconfig-dlopen` as an interim until T6 (TS-3); bench harness `crates/markise-text/benches/shaping.rs` | |
| T2 | Seam refactor in `markise-layout` (`TextShaper`/`TextBlock`/`FragmentId`); `EpaintShaper` only; no visible change | existing tests unchanged; link hits via `TextBlock` | kittest screenshots identical to main; viewer bench ±10 % |
| | *[T2 status 2026-10-27]* | done: seam + `EpaintShaper`, plus an interim `ParleyShaper` and atlas painter behind `MARKISE_TEXT=parley` (above). Existing tests unchanged except how they reach the galley (`Prim::Text { text, .. }` + `text.galley()`); new seam tests run the same caret/hit/selection/wrap contract on both backends (`crates/markise-layout/tests/seam.rs`) | `theme_shots` (all 104 shots, every built-in theme light/dark) byte-identical to `main` on repeat runs (a cold first run once showed ≤ 1-level deltas in ~100 px of 6 shots that did not reproduce; harness nondeterminism, not seen on `main` reruns); viewer bench within noise on a loaded machine; warm-process probe of the 1 MB open: doc frame 4.8 → 5.0 ms (+3 %); app binary +4.9 MB (parley, fontique, ICU4X data, glifo) |
| T3 | Glyph atlas + `ParleyShaper` behind a runtime switch (default epaint): paragraphs, headings, lists, quotes | glyph-run snapshots; atlas eviction/regeneration; kittest screenshots per shaper | NFR-3/5 benches pass on the parley path; RSS delta ≤ 10 MB (§3.1) |
| T4 | Theme fonts on the parley path (§4.1), C3/TH-1/TH-2; ADR 0004 index reused (coordinate with the themes owner) | §4.1 matching tests; importer → FontStore integration | built-in themes render with their `@font-face` files when present |
| T5 | Remaining text on parley (tables, code, TOC, footnotes, front matter), chunk boundaries (§4.2: S5 design or the documented degraded mode), colour emoji incl. COLRv1 path and `monocolorEmoji` | per-kind tests; §4.2 boundary tests; UAX #9 selection-rect tests | **platform gate**: screenshot + fallback checks on macOS (sbix), Windows (Segoe UI Emoji COLR, YaHei) and Linux (Noto Color Emoji, Noto CJK) before parley becomes the default |
| T6 | wasm smoke (§4.6), Linux policy implemented | headless-browser test | fonts from bytes, shaping, atlas, IME agent work in the browser |
| T7 (M2) | Editing on the parley path: `DocPos`/fragments, moves, cross-fragment up/down, selection, IME (§4.3–4.4) | §4.3 and §4.4 tests; spec 02 §5.4/§10 scenarios | NFR-4 bench p99 < 16 ms in the 10 MB corpus; real-IME gate passed on macOS |
| T8 | Accessibility (§4.5) | AccessKit tree tests; screen-reader checks | VoiceOver usable for read, caret, select |
| T9 | Remove the epaint text path and the switch | full suite | only after T5–T8 gates; perf/RSS report; ADR 0002/0004 and NOTICE updated |

## 6. Spike plan (T0) with go/no-go thresholds

- **S0 — matched harness.** One harness per candidate (already in `spikes/adr-0005/`, to be
  folded into T1's bench) with identical input, font, width, scale; cold and warm, ≥ 10 rounds,
  distribution reported; plus a shape+raster variant. *Go*: parley within 3× epaint per shaped
  KiB and first screen of the 10 MB corpus cold < 10 ms (NFR-3 headroom). The current "~2×"
  figure is a hypothesis until then.
- **S1 — theme font paragraph end to end.** eframe window; fontique on a worker; a theme family
  of two woff2 faces split by `unicode-range` plus a bold face; shape with parley; paint via the
  atlas; screenshots vs epaint at 1×/2× light/dark. *Go*: start to first interactive frame stays
  < 250 ms with the store still loading; paragraph painted within 300 ms of start; RSS delta
  ≤ 10 MB vs the epaint path; correct face per weight/range in the screenshot.
- **S2 — IME.** macOS Japanese and Chinese: log egui `ImeEvent` sequences (0.37 since 2026-10-27); inline preedit in a
  parley layout; candidate window at the caret; commit/cancel/selection replacement per §4.4.
  *Go*: all four behaviours correct on macOS; otherwise fix the egui integration before T7
  (this does not depend on the shaper).
- **S3 — bidi line.** "Hello مرحبا بالعالم 123 world" with LTR and RTL base: visual/logical arrows,
  selection rects across direction changes, hit tests at both edges of each run. *Go*: selection
  rects non-overlapping and gap-free; every cluster boundary round-trips under the §4.3 affinity
  rule; arrow sequences match UAX #9 reordering (the first spike printed byte 6 → 36 for a visual
  step; unverified).
- **S4 — atlas under scrolling.** 10 MB corpus, wheel and scrollbar jumps; a COLRv1-only emoji
  font. *Go*: frame CPU p99 ≤ 4 ms (half the 120 fps budget) on wheel scroll and ≤ 8 ms on jumps;
  ≤ 1 mono page + 1 colour page at 1× for the corpus; COLRv1 glyphs drawn in colour via the
  vello_cpu path or by the documented fallback.

- **S5 — chunk context (feasibility gate for §4.2 pieces).** A 200 KiB single line mixing
  Arabic, Hebrew, digits, brackets and nested isolates, cut into pieces: compare per-piece levels,
  mirrored brackets and Arabic joining forms with an unchunked reference layout of the same line
  (wide enough to need no cuts). Try options (a)–(c) of §4.2. *Go*: identical levels, mirroring
  and glyph ids at every cut for normal cuts and forced cuts; whole-line re-analysis on edit
  ≤ 2 ms for a 200 KiB line. *No-go*: pieces ship in the documented degraded mode and the
  limitation is recorded in `99-open-questions.md`.

If parley fails S0, S1 or S3 in a way we cannot fix upstream or in the adapter, fall back to
cosmic-text (§2.3) behind the same seam; S2 failures are egui-integration work either way.

## 7. Spike results so far

### 7.1 T0 (S0–S5), 2026-10-27 — Apple M1 Max, macOS 26.6.2, Rust 1.99.1, egui 0.37

Raw output, commands and caveats: `spikes/adr-0005/README.md` ("T0 spikes").

| Gate | Threshold | Measured | Verdict |
|---|---|---|---|
| S0 throughput | parley ≤ 3× epaint per shaped KiB | epaint 0.37 77.8 µs/KiB; parley fast path 1.59×; parley with CSS theme-face matching 2.33× | GO |
| S0 first screen | 10 MB corpus cold < 10 ms | 1.32 ms (layout of the paragraphs filling 800 px at 2×) | GO |
| S1 start | first interactive frame < 250 ms, store loading | warm 206–229 ms (epaint 188–216); cold first run 307 ms | GO warm; repeat cold in T3 |
| S1 paint | paragraph < 300 ms | warm 210–233 ms; cold first run 310 ms | GO warm |
| S1 memory | RSS Δ ≤ 10 MB vs epaint | 1024² page +13.6…17.4 MB; **512² page +6.0…7.5 MB** | GO with a 512² initial page |
| S1 faces | correct face per weight/range | regular 79 / digits 14 / bold 10 glyphs (expected), screenshots 1×/2× light/dark | GO |
| S2 IME | 4 behaviours on macOS with real IMEs | 6/6 injected-event tests; real-IME logs pending | INCOMPLETE (owner run) |
| S3 bidi | gap-free rects, round trips, UAX #9 arrows | 1184 checks, 0 failures (LTR and RTL base) | GO |
| S4 scroll | p99 ≤ 4 ms wheel / ≤ 8 ms jump; ≤ 1+1 pages at 1×; COLRv1 | 0.36 / 1.31 ms at 1× (0.37 / 1.86 at 2×); 1 mono + 1 colour page; COLRv1 drawn in colour | GO (CPU only) |
| S5 pieces | identical levels/mirroring/glyphs at cuts; re-analysis ≤ 2 ms | (b) with L1 line levels: order and mirroring match an independent reference on this input; glyphs at forced cuts only with (c); re-analysis 10.3–10.6 ms | NO-GO → §4.2 proposal |

None of the parley gates failed in a way that points to cosmic-text (§6 fallback rule): S0 and
S3 pass; the S1 memory miss is the atlas page size; S5's issues are design/time, not parley
defects. **Recommended ADR changes ([T0] marks above):** vello_cpu as the only rasteriser (no
swash); 512² initial atlas page; theme-face loading on a worker; S5 design per §4.2 (option b +
off-thread analysis, upstream context API later); adopt ADR 0006 Q10 (`DocPos.virt`,
item-relative `FragmentId`); keep `complex-scripts` as a feature (owner decision on +3.1 MB);
move ADR 0004's font index into `markise-text` (T4).

### 7.2 First round (2026-10-26)

Raw output, commands, platform and limitations: `spikes/adr-0005/README.md`. Summary (one run,
Apple M-series, macOS, Rust 1.89, release; figures are medians of ten round averages, not
per-operation latency distributions): epaint `layout_job` on galley-cache misses with a warm
glyph atlas 58 µs/KiB (includes CPU rasterisation of atlas misses); wordcraft shape-only 54
µs/KiB; parley build+shape+break 105 µs/KiB — different scopes, same input. wordcraft-fonts: 303
MiB owned font bytes after five system fallbacks (RSS 5.02 → 314.2 MiB). parley: shape-only RSS
17.39 → 19.94 MiB for the same five scripts (no glyph decoding); no `.notdef` in the first mixed
layout; grapheme-sized caret step over a ZWJ family. There is no isolated glyph-raster benchmark,
no texture upload, no GPU rendering and no keystroke-to-pixel measurement. The epaint baseline is
built without egui's bundled fonts; the wordcraft spike binary embeds that crate's OFL fonts.

## Consequences

- One new L2 crate and new transitive crates (parley, fontique, harfrust, skrifa, ICU4X data,
  vello_cpu with `text`/`png` for all glyph rasterisation (*[T0]*: no swash), wuff); `NOTICE` gains harfrust (MIT), ICU4X data (Unicode-3.0) and
  the vendored WordCraft code (MIT/Apache). Build time and binary size grow (measured in T1).
- Text capability: bidi, complex scripts, ligatures, CSS-matched theme fonts, real styles,
  colour emoji. Cost per shaped item is expected to rise (hypothesis ~2×, to be measured).
- We own the atlas, glyph painting, caret/selection overlay, IME composition and document
  accessibility.
- parley's pre-1.0 churn is absorbed by `markise-text`; versions pinned, upgrades deliberate.
- `unicode-range`, CSS face matching, WOFF/WOFF2 and chunk contexts are adapter features we
  maintain and test.
