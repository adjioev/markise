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
#
# Output (prints the archive path on stdout), each with README.md, LICENSE-MIT, LICENSE-APACHE,
# NOTICE, THIRD_PARTY/ (full license texts of bundled third-party data, e.g. syntax
# definitions) and BUILD-INFO.txt (commit, toolchain, runner image):
#   macOS    markise-<label>-<platform>.dmg: Markise.app (icon, .md/.markdown document types,
#            licences also in Contents/Resources), an /Applications link, markise-cli
#            (or .zip with MACOS_FORMAT=zip)
#   Windows  markise-<label>-<platform>.zip: markise.exe (icon embedded), markise-cli.exe
#   Linux    markise-<label>-<platform>.tar.gz: markise, markise-cli and share/ (desktop entry
#            and hicolor icons; `cp -r share ~/.local/` installs them for the current user)
#
# Reproducibility: entries are sorted (C locale), all timestamps are SOURCE_DATE_EPOCH,
# permissions are normalised (755 executables/dirs, 644 files), tar records owner/group 0 and
# gzip stores no name/time; zip entries carry no extra attributes. Remaining limits: the
# binaries themselves are only as reproducible as the toolchain and runner image (recorded in
# BUILD-INFO.txt; GitHub's macOS/Windows images float), Windows archives are made with 7-Zip
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
    cp "$bin_dir/markise" "$bin_dir/markise-cli" "$dest/"
    app_id=io.github.adjioev.markise
    mkdir -p "$dest/share/applications"
    cp "$root/packaging/linux/$app_id.desktop" "$dest/share/applications/"
    for size in 16 32 48 64 128 256 512; do
      mkdir -p "$dest/share/icons/hicolor/${size}x${size}/apps"
      cp "$root/assets/brand/png/markise-$size.png" "$dest/share/icons/hicolor/${size}x${size}/apps/$app_id.png"
    done
    normalise markise markise-cli
    archive="$out_dir/$archive_name.tar.gz"
    tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
      --format=gnu -cf - "$name" | gzip -n -9 > "$archive"
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
