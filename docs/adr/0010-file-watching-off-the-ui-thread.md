# ADR 0010 — File watching off the UI thread

Status: accepted for the `perf-open-delay` branch (2026-10-28, issue #24). Scope:
`crates/markise-engine` (`watch`), `crates/markise-ui-egui` (`app`).

## Context

Choosing a small file in the file tree sometimes took close to a second before it was shown
(issue #24). Measured in the release app on macOS with `MARKISE_TIMING=1` and a temporary
per-frame trace, mounting a ~200-file notes folder and clicking 0.2–12 KB files:

| stage | time |
|---|---|
| click → load request (UI thread) | < 1 ms |
| read + parse + view model (loader worker) | 0.5–2 ms |
| worker → UI hand-off (repaint wake-up) | 1–13 ms |
| `FileWatcher::watch_file` inside `install` (UI thread) | **1 ms … 1.5 s**, typically 1–15 ms, spikes of 170–1500 ms |
| view build + first layout and paint | 3–5 ms |

Everything except the watcher was a few milliseconds. `watch_file` unwatches the previous
document's directory and watches the new one; with `notify`'s FSEvents backend each of these
calls stops the event stream (waiting for its run loop thread to quiesce and exit, then
purging pending events) and starts a new one covering all watched paths, including the
recursive watch of the mounted folder. How long that takes depends on system-wide FSEvents
activity, hence the unpredictable stalls. The UI thread was blocked for the whole time, so
the frame that showed the document came late. Opening a file outside a mounted folder and
mounting a folder had the same synchronous calls.

## Decision

1. `FileWatcher` owns its `notify` watcher on a dedicated worker thread (`markise-watch`).
   `watch_file`, `watch_folder` and `watch_themes` only post the wanted target and return;
   no file-system call (canonicalisation, watch registration) runs on the caller's thread.
2. The worker keeps a wanted state (open file, mounted folder, theme folder), drains bursts
   of requests (fast clicking through the tree) into one update, and applies the difference
   with `notify`'s batched `paths_mut` API, so the backend restarts at most once per update.
3. The open file's directory gets no watch of its own when a recursive watch (mounted folder,
   theme folder) already covers it. Selecting files inside the mounted folder therefore does
   not touch the backend at all.
4. Registration is asynchronous, and on macOS every registration change stops the stream and
   restarts it "since now", so changes in that window have no events. The worker therefore
   catches up after every registration change and whenever a new file is opened:
   - it reports `FolderWatched`, and the UI lists every directory of the tree again (the tree
     is listed at once, while the folder watch may still be pending);
   - it compares the open file with the version the caller read, by content identity (length
     and hash of the bytes, `DiskVersion`), never by modification time alone: the loader reads
     and then stats, so a replace in between pairs old bytes with the new time, and some
     tools preserve the time. A difference is reported as `FileChanged` / `FileRemoved`; the
     UI re-reads and decides as for any external change. The worker reads the file for this
     (off the UI thread). This also closes the older gap between the loader's read and the
     watch registration.
   - the baseline it compares with is the version the UI last acknowledged: the loaded one,
     then each successful save and each accepted reload (`FileWatcher::acknowledge`), and
     after a detected deletion "known missing", so a later recreation is still reported.
5. Failures are not hidden: a path that cannot be added, or a stream that cannot be started,
   is left out of the active set (so the next request registers it again), reported as
   `WatchEvent::Error` (the UI shows a notice and logs it with `MARKISE_TIMING`), and
   `FileWatcher::sync` returns it.
6. `FileWatcher::sync` waits until earlier requests are applied (tests and tools only). The
   theme folder gets the same catch-up instead: once its watch is in effect the folder itself
   is reported as changed, so user themes are re-imported and the catalog re-read (the theme
   worker never waits for a registration; it also saves the settings).
7. The timing log reports `first-document-frame` for the frame that shows the opened
   document (it used to fire on the click frame, still showing the previous document), and
   the file size for that log line is read only when timing is enabled.

## Consequences

- Click → first frame of a small file: 2–10 ms in the release app (was up to ~1.5 s);
  covered by `crates/markise-ui-egui/tests/open_latency.rs`, which stalls the watcher worker
  for 800 ms through `JobHooks::before_watch` and asserts the open stays fast.
- Dropping a `FileWatcher` no longer blocks: the worker drops the `notify` watcher.
- Watch registration is asynchronous; code that needs it in effect (tests) calls `sync`.
  An external change made right after an open is reported once the registration is in
  effect; under heavy FSEvents load (the UI test suite starting many streams in parallel)
  that took 7–10 s in measurements. Before, the same wait froze the UI thread instead.
  `tests/watch_gaps.rs` and the `watch` unit tests hold the worker inside a registration
  change and change files meanwhile (tree refresh, folder switch with an open document,
  content-identity comparison, failed registration and retry).
- After a registration change the worker reads the open file once to compare it (a 10 MB
  file costs it some milliseconds; the UI thread is not involved).
- One more long-lived thread per watcher (two in the app: documents and themes).
