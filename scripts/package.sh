#!/usr/bin/env bash
# Package release binaries into a distributable archive.
#
# usage: scripts/package.sh <platform> <label> <bin-dir> <out-dir>
#   platform: macos-universal | macos-aarch64 | macos-x86_64 | linux-x86_64 | windows-x86_64 | …
#             (the prefix selects the format, the whole string goes into the file name)
#   label:    archive/folder label, e.g. v0.1.0-beta.1 or 0.1.0-beta.1-abcdef0 (a leading "v"
#             is stripped)
#   bin-dir:  folder with the release binaries markise and markise-cli (.exe on Windows)
#   out-dir:  where the archive is written
#
# Environment:
#   BUNDLE_VERSION       macOS CFBundleShortVersionString (N.N.N; a SemVer prerelease or build
#                        suffix such as -beta.1 is stripped). Default: the leading N.N.N of the
#                        label; the script fails if neither is numeric.
#   BUILD_NUMBER         macOS CFBundleVersion, a non-negative integer. Default (deterministic
#                        for a given version): major*1000000 + minor*1000 + patch of the
#                        bundle version (minor and patch must then be < 1000).
#   SOURCE_DATE_EPOCH    timestamp for every archive entry (default: the HEAD commit time).
#   PACKAGE_STABLE_NAME  1 = name the archive markise-<platform>.<ext> (no label), so
#                        https://github.com/<owner>/<repo>/releases/latest/download/… links work.
#   MACOS_FORMAT         dmg (default) or zip.
#   LINUX_FORMATS        Linux outputs, space-separated: any of tar.gz deb AppImage (default:
#                        all three). deb needs dpkg-deb and dpkg-shlibdeps (dpkg-dev); AppImage
#                        downloads the pinned appimagetool and runtime
#                        (scripts/fetch-appimage-tools.sh).
#
# Output (prints the archive path on stdout), each with README.md, LICENSE-MIT, LICENSE-APACHE,
# NOTICE, THIRD_PARTY/ (full license texts of bundled third-party data, e.g. syntax
# definitions) and BUILD-INFO.txt (commit, toolchain, runner image):
#   macOS    markise-<label>-<platform>.dmg: Markise.app (icon, .md/.markdown document types,
#            licences also in Contents/Resources), an /Applications link, markise-cli
#            (or .zip with MACOS_FORMAT=zip)
#   Windows  markise-<label>-<platform>.zip: markise.exe (icon embedded), markise-cli.exe
#   Linux    markise-<label>-<platform>.tar.gz: markise, markise-cli and share/ (desktop entry,
#            hicolor icons, AppStream metainfo; `cp -r share ~/.local/` installs them for the
#            current user)
#            markise_<label>_<arch>.deb (always this name, e.g. markise_0.1.0-beta.1_amd64.deb;
#            the package Version comes from scripts/deb-version.sh, 0.1.0~beta.1-1, which sorts
#            before 0.1.0-1; GitHub would rewrite "~" in an asset name): /usr/bin/markise and
#            markise-cli, the same share/ files under /usr/share, licences and a Debian
#            copyright file in /usr/share/doc/markise; Depends from dpkg-shlibdeps plus the
#            dlopen()ed X11/Wayland/EGL/GL libraries
#            markise-<label>-<arch>.AppImage (markise-<arch>.AppImage with
#            PACKAGE_STABLE_NAME=1): the same files in an AppDir, built by appimagetool with
#            the pinned type 2 runtime; no libraries bundled (see docs/release/README.md)
#            Linux prints one path per line.
#
# Reproducibility: entries are sorted (C locale), all timestamps are SOURCE_DATE_EPOCH,
# permissions are normalised (755 executables/dirs, 644 files), tar records owner/group 0 and
# gzip stores no name/time; zip entries carry no extra attributes. Remaining limits: the
# binaries themselves are only as reproducible as the toolchain and runner image (recorded in
# BUILD-INFO.txt; GitHub's macOS/Windows images float), the .deb carries the same normalised
# tree (dpkg-deb sorts entries; ar times come from SOURCE_DATE_EPOCH; xz), the AppImage is a
# squashfs made by the mksquashfs inside the pinned appimagetool (see docs/release/README.md for
# what was checked), Windows archives are made with 7-Zip
# when `zip` is missing (its version is recorded), and ZIP stores DOS local time (the script
# forces TZ=UTC). The .dmg (hdiutil, UDZO) is not byte-reproducible: HFS+/APFS images carry
# creation dates and UUIDs; its contents are. macOS bundles are ad-hoc signed only (no Developer ID, no notarization yet —
# see .github/workflows/release.yml).
set -euo pipefail
export LC_ALL=C TZ=UTC

platform="${1:?platform}"
label="${2:?label}"
label="${label#v}"
bin_dir="${3:?bin-dir}"
out_dir="${4:?out-dir}"
root="$(cd "$(dirname "$0")/.." && pwd)"
name="markise-$label-$platform"
archive_name="$name"
if [[ "${PACKAGE_STABLE_NAME:-0}" == 1 ]]; then archive_name="markise-$platform"; fi

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  SOURCE_DATE_EPOCH="$(git -C "$root" log -1 --format=%ct)"
fi
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || { echo "bad SOURCE_DATE_EPOCH: $SOURCE_DATE_EPOCH" >&2; exit 2; }
# POSIX `touch -t` stamp (works with BSD and GNU date)
stamp="$(date -u -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S 2>/dev/null || date -u -d "@$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S)"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
dest="$stage/$name"
mkdir -p "$dest"
cp "$root/README.md" "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/NOTICE" "$dest/"
cp -R "$root/THIRD_PARTY" "$dest/THIRD_PARTY"
{
  echo "Markise $label ($platform)"
  echo "commit: $(git -C "$root" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "source-date-epoch: $SOURCE_DATE_EPOCH"
  echo "runner: ${ImageOS:-local} ${ImageVersion:-} (${RUNNER_OS:-$(uname -s)} ${RUNNER_ARCH:-$(uname -m)})"
  echo "cargo: $(cargo -V 2>/dev/null || echo unknown)"
  rustc -vV 2>/dev/null | sed 's/^/rustc: /' || true
} > "$dest/BUILD-INFO.txt"

# Normalise permissions and timestamps, then list entries in a stable order.
normalise() {
  find "$dest" -type d -exec chmod 755 {} +
  find "$dest" -type f -exec chmod 644 {} +
  local exe
  for exe in "$@"; do chmod 755 "$dest/$exe"; done
  find "$dest" -exec touch -h -t "$stamp" {} +
}
sorted_files() { (cd "$stage" && find "$name" -type f | sort); }

make_zip() {
  local archive="$1"
  rm -f "$archive"
  if command -v zip >/dev/null; then
    (cd "$stage" && sorted_files | zip -X -D -9 -q "$archive" -@)
  else
    sorted_files > "$stage/.list"
    (cd "$stage" && 7z a -tzip -mx=9 -bso0 -bsp0 "$archive" @.list)
    echo "7z: $(7z i 2>/dev/null | head -n 2 | tail -n 1)" >&2
  fi
}

case "$platform" in
  macos-*)
    short="${BUNDLE_VERSION:-}"
    short="${short%%[-+]*}"
    if [[ -z "$short" && "$label" =~ ^([0-9]+\.[0-9]+\.[0-9]+) ]]; then short="${BASH_REMATCH[1]}"; fi
    [[ "$short" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "BUNDLE_VERSION must be N.N.N (got '$short')" >&2; exit 2; }
    if [[ -z "${BUILD_NUMBER:-}" ]]; then
      IFS=. read -r v_major v_minor v_patch <<< "$short"
      if (( 10#$v_minor >= 1000 || 10#$v_patch >= 1000 )); then
        echo "minor/patch >= 1000: set BUILD_NUMBER explicitly" >&2; exit 2
      fi
      BUILD_NUMBER=$(( 10#$v_major * 1000000 + 10#$v_minor * 1000 + 10#$v_patch ))
    fi
    build="$BUILD_NUMBER"
    [[ "$build" =~ ^(0|[1-9][0-9]*)$ ]] || { echo "BUILD_NUMBER must be a non-negative integer (got '$build')" >&2; exit 2; }

    app="$dest/Markise.app/Contents"
    mkdir -p "$app/MacOS" "$app/Resources"
    cp "$bin_dir/markise" "$app/MacOS/markise"
    cp "$root/assets/brand/markise.icns" "$app/Resources/markise.icns"
    cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/NOTICE" "$app/Resources/"
    cp -R "$root/THIRD_PARTY" "$app/Resources/THIRD_PARTY"
    cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Markise</string>
  <key>CFBundleDisplayName</key><string>Markise</string>
  <key>CFBundleIdentifier</key><string>io.github.adjioev.markise</string>
  <key>CFBundleExecutable</key><string>markise</string>
  <key>CFBundleIconFile</key><string>markise</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$short</string>
  <key>CFBundleVersion</key><string>$build</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Markise contributors, MIT OR Apache-2.0</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Markdown document</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array><string>net.daringfireball.markdown</string></array>
      <key>CFBundleTypeExtensions</key>
      <array><string>md</string><string>markdown</string></array>
    </dict>
  </array>
  <key>UTImportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>net.daringfireball.markdown</string>
      <key>UTTypeDescription</key><string>Markdown document</string>
      <key>UTTypeConformsTo</key>
      <array><string>public.plain-text</string></array>
      <key>UTTypeReferenceURL</key><string>https://daringfireball.net/projects/markdown/</string>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key>
        <array><string>md</string><string>markdown</string></array>
        <key>public.mime-type</key>
        <array><string>text/markdown</string></array>
      </dict>
    </dict>
  </array>
</dict>
</plist>
PLIST
    plutil -lint "$app/Info.plist" >/dev/null
    cp "$bin_dir/markise-cli" "$dest/markise-cli"
    normalise "Markise.app/Contents/MacOS/markise" "markise-cli"
    # Ad-hoc signature so Apple Silicon runs the (lipo'd) binaries; not a Developer ID signature.
    codesign --force --sign - "$dest/markise-cli" 2>/dev/null
    codesign --force --deep --sign - "$dest/Markise.app" 2>/dev/null
    find "$dest" -exec touch -h -t "$stamp" {} +
    case "${MACOS_FORMAT:-dmg}" in
      zip)
        archive="$out_dir/$archive_name.zip"
        make_zip "$archive"
        ;;
      dmg)
        archive="$out_dir/$archive_name.dmg"
        rm -f "$archive"
        ln -s /Applications "$dest/Applications"
        touch -h -t "$stamp" "$dest/Applications"
        # hdiutil occasionally fails with "Resource busy" on CI runners: retry
        for attempt in 1 2 3 4 5; do
          if hdiutil create -quiet -volname "Markise" -srcfolder "$dest" -fs HFS+ \
              -format UDZO -imagekey zlib-level=9 -ov "$archive"; then
            break
          fi
          (( attempt < 5 )) || { echo "hdiutil create failed" >&2; exit 1; }
          sleep $(( attempt * 3 ))
        done
        hdiutil verify -quiet "$archive"
        ;;
      *)
        echo "MACOS_FORMAT must be dmg or zip (got '${MACOS_FORMAT}')" >&2
        exit 2
        ;;
    esac
    ;;
  linux-*)
    app_id=io.github.adjioev.markise
    # deb architecture / AppImage architecture for the platform
    case "$platform" in
      linux-x86_64) deb_arch=amd64 appimage_arch=x86_64 ;;
      *) deb_arch="" appimage_arch="" ;;
    esac
    formats="${LINUX_FORMATS:-tar.gz deb AppImage}"
    # desktop entry, hicolor icons and AppStream metadata into <prefix>/share
    install_share() {
      local share="$1/share"
      mkdir -p "$share/applications" "$share/metainfo"
      cp "$root/packaging/linux/$app_id.desktop" "$share/applications/"
      sed -e "s/@VERSION@/$label/" -e "s/@DATE@/$(date -u -r "$SOURCE_DATE_EPOCH" +%F 2>/dev/null || date -u -d "@$SOURCE_DATE_EPOCH" +%F)/" \
        "$root/packaging/linux/$app_id.metainfo.xml" > "$share/metainfo/$app_id.metainfo.xml"
      local size
      for size in 16 32 48 64 128 256 512; do
        mkdir -p "$share/icons/hicolor/${size}x${size}/apps"
        cp "$root/assets/brand/png/markise-$size.png" "$share/icons/hicolor/${size}x${size}/apps/$app_id.png"
      done
    }
    # licences and build info into a doc folder
    install_docs() {
      mkdir -p "$1"
      cp "$dest/README.md" "$dest/LICENSE-MIT" "$dest/LICENSE-APACHE" "$dest/NOTICE" "$dest/BUILD-INFO.txt" "$1/"
      cp -R "$dest/THIRD_PARTY" "$1/THIRD_PARTY"
    }
    # like normalise, for another tree: 755 dirs and the given executables, 644 files
    normalise_tree() {
      local tree="$1"; shift
      find "$tree" -type d -exec chmod 755 {} +
      find "$tree" -type f -exec chmod 644 {} +
      local exe
      for exe in "$@"; do chmod 755 "$tree/$exe"; done
      find "$tree" -exec touch -h -t "$stamp" {} +
    }

    cp "$bin_dir/markise" "$bin_dir/markise-cli" "$dest/"
    install_share "$dest"
    normalise markise markise-cli
    for format in $formats; do
      case "$format" in
        tar.gz)
          archive="$out_dir/$archive_name.tar.gz"
          tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
            --format=gnu -cf - "$name" | gzip -n -9 > "$archive"
          echo "$archive"
          ;;
        deb)
          [[ -n "$deb_arch" ]] || { echo "no .deb architecture for $platform" >&2; exit 2; }
          # Debian version (scripts/deb-version.sh): the first "-" becomes "~" so prereleases
          # sort before the release (0.1.0~beta.1-1 < 0.1.0-1), Debian revision "-1".
          deb_version="$("$root/scripts/deb-version.sh" "$label")"
          pkg="$stage/deb/markise"
          mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin"
          cp "$bin_dir/markise" "$bin_dir/markise-cli" "$pkg/usr/bin/"
          install_share "$pkg/usr"
          install_docs "$pkg/usr/share/doc/markise"
          cat > "$pkg/usr/share/doc/markise/copyright" <<COPYRIGHT
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Markise
Upstream-Contact: https://github.com/adjioev/markise/issues
Source: https://github.com/adjioev/markise

Files: *
Copyright: 2026 The Markise contributors
License: MIT or Apache-2.0
Comment: Third-party components compiled into the programs (syntax definitions, fonts) and
 their licences are listed in /usr/share/doc/markise/NOTICE; full licence texts are in
 /usr/share/doc/markise/THIRD_PARTY/, LICENSE-MIT and LICENSE-APACHE.

License: MIT
 See /usr/share/doc/markise/LICENSE-MIT.

License: Apache-2.0
 See /usr/share/common-licenses/Apache-2.0 (also /usr/share/doc/markise/LICENSE-APACHE).
COPYRIGHT
          maintainer="The Markise contributors <4410+adjioev@users.noreply.github.com>"
          changelog_date="$(date -u -R -r "$SOURCE_DATE_EPOCH" 2>/dev/null || date -u -R -d "@$SOURCE_DATE_EPOCH")"
          printf '%s\n' "markise ($deb_version) unstable; urgency=medium" "" \
            "  * Markise $label." "  * Release notes: https://github.com/adjioev/markise/releases" "" \
            " -- $maintainer  $changelog_date" | gzip -n -9 > "$pkg/usr/share/doc/markise/changelog.Debian.gz"
          # Shared-library dependencies of the ELF files (libc6, libgcc-s1), from dpkg-shlibdeps.
          shlibs="$(cd "$stage/deb" && mkdir -p debian && printf 'Source: markise\n\nPackage: markise\nArchitecture: any\n' > debian/control \
            && dpkg-shlibdeps -O markise/usr/bin/markise markise/usr/bin/markise-cli | sed -n 's/^shlibs:Depends=//p')"
          rm -rf "$stage/deb/debian"
          [[ -n "$shlibs" ]] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }
          # Libraries loaded with dlopen, which dpkg-shlibdeps cannot see (the sonames found in
          # the binary; the release build job prints them): X11 (libX11, libX11-xcb, libxcb,
          # Xcursor, Xi, Xrender, xkbcommon-x11) and Wayland (client, egl, xkbcommon) for winit,
          # EGL/GL for glutin and wgpu, fontconfig for system fonts, libdbus for the file dialogs
          # (rfd talks to xdg-desktop-portal through it). Vulkan (wgpu's first choice; glow over
          # OpenGL takes over without it) and the portal itself are recommended.
          depends="$shlibs, libx11-6, libx11-xcb1, libxcb1, libxcursor1, libxi6, libxrender1, libxkbcommon-x11-0, libxkbcommon0, libwayland-client0, libwayland-egl1, libegl1, libgl1, libfontconfig1, libdbus-1-3"
          installed_kb="$(du -sk "$pkg/usr" | cut -f1)"
          cat > "$pkg/DEBIAN/control" <<CONTROL
Package: markise
Version: $deb_version
Architecture: $deb_arch
Maintainer: $maintainer
Installed-Size: $installed_kb
Depends: $depends
Recommends: libvulkan1, xdg-desktop-portal
Section: editors
Priority: optional
Homepage: https://adjioev.github.io/markise-site/
Description: live-preview Markdown editor
 Markise shows the formatted page while you type; Markdown markers appear
 around the caret and the file stays plain Markdown. Tables, task lists, TeX
 math, Mermaid diagrams, highlighted code, Obsidian vaults with wikilinks,
 CriticMarkup comments, themes, HTML and PDF export.
 .
 Also contains markise-cli, a command-line tool for HTML export and Markdown
 round-trip checks.
CONTROL
          normalise_tree "$pkg" usr/bin/markise usr/bin/markise-cli
          chmod 755 "$pkg/DEBIAN"
          # file name with the label, not the Debian version: GitHub rewrites "~" in asset names
          archive="$out_dir/markise_${label}_${deb_arch}.deb"
          # xz, not dpkg's zstd default on Ubuntu: Debian 12's dpkg cannot read zstd members.
          # dpkg-deb sorts entries and takes the ar timestamps from SOURCE_DATE_EPOCH.
          SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" dpkg-deb --root-owner-group -Zxz -z9 --build "$pkg" "$archive" >/dev/null
          echo "$archive"
          ;;
        AppImage)
          [[ -n "$appimage_arch" ]] || { echo "no AppImage architecture for $platform" >&2; exit 2; }
          tools="$("$root/scripts/fetch-appimage-tools.sh")"
          appdir="$stage/AppDir"
          mkdir -p "$appdir/usr/bin"
          cp "$bin_dir/markise" "$bin_dir/markise-cli" "$appdir/usr/bin/"
          install_share "$appdir/usr"
          # appimagetool looks for (and validates) the AppStream file under its older name
          mv "$appdir/usr/share/metainfo/$app_id.metainfo.xml" "$appdir/usr/share/metainfo/$app_id.appdata.xml"
          install_docs "$appdir/usr/share/doc/markise"
          cp -R "$root/packaging/linux/appimage-runtime" "$appdir/usr/share/doc/markise/appimage-runtime"
          cp "$root/packaging/linux/$app_id.desktop" "$appdir/"
          cp "$root/assets/brand/png/markise-256.png" "$appdir/$app_id.png"
          ln -s "$app_id.png" "$appdir/.DirIcon"
          # AppRun: the app, or markise-cli when the AppImage is started through a link or copy
          # whose name starts with markise-cli (the runtime passes that name in ARGV0).
          cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
case "${ARGV0##*/}" in
  markise-cli*) exec "$here/usr/bin/markise-cli" "$@" ;;
esac
exec "$here/usr/bin/markise" "$@"
APPRUN
          normalise_tree "$appdir" AppRun usr/bin/markise usr/bin/markise-cli
          if [[ "${PACKAGE_STABLE_NAME:-0}" == 1 ]]; then
            archive="$out_dir/markise-$appimage_arch.AppImage"
          else
            archive="$out_dir/markise-$label-$appimage_arch.AppImage"
          fi
          rm -f "$archive"
          # --appimage-extract-and-run: no FUSE needed on the build host. appimagetool checks
          # the desktop entry (desktop-file-validate) and the AppStream file (appstreamcli)
          # when those tools are installed. No update information is embedded.
          ARCH="$appimage_arch" VERSION="$label" SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" \
            "$tools/appimagetool-x86_64.AppImage" --appimage-extract-and-run \
            --runtime-file "$tools/runtime-$appimage_arch" --comp zstd \
            "$appdir" "$archive" >&2
          chmod 755 "$archive"
          echo "$archive"
          ;;
        *)
          echo "unknown Linux format '$format' (LINUX_FORMATS: tar.gz deb AppImage)" >&2
          exit 2
          ;;
      esac
    done
    exit 0
    ;;
  windows-*)
    cp "$bin_dir/markise.exe" "$bin_dir/markise-cli.exe" "$dest/"
    normalise markise.exe markise-cli.exe
    archive="$out_dir/$archive_name.zip"
    make_zip "$archive"
    ;;
  *)
    echo "unknown platform: $platform" >&2
    exit 2
    ;;
esac
echo "$archive"
