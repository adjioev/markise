//! Selector matching against the virtual DOM, the cascade (importance, specificity, order),
//! custom-property inheritance and `var()` resolution, and computed values with inheritance
//! (spec 03 §2.4 step 3).

use std::collections::HashMap;
use std::rc::Rc;

use super::ImportOptions;
use super::dom::Dom;
use super::selector::{PseudoElement, Spec};
use super::sheet::{Cond, Sheet};
use super::value::{BC, BS, BW, Col, Expanded, Len, NP, P, V, expand, substitute};
use crate::Rgba;

/// Pseudo-element slots we compute: none, ::before, ::after, ::selection, ::marker.
pub(crate) const PSEUDO_SLOTS: usize = 5;

pub(crate) fn pseudo_slot(p: Option<PseudoElement>) -> Option<usize> {
    match p {
        None => Some(0),
        Some(PseudoElement::Before) => Some(1),
        Some(PseudoElement::After) => Some(2),
        Some(PseudoElement::Selection) => Some(3),
        Some(PseudoElement::Marker) => Some(4),
        Some(PseudoElement::Other) => None,
    }
}

/// Which rules match which node (per pseudo slot), with the best specificity per rule.
pub(crate) struct Matches {
    pub per_node: Vec<[Vec<(u32, Spec)>; PSEUDO_SLOTS]>,
    pub rule_matched: Vec<bool>,
    pub selectors_matched: usize,
}

/// `stats_from`: rules before this index (the built-in base layer) are not counted in
/// `selectors_matched`.
pub(crate) fn match_rules(sheet: &Sheet, dom: &Dom, stats_from: usize) -> Matches {
    let n = dom.nodes.len();
    let mut per_node: Vec<[Vec<(u32, Spec)>; PSEUDO_SLOTS]> = (0..n).map(|_| Default::default()).collect();
    let mut rule_matched = vec![false; sheet.rules.len()];
    let mut selectors_matched = 0;
    for (ri, rule) in sheet.rules.iter().enumerate() {
        for sel in &rule.selectors {
            let Some(slot) = pseudo_slot(sel.pseudo) else { continue };
            let mut hit = false;
            for (node, slots) in per_node.iter_mut().enumerate() {
                if sel.matches(dom, node) {
                    hit = true;
                    let list = &mut slots[slot];
                    match list.last_mut() {
                        Some((r, s)) if *r == ri as u32 => *s = (*s).max(sel.spec),
                        _ => list.push((ri as u32, sel.spec)),
                    }
                }
            }
            if hit {
                if ri >= stats_from {
                    selectors_matched += 1;
                }
                rule_matched[ri] = true;
            }
        }
    }
    Matches {
        per_node,
        rule_matched,
        selectors_matched,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    Light,
    Dark,
}

impl Variant {
    fn accepts(self, c: Cond) -> bool {
        matches!(
            (self, c),
            (_, Cond::Always) | (Variant::Light, Cond::Light) | (Variant::Dark, Cond::Dark)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineH {
    Mult(f32),
    Px(f32),
}

/// Computed values of one element. `None` means "not set by the theme" (the base theme's
/// value is kept).
#[derive(Clone, Debug)]
pub(crate) struct Style {
    pub color: Option<Rgba>,
    pub bg: Option<Rgba>,
    pub font_size: f32,
    pub font_size_set: bool,
    pub family: Option<Vec<String>>,
    pub weight: Option<u16>,
    pub italic: Option<bool>,
    pub line_height: Option<LineH>,
    pub margin: [Option<f32>; 4],
    pub padding: [Option<f32>; 4],
    pub bw: [Option<f32>; 4],
    pub bs: [Option<bool>; 4],
    /// `None` with a visible border means `currentColor`.
    pub bc: [Option<Rgba>; 4],
    pub radius: Option<f32>,
    pub max_width: Option<f32>,
    pub width: Option<f32>,
    pub align: Option<String>,
    pub deco: Option<(bool, bool)>,
    pub transform: Option<String>,
    pub letter: Option<f32>,
    pub list_style: Option<String>,
    /// Effective opacity (product along the ancestor chain).
    pub opacity: f32,
    pub content: Option<Option<String>>,
    pub display_none: bool,
    /// The box hugs its content: `display: inline(-block)`, `width: fit-content`, … (not
    /// inherited).
    pub fit: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: None,
            bg: None,
            font_size: 16.0,
            font_size_set: false,
            family: None,
            weight: None,
            italic: None,
            line_height: None,
            margin: [None; 4],
            padding: [None; 4],
            bw: [None; 4],
            bs: [None; 4],
            bc: [None; 4],
            radius: None,
            max_width: None,
            width: None,
            align: None,
            deco: None,
            transform: None,
            letter: None,
            list_style: None,
            opacity: 1.0,
            content: None,
            display_none: false,
            fit: false,
        }
    }
}

impl Style {
    /// Visible border on `side` (0 top, 1 right, 2 bottom, 3 left): (width, color).
    pub fn border(&self, side: usize) -> Option<(f32, Rgba)> {
        let w = self.bw[side].unwrap_or(3.0);
        if self.bs[side] != Some(true) || w <= 0.0 {
            return None;
        }
        let c = self.bc[side].or(self.color)?;
        Some((w, c))
    }

    /// Whether the theme set any border property on `side` (even to none).
    pub fn border_set(&self, side: usize) -> bool {
        self.bs[side].is_some() || self.bw[side].is_some()
    }

    /// Text color with the effective opacity applied.
    pub fn text(&self) -> Option<Rgba> {
        self.color.map(|c| fade(c, self.opacity))
    }

    /// Opaque-ish background: set and not fully transparent.
    pub fn fill(&self) -> Option<Rgba> {
        self.bg.filter(|c| c.0[3] > 0).map(|c| fade(c, self.opacity))
    }
}

pub(crate) fn fade(c: Rgba, opacity: f32) -> Rgba {
    if opacity >= 1.0 {
        return c;
    }
    let a = (c.0[3] as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
    c.with_alpha(a)
}

/// Outcome of a declaration, recorded for the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Outcome {
    Ignored,
    BadValue,
    Applied,
}

pub(crate) struct Computed {
    pub styles: Vec<Style>,
    pub pseudo: HashMap<(usize, usize), Style>,
    pub vars: Vec<Rc<HashMap<String, String>>>,
}

/// Compute every node's style for one color-scheme variant. `outcomes[(rule, decl)]` is raised
/// to the best outcome seen.
pub(crate) fn compute(
    sheet: &Sheet,
    dom: &Dom,
    m: &Matches,
    variant: Variant,
    opts: &ImportOptions,
    outcomes: &mut HashMap<(u32, u32), Outcome>,
) -> Computed {
    let n = dom.nodes.len();
    let mut styles: Vec<Style> = Vec::with_capacity(n);
    let mut vars: Vec<Rc<HashMap<String, String>>> = Vec::with_capacity(n);
    let mut pseudo = HashMap::new();
    let empty = Rc::new(HashMap::new());
    for node in 0..n {
        let parent = dom.nodes[node].parent;
        let parent_vars = parent.map(|p| vars[p].clone()).unwrap_or_else(|| empty.clone());
        let entries = sorted_entries(sheet, &m.per_node[node][0], variant);
        let my_vars = resolve_vars(sheet, &entries, parent_vars);
        let parent_style = parent.map(|p| &styles[p]);
        let root_fs = if node == 0 { 16.0 } else { styles[0].font_size };
        let spec = specified(sheet, &entries, &my_vars, outcomes);
        let st = compute_style(&spec, parent_style, root_fs, opts);
        for slot in 1..PSEUDO_SLOTS {
            if m.per_node[node][slot].is_empty() {
                continue;
            }
            let pe = sorted_entries(sheet, &m.per_node[node][slot], variant);
            if pe.is_empty() {
                continue;
            }
            let pvars = resolve_vars(sheet, &pe, my_vars.clone());
            let ps = specified(sheet, &pe, &pvars, outcomes);
            let pst = compute_style(&ps, Some(&st), root_fs, opts);
            pseudo.insert((node, slot), pst);
        }
        styles.push(st);
        vars.push(my_vars);
    }
    Computed { styles, pseudo, vars }
}

/// (important, spec, rule, decl) sorted ascending by cascade precedence.
fn sorted_entries(sheet: &Sheet, matched: &[(u32, Spec)], variant: Variant) -> Vec<(bool, Spec, u32, u32)> {
    let mut v = Vec::new();
    for &(ri, spec) in matched {
        let rule = &sheet.rules[ri as usize];
        if !variant.accepts(rule.cond) {
            continue;
        }
        for (di, d) in rule.decls.iter().enumerate() {
            v.push((d.important, spec, ri, di as u32));
        }
    }
    v.sort_unstable();
    v
}

fn resolve_vars(
    sheet: &Sheet,
    entries: &[(bool, Spec, u32, u32)],
    parent: Rc<HashMap<String, String>>,
) -> Rc<HashMap<String, String>> {
    let mut own: HashMap<&str, &str> = HashMap::new();
    for &(_, _, ri, di) in entries {
        let d = &sheet.rules[ri as usize].decls[di as usize];
        if d.name.starts_with("--") {
            own.insert(d.name.as_str(), d.value.as_str());
        }
    }
    if own.is_empty() {
        return parent;
    }
    let mut map = (*parent).clone();
    let lookup = |name: &str| -> Option<String> {
        match own.get(name) {
            Some(v) => Some(v.to_string()),
            None => parent.get(name).cloned(),
        }
    };
    let mut resolved = Vec::with_capacity(own.len());
    for (name, raw) in &own {
        let lowered = raw.trim();
        let wide = lowered.to_ascii_lowercase();
        let val = match wide.as_str() {
            "inherit" => parent.get(*name).cloned(),
            "initial" | "unset" | "revert" | "" => None,
            _ => substitute(raw, &|n| if n == *name { None } else { lookup(n) }),
        };
        resolved.push((name.to_string(), val));
    }
    for (name, val) in resolved {
        match val {
            Some(v) => {
                map.insert(name, v.trim().to_string());
            }
            None => {
                map.remove(&name);
            }
        }
    }
    Rc::new(map)
}

fn specified(
    sheet: &Sheet,
    entries: &[(bool, Spec, u32, u32)],
    vars: &HashMap<String, String>,
    outcomes: &mut HashMap<(u32, u32), Outcome>,
) -> Vec<Option<V>> {
    let mut spec: Vec<Option<V>> = vec![None; NP];
    for &(_, _, ri, di) in entries {
        let d = &sheet.rules[ri as usize].decls[di as usize];
        if d.name.starts_with("--") {
            outcomes.insert((ri, di), Outcome::Applied);
            continue;
        }
        let value = substitute(&d.value, &|n| vars.get(n).cloned());
        let outcome = match value {
            None => {
                // invalid at computed-value time: behaves as `unset`
                if let Expanded::Ok(list) = expand(&d.name, "unset") {
                    for (p, v) in list {
                        spec[p.idx()] = Some(v);
                    }
                }
                Outcome::BadValue
            }
            Some(v) => match expand(&d.name, &v) {
                Expanded::Ok(list) => {
                    for (p, v) in list {
                        spec[p.idx()] = Some(v);
                    }
                    Outcome::Applied
                }
                Expanded::BadValue => Outcome::BadValue,
                Expanded::Ignored => Outcome::Ignored,
            },
        };
        let e = outcomes.entry((ri, di)).or_insert(outcome);
        if outcome > *e {
            *e = outcome;
        }
    }
    spec
}

fn len_px(l: Len, fs: f32, root_fs: f32, pct_base: f32, opts: &ImportOptions) -> Option<f32> {
    let v = match l {
        Len::Px(v) => v,
        Len::Em(e) => e * fs,
        Len::Rem(r) => r * root_fs,
        Len::Pct(p) => p / 100.0 * pct_base,
        Len::Vw(v) => v / 100.0 * opts.viewport_width,
        Len::Vh(v) => v / 100.0 * opts.viewport_height,
        Len::Num(_) | Len::Auto | Len::None | Len::Normal => return None,
    };
    v.is_finite().then_some(v)
}

fn compute_style(spec: &[Option<V>], parent: Option<&Style>, root_fs: f32, opts: &ImportOptions) -> Style {
    let dflt = Style::default();
    let par = parent.unwrap_or(&dflt);
    let mut s = Style {
        // inherited properties start from the parent
        color: par.color,
        font_size: par.font_size,
        font_size_set: par.font_size_set,
        family: par.family.clone(),
        weight: par.weight,
        italic: par.italic,
        line_height: par.line_height,
        align: par.align.clone(),
        transform: par.transform.clone(),
        letter: par.letter,
        list_style: par.list_style.clone(),
        opacity: par.opacity,
        ..Style::default()
    };
    let get = |p: P| spec[p.idx()].as_ref();
    let content_w = opts.viewport_width;

    // font-size first: other lengths depend on it
    if let Some(V::Len(l)) = get(P::FontSize) {
        let base = par.font_size;
        if let Some(px) = len_px(*l, base, root_fs, base, opts) {
            s.font_size = px.clamp(1.0, 400.0);
            s.font_size_set = true;
        }
    }
    let fs = s.font_size;

    if let Some(V::Color(Col::Rgba(c))) = get(P::Color) {
        s.color = Some(*c)
    }
    s.bg = match get(P::Bg) {
        Some(V::Color(Col::Rgba(c))) => Some(*c),
        Some(V::Color(Col::Current)) => s.color,
        Some(V::Inherit) => par.bg,
        _ => None,
    };
    if let Some(V::Families(f)) = get(P::Family) {
        s.family = Some(f.clone());
    }
    match get(P::Weight) {
        Some(V::Weight(w)) => s.weight = Some(*w),
        Some(V::Bolder) => {
            let w = par.weight.unwrap_or(400);
            s.weight = Some(if w < 350 {
                400
            } else if w < 550 {
                700
            } else {
                900
            });
        }
        Some(V::Lighter) => {
            let w = par.weight.unwrap_or(400);
            s.weight = Some(if w < 550 {
                100
            } else if w < 750 {
                400
            } else {
                700
            });
        }
        _ => {}
    }
    if let Some(V::Bool(b)) = get(P::Italic) {
        s.italic = Some(*b);
    }
    if let Some(V::Len(l)) = get(P::LineHeight) {
        s.line_height = match *l {
            Len::Num(n) if n >= 0.0 => Some(LineH::Mult(n)),
            Len::Normal => Some(LineH::Mult(1.2)),
            other => len_px(other, fs, root_fs, fs, opts).map(LineH::Px).or(s.line_height),
        };
    }
    let boxes = [
        ([P::MarginT, P::MarginR, P::MarginB, P::MarginL], 0),
        ([P::PadT, P::PadR, P::PadB, P::PadL], 1),
    ];
    for (props, which) in boxes {
        for (i, p) in props.iter().enumerate() {
            let v = match get(*p) {
                Some(V::Len(Len::Auto)) => None,
                Some(V::Len(l)) => len_px(*l, fs, root_fs, content_w, opts),
                Some(V::Inherit) => {
                    if which == 0 {
                        par.margin[i]
                    } else {
                        par.padding[i]
                    }
                }
                Some(V::Initial) => Some(0.0),
                _ => None,
            };
            if which == 0 {
                s.margin[i] = v;
            } else {
                s.padding[i] = v;
            }
        }
    }
    for i in 0..4 {
        s.bw[i] = match get(BW[i]) {
            Some(V::Len(l)) => len_px(*l, fs, root_fs, 0.0, opts),
            Some(V::Inherit) => par.bw[i],
            Some(V::Initial) => Some(3.0),
            _ => None,
        };
        s.bs[i] = match get(BS[i]) {
            Some(V::Kw(k)) => Some(k != "none" && k != "hidden"),
            Some(V::Inherit) => par.bs[i],
            Some(V::Initial) => Some(false),
            _ => None,
        };
        s.bc[i] = match get(BC[i]) {
            Some(V::Color(Col::Rgba(c))) => Some(*c),
            Some(V::Inherit) => par.bc[i],
            _ => None,
        };
    }
    if let Some(V::Len(l)) = get(P::Radius) {
        s.radius = len_px(*l, fs, root_fs, 0.0, opts);
    }
    s.max_width = match get(P::MaxWidth) {
        Some(V::Len(Len::None)) => Some(f32::INFINITY),
        Some(V::Len(l)) => len_px(*l, fs, root_fs, content_w, opts),
        Some(V::Inherit) => par.max_width,
        _ => None,
    };
    s.width = match get(P::Width) {
        Some(V::Len(l)) => len_px(*l, fs, root_fs, content_w, opts),
        Some(V::Inherit) => par.width,
        _ => None,
    };
    let width_fit = matches!(get(P::Width), Some(V::Kw(k)) if k == "fit-content");
    if let Some(V::Kw(k)) = get(P::Align) {
        s.align = Some(k.clone());
    }
    if let Some(V::Kw(k)) = get(P::Transform) {
        s.transform = Some(k.clone());
    }
    match get(P::Letter) {
        Some(V::Len(Len::Normal)) => s.letter = Some(0.0),
        Some(V::Len(l)) => s.letter = len_px(*l, fs, root_fs, 0.0, opts).or(s.letter),
        _ => {}
    }
    if let Some(V::Kw(k)) = get(P::ListStyle) {
        s.list_style = Some(k.clone());
    }
    s.deco = match get(P::Deco) {
        Some(V::Deco(u, l)) => Some((*u, *l)),
        Some(V::Inherit) => par.deco,
        _ => None,
    };
    if let Some(V::Len(Len::Num(o))) = get(P::Opacity) {
        s.opacity = (par.opacity * o).clamp(0.0, 1.0);
    }
    if let Some(V::Text(t)) = get(P::Content) {
        s.content = Some(t.clone());
    }
    if let Some(V::Kw(k)) = get(P::Display) {
        s.display_none = k == "none";
        s.fit = matches!(
            k.as_str(),
            "inline" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "table"
        );
    }
    s.fit |= width_fit;
    s
}
