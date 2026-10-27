//! Style and color scheme as independent parts of a theme (ADR 0007).
//!
//! A [`Theme`] — what layout consumes — is the composition of
//!
//! - a [`Style`]: typography and layout (font stacks, sizes, line height, spacing, column width,
//!   heading decorations, quote / list / table / code-box shapes, alignment, sidebar metrics);
//! - a [`ColorScheme`]: every color (page, text, headings, links, boxes, syntax tokens, chrome,
//!   sidebar, selection) plus the light/dark flag;
//! - user [`Overrides`] (body font, base size, line height, content width).
//!
//! [`compose`] builds a theme from the parts; [`Theme::style`] and [`Theme::colors`] decompose
//! any theme (native or imported) again, so `compose(&t.style(), &t.colors()) == t` for every
//! theme. Where a style asks for a decoration the scheme has no color for (a heading
//! background, a left bar, a code-box border), the color is derived from the scheme's own
//! roles (surface, accent, borders), never invented.
//!
//! Built-in styles ([`BUILTIN_STYLES`]) are original Markise designs; their font stacks name
//! widely installed system fonts with generic fallbacks (no bundled fonts).

use crate::{
    BoxStyle, ChromeColors, HeadingDecor, HeadingStyle, InlineColors, ListStyles, Rgba, SidebarStyle, SyntaxColors,
    TextAlign, TextTransform, Theme, ThemeFonts, TokenStyle,
};

// ------------------------------------------------------------------------------------ style

/// Shape of one heading level (no colors).
#[derive(Clone, Debug, PartialEq)]
pub struct HeadingShape {
    /// Font size in points.
    pub size: f32,
    pub bold: bool,
    pub margin_top: f32,
    pub margin_bottom: f32,
    /// Bottom rule.
    pub rule: bool,
    pub italic: bool,
    pub align: TextAlign,
    pub transform: TextTransform,
    pub letter_spacing: f32,
    /// Generated text before / after the heading.
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    /// Background box ("pill"): on/off, padding (horizontal, vertical), radius, hug the text.
    pub background: bool,
    pub padding: [f32; 2],
    pub radius: f32,
    pub fit: bool,
    /// Left bar width in points.
    pub bar: Option<f32>,
}

/// Shape of a box (code, math, front matter, inline code).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShape {
    pub padding: f32,
    pub radius: f32,
    /// Draw a border.
    pub border: bool,
}

/// Sidebar typography and metrics; `None` = UI default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarMetrics {
    pub fonts: Vec<String>,
    pub font_size: Option<f32>,
    pub bold: Option<bool>,
    pub row_height: Option<f32>,
    pub padding_x: Option<f32>,
    pub indent: Option<f32>,
    pub radius: Option<f32>,
}

/// Typography and layout of a theme (no colors).
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub name: String,
    pub font_size: f32,
    pub mono_scale: f32,
    pub line_height: f32,
    pub column_max_width: f32,
    pub column_padding_x: f32,
    pub column_padding_y: f32,
    pub block_spacing: f32,
    pub fonts: ThemeFonts,
    pub headings: [HeadingShape; 6],
    pub link_underline: bool,
    pub inline_code: BoxShape,
    pub code_block: BoxShape,
    pub math_block: BoxShape,
    pub meta_block: BoxShape,
    pub quote_bar_width: f32,
    pub quote_indent: f32,
    pub quote_italic: bool,
    pub quote_prefix: Option<String>,
    /// Blockquotes get a background fill.
    pub quote_fill: bool,
    pub list_indent: f32,
    pub list_styles: ListStyles,
    pub table_cell_padding: f32,
    pub text_align: TextAlign,
    pub sidebar: SidebarMetrics,
}

// ------------------------------------------------------------------------------ color scheme

/// Colors of one heading level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadingColors {
    pub color: Rgba,
    /// Bottom rule color (used when the style draws a rule; `None` = the scheme's rule color).
    pub rule: Option<Rgba>,
    /// Background box color (`None` = the scheme's surface color).
    pub background: Option<Rgba>,
    /// Left bar color (`None` = the scheme's accent).
    pub bar: Option<Rgba>,
    /// Generated prefix/suffix color (`None` = heading color).
    pub prefix: Option<Rgba>,
}

/// Colors of a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxColors {
    pub background: Rgba,
    pub text: Rgba,
    /// Border color (used when the style draws one; `None` = the table border color).
    pub border: Option<Rgba>,
}

/// Sidebar colors; `None` = UI default derived from the chrome colors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SidebarColors {
    pub text: Option<Rgba>,
    pub active_text: Option<Rgba>,
    pub active_bg: Option<Rgba>,
    pub hover_text: Option<Rgba>,
    pub hover_bg: Option<Rgba>,
    pub outline_active_text: Option<Rgba>,
    pub outline_active_bg: Option<Rgba>,
    pub icon: Option<Rgba>,
}

/// Every color of a theme (one appearance: light or dark).
#[derive(Clone, Debug, PartialEq)]
pub struct ColorScheme {
    pub name: String,
    /// Dark chrome (spec 03 §1.4).
    pub dark: bool,
    /// Comes from a scheme-aware theme (`prefers-color-scheme`).
    pub scheme_aware: bool,
    pub page_bg: Rgba,
    pub text: Rgba,
    pub headings: [HeadingColors; 6],
    pub link: Rgba,
    pub inline_code: BoxColors,
    pub code_block: BoxColors,
    pub math_block: BoxColors,
    pub meta_block: BoxColors,
    pub quote_bar: Rgba,
    pub quote_text: Rgba,
    /// Quote fill (used when the style fills quotes; transparent = the code-block background).
    pub quote_bg: Rgba,
    pub list_marker: Rgba,
    pub checkbox_border: Rgba,
    pub checkbox_checked: Rgba,
    pub rule: Rgba,
    pub table_border: Rgba,
    pub table_header_bg: Rgba,
    pub table_stripe_bg: Rgba,
    pub highlight_bg: Rgba,
    pub meta_text: Rgba,
    pub alert: [Rgba; 5],
    pub image_placeholder: Rgba,
    pub syntax: SyntaxColors,
    pub syntax_tokens: Vec<TokenStyle>,
    pub chrome: ChromeColors,
    pub inline: InlineColors,
    pub selection_text: Option<Rgba>,
    pub sidebar: SidebarColors,
}

// ---------------------------------------------------------------------------------- overrides

/// User overrides on top of the style (settings "Font & size…"); `None` = the style's value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    /// Body font family, tried before the style's body stack.
    pub body_font: Option<String>,
    /// Base font size in points; heading sizes scale with it.
    pub font_size: Option<f32>,
    /// Unitless line height.
    pub line_height: Option<f32>,
    /// Maximum content (column) width in points.
    pub content_width: Option<f32>,
}

impl Overrides {
    pub const FONT_SIZE: (f32, f32) = (9.0, 36.0);
    pub const LINE_HEIGHT: (f32, f32) = (1.0, 3.0);
    pub const CONTENT_WIDTH: (f32, f32) = (360.0, 2400.0);

    pub fn is_empty(&self) -> bool {
        *self == Overrides::default()
    }

    /// Applies the overrides to a style (values clamped to sane ranges, non-finite ignored).
    pub fn apply(&self, s: &mut Style) {
        let clamp = |v: f32, (lo, hi): (f32, f32)| v.is_finite().then(|| v.clamp(lo, hi));
        if let Some(f) = self.body_font.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            s.fonts.body.retain(|b| !b.eq_ignore_ascii_case(f));
            s.fonts.body.insert(0, f.to_string());
        }
        if let Some(size) = self.font_size.and_then(|v| clamp(v, Self::FONT_SIZE)) {
            let k = size / s.font_size;
            if k.is_finite() && k > 0.0 {
                for h in &mut s.headings {
                    h.size = (h.size * k * 4.0).round() / 4.0;
                }
            }
            s.font_size = size;
        }
        if let Some(lh) = self.line_height.and_then(|v| clamp(v, Self::LINE_HEIGHT)) {
            s.line_height = lh;
        }
        if let Some(w) = self.content_width.and_then(|v| clamp(v, Self::CONTENT_WIDTH)) {
            s.column_max_width = w;
        }
    }
}

// ------------------------------------------------------------------------- (de)composition

impl Theme {
    /// The typography/layout part of this theme.
    pub fn style(&self) -> Style {
        let shape = |b: &BoxStyle| BoxShape {
            padding: b.padding,
            radius: b.radius,
            border: b.border.is_some(),
        };
        let headings = std::array::from_fn(|i| {
            let (h, d) = (&self.headings[i], &self.heading_decor[i]);
            HeadingShape {
                size: h.size,
                bold: h.bold,
                margin_top: h.margin_top,
                margin_bottom: h.margin_bottom,
                rule: h.rule.is_some(),
                italic: d.italic,
                align: d.align,
                transform: d.transform,
                letter_spacing: d.letter_spacing,
                prefix: d.prefix.clone(),
                suffix: d.suffix.clone(),
                background: d.background.is_some(),
                padding: d.padding,
                radius: d.radius,
                fit: d.fit,
                bar: d.bar.map(|(w, _)| w),
            }
        });
        let sb = &self.sidebar;
        Style {
            name: self.name.clone(),
            font_size: self.font_size,
            mono_scale: self.mono_scale,
            line_height: self.line_height,
            column_max_width: self.column_max_width,
            column_padding_x: self.column_padding_x,
            column_padding_y: self.column_padding_y,
            block_spacing: self.block_spacing,
            fonts: self.fonts.clone(),
            headings,
            link_underline: self.link_underline,
            inline_code: shape(&self.inline_code),
            code_block: shape(&self.code_block),
            math_block: shape(&self.math_block),
            meta_block: shape(&self.meta_block),
            quote_bar_width: self.quote_bar_width,
            quote_indent: self.quote_indent,
            quote_italic: self.quote_italic,
            quote_prefix: self.quote_prefix.clone(),
            quote_fill: self.quote_bg.0[3] > 0,
            list_indent: self.list_indent,
            list_styles: self.list_styles.clone(),
            table_cell_padding: self.table_cell_padding,
            text_align: self.text_align,
            sidebar: SidebarMetrics {
                fonts: sb.fonts.clone(),
                font_size: sb.font_size,
                bold: sb.bold,
                row_height: sb.row_height,
                padding_x: sb.padding_x,
                indent: sb.indent,
                radius: sb.radius,
            },
        }
    }

    /// The color part of this theme.
    pub fn colors(&self) -> ColorScheme {
        let bx = |b: &BoxStyle| BoxColors {
            background: b.background,
            text: b.text,
            border: b.border,
        };
        let headings = std::array::from_fn(|i| {
            let (h, d) = (&self.headings[i], &self.heading_decor[i]);
            HeadingColors {
                color: h.color,
                rule: h.rule,
                background: d.background,
                bar: d.bar.map(|(_, c)| c),
                prefix: d.prefix_color,
            }
        });
        let sb = &self.sidebar;
        ColorScheme {
            name: self.name.clone(),
            dark: self.dark,
            scheme_aware: self.scheme_aware,
            page_bg: self.page_bg,
            text: self.text,
            headings,
            link: self.link,
            inline_code: bx(&self.inline_code),
            code_block: bx(&self.code_block),
            math_block: bx(&self.math_block),
            meta_block: bx(&self.meta_block),
            quote_bar: self.quote_bar,
            quote_text: self.quote_text,
            quote_bg: self.quote_bg,
            list_marker: self.list_marker,
            checkbox_border: self.checkbox_border,
            checkbox_checked: self.checkbox_checked,
            rule: self.rule,
            table_border: self.table_border,
            table_header_bg: self.table_header_bg,
            table_stripe_bg: self.table_stripe_bg,
            highlight_bg: self.highlight_bg,
            meta_text: self.meta_text,
            alert: self.alert,
            image_placeholder: self.image_placeholder,
            syntax: self.syntax,
            syntax_tokens: self.syntax_tokens.clone(),
            chrome: self.chrome,
            inline: self.inline,
            selection_text: self.selection_text,
            sidebar: SidebarColors {
                text: sb.text,
                active_text: sb.active_text,
                active_bg: sb.active_bg,
                hover_text: sb.hover_text,
                hover_bg: sb.hover_bg,
                outline_active_text: sb.outline_active_text,
                outline_active_bg: sb.outline_active_bg,
                icon: sb.icon,
            },
        }
    }
}

/// Builds the theme layout consumes from a style and a color scheme. The theme is named after
/// the scheme; callers rename it for the menu (see [`composed_name`]).
pub fn compose(style: &Style, colors: &ColorScheme) -> Theme {
    let c = colors;
    let bx = |shape: &BoxShape, col: &BoxColors| BoxStyle {
        background: col.background,
        text: col.text,
        border: shape.border.then(|| col.border.unwrap_or(c.table_border)),
        padding: shape.padding,
        radius: shape.radius,
    };
    let headings = std::array::from_fn(|i| {
        let (s, k) = (&style.headings[i], &c.headings[i]);
        // the composition changes the heading's background: keep its text legible (3:1, large
        // text) with a color of the scheme
        let color = match (s.background, k.background) {
            // a box the style does not draw: the text now sits on the page; the box color
            // itself is the natural replacement (e.g. white text on a blue pill → blue text)
            (false, Some(b)) => {
                let opaque_box = (b.0[3] == 255).then_some(b);
                legible(3.0, c.page_bg, [Some(k.color), opaque_box, Some(c.text)])
            }
            // a box the scheme has no color for: the derived surface
            // (composited over the page: a transparent surface leaves the page color)
            (true, None) => legible(
                3.0,
                over(c.table_header_bg, c.page_bg),
                [Some(k.color), Some(c.text), Some(c.page_bg)],
            ),
            _ => k.color,
        };
        HeadingStyle {
            size: s.size,
            color,
            bold: s.bold,
            margin_top: s.margin_top,
            margin_bottom: s.margin_bottom,
            rule: s.rule.then(|| k.rule.unwrap_or(c.rule)),
        }
    });
    let heading_decor = std::array::from_fn(|i| {
        let (s, k) = (&style.headings[i], &c.headings[i]);
        HeadingDecor {
            italic: s.italic,
            align: s.align,
            transform: s.transform,
            letter_spacing: s.letter_spacing,
            prefix: s.prefix.clone(),
            suffix: s.suffix.clone(),
            prefix_color: k.prefix,
            background: s.background.then(|| k.background.unwrap_or(c.table_header_bg)),
            padding: s.padding,
            radius: s.radius,
            fit: s.fit,
            bar: s.bar.map(|w| (w, k.bar.unwrap_or(c.chrome.accent))),
        }
    });
    // quote fill on/off comes from the style; when that changes the quote's background, keep
    // the quote text legible (4.5:1) with a color of the scheme
    let (quote_bg, quote_text) = match (style.quote_fill, c.quote_bg.0[3] > 0) {
        // unchanged (a transparent value is kept as is, so decomposition is lossless)
        (false, false) | (true, true) => (c.quote_bg, c.quote_text),
        // fill removed: the text now sits on the page
        (false, true) => (
            Rgba::TRANSPARENT,
            legible(4.5, c.page_bg, [Some(c.quote_text), Some(c.text), None]),
        ),
        // fill added on the code-block surface
        (true, false) => (
            c.code_block.background,
            legible(
                4.5,
                over(c.code_block.background, c.page_bg),
                [Some(c.quote_text), Some(c.code_block.text), Some(c.text)],
            ),
        ),
    };
    let (m, k) = (&style.sidebar, &c.sidebar);
    Theme {
        name: c.name.clone(),
        dark: c.dark,
        page_bg: c.page_bg,
        text: c.text,
        font_size: style.font_size,
        mono_scale: style.mono_scale,
        line_height: style.line_height,
        column_max_width: style.column_max_width,
        column_padding_x: style.column_padding_x,
        column_padding_y: style.column_padding_y,
        block_spacing: style.block_spacing,
        headings,
        link: c.link,
        inline_code: bx(&style.inline_code, &c.inline_code),
        code_block: bx(&style.code_block, &c.code_block),
        math_block: bx(&style.math_block, &c.math_block),
        meta_block: bx(&style.meta_block, &c.meta_block),
        quote_bar: c.quote_bar,
        quote_bar_width: style.quote_bar_width,
        quote_indent: style.quote_indent,
        quote_text,
        list_indent: style.list_indent,
        list_marker: c.list_marker,
        checkbox_border: c.checkbox_border,
        checkbox_checked: c.checkbox_checked,
        rule: c.rule,
        table_border: c.table_border,
        table_header_bg: c.table_header_bg,
        table_stripe_bg: c.table_stripe_bg,
        table_cell_padding: style.table_cell_padding,
        highlight_bg: c.highlight_bg,
        meta_text: c.meta_text,
        alert: c.alert,
        image_placeholder: c.image_placeholder,
        syntax: c.syntax,
        chrome: c.chrome,
        scheme_aware: c.scheme_aware,
        fonts: style.fonts.clone(),
        heading_decor,
        link_underline: style.link_underline,
        quote_bg,
        quote_italic: style.quote_italic,
        quote_prefix: style.quote_prefix.clone(),
        inline: c.inline,
        list_styles: style.list_styles.clone(),
        syntax_tokens: c.syntax_tokens.clone(),
        selection_text: c.selection_text,
        text_align: style.text_align,
        sidebar: SidebarStyle {
            fonts: m.fonts.clone(),
            font_size: m.font_size,
            bold: m.bold,
            text: k.text,
            active_text: k.active_text,
            active_bg: k.active_bg,
            hover_text: k.hover_text,
            hover_bg: k.hover_bg,
            outline_active_text: k.outline_active_text,
            outline_active_bg: k.outline_active_bg,
            icon: k.icon,
            row_height: m.row_height,
            padding_x: m.padding_x,
            indent: m.indent,
            radius: m.radius,
        },
    }
}

/// `top` composited over `bottom` (straight alpha; the result is opaque when `bottom` is).
pub fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let a = top.0[3] as f32 / 255.0;
    let ch = |i: usize| (top.0[i] as f32 * a + bottom.0[i] as f32 * (1.0 - a)).round() as u8;
    let alpha = (a + bottom.0[3] as f32 / 255.0 * (1.0 - a)) * 255.0;
    Rgba([ch(0), ch(1), ch(2), alpha.round() as u8])
}

/// The first candidate with at least `min` contrast on `bg`, else the most contrasting one.
fn legible<const N: usize>(min: f64, bg: Rgba, candidates: [Option<Rgba>; N]) -> Rgba {
    let mut it = candidates.into_iter().flatten();
    let first = it.next().expect("at least one candidate");
    if first.contrast(bg) >= min {
        return first;
    }
    let mut best = first;
    for c in it {
        if c.contrast(bg) >= min {
            return c;
        }
        if c.contrast(bg) > best.contrast(bg) {
            best = c;
        }
    }
    best
}

/// Composes with user overrides applied to the style.
pub fn compose_with(style: &Style, colors: &ColorScheme, overrides: &Overrides) -> Theme {
    if overrides.is_empty() {
        return compose(style, colors);
    }
    let mut s = style.clone();
    overrides.apply(&mut s);
    compose(&s, colors)
}

/// Display name of a composition: the colors' name alone when the style is the default (or the
/// colors' own), otherwise "colors · style".
pub fn composed_name(style: &str, colors: &str, style_is_default: bool) -> String {
    if style_is_default || style == colors {
        colors.to_string()
    } else {
        format!("{colors} · {style}")
    }
}

// ---------------------------------------------------------------------------- built-in styles

/// A built-in style: stable id (setting `style:<id>`), label, constructor.
#[derive(Debug)]
pub struct BuiltinStyle {
    pub id: &'static str,
    pub name: &'static str,
    pub build: fn() -> Style,
}

/// Built-in styles in menu order; the first is the default.
pub static BUILTIN_STYLES: &[BuiltinStyle] = &[
    BuiltinStyle {
        id: "markise",
        name: "Markise Default",
        build: markise_default,
    },
    BuiltinStyle {
        id: "classic",
        name: "Classic",
        build: classic,
    },
    BuiltinStyle {
        id: "modern",
        name: "Modern",
        build: modern,
    },
    BuiltinStyle {
        id: "technical",
        name: "Technical",
        build: technical,
    },
    BuiltinStyle {
        id: "compact",
        name: "Compact",
        build: compact,
    },
];

/// The id of the default style.
pub const DEFAULT_STYLE: &str = "markise";

/// A built-in style by id.
pub fn builtin_style(id: &str) -> Option<Style> {
    BUILTIN_STYLES.iter().find(|s| s.id == id).map(|s| (s.build)())
}

fn stack(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// Monospace stack shared by the built-in styles.
const MONO: &[&str] = &[
    "SF Mono",
    "Menlo",
    "Cascadia Mono",
    "Consolas",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "monospace",
];

/// Markise Default: the typography of Markise Light/Dark.
pub fn markise_default() -> Style {
    Style {
        name: "Markise Default".into(),
        ..Theme::light().style()
    }
}

fn plain_heading(size: f32, top: f32, bottom: f32) -> HeadingShape {
    HeadingShape {
        size,
        bold: true,
        margin_top: top,
        margin_bottom: bottom,
        rule: false,
        italic: false,
        align: TextAlign::Start,
        transform: TextTransform::None,
        letter_spacing: 0.0,
        prefix: None,
        suffix: None,
        background: false,
        padding: [0.0, 0.0],
        radius: 0.0,
        fit: false,
        bar: None,
    }
}

/// Classic: a book page — old-style serif, justified text, a narrow measure, quiet headings.
pub fn classic() -> Style {
    let serif = stack(&[
        "Iowan Old Style",
        "Palatino Linotype",
        "Palatino",
        "Book Antiqua",
        "Georgia",
        "Cambria",
        "Liberation Serif",
        "DejaVu Serif",
        "serif",
    ]);
    let d = markise_default();
    Style {
        name: "Classic".into(),
        font_size: 17.0,
        mono_scale: 0.85,
        line_height: 1.65,
        column_max_width: 680.0,
        column_padding_x: 48.0,
        column_padding_y: 40.0,
        block_spacing: 14.0,
        fonts: ThemeFonts {
            body: serif.clone(),
            heading: serif,
            mono: stack(MONO),
            faces: Vec::new(),
        },
        headings: [
            HeadingShape {
                bold: false,
                align: TextAlign::Center,
                letter_spacing: 0.5,
                ..plain_heading(32.0, 30.0, 18.0)
            },
            HeadingShape {
                bold: false,
                rule: true,
                ..plain_heading(25.0, 28.0, 12.0)
            },
            HeadingShape {
                bold: false,
                italic: true,
                ..plain_heading(21.0, 22.0, 8.0)
            },
            HeadingShape {
                transform: TextTransform::Uppercase,
                letter_spacing: 1.4,
                ..plain_heading(14.0, 20.0, 6.0)
            },
            HeadingShape {
                italic: true,
                ..plain_heading(16.0, 16.0, 4.0)
            },
            HeadingShape {
                transform: TextTransform::Uppercase,
                letter_spacing: 1.0,
                bold: false,
                ..plain_heading(13.0, 14.0, 4.0)
            },
        ],
        link_underline: true,
        inline_code: BoxShape {
            padding: 2.0,
            radius: 2.0,
            border: false,
        },
        code_block: BoxShape {
            padding: 14.0,
            radius: 2.0,
            border: true,
        },
        math_block: BoxShape {
            padding: 12.0,
            radius: 2.0,
            border: true,
        },
        meta_block: BoxShape {
            padding: 8.0,
            radius: 2.0,
            border: false,
        },
        quote_bar_width: 1.0,
        quote_indent: 26.0,
        quote_italic: true,
        quote_prefix: None,
        quote_fill: false,
        list_indent: 28.0,
        list_styles: ListStyles::default(),
        table_cell_padding: 7.0,
        text_align: TextAlign::Justify,
        sidebar: SidebarMetrics {
            row_height: Some(25.0),
            ..d.sidebar
        },
    }
}

/// Modern: a clean sans with generous spacing, bold tight headings, a bar on h2, soft boxes.
pub fn modern() -> Style {
    let sans = stack(&[
        "Avenir Next",
        "Segoe UI",
        "Helvetica Neue",
        "Ubuntu",
        "Cantarell",
        "Noto Sans",
        "Arial",
        "sans-serif",
    ]);
    Style {
        name: "Modern".into(),
        font_size: 16.0,
        mono_scale: 0.88,
        line_height: 1.75,
        column_max_width: 760.0,
        column_padding_x: 44.0,
        column_padding_y: 36.0,
        block_spacing: 16.0,
        fonts: ThemeFonts {
            body: sans.clone(),
            heading: sans.clone(),
            mono: stack(MONO),
            faces: Vec::new(),
        },
        headings: [
            HeadingShape {
                letter_spacing: -0.6,
                ..plain_heading(36.0, 30.0, 14.0)
            },
            HeadingShape {
                letter_spacing: -0.3,
                bar: Some(4.0),
                padding: [12.0, 0.0],
                ..plain_heading(26.0, 28.0, 10.0)
            },
            plain_heading(20.0, 22.0, 8.0),
            plain_heading(17.0, 18.0, 6.0),
            HeadingShape {
                transform: TextTransform::Uppercase,
                letter_spacing: 0.8,
                ..plain_heading(14.0, 16.0, 4.0)
            },
            HeadingShape {
                transform: TextTransform::Uppercase,
                letter_spacing: 0.8,
                ..plain_heading(12.5, 14.0, 4.0)
            },
        ],
        link_underline: false,
        inline_code: BoxShape {
            padding: 3.0,
            radius: 5.0,
            border: false,
        },
        code_block: BoxShape {
            padding: 16.0,
            radius: 10.0,
            border: false,
        },
        math_block: BoxShape {
            padding: 14.0,
            radius: 10.0,
            border: false,
        },
        meta_block: BoxShape {
            padding: 10.0,
            radius: 8.0,
            border: false,
        },
        quote_bar_width: 4.0,
        quote_indent: 20.0,
        quote_italic: false,
        quote_prefix: None,
        quote_fill: true,
        list_indent: 26.0,
        list_styles: ListStyles::default(),
        table_cell_padding: 9.0,
        text_align: TextAlign::Start,
        sidebar: SidebarMetrics {
            fonts: sans,
            font_size: Some(13.0),
            bold: None,
            row_height: Some(28.0),
            padding_x: Some(8.0),
            indent: Some(14.0),
            radius: Some(6.0),
        },
    }
}

/// Technical: a code notebook — monospace everywhere, Markdown-like heading prefixes, square
/// boxes, a wide column.
pub fn technical() -> Style {
    let mono = stack(&[
        "JetBrains Mono",
        "SF Mono",
        "Menlo",
        "Cascadia Mono",
        "Consolas",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "monospace",
    ]);
    let h = |level: usize, size: f32, top: f32, bottom: f32| HeadingShape {
        prefix: Some(format!("{} ", "#".repeat(level))),
        ..plain_heading(size, top, bottom)
    };
    Style {
        name: "Technical".into(),
        font_size: 14.5,
        mono_scale: 1.0,
        line_height: 1.6,
        column_max_width: 860.0,
        column_padding_x: 36.0,
        column_padding_y: 28.0,
        block_spacing: 12.0,
        fonts: ThemeFonts {
            body: mono.clone(),
            heading: mono.clone(),
            mono: mono.clone(),
            faces: Vec::new(),
        },
        headings: [
            HeadingShape {
                rule: true,
                ..h(1, 23.0, 24.0, 12.0)
            },
            h(2, 19.0, 22.0, 8.0),
            h(3, 16.5, 18.0, 6.0),
            h(4, 15.0, 14.0, 4.0),
            h(5, 14.5, 12.0, 4.0),
            h(6, 14.5, 12.0, 4.0),
        ],
        link_underline: true,
        inline_code: BoxShape {
            padding: 2.0,
            radius: 0.0,
            border: false,
        },
        code_block: BoxShape {
            padding: 12.0,
            radius: 0.0,
            border: true,
        },
        math_block: BoxShape {
            padding: 10.0,
            radius: 0.0,
            border: true,
        },
        meta_block: BoxShape {
            padding: 8.0,
            radius: 0.0,
            border: true,
        },
        quote_bar_width: 2.0,
        quote_indent: 18.0,
        quote_italic: false,
        quote_prefix: None,
        quote_fill: false,
        list_indent: 24.0,
        list_styles: ListStyles {
            bullets: stack(&["square", "square", "square"]),
            numbers: Vec::new(),
        },
        table_cell_padding: 6.0,
        text_align: TextAlign::Start,
        sidebar: SidebarMetrics {
            fonts: mono,
            font_size: Some(12.0),
            bold: None,
            row_height: Some(22.0),
            padding_x: Some(6.0),
            indent: Some(12.0),
            radius: Some(0.0),
        },
    }
}

/// Compact: dense reading for reference material — smaller type, tight spacing, wide column.
pub fn compact() -> Style {
    let sans = stack(&[
        "Segoe UI",
        "SF Pro Text",
        "Helvetica Neue",
        "Roboto",
        "Noto Sans",
        "Liberation Sans",
        "Arial",
        "sans-serif",
    ]);
    Style {
        name: "Compact".into(),
        font_size: 14.0,
        mono_scale: 0.9,
        line_height: 1.42,
        column_max_width: 980.0,
        column_padding_x: 24.0,
        column_padding_y: 16.0,
        block_spacing: 7.0,
        fonts: ThemeFonts {
            body: sans.clone(),
            heading: sans.clone(),
            mono: stack(MONO),
            faces: Vec::new(),
        },
        headings: [
            HeadingShape {
                rule: true,
                ..plain_heading(23.0, 14.0, 6.0)
            },
            HeadingShape {
                rule: true,
                ..plain_heading(19.0, 12.0, 5.0)
            },
            plain_heading(16.5, 10.0, 4.0),
            plain_heading(15.0, 8.0, 3.0),
            plain_heading(14.0, 8.0, 2.0),
            plain_heading(13.0, 8.0, 2.0),
        ],
        link_underline: false,
        inline_code: BoxShape {
            padding: 1.5,
            radius: 2.0,
            border: false,
        },
        code_block: BoxShape {
            padding: 8.0,
            radius: 3.0,
            border: true,
        },
        math_block: BoxShape {
            padding: 7.0,
            radius: 3.0,
            border: true,
        },
        meta_block: BoxShape {
            padding: 6.0,
            radius: 3.0,
            border: false,
        },
        quote_bar_width: 3.0,
        quote_indent: 14.0,
        quote_italic: false,
        quote_prefix: None,
        quote_fill: false,
        list_indent: 20.0,
        list_styles: ListStyles::default(),
        table_cell_padding: 4.0,
        text_align: TextAlign::Start,
        sidebar: SidebarMetrics {
            fonts: sans,
            font_size: Some(12.0),
            bold: None,
            row_height: Some(20.0),
            padding_x: Some(5.0),
            indent: Some(12.0),
            radius: Some(3.0),
        },
    }
}

// ------------------------------------------------------------------------------- color schemes

/// The color scheme of a native theme id (`markise-light`, `markise-dark`, palette ids).
/// Generated heading prefixes (e.g. the Technical style's `#`) take the list-marker color,
/// so they read as markup rather than as part of the title.
pub fn native_colors(id: &str) -> Option<ColorScheme> {
    Theme::native(id).map(|t| {
        let mut c = t.colors();
        for h in &mut c.headings {
            h.prefix.get_or_insert(t.list_marker);
        }
        c
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palettes::PALETTE_THEMES;

    fn all_native_schemes() -> Vec<ColorScheme> {
        let mut v = vec![Theme::light().colors(), Theme::dark().colors()];
        v.extend(PALETTE_THEMES.iter().map(|p| native_colors(p.id).unwrap()));
        v
    }

    #[test]
    fn decompose_then_compose_is_identity_for_native_themes() {
        let mut themes = vec![Theme::light(), Theme::dark()];
        themes.extend(PALETTE_THEMES.iter().map(|p| Theme::native(p.id).unwrap()));
        for t in themes {
            assert_eq!(compose(&t.style(), &t.colors()), t, "{}", t.name);
        }
    }

    #[test]
    fn style_fields_come_from_the_style_and_colors_from_the_scheme() {
        let colors = native_colors("nord").unwrap();
        for bs in BUILTIN_STYLES {
            let style = (bs.build)();
            let t = compose(&style, &colors);
            // style part
            assert_eq!(t.font_size, style.font_size, "{}", bs.id);
            assert_eq!(t.line_height, style.line_height);
            assert_eq!(t.column_max_width, style.column_max_width);
            assert_eq!(t.block_spacing, style.block_spacing);
            assert_eq!(t.fonts, style.fonts);
            assert_eq!(t.text_align, style.text_align);
            assert_eq!(t.code_block.radius, style.code_block.radius);
            assert_eq!(t.code_block.border.is_some(), style.code_block.border);
            for i in 0..6 {
                assert_eq!(t.headings[i].size, style.headings[i].size);
                assert_eq!(t.headings[i].rule.is_some(), style.headings[i].rule);
                assert_eq!(t.heading_decor[i].prefix, style.headings[i].prefix);
                assert_eq!(t.heading_decor[i].bar.map(|b| b.0), style.headings[i].bar);
                // color part
                assert_eq!(t.headings[i].color, colors.headings[i].color);
            }
            assert_eq!(t.sidebar.row_height, style.sidebar.row_height);
            // color part
            assert_eq!(t.page_bg, colors.page_bg);
            assert_eq!(t.text, colors.text);
            assert_eq!(t.link, colors.link);
            assert_eq!(t.dark, colors.dark);
            assert_eq!(t.code_block.background, colors.code_block.background);
            assert_eq!(t.syntax, colors.syntax);
            assert_eq!(t.chrome, colors.chrome);
            assert_eq!(t.sidebar.text, colors.sidebar.text);
            // decomposing the composition gives the parts back (up to derived colors)
            assert_eq!(t.style().font_size, style.font_size);
            assert_eq!(t.colors().page_bg, colors.page_bg);
        }
    }

    #[test]
    fn missing_decoration_colors_are_derived_from_the_scheme() {
        let colors = Theme::dark().colors();
        let mut style = markise_default();
        style.headings[2].background = true;
        style.headings[2].bar = Some(3.0);
        style.headings[3].rule = true;
        style.quote_fill = true;
        style.inline_code.border = true;
        let t = compose(&style, &colors);
        assert_eq!(t.heading_decor[2].background, Some(colors.table_header_bg));
        assert_eq!(t.heading_decor[2].bar, Some((3.0, colors.chrome.accent)));
        assert_eq!(t.headings[3].rule, Some(colors.rule));
        assert_eq!(t.quote_bg, colors.code_block.background);
        assert_eq!(t.inline_code.border, Some(colors.table_border));
    }

    #[test]
    fn transparent_quote_backgrounds_survive_decomposition() {
        // P3 boundary: a transparent value that is not all zeros
        let mut t = Theme::light();
        t.quote_bg = Rgba([255, 0, 0, 0]);
        assert!(!t.style().quote_fill);
        assert_eq!(compose(&t.style(), &t.colors()), t);
        assert_eq!(compose(&t.style(), &t.colors()).fingerprint(), t.fingerprint());
    }

    #[test]
    fn quote_text_stays_legible_when_the_style_removes_or_adds_the_fill() {
        // a custom scheme: white page, white quote text on a dark quote fill
        let mut colors = Theme::light().colors();
        colors.page_bg = Rgba::hex(0xffffff);
        colors.quote_bg = Rgba::hex(0x202830);
        colors.quote_text = Rgba::hex(0xffffff);
        // the complete theme (a filled style) keeps its colors
        let mut filled = markise_default();
        filled.quote_fill = true;
        let t = compose(&filled, &colors);
        assert_eq!((t.quote_bg, t.quote_text), (colors.quote_bg, colors.quote_text));
        // a style without fill: the text moves to the page and must change
        for bs in BUILTIN_STYLES.iter().filter(|s| !(s.build)().quote_fill) {
            let t = compose(&(bs.build)(), &colors);
            assert_eq!(t.quote_bg.0[3], 0, "{}", bs.id);
            assert!(t.quote_text.contrast(t.page_bg) >= 4.5, "{}: {:?}", bs.id, t.quote_text);
            assert_eq!(t.quote_text, colors.text, "a scheme color");
        }
        // a style with fill on a scheme without one: the code surface, legible text
        let mut colors = Theme::dark().colors();
        colors.quote_bg = Rgba::TRANSPARENT;
        colors.code_block.background = Rgba::hex(0xf0f0f0); // light code boxes in a dark scheme
        colors.code_block.text = Rgba::hex(0x202020);
        let t = compose(&modern(), &colors);
        assert_eq!(t.quote_bg, colors.code_block.background);
        assert!(t.quote_text.contrast(t.quote_bg) >= 4.5, "{:?}", t.quote_text);
        assert_eq!(t.quote_text, colors.code_block.text);
    }

    #[test]
    fn a_transparent_surface_counts_as_the_page_for_an_added_heading_box() {
        // white page, black text, no heading box, transparent surface (importer output for
        // bordered tables without fills)
        let mut colors = Theme::light().colors();
        colors.page_bg = Rgba::hex(0xffffff);
        colors.text = Rgba::hex(0x000000);
        colors.headings[0].color = Rgba::hex(0x000000);
        colors.headings[0].background = None;
        colors.table_header_bg = Rgba::TRANSPARENT;
        let mut boxed = markise_default();
        boxed.headings[0].background = true;
        let t = compose(&boxed, &colors);
        assert_eq!(t.headings[0].color, Rgba::hex(0x000000), "not white on white");
        // a half-transparent dark surface over white is mid-grey: black still reads (≥ 3:1)
        colors.table_header_bg = Rgba([0, 0, 0, 100]);
        let t = compose(&boxed, &colors);
        let eff = over(colors.table_header_bg, colors.page_bg);
        assert!(
            t.headings[0].color.contrast(eff) >= 3.0,
            "{:?} on {eff:?}",
            t.headings[0].color
        );
    }

    #[test]
    fn compositing() {
        let white = Rgba::hex(0xffffff);
        assert_eq!(over(Rgba::TRANSPARENT, white), white);
        assert_eq!(over(Rgba::hex(0x102030), white), Rgba::hex(0x102030));
        assert_eq!(over(Rgba([0, 0, 0, 128]), white), Rgba([127, 127, 127, 255]));
    }

    #[test]
    fn heading_text_stays_legible_on_an_added_box() {
        // a custom scheme whose raised surface is close to its heading color
        let mut colors = Theme::light().colors();
        colors.table_header_bg = Rgba::hex(0x303030);
        let mut boxed = markise_default();
        boxed.headings[0].background = true;
        let t = compose(&boxed, &colors);
        assert_eq!(t.heading_decor[0].background, Some(Rgba::hex(0x303030)));
        assert!(t.headings[0].color.contrast(Rgba::hex(0x303030)) >= 3.0);
        assert_eq!(t.headings[0].color, colors.page_bg, "light page color on the dark box");
        // without a box the scheme's heading color is kept
        assert_eq!(
            compose(&markise_default(), &colors).headings[0].color,
            colors.headings[0].color
        );
    }

    #[test]
    fn a_heading_color_made_for_a_box_stays_legible_without_the_box() {
        // e.g. white h2 text on a blue pill, used with a style that draws no pill
        let mut colors = Theme::light().colors();
        colors.headings[1].color = Rgba::hex(0xffffff);
        colors.headings[1].background = Some(Rgba::hex(0x4870ac));
        let t = compose(&markise_default(), &colors);
        assert_eq!(t.headings[1].color, Rgba::hex(0x4870ac));
        assert_eq!(t.heading_decor[1].background, None);
        // a pale box color falls back to the body text color
        colors.headings[1].background = Some(Rgba::hex(0xf4f4f4));
        assert_eq!(compose(&markise_default(), &colors).headings[1].color, colors.text);
        // the style that draws the box keeps the scheme's heading color
        let mut boxed = markise_default();
        boxed.headings[1].background = true;
        assert_eq!(compose(&boxed, &colors).headings[1].color, Rgba::hex(0xffffff));
    }

    #[test]
    fn fingerprint_changes_when_either_part_changes() {
        let a = compose(&markise_default(), &Theme::light().colors());
        let b = compose(&classic(), &Theme::light().colors());
        let c = compose(&markise_default(), &native_colors("nord").unwrap());
        assert_ne!(a.fingerprint(), b.fingerprint(), "style change");
        assert_ne!(a.fingerprint(), c.fingerprint(), "colors change");
        let o = Overrides {
            line_height: Some(1.9),
            ..Default::default()
        };
        let d = compose_with(&markise_default(), &Theme::light().colors(), &o);
        assert_ne!(a.fingerprint(), d.fingerprint(), "override change");
    }

    #[test]
    fn overrides_apply_and_clamp() {
        let mut s = markise_default();
        let h1 = s.headings[0].size;
        Overrides {
            body_font: Some(" Georgia ".into()),
            font_size: Some(20.0),
            line_height: Some(9.0),
            content_width: Some(f32::NAN),
        }
        .apply(&mut s);
        assert_eq!(s.fonts.body.first().map(String::as_str), Some("Georgia"));
        assert_eq!(s.font_size, 20.0);
        assert_eq!(
            s.headings[0].size,
            h1 * 20.0 / 16.0,
            "headings scale with the base size"
        );
        assert_eq!(s.line_height, Overrides::LINE_HEIGHT.1, "clamped");
        assert_eq!(s.column_max_width, markise_default().column_max_width, "NaN ignored");
        // a font already in the stack moves to the front instead of appearing twice
        let mut s = classic();
        Overrides {
            body_font: Some("georgia".into()),
            ..Default::default()
        }
        .apply(&mut s);
        assert_eq!(s.fonts.body[0], "georgia");
        assert_eq!(
            s.fonts
                .body
                .iter()
                .filter(|f| f.eq_ignore_ascii_case("georgia"))
                .count(),
            1
        );
    }

    #[test]
    fn builtin_styles_have_unique_ids_and_markise_is_default() {
        assert_eq!(BUILTIN_STYLES[0].id, DEFAULT_STYLE);
        let mut ids: Vec<_> = BUILTIN_STYLES.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN_STYLES.len());
        for s in BUILTIN_STYLES {
            assert_eq!((s.build)().name, s.name);
        }
        // the default style reproduces Markise Light exactly
        assert_eq!(compose(&markise_default(), &Theme::light().colors()), Theme::light());
    }

    /// WCAG AA (4.5:1) for body text, links and code text, and 3:1 (large text) for headings
    /// on their background, for every built-in style × native color scheme.
    #[test]
    fn every_style_and_scheme_combination_meets_wcag_aa() {
        let mut failures = Vec::new();
        for bs in BUILTIN_STYLES {
            let style = (bs.build)();
            for scheme in all_native_schemes() {
                let t = compose(&style, &scheme);
                let quote_bg = if t.quote_bg.0[3] > 0 { t.quote_bg } else { t.page_bg };
                let mut checks = vec![
                    ("text", t.text, t.page_bg, 4.5),
                    ("link", t.link, t.page_bg, 4.5),
                    ("inline code", t.inline_code.text, t.inline_code.background, 4.5),
                    ("code block", t.code_block.text, t.code_block.background, 4.5),
                    ("quote", t.quote_text, quote_bg, 4.5),
                ];
                for (i, h) in t.headings.iter().enumerate() {
                    let bg = t.heading_decor[i].background.unwrap_or(t.page_bg);
                    checks.push(("heading", h.color, bg, 3.0));
                }
                for (what, fg, bg, min) in checks {
                    let c = fg.contrast(bg);
                    if c < min {
                        failures.push(format!("{} × {} {what}: {c:.2}", bs.name, scheme.name));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
