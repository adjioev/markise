use std::fs;
use std::path::{Path, PathBuf};

use markise_package::{
    BINDING_FILE, InstalledPackage, Pending, Problem, Resolution, Sha256Hex, Store, accept, resolve,
};

use crate::fixture_archive::{file, sample_archive, targz};

struct Site {
    _dir: tempfile::TempDir,
    folder: PathBuf,
    doc: PathBuf,
    store: Store,
}

impl Site {
    fn new() -> Site {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("content");
        fs::create_dir_all(folder.join("blocks")).unwrap();
        let store = Store::in_config(&dir.path().join("config"));
        let doc = folder.join("doc.md");
        fs::write(&doc, "# doc\n").unwrap();
        Site {
            folder,
            doc,
            store,
            _dir: dir,
        }
    }

    fn bind(&self, archive: &[u8]) -> Sha256Hex {
        fs::write(self.folder.join("blocks/p.tar.gz"), archive).unwrap();
        let sha = Sha256Hex::of(archive);
        self.bind_raw("sample", "1.2.0", "blocks/p.tar.gz", sha.as_str());
        sha
    }

    fn bind_raw(&self, library: &str, version: &str, source: &str, sha: &str) {
        fs::write(
            self.folder.join(BINDING_FILE),
            format!(r#"{{"library":"{library}","version":"{version}","source":"{source}","sha256":"{sha}"}}"#),
        )
        .unwrap();
    }

    fn resolve(&self) -> Resolution {
        resolve(&self.doc, &self.store, Some(self.folder.parent().unwrap()))
    }

    fn pending(&self) -> Pending {
        match self.resolve() {
            Resolution::Ask(p) => p,
            other => panic!("expected a question, got {other:?}"),
        }
    }

    fn ready(&self) -> InstalledPackage {
        match self.resolve() {
            Resolution::Ready(p) => p,
            other => panic!("expected a ready package, got {other:?}"),
        }
    }

    fn problem(&self) -> Problem {
        match self.resolve() {
            Resolution::Failed { problem, .. } => problem,
            other => panic!("expected a failure, got {other:?}"),
        }
    }
}

/// The store's check, against the digest the consent record holds.
fn load(site: &Site, sha: &Sha256Hex) -> Result<Option<markise_package::Files>, String> {
    let digest = site.store.acceptance("sample", sha).unwrap().files_digest;
    site.store.load(sha, &digest)
}

/// The sample package with another version in its manifest.
fn archive_of_version(version: &str) -> Vec<u8> {
    let files = crate::common::files();
    let manifest = crate::common::MANIFEST.replace("1.2.0", version);
    let mut entries = vec![file("manifest.json", manifest.as_bytes())];
    entries.extend(
        files
            .iter()
            .filter(|(p, _)| *p != "manifest.json")
            .map(|(p, c)| file(p, c)),
    );
    targz(&entries)
}

#[test]
fn no_binding_touches_nothing() {
    let site = Site::new();
    assert!(matches!(site.resolve(), Resolution::Unbound));
    assert!(!site.store.root().exists(), "nothing is created");
}

#[test]
fn asking_writes_nothing_and_yes_installs() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());

    let pending = site.pending();
    assert_eq!(pending.id.library, "sample");
    assert_eq!(pending.sha256, sha);
    assert_eq!(pending.block_count, 3);
    assert_eq!(pending.previous, None);
    assert!(!site.store.root().exists(), "no consent, nothing written");

    let installed = accept(&pending, &site.store).unwrap();
    assert_eq!(installed.root, site.store.files_dir(&sha));
    assert!(installed.root.join("components/poll/poll.js").is_file());
    assert!(site.store.is_accepted("sample", &sha));
    assert_eq!(installed.index.block_count(), 3);

    let again = site.ready();
    assert_eq!(again.sha256, sha);
    assert!(again.index.block("tip").is_some());
    assert!(again.asset_path("components/tip/tip.css").unwrap().is_file());
    assert!(again.asset_path("../../trust.json").is_err());
    assert!(again.asset_path("/etc/hosts").is_err());
    assert!(again.asset_path(".").is_err());
}

#[test]
fn works_offline_once_accepted_and_reinstalls_from_source_when_needed() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();

    // the source may go away
    fs::remove_file(site.folder.join("blocks/p.tar.gz")).unwrap();
    site.ready();

    // losing both is an error that names the file
    fs::remove_dir_all(site.store.files_dir(&sha).parent().unwrap()).unwrap();
    assert!(matches!(site.problem(), Problem::Source(_)));

    // the source comes back: no second question, the consent still holds
    fs::write(site.folder.join("blocks/p.tar.gz"), sample_archive()).unwrap();
    assert_eq!(site.ready().sha256, sha);
}

#[test]
fn a_changed_hash_asks_again() {
    let site = Site::new();
    let first = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();

    let second = site.bind(&archive_of_version("1.3.0").clone());
    site.bind_raw("sample", "1.3.0", "blocks/p.tar.gz", second.as_str());
    let pending = site.pending();
    assert_ne!(first, second);
    assert_eq!(pending.id.version, "1.3.0");
    assert_eq!(pending.previous.as_ref().unwrap().version, "1.2.0");

    // the old one stays accepted, for folders still bound to it
    assert!(site.store.is_accepted("sample", &first));
    assert!(!site.store.is_accepted("sample", &second));
}

#[test]
fn consent_follows_the_hash_across_folders() {
    let site = Site::new();
    site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    let other = site.folder.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::copy(site.folder.join(BINDING_FILE), other.join(BINDING_FILE)).unwrap();
    let out = resolve(&other.join("doc.md"), &site.store, None);
    assert!(matches!(out, Resolution::Ready(_)), "{out:?}");
}

#[test]
fn a_tampered_archive_is_refused_with_both_hashes() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    let mut bytes = sample_archive();
    bytes.push(0);
    fs::write(site.folder.join("blocks/p.tar.gz"), &bytes).unwrap();
    match site.problem() {
        Problem::HashMismatch { expected, found } => {
            assert_eq!(expected, sha);
            assert_eq!(found, Sha256Hex::of(&bytes));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_archive_changed_after_the_question_is_not_installed() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    let pending = site.pending();
    fs::write(site.folder.join("blocks/p.tar.gz"), archive_of_version("9.9.9")).unwrap();
    assert!(matches!(
        accept(&pending, &site.store),
        Err(Problem::HashMismatch { .. })
    ));
    assert!(!site.store.is_accepted("sample", &sha));
    assert!(!site.store.files_dir(&sha).exists());
}

#[test]
fn binding_and_package_must_agree() {
    let site = Site::new();
    let archive = sample_archive();
    let sha = site.bind(&archive);
    site.bind_raw("other", "1.2.0", "blocks/p.tar.gz", sha.as_str());
    assert!(matches!(site.problem(), Problem::IdentityMismatch { .. }));
    site.bind_raw("sample", "2.0.0", "blocks/p.tar.gz", sha.as_str());
    assert!(matches!(site.problem(), Problem::IdentityMismatch { .. }));
}

#[test]
fn unsafe_and_invalid_packages_are_refused_before_asking() {
    let site = Site::new();
    site.bind(&targz(&[file("../up.txt", b"x")]));
    assert!(matches!(site.problem(), Problem::Archive(_)));
    site.bind(&targz(&[file("readme.txt", b"x")]));
    assert!(matches!(site.problem(), Problem::Package(_)));
    assert!(!site.store.root().exists());
}

#[test]
fn a_future_format_is_refused_cleanly() {
    let site = Site::new();
    let manifest = r#"{"format":"markise-block-package/99"}"#;
    site.bind(&targz(&[file("manifest.json", manifest.as_bytes())]));
    assert!(matches!(
        site.problem(),
        Problem::Package(markise_package::PackageError::UnsupportedFormat { .. })
    ));
}

#[test]
fn a_missing_source_is_named() {
    let site = Site::new();
    site.bind_raw("sample", "1.2.0", "blocks/none.tar.gz", &"a".repeat(64));
    assert!(matches!(site.problem(), Problem::Source(_)));
}

#[cfg(unix)]
#[test]
fn a_source_link_may_not_leave_the_folder() {
    use std::os::unix::fs::symlink;
    let site = Site::new();
    let archive = sample_archive();
    let sha = Sha256Hex::of(&archive);
    let outside = site.folder.parent().unwrap().join("outside.tar.gz");
    fs::write(&outside, &archive).unwrap();
    symlink(&outside, site.folder.join("blocks/link.tar.gz")).unwrap();
    site.bind_raw("sample", "1.2.0", "blocks/link.tar.gz", sha.as_str());
    match site.problem() {
        Problem::Source(e) => assert!(e.contains("outside"), "{e}"),
        other => panic!("{other:?}"),
    }

    // a folder link out is the same
    fs::create_dir_all(site.folder.parent().unwrap().join("away")).unwrap();
    fs::write(site.folder.parent().unwrap().join("away/p.tar.gz"), &archive).unwrap();
    symlink(site.folder.parent().unwrap().join("away"), site.folder.join("via")).unwrap();
    site.bind_raw("sample", "1.2.0", "via/p.tar.gz", sha.as_str());
    assert!(matches!(site.problem(), Problem::Source(_)));

    // a link that stays inside is fine
    fs::write(site.folder.join("blocks/real.tar.gz"), &archive).unwrap();
    symlink("real.tar.gz", site.folder.join("blocks/inner.tar.gz")).unwrap();
    site.bind_raw("sample", "1.2.0", "blocks/inner.tar.gz", sha.as_str());
    site.pending();
}

#[test]
fn a_damaged_cache_is_replaced_from_the_source() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    let js = site.store.files_dir(&sha).join("components/poll/poll.js");

    fs::write(&js, "changed").unwrap();
    assert!(load(&site, &sha).unwrap_err().contains("not the ones"));
    site.ready();
    assert_eq!(
        fs::read(&js).unwrap(),
        crate::common::files()["components/poll/poll.js"]
    );

    fs::write(site.store.files_dir(&sha).join("added.js"), "x").unwrap();
    assert!(load(&site, &sha).is_err());
    site.ready();
    assert!(!site.store.files_dir(&sha).join("added.js").exists());
    assert!(load(&site, &sha).unwrap().is_some());

    // damaged and no source: refused, not used
    fs::write(&js, "changed").unwrap();
    fs::remove_file(site.folder.join("blocks/p.tar.gz")).unwrap();
    assert!(matches!(site.problem(), Problem::Source(_)));
}

#[cfg(unix)]
#[test]
fn a_link_in_the_cache_counts_as_damage() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    let js = site.store.files_dir(&sha).join("components/poll/poll.js");
    fs::remove_file(&js).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", &js).unwrap();
    assert!(load(&site, &sha).unwrap_err().contains("not a regular file"));
}

#[test]
fn damaged_trust_record_grants_nothing_and_can_be_replaced() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    let pending = site.pending();
    accept(&pending, &site.store).unwrap();
    let trust = site.store.root().join("trust.json");
    fs::write(&trust, "{ not json").unwrap();
    site.pending();
    accept(&pending, &site.store).unwrap();
    assert!(site.store.is_accepted("sample", &sha));
}

#[test]
fn a_newer_trust_record_is_not_overwritten() {
    let site = Site::new();
    site.bind(&sample_archive());
    let pending = site.pending();
    fs::create_dir_all(site.store.root()).unwrap();
    let trust = site.store.root().join("trust.json");
    let newer = r#"{"version":99,"accepted":[]}"#;
    fs::write(&trust, newer).unwrap();
    assert!(matches!(accept(&pending, &site.store), Err(Problem::Store(_))));
    assert_eq!(fs::read_to_string(&trust).unwrap(), newer);
}

#[test]
fn forget_withdraws_consent_and_removes_files() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    site.store.forget("sample", &sha).unwrap();
    assert!(!site.store.is_accepted("sample", &sha));
    assert!(!site.store.files_dir(&sha).exists());
    site.pending();
    assert!(!Path::new(&site.store.files_dir(&sha)).exists());
}

#[test]
fn install_refuses_unclean_paths() {
    // the store checks paths itself, whatever its caller hands it
    let site = Site::new();
    let mut files = markise_package::Files::new();
    files.insert("../escape.txt".to_string(), b"x".to_vec());
    let sha = Sha256Hex::parse(&"a".repeat(64)).unwrap();
    assert!(site.store.install(&sha, &files).is_err());
    assert!(!site.store.root().join("packages/escape.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_linked_files_root_is_damage() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    let files = site.store.files_dir(&sha);
    let away = site.folder.parent().unwrap().join("away");
    fs::rename(&files, &away).unwrap();
    std::os::unix::fs::symlink(&away, &files).unwrap();
    assert!(load(&site, &sha).unwrap_err().contains("not a real folder"));

    // resolving replaces the link and leaves what it pointed to alone
    let ready = site.ready();
    assert!(!fs::symlink_metadata(&ready.root).unwrap().is_symlink());
    assert!(away.join("manifest.json").is_file());
}

#[test]
fn hashes_cannot_name_other_folders() {
    for bad in [
        "../../victim",
        "..",
        "",
        "a/b",
        &"g".repeat(64),
        &"a".repeat(63),
        &"a".repeat(65),
    ] {
        assert!(Sha256Hex::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(Sha256Hex::parse(&"AB".repeat(32)).unwrap().as_str(), "ab".repeat(32));

    // a record naming a traversal is dropped, so it can reach no store operation
    let site = Site::new();
    fs::create_dir_all(site.store.root()).unwrap();
    let record = format!(
        r#"{{"version":1,"accepted":[{{"library":"sample","version":"1","sha256":"../../victim","files_digest":"{}"}}]}}"#,
        "a".repeat(64)
    );
    fs::write(site.store.root().join("trust.json"), record).unwrap();
    assert!(site.store.accepted().is_empty());
}

#[test]
fn a_cache_replaced_whole_does_not_inherit_the_consent() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    let js = site.store.files_dir(&sha).join("components/poll/poll.js");
    fs::write(&js, "replaced").unwrap();

    // from the source again, not served
    let ready = site.ready();
    assert_eq!(
        fs::read(ready.root.join("components/poll/poll.js")).unwrap(),
        crate::common::files()["components/poll/poll.js"]
    );

    // no source: an error, not the replacement
    fs::write(&js, "replaced").unwrap();
    fs::remove_file(site.folder.join("blocks/p.tar.gz")).unwrap();
    assert!(matches!(site.problem(), Problem::Source(_)));
}

#[test]
fn a_record_without_a_files_digest_grants_nothing() {
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    accept(&site.pending(), &site.store).unwrap();
    let record = format!(r#"{{"version":1,"accepted":[{{"library":"sample","version":"1.2.0","sha256":"{sha}"}}]}}"#);
    fs::write(site.store.root().join("trust.json"), record).unwrap();
    site.pending();
}

#[cfg(unix)]
#[test]
fn a_linked_packages_folder_stops_every_store_write() {
    use std::os::unix::fs::symlink;
    let site = Site::new();
    let sha = site.bind(&sample_archive());
    let pending = site.pending();

    let away = site.folder.parent().unwrap().join("away");
    let victim = away.join(sha.as_str());
    fs::create_dir_all(&victim).unwrap();
    fs::create_dir_all(site.store.root()).unwrap();
    symlink(&away, site.store.root().join("packages")).unwrap();

    let e = accept(&pending, &site.store).unwrap_err();
    assert!(
        matches!(&e, Problem::Store(m) if m.contains("not a real folder")),
        "{e:?}"
    );
    assert!(
        !site.store.is_accepted("sample", &sha),
        "no consent for a failed install"
    );
    assert!(site.store.remove_package(&sha).is_err());
    assert!(site.store.forget("sample", &sha).is_err());
    assert!(victim.is_dir(), "nothing outside was removed");
    assert_eq!(fs::read_dir(&away).unwrap().count(), 1, "nothing outside was written");
}

#[test]
fn an_archive_with_colliding_folders_never_reaches_the_store() {
    let site = Site::new();
    let mut entries: Vec<_> = crate::common::files().into_iter().collect();
    entries.push(("Assets/a.js".to_string(), b"a".to_vec()));
    entries.push(("assets/b.js".to_string(), b"b".to_vec()));
    let raw: Vec<_> = entries.iter().map(|(p, c)| file(p, c)).collect();
    site.bind(&targz(&raw));
    assert!(matches!(site.problem(), Problem::Archive(e) if e.contains("differ only by")));
    assert!(!site.store.root().exists());
}
