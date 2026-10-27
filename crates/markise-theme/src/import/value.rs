//! Property values: `var()` substitution, shorthand expansion and value parsing
//! (spec 03 §2.1 property table).

use std::collections::HashMap;

use cssparser::{Parser, Token};

use crate::Rgba;

/// Longhand properties the importer understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum P {
    Color,
    Bg,
    FontSize,
    Family,
    Weight,
    Italic,
    LineHeight,
    MarginT,
    MarginR,
    MarginB,
    MarginL,
    PadT,
    PadR,
    PadB,
    PadL,
    BwT,
    BwR,
    BwB,
    BwL,
    BsT,
    BsR,
    BsB,
    BsL,
    BcT,
    BcR,
    BcB,
    BcL,
    Radius,
    MaxWidth,
    Width,
    Align,
    Deco,
    Transform,
    Letter,
    ListStyle,
    Opacity,
    Content,
    Display,
}

pub(crate) const NP: usize = P::Display as usize + 1;

impl P {
    pub(crate) fn idx(self) -> usize {
        self as usize
    }
}

const MARGIN: [P; 4] = [P::MarginT, P::MarginR, P::MarginB, P::MarginL];
const PAD: [P; 4] = [P::PadT, P::PadR, P::PadB, P::PadL];
pub(crate) const BW: [P; 4] = [P::BwT, P::BwR, P::BwB, P::BwL];
pub(crate) const BS: [P; 4] = [P::BsT, P::BsR, P::BsB, P::BsL];
pub(crate) const BC: [P; 4] = [P::BcT, P::BcR, P::BcB, P::BcL];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Col {
    Rgba(Rgba),
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Len {
    Px(f32),
    Em(f32),
    Rem(f32),
    Pct(f32),
    Vw(f32),
    Vh(f32),
    Num(f32),
    Auto,
    None,
    Normal,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum V {
    Inherit,
    Initial,
    Color(Col),
    Len(Len),
    Families(Vec<String>),
    /// Font weight 1–1000; `Bolder`/`Lighter` relative.
    Weight(u16),
    Bolder,
    Lighter,
    Bool(bool),
    /// Keyword value (lowercase): text-align, text-transform, list-style-type, border style, display.
    Kw(String),
    /// `text-decoration`: (underline, line-through)
    Deco(bool, bool),
    /// `content` text; `None` for `none`/`normal`.
    Text(Option<String>),
}

/// Classification of a property name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PropClass {
    Supported,
    /// Known and deliberately ignored (spec 03 §2.1 IGNORE and the MAY layout properties).
    Ignored,
    Unsupported,
}

pub(crate) fn classify(name: &str) -> PropClass {
    if name.starts_with("--") {
        return PropClass::Supported;
    }
    match name {
        "color"
        | "background-color"
        | "background"
        | "font-size"
        | "font-weight"
        | "font-family"
        | "font-style"
        | "font"
        | "line-height"
        | "margin"
        | "margin-top"
        | "margin-right"
        | "margin-bottom"
        | "margin-left"
        | "padding"
        | "padding-top"
        | "padding-right"
        | "padding-bottom"
        | "padding-left"
        | "border"
        | "border-top"
        | "border-right"
        | "border-bottom"
        | "border-left"
        | "border-width"
        | "border-style"
        | "border-color"
        | "border-top-width"
        | "border-right-width"
        | "border-bottom-width"
        | "border-left-width"
        | "border-top-style"
        | "border-right-style"
        | "border-bottom-style"
        | "border-left-style"
        | "border-top-color"
        | "border-right-color"
        | "border-bottom-color"
        | "border-left-color"
        | "border-radius"
        | "max-width"
        | "width"
        | "text-align"
        | "text-decoration"
        | "text-decoration-line"
        | "text-transform"
        | "letter-spacing"
        | "list-style"
        | "list-style-type"
        | "opacity"
        | "content"
        | "display"
        | "margin-block"
        | "margin-inline"
        | "padding-block"
        | "padding-inline"
        | "margin-block-start"
        | "margin-block-end"
        | "margin-inline-start"
        | "margin-inline-end"
        | "padding-inline-start"
        | "padding-inline-end"
        | "padding-block-start"
        | "padding-block-end"
        | "border-inline-start"
        | "border-left-radius" => PropClass::Supported,
        "height"
        | "min-height"
        | "max-height"
        | "min-width"
        | "box-shadow"
        | "vertical-align"
        | "position"
        | "top"
        | "left"
        | "right"
        | "bottom"
        | "z-index"
        | "float"
        | "clear"
        | "overflow"
        | "overflow-x"
        | "overflow-y"
        | "cursor"
        | "transition"
        | "transition-duration"
        | "transition-property"
        | "transition-timing-function"
        | "transition-delay"
        | "outline"
        | "outline-offset"
        | "outline-color"
        | "outline-style"
        | "outline-width"
        | "appearance"
        | "user-select"
        | "pointer-events"
        | "box-sizing"
        | "text-shadow"
        | "white-space"
        | "word-wrap"
        | "word-break"
        | "overflow-wrap"
        | "list-style-position"
        | "list-style-image"
        | "visibility"
        | "flex"
        | "flex-direction"
        | "flex-wrap"
        | "flex-grow"
        | "flex-shrink"
        | "flex-basis"
        | "align-items"
        | "justify-content"
        | "align-self"
        | "align-content"
        | "gap"
        | "grid-template-columns"
        | "order"
        | "animation"
        | "animation-name"
        | "animation-duration"
        | "will-change"
        | "text-rendering"
        | "font-smoothing"
        | "text-size-adjust"
        | "zoom"
        | "filter"
        | "backdrop-filter"
        | "counter-reset"
        | "counter-increment"
        | "quotes"
        | "text-indent"
        | "tab-size"
        | "hyphens"
        | "page-break-before"
        | "page-break-after"
        | "page-break-inside"
        | "break-before"
        | "break-after"
        | "break-inside"
        | "orphans"
        | "widows"
        | "background-image"
        | "background-size"
        | "background-repeat"
        | "background-position"
        | "background-clip"
        | "background-attachment"
        | "background-origin"
        | "border-collapse"
        | "border-spacing"
        | "table-layout"
        | "caret-color"
        | "scroll-behavior"
        | "scrollbar-width"
        | "scrollbar-color"
        | "resize"
        | "content-visibility"
        | "font-display"
        | "src"
        | "unicode-range"
        | "font-variant-ligatures"
        | "font-feature-settings"
        | "font-variant"
        | "text-decoration-style"
        | "text-decoration-color"
        | "text-decoration-thickness"
        | "text-underline-offset"
        | "word-spacing"
        | "inset"
        | "isolation"
        | "mix-blend-mode"
        | "object-fit"
        | "border-image"
        | "direction"
        | "unicode-bidi"
        | "mask"
        | "text-overflow"
        | "line-break"
        | "font-stretch"
        | "text-emphasis"
        | "fill"
        | "stroke"
        | "stroke-width"
        | "accent-color"
        | "color-scheme"
        | "aspect-ratio"
        | "contain"
        | "touch-action"
        | "print-color-adjust"
        | "image-rendering"
        | "border-top-left-radius"
        | "border-top-right-radius"
        | "border-bottom-left-radius"
        | "border-bottom-right-radius"
        | "text-align-last"
        | "column-count"
        | "column-gap"
        | "columns" => PropClass::Ignored,
        _ if name.starts_with('-') => PropClass::Ignored, // vendor-prefixed (IGNORE row)
        _ => PropClass::Unsupported,
    }
}

// ------------------------------------------------------------------------------------------
// var() substitution

/// Limits for variable substitution (bounded work on adversarial input).
const MAX_VAR_DEPTH: u32 = 24;
/// Maximum `var()` expansions (and nested blocks visited) per substituted value.
const MAX_VAR_EXPANSIONS: u32 = 4096;
pub(crate) const MAX_VALUE_LEN: usize = 16 * 1024;

/// Substitute every `var(--x[, fallback])` in `value` using `lookup`. Returns `None` when a
/// referenced variable is missing without fallback, a cycle is detected, or limits are hit
/// (the declaration is then "invalid at computed-value time").
pub(crate) fn substitute(value: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if !has_var(value) {
        return Some(value.to_string());
    }
    let mut stack = Vec::new();
    let mut budget = MAX_VAR_EXPANSIONS;
    subst_rec(value, lookup, &mut stack, 0, &mut budget)
}

pub(crate) fn has_var(s: &str) -> bool {
    s.len() >= 4 && s.as_bytes().windows(4).any(|w| w.eq_ignore_ascii_case(b"var("))
}

fn subst_rec(
    value: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    stack: &mut Vec<String>,
    depth: u32,
    budget: &mut u32,
) -> Option<String> {
    if depth > MAX_VAR_DEPTH || *budget == 0 {
        return None;
    }
    *budget -= 1;
    if !has_var(value) {
        return Some(value.to_string());
    }
    let mut out = String::with_capacity(value.len());
    let mut p = Parser::new(value);
    let mut copied = p.position();
    loop {
        let before = p.position();
        let tok = match p.next_including_whitespace_and_comments() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        match tok {
            Token::Function(ref name) if name.eq_ignore_ascii_case("var") => {
                out.push_str(p.slice(copied..before));
                let parsed: Result<(String, Option<String>), cssparser::ParseError<()>> = p.parse_nested_block(|p| {
                    let name = p.expect_ident()?.to_string();
                    let fallback = if p.try_parse(|p| p.expect_comma()).is_ok() {
                        let start = p.position();
                        while p.next_including_whitespace_and_comments().is_ok() {}
                        Some(p.slice_from(start).to_string())
                    } else {
                        if !p.is_exhausted() {
                            return Err(cssparser::ParseError::custom(()));
                        }
                        None
                    };
                    Ok((name, fallback))
                });
                let (name, fallback) = parsed.ok()?;
                let resolved = if stack.contains(&name) {
                    None // cycle
                } else {
                    lookup(&name).and_then(|raw| {
                        stack.push(name.clone());
                        let r = subst_rec(&raw, lookup, stack, depth + 1, budget);
                        stack.pop();
                        r
                    })
                };
                let text = match (resolved, fallback) {
                    (Some(v), _) => v,
                    (None, Some(fb)) => subst_rec(&fb, lookup, stack, depth + 1, budget)?,
                    (None, None) => return None,
                };
                out.push_str(&text);
                if out.len() > MAX_VALUE_LEN {
                    return None;
                }
                copied = p.position();
            }
            Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                let inner_has_var = {
                    let state = p.state();
                    let r: Result<bool, cssparser::ParseError<()>> = p.parse_nested_block(|p| {
                        let s = p.position();
                        while p.next_including_whitespace_and_comments().is_ok() {}
                        Ok(has_var(p.slice_from(s)))
                    });
                    p.reset(&state);
                    r.unwrap_or(false)
                };
                if inner_has_var {
                    // copy up to and including the opening token, substitute inside, close
                    out.push_str(p.slice(copied..p.position()));
                    let inner: Result<String, cssparser::ParseError<()>> = p.parse_nested_block(|p| {
                        let s = p.position();
                        while p.next_including_whitespace_and_comments().is_ok() {}
                        Ok(p.slice_from(s).to_string())
                    });
                    let inner = inner.ok()?;
                    out.push_str(&subst_rec(&inner, lookup, stack, depth + 1, budget)?);
                    out.push(match tok {
                        Token::SquareBracketBlock => ']',
                        Token::CurlyBracketBlock => '}',
                        _ => ')',
                    });
                    if out.len() > MAX_VALUE_LEN {
                        return None;
                    }
                    copied = p.position();
                }
            }
            _ => {}
        }
    }
    out.push_str(p.slice_from(copied));
    if out.len() > MAX_VALUE_LEN {
        return None;
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------
// value parsing

/// Outcome of expanding one declaration.
pub(crate) enum Expanded {
    Ok(Vec<(P, V)>),
    /// Supported property, value not understood (counts as unsupported value).
    BadValue,
    /// Property known but ignored.
    Ignored,
}

fn css_wide(v: &str) -> Option<V> {
    match v.to_ascii_lowercase().as_str() {
        "inherit" => Some(V::Inherit),
        "initial" | "unset" | "revert" | "revert-layer" => Some(V::Initial),
        _ => None,
    }
}

/// Expand a declaration (already var-substituted) into longhands.
pub(crate) fn expand(name: &str, value: &str) -> Expanded {
    let value = value.trim();
    if let Some(w) = css_wide(value) {
        return match longhands_of(name) {
            Some(ps) => Expanded::Ok(ps.iter().map(|p| (*p, w.clone())).collect()),
            None => Expanded::Ignored,
        };
    }
    let r: Option<Vec<(P, V)>> = match name {
        "color" => parse_color(value).map(|c| vec![(P::Color, V::Color(c))]),
        "background-color" => parse_color(value).map(|c| vec![(P::Bg, V::Color(c))]),
        "background" => parse_background(value).map(|c| vec![(P::Bg, V::Color(c))]),
        "font-size" => parse_font_size(value).map(|l| vec![(P::FontSize, V::Len(l))]),
        "font-weight" => parse_weight(value).map(|w| vec![(P::Weight, w)]),
        "font-family" => parse_families(value).map(|f| vec![(P::Family, V::Families(f))]),
        "font-style" => parse_font_style(value).map(|b| vec![(P::Italic, V::Bool(b))]),
        "font" => parse_font_shorthand(value),
        "line-height" => parse_line_height(value).map(|l| vec![(P::LineHeight, V::Len(l))]),
        "margin" => box4(value, MARGIN, true),
        "padding" => box4(value, PAD, false),
        "margin-block" => box2(value, [P::MarginT, P::MarginB], true),
        "margin-inline" => box2(value, [P::MarginL, P::MarginR], true),
        "padding-block" => box2(value, [P::PadT, P::PadB], false),
        "padding-inline" => box2(value, [P::PadL, P::PadR], false),
        "margin-top" | "margin-block-start" => one_len(value, P::MarginT, true),
        "margin-right" | "margin-inline-end" => one_len(value, P::MarginR, true),
        "margin-bottom" | "margin-block-end" => one_len(value, P::MarginB, true),
        "margin-left" | "margin-inline-start" => one_len(value, P::MarginL, true),
        "padding-top" | "padding-block-start" => one_len(value, P::PadT, false),
        "padding-right" | "padding-inline-end" => one_len(value, P::PadR, false),
        "padding-bottom" | "padding-block-end" => one_len(value, P::PadB, false),
        "padding-left" | "padding-inline-start" => one_len(value, P::PadL, false),
        "border" => parse_border(value).map(|(w, s, c)| {
            let mut v = Vec::new();
            for i in 0..4 {
                v.push((BW[i], V::Len(w)));
                v.push((BS[i], V::Kw(s.clone())));
                v.push((BC[i], V::Color(c)));
            }
            v
        }),
        "border-top" | "border-right" | "border-bottom" | "border-left" | "border-inline-start" => {
            let i = match name {
                "border-top" => 0,
                "border-right" => 1,
                "border-bottom" => 2,
                _ => 3,
            };
            parse_border(value).map(|(w, s, c)| vec![(BW[i], V::Len(w)), (BS[i], V::Kw(s)), (BC[i], V::Color(c))])
        }
        "border-width" => box4_with(value, BW, |t| parse_border_width(t).map(V::Len)),
        "border-style" => box4_with(value, BS, |t| parse_border_style(t).map(V::Kw)),
        "border-color" => box4_with(value, BC, |t| parse_color(t).map(V::Color)),
        _ if name.starts_with("border-") && name.ends_with("-width") => {
            side_of(name).and_then(|i| parse_border_width(value).map(|l| vec![(BW[i], V::Len(l))]))
        }
        _ if name.starts_with("border-") && name.ends_with("-style") => {
            side_of(name).and_then(|i| parse_border_style(value).map(|k| vec![(BS[i], V::Kw(k))]))
        }
        _ if name.starts_with("border-") && name.ends_with("-color") => {
            side_of(name).and_then(|i| parse_color(value).map(|c| vec![(BC[i], V::Color(c))]))
        }
        "border-radius" => first_token(value)
            .and_then(|t| parse_len(&t, false))
            .map(|l| vec![(P::Radius, V::Len(l))]),
        "max-width" => parse_len_kw(value, false).map(|l| vec![(P::MaxWidth, V::Len(l))]),
        "width" => match value.trim().to_ascii_lowercase().as_str() {
            // intrinsic widths: the box hugs its content (heading "pills")
            "fit-content" | "max-content" | "min-content" | "-webkit-fit-content" | "-moz-fit-content" => {
                Some(vec![(P::Width, V::Kw("fit-content".into()))])
            }
            _ => parse_len_kw(value, false).map(|l| vec![(P::Width, V::Len(l))]),
        },
        "text-align" => keyword(
            value,
            &[
                "left",
                "right",
                "center",
                "justify",
                "start",
                "end",
                "-webkit-center",
                "match-parent",
            ],
        )
        .map(|k| vec![(P::Align, V::Kw(k))]),
        "text-transform" => keyword(value, &["none", "uppercase", "lowercase", "capitalize", "full-width"])
            .map(|k| vec![(P::Transform, V::Kw(k))]),
        "text-decoration" | "text-decoration-line" => {
            parse_decoration(value).map(|(u, l)| vec![(P::Deco, V::Deco(u, l))])
        }
        "letter-spacing" => parse_len_kw(value, true).map(|l| vec![(P::Letter, V::Len(l))]),
        "list-style-type" => parse_list_style_type(value).map(|k| vec![(P::ListStyle, V::Kw(k))]),
        "list-style" => Some(
            value
                .split_ascii_whitespace()
                .find_map(parse_list_style_type)
                .map(|k| vec![(P::ListStyle, V::Kw(k))])
                .unwrap_or_default(),
        ),
        "opacity" => parse_number_or_pct(value).map(|n| vec![(P::Opacity, V::Len(Len::Num(n.clamp(0.0, 1.0))))]),
        "content" => parse_content(value).map(|t| vec![(P::Content, V::Text(t))]),
        "display" => keyword_any(value).map(|k| vec![(P::Display, V::Kw(k))]),
        _ => return Expanded::Ignored,
    };
    match r {
        Some(v) => Expanded::Ok(v),
        None => Expanded::BadValue,
    }
}

fn longhands_of(name: &str) -> Option<Vec<P>> {
    Some(match name {
        "color" => vec![P::Color],
        "background-color" | "background" => vec![P::Bg],
        "font-size" => vec![P::FontSize],
        "font-weight" => vec![P::Weight],
        "font-family" => vec![P::Family],
        "font-style" => vec![P::Italic],
        "font" => vec![P::Italic, P::Weight, P::FontSize, P::LineHeight, P::Family],
        "line-height" => vec![P::LineHeight],
        "margin" => MARGIN.to_vec(),
        "padding" => PAD.to_vec(),
        "margin-top" => vec![P::MarginT],
        "margin-right" => vec![P::MarginR],
        "margin-bottom" => vec![P::MarginB],
        "margin-left" => vec![P::MarginL],
        "padding-top" => vec![P::PadT],
        "padding-right" => vec![P::PadR],
        "padding-bottom" => vec![P::PadB],
        "padding-left" => vec![P::PadL],
        "border" => [BW, BS, BC].concat(),
        "border-color" => BC.to_vec(),
        "border-width" => BW.to_vec(),
        "border-style" => BS.to_vec(),
        "border-top" => vec![P::BwT, P::BsT, P::BcT],
        "border-right" => vec![P::BwR, P::BsR, P::BcR],
        "border-bottom" => vec![P::BwB, P::BsB, P::BcB],
        "border-left" => vec![P::BwL, P::BsL, P::BcL],
        "border-radius" => vec![P::Radius],
        "max-width" => vec![P::MaxWidth],
        "width" => vec![P::Width],
        "text-align" => vec![P::Align],
        "text-decoration" | "text-decoration-line" => vec![P::Deco],
        "text-transform" => vec![P::Transform],
        "letter-spacing" => vec![P::Letter],
        "list-style" | "list-style-type" => vec![P::ListStyle],
        "opacity" => vec![P::Opacity],
        "content" => vec![P::Content],
        "display" => vec![P::Display],
        _ => return None,
    })
}

fn side_of(name: &str) -> Option<usize> {
    let mid = name.strip_prefix("border-")?;
    Some(if mid.starts_with("top") {
        0
    } else if mid.starts_with("right") || mid.starts_with("inline-end") {
        1
    } else if mid.starts_with("bottom") {
        2
    } else if mid.starts_with("left") || mid.starts_with("inline-start") {
        3
    } else {
        return None;
    })
}

/// Split a value into top-level whitespace-separated components (functions kept whole).
pub(crate) fn components(value: &str) -> Vec<String> {
    components_slow(value)
}

fn components_slow(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = Parser::new(value);
    loop {
        let start = p.position();
        let tok = match p.next() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        match tok {
            Token::Comma => out.push(",".to_string()),
            Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                let _: Result<(), cssparser::ParseError<()>> = p.parse_nested_block(|p| {
                    while p.next().is_ok() {}
                    Ok(())
                });
                out.push(p.slice_from(start).to_string());
            }
            _ => out.push(p.slice_from(start).to_string()),
        }
    }
    out
}

fn first_token(value: &str) -> Option<String> {
    components(value).into_iter().next()
}

pub(crate) fn parse_color(value: &str) -> Option<Col> {
    let v = value.trim();
    let mut p = Parser::new(v);
    let c = parse_color_tokens(&mut p)?;
    if !p.is_exhausted() {
        return None;
    }
    Some(c)
}

fn parse_color_tokens(p: &mut Parser) -> Option<Col> {
    let tok = p.next().ok()?.clone();
    match tok {
        Token::Hash(h) | Token::IDHash(h) => {
            let (r, g, b, a) = cssparser::color::parse_hash_color(h.as_bytes()).ok()?;
            Some(Col::Rgba(Rgba([r, g, b, (a * 255.0).round().clamp(0.0, 255.0) as u8])))
        }
        Token::Ident(name) => {
            let lower = name.to_ascii_lowercase();
            match lower.as_str() {
                "transparent" => Some(Col::Rgba(Rgba::TRANSPARENT)),
                "currentcolor" => Some(Col::Current),
                _ => cssparser::color::parse_named_color(&lower)
                    .ok()
                    .map(|(r, g, b)| Col::Rgba(Rgba([r, g, b, 255])))
                    .or_else(|| system_color(&lower)),
            }
        }
        Token::Function(name) => {
            let lower = name.to_ascii_lowercase();
            if lower == "color-mix" {
                return p
                    .parse_nested_block(|p| Ok::<_, cssparser::ParseError<()>>(color_mix(p)))
                    .ok()?;
            }
            let args: Vec<Arg> = p
                .parse_nested_block(|p| Ok::<_, cssparser::ParseError<()>>(color_args(p)))
                .ok()??;
            match lower.as_str() {
                "rgb" | "rgba" => rgb_from(&args),
                "hsl" | "hsla" => hsl_from(&args),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `color-mix(in <space>, <color> [<pct>], <color> [<pct>])` (CSS Color 5). Every
/// interpolation space is approximated by premultiplied sRGB mixing.
fn color_mix(p: &mut Parser) -> Option<Col> {
    p.expect_ident_matching("in").ok()?;
    p.expect_ident().ok()?;
    // optional hue interpolation method (`shorter hue`, …) up to the first comma
    loop {
        match p.next().ok()? {
            Token::Comma => break,
            Token::Ident(_) => {}
            _ => return None,
        }
    }
    let part = |p: &mut Parser| -> Option<(Rgba, Option<f32>)> {
        let mut pct = p.try_parse(|p| p.expect_percentage()).ok();
        let c = match parse_color_tokens(p)? {
            Col::Rgba(c) => c,
            Col::Current => return None,
        };
        if pct.is_none() {
            pct = p.try_parse(|p| p.expect_percentage()).ok();
        }
        Some((c, pct))
    };
    let (c1, p1) = part(p)?;
    p.expect_comma().ok()?;
    let (c2, p2) = part(p)?;
    if !p.is_exhausted() {
        return None;
    }
    let (p1, p2) = match (p1, p2) {
        (None, None) => (0.5, 0.5),
        (Some(a), None) => (a, 1.0 - a),
        (None, Some(b)) => (1.0 - b, b),
        (Some(a), Some(b)) => (a, b),
    };
    if !(0.0..=1.0).contains(&p1) || !(0.0..=1.0).contains(&p2) {
        return None;
    }
    let sum = p1 + p2;
    if sum <= 0.0 {
        return None;
    }
    let mult = sum.min(1.0);
    let (w1, w2) = (p1 / sum, p2 / sum);
    let a1 = c1.0[3] as f32 / 255.0;
    let a2 = c2.0[3] as f32 / 255.0;
    let a = a1 * w1 + a2 * w2;
    if a <= 0.0 {
        return Some(Col::Rgba(Rgba::TRANSPARENT));
    }
    let ch = |i: usize| {
        ((c1.0[i] as f32 * a1 * w1 + c2.0[i] as f32 * a2 * w2) / a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let alpha = (a * mult * 255.0).round().clamp(0.0, 255.0) as u8;
    Some(Col::Rgba(Rgba([ch(0), ch(1), ch(2), alpha])))
}

/// Approximate color of a gradient (`linear-gradient(…)`, `radial-gradient(…)`, …): the
/// average of its color stops, or `None` for mostly transparent patterns (grid lines, dots),
/// which are better ignored than painted as a tint.
fn gradient_color(comp: &str) -> Option<Rgba> {
    let open = comp.find('(')?;
    let name = comp[..open].trim().to_ascii_lowercase();
    if !name.ends_with("gradient") {
        return None;
    }
    let inner = comp[open + 1..].trim_end().strip_suffix(')')?;
    let comps = components(inner);
    let mut stops = Vec::new();
    for piece in comps.split(|c| c == ",") {
        if let Some(Col::Rgba(c)) = piece.iter().find_map(|c| parse_color(c)) {
            stops.push(c);
        }
        if stops.len() >= 64 {
            break;
        }
    }
    if stops.is_empty() || stops.iter().any(|c| c.0[3] < 128) {
        return None;
    }
    let n = stops.len() as u32;
    let avg = |i: usize| (stops.iter().map(|c| c.0[i] as u32).sum::<u32>() / n) as u8;
    Some(Rgba([avg(0), avg(1), avg(2), avg(3)]))
}

fn system_color(name: &str) -> Option<Col> {
    // CSS system colors (light defaults); themes occasionally use them.
    let rgb = match name {
        "canvas" | "window" | "field" | "buttonface" => 0xffffff,
        "canvastext" | "windowtext" | "fieldtext" | "buttontext" => 0x000000,
        "linktext" => 0x0000ee,
        "graytext" => 0x808080,
        "highlight" => 0xb4d5fe,
        _ => return None,
    };
    Some(Col::Rgba(Rgba::hex(rgb)))
}

#[derive(Clone, Copy, Debug)]
enum Arg {
    Num(f32),
    Pct(f32),
    Deg(f32),
    None,
}

fn color_args(p: &mut Parser) -> Option<Vec<Arg>> {
    let mut out = Vec::new();
    while let Ok(tok) = p.next().cloned() {
        match tok {
            Token::Number { value, .. } => out.push(Arg::Num(value)),
            Token::Percentage { unit_value, .. } => out.push(Arg::Pct(unit_value)),
            Token::Dimension { value, ref unit, .. } => {
                let deg = match unit.to_ascii_lowercase().as_str() {
                    "deg" => value,
                    "rad" => value.to_degrees(),
                    "grad" => value * 0.9,
                    "turn" => value * 360.0,
                    _ => return None,
                };
                out.push(Arg::Deg(deg));
            }
            Token::Ident(ref i) if i.eq_ignore_ascii_case("none") => out.push(Arg::None),
            Token::Comma | Token::Delim('/') => {}
            _ => return None,
        }
        if out.len() > 4 {
            return None;
        }
    }
    Some(out)
}

fn chan(a: Arg) -> Option<f32> {
    match a {
        Arg::Num(n) => Some(n.clamp(0.0, 255.0)),
        Arg::Pct(p) => Some((p * 255.0).clamp(0.0, 255.0)),
        Arg::None => Some(0.0),
        Arg::Deg(_) => None,
    }
}

fn alpha(a: Option<&Arg>) -> Option<u8> {
    Some(match a {
        None => 255,
        Some(Arg::Num(n)) => (n.clamp(0.0, 1.0) * 255.0).round() as u8,
        Some(Arg::Pct(p)) => (p.clamp(0.0, 1.0) * 255.0).round() as u8,
        Some(Arg::None) => 0,
        Some(Arg::Deg(_)) => return None,
    })
}

fn rgb_from(args: &[Arg]) -> Option<Col> {
    if args.len() < 3 {
        return None;
    }
    let r = chan(args[0])?.round() as u8;
    let g = chan(args[1])?.round() as u8;
    let b = chan(args[2])?.round() as u8;
    Some(Col::Rgba(Rgba([r, g, b, alpha(args.get(3))?])))
}

fn hsl_from(args: &[Arg]) -> Option<Col> {
    if args.len() < 3 {
        return None;
    }
    let h = match args[0] {
        Arg::Num(n) | Arg::Deg(n) => n,
        Arg::None => 0.0,
        Arg::Pct(_) => return None,
    };
    let pct = |a: Arg| match a {
        Arg::Pct(p) => Some(p.clamp(0.0, 1.0)),
        Arg::Num(n) => Some((n / 100.0).clamp(0.0, 1.0)),
        Arg::None => Some(0.0),
        Arg::Deg(_) => None,
    };
    let s = pct(args[1])?;
    let l = pct(args[2])?;
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Some(Col::Rgba(Rgba([
        f(h + 1.0 / 3.0),
        f(h),
        f(h - 1.0 / 3.0),
        alpha(args.get(3))?,
    ])))
}

/// `background` shorthand: the color component of the final layer; no color ⇒ transparent
/// (the shorthand resets `background-color`). Images and gradients are ignored (spec 03 §2.1).
fn parse_background(value: &str) -> Option<Col> {
    let comps = components(value);
    let last_layer: Vec<&String> = comps
        .rsplit(|c| c == ",")
        .next()
        .map(|l| l.iter().collect())
        .unwrap_or_default();
    for c in &last_layer {
        if let Some(col) = parse_color(c) {
            return Some(col);
        }
    }
    // no plain color: approximate a gradient by its average stop color (heading "pills")
    if let Some(c) = last_layer.iter().find_map(|c| gradient_color(c)) {
        return Some(Col::Rgba(c));
    }
    if last_layer.is_empty() {
        return None;
    }
    Some(Col::Rgba(Rgba::TRANSPARENT))
}

pub(crate) fn parse_len(t: &str, allow_neg: bool) -> Option<Len> {
    let mut p = Parser::new(t.trim());
    let l = match p.next().ok()?.clone() {
        Token::Number { value: 0.0, .. } => Len::Px(0.0),
        Token::Number { value, .. } => Len::Num(value),
        Token::Percentage { unit_value, .. } => Len::Pct(unit_value * 100.0),
        Token::Dimension { value, ref unit, .. } => dim(value, unit)?,
        Token::Ident(ref i) => match i.to_ascii_lowercase().as_str() {
            "auto" => Len::Auto,
            "none" => Len::None,
            "normal" => Len::Normal,
            "thin" => Len::Px(1.0),
            "medium" => Len::Px(3.0),
            "thick" => Len::Px(5.0),
            _ => return None,
        },
        _ => return None,
    };
    if !p.is_exhausted() {
        return None;
    }
    if !allow_neg
        && let Len::Px(v) | Len::Em(v) | Len::Rem(v) | Len::Pct(v) = l
        && v < 0.0
    {
        return None;
    }
    Some(l)
}

fn dim(value: f32, unit: &str) -> Option<Len> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "px" => Len::Px(value),
        "pt" => Len::Px(value * 4.0 / 3.0),
        "pc" => Len::Px(value * 16.0),
        "in" => Len::Px(value * 96.0),
        "cm" => Len::Px(value * 96.0 / 2.54),
        "mm" => Len::Px(value * 96.0 / 25.4),
        "q" => Len::Px(value * 96.0 / 101.6),
        "em" => Len::Em(value),
        "rem" => Len::Rem(value),
        "ex" | "ch" => Len::Em(value * 0.5),
        "vw" => Len::Vw(value),
        "vh" => Len::Vh(value),
        "vmin" => Len::Vh(value),
        "vmax" => Len::Vw(value),
        _ => return None,
    })
}

fn is_len(l: &Len) -> bool {
    !matches!(l, Len::Num(_) | Len::Normal)
}

fn parse_len_kw(value: &str, allow_neg: bool) -> Option<Len> {
    parse_len(value, allow_neg).filter(is_len_or_kw)
}

fn is_len_or_kw(l: &Len) -> bool {
    !matches!(l, Len::Num(_))
}

fn one_len(value: &str, p: P, allow_neg: bool) -> Option<Vec<(P, V)>> {
    parse_len(value, allow_neg).filter(is_len).map(|l| vec![(p, V::Len(l))])
}

fn box4(value: &str, props: [P; 4], allow_neg: bool) -> Option<Vec<(P, V)>> {
    box4_with(value, props, |t| parse_len(t, allow_neg).filter(is_len).map(V::Len))
}

fn box2(value: &str, props: [P; 2], allow_neg: bool) -> Option<Vec<(P, V)>> {
    let comps = components(value);
    let vals: Option<Vec<V>> = comps
        .iter()
        .map(|t| parse_len(t, allow_neg).filter(is_len).map(V::Len))
        .collect();
    let vals = vals?;
    let (a, b) = match vals.as_slice() {
        [a] => (a.clone(), a.clone()),
        [a, b] => (a.clone(), b.clone()),
        _ => return None,
    };
    Some(vec![(props[0], a), (props[1], b)])
}

fn box4_with(value: &str, props: [P; 4], f: impl Fn(&str) -> Option<V>) -> Option<Vec<(P, V)>> {
    let comps = components(value);
    let vals: Option<Vec<V>> = comps.iter().map(|t| f(t)).collect();
    let v = vals?;
    let (t, r, b, l) = match v.as_slice() {
        [a] => (a, a, a, a),
        [a, b] => (a, b, a, b),
        [a, b, c] => (a, b, c, b),
        [a, b, c, d] => (a, b, c, d),
        _ => return None,
    };
    Some(vec![
        (props[0], t.clone()),
        (props[1], r.clone()),
        (props[2], b.clone()),
        (props[3], l.clone()),
    ])
}

fn parse_border_width(t: &str) -> Option<Len> {
    parse_len(t, false).filter(|l| matches!(l, Len::Px(_) | Len::Em(_) | Len::Rem(_)))
}

fn parse_border_style(t: &str) -> Option<String> {
    let k = t.trim().to_ascii_lowercase();
    matches!(
        k.as_str(),
        "none" | "hidden" | "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset"
    )
    .then_some(k)
}

/// `border` shorthand: width, style, color in any order; omitted parts reset to initial
/// (medium, none, currentcolor).
fn parse_border(value: &str) -> Option<(Len, String, Col)> {
    let comps = components(value);
    if comps.is_empty() || comps.len() > 3 {
        return None;
    }
    let (mut w, mut s, mut c) = (None, None, None);
    for t in &comps {
        if w.is_none()
            && let Some(l) = parse_border_width(t)
        {
            w = Some(l);
        } else if s.is_none()
            && let Some(st) = parse_border_style(t)
        {
            s = Some(st);
        } else if c.is_none()
            && let Some(col) = parse_color(t)
        {
            c = Some(col);
        } else {
            return None;
        }
    }
    Some((
        w.unwrap_or(Len::Px(3.0)),
        s.unwrap_or_else(|| "none".into()),
        c.unwrap_or(Col::Current),
    ))
}

fn parse_font_size(value: &str) -> Option<Len> {
    let k = value.trim().to_ascii_lowercase();
    let px = match k.as_str() {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "xxx-large" => 48.0,
        "larger" => return Some(Len::Em(1.2)),
        "smaller" => return Some(Len::Em(1.0 / 1.2)),
        _ => return parse_len(value, false).filter(|l| is_len(l) && !matches!(l, Len::Auto | Len::None)),
    };
    Some(Len::Px(px))
}

fn parse_line_height(value: &str) -> Option<Len> {
    parse_len(value, false).filter(|l| !matches!(l, Len::Auto | Len::None))
}

fn parse_weight(value: &str) -> Option<V> {
    let k = value.trim().to_ascii_lowercase();
    match k.as_str() {
        "normal" => Some(V::Weight(400)),
        "bold" => Some(V::Weight(700)),
        "bolder" => Some(V::Bolder),
        "lighter" => Some(V::Lighter),
        _ => k
            .parse::<f32>()
            .ok()
            .filter(|n| (1.0..=1000.0).contains(n))
            .map(|n| V::Weight(n as u16)),
    }
}

fn parse_font_style(value: &str) -> Option<bool> {
    let k = value.trim().to_ascii_lowercase();
    let first = k.split_ascii_whitespace().next()?;
    match first {
        "normal" => Some(false),
        "italic" | "oblique" => Some(true),
        _ => None,
    }
}

/// `font-family`: comma-separated names (strings or ident sequences), generic families kept.
pub(crate) fn parse_families(value: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut p = Parser::new(value);
    let mut cur: Vec<String> = Vec::new();
    while let Ok(tok) = p.next().cloned() {
        match tok {
            Token::QuotedString(s) => {
                if !cur.is_empty() {
                    return None;
                }
                out.push(s.to_string());
                // expect comma or end
                match p.next() {
                    Ok(Token::Comma) | Err(_) => {}
                    _ => return None,
                }
            }
            Token::Ident(i) => cur.push(i.to_string()),
            Token::Comma => {
                if cur.is_empty() {
                    return None;
                }
                out.push(cur.join(" "));
                cur.clear();
            }
            _ => return None,
        }
    }
    if !cur.is_empty() {
        out.push(cur.join(" "));
    }
    (!out.is_empty()).then_some(out)
}

/// `font` shorthand: `[style] [variant] [weight] [stretch] size[/line-height] family`.
fn parse_font_shorthand(value: &str) -> Option<Vec<(P, V)>> {
    let k = value.trim().to_ascii_lowercase();
    if matches!(
        k.as_str(),
        "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar" | "-webkit-control"
    ) {
        return Some(Vec::new()); // system fonts: nothing to import
    }
    let comps = components(value);
    let mut italic = false;
    let mut weight = 400u16;
    let mut i = 0;
    while i < comps.len() {
        let t = comps[i].to_ascii_lowercase();
        match t.as_str() {
            "normal" | "small-caps" | "condensed" | "expanded" | "semi-condensed" => {}
            "italic" | "oblique" => italic = true,
            _ => match parse_weight(&t) {
                Some(V::Weight(w)) => weight = w,
                Some(V::Bolder) => weight = 700,
                Some(V::Lighter) => weight = 300,
                _ => break,
            },
        }
        i += 1;
    }
    let size_tok = comps.get(i)?;
    let (size_str, lh_str) = match size_tok.split_once('/') {
        Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
        Some((a, _)) => {
            i += 1;
            (a.to_string(), comps.get(i).cloned())
        }
        None => {
            if comps.get(i + 1).map(|s| s.as_str()) == Some("/") {
                i += 2;
                (size_tok.clone(), comps.get(i).cloned())
            } else {
                (size_tok.clone(), None)
            }
        }
    };
    let size = parse_font_size(&size_str)?;
    let lh = match lh_str {
        Some(s) => Some(parse_line_height(&s)?),
        None => None,
    };
    i += 1;
    let fam_text = comps[i.min(comps.len())..]
        .join(" ")
        .replace(" , ", ", ")
        .replace(" ,", ",");
    let families = parse_families(&fam_text)?;
    Some(vec![
        (P::Italic, V::Bool(italic)),
        (P::Weight, V::Weight(weight)),
        (P::FontSize, V::Len(size)),
        (P::LineHeight, V::Len(lh.unwrap_or(Len::Normal))),
        (P::Family, V::Families(families)),
    ])
}

fn keyword(value: &str, allowed: &[&str]) -> Option<String> {
    let k = value.trim().to_ascii_lowercase();
    allowed.contains(&k.as_str()).then_some(k)
}

fn keyword_any(value: &str) -> Option<String> {
    let k = value.trim().to_ascii_lowercase();
    (!k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ')).then_some(k)
}

fn parse_decoration(value: &str) -> Option<(bool, bool)> {
    let comps = components(value);
    let mut u = false;
    let mut l = false;
    let mut any = false;
    for c in &comps {
        match c.to_ascii_lowercase().as_str() {
            "none" => any = true,
            "underline" => {
                u = true;
                any = true
            }
            "line-through" => {
                l = true;
                any = true
            }
            _ => {} // style/color/thickness parts are ignored
        }
    }
    any.then_some((u, l))
}

fn parse_list_style_type(t: &str) -> Option<String> {
    let k = t.trim().to_ascii_lowercase();
    if k.starts_with('"') || k.starts_with('\'') {
        return Some(k.trim_matches(['"', '\'']).to_string());
    }
    matches!(
        k.as_str(),
        "none"
            | "disc"
            | "circle"
            | "square"
            | "decimal"
            | "decimal-leading-zero"
            | "lower-roman"
            | "upper-roman"
            | "lower-alpha"
            | "upper-alpha"
            | "lower-latin"
            | "upper-latin"
            | "lower-greek"
            | "cjk-decimal"
            | "cjk-ideographic"
            | "disclosure-open"
            | "disclosure-closed"
    )
    .then_some(k)
}

fn parse_number_or_pct(value: &str) -> Option<f32> {
    let mut p = Parser::new(value.trim());
    let r = match p.next().ok()?.clone() {
        Token::Number { value, .. } => value,
        Token::Percentage { unit_value, .. } => unit_value,
        _ => return None,
    };
    p.is_exhausted().then_some(r)
}

/// `content`: concatenated strings; `none`/`normal` ⇒ `None`. Counters/`attr()` unsupported.
fn parse_content(value: &str) -> Option<Option<String>> {
    let mut p = Parser::new(value.trim());
    let mut out = String::new();
    let mut any = false;
    while let Ok(tok) = p.next().cloned() {
        match tok {
            Token::QuotedString(s) => {
                out.push_str(&s);
                any = true;
            }
            Token::Ident(i) if i.eq_ignore_ascii_case("none") || i.eq_ignore_ascii_case("normal") => {
                return Some(None);
            }
            Token::Ident(i)
                if matches!(
                    i.to_ascii_lowercase().as_str(),
                    "open-quote" | "close-quote" | "no-open-quote" | "no-close-quote"
                ) =>
            {
                any = true
            }
            _ => return None,
        }
    }
    any.then_some(Some(out))
}

/// Parse a custom-property map value as a color (for well-known variables).
pub(crate) fn var_color(vars: &HashMap<String, String>, name: &str) -> Option<Rgba> {
    let raw = vars.get(name)?;
    let v = substitute(raw, &|n| vars.get(n).cloned())?;
    match parse_color(&v)? {
        Col::Rgba(c) => Some(c),
        Col::Current => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(s: &str) -> Rgba {
        match parse_color(s).unwrap() {
            Col::Rgba(c) => c,
            Col::Current => panic!(),
        }
    }

    #[test]
    fn colors() {
        assert_eq!(col("#abc"), Rgba::hex(0xaabbcc));
        assert_eq!(col("#11223380"), Rgba([0x11, 0x22, 0x33, 0x80]));
        assert_eq!(col("rgb(1, 2, 3)"), Rgba([1, 2, 3, 255]));
        assert_eq!(col("rgba(255,0,0,.5)"), Rgba([255, 0, 0, 128]));
        assert_eq!(col("rgb(100% 0% 0% / 50%)"), Rgba([255, 0, 0, 128]));
        assert_eq!(col("hsl(120, 100%, 50%)"), Rgba([0, 255, 0, 255]));
        assert_eq!(col("RebeccaPurple"), Rgba::hex(0x663399));
        assert_eq!(col("transparent"), Rgba::TRANSPARENT);
        assert!(parse_color("url(x)").is_none());
        assert!(parse_color("#ggg").is_none());
    }

    #[test]
    fn var_substitution_with_fallbacks_and_cycles() {
        let vars: HashMap<String, String> = [
            ("--a", "#123456"),
            ("--b", "var(--a)"),
            ("--c", "var(--d)"),
            ("--d", "var(--c)"),
            ("--w", "2px"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let look = |n: &str| vars.get(n).cloned();
        assert_eq!(substitute("var(--b)", &look).unwrap(), "#123456");
        assert_eq!(substitute("var(--x, red)", &look).unwrap(), " red");
        assert_eq!(substitute("var(--x, var(--a))", &look).unwrap(), " #123456");
        assert!(substitute("var(--c)", &look).is_none());
        assert!(substitute("var(--x)", &look).is_none());
        assert_eq!(
            substitute("var(--w) solid var(--a)", &look).unwrap(),
            "2px solid #123456"
        );
        assert_eq!(
            substitute("rgba(var(--rgb, 1,2,3), .5)", &look).unwrap(),
            "rgba( 1,2,3, .5)"
        );
    }

    #[test]
    fn var_expansion_is_bounded() {
        // exponential blow-up is cut by the length limit
        let mut vars = HashMap::new();
        vars.insert("--l0".to_string(), "xxxxxxxxxxxxxxxx".to_string());
        for i in 1..40 {
            vars.insert(format!("--l{i}"), format!("var(--l{}) var(--l{})", i - 1, i - 1));
        }
        let look = |n: &str| vars.get(n).cloned();
        assert!(substitute("var(--l39)", &look).is_none());
    }

    #[test]
    fn var_expansion_work_is_bounded_even_when_empty() {
        // empty values never hit the length limit; the expansion budget stops the doubling
        let mut vars = HashMap::new();
        vars.insert("--e0".to_string(), String::new());
        for i in 1..40 {
            vars.insert(format!("--e{i}"), format!("var(--e{})var(--e{})", i - 1, i - 1));
        }
        let look = |n: &str| vars.get(n).cloned();
        let start = std::time::Instant::now();
        assert!(substitute("var(--e23)", &look).is_none());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn color_mix_in_srgb() {
        assert_eq!(col("color-mix(in srgb, #000, #fff)"), Rgba([128, 128, 128, 255]));
        assert_eq!(col("color-mix(in srgb, red 25%, blue)"), Rgba([64, 0, 191, 255]));
        // mixing with transparent fades (premultiplied): 90% transparent -> 10% alpha
        assert_eq!(
            col("color-mix(in srgb, #2979ff, transparent 90%)"),
            Rgba([0x29, 0x79, 0xff, 26])
        );
        // percentages summing below 100% scale the alpha
        assert_eq!(
            col("color-mix(in oklab, #fff 20%, #fff 30%)"),
            Rgba([255, 255, 255, 128])
        );
        assert_eq!(parse_color("color-mix(in srgb, red)"), None);
        assert_eq!(parse_color("color-mix(srgb, red, blue)"), None);
    }

    #[test]
    fn shorthands() {
        let Expanded::Ok(v) = expand("margin", "1px 2em") else {
            panic!()
        };
        assert_eq!(v[1], (P::MarginR, V::Len(Len::Em(2.0))));
        assert_eq!(v[3], (P::MarginL, V::Len(Len::Em(2.0))));
        let Expanded::Ok(v) = expand("border-left", "4px solid #ddd") else {
            panic!()
        };
        assert_eq!(v[0], (P::BwL, V::Len(Len::Px(4.0))));
        assert_eq!(v[2], (P::BcL, V::Color(Col::Rgba(Rgba::hex(0xdddddd)))));
        let Expanded::Ok(v) = expand("background", "url(a.png) no-repeat #fff") else {
            panic!()
        };
        assert_eq!(v[0], (P::Bg, V::Color(Col::Rgba(Rgba::hex(0xffffff)))));
        // an opaque gradient is approximated by its average stop color (ADR 0004)
        let Expanded::Ok(v) = expand("background", "linear-gradient(red, blue)") else {
            panic!()
        };
        assert_eq!(v[0], (P::Bg, V::Color(Col::Rgba(Rgba([127, 0, 127, 255])))));
        let Expanded::Ok(v) = expand("background", "linear-gradient(to right, #85c1e9, #3498db 50%, #85c1e9)") else {
            panic!()
        };
        assert!(matches!(v[0], (P::Bg, V::Color(Col::Rgba(Rgba([_, _, _, 255]))))));
        // patterns with transparent stops stay transparent (grid paper, dots)
        let Expanded::Ok(v) = expand(
            "background",
            "linear-gradient(90deg, rgba(0,0,0,.05) 1px, transparent 0)",
        ) else {
            panic!()
        };
        assert_eq!(v[0], (P::Bg, V::Color(Col::Rgba(Rgba::TRANSPARENT))));
        let Expanded::Ok(v) = expand("font", "italic bold 12px/1.5 \"Fira Sans\", serif") else {
            panic!()
        };
        assert!(v.contains(&(P::Family, V::Families(vec!["Fira Sans".into(), "serif".into()]))));
        assert!(v.contains(&(P::LineHeight, V::Len(Len::Num(1.5)))));
        assert!(matches!(expand("color", "bogus"), Expanded::BadValue));
        assert!(matches!(expand("color", "inherit"), Expanded::Ok(_)));
    }

    #[test]
    fn families() {
        assert_eq!(
            parse_families("\"Open Sans\", Helvetica Neue, -apple-system, sans-serif").unwrap(),
            vec!["Open Sans", "Helvetica Neue", "-apple-system", "sans-serif"]
        );
    }
}
