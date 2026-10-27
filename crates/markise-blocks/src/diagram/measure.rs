//! Text measurement for diagram layout (ADR 0009 §4).
//!
//! Layout needs label sizes before anything is painted. The front end plugs in a measurer
//! backed by its own text stack (so boxes fit the glyphs it will draw); export and tests use
//! [`ApproxMeasure`], a font-independent estimate from per-character width classes.

/// Measures single lines of label text.
pub trait TextMeasure: Send + Sync {
    /// Advance width of `line` (no line breaks) at font size `size` in points.
    fn width(&self, line: &str, size: f32, bold: bool) -> f32;

    /// Line height at font size `size`.
    fn line_height(&self, size: f32) -> f32 {
        size * 1.3
    }

    /// Identifies the metrics (font stack) in cache keys; two measurers with the same id must
    /// return the same widths.
    fn id(&self) -> u64;
}

/// Font-independent width estimate for proportional sans-serif text.
///
/// Widths are em fractions by character class, calibrated against common UI sans fonts
/// (Helvetica/Arial/Segoe-like metrics); wide (CJK, emoji) clusters count as 1 em.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApproxMeasure;

impl ApproxMeasure {
    fn em(c: char) -> f32 {
        match c {
            'i' | 'j' | 'l' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' | 'I' => 0.28,
            'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '{' | '}' | '/' | '\\' | '"' | '-' | ' ' => 0.36,
            'm' | 'w' | 'M' | 'W' | '@' | '%' => 0.86,
            'A'..='Z' => 0.68,
            '0'..='9' | '$' | '_' | '+' | '=' | '<' | '>' | '~' | '^' | '*' => 0.56,
            'a'..='z' => 0.53,
            c if is_wide(c) => 1.0,
            c if c.is_control() || is_combining(c) => 0.0,
            _ => 0.6,
        }
    }
}

impl TextMeasure for ApproxMeasure {
    fn width(&self, line: &str, size: f32, bold: bool) -> f32 {
        let w: f32 = line.chars().map(Self::em).sum::<f32>() * size;
        if bold { w * 1.06 } else { w }
    }

    fn id(&self) -> u64 {
        0x4150_5052_4f58 // "APPROX"
    }
}

/// Per-character advances measured with the front end's fonts (em units, printable ASCII,
/// regular and bold); other characters use the average lowercase advance, wide (CJK, emoji)
/// clusters 1 em. Built once per font/theme change on the UI thread from a few hundred shaped
/// glyphs, then shared with the worker, so diagram boxes fit the faces actually painted.
#[derive(Clone, Debug, PartialEq)]
pub struct TableMeasure {
    regular: Box<[f32]>,
    bold: Box<[f32]>,
    other_em: f32,
    line_height: f32,
    id: u64,
}

impl TableMeasure {
    /// `advance(c, bold)` returns the advance of `c` in em units (finite, ≥ 0);
    /// `line_height` is the line height in em.
    pub fn from_fn(mut advance: impl FnMut(char, bool) -> f32, line_height: f32) -> Self {
        let mut get = |c: char, bold: bool| {
            let v = advance(c, bold);
            if v.is_finite() { v.clamp(0.0, 4.0) } else { 0.6 }
        };
        let regular: Box<[f32]> = (32u8..127).map(|b| get(b as char, false)).collect();
        let bold: Box<[f32]> = (32u8..127).map(|b| get(b as char, true)).collect();
        let lower = &regular[(b'a' - 32) as usize..=(b'z' - 32) as usize];
        let other_em = (lower.iter().sum::<f32>() / lower.len() as f32 * 1.15).max(0.3);
        let line_height = if line_height.is_finite() {
            line_height.clamp(1.0, 3.0)
        } else {
            1.3
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in regular.iter().chain(bold.iter()) {
            std::hash::Hash::hash(&v.to_bits(), &mut h);
        }
        std::hash::Hash::hash(&line_height.to_bits(), &mut h);
        let id = std::hash::Hasher::finish(&h);
        TableMeasure {
            regular,
            bold,
            other_em,
            line_height,
            id,
        }
    }
}

impl TextMeasure for TableMeasure {
    fn width(&self, line: &str, size: f32, bold: bool) -> f32 {
        let t = if bold { &self.bold } else { &self.regular };
        let em: f32 = line
            .chars()
            .map(|c| match c as u32 {
                32..=126 => t[(c as u32 - 32) as usize],
                _ if is_wide(c) => 1.0,
                _ if c.is_control() || is_combining(c) => 0.0,
                _ => self.other_em,
            })
            .sum();
        em * size
    }

    fn line_height(&self, size: f32) -> f32 {
        size * self.line_height
    }

    fn id(&self) -> u64 {
        self.id
    }
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F | 0x20D0..=0x20FF)
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD)
}

/// Size of a (possibly multi-line) label: `(width, height)`.
pub fn label_size(m: &dyn TextMeasure, lines: &[String], size: f32, bold: bool) -> (f32, f32) {
    let w = lines.iter().map(|l| m.width(l, size, bold)).fold(0.0, f32::max);
    (w, m.line_height(size) * lines.len().max(1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_measure_uses_the_measured_advances() {
        // a monospace font: every ASCII glyph 0.6 em
        let mono = TableMeasure::from_fn(|_, _| 0.6, 1.2);
        assert!((mono.width("iiii", 10.0, false) - 24.0).abs() < 1e-3);
        assert!((mono.width("WWWW", 10.0, true) - 24.0).abs() < 1e-3);
        assert!((mono.line_height(10.0) - 12.0).abs() < 1e-3);
        let other = TableMeasure::from_fn(|c, _| if c == 'i' { 0.25 } else { 0.5 }, 1.2);
        assert_ne!(mono.id(), other.id());
        assert!(other.width("ii", 10.0, false) < other.width("mm", 10.0, false));
        // non-finite advances are tamed
        let bad = TableMeasure::from_fn(|_, _| f32::NAN, f32::INFINITY);
        assert!(bad.width("abc", 10.0, false).is_finite());
    }

    #[test]
    fn approx_widths_are_monotonic_and_finite() {
        let m = ApproxMeasure;
        assert!(m.width("WWW", 14.0, false) > m.width("iii", 14.0, false));
        assert!(m.width("abc", 28.0, false) > m.width("abc", 14.0, false));
        assert_eq!(m.width("", 14.0, false), 0.0);
        assert!(m.width("漢字", 10.0, false) >= 19.0);
    }
}
