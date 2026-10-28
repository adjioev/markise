# Can Markise handle a 100 MB document?

Research note, 2026-10-28. Measurements only; no product behaviour changed. The app-level numbers
below come from `main` 23a1750 (which includes the M2d-1 source editor, PR #23); the engine stage
split and the call-stack attribution were taken on eb00aca / ab7ee52 the same day (the code paths
involved did not change in between).

**Short answer:** yes for opening, scrolling and typing in ordinary prose or logs; not yet
comfortably for heavily structured documents. A 100 MiB file parses on the loader thread in
0.4–0.9 s while the UI keeps drawing. All numbers here are **headless CPU work**: layout, paint and
tessellation of the real app UI on the UI thread, without GPU upload, present, compositor or input
latency. By that measure scrolling generally fits the 16.7 ms CPU allowance of a 60 fps frame, except next to
a giant `[TOC]`, where p99 reaches 23–24 ms CPU and exceeds it, and typing in the middle of the file stays within 16 ms. Whether real
keystroke-to-pixel latency meets NFR-4 needs the desktop `MARKISE_TIMING=1` measurement, which was
not taken. Five specific, fixable hazards make some operations slow (0.1–1 s) and memory large
(0.2–3.3 GB). None of them needs rope storage; see §4.

## 1. Setup

- Machine: Apple M1 Max, 10 cores, 64 GB, shared and **loaded**: load average 17–35 during the
  app run (22–64 during the engine runs; printed by the bench at the start and end of each
  document). Every frame series is reported as wall time and as the UI thread's own CPU time. Wall
  p99 is dominated by scheduling on this machine; **thread CPU is the signal**. Single runs; expect
  ±30 % on p99.
- Corpus (`markise-corpus --huge`, seed 42, 100 MiB each):
  - **mixed** — the standard benchmark mix (headings, lists, quotes, fenced code, tables, math,
    footnotes, `[TOC]`): 2.45 M lines, 1.99 M view items;
  - **prose** — mostly paragraphs: 2.0 M lines, 0.55 M items;
  - **log** — many short timestamped lines in long paragraphs: 2.88 M lines, 84 k items;
  - and the 10 MiB mixed corpus (NFR reference size) for comparison.
- Harness: `crates/markise-ui-egui/benches/huge.rs` (opt-in: `MARKISE_BENCH_HUGE=1`) runs the real
  `MarkiseApp::ui` on a headless `egui::Context`, one fresh child process per document for clean
  RSS. Scrolling goes through the viewer's own scrollbar (its rect is taken from the viewer), and
  the bench asserts where each jump lands: the bottom drag reaches 1.0000 of the scroll range, the
  middle click 0.5000, random jumps within 1 %. Engine-level stage split: the existing `editor` /
  `typing` benches with `MARKISE_BENCH_CORPUS=<file>`. Hotspots were attributed with macOS `sample`
  (call-stack sampling) on the bench processes.

## 2. Numbers

Rendered view, UI-thread CPU in ms (p50 / p99; wall p99 in brackets where it differs by more than
2×). NFR budgets are the specification's 10 MB budgets; the CPU numbers are only the CPU share of
them.

| Metric | 10 MiB mixed | 100 MiB mixed | 100 MiB prose | 100 MiB log | Budget (10 MB) |
|---|---|---|---|---|---|
| Open: read / parse / model on the loader (wall) | 8 / 37 / 31 | 69 / 409 / 457 | 66 / 247 / 176 | 74 / 227 / 82 | < 1 s to first screen (NFR-3) |
| Open: process CPU, all threads | 100 | 1,131 | 583 | 375 | |
| Open: install + first frame on the UI thread | 12.5 | 124 | 41 | 15 | |
| UI frames while loading, p99 | 0.7 | 0.5 | 0.3 | 0.5 | |
| RSS after open | 98 MB | 1,086 MB | 461 MB | 218 MB | < 300 MB (NFR-7) |
| Peak RSS over the session (without export) | 354 MB | 3,265 MB | 1,231 MB | 729 MB | |
| Wheel scroll at top / middle / bottom, p99 | 2.4 / 1.8 / 1.7 | 23 / 1.4 / 13 (19) | 10 / 1.2 / 5.7 | 1.4 / 1.3 / 1.3 | 16.7 (NFR-5, whole frame) |
| Scrollbar jump, p50 / p99 | 2.0 / 3.7 | 1.8 / 24 | 1.6 / 6.9 | 1.6 / 2.2 | |
| Typing at start, p50 / p99 | 0.8 / 3.9 | 3.3 / 11 | 3.4 / 7.2 | 3.5 / 7.5 | 16 (NFR-4, whole latency) |
| Typing in the middle, p50 / p99 | 0.6 / 1.9 | 1.9 / 10.5 (16) | 2.0 / 4.6 | 1.9 / 3.1 | 16 |
| Typing at the end, p50 / p99 | 0.4 / 1.0 | **0.4 / 111** | 0.3 / 2.7 | 0.4 / 0.8 | 16 |
| Whole-document copies while typing (450 keys) | 0 | **2** (32–119 in earlier runs) | 0 | 0 | 0 |
| 1,000 edits, p50 / p99 (RSS does not grow) | 0.7 / 1.7 | 2.6 / 11 | 2.4 / 5.1 | 2.4 / 3.6 (21) | |
| Paste 1 MiB / 10 MiB (one frame) | 68 / 135 | **880 / 591** | 455 / 287 | 63 / 117 | < 100 ms bulk (ADR 0006 §7.6) |
| Undo of that paste | 23 / 27 | **492 / 475** | 200 / 165 | 12 / 16 | < 100 ms |
| Save: UI-thread part | 1.0–1.3 | 10–28 | 10–17 | 10–20 | |
| Save: until written (wall) | 52–57 ms | 290–321 ms | 276–290 ms | 374–1,182 ms | < 100 ms (NFR-8) |
| `Arc<str>` snapshot of the text, p50 / p99 | 0.27 / 1.1 | 2.7 / 9.6 | 3.0 / 10.7 | 2.6 / 10.5 | ADR §6.2 guess 1–3 ms at 10 MB |
| Text fingerprint (save) | 0.7 | 7.4 | 7.6 | 7.4 | |
| Full document clone (copy-on-write) | 5.6 | 64 | 31 | 18 | |
| HTML export (whole document) | 3.5 s, 436 MB out | skipped¹ | skipped¹ | 1.3 s, 100 MB out, +275 MB RSS | |

¹ Every `[TOC]` renders the full heading list. The mixed corpus has one `[TOC]` per ~200 blocks, so
its HTML grows with TOCs × headings: the 10 MiB file already exports 436 MB of HTML; the 100 MiB
files were skipped (they would produce tens of GB). Real documents have one TOC; this is a corpus
artefact, but an export-size guard would be cheap.

Open wall time is not in the table. On 23a1750 the bench measured 7–8 s from `open()` to the first
document frame for every file, including 10 MiB, although the loader itself finishes in 0.08–0.9 s
and the UI thread spends only 12–124 ms of CPU. On eb00aca the same bench measured 99 ms (10 MiB)
and 0.34–1.25 s (100 MiB). The time is spent waiting, not computing: the UI thread is idle inside
the install frame, and the loader starts late. This looks like a regression between eb00aca and
23a1750 that is independent of file size; it needs its own investigation.

Memory split (engine only, fresh process, 100 MiB): text 100 MB; parsed document +459 MB (mixed),
+215 MB (prose), +79 MB (log); view model +454 / +114 / +17 MB (≈ 210–240 B per item). At 10 MiB:
+41 MB and +28 MB.

Engine stage split, 100 MiB mixed (wall, load ~33–64):

| Stage | p50 | p99 | Gate |
|---|---|---|---|
| `String` insert+remove memmove, start / middle | 3.4 / 1.6 ms | 33 / 21 ms | G1: ≤ 1.5 ms p99 — **fails** |
| `FormatDoc::apply` keystroke, start / middle / end | 2.8 / 1.3 / 0.002 ms | 27 / 9.7 / 0.002 ms | |
| `DocModel::apply` + `View::apply` keystroke | ≤ 0.07 ms | ≤ 0.5 ms | G1 splice: pass |
| G2 cases (2 MB containers, tables, unclosed `$$`) | ≤ 0.9 ms | ≤ 3.1 ms | ≤ 4 ms: pass |
| `type-10mb` script on the 100 MiB file (`EditorView`, thread CPU) | 1.6 ms | 11.9 ms | G3 ≤ 12 ms (letters 4.6, Enter 26 wall) |

Source mode and Side by Side (the M2d-1 source editor on `main`), UI-thread CPU:

| Metric | 10 MiB mixed | 100 MiB mixed | 100 MiB prose | 100 MiB log |
|---|---|---|---|---|
| Enter Source (switch + first frame) | 13 | 69 | 60 | 60 |
| Source: typing p50 / p99 | 0.6 / 2.7 | 2.1 / 11.9 | 1.9 / 2.9 | 2.0 / 3.4 |
| Source: wheel scroll p99 | 1.5 | 1.5 | 1.7 | 0.6 |
| Enter Side by Side | 22 | 284 | 193 | 73 |
| Side by Side: typing p50 / p99 | 0.9 / 2.7 | 2.0 / **63** | 2.3 / 2.9 | 2.2 / 3.1 |
| Side by Side: wheel scroll p99 | 1.9 | 13 | 2.0 | 1.2 |

The source editor scales by itself. The two slow cases are the shared hazards below: in Side by
Side on mixed content the rendered pane shows code blocks, so the copy-on-write stalls of §3.1 hit
typing there too, and entering Side by Side lays out the rendered pane over a 2 M-item model.

## 3. Bottlenecks (in order of impact)

1. **Copy-on-write cascades from background jobs.** Code-highlight and diagram requests capture the
   shared document `Arc` so the block text can be built on the worker (`CodeQuery::source`). While
   such a request is queued or running, the next keystroke finds the document shared and clones it
   (`EditorSession::doc_mut` → `clone_box`: ~60 % block-list clone, ~40 % text memmove — 63 ms on
   100 MiB mixed, 5 ms on 10 MiB). How often it happens depends on how busy the worker is: 2 to 119
   copies per 450 keystrokes across runs. Each copy is a 70–130 ms frame, so the p99 at the end of
   the mixed file is ~110 ms. Every queued old copy also keeps another ~1 GB alive: peak RSS reached
   9 GB in one run at load average ~60. On 10 MiB the race is rare and cheap, so the existing
   benches never saw it.
2. **O(items) work on structural or bulk edits.** Paste and undo of a paste spend ~0.5 s in
   `View::reset_heights` (a cache key per item, including a reference lookup through the block
   path) and in `DocModel::update_tocs`; every item-count change (Enter, Backspace across a block)
   rebuilds the `Fenwick` height tree from scratch (`Fenwick::splice` → `from_values`, ~10 ms at
   2 M items, the bulk of Enter's 15–26 ms on 100 MiB). Cost scales with items, so log files
   (84 k items) are unaffected and mixed (2 M) is hit hardest.
3. **Memory ≈ 5–11× the file size for structured text.** The parsed document is 4.6× the text for
   mixed content and the view model ~230 B per item; paste/undo, copies and Side by Side push the peak to 3.3 GB.
   Prose (4.6× after open) and logs (2.2×) are moderate. A strict reading of NFR-7 (300 MB) cannot
   hold at 100 MB; a proportional one (3 GB) holds after open, not at peak.
4. **Save does whole-text work twice on the UI thread and once more on the loader.** The UI thread
   copies the text (snapshot, 3–11 ms) and hashes it (fingerprint, 7–8 ms), 10–28 ms in total: one
   dropped frame per save and per autosave. After writing, the file watcher's own-save event triggers a full read + parse +
   model on the loader (0.3–1 s CPU and a second document in memory) only to find the content is
   unchanged.
5. **Per-keystroke memmove of the contiguous text.** 1.6–3.4 ms p50 per edit at 100 MiB (G1 memmove
   gate fails), i.e. the typing floor near the start of the file. It fits the 16 ms frame today, but
   leaves less headroom on slower machines.

Secondary: a `[TOC]` over ~100 k headings is one huge item (scroll/jump frames near it 23–24 ms CPU in
mixed; export size, footnote ¹); the install + first frame of a 2 M-item model takes 124 ms on the
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

With the first four (about one week), the headless CPU cost of every keystroke on a 100 MiB
document, including Enter and edits near code blocks, should stay well under 16 ms; paste/undo
should move into the 100 ms class, and the save hitch should go away. End-to-end latency still has
to be confirmed with `MARKISE_TIMING=1` on the desktop, and memory remains the main limit. A rope
fixes only the memmove floor and the snapshot cost:
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
