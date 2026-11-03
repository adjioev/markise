# Getting started

## Download

Markise is published on the [GitHub releases page](https://github.com/adjioev/markise/releases).
The current version is the first beta, [**0.1.0-beta.1**](https://github.com/adjioev/markise/releases/tag/v0.1.0-beta.1).

| Platform | File | Requirements |
|---|---|---|
| macOS | `markise-macos-universal.dmg` | macOS 11 Big Sur or later, Apple silicon or Intel |
| Windows | `markise-windows-x86_64.zip` | Windows 10 or 11, x86_64, a graphics driver with OpenGL 3 |
| Linux | `markise-linux-x86_64.tar.gz` | x86_64, glibc 2.35 or later (for example Ubuntu 22.04, Debian 12, Fedora 36 and newer), X11 or Wayland |

Each download also contains `markise-cli` (a command-line tool for HTML export and round-trip
checks), the licences, `NOTICE` and `THIRD_PARTY/`.

### Check the download (optional)

Each release has a `SHA256SUMS` file. Put it next to the download and run:

```sh
shasum -a 256 -c SHA256SUMS --ignore-missing     # macOS
sha256sum -c SHA256SUMS --ignore-missing         # Linux
```

On Windows, compare the output of `Get-FileHash markise-windows-x86_64.zip` (PowerShell) with
the line in `SHA256SUMS`.

## Install

The beta builds are **not code-signed**: the macOS app has no Apple Developer ID signature and
is not notarized, and the Windows program has no Authenticode signature. Your system therefore
asks for confirmation the first time you open Markise.

### macOS

1. Open `markise-macos-universal.dmg` and drag **Markise** onto **Applications**.
2. Open Markise from Applications. macOS says it cannot verify the app; close that message
   (**Done**, not **Move to Trash**).
3. Open **System Settings → Privacy & Security**, scroll to **Security** and click
   **Open Anyway** next to the note about Markise. Confirm with your password, then click
   **Open**.

From then on Markise opens normally. This is Apple's procedure for apps from unidentified
developers, described in
[Apple's guide to opening apps safely](https://support.apple.com/en-au/102445). On macOS 11 and
12 the setting is in **System Preferences → Security & Privacy → General**. If there is no
**Open Anyway** button, see [the FAQ](faq.md#macos-says-markise-is-damaged-or-cannot-be-opened).

Markise registers itself for `.md` and `.markdown` files: use **Open With → Markise** in Finder,
or make it the default under **Get Info → Open with → Change All…**. To use `markise-cli`, copy
it from the disk image to a folder on your `PATH`.

### Windows

1. Right-click the zip file, choose **Extract All…**, and pick a folder for Markise.
2. Run `markise.exe` from the extracted folder.
3. If Microsoft Defender SmartScreen shows "Windows protected your PC", click **More info**,
   check that the app name is `markise.exe`, then click **Run anyway**.

To open Markdown files with Markise, right-click a `.md` file → **Open with → Choose another
app** → browse to `markise.exe`.

### Linux

```sh
tar xzf markise-linux-x86_64.tar.gz
cd markise-*-linux-x86_64
./markise
```

Optionally install it for your user, with a menu entry, an icon and "Open with" support:

```sh
mkdir -p ~/.local/bin && cp markise markise-cli ~/.local/bin/
cp -r share ~/.local/
```

Markise needs OpenGL or OpenGL ES 3, libxkbcommon, and the Wayland or X11 client libraries;
desktop systems normally have them. There is no AppImage, .deb or Flatpak yet.

## First launch

- The **tutorial** opens as a scratch document. Edit it freely: it is a copy, and your files are
  never touched. Open it again any time from **Help → Tutorial**.
- A small card in the corner asks **"Check for updates automatically?"** with the buttons
  **Check Automatically** and **Not Now**. Nothing is sent until you agree; see
  [Updates and privacy](updates-and-privacy.md).

## Open, write and save

- **File → Open…** (<kbd>Cmd+O</kbd>) opens a file. **File → Open Folder…**
  (<kbd>Shift+Cmd+O</kbd>) shows a folder in the sidebar; see
  [Folders and vaults](folders-and-vaults.md).
- **File → New** (<kbd>Cmd+N</kbd>) starts an untitled document.
- **File → Save** (<kbd>Cmd+S</kbd>) saves; the first save of a new document asks for a name.
- Autosave is off by default, but unsaved changes are backed up as drafts and offered again
  after a crash; see [Saving and recovery](saving-and-recovery.md).

Next: [Editing](editing.md).
