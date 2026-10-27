//! Heading anchors (spec 01 §7, spec 03 §5).

use std::collections::HashMap;

use crate::text::is_punctuation;

/// Base slug of a heading's rendered text (R7.1).
pub fn slug(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for c in lower.trim().chars() {
        match c {
            '—' => out.push_str("---"),
            '–' => out.push_str("--"),
            '-' => out.push('-'),
            c if c.is_whitespace() => out.push('-'),
            '+' | '=' => {}
            c if is_punctuation(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Assigns unique anchors in document order (R7.2, R7.3).
#[derive(Debug, Default, Clone)]
pub struct SlugTracker {
    counts: HashMap<String, usize>,
}

impl SlugTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an anchor that exists independently of headings (e.g. `<a name="x">`).
    pub fn add_existing(&mut self, anchor: &str) {
        *self.counts.entry(anchor.to_lowercase()).or_default() += 1;
    }

    /// Unique anchor for a heading with the given rendered text.
    pub fn anchor_for(&mut self, heading_text: &str) -> String {
        let base = slug(heading_text);
        let n = self.counts.entry(base.to_lowercase()).or_default();
        let mut id = if *n > 0 {
            format!("{base}-{}", *n + 1)
        } else {
            base.clone()
        };
        *n += 1;
        // `write` is reserved for the document container (R7.3).
        if id == "write" {
            id = "write-1".to_string();
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t31_anchors() {
        let mut t = SlugTracker::new();
        assert_eq!(t.anchor_for("Intro"), "intro");
        assert_eq!(t.anchor_for("Intro"), "intro-2");
        assert_eq!(t.anchor_for("A — B"), "a-----b");
        assert_eq!(t.anchor_for("C++ & D"), "c--d");
        assert_eq!(t.anchor_for("Intro"), "intro-3");
    }

    #[test]
    fn t32_write_reserved() {
        let mut t = SlugTracker::new();
        assert_eq!(t.anchor_for("Write"), "write-1");
    }

    #[test]
    fn unicode_slug() {
        assert_eq!(slug("日本語の見出し！"), "日本語の見出し");
        assert_eq!(slug("a–b"), "a--b");
    }

    #[test]
    fn html_anchor_counts() {
        let mut t = SlugTracker::new();
        t.add_existing("intro");
        assert_eq!(t.anchor_for("Intro"), "intro-2");
    }
}
