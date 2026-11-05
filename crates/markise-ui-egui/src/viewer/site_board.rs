//! A document's pictures of blocks drawn by their site's code (ADR 0023): between the layout, which
//! asks without blocking, and the web client, which starts a helper only when a block on screen
//! needs one. Exists only for a package in use. Pictures are textures, bounded together.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use markise_engine::block_page::BlockPage;
use markise_engine::block_view::PackageViews;
use markise_layout::{SitePicture, SiteQuery, SiteShot};
use markise_package::InstalledPackage;
use markise_theme::{Rgba, Theme};
use markise_web_render::client::{MAX_WANTED, Spawner};
use markise_web_render::{Availability, Client, Job, Limits, Outcome, ProcessSpawner, Shot, ShotCache};

/// What all pictures of a document may take, in bytes of RGBA. Eviction is by least recent use, and
/// a picture gone comes again when it is next on screen.
const MEMORY_LIMIT: usize = 256 << 20;
/// Pictures made textures per frame: each is a copy on the UI thread.
const UPLOADS_PER_FRAME: usize = 2;
/// Blocks that failed, and those known to be wanted, remembered at most.
const REMEMBERED: usize = 1024;

/// How helpers are made: a process, or a test's.
pub type HelperSource = Arc<dyn Fn() -> Option<Box<dyn Spawner>> + Send + Sync>;

/// The helper next to this program, where one is shipped. Elsewhere, blocks stay cards.
pub fn system_web_helpers() -> Option<HelperSource> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let program = std::env::current_exe().ok()?.parent()?.join("markise-web-helper");
    program.is_file().then(|| -> HelperSource {
        Arc::new(move || {
            Some(Box::new(ProcessSpawner {
                program: program.clone(),
            }) as Box<dyn Spawner>)
        })
    })
}

struct Picture {
    texture: TextureHandle,
    picture: SitePicture,
}

#[derive(Default)]
struct Asked {
    jobs: HashMap<u64, Job>,
    /// What is on screen, or about to be, this frame.
    seen: Vec<u64>,
    /// What the client was last told.
    told: Vec<u64>,
    failed: HashMap<u64, Arc<str>>,
    /// The newest picture of each block, by its identity: it stands in for the next one.
    latest: HashMap<u64, u64>,
    /// A key was seen whose job is gone: the view has to ask again.
    lost_job: bool,
}

pub struct SiteBoard {
    client: Option<Client>,
    views: Arc<PackageViews>,
    package: InstalledPackage,
    folder: Option<PathBuf>,
    session: String,
    language: Mutex<String>,
    asked: Mutex<Asked>,
    pictures: Mutex<ShotCache<Picture>>,
    /// Come from the client and not yet textures.
    waiting: Mutex<Vec<(u64, Shot)>>,
    was_available: Mutex<bool>,
}

fn css_color(c: Rgba) -> String {
    let [r, g, b, a] = c.0;
    format!("rgba({r},{g},{b},{:.3})", a as f32 / 255.0)
}

/// What a page gets of the theme (ADR 0023).
fn theme_vars(t: &Theme) -> Vec<(String, String)> {
    let var = |name: &str, value: String| (format!("--markise-{name}"), value);
    vec![
        var("background", css_color(t.page_bg)),
        var("text", css_color(t.text)),
        var("link", css_color(t.link)),
        var("accent", css_color(t.chrome.accent)),
        var("selection", css_color(t.chrome.selection_bg)),
        var("font-size", format!("{}px", t.font_size)),
    ]
}

impl SiteBoard {
    /// `helpers`: `None` where nothing can draw. Nothing starts here: a helper comes when a block on
    /// screen asks for one.
    pub fn new(
        views: Arc<PackageViews>,
        package: InstalledPackage,
        folder: Option<PathBuf>,
        helpers: Option<&HelperSource>,
        ctx: &Context,
    ) -> SiteBoard {
        let wake = ctx.clone();
        let client = helpers.and_then(|h| h()).map(|spawner| {
            let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || wake.request_repaint());
            Client::new(spawner, Limits::default(), wake)
        });
        // no storage shared between packages, and no document's data carried into another's blocks
        let mut h = DefaultHasher::new();
        folder.hash(&mut h);
        let session = format!("{}-{:016x}", package.sha256, h.finish());
        SiteBoard {
            client,
            views,
            package,
            folder,
            session,
            language: Mutex::default(),
            asked: Mutex::default(),
            pictures: Mutex::new(ShotCache::new(MEMORY_LIMIT)),
            waiting: Mutex::default(),
            was_available: Mutex::new(true),
        }
    }

    /// Whether this board is the one for that package and folder.
    pub fn is_for(&self, package: &InstalledPackage, folder: Option<&PathBuf>) -> bool {
        self.package.sha256 == package.sha256 && self.folder.as_ref() == folder
    }

    /// Whether anything can draw here, as far as is known.
    pub fn usable(&self) -> bool {
        self.client
            .as_ref()
            .is_some_and(|c| c.availability() != Availability::Unavailable)
    }

    /// Something asked for is still to come.
    pub fn is_busy(&self) -> bool {
        self.usable()
            && (!self.asked.lock().unwrap_or_else(|e| e.into_inner()).told.is_empty()
                || !self.waiting.lock().unwrap_or_else(|e| e.into_inner()).is_empty())
    }

    /// The document's language; a change is another picture of every block.
    pub fn set_language(&self, language: String) -> bool {
        let mut now = self.language.lock().unwrap_or_else(|e| e.into_inner());
        if *now == language {
            return false;
        }
        *now = language;
        true
    }

    /// The layout asks. Never blocks: at most a job is written down.
    pub fn shot(&self, q: &SiteQuery<'_>) -> (u64, SiteShot) {
        let language = self.language.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut h = DefaultHasher::new();
        (q.hash, &self.session, &language).hash(&mut h);
        let key = h.finish();
        let mut pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = pictures.get(key) {
            asked.latest.insert(q.identity, key);
            return (key, SiteShot::Ready(p.picture));
        }
        if let Some(why) = asked.failed.get(&key) {
            return (key, SiteShot::Failed(why.clone()));
        }
        if !asked.jobs.contains_key(&key) {
            if asked.jobs.len() >= REMEMBERED {
                let seen: HashSet<u64> = asked.seen.iter().copied().collect();
                asked.jobs.retain(|k, _| seen.contains(k));
            }
            let page = BlockPage::new(
                self.views.clone(),
                self.package.clone(),
                self.folder.clone(),
                q.name,
                q.args,
                q.source,
            );
            let job = Job {
                key,
                session: self.session.clone(),
                width: q.width.round().clamp(16.0, 4096.0) as u32,
                scale: (q.scale as f64).clamp(0.5, 4.0),
                dark: q.theme.dark,
                lang: language,
                vars: theme_vars(q.theme),
                source: Arc::new(page),
            };
            asked.jobs.insert(key, job);
        }
        let old = asked
            .latest
            .get(&q.identity)
            .and_then(|k| pictures.peek(*k))
            .map(|p| p.picture);
        (key, SiteShot::Pending(old))
    }

    /// The layout says what is on screen.
    pub fn seen(&self, key: u64) {
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        if !asked.seen.contains(&key) {
            asked.seen.push(key);
        }
    }

    pub fn texture(&self, id: u64) -> Option<egui::TextureId> {
        let pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        pictures.peek(id).map(|p| p.texture.id())
    }

    /// Before a frame is laid out: what has come becomes pictures. `true`: the view has to ask again.
    pub fn take_arrivals(&self, ctx: &Context) -> bool {
        let Some(client) = &self.client else { return false };
        let mut changed = false;
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        for (key, outcome) in client.take_results() {
            asked.jobs.remove(&key);
            match outcome {
                Outcome::Drawn(shot) => waiting.push((key, shot)),
                Outcome::Failed(why) => {
                    if asked.failed.len() >= REMEMBERED {
                        asked.failed.clear();
                    }
                    asked.failed.insert(key, why.into());
                    changed = true;
                }
            }
        }
        let now = waiting.len().min(UPLOADS_PER_FRAME);
        let mut pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        let limit = ctx.input(|i| i.max_texture_side);
        for (key, shot) in waiting.drain(..now) {
            let size = [shot.px_width as usize, shot.px_height as usize];
            // refused rather than scaled on this thread or cut in tiles: a limit under the helper's
            // own is rare, and a card says all there is to say
            if size[0] > limit || size[1] > limit {
                asked
                    .failed
                    .insert(key, "The picture is larger than this graphics card takes.".into());
                changed = true;
                continue;
            }
            let image = ColorImage::from_rgba_premultiplied(size, &shot.rgba);
            let cost = shot.rgba.len();
            let texture = ctx.load_texture(format!("markise-block-{key:x}"), image, TextureOptions::LINEAR);
            let picture = SitePicture {
                id: key,
                width: shot.css_width as f32,
                height: shot.css_height as f32,
            };
            pictures.insert(key, Picture { texture, picture }, cost);
            changed = true;
        }
        if !waiting.is_empty() {
            ctx.request_repaint();
        }
        let available = client.availability() != Availability::Unavailable;
        let mut was = self.was_available.lock().unwrap_or_else(|e| e.into_inner());
        if *was != available {
            *was = available;
            changed = true;
        }
        changed || std::mem::take(&mut asked.lost_job)
    }

    /// After a frame is laid out: the client hears what is wanted, when that changed. A document at
    /// rest has nothing to say, and no frame or thread wakes for it.
    pub fn tell_client(&self) {
        let Some(client) = &self.client else { return };
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        let pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        let seen = std::mem::take(&mut asked.seen);
        let waiting: HashSet<u64> = (self.waiting.lock().unwrap_or_else(|e| e.into_inner()).iter())
            .map(|w| w.0)
            .collect();
        let wanted: Vec<u64> = (seen.iter().copied())
            .filter(|k| !pictures.contains(*k) && !asked.failed.contains_key(k) && !waiting.contains(k))
            .take(MAX_WANTED)
            .collect();
        let (submit, lost) = plan_submission(&wanted, |k| asked.jobs.contains_key(&k), &asked.told);
        asked.lost_job |= lost;
        if let Some(submit) = submit {
            let jobs = submit.iter().filter_map(|k| asked.jobs.get(k).cloned()).collect();
            client.want(jobs);
            asked.told = submit;
        }
    }
}

/// What to tell the client, given what is wanted, which of it has a job, and what it was last told:
/// the list when it changed, and whether a job is missing. What has no job is not told, and so not
/// recorded as told: it goes out when the view has asked for it again.
fn plan_submission(wanted: &[u64], has_job: impl Fn(u64) -> bool, told: &[u64]) -> (Option<Vec<u64>>, bool) {
    let submit: Vec<u64> = wanted.iter().copied().filter(|k| has_job(*k)).collect();
    let lost = submit.len() != wanted.len();
    ((submit != told).then_some(submit), lost)
}

impl Drop for SiteBoard {
    /// Ending a helper can take a moment: not on the UI thread.
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            let _ = std::thread::Builder::new()
                .name("markise-web-end".into())
                .spawn(move || drop(client));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plan_submission;

    #[test]
    fn what_has_no_job_is_not_recorded_as_told() {
        let has = |k: u64| k != 2;
        let (submit, lost) = plan_submission(&[1, 2], has, &[]);
        assert_eq!(submit, Some(vec![1]));
        assert!(lost, "the view has to ask again");
        // told [1]; the job of 2 is back: it goes out
        let (submit, lost) = plan_submission(&[1, 2], |_| true, &[1]);
        assert_eq!(submit, Some(vec![1, 2]));
        assert!(!lost);
        // nothing changed: nothing to say
        assert_eq!(plan_submission(&[1, 2], |_| true, &[1, 2]), (None, false));
    }
}
