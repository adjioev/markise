//! `FontStore` (ADR 0005 §4.1 `store.rs` + `itemize.rs`): the fontique collection (system fonts
//! discovered on a worker thread), theme `@font-face` faces under private names, the
//! face-name index for `local()`, a font generation counter, and CSS cluster matching that
//! turns a style run into parley ranges with explicit family lists.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use parley::fontique::{
    Blob, Collection, CollectionOptions, FontInfoOverride, FontStyle, FontWeight, FontWidth, GenericFamily,
};
use parley::{FontContext, FontFamilyName};
use unicode_segmentation::UnicodeSegmentation;

use crate::coverage::{FaceCoverage, VsMode};
use crate::face::{FaceError, FaceRule, FontBytes, LoadPolicy, face_count, load_rule};
use crate::matching::{self, FaceSlope, Request};
use crate::names::FaceNameIndex;
use crate::style::{Family, Generic, Slope, TextStyle};

/// Index of a theme face in its store.
pub type FaceId = usize;

/// A loaded `@font-face` rule.
pub(crate) struct ThemeFace {
    pub rule: FaceRule,
    pub private_name: String,
    pub blob: Blob<u8>,
    pub index: u32,
    /// Variation axes of the face (`wght`, `wdth`, `slnt`, …).
    pub axes: Vec<[u8; 4]>,
    pub family_id: Option<parley::fontique::FamilyId>,
}

/// Result of a finished system discovery, sent from the worker.
#[cfg(not(target_arch = "wasm32"))]
struct Discovery {
    collection: Collection,
    names: FaceNameIndex,
    elapsed: Duration,
}

/// (family, weight, slope bits) → resolved system face.
type SystemKey = (String, u32, u32, u32);

/// Cache bounds (entries). Matching caches live outside the layout-cache budget, so each is
/// capped and simply cleared when full (they are cheap to refill). Worst case ≈ 1 MB for the
/// coverage cache (24-byte entries plus table overhead) and well under 1 MB for the others.
pub(crate) const COV_CACHE_CAP: usize = 16 * 1024;
pub(crate) const SEL_CACHE_CAP: usize = 1024;
pub(crate) const SYSTEM_CACHE_CAP: usize = 1024;
/// (family, weight bits, slope bits, stretch bits) → face-set selection.
type SelKey = (String, u32, u32, u32);

/// Fonts available to the shaper.
pub struct FontStore {
    pub(crate) fcx: FontContext,
    pub(crate) faces: Vec<ThemeFace>,
    families: HashMap<String, Vec<FaceId>>,
    names: Option<Arc<FaceNameIndex>>,
    generation: u64,
    policy: LoadPolicy,
    system_ready: bool,
    discovery_time: Option<Duration>,
    #[cfg(not(target_arch = "wasm32"))]
    pending: Option<std::sync::mpsc::Receiver<Discovery>>,
    system_cache: HashMap<SystemKey, Option<(Blob<u8>, u32)>>,
    sel_cache: HashMap<SelKey, Option<Arc<RunSel>>>,
    /// (blob id, face index, char) → supported (single-character clusters).
    cov_cache: ahash::AHashMap<(u64, u32, u32), bool>,
    /// Private names are never reused (a stale registration must not match a new face).
    next_private: usize,
}

impl Default for FontStore {
    fn default() -> Self {
        Self::new()
    }
}

fn family_key(s: &str) -> String {
    s.trim().to_lowercase()
}

fn to_fontique_style(s: Slope) -> FontStyle {
    match s {
        Slope::Normal => FontStyle::Normal,
        Slope::Italic => FontStyle::Italic,
        Slope::Oblique(a) => FontStyle::Oblique(Some(a)),
    }
}

fn face_style(s: FaceSlope) -> FontStyle {
    match s {
        FaceSlope::Normal => FontStyle::Normal,
        FaceSlope::Italic => FontStyle::Italic,
        FaceSlope::Oblique(lo, _) => FontStyle::Oblique(Some(lo)),
    }
}

pub(crate) fn to_generic(g: Generic) -> GenericFamily {
    match g {
        Generic::Serif => GenericFamily::Serif,
        Generic::SansSerif => GenericFamily::SansSerif,
        Generic::Monospace => GenericFamily::Monospace,
        Generic::Cursive => GenericFamily::Cursive,
        Generic::Fantasy => GenericFamily::Fantasy,
        Generic::SystemUi => GenericFamily::SystemUi,
        Generic::Emoji => GenericFamily::Emoji,
        Generic::Math => GenericFamily::Math,
    }
}

/// Copies one face of a collection into a standalone sfnt (fontique registers every face of a
/// blob under the override name, so a `local()` hit inside a .ttc must be isolated).
fn extract_face(data: &[u8], index: u32) -> Option<Vec<u8>> {
    use skrifa::raw::{FileRef, TableProvider};
    let font = match FileRef::new(data).ok()? {
        FileRef::Font(_) => return None,
        FileRef::Collection(c) => c.get(index).ok()?,
    };
    let dir = font.table_directory();
    let mut tables = std::collections::BTreeMap::new();
    for rec in dir.table_records() {
        let tag = rec.tag().to_be_bytes();
        let d = font.data_for_tag(rec.tag())?;
        tables.insert(tag, d.as_bytes().to_vec());
    }
    Some(crate::face::sfnt(&tables))
}

impl FontStore {
    /// An empty store: no system fonts yet (call [`FontStore::start_system_discovery`]), usable
    /// immediately with theme faces and last-resort shaping.
    pub fn new() -> Self {
        FontStore {
            fcx: FontContext {
                collection: Collection::new(CollectionOptions {
                    shared: false,
                    system_fonts: false,
                }),
                source_cache: Default::default(),
            },
            faces: Vec::new(),
            families: HashMap::new(),
            names: None,
            generation: 0,
            policy: LoadPolicy::default(),
            system_ready: false,
            discovery_time: None,
            #[cfg(not(target_arch = "wasm32"))]
            pending: None,
            system_cache: HashMap::new(),
            next_private: 0,
            sel_cache: HashMap::new(),
            cov_cache: Default::default(),
        }
    }

    /// Sets where `@font-face` file sources may come from.
    pub fn set_policy(&mut self, policy: LoadPolicy) {
        self.policy = policy;
    }

    /// Font generation: bumped whenever the set of usable fonts changes (layouts keyed by it
    /// must be redone).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn system_ready(&self) -> bool {
        self.system_ready
    }

    /// Time the worker needed for system discovery (collection + face-name index).
    pub fn discovery_time(&self) -> Option<Duration> {
        self.discovery_time
    }

    /// Starts discovering system fonts and building the face-name index on a worker thread
    /// (never on the caller's thread). Results are installed by [`FontStore::poll`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_system_discovery(&mut self) {
        self.start_discovery_in(FaceNameIndex::platform_dirs(), true);
    }

    /// Like [`FontStore::start_system_discovery`] with explicit name-index folders; `system`
    /// false skips the platform collection (tests).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_discovery_in(&mut self, dirs: Vec<std::path::PathBuf>, system: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending = Some(rx);
        std::thread::Builder::new()
            .name("markise-text-fonts".into())
            .spawn(move || {
                let t = std::time::Instant::now();
                let mut collection = Collection::new(CollectionOptions {
                    shared: false,
                    system_fonts: system,
                });
                // touch the family list so the platform enumeration happens here
                let _ = collection.family_names().count();
                let names = FaceNameIndex::scan(&dirs);
                let _ = tx.send(Discovery {
                    collection,
                    names,
                    elapsed: t.elapsed(),
                });
            })
            .expect("spawn font discovery thread");
    }

    /// wasm: no system fonts; fonts arrive as bytes.
    #[cfg(target_arch = "wasm32")]
    pub fn start_system_discovery(&mut self) {}

    /// Installs a finished discovery. Returns true when fonts changed (generation bumped).
    pub fn poll(&mut self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(rx) = &self.pending else {
                return false;
            };
            match rx.try_recv() {
                Ok(d) => {
                    self.install(d);
                    true
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    false
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Blocks until discovery finishes (tests, benches, spikes; never on a UI thread).
    pub fn wait_for_system(&mut self, timeout: Duration) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(rx) = &self.pending else {
                return self.system_ready;
            };
            match rx.recv_timeout(timeout) {
                Ok(d) => {
                    self.install(d);
                    true
                }
                Err(_) => false,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = timeout;
            false
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn install(&mut self, d: Discovery) {
        self.fcx.collection = d.collection;
        self.fcx.source_cache = Default::default();
        for f in &mut self.faces {
            f.family_id = Self::register(&mut self.fcx.collection, f);
        }
        self.names = Some(Arc::new(d.names));
        self.system_ready = true;
        self.discovery_time = Some(d.elapsed);
        self.system_cache.clear();
        self.cov_cache.clear();
        self.generation += 1;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.pending = None;
        }
    }

    /// Supplies a face-name index directly (tests; wasm has none).
    pub fn set_name_index(&mut self, idx: FaceNameIndex) {
        self.names = Some(Arc::new(idx));
    }

    pub fn name_index(&self) -> Option<&FaceNameIndex> {
        self.names.as_deref()
    }

    fn register(collection: &mut Collection, f: &ThemeFace) -> Option<parley::fontique::FamilyId> {
        let d = f.rule.descriptors;
        let r = collection.register_fonts(
            f.blob.clone(),
            Some(FontInfoOverride {
                family_name: Some(&f.private_name),
                width: Some(FontWidth::from_percentage(d.stretch.0)),
                style: Some(face_style(d.slope)),
                weight: Some(FontWeight::new(d.weight.0)),
                axes: None,
            }),
        );
        r.first().map(|(id, _)| *id)
    }

    /// Loads an `@font-face` rule (sources in order, first that loads wins) and registers it
    /// under a private family name. Bumps the generation.
    pub fn add_face(&mut self, rule: FaceRule) -> Result<FaceId, FaceError> {
        let rule = rule.normalized();
        let loaded = load_rule(&rule, &self.policy, self.names.as_deref())?;
        let (bytes, index) = if face_count(loaded.bytes.as_ref()) > 1 || loaded.index > 0 {
            match extract_face(loaded.bytes.as_ref(), loaded.index) {
                Some(v) => (FontBytes::Owned(v), 0),
                None => (loaded.bytes, loaded.index),
            }
        } else {
            (loaded.bytes, loaded.index)
        };
        let axes: Vec<[u8; 4]> = skrifa::FontRef::from_index(bytes.as_ref(), index)
            .map(|f| {
                use skrifa::MetadataProvider;
                f.axes().iter().map(|a| a.tag().to_be_bytes()).collect()
            })
            .unwrap_or_default();
        let id = self.faces.len();
        let mut face = ThemeFace {
            rule,
            private_name: format!("mk-ff-{}", self.next_private),
            blob: Blob::new(Arc::new(bytes)),
            index,
            axes,
            family_id: None,
        };
        self.next_private += 1;
        face.family_id = Self::register(&mut self.fcx.collection, &face);
        self.families.entry(family_key(&face.rule.family)).or_default().push(id);
        self.faces.push(face);
        self.sel_cache.clear();
        self.generation += 1;
        Ok(id)
    }

    /// Removes all theme faces (theme switch). Bumps the generation.
    pub fn clear_faces(&mut self) {
        let removed: Vec<u64> = self.faces.iter().map(|f| f.blob.id()).collect();
        for f in self.faces.drain(..) {
            if let Some(fid) = f.family_id {
                let d = f.rule.descriptors;
                self.fcx.collection.unregister_font(
                    fid,
                    FontWidth::from_percentage(d.stretch.0),
                    face_style(d.slope),
                    FontWeight::new(d.weight.0),
                );
            }
        }
        self.families.clear();
        self.sel_cache.clear();
        // coverage of removed theme blobs can never be hit again (new blobs get new ids)
        let theme_blobs: std::collections::HashSet<u64> = removed.iter().copied().collect();
        self.cov_cache.retain(|k, _| !theme_blobs.contains(&k.0));
        self.generation += 1;
    }

    /// Entries in the matching caches: (face-set selection, system faces, coverage).
    pub fn cache_entries(&self) -> (usize, usize, usize) {
        (self.sel_cache.len(), self.system_cache.len(), self.cov_cache.len())
    }

    /// Cache capacities matching [`FontStore::cache_entries`].
    pub fn cache_caps() -> (usize, usize, usize) {
        (SEL_CACHE_CAP, SYSTEM_CACHE_CAP, COV_CACHE_CAP)
    }

    /// Registers font bytes as an installed ("system") family, as platform discovery would
    /// (tests, wasm builds that ship fonts as bytes).
    pub fn register_system_family(&mut self, family: &str, bytes: Vec<u8>, stretch: f32, weight: f32) {
        self.register_blob(family, Blob::new(Arc::new(bytes)), stretch, weight);
    }

    /// Registers face `index` of shared font bytes (e.g. a memory-mapped file or another font
    /// system's blob) as an installed family. A single font file is registered without a copy;
    /// for a collection (.ttc/.otc) only the selected face is registered — copied into a
    /// standalone sfnt, since fontique would otherwise register every face of the blob under
    /// `family`. Returns false (nothing registered) when `index` is not a face of the file.
    pub fn register_system_family_shared(
        &mut self,
        family: &str,
        bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
        index: u32,
        stretch: f32,
        weight: f32,
    ) -> bool {
        use skrifa::raw::FileRef;
        let data: &[u8] = (*bytes).as_ref();
        let blob = match FileRef::new(data) {
            Ok(FileRef::Font(_)) if index == 0 => Blob::new(bytes.clone()),
            Ok(FileRef::Collection(_)) => match extract_face(data, index) {
                Some(face) => Blob::new(Arc::new(face)),
                None => return false,
            },
            _ => return false,
        };
        self.register_blob(family, blob, stretch, weight);
        true
    }

    fn register_blob(&mut self, family: &str, blob: Blob<u8>, stretch: f32, weight: f32) {
        self.fcx.collection.register_fonts(
            blob,
            Some(FontInfoOverride {
                family_name: Some(family),
                width: Some(FontWidth::from_percentage(stretch)),
                style: None,
                weight: Some(FontWeight::new(weight)),
                axes: None,
            }),
        );
        self.system_cache.clear();
        self.generation += 1;
    }

    /// Private family name of a theme face (for diagnostics and tests).
    pub fn face_family_name(&self, id: FaceId) -> &str {
        &self.faces[id].private_name
    }

    /// Blob id of a theme face's font data (glyph runs report it, see `GlyphRun::font`).
    pub fn face_blob_id(&self, id: FaceId) -> u64 {
        self.faces[id].blob.id()
    }

    pub fn has_theme_family(&self, name: &str) -> bool {
        self.families.contains_key(&family_key(name))
    }

    /// Names of all system families (empty until discovery finished).
    pub fn system_family_names(&mut self) -> Vec<String> {
        self.fcx.collection.family_names().map(str::to_owned).collect()
    }

    /// Font bytes and index of the system face that `family` resolves to for `style`.
    fn system_face(&mut self, family: &Family, style: &TextStyle) -> Option<(Blob<u8>, u32)> {
        let name = match family {
            Family::Named(n) => n.to_string(),
            Family::Generic(g) => format!("\u{0}generic:{g:?}"),
        };
        // keyed by every property fontique matches on, exact (normalised) values
        let req = Request {
            weight: style.weight,
            slope: style.slope,
            stretch: style.stretch,
        }
        .normalized();
        let key = (name, req.weight.to_bits(), slope_bits(req.slope), req.stretch.to_bits());
        if let Some(v) = self.system_cache.get(&key) {
            return v.clone();
        }
        if self.system_cache.len() >= SYSTEM_CACHE_CAP {
            self.system_cache.clear();
        }
        let coll = &mut self.fcx.collection;
        let fam = match family {
            Family::Named(n) => coll.family_by_name(n),
            Family::Generic(g) => {
                let id = coll.generic_families(to_generic(*g)).next();
                id.and_then(|id| coll.family(id))
            }
        };
        let r = fam.and_then(|fam| {
            let font = fam.match_font(
                FontWidth::from_percentage(req.stretch),
                to_fontique_style(req.slope),
                FontWeight::new(req.weight),
                true,
            )?;
            let blob = font.load(Some(&mut self.fcx.source_cache))?;
            Some((blob, font.index()))
        });
        self.system_cache.insert(key, r.clone());
        r
    }

    /// CSS cluster matching (ADR 0005 §4.1 steps 1–4) for one style run.
    pub(crate) fn itemize(&mut self, text: &str, range: Range<usize>, style: &TextStyle) -> Vec<Piece> {
        let is_theme = |f: &Family| matches!(f, Family::Named(n) if self.families.contains_key(&family_key(n)));
        if !style.families.iter().any(is_theme) {
            // fast path: no theme family → parley/fontique match the whole stack
            return vec![Piece {
                range,
                choice: Choice::default(),
            }];
        };
        let req = Request {
            weight: style.weight,
            slope: style.slope,
            stretch: style.stretch,
        };
        // per-run context: face-set selection per theme family (descriptors only, step 2)
        let ctx: Vec<FamCtx> = style
            .families
            .iter()
            .map(|f| match f {
                Family::Named(n) if self.families.contains_key(&family_key(n)) => FamCtx::Theme(self.selection(n, req)),
                _ => FamCtx::Other(f.clone()),
            })
            .collect();
        let mut out: Vec<Piece> = Vec::new();
        let mut marker = 0u8;
        let sub = &text[range.clone()];
        for (off, cluster) in sub.grapheme_indices(true) {
            let start = range.start + off;
            let pieces = self.match_cluster(cluster, start, style, &ctx, &mut marker);
            for p in pieces {
                match out.last_mut() {
                    Some(last) if last.choice == p.choice && last.range.end == p.range.start => {
                        last.range.end = p.range.end
                    }
                    _ => out.push(p),
                }
            }
        }
        out
    }

    fn match_cluster(
        &mut self,
        cluster: &str,
        start: usize,
        style: &TextStyle,
        ctx: &[FamCtx],
        marker: &mut u8,
    ) -> Vec<Piece> {
        let whole = start..start + cluster.len();
        for mode in [VsMode::Strict, VsMode::IgnoreSelectors] {
            if let Some(choice) = self.match_whole(cluster, style, ctx, mode) {
                return vec![Piece { range: whole, choice }];
            }
        }
        // rule 4b: longest leading sequence, then each remaining character on its own
        let first = cluster.chars().next().unwrap_or(' ');
        if cluster.chars().count() > 1 {
            let mut best: Option<(usize, Choice)> = None;
            for (i, fc) in ctx.iter().enumerate() {
                if let FamCtx::Other(fam) = fc {
                    if let Some((blob, idx)) = self.system_face(fam, style)
                        && let Some(cov) = FaceCoverage::new(blob.data(), idx)
                    {
                        let n = cov.leading_support(cluster);
                        if n > 0 && best.as_ref().is_none_or(|(b, _)| n > *b) {
                            best = Some((n, Choice::fallback(i)));
                        }
                    }
                    continue;
                }
                let FamCtx::Theme(Some(sel)) = fc else { continue };
                for &face in sel.order.iter() {
                    let f = &self.faces[face];
                    if !f.rule.covers(first) {
                        continue;
                    }
                    let Some(cov) = FaceCoverage::new(f.blob.data(), f.index) else {
                        continue;
                    };
                    let n = cov.leading_support(cluster);
                    if n > 0 && best.as_ref().is_none_or(|(b, _)| n > *b) {
                        best = Some((n, Choice::theme(face, i + 1, &sel.sel)));
                    }
                }
            }
            if let Some((n, choice)) = best
                && n < cluster.len()
            {
                let mut v = vec![Piece {
                    range: start..start + n,
                    choice,
                }];
                for (o, c) in cluster[n..].char_indices() {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    let mut choice = self
                        .match_whole(s, style, ctx, VsMode::IgnoreSelectors)
                        .unwrap_or_else(|| Choice::fallback(0));
                    *marker = marker.wrapping_add(1).max(1);
                    choice.split_marker = *marker;
                    let b = start + n + o;
                    v.push(Piece {
                        range: b..b + c.len_utf8(),
                        choice,
                    });
                }
                return v;
            }
        }
        vec![Piece {
            range: whole,
            choice: Choice::fallback(0),
        }]
    }

    /// Face-set selection for theme family `name`: candidates in check order (reverse
    /// declaration order within the winning set); `unicode-range` is applied per cluster.
    fn selection(&mut self, name: &str, req: Request) -> Option<Arc<RunSel>> {
        let key = (
            family_key(name),
            req.weight.to_bits(),
            slope_bits(req.slope),
            req.stretch.to_bits(),
        );
        if let Some(v) = self.sel_cache.get(&key) {
            return v.clone();
        }
        if self.sel_cache.len() >= SEL_CACHE_CAP {
            self.sel_cache.clear();
        }
        let v = self.families.get(&key.0).and_then(|ids| {
            let descs: Vec<_> = ids.iter().map(|&i| self.faces[i].rule.descriptors).collect();
            let sel = matching::select(&descs, req)?;
            let order: Vec<FaceId> = sel.faces.iter().rev().map(|&k| ids[k]).collect();
            Some(Arc::new(RunSel { order, sel }))
        });
        self.sel_cache.insert(key, v.clone());
        v
    }

    /// Rule 4a support check with a cache for single-character clusters (the common case).
    fn supports(&mut self, blob: &Blob<u8>, index: u32, cluster: &str, mode: VsMode) -> bool {
        let mut it = cluster.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            let key = (blob.id(), index, c as u32);
            if let Some(&v) = self.cov_cache.get(&key) {
                return v;
            }
            let v = FaceCoverage::new(blob.data(), index).is_some_and(|f| f.supports(cluster, mode));
            if self.cov_cache.len() >= COV_CACHE_CAP {
                self.cov_cache.clear();
            }
            self.cov_cache.insert(key, v);
            return v;
        }
        FaceCoverage::new(blob.data(), index).is_some_and(|f| f.supports(cluster, mode))
    }

    fn match_whole(&mut self, cluster: &str, style: &TextStyle, ctx: &[FamCtx], mode: VsMode) -> Option<Choice> {
        let first = cluster.chars().next()?;
        for (i, fc) in ctx.iter().enumerate() {
            match fc {
                FamCtx::Theme(None) => {}
                FamCtx::Theme(Some(sel)) => {
                    for &face in &sel.order {
                        let f = &self.faces[face];
                        if !f.rule.covers(first) {
                            continue;
                        }
                        let (blob, index) = (f.blob.clone(), f.index);
                        if self.supports(&blob, index, cluster, mode) {
                            return Some(Choice::theme(face, i + 1, &sel.sel));
                        }
                    }
                }
                FamCtx::Other(fam) => {
                    if let Some((blob, idx)) = self.system_face(fam, style)
                        && self.supports(&blob, idx, cluster, mode)
                    {
                        return Some(Choice::fallback(i));
                    }
                }
            }
        }
        // no stack entry supports the whole cluster: the caller retries with selectors
        // ignored, then rule 4b, then hands the cluster to parley's own fallback
        None
    }

    /// The parley family list for a choice: the chosen theme face (if any), then the stack from
    /// `start` with theme families removed (they are only reachable through private names).
    pub(crate) fn family_list(&self, style: &TextStyle, choice: &Choice) -> Vec<FontFamilyName<'static>> {
        let mut v = Vec::new();
        if let Some(f) = choice.face {
            v.push(FontFamilyName::Named(self.faces[f].private_name.clone().into()));
        }
        for fam in style.families.iter().skip(choice.start) {
            match fam {
                Family::Named(n) if self.families.contains_key(&family_key(n)) => {}
                Family::Named(n) => v.push(FontFamilyName::Named(n.to_string().into())),
                Family::Generic(g) => v.push(FontFamilyName::Generic(to_generic(*g))),
            }
        }
        v
    }
}

fn slope_bits(s: Slope) -> u32 {
    match s {
        Slope::Normal => 0,
        Slope::Italic => 1,
        Slope::Oblique(a) => 2 + a.to_bits(),
    }
}

/// Face-set selection of one theme family for one request.
pub(crate) struct RunSel {
    order: Vec<FaceId>,
    sel: matching::Selection,
}

/// Per-run view of one stack entry up to the last theme family.
enum FamCtx {
    Theme(Option<Arc<RunSel>>),
    Other(Family),
}

/// Font choice for a piece of a style run.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Choice {
    /// Theme face chosen by cluster matching.
    pub face: Option<FaceId>,
    /// First stack index parley may use after the chosen face.
    pub start: usize,
    /// Matched weight (the request clamped into the face's range): `wght` for variable faces.
    pub wght: Option<f32>,
    /// Matched stretch (request clamped into the face's range): `wdth` for variable faces.
    pub wdth: Option<f32>,
    /// Matched oblique angle in degrees: `slnt` = −angle for variable faces.
    pub oblique: Option<f32>,
    /// Bold synthesis allowed (request ≥ 600, matched set ≤ 500).
    pub synth_bold: bool,
    /// Slant synthesis allowed (italic/oblique requested, matched set upright).
    pub synth_slant: bool,
    /// Non-zero for pieces split off a grapheme by rule 4b: forces a shaping-item boundary.
    pub split_marker: u8,
}

impl Choice {
    fn fallback(start: usize) -> Self {
        Choice {
            start,
            ..Default::default()
        }
    }

    fn theme(face: FaceId, start: usize, sel: &matching::Selection) -> Self {
        Choice {
            face: Some(face),
            start,
            wght: Some(sel.weight),
            wdth: Some(sel.stretch),
            oblique: sel.oblique,
            synth_bold: sel.synthesize_bold,
            synth_slant: sel.synthesize_slant,
            split_marker: 0,
        }
    }
}

/// A byte range of a style run with its font choice.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Piece {
    pub range: Range<usize>,
    pub choice: Choice,
}
