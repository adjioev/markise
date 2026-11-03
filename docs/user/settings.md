# Settings

Open **Preferences…** with <kbd>Cmd+,</kbd> (<kbd>Ctrl+,</kbd>; on Windows and Linux it is at the
end of the **Edit** menu). Changes apply immediately. The search field at the top finds a
setting by name.

Most sections have a **Reset to Defaults** button at the bottom (Editor, Markdown, Appearance,
Source Mode, Files & Images); it asks before resetting.

## General

| Setting | Default | |
|---|---|---|
| Custom key bindings | none | Shows your `keyBinding` changes; see [Keyboard shortcuts](shortcuts.md#change-a-shortcut). |
| Show word count | on | The status bar count; click it to switch between words, characters and characters without spaces. |
| Check for updates automatically | not set (off) | Asked on first launch; see [Updates and privacy](updates-and-privacy.md). |

## Editor

| Setting | Default | |
|---|---|---|
| Auto-pair brackets and quotes | on | Typing an opening bracket or quote adds the closing one. |
| Auto-pair Markdown syntax | off | The same for Markdown markers such as `**`. |
| Smart punctuation (convert on input) | off | Straight quotes become curly quotes as you type. |
| Smart dashes: -- as em dash | off | Only with smart punctuation on. Off: `--` gives an en dash and `---` an em dash. |

## Markdown

| Setting | Default | |
|---|---|---|
| Render diagrams | on | Mermaid, sequence and flow fences are drawn; see [Math and diagrams](math-and-diagrams.md). |
| Wikilinks | on | `[[Note]]` links; see [Folders and vaults](folders-and-vaults.md). |
| Comments & suggestions (CriticMarkup) | on | See [Comments and suggestions](comments-and-suggestions.md). |
| Show comments | In the margin | Or "As bubbles (hover to read)". |
| Keep comments in exports | on | HTML and PDF keep comments. |
| Highlight code (this session) | on | Syntax colouring of code blocks, for the current session. |

## Appearance

| Setting | Default | |
|---|---|---|
| Theme | Markise | Complete themes as preview cards. |
| Style, Colors | Markise Default, Markise | Pick them separately; see [Themes](themes.md). |
| Light or dark | System | System, Light or Dark. |
| Theme files | | **Theme Report…**, **Open Theme Folder**, **Import Theme…** |
| Body font, Cyrillic, Greek and CJK font | the style's | Font overrides. |
| Base size, Line height, Content width | the style's | Each has a button to go back to the style's value. |
| Show toolbar, Toolbar position | on, Top | The formatting toolbar above or below the page. |
| Show recent files in the sidebar | on | |
| Zoom | 100 % | 50 % to 300 % in steps of 10 %. |

## Source Mode

| Setting | Default |
|---|---|
| Line numbers | off |
| Soft wrap | on |
| Highlight current line | on |
| Show whitespace | off |
| Font size | automatic (matches the body text), 8–40 pt |

## Files and images

| Setting | Default | |
|---|---|---|
| Automatically save files | off | See [Saving and recovery](saving-and-recovery.md). |
| Autosave interval (minutes) | 5 | 1 to 1440. |
| Back up unsaved drafts | on | |
| Save when closing | off | |
| Remember recent files and folders | on | |
| Save pasted and dropped images to | Folder next to the note | Or "Folder in the root" or "Same folder as the note". |
| Image folder | `assets` | |
| Use the Obsidian attachment folder when set | on | |
| Use relative paths | on | |
| Copy dropped image files into the image folder | on | |
| Copy other dropped files too | off | |
| Paste image URLs as images | on | |

The **Locations** part shows where the settings file and your themes are.

## Export

There are no export settings in the beta: each format uses its built-in options, and Pandoc is
found automatically. See [Export](export.md).

## The settings file

All preferences are stored in one JSON file:

| System | Settings file |
|---|---|
| macOS | `~/Library/Application Support/markise/settings.json` |
| Windows | `%APPDATA%\markise\settings.json` |
| Linux | `~/.config/markise/settings.json` (or `$XDG_CONFIG_HOME/markise/settings.json`) |

You can edit it by hand while Markise is closed; changes made while it runs are read at the
next start. Keys Markise does not know are kept. If the file cannot be read, Markise starts with
the defaults and does not overwrite the file until you change a setting. Several running
instances do not overwrite each other's changes.
