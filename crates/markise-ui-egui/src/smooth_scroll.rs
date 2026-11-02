//! Smooth scrolling of the document panes (issue #86): the rendered document, the source
//! editor and both panes of Side by Side.
//!
//! - **Precise deltas** (touchpads, Magic Mouse: wheel events in points, including macOS
//!   momentum) are applied as delivered, but spread over about one frame: each frame applies the
//!   delivered-but-not-yet-applied distance with an 8 ms half-life ([`Drain`]). Events do not
//!   arrive one per frame (none in one frame, two in the next; frames drawn for other reasons get
//!   none), so applying them in the frame they arrive stutters, while egui's own smoothing (90 %
//!   in 100 ms) lags behind the fingers.
//! - **Discrete deltas** (mouse wheel notches: lines or pages) drive a critically damped spring
//!   ([`Spring`], ~90 % of a notch in ~120 ms). A notch adds to the distance still to go and
//!   keeps the current velocity, so notches ~50 ms apart blend into one continuous motion.
//! - **Frame pacing**: while a gesture, its momentum or a notch is moving the pane, and for
//!   [`ACTIVE_TAIL`] after the last wheel event, the pane asks for a frame every display refresh
//!   (the renderer presents with vsync). Afterwards nothing is scheduled: an idle window stays
//!   silent (NFR-10).
//! - **Pixel snap**: the panes paint at the scroll offset rounded to device pixels and round item
//!   origins ([`offset_shift`], [`snap_top`]), so every line moves by whole pixels and
//!   neighbouring lines never cross a pixel boundary on different frames (1 px relative wobble).
//!
//! In both cases the total distance is exactly the delivered one; only its timing changes.
//!
//! - **Ownership of the wheel events**: before egui sees a frame's input, the app hands the wheel
//!   events over the pane the pointer is on to that pane ([`claim_wheel_events`], from eframe's
//!   `raw_input_hook`), so egui's own smoothing never queues them and no other widget (the
//!   sidebar under the pointer a moment later) scrolls with the same gesture.
//! - **Programmatic scrolls win**: when anything else moves a pane (outline or anchor jump, find,
//!   caret reveal, keyboard, scrollbar, Side by Side sync), its pending motion is dropped
//!   ([`SmoothScroll::follow`]), so it does not drift away from the requested position.
//!
//! Other scroll areas (sidebar, Quick Open, dialogs, menus) keep egui's `ScrollArea` behaviour:
//! they are short lists where the difference does not show.
//!
//! Diagnostics (environment, read once): `MARKISE_SCROLL_SMOOTH=egui` uses egui's smoothed delta
//! instead (comparison only); `MARKISE_SCROLL_SNAP=0` turns the pixel snap off. The frame log
//! (`MARKISE_FRAME_LOG`, see `docs/diagnostics.md`) reads [`take_frame_diag`].

use egui::{Context, Event, Id, MouseWheelSource, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, Ui, Vec2, vec2};

/// Half-life of the precise-delta drain (seconds): about one frame at 120 Hz.
pub const PRECISE_HALF_LIFE: f32 = 0.008;
/// Natural frequency of the wheel spring (1/s): critically damped, ~90 % of a notch in ~120 ms.
pub const WHEEL_OMEGA: f32 = 33.0;
/// A pane keeps drawing a frame per display refresh this long after its last wheel event
/// (seconds), so sparse events still get evenly paced frames.
pub const ACTIVE_TAIL: f64 = 0.150;
/// Lines of the pane's text per wheel line unit (a notch), the usual desktop default.
pub const LINES_PER_NOTCH: f32 = 3.0;
/// The drain applies what is left in one step once it is below this (points).
const DRAIN_SNAP: f64 = 0.5;
/// The spring is at rest when the distance left and the velocity are below these (points,
/// points per second); the rest is applied in that frame.
const SPRING_REST_DIST: f32 = 0.25;
const SPRING_REST_VEL: f32 = 5.0;
/// Integration sub-step of the spring (seconds), keeps it stable at long frames.
const SPRING_SUBSTEP: f32 = 0.004;
/// Frame time bounds used for the maths (seconds): a frame after an idle period counts as one
/// display refresh (egui's `stable_dt`), a stalled one as at most 50 ms.
const DT_MIN: f32 = 0.001;
const DT_MAX: f32 = 0.05;

/// How a wheel event's delta is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelKind {
    /// Points: touchpads, Magic Mouse, macOS momentum ([`Drain`]).
    Precise,
    /// Lines or pages: mouse wheel notches ([`Spring`]).
    Discrete,
}

/// Precise for point deltas, discrete for line and page deltas (winit reports a notched
/// wheel as `LineDelta`, a touchpad as `PixelDelta`).
pub fn classify(unit: MouseWheelUnit) -> WheelKind {
    match unit {
        MouseWheelUnit::Point => WheelKind::Precise,
        MouseWheelUnit::Line | MouseWheelUnit::Page => WheelKind::Discrete,
    }
}

/// The wheel events of one frame, in points (positive y: the content moves down, as egui's
/// scroll deltas).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WheelInput {
    pub precise: Vec2,
    pub discrete: Vec2,
    /// Wheel events counted (zoom events with the command modifier are not).
    pub events: u32,
    /// Of which macOS momentum events.
    pub momentum: u32,
    /// Phases in order (`S`tart / `M`ove / `E`nd / `C`ancel), only while diagnostics are on.
    pub phases: String,
}

impl WheelInput {
    /// Collects the wheel events of a frame. `line`: points per line unit; `page`: points per
    /// page unit. Command + wheel is zoom and ignored; Shift makes the scroll horizontal and
    /// Alt vertical, as egui does.
    pub fn collect<'a>(events: impl IntoIterator<Item = &'a Event>, line: f32, page: f32) -> Self {
        let diag = diag_enabled();
        let mut w = WheelInput::default();
        for e in events {
            let Event::MouseWheel {
                unit,
                delta,
                phase,
                source,
                modifiers,
            } = e
            else {
                continue;
            };
            if modifiers.command {
                continue;
            }
            w.events += 1;
            if matches!(source, MouseWheelSource::Momentum) {
                w.momentum += 1;
            }
            if diag {
                w.phases.push(match phase {
                    TouchPhase::Start => 'S',
                    TouchPhase::Move => 'M',
                    TouchPhase::End => 'E',
                    TouchPhase::Cancel => 'C',
                });
            }
            let mut d = match unit {
                MouseWheelUnit::Point => *delta,
                MouseWheelUnit::Line => *delta * line,
                MouseWheelUnit::Page => *delta * page,
            };
            if modifiers.shift && !modifiers.alt {
                // (macOS already sends horizontal deltas with Shift down)
                d = vec2(d.x + d.y, 0.0);
            } else if modifiers.alt && !modifiers.shift {
                d = vec2(0.0, d.x + d.y);
            }
            match classify(*unit) {
                WheelKind::Precise => w.precise += d,
                WheelKind::Discrete => w.discrete += d,
            }
        }
        w
    }
}

/// Precise deltas: what was delivered and not applied yet, drained with a half-life of
/// [`PRECISE_HALF_LIFE`]. The applied total equals the delivered total.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Drain {
    pending: f64,
}

impl Drain {
    pub fn add(&mut self, d: f32) {
        self.pending += d as f64;
    }

    /// The distance to apply in a frame of `dt` seconds.
    pub fn step(&mut self, dt: f32) -> f32 {
        if self.pending == 0.0 {
            return 0.0;
        }
        let f = 1.0 - 0.5f64.powf((dt / PRECISE_HALF_LIFE) as f64);
        let a = if self.pending.abs() < DRAIN_SNAP {
            self.pending
        } else {
            self.pending * f
        };
        self.pending -= a;
        a as f32
    }

    pub fn pending(&self) -> f32 {
        self.pending as f32
    }

    pub fn is_idle(&self) -> bool {
        self.pending == 0.0
    }
}

/// Discrete deltas: a critically damped spring towards the distance still to go. A notch adds
/// to that distance and keeps the velocity. The applied total equals the delivered total.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    remaining: f32,
    vel: f32,
}

impl Spring {
    pub fn add(&mut self, d: f32) {
        self.remaining += d;
    }

    /// The distance to apply in a frame of `dt` seconds.
    pub fn step(&mut self, dt: f32) -> f32 {
        if self.is_idle() {
            return 0.0;
        }
        let n = (dt / SPRING_SUBSTEP).ceil().max(1.0);
        let h = dt / n;
        let mut moved = 0.0;
        for _ in 0..n as usize {
            let acc = WHEEL_OMEGA * WHEEL_OMEGA * self.remaining - 2.0 * WHEEL_OMEGA * self.vel;
            self.vel += acc * h;
            let s = self.vel * h;
            self.remaining -= s;
            moved += s;
        }
        if self.remaining.abs() < SPRING_REST_DIST && self.vel.abs() < SPRING_REST_VEL {
            moved += self.remaining;
            *self = Spring::default();
        }
        moved
    }

    pub fn remaining(&self) -> f32 {
        self.remaining
    }

    pub fn velocity(&self) -> f32 {
        self.vel
    }

    pub fn is_idle(&self) -> bool {
        self.remaining == 0.0 && self.vel == 0.0
    }
}

/// Smooth-scroll state of one pane (both axes).
#[derive(Clone, Debug)]
pub struct SmoothScroll {
    precise: [Drain; 2],
    wheel: [Spring; 2],
    /// egui time of the last wheel event this pane accepted.
    last_event: f64,
    /// The pane's scroll anchor as this state last left it ([`Self::follow`]).
    expected: Option<(usize, f32)>,
}

impl Default for SmoothScroll {
    fn default() -> Self {
        SmoothScroll {
            precise: Default::default(),
            wheel: Default::default(),
            last_event: f64::NEG_INFINITY,
            expected: None,
        }
    }
}

impl SmoothScroll {
    /// Advances one frame: `input` is taken only when `accept` (the pointer is over the pane);
    /// motion already under way continues either way. Returns the scroll delta to apply
    /// (egui's sign: positive y moves the content down).
    pub fn step(&mut self, input: &WheelInput, accept: bool, now: f64, dt: f32) -> Vec2 {
        let dt = dt.clamp(DT_MIN, DT_MAX);
        if accept && input.events > 0 {
            self.last_event = now;
            for k in 0..2 {
                self.precise[k].add(input.precise[k]);
                self.wheel[k].add(input.discrete[k]);
            }
        }
        let mut d = Vec2::ZERO;
        for k in 0..2 {
            d[k] = self.precise[k].step(dt) + self.wheel[k].step(dt);
        }
        d
    }

    /// Something is still moving, or the last wheel event is less than [`ACTIVE_TAIL`] ago:
    /// the pane wants a frame every display refresh.
    pub fn active(&self, now: f64) -> bool {
        !self.is_settled() || now - self.last_event < ACTIVE_TAIL
    }

    /// No delivered distance is left to apply.
    pub fn is_settled(&self) -> bool {
        self.precise.iter().all(Drain::is_idle) && self.wheel.iter().all(Spring::is_idle)
    }

    /// Delivered but not yet applied (points, y), for the frame log.
    pub fn pending_y(&self) -> f32 {
        self.precise[1].pending() + self.wheel[1].remaining()
    }

    /// Stops all motion (a jump elsewhere: scrollbar, outline, find).
    pub fn stop(&mut self) {
        *self = SmoothScroll {
            last_event: self.last_event,
            expected: self.expected,
            ..Default::default()
        };
    }

    /// Checks the pane's scroll anchor against where this state last left it: if something
    /// else moved the pane in between (a programmatic scroll), the pending motion is dropped so
    /// it does not drift away from the requested position. Then remembers `anchor`.
    ///
    /// Panes call it before applying their wheel delta (catches moves since the last frame),
    /// after applying it with `own = true` (the pane's own move), before laying out (catches
    /// moves later in the frame) and after laying out with `own = true` (layout normalises the
    /// anchor when estimated heights become real).
    pub fn follow(&mut self, anchor: (usize, f32), own: bool) {
        if !own && self.expected.is_some_and(|e| e != anchor) {
            self.stop();
        }
        self.expected = Some(anchor);
    }
}

/// The wheel/trackpad scroll delta for a pane this frame: reads the frame's wheel events,
/// advances `state`, consumes egui's smoothed delta while the pointer is over the pane (so no
/// other widget scrolls with it), and asks for the next frame while the motion is active.
/// `line`: points per wheel line unit; `page`: points per page unit.
pub fn take_delta(ui: &Ui, state: &mut SmoothScroll, hovered: bool, line: f32, page: f32) -> Vec2 {
    // (claimed events belong to the frame's first pass; a repeated pass gets no input events)
    let claimed = if ui.ctx().current_pass_index() == 0 {
        ui.ctx().data(|d| d.get_temp::<Claimed>(Id::unique(CLAIMED_ID)))
    } else {
        None
    };
    let (input, smooth, now, dt) = ui.input(|i| {
        let mine = claimed.as_ref().map_or(&[][..], |c| &c.events[..]);
        (
            WheelInput::collect(i.events.iter().chain(mine), line, page),
            i.smooth_scroll_delta,
            i.time,
            i.stable_dt,
        )
    });
    let d = if egui_smoothing() {
        if hovered { smooth } else { Vec2::ZERO }
    } else {
        let d = state.step(&input, hovered, now, dt);
        if state.active(now) {
            ui.ctx().request_repaint();
        }
        d
    };
    if hovered {
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    if diag_enabled() {
        let mut f = frame_diag();
        f.events += input.events;
        f.momentum += input.momentum;
        f.phases.push_str(&input.phases);
        f.raw += input.precise.y + input.discrete.y;
        f.smooth += smooth.y;
        f.applied += d.y;
        f.pending += state.pending_y();
    }
    d
}

// ---- wheel event ownership

const CLAIMED_ID: &str = "markise-smooth-scroll-claimed";
const PANES_ID: &str = "markise-smooth-scroll-panes";

/// Wheel events of this frame taken from egui's input for the panes.
#[derive(Clone, Default)]
struct Claimed {
    events: std::sync::Arc<Vec<Event>>,
}

/// Screen rectangles of the panes the pointer was over (honouring layers) in the frame being
/// built, and in the frame before.
#[derive(Clone, Default)]
struct Panes {
    current: Vec<Rect>,
    last: Vec<Rect>,
}

/// A pane notes that the pointer is over it (egui `hovered`, so not under a popup or dialog):
/// the next frame's wheel events at that spot are claimed for the panes.
pub fn note_hovered_pane(ctx: &Context, rect: Rect) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Panes>(Id::unique(PANES_ID))
            .current
            .push(rect)
    });
}

/// Before egui processes `raw` (eframe's `raw_input_hook`): when the pointer is over a pane that
/// was hovered in the last frame, its wheel events (not Command + wheel: zoom) are taken out of
/// `raw` and kept for the panes ([`take_delta`]), so egui neither smooths them nor hands them
/// to another widget. Elsewhere the input is left alone.
pub fn claim_wheel_events(ctx: &Context, raw: &mut RawInput) {
    let panes = ctx.data_mut(|d| {
        let p = d.get_temp_mut_or_default::<Panes>(Id::unique(PANES_ID));
        p.last = std::mem::take(&mut p.current);
        p.last.clone()
    });
    let mut pointer: Option<Pos2> = ctx.input(|i| i.pointer.latest_pos());
    for e in &raw.events {
        match e {
            Event::PointerMoved(p) => pointer = Some(*p),
            Event::PointerGone => pointer = None,
            _ => {}
        }
    }
    let over = pointer.is_some_and(|p| panes.iter().any(|r| r.contains(p)));
    let mut claimed = Vec::new();
    if over && !egui_smoothing() {
        raw.events.retain(|e| {
            let take = matches!(e, Event::MouseWheel { modifiers, .. } if !modifiers.command);
            if take {
                claimed.push(e.clone());
            }
            !take
        });
    }
    ctx.data_mut(|d| {
        d.insert_temp(
            Id::unique(CLAIMED_ID),
            Claimed {
                events: std::sync::Arc::new(claimed),
            },
        )
    });
}

/// `MARKISE_SCROLL_SMOOTH=egui`: egui's smoothed delta instead (diagnostic comparison).
fn egui_smoothing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MARKISE_SCROLL_SMOOTH").is_ok_and(|v| v == "egui"))
}

// ---- pixel snap

/// Pixel snap of the scroll offset and item origins (on unless `MARKISE_SCROLL_SNAP=0`).
pub fn snap_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MARKISE_SCROLL_SNAP").map_or(true, |v| v != "0"))
}

/// `v` (points) rounded to the nearest device pixel.
pub fn round_to_pixel(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

/// How far the scroll offset is past its nearest device pixel (points): adding it to an item
/// top relative to the viewport renders at the snapped offset.
pub fn offset_shift(scroll: f64, ppp: f32) -> f32 {
    let ppp = ppp as f64;
    (scroll - (scroll * ppp).round() / ppp) as f32
}

/// An item top relative to the surface, rendered at the snapped offset (`shift` from
/// [`offset_shift`]) with its screen origin on a device pixel.
pub fn snap_top(y: f32, shift: f32, surface_top: f32, ppp: f32) -> f32 {
    round_to_pixel(surface_top + y + shift, ppp) - surface_top
}

/// The snap correction of the item containing `y` (relative to the surface, unsnapped):
/// `tops` are (unsnapped, snapped) item tops in order. Things positioned inside an item (the
/// caret, the current-line band) move with their item.
pub fn shift_at(tops: &[(f32, f32)], y: f32) -> f32 {
    let i = tops.partition_point(|t| t.0 <= y);
    match i {
        0 => tops.first().map_or(0.0, |t| t.1 - t.0),
        _ => tops[i - 1].1 - tops[i - 1].0,
    }
}

// ---- diagnostics (frame log)

/// Scroll input of the last frame, summed over the panes (`MARKISE_FRAME_LOG`).
#[derive(Clone, Debug, Default)]
pub struct FrameDiag {
    pub events: u32,
    pub momentum: u32,
    pub phases: String,
    /// Delivered, egui-smoothed and applied deltas (points, y).
    pub raw: f32,
    pub smooth: f32,
    pub applied: f32,
    /// Delivered but not yet applied after the frame (points, y).
    pub pending: f32,
    /// Scroll offset and document height of the rendered pane after the frame.
    pub scroll_y: f64,
    pub total: f64,
}

static DIAG_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FRAME_DIAG: std::sync::Mutex<FrameDiag> = std::sync::Mutex::new(FrameDiag {
    events: 0,
    momentum: 0,
    phases: String::new(),
    raw: 0.0,
    smooth: 0.0,
    applied: 0.0,
    pending: 0.0,
    scroll_y: 0.0,
    total: 0.0,
});

/// Turns on collecting [`FrameDiag`] (the frame log does at its start).
pub fn enable_diag() {
    DIAG_ON.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn diag_enabled() -> bool {
    DIAG_ON.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn frame_diag() -> std::sync::MutexGuard<'static, FrameDiag> {
    FRAME_DIAG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Takes the scroll input of the last frame.
pub fn take_frame_diag() -> FrameDiag {
    std::mem::take(&mut *frame_diag())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    const FRAME_120: f32 = 1.0 / 120.0;

    fn wheel(unit: MouseWheelUnit, dy: f32, modifiers: Modifiers) -> Event {
        Event::MouseWheel {
            unit,
            delta: vec2(0.0, dy),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers,
        }
    }

    #[test]
    fn point_deltas_are_precise_lines_and_pages_discrete() {
        assert_eq!(classify(MouseWheelUnit::Point), WheelKind::Precise);
        assert_eq!(classify(MouseWheelUnit::Line), WheelKind::Discrete);
        assert_eq!(classify(MouseWheelUnit::Page), WheelKind::Discrete);
        let events = [
            wheel(MouseWheelUnit::Point, 3.0, Modifiers::NONE),
            wheel(MouseWheelUnit::Line, -1.0, Modifiers::NONE),
            wheel(MouseWheelUnit::Page, 0.5, Modifiers::NONE),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: vec2(0.0, 7.0),
                phase: TouchPhase::End,
                source: MouseWheelSource::Momentum,
                modifiers: Modifiers::NONE,
            },
            // zoom, not scroll
            wheel(MouseWheelUnit::Line, 5.0, Modifiers::COMMAND),
            Event::Text("x".into()),
        ];
        let w = WheelInput::collect(&events, 20.0, 600.0);
        assert_eq!(w.precise, vec2(0.0, 10.0));
        assert_eq!(w.discrete, vec2(0.0, -20.0 + 300.0));
        assert_eq!((w.events, w.momentum), (4, 1));
    }

    #[test]
    fn shift_scrolls_horizontally() {
        let w = WheelInput::collect(&[wheel(MouseWheelUnit::Line, -1.0, Modifiers::SHIFT)], 20.0, 600.0);
        assert_eq!(w.discrete, vec2(-20.0, 0.0));
    }

    /// Runs `frames` frames of `dt`; `deliver(frame)` is the precise delta delivered in it.
    fn run(s: &mut SmoothScroll, frames: usize, dt: f32, deliver: impl Fn(usize) -> (f32, f32)) -> Vec<f32> {
        (0..frames)
            .map(|f| {
                let (p, w) = deliver(f);
                let input = WheelInput {
                    precise: vec2(0.0, p),
                    discrete: vec2(0.0, w),
                    events: (p != 0.0 || w != 0.0) as u32,
                    ..Default::default()
                };
                s.step(&input, true, f as f64 * dt as f64, dt).y
            })
            .collect()
    }

    #[test]
    fn drain_applies_the_exact_total_within_a_few_frames() {
        let mut d = Drain::default();
        d.add(123.4);
        let mut sum = 0.0f64;
        let mut frames = 0;
        while !d.is_idle() {
            sum += d.step(FRAME_120) as f64;
            frames += 1;
            assert!(frames < 20, "drain never settles");
        }
        assert!((sum - 123.4f32 as f64).abs() < 1e-4, "{sum}");
        assert!(frames <= 10, "settled in {frames} frames");
        // about half the distance in one frame of 8 ms
        let mut d = Drain::default();
        d.add(100.0);
        let first = d.step(PRECISE_HALF_LIFE);
        assert!((first - 50.0).abs() < 0.01, "{first}");
    }

    #[test]
    fn clustered_precise_events_never_stall_a_frame() {
        // 10 pt per event at an average of one event per 120 Hz frame, delivered unevenly:
        // none, two, one, none, three, ...
        let pattern = [0usize, 2, 1, 0, 3, 0, 1, 2, 0, 1];
        let mut s = SmoothScroll::default();
        let applied = run(&mut s, 200, FRAME_120, |f| {
            if f < 100 {
                (pattern[f % pattern.len()] as f32 * -10.0, 0.0)
            } else {
                (0.0, 0.0)
            }
        });
        let delivered: f32 = (0..100).map(|f| pattern[f % pattern.len()] as f32 * -10.0).sum();
        let total: f32 = applied.iter().sum();
        assert!((total - delivered).abs() < 1e-2, "{total} vs {delivered}");
        // after the first event every frame of the gesture moves
        let first = applied.iter().position(|a| *a != 0.0).unwrap();
        let stalls = applied[first..100].iter().filter(|a| **a == 0.0).count();
        assert_eq!(stalls, 0, "frames without movement during the gesture: {applied:?}");
        assert!(s.is_settled());
    }

    #[test]
    fn wheel_notch_reaches_ninety_percent_in_about_120_ms() {
        let mut sp = Spring::default();
        sp.add(-100.0);
        let mut moved = 0.0;
        let mut t = 0.0;
        while moved > -90.0 {
            moved += sp.step(FRAME_120);
            t += FRAME_120;
            assert!(t < 1.0);
        }
        assert!((0.09..=0.15).contains(&t), "90 % after {t} s");
        while !sp.is_idle() {
            moved += sp.step(FRAME_120);
        }
        assert!((moved + 100.0).abs() < 1e-3, "{moved}");
        // never overshoots: critically damped
        let mut sp = Spring::default();
        sp.add(50.0);
        let mut pos = 0.0;
        for _ in 0..200 {
            pos += sp.step(FRAME_120);
            assert!(pos <= 50.0 + 1e-3, "{pos}");
        }
    }

    #[test]
    fn wheel_notches_blend_into_one_motion() {
        // notches every 50 ms (6 frames at 120 Hz), as a mouse wheel turned steadily
        let mut s = SmoothScroll::default();
        let applied = run(&mut s, 120, FRAME_120, |f| {
            (0.0, if f < 60 && f % 6 == 0 { -60.0 } else { 0.0 })
        });
        let total: f32 = applied.iter().sum();
        assert!((total + 600.0).abs() < 1e-2, "{total}");
        // every frame from the first notch until well after the last one moves, in one direction
        let moving = &applied[0..66];
        assert!(moving.iter().all(|a| *a < 0.0), "{moving:?}");
        // the velocity carries over: the frame after the second notch moves more than the frame
        // after a notch from rest (the motion does not restart)
        let mut one = SmoothScroll::default();
        let single = run(&mut one, 12, FRAME_120, |f| (0.0, if f == 0 { -60.0 } else { 0.0 }));
        assert!(applied[7].abs() > single[1].abs(), "{} vs {}", applied[7], single[1]);
        assert!(s.is_settled());
    }

    #[test]
    fn motion_continues_off_the_pane_but_new_input_is_ignored() {
        let mut s = SmoothScroll::default();
        let input = WheelInput {
            precise: vec2(0.0, 40.0),
            events: 1,
            ..Default::default()
        };
        let first = s.step(&input, true, 0.0, FRAME_120).y;
        let mut total = first;
        for f in 1..20 {
            total += s.step(&input, false, f as f64 * FRAME_120 as f64, FRAME_120).y;
        }
        assert!((total - 40.0).abs() < 1e-3, "{total}");
    }

    #[test]
    fn active_while_moving_and_for_the_tail_then_idle() {
        let mut s = SmoothScroll::default();
        assert!(!s.active(0.0), "a fresh pane is idle");
        let input = WheelInput {
            precise: vec2(0.0, 0.4),
            events: 1,
            ..Default::default()
        };
        s.step(&input, true, 10.0, FRAME_120);
        assert!(s.is_settled(), "a delta under half a point is applied at once");
        assert!(s.active(10.1), "tail after the last event");
        assert!(!s.active(10.0 + ACTIVE_TAIL + 0.001), "silent after the tail");
        s.stop();
        assert!(s.is_settled());
    }

    #[test]
    fn a_programmatic_scroll_drops_pending_motion() {
        let notch = WheelInput {
            discrete: vec2(0.0, -60.0),
            events: 1,
            ..Default::default()
        };
        let mut s = SmoothScroll::default();
        s.follow((3, 10.0), false);
        s.step(&notch, true, 0.0, FRAME_120);
        // the pane's own move and the layout's normalisation keep the motion
        s.follow((3, 14.0), true);
        s.follow((3, 14.0), false);
        s.follow((4, 0.5), true);
        assert!(!s.is_settled());
        // anything else moving the pane (a jump) stops it
        s.follow((40, 0.0), false);
        assert!(s.is_settled(), "pending motion dropped");
        assert_eq!(s.step(&WheelInput::default(), true, 0.01, FRAME_120), Vec2::ZERO);
        // new input after the jump moves again
        s.step(&notch, true, 0.02, FRAME_120);
        assert!(!s.is_settled());
    }

    /// Runs frames of a document pane (left half, smooth scrolling) next to another scrollable
    /// widget (right half, reads egui's smoothed delta) with `claim` as the app's input hook.
    /// Returns the distance the pane and the other widget scrolled.
    fn pane_and_other(claim: bool) -> (f32, f32) {
        let ctx = Context::default();
        let pane = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 600.0));
        let other = Rect::from_min_max(egui::pos2(400.0, 0.0), egui::pos2(800.0, 600.0));
        let mut state = SmoothScroll::default();
        let (mut pane_d, mut other_d) = (0.0, 0.0);
        let notch = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        };
        for f in 0..60 {
            let events = match f {
                0 => vec![Event::PointerMoved(egui::pos2(100.0, 100.0))],
                // (egui hit-tests against the widgets of the previous pass)
                3 => vec![notch.clone()],
                // right after the notch the pointer moves on to the other widget
                5 => vec![Event::PointerMoved(egui::pos2(600.0, 100.0))],
                _ => vec![],
            };
            let mut raw = RawInput {
                screen_rect: Some(Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(800.0, 600.0))),
                time: Some(f as f64 / 120.0),
                predicted_dt: FRAME_120,
                events,
                ..Default::default()
            };
            if claim {
                claim_wheel_events(&ctx, &mut raw);
            }
            let mut out = ctx.run_ui(raw, |ui| {
                let resp = ui.interact(pane, Id::unique("pane"), egui::Sense::hover());
                if resp.hovered() {
                    note_hovered_pane(ui.ctx(), pane);
                }
                pane_d += take_delta(ui, &mut state, resp.hovered(), 60.0, 600.0).y;
                if ui.rect_contains_pointer(other) {
                    other_d += ui.input(|i| i.smooth_scroll_delta.y);
                }
            });
            out.textures_delta.clear(); // headless: no renderer
        }
        (pane_d, other_d)
    }

    #[test]
    fn claimed_wheel_events_do_not_also_scroll_the_next_widget() {
        let (pane, other) = pane_and_other(true);
        assert!((pane + 60.0).abs() < 1e-3, "the pane got the whole notch: {pane}");
        assert_eq!(other, 0.0, "nothing of the notch reached the other widget");
        // without the hook egui keeps smoothing the notch and hands the rest to the widget
        // under the pointer (the bug this guards against)
        let (_, leaked) = pane_and_other(false);
        assert!(leaked < -1.0, "{leaked}");
    }

    #[test]
    fn snap_rounds_offset_and_tops_to_device_pixels() {
        assert_eq!(round_to_pixel(10.3, 2.0), 10.5);
        assert_eq!(round_to_pixel(10.2, 2.0), 10.0);
        assert_eq!(round_to_pixel(10.2, 1.0), 10.0);
        let shift = offset_shift(100.3, 2.0);
        assert!((shift - (-0.2)).abs() < 1e-4, "{shift}");
        assert!(offset_shift(100.5, 2.0).abs() < 1e-6);
        // an item at document y 50.0 with the offset 100.3, surface at screen y 33.25: rendered
        // as at offset 100.5 and on a device pixel
        let y = 50.0 - 100.3f32;
        let t = snap_top(y, shift, 33.25, 2.0);
        let screen = 33.25 + t;
        assert_eq!((screen * 2.0).fract(), 0.0, "{screen}");
        assert!((t - (50.0 - 100.5)).abs() <= 0.25 + 1e-4, "{t}");
        // while the offset moves by sub-pixel amounts, two items keep their distance
        for k in 0..40 {
            let s = 100.0 + k as f64 * 0.07;
            let sh = offset_shift(s, 2.0);
            let a = snap_top((10.0 - s) as f32, sh, 0.0, 2.0);
            let b = snap_top((37.3 - s) as f32, sh, 0.0, 2.0);
            assert_eq!(b - a, 27.5, "offset {s}");
        }
    }

    #[test]
    fn shift_at_picks_the_containing_item() {
        let tops = [(-5.2, -5.0), (20.1, 20.0), (44.9, 45.0)];
        assert!((shift_at(&tops, 0.0) - 0.2).abs() < 1e-5);
        assert!((shift_at(&tops, 30.0) + 0.1).abs() < 1e-5);
        assert!((shift_at(&tops, 50.0) - 0.1).abs() < 1e-5);
        assert!((shift_at(&tops, -10.0) - 0.2).abs() < 1e-5);
        assert_eq!(shift_at(&[], 3.0), 0.0);
    }
}
