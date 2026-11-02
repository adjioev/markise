# Diagnostics: renderer, scrolling and frame pacing

Environment switches of the desktop app (`markise`) for checking rendering and scroll
smoothness on a real display. None of them is needed in normal use; all are read once at start.

## Renderer

Markise renders with **wgpu** (Metal on macOS; DX12, Vulkan or OpenGL ES on Windows and Linux),
presenting with `AutoVsync` and a maximum frame latency of one frame, on the low-power GPU. If
wgpu cannot start (no usable adapter or surface), the app starts again with **glow** (OpenGL)
in the same process. The diagnostics log (`markise.log`, folder per platform: see
`apps/markise/src/diag.rs`) records which one started, for example
`renderer: wgpu Metal (Apple M1 Max, IntegratedGpu)`, or the reason for a fallback.

| Variable | Effect |
|---|---|
| `MARKISE_RENDERER=glow` | use glow (OpenGL) and skip wgpu (`wgpu`: the default) |
| `MARKISE_PRESENT=fifo\|mailbox\|immediate\|autovsync\|autonovsync` | wgpu present mode (default `autovsync`) |
| `MARKISE_FRAME_LATENCY=<n>` | wgpu maximum frame latency (default 1) |
| `WGPU_BACKEND`, `WGPU_POWER_PREF`, `WGPU_ADAPTER_NAME` | wgpu's own adapter selection; e.g. `WGPU_BACKEND=vulkan` on macOS finds no adapter and shows the glow fallback |

## Scrolling (issue #86)

The document panes (rendered view, source editor, both panes of Side by Side) scroll as follows
(`crates/markise-ui-egui/src/smooth_scroll.rs`):

- touchpad and Magic Mouse deltas (points, including macOS momentum) are applied as delivered,
  spread over about one frame (8 ms half-life);
- mouse wheel notches (lines or pages; one line unit is three lines of the pane's text) drive a
  critically damped spring, ~90 % of a notch in ~120 ms; a notch adds to the distance still to
  go and keeps the velocity;
- while something moves, and for 150 ms after the last wheel event, a frame is drawn every
  display refresh; then nothing is scheduled (an idle window stays silent);
- the panes paint at the scroll offset rounded to device pixels, with item origins on device
  pixels, so lines never shift against each other by a pixel while scrolling.

The sidebar, Quick Open, dialogs and menus keep egui's default scrolling (short lists).

| Variable | Effect |
|---|---|
| `MARKISE_SCROLL_SMOOTH=egui` | egui's own smoothed wheel delta instead (comparison only) |
| `MARKISE_SCROLL_SNAP=0` | no pixel snap |

## Frame log

`MARKISE_FRAME_LOG=1` writes one CSV line per frame to `markise-frames-<pid>.csv` in the
temporary folder (`$TMPDIR`, `/tmp`, `%TEMP%`); `MARKISE_FRAME_LOG=<path>` writes to that file.
The path is printed to stderr. The first line is a header comment with the renderer, present
mode, frame latency, the display's maximum refresh rate (macOS) and the scale factor. Columns:

| Column | Meaning |
|---|---|
| `frame`, `t_ms` | frame number, start time since the log began |
| `interval_ms` | time since the previous frame started |
| `ui_wall_ms`, `ui_cpu_ms` | wall and thread CPU time of the app's UI code this frame (CPU: macOS/Linux) |
| `prev_frame_cpu_ms` | eframe's CPU time of the previous whole frame (including painting) |
| `predicted_dt_ms` | egui's predicted frame interval |
| `input_events` | input events of the frame |
| `wheel_events`, `momentum_events`, `phases` | wheel events (of which macOS momentum), their phases in order (`S`tart, `M`ove, `E`nd, `C`ancel) |
| `delta_raw_pt` | delivered wheel delta (points, y; lines converted) |
| `delta_egui_smooth_pt` | egui's smoothed delta of the frame (for comparison) |
| `delta_applied_pt` | delta the panes applied |
| `pending_pt` | delivered but not yet applied after the frame |
| `scroll_y`, `total_h` | rendered pane's scroll offset and document height |
| `repaint_requested` | another frame was requested |
| `input_driven` | the frame had input events |

Steady scrolling on a 120 Hz display shows `interval_ms` near 8.3 while `delta_applied_pt` is
non-zero, and no frames with `delta_applied_pt` 0 between moving frames.

## Start-up timing

`MARKISE_TIMING=1` prints a timing log to stderr (window and renderer ready, first frame,
document open); `MARKISE_TIMING_EXIT=<secs>` quits after the first frame plus that many idle
seconds and reports the frames drawn while idle. `scripts/smoke-start.sh` uses both.
