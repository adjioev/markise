# Editing

## Three views

| View | How | What you see |
|---|---|---|
| Rendered | **View → Rendered** | The formatted page. Click anywhere and type; the Markdown markers around the caret are revealed while you edit. |
| Source Code Mode | **View → Source Code Mode**, <kbd>Cmd+/</kbd> (<kbd>Ctrl+/</kbd>) | The plain Markdown text with syntax colouring. |
| Side by Side | **View → Side by Side**, <kbd>Ctrl+Cmd+/</kbd> (<kbd>Ctrl+Alt+/</kbd>) | The source on the left, the rendered page on the right; both scroll together. You type in the source; the rendered side is read-only. |

The shortcut toggles a view on and off; choosing it from the menu always switches to it. Undo
and redo work across views: an edit made in the source is one undo step in the rendered view.

Source Code Mode has its own preferences (line numbers, soft wrap, current-line highlight,
visible whitespace, font size): see [Settings](settings.md#source-mode).

## Formatting

Select text and use the **Format** menu or a shortcut, or type the Markdown yourself — writing
`**word**` turns bold as soon as you close it.

| Format | macOS | Windows and Linux | Markdown |
|---|---|---|---|
| Strong (bold) | <kbd>Cmd+B</kbd> | <kbd>Ctrl+B</kbd> | `**text**` |
| Emphasis (italic) | <kbd>Cmd+I</kbd> | <kbd>Ctrl+I</kbd> | `*text*` |
| Underline | <kbd>Cmd+U</kbd> | <kbd>Ctrl+U</kbd> | `<u>text</u>` |
| Code | <kbd>Ctrl+&#96;</kbd> | <kbd>Ctrl+Shift+&#96;</kbd> | `` `code` `` |
| Strike | <kbd>Ctrl+Shift+&#96;</kbd> | <kbd>Alt+Shift+5</kbd> | `~~text~~` |
| Hyperlink | <kbd>Cmd+K</kbd> | <kbd>Ctrl+K</kbd> | `[text](url)` |
| Image | <kbd>Ctrl+Cmd+I</kbd> | <kbd>Ctrl+Shift+I</kbd> | `![alt](path)` |
| Clear Format | <kbd>Cmd+\\</kbd> | <kbd>Ctrl+\\</kbd> | |

The formatting toolbar above the page offers the same commands; **View → Toolbar** hides it.

## Headings, lists and quotes

- **Paragraph → Heading 1–6**: <kbd>Cmd+1</kbd> … <kbd>Cmd+6</kbd>; **Paragraph**:
  <kbd>Cmd+0</kbd>. Or start a line with `#`, `##`, … and a space.
- Start a line with `- ` for a bullet list, `1. ` for a numbered list, `- [ ] ` for a task, `> `
  for a quote. The **Paragraph** menu has the same commands.
- <kbd>Tab</kbd> indents a list item, <kbd>Shift+Tab</kbd> outdents it, and <kbd>Enter</kbd> on an
  empty item ends the list. Click a task box to tick it.
- <kbd>Alt+↑</kbd> / <kbd>Alt+↓</kbd> move the current block or list item up or down.

## Callouts

A quote whose first line starts with `[!name]` is a callout: a coloured block with an icon and a
title.

```markdown
> [!warning] Mind the step
> The body, any Markdown.

> [!success]- Starts closed
> Hidden until opened.

> [!info]+ Starts open
> Can be closed.
```

- **Type**: the name decides the colour and the icon, in any letter case. Known names are note,
  info, tip, hint, important, success, check, done, question, help, faq, warning, caution,
  attention, failure, fail, missing, danger, error, bug, example, quote, cite, abstract, summary,
  tldr and todo. Any other name of letters, digits, `-` and `_` shows in the note style.
- **Title**: the text after the tag, with inline Markdown. Without one, the title is the name with a
  capital first letter.
- **Folding**: `-` after the tag starts the callout closed, `+` starts it open, and without either it
  cannot be folded. Click the header to open or close it. A caret or a search hit going inside opens
  it. This is only how you see it for now: the file does not change, nothing is marked as unsaved,
  there is nothing to undo, and the callout starts as written when you open the file again.

Typing `[!name]` as the first line of a quote makes the callout and puts the caret in its body. Edit
the title by moving up from the body with the arrow keys. To add a title or a fold mark to a callout
that has none, use Source Code Mode, which always shows the text as written. <kbd>Backspace</kbd> at
the start of the title, or of the body of a callout without a title, removes the tag and leaves a
plain quote.

Callouts can be nested and can sit in lists. Exports show them too: see [Export](export.md#callouts).

## Tables

**Paragraph → Table → Insert Table…** (<kbd>Opt+Cmd+T</kbd>, <kbd>Ctrl+Shift+T</kbd> on Windows
and Linux) asks for the size: point at the grid to pick up to 6 columns and 9 rows, or type the
numbers for a larger table, then press <kbd>Enter</kbd> or click **Insert** (<kbd>Esc</kbd>
cancels). The size counts the rows below the header; a header row is always added. The new table
goes after the block the caret is in (inside a list item or quote, it stays there), and the caret
waits in the first header cell. The same command is in the toolbar's **More** menu and in the
document's context menu. Undo removes the whole table in one step.

Click into a cell and type; <kbd>Tab</kbd> moves to the next cell, empty cells included.
**Paragraph → Table** also adds and deletes rows and columns, moves them, and aligns a column.
A new row or column starts empty, with the caret in it, ready for typing. **Add Row Below** is
<kbd>Cmd+Enter</kbd> (<kbd>Ctrl+Enter</kbd>).

## Between blocks

To write a paragraph between two blocks that sit next to each other without one — two tables,
a table and a code block, a formula, a diagram, an image, a horizontal line, an HTML block, or
the end of a quote or list — click in the space between them, or press <kbd>↓</kbd> on the last
line of the first block or <kbd>↑</kbd> on the first line of the second (<kbd>←</kbd> and
<kbd>→</kbd> stop there too). The caret waits on an empty line; nothing is written to the file
until you type, and moving away leaves the file unchanged. <kbd>Enter</kbd> at the end of a
table's last row also goes there. What you type becomes a paragraph of its own, with blank
lines around it so neither neighbour takes it in.

## Code blocks

Fenced code blocks (three backticks and a language name) are syntax-highlighted. Click the
language label to change the language, or the copy button to copy the code. **View → Highlight
Code** turns highlighting off for the session.

## Links

Links open with <kbd>Cmd</kbd>-click (<kbd>Ctrl</kbd>-click on Windows and Linux), so a plain
click can place the caret inside the link text. Links between notes in a folder are covered in
[Folders and vaults](folders-and-vaults.md).

## Images

In the rendered view an image shows at its own size at most, within the column and 400 points high, scaled with the window's zoom. The file, the Markdown and exports stay as they are, and a double click or the expand button shows the image whole.

Paste an image (for example a screenshot) or drag image files into a document. Markise copies
the image into a folder next to the note — `assets` by default — and inserts a Markdown image
with a relative path. Pasted image data is saved as `Pasted image <date and time>.png`.

- Dropping other files inserts a link to them; whether they are copied too is a preference.
- Pasting an image URL inserts it as an image.
- Undo removes the inserted Markdown but never deletes the copied file.
- In a document that has never been saved, images go to Markise's own app-data folder
  (`user-images`, next to the settings file) and are linked with absolute paths.
- Limits: 50 MiB per file; clipboard images up to 100 megapixels.

To look at an image closely, open it in a full-window view: double-click it, or hover it and
press the expand button in its corner. It opens fitted to the window, at its full resolution
even if the document shows a reduced copy. The mouse wheel or a pinch zooms around the pointer,
dragging pans, and <kbd>+</kbd>, <kbd>-</kbd> and <kbd>0</kbd> zoom in, zoom out and fit it again.
<kbd>Esc</kbd> or a click outside the image closes the view. A single click still places the
caret, and <kbd>Cmd</kbd>/<kbd>Ctrl</kbd>+click on an image inside a link still follows the link.

To change an image, hover it. **Replace…** opens the image picker and changes only the path: the
description and the title stay, and an image from anywhere, a web address included, can be
replaced. **Edit…** opens a small window with the
description and the path as written. Each is one undo step, and undo never deletes a copied file.
A reference image (`![text][label]`) has its path in the definition, so it has neither button; a
picture in a block's native view belongs to the block's form; and comments shown as No Markup or
Original leave the document read-only.

The image picker shows thumbnails of the images in the document's folder and its subfolders, the
nearest first, up to 2000 images and six levels down; a note says when there are more. Type to
search by name, use the arrow keys and <kbd>Return</kbd> or double-click to choose, <kbd>Esc</kbd>
to close. The current image is selected, and a dot marks images the document already uses. An image
in the folder is linked where it is. **Browse…** opens the system's file dialog, and a file dropped
on the window is added to the list; both go by the same rules as a drop. A document that has never
been saved has no folder, so it goes straight to the file dialog. A block's form uses the same
picker for **Choose file**.

Where images go is set in **Preferences → Files & Images**; see
[Settings](settings.md#files-and-images). In an Obsidian vault, the vault's attachment folder is
used when it is set.

## Find and replace

| Command | macOS | Windows and Linux |
|---|---|---|
| Find… | <kbd>Cmd+F</kbd> | <kbd>Ctrl+F</kbd> |
| Find Next / Find Previous | <kbd>Cmd+G</kbd> / <kbd>Shift+Cmd+G</kbd> | <kbd>F3</kbd> / <kbd>Shift+F3</kbd> |
| Find and Replace… | <kbd>Alt+Cmd+F</kbd> | <kbd>Ctrl+H</kbd> |
| Replace Next | <kbd>Alt+Cmd+E</kbd> | <kbd>Ctrl+Alt+E</kbd> |

The find bar has toggles for **Match case** (Aa), **Whole word** and **regular expressions**
(.*). With regular expressions, the replacement can use captured groups as `$1` or `${name}`;
write `$$` for a literal dollar sign. **Replace All** is one undo step, and the bar offers
**Undo Replace All** right after it.

Find searches the Markdown source of the current document in every view, link addresses
included; a match hidden by formatting can be shown with **Show in Source**. Other files are not
searched: search across a folder is not available in the beta.

## Copy and paste

Copy and cut put the Markdown source of the selection on the clipboard. Paste inserts text as
plain text; rich text copied from other applications is not converted to Markdown yet.

## Typing helpers

**Preferences → Editor** controls auto-pairing of brackets and quotes (on by default),
auto-pairing of Markdown markers, and smart punctuation (curly quotes, dashes), which is off by
default. See [Settings](settings.md#editor).
