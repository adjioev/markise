//! Pandoc bridge against an installed Pandoc (spec 04 §8.2). Skipped (with a note on stderr)
//! when `pandoc` is not installed.
//!
//! - JSON round trip: our AST → `pandoc -f json -t json` must come back unchanged (Pandoc
//!   accepts every node we emit and reads it as we meant it).
//! - Native comparison: for constructs where our dialect and Pandoc's CommonMark agree, our
//!   AST and Pandoc's own reading of the same Markdown give the same `native` output.
//! - Real exports: docx (zip container) and LaTeX output written through `export`.

use std::path::PathBuf;
use std::time::Duration;

use markise_export::job::JobControl;
use markise_export::model::{ExDoc, ModelOptions};
use markise_export::pandoc::{self, PandocFormat, PandocInfo, PandocOptions};
use serde_json::Value;

fn info() -> Option<PandocInfo> {
    let Some(p) = pandoc::find_pandoc(None) else {
        eprintln!("pandoc not installed: skipping Pandoc tests");
        return None;
    };
    Some(pandoc::probe(&p, &JobControl::new()).expect("probe installed pandoc"))
}

fn model(md: &str) -> ExDoc {
    ExDoc::build(&markise_md::parse(md), &ModelOptions::default())
}

fn run(info: &PandocInfo, args: &[&str], input: &str) -> String {
    let out = pandoc::run(
        &info.path,
        args,
        None,
        input.as_bytes(),
        Duration::from_secs(60),
        &JobControl::new(),
    )
    .unwrap_or_else(|e| panic!("pandoc {args:?}: {e}"));
    String::from_utf8(out).unwrap()
}

const RICH: &str = "---\ntitle: Round trip\nauthor: Ann\n---\n# Intro\n\nText with **bold**, *em*, ~~del~~, `code`, <u>under</u>, <mark>mark</mark>, \
H<sub>2</sub>O, x<sup>2</sup>, a [link](https://example.org \"T\"), ![img](a.png) and a note[^1].\n\n\
> quote\n\n> [!NOTE]\n> alert body\n\n- one\n- [x] done\n- [ ] open\n\n3. three\n4. four\n\n\
| a | b |\n|:--|--:|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n$$\nx^2\n$$\n\n---\n\nInline $e$ math.\n\n[^1]: The note.\n";

#[test]
fn json_round_trip_is_identity() {
    let Some(info) = info() else { return };
    let ours = pandoc::to_pandoc_json(&model(RICH), &info.api);
    let back = run(&info, &["-f", "json", "-t", "json"], &ours.to_string());
    let back: Value = serde_json::from_str(&back).unwrap();
    assert_eq!(back["blocks"], ours["blocks"], "Pandoc changed our blocks");
    assert_eq!(back["meta"], ours["meta"]);
}

#[test]
fn native_matches_pandoc_reading_the_same_markdown() {
    let Some(info) = info() else { return };
    let md = "# Intro\n\nSome *emphasis* and **strong** text with `code` and a [link](https://example.org).\n\n\
- tight one\n- tight two\n\n1. first\n2. second\n\n> A quote.\n\n```python\nprint(1)\n```\n\n---\n\nLast paragraph.\n";
    let ours = pandoc::to_pandoc_json(&model(md), &info.api).to_string();
    let a = run(&info, &["-f", "json", "-t", "native"], &ours);
    let b = run(&info, &["-f", "gfm", "-t", "native"], md);
    assert_eq!(a, b);
}

#[test]
fn extension_mapping_in_native_output() {
    let Some(info) = info() else { return };
    let ours = pandoc::to_pandoc_json(&model(RICH), &info.api).to_string();
    let native = run(&info, &["-f", "json", "-t", "native", "-s"], &ours);
    for needle in [
        "Underline [ Str \"under\" ]",
        "Span ( \"\" , [ \"mark\" ] , [] ) [ Str \"mark\" ]",
        "( \"\" , [ \"note\" ] , [] )",
        "Div ( \"\" , [ \"title\" ] , [] ) [ Para [ Str \"Note\" ] ]",
        "Str \"\\9746\"",
        "Note",
        "Math DisplayMath \"x^2\"",
        "CodeBlock ( \"\" , [ \"rust\" ] , [] ) \"fn main() {}\"",
        "( 3 , Decimal , Period )",
        "AlignRight",
        "MetaInlines [ Str \"Round\" , Space , Str \"trip\" ]",
    ] {
        assert!(native.contains(needle), "missing {needle:?} in:\n{native}");
    }
}

#[test]
fn exports_docx_and_latex() {
    let Some(info) = info() else { return };
    let dir = tempfile::tempdir().unwrap();
    let doc = model(RICH);
    let docx: PathBuf = dir.path().join("out.docx");
    pandoc::export(
        &info,
        &doc,
        &docx,
        &PandocOptions::new(PandocFormat::Docx),
        &JobControl::new(),
    )
    .unwrap();
    let bytes = std::fs::read(&docx).unwrap();
    assert!(bytes.starts_with(b"PK"), "docx is a zip container");
    let tex = dir.path().join("out.tex");
    pandoc::export(
        &info,
        &doc,
        &tex,
        &PandocOptions::new(PandocFormat::Latex),
        &JobControl::new(),
    )
    .unwrap();
    let t = std::fs::read_to_string(&tex).unwrap();
    assert!(t.contains("\\section") && t.contains("Intro") && t.contains("\\documentclass"));
    for f in [
        PandocFormat::Rst,
        PandocFormat::MediaWiki,
        PandocFormat::Textile,
        PandocFormat::Opml,
    ] {
        let p = dir.path().join(format!("out.{}", f.extension()));
        pandoc::export(&info, &doc, &p, &PandocOptions::new(f), &JobControl::new())
            .unwrap_or_else(|e| panic!("{f:?}: {e}"));
        assert!(std::fs::metadata(&p).unwrap().len() > 0);
    }
}

#[test]
fn pandoc_errors_carry_stderr() {
    let Some(info) = info() else { return };
    let e = pandoc::run(
        &info.path,
        &["-f", "json", "-t", "native"],
        None,
        b"{not json",
        Duration::from_secs(30),
        &JobControl::new(),
    )
    .unwrap_err();
    match e {
        pandoc::PandocError::Failed { code, stderr } => {
            assert_ne!(code, Some(0));
            assert!(!stderr.is_empty());
        }
        other => panic!("{other:?}"),
    }
}
