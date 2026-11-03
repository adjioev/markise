# ADR 0017: progressive open

Status: accepted for issue #126 (owner decision 2026-11-02, after the prototype on branch
`proto/lazy-view`, `docs/research/lazy-view-prototype.md` there). Builds on the paged view
model (ADR 0016).

## Context

Opening a 100 MiB file took 0.3–1.4 s before the first frame (read ~40 ms, the rest parse and
model build), all of it before anything was shown. The prototype showed the first screen
after 35–50 ms by parsing a small first part first.

## Decision

**Two steps for large files.** An open (not a reload) of a file with at least 4 MiB of decoded
text (`ProgressiveOpts`, `crates/markise-engine/src/load.rs`; `MARKISE_PROGRESSIVE_OPEN=0`
switches it off) reads and decodes the whole file once, parses and models only a first part of
about 200 KiB and returns it at once (`Outcome::Partial`). The whole text is parsed and
modelled on a thread of its own (`load::Rest`); the loader worker is free for the next request
meanwhile, so opening another file does not wait for it. Dropping the `Rest` cancels it at its
next stage.

**Where the first part ends** (`markise_md::progressive_prefix`). Markdown block structure is
decided line by line from the top, so every top-level block of a prefix that is followed by
another block in that prefix parses the same in the whole text; only the last one can still
grow (an open fence, a list, a paragraph a later `===` turns into a heading, a table). The
part is therefore cut at the start of the last top-level block of a parse of about 200 KiB
(at a line end inside one very large block, whose first part is then shown as such). Front
matter is the exception (its opening line counts only if a closing line follows anywhere):
a file whose front matter does not close within the part opens in one step. A differential
test checks on generated corpora (all profiles, LF and CRLF) that every block of the part
equals the whole document's.

**What the first part shows provisionally.** Inline facts that depend on later text:
reference definitions and footnotes defined after the cut, the outline and `[toc]`, and a
`<details>` whose `</details>` is after the cut (unpaired, so its content is shown). The whole
document replaces all of them.

**Swap without moving the view** (`EditorView::continue_with`). The edits made in the part are
carried over as one verbatim patch set — the diff of the session's text against the original
part, with the session's literal hints — applied to the whole document, the model following
incrementally (`DocModel::apply`; a patch reaching placeholders re-pages its region). The
session adopts the result without an undo step (`EditorSession::adopt_continuation`): the
history, the selection, live marks and the saved state all lie in the shown text, which the
whole text starts with, so they stay valid; the revision counter moves on (find, the comments
list and other results computed for the part are refreshed), the word count and the comment
scan start over on the whole document. The view (`View::set_model_keep`) keeps the scroll
anchor's item (by source position) and the offset into it, the reader's open/closed
disclosure choices and expanded blocks; a disclosure crossing the cut is closed as written
unless the scroll anchor or the caret is inside it (then it stays open, so what the reader
looks at stays). The source pane, if open, is rebuilt; positions kept from the last frame
remain valid (nothing before the cut moved).

**While the document is partial** (`app/progressive.rs`):

- Reading, scrolling and editing work on the part. Outline, find, Go to End and `[toc]`
  cover the loaded part: the find bar says "Loaded part" (results are refreshed when the
  whole document is in), the outline ends with "More headings when the file has loaded…".
  This was preferred to waiting: the part is useful at once and every result is corrected
  automatically.
- Save, Save As and Save on Close wait and run once the whole document is in; the status bar
  says "Loading the rest of the file… (saving when done)". Autosave and draft backups wait
  (their timers run on). Nothing ever writes the part alone.
- Export, Save Copy Without Comments, Replace All and Accept/Reject All are unavailable: they
  would cover only the part.
- The word count is not shown; it starts on the whole document.
- The loading state is shown only in the status bar, never above the document (owner
  decision: a banner moved the text down and back).
- An external change is checked when the document is complete (the watcher's event is
  queued as during any load): a clean document reloads, an edited one gets the conflict
  banner.
- Opening another file replaces the partial document when it loads (the rest is cancelled);
  if that open fails, the partial document stays and completes. Close and Quit of a clean
  document close at once; with unsaved edits they ask as usual (Save waits for the rest).
- A recovered draft opens progressively as an unsaved copy. A lossily decoded file stays
  unsaveable. A re-parse with new options (a dialect switch) waits for the whole document.
- If the rest cannot be loaded (the thread stopped), the part stays with an error banner and
  saving disabled.

## Measurements

100 MiB corpora (`markise-corpus --huge`), paged models, headless through the real app
(`MARKISE_BENCH_LAZY=1 MARKISE_LAZY_OPEN=1 MARKISE_LAZY_FILES=… cargo bench -p
markise-ui-egui --bench lazy`), Apple M-series, release, 2026-11-03. Frame times are UI-thread
CPU; "edit" types three characters into the part while the rest loads.

| | mixed | prose | log |
|---|---|---|---|
| Open → first painted frame, normal | 785 ms | 525 ms | 291 ms |
| Open → first painted frame, progressive | **52 ms** | **51 ms** | **41 ms** |
| Open → fully loaded, progressive (normal = first frame above) | 813 ms | 572 ms | 311 ms |
| Slowest frame while loading (incl. scrolling) | 1.9 ms | 2.4 ms | 1.2 ms |
| Install frame without / with an edit | 1.4 / 4.6 ms | 1.7 / 4.8 ms | 1.6 / 4.9 ms |
| Content on screen unchanged at install; edit kept, undo restores the file; model equals a full build | yes | yes | yes |
| Peak live heap, normal / progressive (MB, 100 MB of it before open) | 474 / 476 | 386 / 388 | 267 / 269 |

Reading gets 1 MiB of spare capacity (`load.rs`, `READ_HEADROOM`): without it the first
edit of a 100 MiB text reallocates and copies it (~30 ms and a second copy on the UI thread);
the install with an edit took 30–35 ms and +100 MB before.

## Known limits

- Memory during the load is that of a normal open plus the part (the text is read once and
  moved to the rest thread).
- The rest is parsed in one piece; cancelling it takes effect between parse and model.
- Typing at the very end of the part inserts before the rest of the file (where the text
  continues), as in the file.
