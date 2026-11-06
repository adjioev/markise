//! Sections (ADR 0026) do not depend on what a view model holds: a paged and a full model of the
//! same document, through random edits, agree on every boundary, and a boundary is a rule item
//! wherever items are real.

use markise_doc::{FormatDoc, PatchSet};
use markise_layout::{DocModel, ItemKind, Paging};
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
