#!/usr/bin/env bash
# The helper as it ships (ADR 0023): mounts a macOS disk image, checks the bundle's signature and
# that the helper is in it, and runs its self-test: a fixture block has to draw, and a page
# attempting everything forbidden has to reach nothing. No window shows: the helper orders none in.
#
# usage: scripts/smoke-web-helper.sh <markise .dmg>
set -euo pipefail

dmg="$1"
mount="$(mktemp -d)"
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount" "$dmg"
trap 'hdiutil detach -quiet "$mount" || hdiutil detach -quiet -force "$mount" || true; rmdir "$mount" 2>/dev/null || true' EXIT

# (not through the image's link to /Applications: that is someone's installed copy)
app="$mount/Markise.app"
helper="$app/Contents/MacOS/markise-web-helper"
[ -x "$helper" ] || { echo "no helper at $helper" >&2; exit 1; }
codesign --verify --deep --strict "$app"
codesign --verify --strict "$helper"
lipo -info "$helper"

out="$(mktemp)"
# a watchdog instead of timeout(1), which macOS lacks
"$helper" --self-test >"$out" 2>&1 &
pid=$!
(sleep 600 && kill "$pid") >/dev/null 2>&1 &
dog=$!
status=0
wait "$pid" || status=$?
kill "$dog" 2>/dev/null || true
cat "$out"
if [ "$status" != 0 ]; then
  echo "the packaged helper's self-test failed with status $status" >&2
  exit 1
fi
tail -n 1 "$out" | grep -qx ok
echo "the packaged helper draws, and reaches nothing it must not"
