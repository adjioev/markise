//! `markise-theme` — the native theme model (layer L2, spec `docs/spec/03-theme-contract.md` §2).
//!
//! A [`Theme`] is a flat set of *slots*: page and chrome colors, the content column, and one
//! style per content element (headings, paragraph, quote, code, table, …). The slot set follows
//! the element keys of spec 03 §2.2 and the well-known variables of §2.3 so that the CSS-subset
//! importer (milestone M3) can fill it; nothing here depends on a GUI toolkit.
//!
//! The two built-in themes ([`Theme::light`], [`Theme::dark`]) are original Markise designs.
//!
//! A theme is also the composition of a [`Style`] (typography and layout) and a
//! [`ColorScheme`] (every color), see [`style`] and ADR 0007.
//!
//! [`import`] turns a CSS theme written for the spec 03 DOM contract into a [`Theme`]
//! (spec 03 §2); [`discover`] lists the themes in a theme folder (spec 03 §1).

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

pub mod builtin;
pub mod catalog;
pub mod discover;
pub mod import;
pub mod install;
pub mod palettes;
pub mod style;
pub mod syntax;

pub use import::{ImportOptions, ImportReport, ImportResult, Issue, Location, import_css, import_theme_file};
pub use style::{ColorScheme, Overrides, Style, compose, compose_with};
pub use syntax::{TokenKind, TokenLook, TokenRun};

/// An sRGB color with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    /// `0xRRGGBB`, opaque.
    pub const fn hex(rgb: u32) -> Self {
        Rgba([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255])
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        let [r, g, b, _] = self.0;
        Rgba([r, g, b, a])
    }

    pub const TRANSPARENT: Rgba = Rgba([0, 0, 0, 0]);

    /// WCAG 2 relative luminance (alpha ignored).
    pub fn luminance(self) -> f64 {
        let lin = |c: u8| {
            let c = c as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let [r, g, b, _] = self.0;
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    /// WCAG 2 contrast ratio (1–21) between two opaque colors.
    pub fn contrast(self, other: Rgba) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Perceived brightness in `0..=1` using the luma rule of spec 03 §1.4.
    pub fn brightness(self) -> f32 {
        let [r, g, b, a] = self.0;
        if a == 0 {
            return 1.0; // fully transparent counts as white (spec 03 §1.4)
        }
        (299.0 * r as f32 + 587.0 * g as f32 + 114.0 * b as f32) / 256_000.0
    }
}

/// Font family slot. The UI maps these to concrete fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// Body text (proportional).
    Body,
    /// Code (monospace).
    Mono,
}

/// Style of one heading level (`#write h1` … `h6`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadingStyle {
    /// Font size in points.
    pub size: f32,
    pub color: Rgba,
    pub bold: bool,
    /// Space above / below in points.
    pub margin_top: f32,
    pub margin_bottom: f32,
    /// Rule under the heading (`border-bottom`), if any.
    pub rule: Option<Rgba>,
}

/// Box style for code blocks, math/diagram placeholders and front matter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxStyle {
    pub background: Rgba,
    pub text: Rgba,
    pub border: Option<Rgba>,
    pub padding: f32,
    pub radius: f32,
}

/// Syntax-highlighting token colors (spec 03 §4.5 token classes, reduced set); resolved per
/// token kind by [`Theme::token_look`](crate::syntax).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    pub keyword: Rgba,
    pub string: Rgba,
    pub comment: Rgba,
    pub number: Rgba,
    /// Function / definition names (`cm-def`).
    pub function: Rgba,
    /// Type names (`cm-type`, `cm-variable-3`).
    pub type_name: Rgba,
}

/// Window chrome colors (sidebar, selection; spec 03 §2.3 variables).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeColors {
    /// `--side-bar-bg-color`
    pub sidebar_bg: Rgba,
    /// `--control-text-color`
    pub control_text: Rgba,
    /// `--active-file-bg-color`
    pub active_bg: Rgba,
    /// `--select-text-bg-color`
    pub selection_bg: Rgba,
    /// `--primary-color`
    pub accent: Rgba,
}

/// Horizontal text alignment (`text-align`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

/// `text-transform`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

/// Heading styling beyond [`HeadingStyle`], filled by the CSS importer (defaults: none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeadingDecor {
    pub italic: bool,
    pub align: TextAlign,
    pub transform: TextTransform,
    /// `letter-spacing` in points.
    pub letter_spacing: f32,
    /// `::before` / `::after` generated text (spec 03 §2.1 `content`).
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    /// Prefix color, if different from the heading color.
    pub prefix_color: Option<Rgba>,
    /// Background fill behind the heading.
    pub background: Option<Rgba>,
    /// Padding of the heading box (`padding`): horizontal (left) and vertical (top), points.
    pub padding: [f32; 2],
    /// Corner radius of the heading background (`border-radius`).
    pub radius: f32,
    /// The heading box hugs its text (`display: inline-block`, `width: fit-content`, …)
    /// instead of spanning the column; affects the background only.
    pub fit: bool,
    /// Bar on the left (`border-left`): width in points and color.
    pub bar: Option<(f32, Rgba)>,
}

/// Where a `@font-face` source lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontSource {
    /// A local file relative to the theme (`exists` checked at import time).
    File {
        path: PathBuf,
        format: Option<String>,
        exists: bool,
    },
    /// `local("Name")`: an installed system font.
    Local(String),
    /// A remote URL (never fetched by the importer).
    Remote(String),
    /// A `data:` URL; only its size is recorded (loading it is a UI-layer TODO, C3).
    Embedded { format: Option<String>, bytes: usize },
}

/// One `@font-face` rule (spec 03 §1.5). Loading the faces into the UI's font system is the UI
/// layer's job (open question C3: register each face as a separate fallback in declaration
/// order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFace {
    pub family: String,
    pub sources: Vec<FontSource>,
    /// `font-weight` range (a single weight has `min == max`).
    pub weight: (u16, u16),
    pub italic: bool,
    /// Inclusive code point ranges; empty means all.
    pub unicode_range: Vec<(u32, u32)>,
    pub display: Option<String>,
}

/// Font stacks (CSS `font-family` lists, generic families kept as `serif`, `monospace`, …).
/// Empty means "UI default".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeFonts {
    pub body: Vec<String>,
    /// Heading stack when it differs from the body stack.
    pub heading: Vec<String>,
    pub mono: Vec<String>,
    pub faces: Vec<FontFace>,
}

/// Inline semantic colors (`strong`, `em`, `del`, `mark` text); `None` = body text color.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InlineColors {
    pub strong: Option<Rgba>,
    pub em: Option<Rgba>,
    pub del: Option<Rgba>,
    pub mark_text: Option<Rgba>,
    /// Markdown marker characters (`.md-meta`, `--md-char-color`).
    pub markers: Option<Rgba>,
}

/// Style of one CodeMirror token class (spec 03 §4.5), e.g. `cm-keyword`.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenStyle {
    pub class: String,
    pub color: Option<Rgba>,
    pub bold: bool,
    pub italic: bool,
}

/// List marker styles per nesting level (`list-style-type`); empty = UI default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListStyles {
    pub bullets: Vec<String>,
    pub numbers: Vec<String>,
}

/// Sidebar (file tree, outline, tabs) styling (spec 03 §3.2 chrome selectors). `None` means
/// "not styled by the theme": the UI uses its defaults derived from [`ChromeColors`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarStyle {
    /// Font stack of sidebar items; empty = the UI font.
    pub fonts: Vec<String>,
    /// Item font size in points.
    pub font_size: Option<f32>,
    pub bold: Option<bool>,
    /// Item text.
    pub text: Option<Rgba>,
    /// Active (open) file / current tab: text and background.
    pub active_text: Option<Rgba>,
    pub active_bg: Option<Rgba>,
    /// Hovered item.
    pub hover_text: Option<Rgba>,
    pub hover_bg: Option<Rgba>,
    /// Current outline heading.
    pub outline_active_text: Option<Rgba>,
    pub outline_active_bg: Option<Rgba>,
    /// Icon tint (file/folder icons, outline level markers).
    pub icon: Option<Rgba>,
    /// Row height in points (from `line-height`, `height` or padding).
    pub row_height: Option<f32>,
    /// Horizontal item padding in points.
    pub padding_x: Option<f32>,
    /// Indentation per nesting level in points.
    pub indent: Option<f32>,
    /// Corner radius of the active/hover background.
    pub radius: Option<f32>,
}

/// A complete native theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    /// Dark chrome (spec 03 §1.4).
    pub dark: bool,

    // ---- page and column (`body`, `#write`)
    /// `--bg-color`
    pub page_bg: Rgba,
    /// `--text-color`
    pub text: Rgba,
    /// Body font size in points (`html`/`#write` `font-size`).
    pub font_size: f32,
    /// Monospace size relative to the body size.
    pub mono_scale: f32,
    /// Unitless line height multiplier.
    pub line_height: f32,
    /// `#write max-width` in points.
    pub column_max_width: f32,
    /// `#write` horizontal padding in points.
    pub column_padding_x: f32,
    /// `#write` top/bottom padding in points.
    pub column_padding_y: f32,
    /// Vertical gap between blocks (`p` margin).
    pub block_spacing: f32,

    // ---- elements
    pub headings: [HeadingStyle; 6],
    /// `a`
    pub link: Rgba,
    /// `code` (inline)
    pub inline_code: BoxStyle,
    /// `.md-fences`
    pub code_block: BoxStyle,
    /// `.mathjax-block` / diagram placeholders (source shown, M1).
    pub math_block: BoxStyle,
    /// `pre.md-meta-block` (front matter)
    pub meta_block: BoxStyle,
    /// `blockquote` bar color, bar width, indent and text color.
    pub quote_bar: Rgba,
    pub quote_bar_width: f32,
    pub quote_indent: f32,
    pub quote_text: Rgba,
    /// `ul`/`ol` indent per level and marker color.
    pub list_indent: f32,
    pub list_marker: Rgba,
    /// `.md-task-list-item > input`
    pub checkbox_border: Rgba,
    pub checkbox_checked: Rgba,
    /// `hr`
    pub rule: Rgba,
    /// `table`, `th`, `tr:nth-child(2n)`
    pub table_border: Rgba,
    pub table_header_bg: Rgba,
    pub table_stripe_bg: Rgba,
    pub table_cell_padding: f32,
    /// `mark`
    pub highlight_bg: Rgba,
    /// `--meta-content-color`: de-emphasised text (HTML source, link definitions, captions).
    pub meta_text: Rgba,
    /// Alert (`.md-alert-*`) accent colors: note, tip, important, warning, caution.
    pub alert: [Rgba; 5],
    /// Image placeholder fill while loading / on error.
    pub image_placeholder: Rgba,

    pub syntax: SyntaxColors,
    pub chrome: ChromeColors,

    // ---- added for the CSS importer (M3); built-ins use the defaults below
    /// The theme follows the OS light/dark setting (`@media (prefers-color-scheme)`, spec 03
    /// §1.4); the importer then produces a light and a dark variant.
    pub scheme_aware: bool,
    pub fonts: ThemeFonts,
    pub heading_decor: [HeadingDecor; 6],
    /// Links are underlined (`text-decoration`).
    pub link_underline: bool,
    /// `blockquote` background, italic text and generated prefix (`::before`).
    pub quote_bg: Rgba,
    pub quote_italic: bool,
    pub quote_prefix: Option<String>,
    pub inline: InlineColors,
    pub list_styles: ListStyles,
    /// Colors of the CodeMirror token classes the theme styles, beyond [`SyntaxColors`].
    pub syntax_tokens: Vec<TokenStyle>,
    /// `::selection` text color.
    pub selection_text: Option<Rgba>,
    /// Paragraph `text-align`.
    pub text_align: TextAlign,
    /// Sidebar styling (file tree, outline, tabs).
    pub sidebar: SidebarStyle,
}

impl Theme {
    /// Built-in light theme ("Markise Light", original design).
    pub fn light() -> Theme {
        let ink = Rgba::hex(0x2a2b2e);
        let head = Rgba::hex(0x17181a);
        Theme {
            name: "Markise Light".into(),
            dark: false,
            page_bg: Rgba::hex(0xfcfcfa),
            text: ink,
            font_size: 16.0,
            mono_scale: 0.9,
            line_height: 1.6,
            column_max_width: 780.0,
            column_padding_x: 40.0,
            column_padding_y: 28.0,
            block_spacing: 12.0,
            headings: headings(head, Some(Rgba::hex(0xe4e1d8)), Rgba::hex(0x5d5f63)),
            link: Rgba::hex(0x1c66a8),
            inline_code: BoxStyle {
                background: Rgba::hex(0xefede7),
                text: Rgba::hex(0x8a3324),
                border: None,
                padding: 2.0,
                radius: 3.0,
            },
            code_block: BoxStyle {
                background: Rgba::hex(0xf4f3ef),
                text: Rgba::hex(0x2a2b2e),
                border: Some(Rgba::hex(0xe6e3db)),
                padding: 12.0,
                radius: 5.0,
            },
            math_block: BoxStyle {
                background: Rgba::hex(0xf3f5f9),
                text: Rgba::hex(0x30476b),
                border: Some(Rgba::hex(0xdfe5ef)),
                padding: 10.0,
                radius: 5.0,
            },
            meta_block: BoxStyle {
                background: Rgba::hex(0xf6f5f1),
                text: Rgba::hex(0x76726a),
                border: None,
                padding: 8.0,
                radius: 4.0,
            },
            quote_bar: Rgba::hex(0xd6d2c6),
            quote_bar_width: 3.0,
            quote_indent: 18.0,
            quote_text: Rgba::hex(0x5b5953),
            list_indent: 26.0,
            list_marker: Rgba::hex(0x6d6f73),
            checkbox_border: Rgba::hex(0x9a9c9f),
            checkbox_checked: Rgba::hex(0x2f7d5b),
            rule: Rgba::hex(0xdedbd2),
            table_border: Rgba::hex(0xdcd9d0),
            table_header_bg: Rgba::hex(0xf2f0ea),
            table_stripe_bg: Rgba::hex(0xf8f7f3),
            table_cell_padding: 7.0,
            highlight_bg: Rgba::hex(0xfbeb9b),
            meta_text: Rgba::hex(0x86827a),
            alert: [
                Rgba::hex(0x2f6fb5),
                Rgba::hex(0x2f8a4f),
                Rgba::hex(0x7a4fc0),
                Rgba::hex(0xb07a12),
                Rgba::hex(0xc0392b),
            ],
            image_placeholder: Rgba::hex(0xeceae4),
            syntax: SyntaxColors {
                keyword: Rgba::hex(0x8b3fa8),
                string: Rgba::hex(0x2d7a3a),
                comment: Rgba::hex(0x8d8a82),
                number: Rgba::hex(0xb35a13),
                function: Rgba::hex(0x1f5f99),
                type_name: Rgba::hex(0x8a5a00),
            },
            chrome: ChromeColors {
                sidebar_bg: Rgba::hex(0xf2f1ed),
                control_text: Rgba::hex(0x45474b),
                active_bg: Rgba::hex(0xe2e0d9),
                selection_bg: Rgba::hex(0xb9d6f2),
                accent: Rgba::hex(0x1c66a8),
            },
            scheme_aware: false,
            fonts: ThemeFonts::default(),
            heading_decor: Default::default(),
            link_underline: false,
            quote_bg: Rgba::TRANSPARENT,
            quote_italic: false,
            quote_prefix: None,
            inline: InlineColors::default(),
            list_styles: ListStyles::default(),
            syntax_tokens: Vec::new(),
            selection_text: None,
            text_align: TextAlign::Start,
            sidebar: SidebarStyle {
                text: Some(Rgba::hex(0x45474b)),
                active_text: Some(Rgba::hex(0x17181a)),
                active_bg: Some(Rgba::hex(0xe2e0d9)),
                hover_bg: Some(Rgba::hex(0xeae8e2)),
                outline_active_text: Some(Rgba::hex(0x1c66a8)),
                icon: Some(Rgba::hex(0x8a8c90)),
                row_height: Some(24.0),
                padding_x: Some(6.0),
                indent: Some(14.0),
                radius: Some(4.0),
                ..SidebarStyle::default()
            },
        }
    }

    /// Built-in dark theme ("Markise Dark", original design).
    pub fn dark() -> Theme {
        let ink = Rgba::hex(0xd6d6d0);
        let head = Rgba::hex(0xf1f1ea);
        Theme {
            name: "Markise Dark".into(),
            dark: true,
            page_bg: Rgba::hex(0x1d1e21),
            text: ink,
            headings: headings(head, Some(Rgba::hex(0x34363b)), Rgba::hex(0xa3a5a8)),
            link: Rgba::hex(0x72b3ea),
            inline_code: BoxStyle {
                background: Rgba::hex(0x2c2e33),
                text: Rgba::hex(0xe7a17a),
                border: None,
                padding: 2.0,
                radius: 3.0,
            },
            code_block: BoxStyle {
                background: Rgba::hex(0x25272b),
                text: Rgba::hex(0xd6d6d0),
                border: Some(Rgba::hex(0x31343a)),
                padding: 12.0,
                radius: 5.0,
            },
            math_block: BoxStyle {
                background: Rgba::hex(0x232832),
                text: Rgba::hex(0xa9c2e8),
                border: Some(Rgba::hex(0x2f3644)),
                padding: 10.0,
                radius: 5.0,
            },
            meta_block: BoxStyle {
                background: Rgba::hex(0x232427),
                text: Rgba::hex(0x8e8c86),
                border: None,
                padding: 8.0,
                radius: 4.0,
            },
            quote_bar: Rgba::hex(0x4a4d53),
            quote_text: Rgba::hex(0xa9a8a2),
            list_marker: Rgba::hex(0x9a9ca0),
            checkbox_border: Rgba::hex(0x7c7f84),
            checkbox_checked: Rgba::hex(0x58b98a),
            rule: Rgba::hex(0x393c41),
            table_border: Rgba::hex(0x3a3d43),
            table_header_bg: Rgba::hex(0x26282c),
            table_stripe_bg: Rgba::hex(0x212226),
            highlight_bg: Rgba::hex(0x6a5a1c),
            meta_text: Rgba::hex(0x8c8a84),
            alert: [
                Rgba::hex(0x5b9be0),
                Rgba::hex(0x55b877),
                Rgba::hex(0xa585e0),
                Rgba::hex(0xd9a63a),
                Rgba::hex(0xe56b5d),
            ],
            image_placeholder: Rgba::hex(0x2a2c30),
            syntax: SyntaxColors {
                keyword: Rgba::hex(0xc792ea),
                string: Rgba::hex(0x9ccc7c),
                comment: Rgba::hex(0x7f8187),
                number: Rgba::hex(0xf0a35e),
                function: Rgba::hex(0x82b8ea),
                type_name: Rgba::hex(0xe5c07b),
            },
            chrome: ChromeColors {
                sidebar_bg: Rgba::hex(0x18191b),
                control_text: Rgba::hex(0xbdbdb7),
                active_bg: Rgba::hex(0x2d2f34),
                selection_bg: Rgba::hex(0x2f4f72),
                accent: Rgba::hex(0x72b3ea),
            },
            sidebar: SidebarStyle {
                text: Some(Rgba::hex(0xbdbdb7)),
                active_text: Some(Rgba::hex(0xf1f1ea)),
                active_bg: Some(Rgba::hex(0x2d2f34)),
                hover_bg: Some(Rgba::hex(0x232528)),
                outline_active_text: Some(Rgba::hex(0x72b3ea)),
                icon: Some(Rgba::hex(0x8a8c90)),
                row_height: Some(24.0),
                padding_x: Some(6.0),
                indent: Some(14.0),
                radius: Some(4.0),
                ..SidebarStyle::default()
            },
            ..Theme::light()
        }
    }

    /// A native theme by id: `markise-light`, `markise-dark`, or a palette theme
    /// ([`palettes::PALETTE_THEMES`]).
    pub fn native(id: &str) -> Option<Theme> {
        match id {
            "markise-light" => Some(Theme::light()),
            "markise-dark" => Some(Theme::dark()),
            _ => palettes::palette(id),
        }
    }

    /// A stable fingerprint of every slot; part of the layout-cache key, so any theme change
    /// invalidates cached block layouts.
    pub fn fingerprint(&self) -> u64 {
        // `Debug` covers every field (floats included) without a manual `Hash` impl.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{self:?}").hash(&mut h);
        h.finish()
    }

    /// Body line height in points.
    pub fn line_px(&self) -> f32 {
        self.font_size * self.line_height
    }

    /// Monospace font size in points.
    pub fn mono_size(&self) -> f32 {
        (self.font_size * self.mono_scale).round()
    }
}

fn headings(color: Rgba, rule: Option<Rgba>, h6: Rgba) -> [HeadingStyle; 6] {
    let h = |size: f32, top: f32, bottom: f32, rule: Option<Rgba>| HeadingStyle {
        size,
        color,
        bold: true,
        margin_top: top,
        margin_bottom: bottom,
        rule,
    };
    [
        h(32.0, 22.0, 10.0, rule),
        h(25.0, 20.0, 8.0, rule),
        h(20.5, 16.0, 6.0, None),
        h(17.5, 14.0, 4.0, None),
        h(16.0, 12.0, 4.0, None),
        HeadingStyle {
            color: h6,
            ..h(15.0, 12.0, 4.0, None)
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_classify_by_luma_rule() {
        // spec 03 §1.4: brightness < 0.5 ⇒ dark chrome
        assert!(Theme::light().page_bg.brightness() >= 0.5);
        assert!(!Theme::light().dark);
        assert!(Theme::dark().page_bg.brightness() < 0.5);
        assert!(Theme::dark().dark);
        assert_eq!(Rgba::TRANSPARENT.brightness(), 1.0);
    }

    #[test]
    fn fingerprint_changes_with_any_slot() {
        let a = Theme::light();
        let mut b = a.clone();
        assert_eq!(a.fingerprint(), b.fingerprint());
        b.font_size += 1.0;
        assert_ne!(a.fingerprint(), b.fingerprint());
        assert_ne!(Theme::light().fingerprint(), Theme::dark().fingerprint());
    }
}
