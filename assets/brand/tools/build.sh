#!/usr/bin/env bash
# Rebuild the Markise brand assets: SVGs (from tools/make_svgs.py), PNGs, macOS .icns, Windows .ico.
#
# Needs: rsvg-convert (librsvg, LGPL tool, not linked into Markise), ImageMagick `magick`
# (ICO packing), `iconutil` (macOS, for .icns). For regenerating the SVGs (lockup wordmark):
# python3 with fonttools + uharfbuzz and the Playfair Display variable fonts (OFL-1.1):
#   FONT_DIR=/path/to/fonts tools/build.sh --svg
# Without --svg only the rasters are rebuilt from the committed SVGs.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" == "--svg" ]]; then
  : "${FONT_DIR:?set FONT_DIR to a folder with PlayfairDisplay.ttf and Nunito.ttf (variable fonts)}"
  "${PYTHON:-python3}" tools/make_svgs.py "$FONT_DIR/PlayfairDisplay.ttf" "$FONT_DIR/Nunito.ttf"
fi

# The app icon is the dark variant. Sizes <= 32 px use the simplified favicon artwork.
src_for() { if (( $1 <= 32 )); then echo markise-favicon-dark.svg; else echo markise-icon-dark.svg; fi; }

mkdir -p png
for s in 16 32 48 64 128 256 512 1024; do
  rsvg-convert -w "$s" -h "$s" "$(src_for "$s")" -o "png/markise-$s.png"
done
for s in 16 32 256 512; do
  if (( s <= 32 )); then src=markise-favicon-light.svg; else src=markise-icon-light.svg; fi
  rsvg-convert -w "$s" -h "$s" "$src" -o "png/markise-light-$s.png"
done

# Windows .ico: 16, 32, 48, 64, 128, 256.
magick png/markise-16.png png/markise-32.png png/markise-48.png png/markise-64.png \
  png/markise-128.png png/markise-256.png markise.ico

# macOS .icns from the Big Sur-grid artwork (favicon artwork for the 16/32 px slots).
if command -v iconutil >/dev/null; then
  tmp="$(mktemp -d)"
  set_dir="$tmp/markise.iconset"
  mkdir -p "$set_dir"
  for s in 16 32 128 256 512; do
    for scale in 1 2; do
      px=$((s * scale))
      name="icon_${s}x${s}"; [[ $scale == 2 ]] && name="${name}@2x"
      if (( px <= 32 )); then src=markise-favicon-dark.svg; else src=markise-icon-dark-macos.svg; fi
      rsvg-convert -w "$px" -h "$px" "$src" -o "$set_dir/$name.png"
    done
  done
  iconutil -c icns "$set_dir" -o markise.icns
  rm -r "$set_dir"
  rmdir "$tmp"
else
  echo "iconutil not found (not macOS): markise.icns not rebuilt" >&2
fi
echo "brand assets rebuilt"
