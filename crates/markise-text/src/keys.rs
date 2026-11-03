//! The bundled keyboard-symbol fallback font (issue #131): "Markise Keys", a 24-glyph subset of
//! Inter (SIL OFL 1.1; provenance in `assets/fonts/keys/SOURCE.md`). It covers the macOS key
//! glyphs the tutorial, the menus and shortcut hints use (⌘ ⇧ ⌥ ⌃ ⎋ ↩ ⌫ ⌦ ⇥ ← → ↑ ↓ ⇞ ⇟ …),
//! which egui's bundled fonts and most body fonts lack. Front ends append it to their fallback
//! chains so these glyphs never render as boxes on any platform.

/// Family name of the bundled face (its `name` table).
pub const FAMILY: &str = "Markise Keys";

/// The font file (about 4 KB).
pub const FONT: &[u8] = include_bytes!("../../../assets/fonts/keys/MarkiseKeys-Regular.ttf");

/// The keyboard symbols the bundled face is guaranteed to cover.
pub const SYMBOLS: &[char] = &[
    '⌘', '⇧', '⌥', '⌃', '⎋', '↩', '⌫', '⌦', '⇥', '⇤', '←', '→', '↑', '↓', '⇞', '⇟', '⏎', '⇪', '⏏', '␣', '↖', '↘', '⌅',
    '⌧',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_face_parses_and_covers_every_key_symbol() {
        assert!(crate::fallback::covers(FONT, 0, SYMBOLS));
        assert!(crate::fallback::Profile::read(FONT, 0).is_some());
        assert!(!crate::fallback::covers(FONT, 0, &['a']), "symbols only");
    }
}
