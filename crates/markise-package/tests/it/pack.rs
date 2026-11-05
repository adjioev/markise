use std::fs;

use markise_package::{PackageIndex, pack_folder, read_archive};

use crate::common;

fn folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in common::files() {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    dir
}

#[test]
fn a_folder_becomes_an_archive_the_reader_takes() {
    let dir = folder();
    let packed = pack_folder(dir.path()).unwrap();
    assert_eq!(packed.id.library, "sample");
    assert_eq!(packed.id.version, "1.2.0");
    let files = read_archive(&packed.bytes).unwrap();
    assert_eq!(files, common::files());
    assert!(PackageIndex::from_files(&files).is_ok());
}

#[test]
fn the_same_files_give_the_same_bytes() {
    let (a, b) = (folder(), folder());
    fs::write(a.path().join(".DS_Store"), "left by a file manager").unwrap();
    assert_eq!(
        pack_folder(a.path()).unwrap().sha256,
        pack_folder(b.path()).unwrap().sha256
    );
}

#[test]
fn an_invalid_package_is_no_archive() {
    let dir = folder();
    fs::write(dir.path().join("manifest.json"), "{}").unwrap();
    assert!(pack_folder(dir.path()).is_err());
}

#[cfg(unix)]
#[test]
fn a_link_is_refused() {
    let dir = folder();
    std::os::unix::fs::symlink("/etc/hosts", dir.path().join("styles.css")).unwrap();
    let why = pack_folder(dir.path()).unwrap_err();
    assert!(why.contains("not a regular file"), "{why}");
}
