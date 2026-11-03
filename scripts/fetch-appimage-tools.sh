#!/usr/bin/env bash
# Download the pinned AppImage build tools and print the folder that holds them.
#
# usage: scripts/fetch-appimage-tools.sh
#
# Files (x86_64 only), each checked against its SHA-256 before use (also when cached):
#   appimagetool-x86_64.AppImage  https://github.com/AppImage/appimagetool release 1.9.1 (MIT).
#                                 A build tool: it is run, not shipped.
#   runtime-x86_64                https://github.com/AppImage/type2-runtime release 20251108
#                                 (MIT; statically links libfuse LGPL-2.1, squashfuse, zstd, zlib,
#                                 musl). It becomes the first part of every AppImage, so it IS
#                                 shipped: licences in packaging/linux/appimage-runtime/.
#                                 Passed to appimagetool with --runtime-file, otherwise
#                                 appimagetool would download the floating "continuous" runtime.
#
# Cache folder: $MARKISE_TOOLS_CACHE, else ${XDG_CACHE_HOME:-~/.cache}/markise-tools. Downloads go
# to a temporary name and are moved into place only after the checksum matches.
set -euo pipefail

cache="${MARKISE_TOOLS_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/markise-tools}"
mkdir -p "$cache"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

fetch() {
  local name="$1" url="$2" want="$3" got
  local file="$cache/$name"
  if [[ -f "$file" ]] && [[ "$(sha256 "$file")" == "$want" ]]; then
    return
  fi
  echo "fetch-appimage-tools: downloading $url" >&2
  curl -fsSL --retry 3 --proto '=https' --tlsv1.2 -o "$file.part" "$url"
  got="$(sha256 "$file.part")"
  if [[ "$got" != "$want" ]]; then
    rm -f "$file.part"
    echo "fetch-appimage-tools: SHA-256 mismatch for $name: got $got, want $want" >&2
    exit 1
  fi
  chmod 755 "$file.part"
  mv -f "$file.part" "$file"
}

fetch appimagetool-x86_64.AppImage \
  https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
  ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
fetch runtime-x86_64 \
  https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
  2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

echo "$cache"
