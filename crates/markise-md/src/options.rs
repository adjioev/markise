//! Parse and write options (spec 01 §5).

use crate::text::LineEnding;

/// Options that influence parsing (spec 01 §5). Defaults follow the spec defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseOptions {
    /// Strict mode: ATX heading needs a space after the hashes (R3.2.1, R6.1).
    pub strict: bool,
    /// `$…$`, `$$…$$` inline math (§4.2). Default on.
    pub inline_math: bool,
    /// Looser "legacy/compatible" inline math rule (R4.2.2). Default off.
    pub legacy_inline_math: bool,
    /// LaTeX delimiters `\(…\)` and `\[…\]` (R3.7.2). Default on.
    pub latex_delimiters: bool,
    /// ```` ```math ```` fences render as display math (R3.7.4). Default on.
    pub code_math: bool,
    /// `~x~` subscript. Default off.
    pub subscript: bool,
    /// `^x^` superscript. Default off.
    pub superscript: bool,
    /// `==x==` highlight. Default off.
    pub highlight: bool,
    /// GitHub alerts `> [!NOTE]` (R3.3.4). Default on.
    pub alerts: bool,
    /// Diagram fences `mermaid`/`sequence`/`flow` (§4.6). Default on (owner, 2026-10-27, DG-2).
    pub diagrams: bool,
    /// Bare URL autolink detection (display only). Default on.
    pub auto_links: bool,
    /// Remap typographic look-alikes to Markdown syntax when parsing (R4.5.2). Default off.
    pub remap_unicode_punctuation: bool,
    /// Ordered list interrupting a paragraph must start at 1 (R3.4.8). Default off.
    pub strict_number_start: bool,
    /// Parse block-level HTML (R3.14.4). Default on.
    pub block_html: bool,
    /// Emoji shortcodes `:smile:`. Default on.
    pub emoji: bool,
    /// Smart punctuation applied at render time (R4.5.1 "convert on rendering"). Default off.
    pub smart_punctuation_render: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            strict: false,
            inline_math: true,
            legacy_inline_math: false,
            latex_delimiters: true,
            code_math: true,
            subscript: false,
            superscript: false,
            highlight: false,
            alerts: true,
            diagrams: true,
            auto_links: true,
            remap_unicode_punctuation: false,
            strict_number_start: false,
            block_html: true,
            emoji: true,
            smart_punctuation_render: false,
        }
    }
}

/// Heading style for newly created headings (R3.2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HeadingStyle {
    #[default]
    Atx,
    AtxClosed,
    Setext,
    VariableSetext,
}

/// Numbering style for new ordered lists (R3.4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OrderedStyle {
    #[default]
    Incrementing,
    Repeated,
}

/// Continuation indent for new lists/quotes (§5 "Indent Size on Save").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentSize {
    Spaces(u8),
    Tab,
}

impl Default for IndentSize {
    fn default() -> Self {
        IndentSize::Spaces(2)
    }
}

/// Preferences that apply only to blocks created fresh in the editor (R1.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOptions {
    pub heading_style: HeadingStyle,
    /// `-`, `+` or `*`.
    pub unordered_bullet: char,
    pub ordered_style: OrderedStyle,
    pub indent_size: IndentSize,
    pub pretty_indentation: bool,
    /// Used when the opened text has no line ending to detect (R2.2).
    pub default_eol: LineEnding,
    /// Final newline for new/empty documents (R2.3). Default off.
    pub prefer_final_newline: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            heading_style: HeadingStyle::Atx,
            unordered_bullet: '-',
            ordered_style: OrderedStyle::Incrementing,
            indent_size: IndentSize::Spaces(2),
            pretty_indentation: false,
            default_eol: LineEnding::Lf,
            prefer_final_newline: false,
        }
    }
}

impl WriteOptions {
    /// Number of columns for the configured indent size (Tab counts as 4 here).
    pub fn indent_cols(&self) -> usize {
        match self.indent_size {
            IndentSize::Spaces(n) => n as usize,
            IndentSize::Tab => 4,
        }
    }
}
