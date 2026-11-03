# FAQ and troubleshooting

## Known limitations

The beta covers everyday writing, but some things are not there yet:

- **Signing:** the macOS and Windows builds are not code-signed; macOS builds are not notarized
  ([Getting started](getting-started.md#install)).
- **Documents and files:** one document per window, no tabs; no search across a folder; links are
  not updated when a file is renamed.
- **Editing:** paste inserts plain text (rich text is not converted to Markdown); the text being
  composed with an input method (IME) is not shown inline until it is committed; there is no
  spell checker.
- **Wikilinks:** embeds (`![[note]]`) and block references (`#^id`) are not supported.
- **Diagrams:** of the Mermaid diagram types, only flowcharts and sequence diagrams are drawn.
- **Export:** no export settings; PDF and PNG show math and diagrams as source text and use
  local images only; the Pandoc path cannot be set by hand ([Export](export.md#limitations-in-the-beta)).
- **Themes:** a subset of CSS; web fonts in themes are not loaded ([Themes](themes.md#what-compatible-means)).
- **Shortcuts:** changed in `settings.json` only; file and view commands cannot be rebound
  ([Keyboard shortcuts](shortcuts.md#change-a-shortcut)).
- **Updates:** Markise tells you about a new version but never installs it.
- **Linux:** 0.1.0-beta.1 is a tarball only; later releases add an AppImage and a `.deb`
  package. No Flatpak and no apt repository.

## Is Markise free? Where is the source?

Markise is free and open source, dual-licensed under MIT or Apache-2.0. The source, issues and
releases are on [GitHub](https://github.com/adjioev/markise).

## Does Markise change my files?

Only when you edit and save. Opening a file and saving it without edits writes it back byte for
byte. Markise stores nothing inside your notes folder except what you create there (notes, pasted
images), and never writes into an Obsidian vault's `.obsidian/` folder.

## Can I use Markise and Obsidian on the same vault?

Yes. Markise reads the vault's link and attachment settings and leaves everything else alone;
see [Folders and vaults](folders-and-vaults.md#obsidian-vaults). Obsidian plugins, canvases and
embeds are not supported in Markise.

## Installation troubleshooting

### macOS says Markise is damaged or cannot be opened

First follow the **Open Anyway** steps in [Getting started](getting-started.md#macos). If macOS
still refuses — for example it reports that the app "is damaged" and offers no **Open Anyway**
button — the download's quarantine flag can be removed in Terminal. Do this only for a copy you
downloaded from the [official releases page](https://github.com/adjioev/markise/releases)
(ideally after checking it against `SHA256SUMS`):

```sh
xattr -dr com.apple.quarantine /Applications/Markise.app
```

### Windows: SmartScreen blocks the app

Click **More info**, then **Run anyway**. If there is no **Run anyway** button, your
organisation's policy may block unsigned programs; ask your administrator.

### Windows or Linux: the window does not open or stays blank

Markise needs a graphics driver with OpenGL 3 (or OpenGL ES 3 on Linux). Update the graphics
driver, then start Markise from a terminal (`markise.exe FILE.md` or `./markise FILE.md`) to see
its messages, and check the log file (below).

### Linux: missing libraries

Install the OpenGL/EGL, libxkbcommon and Wayland or X11 client libraries from your
distribution (the `.deb` package brings them in when installed with `apt install`). The build
needs glibc 2.35 or later.

### Linux: the AppImage does not start (FUSE)

Install `fuse3` (`sudo apt install fuse3` on Debian and Ubuntu), or run
`./markise-x86_64.AppImage --appimage-extract-and-run`.

## Something went wrong

### Where is the log file?

`markise.log` holds start lines, crash locations and fatal errors — never document contents:

| System | Log folder |
|---|---|
| macOS | `~/Library/Logs/Markise/` |
| Windows | `%LOCALAPPDATA%\Markise\logs\` |
| Linux | `~/.local/state/markise/logs/` (or `$XDG_STATE_HOME/markise/logs/`) |

### I lost unsaved changes after a crash

Start Markise again: the **Recover unsaved drafts** window lists the drafts it kept, also
available from **File → Recover Unsaved Drafts…**. See
[Saving and recovery](saving-and-recovery.md).

### How do I reset all settings?

Use **Reset to Defaults** in each Preferences section, or quit Markise and delete
`settings.json` ([Settings](settings.md#the-settings-file)).

## Reporting a bug

Open an issue at [github.com/adjioev/markise/issues](https://github.com/adjioev/markise/issues)
with:

1. the Markise version (**About Markise**) and your operating system and version;
2. what you did, what you expected, and what happened;
3. if possible, a small Markdown file that shows the problem — remove anything private first;
4. for a crash, the log file `markise.log`.

Ideas and feature requests are welcome there too.
