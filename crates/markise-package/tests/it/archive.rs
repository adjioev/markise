use markise_package::limits;
use markise_package::{clean_relative, read_archive};

use crate::fixture_archive::{file, folder, gzip, link, sample_archive, tar, targz};

#[test]
fn reads_a_well_formed_archive() {
    let files = read_archive(&sample_archive()).unwrap();
    assert_eq!(files, crate::common::files(), "paths come out without ./");
}

#[test]
fn unsafe_paths_refuse_the_whole_archive() {
    for path in [
        "../up.txt",
        "a/../../up.txt",
        "/etc/passwd",
        "a\\b.txt",
        "C:/x.txt",
        "a/NUL.txt",
        "a/com1",
        "name.",
        "~/x",
    ] {
        let archive = targz(&[file("ok.txt", b"x"), file(path, b"y")]);
        let e = read_archive(&archive).unwrap_err();
        assert!(e.contains(path) || e.contains("name"), "{path}: {e}");
    }
    assert!(read_archive(&targz(&[folder("../up/")])).is_err());
}

#[test]
fn links_and_devices_refuse_the_whole_archive() {
    for flag in *b"12" {
        let archive = targz(&[link("evil", flag, "/etc/passwd")]);
        let e = read_archive(&archive).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
    }
    for flag in *b"346" {
        assert!(read_archive(&targz(&[link("dev", flag, "")])).is_err(), "{flag}");
    }
}

#[test]
fn duplicate_paths_refuse_also_by_case() {
    let e = read_archive(&targz(&[file("a.txt", b"1"), file("a.txt", b"2")])).unwrap_err();
    assert!(e.contains("twice"), "{e}");
    let e = read_archive(&targz(&[file("a.txt", b"1"), file("A.TXT", b"2")])).unwrap_err();
    assert!(e.contains("twice"), "{e}");
    let e = read_archive(&targz(&[file("./a.txt", b"1"), file("a.txt", b"2")])).unwrap_err();
    assert!(e.contains("twice"), "{e}");
}

#[test]
fn compression_bombs_stop_at_the_limits() {
    let big = vec![0u8; (limits::MAX_FILE_BYTES + 1) as usize];
    let e = read_archive(&targz(&[file("big.bin", &big)])).unwrap_err();
    assert!(e.contains("larger than"), "{e}");

    // many files, each allowed, together too much
    let part = vec![0u8; (limits::MAX_FILE_BYTES / 2) as usize];
    let names: Vec<String> = (0..5).map(|i| format!("f{i}.bin")).collect();
    let entries: Vec<_> = names.iter().map(|n| file(n, &part)).collect();
    let e = read_archive(&targz(&entries)).unwrap_err();
    assert!(e.contains("unpacks to more than"), "{e}");
}

#[test]
fn too_many_entries_refuse() {
    let names: Vec<String> = (0..=limits::MAX_FILES).map(|i| format!("{i}/")).collect();
    let entries: Vec<_> = names.iter().map(|n| folder(n)).collect();
    let e = read_archive(&targz(&entries)).unwrap_err();
    assert!(e.contains("entries"), "{e}");
}

#[test]
fn a_size_field_cannot_lie() {
    // the header states more than the archive holds
    let mut raw = tar(&[file("a.txt", b"hello")]);
    raw.truncate(512 + 2);
    assert!(read_archive(&gzip(&raw)).is_err());
}

#[test]
fn garbage_is_an_error() {
    assert!(read_archive(b"this is not gzip").is_err());
    assert!(read_archive(&gzip(b"this is not tar, not at all")).is_err());
}

#[test]
fn clean_relative_rules() {
    assert_eq!(clean_relative("./a/./b.txt").unwrap(), "a/b.txt");
    assert_eq!(clean_relative("blocks/x-1.0.tar.gz").unwrap(), "blocks/x-1.0.tar.gz");
    assert_eq!(clean_relative(".").unwrap(), "");
    assert_eq!(clean_relative("имя/файл.css").unwrap(), "имя/файл.css");
    for bad in ["../a", "a/..", "/a", "a\\b", "c:x", "a/AUX", "a\0b", "x "] {
        assert!(clean_relative(bad).is_err(), "{bad}");
    }
    let deep = vec!["d"; 40].join("/");
    assert!(clean_relative(&deep).is_err());
}
