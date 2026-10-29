//! Colours of review marks (comments and suggestions, ADR 0012), derived from a theme:
//! insertions green, deletions red, highlights yellow. The theme's alert accents (tip,
//! caution) and its `mark` background are used when they have the expected hue; otherwise
//! fixed fallbacks for light and dark themes.

use crate::{Rgba, Theme};

/// Review mark colours of a theme ([`Theme::review_colors`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReviewColors {
    /// Inserted text (and its underline).
    pub insert: Rgba,
    /// Deleted text (and its strike-through line).
    pub delete: Rgba,
    /// Background of highlighted passages.
    pub highlight_bg: Rgba,
    /// Background of a comment bubble.
    pub comment_bg: Rgba,
    /// Text of a comment bubble.
    pub comment_text: Rgba,
}

/// `true` when channel `i` (0 red, 1 green, 2 blue) clearly dominates the others.
fn dominant(c: Rgba, i: usize) -> bool {
    let v = c.0;
    let (x, o1, o2) = (v[i] as i32, v[(i + 1) % 3] as i32, v[(i + 2) % 3] as i32);
    v[3] > 0 && x > o1 + 24 && x > o2 + 24
}

/// Yellow-ish: red and green both clearly above blue.
fn yellowish(c: Rgba) -> bool {
    let [r, g, b, a] = c.0;
    a > 0 && (r as i32) > (b as i32) + 40 && (g as i32) > (b as i32) + 30
}

impl Theme {
    /// The review mark colours of this theme.
    pub fn review_colors(&self) -> ReviewColors {
        let tip = self.alert[1];
        let caution = self.alert[4];
        let insert = if dominant(tip, 1) {
            tip
        } else if self.dark {
            Rgba::hex(0x56d364)
        } else {
            Rgba::hex(0x1a7f37)
        };
        let delete = if dominant(caution, 0) {
            caution
        } else if self.dark {
            Rgba::hex(0xff7b72)
        } else {
            Rgba::hex(0xcf222e)
        };
        let highlight_bg = if yellowish(self.highlight_bg) {
            self.highlight_bg
        } else if self.dark {
            Rgba::hex(0x6a5a1c)
        } else {
            Rgba::hex(0xfbeb9b)
        };
        let comment_bg = highlight_bg.with_alpha(if self.dark { 110 } else { 140 });
        ReviewColors {
            insert,
            delete,
            highlight_bg,
            comment_bg,
            comment_text: self.text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_the_expected_hues() {
        for t in [Theme::light(), Theme::dark()] {
            let c = t.review_colors();
            assert!(dominant(c.insert, 1), "{}: insert {:?}", t.name, c.insert);
            assert!(dominant(c.delete, 0), "{}: delete {:?}", t.name, c.delete);
            assert!(yellowish(c.highlight_bg), "{}: highlight {:?}", t.name, c.highlight_bg);
        }
    }

    #[test]
    fn odd_theme_colours_fall_back() {
        let mut t = Theme::light();
        t.alert = [Rgba::hex(0x808080); 5];
        t.highlight_bg = Rgba::hex(0x3050f0);
        let c = t.review_colors();
        assert_eq!(c.insert, Rgba::hex(0x1a7f37));
        assert_eq!(c.delete, Rgba::hex(0xcf222e));
        assert_eq!(c.highlight_bg, Rgba::hex(0xfbeb9b));
        t.dark = true;
        assert_eq!(t.review_colors().insert, Rgba::hex(0x56d364));
    }
}
