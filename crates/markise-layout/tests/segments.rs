//! Second review: splitter robustness (items 5, 7, 10).

use markise_layout::block::{NoImages, Prim};
use markise_layout::model::{MAX_CELL_BYTES, Segment};
use markise_layout::{ItemKind, testing};
use markise_theme::Theme;
use proptest::prelude::*;
use std::sync::Arc;

fn model(src: &str) -> testing::Shown {
    testing::Shown::new(Arc::new(markise_md::parse(src)))
}

/// Concatenated segment spans must reproduce every content line exactly.
fn check_cover(src: &str) {
    let m = model(src);
    let text = m.doc.text();
    let mut i = 0;
    while i < m.items.len() {
        let it = &m.items[i];
        if let Some(Segment::Lines { .. }) = &it.segment {
            let start = m.item_start(i);
            let block = m.doc.leaf(start).unwrap();
            let mut pieces = String::new();
            let mut j = i;
            while j < m.items.len() && m.item_span(j) == m.item_span(i) {
                // segment spans are item-relative (C-18)
                for s in m.item_lines(&*m.doc, j) {
                    assert!(text.is_char_boundary(s.start) && text.is_char_boundary(s.end));
                    pieces.push_str(s.slice(text));
                }
                j += 1;
            }
            let lines: String = block.lines.iter().map(|l| l.slice(text)).collect();
            assert_eq!(pieces, lines);
            i = j;
        } else {
            i += 1;
        }
    }
}

#[test]
fn long_multibyte_lines_do_not_panic_item5() {
    let line = "界".repeat(2000);
    check_cover(&format!("{line}\n"));
    check_cover(&format!("```\n{line}\n```\n"));
    check_cover(&format!("{}\n", "é界 ".repeat(5000)));
    check_cover(&format!("| a | b |\n|---|---|\n| {line} | x |\n"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn splitters_accept_any_multibyte_text(
        parts in proptest::collection::vec(prop_oneof![
            Just("界"), Just("é"), Just("😀"), Just(" "), Just("a"), Just("\n"), Just("\u{200B}")
        ], 1..400),
        rep in 1usize..60,
        kind in 0u8..3,
    ) {
        let unit: String = parts.concat();
        let body = unit.repeat(rep);
        let src = match kind {
            0 => body,
            1 => format!("```\n{body}\n```\n"),
            _ => format!("| h | i |\n|---|--:|\n| {} | z |\n", body.replace('\n', " ")),
        };
        check_cover(&src);
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut v = model(&src).view(Theme::light(), 500.0, 1.0);
        v.frame(600.0, 300.0, &mut f, &NoImages);
    }
}

#[test]
fn huge_table_cell_is_capped_item7() {
    let cell = "word ".repeat(400_000); // 2 MB in one cell
    let src = format!("| a | b |\n|---|---|\n| {cell} | x |\n| small | y |\n");
    let m = model(&src);
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let mut v = m.view(Theme::light(), 600.0, 1.0);
    for i in 0..m.items.len() {
        let l = v.layout(i, &mut f, &NoImages);
        let shaped: usize = l
            .prims
            .iter()
            .map(|p| match p {
                Prim::Text { text, .. } => text.text().len(),
                _ => 0,
            })
            .sum();
        assert!(shaped < 4 * MAX_CELL_BYTES, "item {i} shaped {shaped} bytes");
        let has_marker = l
            .prims
            .iter()
            .any(|p| matches!(p, Prim::Text { text, .. } if text.text().ends_with('…')));
        if i == 0 {
            assert!(has_marker, "truncation indicator");
        }
    }
}

#[test]
fn segmented_table_alignment_is_part_of_the_key_item10() {
    let mut rows = String::new();
    for i in 0..400 {
        rows.push_str(&format!("| r{i} | {i} |\n"));
    }
    let a = model(&format!("| x | y |\n|:--|---|\n{rows}"));
    let b = model(&format!("| x | y |\n|--:|---|\n{rows}"));
    assert!(a.items.len() > 2 && a.items.iter().all(|i| i.kind == ItemKind::Table));
    assert_eq!(a.items.len(), b.items.len());
    for (x, y) in a.items.iter().zip(b.items.iter()) {
        assert_ne!(x.hash, y.hash, "every segment's key changes with the alignment");
    }
}
