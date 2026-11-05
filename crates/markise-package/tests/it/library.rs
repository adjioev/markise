//! The block library's data (ADR 0022): order, text and what a query matches.

use std::collections::BTreeMap;

use markise_package::PackageIndex;
use serde_json::json;

use crate::common;

fn sample() -> PackageIndex {
    PackageIndex::from_files(&common::files()).unwrap()
}

fn package(palette: serde_json::Value) -> PackageIndex {
    let mut files: BTreeMap<String, Vec<u8>> = common::files();
    files.insert("editor/palette.json".into(), palette.to_string().into_bytes());
    PackageIndex::from_files(&files).unwrap()
}

fn titles(index: &PackageIndex, query: &str) -> Vec<String> {
    let library = index.library("en");
    (library.filter(query).into_iter())
        .map(|i| library.entries()[i].title.clone())
        .collect()
}

#[test]
fn rows_follow_the_packages_groups_and_order() {
    let library = sample().library("en");
    assert_eq!(library.groups(), ["Text", "Other"]);
    let rows: Vec<_> = (library.entries().iter())
        .map(|e| (e.title.as_str(), e.block.as_str(), e.group))
        .collect();
    assert_eq!(
        rows,
        [("Plain tip", "tip", Some(0)), ("Two answers", "poll", Some(1))],
        "a block with no preset has nothing to insert, so no row"
    );
    assert_eq!(library.entries()[0].icon.as_deref(), Some("bulb"));
    assert_eq!(library.entries()[0].detail.as_deref(), Some("A short hint"));
}

#[test]
fn a_preset_of_a_block_in_no_group_comes_last_and_rows_of_one_block_say_whose_they_are() {
    let index = package(json!({
        "groups": [{"id": "later", "label": "Later", "blocks": ["poll"]}],
        "blocks": {"tip": {"description": "A hint"}},
        "presets": [
            {"id": "a", "block": "tip", "label": "One", "markdown": ":::tip\n\nx\n\n:::\n"},
            {"id": "b", "block": "tip", "label": "Two", "markdown": ":::tip\n\ny\n\n:::\n"},
            {"id": "c", "block": "poll", "label": "Three", "markdown": ":::poll\n\n:::\n"},
        ]
    }));
    let library = index.library("en");
    let rows: Vec<_> = (library.entries().iter())
        .map(|e| (e.title.as_str(), e.group, e.preset))
        .collect();
    assert_eq!(rows, [("Three", Some(0), 2), ("One", None, 0), ("Two", None, 1)]);
    assert!(library.entries()[1].detail.as_deref().unwrap().ends_with(" · A hint"));
}

#[test]
fn a_query_matches_every_language_and_each_of_its_words() {
    let index = package(json!({
        "groups": [{"id": "g", "label": {"en": "Group", "ru": "Группа"}, "blocks": ["tip", "poll"]}],
        "blocks": {"tip": {"description": {"en": "A short hint", "ru": "Короткая подсказка"}}},
        "presets": [
            {"id": "a", "block": "tip", "label": {"en": "Plain", "ru": "Простая Ёлка"},
             "keywords": "совет note", "markdown": ":::tip\n\nx\n\n:::\n"},
            {"id": "b", "block": "poll", "label": {"en": "Answers", "ru": "Ответы"},
             "markdown": ":::poll\n\n:::\n"},
        ]
    }));
    assert_eq!(titles(&index, ""), ["Plain", "Answers"]);
    assert_eq!(titles(&index, "  "), ["Plain", "Answers"]);
    assert_eq!(titles(&index, "ОТВЕТ"), ["Answers"], "Russian label, any case");
    assert_eq!(titles(&index, "answ"), ["Answers"], "English label, a prefix");
    assert_eq!(titles(&index, "совет"), ["Plain"], "keywords");
    assert_eq!(
        titles(&index, "подсказка"),
        ["Plain"],
        "a description in another language"
    );
    assert_eq!(titles(&index, "елка"), ["Plain"], "ё and е are one letter");
    assert_eq!(titles(&index, "note plain"), ["Plain"], "every word, in any order");
    assert_eq!(titles(&index, "note answers"), Vec::<String>::new());
    assert_eq!(titles(&index, "poll"), ["Answers"], "the block's name");
    assert_eq!(index.library("ru").entries()[0].title, "Простая Ёлка");
}
