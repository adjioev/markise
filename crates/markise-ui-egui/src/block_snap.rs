//! PROTOTYPE (branch `proto/block-snapshots`, not product code): `:::goals` / `:::question`
//! blocks shown in the Side by Side rendered pane as snapshots made by the
//! `markise-web-render` helper process (system WebKit with the site's CSS/JS).
//!
//! - The UI thread only scans the model for blocks, looks up the cache and paints textures.
//! - A worker thread owns the helper process (spawned lazily, restarted with a pause when it
//!   dies or hangs), reads the site-code folder, builds the block HTML
//!   (`markise_export::render_body` of the inner Markdown inside
//!   `<section data-component data-variant>`), decodes the PNG answers.
//! - The UI sends the worker its whole wish list (visible blocks first) whenever it changes;
//!   the worker always works on the first wish not done yet, so stale wishes (old revisions,
//!   blocks scrolled away) are simply never started.
//! - Cache: (block content, width, scale) → texture, tagged with the code-folder hash; while
//!   a block's new image is pending its previous image stays (no flicker while typing).
//!
//! Configuration: site code is discovered from the document (see [`Site`]);
//! `MARKISE_BLOCK_SITE=<folder>` is the fallback (`blocks/<name>/<name>.{css,js}`,
//! same layout); `MARKISE_WEB_RENDER=<path>` overrides the
//! helper path (default: next to the Markise executable); `MARKISE_BLOCK_LOG=1` logs timings
//! to stderr.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use egui::{ColorImage, TextureHandle, TextureOptions};
use markise_layout::{DocModel, ItemKind};
use serde_json::{Value, json};

/// A `:::name variant` … `:::` block found in the model.
#[derive(Clone, Debug)]
pub struct Block {
    /// Item of the opening `:::name` line.
    pub open: usize,
    /// Item of the closing `:::` line.
    pub close: usize,
    /// Source offset of the opening line (click target).
    pub offset: usize,
    pub name: String,
    /// First argument after the name (`multiple`, `lamps`, a scene preset …).
    pub variant: String,
    /// Everything after the name.
    pub args: String,
    pub inner: String,
    /// Content key (name, variant, inner Markdown).
    pub content: u64,
    /// End of the closing `:::` line (source offset).
    pub end: usize,
}

fn marker_of(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix(":::")?;
    let mut words = rest.split_whitespace();
    let name = words.next()?.to_string();
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let args = words.collect::<Vec<_>>().join(" ");
    Some((name, args))
}

/// Finds the `:::name` blocks of the document (any name; blocks without site code fail in
/// the worker and stay Markdown): an opening paragraph
/// `:::name [variant]` and the next paragraph that is exactly `:::`.
pub fn find_blocks(model: &DocModel, text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(usize, usize, usize, String, String)> = None; // item, start, end, name, variant
    for (i, (start, it)) in model.items.iter_with_start().enumerate() {
        if !matches!(it.kind, ItemKind::Paragraph) || it.len > 64 {
            continue;
        }
        let end = (start + it.len).min(text.len());
        let Some(line) = text.get(start..end).map(str::trim) else {
            continue;
        };
        if !line.starts_with(":::") {
            continue;
        }
        if line == ":::" {
            if let Some((oi, ostart, oend, name, args)) = open.take() {
                let inner = text.get(oend..start).unwrap_or("").trim_matches('\n').to_string();
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&name, &args, &inner).hash(&mut h);
                out.push(Block {
                    open: oi,
                    close: i,
                    offset: ostart,
                    variant: args.split_whitespace().next().unwrap_or("").to_string(),
                    name,
                    args,
                    inner,
                    content: h.finish(),
                    end,
                });
            }
        } else if let Some((name, args)) = marker_of(line) {
            // a new opener before a closer: the previous one was not a block
            open = Some((i, start, end, name, args));
        }
    }
    out
}

/// Markise's current theme as handed to the site code: `data-theme="dark|light"` and
/// `color-scheme` on `<html>`, plus CSS custom properties `--markise-bg`, `--markise-fg`,
/// `--markise-accent`, `--markise-muted`, `--markise-font-size`.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeVars {
    pub dark: bool,
    /// CSS declarations for `<html style>`.
    pub vars: String,
}

impl ThemeVars {
    pub fn from_theme(t: &markise_theme::Theme) -> Self {
        let hex = |c: markise_theme::Rgba| {
            let [r, g, b, a] = c.0;
            if a == 255 {
                format!("#{r:02x}{g:02x}{b:02x}")
            } else {
                format!("rgba({r},{g},{b},{:.3})", a as f32 / 255.0)
            }
        };
        ThemeVars {
            dark: t.dark,
            vars: format!(
                "--markise-bg:{};--markise-fg:{};--markise-accent:{};--markise-muted:{};--markise-font-size:{}px",
                hex(t.page_bg),
                hex(t.text),
                hex(t.link),
                hex(t.meta_text),
                t.font_size
            ),
        }
    }

    pub fn mode(&self) -> &'static str {
        if self.dark { "dark" } else { "light" }
    }
}

/// A request to the worker.
#[derive(Clone, Debug)]
struct Job {
    theme: ThemeVars,
    key: u64,
    name: String,
    variant: String,
    args: String,
    inner: String,
    width: f32,
    scale: f32,
}

enum FromWorker {
    Done {
        key: u64,
        folder: u64,
        image: ColorImage,
        width: f32,
        height: f32,
        info: Value,
    },
    Failed {
        key: u64,
        folder: u64,
        error: String,
    },
    Folder(u64),
}

struct Snap {
    tex: TextureHandle,
    /// Block size in points (= CSS px).
    width: f32,
    height: f32,
    last_used: u64,
}

/// Per-viewer snapshot state.
pub struct BlockSnaps {
    to_worker: Sender<Vec<Job>>,
    from_worker: Receiver<FromWorker>,
    folder: u64,
    /// (content key, folder) → image.
    cache: HashMap<(u64, u64), Snap>,
    failed: HashSet<(u64, u64)>,
    /// Image shown per block ordinal while its current content is pending.
    last_shown: Vec<Option<(u64, u64)>>,
    /// Blocks of the current model.
    pub blocks: Vec<Block>,
    scanned: Option<(usize, u64)>,
    sent: Vec<u64>,
    frame: u64,
    log: bool,
    created: Instant,
    /// First moment each key was wanted (latency of the snapshot pipeline).
    wanted_at: HashMap<u64, Instant>,
    /// Set when every block of the document had its image for the first time.
    all_shown: bool,
    /// Per block of the last frame: its current content's image is shown.
    pub current: Vec<bool>,
    /// PROTOTYPE (proto/block-forms): the block edit dialog.
    pub form: crate::block_edit::FormState,
    /// Width the images are rendered at (frozen while the edit panel is resized).
    key_width: Option<f32>,
    /// Live preview applied to the block list (from the form), and the new-block preview.
    preview_applied: Option<crate::block_edit::Preview>,
    draft_block: Option<Block>,
    last_draft: Option<(u64, u64)>,
    /// The new block's preview image this frame (texture, size in points).
    pub draft_image: Option<(egui::TextureId, egui::Vec2)>,
    /// Palette thumbnails wanted this frame (key, block, width) and the theme/scale of the
    /// last frame they render with.
    thumb_wants: Vec<(u64, Block, f32)>,
    last_theme: Option<ThemeVars>,
    last_scale: f32,
    pending: usize,
}

/// Where the image of a block is painted this frame.
pub struct Placed {
    pub open: usize,
    pub close: usize,
    pub offset: usize,
    /// End of the closing `:::` line.
    pub end: usize,
    pub tex: egui::TextureId,
    pub size: egui::Vec2,
}

fn log_on() -> bool {
    std::env::var_os("MARKISE_BLOCK_LOG").is_some_and(|v| v != "0")
}

impl BlockSnaps {
    /// Starts the worker when the site folder is configured (`MARKISE_BLOCK_SITE`).
    /// `root`: the block library (package cache / dev folder / site), see [`crate::block_lib`].
    pub fn from_env(ctx: &egui::Context, doc: Option<PathBuf>, root: Option<PathBuf>) -> Option<Self> {
        let fallback = std::env::var_os("MARKISE_BLOCK_SITE").map(PathBuf::from);
        let site = Site::new(doc.clone(), fallback, root.clone());
        if log_on() {
            eprintln!("[blocksnap] site root: {:?}; fallback: {:?}", site.root, site.fallback);
        }
        let helper = std::env::var_os("MARKISE_WEB_RENDER").map(PathBuf::from).or_else(|| {
            let exe = std::env::current_exe().ok()?;
            Some(exe.with_file_name("markise-web-render"))
        })?;
        let (to_worker, jobs) = mpsc::channel();
        let (results, from_worker) = mpsc::channel();
        let (events, events_rx) = mpsc::channel();
        let live: LiveStdin = Default::default();
        drop(events_rx);
        let form = crate::block_edit::FormState::new(doc.clone(), root, ctx);
        let ctx = ctx.clone();
        let log = log_on();
        std::thread::Builder::new()
            .name("markise-block-snap".into())
            .spawn(move || {
                let mut w = Worker::new(site, helper, ctx, results, log);
                w.live = live;
                w.events = Some(events);
                w.run(jobs)
            })
            .ok()?;
        Some(BlockSnaps {
            to_worker,
            from_worker,
            folder: 0,
            cache: HashMap::new(),
            failed: HashSet::new(),
            last_shown: Vec::new(),
            blocks: Vec::new(),
            scanned: None,
            sent: Vec::new(),
            frame: 0,
            log,
            created: Instant::now(),
            wanted_at: HashMap::new(),
            all_shown: false,
            current: Vec::new(),
            form,
            key_width: None,
            preview_applied: None,
            draft_block: None,
            last_draft: None,
            draft_image: None,
            thumb_wants: Vec::new(),
            last_theme: None,
            last_scale: 2.0,
            pending: 0,
        })
    }

    /// Blocks of the last frame still waiting for their current image (not failed).
    pub fn pending(&self) -> usize {
        self.pending
    }

    fn key_of(b: &Block, width: f32, scale: f32, theme: &ThemeVars) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            b.content,
            (width * 4.0) as u32,
            (scale * 100.0) as u32,
            theme.dark,
            &theme.vars,
        )
            .hash(&mut h);
        h.finish()
    }

    /// One frame: rescans blocks when the model changed, takes finished snapshots, sends the
    /// wish list and returns the height overrides (item → layout height) and the images to
    /// paint. `visible`: items visible at the last frame (wished first).
    pub fn frame(
        &mut self,
        ctx: &egui::Context,
        model: &std::sync::Arc<DocModel>,
        text: &str,
        revision: u64,
        width: f32,
        scale: f32,
        spacing: f32,
        visible: &[usize],
        theme: &ThemeVars,
    ) -> (HashMap<usize, f32>, Vec<Placed>) {
        self.frame += 1;
        // PROTOTYPE (block forms): while the edit panel is being resized (and shortly after)
        // images keep their old width (scaled); they re-render once the width settles
        let kw = if crate::form_panel::resizing() {
            ctx.request_repaint_after(Duration::from_millis(100));
            self.key_width.unwrap_or(width)
        } else {
            self.key_width = Some(width);
            width
        };
        let id = (std::sync::Arc::as_ptr(model) as usize, revision);
        if self.form.preview != self.preview_applied {
            self.scanned = None;
        }
        if self.scanned != Some(id) {
            let t = Instant::now();
            self.blocks = find_blocks(model, text);
            // live preview of the form: the edited block shows the form's values; a new block
            // gets a synthetic entry (rendered first, placed by the viewer)
            self.preview_applied = self.form.preview.clone();
            self.draft_block = None;
            if let Some(p) = &self.preview_applied
                && let Some(pb) = preview_block(&p.markdown)
            {
                match p.at {
                    crate::block_edit::PreviewAt::Edit(off) => {
                        if let Some(b) = self.blocks.iter_mut().find(|b| b.offset == off) {
                            b.name = pb.name;
                            b.args = pb.args;
                            b.variant = pb.variant;
                            b.inner = pb.inner;
                            b.content = pb.content;
                        }
                    }
                    crate::block_edit::PreviewAt::Insert(..) => self.draft_block = Some(pb),
                }
            }
            self.scanned = Some(id);
            self.last_shown.resize(self.blocks.len(), None);
            if self.log {
                eprintln!(
                    "[blocksnap] scan: {} blocks in {:.2} ms",
                    self.blocks.len(),
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
        }
        // ---- results
        while let Ok(msg) = self.from_worker.try_recv() {
            match msg {
                FromWorker::Done {
                    key,
                    folder,
                    image,
                    width,
                    height,
                    info,
                } => {
                    let t = Instant::now();
                    let tex = ctx.load_texture(format!("blocksnap-{key:x}"), image, TextureOptions::LINEAR);
                    if self.log {
                        let since = self
                            .wanted_at
                            .get(&key)
                            .map_or(f64::NAN, |w| w.elapsed().as_secs_f64() * 1e3);
                        eprintln!(
                            "[blocksnap] image {key:016x}: wanted→received {since:.1} ms, upload {:.2} ms, helper {}",
                            t.elapsed().as_secs_f64() * 1e3,
                            info
                        );
                    }
                    self.cache.insert(
                        (key, folder),
                        Snap {
                            tex,
                            width,
                            height,
                            last_used: self.frame,
                        },
                    );
                }
                FromWorker::Failed { key, folder, error } => {
                    if self.log {
                        eprintln!("[blocksnap] failed {key:016x}: {error}");
                    }
                    self.failed.insert((key, folder));
                }
                FromWorker::Folder(h) => {
                    if self.log && self.folder != 0 {
                        eprintln!("[blocksnap] site code changed: re-rendering");
                    }
                    self.folder = h;
                }
            }
        }
        // ---- wishes: visible blocks first, then the rest in document order
        let mut order: Vec<usize> = (0..self.blocks.len()).collect();
        let is_vis = |b: &Block| visible.iter().any(|&v| v >= b.open && v <= b.close);
        order.sort_by_key(|&i| !is_vis(&self.blocks[i]));
        let mut jobs = Vec::new();
        let mut keys = Vec::new();
        if let Some(b) = &self.draft_block {
            let key = Self::key_of(b, kw, scale, theme);
            if self.folder != 0
                && !self.cache.contains_key(&(key, self.folder))
                && !self.failed.contains(&(key, self.folder))
            {
                keys.push(key);
                jobs.push(Job {
                    theme: theme.clone(),
                    key,
                    name: b.name.clone(),
                    variant: b.variant.clone(),
                    args: b.args.clone(),
                    inner: b.inner.clone(),
                    width: kw,
                    scale,
                });
            }
        }
        for &i in &order {
            let b = &self.blocks[i];
            let key = Self::key_of(b, kw, scale, theme);
            if self.folder == 0
                || (!self.cache.contains_key(&(key, self.folder)) && !self.failed.contains(&(key, self.folder)))
            {
                if self.log && !self.wanted_at.contains_key(&key) {
                    eprintln!(
                        "[blocksnap] want block #{i} {}: content {:016x} width {width} scale {scale} folder {:x}",
                        b.name, b.content, self.folder
                    );
                }
                self.wanted_at.entry(key).or_insert_with(Instant::now);
                keys.push(key);
                jobs.push(Job {
                    theme: theme.clone(),
                    key,
                    name: b.name.clone(),
                    variant: b.variant.clone(),
                    args: b.args.clone(),
                    inner: b.inner.clone(),
                    width: kw,
                    scale,
                });
            }
        }
        // palette thumbnails (after the document's blocks)
        for (key, b, w) in std::mem::take(&mut self.thumb_wants) {
            if self.folder != 0
                && !self.cache.contains_key(&(key, self.folder))
                && !self.failed.contains(&(key, self.folder))
            {
                keys.push(key);
                jobs.push(Job {
                    theme: theme.clone(),
                    key,
                    name: b.name.clone(),
                    variant: b.variant.clone(),
                    args: b.args.clone(),
                    inner: b.inner.clone(),
                    width: w,
                    scale,
                });
            }
        }
        self.last_theme = Some(theme.clone());
        self.last_scale = scale;
        if keys != self.sent {
            self.sent = keys;
            let _ = self.to_worker.send(jobs);
        }
        // ---- placement
        let mut over = HashMap::new();
        let mut placed = Vec::new();
        let mut missing = 0;
        let mut pending = 0;
        self.current.clear();
        for (i, b) in self.blocks.iter().enumerate() {
            let key = Self::key_of(b, kw, scale, theme);
            let cur = (key, self.folder);
            let shown = if self.cache.contains_key(&cur) {
                if self.log && self.last_shown[i] != Some(cur) {
                    if let Some(w) = self.wanted_at.get(&key) {
                        eprintln!(
                            "[blocksnap] block #{i} ({}) shown {:.1} ms after it was wanted",
                            b.name,
                            w.elapsed().as_secs_f64() * 1e3
                        );
                    }
                }
                self.last_shown[i] = Some(cur);
                self.current.push(true);
                Some(cur)
            } else {
                missing += 1;
                if !self.failed.contains(&cur) {
                    pending += 1;
                }
                self.current.push(false);
                // stale image of this block while the new one renders (same width only:
                // a resize shows Markdown rather than a stretched picture)
                self.last_shown[i].filter(|k| self.cache.contains_key(k) && !self.failed.contains(&cur))
            };
            let Some(k) = shown else { continue };
            let snap = self.cache.get_mut(&k).expect("cached");
            snap.last_used = self.frame;
            // a stale image of another width is scaled to the column until its new one arrives
            let h = snap.height * width / snap.width.max(1.0);
            over.insert(b.open, spacing + h);
            for j in b.open + 1..=b.close {
                over.insert(j, 0.0);
            }
            placed.push(Placed {
                open: b.open,
                close: b.close,
                offset: b.offset,
                end: b.end,
                tex: snap.tex.id(),
                size: egui::vec2(width, h),
            });
        }
        self.pending = if self.folder == 0 { self.blocks.len() } else { pending };
        // the new block's preview (its last image while the next one renders)
        self.draft_image = None;
        if let Some(b) = &self.draft_block {
            let cur = (Self::key_of(b, kw, scale, theme), self.folder);
            if self.cache.contains_key(&cur) {
                self.last_draft = Some(cur);
            }
            if let Some(k) = self.last_draft.filter(|k| self.cache.contains_key(k))
                && let Some(snap) = self.cache.get_mut(&k)
            {
                snap.last_used = self.frame;
                let h = snap.height * width / snap.width.max(1.0);
                self.draft_image = Some((snap.tex.id(), egui::vec2(width, h)));
            }
        } else {
            self.last_draft = None;
        }
        if missing == 0 && !self.blocks.is_empty() && !self.all_shown {
            self.all_shown = true;
            if self.log {
                eprintln!(
                    "[blocksnap] all {} blocks shown {:.1} ms after the first Side by Side frame",
                    self.blocks.len(),
                    self.created.elapsed().as_secs_f64() * 1e3
                );
            }
        }
        // ---- eviction: keep what is shown plus a margin of recent images
        let limit = 3 * self.blocks.len() + 16;
        if self.cache.len() > limit {
            let mut by_age: Vec<((u64, u64), u64)> = self.cache.iter().map(|(k, s)| (*k, s.last_used)).collect();
            by_age.sort_by_key(|(_, u)| *u);
            for (k, u) in by_age.into_iter().take(self.cache.len() - limit) {
                if u != self.frame {
                    self.cache.remove(&k);
                }
            }
        }
        (over, placed)
    }
}

// ---------------------------------------------------------------- worker

/// The site code of the snapshot components.
///
/// Discovery (owner convention for the prototype): from the document's folder, walk up to the
/// first folder holding `components/blocks.json` = the site root; a component's code is
/// `<root>/blocks/<name>/<name>.css` and `.js` (EDS-style). A component without such files
/// there (or a document outside any site) uses the fallback folder (`MARKISE_BLOCK_SITE`, same
/// `blocks/<name>/…` layout). The code hash covers the resolved files' contents.
struct Site {
    root: Option<PathBuf>,
    /// Folder of the document (relative picture paths resolve against it).
    doc_dir: Option<PathBuf>,
    fallback: Option<PathBuf>,
    stamp: Vec<(PathBuf, Option<SystemTime>, u64)>,
    /// component → (css, js, folder it came from)
    code: HashMap<String, (String, String, PathBuf)>,
    hash: u64,
    checked: Option<Instant>,
}

/// The site root of a document: the nearest ancestor folder with `components/blocks.json`.
pub fn site_root(doc: &std::path::Path) -> Option<PathBuf> {
    let mut dir = doc.parent();
    while let Some(d) = dir {
        if d.join("components").join("blocks.json").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

impl Site {
    fn new(doc: Option<PathBuf>, fallback: Option<PathBuf>, root: Option<PathBuf>) -> Self {
        Site {
            root,
            doc_dir: doc.as_deref().and_then(|d| d.parent()).map(|d| d.to_path_buf()),
            fallback,
            stamp: Vec::new(),
            code: HashMap::new(),
            hash: 0,
            checked: None,
        }
    }

    /// The folder holding a component's code: the site root's if it has any file for it.
    fn component_dir(&self, name: &str) -> Option<PathBuf> {
        [self.root.as_ref(), self.fallback.as_ref()]
            .into_iter()
            .flatten()
            .map(|r| r.join("blocks").join(name))
            .find(|d| d.join(format!("{name}.css")).is_file() || d.join(format!("{name}.js")).is_file())
    }

    /// Re-resolves and re-reads the code (at most every 0.7 s); `true` when it changed.
    fn refresh(&mut self) -> bool {
        if self.checked.is_some_and(|t| t.elapsed() < Duration::from_millis(700)) {
            return false;
        }
        self.checked = Some(Instant::now());
        let mut stamp = Vec::new();
        let mut dirs = Vec::new();
        // every `blocks/<name>/` folder of the site root and of the fallback
        let mut names: Vec<String> = [self.root.as_ref(), self.fallback.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|r| std::fs::read_dir(r.join("blocks")).ok())
            .flat_map(|rd| rd.flatten())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        names.dedup();
        for name in &names {
            let Some(d) = self.component_dir(name) else { continue };
            for ext in ["css", "js"] {
                let f = d.join(format!("{name}.{ext}"));
                let md = std::fs::metadata(&f).ok();
                stamp.push((
                    f,
                    md.as_ref().and_then(|m| m.modified().ok()),
                    md.map_or(0, |m| m.len()),
                ));
            }
            dirs.push((name.to_string(), d));
        }
        if stamp == self.stamp && self.hash != 0 {
            return false;
        }
        self.code.clear();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (name, d) in dirs {
            let css = absolute_css_urls(
                &std::fs::read_to_string(d.join(format!("{name}.css"))).unwrap_or_default(),
                &d,
            );
            let js = std::fs::read_to_string(d.join(format!("{name}.js"))).unwrap_or_default();
            (&name, &css, &js).hash(&mut h);
            self.code.insert(name, (css, js, d));
        }
        self.stamp = stamp;
        self.hash = h.finish() | 1;
        true
    }

    fn get(&self, name: &str) -> Option<&(String, String, PathBuf)> {
        self.code.get(name)
    }
}

/// The running helper's stdin, shared with the edit dialog (frames are written whole under
/// the lock).
pub type LiveStdin = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<ChildStdin>>>>>;

struct Helper {
    child: Child,
    stdin: std::sync::Arc<std::sync::Mutex<ChildStdin>>,
    frames: Receiver<std::io::Result<(Value, Vec<u8>)>>,
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_part(r: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > 256 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn write_frame(w: &mut impl Write, json: &Value) -> std::io::Result<()> {
    let j = serde_json::to_vec(json)?;
    w.write_all(&(j.len() as u32).to_le_bytes())?;
    w.write_all(&j)?;
    w.write_all(&0u32.to_le_bytes())?;
    w.flush()
}

struct Worker {
    site: Site,
    helper_path: PathBuf,
    helper: Option<Helper>,
    /// Earliest next spawn attempt (pause after a crash or hang).
    next_spawn: Instant,
    ctx: egui::Context,
    out: Sender<FromWorker>,
    done: HashSet<(u64, u64)>,
    log: bool,
    live: LiveStdin,
    /// Unsolicited helper frames (`event`: the edit dialog) go here.
    events: Option<Sender<Value>>,
}

impl Worker {
    fn new(site: Site, helper_path: PathBuf, ctx: egui::Context, out: Sender<FromWorker>, log: bool) -> Self {
        Worker {
            site,
            helper_path,
            helper: None,
            next_spawn: Instant::now(),
            ctx,
            out,
            done: HashSet::new(),
            log,
            live: Default::default(),
            events: None,
        }
    }

    fn send(&self, m: FromWorker) {
        let _ = self.out.send(m);
        self.ctx.request_repaint();
    }

    fn run(mut self, jobs: Receiver<Vec<Job>>) {
        let mut want: Vec<Job> = Vec::new();
        loop {
            if self.site.refresh() {
                if self.log {
                    eprintln!("[blocksnap] site code (re)loaded, hash {:x}", self.site.hash);
                }
                self.done.clear();
                self.send(FromWorker::Folder(self.site.hash));
            }
            let folder = self.site.hash;
            let next = want.iter().find(|j| !self.done.contains(&(j.key, folder))).cloned();
            // newest wish list wins; wait only when idle
            let msg = if next.is_some() {
                jobs.try_recv()
                    .map_err(|e| matches!(e, mpsc::TryRecvError::Disconnected))
            } else {
                jobs.recv_timeout(Duration::from_millis(700))
                    .map_err(|e| matches!(e, RecvTimeoutError::Disconnected))
            };
            match msg {
                Ok(mut w) => {
                    while let Ok(newer) = jobs.try_recv() {
                        w = newer;
                    }
                    want = w;
                    continue;
                }
                Err(true) => return, // the viewer is gone
                Err(false) => {}
            }
            let Some(job) = next else { continue };
            self.done.insert((job.key, folder));
            match self.render(&job) {
                Ok((image, height, info)) => self.send(FromWorker::Done {
                    key: job.key,
                    folder,
                    image,
                    width: job.width,
                    height,
                    info,
                }),
                Err(error) => self.send(FromWorker::Failed {
                    key: job.key,
                    folder,
                    error,
                }),
            }
        }
    }

    fn spawn(&mut self) -> Result<(), String> {
        if self.helper.is_some() {
            return Ok(());
        }
        let now = Instant::now();
        if now < self.next_spawn {
            std::thread::sleep(self.next_spawn - now);
        }
        self.next_spawn = Instant::now() + Duration::from_secs(2);
        let t = Instant::now();
        // read access for the page: the site root (pictures and fonts of the site), else the
        // document's folder
        // (block packages: the library lives in the cache, the pictures next to the document, so
        // the read-access folder is their common ancestor)
        let root = match (self.site.root.clone(), self.site.doc_dir.clone()) {
            (Some(r), Some(d)) => r
                .components()
                .zip(d.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
            (r, d) => r.or(d).unwrap_or_default(),
        };
        let mut child = Command::new(&self.helper_path)
            .env("MARKISE_WR_ROOT", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.helper_path.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, frames) = mpsc::channel();
        let events = self.events.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let f = read_part(&mut r).and_then(|j| {
                    let b = read_part(&mut r)?;
                    let v: Value = serde_json::from_slice(&j).map_err(std::io::Error::other)?;
                    Ok((v, b))
                });
                if let (Ok((v, _)), Some(ev)) = (&f, &events)
                    && v.get("event").is_some()
                {
                    let _ = ev.send(v.clone());
                    ctx.request_repaint();
                    continue;
                }
                let end = f.is_err();
                if tx.send(f).is_err() || end {
                    return;
                }
            }
        });
        let stdin = std::sync::Arc::new(std::sync::Mutex::new(stdin));
        *self.live.lock().unwrap_or_else(|e| e.into_inner()) = Some(stdin.clone());
        let h = Helper { child, stdin, frames };
        match h.frames.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok((v, _))) if v["ready"] == true => {
                if self.log {
                    eprintln!(
                        "[blocksnap] helper ready: spawn→ready {:.1} ms (helper's own startup {} ms)",
                        t.elapsed().as_secs_f64() * 1e3,
                        v["ms_startup"]
                    );
                }
                self.helper = Some(h);
                Ok(())
            }
            other => Err(format!("helper did not get ready: {other:?}")),
        }
    }

    fn render(&mut self, job: &Job) -> Result<(ColorImage, f32, Value), String> {
        let Some((css, js, dir)) = self.site.get(&job.name).cloned() else {
            return Err(format!(
                "no site code for `{}` (no blocks/{0}/ in the site root or the fallback)",
                job.name
            ));
        };
        if self.log {
            eprintln!("[blocksnap] {} code from {}", job.name, dir.display());
        }
        let t = Instant::now();
        let doc = markise_md::parse(&job.inner);
        let body = markise_export::render_body(&doc, &markise_export::HtmlOptions::default());
        let attr = |s: &str| html_escape::encode_double_quoted_attribute(s).to_string();
        let html = format!(
            "<section data-component=\"{}\" data-variant=\"{}\" data-args=\"{}\">\n{body}</section>",
            attr(&job.name),
            attr(&job.variant),
            attr(&job.args)
        );
        if let Some(dir) = std::env::var_os("MARKISE_BLOCK_DUMP").map(PathBuf::from) {
            let _ = std::fs::write(dir.join(format!("{}-{:016x}.html", job.name, job.key)), &html);
        }
        let ms_html = t.elapsed().as_secs_f64() * 1e3;
        let req = json!({
            "id": job.key,
            "width": job.width,
            "scale": job.scale,
            "html": html,
            "css": css,
            "js": js,
            "theme": job.theme.mode(),
            "vars": job.theme.vars,
            "base": self.site.doc_dir.as_deref().map(|d| file_url(d, true)).unwrap_or_default(),
        });
        let mut last = String::new();
        for _attempt in 0..2 {
            self.spawn()?;
            let h = self.helper.as_mut().expect("helper");
            let t_ipc = Instant::now();
            let wrote = write_frame(&mut *h.stdin.lock().unwrap_or_else(|e| e.into_inner()), &req);
            if let Err(e) = wrote {
                last = format!("write: {e}");
                self.helper = None;
                continue;
            }
            match h.frames.recv_timeout(Duration::from_secs(8)) {
                Ok(Ok((v, png))) => {
                    let ms_rt = t_ipc.elapsed().as_secs_f64() * 1e3;
                    if v["ok"] != true {
                        return Err(v["error"].as_str().unwrap_or("helper error").to_string());
                    }
                    let t_dec = Instant::now();
                    let image = decode_png(&png)?;
                    let mut info = v.clone();
                    info["ms_html"] = json!(ms_html);
                    info["ms_roundtrip"] = json!(ms_rt);
                    info["ms_decode"] = json!(t_dec.elapsed().as_secs_f64() * 1e3);
                    let height = v["height"].as_f64().unwrap_or(0.0) as f32;
                    return Ok((image, height, info));
                }
                Ok(Err(e)) => {
                    last = format!("helper died: {e}");
                    self.helper = None;
                }
                Err(_) => {
                    last = "helper timed out".into();
                    self.helper = None; // killed on drop, restarted after a pause
                }
            }
        }
        Err(last)
    }
}

/// `file://` URL of a path (directories end with `/`).
fn file_url(p: &std::path::Path, dir: bool) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    if dir && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Rewrites relative `url(...)` references of a block's CSS to absolute `file://` URLs of
/// its folder (the CSS is injected as text, so the page would resolve them against the
/// document instead).
fn absolute_css_urls(css: &str, dir: &std::path::Path) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("url(") {
        out.push_str(&rest[..i + 4]);
        rest = &rest[i + 4..];
        let Some(end) = rest.find(')') else { break };
        let raw = rest[..end].trim();
        let quote = raw.chars().next().filter(|c| *c == '"' || *c == '\'');
        let inner = raw.trim_matches(|c| c == '"' || c == '\'');
        let absolute = inner.is_empty() || inner.starts_with('#') || inner.starts_with('/') || inner.contains(':');
        if absolute {
            out.push_str(&rest[..end]);
        } else {
            let q = quote.map(String::from).unwrap_or_else(|| "\"".into());
            out.push_str(&format!("{q}{}{q}", file_url(&dir.join(inner), false)));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn decode_png(bytes: &[u8]) -> Result<ColorImage, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png size")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let buf = &buf[..info.buffer_size()];
    let img = match info.color_type {
        png::ColorType::Rgba => ColorImage::from_rgba_unmultiplied([w, h], buf),
        png::ColorType::Rgb => ColorImage::from_rgb([w, h], buf),
        png::ColorType::GrayscaleAlpha => {
            let rgba: Vec<u8> = buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect();
            ColorImage::from_rgba_unmultiplied([w, h], &rgba)
        }
        png::ColorType::Grayscale => {
            let rgb: Vec<u8> = buf.iter().flat_map(|&g| [g, g, g]).collect();
            ColorImage::from_rgb([w, h], &rgb)
        }
        other => return Err(format!("png color type {other:?}")),
    };
    Ok(img)
}

// ---------------------------------------------------------------- measurement script

/// PROTOTYPE measurement driver (`MARKISE_PROTO_SCRIPT=1`, Side by Side only): once every block
/// is shown it types into the first `:::goals` and the first `:::question` block through the
/// source pane (the same path as a keystroke), measures keystroke → new image shown, a fast
/// typing burst, frame times, takes window screenshots into `MARKISE_PROTO_SHOTS` (folder)
/// and, with `MARKISE_PROTO_EXIT=1`, exits.
#[derive(Default)]
pub struct ProtoScript {
    step: usize,
    since: Option<Instant>,
    typed_at: Option<Instant>,
    lat: Vec<(String, f64)>,
    frames: Vec<f64>,
    burst_frames: Vec<f64>,
    started: bool,
    /// Tour mode: index of the next block to screenshot.
    tour: usize,
}

const SLOW_STEPS: usize = 8;
const BURST: usize = 12;

impl ProtoScript {
    pub fn enabled() -> bool {
        std::env::var_os("MARKISE_PROTO_SCRIPT").is_some_and(|v| v != "0") || Self::tour_on()
    }

    /// `MARKISE_PROTO_TOUR=1`: no typing; once every block is shown, scrolls to each block in
    /// turn, screenshots the window (`MARKISE_PROTO_SHOTS`, `<doc>-<theme>-NN.png`) and exits.
    fn tour_on() -> bool {
        std::env::var_os("MARKISE_PROTO_TOUR").is_some_and(|v| v != "0")
    }

    fn tour_frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
        ctx.request_repaint_after(Duration::from_millis(16));
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let now = Instant::now();
        // wait for every block that has site code (failed ones stay Markdown)
        let settled = !snaps.blocks.is_empty() && snaps.pending() == 0;
        let since = *self.since.get_or_insert(now);
        if !settled && now - since < Duration::from_secs(20) {
            return;
        }
        if now - since < Duration::from_millis(700) {
            return;
        }
        let n = snaps.blocks.len();
        if self.tour > n {
            std::process::exit(0);
        }
        if self.tour == n {
            // last shot was requested in the previous step; give it time, then exit
            if now - since > Duration::from_millis(800) {
                std::process::exit(0);
            }
            return;
        }
        let b = snaps.blocks[self.tour].clone();
        if !self.started {
            viewer.ed.view.scroll_to_item(b.open);
            viewer.ed.view.scroll_by(-40.0);
            self.started = true;
            self.since = Some(now);
            return;
        }
        let stem = viewer
            .ed
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "doc".into());
        let theme = std::env::var("MARKISE_PROTO_THEME").unwrap_or_else(|_| "system".into());
        self.shot(ctx, &format!("{stem}-{theme}-{:02}-{}", self.tour, b.name));
        self.tour += 1;
        self.started = false;
        self.since = Some(now);
    }

    /// Insert position in block `b`: end of the first list item's text (before an answer label).
    fn target(text: &str, b: &Block) -> Option<usize> {
        let block = text.get(b.offset..)?;
        let mut off = b.offset;
        for line in block.lines() {
            let t = line.trim_start();
            if t.starts_with("- ") {
                let end = match line.find(" {#") {
                    Some(i) => i,
                    None => line.trim_end().len(),
                };
                return Some(off + end);
            }
            off += line.len() + 1;
        }
        None
    }

    pub fn shot(&mut self, ctx: &egui::Context, name: &str) {
        let Some(dir) = std::env::var_os("MARKISE_PROTO_SHOTS").map(PathBuf::from) else {
            return;
        };
        let path = dir.join(format!("{name}.png"));
        let cb = egui::ScreenshotCallback::new(move |img: std::sync::Arc<ColorImage>| {
            let res = (|| -> Result<(), String> {
                let f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.size[0] as u32, img.size[1] as u32);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                let mut w = enc.write_header().map_err(|e| e.to_string())?;
                let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
                w.write_image_data(&raw).map_err(|e| e.to_string())
            })();
            eprintln!("[proto] screenshot {} {:?}", path.display(), res);
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(cb));
    }

    /// One frame. `frame_ms`: CPU time of the previous Side by Side frame.
    pub fn frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer, frame_ms: Option<f64>) {
        if Self::tour_on() {
            return self.tour_frame(ctx, viewer);
        }
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let all = !snaps.current.is_empty() && snaps.current.iter().all(|&c| c);
        if let Some(ms) = frame_ms
            && self.started
        {
            if (SLOW_STEPS * 2..SLOW_STEPS * 2 + 2).contains(&self.step) {
                self.burst_frames.push(ms);
            } else {
                self.frames.push(ms);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let text = viewer.ed.session.text().to_string();
        let goals = snaps.blocks.iter().position(|b| b.name == "goals");
        let question = snaps.blocks.iter().position(|b| b.name == "question");
        let now = Instant::now();
        if !self.started {
            if all {
                let t = *self.since.get_or_insert(now);
                if now - t < Duration::from_millis(20)
                    && let Some(b) = snaps.blocks.first()
                {
                    let open = b.open;
                    viewer.ed.view.scroll_to_item(open);
                    viewer.ed.view.scroll_by(-120.0);
                }
                if now - t > Duration::from_millis(1500) {
                    self.started = true;
                    self.since = Some(now);
                    self.shot(ctx, "01-initial");
                }
            }
            return;
        }
        let since = self.since.unwrap_or(now);
        let type_into = |viewer: &mut crate::viewer::DocViewer, bi: usize, s: &str| {
            let Some(b) = viewer.snaps.as_ref().and_then(|x| x.blocks.get(bi)).cloned() else {
                return;
            };
            let Some(pos) = Self::target(&text, &b) else { return };
            viewer.ed.source_set_selection(markise_doc::Selection::caret(pos));
            let _ = viewer.ed.source_insert(s, false);
            viewer.ed.view.scroll_to_item(b.open);
            viewer.ed.view.scroll_by(-60.0);
        };
        let s = self.step;
        if s < SLOW_STEPS * 2 {
            // slow typing: one keystroke, wait for its image, pause, next
            let (bi, label) = if s < SLOW_STEPS {
                (goals, "goals")
            } else {
                (question, "question")
            };
            let Some(bi) = bi else {
                self.step += 1;
                return;
            };
            match self.typed_at {
                None if now - since > Duration::from_millis(300) => {
                    self.typed_at = Some(Instant::now());
                    type_into(viewer, bi, if label == "goals" { "!" } else { "?" });
                }
                Some(t) if viewer.snaps.as_ref().is_some_and(|x| x.current.get(bi) == Some(&true)) => {
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    eprintln!("[proto] keystroke→image ({label}) {ms:.1} ms");
                    self.lat.push((label.into(), ms));
                    self.typed_at = None;
                    self.since = Some(now);
                    self.step += 1;
                    if self.step == SLOW_STEPS {
                        self.shot(ctx, "02-goal-edited");
                    } else if self.step == SLOW_STEPS * 2 {
                        self.shot(ctx, "03-answer-edited");
                    }
                }
                _ => {}
            }
        } else if s == SLOW_STEPS * 2 {
            // burst: BURST keystrokes 40 ms apart into the question
            let Some(bi) = question else {
                self.step += 2;
                return;
            };
            let n = self.lat.iter().filter(|(l, _)| l == "burst-key").count();
            if n < BURST {
                if now - since >= Duration::from_millis(40) {
                    type_into(viewer, bi, "x");
                    self.lat.push(("burst-key".into(), 0.0));
                    self.since = Some(now);
                    self.typed_at = Some(now);
                }
            } else {
                self.step += 1;
            }
        } else if s == SLOW_STEPS * 2 + 1 {
            let bi = question.unwrap_or(0);
            if let Some(t) = self.typed_at
                && viewer.snaps.as_ref().is_some_and(|x| x.current.get(bi) == Some(&true))
            {
                let ms = t.elapsed().as_secs_f64() * 1e3;
                eprintln!("[proto] burst: last keystroke→final image {ms:.1} ms");
                self.lat.push(("burst-final".into(), ms));
                self.typed_at = None;
                self.since = Some(now);
                self.step += 1;
            }
        } else if s == SLOW_STEPS * 2 + 2 {
            if now - since > Duration::from_millis(800) {
                self.report();
                self.step += 1;
                self.since = Some(now);
            }
        } else if s == SLOW_STEPS * 2 + 3
            && now - since > Duration::from_millis(1200)
            && std::env::var_os("MARKISE_PROTO_EXIT").is_some_and(|v| v != "0")
        {
            std::process::exit(0);
        }
    }

    fn report(&self) {
        let stats = |v: &mut Vec<f64>| -> String {
            if v.is_empty() {
                return "n/a".into();
            }
            v.sort_by(|a, b| a.total_cmp(b));
            let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
            format!(
                "n={} p50={:.1} p90={:.1} p99={:.1} max={:.1} ms",
                v.len(),
                p(0.5),
                p(0.9),
                p(0.99),
                v[v.len() - 1]
            )
        };
        for label in ["goals", "question"] {
            let mut v: Vec<f64> = self.lat.iter().filter(|(l, _)| l == label).map(|x| x.1).collect();
            eprintln!("[proto] keystroke→image {label}: {}", stats(&mut v));
        }
        eprintln!(
            "[proto] Side by Side frame CPU (typing + snapshots arriving): {}",
            stats(&mut self.frames.clone())
        );
        eprintln!(
            "[proto] Side by Side frame CPU during the burst: {}",
            stats(&mut self.burst_frames.clone())
        );
    }
}

impl BlockSnaps {
    /// PROTOTYPE (block palette): the rendered thumbnail of a preset at `width` (CSS px), or
    /// None while it renders (it is wished for with the next frame's jobs).
    pub fn thumb(&mut self, md: &str, width: f32) -> Option<(egui::TextureId, egui::Vec2)> {
        let theme = self.last_theme.clone()?;
        let b = preview_block(md)?;
        let key = Self::key_of(&b, width, self.last_scale, &theme);
        if let Some(snap) = self.cache.get_mut(&(key, self.folder)) {
            snap.last_used = self.frame;
            return Some((snap.tex.id(), egui::vec2(snap.width, snap.height)));
        }
        if !self.thumb_wants.iter().any(|(k, _, _)| *k == key) {
            self.thumb_wants.push((key, b, width));
        }
        None
    }
}

/// A block entry for a preview's Markdown (`:::name args` … `:::`).
fn preview_block(md: &str) -> Option<Block> {
    let b = markise_engine::block_form::block_at(md, 0)?;
    let inner = md.get(b.inner.clone()).unwrap_or("").trim_matches('\n').to_string();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&b.name, &b.args, &inner).hash(&mut h);
    Some(Block {
        open: usize::MAX,
        close: usize::MAX,
        offset: usize::MAX,
        variant: b.args.split_whitespace().next().unwrap_or("").to_string(),
        name: b.name,
        args: b.args,
        inner,
        content: h.finish(),
        end: usize::MAX,
    })
}
