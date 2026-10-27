//! `markise-cli theme-import <theme.css> [--json]`: import a CSS theme (spec 03 §2) and print
//! the resulting theme summary and the import report.

use std::path::Path;
use std::process::ExitCode;

use markise_theme::{ImportOptions, ImportReport, ImportResult, Issue, Rgba, Theme, import_theme_file};
use serde_json::{Value, json};

fn hex(c: Rgba) -> String {
    let [r, g, b, a] = c.0;
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

pub fn import(path: &str, json: bool) -> Result<ExitCode, String> {
    let r = import_theme_file(Path::new(path), &ImportOptions::default()).map_err(|e| format!("{path}: {e}"))?;
    if json {
        let v = to_json(&r);
        println!("{}", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
    } else {
        print_text(&r);
    }
    Ok(ExitCode::SUCCESS)
}

fn theme_json(t: &Theme) -> Value {
    json!({
        "name": t.name,
        "dark": t.dark,
        "scheme_aware": t.scheme_aware,
        "page_bg": hex(t.page_bg),
        "text": hex(t.text),
        "link": hex(t.link),
        "font_size": t.font_size,
        "line_height": t.line_height,
        "column_max_width": t.column_max_width,
        "column_padding": [t.column_padding_x, t.column_padding_y],
        "fonts": {
            "body": t.fonts.body,
            "heading": t.fonts.heading,
            "mono": t.fonts.mono,
            "faces": t.fonts.faces.iter().map(|f| json!({
                "family": f.family,
                "weight": [f.weight.0, f.weight.1],
                "italic": f.italic,
                "unicode_ranges": f.unicode_range.len(),
                "sources": f.sources.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        },
        "headings": t.headings.iter().zip(&t.heading_decor).map(|(h, d)| json!({
            "size": h.size,
            "color": hex(h.color),
            "bold": h.bold,
            "margin": [h.margin_top, h.margin_bottom],
            "rule": h.rule.map(hex),
            "prefix": d.prefix,
            "suffix": d.suffix,
            "align": format!("{:?}", d.align),
            "background": d.background.map(hex),
            "padding": d.padding,
            "radius": d.radius,
            "fit": d.fit,
            "bar": d.bar.map(|(w, c)| json!([w, hex(c)])),
        })).collect::<Vec<_>>(),
        "sidebar": {
            "fonts": t.sidebar.fonts, "font_size": t.sidebar.font_size, "bold": t.sidebar.bold,
            "text": t.sidebar.text.map(hex), "active_text": t.sidebar.active_text.map(hex),
            "active_bg": t.sidebar.active_bg.map(hex), "hover_bg": t.sidebar.hover_bg.map(hex),
            "outline_active_text": t.sidebar.outline_active_text.map(hex),
            "row_height": t.sidebar.row_height, "indent": t.sidebar.indent,
        },
        "link_underline": t.link_underline,
        "text_align": format!("{:?}", t.text_align),
        "inline": {
            "strong": t.inline.strong.map(hex), "em": t.inline.em.map(hex),
            "del": t.inline.del.map(hex), "mark_text": t.inline.mark_text.map(hex),
        },
        "lists": { "indent": t.list_indent, "marker": hex(t.list_marker), "bullets": t.list_styles.bullets, "numbers": t.list_styles.numbers },
        "inline_code": { "background": hex(t.inline_code.background), "text": hex(t.inline_code.text) },
        "code_block": { "background": hex(t.code_block.background), "text": hex(t.code_block.text), "border": t.code_block.border.map(hex) },
        "quote": { "bar": hex(t.quote_bar), "bar_width": t.quote_bar_width, "text": hex(t.quote_text), "background": hex(t.quote_bg), "italic": t.quote_italic, "prefix": t.quote_prefix },
        "table": { "border": hex(t.table_border), "header_bg": hex(t.table_header_bg), "stripe_bg": hex(t.table_stripe_bg) },
        "rule": hex(t.rule),
        "highlight_bg": hex(t.highlight_bg),
        "syntax": {
            "keyword": hex(t.syntax.keyword), "string": hex(t.syntax.string),
            "comment": hex(t.syntax.comment), "number": hex(t.syntax.number),
            "tokens": t.syntax_tokens.len(),
        },
        "chrome": {
            "sidebar_bg": hex(t.chrome.sidebar_bg), "control_text": hex(t.chrome.control_text),
            "active_bg": hex(t.chrome.active_bg), "selection_bg": hex(t.chrome.selection_bg),
            "accent": hex(t.chrome.accent),
        },
    })
}

fn issues_json(rep: &ImportReport, v: &[Issue]) -> Value {
    Value::Array(
        v.iter()
            .map(|i| {
                json!({
                    "name": i.name,
                    "count": i.count,
                    "locations": i.locations.iter().map(|l| rep.describe(*l)).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn to_json(r: &ImportResult) -> Value {
    let rep = &r.report;
    json!({
        "theme": theme_json(&r.theme),
        "dark_variant": r.dark_variant.as_ref().map(theme_json),
        "print_rules": r.print_css.len(),
        "include_when_export": r.include_when_export,
        "report": {
            "files": rep.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "scheme_aware": rep.scheme_aware,
            "dark": rep.dark,
            "rules": rep.rules,
            "selectors": rep.selectors,
            "selectors_matched": rep.selectors_matched,
            "declarations": rep.declarations,
            "content_declarations": rep.content_declarations,
            "applied": rep.applied,
            "ignored": rep.ignored,
            "coverage": rep.coverage(),
            "font_faces": rep.font_faces,
            "unsupported_selectors": issues_json(rep, &rep.unsupported_selectors),
            "unsupported_properties": issues_json(rep, &rep.unsupported_properties),
            "unsupported_values": issues_json(rep, &rep.unsupported_values),
            "unsupported_at_rules": issues_json(rep, &rep.unsupported_at_rules),
            "ignored_properties": issues_json(rep, &rep.ignored_properties),
            "warnings": rep.warnings,
        },
    })
}

fn print_theme(label: &str, t: &Theme) {
    println!(
        "{label}: \"{}\" ({}{})",
        t.name,
        if t.dark { "dark" } else { "light" },
        if t.scheme_aware { ", scheme-aware" } else { "" }
    );
    println!(
        "  page {}  text {}  link {}  accent {}",
        hex(t.page_bg),
        hex(t.text),
        hex(t.link),
        hex(t.chrome.accent)
    );
    println!(
        "  font {}pt × {:.2}  column ≤ {} (padding {} / {})",
        t.font_size, t.line_height, t.column_max_width, t.column_padding_x, t.column_padding_y
    );
    let stack = |v: &Vec<String>| {
        if v.is_empty() {
            "(default)".to_string()
        } else {
            v.join(", ")
        }
    };
    println!("  body font: {}", stack(&t.fonts.body));
    if !t.fonts.heading.is_empty() {
        println!("  heading font: {}", stack(&t.fonts.heading));
    }
    println!("  mono font: {}", stack(&t.fonts.mono));
    let hs: Vec<String> = t
        .headings
        .iter()
        .map(|h| format!("{}{} {}", h.size, if h.bold { "b" } else { "" }, hex(h.color)))
        .collect();
    println!("  headings: {}", hs.join(" | "));
    println!(
        "  code {} on {} | block {} on {} | quote bar {} ×{} | table {} head {}",
        hex(t.inline_code.text),
        hex(t.inline_code.background),
        hex(t.code_block.text),
        hex(t.code_block.background),
        hex(t.quote_bar),
        t.quote_bar_width,
        hex(t.table_border),
        hex(t.table_header_bg)
    );
    println!(
        "  syntax kw {} str {} com {} num {} (+{} token classes) | sidebar {}",
        hex(t.syntax.keyword),
        hex(t.syntax.string),
        hex(t.syntax.comment),
        hex(t.syntax.number),
        t.syntax_tokens.len(),
        hex(t.chrome.sidebar_bg)
    );
}

fn print_issues(rep: &ImportReport, title: &str, v: &[Issue]) {
    if v.is_empty() {
        return;
    }
    println!("{title}:");
    for i in v.iter().take(30) {
        let locs: Vec<String> = i.locations.iter().take(3).map(|l| rep.describe(*l)).collect();
        println!("  {:>4}× {}  [{}]", i.count, i.name, locs.join(", "));
    }
    if v.len() > 30 {
        println!("  … {} more", v.len() - 30);
    }
}

fn print_text(r: &ImportResult) {
    print_theme("theme", &r.theme);
    if let Some(d) = &r.dark_variant {
        print_theme("dark variant", d);
    }
    let rep = &r.report;
    println!();
    println!(
        "files: {}",
        rep.files
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "rules {} | selectors {} (matched {}) | declarations {} (content {}, applied {}, ignored {}) | coverage {:.1}%",
        rep.rules,
        rep.selectors,
        rep.selectors_matched,
        rep.declarations,
        rep.content_declarations,
        rep.applied,
        rep.ignored,
        rep.coverage() * 100.0
    );
    println!(
        "@font-face {} | @media print blocks {} | @include-when-export {}",
        rep.font_faces,
        r.print_css.len(),
        r.include_when_export.len()
    );
    print_issues(rep, "unsupported properties", &rep.unsupported_properties);
    print_issues(rep, "unsupported values", &rep.unsupported_values);
    print_issues(rep, "unsupported selectors", &rep.unsupported_selectors);
    print_issues(rep, "skipped at-rules", &rep.unsupported_at_rules);
    for w in &rep.warnings {
        println!("warning: {w}");
    }
}
