# Next release (unreleased)

Changes on `main` since 0.1.0-beta.1, collected for the next release notes. When cutting the
release, copy this into `docs/release/<version>.md` (keep the structure of
`docs/release/0.1.0-beta.1.md`) and empty this file.

## Downloads

New for Linux x86_64 (glibc 2.35 or later), next to `markise-linux-x86_64.tar.gz`:

| File | Notes |
|---|---|
| `markise-x86_64.AppImage` | one file for any distribution: `chmod +x markise-x86_64.AppImage`, then run it. Needs FUSE (`fuse3`), or start it with `--appimage-extract-and-run` |
| `markise_<version>_amd64.deb` | Debian 12, Ubuntu 22.04 and newer: `sudo apt install ./markise_<version>_amd64.deb` installs `markise` and `markise-cli` with their dependencies, a menu entry, icons and "Open with" for Markdown files |

Both are listed in `SHA256SUMS`. The AppImage contains the AppImage runtime, whose licences
(MIT; libfuse LGPL-2.1, squashfuse, zstd, zlib, musl) are inside it under
`usr/share/doc/markise/appimage-runtime/`. No Flatpak or apt repository yet.

## Site blocks

A document can now use the blocks of a website, described by a **block package** the site owns.
See [Blocks](../user/blocks.md) and [Making block packages](../user/block-packages.md).

- `:::name` containers: parsed as blocks with no fixed list of names, byte-exact in the file, a simple
  card on screen and a plain frame in exports.
- Block packages: `blocks.config.json` binds a folder to a `.tar.gz` by its hash. Nothing is written
  or run before you accept it; the status bar shows the state and offers Stop Using This Package.
- The form: every field of a block in a panel beside the document.
- Native views (callout, card, choices, media) and typing in place, with one undo step per action.
- The block library: `/` on an empty line, or Insert Block…, inserts what the package offers.
- Moving blocks by a handle or with Move Block Up and Down, and Delete Block.
- macOS 14 and later: a block can show as a picture drawn by the site's own code, in a separate
  process with no network and no access to your files. Not interactive. Windows, Linux and older
  macOS show cards for those blocks.
- Switch a block, or all of them, between the native view, the site's picture and the card.
- A `⚠` on a block that does not match what the package expects, with the reasons on hover.
- An example package for a made-up recipe site, in `examples/block-package`
  (`cargo xtask block-package` builds it).

## Images

- In the rendered view, images show at most 400 points high, for every document. The file and
  exports do not change.
- **Replace…** and **Edit…** on an image change its path and description from the rendered view.
- An image picker with thumbnails from the document's folder, also for a block's form. A file a
  form takes from outside the folder is now copied in by the attachment rules rather than refused.

## Changes

- `markise --version` and `markise-cli --version` print the version.
- Linux desktop entry: categories `Utility;TextEditor;` (was also `Office`, which listed
  Markise twice in some menus); AppStream metadata
  (`share/metainfo/io.github.adjioev.markise.metainfo.xml`) in the tarball and both packages.
