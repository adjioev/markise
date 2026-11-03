# Large files

Markise is built to open and edit large Markdown files — logs, exports, generated
documentation — without freezing. Only the part of a document that is on screen is laid out and
drawn.

## Opening in two steps

When you open a file with **4 MiB or more** of text, Markise shows the first part (about
200 KiB) right away and loads the rest in the background. You can read, scroll and type in the
first part meanwhile; the status bar says **"Loading the rest of the file…"** until the whole
file is in. Reloading a file that is already open does not use the two steps.

### Measurements

Recorded on 3 November 2026 on an Apple M-series Mac, release build, headless through the real
app, with 100 MiB test documents:

| | mixed | prose | log |
|---|---|---|---|
| Open → first painted frame | 52 ms | 51 ms | 41 ms |
| Open → whole file loaded | 813 ms | 572 ms | 311 ms |
| The same file opened in one step, first frame | 785 ms | 525 ms | 291 ms |
| Slowest frame while loading, scrolling included | 1.9 ms | 2.4 ms | 1.2 ms |

"Headless" means the app ran without a window: the times do not include the operating system's
window, GPU and compositor work, so on screen they are longer. Your machine and your files will
give different numbers. The full method and table are in
[ADR 0017](https://github.com/adjioev/markise/blob/main/docs/adr/0017-progressive-open.md).

## While the rest is loading

| Feature | Behaviour until the whole file is loaded |
|---|---|
| Find | searches the loaded part only (the find bar says "Loaded part") |
| Outline | lists the headings found so far ("More headings when the file has loaded…") |
| Save, Save As, save on close | wait for the rest, then save ("saving when done" in the status bar) |
| Autosave and draft backups | wait |
| Export, Save Copy Without Comments, Replace All, Accept All / Reject All | unavailable |
| Word count | hidden |

If the rest of the file cannot be loaded, Markise keeps the first part on screen, says so in a
banner, and disables saving so the file on disk is never replaced by a partial copy.

## Tips

- Memory use grows with the file: in the test above, the heap peaked at about 270–480 MB
  for the 100 MiB documents.
- Files that are not valid UTF-8 open with replacement characters and cannot be saved, so they
  are never damaged; draft backups are not made for them either.
