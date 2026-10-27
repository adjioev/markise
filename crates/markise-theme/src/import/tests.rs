//! Feature tests for the CSS importer, using small original CSS snippets.

use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::{FontSource, Rgba, TextAlign, TextTransform};

fn imp(css: &str) -> ImportResult {
    import_css(css, Path::new("."), &ImportOptions::default())
}

fn hex(v: u32) -> Rgba {
    Rgba::hex(v)
}

/// Spec 03 §7 minimal example theme (original values from the spec).
const SPEC_EXAMPLE: &str = r#"
:root { --bg-color: #fbfaf7; --text-color: #2b2b2b; --side-bar-bg-color: #f0eee8; --monospace: "JetBrains Mono", monospace; }
html { font-size: 17px; }
body { background-color: var(--bg-color); color: var(--text-color); }
#write { max-width: 46em; padding: 24px 48px; font-family: "Iowan Old Style", Georgia, serif; line-height: 1.65; }
#write h1 { font-size: 2.1rem; font-weight: 700; margin: 1.4em 0 0.6em; border-bottom: 1px solid #ddd6c8; }
#write h2 { font-size: 1.6rem; color: #6b4f2a; }
#write a { color: #9a3b1b; text-decoration: none; }
#write blockquote { border-left: 3px solid #c9b994; padding-left: 1em; color: #6d675c; }
#write code { font-family: var(--monospace); background: #efe9dc; padding: 0 4px; border-radius: 3px; }
#write .md-fences { background: #f3efe6; padding: 10px 14px; border-radius: 4px; }
.cm-s-inner .cm-keyword { color: #8a3ffc; } .cm-s-inner .cm-string { color: #2a7d4f; } .cm-s-inner .cm-comment { color: #9a948a; font-style: italic; }
#write table th { background: #efe9dc; } #write table td, #write table th { border: 1px solid #ddd6c8; padding: 4px 10px; }
#write .md-task-list-item > input { margin-left: -1.3em; }
@media (prefers-color-scheme: dark) { :root { --bg-color: #1e1d1b; --text-color: #e6e1d6; } #write h2 { color: #d8b57a; } }
@media print { #write { max-width: none; } }
"#;

#[test]
fn spec_example_theme_imports() {
    let r = imp(SPEC_EXAMPLE);
    let t = &r.theme;
    assert!(r.report.scheme_aware && t.scheme_aware);
    assert!(!t.dark);
    assert_eq!(t.page_bg, hex(0xfbfaf7));
    assert_eq!(t.text, hex(0x2b2b2b));
    assert_eq!(t.font_size, 17.0);
    assert!((t.line_height - 1.65).abs() < 1e-4);
    // em on #write is relative to its own (inherited 17px) font size
    assert!(
        (t.column_max_width - 46.0 * 17.0).abs() < 0.01,
        "{}",
        t.column_max_width
    );
    assert_eq!(t.column_padding_x, 48.0);
    assert_eq!(t.column_padding_y, 24.0);
    assert_eq!(t.fonts.body, vec!["Iowan Old Style", "Georgia", "serif"]);
    assert_eq!(t.fonts.mono, vec!["JetBrains Mono", "monospace"]);
    // headings: rem is relative to the html font size
    assert!((t.headings[0].size - 2.1 * 17.0).abs() < 0.01);
    assert!((t.headings[0].margin_top - 1.4 * 2.1 * 17.0).abs() < 0.01);
    assert_eq!(t.headings[0].rule, Some(hex(0xddd6c8)));
    assert_eq!(t.headings[1].color, hex(0x6b4f2a));
    assert_eq!(t.headings[2].color, hex(0x2b2b2b), "inherits body text");
    assert_eq!(t.link, hex(0x9a3b1b));
    assert!(!t.link_underline);
    assert_eq!(t.quote_bar, hex(0xc9b994));
    assert_eq!(t.quote_bar_width, 3.0);
    assert_eq!(t.quote_text, hex(0x6d675c));
    assert_eq!(t.inline_code.background, hex(0xefe9dc));
    assert_eq!(t.inline_code.radius, 3.0);
    assert_eq!(t.code_block.background, hex(0xf3efe6));
    assert_eq!(t.code_block.padding, 14.0);
    assert_eq!(t.syntax.keyword, hex(0x8a3ffc));
    assert_eq!(t.syntax.string, hex(0x2a7d4f));
    assert_eq!(t.syntax.comment, hex(0x9a948a));
    assert!(t.syntax_tokens.iter().any(|k| k.class == "cm-comment" && k.italic));
    assert_eq!(t.table_header_bg, hex(0xefe9dc));
    assert_eq!(t.table_border, hex(0xddd6c8));
    assert_eq!(t.table_cell_padding, 10.0);
    assert_eq!(t.chrome.sidebar_bg, hex(0xf0eee8));
    // dark variant from the media query
    let d = r.dark_variant.as_ref().expect("dark variant");
    assert!(d.dark);
    assert_eq!(d.page_bg, hex(0x1e1d1b));
    assert_eq!(d.text, hex(0xe6e1d6));
    assert_eq!(d.headings[1].color, hex(0xd8b57a));
    assert_eq!(d.link, hex(0x9a3b1b), "unconditional rules apply to both variants");
    assert!(std::ptr::eq(r.variant(true), d));
    // print rules are kept for export, not applied
    assert_eq!(r.print_css.len(), 1);
    assert!(r.print_css[0].contains("max-width: none"));
    assert!(
        r.report.unsupported_selectors.is_empty(),
        "{:?}",
        r.report.unsupported_selectors
    );
    assert!(r.report.coverage() > 0.95, "{}", r.report.coverage());
}

#[test]
fn dark_detection_by_luma_without_media_query() {
    // spec 03 §1.4: brightness < 0.5 ⇒ dark; fully transparent counts as white
    let r = imp("body { background: #202124; color: #ddd }");
    assert!(r.theme.dark && !r.theme.scheme_aware && r.dark_variant.is_none());
    assert_eq!(r.theme.page_bg, hex(0x202124));
    // the dark built-in is the base, so unset slots stay readable
    assert_eq!(
        r.theme.code_block.background,
        crate::Theme::dark().code_block.background
    );
    let r = imp("body { background: transparent }");
    assert!(!r.theme.dark);
    let r = imp("body { background: #808080 }");
    assert!(!r.theme.dark, "0x80 gives exactly 0.5: light");
    let r = imp("body { background: #7f7f7f }");
    assert!(r.theme.dark, "0x7f gives 0.496: dark");
    // html background (body transparent) also counts
    let r = imp("html { background-color: #111 } ");
    assert!(r.theme.dark);
    // --bg-color alone (spec 03 §2.3 direct mapping)
    let r = imp(":root { --bg-color: #101010 }");
    assert!(r.theme.dark);
}

#[test]
fn cascade_specificity_order_and_important() {
    let r = imp("#write h1 { color: #111111 } h1 { color: #222222 }
         p { color: #333333 } p { color: #444444 }
         a { color: #555555 !important } #write a { color: #666666 }");
    assert_eq!(r.theme.headings[0].color, hex(0x111111), "id beats later type selector");
    assert_eq!(r.theme.text, hex(0x444444), "later wins at equal specificity");
    assert_eq!(r.theme.link, hex(0x555555), "!important beats specificity");
}

#[test]
fn var_fallbacks_and_inheritance_of_custom_properties() {
    let r = imp(":root { --accent: #aa0000; --h: var(--accent); }
         #write { --code-bg: #eeeeee; }
         h1 { color: var(--h); }
         h2 { color: var(--missing, #00aa00); }
         h3 { color: var(--missing); }
         code { background: var(--code-bg); }
         a { color: var(--accent, blue); }");
    assert_eq!(r.theme.headings[0].color, hex(0xaa0000));
    assert_eq!(r.theme.headings[1].color, hex(0x00aa00));
    // invalid at computed-value time ⇒ unset ⇒ nothing set: the base theme's heading color
    assert_eq!(r.theme.headings[2].color, crate::Theme::light().headings[2].color);
    assert_eq!(
        r.theme.inline_code.background,
        hex(0xeeeeee),
        "variable defined on #write"
    );
    assert_eq!(r.theme.link, hex(0xaa0000));
    assert_eq!(
        r.report.unsupported_values.len(),
        1,
        "{:?}",
        r.report.unsupported_values
    );
}

#[test]
fn well_known_variables_map_to_chrome_slots() {
    let r = imp(":root { --side-bar-bg-color: #010203; --control-text-color: #040506;
                 --active-file-bg-color: #070809; --select-text-bg-color: #0a0b0c;
                 --primary-color: #0d0e0f; --meta-content-color: #101112; }");
    let c = &r.theme.chrome;
    assert_eq!(c.sidebar_bg, hex(0x010203));
    assert_eq!(c.control_text, hex(0x040506));
    assert_eq!(c.active_bg, hex(0x070809));
    assert_eq!(c.selection_bg, hex(0x0a0b0c));
    assert_eq!(c.accent, hex(0x0d0e0f));
    assert_eq!(r.theme.meta_text, hex(0x101112));
}

#[test]
fn h_aliases_c5() {
    let r = imp(".h2 { font-size: 30px; color: #123123 }");
    assert_eq!(r.theme.headings[1].size, 30.0);
    assert_eq!(r.theme.headings[1].color, hex(0x123123));
}

#[test]
fn units_resolve() {
    let r = imp(
        "html { font-size: 20px } body { font-size: 0.8rem } p { font-size: 125% }
         h1 { font-size: 2em; margin-top: 1em } h2 { font-size: 12pt } h3 { font-size: larger }
         code { font-size: 0.9em }",
    );
    // body 16px, p 20px
    assert_eq!(r.theme.font_size, 20.0);
    assert_eq!(
        r.theme.headings[0].size, 32.0,
        "em relative to the parent (#write ← body 16px)"
    );
    assert_eq!(r.theme.headings[0].margin_top, 32.0, "margin em relative to own size");
    assert_eq!(r.theme.headings[1].size, 16.0);
    assert!((r.theme.headings[2].size - 19.2).abs() < 0.01);
    assert!((r.theme.mono_scale - 0.9).abs() < 1e-4);
}

#[test]
fn line_height_lengths_become_multipliers() {
    let r = imp("p { font-size: 20px; line-height: 30px }");
    assert!((r.theme.line_height - 1.5).abs() < 1e-4);
    let r = imp("#write { line-height: 1.8em } p { font-size: 10px }");
    // em line-height computes to px on #write (1.8 × 16) and is inherited as a length
    assert!((r.theme.line_height - 2.88).abs() < 1e-3, "{}", r.theme.line_height);
}

#[test]
fn heading_decorations_and_pseudo_elements() {
    let r = imp(
        "h1 { text-transform: uppercase; letter-spacing: 2px; text-align: center; font-style: italic }
         h2::before { content: '§ '; color: #ff0000 }
         h3:after { content: \"\" }
         blockquote::before { content: '\\201C' }
         blockquote { background: #fafafa; font-style: italic }",
    );
    let d = &r.theme.heading_decor;
    assert_eq!(d[0].transform, TextTransform::Uppercase);
    assert_eq!(d[0].letter_spacing, 2.0);
    assert_eq!(d[0].align, TextAlign::Center);
    assert!(d[0].italic);
    assert_eq!(d[1].prefix.as_deref(), Some("§ "));
    assert_eq!(d[1].prefix_color, Some(hex(0xff0000)));
    assert_eq!(d[2].suffix, None, "empty content is no suffix");
    assert_eq!(r.theme.quote_prefix.as_deref(), Some("\u{201C}"));
    assert_eq!(r.theme.quote_bg, hex(0xfafafa));
    assert!(r.theme.quote_italic);
}

#[test]
fn structural_and_attribute_selectors() {
    let r = imp("table tr:nth-child(2n) { background-color: #f6f8fa }
         table thead tr th { background: #eaeaea }
         .md-task-list-item > input[type=\"checkbox\"] { border: 1px solid #aaaaaa }
         .md-task-list-item > input:checked { background-color: #22aa22 }
         #write > p:first-child { color: #ff0000 }
         #write > *:last-child { color: #00ff00 }
         ul ul { list-style-type: circle } ul { list-style: square inside } ol { list-style-type: lower-roman }");
    assert_eq!(r.theme.table_stripe_bg, hex(0xf6f8fa));
    assert_eq!(r.theme.table_header_bg, hex(0xeaeaea));
    assert_eq!(r.theme.checkbox_border, hex(0xaaaaaa));
    assert_eq!(r.theme.checkbox_checked, hex(0x22aa22));
    assert_ne!(r.theme.text, hex(0xff0000), "generic paragraph is not the first child");
    assert_ne!(r.theme.text, hex(0x00ff00));
    assert_eq!(r.theme.list_styles.bullets, vec!["square", "circle", "circle"]);
    assert_eq!(r.theme.list_styles.numbers, vec!["lower-roman"]);
}

#[test]
fn state_selectors_do_not_apply() {
    let r = imp("a:hover { color: #ff0000 } a { color: #0000ff } .on-focus-mode p { color: #00ff00 }");
    assert_eq!(r.theme.link, hex(0x0000ff));
    assert_ne!(r.theme.text, hex(0x00ff00));
}

#[test]
fn invalid_selector_drops_the_whole_rule() {
    // like a browser: one invalid selector invalidates the rule
    let r = imp("p, p::-moz-selection { color: #ff0000 } p:has(a), h1 { color: #00ff00 }");
    assert_ne!(r.theme.text, hex(0xff0000));
    assert_eq!(
        r.theme.headings[0].color,
        hex(0x00ff00),
        ":has() drops only its selector"
    );
    let names: Vec<_> = r.report.unsupported_selectors.iter().map(|i| i.name.clone()).collect();
    assert!(
        names.iter().any(|n| n.starts_with("invalid: p::-moz-selection")),
        "{names:?}"
    );
    assert!(names.iter().any(|n| n.contains(":has()")), "{names:?}");
}

#[test]
fn report_lists_unsupported_items_with_lines() {
    let r = imp(
        "p {\n  color: #111;\n  frobnicate: 1;\n  margin: calc(1px + 2px);\n  transition: all 1s;\n}\n@keyframes spin { from { color: red } }\n@media (scan: interlace) { p { color: red } }\n.never-matches { frobnicate: 2 }",
    );
    let rep = &r.report;
    let prop = &rep.unsupported_properties[0];
    assert_eq!(prop.name, "frobnicate");
    assert_eq!(prop.count, 1, "only rules matching modelled elements count");
    assert_eq!(prop.locations[0].line, 3);
    assert_eq!(rep.describe(prop.locations[0]), "<input>:3");
    assert_eq!(rep.unsupported_values[0].name, "margin: calc(1px + 2px)");
    assert_eq!(rep.unsupported_values[0].locations[0].line, 4);
    assert!(
        rep.unsupported_at_rules
            .iter()
            .any(|i| i.name == "@keyframes" && i.locations[0].line == 7)
    );
    assert!(rep.unsupported_at_rules.iter().any(|i| i.name == "@media (scan)"));
    assert_eq!(rep.ignored, 1, "transition");
    assert_eq!(rep.applied, 1);
    assert_eq!(rep.content_declarations, 4);
    assert!((rep.coverage() - 1.0 / 3.0).abs() < 1e-9);
}

#[test]
fn media_width_queries_use_the_nominal_viewport() {
    let opts = ImportOptions {
        viewport_width: 1200.0,
        ..Default::default()
    };
    let css = "#write { max-width: 800px } @media (min-width: 1100px) { #write { max-width: 1000px } }
               @media screen and (max-width: 600px) { #write { max-width: 400px } }
               @media not print { p { color: #123456 } }
               @media (prefers-dark-interface) { h1 { color: #ffffff } }";
    let r = import_css(css, Path::new("."), &opts);
    assert_eq!(r.theme.column_max_width, 1000.0);
    assert_eq!(r.theme.text, hex(0x123456));
    assert!(r.theme.scheme_aware, "prefers-dark-interface is a dark media alias");
    assert_eq!(r.dark_variant.unwrap().headings[0].color, hex(0xffffff));
}

#[test]
fn css_nesting() {
    let r = imp("#write { h1 { color: #101010 } & > p { color: #202020 } .md-fences { background: #303030 } }");
    assert_eq!(r.theme.headings[0].color, hex(0x101010));
    assert_eq!(r.theme.text, hex(0x202020));
    assert_eq!(r.theme.code_block.background, hex(0x303030));
}

#[test]
fn background_shorthand_and_colors() {
    let r = imp(
        "pre.md-fences { background: url(x.png) repeat-x, rgba(0, 0, 0, 0.5) } mark { background: hsl(60, 100%, 50%) }
         hr { border: none; height: 1px; background-color: #cccccc }",
    );
    // half-transparent black over the light page
    let bg = r.theme.code_block.background;
    assert_eq!(bg.0[3], 255);
    assert!(bg.0[0] > 100 && bg.0[0] < 140, "{bg:?}");
    assert_eq!(r.theme.highlight_bg, hex(0xffff00));
    assert_eq!(r.theme.rule, hex(0xcccccc));
}

#[test]
fn code_block_codemirror_paints_over_pre() {
    let r = imp(
        ".md-fences { background: #eeeeee; border: 1px solid #dddddd } .CodeMirror { background: #222222; color: #fafafa } .cm-s-inner .cm-number { color: #ff8800; font-weight: bold }",
    );
    assert_eq!(r.theme.code_block.background, hex(0x222222));
    assert_eq!(r.theme.code_block.text, hex(0xfafafa));
    assert_eq!(r.theme.code_block.border, Some(hex(0xdddddd)));
    assert_eq!(r.theme.syntax.number, hex(0xff8800));
    assert!(r.theme.syntax_tokens.iter().any(|t| t.class == "cm-number" && t.bold));
}

#[test]
fn import_relative_with_cycle_and_depth_limits() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir(d.join("sub")).unwrap();
    std::fs::write(d.join("theme.css"), "@import 'sub/a.css'; @import url(\"missing.css\"); @import url(https://fonts.example/x.css); h1 { color: #111111 }").unwrap();
    std::fs::write(d.join("sub/a.css"), "@import \"b.css\"; p { color: #222222 }").unwrap();
    std::fs::write(d.join("sub/b.css"), "@import \"a.css\"; a { color: #333333 }").unwrap();
    let r = import_theme_file(&d.join("theme.css"), &ImportOptions::default()).unwrap();
    assert_eq!(r.theme.name, "Theme");
    assert_eq!(r.theme.headings[0].color, hex(0x111111));
    assert_eq!(r.theme.text, hex(0x222222));
    assert_eq!(r.theme.link, hex(0x333333));
    assert_eq!(r.report.files.len(), 3);
    let w = r.report.warnings.join("\n");
    assert!(w.contains("cycle"), "{w}");
    assert!(w.contains("missing.css"), "{w}");
    assert!(r.report.unsupported_at_rules.iter().any(|i| i.name.contains("remote")));

    // depth limit
    for i in 0..15 {
        std::fs::write(d.join(format!("d{i}.css")), format!("@import 'd{}.css'; ", i + 1)).unwrap();
    }
    std::fs::write(d.join("d15.css"), "p { color: #444444 }").unwrap();
    let r = import_theme_file(&d.join("d0.css"), &ImportOptions::default()).unwrap();
    assert!(r.report.warnings.iter().any(|w| w.contains("depth limit")));
    assert_ne!(r.theme.text, hex(0x444444));
}

#[test]
fn imports_are_confined_to_the_theme_folder_ti9() {
    let outer = tempfile::tempdir().unwrap();
    let o = outer.path();
    let theme_dir = o.join("themes");
    std::fs::create_dir_all(theme_dir.join("assets")).unwrap();
    std::fs::write(o.join("secret.css"), "h1 { color: #ff0000 }").unwrap();
    let abs = o.join("secret.css").canonicalize().unwrap();
    std::fs::write(theme_dir.join("assets/ok.css"), "a { color: #00aa00 }").unwrap();
    // `..` that stays inside the folder is fine
    std::fs::write(theme_dir.join("assets/up.css"), "@import '../inside.css';").unwrap();
    std::fs::write(theme_dir.join("inside.css"), "p { color: #0000aa }").unwrap();
    std::fs::write(
        theme_dir.join("t.css"),
        format!(
            "@import 'assets/ok.css'; @import 'assets/up.css'; @import '../secret.css';\n\
             @import 'assets/../../secret.css'; @import url('{}'); @import 'file:///etc/x.css';",
            abs.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(o.join("secret.css"), theme_dir.join("link-out.css")).unwrap();
        std::os::unix::fs::symlink(theme_dir.join("inside.css"), theme_dir.join("assets/link-in.css")).unwrap();
        let mut t = std::fs::read_to_string(theme_dir.join("t.css")).unwrap();
        t.push_str("\n@import 'link-out.css'; @import 'assets/link-in.css';");
        std::fs::write(theme_dir.join("t.css"), t).unwrap();
    }
    let r = import_theme_file(&theme_dir.join("t.css"), &ImportOptions::default()).unwrap();
    assert_eq!(r.theme.link, hex(0x00aa00), "subfolder import applies");
    assert_eq!(r.theme.text, hex(0x0000aa), "`..` inside the folder applies");
    assert_ne!(
        r.theme.headings[0].color,
        hex(0xff0000),
        "nothing outside the folder applies"
    );
    let rep = &r.report;
    let count = |name: &str| {
        rep.unsupported_at_rules
            .iter()
            .find(|i| i.name == name)
            .map_or(0, |i| i.count)
    };
    let outside = if cfg!(unix) { 3 } else { 2 };
    assert_eq!(
        count("@import (outside the theme folder, rejected)"),
        outside,
        "{:?}",
        rep.unsupported_at_rules
    );
    assert_eq!(
        count("@import (absolute path, rejected)"),
        2,
        "{:?}",
        rep.unsupported_at_rules
    );
    assert!(rep.warnings.iter().any(|w| w.contains("outside the theme folder")));
    assert!(!rep.files.iter().any(|f| f.ends_with("secret.css")));

    // a string import is confined to the given base directory
    let r = import_css("@import '../secret.css';", &theme_dir, &ImportOptions::default());
    assert_ne!(r.theme.headings[0].color, hex(0xff0000));
    assert!(r.report.warnings.iter().any(|w| w.contains("outside the theme folder")));
}

#[test]
fn layering_user_css_files() {
    // spec 03 §1.2: theme → base.user.css → {name}.user.css
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(
        d.join("my-first_theme.css"),
        "h1 { color: #111111 } h2 { color: #111111 } h3 { color: #111111 }",
    )
    .unwrap();
    std::fs::write(d.join("base.user.css"), "h2 { color: #222222 } h3 { color: #222222 }").unwrap();
    std::fs::write(d.join("my-first_theme.user.css"), "h3 { color: #333333 }").unwrap();
    std::fs::write(d.join("other.user.css"), "h1 { color: #999999 }").unwrap();
    let r = import_theme_file(&d.join("my-first_theme.css"), &ImportOptions::default()).unwrap();
    assert_eq!(r.theme.name, "My First Theme", "A20");
    let h = &r.theme.headings;
    assert_eq!(
        (h[0].color, h[1].color, h[2].color),
        (hex(0x111111), hex(0x222222), hex(0x333333))
    );
    assert_eq!(r.report.files.len(), 3);
}

#[test]
fn display_names_a20() {
    assert_eq!(display_name("my-first-theme"), "My First Theme");
    assert_eq!(display_name("solarized_dark"), "Solarized Dark");
    assert_eq!(display_name("drake-vue3"), "Drake Vue3");
    assert_eq!(display_name("OneDark"), "OneDark");
}

#[test]
fn font_face_records_local_files_and_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir(d.join("fonts")).unwrap();
    std::fs::write(d.join("fonts/body-latin.woff2"), b"x").unwrap();
    let css = "@font-face { font-family: 'Body Face'; font-style: italic; font-weight: 300 700;
                 src: local('Body Face'), url('fonts/body-latin.woff2') format('woff2'), url(fonts/nope.ttf);
                 unicode-range: U+0000-00FF, U+0131, U+0400-045F; font-display: swap }
               @font-face { font-family: Remote; src: url(https://x.example/a.woff2) format('woff2') }
               @font-face { src: url(a.ttf) }
               p { font-family: 'Body Face', serif }";
    let r = import_css(css, d, &ImportOptions::default());
    let faces = &r.theme.fonts.faces;
    assert_eq!(faces.len(), 2);
    let f = &faces[0];
    assert_eq!(f.family, "Body Face");
    assert!(f.italic);
    assert_eq!(f.weight, (300, 700));
    assert_eq!(f.unicode_range, vec![(0, 0xff), (0x131, 0x131), (0x400, 0x45f)]);
    assert_eq!(f.display.as_deref(), Some("swap"));
    // `local()` and `url()` are separate list entries
    assert_eq!(f.sources.len(), 3);
    assert_eq!(f.sources[0], FontSource::Local("Body Face".into()));
    match &f.sources[1] {
        FontSource::File { path, format, exists } => {
            assert!(path.ends_with("fonts/body-latin.woff2"));
            assert_eq!(format.as_deref(), Some("woff2"));
            assert!(*exists);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(&f.sources[2], FontSource::File { exists: false, .. }));
    assert!(matches!(&faces[1].sources[0], FontSource::Remote(_)));
    assert!(
        r.report
            .unsupported_at_rules
            .iter()
            .any(|i| i.name.contains("no font-family"))
    );
    assert_eq!(r.theme.fonts.body, vec!["Body Face", "serif"]);
}

#[test]
fn include_when_export_and_charset() {
    let r =
        imp("@charset \"utf-8\"; @include-when-export url(https://fonts.example/css?family=X); p { color: #121212 }");
    assert_eq!(r.include_when_export, vec!["https://fonts.example/css?family=X"]);
    assert_eq!(r.theme.text, hex(0x121212));
    assert!(
        r.report.unsupported_at_rules.is_empty(),
        "{:?}",
        r.report.unsupported_at_rules
    );
}

#[test]
fn opacity_fades_text() {
    let r = imp("p { color: #000000 } .md-meta { opacity: 0.5 } a { color: #ff0000; opacity: 50% }");
    assert_eq!(r.theme.inline.markers.map(|c| c.0[3]), Some(128));
    assert_eq!(r.theme.link.0[3], 128);
}

#[test]
fn limits_bound_the_work() {
    let opts = ImportOptions {
        max_rules: 10,
        ..Default::default()
    };
    let css: String = (0..100).map(|i| format!("p {{ color: #{i:06x} }}\n")).collect();
    let r = import_css(&css, Path::new("."), &opts);
    assert_eq!(r.report.rules, 10);
    assert!(r.report.warnings.iter().any(|w| w.contains("rule limit")));
    assert_eq!(r.theme.text, hex(9));

    let opts = ImportOptions {
        max_selectors: 100,
        ..Default::default()
    };
    let css = format!("{} {{ color: red }} p {{ color: blue }}", vec!["a"; 150].join(","));
    let r = import_css(&css, Path::new("."), &opts);
    assert!(r.report.warnings.iter().any(|w| w.contains("selector limit")));
    assert_eq!(r.report.rules, 1);

    let opts = ImportOptions {
        max_file_bytes: 64,
        ..Default::default()
    };
    let r = import_css(&"p { color: red }".repeat(100), Path::new("."), &opts);
    assert!(r.report.warnings.iter().any(|w| w.contains("truncated")));
}

#[test]
fn garbage_never_panics() {
    for css in [
        "",
        "{",
        "}}}}",
        "p { color: ",
        "@media",
        "@media (",
        "@import",
        "@font-face { src: url(",
        "p:nth-child(",
        "a[",
        "#write { & { & { & { color: red } } } }",
        "p { --x: var(--x); color: var(--x) }",
        ":root { --a: var(--b); --b: var(--a) } p { color: var(--a, red) }",
        "\u{0}\u{feff}p{color:red}",
        "p { color: rgb(1e400, -1, nan) }",
        "/* unterminated",
        "p { font: 12px/ }",
        "p { font-size: 1e38em } h1 { font-size: -5px }",
        "@media print { p { color: red }",
        "h1::before { content: attr(x) counter(y) }",
    ] {
        let _ = imp(css);
    }
}

#[test]
fn deep_nesting_is_bounded() {
    let deep = format!("{}{}", "@media screen {".repeat(200), "}".repeat(200));
    let _ = imp(&deep);
    let deep = format!("p {{ color: {}red{} }}", "rgb(".repeat(500), ")".repeat(500));
    let _ = imp(&deep);
    let deep = format!("{} {{ color: red }}", ":is(".repeat(100) + "p" + &")".repeat(100));
    let _ = imp(&deep);
}

mod fuzz {
    use super::*;
    use proptest::prelude::*;

    fn fragment() -> impl Strategy<Value = String> {
        let sel = prop::sample::select(vec![
            "#write h1",
            "h2",
            "p",
            "a:hover",
            ".md-fences",
            "table tr:nth-child(2n)",
            "blockquote::before",
            ":root",
            "body",
            "*",
            "ul > li",
            "h3 + p",
            ".cm-s-inner .cm-keyword",
            "input:checked",
            "&",
            ":not(p)",
            ":is(h1, h2)",
            "p:nth-child(2n+1)",
            "[lang=x]",
            "::selection",
            "@",
            "",
        ]);
        let prop_name = prop::sample::select(vec![
            "color",
            "background",
            "font-size",
            "margin",
            "border",
            "font",
            "--x",
            "--y",
            "line-height",
            "content",
            "padding",
            "border-left",
            "font-family",
            "max-width",
            "opacity",
            "bogus",
        ]);
        let val = prop::sample::select(vec![
            "red",
            "#fff",
            "1em",
            "var(--x)",
            "var(--y, var(--x))",
            "rgba(0,0,0,.5)",
            "1px solid #000",
            "12px/1.5 serif",
            "'a'",
            "calc(1px+2px)",
            "none",
            "inherit",
            "var(",
            "100%",
            "-3rem",
            "url(x)",
        ]);
        prop_oneof![
            (sel.clone(), prop_name, val).prop_map(|(s, p, v)| format!("{s} {{ {p}: {v} }}")),
            sel.clone()
                .prop_map(|s| format!("@media (prefers-color-scheme: dark) {{ {s} {{ color: #000 }} }}")),
            sel.prop_map(|s| format!("{s} {{ h1 {{ color: red }} & p {{ color: blue }} }}")),
            Just("@import 'x.css';".to_string()),
            Just("@font-face { font-family: x; src: url(y.woff) }".to_string()),
            "[ -~]{0,40}".prop_map(|s| s),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

        #[test]
        fn arbitrary_bytes_never_panic(s in "\\PC{0,400}") {
            let _ = imp(&s);
        }

        #[test]
        fn structured_css_never_panics_and_is_fast(frags in prop::collection::vec(fragment(), 0..40)) {
            let css = frags.join("\n");
            let start = Instant::now();
            let r = imp(&css);
            prop_assert!(start.elapsed() < Duration::from_secs(2));
            prop_assert!(r.theme.font_size.is_finite() && r.theme.line_height.is_finite());
        }
    }
}

#[test]
fn large_input_is_fast() {
    // 20k rules of mixed selectors: matching and cascade stay well within budget
    let mut css = String::new();
    for i in 0..5000 {
        css.push_str(&format!(
            "#write h{} {{ color: #{:06x}; margin: {}px 0 }}\n.cm-s-inner .cm-x{i} {{ color: red }}\n#typora-sidebar .item{i}:hover {{ background: blue }}\n:root {{ --v{i}: {i}px }}\n",
            i % 6 + 1,
            i,
            i % 40
        ));
    }
    let start = Instant::now();
    let r = imp(&css);
    let el = start.elapsed();
    assert_eq!(r.report.rules, 20_000);
    assert!(
        el < Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 }),
        "{el:?}"
    );
}

#[test]
fn sidebar_chrome_selectors_map_to_the_sidebar_style() {
    let r = imp(r#"
body { font-family: Georgia; color: #222; font-size: 16px }
#typora-sidebar { background: #20242c; font-family: "Fira Sans", sans-serif; font-size: 14px; color: #c8ccd4 }
.file-node-content { line-height: 30px; padding-left: 10px }
.file-tree-node.active > .file-node-content { background: #2f3542; color: #ffffff; border-radius: 6px }
.outline-item-active { color: #e5c07b }
.outline-h3 { padding-left: 32px }
.outline-h1 { padding-left: 4px }
:root { --item-hover-bg-color: #282c34 }
"#);
    let s = &r.theme.sidebar;
    assert_eq!(s.fonts, vec!["Fira Sans", "sans-serif"]);
    assert_eq!(s.font_size, Some(14.0));
    assert_eq!(s.text, Some(hex(0xc8ccd4)));
    assert_eq!(s.active_bg, Some(hex(0x2f3542)));
    assert_eq!(s.active_text, Some(hex(0xffffff)));
    assert_eq!(s.outline_active_text, Some(hex(0xe5c07b)));
    assert_eq!(s.hover_bg, Some(hex(0x282c34)));
    assert_eq!(s.row_height, Some(30.0));
    assert_eq!(s.padding_x, Some(10.0));
    assert_eq!(s.indent, Some(14.0), "(32 - 4) / 2 levels");
    assert_eq!(s.radius, Some(6.0));
    assert_eq!(r.theme.chrome.sidebar_bg, hex(0x20242c));

    // a theme that does not style the sidebar: no fonts or sizes inherited from the content
    let r = imp("body { font-family: Georgia; font-size: 18px; color: #333 }");
    let s = &r.theme.sidebar;
    assert!(s.fonts.is_empty() && s.font_size.is_none() && s.text.is_none());
}
