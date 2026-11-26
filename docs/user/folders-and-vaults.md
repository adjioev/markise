# Folders and vaults

Markise works on ordinary folders of Markdown files. An Obsidian vault is such a folder, so you
open it the same way: nothing is imported, converted or copied.

## Open a folder

**File → Open Folder…** (<kbd>Shift+Cmd+O</kbd>, <kbd>Ctrl+Shift+O</kbd>) shows the folder in the
sidebar. When you open a single file, its folder is used. **File → Open Recent** remembers files
and folders (unless you turn that off in **Preferences → Files & Images**).

**View → Toggle Sidebar** (<kbd>Shift+Cmd+L</kbd>, <kbd>Ctrl+Shift+L</kbd>) shows or hides the
sidebar. Its activity bar switches between **Files**, the **Outline** of the current document
(click a heading to jump to it), **Comments** and the folder's **Tags** (see [Tags](#tags)).
The Search icon is a placeholder: full-text search across a folder is not available in the
beta; search by tag is.

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

Embeds of images (`![[photo.png]]`) show the image; see [Images](editing.md#images). Not
supported yet: embeds of notes (`![[note]]` is shown as a link to the note, not its text) and
block references (`[[note#^id]]` opens the note without scrolling).

## Tags

Label notes with tags, in the text or in the front matter:

```markdown
---
tags: [project/alpha, idea]
---
Notes for today. #idea #travel/2026
```

- **In the text**, a tag is `#` followed by letters, digits, `_`, `-` or `/` (any script, emoji
  too), at the start of a line or after a space or `(`. It ends at a space or punctuation, so
  `#idea.` is the tag `idea`. These are not tags: `#1984` (digits only), `a#b`, the `#` of a
  link such as `https://example.com/#part`, `# Heading`, and anything in code, math or HTML.
- **In the front matter**, `tags:` (or `tag:`) takes a list `[a, b]`, one item per line
  (`- a`), or names separated by commas or spaces (`tags: a, b`). A leading `#` is allowed; an
  item with a space in it is ignored.
- **Nested tags** use `/`: a note tagged `#area/health` also counts for `area`.
- Tags ignore letter case: `#Idea` and `#idea` are the same tag.

In the rendered view a tag is shown as a pill. The file keeps exactly what you typed. Click a tag
to put the cursor in it; <kbd>Cmd</kbd>-click (<kbd>Ctrl</kbd>-click) it to list the notes with
that tag.

**The Tags view** (the `#` icon in the activity bar) lists every tag of the opened folder with its
number of notes; nested tags sit under their parent. Click a tag to see its notes, and a note to
open it. Type in **Find** to narrow the list by name; `tag:name` or `#name` followed by
<kbd>Enter</kbd> lists the notes with that tag (and with tags nested below it).

The tag list follows the files on disk: it updates when a note is saved or changed by another
program, not while you type. Notes larger than 4 MB are not read for tags.

A line that starts with `#` directly followed by a word, such as `#idea`, is a tag, not a
heading; write `# Heading` (with a space) for a heading. If your notes use headings without the
space, turn off **Preferences → Markdown → Tags #tag**: `#` in the text is then plain text again
(tags in front matter are still listed).

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
responsive, then reads the notes for their tags, and then follows changes made by other
programs. Links work as soon as the files are listed; while notes are still being read, the Tags
view says so. The index lives in memory only; nothing is written to the folder.
