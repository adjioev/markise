//! Sections (ADR 0026) do not depend on what a view model holds: a paged and a full model of the
//! same document, through random edits, agree on every boundary, and a boundary is a rule item
//! wherever items are real.

use std::sync::Arc;

use markise_doc::{FormatDoc, PatchSet};
use markise_layout::block::{NoImages, Prim, merged_bands};
use markise_layout::caret::Geometry;
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
        let rows = |v: &View| (0..v.outline_row_count(None)).map(|r| v.outline_row(r, None)).collect::<Vec<_>>();
        let (a, b) = (rows(&views[0]), rows(&views[1]));
        prop_assert_eq!(&a, &b);
        // each kind once, in order
        let sections: Vec<usize> = a.iter().filter_map(|r| match r { OutlineRow::Section(k) => Some(*k), _ => None }).collect();
        let heads: Vec<usize> = a.iter().filter_map(|r| match r { OutlineRow::Heading(k) => Some(*k), _ => None }).collect();
        prop_assert_eq!(sections, (0..d.section_count()).collect::<Vec<_>>());
        // every heading in order, but for one that says what its section's row says
        prop_assert!(heads.windows(2).all(|w| w[0] < w[1]));
        let m = views[1].model().clone();
        for h in (0..m.outline.len()).filter(|h| !heads.contains(h)) {
            let at = m.item_start(m.outline[h].item);
            let section = d.section_at(at).index;
            let own = views[1].section_own_name(section);
            prop_assert_eq!(own.as_deref(), Some(m.outline[h].text.trim()));
        }
        // a heading comes after its section's row, by the full model's positions
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
fn outline_rows_list_what_they_are_told_and_a_current_heading_not_listed_goes_to_its_section() {
    let s = "# A\n\n:::q\n\n## In a block\n\n:::\n\n---\n\n## B\n\n:::q\n\n## In another\n\n:::\n\n## C\n";
    let d: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(s));
    let model = Arc::new(DocModel::build_with(&*d, None, Paging::Full));
    let mut v = View::new(d, model, Theme::light(), 700.0, 1.0);
    v.set_sections(true);
    assert_eq!(v.model().outline.len(), 5);
    let only = [0usize, 2, 4];
    use OutlineRow::*;
    let rows: Vec<_> = (0..v.outline_row_count(Some(&only)))
        .map(|r| v.outline_row(r, Some(&only)))
        .collect();
    // a first section's `#` is the title, with a row of its own; `## B` is what its section goes by, and has none
    assert_eq!(rows, [Section(0), Heading(0), Section(1), Heading(4)]);
    // the heading in a block is not listed: the one before it stands for it, here its section's row
    assert_eq!(v.outline_row_of(0, Some(1), Some(&only)), 1);
    assert_eq!(v.outline_row_of(1, Some(3), Some(&only)), 2);
    assert_eq!(
        v.outline_row_of(1, Some(2), Some(&only)),
        2,
        "the heading that names it: its row"
    );
    assert_eq!(
        v.outline_row_of(1, Some(1), Some(&only)),
        2,
        "before the section: the section"
    );
    assert_eq!(v.outline_row_of(1, Some(4), Some(&only)), 3);
    assert_eq!(v.outline_row_of(1, Some(4), None), 5);
    // no heading in a block names a section
    assert_eq!(v.section_name(1), "B");
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
    assert_eq!(texts(&lay_out(&mut v, a)), ["2"], "no metadata: its number");
    assert_eq!(texts(&lay_out(&mut v, b)), ["3"], "style is no name");
    let line = item_of(&v, "***");
    assert!(texts(&lay_out(&mut v, line)).is_empty());
    assert!(
        !markise_layout::caret::has_stops(&lay_out(&mut v, a)),
        "no caret stands in it"
    );
    assert!(v.height(a) > 2.0 * v.height(line), "clearly more than a rule");
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
fn a_closed_fold_and_sections_agree_on_heights() {
    let s = "---\nauthoring: blocks-v3\n---\n\n:::section-metadata\n\nid: top\n\n:::\n\n> [!note]- Folded\n>\n> Body.\n>\n> ---\n>\n> More.\n\n---\n\nAfter.\n";
    let mut v = view_of(s, true);
    frame(&mut v);
    assert_eq!(v.doc().section_count(), 2, "a rule in a callout is no boundary");
    let body = v.model().item_at_offset(s.find("Body.").unwrap());
    assert_eq!(v.height(body), 0.0, "folded");
    let inner = v.model().item_at_offset(s.find("> ---").unwrap() + 2);
    assert_eq!(v.height(inner), 0.0, "and the rule with it");
    assert_eq!(
        v.height(v.model().item_at_offset(s.find("id: top").unwrap())),
        0.0,
        "under the plaque"
    );
    let rule = rule_item(&v, 1);
    assert!(v.height(rule) > 0.0);
    // what estimates say comes to what layout says, with both kept out of the count
    let mut full = view_of(s, true);
    let d = full.doc().clone();
    let paged = Arc::new(DocModel::build_with(&*d, None, Paging::Paged { tops: 2 }));
    let mut p = View::new(d, paged, Theme::light(), 700.0, 1.0);
    p.set_sections(true);
    frame(&mut full);
    frame(&mut p);
    frame(&mut p);
    frame(&mut full);
    assert!(
        (full.total_height() - p.total_height()).abs() < 1.0,
        "{} {}",
        full.total_height(),
        p.total_height()
    );
}

#[test]
fn a_name_is_the_id_else_the_first_heading_else_the_number() {
    let s = "One.\n\n---\n\n:::section-metadata\n\nid: named\nkind: a\n\n:::\n\n## Heading\n\n---\n\n:::section-metadata\n\nkind: a\n\n:::\n\n:::card\n\n## In a block\n\n:::\n\n## Mine {#hidden}\n\n---\n\nNothing.\n";
    let v = view_of(s, true);
    let names: Vec<String> = (0..4).map(|i| v.section_name(i)).collect();
    assert_eq!(names, ["Section 1", "named", "Mine", "Section 4"]);
}

#[test]
fn a_rule_nearly_a_boundary_and_a_block_where_none_can_be_say_so() {
    let s = "One.\n\n----\n\n- - -\n\n***\n\n:::section-metadata\n\na: b\n\n:::\n\nTwo.\n";
    let mut v = view_of(s, true);
    frame(&mut v);
    let i = item_of(&v, "----");
    assert!(texts(&lay_out(&mut v, i))[0].starts_with("No section starts here"));
    let i = item_of(&v, "- - -");
    assert!(texts(&lay_out(&mut v, i))[0].starts_with("No section starts here"));
    let i = item_of(&v, "***");
    assert!(texts(&lay_out(&mut v, i)).is_empty());
    let i = item_of(&v, ":::section-metadata");
    let l = lay_out(&mut v, i);
    assert!(l.plaque.is_none() && l.card_top.is_some(), "a card");
    assert!(l.warning.is_some(), "that says why");
    assert!(texts(&l).iter().any(|t| t.contains("goes first")), "{:?}", texts(&l));
    assert_eq!(v.section_warnings(i).unwrap().len(), 1);
    // with sections off none of it
    let mut off = view_of(s, false);
    frame(&mut off);
    let i = item_of(&off, "----");
    assert!(texts(&lay_out(&mut off, i)).is_empty());
}

#[test]
fn an_unclosed_metadata_block_says_so_under_its_opener() {
    let s = "One.\n\n---\n\n:::section-metadata\n\n```\na: b\n```\n\nText.\n";
    let mut v = view_of(s, true);
    frame(&mut v);
    let i = item_of(&v, ":::section-metadata");
    let t = texts(&lay_out(&mut v, i));
    assert!(t.iter().any(|t| t.contains("no closing marker")), "{t:?}");
    let i = item_of(&v, "One.");
    assert!(!texts(&lay_out(&mut v, i)).iter().any(|t| t.contains('⚠')));
}

#[test]
fn a_rule_in_a_metadata_block_hides_with_the_rest_and_a_click_stays_out() {
    let s = "One.\n\n---\n\n:::section-metadata\n\na: b\n\n---\n\nc: d\n\n:::\n\nTwo.\n";
    let mut v = view_of(s, true);
    frame(&mut v);
    let inner = s.match_indices("---").nth(1).unwrap().0;
    let rule = v.model().item_at_offset(inner);
    assert_eq!(v.model().item_start(rule), inner);
    assert_eq!(v.height(rule), 0.0, "under the plaque");
    assert!(lay_out(&mut v, rule).prims.is_empty());
    let opener = item_of(&v, ":::section-metadata");
    let (open, close) = (s.find(":::section").unwrap(), s.rfind(":::").unwrap() + 3);
    let l = lay_out(&mut v, opener);
    let plaque = l.plaque.unwrap();
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut g = Geometry {
        view: &mut v,
        shaper: &mut f,
        images: &NoImages,
    };
    let (mut high, mut low) = (plaque.center_top(), plaque.center_bottom());
    high.y += 1.0;
    low.y -= 1.0;
    for pt in [high, low] {
        let at = g.hit(opener, pt).expect("a place");
        assert!(at.offset <= open || at.offset >= close, "{pt:?}: {at:?}");
    }
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
    assert_eq!(texts(&lay_out(&mut v, i)), ["2", "new"]);
}

const FRAMED: SectionStyleFor = SectionStyleFor {
    frame: true,
    numbered: true,
};
struct SectionStyleFor {
    frame: bool,
    numbered: bool,
}

fn styled(text: &str, how: &SectionStyleFor, paging: Paging) -> View {
    let d: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(text));
    let model = Arc::new(DocModel::build_with(&*d, None, paging));
    let mut v = View::new(d, model, Theme::light(), 700.0, 1.0);
    v.set_sections(true);
    v.set_section_style(markise_layout::SectionStyle {
        frame: how.frame,
        accent: None,
        numbered: how.numbered,
    });
    v
}

const THREE: &str =
    "---\nlang: en\n---\n\n# First\n\nOne.\n\n---\n\n## Second\n\nTwo.\n\n- a\n- b\n\n---\n\n## Third\n\nThree.\n";

#[test]
fn a_boundary_is_as_tall_as_it_is_estimated_however_it_looks() {
    for how in [
        SectionStyleFor {
            frame: false,
            numbered: true,
        },
        SectionStyleFor {
            frame: false,
            numbered: false,
        },
        FRAMED,
        SectionStyleFor {
            frame: true,
            numbered: false,
        },
    ] {
        let mut v = styled(THREE, &how, Paging::Full);
        for k in [1, 2] {
            let i = rule_item(&v, k);
            let estimate = v.estimate(i);
            assert_eq!(
                lay_out(&mut v, i).height,
                estimate,
                "frame {} numbered {}",
                how.frame,
                how.numbered
            );
        }
    }
}

#[test]
fn a_frame_is_one_shape_per_section_with_a_gap_between() {
    let mut v = styled(THREE, &FRAMED, Paging::Full);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let shown = v.frame(4000.0, 0.0, &mut f, &NoImages);
    let runs = merged_bands(
        shown
            .iter()
            .map(|s| (epaint::pos2(0.0, s.y), s.layout.bands.as_slice())),
    );
    assert_eq!(runs.len(), 3, "{runs:?}");
    for pair in runs.windows(2) {
        assert!(
            pair[1].rect.top() > pair[0].rect.bottom() + 8.0,
            "a clear gap: {runs:?}"
        );
        assert_eq!(pair[0].rect.width(), pair[1].rect.width());
    }
    assert!(runs.iter().all(|r| r.stroke.width > 0.0 && r.radius > 0.0));
    // it stands out over the inset on both sides, and front matter is outside it
    assert!(runs[0].rect.left() < 0.0 && runs[0].rect.right() > 700.0);
    let front = shown.first().unwrap();
    assert!(front.layout.bands.is_empty());
    assert!(runs[0].rect.top() >= front.y + front.layout.height);
    // the last item holds the end of the last frame
    let last = shown.last().unwrap();
    assert!(runs[2].rect.bottom() > last.y + last.layout.height);
}

#[test]
fn with_no_frame_there_are_no_bands_and_a_first_section_with_nothing_has_no_frame() {
    let mut v = styled(
        THREE,
        &SectionStyleFor {
            frame: false,
            numbered: true,
        },
        Paging::Full,
    );
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    assert!(
        v.frame(4000.0, 0.0, &mut f, &NoImages)
            .iter()
            .all(|s| s.layout.bands.is_empty())
    );
    let mut v = styled(
        "---\nlang: en\n---\n\n---\n\nOne.\n\n---\n\nTwo.\n",
        &FRAMED,
        Paging::Full,
    );
    let shown = v.frame(4000.0, 0.0, &mut f, &NoImages);
    let runs = merged_bands(
        shown
            .iter()
            .map(|s| (epaint::pos2(0.0, s.y), s.layout.bands.as_slice())),
    );
    assert_eq!(runs.len(), 2, "{runs:?}");
    // its numbers go from what shows
    let i = rule_item(&v, 1);
    assert_eq!(texts(&lay_out(&mut v, i)), ["1"]);
}

/// A placeholder's height is what its items come to, near enough for a scroll bar: every boundary counted.
#[test]
fn a_page_estimate_counts_what_boundaries_add() {
    let mut text = String::from("---\nlang: en\n---\n\n");
    for k in 0..40 {
        text.push_str(&format!("One line of text number {k}.\n\n---\n\n"));
    }
    text.push_str("End.\n");
    for how in [
        SectionStyleFor {
            frame: false,
            numbered: true,
        },
        FRAMED,
    ] {
        let paged = styled(&text, &how, Paging::Paged { tops: 8 });
        let mut full = styled(&text, &how, Paging::Full);
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        full.frame(100.0, 0.0, &mut f, &NoImages);
        let whole: f32 = (0..full.len()).map(|i| lay_out(&mut full, i).height).sum();
        let estimated: f32 = (0..paged.len()).map(|i| paged.estimate(i)).sum();
        assert!(
            (whole - estimated).abs() / whole < 0.03,
            "frame {}: {whole} {estimated}",
            how.frame
        );
    }
}

#[test]
fn how_a_section_looks_changes_no_byte() {
    for how in [
        SectionStyleFor {
            frame: false,
            numbered: true,
        },
        FRAMED,
    ] {
        let mut v = styled(THREE, &how, Paging::Full);
        frame(&mut v);
        assert_eq!(v.doc().text(), THREE);
    }
}

fn framed_band(top: f32, bottom: f32, opens: bool, closes: bool) -> markise_layout::block::Band {
    markise_layout::block::Band {
        rect: epaint::Rect::from_min_max(epaint::pos2(0.0, top), epaint::pos2(100.0, bottom)),
        fill: epaint::Color32::from_rgba_unmultiplied(0, 0, 200, 10),
        opens,
        closes,
        stroke: epaint::Stroke::new(1.5, epaint::Color32::BLUE),
        radius: 10.0,
    }
}

/// What is on screen of a section that goes on past it has no edge and no corner to show.
#[test]
fn a_frame_has_edges_and_corners_only_at_its_true_ends() {
    let at = |y: f32| epaint::pos2(0.0, y);
    let (middle, start, end) = (
        [framed_band(0.0, 50.0, false, false)],
        [framed_band(0.0, 50.0, true, false)],
        [framed_band(0.0, 50.0, false, true)],
    );
    // the middle of a section only: straight off both ways, by more than a corner and its edge
    let runs = merged_bands([(at(100.0), &middle[..]), (at(150.0), &middle[..])]);
    assert_eq!(runs.len(), 1);
    assert!(runs[0].rect.top() < 100.0 - 11.5, "{runs:?}");
    assert!(runs[0].rect.bottom() > 200.0 + 11.5, "{runs:?}");
    // its start is where it says, its bottom goes on
    let runs = merged_bands([(at(100.0), &start[..]), (at(150.0), &middle[..])]);
    assert_eq!(runs[0].rect.top(), 100.0);
    assert!(runs[0].rect.bottom() > 200.0 + 11.5, "{runs:?}");
    // its end is where it says, its top goes on
    let runs = merged_bands([(at(100.0), &middle[..]), (at(150.0), &end[..])]);
    assert!(runs[0].rect.top() < 100.0 - 11.5, "{runs:?}");
    assert_eq!(runs[0].rect.bottom(), 200.0);
    // a block's ground has neither, and stays what it was
    let plain = [markise_layout::block::Band {
        stroke: epaint::Stroke::NONE,
        radius: 0.0,
        ..middle[0]
    }];
    let runs = merged_bands([(at(100.0), &plain[..]), (at(150.0), &plain[..])]);
    assert_eq!((runs[0].rect.top(), runs[0].rect.bottom()), (100.0, 200.0));
}

/// Scrolled into a long section, on a paged view too, and across what a page cuts.
#[test]
fn scrolled_into_a_long_section_nothing_shows_of_its_ends() {
    let mut text = String::from("One.\n\n---\n\n## Long\n\n");
    for k in 0..200 {
        text.push_str(&format!("Paragraph number {k} of a long section.\n\n"));
    }
    text.push_str("---\n\nLast.\n");
    for paging in [Paging::Full, Paging::Paged { tops: 8 }] {
        let mut v = styled(&text, &FRAMED, paging);
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        v.frame(600.0, 0.0, &mut f, &NoImages);
        let middle = text.find("Paragraph number 100 ").unwrap();
        v.ensure_offset(middle);
        let i = v.model().item_at_offset(middle);
        v.scroll_to_item(i);
        let shown = v.frame(600.0, 0.0, &mut f, &NoImages);
        let runs = merged_bands(
            shown
                .iter()
                .map(|s| (epaint::pos2(0.0, s.y), s.layout.bands.as_slice())),
        );
        assert_eq!(runs.len(), 1, "{runs:?}");
        let (r, reach) = (runs[0].rect, runs[0].radius + runs[0].stroke.width);
        assert!(r.top() <= -reach && r.bottom() >= 600.0 + reach, "{r:?}");
    }
}

/// What stands on a boundary's row stands inside its item, however it looks.
#[test]
fn a_boundary_s_row_is_inside_its_item() {
    for frame in [false, true] {
        for numbered in [false, true] {
            let how = SectionStyleFor { frame, numbered };
            let mut v = styled("One.\n\n---\n\nTwo.\n", &how, Paging::Full);
            let i = rule_item(&v, 1);
            let l = lay_out(&mut v, i);
            let mid = l.divider_mid.unwrap();
            let line = Theme::light().line_px();
            assert!(
                mid - line * 0.5 >= 0.0 && mid + line * 0.5 <= l.height + 0.01,
                "frame {frame} numbered {numbered}: {mid} {}",
                l.height
            );
        }
    }
}

/// What a section's row says, and which heading rows stand under it.
#[test]
fn a_section_named_by_its_first_heading_has_no_row_for_it_and_the_title_keeps_its_own() {
    use OutlineRow::*;
    let rows_of = |s: &str, numbered: bool| {
        let d: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(s));
        let model = Arc::new(DocModel::build_with(&*d, None, Paging::Full));
        let mut v = View::new(d, model, Theme::light(), 700.0, 1.0);
        v.set_sections(true);
        v.set_section_style(markise_layout::SectionStyle {
            numbered,
            ..Default::default()
        });
        let rows: Vec<_> = (0..v.outline_row_count(None)).map(|r| v.outline_row(r, None)).collect();
        for (r, row) in rows.iter().enumerate() {
            // each row is where the way back says
            match row {
                Section(k) => assert_eq!(v.outline_row_of(*k, None, None), r, "{s:?}"),
                Heading(h) => {
                    let m = v.model().clone();
                    let k = v.doc().section_at(m.item_start(m.outline[*h].item)).index;
                    assert_eq!(v.outline_row_of(k, Some(*h), None), r, "{s:?}");
                }
            }
        }
        (rows, v)
    };
    // the title stays a heading under a section with no name; others go by their heading, and the next stays
    let (rows, v) = rows_of("# T\n\n## Under\n\n---\n\n## B\n\n### C\n\n## D\n", true);
    assert_eq!(
        rows,
        [Section(0), Heading(0), Heading(1), Section(1), Heading(3), Heading(4)]
    );
    assert_eq!(v.section_own_name(0), None);
    assert_eq!(v.section_own_name(1).as_deref(), Some("B"));
    assert_eq!(v.section_shown_number(1), Some(2));
    // no `#`: the first goes by its heading as any
    let (rows, _) = rows_of("## A\n\n---\n\nText.\n", true);
    assert_eq!(rows, [Section(0), Section(1)]);
    // an id is what it goes by, and its heading says something else
    let (rows, v) = rows_of("A\n\n---\n\n:::section-metadata\n\nid: x\n\n:::\n\n## B\n", false);
    assert_eq!(rows, [Section(0), Section(1), Heading(0)]);
    assert_eq!(v.section_own_name(1).as_deref(), Some("x"));
    assert_eq!(v.section_shown_number(1), None, "with no number");
    // a heading in a block before it is listed first, and the one that names it keeps its row
    let (rows, _) = rows_of("A\n\n---\n\n:::q\n\n## In\n\n:::\n\n## B\n", true);
    assert_eq!(rows, [Section(0), Section(1), Heading(0), Heading(1)]);
    // metadata before it is no block for this
    let (rows, _) = rows_of("A\n\n---\n\n:::section-metadata\n\nkind: x\n\n:::\n\n## B\n", true);
    assert_eq!(rows, [Section(0), Section(1)]);
}
