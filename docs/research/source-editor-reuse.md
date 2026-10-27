# Markdown source editor: what to reuse from the Rust ecosystem

Research note, 2026-10-27. Docs only. Only general Rust-ecosystem projects were read (shallow clones or
crate tarballs); no internals of the reference product. Inputs: ADR 0006, spec 02 §12.1/§12.1.1,
`docs/research/wordcraft-reuse.md`, code at `origin/main` 2f5889d.

Benchmarks: throwaway crate outside the repo; corpus `markise-corpus::generate(10 MiB, seed 42)`
(10,485,800 bytes, 245,391 lines); Apple M1 Max under heavy load (load average ~42), so p99 values are
noise-inflated — decisions rest on order-of-magnitude gaps; treat p50 as the signal.

## 1. Bottom line

1. **No editor widget or core to adopt.** None fits egui + our byte-offset patch/undo/selection engine +
   10 MB files at p99 < 16 ms: TextEdit-based widgets lay out the whole text; cosmic-text and iced bring a
   second font/shaping stack next to parley; floem/Lapce and Helix are tied to their own rope, framework or
   tree-sitter; Zed's editor crates are GPL. Build our own egui widget on the virtualised
   `markise-layout::View` (+ `Fenwick`), whose items are runs of physical source lines (ADR 0006 §5.5).
2. **Keep `String` + `RelChunks`; no rope now.** 10 MB: insert+remove memmove 0.22 ms p50 (middle) /
   0.46 ms (start); `Arc<str>` snapshot 0.27 ms p50 (ADR §6.2 guessed 1–3 ms); naive `Vec<usize>`
   line-index shift 37 µs. A rope is ~1000× faster per edit with O(1) snapshots, but would push
   markise-md, `FormatDoc` slicing, syntect and regex off `&str` to save a fraction of a millisecond.
   **crop** (byte-indexed, fastest) is the documented G1 fallback; ropey second.
3. **No tree-sitter-md or third-party Markdown parser for highlighting.** tree-sitter-md block grammar on
   10 MB: 2.0–3.6 s full parse, 100–160 ms p50 per one-character edit; its README disclaims
   correctness-critical use and it differs from spec 01. markise-md: 36 ms full, 0.25 ms p50 incremental;
   pulldown-cmark 85–100 ms full, no incremental mode. Highlight from our own CST (spec 02 §12.1.1).
4. **Reuse = crates already in `Cargo.lock`, plus designs.** Crates: `regex`, `regex-syntax`,
   `memchr::memmem`, `unicode-width` 0.2, `unicode-segmentation`, `syntect`, `parley`, the egui 0.37 IME API.
   Designs: Helix (auto-pair rules, bounded bracket scan, soft-wrap formatter — MPL-2.0, design only, no
   copying), floem-editor-core (selection/insert semantics on byte offsets), floem `RVLine`, xi-editor's
   rope/wrap notes, Zed's public SumTree write-up, dprint (table output). Later vendoring candidate: Zed
   `sum_tree` (Apache-2.0).

## 2. Measurements (10 MiB)

| Operation | p50 | p99 (noisy) |
|---|---|---|
| `String` insert+remove 1 char at start (2 memmoves) | 461 µs | 2.17 ms |
| `String` insert+remove 1 char in the middle | 216 µs | 1.58 ms |
| `Arc<str>` snapshot copy | 269 µs | 1.33 ms |
| Line-start index build with memchr | 4.3 ms | 9.6 ms |
| `Vec<usize>` line-index shift after a middle edit | 37 µs | 83 µs |
| byte→line binary search | 41 ns | 42 ns |
| ropey 1.6.1 build / insert+remove / byte→line / clone | 3.9 ms / 167 ns / 42 ns / 41 ns | |
| crop 0.4.3 build / insert+delete / line_of_byte / clone | 0.97 ms / 125 ns / 41 ns / 41 ns | |
| jumprope insert+remove (char-indexed, no line metrics) | 84 ns | |
| `memchr::memmem` no-match scan | 332 µs | 339 µs |
| `regex` no-match scan | 333 µs | 851 µs |
| `regex` `(?i)\bword\b`, all matches | 1.21 ms | |
| `regex-cursor` over ropey, no-match scan | 661 µs | |
| **markise-md** full parse / incremental 1-char edit | **35.8 ms / 255 µs** | 44 ms / 1.1 ms |
| pulldown-cmark full parse with offsets | 85–100 ms | |
| **tree-sitter-md** full parse 1 MB / 10 MB | 207 ms / **2.0–3.6 s** | |
| **tree-sitter-md** incremental 1-char edit 1 MB / 10 MB | 8–12 ms / **100–160 ms** | up to 2.5 s |
| unicode-segmentation graphemes / word bounds, 180-byte line | 2.7 / 2.1 µs | |
| unicode-width `width()`, 180-byte line | 0.29 µs | |

`unicode-width` 0.2.2 already returns width 2 for emoji ZWJ sequences, flags, VS16 emoji, CJK and Hangul;
a combining `é` is 1; a tab counts as 1 and needs special handling. No extra display-width crate needed.

## 3. Candidates

Licence allowlist per AGENTS.md. Activity = latest crates.io release / last GitHub push as of 2026-10-27.

| Candidate | Licence | Activity | Verdict | Why |
|---|---|---|---|---|
| Our `String` + `RelChunks` | ours | — | **keep** | microbenchmarks well inside budget (formal G1 p99 verification per ADR 0006 §7.6 still pending — memmove p99 is noise-bound, §13); parser, syntect, regex want `&str` |
| crop 0.4.3 | MIT | 2025-04 / 2026-08 | **G1 fallback #1** | fastest; byte offsets match `DocPos` |
| ropey 1.6.1 / 2.0 beta | MIT | 2023-10, beta 2025-08 | **G1 fallback #2** | widely used; 1.x char-indexed |
| xi-rope / lapce-xi-rope | Apache-2.0 | 2019 / 2025-12 | reject; borrow design | discontinued |
| jumprope 1.1.2 | ISC OR Apache-2.0 | 2023-05 | reject | char indices, no lines |
| helix-core (git) | MPL-2.0 | 2026-09 | **design only** | ropey + char indices, tree-sitter-tied |
| floem-editor-core 0.2.0 | MIT | 2024-11 / 2026-09 | **borrow design** | byte offsets but bound to xi `Rope`/`RopeDelta`; good selection/insert/auto-pair/indent reference |
| floem views/editor `visual_line.rs` | MIT | 2026-09 | borrow the RVLine idea | its visual-row lookup is O(lines) when wrapping |
| Lapce / lapce-core | Apache-2.0 | 2026-10 | reject | floem + tree-sitter coupled |
| xi-editor | Apache-2.0 | discontinued | reject; read design docs | |
| Zed `editor`, `rope`, `text`, `language`, `multi_buffer` | GPL-3.0-or-later | 2026-10 | **reject; do not read** | licence |
| Zed `gpui` | Apache-2.0 | 2025-10 | reject | not egui |
| Zed `sum_tree` | Apache-2.0 | git | later vendoring candidate | fold/wrap/line index if ever needed |
| cosmic-text `Editor` | MIT OR Apache-2.0 | 2026-04 | reject | second text stack |
| egui `TextEdit` | MIT OR Apache-2.0 | 2026-10 | reject as editor; reference its IME code | whole-buffer galley (cause of the 2 MB limit) |
| egui_code_editor 0.4.2 | MIT | 2026-10 | reject | TextEdit wrapper |
| egui_extras syntax_highlighting | MIT OR Apache-2.0 | 2026-10 | reject | whole-string `LayoutJob` |
| iced `text_editor` | MIT | 2025-12 | reject | iced + cosmic-text |
| WordCraft | MIT OR Apache-2.0 | 2026-10 | see `wordcraft-reuse.md` | no code-editor widget |
| kibi / zee | MIT OR Apache-2.0 / Apache-2.0 | | reject | terminal editors |
| ox / amp | GPL-2.0 / GPL-3.0 | | reject (licence) | |
| egui editor crates first published Oct 2026 | MIT/Apache | | reject (too young); watch | |
| tree-sitter + tree-sitter-md | MIT | 2026-10 | reject | 100–160 ms/keystroke at 10 MB; C toolchain; not dialect-exact |
| pulldown-cmark / comrak / markdown-rs | MIT / BSD-2 / MIT | | reject | full parse only |
| syntect 5.3 (fancy-regex) | MIT | 2025-09 | adopted; **add checkpoints** | `ParseState`, `HighlightState`, `ScopeStack` are `Clone + Eq` |
| regex + regex-syntax | MIT OR Apache-2.0 | 2026-07 | **adopt** (in lock) | |
| memchr memmem | Unlicense OR MIT | 2026-07 | **adopt** (in lock) | |
| regex-cursor | MIT OR Apache-2.0 | 2025-02 | only with a rope | |
| unicode-width 0.2 / unicode-segmentation | MIT OR Apache-2.0 | | **adopt** (in lock) | tables, graphemes, words |
| icu_segmenter (via parley) | Unicode-3.0 | 2026-10 | via parley only | already owner-approved (code and data) |
| markdown-table-formatter | MIT | 2023-11 | reject | whole-document rewrite breaks A8 |
| dprint-plugin-markdown | MIT | 2026-10 | borrow design | table/alignment-row output |
| egui 0.37 IME API | MIT OR Apache-2.0 | 2026-10 | **adopt** | see §4.4 |

## 4. Findings

### 4.1 Rope versus our chunks
Rope gains (O(log n) edits ≈150 ns vs ≈0.23 ms, O(1) snapshots, line index) are all far inside budget;
reparse/model costs don't depend on storage; worker snapshots are per debounce window. Line numbers need
no rope: keep per-item line counts in `Fenwick`/`RelChunks`. **Verdict:** keep `String`, route editor
text reads through a narrow `TextSource` API from M2b so crop stays a cheap swap; reassess ropey 2.0
(byte-indexed) when stable.

### 4.2 Editor cores
- TextEdit family: O(document) per change — unusable at 10 MB.
- cosmic-text / iced: a second text stack splitting font handling with the rendered view.
- floem-editor-core: closest conceptually (GUI-free, byte offsets) but APIs are xi rope deltas — use as a
  design and test-case reference. Take floem's VLine/RVLine split; avoid its linear visual-row walk.
- Helix (MPL-2.0, design only): auto-pairs (close an opener only before whitespace/closer/EOF; same-char
  pairs not after alphanumerics; step over the closer; Backspace in an empty pair deletes both — spec
  §12.1.1 is stricter: step over only closers auto-pair inserted, tracked as live marks); bracket matching
  capped at ~10,000 chars; soft wrap at word boundaries with indent-preserving continuation rows formatted
  from an anchor near the viewport; `Range`/`Assoc` ≈ our live marks with affinity.
- Zed: GPL editor — do not read; the public SumTree/display-map write-up is useful architecture.

### 4.3 Parsing and highlighting
Per-line `SourceTokens` view from the markise-md CST mapped to `cm-*` tokens, built from touched top-level
blocks and cached per item hash **plus its external dependencies**: reference-style links/footnotes
(including unresolved lookups — adding `[label]: url` elsewhere must re-token an unchanged `[label]`) are
invalidated by changed labels exactly like the rendered view's dependency-scoped invalidation (ADR 0006
§7.4), not by item hash alone. syntect: per-chunk `ParseState`/`HighlightState` checkpoints in the
existing worker (64 lines / 16 KiB / 8 ms chunks); resume at the edited chunk and stop when the state
converges; cap line length against fancy-regex backtracking.

### 4.4 Algorithms

| Feature | Recommendation | Reuse |
|---|---|---|
| List continuation, C11 renumbering, Tab indent | build as planners (ADR §5.2) | design: Helix/floem multi-line indent |
| Backspace after a bare marker | build from CST marker spans | — |
| Auto-pair, step-over, emphasis wrap | build with tracked closers | design: Helix/floem rules and tests |
| Bracket matching | build: exact CST pairs + bounded plaintext scan (~10k chars) | design: Helix |
| Table realign, Format Table | build on table spans + `unicode-width` (tabs special-cased, CJK = 2), per A8 / ADR §5.3 | design: dprint |
| Folding (P2) | build from the CST; folded items height 0 + placeholder in `View` | reserve: `sum_tree` |
| Soft wrap + line-number gutter at 10 MB | build on `View` + `Fenwick`: monospace estimate for unshaped rows, parley only for visible rows, gutter from per-item line counts | design: floem RVLine, Helix wrap indent, xi incremental wrap |
| Current line, whitespace marks | paint time | — |
| Find | adopt `regex` + `regex-syntax` + `memmem` on worker `Arc<str>` snapshots | WordCraft find design |
| Graphemes/words | `unicode-segmentation`; ICU via parley for dictionary scripts | — |
| IME | egui 0.37 events + ADR 0005 `Composition` | references: egui TextEdit IME, WordCraft `ime_text.rs` |

IME (verified in crate sources): egui 0.37 `ImeEvent::Preedit { text, active_range_chars }` (egui-winit
converts winit's byte range to chars), `ImeEvent::Commit`, `IMEOutput { purpose, rect, cursor_rect,
should_interrupt_composition }`. `ImeEvent::DeleteSurrounding` exists but egui-winit 0.37 on winit 0.30
never emits it — the gap stays as in ADR §6.3.

## 5. Risks
- Rope reversal cost if text access leaks as `String` → `TextSource` from M2b, G1 bench in CI.
- Fence highlight latency / pathological regex lines → checkpoints, line cap, worker-only, one-frame stale style.
- Soft-wrap estimate drift → hidden by anchor scrolling; exact for monospace ASCII.
- Licences: Helix MPL (no copying into our files); Zed editor crates GPL (do not read).
- Young crates → re-check at M3. Benchmark noise → re-measure in the `editor-keystroke` bench (ADR §7.7).

## 6. Plan mapped onto ADR 0006 milestones

| Milestone | Build | Reuse |
|---|---|---|
| M2b caret/selection/typing | caret stepping, goal-x, mouse/word/line selection, `TextSource` API, per-item line counts in `View` | `unicode-segmentation`; design: floem `Selection`, Helix `Range`/`Assoc` tests |
| M2c input rules | list/quote continuation, C11 renumbering, multi-line Tab/Shift+Tab incl. quote levels, marker Backspace, auto-pair with tracked closers, emphasis wrap, URL paste (C50) | design: Helix `auto_pairs`, floem `Editor::insert` cases |
| M2d source editor | egui widget on `View`: gutter, soft wrap, current line, whitespace marks, CST → `cm-*` highlighting, bracket match, table navigation/realign/Format Table, viewport-centre mapping, no 2 MB limit; folding design | `unicode-width`, `syntect` + checkpoints, parley; design: floem RVLine, Helix formatter, dprint; reserve `sum_tree` |
| M2e find, IME | find/replace with query classes and stamps, worker search on `Arc<str>`, Replace All as one `PatchSet`; ADR 0005 `Composition` | `regex`, `regex-syntax`, `memmem`, egui 0.37 IME |

Benchmarks to add: source-pane `type-10mb` with wrap and gutter; scroll-to-end with wrap (no O(lines)
walks); fence re-highlight in a 10k-line block; realign of a 5k-row aligned table; find-active typing with
a literal query and with `(?i)\bword\b`.

## 7. Sources
Code read (shallow clones, 2026-10-27): helix-core; floem editor-core and views/editor; lapce;
egui_code_editor; cosmic-text; iced; crop; ropey. Crate sources: markdown-table-formatter 0.3.0, egui and
egui-winit 0.37.0, syntect 5.3.0. Licences and activity: crates.io and GitHub APIs (for Zed, only each
crate's `Cargo.toml` licence field). Web: tree-sitter-markdown README
(https://github.com/tree-sitter-grammars/tree-sitter-markdown), Zed SumTree write-up
(https://zed.dev/blog/zed-decoded-rope-sumtree), xi-editor docs (https://xi-editor.io/docs.html).
