//! Fuzzes the public editing/generation API with arbitrary Unicode (review P1 #1 audit:
//! no byte-index slicing may split a character).

use markise_md::*;
use proptest::prelude::*;

fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const DOCS: &[&str] = &[
    "| a | b |\n|---|---|\n| é | ü |\n",
    "|x|\n|-|\n|$é|\n",
    "- [ ] é\n- [x] ü\n",
    "1. é\n2. ü\n",
    "```\nü\n```\n",
    "---\nk: é\n---\n日本\n",
    "> é\n> ü\n",
    "\u{FEFF}é\n",
    "",
];

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        "\\PC{0,20}",
        prop::collection::vec(
            prop::sample::select(vec![
                "$", "`", "|", "\\", "é", "日", "\n", " ", "---", "    ", "\t", "``", "$é"
            ]),
            0..12
        )
        .prop_map(|v| v.concat()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(512), .. ProptestConfig::default() })]

    #[test]
    fn prop_api_never_panics(doc_i in 0..DOCS.len(), value in text(), pos in 0usize..64, level in 1u8..7) {
        let w = WriteOptions::default();
        // generators
        let _ = table::escape_cell(&value);
        let _ = write::heading(&value, level, &w, false);
        let _ = write::paragraph(&value);
        let _ = write::fenced_code(&value, &value);
        let _ = write::front_matter(&value, "---");
        let _ = write::list(false, &[(Some(true), value.as_str())], &w);
        let _ = write::new_table(&[Align::Center, Align::Right], &[vec![value.clone(), "x".into()], vec!["é".into(), value.clone()]]);
        let _ = slug::slug(&value);
        let _ = frontmatter::parse_shallow_yaml(&value);
        // ops on documents
        let base = DOCS[doc_i];
        let mut d = parse(base);
        let p = pos.min(d.text().len());
        let p = (0..=p).rev().find(|&i| d.text().is_char_boundary(i)).unwrap_or(0);
        let _ = ops::toggle_task(&mut d, p);
        let _ = d.block_path_at(p);
        if !d.blocks().is_empty() {
            let _ = ops::set_table_cell(&mut d, &[0], 1, 0, &value);
            let _ = ops::insert_list_item(&mut d, &[0], 0, &value, Some(false));
            let _ = ops::set_code_content(&mut d, &[0], &value);
            let _ = ops::replace_block(&mut d, &[0], &value);
        }
        let _ = ops::set_front_matter(&mut d, &value);
        if !value.trim().is_empty() {
            let _ = ops::insert_block_after(&mut d, Some(0), &value, &w);
            let _ = ops::insert_block_after(&mut d, None, &value, &w);
        }
        let full = parse(d.text());
        prop_assert_eq!(d.blocks(), full.blocks());
        prop_assert_eq!(d.serialize(), d.text().to_string());
    }
}
