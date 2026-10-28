//! Resolution tests on the spec 06 §16.1 fixture vault (tables §16.2, §16.4, §16.7).

use super::*;

/// The §16.1 fixture tree (`.md` unless shown; dot-folders are never indexed, so they are left
/// out here as the scanner leaves them out). `case_sensitive`: also `Case/draft.md`.
fn fixture(case_sensitive: bool) -> VaultIndex {
    let mut v: Vec<&str> = vec![
        "Home.md",
        "Readme.md",
        "Ideas.md",
        "Inbox/Ideas.md",
        "Projects/Plan.md",
        "Projects/Alpha.md",
        "Areas/Today.md",
        "Projects/Alpha/Readme.md",
        "Areas/Plan.md",
        "Areas/plan notes.md",
        "Archive/2024/Plan.md",
        "Notes/Report v1.8.md",
        "Notes/Cafe\u{301}.md", // stored NFD on disk
        "Notes/Q&A.md",
        "Notes/50% done.md",
        "Notes/Hash#Tag.md",
        "Case/Draft.md",
        "Attachments/Diagram.png",
        "Attachments/Spec.pdf",
        "Attachments/data.json",
    ];
    if case_sensitive {
        v.push("Case/draft.md");
    }
    VaultIndex::from_rels(PathBuf::from("/v"), v.into_iter().map(String::from), 1)
}

fn wl(ix: &VaultIndex, from: &str, target: &str) -> Option<String> {
    ix.resolve_wikilink(target, Some(from)).map(|f| f.rel.clone())
}

fn md(ix: &VaultIndex, from: &str, dest: &str) -> Option<String> {
    ix.resolve_markdown(dest, Some(from)).map(|r| r.rel)
}

#[test]
fn wikilink_resolution_l1_to_l32() {
    let ix = fixture(false);
    let s = |x: &str| Some(x.to_string());
    assert_eq!(wl(&ix, "Home.md", "Ideas"), s("Ideas.md"), "L1");
    assert_eq!(wl(&ix, "Inbox/Ideas.md", "Ideas"), s("Inbox/Ideas.md"), "L2");
    assert_eq!(wl(&ix, "Areas/plan notes.md", "Plan"), s("Areas/Plan.md"), "L3");
    assert_eq!(wl(&ix, "Home.md", "Plan"), s("Areas/Plan.md"), "L4");
    assert_eq!(wl(&ix, "Home.md", "Projects/Plan"), s("Projects/Plan.md"), "L5");
    assert_eq!(wl(&ix, "Home.md", "2024/Plan"), s("Archive/2024/Plan.md"), "L6");
    assert_eq!(wl(&ix, "Projects/Alpha.md", "./Plan"), s("Projects/Plan.md"), "L7");
    assert_eq!(wl(&ix, "Projects/Alpha.md", "../Areas/Plan"), s("Areas/Plan.md"), "L8");
    assert_eq!(wl(&ix, "Home.md", "/Readme"), s("Readme.md"), "L9");
    assert_eq!(wl(&ix, "Projects/Alpha.md", "Readme"), s("Readme.md"), "L10");
    assert_eq!(
        wl(&ix, "Projects/Alpha/Readme.md", "Readme"),
        s("Projects/Alpha/Readme.md"),
        "L11"
    );
    assert_eq!(wl(&ix, "Home.md", "Alpha"), s("Projects/Alpha.md"), "L12");
    assert_eq!(wl(&ix, "Home.md", "Report v1.8"), s("Notes/Report v1.8.md"), "L13");
    assert_eq!(wl(&ix, "Home.md", "Report v1.8.md"), s("Notes/Report v1.8.md"), "L14");
    assert_eq!(wl(&ix, "Home.md", "Diagram.png"), s("Attachments/Diagram.png"), "L15");
    assert_eq!(wl(&ix, "Home.md", "Diagram"), None, "L16");
    assert_eq!(wl(&ix, "Home.md", "café"), s("Notes/Café.md"), "L17 (NFC/NFD, case)");
    assert_eq!(wl(&ix, "Home.md", "Q&A"), s("Notes/Q&A.md"), "L18");
    // L19: `[[Hash#Tag]]` targets `Hash` (the parser split off the subpath)
    assert_eq!(wl(&ix, "Home.md", "Hash"), None, "L19");
    assert_eq!(wl(&ix, "Home.md", "draft"), s("Case/Draft.md"), "L21");
    assert_eq!(wl(&ix, "Home.md", "Old"), None, "L23 (.trash is not indexed)");
    assert_eq!(wl(&ix, "Home.md", "  Plan  "), s("Areas/Plan.md"), "L24");
    assert_eq!(wl(&ix, "Home.md", "_TOC_"), None, "L30");
    assert_eq!(wl(&ix, "Home.md", "data.json"), s("Attachments/data.json"), "L32");
    // V2.4.1: an empty target is the linking note
    assert_eq!(wl(&ix, "Projects/Plan.md", ""), s("Projects/Plan.md"));
}

#[test]
fn l22_e15_exact_case_wins() {
    let ix = fixture(true);
    assert_eq!(wl(&ix, "Home.md", "draft"), Some("Case/draft.md".into()), "L22");
    assert_eq!(wl(&ix, "Home.md", "Draft"), Some("Case/Draft.md".into()));
    let e15 = VaultIndex::from_rels(PathBuf::from("/v"), ["Plan.md".into(), "plan.md".into()], 1);
    assert_eq!(wl(&e15, "x.md", "plan"), Some("plan.md".into()));
    assert_eq!(wl(&e15, "x.md", "PLAN"), Some("Plan.md".into()), "path order");
    assert_eq!(e15.len(), 2, "both indexed");
}

#[test]
fn markdown_link_resolution_m1_to_m9() {
    let ix = fixture(false);
    let s = |x: &str| Some(x.to_string());
    assert_eq!(md(&ix, "Projects/Alpha.md", "Plan.md"), s("Projects/Plan.md"), "M1");
    assert_eq!(md(&ix, "Home.md", "Plan.md"), s("Areas/Plan.md"), "M2");
    assert_eq!(md(&ix, "Projects/Alpha.md", "/Areas/Plan.md"), s("Areas/Plan.md"), "M3");
    assert_eq!(
        md(&ix, "Areas/Plan.md", "plan%20notes.md"),
        s("Areas/plan notes.md"),
        "M4"
    );
    assert_eq!(
        md(&ix, "Areas/Plan.md", "<plan notes.md>"),
        s("Areas/plan notes.md"),
        "M5"
    );
    assert_eq!(
        md(&ix, "Home.md", "Notes/50%25%20done.md"),
        s("Notes/50% done.md"),
        "M6"
    );
    assert_eq!(md(&ix, "Home.md", "../outside.md"), None, "M7");
    assert_eq!(md(&ix, "Home.md", "https://example.com/Plan.md"), None, "M8");
    assert_eq!(md(&ix, "Home.md", "Projects/Plan"), s("Projects/Plan.md"), "M9");
    // L20
    assert_eq!(md(&ix, "Home.md", "Notes/Hash%23Tag.md"), s("Notes/Hash#Tag.md"), "L20");
    let r = ix.resolve_markdown("Projects/Plan.md#Goals", Some("Home.md")).unwrap();
    assert_eq!(r.fragment.as_deref(), Some("Goals"));
    assert_eq!(ix.resolve_markdown("#x", Some("Home.md")), None);
}

#[test]
fn shortest_links_v14_3() {
    let ix = fixture(false);
    let f = |r: &str| ix.file(r).unwrap().clone();
    assert_eq!(ix.shortest_link(&f("Projects/Plan.md")), "Projects/Plan");
    assert_eq!(ix.shortest_link(&f("Notes/Q&A.md")), "Q&A");
    assert_eq!(ix.shortest_link(&f("Attachments/Diagram.png")), "Diagram.png");
    assert_eq!(ix.shortest_link(&f("Home.md")), "Home");
}

#[test]
fn suggestions_rank_v3_3() {
    let ix = fixture(false);
    let names = |q: &str| ix.suggest(q, 50).iter().map(|f| f.rel.clone()).collect::<Vec<_>>();
    let plan = names("plan");
    // exact stems first (shorter paths first), then the prefix match
    assert_eq!(
        &plan[..3],
        ["Areas/Plan.md", "Projects/Plan.md", "Archive/2024/Plan.md"]
    );
    assert_eq!(plan[3], "Areas/plan notes.md");
    assert!(
        names("pn").contains(&"Areas/plan notes.md".to_string()),
        "word-start subsequence"
    );
    assert!(names("zzz").is_empty());
    // only notes
    assert!(!names("diag").iter().any(|r| r.ends_with(".png")));
    // path match
    assert!(names("inbox").contains(&"Inbox/Ideas.md".to_string()));
    assert_eq!(ix.suggest("", 3).len(), 3);
}

#[test]
fn path_helpers() {
    assert_eq!(percent_decode("a%20b%zz%"), "a b%zz%");
    assert_eq!(percent_encode_path("Hash#Tag (1).md"), "Hash%23Tag%20%281%29.md");
    assert_eq!(join_rel("a/b", "../c"), Some("a/c".into()));
    assert_eq!(join_rel("", "../c"), None);
    assert_eq!(
        rel_path(Path::new("/v"), Path::new("/v/Notes/Cafe\u{301}.md")).as_deref(),
        Some("Notes/Café.md")
    );
    assert_eq!(rel_path(Path::new("/v"), Path::new("/w/x.md")), None);
    assert_eq!(
        relative_path(Path::new("/v/a"), Path::new("/v/b/c d.md")).as_deref(),
        Some("../b/c d.md")
    );
}

#[test]
fn scanner_tracks_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("sub/deep")).unwrap();
    std::fs::create_dir_all(root.join(".obsidian")).unwrap();
    std::fs::write(root.join("A.md"), "").unwrap();
    std::fs::write(root.join("sub/B.md"), "").unwrap();
    std::fs::write(root.join("sub/deep/C.md"), "").unwrap();
    std::fs::write(root.join(".obsidian/app.json"), "{}").unwrap();
    let sc = VaultScanner::new(Arc::new(|| {})).unwrap();
    let t = std::time::Duration::from_secs(10);
    sc.mount(root.clone());
    let ix = sc.recv_timeout(t).unwrap();
    let rels: Vec<&str> = ix.files().iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["A.md", "sub/B.md", "sub/deep/C.md"]);
    // a new file and a new folder appear after the watcher reports their directories
    std::fs::write(root.join("sub/D.md"), "").unwrap();
    std::fs::create_dir_all(root.join("sub/new")).unwrap();
    std::fs::write(root.join("sub/new/E.md"), "").unwrap();
    sc.changed(root.join("sub"));
    let ix2 = sc.recv_timeout(t).unwrap();
    assert!(ix2.generation() > ix.generation());
    assert!(ix2.file("sub/D.md").is_some() && ix2.file("sub/new/E.md").is_some());
    // a removed folder drops its subtree
    std::fs::remove_dir_all(root.join("sub/deep")).unwrap();
    sc.changed(root.join("sub/deep"));
    let ix3 = sc.recv_timeout(t).unwrap();
    assert!(ix3.file("sub/deep/C.md").is_none());
    assert!(ix3.file("sub/B.md").is_some());
}
