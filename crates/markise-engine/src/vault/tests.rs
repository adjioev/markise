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

#[test]
fn settings_q_v15_q_v18_and_app_json() {
    let plain = VaultSettings::plain();
    assert!(!plain.obsidian && !plain.wikilinks);
    assert_eq!(plain.link_format, LinkFormat::Relative);
    assert_eq!(plain.new_notes, NewNoteLocation::Current);
    let ob = VaultSettings::obsidian(None);
    assert!(ob.obsidian && ob.wikilinks);
    assert_eq!(
        (ob.link_format, ob.new_notes.clone()),
        (LinkFormat::Shortest, NewNoteLocation::Root)
    );
    assert_eq!(VaultSettings::obsidian(Some("{}")), ob, "E-fixture: empty app.json");
    let custom = VaultSettings::obsidian(Some(
        r#"{"useMarkdownLinks": true, "newLinkFormat": "absolute", "newFileLocation": "folder", "newFileFolderPath": "/Inbox/"}"#,
    ));
    assert!(!custom.wikilinks);
    assert_eq!(custom.link_format, LinkFormat::Absolute);
    assert_eq!(custom.new_notes, NewNoteLocation::Folder("Inbox".into()));
    // unknown values are ignored
    let odd = VaultSettings::obsidian(Some(r#"{"newLinkFormat": "weird", "useMarkdownLinks": "yes"}"#));
    assert_eq!(odd, ob);
    assert_eq!(VaultSettings::obsidian(Some("not json")), ob);
}

#[test]
fn links_written_per_v14_3() {
    let ix = fixture(false);
    let f = |r: &str| ix.file(r).unwrap().clone();
    let from = Some("Areas/Today.md");
    let with = |fmt: LinkFormat| {
        fixture(false).with_settings(VaultSettings {
            link_format: fmt,
            ..VaultSettings::obsidian(None)
        })
    };
    let sh = with(LinkFormat::Shortest);
    assert_eq!(sh.link_for(&f("Projects/Plan.md"), from, true), "[[Projects/Plan]]");
    assert_eq!(sh.link_for(&f("Notes/Q&A.md"), from, true), "[[Q&A]]");
    let rel = with(LinkFormat::Relative);
    assert_eq!(rel.link_for(&f("Projects/Plan.md"), from, true), "[[../Projects/Plan]]");
    assert_eq!(rel.link_for(&f("Areas/Plan.md"), from, true), "[[Plan]]");
    let abs = with(LinkFormat::Absolute);
    assert_eq!(abs.link_for(&f("Projects/Plan.md"), from, true), "[[Projects/Plan]]");
    // Markdown links: extension kept, path encoded (V2.6.3)
    assert_eq!(
        rel.link_for(&f("Areas/plan notes.md"), Some("Home.md"), false),
        "[plan notes](Areas/plan%20notes.md)"
    );
    assert_eq!(
        rel.link_for(&f("Projects/Plan.md"), from, false),
        "[Plan](../Projects/Plan.md)"
    );
    // E6: a name that cannot be a wikilink becomes a Markdown link
    assert_eq!(
        abs.link_for(&f("Notes/Hash#Tag.md"), Some("Home.md"), true),
        "[Hash#Tag](Notes/Hash%23Tag.md)"
    );
}

#[test]
fn scanner_reads_obsidian_settings() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("A.md"), "").unwrap();
    let sc = VaultScanner::new(Arc::new(|| {})).unwrap();
    let t = std::time::Duration::from_secs(10);
    sc.mount(root.clone());
    assert!(!sc.recv_timeout(t).unwrap().settings().obsidian);
    std::fs::create_dir_all(root.join(".obsidian")).unwrap();
    std::fs::write(root.join(".obsidian/app.json"), r#"{"useMarkdownLinks": true}"#).unwrap();
    sc.changed(root.join(".obsidian"));
    let s = sc.recv_timeout(t).unwrap().settings().clone();
    assert!(s.obsidian && !s.wikilinks);
    // unchanged settings (Obsidian rewriting app.json or its other files): no new snapshot
    std::fs::write(root.join(".obsidian/app.json"), r#"{"useMarkdownLinks":true}"#).unwrap();
    std::fs::write(root.join(".obsidian/workspace.json"), "{}").unwrap();
    sc.changed(root.join(".obsidian"));
    sc.changed(root.join(".obsidian/workspace.json"));
    assert!(
        sc.recv_timeout(std::time::Duration::from_millis(500)).is_none(),
        "no snapshot"
    );
    // a real change does publish
    std::fs::write(root.join(".obsidian/app.json"), r#"{"useMarkdownLinks":false}"#).unwrap();
    sc.changed(root.join(".obsidian"));
    assert!(sc.recv_timeout(t).unwrap().settings().wikilinks);
    assert!(!root.join(".markise").exists());
}

/// Review P2-6: names stored NFD on disk resolve from NFC links and open with their disk
/// spelling; NFD folders are scanned.
#[test]
fn nfd_names_on_disk_resolve_and_open() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let nfd_dir = "Re\u{301}sume\u{301}";
    std::fs::create_dir_all(root.join(nfd_dir)).unwrap();
    std::fs::write(root.join(nfd_dir).join("Cafe\u{301}.md"), "x").unwrap();
    let sc = VaultScanner::new(Arc::new(|| {})).unwrap();
    sc.mount(root.clone());
    let ix = sc.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let f = ix.resolve_wikilink("café", None).expect("found through NFC");
    assert_eq!(f.rel, "Résumé/Café.md");
    let abs = ix.abs(&f.rel);
    assert_eq!(
        std::fs::read_to_string(&abs).unwrap(),
        "x",
        "opens with the disk spelling"
    );
    assert_eq!(ix.rel_of(&abs).as_deref(), Some("Résumé/Café.md"));
    // a change event for the NFD folder is applied
    std::fs::write(root.join(nfd_dir).join("Two.md"), "").unwrap();
    sc.changed(root.join(nfd_dir));
    let ix2 = sc.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(ix2.file("Résumé/Two.md").is_some());
}

/// Review P2-8: portable names only.
#[test]
fn new_note_names_are_portable() {
    for ok in ["New idea.md", "Projects/New/Deep.md", "Console.md", "a.b.md", "Café.md"] {
        assert!(check_new_note(ok).is_ok(), "{ok}");
    }
    for bad in [
        "a:b.md",
        "CON.md",
        "con.txt.md",
        "Projects/NUL/x.md",
        "lpt9.md",
        "COM1 .md",
        "trail./x.md",
        "x /y.md",
        " lead.md",
        "x/../y.md",
        ".obsidian/New.md",
        "Notes/.hidden.md",
        "q?.md",
        "",
    ] {
        assert!(check_new_note(bad).is_err(), "{bad:?}");
    }
}

/// Review P1-1 / P1-2: creation never leaves the vault or enters a protected folder, not even
/// through symbolic links.
#[cfg(unix)]
#[test]
fn note_creation_stays_inside_the_vault() {
    let outer = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(outer.path()).unwrap();
    let root = base.join("vault");
    let outside = base.join("outside");
    std::fs::create_dir_all(root.join(".obsidian")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
    std::os::unix::fs::symlink(root.join(".obsidian"), root.join("cfg")).unwrap();
    assert!(create_note(&root, "out/New.md").is_err());
    assert!(create_note(&root, "out/deeper/New.md").is_err());
    assert!(create_note(&root, "cfg/New.md").is_err());
    assert!(create_note(&root, ".obsidian/New.md").is_err());
    // a symbolic link where the note would go (created by a racing process): refused, the
    // target outside untouched
    std::fs::write(outside.join("target.md"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.join("target.md"), root.join("Link.md")).unwrap();
    assert!(create_note(&root, "Link.md").is_err());
    assert_eq!(std::fs::read_to_string(outside.join("target.md")).unwrap(), "outside");
    std::fs::remove_file(outside.join("target.md")).unwrap();
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(root.join(".obsidian")).unwrap().count(), 0);
    // normal creation, folders included; an existing file is returned unchanged
    let p = create_note(&root, "Projects/New/Deep.md").unwrap();
    assert_eq!(p, root.join("Projects/New/Deep.md"));
    std::fs::write(&p, "kept").unwrap();
    assert_eq!(create_note(&root, "Projects/New/Deep.md").unwrap(), p);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "kept");
    let rx = spawn_create_note(root.clone(), "Top.md".into(), Arc::new(|| {}));
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap().unwrap(),
        root.join("Top.md")
    );
}

/// Review round 2: a new note under an existing NFD folder goes into that folder (no NFC
/// twin is created).
#[test]
fn creation_keeps_the_disk_spelling_of_folders() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let nfd = "Re\u{301}sume\u{301}";
    std::fs::create_dir_all(root.join(nfd)).unwrap();
    let p = create_note(&root, "Résumé/New.md").unwrap();
    assert_eq!(p, root.join(nfd).join("New.md"));
    let names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [nfd.to_string()], "no second folder");
}

/// Review round 2: every generated link parses back and resolves to the chosen file — with
/// syntax characters in folder and file names and descendant paths that a shorter form
/// would send elsewhere.
#[test]
fn generated_links_round_trip() {
    use markise_md::inline::{InlineContext, InlineKind, parse_inlines};
    let rels = [
        "A/Here.md",
        "A/Sub/Note.md",
        "Sub/Note.md",
        "Topic].md",
        "We#ird/Plan.md",
        "Pipe|Dir/X.md",
        "Brackets [x]/Y.md",
        "a b/c d.md",
        "Star*_name.md",
        "50% done.md",
        "Café/Résumé.md",
    ];
    for fmt in [LinkFormat::Shortest, LinkFormat::Relative, LinkFormat::Absolute] {
        for obsidian in [true, false] {
            let settings = VaultSettings {
                link_format: fmt,
                ..if obsidian {
                    VaultSettings::obsidian(None)
                } else {
                    VaultSettings::plain()
                }
            };
            let ix = VaultIndex::from_rels(PathBuf::from("/v"), rels.iter().map(|r| r.to_string()), 1)
                .with_settings(settings);
            for from in ["A/Here.md", "Sub/Note.md", "Topic].md"] {
                for f in ix.files() {
                    let link = ix.link_for(f, Some(from), obsidian);
                    let opts = markise_md::ParseOptions::default();
                    let nodes = parse_inlines(&link, &opts, &InlineContext::default());
                    assert_eq!(nodes.len(), 1, "{fmt:?} {from} → {}: one node in {link:?}", f.rel);
                    let got = match &nodes[0].kind {
                        InlineKind::WikiLink { target, subpath, .. } => {
                            assert!(subpath.is_none(), "{link}");
                            ix.resolve_wikilink(target, Some(from)).map(|g| g.rel.clone())
                        }
                        InlineKind::Link { dest, .. } => ix.resolve_markdown(dest, Some(from)).map(|r| r.rel),
                        k => panic!("{link}: {k:?}"),
                    };
                    assert_eq!(got.as_deref(), Some(f.rel.as_str()), "{fmt:?} {from}: {link}");
                }
            }
        }
    }
    // the descendant case: from A/, A/Sub/Note.md must not be written as `Sub/Note`
    let ix = VaultIndex::from_rels(PathBuf::from("/v"), rels.iter().map(|r| r.to_string()), 1).with_settings(
        VaultSettings {
            link_format: LinkFormat::Relative,
            ..VaultSettings::obsidian(None)
        },
    );
    let f = ix.file("A/Sub/Note.md").unwrap();
    assert_eq!(ix.link_for(f, Some("A/Here.md"), true), "[[./Sub/Note]]");
    let topic = ix.file("Topic].md").unwrap();
    assert_eq!(
        ix.link_for(topic, Some("A/Here.md"), true),
        "[Topic\\]](../Topic%5D.md)"
    );
}

/// Review round 3: a racing process swapping a folder of the note's path for a symbolic link
/// to the outside never gets a file written outside the vault. Iterations: 1000 by default,
/// `MARKISE_STRESS_N` for longer runs.
#[cfg(unix)]
#[test]
fn creation_races_with_symlink_swaps_never_write_outside() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let n: usize = std::env::var("MARKISE_STRESS_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let outer = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(outer.path()).unwrap();
    let root = base.join("vault");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let (stop, a, real, outside) = (stop.clone(), root.join("A"), root.join("A-real"), outside.clone());
        std::thread::spawn(move || {
            std::fs::create_dir_all(&real).unwrap();
            while !stop.load(Ordering::Relaxed) {
                // a real folder …
                let _ = std::fs::rename(&real, &a);
                // … replaced by a link to the outside
                let _ = std::fs::rename(&a, &real);
                let _ = std::os::unix::fs::symlink(&outside, &a);
                let _ = std::fs::remove_file(&a);
            }
        })
    };
    let mut created = 0;
    for i in 0..n {
        if create_note(&root, &format!("A/note {i}.md")).is_ok() {
            created += 1;
        }
        if create_note(&root, &format!("A/sub/deep {i}.md")).is_ok() {
            created += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();
    let leaked: Vec<_> = walk_files(&outside);
    assert!(
        leaked.is_empty(),
        "{} files outside after {created} creations: {leaked:?}",
        leaked.len()
    );
}

#[cfg(unix)]
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}
