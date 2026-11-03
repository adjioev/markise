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

## Changes

- `markise --version` and `markise-cli --version` print the version.
- Linux desktop entry: categories `Utility;TextEditor;` (was also `Office`, which listed
  Markise twice in some menus); AppStream metadata
  (`share/metainfo/io.github.adjioev.markise.metainfo.xml`) in the tarball and both packages.
