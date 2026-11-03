# Releasing Markise

Release artifacts are built by `cargo xtask dist <platform>` (xtask/src/dist.rs) and packaged
by `scripts/package.sh`; `.github/workflows/release.yml` runs the same command for every
platform. Publishing is always a manual, owner-approved step: the workflow only ever creates a
**draft** release.

| Platform | Artifact | Contents |
|---|---|---|
| `macos-universal` | `markise-macos-universal.dmg` | `Markise.app` (aarch64 + x86_64, ad-hoc signed, `.md`/`.markdown` document types), `/Applications` link, `markise-cli`, licences |
| `windows-x86_64` | `markise-windows-x86_64.zip` | `markise.exe` (icon embedded, no console window), `markise-cli.exe`, licences |
| `linux-x86_64` | `markise-linux-x86_64.tar.gz` | `markise`, `markise-cli`, `share/` (desktop entry, hicolor icons, AppStream metainfo), licences |
| | `markise-x86_64.AppImage` | the same files in an AppDir (`AppRun`, desktop entry, icon), AppImage type 2 runtime; no bundled libraries |
| | `markise_<version>_amd64.deb` | `/usr/bin/markise`, `/usr/bin/markise-cli`, `/usr/share/{applications,icons/hicolor,metainfo}`, `/usr/share/doc/markise` (licences, Debian `copyright`) |
| | `SHA256SUMS` | SHA-256 of all of the above |

Tag runs use versionless asset names (for `/releases/latest/download/…` links); local and
dry-run artifacts carry the label, e.g. `markise-0.1.0-beta.1-macos-universal.dmg`. The `.deb`
always carries the label (`markise_0.1.0-beta.1_amd64.deb`, the Debian convention).

## Linux packages

All three Linux artifacts are made by `scripts/package.sh linux-x86_64` from the same staged
files (`LINUX_FORMATS` picks a subset) on `ubuntu-22.04`, so they run on glibc ≥ 2.35. The
`build` job prints the libraries the binary links (`NEEDED`: libc, libm, libgcc_s), the sonames
it may `dlopen`, and its highest glibc symbol version. The `linux-smoke` job installs the `.deb`
on clean ubuntu-22.04 and ubuntu-24.04 runners (`dpkg -i`, `apt-get -f install`), runs
`markise --version`, `markise-cli html`, and `scripts/smoke-start.sh` under Xvfb, removes the
package, then starts the AppImage with `--appimage-extract-and-run` the same way (and through
a `markise-cli` link). The draft release waits for it.

**Libraries.** The binary links only libc, libm, libgcc_s and the dynamic loader (`NEEDED`) and
needs glibc symbols up to 2.35. Everything else is loaded with `dlopen` (sonames found in the
dry-run binary): X11 (libX11, libX11-xcb, libxcb, libXcursor, libXi, libXrender,
libxkbcommon-x11) and Wayland (libwayland-client, libwayland-egl, libxkbcommon) for winit,
libEGL/libGL/libvulkan for glutin and wgpu, libfontconfig for system fonts (yeslogic-fontconfig),
libdbus-1 for the file dialogs (rfd's xdg-desktop-portal backend). The AppImage bundles **none**
of them: the GL/EGL/Vulkan loaders must match the host's drivers (AppImage practice is never to
bundle them), and the others are stable-ABI libraries present on every desktop with a display
server (most are on the AppImage exclude list for that reason).
The AppImage therefore has the same requirements as the tarball, and NOTICE/THIRD_PARTY
cover its contents, plus the runtime below.

**.deb dependencies.** `Depends` = the `dpkg-shlibdeps` result (`libc6 (>= 2.35), libgcc-s1`)
plus the `dlopen`ed libraries above by their Debian/Ubuntu package names (`libx11-6,
libx11-xcb1, libxcb1, libxcursor1, libxi6, libxrender1, libxkbcommon-x11-0, libxkbcommon0,
libwayland-client0, libwayland-egl1, libegl1, libgl1, libfontconfig1, libdbus-1-3`), which
dpkg-shlibdeps cannot see. `Recommends: libvulkan1`
(wgpu's Vulkan backend; glow over OpenGL takes over without it) and `xdg-desktop-portal` (file
dialogs). Version: the label with every `-` turned into `~` (`0.1.0~beta.1`, dry runs
`0.1.0~beta.1~dryrun.<sha7>`), so prereleases sort before the final `0.1.0`. No maintainer
scripts: the desktop and icon caches are refreshed by the dpkg triggers of
desktop-file-utils and the icon theme. `text/markdown` comes from shared-mime-info, so no MIME
definition is shipped.

**AppImage tools** (`scripts/fetch-appimage-tools.sh`, downloaded on the build runner, SHA-256
checked, cached in `~/.cache/markise-tools`):

| Tool | Version | SHA-256 | Licence | Shipped? |
|---|---|---|---|---|
| appimagetool-x86_64.AppImage | release 1.9.1 (reports "continuous build 296, git 8c8c91f") | `ed4ce84f…cb384eb0` | MIT | no, build tool |
| type2-runtime `runtime-x86_64` | release 20251108 | `2fca8b44…3a47ec260d` | MIT; static libfuse 3.15 (LGPL-2.1), squashfuse (BSD-2-Clause), zstd (BSD-3-Clause), zlib, musl (MIT) | **yes**, the first part of the AppImage |

The runtime's licence texts are in `packaging/linux/appimage-runtime/` and inside the AppImage
(`usr/share/doc/markise/appimage-runtime/`). dpkg-deb, dpkg-shlibdeps, desktop-file-validate
and appstreamcli (validation run by appimagetool) are Ubuntu build tools, not shipped. No
update information (zsync) is embedded in the AppImage.

**Reproducibility.** The `.deb` holds the normalised tree (sorted, SOURCE_DATE_EPOCH times,
root ownership, xz) and the AppImage a squashfs written by appimagetool's mksquashfs; two local
builds of the same inputs (ubuntu:22.04 container, placeholder binaries) gave byte-identical
`.tar.gz`, `.deb` and `.AppImage`. As for the other platforms, the binaries themselves are only
as reproducible as the toolchain and runner image.

## Local build

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin   # once (macOS)
cargo xtask dist macos-universal        # -> dist/markise-<version>-macos-universal.dmg
cargo xtask dist --help                 # all platforms and options
```

Each platform is built on its own OS (no cross-compilation). The `dist` profile is `release`
plus thin LTO and stripped symbols.

## Dry run (no release)

```sh
gh workflow run release.yml --ref <branch>      # or Actions > Release > Run workflow
gh run watch
gh run download <run-id> -D /tmp/markise-dry-run
```

Builds the selected commit as `<version>-dryrun.<sha7>` and uploads the archives and
`SHA256SUMS` to the workflow run only (7 days).

## Cutting a release

1. On a branch, set `version` in `[workspace.package]` of `Cargo.toml` (e.g. `0.1.0-beta.1`),
   run `cargo check` so `Cargo.lock` follows, and write `docs/release/<version>.md` (start from
   `docs/release/next.md`, which collects changes since the last release, then empty it); merge
   to `main` (owner approval).
2. Optional: dry-run the workflow on `main` and smoke-test the artifacts.
3. Tag the merge commit and push the tag (owner approval):

   ```sh
   git switch main && git pull --ff-only
   git tag -a v0.1.0-beta.1 -m "Markise 0.1.0-beta.1"
   git push origin v0.1.0-beta.1
   ```

   The workflow checks that the tag equals `v` + the workspace version and that the release
   notes file exists, builds all platforms, and creates a new **draft** release (a GitHub
   *pre-release* when the version has a `-beta`/`-rc` part) with the archives and
   `SHA256SUMS`. It refuses to run if the tag already has a published release.
4. Download the draft's assets, check them (`shasum -a 256 -c SHA256SUMS`), launch each app
   (Linux: the AppImage, and the `.deb` via `sudo apt install ./markise_*_amd64.deb`).
5. Edit the draft notes if needed (check the feature list against `main`), then **Publish release**.

A failed tag run can be re-run from the Actions page; each successful run creates a new draft,
so delete stale drafts by hand. To move a tag that has no published release:
`git push origin :refs/tags/vX` and tag again.

## Not done yet

- macOS Developer ID signing and notarization, Windows Authenticode signing (need owner
  accounts and secrets).
- Linux Flatpak, a signed apt repository, AppImage update information (zsync).
- The .dmg is not byte-reproducible (disk images carry creation dates and UUIDs); the archive
  contents follow the rules in `scripts/package.sh`.
