//! Palette themes (ADR 0004 §Palettes): original native Markise themes built from published,
//! permissively licensed color palettes — only the palettes' hex values and their documented
//! roles (background / surface / text / accent) are used; the mapping to theme slots is
//! Markise's own ([`build`]). Sources, commits and licenses: `assets/themes/palettes/SOURCES.md`
//! and the repository `NOTICE`. Markise Night is an original palette.

use crate::{BoxStyle, ChromeColors, HeadingStyle, InlineColors, Rgba, SidebarStyle, SyntaxColors, Theme, ThemeFonts};

/// The colors a palette contributes, by role.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Page background.
    pub bg: Rgba,
    /// One step away from `bg` (sidebar, code blocks): darker in dark palettes.
    pub bg_alt: Rgba,
    /// Raised surface (inline code, table header, active item).
    pub surface: Rgba,
    /// Borders, rules, inactive markers.
    pub overlay: Rgba,
    /// Body text.
    pub text: Rgba,
    /// The palette's highest-contrast text color (used where `text` is not legible enough,
    /// e.g. on the `mark` tint).
    pub ink: Rgba,
    /// Secondary text (quotes, h5/h6, sidebar).
    pub subtext: Rgba,
    /// De-emphasised text (meta, comments in prose, labels).
    pub muted: Rgba,
    /// Links and the primary accent.
    pub accent: Rgba,
    /// Secondary accent (h3/h4, list markers).
    pub accent2: Rgba,
    /// Inline code text.
    pub code: Rgba,
    /// Inline code background.
    pub code_bg: Rgba,
    /// Code / math / front-matter block background.
    pub block_bg: Rgba,
    pub red: Rgba,
    pub yellow: Rgba,
    pub green: Rgba,
    pub blue: Rgba,
    pub purple: Rgba,
    /// Text selection background.
    pub selection: Rgba,
    pub syntax: SyntaxColors,
}

const fn h(rgb: u32) -> Rgba {
    Rgba::hex(rgb)
}

/// `pct` % of the way from palette color `a` to palette color `b` (intermediate shades are
/// always mixes of two palette colors, never new hues).
const fn mx(a: u32, b: u32, pct: u32) -> Rgba {
    Rgba([mx_ch(a, b, pct, 16), mx_ch(a, b, pct, 8), mx_ch(a, b, pct, 0), 255])
}

const fn mx_ch(a: u32, b: u32, pct: u32, shift: u32) -> u8 {
    let x = (a >> shift) & 0xff;
    let y = (b >> shift) & 0xff;
    ((x * (100 - pct) + y * pct + 50) / 100) as u8
}

const fn syntax(keyword: u32, string: u32, number: u32, comment: u32, function: u32, type_name: u32) -> SyntaxColors {
    SyntaxColors {
        keyword: h(keyword),
        string: h(string),
        comment: h(comment),
        number: h(number),
        function: h(function),
        type_name: h(type_name),
    }
}

/// One palette theme: stable id (setting `palette:<id>` via its family), label, appearance.
#[derive(Debug)]
pub struct PaletteTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub palette: Palette,
}

/// A menu entry of the palette pack: a light and/or dark palette theme.
#[derive(Debug)]
pub struct PaletteFamily {
    pub id: &'static str,
    pub name: &'static str,
    pub light: Option<&'static str>,
    pub dark: Option<&'static str>,
}

/// Families in menu order; pairs where the palette has both appearances.
pub static PALETTE_FAMILIES: &[PaletteFamily] = &[
    PaletteFamily {
        id: "catppuccin",
        name: "Catppuccin (Latte / Mocha)",
        light: Some("catppuccin-latte"),
        dark: Some("catppuccin-mocha"),
    },
    PaletteFamily {
        id: "catppuccin-frappe",
        name: "Catppuccin Frappé",
        light: None,
        dark: Some("catppuccin-frappe"),
    },
    PaletteFamily {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        light: None,
        dark: Some("catppuccin-macchiato"),
    },
    PaletteFamily {
        id: "dracula",
        name: "Dracula",
        light: None,
        dark: Some("dracula"),
    },
    PaletteFamily {
        id: "gruvbox",
        name: "Gruvbox",
        light: Some("gruvbox-light"),
        dark: Some("gruvbox-dark"),
    },
    PaletteFamily {
        id: "nord",
        name: "Nord",
        light: None,
        dark: Some("nord"),
    },
    PaletteFamily {
        id: "rose-pine",
        name: "Rosé Pine (Dawn / Main)",
        light: Some("rose-pine-dawn"),
        dark: Some("rose-pine"),
    },
    PaletteFamily {
        id: "rose-pine-moon",
        name: "Rosé Pine Moon",
        light: None,
        dark: Some("rose-pine-moon"),
    },
    PaletteFamily {
        id: "tokyo-night",
        name: "Tokyo Night (Day / Night)",
        light: Some("tokyo-night-day"),
        dark: Some("tokyo-night"),
    },
    PaletteFamily {
        id: "tokyo-night-storm",
        name: "Tokyo Night Storm",
        light: None,
        dark: Some("tokyo-night-storm"),
    },
];

/// Every palette theme. Hex values are copied from the palette sources (see SOURCES.md);
/// role assignments follow each palette's usage notes; surfaces not in a palette are the
/// palette's own neighbouring shades.
pub static PALETTE_THEMES: &[PaletteTheme] = &[
    // ---- Markise Night (original): calm slate, soft light text, restrained steel-blue accent
    PaletteTheme {
        id: "markise-night",
        name: "Markise Night",
        dark: true,
        palette: Palette {
            bg: h(0x1b2027),
            bg_alt: h(0x161a20),
            surface: h(0x252c35),
            overlay: h(0x38424e),
            text: h(0xd5dbe2),
            ink: h(0xd5dbe2),
            subtext: h(0xaab3be),
            muted: h(0x808b98),
            accent: h(0x86b8d8),
            accent2: h(0xcdb68c),
            code: h(0xe2b48c),
            code_bg: h(0x252c35),
            block_bg: h(0x161a20),
            red: h(0xe07f80),
            yellow: h(0xe6cf8f),
            green: h(0x93c9a0),
            blue: h(0x86b8d8),
            purple: h(0xb9a6e2),
            selection: h(0x34506a),
            syntax: syntax(0xb9a6e2, 0x93c9a0, 0xe2b48c, 0x808b98, 0x86b8d8, 0xcdb68c),
        },
    },
    // ---- Catppuccin (catppuccin/palette): base / mantle / surface0 / surface1, text / subtext
    PaletteTheme {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        dark: false,
        palette: Palette {
            bg: h(0xeff1f5),
            bg_alt: h(0xe6e9ef),
            surface: h(0xccd0da),
            overlay: h(0xbcc0cc),
            text: h(0x4c4f69),
            ink: h(0x4c4f69),
            subtext: h(0x5c5f77),
            muted: h(0x6c6f85),
            accent: h(0x8839ef),
            accent2: h(0x1e66f5),
            code: h(0xd20f39),
            code_bg: mx(0xeff1f5, 0xccd0da, 15),
            block_bg: h(0xe6e9ef),
            red: h(0xd20f39),
            yellow: h(0xdf8e1d),
            green: h(0x40a02b),
            blue: h(0x1e66f5),
            purple: h(0x8839ef),
            selection: h(0xacb0be),
            syntax: syntax(0x8839ef, 0x40a02b, 0xfe640b, 0x7c7f93, 0x1e66f5, 0xdf8e1d),
        },
    },
    PaletteTheme {
        id: "catppuccin-frappe",
        name: "Catppuccin Frappé",
        dark: true,
        palette: Palette {
            bg: h(0x303446),
            bg_alt: h(0x292c3c),
            surface: h(0x414559),
            overlay: h(0x51576d),
            text: h(0xc6d0f5),
            ink: h(0xc6d0f5),
            subtext: h(0xb5bfe2),
            muted: h(0xa5adce),
            accent: h(0x8caaee),
            accent2: h(0xca9ee6),
            code: h(0xef9f76),
            code_bg: mx(0x303446, 0x414559, 50),
            block_bg: h(0x292c3c),
            red: h(0xe78284),
            yellow: h(0xe5c890),
            green: h(0xa6d189),
            blue: h(0x8caaee),
            purple: h(0xca9ee6),
            selection: h(0x626880),
            syntax: syntax(0xca9ee6, 0xa6d189, 0xef9f76, 0x949cbb, 0x8caaee, 0xe5c890),
        },
    },
    PaletteTheme {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        dark: true,
        palette: Palette {
            bg: h(0x24273a),
            bg_alt: h(0x1e2030),
            surface: h(0x363a4f),
            overlay: h(0x494d64),
            text: h(0xcad3f5),
            ink: h(0xcad3f5),
            subtext: h(0xb8c0e0),
            muted: h(0xa5adcb),
            accent: h(0x8aadf4),
            accent2: h(0xc6a0f6),
            code: h(0xf5a97f),
            code_bg: h(0x363a4f),
            block_bg: h(0x1e2030),
            red: h(0xed8796),
            yellow: h(0xeed49f),
            green: h(0xa6da95),
            blue: h(0x8aadf4),
            purple: h(0xc6a0f6),
            selection: h(0x5b6078),
            syntax: syntax(0xc6a0f6, 0xa6da95, 0xf5a97f, 0x939ab7, 0x8aadf4, 0xeed49f),
        },
    },
    PaletteTheme {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        dark: true,
        palette: Palette {
            bg: h(0x1e1e2e),
            bg_alt: h(0x181825),
            surface: h(0x313244),
            overlay: h(0x45475a),
            text: h(0xcdd6f4),
            ink: h(0xcdd6f4),
            subtext: h(0xbac2de),
            muted: h(0xa6adc8),
            accent: h(0x89b4fa),
            accent2: h(0xcba6f7),
            code: h(0xfab387),
            code_bg: h(0x313244),
            block_bg: h(0x181825),
            red: h(0xf38ba8),
            yellow: h(0xf9e2af),
            green: h(0xa6e3a1),
            blue: h(0x89b4fa),
            purple: h(0xcba6f7),
            selection: h(0x585b70),
            syntax: syntax(0xcba6f7, 0xa6e3a1, 0xfab387, 0x9399b2, 0x89b4fa, 0xf9e2af),
        },
    },
    // ---- Dracula (dracula/dracula-theme spec): background, current line, foreground, comment
    PaletteTheme {
        id: "dracula",
        name: "Dracula",
        dark: true,
        palette: Palette {
            bg: h(0x282a36),
            bg_alt: mx(0x282a36, 0x44475a, 35),
            surface: h(0x44475a),
            overlay: mx(0x44475a, 0x6272a4, 50),
            text: h(0xf8f8f2),
            ink: h(0xf8f8f2),
            subtext: mx(0xf8f8f2, 0x6272a4, 25),
            muted: mx(0xf8f8f2, 0x6272a4, 55),
            accent: h(0x8be9fd),
            accent2: h(0xbd93f9),
            code: h(0x50fa7b),
            code_bg: h(0x44475a),
            block_bg: mx(0x282a36, 0x44475a, 35),
            red: h(0xff5555),
            yellow: h(0xf1fa8c),
            green: h(0x50fa7b),
            blue: h(0x8be9fd),
            purple: h(0xbd93f9),
            selection: h(0x44475a),
            syntax: syntax(0xff79c6, 0xf1fa8c, 0xbd93f9, 0x6272a4, 0x50fa7b, 0x8be9fd),
        },
    },
    // ---- Gruvbox (morhetz/gruvbox): bg0 / bg1 / bg2, fg1 / fg2 / gray; light uses the faded accents
    PaletteTheme {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        dark: true,
        palette: Palette {
            bg: h(0x282828),
            bg_alt: h(0x1d2021),
            surface: h(0x3c3836),
            overlay: h(0x504945),
            text: h(0xebdbb2),
            ink: h(0xebdbb2),
            subtext: h(0xd5c4a1),
            muted: h(0xa89984),
            accent: h(0x83a598),
            accent2: h(0xfabd2f),
            code: h(0xfe8019),
            code_bg: h(0x3c3836),
            block_bg: h(0x1d2021),
            red: h(0xfb4934),
            yellow: h(0xfabd2f),
            green: h(0xb8bb26),
            blue: h(0x83a598),
            purple: h(0xd3869b),
            selection: h(0x504945),
            syntax: syntax(0xfb4934, 0xb8bb26, 0xd3869b, 0x928374, 0x8ec07c, 0xfabd2f),
        },
    },
    PaletteTheme {
        id: "gruvbox-light",
        name: "Gruvbox Light",
        dark: false,
        palette: Palette {
            bg: h(0xfbf1c7),
            bg_alt: h(0xf2e5bc),
            surface: h(0xebdbb2),
            overlay: h(0xd5c4a1),
            text: h(0x3c3836),
            ink: h(0x3c3836),
            subtext: h(0x504945),
            muted: h(0x7c6f64),
            accent: h(0x076678),
            accent2: h(0xaf3a03),
            code: h(0x9d0006),
            code_bg: h(0xebdbb2),
            block_bg: h(0xf2e5bc),
            red: h(0x9d0006),
            yellow: h(0xb57614),
            green: h(0x79740e),
            blue: h(0x076678),
            purple: h(0x8f3f71),
            selection: h(0xd5c4a1),
            syntax: syntax(0x9d0006, 0x79740e, 0x8f3f71, 0x928374, 0x427b58, 0xb57614),
        },
    },
    // ---- Nord (nordtheme/nord): Polar Night bg, Snow Storm text, Frost accents, Aurora colors
    PaletteTheme {
        id: "nord",
        name: "Nord",
        dark: true,
        palette: Palette {
            bg: h(0x2e3440),
            bg_alt: h(0x3b4252),
            surface: h(0x434c5e),
            overlay: h(0x4c566a),
            text: h(0xeceff4),
            ink: h(0xeceff4),
            subtext: h(0xd8dee9),
            muted: mx(0xd8dee9, 0x4c566a, 40),
            accent: h(0x88c0d0),
            accent2: h(0x8fbcbb),
            code: h(0xebcb8b),
            code_bg: h(0x434c5e),
            block_bg: h(0x3b4252),
            red: h(0xbf616a),
            yellow: h(0xebcb8b),
            green: h(0xa3be8c),
            blue: h(0x81a1c1),
            purple: h(0xb48ead),
            selection: h(0x434c5e),
            syntax: SyntaxColors {
                comment: mx(0x4c566a, 0xd8dee9, 35),
                ..syntax(0x81a1c1, 0xa3be8c, 0xb48ead, 0x4c566a, 0x88c0d0, 0x8fbcbb)
            },
        },
    },
    // ---- Rosé Pine (rose-pine/palette): base / surface / overlay, text / subtle / muted
    PaletteTheme {
        id: "rose-pine",
        name: "Rosé Pine",
        dark: true,
        palette: Palette {
            bg: h(0x191724),
            bg_alt: h(0x1f1d2e),
            surface: h(0x26233a),
            overlay: mx(0x26233a, 0x6e6a86, 35),
            text: h(0xe0def4),
            ink: h(0xe0def4),
            subtext: mx(0xe0def4, 0x908caa, 50),
            muted: h(0x908caa),
            accent: h(0x9ccfd8),
            accent2: h(0xc4a7e7),
            code: h(0xebbcba),
            code_bg: h(0x26233a),
            block_bg: h(0x1f1d2e),
            red: h(0xeb6f92),
            yellow: h(0xf6c177),
            green: h(0x9ccfd8),
            blue: h(0x9ccfd8),
            purple: h(0xc4a7e7),
            selection: mx(0x26233a, 0x6e6a86, 35),
            syntax: syntax(0x31748f, 0xf6c177, 0xebbcba, 0x6e6a86, 0xebbcba, 0x9ccfd8),
        },
    },
    PaletteTheme {
        id: "rose-pine-moon",
        name: "Rosé Pine Moon",
        dark: true,
        palette: Palette {
            bg: h(0x232136),
            bg_alt: h(0x2a273f),
            surface: h(0x393552),
            overlay: mx(0x393552, 0x6e6a86, 35),
            text: h(0xe0def4),
            ink: h(0xe0def4),
            subtext: mx(0xe0def4, 0x908caa, 50),
            muted: h(0x908caa),
            accent: h(0x9ccfd8),
            accent2: h(0xc4a7e7),
            code: h(0xea9a97),
            code_bg: h(0x393552),
            block_bg: h(0x2a273f),
            red: h(0xeb6f92),
            yellow: h(0xf6c177),
            green: h(0x9ccfd8),
            blue: h(0x9ccfd8),
            purple: h(0xc4a7e7),
            selection: mx(0x393552, 0x6e6a86, 35),
            syntax: syntax(0x3e8fb0, 0xf6c177, 0xea9a97, 0x6e6a86, 0xea9a97, 0x9ccfd8),
        },
    },
    PaletteTheme {
        id: "rose-pine-dawn",
        name: "Rosé Pine Dawn",
        dark: false,
        palette: Palette {
            bg: h(0xfaf4ed),
            bg_alt: h(0xfffaf3),
            surface: h(0xf2e9e1),
            overlay: mx(0xf2e9e1, 0x9893a5, 35),
            text: h(0x464261),
            ink: h(0x464261),
            subtext: mx(0x464261, 0x797593, 40),
            muted: h(0x797593),
            accent: h(0x286983),
            accent2: h(0x907aa9),
            code: mx(0xb4637a, 0x464261, 30),
            code_bg: h(0xfffaf3),
            block_bg: h(0xfffaf3),
            red: h(0xb4637a),
            yellow: h(0xea9d34),
            green: h(0x56949f),
            blue: h(0x286983),
            purple: h(0x907aa9),
            selection: mx(0xf2e9e1, 0x9893a5, 35),
            syntax: syntax(0x286983, 0xea9d34, 0xd7827e, 0x9893a5, 0xd7827e, 0x56949f),
        },
    },
    // ---- Tokyo Night (enkia/tokyo-night-vscode-theme; Day: folke/tokyonight.nvim)
    PaletteTheme {
        id: "tokyo-night",
        name: "Tokyo Night",
        dark: true,
        palette: Palette {
            bg: h(0x1a1b26),
            bg_alt: h(0x16161e),
            surface: h(0x24283b),
            overlay: h(0x414868),
            text: h(0xc0caf5),
            ink: h(0xc0caf5),
            subtext: h(0xa9b1d6),
            muted: h(0x9aa5ce),
            accent: h(0x7aa2f7),
            accent2: h(0x7dcfff),
            code: h(0x7dcfff),
            code_bg: h(0x24283b),
            block_bg: h(0x16161e),
            red: h(0xf7768e),
            yellow: h(0xe0af68),
            green: h(0x9ece6a),
            blue: h(0x7aa2f7),
            purple: h(0xbb9af7),
            selection: h(0x283457),
            syntax: syntax(0xbb9af7, 0x9ece6a, 0xff9e64, 0x565f89, 0x7aa2f7, 0x2ac3de),
        },
    },
    PaletteTheme {
        id: "tokyo-night-storm",
        name: "Tokyo Night Storm",
        dark: true,
        palette: Palette {
            bg: h(0x24283b),
            bg_alt: h(0x1f2335),
            surface: h(0x292e42),
            overlay: h(0x414868),
            text: h(0xc0caf5),
            ink: h(0xc0caf5),
            subtext: h(0xa9b1d6),
            muted: h(0x9aa5ce),
            accent: h(0x7aa2f7),
            accent2: h(0x7dcfff),
            code: h(0x7dcfff),
            code_bg: h(0x292e42),
            block_bg: h(0x1f2335),
            red: h(0xf7768e),
            yellow: h(0xe0af68),
            green: h(0x9ece6a),
            blue: h(0x7aa2f7),
            purple: h(0xbb9af7),
            selection: h(0x2e3c64),
            syntax: syntax(0xbb9af7, 0x9ece6a, 0xff9e64, 0x565f89, 0x7aa2f7, 0x2ac3de),
        },
    },
    PaletteTheme {
        id: "tokyo-night-day",
        name: "Tokyo Night Day",
        dark: false,
        palette: Palette {
            bg: h(0xe1e2e7),
            bg_alt: h(0xd0d5e3),
            surface: h(0xc4c8da),
            overlay: h(0xa8aecb),
            text: h(0x3760bf),
            ink: h(0x2e5857),
            subtext: h(0x3760bf),
            muted: h(0x68709a),
            accent: h(0x006a83),
            accent2: h(0x7847bd),
            code: h(0x006a83),
            code_bg: mx(0xe1e2e7, 0xd0d5e3, 30),
            block_bg: h(0xe1e2e7),
            red: h(0xc64343),
            yellow: h(0x8c6c3e),
            green: h(0x587539),
            blue: h(0x2e7de9),
            purple: h(0x7847bd),
            selection: h(0xb7c1e3),
            syntax: syntax(0x9854f1, 0x587539, 0xb15c00, 0x848cb5, 0x2e7de9, 0x188092),
        },
    },
];

/// The palette theme with this id.
pub fn palette_theme(id: &str) -> Option<&'static PaletteTheme> {
    PALETTE_THEMES.iter().find(|t| t.id == id)
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let ch = |i: usize| (a.0[i] as f32 + (b.0[i] as f32 - a.0[i] as f32) * t).round() as u8;
    Rgba([ch(0), ch(1), ch(2), 255])
}

/// Markise's mapping of a palette to the theme slots (original design): quiet page, body text
/// from the palette's text role, bright headings with an accent on h3/h4, links in the primary
/// accent, code on the raised surface, accent quote bar, palette hues for alerts and markers.
pub fn build(name: &str, dark: bool, p: &Palette) -> Theme {
    let base = if dark { Theme::dark() } else { Theme::light() };
    let head = |size: f32, color: Rgba, top: f32, bottom: f32, rule: Option<Rgba>| HeadingStyle {
        size,
        color,
        bold: true,
        margin_top: top,
        margin_bottom: bottom,
        rule,
    };
    let strong_heading = if dark {
        mix(p.text, Rgba::hex(0xffffff), 0.25)
    } else {
        mix(p.text, Rgba::hex(0x000000), 0.15)
    };
    let rule = mix(p.overlay, p.bg, 0.35);
    // `mark`: a tint of the palette's yellow under body text, or — when that is not legible
    // (AA) — the full yellow under page-colored text; whichever contrasts better
    let tint = mix(p.yellow, p.bg, if dark { 0.68 } else { 0.78 });
    let candidates = [(tint, p.text), (tint, p.ink), (p.yellow, p.bg)];
    let (mark_bg, ink) = candidates
        .iter()
        .copied()
        .find(|(bg, fg)| fg.contrast(*bg) >= 4.5)
        .unwrap_or_else(|| {
            candidates
                .iter()
                .copied()
                .max_by(|a, b| a.1.contrast(a.0).total_cmp(&b.1.contrast(b.0)))
                .expect("candidates")
        });
    let mark_text = (ink != p.text).then_some(ink);
    Theme {
        name: name.to_string(),
        dark,
        page_bg: p.bg,
        text: p.text,
        headings: [
            head(32.0, strong_heading, 22.0, 10.0, Some(rule)),
            head(25.0, strong_heading, 20.0, 8.0, Some(rule)),
            head(20.5, p.accent2, 16.0, 6.0, None),
            head(17.5, p.accent2, 14.0, 4.0, None),
            head(16.0, p.text, 12.0, 4.0, None),
            head(15.0, p.subtext, 12.0, 4.0, None),
        ],
        link: p.accent,
        inline_code: BoxStyle {
            background: p.code_bg,
            text: p.code,
            border: None,
            padding: 2.0,
            radius: 3.0,
        },
        code_block: BoxStyle {
            background: p.block_bg,
            text: p.text,
            border: Some(mix(p.overlay, p.block_bg, 0.5)),
            padding: 12.0,
            radius: 6.0,
        },
        math_block: BoxStyle {
            background: p.block_bg,
            text: p.purple,
            border: Some(mix(p.overlay, p.block_bg, 0.5)),
            padding: 10.0,
            radius: 6.0,
        },
        meta_block: BoxStyle {
            background: p.block_bg,
            text: p.muted,
            border: None,
            padding: 8.0,
            radius: 4.0,
        },
        quote_bar: p.accent2,
        quote_text: p.subtext,
        list_marker: p.accent2,
        checkbox_border: p.overlay,
        checkbox_checked: p.green,
        rule,
        table_border: p.overlay,
        table_header_bg: p.surface,
        table_stripe_bg: mix(p.bg, p.bg_alt, 0.6),
        highlight_bg: mark_bg,
        meta_text: p.muted,
        alert: [p.blue, p.green, p.purple, p.yellow, p.red],
        image_placeholder: p.surface,
        syntax: p.syntax,
        chrome: ChromeColors {
            sidebar_bg: p.bg_alt,
            control_text: p.subtext,
            active_bg: p.surface,
            selection_bg: p.selection,
            accent: p.accent,
        },
        fonts: ThemeFonts::default(),
        selection_text: Some(p.text),
        sidebar: SidebarStyle {
            text: Some(p.subtext),
            active_text: Some(p.text),
            active_bg: Some(p.surface),
            hover_bg: Some(mix(p.bg_alt, p.surface, 0.5)),
            outline_active_text: Some(p.accent),
            icon: Some(p.accent2),
            row_height: Some(24.0),
            padding_x: Some(6.0),
            indent: Some(14.0),
            radius: Some(4.0),
            ..SidebarStyle::default()
        },
        inline: InlineColors {
            mark_text,
            ..InlineColors::default()
        },
        ..base
    }
}

/// The theme for a palette id.
pub fn palette(id: &str) -> Option<Theme> {
    palette_theme(id).map(|t| build(t.name, t.dark, &t.palette))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_reference_existing_themes_with_matching_appearance() {
        for f in PALETTE_FAMILIES {
            if let Some(l) = f.light {
                assert!(!palette_theme(l).unwrap().dark, "{l}");
            }
            if let Some(d) = f.dark {
                assert!(palette_theme(d).unwrap().dark, "{d}");
            }
        }
        // every theme is reachable from the menu (Markise Night has its own entry)
        for t in PALETTE_THEMES {
            let listed = PALETTE_FAMILIES
                .iter()
                .any(|f| f.light == Some(t.id) || f.dark == Some(t.id));
            assert!(listed || t.id == "markise-night", "{}", t.id);
        }
    }

    /// WCAG AA (4.5:1) for body text, links and code text in every palette theme.
    #[test]
    fn contrast_meets_wcag_aa() {
        let mut report = String::new();
        let mut failures = Vec::new();
        for pt in PALETTE_THEMES {
            let t = palette(pt.id).unwrap();
            let checks = [
                ("text", t.text, t.page_bg),
                ("link", t.link, t.page_bg),
                ("inline code", t.inline_code.text, t.inline_code.background),
                ("code block", t.code_block.text, t.code_block.background),
                ("quote", t.quote_text, t.page_bg),
                ("mark", t.inline.mark_text.unwrap_or(t.text), t.highlight_bg),
            ];
            report.push_str(&format!("{:<22}", pt.name));
            for (what, fg, bg) in checks {
                let c = fg.contrast(bg);
                report.push_str(&format!(" {what} {c:5.2}"));
                if c < 4.5 {
                    failures.push(format!("{} {what}: {c:.2}", pt.name));
                }
            }
            report.push('\n');
        }
        println!("{report}");
        assert!(failures.is_empty(), "below 4.5:1: {failures:?}\n{report}");
        // the luma rule agrees with each theme's appearance (spec 03 §1.4)
        for pt in PALETTE_THEMES {
            assert_eq!(palette(pt.id).unwrap().page_bg.brightness() < 0.5, pt.dark, "{}", pt.id);
        }
    }

    #[test]
    fn contrast_formula() {
        assert!((Rgba::hex(0x000000).contrast(Rgba::hex(0xffffff)) - 21.0).abs() < 0.01);
        assert!((Rgba::hex(0x777777).contrast(Rgba::hex(0xffffff)) - 4.48).abs() < 0.02);
    }
}
