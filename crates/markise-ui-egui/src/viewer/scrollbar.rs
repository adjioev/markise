//! Scrollbar geometry and thumb dragging (issue #123).
//!
//! The thumb travels the bar height minus its own height. While it is dragged it follows the
//! pointer from the point where it was grabbed; a press outside the thumb centres the thumb
//! there. The bottom of the travel is the end of the document, whatever the document height
//! is by then (estimated heights refine while items are laid out during a drag).

/// Smallest thumb height in points.
const MIN_THUMB: f32 = 24.0;

/// Where the scrollbar asks the view to scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// Absolute scroll position of the viewport top.
    Y(f64),
    /// The end of the document.
    End,
}

/// Thumb height and its travel (bar height minus thumb height) for a viewport `vh` points
/// tall over a document `total` points tall.
pub(crate) fn thumb_geometry(vh: f32, total: f64) -> (f32, f32) {
    let h = ((vh as f64 / total.max(1.0)) * vh as f64)
        .max(MIN_THUMB as f64)
        .min(vh as f64) as f32;
    (h, (vh - h).max(0.0))
}

/// Thumb top relative to the bar top for scroll position `y`.
pub(crate) fn thumb_top(vh: f32, total: f64, y: f64) -> f32 {
    let (_, travel) = thumb_geometry(vh, total);
    let frac = (y / (total - vh as f64).max(1.0)).clamp(0.0, 1.0) as f32;
    frac * travel
}

/// State of a press on the scrollbar.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ThumbDrag {
    /// Where the thumb is held, in points below its top, from the press until the release.
    grab: Option<f32>,
}

impl ThumbDrag {
    /// The scroll target while the bar is pressed. `press` and `pointer` are relative to the
    /// bar top; `y` is the scroll position before this frame.
    pub(crate) fn target(&mut self, press: f32, pointer: f32, vh: f32, total: f64, y: f64) -> Target {
        let (h, travel) = thumb_geometry(vh, total);
        let grab = *self.grab.get_or_insert_with(|| {
            let top = thumb_top(vh, total, y);
            if (top..=top + h).contains(&press) {
                press - top
            } else {
                h * 0.5
            }
        });
        if travel <= 0.0 {
            return Target::Y(0.0);
        }
        let frac = ((pointer - grab) / travel).clamp(0.0, 1.0) as f64;
        if frac >= 1.0 {
            Target::End
        } else {
            Target::Y(frac * (total - vh as f64).max(0.0))
        }
    }

    /// The press ended.
    pub(crate) fn release(&mut self) {
        self.grab = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VH: f32 = 600.0;
    const TOTAL: f64 = 600_000.0;

    fn top_of(t: Target) -> f32 {
        match t {
            Target::Y(y) => thumb_top(VH, TOTAL, y),
            Target::End => thumb_geometry(VH, TOTAL).1,
        }
    }

    /// #123: the thumb keeps the point where it was grabbed under the pointer.
    #[test]
    fn grab_offset_is_preserved() {
        let y = (TOTAL - VH as f64) * 0.25;
        let top = thumb_top(VH, TOTAL, y);
        let mut d = ThumbDrag::default();
        // press 10 pt below the thumb top: nothing moves
        let press = top + 10.0;
        assert!((top_of(d.target(press, press, VH, TOTAL, y)) - top).abs() < 0.01);
        // drag 100 pt down: the thumb top moves 100 pt
        let t = d.target(press, press + 100.0, VH, TOTAL, y);
        assert!((top_of(t) - (top + 100.0)).abs() < 0.01, "{t:?}");
    }

    /// #123: a press outside the thumb centres the thumb at the pointer.
    #[test]
    fn press_outside_the_thumb_centres_it() {
        let mut d = ThumbDrag::default();
        let (h, _) = thumb_geometry(VH, TOTAL);
        let t = d.target(300.0, 300.0, VH, TOTAL, 0.0);
        assert!((top_of(t) + h * 0.5 - 300.0).abs() < 0.01, "{t:?}");
    }

    /// #123: the thumb at the bottom of the bar is the end of the document, also when it was
    /// grabbed at its top (the pointer stays a thumb height above the bar end).
    #[test]
    fn thumb_at_the_bottom_is_the_end() {
        let mut d = ThumbDrag::default();
        let (h, travel) = thumb_geometry(VH, TOTAL);
        let press = 1.0;
        assert_eq!(d.target(press, press, VH, TOTAL, 0.0), Target::Y(0.0));
        assert_eq!(d.target(press, travel + press, VH, TOTAL, 0.0), Target::End);
        assert_eq!(d.target(press, VH + 50.0, VH, TOTAL, 0.0), Target::End);
        // a fresh press at the very bottom of the bar
        d.release();
        assert_eq!(d.target(VH - 1.0, VH - 1.0, VH, TOTAL, 0.0), Target::End);
        assert!(h >= MIN_THUMB);
    }

    #[test]
    fn short_document_has_no_travel() {
        let mut d = ThumbDrag::default();
        assert_eq!(thumb_geometry(VH, 100.0), (VH, 0.0));
        assert_eq!(d.target(10.0, 300.0, VH, 100.0, 0.0), Target::Y(0.0));
    }
}
