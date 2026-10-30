//! Where review-comment cards go (ADR 0012, issue #91): margin cards beside the content column
//! when the visible right margin holds one, else bubbles whose popover stays on screen.
//!
//! Everything here is plain geometry in screen points, so it is unit-tested without a UI.

use egui::{Pos2, Rect, pos2};

/// Gap between the content column's right edge and a margin card (the connector runs in it).
pub const CARD_GAP: f32 = 20.0;
/// Space kept free right of a margin card, before the visible area's right edge.
pub const CARD_RIGHT_PAD: f32 = 16.0;
/// Widest margin card.
pub const CARD_MAX_W: f32 = 240.0;
/// Narrowest margin card: below it comments fall back to bubbles.
pub const CARD_MIN_W: f32 = 154.0;
/// Narrowest right margin that still holds comment cards.
pub const MIN_MARGIN: f32 = CARD_GAP + CARD_MIN_W + CARD_RIGHT_PAD;
/// Widest bubble popover, and the narrowest one while the visible area allows it.
const POPOVER_MAX_W: f32 = 240.0;
const POPOVER_MIN_W: f32 = 120.0;
/// Distance a popover keeps from the visible area's edges.
const EDGE: f32 = 4.0;
/// Width of the comment gutter reserved at the right of the document surface in margin mode
/// while the document has comments (owner decision on #91; points, so it scales with zoom).
pub const GUTTER_W: f32 = 260.0;
/// Narrowest content area left of the gutter: a narrower window shows bubbles instead.
pub const MIN_READABLE: f32 = 420.0;

/// Whether to reserve the comment gutter for a content area `content_w` wide (the surface
/// without its scrollbar) when the document `has_comments`, in margin mode (`margin_mode`).
pub fn reserve_gutter(margin_mode: bool, has_comments: bool, content_w: f32) -> bool {
    margin_mode && has_comments && content_w - GUTTER_W >= MIN_READABLE
}

/// Whether a gutter change may be applied this frame: only on a frame without pointer presses,
/// releases or a held button, so a click is never hit-tested against geometry of another width
/// (the previous frame's column with this frame's layout); otherwise it waits a frame.
pub fn gutter_change_allowed(pointer_busy: bool) -> bool {
    !pointer_busy
}

/// The part of the document surface that is actually on screen: the surface rectangle (`rect`)
/// without its scrollbar strip (`scrollbar_w`), cut to the UI's clip rectangle and to the
/// window's content rectangle. A surface laid out wider than the window (or than its parent's
/// clip) must not get cards in the part nobody sees.
pub fn visible_area(rect: Rect, scrollbar_w: f32, clip: Rect, window: Rect) -> Rect {
    let mut r = rect.intersect(clip).intersect(window);
    r.max.x = r.max.x.min(rect.right() - scrollbar_w);
    r
}

/// The margin card column, `(x, width)`, for a content column ending at `col_right` (its
/// right edge after max-width, padding and centring; pass the rightmost painted comment glyph
/// too, if it is further right), inside the visible area `vis`. `None` when the margin between
/// them is narrower than [`MIN_MARGIN`] — a left-aligned or full-width column has ~0 margin —
/// and the comments are shown as bubbles.
pub fn margin_column(vis: Rect, col_right: f32) -> Option<(f32, f32)> {
    let margin = vis.right() - col_right;
    if !margin.is_finite() || margin < MIN_MARGIN || vis.width() <= 0.0 {
        return None;
    }
    let w = (margin - CARD_GAP - CARD_RIGHT_PAD).min(CARD_MAX_W);
    Some((col_right + CARD_GAP, w))
}

/// Width for a bubble popover in the visible area `vis`.
pub fn popover_width(vis: Rect) -> f32 {
    let room = (vis.width() - 2.0 * EDGE).max(1.0);
    POPOVER_MAX_W.min(vis.width() - 24.0).max(POPOVER_MIN_W).min(room)
}

/// Top-left corner of a popover of `size` for the bubble `glyph`, kept inside `vis`: below the
/// glyph starting at its left, flipped left (ending at the glyph's right) near the right edge,
/// flipped up (above the glyph) near the bottom; clamped into `vis` when neither side fits.
pub fn popover_pos(glyph: Rect, size: egui::Vec2, vis: Rect) -> Pos2 {
    let (lo_x, hi_x) = (vis.left() + EDGE, vis.right() - EDGE - size.x);
    let mut x = glyph.left() - 8.0;
    if x > hi_x {
        // flip left: the popover ends at the glyph's right edge
        x = glyph.right() + 8.0 - size.x;
    }
    let x = if hi_x < lo_x { lo_x } else { x.clamp(lo_x, hi_x) };
    let (lo_y, hi_y) = (vis.top() + EDGE, vis.bottom() - EDGE - size.y);
    let below = glyph.bottom() + 4.0;
    let above = glyph.top() - 4.0 - size.y;
    let y = if below <= hi_y {
        below
    } else if above >= lo_y {
        above
    } else if hi_y < lo_y {
        lo_y
    } else {
        // neither side fits whole: the side with more room, clamped
        let room_below = vis.bottom() - glyph.bottom();
        let room_above = glyph.top() - vis.top();
        (if room_below >= room_above { below } else { above }).clamp(lo_y, hi_y)
    };
    pos2(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    #[test]
    fn gutter_is_reserved_only_with_comments_in_margin_mode_and_room() {
        assert!(reserve_gutter(true, true, 1200.0));
        assert!(!reserve_gutter(false, true, 1200.0), "bubbles mode");
        assert!(!reserve_gutter(true, false, 1200.0), "no comments");
        assert!(reserve_gutter(true, true, GUTTER_W + MIN_READABLE));
        assert!(
            !reserve_gutter(true, true, GUTTER_W + MIN_READABLE - 1.0),
            "narrow window"
        );
    }

    #[test]
    fn visible_area_drops_the_scrollbar_and_what_is_off_screen() {
        let window = r(0.0, 0.0, 1000.0, 700.0);
        // a surface laid out wider than the window (issue #91)
        let v = visible_area(r(300.0, 50.0, 1400.0, 690.0), 10.0, window, window);
        assert_eq!(v, r(300.0, 50.0, 1000.0, 690.0));
        // inside the window: only the scrollbar strip goes
        let v = visible_area(r(300.0, 50.0, 1000.0, 690.0), 10.0, window, window);
        assert_eq!(v.right(), 990.0);
        // a narrower clip (a parent panel) wins
        let v = visible_area(r(300.0, 50.0, 1000.0, 690.0), 10.0, r(0.0, 0.0, 800.0, 700.0), window);
        assert_eq!(v.right(), 800.0);
    }

    #[test]
    fn margin_cards_need_room_right_of_the_column() {
        let vis = r(280.0, 80.0, 1890.0, 720.0);
        // centred 950 column in a wide window: cards fit, inside the visible area
        let (x, w) = margin_column(vis, 1500.0).unwrap();
        assert_eq!(x, 1520.0);
        assert!(x + w <= vis.right() - CARD_RIGHT_PAD + 0.01, "{x} {w}");
        assert!((CARD_MIN_W..=CARD_MAX_W).contains(&w));
        // exactly the minimum
        let (x, w) = margin_column(vis, vis.right() - MIN_MARGIN).unwrap();
        assert!((w - CARD_MIN_W).abs() < 0.01);
        assert!(x + w <= vis.right());
        // too narrow, a full-width / left-aligned column (~0 margin), or a column past the
        // visible edge: bubbles
        assert!(margin_column(vis, vis.right() - MIN_MARGIN + 1.0).is_none());
        assert!(margin_column(vis, vis.right() - 2.0).is_none());
        assert!(margin_column(vis, vis.right() + 300.0).is_none());
        assert!(
            margin_column(r(0.0, 0.0, 0.0, 0.0), -500.0).is_none(),
            "nothing visible"
        );
        assert!(margin_column(vis, f32::NAN).is_none());
    }

    #[test]
    fn popovers_flip_left_and_up_near_the_edges() {
        let vis = r(280.0, 80.0, 1000.0, 700.0);
        let size = vec2(240.0, 80.0);
        let inside = |p: Pos2| vis.contains_rect(Rect::from_min_size(p, size));
        // room everywhere: below the glyph, from its left
        let g = r(400.0, 200.0, 416.0, 218.0);
        assert_eq!(popover_pos(g, size, vis), pos2(392.0, 222.0));
        // near the right edge: flipped left, ending at the glyph
        let g = r(960.0, 200.0, 976.0, 218.0);
        let p = popover_pos(g, size, vis);
        assert!(inside(p), "{p:?}");
        assert!((p.x + size.x - 984.0).abs() < 0.01, "{p:?}");
        // near the bottom: above the glyph
        let g = r(400.0, 660.0, 416.0, 678.0);
        let p = popover_pos(g, size, vis);
        assert!(inside(p) && p.y + size.y <= g.top(), "{p:?}");
        // bottom-right corner: both
        let g = r(970.0, 670.0, 986.0, 688.0);
        let p = popover_pos(g, size, vis);
        assert!(inside(p) && p.y + size.y <= g.top() && p.x < g.left(), "{p:?}");
        // near the left edge (under the sidebar's edge): clamped right
        let g = r(282.0, 200.0, 298.0, 218.0);
        assert!(popover_pos(g, size, vis).x >= vis.left());
        // a short visible area: neither side fits whole; clamped inside
        let low = r(280.0, 80.0, 1000.0, 200.0);
        let g = r(400.0, 120.0, 416.0, 138.0);
        let p = popover_pos(g, vec2(240.0, 100.0), low);
        assert!(low.contains_rect(Rect::from_min_size(p, vec2(240.0, 100.0))), "{p:?}");
        // the width follows a narrow visible area
        assert!(popover_width(r(0.0, 0.0, 100.0, 100.0)) <= 92.0);
        assert_eq!(popover_width(vis), 240.0);
    }
}
