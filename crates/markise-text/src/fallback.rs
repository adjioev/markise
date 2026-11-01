//! Font coverage and metric matching for display-only run fallback (issue #92).
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
    raw::TableProvider,
};

#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub x_height: Option<f32>,
    pub weight: u16,
    pub serif: Option<bool>,
}
impl Profile {
    pub fn read(data: &[u8], index: u32) -> Option<Self> {
        let f = FontRef::from_index(data, index).ok()?;
        let m = f.metrics(Size::unscaled(), LocationRef::default());
        let upem = m.units_per_em as f32;
        let x_height = m
            .x_height
            .filter(|h| *h > 0.0)
            .or_else(|| {
                let glyph = f.charmap().map('x')?;
                f.glyph_metrics(Size::unscaled(), LocationRef::default())
                    .bounds(glyph)
                    .map(|b| b.y_max - b.y_min)
            })
            .map(|h| h / upem)
            .filter(|h| h.is_finite() && (0.1..=1.5).contains(h));
        let os2 = f.os2().ok();
        let serif = os2.as_ref().and_then(|o| {
            let class = (o.s_family_class() as u16 >> 8) as u8;
            match class {
                1..=7 => Some(true),
                8 => Some(false),
                _ => {
                    let p = o.panose_10();
                    if p.first() == Some(&2) {
                        match p.get(1) {
                            Some(2..=10) => Some(true),
                            Some(11..=15) => Some(false),
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
            }
        });
        Some(Self {
            x_height,
            weight: os2.map_or(400, |o| o.us_weight_class()),
            serif,
        })
    }
    /// Match the primary face's x-height without changing the configured line box.
    pub fn scale_to(self, primary: Self) -> f32 {
        match (primary.x_height, self.x_height) {
            (Some(a), Some(b)) => (a / b).clamp(0.75, 1.35),
            _ => 1.0,
        }
    }
    pub fn distance(self, primary: Self, bold: bool) -> u32 {
        let style = match (primary.serif, self.serif) {
            (Some(a), Some(b)) if a != b => 10_000,
            _ => 0,
        };
        let weight = self.weight.abs_diff(if bold { 700 } else { primary.weight }) as u32 * 4;
        let metric = match (primary.x_height, self.x_height) {
            (Some(a), Some(b)) => ((a - b).abs() * 1000.0) as u32,
            _ => 100,
        };
        style + weight + metric
    }
}

/// Letters, digits and combining marks determine the run face; punctuation and emoji retain
/// the normal fallback chain. Combining marks must stay with their grapheme.
pub fn required_chars(text: &str) -> Vec<char> {
    let mut chars: Vec<_> = text
        .chars()
        .filter(|c| c.is_alphanumeric() || unicode_normalization::char::is_combining_mark(*c))
        .collect();
    chars.sort_unstable();
    chars.dedup();
    chars
}
pub fn covers(data: &[u8], index: u32, chars: &[char]) -> bool {
    let Ok(f) = FontRef::from_index(data, index) else {
        return false;
    };
    let map = f.charmap();
    chars.iter().all(|&c| map.map(c).is_some_and(|g| g.to_u32() != 0))
}

/// Script-specific user choices supported by Appearance. Shared punctuation has no script.
pub fn script(c: char) -> Option<&'static str> {
    match c as u32 {
        0x0400..=0x052f | 0x1c80..=0x1c8f | 0x2de0..=0x2dff | 0xa640..=0xa69f => Some("cyrillic"),
        0x0370..=0x03ff | 0x1f00..=0x1fff => Some("greek"),
        0x2e80..=0x2fff
        | 0x3040..=0x30ff
        | 0x3100..=0x31ff
        | 0x3400..=0x9fff
        | 0xac00..=0xd7af
        | 0xf900..=0xfaff
        | 0x20000..=0x323af => Some("cjk"),
        _ => None,
    }
}
