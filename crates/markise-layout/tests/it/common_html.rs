//! Issue #103 / spec01 §3.14: presentation only, preserving raw HTML and source bytes.
use markise_doc::{DocPos, FormatDoc, PatchSet};
use markise_layout::{
    View,
    block::{NoImages, Prim},
    testing,
};
use markise_theme::Theme;
use std::sync::Arc;
fn view(s: &str) -> View {
    testing::Shown::new(Arc::new(markise_md::parse(s))).view(Theme::light(), 600.0, 1.0)
}
fn shown(v: &mut View) -> String {
    let mut fonts = testing::fonts();
    let mut sh = fonts.with_pixels_per_point(1.0);
    v.frame(2000.0, 0.0, &mut sh, &NoImages)
        .iter()
        .flat_map(|v| &v.layout.prims)
        .filter_map(|p| {
            if let Prim::Text { text, .. } = p {
                Some(text.text().to_owned())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn nested_details_open_attribute_toggle_and_bytes() {
    let src = "Before\n\n<details>\n<summary>Outer &amp; title</summary>\n\nOuter body\n\n<details open>\n<summary>Inner</summary>\n\nInner body\n\n</details>\n</details>\n\nAfter\n";
    let mut v = view(src);
    let first = shown(&mut v);
    assert!(first.contains("Outer & title"), "{first}");
    assert!(
        !first.contains("Outer body") && !first.contains("Inner body"),
        "{first}"
    );
    let outer = v
        .model()
        .items
        .iter()
        .position(|i| {
            matches!(
                i.raw.as_deref(),
                Some(&markise_doc::RawPresentation::DisclosureStart { .. })
            )
        })
        .unwrap();
    v.toggle_expanded(outer);
    let expanded = shown(&mut v);
    assert!(
        expanded.contains("Outer body") && expanded.contains("Inner body"),
        "{expanded}"
    );
    assert!(
        !expanded.contains("<details") && !expanded.contains("</details"),
        "{expanded}"
    );
    assert_eq!(v.doc().text(), src);
    v.toggle_expanded(outer);
    assert!(!shown(&mut v).contains("Outer body"));
}
#[test]
fn standalone_summary_unpaired_and_unknown_remain_safe() {
    let mut v = view(
        "<details open>\n\n<summary>Title</summary>\n\nContent\n\n</details>\n\n<details>\n\nUnpaired body\n\n<widget>unknown</widget>\n",
    );
    let text = shown(&mut v);
    assert!(text.contains("Title") && text.contains("Content"), "{text}");
    assert!(!text.contains("<summary>"), "{text}");
    assert!(text.contains("<details>") && text.contains("unknown"), "{text}");
}
#[test]
fn comments_and_keyboard_markers_hide_and_reveal() {
    let src =
        "Before\n\n<!-- hidden block -->\n\nPress <kbd>Ctrl</kbd> + <kbd>Enter</kbd><!-- hidden inline --> now.\n";
    let mut v = view(src);
    let text = shown(&mut v);
    assert!(text.contains("Ctrl") && text.contains("Enter"), "{text}");
    assert!(!text.contains("hidden") && !text.contains("kbd"), "{text}");
    let at = src.find("hidden block").unwrap();
    let focus = markise_layout::caret::focus_for(&v, DocPos::new(at), true);
    v.set_focus(focus);
    assert!(shown(&mut v).contains("hidden block"));
    let at = src.find("hidden inline").unwrap();
    let focus = markise_layout::caret::focus_for(&v, DocPos::new(at), true);
    v.set_focus(focus);
    assert!(shown(&mut v).contains("<!-- hidden inline -->"));
    let at = src.find("Ctrl").unwrap();
    let focus = markise_layout::caret::focus_for(&v, DocPos::new(at), true);
    v.set_focus(focus);
    assert!(shown(&mut v).contains("<kbd>"));
    assert_eq!(v.doc().text(), src);
}
#[test]
fn collapsed_content_is_not_laid_out_and_caret_can_reveal_it() {
    let src = format!("<details>\n\n{}\n</details>\n\nAfter\n", "paragraph\n\n".repeat(500));
    let mut v = view(&src);
    shown(&mut v);
    assert!(v.laid_out.len() < 10, "{}", v.laid_out.len());
    let pos = src.find("paragraph").unwrap();
    let focus = markise_layout::caret::focus_for(&v, DocPos::new(pos), true);
    v.set_focus(focus);
    assert!(shown(&mut v).contains("paragraph"));
}
#[test]
fn incremental_changes_repair_pairing_without_losing_toggled_state() {
    let src = "Before\n\n<details>\n<summary>Title</summary>\n\nBody\n\n</details>\n\nAfter\n";
    let mut d = markise_md::parse(src);
    let mut v = view(src);
    shown(&mut v); // a paged model's pages near the viewport come in with the first frame
    v.toggle_expanded(1);
    assert!(shown(&mut v).contains("Body"));
    let at = d.text().find("Body").unwrap();
    let edit = d.apply(&PatchSet::single_verbatim(at..at + 4, "Changed"));
    let change = v.model_mut().apply(&d, &edit);
    v.apply_change(Arc::new(d.clone()), &change);
    assert!(shown(&mut v).contains("Changed"));
    let at = d.text().find("</details>").unwrap();
    let edit = d.apply(&PatchSet::single_verbatim(at..at + 10, "</other>"));
    let change = v.model_mut().apply(&d, &edit);
    v.apply_change(Arc::new(d.clone()), &change);
    assert!(shown(&mut v).contains("<details>"));
}

#[test]
fn quoted_disclosures_pair_only_inside_their_own_container() {
    let mut v = view(
        "> <details>\n> <summary>Quoted</summary>\n>\n> Hidden quote body\n>\n> </details>\n\n<details>\n\n> </details>\n\nUnpaired\n",
    );
    let text = shown(&mut v);
    assert!(text.contains("Quoted") && !text.contains("Hidden quote body"), "{text}");
    assert!(
        text.contains("<details>"),
        "cross-container closer must not consume opener: {text}"
    );
}

#[test]
fn caret_navigation_skips_collapsed_children() {
    let src = "Before\n\n<details>\n<summary>Hint</summary>\n\nHidden\n\n</details>\n\nAfter\n";
    let mut v = view(src);
    shown(&mut v);
    let mut fonts = testing::fonts();
    let mut shaper = fonts.with_pixels_per_point(1.0);
    let mut geo = markise_layout::caret::Geometry {
        view: &mut v,
        shaper: &mut shaper,
        images: &NoImages,
    };
    let after = src.find("After").unwrap();
    let prev = geo.horizontal(DocPos::new(after), false, false);
    assert!(prev.offset < src.find("Hidden").unwrap(), "{prev:?}");
    let end = geo.doc_edge(true);
    assert!(end.offset >= after, "{end:?}");
}
