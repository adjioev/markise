//! End-to-end tests of the `markise-cli` binary.
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_markise-cli"))
}

fn corpus(name: &str) -> String {
    format!(
        "{}/../../crates/markise-md/tests/corpus/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn roundtrip_ok_for_corpus() {
    for f in ["crlf.md", "mixed-eol.md", "edge.md", "no-newline.md", "frontmatter.md"] {
        let out = bin().args(["roundtrip", &corpus(f)]).output().unwrap();
        assert!(out.status.success(), "{f}: {}", String::from_utf8_lossy(&out.stdout));
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("OK "));
    }
}

#[test]
fn html_export_has_contract_skeleton() {
    let out = bin().args(["html", &corpus("tables.md")]).output().unwrap();
    assert!(out.status.success());
    let html = String::from_utf8(out.stdout).unwrap();
    assert!(html.contains("<body class='typora-export'>"));
    assert!(html.contains("<div id='write'"));
    assert!(html.contains("<figure class='table-figure'>"));
    assert!(html.contains("<title>tables</title>"));
}

#[test]
fn usage_errors_exit_2() {
    let out = bin().arg("bogus").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = bin().args(["roundtrip", "/nonexistent/file.md"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn theme_import_prints_summary_and_json_report() {
    let dir = std::env::temp_dir().join(format!("markise-cli-theme-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let css = dir.join("night-owl.css");
    std::fs::write(
        &css,
        "body { background: #101418; color: #d0d4d8 }\n#write h1 { color: #ffcc66 }\np { frobnicate: 1 }\n",
    )
    .unwrap();
    let out = bin().args(["theme-import", css.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("theme: \"Night Owl\" (dark)"), "{text}");
    assert!(text.contains("page #101418"), "{text}");
    assert!(text.contains("frobnicate  [night-owl.css:3]"), "{text}");

    let out = bin()
        .args(["theme-import", css.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["theme"]["dark"], true);
    assert_eq!(v["theme"]["headings"][0]["color"], "#ffcc66");
    assert_eq!(v["report"]["unsupported_properties"][0]["name"], "frobnicate");
    let _ = std::fs::remove_dir_all(&dir);

    let out = bin().args(["theme-import", "/nonexistent/x.css"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// Issue #29: an HTML export estimated above 200 MB prints a warning and is still written;
/// with `--max-size` below the estimate it is refused and nothing is written.
#[test]
fn html_size_warning_and_max_size() {
    let dir = std::env::temp_dir().join(format!("markise-cli-size-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 1500 headings (~110 KB of TOC each) and 2000 [TOC] blocks: ~220 MB estimated
    let mut md = String::new();
    for i in 0..2000 {
        md.push_str("[TOC]\n\n");
        if i < 1500 {
            md.push_str(&format!("## A rather long heading number {i} for the size test\n\n"));
        }
    }
    let src = dir.join("tocs.md");
    std::fs::write(&src, &md).unwrap();
    let src = src.to_str().unwrap();

    // refused: exit 1, message on stderr, no file
    let out_path = dir.join("refused.html");
    let out = bin()
        .args(["html", src, "-o", out_path.to_str().unwrap(), "--max-size", "1M"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("refusing") && err.contains("2000 tables of contents"),
        "{err}"
    );
    assert!(!out_path.exists());

    // warning only: written in full
    let out_path = dir.join("big.html");
    let out = bin()
        .args(["html", src, "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning") && err.contains("2000 tables of contents"),
        "{err}"
    );
    assert!(std::fs::metadata(&out_path).unwrap().len() > 200_000_000);

    // a bad size is a usage error; a normal document prints nothing
    let out = bin().args(["html", src, "--max-size", "lots"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let small = dir.join("small.md");
    std::fs::write(&small, "[TOC]\n\n# A\n").unwrap();
    let out = bin()
        .args(["html", small.to_str().unwrap(), "--max-size", "500M"])
        .output()
        .unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
