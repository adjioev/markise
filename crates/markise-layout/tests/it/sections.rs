//! Sections (ADR 0026) do not depend on what a view model holds: a paged and a full model of the
//! same document, through random edits, agree on every boundary, and a boundary is a rule item
//! wherever items are real.

use std::sync::Arc;

use markise_doc::{FormatDoc, PatchSet};
use markise_layout::block::{NoImages, Prim};
use markise_layout::{BlockViewMode, DocModel, ItemKind, OutlineRow, Paging, View, testing};
use markise_theme::Theme;
use proptest::prelude::*;

fn line() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "",
        "",
        "",
        "text",
        "text",
        "---",
        "---",
        "***",
        "----",
        ":::a",
        ":::section-metadata",
        ":::",
        "key: value",
        "# heading",
        "- item",
        "> quote",
        "```",
    ])
}

fn doc_text() -> impl Strategy<Value = String> {
    prop::collection::vec(line(), 0..60).prop_map(|l| {
        let mut s = l.join("\n");
        s.push('\n');
        s
    })
}

fn boundaries(d: &dyn FormatDoc) -> Vec<usize> {
    (1..d.section_count())
        .map(|i| d.section(i).unwrap().boundary.unwrap().start)
        .collect()
}

/// The rule items of a model that are boundaries, reading a placeholder's items without
/// materializing it.
fn rule_items(m: &DocModel, d: &dyn FormatDoc) -> Vec<usize> {
    let mut out = Vec::new();
    let mut note = |kind: &ItemKind, start: usize| {
        if *kind == ItemKind::Rule && d.section_at(start).boundary.is_some_and(|b| b.start == start) {
            out.push(start);
        }
    };
    for i in 0..m.items.len() {
        if m.is_page(i) {
            for (start, it) in m.page_items(d, i) {
                note(&it.kind, start);
            }
        } else {
            note(&m.items[i].kind, m.item_start(i));
        }
    }
    out
}

proptest! {
    #[test]
    fn paged_and_full_models_agree_through_edits(
        s in doc_text(),
        ops in prop::collection::vec((any::<prop::sample::Index>(), 0usize..8, line(), any::<bool>()), 0..6),
    ) {
        let mut d = markise_md::parse(&s);
        let mut paged = DocModel::build_with(&d, None, Paging::Paged { tops: 3 });
        let mut full = DocModel::build_with(&d, None, Paging::Full);
        for (at, del, ins, newline) in ops {
            let len = d.text().len();
            let a = at.index(len + 1);
            let z = (a + del).min(len);
            let insert = if newline { format!("{ins}\n") } else { ins.to_string() };
            let e = FormatDoc::apply(&mut d, &PatchSet::single_verbatim(a..z, insert));
            paged.apply(&d, &e);
            full.apply(&d, &e);
            let want = boundaries(&d);
            prop_assert_eq!(&rule_items(&full, &d), &want);
            prop_assert_eq!(&rule_items(&paged, &d), &want);
        }
        let want = boundaries(&d);
        prop_assert_eq!(&rule_items(&full, &d), &want);
        prop_assert_eq!(&rule_items(&paged, &d), &want);
        // every boundary also lies in the item a jump would go to, placeholder or not
        for &b in &want {
            let i = paged.item_at_offset(b);
            prop_assert!(paged.is_page(i) || paged.items[i].kind == ItemKind::Rule, "{}", b);
        }
    }
}

proptest! {
    /// The outline's rows are the same whatever a model holds as real items, and they run in
    /// the document's order, each section before its headings.
    #[test]
    fn outline_rows_agree_on_paged_and_full_models(s in doc_text()) {
        let d: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(&s));
        let mut views = [Paging::Paged { tops: 3 }, Paging::Full].map(|paging| {
            let model = Arc::new(DocModel::build_with(&*d, None, paging));
            let mut v = View::new(d.clone(), model, Theme::light(), 700.0, 1.0);
            v.set_sections(true);
            v
        });
        let rows = |v: &View| (0..v.outline_row_count()).map(|r| v.outline_row(r)).collect::<Vec<_>>();
        let (a, b) = (rows(&views[0]), rows(&views[1]));
        prop_assert_eq!(&a, &b);
        // each kind once, in order
        let sections: Vec<usize> = a.iter().filter_map(|r| match r { OutlineRow::Section(k) => Some(*k), _ => None }).collect();
        let heads: Vec<usize> = a.iter().filter_map(|r| match r { OutlineRow::Heading(k) => Some(*k), _ => None }).collect();
        prop_assert_eq!(sections, (0..d.section_count()).collect::<Vec<_>>());
        prop_assert_eq!(heads, (0..views[1].model().outline.len()).collect::<Vec<_>>());
        // a heading comes after its section's row, by the full model's positions
        let m = views[1].model().clone();
        let mut current = 0;
        for r in &b {
            match r {
                OutlineRow::Section(k) => current = *k,
                OutlineRow::Heading(h) => {
                    let at = m.item_start(m.outline[*h].item);
                    prop_assert_eq!(d.section_at(at).index, current);
                }
            }
        }
        // a jump materializes the page it goes to, and only then
        let last = d.section_count() - 1;
        let i = views[0].ensure_section(last).unwrap();
        prop_assert!(!views[0].model().is_page(i));
    }
}

#[test]
fn asking_about_sections_materializes_nothing() {
    let mut s = String::from("# Doc\n\n");
    for i in 0..500 {
        s.push_str(&format!("---\n\n## Part {i}\n\nText.\n\n"));
    }
    let d = markise_md::parse(&s);
    let m = DocModel::build_with(&d, None, Paging::Paged { tops: 8 });
    let items = m.items.len();
    assert!((0..items).filter(|&i| m.is_page(i)).count() > 40);
    assert_eq!(d.section_count(), 501);
    assert_eq!(d.section_at(s.len() - 1).index, 500);
    assert_eq!(m.items.len(), items);
}

// ---- on screen

const DOC: &str = "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\nid: top\nkind: opening\nnot a pair\n\n:::\n\nFirst.\n\n---\n\nSecond.\n\n***\n\nStill second.\n\n---\n\n:::section-metadata\n\nstyle: wide\n\n:::\n\nThird.\n";

fn view_of(text: &str, on: bool) -> View {
    let d: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(text));
    let model = Arc::new(DocModel::build_with(&*d, None, Paging::Full));
    let mut v = View::new(d, model, Theme::light(), 700.0, 1.0);
    v.set_sections(on);
    v
}

fn lay_out(v: &mut View, i: usize) -> Arc<markise_layout::BlockLayout> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.layout(i, &mut f, &NoImages)
}

fn frame(v: &mut View) {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.frame(2000.0, 0.0, &mut f, &NoImages);
}

fn texts(l: &markise_layout::BlockLayout) -> Vec<String> {
    (l.prims.iter())
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.text().to_string()),
            _ => None,
        })
        .collect()
}

fn item_of(v: &View, needle: &str) -> usize {
    v.model().item_at_offset(v.doc().text().find(needle).unwrap())
}

/// The nth `---` of the text that is a line of its own, after the front matter.
fn rule_item(v: &View, section: usize) -> usize {
    let at = v.doc().section(section).unwrap().boundary.unwrap().start;
    v.model().item_at_offset(at)
}

#[test]
fn a_boundary_is_a_labelled_divider_and_other_rules_stay_lines() {
    let mut v = view_of(DOC, true);
    frame(&mut v);
    let (a, b) = (rule_item(&v, 1), rule_item(&v, 2));
    assert_eq!(texts(&lay_out(&mut v, a)), ["Section 2"], "no metadata: its number");
    assert_eq!(texts(&lay_out(&mut v, b)), ["Section 3 · wide"]);
    let line = item_of(&v, "***");
    assert!(texts(&lay_out(&mut v, line)).is_empty());
    assert!(
        !markise_layout::caret::has_stops(&lay_out(&mut v, a)),
        "no caret stands in it"
    );
    // the same height as the line it stands for, so nothing else moves
    assert_eq!(v.height(a), v.height(line));
}

#[test]
fn a_metadata_block_is_a_plaque_and_its_items_hide() {
    let mut v = view_of(DOC, true);
    frame(&mut v);
    let (opener, body) = (item_of(&v, ":::section-metadata"), item_of(&v, "id: top"));
    let closer = body + 1;
    let l = lay_out(&mut v, opener);
    assert!(texts(&l)[0].contains("id: top"));
    assert!(texts(&l)[0].contains("kind: opening"));
    assert!(texts(&l)[1].starts_with('⚠'), "{:?}", texts(&l));
    assert!(l.plaque.is_some() && l.warning.is_some());
    assert!(l.card_top.is_none(), "no block buttons on it");
    assert_eq!(v.height(body), 0.0);
    assert_eq!(v.height(closer), 0.0);
    assert!(!markise_layout::caret::has_stops(&l));
    assert_eq!(v.section_warnings(opener).unwrap().len(), 1);
}

#[test]
fn off_nothing_changes() {
    let mut v = view_of(DOC, false);
    frame(&mut v);
    let (a, body) = (rule_item(&v, 1), item_of(&v, "id: top"));
    assert!(texts(&lay_out(&mut v, a)).is_empty());
    assert!(v.height(body) > 0.0);
    let opener = item_of(&v, ":::section-metadata");
    let l = lay_out(&mut v, opener);
    assert!(l.plaque.is_none() && l.card_top.is_some());
}

#[test]
fn the_markdown_state_and_a_selection_inside_show_the_card() {
    let mut v = view_of(DOC, true);
    frame(&mut v);
    let (opener, body) = (item_of(&v, ":::section-metadata"), item_of(&v, "id: top"));
    let at = v.model().item_start(opener);
    assert!(v.is_section_metadata(at));
    v.set_block_modes(BlockViewMode::Native, vec![(at, BlockViewMode::Markdown)]);
    frame(&mut v);
    assert!(lay_out(&mut v, opener).plaque.is_none());
    assert!(v.height(body) > 0.0);
    v.set_block_modes(BlockViewMode::Native, Vec::new());
    frame(&mut v);
    assert_eq!(v.height(body), 0.0);
    // a selection in it: what is selected can be seen
    let inside = v.doc().text().find("kind:").unwrap();
    v.set_selection_for_sections(&[inside]);
    frame(&mut v);
    assert!(v.height(body) > 0.0);
    v.set_selection_for_sections(&[v.doc().text().find("First").unwrap()]);
    frame(&mut v);
    assert_eq!(v.height(body), 0.0);
}

#[test]
fn a_rule_in_a_block_and_a_metadata_block_not_first_stay_what_they_are() {
    let s = "Text.\n\n:::section-metadata\n\na: b\n\n:::\n\n:::card\n\n---\n\n:::\n";
    let mut v = view_of(s, true);
    frame(&mut v);
    let opener = item_of(&v, ":::section-metadata");
    assert!(lay_out(&mut v, opener).plaque.is_none());
    assert!(v.height(item_of(&v, "a: b")) > 0.0);
    let rule = item_of(&v, "---");
    assert!(texts(&lay_out(&mut v, rule)).is_empty());
}

#[test]
fn it_follows_the_theme_and_its_size() {
    let mut small = view_of(DOC, true);
    let mut large = view_of(DOC, true);
    let mut big = Theme::light();
    big.font_size *= 2.0;
    large.set_params(700.0, &big, 1.0);
    frame(&mut small);
    frame(&mut large);
    let (a, b) = (
        item_of(&small, ":::section-metadata"),
        item_of(&large, ":::section-metadata"),
    );
    let h = |v: &mut View, i| lay_out(v, i).plaque.unwrap().height();
    assert!(h(&mut large, b) > 1.5 * h(&mut small, a));
    // another theme is another layout, also of the words on a divider
    let dark = Theme {
        dark: true,
        rule: markise_theme::Rgba::hex(0x123456),
        ..Theme::light()
    };
    let i = rule_item(&small, 1);
    let before = lay_out(&mut small, i);
    small.set_params(700.0, &dark, 1.0);
    frame(&mut small);
    assert!(!Arc::ptr_eq(&before, &lay_out(&mut small, i)));
}

#[test]
fn editing_metadata_renames_the_divider_above_it_without_a_rebuild() {
    let s = "One.\n\n---\n\n:::section-metadata\n\nid: old\n\n:::\n\nTwo.\n";
    let mut d = markise_md::parse(s);
    let mut m = DocModel::build_with(&d, None, Paging::Full);
    let at = s.find("old").unwrap();
    let e = FormatDoc::apply(&mut d, &PatchSet::single_verbatim(at..at + 3, "new"));
    m.apply(&d, &e);
    let d: Arc<dyn FormatDoc> = Arc::new(d);
    let mut v = View::new(d, Arc::new(m), Theme::light(), 700.0, 1.0);
    v.set_sections(true);
    frame(&mut v);
    let i = rule_item(&v, 1);
    assert_eq!(texts(&lay_out(&mut v, i)), ["new"]);
}
