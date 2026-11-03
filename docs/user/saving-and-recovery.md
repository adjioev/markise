# Saving and recovery

Markise protects unsaved work in two separate ways: **draft backups** (on by default) and
**autosave** (off by default).

| | Draft backups | Autosave |
|---|---|---|
| What it does | Copies unsaved changes to a private recovery folder | Saves the document to its own file |
| Default | On | Off |
| How often | At most every 30 seconds while there are unsaved changes | Every 5 minutes (1 to 1440, adjustable) |
| Touches your file | No | Yes |
| Setting | **Preferences → Files & Images → Back up unsaved drafts** | **Automatically save files**, **Autosave interval (minutes)** |

## Draft backups and recovery

While a document has unsaved changes, Markise writes a draft copy to its recovery folder. The
draft is deleted as soon as you save, discard the changes or close the document.

If Markise or your computer stops before you saved, the **Recover unsaved drafts** window opens
at the next launch. Each draft opens as an unsaved copy; save it where you like. The window can
be opened again from **File → Recover Unsaved Drafts…**. Several running Markise windows never
offer each other's drafts.

Drafts are stored in the `recovery` folder next to the settings file:

| System | Recovery folder |
|---|---|
| macOS | `~/Library/Application Support/markise/recovery/` |
| Windows | `%APPDATA%\markise\recovery\` |
| Linux | `~/.config/markise/recovery/` (or `$XDG_CONFIG_HOME/markise/recovery/`) |

Limits of draft backups:

- Changes made in the last 30 seconds before a crash may be missing from the draft.
- No drafts are made for files that are not valid UTF-8, or for a large file that is still
  loading.
- Draft backups are not a backup of your files: keep your own backups or use version control.

## Autosave

With **Automatically save files** on, Markise saves documents that already have a file at the
chosen interval. It does not autosave untitled documents, a file that was deleted on disk, or
while a question or a conflict is waiting for your answer.

**Save when closing** (off by default) saves changes when you close a document instead of
asking Save / Discard / Cancel.

## How saving works

Markise writes the new content to a temporary file and then replaces the original, keeping a
backup of the original until the replacement has succeeded. If saving fails, your text stays in
the editor and a banner shows the error.

## When another program changes the file

Markise watches the open file:

- **No unsaved changes in Markise:** the document reloads by itself. The reload is one undo
  step, so <kbd>Cmd+Z</kbd> brings back what you had.
- **Unsaved changes in Markise:** a banner says "The file changed on disk and you have unsaved
  edits." Choose **Reload from disk** or **Keep my edits**.
- **Saving over a changed file:** Markise refuses at first and offers **Overwrite**, **Reload
  from disk (discard my edits)** or **Cancel**. It never silently overwrites a file that
  another program changed.
- **File deleted or moved:** Markise keeps the document open and says so; save to write it
  again.
