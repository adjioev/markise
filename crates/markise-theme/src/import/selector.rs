//! Selector parsing and matching for the theme importer.
//!
//! Supports the selector grammar real themes use (spec 03 §2.2): type, universal, `#id`,
//! `.class`, attribute selectors (all operators), the four combinators, structural
//! pseudo-classes (`:first-child`, `:nth-child()`, …), `:not()`, `:is()`, `:where()`,
//! `:checked`, `:root`, state pseudo-classes (which never match: the importer computes the
//! resting state) and the pseudo-elements `::before`, `::after`, `::selection`, `::marker`.
//!
//! Invalid selectors (syntax errors, unknown or other-engine pseudo-classes) invalidate the
//! whole rule, as in a browser. Valid but unsupported selectors (`:has()`, `:nth-child(… of S)`)
//! drop only that selector from its list.

use cssparser::{Parser, Token};

use super::dom::Dom;

/// Specificity packed into one ordered integer: `(ids, classes, types)`, 10 bits each.
pub(crate) type Spec = u32;

fn pack(a: u32, b: u32, c: u32) -> Spec {
    (a.min(1023) << 20) | (b.min(1023) << 10) | c.min(1023)
}

fn unpack(s: Spec) -> (u32, u32, u32) {
    (s >> 20, (s >> 10) & 1023, s & 1023)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Comb {
    Descendant,
    Child,
    Adjacent,
    Sibling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttrOp {
    Exists,
    Eq,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Clone, Debug)]
pub(crate) struct Attr {
    name: String,
    op: AttrOp,
    value: String,
    ci: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum Pseudo {
    /// `:nth-child(an+b)` family; `first-child` is `a=0,b=1`.
    Nth {
        a: i32,
        b: i32,
        from_end: bool,
        of_type: bool,
    },
    Only {
        of_type: bool,
    },
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    Checked,
    Root,
    Empty,
    /// `a:link`, `:any-link`.
    Link,
    /// A state the resting document never has (`:hover`, `:focus`, …).
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PseudoElement {
    Before,
    After,
    Selection,
    Marker,
    /// Valid but irrelevant for the native model (`::placeholder`, `::-webkit-scrollbar`, …).
    Other,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Compound {
    tag: Option<String>,
    ids: Vec<String>,
    classes: Vec<String>,
    attrs: Vec<Attr>,
    pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug)]
pub(crate) struct Selector {
    /// Left to right; `combs[i]` sits between `parts[i]` and `parts[i + 1]`.
    parts: Vec<Compound>,
    combs: Vec<Comb>,
    pub pseudo: Option<PseudoElement>,
    pub spec: Spec,
}

/// Why a selector was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelError {
    /// Invalid in a browser: the whole rule is dropped.
    Invalid(String),
    /// Valid CSS the importer does not evaluate: only this selector is dropped.
    Unsupported(String),
}

/// Parse a selector list. Returns the parsed selectors plus the rejected ones (with the
/// selector text). If any selector is `Invalid`, the caller drops the whole rule.
pub(crate) fn parse_selector_list(text: &str) -> (Vec<Selector>, Vec<(String, SelError)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for piece in split_top_level_commas(text) {
        let t = piece.trim();
        if t.is_empty() {
            bad.push((t.to_string(), SelError::Invalid("empty selector".into())));
            continue;
        }
        match parse_selector(t, 0) {
            Ok(s) => ok.push(s),
            Err(e) => bad.push((t.to_string(), e)),
        }
    }
    (ok, bad)
}

/// Split on commas that are not inside brackets, parentheses or strings.
pub(crate) fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escape = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') => escape = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[') => depth += 1,
            (None, ')' | ']') => depth -= 1,
            (None, ',') if depth <= 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

const MAX_NEST: u32 = 8;

fn parse_selector(text: &str, nest: u32) -> Result<Selector, SelError> {
    if nest > MAX_NEST {
        return Err(SelError::Unsupported("selector nesting too deep".into()));
    }
    let mut p = Parser::new(text);
    let mut parts: Vec<Compound> = Vec::new();
    let mut combs: Vec<Comb> = Vec::new();
    let mut cur = Compound::default();
    let mut cur_empty = true;
    let mut pending: Option<Comb> = None;
    let mut pseudo: Option<PseudoElement> = None;
    let invalid = |m: &str| SelError::Invalid(m.to_string());

    while let Ok(tok) = p.next_including_whitespace().cloned() {
        if pseudo.is_some() && !matches!(tok, Token::WhiteSpace(_) | Token::Colon) {
            // only user-action pseudo-classes may follow a pseudo-element; treat the rest as invalid
            return Err(invalid("content after pseudo-element"));
        }
        let start_compound = |cur_empty: &mut bool,
                              parts: &mut Vec<Compound>,
                              combs: &mut Vec<Comb>,
                              cur: &mut Compound,
                              pending: &mut Option<Comb>|
         -> Result<(), SelError> {
            if let Some(c) = pending.take() {
                if *cur_empty {
                    return Err(invalid("combinator without left side"));
                }
                parts.push(std::mem::take(cur));
                combs.push(c);
                *cur_empty = true;
            }
            Ok(())
        };
        match tok {
            Token::WhiteSpace(_) => {
                if !cur_empty && pending.is_none() {
                    pending = Some(Comb::Descendant);
                }
            }
            Token::Delim(c @ ('>' | '+' | '~')) => {
                if cur_empty && parts.is_empty() {
                    return Err(invalid("leading combinator"));
                }
                if matches!(pending, Some(Comb::Child | Comb::Adjacent | Comb::Sibling)) {
                    return Err(invalid("double combinator"));
                }
                pending = Some(match c {
                    '>' => Comb::Child,
                    '+' => Comb::Adjacent,
                    _ => Comb::Sibling,
                });
            }
            Token::Ident(name) => {
                start_compound(&mut cur_empty, &mut parts, &mut combs, &mut cur, &mut pending)?;
                if !cur_empty {
                    return Err(invalid("type selector inside compound"));
                }
                cur.tag = Some(name.to_ascii_lowercase());
                cur_empty = false;
            }
            Token::Delim('*') => {
                start_compound(&mut cur_empty, &mut parts, &mut combs, &mut cur, &mut pending)?;
                if !cur_empty {
                    return Err(invalid("universal selector inside compound"));
                }
                cur_empty = false;
            }
            Token::Delim('|') => return Err(SelError::Unsupported("namespace selector".into())),
            Token::Delim('&') => return Err(invalid("nesting selector outside a nested rule")),
            Token::IDHash(name) => {
                start_compound(&mut cur_empty, &mut parts, &mut combs, &mut cur, &mut pending)?;
                cur.ids.push(name.to_string());
                cur_empty = false;
            }
            Token::Delim('.') => {
                start_compound(&mut cur_empty, &mut parts, &mut combs, &mut cur, &mut pending)?;
                match p.next_including_whitespace() {
                    Ok(Token::Ident(name)) => cur.classes.push(name.to_string()),
                    _ => return Err(invalid("`.` not followed by a class name")),
                }
                cur_empty = false;
            }
            Token::SquareBracketBlock => {
                start_compound(&mut cur_empty, &mut parts, &mut combs, &mut cur, &mut pending)?;
                let attr = p
                    .parse_nested_block(|p| parse_attr(p).map_err(|_| cssparser::ParseError::<()>::custom(())))
                    .map_err(|_| invalid("bad attribute selector"))?;
                cur.attrs.push(attr);
                cur_empty = false;
            }
            Token::Colon => {
                start_compound(&mut cur_empty, &mut parts, &mut combs, &mut cur, &mut pending)?;
                let next = p.next_including_whitespace().cloned();
                match next {
                    Ok(Token::Colon) => {
                        let el = match p.next_including_whitespace().cloned() {
                            Ok(Token::Ident(n)) => pseudo_element(&n)?,
                            Ok(Token::Function(n)) => {
                                // `::part()`, `::slotted()`, `::highlight()`: skip the argument
                                let _ = p.parse_nested_block(|p| {
                                    while p.next().is_ok() {}
                                    Ok::<(), cssparser::ParseError<()>>(())
                                });
                                pseudo_element(&n)?
                            }
                            _ => return Err(invalid("bad pseudo-element")),
                        };
                        pseudo = Some(el);
                        cur_empty = false;
                    }
                    Ok(Token::Ident(n)) => {
                        let lower = n.to_ascii_lowercase();
                        if let Some(el) = legacy_pseudo_element(&lower) {
                            pseudo = Some(el);
                        } else {
                            cur.pseudos.push(pseudo_class(&lower)?);
                        }
                        cur_empty = false;
                    }
                    Ok(Token::Function(n)) => {
                        let lower = n.to_ascii_lowercase();
                        let res: Result<Pseudo, SelError> = p
                            .parse_nested_block(|p| {
                                let start = p.position();
                                while p.next_including_whitespace_and_comments().is_ok() {}
                                let arg = p.slice_from(start).to_string();
                                Ok::<String, cssparser::ParseError<()>>(arg)
                            })
                            .map_err(|_| invalid("bad pseudo-class argument"))
                            .and_then(|arg| functional_pseudo(&lower, &arg, nest));
                        cur.pseudos.push(res?);
                        cur_empty = false;
                    }
                    _ => return Err(invalid("bad pseudo-class")),
                }
            }
            Token::Comment(_) => {}
            _ => return Err(invalid("unexpected token in selector")),
        }
    }
    if pending.is_some() && !matches!(pending, Some(Comb::Descendant)) {
        return Err(invalid("trailing combinator"));
    }
    if cur_empty {
        return Err(invalid("empty compound selector"));
    }
    parts.push(cur);
    let spec = specificity(&parts, pseudo.is_some());
    Ok(Selector {
        parts,
        combs,
        pseudo,
        spec,
    })
}

fn parse_attr<'i>(p: &mut Parser<'i>) -> Result<Attr, ()> {
    let name = match p.next().map_err(|_| ())? {
        Token::Ident(n) => n.to_ascii_lowercase(),
        _ => return Err(()),
    };
    let op = match p.next() {
        Err(_) => {
            return Ok(Attr {
                name,
                op: AttrOp::Exists,
                value: String::new(),
                ci: false,
            });
        }
        Ok(Token::Delim('=')) => AttrOp::Eq,
        Ok(Token::IncludeMatch) => AttrOp::Includes,
        Ok(Token::DashMatch) => AttrOp::Dash,
        Ok(Token::PrefixMatch) => AttrOp::Prefix,
        Ok(Token::SuffixMatch) => AttrOp::Suffix,
        Ok(Token::SubstringMatch) => AttrOp::Substring,
        Ok(_) => return Err(()),
    };
    let value = match p.next().map_err(|_| ())? {
        Token::Ident(v) | Token::QuotedString(v) => v.to_string(),
        _ => return Err(()),
    };
    let ci = match p.next() {
        Err(_) => false,
        Ok(Token::Ident(f)) if f.eq_ignore_ascii_case("i") => true,
        Ok(Token::Ident(f)) if f.eq_ignore_ascii_case("s") => false,
        Ok(_) => return Err(()),
    };
    if !p.is_exhausted() {
        return Err(());
    }
    Ok(Attr { name, op, value, ci })
}

fn legacy_pseudo_element(n: &str) -> Option<PseudoElement> {
    match n {
        "before" => Some(PseudoElement::Before),
        "after" => Some(PseudoElement::After),
        "first-line" | "first-letter" => Some(PseudoElement::Other),
        _ => None,
    }
}

fn pseudo_element(n: &str) -> Result<PseudoElement, SelError> {
    let n = n.to_ascii_lowercase();
    Ok(match n.as_str() {
        "before" => PseudoElement::Before,
        "after" => PseudoElement::After,
        "selection" => PseudoElement::Selection,
        "marker" => PseudoElement::Marker,
        "first-line"
        | "first-letter"
        | "placeholder"
        | "backdrop"
        | "file-selector-button"
        | "part"
        | "slotted"
        | "highlight"
        | "cue"
        | "grammar-error"
        | "spelling-error"
        | "target-text" => PseudoElement::Other,
        _ if n.starts_with("-webkit-") => PseudoElement::Other,
        _ => return Err(SelError::Invalid(format!("unknown pseudo-element ::{n}"))),
    })
}

fn pseudo_class(n: &str) -> Result<Pseudo, SelError> {
    Ok(match n {
        "first-child" => nth(0, 1, false, false),
        "last-child" => nth(0, 1, true, false),
        "first-of-type" => nth(0, 1, false, true),
        "last-of-type" => nth(0, 1, true, true),
        "only-child" => Pseudo::Only { of_type: false },
        "only-of-type" => Pseudo::Only { of_type: true },
        "checked" => Pseudo::Checked,
        "root" | "scope" => Pseudo::Root,
        "empty" => Pseudo::Empty,
        "link" | "any-link" => Pseudo::Link,
        "hover" | "active" | "focus" | "focus-within" | "focus-visible" | "visited" | "target" | "target-within"
        | "disabled" | "enabled" | "read-only" | "read-write" | "placeholder-shown" | "default" | "indeterminate"
        | "valid" | "invalid" | "required" | "optional" | "in-range" | "out-of-range" | "fullscreen" | "modal"
        | "open" | "closed" | "popover-open" | "autofill" | "user-invalid" | "user-valid" | "defined" | "playing"
        | "paused" | "picture-in-picture" | "host" | "window-inactive" | "horizontal" | "vertical" | "decrement"
        | "increment" | "start" | "end" | "double-button" | "single-button" | "no-button" | "corner-present" => {
            Pseudo::Never
        }
        _ if n.starts_with("-webkit-") => Pseudo::Never,
        _ => return Err(SelError::Invalid(format!("unknown pseudo-class :{n}"))),
    })
}

fn nth(a: i32, b: i32, from_end: bool, of_type: bool) -> Pseudo {
    Pseudo::Nth {
        a,
        b,
        from_end,
        of_type,
    }
}

fn functional_pseudo(name: &str, arg: &str, nest: u32) -> Result<Pseudo, SelError> {
    match name {
        "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
            if name.ends_with("child") && contains_word(arg, "of") {
                return Err(SelError::Unsupported(format!(":{name}(… of S)")));
            }
            let mut p = Parser::new(arg);
            let (a, b) =
                cssparser::parse_nth(&mut p).map_err(|_| SelError::Invalid(format!("bad :{name}() argument")))?;
            if !p.is_exhausted() {
                return Err(SelError::Invalid(format!("bad :{name}() argument")));
            }
            Ok(nth(a, b, name.contains("last"), name.ends_with("of-type")))
        }
        "not" | "is" | "where" | "matches" | "-webkit-any" => {
            let (list, bad) = parse_selector_list_nested(arg, nest + 1);
            if let Some((_, e)) = bad
                .into_iter()
                .find(|(_, e)| name == "not" || matches!(e, SelError::Unsupported(_)))
            {
                // `:is()`/`:where()` are forgiving for invalid members; `:not()` is not.
                return Err(e);
            }
            if name == "not" {
                if list.is_empty() {
                    return Err(SelError::Invalid(":not() without argument".into()));
                }
                Ok(Pseudo::Not(list))
            } else if name == "where" {
                Ok(Pseudo::Is(
                    list.into_iter()
                        .map(|mut s| {
                            s.spec = 0;
                            s
                        })
                        .collect(),
                ))
            } else {
                Ok(Pseudo::Is(list))
            }
        }
        "has" => Err(SelError::Unsupported(":has()".into())),
        "lang" | "dir" | "host" | "host-context" | "state" | "-webkit-dir" => Ok(Pseudo::Never),
        _ if name.starts_with("-webkit-") => Ok(Pseudo::Never),
        _ => Err(SelError::Invalid(format!("unknown pseudo-class :{name}()"))),
    }
}

fn parse_selector_list_nested(text: &str, nest: u32) -> (Vec<Selector>, Vec<(String, SelError)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for piece in split_top_level_commas(text) {
        let t = piece.trim();
        match parse_selector(t, nest) {
            Ok(s) if s.pseudo.is_none() => ok.push(s),
            Ok(_) => bad.push((
                t.to_string(),
                SelError::Invalid("pseudo-element in selector argument".into()),
            )),
            Err(e) => bad.push((t.to_string(), e)),
        }
    }
    (ok, bad)
}

fn contains_word(s: &str, w: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .any(|t| t.eq_ignore_ascii_case(w))
}

fn specificity(parts: &[Compound], pseudo_el: bool) -> Spec {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    for part in parts {
        a += part.ids.len() as u32;
        b += (part.classes.len() + part.attrs.len()) as u32;
        if part.tag.is_some() {
            c += 1;
        }
        for ps in &part.pseudos {
            match ps {
                Pseudo::Not(list) | Pseudo::Is(list) => {
                    let max = list.iter().map(|s| s.spec).max().unwrap_or(0);
                    let (x, y, z) = unpack(max);
                    a += x;
                    b += y;
                    c += z;
                }
                _ => b += 1,
            }
        }
    }
    if pseudo_el {
        c += 1;
    }
    pack(a, b, c)
}

// ------------------------------------------------------------------------------------------
// Matching against the virtual DOM

impl Selector {
    /// Does this selector match element `node` (ignoring any pseudo-element)?
    pub(crate) fn matches(&self, dom: &Dom, node: usize) -> bool {
        let last = self.parts.len() - 1;
        match_from(&self.parts, &self.combs, last, dom, node, 0)
    }
}

fn match_from(parts: &[Compound], combs: &[Comb], i: usize, dom: &Dom, node: usize, depth: u32) -> bool {
    if depth > 64 || !compound_matches(&parts[i], dom, node) {
        return false;
    }
    if i == 0 {
        return true;
    }
    match combs[i - 1] {
        Comb::Child => dom.nodes[node]
            .parent
            .is_some_and(|p| match_from(parts, combs, i - 1, dom, p, depth + 1)),
        Comb::Descendant => {
            let mut cur = dom.nodes[node].parent;
            while let Some(p) = cur {
                if match_from(parts, combs, i - 1, dom, p, depth + 1) {
                    return true;
                }
                cur = dom.nodes[p].parent;
            }
            false
        }
        Comb::Adjacent => dom
            .prev_sibling(node)
            .is_some_and(|s| match_from(parts, combs, i - 1, dom, s, depth + 1)),
        Comb::Sibling => {
            let mut cur = dom.prev_sibling(node);
            while let Some(s) = cur {
                if match_from(parts, combs, i - 1, dom, s, depth + 1) {
                    return true;
                }
                cur = dom.prev_sibling(s);
            }
            false
        }
    }
}

fn compound_matches(c: &Compound, dom: &Dom, node: usize) -> bool {
    let n = &dom.nodes[node];
    if let Some(t) = &c.tag
        && t != n.tag
    {
        return false;
    }
    if !c.ids.iter().all(|id| n.id == Some(id.as_str())) {
        return false;
    }
    if !c.classes.iter().all(|cl| n.classes.iter().any(|x| x == cl)) {
        return false;
    }
    if !c.attrs.iter().all(|a| attr_matches(a, n.attr(&a.name))) {
        return false;
    }
    c.pseudos.iter().all(|ps| pseudo_matches(ps, dom, node))
}

fn attr_matches(a: &Attr, val: Option<&str>) -> bool {
    let Some(v) = val else { return false };
    let (v, want) = if a.ci {
        (v.to_ascii_lowercase(), a.value.to_ascii_lowercase())
    } else {
        (v.to_string(), a.value.clone())
    };
    match a.op {
        AttrOp::Exists => true,
        AttrOp::Eq => v == want,
        AttrOp::Includes => v.split_ascii_whitespace().any(|w| w == want),
        AttrOp::Dash => v == want || v.starts_with(&format!("{want}-")),
        AttrOp::Prefix => !want.is_empty() && v.starts_with(&want),
        AttrOp::Suffix => !want.is_empty() && v.ends_with(&want),
        AttrOp::Substring => !want.is_empty() && v.contains(&want),
    }
}

fn pseudo_matches(ps: &Pseudo, dom: &Dom, node: usize) -> bool {
    match ps {
        Pseudo::Nth {
            a,
            b,
            from_end,
            of_type,
        } => {
            let (idx, count) = dom.sibling_position(node, *of_type);
            let pos = if *from_end { count - idx + 1 } else { idx } as i64;
            let (a, b) = (*a as i64, *b as i64);
            if a == 0 {
                pos == b
            } else {
                let diff = pos - b;
                diff % a == 0 && diff / a >= 0
            }
        }
        Pseudo::Only { of_type } => dom.sibling_position(node, *of_type).1 == 1,
        Pseudo::Not(list) => !list.iter().any(|s| s.matches(dom, node)),
        Pseudo::Is(list) => list.iter().any(|s| s.matches(dom, node)),
        Pseudo::Checked => dom.nodes[node].attr("checked").is_some(),
        Pseudo::Root => node == 0,
        Pseudo::Empty => dom.nodes[node].children.is_empty() && !dom.nodes[node].has_text,
        Pseudo::Link => dom.nodes[node].tag == "a",
        Pseudo::Never => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_of(s: &str) -> (u32, u32, u32) {
        let (ok, bad) = parse_selector_list(s);
        assert!(bad.is_empty(), "{s}: {bad:?}");
        unpack(ok[0].spec)
    }

    #[test]
    fn specificity_follows_selectors_4() {
        assert_eq!(spec_of("#write h1"), (1, 0, 1));
        assert_eq!(spec_of(".md-fences"), (0, 1, 0));
        assert_eq!(spec_of("table tr:nth-child(2n)"), (0, 1, 2));
        assert_eq!(spec_of("h1::before"), (0, 0, 2));
        assert_eq!(spec_of(":not(#a, .b) p"), (1, 0, 1));
        assert_eq!(spec_of(":where(#a) p"), (0, 0, 1));
        assert_eq!(spec_of("input[type=checkbox]:checked"), (0, 2, 1));
    }

    #[test]
    fn invalid_and_unsupported_are_distinguished() {
        let (_, bad) = parse_selector_list("p:has(a)");
        assert!(matches!(bad[0].1, SelError::Unsupported(_)));
        let (_, bad) = parse_selector_list("p::-moz-selection");
        assert!(matches!(bad[0].1, SelError::Invalid(_)));
        let (_, bad) = parse_selector_list("p:frobnicate");
        assert!(matches!(bad[0].1, SelError::Invalid(_)));
        let (ok, bad) = parse_selector_list("::-webkit-scrollbar, a:hover, h1:before");
        assert!(bad.is_empty());
        assert_eq!(ok.len(), 3);
        let (_, bad) = parse_selector_list("> p");
        assert!(matches!(bad[0].1, SelError::Invalid(_)));
    }

    #[test]
    fn splits_commas_at_top_level_only() {
        assert_eq!(
            split_top_level_commas("a, :is(b, c), [x=\",\"]"),
            vec!["a", " :is(b, c)", " [x=\",\"]"]
        );
    }
}
