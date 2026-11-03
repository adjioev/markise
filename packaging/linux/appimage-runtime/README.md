# Licences of the AppImage runtime

`markise-x86_64.AppImage` starts with the **AppImage type 2 runtime**, a small static program
that mounts (or extracts) the image appended to it and starts Markise. The runtime is not part
of Markise and is not linked with it; it is shipped inside the same file. `scripts/package.sh`
copies this folder into the image (`usr/share/doc/markise/appimage-runtime/`).

Runtime used: `runtime-x86_64` of https://github.com/AppImage/type2-runtime release
**20251108** (tag commit `dd6cebedcbddde9c82f89b011e8e1d40b6e43868`), SHA-256
`2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d` (pinned and verified by
`scripts/fetch-appimage-tools.sh`). Its own code is MIT; it statically links the libraries
below (versions from its build scripts at that tag: libfuse and squashfuse are downloaded with
pinned checksums, the rest are Alpine Linux 3.21 packages).

| Component | Version | Licence | Text in this folder |
|---|---|---|---|
| type2-runtime | 20251108 | MIT | `type2-runtime-MIT.txt` (the repository's `LICENSE`, which also lists the components) |
| libfuse | 3.15.0, patched (`patches/libfuse/mount.c.diff` in type2-runtime) | LGPL-2.1 | `libfuse-LGPL-2.1.txt` (libfuse `LGPL2.txt` at `fuse-3.15.0`) |
| squashfuse | 0.5.2 | BSD-2-Clause | `squashfuse-BSD-2-Clause.txt` (`LICENSE` at `0.5.2`) |
| zstd | 1.5.6 (Alpine 3.21 `zstd-static`) | BSD-3-Clause (zstd is dual BSD-3-Clause / GPL-2.0; used under BSD) | `zstd-BSD-3-Clause.txt` (`LICENSE` at `v1.5.6`) |
| zlib | 1.3.1 (Alpine 3.21 `zlib-static`) | Zlib | `zlib.txt` (`LICENSE` at `v1.3.1`) |
| musl libc | 1.2.5 (Alpine 3.21) | MIT | `musl-MIT.txt` (`COPYRIGHT` at `v1.2.5`) |

Corresponding source for the LGPL-2.1 part (libfuse 3.15.0 and the patch) and the build
scripts: https://github.com/AppImage/type2-runtime/tree/20251108 and
https://github.com/libfuse/libfuse/releases/tag/fuse-3.15.0. The runtime is a separate
executable, so a user can replace it (`--appimage-extract` gives the unpacked application,
which runs without it).

When the pinned runtime changes, update the versions and texts here.
