# Keyboard shortcuts

**Help → Keyboard Shortcuts** (<kbd>Shift+Cmd+/</kbd>, that is <kbd>Cmd+?</kbd>; on Windows and
Linux <kbd>Ctrl+Shift+/</kbd>) lists every shortcut that is in effect on your system, including
your own changes. Its search field finds commands by name, key or common alias ("bold" finds
Strong).

The tables below list the default keys. A dash means the command has no default key; it is
still in the menu.

## File

| Command | macOS | Windows and Linux |
|---|---|---|
| New | Cmd+N | Ctrl+N |
| Open… | Cmd+O | Ctrl+O |
| Open Folder… | Shift+Cmd+O | Ctrl+Shift+O |
| Quick Open… | Cmd+P | Ctrl+P |
| Save | Cmd+S | Ctrl+S |
| Save As… | Shift+Cmd+S | Ctrl+Shift+S |
| Close | Cmd+W | Ctrl+W |
| Preferences… | Cmd+, | Ctrl+, |

## Edit

| Command | macOS | Windows and Linux |
|---|---|---|
| Undo | Cmd+Z | Ctrl+Z |
| Redo | Shift+Cmd+Z | Ctrl+Shift+Z or Ctrl+Y |
| Cut, Copy, Paste | Cmd+X, Cmd+C, Cmd+V | Ctrl+X, Ctrl+C, Ctrl+V |
| Select All | Cmd+A | Ctrl+A |
| Select Word | Cmd+D | Ctrl+D |
| Select Line / Sentence | Cmd+L | Ctrl+L |
| Select Styled Scope | Cmd+E | Ctrl+E |
| Select Paragraph / Block | Alt+Cmd+P | Ctrl+Alt+P |
| Delete Word | Shift+Cmd+D | Ctrl+Shift+D |
| Delete Line / Sentence | Shift+Alt+Cmd+L | Ctrl+Alt+Shift+L |
| Delete Styled Scope | Shift+Alt+Cmd+E | Ctrl+Alt+Shift+E |
| Delete Paragraph / Block | Shift+Alt+Cmd+P | Ctrl+Alt+Shift+P |
| Move Row Up / Down (the current block) | Alt+↑ / Alt+↓ | Alt+↑ / Alt+↓ |
| Open Line | Ctrl+O | — |
| Add Comment | Alt+Cmd+M | Ctrl+Alt+M |

## Find

| Command | macOS | Windows and Linux |
|---|---|---|
| Find… | Cmd+F | Ctrl+F |
| Find Next | Cmd+G | F3 |
| Find Previous | Shift+Cmd+G | Shift+F3 |
| Find and Replace… | Alt+Cmd+F | Ctrl+H |
| Replace Next | Alt+Cmd+E | Ctrl+Alt+E |

## Paragraph

| Command | macOS | Windows and Linux |
|---|---|---|
| Heading 1 … Heading 6 | Cmd+1 … Cmd+6 | Ctrl+1 … Ctrl+6 |
| Paragraph | Cmd+0 | Ctrl+0 |
| Increase / Decrease Heading Level | Cmd+= / Cmd+- | Ctrl+= / Ctrl+- |
| Quote | Alt+Cmd+Q | Ctrl+Shift+Q |
| Ordered List | Alt+Cmd+O | Ctrl+Shift+[ |
| Unordered List | Alt+Cmd+U | Ctrl+Shift+] |
| Task List | Alt+Cmd+X | Ctrl+Alt+X |
| Toggle Task Status | Ctrl+X | — |
| Indent / Outdent | Cmd+] / Cmd+[ | Ctrl+] / Ctrl+[ |
| Horizontal Line | Alt+Cmd+- | Ctrl+Alt+- |

### Table (Paragraph → Table)

| Command | macOS | Windows and Linux |
|---|---|---|
| Add Row Below | Cmd+Enter | Ctrl+Enter |
| Delete Row | Shift+Cmd+Backspace | Ctrl+Shift+Backspace |
| Move Table Row Up / Down | Ctrl+Cmd+↑ / ↓ | Ctrl+Alt+↑ / ↓ |
| Move Table Column Left / Right | Ctrl+Cmd+← / → | Ctrl+Alt+← / → |

Add Row Above, Add Column Before and After, Delete Column and the column alignments have no
default key.

## Blocks

Delete Block works on any block. The others need a block package in use; see [Blocks](blocks.md).

| Command | macOS | Windows and Linux |
|---|---|---|
| Insert Block… (or `/` on an empty line) | Alt+Cmd+B | Ctrl+Alt+B |
| Edit Block… | Shift+Cmd+E | Ctrl+Shift+E |
| Move Block Up / Down | Alt+Cmd+↑ / ↓ | Ctrl+Shift+↑ / ↓ |
| Delete Block | Alt+Cmd+⌫ | Ctrl+Shift+Delete |
| Switch This Block's View | Shift+Cmd+M | Ctrl+Shift+M |
| Switch All Blocks' Views | Shift+Alt+Cmd+M | Ctrl+Shift+Alt+M |

### Inside a block

In a block's native view.

| Key | Where | What it does |
|---|---|---|
| Tab / Shift+Tab | any field | next / previous field |
| Return | a list item or answer | what follows the caret goes to a new item after it |
| Return | an empty last item | removes it, and the caret stays at the end of the one before |
| Return | a heading | next field |
| Return | text, a quote | new paragraph |
| Shift+Return | text, a quote | line break in the paragraph |
| Backspace, Delete | an empty item | removes it, unless it is the only one |
| Backspace | between two paragraphs of one text field | joins them |
| Esc | while dragging a block | cancels the drag |

### The block library

| Key | What it does |
|---|---|
| ↑ / ↓ | previous / next row |
| Return | inserts the row; with no row matching, closes and types what you typed |
| / | right after the first `/`: closes and types one `/` |
| Esc, or Backspace with nothing typed | closes |

### The block form

While the keyboard is in the form. Other document commands wait until it closes.

| Command | macOS | Windows and Linux |
|---|---|---|
| Save the form | Cmd+Return or Cmd+S | Ctrl+Enter or Ctrl+S |
| Cancel | Esc or Cmd+W | Esc or Ctrl+W |
| Move a row up / down | Alt+↑ / Alt+↓ | Alt+↑ / Alt+↓ |

### The image picker

| Key | What it does |
|---|---|
| Arrow keys | move through the images |
| Return | uses the selected image |
| Esc | closes |

## Format

| Command | macOS | Windows and Linux |
|---|---|---|
| Strong | Cmd+B | Ctrl+B |
| Emphasis | Cmd+I | Ctrl+I |
| Underline | Cmd+U | Ctrl+U |
| Code | Ctrl+&#96; | Ctrl+Shift+&#96; |
| Strike | Ctrl+Shift+&#96; | Alt+Shift+5 |
| Highlight | Shift+Cmd+H | Ctrl+Shift+H |
| Inline Math | Ctrl+M | — |
| Comment | Ctrl+- | — |
| Hyperlink | Cmd+K | Ctrl+K |
| Image | Ctrl+Cmd+I | Ctrl+Shift+I |
| Clear Format | Cmd+\\ | Ctrl+\\ |

## View and Go

| Command | macOS | Windows and Linux |
|---|---|---|
| Source Code Mode | Cmd+/ | Ctrl+/ |
| Side by Side | Ctrl+Cmd+/ | Ctrl+Alt+/ |
| Toggle Sidebar | Shift+Cmd+L | Ctrl+Shift+L |
| Zoom In / Zoom Out | Shift+Cmd+= / Shift+Cmd+- | Ctrl+Shift+= / Ctrl+Shift+- |
| Actual Size | Shift+Cmd+0 | Ctrl+Shift+0 |
| Full Screen | Ctrl+Cmd+F | F11 |
| Back / Forward | Alt+Cmd+← / Alt+Cmd+→ | Alt+← / Alt+→ |
| Keyboard Shortcuts | Shift+Cmd+/ | Ctrl+Shift+/ |

## Moving and selecting

| Action | macOS | Windows and Linux |
|---|---|---|
| Word left / right | Alt+← / Alt+→ | Ctrl+← / Ctrl+→ |
| Line start / end | Cmd+← / Cmd+→, Home / End | Home / End |
| Document start / end | Cmd+↑ / Cmd+↓ | Ctrl+Home / Ctrl+End |
| Page up / down | Page Up / Page Down | Page Up / Page Down |
| Delete word backward / forward | Alt+Backspace / Alt+Delete | Ctrl+Backspace / Ctrl+Delete |
| Line break inside a paragraph | Shift+Return | Shift+Enter |

Every movement key with <kbd>Shift</kbd> extends the selection.

## Shortcuts by view

In Source Code Mode, and in the source pane of Side by Side, the **Format** and **Paragraph**
commands are not available; typing, undo, selection and movement keys work as usual. Pressing
the Source Code Mode shortcut in Side by Side switches to the full source view.

## Change a shortcut

There is no shortcut editor yet. Add a `"keyBinding"` object to `settings.json` (see
[Settings](settings.md#the-settings-file)) that maps a command's name or id to a key, and
restart Markise:

```json
{
  "keyBinding": {
    "Strong": "Ctrl+Shift+K",
    "Toggle Task Status": ""
  }
}
```

- The command loses its default key; an empty string removes the key altogether.
- If two commands get the same key, the later entry wins.
- Unknown command names, keys that cannot be read, and system keys are ignored. Reserved on
  macOS: Cmd+H, Alt+Cmd+H, Cmd+M, Cmd+Q, Cmd+,, Cmd+Tab, Cmd+&#96; and Cmd+Space; on Windows and
  Linux: Alt+F4 and Alt+Tab.
- Editing and formatting commands can be changed this way. File, view and navigation commands
  (Quick Open, Close, zoom, Side by Side, Back and Forward, Keyboard Shortcuts) cannot be
  rebound yet.

**Preferences → General → Custom key bindings** shows the changes in effect.
