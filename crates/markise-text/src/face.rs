//! `@font-face` loading (ADR 0005 §4.1 `face.rs`): the `src` list is tried in order and the
//! first source that loads wins. `local()` goes through the adapter-owned [`FaceNameIndex`],
//! files are confined to a root (TI-9) and size-capped, WOFF/WOFF2 are unwrapped with `wuff`.
//! Remote URLs are never fetched (the theme model does not even pass them here).

use std::collections::BTreeMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skrifa::raw::{FileRef, TableProvider};
use skrifa::{FontRef, Tag};

use crate::matching::FaceDescriptors;
use crate::names::FaceNameIndex;

/// One `@font-face` rule, already parsed (the theme importer maps its `FontFace` onto this).
#[derive(Clone, Debug, PartialEq)]
pub struct FaceRule {
    pub family: String,
    pub sources: Vec<FaceSource>,
    pub descriptors: FaceDescriptors,
    /// Inclusive code point ranges; empty = all code points.
    pub unicode_range: Vec<RangeInclusive<u32>>,
}

impl FaceRule {
    pub fn new(family: &str, sources: Vec<FaceSource>) -> Self {
        FaceRule {
            family: family.to_owned(),
            sources,
            descriptors: FaceDescriptors::default(),
            unicode_range: Vec::new(),
        }
    }

    pub fn weight(mut self, lo: f32, hi: f32) -> Self {
        self.descriptors.weight = (lo, hi);
        self
    }

    /// `font-stretch` descriptor range in percent.
    pub fn stretch(mut self, lo: f32, hi: f32) -> Self {
        self.descriptors.stretch = (lo, hi);
        self
    }

    /// Normalised copy as the store uses it: descriptors per
    /// [`FaceDescriptors::normalized`], `unicode-range` entries with reversed bounds swapped and
    /// clamped to U+10FFFF.
    pub fn normalized(&self) -> FaceRule {
        let mut r = self.clone();
        r.descriptors = r.descriptors.normalized();
        r.unicode_range = r
            .unicode_range
            .iter()
            .map(|x| {
                let (a, b) = (*x.start().min(&0x10FFFF), *x.end().min(&0x10FFFF));
                a.min(b)..=a.max(b)
            })
            .collect();
        r
    }

    pub fn slope(mut self, s: crate::matching::FaceSlope) -> Self {
        self.descriptors.slope = s;
        self
    }

    pub fn unicode_range(mut self, r: impl IntoIterator<Item = RangeInclusive<u32>>) -> Self {
        self.unicode_range = r.into_iter().collect();
        self
    }

    /// Does the rule's `unicode-range` include `c`?
    pub fn covers(&self, c: char) -> bool {
        self.unicode_range.is_empty() || self.unicode_range.iter().any(|r| r.contains(&(c as u32)))
    }
}

/// One entry of a `src` list.
#[derive(Clone, PartialEq)]
pub enum FaceSource {
    /// `local("Full Name")` or `local(PostScriptName)`.
    Local(String),
    /// A local file (ttf/otf/ttc/woff/woff2), optionally a face index in a collection.
    File(PathBuf),
    /// Font bytes supplied by the caller (wasm, tests, `data:` URLs later).
    Bytes(Arc<Vec<u8>>),
}

impl fmt::Debug for FaceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FaceSource::Local(n) => write!(f, "local({n:?})"),
            FaceSource::File(p) => write!(f, "file({})", p.display()),
            FaceSource::Bytes(b) => write!(f, "bytes({} B)", b.len()),
        }
    }
}

/// Where `File` sources may come from and how large they may be.
#[derive(Clone, Debug)]
pub struct LoadPolicy {
    /// Files must resolve (after symlinks) inside this folder; `None` = no file sources.
    pub root: Option<PathBuf>,
    /// Maximum size of a font file and of a decoded WOFF/WOFF2 payload.
    pub max_bytes: usize,
}

impl Default for LoadPolicy {
    fn default() -> Self {
        LoadPolicy {
            root: None,
            max_bytes: 64 << 20,
        }
    }
}

/// Why one source failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// `local()` name not in the face-name index.
    LocalNotFound,
    /// The face-name index is still being built (retry on the next font generation).
    IndexPending,
    /// File outside the policy root, or no root configured.
    OutsideRoot,
    TooLarge,
    Io(String),
    /// WOFF/WOFF2 could not be decoded.
    Decode(String),
    /// Not a usable font (unparseable or missing required tables).
    Invalid(String),
}

/// Error for a whole rule: every source failed (with the per-source reasons in order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceError {
    pub failures: Vec<(String, SourceError)>,
}

impl fmt::Display for FaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no @font-face source loaded:")?;
        for (s, e) in &self.failures {
            write!(f, " {s}: {e:?};")?;
        }
        Ok(())
    }
}

impl std::error::Error for FaceError {}

/// Font bytes: owned (decoded WOFF, caller bytes) or memory-mapped.
pub(crate) enum FontBytes {
    Owned(Vec<u8>),
    Shared(Arc<Vec<u8>>),
    #[cfg(not(target_arch = "wasm32"))]
    Mapped(memmap2::Mmap),
}

impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        match self {
            FontBytes::Owned(v) => v,
            FontBytes::Shared(v) => v,
            #[cfg(not(target_arch = "wasm32"))]
            FontBytes::Mapped(m) => m,
        }
    }
}

/// A successfully loaded source.
pub(crate) struct Loaded {
    pub bytes: FontBytes,
    pub index: u32,
}

/// Required tables for a face we hand to the shaper.
pub(crate) fn validate(data: &[u8], index: u32) -> Result<(), SourceError> {
    let font = FontRef::from_index(data, index).map_err(|e| SourceError::Invalid(e.to_string()))?;
    for t in [b"cmap", b"head", b"hhea", b"hmtx", b"maxp"] {
        if font.data_for_tag(Tag::new(t)).is_none() {
            return Err(SourceError::Invalid(format!("missing {}", String::from_utf8_lossy(t))));
        }
    }
    let outlines = [b"glyf", b"CFF ", b"CFF2", b"sbix", b"CBDT", b"COLR"]
        .iter()
        .any(|t| font.data_for_tag(Tag::new(t)).is_some());
    if !outlines {
        return Err(SourceError::Invalid("no glyph data".into()));
    }
    font.charmap_ok()
}

trait CharmapOk {
    fn charmap_ok(&self) -> Result<(), SourceError>;
}

impl CharmapOk for FontRef<'_> {
    fn charmap_ok(&self) -> Result<(), SourceError> {
        self.cmap()
            .map(|_| ())
            .map_err(|e| SourceError::Invalid(format!("cmap: {e}")))
    }
}

fn be16_at(d: &[u8], o: usize) -> Option<usize> {
    d.get(o..o.checked_add(2)?)
        .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
}

fn be32_at(d: &[u8], o: usize) -> Option<usize> {
    d.get(o..o.checked_add(4)?)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn bad(msg: &str) -> SourceError {
    SourceError::Decode(msg.into())
}

/// Size of the sfnt a WOFF 1.0 file decodes to, from its table directory (checked arithmetic;
/// every table must lie inside the file and not claim to shrink when decompressed). Fails with
/// `TooLarge` above `max_bytes` — before anything is allocated.
pub(crate) fn woff1_decoded_size(d: &[u8], max_bytes: usize) -> Result<usize, SourceError> {
    let n = be16_at(d, 12).ok_or_else(|| bad("short WOFF header"))?;
    if n == 0 || n > 512 {
        return Err(bad("WOFF table count"));
    }
    let mut total: usize = 12 + 16 * n;
    for i in 0..n {
        let e = 44 + 20 * i;
        let off = be32_at(d, e + 4).ok_or_else(|| bad("short WOFF directory"))?;
        let comp = be32_at(d, e + 8).ok_or_else(|| bad("short WOFF directory"))?;
        let orig = be32_at(d, e + 12).ok_or_else(|| bad("short WOFF directory"))?;
        if comp > orig || off.checked_add(comp).is_none_or(|end| end > d.len()) {
            return Err(bad("WOFF table outside the file"));
        }
        total = total
            .checked_add(orig.checked_add(3).ok_or(SourceError::TooLarge)? & !3)
            .ok_or(SourceError::TooLarge)?;
        if total > max_bytes {
            return Err(SourceError::TooLarge);
        }
    }
    Ok(total)
}

fn base128(d: &[u8], pos: &mut usize) -> Option<usize> {
    let mut v: u32 = 0;
    for i in 0..5 {
        let b = *d.get(*pos)?;
        *pos += 1;
        if i == 0 && b == 0x80 {
            return None; // leading zeros
        }
        if v & 0xFE00_0000 != 0 {
            return None; // overflow
        }
        v = (v << 7) | u32::from(b & 0x7F);
        if b & 0x80 == 0 {
            return Some(v as usize);
        }
    }
    None
}

/// (decoded sfnt size bound, decompressed stream size) of a WOFF2 file from its table directory
/// (`origLength` of every table, `transformLength` where present), checked against `max_bytes`.
pub(crate) fn woff2_sizes(d: &[u8], max_bytes: usize) -> Result<(usize, usize), SourceError> {
    let n = be16_at(d, 12).ok_or_else(|| bad("short WOFF2 header"))?;
    if n == 0 || n > 512 {
        return Err(bad("WOFF2 table count"));
    }
    let mut pos = 48;
    let mut total: usize = 12 + 16 * n;
    let mut stream: usize = 0;
    for _ in 0..n {
        let flags = *d.get(pos).ok_or_else(|| bad("short WOFF2 directory"))?;
        pos += 1;
        let idx = flags & 63;
        if idx == 63 {
            pos += 4;
        }
        let version = flags >> 6;
        let orig = base128(d, &mut pos).ok_or_else(|| bad("WOFF2 origLength"))?;
        let transformed = if idx == 10 || idx == 11 {
            version == 0
        } else {
            version != 0
        };
        let len = if transformed {
            base128(d, &mut pos).ok_or_else(|| bad("WOFF2 transformLength"))?
        } else {
            orig
        };
        total = total
            .checked_add(orig.checked_add(3).ok_or(SourceError::TooLarge)? & !3)
            .ok_or(SourceError::TooLarge)?;
        stream = stream.checked_add(len).ok_or(SourceError::TooLarge)?;
        if total > max_bytes || stream > max_bytes {
            return Err(SourceError::TooLarge);
        }
    }
    Ok((total, stream))
}

/// Reads at most `limit` bytes from a decompressor; more output is an error (bomb guard).
fn bounded_read(r: impl std::io::Read, expected: usize, limit: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if expected > limit {
        return Err("decompressed size above the limit".into());
    }
    let mut out = Vec::with_capacity(expected);
    std::io::Read::read_to_end(&mut std::io::Read::take(r, limit as u64 + 1), &mut out)?;
    if out.len() > limit || out.len() != expected {
        return Err("decompressed size differs from the directory".into());
    }
    Ok(out)
}

/// Unwraps WOFF/WOFF2 (magic-sniffed); sfnt/collections pass through. `max_bytes` bounds the
/// output: directory-derived sizes are checked (checked arithmetic) before decoding, every
/// decompression is limited while it runs (cumulative for WOFF 1.0 tables), and the final
/// size is checked again.
pub(crate) fn decode(raw: FontBytes, max_bytes: usize) -> Result<FontBytes, SourceError> {
    let data = raw.as_ref();
    let out = match data.get(0..4) {
        Some(b"wOFF") => {
            let bound = woff1_decoded_size(data, max_bytes)?;
            let mut used = 0usize;
            let mut z = |src: &[u8], expected: usize| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
                let left = bound.saturating_sub(used);
                let v = bounded_read(flate2::read::ZlibDecoder::new(src), expected, left)?;
                used += v.len();
                Ok(v)
            };
            wuff::decompress_woff1_with_custom_z(data, &mut z).map_err(|e| SourceError::Decode(format!("{e:?}")))?
        }
        Some(b"wOF2") => {
            let (_, stream) = woff2_sizes(data, max_bytes)?;
            let mut br = |src: &[u8], expected: usize| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
                bounded_read(brotli_decompressor::Decompressor::new(src, 4096), expected, stream)
            };
            wuff::decompress_woff2_with_custom_brotli(data, &mut br)
                .map_err(|e| SourceError::Decode(format!("{e:?}")))?
        }
        _ => return Ok(raw),
    };
    if out.len() > max_bytes {
        return Err(SourceError::TooLarge);
    }
    Ok(FontBytes::Owned(out))
}

/// Number of faces in an sfnt or collection.
pub(crate) fn face_count(data: &[u8]) -> u32 {
    match FileRef::new(data) {
        Ok(FileRef::Font(_)) => 1,
        Ok(FileRef::Collection(c)) => c.len(),
        Err(_) => 0,
    }
}

fn confined(path: &Path, policy: &LoadPolicy) -> Result<PathBuf, SourceError> {
    let root = policy.root.as_ref().ok_or(SourceError::OutsideRoot)?;
    let root = root.canonicalize().map_err(|e| SourceError::Io(e.to_string()))?;
    let p = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let p = p.canonicalize().map_err(|e| SourceError::Io(e.to_string()))?;
    if !p.starts_with(&root) {
        return Err(SourceError::OutsideRoot);
    }
    Ok(p)
}

/// The OS-protected system font folder of this platform (same policy as ADR 0004 /
/// `markise_ui_egui::fonts`): read-only to user processes (macOS SIP volume,
/// `%WINDIR%\Fonts`, root-owned `/usr/share/fonts`).
fn protected_root() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/System/Library/Fonts"))
    } else if cfg!(windows) {
        Some(PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts"))
    } else {
        Some(PathBuf::from("/usr/share/fonts"))
    }
}

/// Is `path` inside the protected folder with no symbolic link in the folder or below it?
fn is_protected(path: &Path) -> bool {
    let Some(root) = protected_root() else {
        return false;
    };
    let Ok(rel) = path.strip_prefix(&root) else {
        return false;
    };
    let link = |p: &Path| std::fs::symlink_metadata(p).map_or(true, |m| m.file_type().is_symlink());
    if link(&root) {
        return false;
    }
    let mut p = root.clone();
    for c in rel.components() {
        let std::path::Component::Normal(c) = c else {
            return false;
        };
        p.push(c);
        if link(&p) {
            return false;
        }
    }
    true
}

/// Opens a font file. Files in the OS-protected system font folder are memory-mapped (under the
/// ADR 0004 threat model — accidents, not malicious local processes — user programs cannot
/// truncate them, so the mapping cannot fault). Every other file (theme folders, user font
/// folders) is read into an owned snapshot from the opened handle, size-checked on the handle
/// and bounded while reading, so a file edited or truncated later cannot affect us.
pub(crate) fn open_file(path: &Path, max_bytes: usize) -> Result<FontBytes, SourceError> {
    use std::io::Read;
    // regular files only, opened without blocking (a FIFO named `.ttf` must not hang)
    let (f, len) = markise_doc::resource::open_regular(path).map_err(|e| SourceError::Io(e.to_string()))?;
    if len > max_bytes as u64 {
        return Err(SourceError::TooLarge);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if is_protected(path) {
        // SAFETY: protected system fonts are not modified by user processes (ADR 0004 threat
        // model); see `is_protected` for the folder and symlink checks.
        let m = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| SourceError::Io(e.to_string()))?;
        if m.len() > max_bytes {
            return Err(SourceError::TooLarge);
        }
        return Ok(FontBytes::Mapped(m));
    }
    let mut v = Vec::with_capacity(len as usize);
    f.take(max_bytes as u64 + 1)
        .read_to_end(&mut v)
        .map_err(|e| SourceError::Io(e.to_string()))?;
    if v.len() > max_bytes {
        return Err(SourceError::TooLarge);
    }
    Ok(FontBytes::Owned(v))
}

/// Tries the sources of `rule` in order.
pub(crate) fn load_rule(
    rule: &FaceRule,
    policy: &LoadPolicy,
    names: Option<&FaceNameIndex>,
) -> Result<Loaded, FaceError> {
    let mut failures = Vec::new();
    for src in &rule.sources {
        let r = (|| -> Result<Loaded, SourceError> {
            let (raw, index) = match src {
                FaceSource::Local(name) => {
                    let idx = names.ok_or(SourceError::IndexPending)?;
                    let loc = idx.find(name).ok_or(SourceError::LocalNotFound)?;
                    (open_file(&loc.path, policy.max_bytes)?, loc.index)
                }
                FaceSource::File(p) => {
                    let p = confined(p, policy)?;
                    (open_file(&p, policy.max_bytes)?, 0)
                }
                FaceSource::Bytes(b) => {
                    if b.len() > policy.max_bytes {
                        return Err(SourceError::TooLarge);
                    }
                    (FontBytes::Shared(b.clone()), 0)
                }
            };
            let bytes = decode(raw, policy.max_bytes)?;
            validate(bytes.as_ref(), index)?;
            Ok(Loaded { bytes, index })
        })();
        match r {
            Ok(l) => return Ok(l),
            Err(e) => failures.push((format!("{src:?}"), e)),
        }
    }
    Err(FaceError { failures })
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend(x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend(x.to_be_bytes());
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(b));
    }
    sum
}

/// Assembles tables (sorted by tag) into an sfnt (also used to isolate one face of a
/// collection, see `FontStore::add_face`).
pub fn sfnt(tables: &BTreeMap<[u8; 4], Vec<u8>>) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut es = 0u16;
    while (1u16 << (es + 1)) <= n {
        es += 1;
    }
    let sr = (1u16 << es) * 16;
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, n);
    be16(&mut out, sr);
    be16(&mut out, es);
    be16(&mut out, n * 16 - sr);
    let mut offset = 12 + 16 * n as u32;
    let mut data = Vec::new();
    for (tag, t) in tables {
        out.extend(tag);
        be32(&mut out, checksum(t));
        be32(&mut out, offset);
        be32(&mut out, t.len() as u32);
        data.extend(t);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        offset = 12 + 16 * n as u32 + data.len() as u32;
    }
    out.extend(data);
    out
}
