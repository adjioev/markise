//! Built-in themes (ADR 0004): every vendored theme imports from its extracted copy without
//! panic, with high coverage, and its distinctive colors reach the native theme.

use markise_theme::catalog::{VariantSource, builtin_families};
use markise_theme::{ImportOptions, ImportResult, Rgba, builtin, import_theme_file};

fn import_all() -> Vec<(String, ImportResult, ImportResult)> {
    let dir = tempfile::tempdir().unwrap();
    let root = builtin::extract(dir.path()).unwrap();
    let mut out = Vec::new();
    for f in builtin_families(&root) {
        let load = |v: &Option<VariantSource>| match v {
            Some(VariantSource::Css { path, .. }) => import_theme_file(path, &ImportOptions::default()).unwrap(),
            other => panic!("unexpected {other:?}"),
        };
        out.push((f.name.clone(), load(&f.light), load(&f.dark)));
    }
    out
}

fn hex(v: u32) -> Rgba {
    Rgba::hex(v)
}

#[test]
fn builtins_import_with_high_coverage_and_their_own_colors() {
    let all = import_all();
    assert_eq!(all.len(), 3);
    for (name, light, dark) in &all {
        for (variant, r) in [("light", light), ("dark", dark)] {
            let rep = &r.report;
            assert!(rep.warnings.is_empty(), "{name} {variant}: {:?}", rep.warnings);
            assert!(
                rep.coverage() >= 0.90,
                "{name} {variant}: coverage {:.3}",
                rep.coverage()
            );
            assert!(rep.applied >= 100, "{name} {variant}: applied {}", rep.applied);
        }
        assert!(!light.theme.dark, "{name} light variant has light chrome");
        assert!(dark.theme.dark, "{name} dark variant has dark chrome");
        assert_ne!(light.theme.page_bg, dark.theme.page_bg, "{name}");
    }
    let get = |n: &str| all.iter().find(|(name, ..)| name == n).unwrap();

    // Drake: red-orange accent links, pink quote fill, centred h1, monospace body stack
    let (_, l, d) = get("Drake");
    assert_eq!(l.theme.link, hex(0xd63200));
    assert_eq!(l.theme.quote_bg, hex(0xfdefee));
    assert_eq!(l.theme.headings[0].color, hex(0x273849));
    assert_eq!(l.theme.heading_decor[0].align, markise_theme::TextAlign::Center);
    assert_eq!(l.theme.fonts.body.first().map(String::as_str), Some("Iosevka Curly"));
    assert_eq!(d.theme.page_bg, hex(0x2b2b2b));
    assert_eq!(d.theme.text, hex(0xacb7c4));

    // Lapis: blue headings, h2 as a filled pill, underlined links, justified text
    let (_, l, d) = get("Lapis");
    assert_eq!(l.theme.headings[0].color, hex(0x4870ac));
    assert_eq!(l.theme.heading_decor[1].background, Some(hex(0x4870ac)));
    assert!(l.theme.heading_decor[1].fit);
    assert!(l.theme.link_underline);
    assert_eq!(l.theme.text_align, markise_theme::TextAlign::Justify);
    assert_eq!(l.theme.quote_bar, hex(0x4870ac));
    assert_eq!(d.theme.page_bg, hex(0x1e222a));
    assert_eq!(d.theme.heading_decor[1].background, Some(hex(0x47556d)));

    // Phycat: gradient h2 pill approximated by its stop average, Optima body, abyss neon
    let (_, l, d) = get("Phycat");
    let bg = l.theme.heading_decor[1].background.expect("h2 background");
    assert!(bg.0[2] > bg.0[0] + 60, "blue-ish h2 pill, got {bg:?}");
    assert_eq!(l.theme.headings[1].color, hex(0xffffff));
    assert_eq!(l.theme.fonts.body.first().map(String::as_str), Some("Optima-Regular"));
    assert_eq!(l.theme.list_styles.bullets, vec!["disc", "circle", "square"]);
    assert_eq!(d.theme.page_bg, hex(0x0f111a));
    assert_eq!(d.theme.link, hex(0x00f3ff));
    // the Savor heading icons are gone from the vendored copy
    assert!(
        l.theme
            .heading_decor
            .iter()
            .skip(2)
            .all(|h| h.suffix.as_deref().is_none_or(|s| s.trim().is_empty()))
    );
}

#[test]
fn vendored_phycat_has_no_savor_icons() {
    for t in builtin::BUILTIN_THEMES {
        for f in t.files {
            if !f.path.ends_with(".css") {
                continue;
            }
            let text = String::from_utf8_lossy(f.bytes);
            assert!(
                !text.contains("icon-shape"),
                "{}/{} still has heading icons",
                t.id,
                f.path
            );
        }
    }
}

/// ADR 0007: an imported theme decomposes into a style and a color scheme that compose back
/// to the same theme, and each part combines with the native parts legibly.
#[test]
fn imported_themes_decompose_into_style_and_colors() {
    use markise_theme::style::{BUILTIN_STYLES, native_colors};
    use markise_theme::{Theme, compose};
    let all = import_all();
    let mut natives = vec![
        native_colors("markise-light").unwrap(),
        native_colors("markise-dark").unwrap(),
    ];
    natives.extend(
        markise_theme::palettes::PALETTE_THEMES
            .iter()
            .map(|p| native_colors(p.id).unwrap()),
    );
    let mut failures = Vec::new();
    let mut check = |what: String, t: &Theme| {
        for (i, h) in t.headings.iter().enumerate() {
            let bg = t.heading_decor[i].background.unwrap_or(t.page_bg);
            let c = h.color.contrast(bg);
            if c < 3.0 {
                failures.push(format!("{what} h{}: {c:.2}", i + 1));
            }
        }
    };
    for (name, light, dark) in &all {
        for r in [light, dark] {
            let t = &r.theme;
            assert_eq!(&compose(&t.style(), &t.colors()), t, "{name}: identity");
            // this theme's style with native colors
            for scheme in &natives {
                let x = compose(&t.style(), scheme);
                assert_eq!((x.page_bg, x.text, x.link), (scheme.page_bg, scheme.text, scheme.link));
                assert_eq!((x.fonts.clone(), x.font_size), (t.fonts.clone(), t.font_size));
                check(format!("{} style × {}", t.name, scheme.name), &x);
            }
            // this theme's colors with every built-in style
            for bs in BUILTIN_STYLES {
                let style = (bs.build)();
                let x = compose(&style, &t.colors());
                assert_eq!((x.page_bg, x.text, x.dark), (t.page_bg, t.text, t.dark));
                assert_eq!(x.fonts, style.fonts);
                check(format!("{} × {} colors", bs.name, t.name), &x);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
