use std::fs;
use std::path::Path;

use markise_package::{BINDING_FILE, find_binding, parse_binding, read_binding};

const SHA: &str = "aaaabbbbccccddddaaaabbbbccccddddaaaabbbbccccddddaaaabbbbccccdddd";

fn json(source: &str) -> String {
    format!(r#"{{"library":"sample","version":"1.2.0","source":"{source}","sha256":"{SHA}"}}"#)
}

#[test]
fn nearest_binding_wins_and_the_walk_stops_at_home() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let home = root.join("home");
    let deep = home.join("a/b/c");
    fs::create_dir_all(&deep).unwrap();
    let doc = deep.join("doc.md");
    assert_eq!(find_binding(&doc, Some(&home)), None);

    fs::write(root.join(BINDING_FILE), json("x.tar.gz")).unwrap();
    assert_eq!(find_binding(&doc, Some(&home)), None, "above home is not looked at");
    assert_eq!(
        find_binding(&doc, None),
        Some(root.join(BINDING_FILE)),
        "without a home the walk goes on"
    );

    fs::write(home.join(BINDING_FILE), json("x.tar.gz")).unwrap();
    assert_eq!(
        find_binding(&doc, Some(&home)),
        Some(home.join(BINDING_FILE)),
        "home itself counts"
    );

    let near = home.join("a/b").join(BINDING_FILE);
    fs::write(&near, json("x.tar.gz")).unwrap();
    assert_eq!(find_binding(&doc, Some(&home)), Some(near));

    let nearest = deep.join(BINDING_FILE);
    fs::write(&nearest, json("x.tar.gz")).unwrap();
    assert_eq!(
        find_binding(&doc, Some(&home)),
        Some(nearest),
        "the document's folder comes first"
    );
}

#[test]
fn a_relative_document_path_ends_the_walk() {
    assert_eq!(find_binding(Path::new("doc.md"), None), None);
}

#[test]
fn parses_a_binding() {
    let upper = json("blocks/x.tar.gz").replace(SHA, &SHA.to_uppercase());
    let b = parse_binding(Path::new("/p/blocks.config.json"), upper.as_bytes()).unwrap();
    assert_eq!(b.library, "sample");
    assert_eq!(b.version, "1.2.0");
    assert_eq!(b.source, "blocks/x.tar.gz");
    assert_eq!(b.sha256, SHA, "lowercase");
    assert_eq!(b.folder(), Path::new("/p"));
}

#[test]
fn refuses_bad_bindings() {
    let file = Path::new("/p/blocks.config.json");
    let cases = [
        ("not json".to_string(), "not valid JSON"),
        (json("x").replace("library", "lib"), "no \"library\""),
        (json("x").replace("version", "ver"), "no \"version\""),
        (json("x").replace("source", "src"), "no \"source\""),
        (json("x").replace("sha256", "sha"), "no \"sha256\""),
        (json("x").replace(SHA, "abc"), "64 hexadecimal"),
        (json("x").replace(SHA, &"z".repeat(64)), "64 hexadecimal"),
        (json("x").replace("sample", "a/b"), "not valid"),
        (json("../x.tar.gz"), "outside the folder"),
        (json("/abs/x.tar.gz"), "absolute"),
        (json("~/x.tar.gz"), "outside the folder"),
        (json("C:/x.tar.gz"), "holds the character"),
        (json("."), "does not name a file"),
        (json("https://example.com/x.tar.gz"), "network address"),
    ];
    for (text, expected) in cases {
        let e = parse_binding(file, text.as_bytes()).unwrap_err();
        assert!(e.contains(expected), "{text}: {e}");
    }
}

#[cfg(unix)]
#[test]
fn a_binding_that_is_a_link_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.json");
    fs::write(&real, json("x.tar.gz")).unwrap();
    let file = dir.path().join(BINDING_FILE);
    std::os::unix::fs::symlink(&real, &file).unwrap();
    assert_eq!(find_binding(&dir.path().join("doc.md"), None), Some(file.clone()));
    assert!(read_binding(&file).unwrap_err().contains("link"));
}

#[test]
fn an_oversized_binding_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(BINDING_FILE);
    fs::write(&file, " ".repeat(65 << 10)).unwrap();
    assert!(read_binding(&file).unwrap_err().contains("larger than"));
}
