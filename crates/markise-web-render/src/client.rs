//! The client of the helper (ADR 0023). The UI thread says what it wants and takes what has come;
//! neither ever waits. A thread of its own owns the helper: starts it when there is something to
//! draw and not before, restarts it after a crash or a hang with a growing pause, lets it go when idle,
//! and validates everything it says. No web engine here: tests play the helper over pipes.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::proto::{self, Area, FailKind, FromHelper, RenderRequest, Rendered, ToHelper};

/// What the UI thread may have on its wish list.
pub const MAX_WANTED: usize = 64;
/// What one render may fetch through the client.
const MAX_FILES: u32 = 1024;
const MAX_FILE_BYTES: u64 = 256 << 20;
/// A helper's messages waiting to be read, then its stdin, each at most this long.
const MAX_OUTGOING: usize = 16;
/// What a frame counts for over its bytes: a stream of little ones is no cheaper than that.
const FRAME_COST: usize = 4096;

/// How long things may take.
#[derive(Clone, Debug)]
pub struct Limits {
    /// What a render is given in the helper.
    pub render: Duration,
    /// How much longer the client waits before it kills the helper.
    pub grace: Duration,
    /// From starting a helper to its first frame.
    pub start: Duration,
    /// With nothing to do for this long, the helper goes, and its memory with it.
    pub idle: Duration,
    /// The pause after a helper's end, doubled each time in a row.
    pub backoff: Duration,
    /// Helpers lost in a row with nothing drawn before this counts as no helper at all.
    pub losses: u32,
    /// What a helper has said and the client has not yet taken, in bytes: more than this ends it.
    pub queued_bytes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            render: Duration::from_secs(5),
            grace: Duration::from_secs(2),
            start: Duration::from_secs(10),
            idle: Duration::from_secs(120),
            backoff: Duration::from_millis(250),
            losses: 5,
            queued_bytes: 256 << 20,
        }
    }
}

/// What goes into a page, once it is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    pub html: String,
    /// In order, by path in the package.
    pub styles: Vec<String>,
    pub module: Option<String>,
}

/// What a job knows and the client does not. Called on the client's thread, never the UI's.
pub trait JobSource: Send + Sync {
    fn prepare(&self) -> Result<Prepared, String>;
    /// The file a page asks for, or none. The path is untrusted: this is what decides.
    fn file(&self, area: Area, path: &str) -> Option<Vec<u8>>;
    /// Which images of this job's own could not be had and why, in plain words, for when a page says
    /// that one did not come. `None`: nothing more to say than the helper did.
    fn images_note(&self) -> Option<String> {
        None
    }
}

/// One image wanted.
#[derive(Clone)]
pub struct Job {
    /// What names the image: everything it depends on.
    pub key: u64,
    /// One web data store per session: nothing is shared between two.
    pub session: String,
    pub width: u32,
    pub scale: f64,
    pub dark: bool,
    pub lang: String,
    pub vars: Vec<(String, String)>,
    pub source: Arc<dyn JobSource>,
}

/// An image. RGBA8, premultiplied, sRGB, top row first, with no gap between rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub css_width: u32,
    pub css_height: f64,
    pub px_width: u32,
    pub px_height: u32,
    pub rgba: Vec<u8>,
    /// Milliseconds the helper took.
    pub ms: f64,
}

/// What became of a job.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Drawn(Shot),
    /// In words, not for alarm: the block stays a card. It is not tried again.
    Failed(String),
    /// The same, because a file it asked for was not there or could not be read: that can change
    /// with no change to the block.
    MissingImage(String),
}

/// How the helper is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// As far as anyone knows.
    Unknown,
    Available,
    /// None on this system, or it keeps being lost: blocks stay cards, with no error.
    Unavailable,
}

/// A helper as the client's thread sees it.
pub struct Spawned {
    pub to: Box<dyn Write + Send>,
    pub from: Box<dyn Read + Send>,
    /// Ends it, at once, and collects it.
    pub kill: Box<dyn FnMut() + Send>,
}

/// How helpers come to be.
pub trait Spawner: Send {
    fn spawn(&mut self) -> io::Result<Spawned>;
}

/// The real one: a process with little to go on, reading and writing pipes.
pub struct ProcessSpawner {
    pub program: PathBuf,
}

impl Spawner for ProcessSpawner {
    fn spawn(&mut self) -> io::Result<Spawned> {
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in ["HOME", "TMPDIR", "PATH"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child: Child = command.spawn()?;
        let to = child.stdin.take().ok_or_else(|| io::Error::other("no stdin"))?;
        let from = child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
        Ok(Spawned {
            to: Box::new(to),
            from: Box::new(from),
            kill: Box::new(move || {
                let _ = child.kill();
                let _ = child.wait();
            }),
        })
    }
}

enum Event {
    Want(Vec<Job>),
    Message(u64, FromHelper, Vec<u8>),
    /// That helper said something it must not, or is gone.
    Lost(u64),
    Stop,
}

/// The UI thread's handle. Dropping it ends the thread and the helper.
pub struct Client {
    events: Sender<Event>,
    results: Mutex<Receiver<(u64, Outcome)>>,
    availability: Arc<AtomicU8>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Client {
    /// Nothing starts until something is wanted. `wake` asks for a frame: called when a result is
    /// ready, from the client's thread.
    pub fn new(spawner: Box<dyn Spawner>, limits: Limits, wake: Arc<dyn Fn() + Send + Sync>) -> Client {
        let (events, inbox) = mpsc::channel();
        let (out, results) = mpsc::channel();
        let availability = Arc::new(AtomicU8::new(0));
        let worker = Worker {
            spawner,
            limits,
            wake,
            inbox,
            events: events.clone(),
            out,
            availability: availability.clone(),
            helper: None,
            generation: 0,
            queue: VecDeque::new(),
            flight: None,
            next_id: 1,
            losses: 0,
            not_before: None,
            idle_since: Instant::now(),
            gone: false,
        };
        let thread = std::thread::Builder::new()
            .name("markise-web-client".into())
            .spawn(move || worker.run())
            .ok();
        Client {
            events,
            results: Mutex::new(results),
            availability,
            thread,
        }
    }

    /// All that is wanted now, the most wanted first (at most [`MAX_WANTED`]). What is not on it any
    /// more is dropped, and what is being drawn is cancelled.
    pub fn want(&self, mut jobs: Vec<Job>) {
        jobs.truncate(MAX_WANTED);
        let _ = self.events.send(Event::Want(jobs));
    }

    /// What has come since the last look, by key.
    pub fn take_results(&self) -> Vec<(u64, Outcome)> {
        let results = self.results.lock().unwrap_or_else(|e| e.into_inner());
        results.try_iter().collect()
    }

    pub fn availability(&self) -> Availability {
        match self.availability.load(Ordering::Relaxed) {
            1 => Availability::Available,
            2 => Availability::Unavailable,
            _ => Availability::Unknown,
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Helper {
    generation: u64,
    to: SyncSender<(ToHelper, Vec<u8>)>,
    kill: Box<dyn FnMut() + Send>,
    /// Until its first frame.
    start_deadline: Option<Instant>,
    /// What its reader has handed over and this thread has not taken yet, in bytes.
    queued: Arc<AtomicUsize>,
}

struct Flight {
    id: u64,
    job: Job,
    deadline: Instant,
    files: u32,
    bytes: u64,
    cancelled: bool,
}

struct Worker {
    spawner: Box<dyn Spawner>,
    limits: Limits,
    wake: Arc<dyn Fn() + Send + Sync>,
    inbox: Receiver<Event>,
    events: Sender<Event>,
    out: Sender<(u64, Outcome)>,
    availability: Arc<AtomicU8>,
    helper: Option<Helper>,
    generation: u64,
    queue: VecDeque<Job>,
    flight: Option<Flight>,
    next_id: u64,
    /// Helpers lost since one last drew anything.
    losses: u32,
    /// No new helper before this.
    not_before: Option<Instant>,
    idle_since: Instant,
    /// There is no helper to be had.
    gone: bool,
}

impl Worker {
    fn run(mut self) {
        loop {
            // first, whatever is coming in: a stream of frames must not hold a deadline off
            self.timer();
            self.advance();
            let event = match self.next_deadline() {
                Some(at) => match self.inbox.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(event) => Some(event),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => Some(Event::Stop),
                },
                // nothing going on: no waking up
                None => Some(self.inbox.recv().unwrap_or(Event::Stop)),
            };
            match event {
                None => {}
                Some(Event::Want(jobs)) => self.want(jobs),
                Some(Event::Message(generation, message, bytes)) => {
                    if let Some(h) = self.helper.as_ref().filter(|h| h.generation == generation) {
                        h.queued.fetch_sub(bytes.len() + FRAME_COST, Ordering::Relaxed);
                    }
                    self.message(generation, message, bytes);
                }
                Some(Event::Lost(generation)) => {
                    if self.helper.as_ref().is_some_and(|h| h.generation == generation) {
                        self.lost("The helper stopped.");
                    }
                }
                Some(Event::Stop) => {
                    self.kill();
                    return;
                }
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        if let Some(h) = &self.helper {
            if let Some(at) = h.start_deadline {
                return Some(at);
            }
            return Some(match &self.flight {
                Some(f) => f.deadline,
                None => self.idle_since + self.limits.idle,
            });
        }
        // a helper is to be started when the pause is over
        (!self.queue.is_empty() && !self.gone).then_some(self.not_before?)
    }

    fn timer(&mut self) {
        let now = Instant::now();
        let Some(h) = &self.helper else { return };
        if h.start_deadline.is_some_and(|at| at <= now) {
            return self.lost("The helper did not start in time.");
        }
        match &self.flight {
            Some(f) if f.deadline <= now => self.lost("The block took too long to draw."),
            None if self.idle_since + self.limits.idle <= now => self.kill(),
            _ => {}
        }
    }

    fn set_availability(&self, a: Availability) {
        let n = match a {
            Availability::Unknown => 0,
            Availability::Available => 1,
            Availability::Unavailable => 2,
        };
        self.availability.store(n, Ordering::Relaxed);
    }

    fn kill(&mut self) {
        if let Some(mut h) = self.helper.take() {
            (h.kill)();
        }
    }

    fn deliver(&self, key: u64, outcome: Outcome) {
        if self.out.send((key, outcome)).is_ok() {
            (self.wake)();
        }
    }

    /// The helper is no more. What it was drawing is not tried again, so a block that takes a helper
    /// down does not do it twice.
    fn lost(&mut self, why: &str) {
        self.kill();
        if let Some(f) = self.flight.take()
            && !f.cancelled
        {
            self.deliver(f.job.key, Outcome::Failed(why.to_string()));
        }
        self.losses += 1;
        if self.losses >= self.limits.losses {
            self.no_helper();
            return;
        }
        let pause = self.limits.backoff * 2u32.saturating_pow(self.losses.saturating_sub(1));
        self.not_before = Some(Instant::now() + pause);
    }

    /// For the rest of this client's life. Nothing is reported: a card needs no explaining.
    fn no_helper(&mut self) {
        self.kill();
        self.gone = true;
        self.queue.clear();
        self.flight = None;
        self.set_availability(Availability::Unavailable);
        (self.wake)();
    }

    fn want(&mut self, jobs: Vec<Job>) {
        if self.gone {
            return;
        }
        if let Some(f) = self.flight.as_mut()
            && !f.cancelled
            && !jobs.iter().any(|j| j.key == f.job.key)
        {
            f.cancelled = true;
            let id = f.id;
            self.send(ToHelper::Cancel { id }, Vec::new());
        }
        let flying = self.flight.as_ref().filter(|f| !f.cancelled).map(|f| f.job.key);
        self.queue = jobs.into_iter().filter(|j| Some(j.key) != flying).collect();
        self.idle_since = Instant::now();
    }

    /// Hands a message to the helper's writer, which never holds this thread up. A helper that has
    /// stopped reading is as good as hung.
    fn send(&mut self, message: ToHelper, bytes: Vec<u8>) {
        let Some(h) = &self.helper else { return };
        match h.to.try_send((message, bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.lost("The helper stopped listening."),
            Err(TrySendError::Disconnected(_)) => self.lost("The helper stopped."),
        }
    }

    fn start(&mut self) {
        if self.not_before.is_some_and(|at| at > Instant::now()) {
            return;
        }
        self.not_before = None;
        let spawned = match self.spawner.spawn() {
            Ok(spawned) => spawned,
            Err(_) => return self.no_helper(),
        };
        self.generation += 1;
        let generation = self.generation;
        let Spawned { mut to, mut from, kill } = spawned;
        let (writer, outgoing) = mpsc::sync_channel::<(ToHelper, Vec<u8>)>(MAX_OUTGOING);
        let events = self.events.clone();
        let queued = Arc::new(AtomicUsize::new(0));
        let (counted, budget) = (queued.clone(), self.limits.queued_bytes);
        let reading = std::thread::Builder::new()
            .name("markise-web-reader".into())
            .spawn(move || {
                loop {
                    match proto::read_from_helper(&mut from) {
                        Ok((message, bytes)) => {
                            // over the budget it is ended, and nothing more of it is read or kept
                            let cost = bytes.len() + FRAME_COST;
                            if counted.fetch_add(cost, Ordering::Relaxed) + cost > budget {
                                let _ = events.send(Event::Lost(generation));
                                return;
                            }
                            if events.send(Event::Message(generation, message, bytes)).is_err() {
                                return;
                            }
                        }
                        Err(_) => {
                            let _ = events.send(Event::Lost(generation));
                            return;
                        }
                    }
                }
            });
        let writing = std::thread::Builder::new()
            .name("markise-web-writer".into())
            .spawn(move || {
                for (message, bytes) in outgoing {
                    if proto::write_frame(&mut to, &message.encode(), &bytes).is_err() {
                        return;
                    }
                }
            });
        let mut helper = Helper {
            generation,
            to: writer,
            kill,
            start_deadline: Some(Instant::now() + self.limits.start),
            queued,
        };
        if reading.is_err() || writing.is_err() {
            (helper.kill)();
            return self.no_helper();
        }
        self.helper = Some(helper);
    }

    /// Whatever can happen next, happens.
    fn advance(&mut self) {
        if self.gone || self.flight.is_some() {
            return;
        }
        if self.queue.is_empty() {
            return;
        }
        if self.helper.is_none() {
            self.start();
        }
        if self.helper.as_ref().is_none_or(|h| h.start_deadline.is_some()) {
            return;
        }
        // a failed send ends the helper: what is left waits for the next one
        while self.flight.is_none() && self.helper.is_some() {
            let Some(job) = self.queue.pop_front() else { return };
            match job.source.prepare() {
                Ok(prepared) => self.fly(job, prepared),
                Err(why) => self.deliver(job.key, Outcome::Failed(why)),
            }
        }
    }

    fn fly(&mut self, job: Job, prepared: Prepared) {
        let id = self.next_id;
        self.next_id += 1;
        let request = RenderRequest {
            id,
            session: job.session.clone(),
            width: job.width,
            scale: job.scale,
            dark: job.dark,
            lang: job.lang.clone(),
            vars: job.vars.clone(),
            html: prepared.html,
            styles: prepared.styles,
            module: prepared.module,
            limit_ms: self.limits.render.as_millis() as u32,
        };
        if request.validate().is_err() {
            return self.deliver(job.key, Outcome::Failed("The block is too large to draw.".to_string()));
        }
        self.flight = Some(Flight {
            id,
            job,
            deadline: Instant::now() + self.limits.render + self.limits.grace,
            files: 0,
            bytes: 0,
            cancelled: false,
        });
        self.send(ToHelper::Render(request), Vec::new());
    }

    fn message(&mut self, generation: u64, message: FromHelper, bytes: Vec<u8>) {
        if self.helper.as_ref().is_none_or(|h| h.generation != generation) {
            return;
        }
        match message {
            FromHelper::Hello { protocol, available } => {
                if self.helper.as_ref().is_some_and(|h| h.start_deadline.is_none()) {
                    return self.lost("The helper said hello twice.");
                }
                if protocol != proto::PROTOCOL || !available {
                    return self.no_helper();
                }
                if let Some(h) = self.helper.as_mut() {
                    h.start_deadline = None;
                }
                self.set_availability(Availability::Available);
                self.idle_since = Instant::now();
            }
            FromHelper::NeedFile { token, id, area, path } => self.need_file(token, id, area, &path),
            FromHelper::Rendered(r) => self.rendered(r, bytes),
            FromHelper::Failed { id, kind, reason } => {
                let Some(f) = self.flight.as_ref().filter(|f| f.id == id) else {
                    return;
                };
                let (key, cancelled, source) = (f.job.key, f.cancelled, f.job.source.clone());
                // a helper hung exits by itself: nothing to wait for but its end
                if kind == FailKind::Hung {
                    return self.lost("The block took too long to draw.");
                }
                self.flight = None;
                self.idle_since = Instant::now();
                if !cancelled && kind != FailKind::Cancelled {
                    // the page's word is that one did not come; what it was, this side knows
                    let outcome = if kind == FailKind::Images {
                        Outcome::MissingImage(source.images_note().unwrap_or(reason))
                    } else {
                        Outcome::Failed(reason)
                    };
                    self.deliver(key, outcome);
                }
            }
        }
    }

    fn need_file(&mut self, token: u64, id: u64, area: Area, path: &str) {
        let Some(f) = self.flight.as_mut().filter(|f| f.id == id && !f.cancelled) else {
            return self.send(ToHelper::File { token, found: false }, Vec::new());
        };
        f.files += 1;
        let source = f.job.source.clone();
        let file = if f.files > MAX_FILES || f.bytes > MAX_FILE_BYTES {
            None
        } else {
            source.file(area, path)
        };
        let file = file.filter(|bytes| bytes.len() <= proto::MAX_FILE);
        if let Some(bytes) = &file {
            f.bytes += bytes.len() as u64;
        }
        let message = ToHelper::File {
            token,
            found: file.is_some(),
        };
        self.send(message, file.unwrap_or_default());
    }

    fn rendered(&mut self, r: Rendered, bytes: Vec<u8>) {
        let Some(f) = self.flight.as_ref().filter(|f| f.id == r.id) else {
            return; // cancelled or from before: of no use, and bounded when it was read
        };
        // what it says has to be what was asked for: a false height would go into the layout
        let (width, scale) = (f.job.width as f64, f.job.scale);
        let right_width = (r.px_width as f64 - width * scale).abs() <= 2.0;
        let right_height = (r.px_height as f64 - r.css_height * scale).abs() <= 2.0;
        if !right_width || !right_height {
            return self.lost("The helper sent an image of the wrong size.");
        }
        let Some(f) = self.flight.take() else { return };
        self.losses = 0;
        self.idle_since = Instant::now();
        if !f.cancelled {
            let shot = Shot {
                css_width: f.job.width,
                css_height: r.css_height,
                px_width: r.px_width,
                px_height: r.px_height,
                rgba: bytes,
                ms: r.ms,
            };
            self.deliver(f.job.key, Outcome::Drawn(shot));
        }
    }
}
