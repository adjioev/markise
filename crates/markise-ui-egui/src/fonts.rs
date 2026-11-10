//! Font setup: egui's bundled fonts plus, when present, system fonts for body text, bold, CJK
//! and symbol fallback, the bundled keyboard-symbol face ([`markise_text::keys`], issue #131),
//! and the font stacks of the active theme (`font-family` lists, spec 03 §2.1)
//! resolved to installed system fonts by family name.
//!
//! Fonts in OS-protected folders are memory-mapped (only the pages used become resident — a
//! 24 MB CJK collection costs almost nothing until CJK text appears); fonts in user-writable
//! folders are read into owned, size-bounded snapshots (a mapped file truncated by another
//! program would fault). Every face is validated with epaint's own parser (skrifa) before it
//! is handed to epaint. Each file is loaded at most once per process. The only bundled face of
//! our own is the 4 KB keyboard-symbol subset (OFL-1.1, `assets/fonts/keys/SOURCE.md`); it sits
//! after the text fonts and before egui's emoji fonts in every chain, so ⌘ ⇧ ⌥ ⌃ ⎋ ↩ ⌫ … never
//! render as boxes (body, headings, source view, menus and tooltips alike).
//!
//! Theme fonts: [`resolve`] looks the names of a theme's body / heading / monospace stacks up
//! in an index of the installed fonts ([`SystemFonts`], built once by scanning the platform
//! font folders and reading each face's name table; ~100 ms, so callers run it off the UI
//! thread) and takes up to three installed families per stack, in stack order, as fallbacks in
//! front of egui's defaults. Generic families (`serif`, `monospace`, …) map to per-platform
//! lists; `sans-serif` / `system-ui` mean the default body font. `@font-face` rules contribute
//! their `local("…")` names; their font **files are not loaded yet** (C3, the next milestone
//! with custom shaping), so such families fall back to the next installed name in the stack.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily};
use markise_layout::block::{FontSet, RoleFaces, register_families};
use markise_theme::{FontSource, Theme};

/// Candidate system font files of one platform.
struct Candidates {
    body: &'static [&'static str],
    bold: &'static [&'static str],
    cjk: &'static [&'static str],
    /// A broad symbol font (arrows, technical and miscellaneous symbols), loaded with the CJK
    /// fallback when a document needs glyphs the primary fonts lack.
    symbols: &'static [&'static str],
    /// The platform's colour emoji font: the last face of every chain, for the emoji no other face
    /// has (egui's bundled emoji font stops at an older Unicode version, so 🧪 drew as a box, issue #231).
    emoji: &'static [&'static str],
}

/// Candidate system fonts of this platform.
fn candidates() -> Candidates {
    if cfg!(target_os = "macos") {
        Candidates {
            body: &["/System/Library/Fonts/Supplemental/Arial.ttf"],
            bold: &["/System/Library/Fonts/Supplemental/Arial Bold.ttf"],
            cjk: &[
                "/System/Library/Fonts/Hiragino Sans GB.ttc",
                "/System/Library/Fonts/STHeiti Light.ttc",
            ],
            symbols: &["/System/Library/Fonts/Apple Symbols.ttf"],
            emoji: &["/System/Library/Fonts/Apple Color Emoji.ttc"],
        }
    } else if cfg!(target_os = "windows") {
        Candidates {
            body: &["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\arial.ttf"],
            bold: &["C:\\Windows\\Fonts\\segoeuib.ttf", "C:\\Windows\\Fonts\\arialbd.ttf"],
            cjk: &["C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\simsun.ttc"],
            symbols: &["C:\\Windows\\Fonts\\seguisym.ttf"],
            emoji: &["C:\\Windows\\Fonts\\seguiemj.ttf"],
        }
    } else {
        Candidates {
            body: &[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
            ],
            bold: &[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
                "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            ],
            cjk: &[
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            ],
            symbols: &[
                "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
                "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
                "/usr/share/fonts/google-noto/NotoSansSymbols2-Regular.ttf",
            ],
            emoji: &[
                "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
                "/usr/share/fonts/noto/NotoColorEmoji.ttf",
                "/usr/share/fonts/google-noto-emoji/NotoColorEmoji.ttf",
                "/usr/share/fonts/noto-emoji/NotoColorEmoji.ttf",
            ],
        }
    }
}

/// Font-data name of the bundled keyboard-symbol face ([`markise_text::keys`]).
pub const KEYS_FONT: &str = "markise-keys";

/// Inserts `name` into `list` in front of egui's last-resort fallbacks (icons, emoji): after
/// the fonts that cover the text, before the emoji fonts (which have their own, emoji-style
/// glyphs for some of the same code points, e.g. ↩).
fn insert_before_emoji(list: &mut Vec<String>, name: &str) {
    if list.iter().any(|n| n == name) {
        return;
    }
    let at = list
        .iter()
        .position(|n| matches!(n.as_str(), "egui-icons" | "NotoEmoji-Regular" | "emoji-icon-font"))
        .unwrap_or(list.len());
    list.insert(at, name.to_owned());
}

/// Largest font file read.
const MAX_FONT_BYTES: u64 = 64 << 20;

/// Largest font file mapped from the OS-protected folder: only the pages of glyphs in use become
/// resident, so a colour emoji collection of some 200 MB (macOS) costs what its used emoji take.
const MAX_MAPPED_FONT_BYTES: u64 = 512 << 20;

/// Bytes of font data retained by the process at most (all faces handed to epaint).
pub const FONT_BUDGET: usize = 128 << 20;

/// Reads a font file into an owned buffer (no memory mapping: a mapped file truncated by
/// another program would fault the process).
fn read_font(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 || len > MAX_FONT_BYTES {
        return None;
    }
    let mut v = Vec::with_capacity(len as usize);
    file.take(MAX_FONT_BYTES).read_to_end(&mut v).ok()?;
    Some(v)
}

/// Whether epaint can load face `index` of `bytes` and lay text out with it: skrifa (epaint's
/// parser) must read the font directory and the tables horizontal layout depends on — `head`
/// (sane units per em: epaint scales by `1 / units_per_em`), `hhea`, `maxp` and a complete
/// `hmtx` — and sampled glyphs must have advances (some positive). skrifa's metrics accessors fall
/// back to zeros for missing tables (a face with `head` but no `hhea` would render with zero
/// advances), so the tables are checked directly. Guarded against panics. Faces that fail are
/// not handed to epaint and are reported instead.
pub fn valid_face(bytes: &[u8], index: u32) -> bool {
    std::panic::catch_unwind(|| face_tables_ok(bytes, index).unwrap_or(false)).unwrap_or(false)
}

fn face_tables_ok(bytes: &[u8], index: u32) -> Option<bool> {
    use skrifa::MetadataProvider;
    use skrifa::instance::{LocationRef, Size};
    use skrifa::raw::TableProvider;
    let f = skrifa::FontRef::from_index(bytes, index).ok()?;
    let upem = f.head().ok()?.units_per_em();
    let long_metrics = f.hhea().ok()?.number_of_h_metrics();
    let glyphs = f.maxp().ok()?.num_glyphs();
    // `hmtx` is read with `hhea`'s metric count: a truncated table fails here
    let hmtx = f.hmtx().ok()?;
    if !(16..=16384).contains(&upem)
        || glyphs == 0
        || long_metrics == 0
        || long_metrics > glyphs
        || hmtx.h_metrics().len() != usize::from(long_metrics)
    {
        return Some(false);
    }
    // advances are retrievable, and positive for some of the first mapped visible characters
    // (the first ones may be zero-width marks); `.notdef` stands in when nothing is mapped
    let metrics = f.glyph_metrics(Size::unscaled(), LocationRef::default());
    let mut sample = f
        .charmap()
        .mappings()
        .filter(|&(c, _)| char::from_u32(c).is_some_and(|c| !c.is_whitespace() && !c.is_control()))
        .map(|(_, g)| g)
        .take(64)
        .peekable();
    if sample.peek().is_none() {
        return Some(metrics.advance_width(skrifa::GlyphId::new(0))? > 0.0);
    }
    let mut any_positive = false;
    for g in sample {
        any_positive |= metrics.advance_width(g)? > 0.0;
    }
    Some(any_positive)
}

struct Entry {
    data: Arc<FontData>,
    bytes: usize,
    tick: u64,
}

/// Process-wide store of loaded faces: owned snapshots shared with egui through `Arc`,
/// bounded by [`FONT_BUDGET`]; faces no longer referenced elsewhere (egui dropped them) are
/// evicted least-recently-used when room is needed.
#[derive(Default)]
struct Store {
    map: HashMap<(PathBuf, u32), Entry>,
    total: usize,
    tick: u64,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Bytes currently retained by the font store.
pub fn retained_bytes() -> usize {
    store().lock().map_or(0, |s| s.total)
}

/// Loads face `index` of `path` (validated before it is kept), or the reason it is not
/// usable.
fn load_face(path: &Path, index: u32) -> Result<Arc<FontData>, &'static str> {
    if let Some(bytes) = protected_map(path) {
        if !valid_face(bytes, index) {
            return Err("invalid font data");
        }
        let mut d = FontData::from_static(bytes);
        d.index = index;
        return Ok(Arc::new(d));
    }
    load_into(store(), FONT_BUDGET, path, index)
}

/// The OS-protected system font folder of this platform.
fn protected_root() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/System/Library/Fonts"))
    } else if cfg!(windows) {
        Some(PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts"))
    } else {
        Some(PathBuf::from("/usr/share/fonts"))
    }
}

/// Memory-maps a font from the OS-protected system font folder (read-only to users: macOS
/// SIP volume, `%WINDIR%\Fonts`, root-owned `/usr/share/fonts`), opened by a path whose
/// components below that folder are checked not to be symbolic links — under the threat
/// model of ADR 0004 such a file cannot be truncated by the user's programs. Mapped pages are
/// shared and only become resident when glyphs are used (a CJK collection of tens of MB costs
/// little until CJK text appears), and epaint borrows them without copying. Mappings are made
/// once per path and kept for the process lifetime (bounded by the fonts used). Everything
/// else is loaded as an owned snapshot (see [`load_into`]).
fn protected_map(path: &Path) -> Option<&'static [u8]> {
    static MAPS: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    let root = protected_root()?;
    let rel = path.strip_prefix(&root).ok()?;
    // no symbolic link in the folder itself or below it (a link could point to a user file)
    let mut p = root.clone();
    if std::fs::symlink_metadata(&p).ok()?.file_type().is_symlink() {
        return None;
    }
    for c in rel.components() {
        let std::path::Component::Normal(c) = c else {
            return None;
        };
        p.push(c);
        if std::fs::symlink_metadata(&p).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    let maps = MAPS.get_or_init(Default::default);
    if let Some(b) = maps.lock().ok()?.get(path) {
        return Some(b);
    }
    let file = std::fs::File::open(&p).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 || len > MAX_MAPPED_FONT_BYTES {
        return None;
    }
    // SAFETY: files in the OS-protected system font folder are not modified by user
    // processes (threat model, ADR 0004); the mapping is never unmapped.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes: &'static [u8] = Box::leak(Box::new(map));
    maps.lock().ok()?.insert(path.to_path_buf(), bytes);
    Some(bytes)
}

fn load_into(store: &Mutex<Store>, budget: usize, path: &Path, index: u32) -> Result<Arc<FontData>, &'static str> {
    let key = (path.to_path_buf(), index);
    {
        let mut st = store.lock().map_err(|_| "font store poisoned")?;
        st.tick += 1;
        let tick = st.tick;
        if let Some(e) = st.map.get_mut(&key) {
            e.tick = tick;
            return Ok(e.data.clone());
        }
    }
    let bytes = read_font(path).ok_or("unreadable or too large")?;
    if !valid_face(&bytes, index) {
        return Err("invalid font data"); // not retained
    }
    // epaint parses the shared bytes in place (no copy since 0.34): a face costs its size once
    let len = bytes.len();
    let mut st = store.lock().map_err(|_| "font store poisoned")?;
    if st.total + len > budget {
        // evict unused faces, oldest first
        let mut idle: Vec<((PathBuf, u32), u64, usize)> = st
            .map
            .iter()
            .filter(|(_, e)| Arc::strong_count(&e.data) == 1)
            .map(|(k, e)| (k.clone(), e.tick, e.bytes))
            .collect();
        idle.sort_by_key(|i| i.1);
        for (k, _, b) in idle {
            if st.total + len <= budget {
                break;
            }
            st.map.remove(&k);
            st.total -= b;
        }
        if st.total + len > budget {
            return Err("font memory budget exhausted");
        }
    }
    let data = Arc::new(FontData {
        font: Arc::new(bytes),
        index,
        tweak: Default::default(),
    });
    st.tick += 1;
    let tick = st.tick;
    st.total += len;
    st.map.insert(
        key,
        Entry {
            data: data.clone(),
            bytes: len,
            tick,
        },
    );
    Ok(data)
}

/// Font-data name of the platform's colour emoji font.
pub const EMOJI_FONT: &str = "sys-emoji";

/// The definitions the parley backend registers: all but the colour emoji face. Parley's font store
/// copies a face of a collection into a standalone font, and Apple Color Emoji's is some 190 MB: on
/// the UI thread, outside [`FONT_BUDGET`], and again on every reinstall. Until it can register a mapped
/// face without a copy, the developer backend (`MARKISE_TEXT=parley`) goes without colour emoji.
pub fn for_parley(defs: &FontDefinitions) -> FontDefinitions {
    let mut out = defs.clone();
    out.font_data.remove(EMOJI_FONT);
    for list in out.families.values_mut() {
        list.retain(|n| n != EMOJI_FONT);
    }
    out
}

/// Like [`first`], but only a face mapped from the OS-protected folder: one read into memory would
/// cost all of its size up front.
fn first_mapped(paths: &[&str], name: &str) -> Option<(String, Arc<FontData>)> {
    paths
        .iter()
        .find_map(|p| {
            let bytes = protected_map(Path::new(p))?;
            valid_face(bytes, 0).then(|| Arc::new(FontData::from_static(bytes)))
        })
        .map(|d| (name.to_string(), d))
}

fn first(paths: &[&str], name: &str) -> Option<(String, Arc<FontData>)> {
    paths
        .iter()
        .find_map(|p| load_face(Path::new(p), 0).ok())
        .map(|d| (name.to_string(), d))
}

/// The system default faces (body regular/bold, CJK and symbol fallback); loaded on the theme
/// worker.
#[derive(Clone, Default)]
pub struct DefaultFaces {
    pub body: Option<(String, Arc<FontData>)>,
    pub bold: Option<(String, Arc<FontData>)>,
    pub cjk: Option<(String, Arc<FontData>)>,
    /// The platform's symbol font (Apple Symbols, Segoe UI Symbol, Noto Sans Symbols 2).
    pub symbols: Option<(String, Arc<FontData>)>,
    /// The platform's colour emoji font (Apple Color Emoji, Segoe UI Emoji, Noto Color Emoji).
    pub emoji: Option<(String, Arc<FontData>)>,
}

/// Loads the system default faces (file reads: call off the UI thread). The CJK and symbol
/// fallbacks (tens of MB) are loaded only with `fallback` — when a document first needs a glyph
/// the primary fonts lack (see `View::missing_glyphs`). The emoji font is loaded either way: an
/// emoji in a file name, a tab or any other label asks for no fallback, and it is mapped from the
/// system's protected folder, so it costs only the glyphs drawn (none, when no emoji shows).
pub fn load_defaults(fallback: bool) -> DefaultFaces {
    let c = candidates();
    DefaultFaces {
        body: first(c.body, "sys-body"),
        bold: first(c.bold, "sys-bold"),
        cjk: if fallback { first(c.cjk, "sys-cjk") } else { None },
        symbols: if fallback {
            first(c.symbols, "sys-symbols")
        } else {
            None
        },
        emoji: first_mapped(c.emoji, EMOJI_FONT),
    }
}

/// Font definitions with the Markise families; `system` = also use system fonts (loads
/// them: tests and tools; the app gets them from the theme worker).
pub fn definitions(system: bool) -> FontDefinitions {
    let defaults = if system {
        load_defaults(true)
    } else {
        DefaultFaces::default()
    };
    definitions_with(&defaults, &FontSet::default())
}

/// Font definitions from already loaded faces (no file access): the theme's resolved roles
/// (`themed`, see [`resolve`]) in front of the defaults (system body font, egui's text font, the
/// bundled keyboard symbols, egui's emoji fonts, then the CJK and symbol fallbacks).
pub fn definitions_with(defaults: &DefaultFaces, themed: &FontSet) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    // keyboard symbols for every family (UI text, menus and shortcut hints included)
    defs.font_data.insert(
        KEYS_FONT.into(),
        Arc::new(FontData::from_static(markise_text::keys::FONT)),
    );
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(list) = defs.families.get_mut(&fam) {
            insert_before_emoji(list, KEYS_FONT);
        }
    }
    // CJK and symbol fallbacks for every family (after the primary fonts), and colour emoji last of
    // all: every family below takes this chain as its tail, so the document, the outline, the status
    // bar, tabs and menus all find the same face for a glyph
    for (name, data) in defaults.cjk.iter().chain(&defaults.symbols).chain(&defaults.emoji) {
        defs.font_data.insert(name.clone(), data.clone());
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = defs.families.get_mut(&fam) {
                list.push(name.clone());
            }
        }
    }
    let mut set = themed.clone();
    if set.body.regular.is_empty() {
        set.body = RoleFaces {
            regular: defaults.body.clone().into_iter().collect(),
            bold: defaults.bold.clone().into_iter().collect(),
        };
    }
    // Keep system fallbacks even when a theme supplied a partial-coverage face. A regular
    // fallback must not replace a bold role merely because it happens to cover the script.
    for role in std::iter::once(&mut set.body).chain(set.heading.iter_mut()) {
        for (list, face) in [(&mut role.regular, &defaults.body), (&mut role.bold, &defaults.bold)] {
            if let Some(face) = face
                && !list.iter().any(|(name, _)| name == &face.0)
            {
                list.push(face.clone());
            }
        }
    }
    register_families(&mut defs, &set);
    defs
}

/// Installs the fonts into `ctx` (takes effect at the next pass).
pub fn install(ctx: &egui::Context, system: bool) {
    ctx.set_fonts(definitions(system));
}

/// `true` when the Markise families are registered in `ctx`.
pub fn installed(ctx: &egui::Context) -> bool {
    ctx.fonts(|f| {
        f.definitions()
            .families
            .contains_key(&FontFamily::Name(markise_layout::FAMILY_BODY.into()))
    })
}

// ---------------------------------------------------------------- system font index

/// One installed font face.
#[derive(Clone, Debug)]
pub struct SysFace {
    pub path: PathBuf,
    pub index: u32,
    /// Family name (typographic family when present).
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

/// Index of the installed fonts by lowercase family, full and PostScript name.
#[derive(Debug, Default)]
pub struct SystemFonts {
    pub faces: Vec<SysFace>,
    by_name: HashMap<String, Vec<usize>>,
}

/// Platform font folders.
pub fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/System/Library/Fonts"));
        v.push(PathBuf::from("/Library/Fonts"));
        if let Some(h) = &home {
            v.push(h.join("Library/Fonts"));
        }
    } else if cfg!(windows) {
        let win = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
        v.push(win.join("Fonts"));
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
        }
    } else {
        v.push(PathBuf::from("/usr/share/fonts"));
        v.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            v.push(PathBuf::from(d).join("fonts"));
        }
        if let Some(h) = &home {
            v.push(h.join(".local/share/fonts"));
            v.push(h.join(".fonts"));
        }
    }
    v
}

impl SystemFonts {
    /// Scans `dirs` recursively (depth ≤ 6, ≤ 20 000 files). Unreadable files are skipped.
    pub fn scan(dirs: &[PathBuf]) -> SystemFonts {
        let mut files = Vec::new();
        for d in dirs {
            collect(d, 0, &mut files);
        }
        files.sort();
        files.dedup();
        let mut idx = SystemFonts::default();
        for p in files {
            // owned, bounded read (see `read_font`), dropped after the name scan
            let Some(map) = read_font(&p) else { continue };
            let n = ttf_parser::fonts_in_collection(&map).unwrap_or(1).min(64);
            for i in 0..n {
                let Ok(face) = ttf_parser::Face::parse(&map, i) else {
                    continue;
                };
                let mut family = None;
                let mut typo = None;
                let mut names = Vec::new();
                for name in face.names() {
                    if !matches!(name.name_id, 1 | 4 | 6 | 16) {
                        continue;
                    }
                    let Some(s) = name
                        .to_string()
                        .or_else(|| mac_roman(&name))
                        .filter(|s| !s.trim().is_empty())
                    else {
                        continue;
                    };
                    match name.name_id {
                        1 if family.is_none() => family = Some(s.clone()),
                        16 if typo.is_none() => typo = Some(s.clone()),
                        _ => {}
                    }
                    names.push(s.to_lowercase());
                }
                let Some(family) = typo.or(family) else { continue };
                let fi = idx.faces.len();
                idx.faces.push(SysFace {
                    path: p.clone(),
                    index: i,
                    family: family.clone(),
                    weight: face.weight().to_number(),
                    italic: face.is_italic() || face.is_oblique(),
                });
                names.push(family.to_lowercase());
                names.sort();
                names.dedup();
                for n in names {
                    idx.by_name.entry(n).or_default().push(fi);
                }
            }
        }
        idx
    }

    /// The process-wide index of the platform font folders (scanned on first use; blocking).
    pub fn global() -> &'static SystemFonts {
        static INDEX: OnceLock<SystemFonts> = OnceLock::new();
        INDEX.get_or_init(|| SystemFonts::scan(&font_dirs()))
    }

    /// Faces whose family, full or PostScript name is `name` (case-insensitive), expanded to
    /// every face of the matched family.
    pub fn family_faces(&self, name: &str) -> Vec<&SysFace> {
        let Some(hits) = self.by_name.get(&name.trim().to_lowercase()) else {
            return Vec::new();
        };
        let fam = self.faces[hits[0]].family.to_lowercase();
        let mut v: Vec<&SysFace> = self
            .by_name
            .get(&fam)
            .map(|ids| ids.iter().map(|&i| &self.faces[i]).collect())
            .unwrap_or_default();
        if v.is_empty() {
            v = hits.iter().map(|&i| &self.faces[i]).collect();
        }
        v.retain(|f| f.family.to_lowercase() == fam);
        if v.is_empty() {
            v = hits.iter().map(|&i| &self.faces[i]).collect();
        }
        v
    }
}

/// Names in the Macintosh / Roman encoding (many Apple system fonts carry only these): ASCII
/// is decoded, other bytes are dropped (family names are ASCII in practice).
fn mac_roman(name: &ttf_parser::name::Name) -> Option<String> {
    if name.platform_id != ttf_parser::PlatformId::Macintosh || name.encoding_id != 0 {
        return None;
    }
    let s: String = name
        .name
        .iter()
        .filter(|b| b.is_ascii() && !b.is_ascii_control())
        .map(|&b| b as char)
        .collect();
    (!s.is_empty()).then_some(s)
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 || out.len() > 20_000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            collect(&p, depth + 1, out);
        } else if ft.is_file() || ft.is_symlink() {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
                out.push(p);
            }
        }
    }
}

// ---------------------------------------------------------------- theme font resolution

/// Font stacks of a theme that need resolving: body, heading (empty = body), mono, plus
/// `local()` names declared by `@font-face` rules per family.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontRequest {
    pub body: Vec<String>,
    pub heading: Vec<String>,
    pub mono: Vec<String>,
    pub sidebar: Vec<String>,
    pub locals: Vec<(String, String)>,
    pub scripts: std::collections::BTreeMap<String, String>,
}

impl FontRequest {
    pub fn of(theme: &Theme) -> FontRequest {
        let mut locals = Vec::new();
        for f in &theme.fonts.faces {
            for s in &f.sources {
                if let FontSource::Local(n) = s
                    && !locals.contains(&(f.family.to_lowercase(), n.clone()))
                {
                    locals.push((f.family.to_lowercase(), n.clone()));
                }
            }
        }
        FontRequest {
            body: theme.fonts.body.clone(),
            heading: theme.fonts.heading.clone(),
            mono: theme.fonts.mono.clone(),
            sidebar: theme.sidebar.fonts.clone(),
            locals,
            scripts: Default::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
            && self.heading.is_empty()
            && self.mono.is_empty()
            && self.sidebar.is_empty()
            && self.scripts.is_empty()
    }

    pub fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// What [`resolve`] chose (shown in the theme report).
#[derive(Clone, Default)]
pub struct ResolvedFonts {
    /// System default faces (body, bold, CJK and symbol fallback).
    pub defaults: DefaultFaces,
    pub set: FontSet,
    /// Per role: the installed families used, in fallback order.
    pub chosen: Vec<(&'static str, Vec<String>)>,
    /// Stack names that are not installed (or are `@font-face` files, not loaded yet).
    pub missing: Vec<String>,
}

/// Families a generic CSS family name maps to on this platform; `None` = the default body
/// font (sans-serif and system UI names).
fn generic(name: &str) -> Option<Option<&'static [&'static str]>> {
    let mac = cfg!(target_os = "macos");
    let win = cfg!(windows);
    Some(match name {
        "sans-serif" | "system-ui" | "-apple-system" | "blinkmacsystemfont" | "ui-sans-serif" => None,
        "serif" | "ui-serif" => Some(if mac {
            &["Georgia", "Times New Roman", "Times"][..]
        } else if win {
            &["Georgia", "Times New Roman", "Cambria"][..]
        } else {
            &["DejaVu Serif", "Noto Serif", "Liberation Serif"][..]
        }),
        "monospace" | "ui-monospace" => Some(if mac {
            &["Menlo", "SF Mono", "Monaco", "Courier New"][..]
        } else if win {
            &["Consolas", "Cascadia Mono", "Courier New"][..]
        } else {
            &["DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono"][..]
        }),
        "cursive" | "fantasy" | "math" | "emoji" | "fangsong" | "ui-rounded" => Some(&[][..]),
        _ => return None,
    })
}

const MAX_FALLBACKS: usize = 3;

fn pick_face<'a>(faces: &[&'a SysFace], weight: u16) -> Option<&'a SysFace> {
    faces
        .iter()
        .copied()
        .filter(|f| !f.italic)
        .min_by_key(|f| (f.weight as i32 - weight as i32).abs())
        .or_else(|| faces.first().copied())
}

fn resolve_role(
    stack: &[String],
    req: &FontRequest,
    idx: &SystemFonts,
    role: &'static str,
    out: &mut ResolvedFonts,
) -> Option<RoleFaces> {
    let mut names: Vec<String> = Vec::new();
    for s in stack {
        let lower = s.trim().to_lowercase();
        // `@font-face` aliases: their local() names first (the files are not loaded, C3)
        for (fam, local) in &req.locals {
            if *fam == lower {
                names.push(local.clone());
            }
        }
        match generic(&lower) {
            Some(None) => {
                names.push("\u{0}default".into());
                break; // the default body font ends the stack
            }
            Some(Some(list)) => names.extend(list.iter().map(|s| s.to_string())),
            None => names.push(s.clone()),
        }
    }
    let mut faces = RoleFaces::default();
    let mut no_bold: Vec<(String, Arc<FontData>)> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for n in names {
        if used.len() >= MAX_FALLBACKS {
            break;
        }
        if n == "\u{0}default" {
            break;
        }
        let fam = idx.family_faces(&n);
        if fam.is_empty() {
            if !out.missing.iter().any(|m| m.eq_ignore_ascii_case(&n)) {
                out.missing.push(n);
            }
            continue;
        }
        let family = fam[0].family.clone();
        if used.iter().any(|u| u.eq_ignore_ascii_case(&family)) {
            continue;
        }
        let Some(reg) = pick_face(&fam, 400) else { continue };
        let real_bold = pick_face(&fam, 700).filter(|b| b.weight >= 600);
        let bold = real_bold.unwrap_or(reg);
        let load = |f: &SysFace| -> Result<(String, Arc<FontData>), &'static str> {
            load_face(&f.path, f.index).map(|d| (format!("sys:{}#{}", f.path.display(), f.index), d))
        };
        let (r, b) = match (load(reg), load(bold)) {
            (Ok(r), Ok(b)) => (r, b),
            (Err(why), _) | (_, Err(why)) => {
                // the file changed since indexing, epaint could not load it, or no room
                let what = format!("{family} ({why})");
                if !out.missing.contains(&what) {
                    out.missing.push(what);
                }
                continue;
            }
        };
        faces.regular.push(r);
        if real_bold.is_some() {
            faces.bold.push(b);
        } else {
            no_bold.push(b);
        }
        used.push(family);
    }
    // no synthetic bold in epaint: families with a real bold face come first in the bold role
    // (e.g. a variable font without static bold instances keeps regular text, bold text uses
    // the next family's bold face)
    faces.bold.extend(no_bold);
    out.chosen.push((role, used));
    (!faces.regular.is_empty()).then_some(faces)
}

/// Resolves the theme's font stacks against `idx` (see the module docs).
pub fn resolve_with(req: &FontRequest, idx: &SystemFonts) -> ResolvedFonts {
    let mut out = ResolvedFonts::default();
    let body = resolve_role(&req.body, req, idx, "body", &mut out);
    let heading = if req.heading.is_empty() {
        None
    } else {
        resolve_role(&req.heading, req, idx, "heading", &mut out)
    };
    let mono = resolve_role(&req.mono, req, idx, "mono", &mut out);
    let sidebar = if req.sidebar.is_empty() {
        None
    } else {
        resolve_role(&req.sidebar, req, idx, "sidebar", &mut out)
    };
    let mut scripts = std::collections::BTreeMap::new();
    for (script, name) in &req.scripts {
        if let Some(faces) = resolve_role(std::slice::from_ref(name), req, idx, "script override", &mut out) {
            scripts.insert(script.clone(), faces);
        }
    }
    // a heading stack with no installed font uses the body faces
    out.set = FontSet {
        scripts,
        body: body.unwrap_or_default(),
        heading,
        mono: mono.unwrap_or_default(),
        sidebar,
    };
    out
}

/// [`resolve_with`] against the global system index (blocking on the first call).
/// Also loads the system default faces (with the CJK and symbol fallbacks when `fallback`). File reads:
/// call on the theme worker.
pub fn resolve(req: &FontRequest, fallback: bool) -> ResolvedFonts {
    let mut r = if req.is_empty() {
        ResolvedFonts::default()
    } else {
        resolve_with(req, SystemFonts::global())
    };
    r.defaults = load_defaults(fallback);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_font() -> Vec<u8> {
        let defs = FontDefinitions::default();
        let d = defs.font_data.get("Ubuntu-Light").expect("egui bundles Ubuntu-Light");
        d.font.as_ref().as_ref().to_vec()
    }

    /// `font` with the table directory record of `tag` edited by `edit(tag, length)`.
    fn edit_table_record(font: &[u8], tag: &[u8; 4], edit: impl FnOnce(&mut [u8; 4], &mut u32)) -> Vec<u8> {
        let mut out = font.to_vec();
        let n = u16::from_be_bytes([out[4], out[5]]) as usize;
        let rec = (0..n)
            .map(|i| 12 + 16 * i)
            .find(|&r| &out[r..r + 4] == tag)
            .expect("table present");
        let mut t: [u8; 4] = out[rec..rec + 4].try_into().unwrap();
        let mut len = u32::from_be_bytes(out[rec + 12..rec + 16].try_into().unwrap());
        edit(&mut t, &mut len);
        out[rec..rec + 4].copy_from_slice(&t);
        out[rec + 12..rec + 16].copy_from_slice(&len.to_be_bytes());
        out
    }

    /// The bundled font without its `hhea` table (renamed to `hheb`, which keeps the
    /// directory sorted), `head` intact.
    fn font_without_hhea() -> Vec<u8> {
        let bad = edit_table_record(&bundled_font(), b"hhea", |t, _| *t = *b"hheb");
        let f = skrifa::FontRef::new(&bad).unwrap();
        assert!(skrifa::raw::TableProvider::head(&f).is_ok(), "only hhea is broken");
        bad
    }

    /// The bundled font with its `hmtx` table cut to 2 bytes.
    fn font_with_truncated_hmtx() -> Vec<u8> {
        edit_table_record(&bundled_font(), b"hmtx", |_, len| *len = 2)
    }

    #[test]
    fn invalid_or_truncated_font_data_is_rejected() {
        let good = bundled_font();
        assert!(valid_face(&good, 0));
        assert!(!valid_face(b"not a font at all", 0));
        assert!(!valid_face(&good[..good.len() / 3], 0));
        assert!(!valid_face(&good, 5), "no such face index");
        // structured corruption: the directory parses and `head` is fine, but horizontal
        // layout would get zero advances
        assert!(!valid_face(&font_without_hhea(), 0), "missing hhea");
        assert!(!valid_face(&font_with_truncated_hmtx(), 0), "truncated hmtx");
        let short_hhea = edit_table_record(&good, b"hhea", |_, len| *len = 8);
        assert!(!valid_face(&short_hhea, 0), "truncated hhea");
        let no_maxp = edit_table_record(&good, b"maxp", |t, _| *t = *b"maxq");
        assert!(!valid_face(&no_maxp, 0), "missing maxp");
    }

    #[test]
    fn a_font_replaced_after_indexing_is_skipped_and_reported() {
        let dir = std::env::temp_dir().join(format!("markise-fonts-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Swap.ttf");
        std::fs::write(&path, bundled_font()).unwrap();
        let idx = SystemFonts::scan(std::slice::from_ref(&dir));
        let family = idx.faces.first().expect("indexed").family.clone();
        // replaced before the theme asks for it: by garbage, or by structurally damaged fonts
        // whose directory and `head` still parse
        for bad in [vec![0x42u8; 4096], font_without_hhea(), font_with_truncated_hmtx()] {
            std::fs::write(&path, bad).unwrap();
            let req = FontRequest {
                body: vec![family.clone()],
                ..Default::default()
            };
            let r = resolve_with(&req, &idx);
            assert!(r.set.body.regular.is_empty(), "invalid bytes never reach epaint");
            assert!(
                r.missing.iter().any(|m| m.contains("invalid font data")),
                "{:?}",
                r.missing
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn protected_system_fonts_are_mapped_and_user_paths_are_not() {
        assert!(protected_map(Path::new("/System/Library/Fonts/Menlo.ttc")).is_some());
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.ttf");
        std::fs::write(&p, bundled_font()).unwrap();
        assert!(protected_map(&p).is_none());
    }

    #[test]
    fn retained_font_data_stays_within_the_budget_and_invalid_data_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let font = bundled_font();
        let (a, b, bad) = (
            dir.path().join("a.ttf"),
            dir.path().join("b.ttf"),
            dir.path().join("bad.ttf"),
        );
        std::fs::write(&a, &font).unwrap();
        std::fs::write(&b, &font).unwrap();
        std::fs::write(&bad, vec![7u8; 1000]).unwrap();
        let st = Mutex::new(Store::default());
        // a face is accounted once (epaint shares the bytes): room for one face, not two
        let budget = font.len() * 3 / 2;
        let fa = load_into(&st, budget, &a, 0).unwrap();
        // invalid bytes are rejected before they are retained
        assert!(load_into(&st, budget, &bad, 0).is_err());
        assert_eq!(st.lock().unwrap().total, font.len());
        // `a` is still in use (egui holds it): no room for `b`
        assert_eq!(
            load_into(&st, budget, &b, 0).err(),
            Some("font memory budget exhausted")
        );
        // once `a` is no longer used it is evicted to make room
        drop(fa);
        let fb = load_into(&st, budget, &b, 0).unwrap();
        let s = st.lock().unwrap();
        assert!(s.total <= budget && s.map.len() == 1);
        drop(fb);
        // shared, not copied: a second request returns the same allocation
        drop(s);
        let x = load_into(&st, budget, &b, 0).unwrap();
        let y = load_into(&st, budget, &b, 0).unwrap();
        assert!(Arc::ptr_eq(&x, &y));
    }

    /// Characters of `text` without a glyph in `family` (whitespace and controls aside).
    fn missing(fonts: &mut egui::epaint::text::Fonts, family: &FontFamily, text: &str) -> String {
        let id = egui::FontId::new(14.0, family.clone());
        let mut out: Vec<char> = text
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control() && !fonts.has_glyph(&id, *c))
            .collect();
        out.sort_unstable();
        out.dedup();
        out.into_iter().collect()
    }

    /// Issue #131: with only the bundled fonts (no system font installed — the worst case on
    /// any platform), every character of the welcome tutorial and of the ⌘? cheat sheet (both
    /// platforms; labels, chords, group titles) and every macOS menu shortcut hint has a glyph,
    /// in the document's families (body, bold, headings, source view) and in egui's UI fonts.
    #[test]
    fn tutorial_cheat_sheet_and_menu_hints_have_no_missing_glyphs() {
        use markise_engine::editor::command::Registry;
        use markise_engine::editor::keymap::{Keymap, Platform, chord_label};
        let mut text = crate::tutorial::TEXT.to_string();
        let r = Registry::for_format(&markise_md::MarkdownFormat::default());
        for p in [Platform::Mac, Platform::Other] {
            let k = Keymap::new(&r, p);
            for g in crate::shortcuts::cheat_sheet(&r, &k, p) {
                text.push_str(g.title);
                for row in g.rows {
                    text.push_str(row.label);
                    text.push_str(&row.chord);
                }
            }
            for d in r.describe() {
                if let Some(c) = k.shown_chord_of(d.id) {
                    text.push_str(&chord_label(c, p));
                }
            }
        }
        text.extend(markise_text::keys::SYMBOLS);
        let mut fonts = egui::epaint::text::Fonts::new(Default::default(), definitions(false));
        let families = [
            markise_layout::FAMILY_BODY,
            markise_layout::FAMILY_BOLD,
            markise_layout::FAMILY_HEADING,
            markise_layout::FAMILY_HEADING_BOLD,
            markise_layout::FAMILY_MONO,
            markise_layout::FAMILY_MONO_BOLD,
        ]
        .map(|n| FontFamily::Name(n.into()));
        for fam in families
            .iter()
            .chain(&[FontFamily::Proportional, FontFamily::Monospace])
        {
            assert_eq!(missing(&mut fonts, fam, &text), "", "{fam:?}");
        }
        // the symbols come from the bundled face, not from emoji fonts
        let chars = fonts
            .with_pixels_per_point(1.0)
            .characters(&FontFamily::Proportional)
            .clone();
        for c in ['⇧', '⌥', '⌃', '⎋', '⌫', '⌦', '⇞', '⇟'] {
            assert!(chars[&c].iter().any(|n| n == KEYS_FONT), "{c}: {:?}", chars[&c]);
        }
    }

    #[test]
    fn the_keys_face_precedes_the_emoji_fonts_and_follows_the_text_fonts() {
        let defs = definitions(false);
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            let list = &defs.families[&fam];
            let keys = list.iter().position(|n| n == KEYS_FONT).unwrap();
            let emoji = list.iter().position(|n| n == "NotoEmoji-Regular").unwrap();
            assert!(keys > 0 && keys < emoji, "{fam:?}: {list:?}");
        }
        let body = &defs.families[&FontFamily::Name(markise_layout::FAMILY_BODY.into())];
        assert!(body.iter().any(|n| n == KEYS_FONT), "{body:?}");
    }

    /// Issue #231: an emoji egui's own emoji font lacks (🧪) drew as a box in the outline. Every family
    /// ends with the same faces, the platform's emoji font last: the document's roles, the sidebar and
    /// egui's own families, which the status bar, tabs and menus use.
    #[test]
    fn every_family_ends_with_the_emoji_font() {
        // a stand-in for the platform's face: what matters is where it goes
        let face = Arc::new(FontData::from_static(markise_text::keys::FONT));
        let defaults = DefaultFaces {
            emoji: Some((EMOJI_FONT.to_string(), face)),
            ..DefaultFaces::default()
        };
        let defs = definitions_with(&defaults, &FontSet::default());
        let named = [
            markise_layout::FAMILY_BODY,
            markise_layout::FAMILY_BOLD,
            markise_layout::FAMILY_HEADING,
            markise_layout::FAMILY_HEADING_BOLD,
            markise_layout::FAMILY_MONO,
            markise_layout::FAMILY_MONO_BOLD,
            markise_layout::FAMILY_SIDEBAR,
            markise_layout::FAMILY_SIDEBAR_BOLD,
        ]
        .map(|n| FontFamily::Name(n.into()));
        for fam in named.iter().chain(&[FontFamily::Proportional, FontFamily::Monospace]) {
            let list = &defs.families[fam];
            assert_eq!(list.last().map(String::as_str), Some(EMOJI_FONT), "{fam:?}: {list:?}");
        }
    }

    /// With the platform's emoji font installed (macOS and Windows always, Linux with Noto Color Emoji),
    /// 🧪 has a glyph wherever text is drawn, the outline's family and the editor's alike.
    #[test]
    fn an_emoji_newer_than_eguis_own_font_has_a_glyph_in_every_family() {
        let defaults = load_defaults(false);
        if defaults.emoji.is_none() {
            // every macOS has Apple Color Emoji: there, finding none is the failure
            #[cfg(target_os = "macos")]
            panic!("no Apple Color Emoji");
            #[cfg(not(target_os = "macos"))]
            return;
        }
        let mut fonts =
            egui::epaint::text::Fonts::new(Default::default(), definitions_with(&defaults, &FontSet::default()));
        for fam in [
            FontFamily::Name(markise_layout::FAMILY_SIDEBAR.into()),
            FontFamily::Name(markise_layout::FAMILY_HEADING_BOLD.into()),
            FontFamily::Name(markise_layout::FAMILY_BODY.into()),
            FontFamily::Proportional,
            FontFamily::Monospace,
        ] {
            assert_eq!(missing(&mut fonts, &fam, "🧪 Lab 🧠 🫠"), "", "{fam:?}");
        }
        // egui's own fonts alone lack it: this is what the platform's face is for
        let mut bundled = egui::epaint::text::Fonts::new(Default::default(), definitions(false));
        assert_eq!(missing(&mut bundled, &FontFamily::Proportional, "🧪"), "🧪");
    }

    /// The parley backend registers every face but the emoji one, which it would copy whole.
    #[test]
    fn the_parley_backend_goes_without_the_emoji_face() {
        let face = Arc::new(FontData::from_static(markise_text::keys::FONT));
        let defaults = DefaultFaces {
            emoji: Some((EMOJI_FONT.to_string(), face)),
            ..DefaultFaces::default()
        };
        let defs = definitions_with(&defaults, &FontSet::default());
        let parley = for_parley(&defs);
        assert!(!parley.font_data.contains_key(EMOJI_FONT));
        assert!(parley.families.values().all(|l| !l.iter().any(|n| n == EMOJI_FONT)));
        assert_eq!(
            parley.font_data.len() + 1,
            defs.font_data.len(),
            "everything else stays"
        );
        assert_eq!(parley.families.len(), defs.families.len());
    }

    #[test]
    fn generic_names() {
        assert_eq!(generic("sans-serif"), Some(None));
        assert!(matches!(generic("monospace"), Some(Some(l)) if !l.is_empty()));
        assert_eq!(generic("Georgia"), None);
    }

    #[test]
    fn unknown_names_are_reported_missing() {
        let idx = SystemFonts::default();
        let req = FontRequest {
            body: vec!["No Such Font".into(), "serif".into()],
            ..Default::default()
        };
        let r = resolve_with(&req, &idx);
        assert!(r.set.body.regular.is_empty());
        assert!(r.missing.iter().any(|m| m == "No Such Font"));
    }

    #[test]
    fn request_includes_local_aliases() {
        let mut t = Theme::light();
        t.fonts.body = vec!["JB".into(), "monospace".into()];
        t.fonts.faces.push(markise_theme::FontFace {
            family: "JB".into(),
            sources: vec![FontSource::Local("JetBrains Mono".into())],
            weight: (400, 400),
            italic: false,
            unicode_range: vec![],
            display: None,
        });
        let r = FontRequest::of(&t);
        assert_eq!(r.locals, vec![("jb".to_string(), "JetBrains Mono".to_string())]);
        assert_ne!(r.key(), FontRequest::default().key());
    }
}
