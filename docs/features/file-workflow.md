# Saving, file navigation and table operations

Issues #49, #52 and #54; behavior from spec 02 §6, spec 04 §5/§10 and spec 05.

## Saving and recovery

Preferences → Files & Images → Saving controls autosave-to-file (off by default), its
interval (5 minutes, range 1–1440), draft backups (on), save-on-close (off), and recording
recent files/folders (on). These values are persisted in the `files` settings object using
`enableAutoSave`, `autoSaveTimer`, `noUnsavedDraftsBackup`, `saveOnClose`, `noRecentFiles`.

Dirty documents, including untitled ones, are backed up every 30 seconds when their revision
changed. Backups are Markdown snapshots under `<config>/markise/recovery/session-*/`, named
with a timestamp and document name (untitled drafts use the first nonempty line). Literal
editing hints are escaped in the worker's copy without changing the live document or undo
history. Writes use an atomic replacement and fsync. Session locks exclude live instances.

On next launch, abandoned drafts are offered for recovery. File → Recover Unsaved Drafts
reopens that offer. Recover opens an **unsaved copy**, requiring Save As; it never replaces
the original file. Closing the recovery window leaves those drafts available. Backups are
removed after a covering save or an accepted discard that actually replaces/closes the
document. A failed open retains the original draft. A crash before the first 30-second
backup can still lose those initial edits. This is application recovery on all platforms;
OS version-history integration is not included.

Autosave uses the normal checked save worker and disk-version expectation. Untitled,
lossily decoded and deleted files are not automatically written. An external-change
conflict pauses automatic attempts until the user resolves it. Save-on-close uses the
normal save path, including Save As for untitled documents.

## Opening large files

A file of 4 MiB or more opens progressively (ADR 0017): its first ~200 KB is shown at once
and can be read, scrolled and edited while the rest is parsed in the background; then the
whole document takes its place without moving the text on screen, keeping the edits, undo
history and selection. Meanwhile the status bar says "Loading the rest of the file…"; find
and the outline cover the loaded part (and say so); the word count waits; Save waits and
runs when the file is complete ("saving when done"); autosave and draft backups pause;
export, Save Copy Without Comments, Replace All and Accept/Reject All are unavailable.
`MARKISE_PROGRESSIVE_OPEN=0` switches it off.

## Open Recent and Quick Open

File → Open Recent remembers the latest 30 opened files/folders across launches; Clear
Items clears the persisted list and the sidebar. Recording can be disabled independently
of displaying the sidebar's Recent section.

⌘P (Ctrl+P elsewhere) opens Quick Open. It searches **filenames**, not document contents,
in the mounted folder and, when outside it, the current file's folder. Recent Files and
File Results are separate sections. Type a subsequence, use ↑/↓ to select, Enter to open,
and Escape to return focus. Opening a result uses the normal unsaved-change guard.

Indexing and matching run on a cancellable worker. Hidden entries and symlinks are skipped;
Markdown/plain-text extensions match the sidebar. A scan stops at 100,000 visited entries
and reports that limit. The index refreshes each time the palette opens.

## Tables

Paragraph → Table and the document context menu expose row/column insertion, deletion,
movement, and column alignment. Each operation is one undo step and retains escaped cell
content, line endings and quote/list containers. Column movement carries its alignment.
The header is fixed; deleting/moving it and deleting the last body row/column are disabled.
Commands operate on the caret's table in rendered mode.

macOS shortcuts: add row below ⌘Return; delete row ⇧⌘Backspace; move body row ⌃⌘↑/↓;
move column ⌃⌘←/→. Other platforms use Ctrl+Enter, Ctrl+Shift+Backspace, Ctrl+Alt+arrows.

## Manual verification

- Edit a named document and an untitled draft. Wait 30 seconds, terminate the process,
  relaunch, and recover the contents. Save the copy; confirm it is not offered again.
- Enable autosave with a 1-minute interval. Check a normal save, then modify the file from
  another editor and verify the conflict preserves both versions.
- Enable save-on-close, including an untitled document and a cancelled Save As dialog.
- Reopen the app: Open Recent retains files/folders. Clear Items removes the list.
- Use ⌘P with duplicate filenames in nested folders; test typing, arrows, Enter, Escape,
  and opening while the current document has unsaved edits.
- Open a 100 MB Markdown file: the first screen appears at once, the status bar shows the
  loading note, nothing above the document moves. Scroll and type before it completes; press
  ⌘S while loading ("saving when done") and confirm the saved file is complete. Repeat with
  a change made by another editor while loading, and with Quit while loading.
- Exercise table operations through menus and shortcuts, then undo/redo. Include escaped
  pipes, a quoted table, and CRLF source.
