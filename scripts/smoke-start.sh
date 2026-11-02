#!/usr/bin/env bash
# Start the desktop app on a document, let it quit by itself after a short idle period
# (MARKISE_TIMING_EXIT) and print the renderer it started with, read from its diagnostics log.
# Fails when the app does not quit in time, exits with an error, or logged no renderer.
# CI runs it on every OS of the full matrix (issue #86: wgpu starts, or glow takes over).
#
# usage: scripts/smoke-start.sh <markise binary>
#
# The log and settings folders point into a temporary folder. On Linux without a display the
# app runs under Xvfb (xvfb-run); Mesa's software Vulkan/GL drivers serve wgpu there.
set -euo pipefail

bin="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
state="$(mktemp -d)"
doc="$root/README.md"

export MARKISE_TIMING=1 MARKISE_TIMING_EXIT=1
case "$(uname -s)" in
  Darwin)
    export HOME="$state"
    log="$state/Library/Logs/Markise/markise.log"
    ;;
  MINGW* | MSYS* | CYGWIN*)
    LOCALAPPDATA="$(cygpath -w "$state")"
    export LOCALAPPDATA
    log="$state/Markise/logs/markise.log"
    doc="$(cygpath -w "$doc")"
    ;;
  *)
    export XDG_STATE_HOME="$state/state" XDG_CONFIG_HOME="$state/config"
    log="$state/state/markise/logs/markise.log"
    ;;
esac

run=("$bin" "$doc")
if [ "$(uname -s)" = Linux ] && [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
  run=(xvfb-run -a -s "-screen 0 1280x1024x24" "${run[@]}")
fi

# a watchdog instead of timeout(1), which macOS lacks
"${run[@]}" &
pid=$!
# (no inherited output: a sleeping watchdog would keep a pipe reading our output open)
(sleep 120 && kill "$pid") >/dev/null 2>&1 &
dog=$!
status=0
wait "$pid" || status=$?
kill "$dog" 2>/dev/null || true

echo "--- markise.log"
cat "$log" 2>/dev/null || echo "(no log at $log)"
echo "---"
if [ "$status" != 0 ]; then
  echo "markise exited with status $status (killed after 120 s if 143)" >&2
  exit 1
fi
# "renderer: wgpu Metal (…)" / "renderer: wgpu Vulkan (…)" / "renderer: glow" (after a fallback)
started='renderer: (glow$|wgpu [A-Z])'
if ! grep -Eq "$started" "$log" 2>/dev/null; then
  echo "markise logged no renderer that started" >&2
  exit 1
fi
grep -E "$started" "$log"
