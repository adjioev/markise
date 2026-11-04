//! Builds archives by hand, so that a test can write what a careful tool never would.

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;

/// One tar entry: path, type flag, content, link target.
pub struct Raw<'a> {
    pub path: &'a str,
    pub flag: u8,
    pub content: &'a [u8],
    pub link: &'a str,
}

pub fn file<'a>(path: &'a str, content: &'a [u8]) -> Raw<'a> {
    Raw {
        path,
        flag: b'0',
        content,
        link: "",
    }
}

pub fn folder(path: &str) -> Raw<'_> {
    Raw {
        path,
        flag: b'5',
        content: b"",
        link: "",
    }
}

pub fn link<'a>(path: &'a str, flag: u8, target: &'a str) -> Raw<'a> {
    Raw {
        path,
        flag,
        content: b"",
        link: target,
    }
}

fn octal(field: &mut [u8], value: u64) {
    let text = format!("{value:0width$o}\0", width = field.len() - 1);
    field.copy_from_slice(text.as_bytes());
}

fn header(path: &str, flag: u8, size: u64, link: &str) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[..path.len()].copy_from_slice(path.as_bytes());
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0);
    h[156] = flag;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[148..156].fill(b' ');
    let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
    let text = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(text.as_bytes());
    h
}

/// Raw entries as an uncompressed tar.
pub fn tar(entries: &[Raw<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        out.extend_from_slice(&header(e.path, e.flag, e.content.len() as u64, e.link));
        out.extend_from_slice(e.content);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

pub fn targz(entries: &[Raw<'_>]) -> Vec<u8> {
    gzip(&tar(entries))
}

/// The neutral sample package as an archive, laid out as a typical tool writes it.
pub fn sample_archive() -> Vec<u8> {
    let files = crate::common::files();
    let mut entries = vec![folder("./"), folder("./editor/")];
    let paths: Vec<String> = files.keys().map(|p| format!("./{p}")).collect();
    for (path, content) in paths.iter().zip(files.values()) {
        entries.push(file(path, content));
    }
    targz(&entries)
}
