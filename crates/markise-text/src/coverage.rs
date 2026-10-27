//! Cluster support checks for CSS Fonts 4 §5.3 cluster matching (ADR 0005 §4.1 step 4).

use skrifa::charmap::MapVariant;
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider, Tag};
use unicode_normalization::UnicodeNormalization;

/// Variation selectors (VS1–16, VS17–256).
pub fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// Format controls that fonts need not map for a cluster to count as supported (joiners,
/// bidi controls, word joiner, BOM). Shapers handle them without glyphs of their own.
pub fn is_ignorable_control(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// How strictly a variation-selector sequence is matched (rule 4a: first a face that maps the
/// sequence — or, for VS15/VS16, has the requested presentation — then, failing that in every
/// candidate, the base character alone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsMode {
    Strict,
    IgnoreSelectors,
}

/// A face's coverage view.
pub struct FaceCoverage<'a> {
    font: FontRef<'a>,
    color: bool,
}

impl<'a> FaceCoverage<'a> {
    pub fn new(data: &'a [u8], index: u32) -> Option<Self> {
        let font = FontRef::from_index(data, index).ok()?;
        let color = [b"COLR", b"sbix", b"CBDT"]
            .iter()
            .any(|t| font.data_for_tag(Tag::new(t)).is_some());
        Some(FaceCoverage { font, color })
    }

    /// Has colour glyph tables (emoji presentation).
    pub fn is_color(&self) -> bool {
        self.color
    }

    pub fn maps(&self, c: char) -> bool {
        self.font.charmap().map(c).is_some_and(|g| g.to_u32() != 0)
    }

    fn maps_all(&self, s: impl Iterator<Item = char>) -> bool {
        let cm = self.font.charmap();
        s.filter(|c| !is_ignorable_control(*c))
            .all(|c| cm.map(c).is_some_and(|g| g.to_u32() != 0))
    }

    /// Rule 4a: does the face support the whole cluster (incl. its NFC form and VS rules)?
    pub fn supports(&self, cluster: &str, mode: VsMode) -> bool {
        let has_vs = cluster.chars().any(is_variation_selector);
        if has_vs {
            let cm = self.font.charmap();
            let chars: Vec<char> = cluster.chars().collect();
            let mut ok = true;
            for (i, &c) in chars.iter().enumerate() {
                if is_variation_selector(c) {
                    if mode == VsMode::IgnoreSelectors {
                        continue;
                    }
                    let base = if i > 0 { chars[i - 1] } else { continue };
                    // cmap-14: a non-default mapping to a real glyph, or a default-UVS entry
                    // whose base character maps to a real glyph
                    let mapped = match cm.map_variant(base, c) {
                        Some(MapVariant::Variant(g)) => g.to_u32() != 0,
                        Some(MapVariant::UseDefault) => cm.map(base).is_some_and(|g| g.to_u32() != 0),
                        None => false,
                    };
                    let presentation = match c {
                        '\u{FE0F}' => self.color,
                        '\u{FE0E}' => !self.color,
                        _ => false,
                    };
                    if !(mapped || presentation) {
                        ok = false;
                        break;
                    }
                } else if !is_ignorable_control(c) && cm.map(c).is_none_or(|g| g.to_u32() == 0) {
                    ok = false;
                    break;
                }
            }
            return ok;
        }
        if self.maps_all(cluster.chars()) {
            return true;
        }
        let nfc: String = cluster.nfc().collect();
        nfc != cluster && self.maps_all(nfc.chars())
    }

    /// Rule 4b: byte length of the longest leading sequence of `cluster` (base plus following
    /// characters, canonical equivalents counted) that the face supports; 0 if not even the base.
    pub fn leading_support(&self, cluster: &str) -> usize {
        let ends: Vec<usize> = cluster.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
        for &e in ends.iter().rev() {
            if self.supports(&cluster[..e], VsMode::IgnoreSelectors) {
                return e;
            }
        }
        0
    }
}
