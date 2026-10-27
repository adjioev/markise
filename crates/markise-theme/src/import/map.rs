//! Computed styles → native theme slots (spec 03 §2.2 element keys, §2.3 variables).

use std::collections::HashMap;

use super::cascade::{Computed, LineH, Style, Variant};
use super::dom::{ALERTS, Dom};
use super::sheet::Sheet;
use super::value::{parse_families, var_color};
use crate::{BoxStyle, HeadingDecor, Rgba, TextAlign, TextTransform, Theme, TokenStyle};

/// Composite `top` over `bottom` (straight alpha), result opaque if `bottom` is.
pub(crate) fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let a = top.0[3] as f32 / 255.0;
    if a >= 1.0 {
        return top;
    }
    let ba = bottom.0[3] as f32 / 255.0;
    let out_a = a + ba * (1.0 - a);
    if out_a <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    let ch = |i: usize| {
        ((top.0[i] as f32 * a + bottom.0[i] as f32 * ba * (1.0 - a)) / out_a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgba([ch(0), ch(1), ch(2), (out_a * 255.0).round() as u8])
}

const WHITE: Rgba = Rgba([255, 255, 255, 255]);
/// Column widths beyond this are treated as "no limit" (`max-width: none`).
const MAX_COLUMN: f32 = 10_000.0;

fn align_of(s: &Option<String>) -> Option<TextAlign> {
    Some(match s.as_deref()? {
        "center" | "-webkit-center" => TextAlign::Center,
        "right" | "end" => TextAlign::End,
        "justify" => TextAlign::Justify,
        _ => TextAlign::Start,
    })
}

fn transform_of(s: &Option<String>) -> Option<TextTransform> {
    Some(match s.as_deref()? {
        "uppercase" => TextTransform::Uppercase,
        "lowercase" => TextTransform::Lowercase,
        "capitalize" => TextTransform::Capitalize,
        _ => TextTransform::None,
    })
}

fn first_border(s: &Style) -> Option<(f32, Rgba)> {
    (0..4).find_map(|i| s.border(i))
}

fn box_style(base: BoxStyle, s: &Style, fill_under: Option<Rgba>) -> BoxStyle {
    let mut b = base;
    if let Some(bg) = s.fill() {
        b.background = match fill_under {
            Some(under) => over(bg, under),
            None => bg,
        };
    }
    if let Some(c) = s.text() {
        b.text = c;
    }
    if let Some((_, c)) = first_border(s) {
        b.border = Some(c);
    } else if (0..4).any(|i| s.border_set(i)) {
        b.border = None;
    }
    let pads: Vec<f32> = s.padding.iter().flatten().copied().collect();
    if !pads.is_empty() {
        b.padding = s.padding[3].or(s.padding[0]).unwrap_or(pads[0]).clamp(0.0, 200.0);
    }
    if let Some(r) = s.radius {
        b.radius = r.clamp(0.0, 100.0);
    }
    b
}

/// Build one theme variant. `variant` is `Some` for scheme-aware themes.
pub(crate) fn build_theme(c: &Computed, dom: &Dom, sheet: &Sheet, name: &str, variant: Option<Variant>) -> Theme {
    let st = |n: usize| &c.styles[n];
    let pseudo = |n: usize, slot: usize| c.pseudo.get(&(n, slot));
    let root_vars: &HashMap<String, String> = &c.vars[dom.html];
    let write_vars: &HashMap<String, String> = &c.vars[dom.write];
    let var = |name: &str| var_color(write_vars, name).or_else(|| var_color(root_vars, name));

    // ---- backgrounds and dark detection (spec 03 §1.4)
    let chain = [dom.write, dom.content, dom.body, dom.html];
    let mut page: Option<Rgba> = None;
    for &n in chain.iter().rev() {
        if let Some(bg) = st(n).fill() {
            page = Some(match page {
                Some(under) => over(bg, under),
                None => over(bg, WHITE),
            });
        }
    }
    let page_bg = page.or_else(|| var("--bg-color").map(|c| over(c, WHITE)));
    let chrome_bg = st(dom.body)
        .fill()
        .or_else(|| st(dom.html).fill())
        .or_else(|| var("--bg-color"))
        .map(|c| over(c, WHITE))
        .unwrap_or(WHITE);
    let dark = match variant {
        Some(Variant::Dark) => true,
        Some(Variant::Light) => false,
        None => chrome_bg.brightness() < 0.5,
    };
    let base_dark = page_bg.map(|c| c.brightness() < 0.5).unwrap_or(dark);
    let mut t = if base_dark { Theme::dark() } else { Theme::light() };
    t.name = name.to_string();
    t.dark = dark;
    t.scheme_aware = variant.is_some();
    if let Some(bg) = page_bg {
        t.page_bg = bg;
    }
    let page_under = t.page_bg;

    // ---- body text, font, column
    let p = st(dom.p);
    if let Some(col) = p.text().or_else(|| var("--text-color")) {
        t.text = over(col, page_under);
    }
    if p.font_size_set {
        t.font_size = p.font_size.clamp(6.0, 72.0);
    }
    if let Some(lh) = p.line_height {
        let m = match lh {
            LineH::Mult(m) => m,
            LineH::Px(px) => px / p.font_size.max(1.0),
        };
        t.line_height = m.clamp(0.8, 4.0);
    }
    if let Some(a) = align_of(&p.align) {
        t.text_align = a;
    }
    let w = st(dom.write);
    match (w.max_width, w.width) {
        (Some(mw), _) => t.column_max_width = mw.clamp(200.0, MAX_COLUMN),
        (None, Some(width)) => t.column_max_width = width.clamp(200.0, MAX_COLUMN),
        _ => {}
    }
    let px: Vec<f32> = [w.padding[1], w.padding[3]].into_iter().flatten().collect();
    if !px.is_empty() {
        t.column_padding_x = (px.iter().sum::<f32>() / px.len() as f32).clamp(0.0, 400.0);
    }
    if let Some(py) = w.padding[0].or(w.padding[2]) {
        t.column_padding_y = py.clamp(0.0, 400.0);
    }
    match (p.margin[0], p.margin[2]) {
        (None, None) => {}
        (a, b) => t.block_spacing = a.unwrap_or(0.0).max(b.unwrap_or(0.0)).clamp(0.0, 100.0),
    }

    // ---- fonts
    if let Some(f) = &p.family {
        t.fonts.body = f.clone();
    }
    let h1_family = &st(dom.h[0]).family;
    if let Some(hf) = h1_family
        && Some(hf) != p.family.as_ref()
    {
        t.fonts.heading = hf.clone();
    }
    let mono = st(dom.code)
        .family
        .clone()
        .filter(|f| Some(f) != p.family.as_ref())
        .or_else(|| {
            st(dom.codemirror)
                .family
                .clone()
                .filter(|f| Some(f) != p.family.as_ref())
        })
        .or_else(|| root_vars.get("--monospace").and_then(|v| parse_families(v)));
    if let Some(m) = mono {
        t.fonts.mono = m;
    }
    t.fonts.faces = sheet.faces.clone();
    let code = st(dom.code);
    if code.font_size_set && p.font_size > 0.0 && (code.font_size - p.font_size).abs() > 0.01 {
        t.mono_scale = (code.font_size / p.font_size).clamp(0.5, 1.5);
    }

    // ---- headings
    for i in 0..6 {
        let h = st(dom.h[i]);
        let hs = &mut t.headings[i];
        if h.font_size_set {
            hs.size = h.font_size.clamp(6.0, 120.0);
        }
        if let Some(col) = h.text() {
            hs.color = over(col, page_under);
        }
        if let Some(wt) = h.weight {
            hs.bold = wt >= 600;
        }
        if let Some(m) = h.margin[0] {
            hs.margin_top = m.clamp(0.0, 200.0);
        }
        if let Some(m) = h.margin[2] {
            hs.margin_bottom = m.clamp(0.0, 200.0);
        }
        // like a browser: no rule unless the theme draws one (the built-in h1/h2 rules are
        // Markise's own design and would look foreign under an imported theme)
        hs.rule = h.border(2).map(|(_, col)| col).filter(|c| c.0[3] > 0);
        let mut d = HeadingDecor {
            italic: h.italic.unwrap_or(false),
            align: align_of(&h.align).unwrap_or_default(),
            transform: transform_of(&h.transform).unwrap_or_default(),
            letter_spacing: h.letter.unwrap_or(0.0).clamp(-10.0, 50.0),
            background: h.fill().map(|bg| over(bg, page_under)),
            padding: [
                h.padding[3].or(h.padding[1]).unwrap_or(0.0).clamp(0.0, 100.0),
                h.padding[0].or(h.padding[2]).unwrap_or(0.0).clamp(0.0, 100.0),
            ],
            radius: h.radius.unwrap_or(0.0).clamp(0.0, 100.0),
            fit: h.fit,
            bar: h
                .border(3)
                .filter(|(_, c)| c.0[3] > 0)
                .map(|(w, c)| (w.clamp(0.0, 40.0), c)),
            ..Default::default()
        };
        if let Some(before) = pseudo(dom.h[i], 1)
            && !before.display_none
            && let Some(Some(text)) = &before.content
            && !text.is_empty()
        {
            d.prefix = Some(text.clone());
            d.prefix_color = before.text().filter(|c| Some(*c) != h.text());
        }
        if let Some(after) = pseudo(dom.h[i], 2)
            && !after.display_none
            && let Some(Some(text)) = &after.content
            && !text.is_empty()
        {
            d.suffix = Some(text.clone());
        }
        t.heading_decor[i] = d;
    }

    // ---- inline
    let a = st(dom.a);
    if let Some(col) = a.text() {
        t.link = col;
    }
    if let Some((u, _)) = a.deco {
        t.link_underline = u;
    }
    t.inline_code = box_style(t.inline_code, code, Some(page_under));
    let ptext = p.text();
    let differs = |s: &Style| s.text().filter(|c| Some(*c) != ptext);
    t.inline.strong = differs(st(dom.strong));
    t.inline.em = differs(st(dom.em));
    t.inline.del = differs(st(dom.del));
    t.inline.mark_text = differs(st(dom.mark));
    t.inline.markers = differs(st(dom.md_meta)).or_else(|| var("--md-char-color"));
    if let Some(m) = st(dom.mark).fill() {
        t.highlight_bg = m;
    }

    // ---- code block: the CodeMirror box paints over the <pre>
    let fences = st(dom.fences);
    let cm = st(dom.codemirror);
    let mut cb = box_style(t.code_block, fences, Some(page_under));
    if (fences.fill().is_some() || cm.fill().is_some()) && first_border(fences).is_none() && first_border(cm).is_none()
    {
        // a theme that paints its own code box does not get the built-in border
        cb.border = None;
    }
    if let Some(bg) = cm.fill().or_else(|| st(dom.cm_line).fill()) {
        cb.background = over(bg, cb.background);
    }
    if let Some(col) = cm.text() {
        cb.text = over(col, cb.background);
    }
    if cb.radius == t.code_block.radius
        && let Some(r) = cm.radius
    {
        cb.radius = r.clamp(0.0, 100.0);
    }
    t.code_block = cb;
    t.math_block = box_style(t.math_block, st(dom.math), Some(page_under));
    t.meta_block = box_style(t.meta_block, st(dom.meta_block), Some(page_under));

    // ---- syntax tokens (spec 03 §4.5): only colors the theme sets explicitly
    let code_text = cm.text();
    let mut tokens = Vec::new();
    for &(class, n) in &dom.cm_tokens {
        let s = st(n);
        let color = s.text().filter(|c| Some(*c) != code_text);
        let bold = s.weight.is_some_and(|w| w >= 600) && s.weight != cm.weight;
        let italic = s.italic == Some(true) && cm.italic != Some(true);
        if color.is_none() && !bold && !italic {
            continue;
        }
        match class {
            "cm-keyword" => t.syntax.keyword = color.unwrap_or(t.syntax.keyword),
            "cm-string" => t.syntax.string = color.unwrap_or(t.syntax.string),
            "cm-comment" => t.syntax.comment = color.unwrap_or(t.syntax.comment),
            "cm-number" => t.syntax.number = color.unwrap_or(t.syntax.number),
            "cm-def" => t.syntax.function = color.unwrap_or(t.syntax.function),
            "cm-type" | "cm-variable-3" => t.syntax.type_name = color.unwrap_or(t.syntax.type_name),
            _ => {}
        }
        tokens.push(TokenStyle {
            class: class.to_string(),
            color,
            bold,
            italic,
        });
    }
    t.syntax_tokens = tokens;

    // ---- block quote
    let bq = st(dom.blockquote);
    if let Some((wd, col)) = bq.border(3) {
        t.quote_bar = col;
        t.quote_bar_width = wd.clamp(0.0, 40.0);
    } else if bq.border_set(3) {
        t.quote_bar = Rgba::TRANSPARENT;
        t.quote_bar_width = 0.0;
    }
    if let Some(pl) = bq.padding[3] {
        t.quote_indent = (pl + t.quote_bar_width).clamp(0.0, 200.0);
    }
    // the computed (inherited) color, as a browser shows it; without any color in the theme
    // the quote keeps the built-in de-emphasised color
    if let Some(col) = st(dom.quote_p).text().or_else(|| bq.text()) {
        t.quote_text = over(col, page_under);
    }
    if let Some(bg) = bq.fill() {
        t.quote_bg = bg;
    }
    t.quote_italic = st(dom.quote_p).italic.unwrap_or(false);
    if let Some(before) = pseudo(dom.blockquote, 1)
        && let Some(Some(text)) = &before.content
        && !text.is_empty()
    {
        t.quote_prefix = Some(text.clone());
    }

    // ---- lists
    let ul = st(dom.ul[0]);
    if let Some(pl) = ul.padding[3] {
        t.list_indent = (pl + ul.margin[3].unwrap_or(0.0)).clamp(0.0, 200.0);
    }
    let marker = pseudo(dom.ul_li[0], 4)
        .and_then(Style::text)
        .or_else(|| st(dom.ul_li[0]).text());
    if let Some(m) = marker {
        t.list_marker = m;
    }
    t.list_styles.bullets = dom
        .ul
        .iter()
        .zip(&dom.ul_li)
        .filter_map(|(&u, &li)| st(li).list_style.clone().or_else(|| st(u).list_style.clone()))
        .collect();
    if let Some(ls) = st(dom.ol_li)
        .list_style
        .clone()
        .or_else(|| st(dom.ol).list_style.clone())
    {
        t.list_styles.numbers = vec![ls];
    }
    let open = st(dom.task_input_open);
    if let Some(col) = first_border(open).map(|(_, c)| c) {
        t.checkbox_border = col;
    }
    let done = st(dom.task_input_done);
    if let Some(col) = done
        .fill()
        .or_else(|| first_border(done).map(|(_, c)| c).filter(|c| *c != t.checkbox_border))
    {
        t.checkbox_checked = col;
    }

    // ---- rule, table, alerts
    let hr = st(dom.hr);
    if let Some(col) = (0..4).find_map(|i| hr.border(i)).map(|(_, c)| c).or_else(|| hr.fill()) {
        t.rule = col;
    }
    let td = st(dom.td);
    let th = st(dom.th);
    if let Some(col) = first_border(td)
        .or_else(|| first_border(th))
        .map(|(_, c)| c)
        .or_else(|| var("--table-border-color"))
    {
        t.table_border = col;
    }
    // a theme that styles tables gets no built-in header fill or zebra stripes it did not ask for
    let styles_table = first_border(td).is_some() || first_border(th).is_some() || (0..4).any(|i| td.border_set(i));
    match th.fill().or_else(|| st(dom.thead_tr).fill()) {
        Some(bg) => t.table_header_bg = over(bg, page_under),
        None if styles_table => t.table_header_bg = Rgba::TRANSPARENT,
        None => {}
    }
    match st(dom.td_even).fill().or_else(|| st(dom.tr_even).fill()) {
        Some(bg) => t.table_stripe_bg = over(bg, page_under),
        None if styles_table => t.table_stripe_bg = Rgba::TRANSPARENT,
        None => {}
    }
    if let Some(pad) = td.padding[3].or(td.padding[0]) {
        t.table_cell_padding = pad.clamp(0.0, 100.0);
    }
    for i in 0..ALERTS.len() {
        let al = st(dom.alerts[i]);
        if let Some(col) = al
            .border(3)
            .map(|(_, c)| c)
            .or_else(|| st(dom.alert_titles[i]).text().filter(|c| Some(*c) != ptext))
        {
            t.alert[i] = col;
        }
    }
    if let Some(col) = var("--meta-content-color") {
        t.meta_text = col;
    }

    // ---- chrome (colors only, spec 03 §2.2 note)
    if let Some(col) = st(dom.sidebar).fill().or_else(|| var("--side-bar-bg-color")) {
        t.chrome.sidebar_bg = over(col, page_under);
    }
    if let Some(col) = var("--control-text-color") {
        t.chrome.control_text = col;
    }
    if let Some(col) = var("--active-file-bg-color").or_else(|| var("--item-hover-bg-color")) {
        t.chrome.active_bg = col;
    }
    let sel = [dom.p, dom.write, dom.body, dom.html]
        .iter()
        .find_map(|&n| pseudo(n, 3));
    if let Some(col) = sel.and_then(Style::fill).or_else(|| var("--select-text-bg-color")) {
        t.chrome.selection_bg = col;
    }
    t.selection_text = sel.and_then(Style::text).or_else(|| var("--select-text-font-color"));
    if let Some(col) = var("--primary-color") {
        t.chrome.accent = col;
    }
    map_sidebar(&mut t, c, dom, &var);
    t
}

/// Sidebar chrome (spec 03 §3.2): values the theme sets for the sidebar nodes, recognised by
/// differing from what the node would inherit from the document (`body`) — inherited content
/// styles are not sidebar styles. Unset values stay `None` (UI defaults from the chrome colors).
fn map_sidebar(t: &mut Theme, c: &Computed, dom: &Dom, var: &dyn Fn(&str) -> Option<Rgba>) {
    let st = |n: usize| &c.styles[n];
    let body = st(dom.body);
    let side = st(dom.sidebar);
    let title = st(dom.tree_title);
    let under = t.chrome.sidebar_bg;
    let mut s = crate::SidebarStyle {
        row_height: Some(24.0),
        padding_x: Some(6.0),
        indent: Some(14.0),
        radius: Some(4.0),
        ..Default::default()
    };
    // fonts and size
    if let Some(f) = title.family.as_ref().or(side.family.as_ref())
        && Some(f) != body.family.as_ref()
    {
        s.fonts = f.clone();
    }
    if (title.font_size - body.font_size).abs() > 0.01 {
        s.font_size = Some(title.font_size.clamp(8.0, 32.0));
    }
    if title.weight != body.weight
        && let Some(w) = title.weight
    {
        s.bold = Some(w >= 600);
    }
    // colors
    let differs = |x: Option<Rgba>, y: Option<Rgba>| x.filter(|c| Some(*c) != y);
    s.text = differs(title.text(), body.text())
        .or_else(|| var("--control-text-color"))
        .map(|c| over(c, under));
    let active_row = st(dom.tree_row_active);
    s.active_text = differs(st(dom.tree_title_active).text(), title.text())
        .or_else(|| var("--active-file-text-color"))
        .or_else(|| differs(st(dom.side_tab_active).text(), st(dom.side_tab).text()))
        .map(|c| over(c, under));
    s.active_bg = active_row
        .fill()
        .or_else(|| st(dom.tree_title_active).fill())
        .or_else(|| var("--active-file-bg-color"))
        .map(|c| over(c, under));
    s.hover_bg = var("--item-hover-bg-color").map(|c| over(c, under));
    s.hover_text = var("--item-hover-text-color").map(|c| over(c, under));
    let label = st(dom.outline_label);
    let label_active = st(dom.outline_label_active);
    s.outline_active_text = differs(label_active.text(), label.text())
        .or_else(|| differs(st(dom.outline_item_active).text(), st(dom.outline_item).text()))
        .or_else(|| var("--outline-active-color"))
        .map(|c| over(c, under));
    s.outline_active_bg = st(dom.outline_item_active).fill().map(|c| over(c, under));
    s.icon = differs(st(dom.tree_row).text(), body.text()).map(|c| over(c, under));
    // metrics of a file row
    let row = st(dom.tree_child_row);
    if let Some(lh) = row
        .line_height
        .or(title.line_height)
        .filter(|_| row.line_height != body.line_height)
    {
        let px = match lh {
            LineH::Mult(m) => m * title.font_size,
            LineH::Px(px) => px,
        };
        let pad = row.padding[0].unwrap_or(0.0) + row.padding[2].unwrap_or(0.0);
        s.row_height = Some((px + pad).clamp(14.0, 60.0));
    }
    if let Some(px) = row.padding[3] {
        s.padding_x = Some(px.clamp(0.0, 40.0));
    }
    let lead = |n: usize| {
        let x = st(n);
        x.padding[3].unwrap_or(0.0) + x.margin[3].unwrap_or(0.0)
    };
    let step = (lead(dom.outline_wrap_h3) - lead(dom.outline_wrap_h1)) / 2.0;
    if step > 0.5 {
        s.indent = Some(step.clamp(4.0, 40.0));
    }
    if let Some(r) = active_row.radius {
        s.radius = Some(r.clamp(0.0, 20.0));
    }
    t.sidebar = s;
}
