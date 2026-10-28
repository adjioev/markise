# Can Markise handle a 100 MB document?

Research note, 2026-10-28. Measurements only; no product behaviour changed. Code measured:
`main` eb00aca (rendered view, M1 read-only source mode) and the M2d-1 source editor branch at
ab7ee52 (PR #23, merged afterwards as 1f76e21 with small review fixes) for Source mode and Side by
Side. The bench lands on top of 1f76e21.

**Short answer:** yes for opening, scrolling and typing in ordinary prose or logs; not yet
comfortably for heavily structured documents. A 100 MiB file opens in 0.3–1.3 s with the UI
responsive throughout, scrolls at 60 fps, and typing stays inside the 16 ms keystroke budget in the
middle of the file. Five specific, fixable hazards make some operations slow (0.1–1 s) and memory
large (0.2–3.4 GB). None of them needs rope storage; see §4.

## 1. Setup

- Machine: Apple M1 Max, 10 cores, 64 GB, shared and **heavily loaded**: load average 22–64 during
  the runs (printed by the bench at start/end of each document). Every frame series is reported as
  wall time and the UI thread's own CPU time; wall p99 is dominated by scheduling noise, **thread
  CPU is the signal**. Single runs; expect ±30 % on p99.
- Corpus (`markise-corpus --huge`, seed 42, 100 MiB each):
  - **mixed** — the standard benchmark mix (headings, lists, quotes, fenced code, tables, math,
    footnotes, `[TOC]`): 2.45 M lines, 1.99 M view items;
  - **prose** — mostly paragraphs: 2.0 M lines, 0.55 M items;
  - **log** — many short timestamped lines in long paragraphs: 2.88 M lines, 84 k items;
  - and the 10 MiB mixed corpus (NFR reference size) for comparison.
- Harness: `crates/markise-ui-egui/benches/huge.rs` (opt-in: `MARKISE_BENCH_HUGE=1`), the real
  `MarkiseApp::ui` on a headless `egui::Context` (layout, paint, tessellation; no GPU upload or
  present), one fresh child process per document for clean RSS. Engine-level stage split: the
  existing `editor` / `typing` benches with `MARKISE_BENCH_CORPUS=<file>`. Hotspots were attributed
  with macOS `sample` (call-stack sampling) on the bench processes.

## 2. Numbers

Rendered view on `main`; thread CPU in ms (p50 / p99) unless marked wall; NFR budgets are the
10 MB budgets from the specification.

| Metric | 10 MiB mixed | 100 MiB mixed | 100 MiB prose | 100 MiB log | Budget (10 MB) |
|---|---|---|---|---|---|
| Open → first painted frame (wall) | 99 ms | 1,246 ms | 1,231 ms | 335 ms | < 1 s (NFR-3) |
| … of which read / parse / model (loader thread) | 9 / 36 / 35 | 81 / 385 / 599 | 80 / 269 / 243 | 65 / 172 / 73 | |
| … install + first frame on the UI thread | 10 | 128 | 36 | 14 | |
| UI frames while loading, p99 | 0.5 | 0.5 | 0.5 | 0.5 | |
| RSS after open | 102 MB | 1,087 MB | 480 MB | 218 MB | < 300 MB (NFR-7) |
| Peak RSS over the whole session | 377 MB | 2,796 MB | 1,236 MB | 886 MB | |
| Wheel scroll at top / middle / bottom, p99 | 2.5 / 2.1 / 0.8 | 24 / 13 / 0.8 | 6.9 / 6.1 / 0.7 | 1.3 / 1.4 / 1.2 | 16.7 (NFR-5) |
| Scrollbar jump, p50 / p99 | 1.8 / 3.3 | 2.0 / 27 | 1.7 / 7.1 | 1.6 / 2.5 | |
| Typing at start, p50 / p99 | 0.7 / 3.6 | 3.4 / 28 | 3.2 / 9.3 | 3.6 / 7.2 | 16 (NFR-4) |
| Typing in the middle, p50 / p99 | 0.5 / 1.6 | 1.9 / 10.5 | 1.9 / 4.4 | 1.9 / 2.7 | 16 |
| Typing at the end, p50 / p99 | 0.4 / 0.9 | **0.4 / 98** | 0.3 / 2.7 | 0.3 / 0.8 | 16 |
| Whole-document copies while typing (450 keys) | 0 | **32–119** | 0 | 0 | 0 |
| 1,000 edits, p50 / p99 (RSS does not grow) | 0.7 / 2.4 | 2.6 / 11 | 2.5 / 5.1 | 2.4 / 3.6 | |
| Paste 1 MiB / 10 MiB (one frame) | 66 / 134 | **947 / 613** | 464 / 291 | 63 / 125 | < 100 ms bulk (ADR 0006 §7.6) |
| Undo of that paste | 21 / 25 | **483 / 476** | 179 / 184 | 12 / 17 | < 100 ms |
| Save: UI-thread part | 1.1 | 11–12 | 11–20 | 11–22 | |
| Save: until written (wall) | 54 ms | 500–715 ms | 361–712 ms | 429–583 ms | < 100 ms (NFR-8) |
| `Arc<str>` snapshot of the text, p50 / p99 | 0.26 / 1.1 | 2.7 / 10 | 2.9 / 9.9 | 2.7 / 9.5 | ADR §6.2 guess 1–3 ms at 10 MB |
| Text fingerprint (save) | 0.7 | 7.3 | 7.4 | 7.4 | |
| Full document clone (copy-on-write) | 4.8 | 63 | 30 | 18 | |
| HTML export (whole document) | 3.4 s, 436 MB out | skipped¹ | skipped¹ | 1.3 s, 100 MB out | |

¹ Every `[TOC]` renders the full heading list. The mixed corpus has one `[TOC]` per ~200 blocks, so
its HTML grows with TOCs × headings: the 10 MiB file already exports 436 MB of HTML; the 100 MiB
files were skipped (they would produce tens of GB). Real documents have one TOC; this is a corpus
artefact, but an export-size guard would be cheap.

Memory split (engine only, fresh process, 100 MiB): text 100 MB; parsed document +459 MB (mixed),
+215 MB (prose), +79 MB (log); view model +453 / +111 / +18 MB (≈ 210–240 B per item). At 10 MiB:
+41 MB and +29 MB.

Engine stage split, 100 MiB mixed (wall, load ~33–64):

| Stage | p50 | p99 | Gate |
|---|---|---|---|
| `String` insert+remove memmove, start / middle | 3.4 / 1.6 ms | 33 / 21 ms | G1: ≤ 1.5 ms p99 — **fails** |
| `FormatDoc::apply` keystroke, start / middle / end | 2.8 / 1.3 / 0.002 ms | 27 / 9.7 / 0.002 ms | |
| `DocModel::apply` + `View::apply` keystroke | ≤ 0.07 ms | ≤ 0.5 ms | G1 splice: pass |
| G2 cases (2 MB containers, tables, unclosed `$$`) | ≤ 0.9 ms | ≤ 3.1 ms | ≤ 4 ms: pass |
| `type-10mb` script on the 100 MiB file (`EditorView`, thread CPU) | 1.6 ms | 11.9 ms | G3 ≤ 12 ms (letters 4.6, Enter 26 wall) |

Source mode and Side by Side:

- `main` (M1 source mode): above 2 MB the source view is read-only by design; entering it costs
  60–77 ms (buffer copy + line index), entering Side by Side 78–742 ms (UI thread); scrolling is
  fine (p99 ≤ 1 ms Source, ≤ 14 ms Side by Side).
- PR #23 source editor (run at load 40–60, so wall times are noise; thread CPU): typing in Source
  p50 2.0–2.2 ms, p99 3.1 (prose) / 3.3 (log) / 11.6 (mixed) ms; Side by Side typing p99 3.4–4.4 ms;
  entering Source 62–72 ms, Side by Side 73 (log) / 192 (prose) / 473 (mixed) ms; scrolling p99
  ≤ 2.6 ms Source, ≤ 15 ms Side by Side. The source editor itself scales; the hazards below
  (copies, memory) affect it equally, because both panes share one session.

## 3. Bottlenecks (in order of impact)

1. **Copy-on-write cascades from background jobs.** Code-highlight and diagram requests capture the
   shared document `Arc` so the block text can be built on the worker (`CodeQuery::source`). While
   such a request is queued or running, the next keystroke finds the document shared and clones it
   (`EditorSession::doc_mut` → `clone_box`: ~60 % block-list clone, ~40 % text memmove — 63 ms on
   100 MiB mixed, 5 ms on 10 MiB). Typing with code blocks on screen therefore costs 70–130 ms per
   key at 100 MiB, and each queued old copy keeps another ~1 GB alive: peak RSS reached 9 GB in
   the loaded PR #23 run. On 10 MiB the race is rare and cheap, so the existing benches never saw it.
2. **O(items) work on structural or bulk edits.** Paste and undo of a paste spend ~0.5 s in
   `View::reset_heights` (a cache key per item, including a reference lookup through the block
   path) and in `DocModel::update_tocs`; every item-count change (Enter, Backspace across a block)
   rebuilds the `Fenwick` height tree from scratch (`Fenwick::splice` → `from_values`, ~10 ms at
   2 M items, the bulk of Enter's 15–26 ms on 100 MiB). Cost scales with items, so log files
   (84 k items) are unaffected and mixed (2 M) is hit hardest.
3. **Memory ≈ 5–11× the file size for structured text.** The parsed document is 4.6× the text for
   mixed content and the view model ~230 B per item; paste/undo and copies push the peak to 2.8 GB.
   Prose (4.8× after open) and logs (2.2×) are moderate. A strict reading of NFR-7 (300 MB) cannot
   hold at 100 MB; a proportional one (3 GB) holds after open, not at peak.
4. **Save does whole-text work twice on the UI thread and once more on the loader.** The UI thread
   copies the text (snapshot, 3–10 ms) and hashes it (fingerprint, 7 ms) — one dropped frame per save
   and per autosave. After writing, the file watcher's own-save event triggers a full read + parse +
   model on the loader (0.3–1 s CPU and a second document in memory) only to find the content is
   unchanged.
5. **Per-keystroke memmove of the contiguous text.** 1.6–3.4 ms p50 per edit at 100 MiB (G1 memmove
   gate fails), i.e. the typing floor near the start of the file. It fits the 16 ms frame today, but
   leaves less headroom on slower machines.

Secondary: a `[TOC]` over ~100 k headings is one huge item (scroll/jump frames near it 24–27 ms in
mixed; export size, footnote ¹); the install + first frame of a 2 M-item model takes 128 ms on the
UI thread (one janky frame after the background open).

## 4. Recommendation: targeted fixes, no rope yet

| Fix | Addresses | Effort |
|---|---|---|
| Background requests must not hold the document: build the block text on the UI thread when small (it is the visible block) or hold only that block's text, so no edit ever finds the document shared | 1 | S (≤ 1 day) |
| `Fenwick` splice without a full rebuild (chunked heights like `RelChunks`, or rebuild only the suffix) | 2 | S (1 day) |
| Incremental heights on paste/undo (re-key only the replaced range and dependants, as `apply_change` does for keystrokes); bounded `update_tocs` | 2 | M (2–3 days) |
| Fingerprint and snapshot off the UI thread (or fingerprint from the save worker); skip the reload parse when the disk version matches our own save | 4 | S (1 day) |
| Compact the parsed document and the item record (fewer heap boxes per block/item, spans relative) | 3 | M–L (1–2 weeks; measure first) |
| HTML export size guard for repeated `[TOC]` | ¹ | XS |
| Rope storage (`crop` behind the `TextSource` seam, parser reading a contiguous window) | 5, part of 1 and 4 | L (2+ weeks) |

With the first four (about one week) a 100 MiB document should keep every keystroke, including
Enter and edits near code blocks, well inside 16 ms, bring paste/undo to the 100 ms class, and remove
the save hitch; memory stays the main limit. A rope fixes only the memmove floor and the snapshot cost:
it would not remove the block-list half of the copy-on-write clones, the O(items) passes or the memory
overhead, which dominate today. Revisit it if memmove p99 on the reference machine (M1, 8 GB) exceeds
~4 ms after the targeted fixes, or if 1 GB-class files become a goal.

## 5. Reproducing

```sh
cargo run --release -p markise-corpus -- /path/to/scratch --huge      # 3 × 100 MiB
cargo run --release -p markise-corpus -- /path/to/scratch/ref && mv /path/to/scratch/ref/10mb.md /path/to/scratch/
MARKISE_BENCH_HUGE=1 MARKISE_HUGE_DIR=/path/to/scratch cargo bench -p markise-ui-egui --bench huge
MARKISE_BENCH_CORPUS=/path/to/scratch/huge-mixed.md cargo bench -p markise-engine --bench editor
MARKISE_BENCH_CORPUS=/path/to/scratch/huge-mixed.md cargo bench -p markise-engine --bench typing
```

`MARKISE_HUGE_PROFILES` selects documents (`10mb,mixed,prose,log`), `MARKISE_HUGE_MIB` the size. The
corpus needs ~320 MB of disk plus a working copy per run; delete it afterwards.
