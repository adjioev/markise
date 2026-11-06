use std::collections::BTreeMap;

use markise_package::{FieldKind, Localized, PackageError, PackageIndex, ViewKind, limits};

use crate::common::files;

fn with(path: &str, content: &str) -> BTreeMap<String, Vec<u8>> {
    let mut f = files();
    f.insert(path.to_string(), content.as_bytes().to_vec());
    f
}

/// The refusal of a package that must be invalid, as one text.
fn refusal(files: &BTreeMap<String, Vec<u8>>) -> String {
    match PackageIndex::from_files(files) {
        Err(PackageError::Invalid(issues)) => issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
        other => panic!("expected an invalid package, got {other:?}"),
    }
}

#[test]
fn indexes_blocks_with_schema_view_and_palette() {
    let index = PackageIndex::from_files(&files()).unwrap();
    assert_eq!(index.id().library, "sample");
    assert_eq!(index.id().version, "1.2.0");
    assert_eq!(index.block_names().collect::<Vec<_>>(), ["gallery", "poll", "tip"]);
    assert_eq!(index.block_count(), 3);

    let tip = index.block("tip").unwrap();
    assert_eq!(tip.schema.label.as_ref().unwrap().get("fr"), "Astuce");
    assert_eq!(tip.schema.fields[0].kind, FieldKind::Heading);
    assert_eq!(tip.schema.fields[0].raw["level"], 3);
    assert_eq!(tip.view.unwrap().kind, ViewKind::Callout);
    assert_eq!(tip.view.unwrap().icon.as_deref(), Some("bulb"));
    assert_eq!(tip.palette.unwrap().icon.as_deref(), Some("bulb"));

    let gallery = index.block("gallery").unwrap();
    assert!(gallery.view.is_none(), "views are optional per block");
    assert!(gallery.palette.is_none());
    assert_eq!(gallery.schema.raw["argument"]["name"], "layout");

    assert!(index.block("nothing").is_none());
    assert_eq!(index.groups().len(), 2);
    assert_eq!(index.presets().len(), 2);
    let poll: Vec<_> = index.presets_of("poll").collect();
    assert_eq!(poll.len(), 1);
    assert_eq!(poll[0].extra["site_hint"]["any"][0], "thing", "unknown keys are kept");
    assert_eq!(
        index.block("poll").unwrap().view.unwrap().extra["columns"],
        2,
        "unknown view keys are kept"
    );
}

#[test]
fn views_file_is_optional() {
    let mut f = files();
    f.remove("editor/views.json");
    let index = PackageIndex::from_files(&f).unwrap();
    assert!(index.block("tip").unwrap().view.is_none());
}

#[test]
fn localized_falls_back_to_english_then_first() {
    let text = |pairs: &[(&str, &str)]| {
        Localized::ByLanguage(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    };
    assert_eq!(text(&[("en", "a"), ("fr", "b")]).get("fr"), "b");
    assert_eq!(text(&[("en", "a"), ("fr", "b")]).get("de"), "a");
    assert_eq!(text(&[("fr", "b")]).get("de"), "b");
    assert_eq!(Localized::Plain("x".into()).get("de"), "x");
}

#[test]
fn other_format_versions_refuse_cleanly() {
    for found in ["markise-block-package/1", "markise-block-package/3"] {
        let f = with(
            "manifest.json",
            &format!(r#"{{"format":"{found}","anything":{{"may":"change"}}}}"#),
        );
        assert_eq!(
            PackageIndex::from_files(&f),
            Err(PackageError::UnsupportedFormat {
                found: found.to_string()
            })
        );
    }
    let text = PackageIndex::from_files(&with("manifest.json", r#"{"format":"markise-block-package/3"}"#))
        .unwrap_err()
        .to_string();
    assert!(text.contains("not supported by this version"), "{text}");
}

#[test]
fn manifest_problems_are_named() {
    let mut no_manifest = files();
    no_manifest.remove("manifest.json");
    assert!(refusal(&no_manifest).contains("no manifest"));

    for (manifest, expected) in [
        ("not json", "not valid JSON"),
        ("[1]", "expected a JSON object"),
        (r#"{"library":"x"}"#, "does not name a format"),
        (r#"{"format":"something-else/1"}"#, "not a package format"),
        (
            r#"{"format":"markise-block-package/2","version":"1","blocks":["tip"]}"#,
            "no library",
        ),
        (
            r#"{"format":"markise-block-package/2","library":"../up","version":"1","blocks":["tip"]}"#,
            "library \"../up\" is not valid",
        ),
        (
            r#"{"format":"markise-block-package/2","library":"x","version":"1","blocks":[]}"#,
            "defines no blocks",
        ),
        (
            r#"{"format":"markise-block-package/2","library":"x","version":"1","blocks":["tip","tip"]}"#,
            "appears twice",
        ),
        (
            r#"{"format":"markise-block-package/2","library":"x","version":"1","blocks":["a/b"]}"#,
            "not a valid block name",
        ),
        (
            r#"{"format":"markise-block-package/2","library":"x","version":"1","blocks":["missing"]}"#,
            "the file is missing",
        ),
    ] {
        let text = refusal(&with("manifest.json", manifest));
        assert!(text.contains(expected), "{manifest}: {text}");
    }
}

#[test]
fn a_version_may_carry_a_build_part() {
    let manifest = r#"{"format":"markise-block-package/2","library":"sample","version":"1.0.0-rc.1+abc","blocks":["tip","poll","gallery"]}"#;
    let index = PackageIndex::from_files(&with("manifest.json", manifest)).unwrap();
    assert_eq!(index.id().version, "1.0.0-rc.1+abc");
}

#[test]
fn a_schema_outside_the_manifest_is_refused() {
    let f = with("components/extra/extra.json", r#"{"fields":[]}"#);
    assert!(refusal(&f).contains("block \"extra\" is not in the manifest"));
    // other files in a components folder are opaque
    assert!(PackageIndex::from_files(&with("components/shared/base.json", "not json")).is_ok());
}

#[test]
fn schema_problems_are_named() {
    for (schema, expected) in [
        (r#"{"label":"x"}"#, "fields must be an array"),
        (r#"{"fields":[1]}"#, "field 0 must be an object"),
        (r#"{"fields":[{"type":"text"}]}"#, "field 0 has no name"),
        (r#"{"fields":[{"name":"a b","type":"text"}]}"#, "not a valid name"),
        (r#"{"fields":[{"name":"a"}]}"#, "\"a\" has no type"),
        (
            r#"{"fields":[{"name":"a","type":"picker"}]}"#,
            "unknown type \"picker\"",
        ),
        (
            r#"{"fields":[{"name":"a","type":"text"},{"name":"a","type":"list"}]}"#,
            "\"a\" appears twice",
        ),
        (r#"{"label":1,"fields":[]}"#, "label must be a string"),
        (r#"{"variants":[],"fields":[]}"#, "variants must be an object"),
        (r#"{"argument":"x","fields":[]}"#, "argument must be an object"),
    ] {
        let text = refusal(&with("components/tip/tip.json", schema));
        assert!(text.contains(expected), "{schema}: {text}");
        assert!(text.contains("components/tip/tip.json"), "names the file: {text}");
    }
}

#[test]
fn palette_problems_are_named() {
    let mut no_palette = files();
    no_palette.remove("editor/palette.json");
    assert!(refusal(&no_palette).contains("no palette"));

    for (palette, expected) in [
        (r#"{"format":2}"#, "unsupported palette format"),
        (r#"{"groups":{}}"#, "groups must be an array"),
        (r#"{"groups":[{"blocks":[]}]}"#, "group 0 has no id"),
        (r#"{"groups":[{"id":"a","blocks":["nope"]}]}"#, "unknown block \"nope\""),
        (r#"{"blocks":{"nope":{}}}"#, "unknown block \"nope\""),
        (r#"{"blocks":{"tip":{"icon":"../x"}}}"#, "icon \"../x\""),
        (r#"{"presets":[{"block":"tip"}]}"#, "preset 0 has no id"),
        (r#"{"presets":[{"id":"a","markdown":":::tip\n"}]}"#, "has no block"),
        (r#"{"presets":[{"id":"a","block":"tip"}]}"#, "has no markdown"),
        (
            r#"{"presets":[{"id":"a","block":"tip","markdown":":::tipper\n"}]}"#,
            "must start with :::tip",
        ),
        (
            r#"{"presets":[{"id":"a","block":"tip","markdown":":::tip"},{"id":"a","block":"tip","markdown":":::tip"}]}"#,
            "appears twice",
        ),
    ] {
        let text = refusal(&with("editor/palette.json", palette));
        assert!(text.contains(expected), "{palette}: {text}");
    }
}

#[test]
fn view_problems_are_named() {
    for (views, expected) in [
        (r#"{"nope":{"kind":"card"}}"#, "unknown block \"nope\""),
        (r#"{"tip":1}"#, "must be an object"),
        (r#"{"tip":{}}"#, "has no kind"),
        (r#"{"tip":{"kind":"carousel"}}"#, "unknown kind \"carousel\""),
        (r#"{"tip":{"kind":"card","icon":"a/b"}}"#, "icon \"a/b\""),
    ] {
        let text = refusal(&with("editor/views.json", views));
        assert!(text.contains(expected), "{views}: {text}");
    }
}

#[test]
fn every_generic_view_kind_is_accepted() {
    for (name, kind) in [
        ("callout", ViewKind::Callout),
        ("card", ViewKind::Card),
        ("choices", ViewKind::Choices),
        ("media", ViewKind::Media),
    ] {
        let f = with("editor/views.json", &format!(r#"{{"tip":{{"kind":"{name}"}}}}"#));
        let index = PackageIndex::from_files(&f).unwrap();
        assert_eq!(index.block("tip").unwrap().view.unwrap().kind, kind);
    }
}

#[test]
fn size_limits_refuse() {
    let huge = format!("{{\"pad\":\"{}\"}}", "x".repeat(limits::MAX_JSON_BYTES));
    assert!(refusal(&with("editor/views.json", &huge)).contains("the limit is"));

    let fields: Vec<String> = (0..=limits::MAX_FIELDS)
        .map(|i| format!(r#"{{"name":"f{i}","type":"text"}}"#))
        .collect();
    let schema = format!(r#"{{"fields":[{}]}}"#, fields.join(","));
    assert!(refusal(&with("components/tip/tip.json", &schema)).contains("the limit is"));

    let names: Vec<String> = (0..=limits::MAX_BLOCKS).map(|i| format!("\"b{i}\"")).collect();
    let manifest = format!(
        r#"{{"format":"markise-block-package/2","library":"x","version":"1","blocks":[{}]}}"#,
        names.join(",")
    );
    assert!(refusal(&with("manifest.json", &manifest)).contains("the limit is"));

    let markdown = format!(":::tip\\n{}", "x".repeat(limits::MAX_PRESET_MARKDOWN_BYTES));
    let palette = format!(r#"{{"presets":[{{"id":"a","block":"tip","markdown":"{markdown}"}}]}}"#);
    assert!(refusal(&with("editor/palette.json", &palette)).contains("longer than"));
}

#[test]
fn issues_are_collected_and_capped() {
    let bad: Vec<String> = (0..100)
        .map(|i| format!(r#"{{"id":"p{i}","block":"nope","markdown":":::nope"}}"#))
        .collect();
    let palette = format!(r#"{{"presets":[{}]}}"#, bad.join(","));
    match PackageIndex::from_files(&with("editor/palette.json", &palette)) {
        Err(PackageError::Invalid(issues)) => assert_eq!(issues.len(), limits::MAX_ISSUES),
        other => panic!("{other:?}"),
    }
}

#[test]
fn deeply_nested_json_is_an_error_not_a_crash() {
    let deep = "[".repeat(100_000);
    assert!(refusal(&with("editor/views.json", &deep)).contains("not valid JSON"));
}

#[test]
fn a_long_variant_name_is_refused_before_its_texts_are_read() {
    // 500 KB of name and 30,000 language keys: the shape that copied the name per key
    let long = "v".repeat(500_000);
    let languages: Vec<String> = (0..30_000).map(|i| format!(r#""l{i}":"x""#)).collect();
    let schema = format!(r#"{{"variants":{{"{long}":{{{}}}}},"fields":[]}}"#, languages.join(","));
    assert!(schema.len() < limits::MAX_JSON_BYTES);
    let text = refusal(&with("components/tip/tip.json", &schema));
    assert!(
        text.contains("variant name is longer than"),
        "{}",
        &text[..text.len().min(200)]
    );
    assert!(text.len() < 1000, "the name is not echoed: {} bytes", text.len());
}

#[test]
fn a_long_palette_block_name_does_not_multiply_its_description() {
    let long = "b".repeat(500_000);
    let languages: Vec<String> = (0..30_000).map(|i| format!(r#""l{i}":"x""#)).collect();
    let palette = format!(
        r#"{{"blocks":{{"{long}":{{"description":{{{}}}}}}}}}"#,
        languages.join(",")
    );
    assert!(palette.len() < limits::MAX_JSON_BYTES);
    let text = refusal(&with("editor/palette.json", &palette));
    assert!(text.contains("unknown block"), "{}", &text[..text.len().min(200)]);
}

#[test]
fn palette_block_definitions_must_be_objects() {
    for palette in [
        r#"{"blocks":{"tip":42}}"#,
        r#"{"blocks":{"tip":[]}}"#,
        r#"{"blocks":{"tip":"broken"}}"#,
        r#"{"blocks":{"tip":null}}"#,
    ] {
        let text = refusal(&with("editor/palette.json", palette));
        assert!(text.contains("must be an object"), "{palette}: {text}");
    }
    assert!(PackageIndex::from_files(&with("editor/palette.json", r#"{"blocks":{"tip":{}}}"#)).is_ok());
}

#[test]
fn a_version_is_limited_as_a_whole() {
    let manifest = |version: &str| {
        format!(
            r#"{{"format":"markise-block-package/2","library":"sample","version":"{version}","blocks":["tip","poll","gallery"]}}"#
        )
    };
    let split = format!("{}+{}", "a".repeat(128), "b".repeat(128));
    assert!(refusal(&with("manifest.json", &manifest(&split))).contains("is not valid"));
    let fits = format!("{}+{}", "a".repeat(64), "b".repeat(63));
    assert!(PackageIndex::from_files(&with("manifest.json", &manifest(&fits))).is_ok());
}

#[test]
fn an_argument_s_pattern_that_cannot_be_run_refuses_the_package_at_once() {
    let started = std::time::Instant::now();
    let long = "a".repeat(schema_form_core::MAX_PATTERN_BYTES + 1);
    for (pattern, word) in [
        (serde_json::json!("(unclosed"), "cannot be used"),
        (serde_json::json!("(?=x)y"), "look-around"),
        (serde_json::json!(r"(a)\1"), "backreference"),
        (serde_json::json!("(a{1000}){1000}"), "too large"),
        (serde_json::json!(long), "limit"),
        (serde_json::json!(3), "must be a string"),
    ] {
        let schema = serde_json::json!({"label": "G", "argument": {"pattern": pattern}, "fields": []});
        let why = refusal(&with("components/gallery/gallery.json", &schema.to_string()));
        assert!(why.contains("components/gallery/gallery.json"), "{why}");
        assert!(why.contains("the argument's pattern"), "{why}");
        assert!(why.contains(word), "{pattern}: {why}");
    }
    assert!(started.elapsed().as_secs() < 30);
    // one that a backtracking engine would hang on is harmless here, so it stays
    let schema = serde_json::json!({"label": "G", "argument": {"pattern": "^(a+)+$"}, "fields": []});
    assert!(PackageIndex::from_files(&with("components/gallery/gallery.json", &schema.to_string())).is_ok());
}
