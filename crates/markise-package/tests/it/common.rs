//! A small neutral package, built in code so that a test can change one file of it.

use std::collections::BTreeMap;

pub const MANIFEST: &str = r#"{
  "format": "markise-block-package/2",
  "library": "sample",
  "version": "1.2.0",
  "blocks": ["tip", "poll", "gallery"],
  "created": "2026-01-01T00:00:00Z"
}"#;

pub const TIP: &str = r#"{
  "label": { "en": "Tip", "fr": "Astuce" },
  "fields": [
    { "name": "title", "type": "heading", "level": 3, "min": 0 },
    { "name": "body", "type": "text", "min": 1, "max": null }
  ]
}"#;

pub const POLL: &str = r#"{
  "label": "Poll",
  "variants": { "": { "en": "one answer" }, "multi": { "en": "several answers" } },
  "fields": [
    { "name": "question", "type": "heading", "level": 2 },
    { "name": "answers", "type": "choices", "ids": "required", "minItems": 2 },
    { "name": "why", "type": "quote" }
  ]
}"#;

pub const GALLERY: &str = r#"{
  "label": "Gallery",
  "argument": { "name": "layout", "pattern": "^[a-z]+$" },
  "fields": [
    { "name": "items", "type": "list", "minItems": 1 },
    { "name": "pictures", "type": "image", "min": 0, "max": null }
  ]
}"#;

pub const PALETTE: &str = r#"{
  "format": 1,
  "groups": [
    { "id": "text", "label": { "en": "Text" }, "blocks": ["tip"] },
    { "id": "other", "label": "Other", "blocks": ["poll", "gallery"] }
  ],
  "blocks": {
    "tip": { "description": { "en": "A short hint" }, "icon": "bulb" },
    "poll": { "description": "Answers to choose from", "icon": "list-check" }
  },
  "presets": [
    { "id": "tip-plain", "block": "tip", "label": { "en": "Plain tip" }, "keywords": "hint note",
      "markdown": ":::tip\n\nText.\n\n:::\n" },
    { "id": "poll-two", "block": "poll", "label": "Two answers", "markdown": ":::poll multi\n\n## Q\n\n:::\n",
      "site_hint": { "any": ["thing"] } }
  ]
}"#;

pub const VIEWS: &str = r#"{
  "tip": { "kind": "callout", "accent": "amber", "icon": "bulb" },
  "poll": { "kind": "choices", "accent": "purple", "columns": 2 }
}"#;

/// A complete valid package, opaque files included.
pub fn files() -> BTreeMap<String, Vec<u8>> {
    [
        ("manifest.json", MANIFEST),
        ("components/tip/tip.json", TIP),
        ("components/tip/tip.css", ".tip { color: red }"),
        ("components/poll/poll.json", POLL),
        ("components/poll/poll.js", "this is not parsed here ("),
        ("components/gallery/gallery.json", GALLERY),
        ("editor/palette.json", PALETTE),
        ("editor/views.json", VIEWS),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
    .collect()
}
