# Folders and vaults

Markise works on ordinary folders of Markdown files. An Obsidian vault is such a folder, so you
open it the same way: nothing is imported, converted or copied.

## Open a folder

**File → Open Folder…** (<kbd>Shift+Cmd+O</kbd>, <kbd>Ctrl+Shift+O</kbd>) shows the folder in the
sidebar. When you open a single file, its folder is used. **File → Open Recent** remembers files
and folders (unless you turn that off in **Preferences → Files & Images**).

**View → Toggle Sidebar** (<kbd>Shift+Cmd+L</kbd>, <kbd>Ctrl+Shift+L</kbd>) shows or hides the
sidebar. Its activity bar switches between **Files**, the **Outline** of the current document
(click a heading to jump to it) and **Comments**. The Search icon is a placeholder: search
across a folder is not available in the beta.

## The file tree

The tree lists Markdown and plain-text files (`.md`, `.markdown`, `.txt` and similar
extensions). Hidden files and folders — names starting with a dot, such as `.obsidian` — are not
shown.

Right-click an entry for **Open**, **New File**, **New Folder**, **Rename**, **Move to Trash**,
**Reveal in Finder** (Show in Explorer / Show in File Manager on Windows and Linux) and **Copy
Path**. The buttons in the tree's header collapse all folders and create a new folder or file.

- **New File** creates `Untitled.md` (then `Untitled 2.md`, …) and lets you name it right away.
- **Rename** never replaces an existing file. A document that is open follows its file.
  Links in other notes are **not** updated when you rename a file.
- **Move to Trash** asks first and moves the file to the Trash or Recycle Bin, where you can
  restore it. On macOS, Finder's **Put Back** is not available for files Markise moved there.

## Quick Open

**File → Quick Open…** (<kbd>Cmd+P</kbd>, <kbd>Ctrl+P</kbd>) finds a file by name in the open
folder. Type a few letters of the name — they need not be next to each other — and press
<kbd>Enter</kbd>. Recent files are listed first. Very large folders are searched up to their
first 100,000 entries.

## Links between notes

Markise understands wikilinks (**Preferences → Markdown → Wikilinks**, on by default):

| You write | It links to |
|---|---|
| `[[Tides]]` | the note `Tides.md`, anywhere in the folder |
| `[[Lighthouse#The lamp]]` | the heading "The lamp" in `Lighthouse.md` |
| `[[#Checklist]]` | a heading in the current note |
| `[[Harbour log\|the log]]` | `Harbour log.md`, shown as "the log" |
| `[[crew/Ann]]`, `[[./Ann]]`, `[[../Ann]]` | a path from the folder root, or relative to the current note |

Names are compared without regard to case. When several notes share a name, the one in the same
folder as the current note wins, then the one with the shortest path. Ordinary Markdown links to
local files, such as `[Tides](Tides.md)`, work too. A link that does not resolve is shown in a
lighter colour.

### Follow a link

<kbd>Cmd</kbd>-click (<kbd>Ctrl</kbd>-click) a link to open the note; a heading link scrolls to
the heading. A link to another kind of file (an image, a PDF) opens it in its default
application after asking. Links work in an opened folder; while the folder is still being
indexed, Markise asks you to try again a moment later.

**Go → Back** and **Go → Forward** (<kbd>Alt+Cmd+←</kbd> / <kbd>Alt+Cmd+→</kbd>, on Windows and
Linux <kbd>Alt+←</kbd> / <kbd>Alt+→</kbd>) return to where you were, scroll position included.

### Create a missing note

<kbd>Cmd</kbd>-click a link to a note that does not exist, and Markise offers to create it
("Create … in …?"). The new note is empty and opens right away. It is created next to the note
with the link, unless the link names a folder or the vault's settings say otherwise. Only notes
can be created this way, and never outside the opened folder.

### Autocomplete

In the rendered view, type `[[` to get a list of notes. Keep typing to narrow it, use the arrow
keys, and press <kbd>Enter</kbd> or <kbd>Tab</kbd> to insert the link; <kbd>Esc</kbd> closes the
list. In an Obsidian vault Markise inserts a wikilink (unless the vault is set to Markdown
links); in a plain folder it inserts a Markdown link such as `[Tides](Tides.md)`.

Not supported yet: embeds (`![[note]]` is shown as text) and block references (`[[note#^id]]`
opens the note without scrolling).

## Obsidian vaults

A folder is treated as an Obsidian vault when it contains a `.obsidian` folder. Markise reads
only `.obsidian/app.json`, and only these settings from it: the link format (wikilinks or
Markdown links, and shortest, relative or absolute paths), the attachment folder, and where new
notes go. It never writes anything inside `.obsidian/` — no settings, notes or attachments.
Changes to `app.json` are picked up while Markise runs.

Plugins, canvases, community themes, graph view and other Obsidian features are not used.
Markise is an independent project; see the trademark notice at the bottom of this page.

## Background indexing

When a folder opens, Markise lists its files on a background thread, so the window stays
responsive, and then follows changes made by other programs. The index lives in memory only;
nothing is written to the folder.
