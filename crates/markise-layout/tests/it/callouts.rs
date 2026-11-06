//! Extended callouts in the view (ADR 0025): the header shows the title or the capitalised name,
//! the tag never; colour and icon come from the theme.
use markise_layout::{
    View,
    block::{NoImages, Prim},
    testing,
};
use markise_theme::Theme;
use std::sync::Arc;

fn view_in(s: &str, theme: Theme) -> View {
    testing::Shown::new(Arc::new(markise_md::parse(s))).view(theme, 600.0, 1.0)
}

fn prims(v: &mut View) -> Vec<Prim> {
    let mut fonts = testing::fonts();
    let mut sh = fonts.with_pixels_per_point(1.0);
    v.frame(4000.0, 0.0, &mut sh, &NoImages)
        .iter()
        .flat_map(|v| v.layout.prims.clone())
        .collect()
}

fn texts(v: &mut View) -> Vec<String> {
    prims(v)
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_header_shows_the_title_or_the_name() {
    let t = |s: &str| texts(&mut view_in(s, Theme::light()));
    assert_eq!(t("> [!warning] Mind this\n> Body\n"), ["Mind this", "Body"]);
    assert_eq!(t("> [!warning]\n> Body\n"), ["Warning", "Body"]);
    assert_eq!(t("> [!faq]\n> Body\n"), ["Faq", "Body"]);
    assert_eq!(t("> [!my-thing]\n> Body\n"), ["My-thing", "Body"], "an unknown name");
    assert_eq!(t("> [!NOTE]\n> Body\n"), ["Note", "Body"]);
    assert_eq!(t("> [!info] Only a title\n"), ["Only a title"]);
}

#[test]
fn inline_markdown_in_a_title() {
    let shown = texts(&mut view_in("> [!tip] A **bold** [link](u)\n", Theme::light()));
    assert_eq!(shown, ["A bold link"]);
}

fn accent_of(prims: &[Prim]) -> Vec<epaint::Color32> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { stroke, .. } => Some(stroke.color),
            Prim::Rect { fill, .. } => Some(*fill),
            Prim::Path { fill, stroke, .. } => Some(if stroke.width > 0.0 { stroke.color } else { *fill }),
            Prim::Circle { fill, stroke, .. } => Some(if stroke.width > 0.0 { stroke.color } else { *fill }),
            _ => None,
        })
        .collect()
}

#[test]
fn colours_follow_the_theme() {
    for (theme, name) in [(Theme::light(), "light"), (Theme::dark(), "dark")] {
        let slot = |a: usize| {
            let c = theme.alert[a].0;
            epaint::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
        };
        for (src, want) in [
            ("> [!info] T\n", slot(0)),
            ("> [!success] T\n", slot(1)),
            ("> [!example] T\n", slot(2)),
            ("> [!question] T\n", slot(3)),
            ("> [!danger] T\n", slot(4)),
            ("> [!mine] T\n", slot(0)),
        ] {
            let colours = accent_of(&prims(&mut view_in(src, theme.clone())));
            assert!(
                !colours.is_empty() && colours.contains(&want),
                "{name} {src:?}: {colours:?} lacks {want:?}"
            );
        }
    }
}

/// A bare alert draws no icon: it looks as it did before callouts.
#[test]
fn only_extended_callouts_have_an_icon() {
    let icon_prims = |s: &str| {
        prims(&mut view_in(s, Theme::light()))
            .iter()
            .filter(|p| matches!(p, Prim::Path { .. } | Prim::Circle { .. }))
            .count()
    };
    assert_eq!(icon_prims("> [!NOTE]\n> x\n"), 0);
    assert!(icon_prims("> [!note] T\n> x\n") > 0);
    assert!(icon_prims("> [!info]\n> x\n") > 0);
}

/// Every position of a title is a caret stop, and typing there re-lays out as a title.
#[test]
fn a_title_is_text_with_source_positions() {
    let src = "> [!warning] Title\n> Body\n";
    let mut v = view_in(src, Theme::light());
    let ps = prims(&mut v);
    let map = ps
        .iter()
        .find_map(|p| match p {
            Prim::Text { text, .. } if text.text() == "Title" => text.source_map().cloned(),
            _ => None,
        })
        .expect("a mapped title");
    let at = src.find("Title").unwrap() as u32 - src.find("[!").unwrap() as u32;
    assert_eq!(
        map.to_source(0),
        Some(at),
        "the title's first byte, relative to the item"
    );
}
