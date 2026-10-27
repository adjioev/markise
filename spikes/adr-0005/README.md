# ADR 0005 spikes (experimental, throwaway)

Isolated measurement programs behind `docs/adr/0005-text-shaping.md`. Each has its own
`[workspace]` and lockfile, is **not** a member of the Markise workspace, is not wired into the
app and is not production code. Delete once ADR 0005 is decided and the T1 harness exists.

**T0 spikes S1–S5 (2026-10-27)** live in `t0/` (one spike workspace, shared lockfile; S1–S5
use the new `crates/markise-text` crate by path) — see "T0 spikes" below. The first-round
spikes (`epaint-baseline/`, `wordcraft-fonts/`, `parley-text/`) are kept unchanged for the
record; the matched shaping benchmark now lives in `crates/markise-text/benches/shaping.rs`.

| Spike | What it checks |
|---|---|
| `epaint-baseline/` | epaint 0.33.3 `layout_job` throughput on galley-cache misses (the M1 path); built **without** `default_fonts`, Georgia-only font definitions |
| `wordcraft-fonts/` | `wordcraft-fonts` as a git dependency pinned to `6a2b520fa49aa0d0595cea36589aaf16ca371cd3`: resolves/builds on Rust 1.89, shaping throughput, user font registration, ligatures, system fallback faces and owned font bytes |
| `parley-text/` | parley 0.12 (default features: `system`; **not** `complex-scripts`): font registration under a chosen family, build+shape+break throughput, mixed-script fallback, grapheme caret step, hit test, shape-only RSS |

## T0 spikes (S1–S5), 2026-10-27

Platform for every number below: Apple M1 Max, macOS 26.6.2, rustc 1.99.1, release profile,
egui/eframe/epaint 0.37.0, parley/fontique 0.12.0, vello_cpu 0.2.0. One machine, macOS only.

```sh
cd spikes/adr-0005/t0
cargo run --release -p spike-s3-bidi
cargo run --release -p spike-s5-chunk-context
cargo run --release -p spike-s4-atlas-scroll
cargo run --release -p spike-epaint037-check
cargo build --release -p spike-s1-theme-font
./target/release/spike-s1-theme-font prepare <dir>          # writes 3 .woff2 from system fonts
./target/release/spike-s1-theme-font shots <dir> <out-dir>  # headless PNGs, parley vs epaint
./target/release/spike-s1-theme-font window parley <dir>    # opens a window for 3 s, prints RESULT
./target/release/spike-s1-theme-font window epaint <dir>    #   (S1_PAGE=512 sets the atlas page size)
cargo run --release -p spike-s2-ime-log -- selftest          # injected IME events (headless)
cargo run --release -p spike-s2-ime-log -- window ime.log    # manual real-IME recording tool
cargo bench -p markise-text --bench shaping                  # S0 matched harness (repo root)
```

No font files are committed: S1 writes its WOFF2 files from installed macOS fonts at run time
(Georgia, Courier New, Georgia Bold) into a folder you pass; S4's COLRv1 font is synthesised in
memory; T1 tests use synthetic fonts from `markise_text::testing`.

| Spike | What | Gate (ADR 0005 §6) | Result | Verdict |
|---|---|---|---|---|
| S0 | `markise-text` bench vs epaint 0.37 `layout_job`, same 1660-byte input, Georgia 16 px, 700 px | parley ≤ 3× epaint per shaped KiB; 10 MB first screen cold < 10 ms | epaint 126 µs (77.8 µs/KiB); parley fast path 200 µs (1.59×); parley with CSS theme-face matching 293 µs (2.33×); first screen 1.32 ms | **GO** |
| S1 | theme family: 2 WOFF2 faces split by `unicode-range` + WOFF2 bold, fontique discovery on a worker, atlas → egui texture | first frame < 250 ms with store loading; paragraph < 300 ms; RSS Δ ≤ 10 MB vs epaint; correct faces | warm runs: first frame 206–229 ms (epaint 188–216), painted 210–233 ms; **cold first run 307/310 ms**; RSS Δ **+13.6…17.4 MB with a 1024² page**, **+6.0…7.5 MB with a 512² page**; faces correct (screenshots + glyph counts) | **GO with conditions** (512² initial page; theme-face loading off the UI thread; cold-start repeat) |
| S2 | IME on egui 0.37 with an inline parley preedit | Kotoeri + Pinyin real sequences; 4 behaviours correct | §4.4 state machine + inline preedit + `IMEOutput` + active range: **6/6 injected-event tests pass**; **real-IME sequences not recorded** (needs a human at the keyboard with Japanese/Chinese input sources) | **Incomplete** (pending owner run of `s2 window`) |
| S3 | "Hello مرحبا بالعالم 123 world", LTR and RTL base, vs `unicode-bidi` | gap-free non-overlapping rects; boundary round trip; arrows = UAX #9 | 1184 checks, 0 failures (arrows both ways, 870 selection ranges, run-edge hits, round trips) | **GO** |
| S4 | 10 MB corpus scrolling through the atlas; COLRv1-only font | frame CPU p99 ≤ 4 ms wheel / ≤ 8 ms jumps; ≤ 1 mono + 1 colour page at 1×; COLRv1 in colour | wheel p99 0.36 ms (1×) / 0.37 ms (2×); jumps p99 1.31 / 1.86 ms; 1 mono + 1 colour page (6.3 % used at 1×); synthetic COLRv1 gradient drawn in colour by vello_cpu | **GO** (CPU side only: no tessellation/upload/GPU) |
| S5 | 200 KiB mixed RTL line, pieces vs unchunked | identical levels, mirroring and glyph ids at every normal and forced cut; whole-line re-analysis ≤ 2 ms | option (b) with L1-adjusted line levels: 0 visual-order (vs an independent L1+L2 reference) and 0 mirroring mismatches on this input (both cut sets); degraded: 64/26 mismatches near cuts; glyph ids at forced cuts: degraded 9/49, (b) 9/49 (no context), (c) harfrust with context 49/49; **re-analysis 10.6 ms** | **NO-GO as specified** (timing, and glyph context needs (a) or (c)); see ADR §4.2 update |

### Raw results

```text
=== S0: cargo bench -p markise-text --bench shaping  (criterion, 30 samples; medians)
bench font: /System/Library/Fonts/Supplemental/Georgia.ttf
shaping/epaint037-layout_job-1660B        time: [124.50 µs 126.11 µs 128.13 µs]
shaping/markise-text-theme-face-1660B     time: [289.99 µs 293.37 µs 297.20 µs]   (CSS cluster matching on)
shaping/markise-text-fast-path-1660B      time: [198.77 µs 200.10 µs 201.58 µs]   (system/private family)
shaping/caret-rect+hit-per-stop-1660B     time: [3.4705 ms 3.4846 ms 3.4984 ms]  (1661 stops → 2.1 µs/stop)
shaping/first-screen-10mb-cold-layout     time: [1.3073 ms 1.3165 ms 1.3290 ms]  (paragraphs filling 800 px at 2×)
(before the per-run matching context and coverage cache the theme-face path measured 780 µs = 6.2×)

=== S1 (window mode, alternating runs; RESULT lines abbreviated)
parley  first_frame 306.7 painted 310.4 rss_3s 127088 KiB  discovery 193.8 ms (worker) theme_faces 9.19 ms (UI thread) pages 1 upload 4 MiB   <- first (cold) run
epaint  first_frame 220.1 painted 221.3 rss_3s 110176 KiB
parley  first_frame 229.3 painted 233.0 rss_3s 127680 KiB  discovery 153.3 ms theme_faces 9.26 ms
epaint  first_frame 195.6 painted 196.8 rss_3s 109680 KiB
parley  first_frame 206.3 painted 210.0 rss_3s 127568 KiB  discovery 155.5 ms theme_faces 9.23 ms
epaint  first_frame 203.2 painted 204.3 rss_3s 110752 KiB
RSS attribution (two rounds): parley 126288/123824; no discovery 124720/120336; no discovery + 256² page 113792/113808; epaint 110080/110608
S1_PAGE=512: parley first_frame 216.5/209.1/217.5 painted 219.3/211.9/220.3 rss 116176/116672/116816; epaint 188.0/212.0/215.6 rss 110192/109968/109312
shots: s1-parley-{1,2}x-{light,dark}.png: glyphs per theme face ["regular:79", "digits:14", "bold:10"]  (expected: digits 4+10, bold 10)

=== S2 selftest
PASS Preedit(か) → [Preedit(""), Commit(家)] with selection: replaced in one undo step; Cmd+Z restores
PASS Preedit → Preedit("") → frame without IME events: cancel, text and selection intact
PASS Ending then Preedit(かな) resumes; Commit replaces the selection
PASS focus loss while composing: cancel, no edit
PASS active_range_chars 0..2 → caret after 2 preedit chars
PASS IMEOutput set with a cursor rect while composing
S2 selftest: 6/6 passed

=== S3
== base LTR (layout rtl=false, reference rtl=false)   glyph runs: 8, .notdef glyphs: 0
   Right sequence (byte, x): [(0,0.0),(1,16.3),(2,26.0),(3,31.7),(4,37.4),(5,48.2),(6,53.0),(33,61.6),(34,72.8),(35,83.8),(31,88.6),(29,95.4),(27,99.5),(25,104.1),(23,112.0),(21,116.2),(19,120.7),(17,125.6),(16,130.4),(14,135.0),(12,139.9),(10,150.5),(8,160.3),(6,168.2),(36,173.0),(37,187.7),(38,198.5),(39,206.7),(40,212.4),(41,223.9)]
   selection ranges checked: 435; run edges hit-tested: 4; boundaries with two visual carets: 3; checks 590, failures 0
== base RTL (layout rtl=true, reference rtl=true)    glyph runs: 9, .notdef glyphs: 0
   Right sequence (byte, x): [(36,0.0),(37,14.7),(38,25.5),(39,33.7),(40,39.4),(41,50.9),(35,55.8),(33,64.3),(34,75.5),(35,86.6),(31,91.4),(29,98.1),(27,102.3),(25,106.9),(23,114.7),(21,118.9),(19,123.5),(17,128.4),(16,133.2),(14,137.8),(12,142.7),(10,153.3),(8,163.0),(6,170.9),(5,175.7),(1,192.0),(2,201.7),(3,207.4),(4,213.1),(5,223.9)]
   selection ranges checked: 435; run edges hit-tested: 6; boundaries with two visual carets: 4; checks 594, failures 0
== old spike check: Right from byte 6 (auto dir): [36, 37, 38, 39, 40, 41, 41, 41]
S3 TOTAL checks 1184 failures 0

=== S4
== COLRv1
   synthetic font: COLR version 1, glyph is COLRv1 paint graph: Some(true)
   rasterised via vello_cpu: colour entry true, 58×45 px, 56 distinct opaque colours; left quarter RGB sum [122408, 0, 23452], right quarter [24024, 0, 121836]
   COLRv1 gradient drawn in colour (red→blue): YES
   system fonts with COLR v1: []; COLR v0 only: []
corpus: 10485800 bytes, 178211 paragraphs
== scale 1×: first frame 9.65 ms, quads/frame ~1277
   wheel 600 frames: frame CPU p50 0.15 ms p99 0.36 ms max 1.72 ms (layout p99 0.09, atlas+quads p99 0.25)
   jumps 100: frame CPU p50 0.51 ms p99 1.31 ms max 1.48 ms (layout p99 0.73)
   atlas after wheel: 1 mono + 1 colour pages of 1024², 421 glyphs rasterised; after jumps: 1 mono + 1 colour pages, 775 glyphs, 6.3% of page area used; raster time total 9.2 ms (11.8 µs/glyph)
   first 3000 paragraphs: 166036 glyphs, 0 .notdef
== scale 2×: first frame 5.22 ms, quads/frame ~1277
   wheel 600 frames: frame CPU p50 0.15 ms p99 0.37 ms max 0.82 ms (layout p99 0.08, atlas+quads p99 0.29)
   jumps 100: frame CPU p50 0.50 ms p99 1.86 ms max 2.36 ms (layout p99 0.65)
   atlas after wheel: 1 mono + 1 colour pages of 1024², 417 glyphs rasterised; after jumps: 1 mono + 1 colour pages, 829 glyphs, 26.1% of page area used; raster time total 21.5 ms (26.0 µs/glyph)

=== S5  (revised oracle, after review: L1 per piece, independent source inventory, separate glyph measurement)
line: 204824 bytes, 128952 chars
re-analysis 200 KiB (median of 10): total 10.26 ms = bidi 6.52 + graphemes 2.23 + line breaks 1.23 + scripts 0.28
paragraph base level 1 (RTL)
unchunked parley layout of the 200 KiB line: 45.6 ms
== normal cuts: 50 pieces (avg 4096 B)
   degraded      : source graphemes 123132, missing from layout 0; visual-order mismatches vs L1+L2 reference 64 (64 within 256 B of a cut); brackets 2918, mirroring mismatches 0; char levels (pre-L1) differing from reference 7
   (b) overrides : source graphemes 123132, missing from layout 0; visual-order mismatches vs L1+L2 reference 0 (0 within 256 B of a cut); brackets 2918, mirroring mismatches 0; unicode-bidi line levels (L1) of the encoded pieces differing from the reference line levels 0
   Arabic words straddling a cut: 0
== forced cuts: 50 pieces (avg 4096 B)
   degraded      : source graphemes 123132, missing from layout 0; visual-order mismatches vs L1+L2 reference 26 (26 within 256 B of a cut); brackets 2918, mirroring mismatches 2; char levels (pre-L1) differing from reference 50
   (b) overrides : source graphemes 123132, missing from layout 0; visual-order mismatches vs L1+L2 reference 0; brackets 2918, mirroring mismatches 0; line levels differing 0
   Arabic words straddling a cut: 49; parley glyph ids identical to the unchunked parley layout: degraded 9/49, (b) 9/49; harfrust vs harfrust whole word: no context 9/49, (c) pre/post context 49/49
(with (b) encoding the paragraph levels instead of the L1-adjusted line levels: 34 mismatches, all
 at piece ends — parley does not apply L1 to trailing whitespace inside the inserted overrides)

=== epaint 0.37 check (spike-epaint037-check, Arial)
logical "abc אבג def"      epaint visual (by glyph x) "abc אבג def"         (no reordering: א is leftmost)
logical "Hello مرحبا 123"  epaint visual (by glyph x) "Hello مرحبا 123"     (no reordering)
logical "אב (abc) ג"       epaint visual (by glyph x) "ג )cba( באב (abc) ג"; glyphs 19 = chars 10 (row glyphs inconsistent)
ZWJ family string: 7 chars, 7 row glyphs (cursor stops are char-indexed: CCursor)

=== complex-scripts size (spike-s3-bidi release binary)
with complex-scripts 5,632,736 B; without 2,536,816 B (+3.10 MB dictionary data); epaint-only reference binary 3,114,144 B
```

### Notes per spike (read before quoting)

- **S0**: criterion medians of 30 samples; epaint's glyph atlas is warm and its galley cache is
  missed by a counter, parley's font data is warm. "Theme-face" is the real CSS path (the family
  is an `@font-face` family, so every grapheme goes through cluster matching, with a per-run
  face-set context and a single-character coverage cache); "fast path" is a family parley can
  match directly. The caret figure is caret_rect + hit for every stop of the paragraph.
- **S1**: wall-clock from `main` entry (process start excluded, eframe/GL init included). The
  paragraph uses only theme faces, so it paints before system discovery finishes; discovery
  (fontique collection + the face-name index scan of every font file) ran on a worker in
  153–194 ms. Theme-face loading (3 WOFF2 decodes, 9.2 ms) ran on the UI thread here: move it to
  the worker in T4. RSS: `ps` after 3 s; GPU textures count on Apple unified memory. The atlas
  page is RGBA (egui textures are RGBA): a 1024² page costs 4 MiB staging + 4 MiB texture, which
  alone exceeds the 10 MB budget together with discovery; S4 shows the whole corpus fits in 6 %
  of a 1024² page at 1× (≈ a quarter of a 512² page), so start at 512² (or 256²) and grow.
  Visible differences vs epaint in the PNGs: parley lines are taller (Georgia's metrics line
  height vs egui's), and coverage is used linearly (lighter text) — apply egui's
  `text_alpha_from_coverage` curve in T3 (ADR §3 already plans it).
- **S2**: the injected-event tests run the same state machine through a real `egui::Context`.
  The real-IME part of the gate is **not done**: an agent cannot operate macOS input sources.
  Owner action: `cargo run --release -p spike-s2-ime-log -- window ime.log`, switch to Japanese
  (Kotoeri), type "nihongo" + Space + Return, select a word and convert again, cancel with Esc;
  repeat with Pinyin and Korean; attach `ime.log` (it records every `Ime`, `Key`, `Text`, focus
  event with frame numbers, which answers the open question whether `Commit` arrives in the
  same frame as the clearing `Preedit("")`).
- **S3**: reference = `unicode-bidi` levels + L2 reversal of grapheme clusters; visual slot `k`
  is the pair {right edge of visual cluster k−1, left edge of cluster k} with the affinity that
  attaches the caret to that cluster. One unwrapped line; wrapped bidi lines are covered only by
  the T1 property test with synthetic Hebrew. The first-round "6 → 36" observation is correct:
  Downstream at byte 6 is the right edge of the RTL run.
- **S4**: "frame CPU" = paragraph layout (cached after first use) + atlas lookup/raster + quad
  generation; egui tessellation, texture upload and GPU work are not included. Corpus has Latin,
  CJK and Hangul; every 40th paragraph gets 😀👩‍👩‍👧🎉 (Apple Color Emoji, sbix, via vello_cpu's
  `png` feature). No COLRv1 font is installed on macOS; the synthetic one (PaintGlyph →
  PaintLinearGradient) is drawn by vello_cpu's glifo COLR painter. swash is not used: vello_cpu
  (already in the tree through epaint 0.37) rasterises outlines, bitmaps and COLRv0/v1.
- **S5** (oracle revised after the Codex review): the reference for each piece is the
  whole-line UAX #9 levels *with the per-line L1 reset* (`unicode-bidi` `reordered_levels` for
  the piece as a visual line) and L2 over the **source** piece's visible graphemes (an
  independent inventory: missing or extra clusters are counted, not inferred from the output).
  (b) encodes those *line* levels — RLE/LRE steps + RLO/LRO per run of equal level, base-level
  runs opened by LRM/RLM with no closing mark at the line end, source isolates replaced by
  same-length ZWSP — and parley's own bidi then matches the reference on this generated line:
  0 order mismatches over 123,132 graphemes for both cut sets. Encoding the paragraph levels
  instead left 34 mismatches at piece ends (L1 does not reach whitespace inside the inserted
  controls in parley), so the adapter must apply L1 itself. Invisible formatting characters are
  excluded from the order comparison (their placement is implementation-defined). Glyphs are
  measured separately per option against the unchunked parley layout: (b) keeps 9/49 straddling
  Arabic words (no shaping context across a cut), like degraded mode; only harfrust with
  `set_pre_context`/`set_post_context` (option c) or an upstream parley API (option a) keeps
  them (49/49, measured harfrust vs harfrust on the whole word). One generated input, one font
  (Arial), macOS: evidence of feasibility, not a proof of equivalence. The re-analysis figure is
  `unicode-bidi` 0.3 + `unicode-segmentation` graphemes + ICU4X line segmenter + ICU4X script
  lookup over the whole line.

## Running

```sh
cd spikes/adr-0005/<spike>
cargo run --release            # optional: CARGO_TARGET_DIR=<scratch dir>
```

Platform assumptions: macOS (font paths `/System/Library/Fonts/...`, Georgia used as the stand-in
"theme" font; fallbacks resolved from the system). No fonts, themes or other assets are committed
to this repository or downloaded by the programs. **Dependency-bundled fonts:** the
`wordcraft-fonts` spike embeds that crate's 13 OFL font files (~4.3 MB, `include_bytes!`, not
optional) into its binary — one of the reasons ADR 0005 rejects the crate; `epaint-baseline` is
built without egui's `default_fonts` (OFL/Ubuntu font data awaiting owner approval, M1-5);
`parley-text` bundles no fonts.

Common throughput input: the 83-byte sentence `The quick brown fox jumps over the lazy dog —
office finance, “quotes”, 1/2. ` × 20 (1660 bytes), with an iteration counter appended so no
cache hits, Georgia 16 px, 700 px wide, scale / pixels-per-point 1. One warm-up call, then 10
rounds × 100 iterations; each figure is a **round average** in µs per KiB of input (min / median
/ max over the ten round averages) — not a per-operation latency distribution. The warm-up text
lacks the appended digits, so the first round may include a few cold glyphs/shapes. What each harness times differs and is stated in its output: epaint = shaping +
wrapping + mesh + CPU rasterisation of glyphs missing from its atlas (galley cache misses, glyph
atlas warm); wordcraft = shaping only (it has no line breaking); parley = builder + shaping
+ line breaking + drop.

## Raw results (2026-10-26, Apple M-series laptop, macOS 26, rustc 1.89.0, release profile)

```text
=== epaint-baseline
epaint layout_job (galley-cache miss, warm glyph atlas) µs/KiB, 10 round averages of 100: min 56.4 median 58.3 max 60.0 (16 rows)
=== wordcraft-fonts
FontDb (bundled only) ready in 1.203833ms
added 1 face(s) from /System/Library/Fonts/Supplemental/Georgia.ttf; families: ["Georgia", "Inter", "Inter Medium", "Inter SemiBold", "JetBrains Mono", "Source Sans 3", "Source Sans 3 Semibold", "Source Serif 4", "Source Serif 4 Semibold"]
resolved face: FontFace(Georgia Regular) upem=2048
shape-only µs/KiB over 10 rounds x 100: min 53.5 median 54.2 max 54.5
mixed RTL in Georgia: 19 glyphs, notdef=12 (needs fallback)
fallback for 'م' with system fallback off: None
'office' in Source Serif 4 (bundled): 4 glyphs (ligature if < 6)
loaded font bytes: 379588
RSS before system fallback: 5136 KiB
  fallback 'م' -> Some("Geeza Pro")
  fallback '日' -> Some("Hiragino Sans")
  fallback '한' -> Some("Apple SD Gothic Neo")
  fallback '👩' -> Some("Apple Color Emoji")
  fallback 'क' -> Some("Kohinoor Devanagari")
system fallbacks resolved in 200.963417ms; loaded font bytes now 303 MiB; RSS 321744 KiB
=== parley-text
FontContext::new (system discovery) 87.71325ms
registered 1 family(ies) as ThemeFace
build+shape+break µs/KiB over 10 rounds x 100: min 103.4 median 105.2 max 107.1 (16 lines)
RSS before mixed-script fallback: 17808 KiB
distinct (font blob, face index, rtl, notdef count) tuples over all glyph runs: {"blob0 idx0 rtl=false notdef=0", "blob0 idx0 rtl=true notdef=0", "blob2 idx0 rtl=true notdef=0", "blob3 idx1 rtl=false notdef=0", "blob4 idx0 rtl=false notdef=0", "blob5 idx0 rtl=false notdef=0"}
visual right-arrow byte indices from 6 through RTL: [36, 37, 38, 39, 40, 41, 42, 43]
one arrow over family emoji moves 18 bytes (grapheme = 18)
shape-only RSS after Arabic+CJK+emoji fallback (no rasterisation): 19088 KiB
shape-only RSS after + Devanagari/Korean: 20416 KiB
hit (30,5) -> byte 4
```

Earlier single runs of the same programs: `FontContext::new` 85–154 ms (cold vs warm file cache),
wordcraft fallback resolution 418 ms cold; first cold release build of `wordcraft-fonts` 36 s;
its binary 6.2 MB (4.3 MB of embedded OFL fonts).

## Limitations (read before quoting numbers)

- One machine, one run per row, macOS only; no Windows/Linux, no wasm execution.
- Throughput harnesses time different scopes (see above). There is no isolated glyph-raster
  benchmark, no texture upload, no GPU rendering and no keystroke-to-pixel measurement (epaint's
  figure includes CPU rasterisation of atlas misses, the others none). They do not establish NFR
  compliance.
- Memory: wordcraft "loaded font bytes" are heap-owned copies of whole font files (its crate
  forbids `unsafe`, so no mmap); parley RSS is **shape-only** — fontique maps files and shaping
  touches only cmap/GSUB/GPOS/metrics pages; no glyph outline or emoji bitmap was decoded.
  Units: 17808 → 20416 KiB = 17.39 → 19.94 MiB; wordcraft 5136 → 321744 KiB = 5.02 → 314.2 MiB. A
  matched shape + raster workload is spike S1/S4 work. The wordcraft process also holds a second,
  small bundled-only `FontDb`.
- The parley fallback line prints *distinct tuples*, not a count of runs or fonts; it shows that
  no glyph run of the **first** mixed layout contained `.notdef`, nothing more. The later
  Devanagari/Korean layout is used only for the RSS reading; its glyphs are not inspected.
- The visual-arrow sequence is printed, not checked against UAX #9 (spike S3).
- parley was built without `complex-scripts`, so Thai/Lao/Khmer/Myanmar word segmentation was
  not exercised.
