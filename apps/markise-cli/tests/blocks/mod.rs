//! `markise-cli blocks`: exit codes, reasons and lines, the JSON's shape and the reference, through
//! the binary, on the example package.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use super::bin;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A folder with the example package packed and bound, as a site's content folder would have it.
/// `home` is where the store would be, were anything to write to it.
struct Site {
    root: tempfile::TempDir,
    sha256: String,
}

impl Site {
    fn new() -> Site {
        let root = tempfile::tempdir().unwrap();
        let packed = markise_package::pack_folder(&repo().join("examples/block-package/package")).unwrap();
        let site = root.path().join("home/site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("recipe.tar.gz"), &packed.bytes).unwrap();
        std::fs::write(
            site.join("blocks.config.json"),
            format!(
                r#"{{"library":"recipe-site","version":"1.0.0","source":"recipe.tar.gz","sha256":"{}"}}"#,
                packed.sha256
            ),
        )
        .unwrap();
        Site {
            sha256: packed.sha256.to_string(),
            root,
        }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn folder(&self) -> PathBuf {
        self.home().join("site")
    }

    fn doc(&self, name: &str, text: &str) -> String {
        let path = self.folder().join(name);
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = bin();
        c.env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join("config"))
            .env("USERPROFILE", self.home())
            .env("APPDATA", self.home().join("config"))
            .current_dir(self.folder())
            .arg("blocks")
            .args(args);
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

const MIXED: &str = "\
---
lang: en
---

# Bread

:::tip

Tap the bottom of the loaf.

:::

:::tip

:::

:::question

Which rises?

- [ ] The salt {#a1}
- [x] The yeast {#a2}

:::

:::question

Which rises?

- [ ] The salt {#a1}
- [ ] The yeast {#a2}

:::

:::question

Which rises?

- [x] The salt
- [x] The yeast {#a2}

:::

:::tip caution

Body.

:::

:::ingredients

### For one

- Flour

:::

:::question

Which rises?

- [ ] The salt {#a1}
- [x] The yeast {#a2}

```
code
```

:::

:::sidebar

Body.

:::

::: tip

Not a block.

:::
";

#[test]
fn check_reports_reasons_and_lines_of_a_mix_and_exits_1() {
    let site = Site::new();
    site.doc("mixed.md", MIXED);
    let out = site.run(&["check", "mixed.md"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    let expected = "\
mixed.md:13: tip: Not filled in yet: Text: Required
mixed.md:26: question: Not filled in yet: Answers: Mark exactly one
mixed.md:35: question: Outside its schema: Answers: Mark exactly one
mixed.md:35: question: Outside its schema: Answers 1: Has no id
mixed.md:44: tip: Outside its schema: The block has no variant `caution`.
mixed.md:50: ingredients: Not filled in yet: Serves: Required
mixed.md:65: question: Outside its schema: In no field: ```…
mixed.md:71: sidebar: Unknown block: The package has no block `sidebar`.
mixed.md:77: Marker: `::: tip` is not a marker, so it stays text. A marker is three or more colons, followed directly by a name of letters, digits, `-` and `_`, then any arguments; or by nothing.
mixed.md:81: Marker: `:::` closes nothing, so it stays text.
files: 1, blocks: 9, problems: 10
";
    assert_eq!(text, expected);
}

#[test]
fn check_exits_0_on_the_example_document() {
    let site = Site::new();
    let source = repo().join("examples/block-package/document/weeknight-loaf.md");
    let doc = site.doc("loaf.md", &std::fs::read_to_string(source).unwrap());
    let out = site.run(&["check", &doc]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(stdout(&out).contains(": ok, 5 blocks\n"), "{}", stdout(&out));
}

#[test]
fn check_writes_nothing_and_asks_nothing() {
    let site = Site::new();
    let doc = site.doc("mixed.md", MIXED);
    let before = listing(site.root.path());
    let out = site.run(&["check", "--json", &doc]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        listing(site.root.path()),
        before,
        "the store or anything else was written"
    );
}

fn listing(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let meta = entry.metadata().unwrap();
        if meta.is_dir() {
            out.extend(listing(&entry.path()));
        }
        out.push((entry.path(), meta.len()));
    }
    out.sort();
    out
}

#[test]
fn check_json_has_the_documented_shape() {
    let site = Site::new();
    site.doc("mixed.md", ":::tip\n\n:::\n\n::: x\n");
    site.doc("clean.md", ":::tip\n\nBody.\n\n:::\n");
    let outside = tempfile::tempdir().unwrap();
    let unbound = outside.path().join("unbound.md");
    std::fs::write(&unbound, ":::tip\n\n:::\n").unwrap();
    let unbound = unbound.display().to_string();
    let out = site.run(&["check", "--json", "mixed.md", "clean.md", &unbound]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["format"], "markise-blocks-check/1");
    assert_eq!(
        report["summary"],
        serde_json::json!({"files": 3, "blocks": 2, "problems": 2, "errors": 0, "unbound": 1})
    );
    let mixed = &report["files"][0];
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys(mixed),
        [
            "binding", "blocks", "error", "file", "language", "note", "problems", "status"
        ]
    );
    assert_eq!(mixed["file"], "mixed.md");
    assert_eq!(mixed["status"], "problems");
    assert_eq!(mixed["language"], "en");
    assert_eq!(mixed["binding"]["library"], "recipe-site");
    assert_eq!(mixed["binding"]["version"], "1.0.0");
    assert_eq!(mixed["binding"]["sha256"], site.sha256.as_str());
    assert!(
        mixed["binding"]["file"]
            .as_str()
            .unwrap()
            .ends_with("blocks.config.json")
    );
    assert_eq!(
        mixed["problems"][0],
        serde_json::json!({"line": 1, "block_line": 1, "block": "tip", "kind": "not-filled",
                           "headline": "Not filled in yet", "reason": "Text: Required"})
    );
    assert_eq!(mixed["problems"][1]["kind"], "marker");
    assert_eq!(mixed["problems"][1]["block"], Value::Null);
    assert_eq!(report["files"][1]["status"], "clean");
    assert_eq!(report["files"][1]["problems"], serde_json::json!([]));
    let none = &report["files"][2];
    assert_eq!(keys(none), keys(mixed));
    assert_eq!(none["status"], "unbound");
    assert_eq!(none["binding"], Value::Null);
    assert!(none["note"].as_str().unwrap().contains("blocks.config.json"));
}

#[test]
fn a_file_with_no_binding_is_a_clear_note_and_exit_0() {
    let site = Site::new();
    let outside = tempfile::tempdir().unwrap();
    let doc = outside.path().join("plain.md");
    std::fs::write(&doc, ":::anything\n").unwrap();
    let out = site.run(&["check", doc.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).contains("no blocks.config.json applies to this file"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn package_and_usage_errors_exit_2() {
    let site = Site::new();
    let doc = site.doc("a.md", ":::tip\n\nBody.\n\n:::\n");
    // no file, an unknown flag, an unknown subcommand, docs with both sources or a bad tag
    for args in [
        &["check"][..],
        &["check", "--nope", "a.md"],
        &["nope"],
        &["docs", "--package", "a", "--binding", "b"],
        &["docs", "--lang", "a b"],
        &["docs", "--package"],
    ] {
        let out = site.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"), "{args:?}");
    }
    assert_eq!(site.run(&["check", "missing.md"]).status.code(), Some(2));
    // a changed archive: the pin says no, and a clean file next to it does not hide that
    std::fs::write(site.folder().join("recipe.tar.gz"), b"other bytes").unwrap();
    let out = site.run(&["check", &doc]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stdout(&out).contains("is not the one the binding names"),
        "{}",
        stdout(&out)
    );
    let out = site.run(&["check", "--json", &doc]);
    assert_eq!(out.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["files"][0]["status"], "error");
    assert!(report["files"][0]["error"].as_str().unwrap().contains("sha256"));
    assert_eq!(report["summary"]["errors"], 1);
    // a broken binding is an error too, and the nearest one counts
    std::fs::write(site.folder().join("blocks.config.json"), b"{}").unwrap();
    assert_eq!(site.run(&["check", &doc]).status.code(), Some(2));
    assert_eq!(site.run(&["docs"]).status.code(), Some(2));
}

#[test]
fn help_names_both_commands() {
    let out = bin().args(["blocks", "--help"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("markise-cli blocks check [--json]") && text.contains("markise-cli blocks docs"));
    assert_eq!(
        bin()
            .args(["blocks", "check", "--help"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(0)
    );
}

const SNAPSHOT: &str = "tests/blocks/recipe-site.md";

/// The whole reference of the example package. `UPDATE_SNAPSHOT=1` writes it again; the hash is
/// checked apart from it, so that another version of the compressor is not a change of the text.
#[test]
fn docs_of_the_example_package_match_the_snapshot() {
    let out = bin()
        .current_dir(repo())
        .args(["blocks", "docs", "--package", "examples/block-package/package"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let packed = markise_package::pack_folder(&repo().join("examples/block-package/package")).unwrap();
    let text = stdout(&out);
    let sha = packed.sha256.to_string();
    assert!(text.contains(&format!("sha256 `{sha}`")), "{text}");
    let text = text.replace(&sha, "<sha256>");
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT);
    if std::env::var_os("UPDATE_SNAPSHOT").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(text, std::fs::read_to_string(&path).unwrap());
}

#[test]
fn docs_read_a_folder_an_archive_a_binding_or_the_nearest_binding() {
    let site = Site::new();
    let archive = site.run(&["docs", "--package", "recipe.tar.gz", "--lang", "fr"]);
    assert_eq!(archive.status.code(), Some(0));
    let text = stdout(&archive);
    assert!(
        text.contains(&format!("sha256 `{}`, language `fr`", site.sha256)),
        "{text}"
    );
    assert!(
        text.contains("markise-cli blocks docs --package recipe.tar.gz --lang fr\n"),
        "{text}"
    );
    assert!(
        text.contains("### `tip`: Astuce\n\n**Description:** Un conseil\n"),
        "{text}"
    );
    let bound = stdout(&site.run(&["docs", "--binding", "blocks.config.json"]));
    assert!(bound.contains(&format!("sha256 `{}`, language `en`", site.sha256)));
    let nearest = stdout(&site.run(&["docs"]));
    assert!(nearest.contains("markise-cli blocks docs\n"));
    assert_eq!(
        nearest.replace("markise-cli blocks docs\n", ""),
        bound.replace("markise-cli blocks docs --binding blocks.config.json\n", "")
    );
    // the same package from its folder, and a word a shell would split stays one
    let folder = site.folder().join("a folder");
    std::fs::create_dir_all(&folder).unwrap();
    let out = site.run(&["docs", "--package", "a folder"]);
    assert_eq!(out.status.code(), Some(2), "an empty folder has no manifest");
    let copy = stdout(
        &bin()
            .current_dir(repo())
            .args(["blocks", "docs", "--package", "examples/block-package/package/"])
            .output()
            .unwrap(),
    );
    assert!(copy.contains("### `question`: Question"));
    let two = stdout(&site.run(&["docs", "--package", "recipe.tar.gz", "--lang", "fr"]));
    assert_eq!(two, text, "the same package gives the same bytes");
}

#[test]
fn a_rule_of_docs_is_what_check_does() {
    // what the general rules say about a nesting fence and a repeated id, through the real check
    let site = Site::new();
    let nested = "::::tip\n\nBody.\n\n:::question\n\nWhich?\n\n- [x] A {#a}\n- [ ] B {#b}\n\n:::\n\n::::\n";
    site.doc("nested.md", nested);
    let text = stdout(&site.run(&["check", "nested.md"]));
    assert!(
        text.contains("nested.md:5: tip: Outside its schema: In no field: :::question…"),
        "{text}"
    );
    assert!(!text.contains("Marker"), "{text}");
    let same = ":::tip\n\nBody.\n\n:::question\n\nWhich?\n\n:::\n\n:::\n";
    site.doc("same.md", same);
    let text = stdout(&site.run(&["check", "same.md"]));
    assert!(
        text.contains("same.md:1: tip: Marker: `:::tip` has no closer"),
        "{text}"
    );
    assert!(text.contains("same.md:11: Marker: `:::` closes nothing"), "{text}");
}

/// A link counts where its file lives, as the app resolves it before it looks for a binding.
#[cfg(unix)]
#[test]
fn a_link_to_a_bound_file_is_checked_against_that_files_binding() {
    let site = Site::new();
    site.doc("real.md", ":::tip\n\n:::\n");
    let elsewhere = tempfile::tempdir().unwrap();
    let link = elsewhere.path().join("link.md");
    std::os::unix::fs::symlink(site.folder().join("real.md"), &link).unwrap();
    let out = site.run(&["check", link.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stdout(&out).contains(":1: tip: Not filled in yet: Text: Required"),
        "{}",
        stdout(&out)
    );
    // and the other way: a link in a bound folder to a file with no binding has none
    let plain = elsewhere.path().join("plain.md");
    std::fs::write(&plain, ":::tip\n\n:::\n").unwrap();
    let inside = site.folder().join("inside.md");
    std::os::unix::fs::symlink(&plain, &inside).unwrap();
    let out = site.run(&["check", "inside.md"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("no blocks.config.json applies"),
        "{}",
        stdout(&out)
    );
}

/// The example package with a field label and a block label that clear a terminal.
fn hostile_site() -> Site {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("package");
    copy_folder(&repo().join("examples/block-package/package"), &package);
    let schema = package.join("components/tip/tip.json");
    let mut tip: Value = serde_json::from_slice(&std::fs::read(&schema).unwrap()).unwrap();
    let evil = "\u{1b}[2J\u{1b}[H\u{9b}1m\u{202e}Text\n# Heading";
    tip["label"] = evil.into();
    tip["fields"][1]["label"] = evil.into();
    std::fs::write(&schema, tip.to_string()).unwrap();
    let packed = markise_package::pack_folder(&package).unwrap();
    let site = root.path().join("home/site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("recipe.tar.gz"), &packed.bytes).unwrap();
    std::fs::write(
        site.join("blocks.config.json"),
        format!(
            r#"{{"library":"recipe-site","version":"1.0.0","source":"recipe.tar.gz","sha256":"{}"}}"#,
            packed.sha256
        ),
    )
    .unwrap();
    Site {
        sha256: packed.sha256.to_string(),
        root,
    }
}

fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn raw_hidden(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| markise_package::is_hidden_char(*c) && *c != '\n')
        .collect()
}

#[test]
fn hostile_package_text_stays_data() {
    let site = hostile_site();
    site.doc("a.md", ":::tip\n\n:::\n");
    // the text of check
    let text = stdout(&site.run(&["check", "a.md"]));
    assert_eq!(raw_hidden(&text), [], "{text:?}");
    assert!(
        text.contains(r"a.md:1: tip: Not filled in yet: \u{1b}[2J\u{1b}[H\u{9b}1m\u{202e}Text\u{a}# Heading: Required"),
        "{text}"
    );
    assert_eq!(text.lines().count(), 2, "{text}");
    // the JSON of check: still JSON, every such character an escape, the value intact
    let out = site.run(&["check", "--json", "a.md"]);
    let json = stdout(&out);
    assert_eq!(raw_hidden(&json), [], "{json:?}");
    assert!(
        json.contains(r"\u001b[2J") && json.contains(r"\u009b1m") && json.contains(r"\u202e"),
        "{json}"
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["files"][0]["problems"][0]["reason"],
        "\u{1b}[2J\u{1b}[H\u{9b}1m\u{202e}Text\n# Heading: Required"
    );
    // the reference
    let docs = stdout(&site.run(&["docs", "--package", "recipe.tar.gz"]));
    assert_eq!(raw_hidden(&docs), [], "{docs:?}");
    assert!(!docs.lines().any(|l| l.starts_with("# Heading")), "{docs}");
    assert!(
        docs.contains(r"### `tip`: \\u\{1b\}\[2J\\u\{1b\}\[H\\u\{9b\}1m\\u\{202e\}Text \# Heading"),
        "{docs}"
    );
    // an error that quotes a package
    let package = site.folder().join("broken");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("manifest.json"),
        "{\"format\":\"markise-block-package/2\",\"library\":\"\\u001b[2J\",\"version\":\"1\",\"blocks\":[]}",
    )
    .unwrap();
    let out = site.run(&["docs", "--package", "broken"]);
    assert_eq!(out.status.code(), Some(2));
    let error = String::from_utf8(out.stderr).unwrap();
    assert_eq!(raw_hidden(&error), [], "{error:?}");
}
