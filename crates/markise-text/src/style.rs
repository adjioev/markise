//! Text styles: GUI-free, CSS-shaped (no egui types; ADR 0005 §4.1 `style.rs`).

use std::sync::Arc;

/// A `font-family` entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// A named family: theme `@font-face` family or installed system family.
    Named(Arc<str>),
    Generic(Generic),
}

impl Family {
    pub fn named(name: &str) -> Self {
        Family::Named(name.into())
    }

    /// Parses one CSS family token (quotes stripped by the caller or here).
    pub fn parse(token: &str) -> Self {
        let t = token.trim().trim_matches(|c| c == '"' || c == '\'');
        match t.to_ascii_lowercase().as_str() {
            "serif" => Family::Generic(Generic::Serif),
            "sans-serif" => Family::Generic(Generic::SansSerif),
            "monospace" => Family::Generic(Generic::Monospace),
            "cursive" => Family::Generic(Generic::Cursive),
            "fantasy" => Family::Generic(Generic::Fantasy),
            "system-ui" => Family::Generic(Generic::SystemUi),
            "emoji" => Family::Generic(Generic::Emoji),
            "math" => Family::Generic(Generic::Math),
            _ => Family::Named(t.into()),
        }
    }

    /// Parses a CSS `font-family` list (`"A B", serif`).
    pub fn parse_list(css: &str) -> Vec<Family> {
        css.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Family::parse)
            .collect()
    }
}

/// CSS generic families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Generic {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
    Fantasy,
    SystemUi,
    Emoji,
    Math,
}

/// `font-style`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Slope {
    #[default]
    Normal,
    Italic,
    /// Oblique with an angle in degrees (CSS default 14).
    Oblique(f32),
}

/// One OpenType feature setting (`font-feature-settings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u16,
}

impl Feature {
    pub const fn new(tag: &[u8; 4], value: u16) -> Self {
        Feature { tag: *tag, value }
    }
}

/// Style of a text run. Sizes are in layout units (logical points; `scale` is applied at layout).
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub families: Vec<Family>,
    pub size: f32,
    /// CSS weight 1–1000.
    pub weight: f32,
    pub slope: Slope,
    /// CSS `font-stretch` percentage (100 = normal).
    pub stretch: f32,
    /// Premultiplied-agnostic RGBA, `0xRRGGBBAA`; opaque to this crate.
    pub color: u32,
    pub underline: bool,
    pub strikethrough: bool,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    /// Line height as a multiple of the font size; `None` = font metrics ("normal").
    pub line_height: Option<f32>,
    /// Feature settings applied after the defaults (e.g. `liga` 0 to disable ligatures, R10).
    pub features: Vec<Feature>,
    /// BCP 47 language tag (affects shaping and Han fallback).
    pub locale: Option<Arc<str>>,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            families: vec![Family::Generic(Generic::SansSerif)],
            size: 16.0,
            weight: 400.0,
            slope: Slope::Normal,
            stretch: 100.0,
            color: 0x0000_00FF,
            underline: false,
            strikethrough: false,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            line_height: None,
            features: Vec::new(),
            locale: None,
        }
    }
}

impl TextStyle {
    /// Style with a family list and size.
    pub fn new(families: Vec<Family>, size: f32) -> Self {
        TextStyle {
            families,
            size,
            ..TextStyle::default()
        }
    }

    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }

    pub fn slope(mut self, s: Slope) -> Self {
        self.slope = s;
        self
    }

    /// Disables standard and contextual ligatures (`liga`, `clig`, `calt` = 0): the
    /// "code configurable" part of R10.
    pub fn no_ligatures(mut self) -> Self {
        self.features.extend([
            Feature::new(b"liga", 0),
            Feature::new(b"clig", 0),
            Feature::new(b"calt", 0),
        ]);
        self
    }
}
