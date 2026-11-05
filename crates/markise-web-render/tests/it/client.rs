//! The client against a helper played here over pipes: no web engine in sight.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use markise_web_render::proto::{self, Area, FailKind, FromHelper, PROTOCOL, Rendered, ToHelper};
use markise_web_render::{Availability, Client, Job, JobSource, Limits, Outcome, Prepared, Spawned, Spawner};

/// What a played helper does with each message.
type Script = Arc<dyn Fn(&mut Peer, ToHelper, Vec<u8>) + Send + Sync>;

/// The helper's side of the pipes.
struct Peer {
    from: std::io::PipeReader,
    to: std::io::PipeWriter,
    /// What it received, for the test.
    seen: Sender<ToHelper>,
}

impl Peer {
    fn say(&mut self, m: FromHelper, bytes: &[u8]) {
        let _ = proto::write_frame(&mut self.to, &m.encode(), bytes);
    }

    fn draw(&mut self, id: u64, w: u32, h: u32) {
        let r = Rendered {
            id,
            css_height: h as f64,
            px_width: w,
            px_height: h,
            ms: 1.0,
        };
        self.say(FromHelper::Rendered(r), &vec![7u8; (w * h * 4) as usize]);
    }
}

struct Played {
    script: Script,
    hello: Option<FromHelper>,
    spawned: Arc<AtomicUsize>,
    killed: Arc<AtomicUsize>,
    seen: Sender<ToHelper>,
    fail_to_spawn: bool,
}

impl Spawner for Played {
    fn spawn(&mut self) -> std::io::Result<Spawned> {
        if self.fail_to_spawn {
            return Err(std::io::Error::other("no such helper"));
        }
        self.spawned.fetch_add(1, Ordering::SeqCst);
        let (from_client, to_helper) = std::io::pipe()?;
        let (from_helper, to_client) = std::io::pipe()?;
        let mut peer = Peer {
            from: from_client,
            to: to_client,
            seen: self.seen.clone(),
        };
        let (script, hello) = (self.script.clone(), self.hello.clone());
        let alive = Arc::new(AtomicBool::new(true));
        let on = alive.clone();
        std::thread::spawn(move || {
            if let Some(h) = hello {
                peer.say(h, &[]);
            }
            while let Ok((m, bytes)) = proto::read_to_helper(&mut peer.from) {
                if !on.load(Ordering::SeqCst) {
                    return;
                }
                let _ = peer.seen.send(m.clone());
                script(&mut peer, m, bytes);
            }
        });
        let killed = self.killed.clone();
        Ok(Spawned {
            to: Box::new(to_helper),
            from: Box::new(from_helper),
            kill: Box::new(move || {
                alive.store(false, Ordering::SeqCst);
                killed.fetch_add(1, Ordering::SeqCst);
            }),
        })
    }
}

struct Files(HashMap<String, Vec<u8>>);

impl JobSource for Files {
    fn prepare(&self) -> Result<Prepared, String> {
        if self.0.contains_key("refuse") {
            return Err("no such thing".to_string());
        }
        Ok(Prepared {
            html: "<section></section>".to_string(),
            styles: vec!["a.css".to_string()],
            module: None,
        })
    }
    fn file(&self, _area: Area, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

fn job(key: u64) -> Job {
    Job {
        key,
        session: "s".to_string(),
        width: 20,
        scale: 1.0,
        dark: false,
        lang: "en".to_string(),
        vars: Vec::new(),
        source: Arc::new(Files(HashMap::from([("a.css".to_string(), b"css".to_vec())]))),
    }
}

struct Rig {
    client: Client,
    spawned: Arc<AtomicUsize>,
    killed: Arc<AtomicUsize>,
    seen: Receiver<ToHelper>,
    wakes: Arc<AtomicUsize>,
}

fn limits() -> Limits {
    Limits {
        render: Duration::from_millis(300),
        grace: Duration::from_millis(100),
        start: Duration::from_millis(300),
        idle: Duration::from_secs(60),
        backoff: Duration::from_millis(10),
        losses: 3,
        queued_bytes: 256 << 20,
    }
}

fn hello() -> Option<FromHelper> {
    Some(FromHelper::Hello {
        protocol: PROTOCOL,
        available: true,
    })
}

fn rig_with(script: Script, hello: Option<FromHelper>, limits: Limits, fail_to_spawn: bool) -> Rig {
    let (seen_tx, seen) = channel();
    let (spawned, killed) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let played = Played {
        script,
        hello,
        spawned: spawned.clone(),
        killed: killed.clone(),
        seen: seen_tx,
        fail_to_spawn,
    };
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let client = Client::new(
        Box::new(played),
        limits,
        Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    );
    Rig {
        client,
        spawned,
        killed,
        seen,
        wakes,
    }
}

fn rig(script: impl Fn(&mut Peer, ToHelper, Vec<u8>) + Send + Sync + 'static) -> Rig {
    rig_with(Arc::new(script), hello(), limits(), false)
}

/// Draws whatever it is asked to.
fn drawing(peer: &mut Peer, m: ToHelper, _bytes: Vec<u8>) {
    if let ToHelper::Render(r) = m {
        peer.draw(r.id, r.width, 20);
    }
}

fn results(r: &Rig, n: usize) -> Vec<(u64, Outcome)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = Vec::new();
    while out.len() < n {
        out.extend(r.client.take_results());
        assert!(Instant::now() < deadline, "only {out:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    out
}

fn until(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn nothing_starts_until_something_is_wanted() {
    let r = rig(drawing);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(r.spawned.load(Ordering::SeqCst), 0);
    r.client.want(Vec::new());
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(r.spawned.load(Ordering::SeqCst), 0, "an empty list starts nothing");
    assert_eq!(r.client.availability(), Availability::Unknown);
}

#[test]
fn a_block_is_drawn_and_a_frame_is_asked_for() {
    let r = rig(drawing);
    r.client.want(vec![job(5)]);
    let got = results(&r, 1);
    assert_eq!(got[0].0, 5);
    let Outcome::Drawn(s) = &got[0].1 else {
        panic!("{got:?}")
    };
    assert_eq!((s.css_width, s.css_height, s.px_width, s.px_height), (20, 20.0, 20, 20));
    assert_eq!(s.rgba.len(), 1600);
    assert!(r.wakes.load(Ordering::SeqCst) >= 1);
    assert_eq!(r.client.availability(), Availability::Available);
    assert_eq!(r.spawned.load(Ordering::SeqCst), 1);
}

#[test]
fn the_most_wanted_goes_first_and_one_goes_at_a_time() {
    let r = rig(drawing);
    r.client.want(vec![job(1), job(2), job(3)]);
    let got: Vec<u64> = results(&r, 3).into_iter().map(|x| x.0).collect();
    assert_eq!(got, [1, 2, 3]);
}

#[test]
fn what_is_no_longer_wanted_is_cancelled_or_dropped() {
    // the first render does not end by itself: it waits for its cancel
    let r = rig(|peer, m, _| match m {
        ToHelper::Render(q) if q.lang == "first" => {}
        ToHelper::Render(q) => peer.draw(q.id, q.width, 20),
        ToHelper::Cancel { id } => peer.say(
            FromHelper::Failed {
                id,
                kind: FailKind::Cancelled,
                reason: "cancelled".to_string(),
            },
            &[],
        ),
        _ => {}
    });
    let mut first = job(1);
    first.lang = "first".to_string();
    r.client.want(vec![first, job(2)]);
    until(|| matches!(r.seen.try_recv(), Ok(ToHelper::Render(_))));
    // the block scrolled away, another came
    r.client.want(vec![job(3)]);
    let got = results(&r, 1);
    assert_eq!(got[0].0, 3, "not the cancelled one, and not the one never begun");
    std::thread::sleep(Duration::from_millis(30));
    assert!(r.client.take_results().is_empty(), "nothing more comes");
}

#[test]
fn a_block_still_wanted_is_not_begun_twice() {
    let r = rig(|peer, m, _| {
        if let ToHelper::Render(q) = m {
            std::thread::sleep(Duration::from_millis(50));
            peer.draw(q.id, q.width, 20);
        }
    });
    r.client.want(vec![job(1)]);
    until(|| matches!(r.seen.try_recv(), Ok(ToHelper::Render(_))));
    r.client.want(vec![job(1)]);
    r.client.want(vec![job(1)]);
    assert_eq!(results(&r, 1).len(), 1);
    std::thread::sleep(Duration::from_millis(30));
    assert!(r.seen.try_recv().is_err(), "once");
}

#[test]
fn files_go_through_the_job_and_only_for_its_render() {
    let r = rig(|peer, m, bytes| match m {
        ToHelper::Render(q) => {
            for (token, id, path) in [(1, q.id, "a.css"), (2, q.id, "missing"), (3, q.id + 99, "a.css")] {
                peer.say(
                    FromHelper::NeedFile {
                        token,
                        id,
                        area: Area::Package,
                        path: path.to_string(),
                    },
                    &[],
                );
            }
            let id = q.id;
            let answers: Vec<(u64, bool, Vec<u8>)> = (0..3)
                .map(|_| match proto::read_to_helper(&mut peer.from).unwrap() {
                    (ToHelper::File { token, found }, b) => (token, found, b),
                    other => panic!("{other:?}"),
                })
                .collect();
            assert_eq!(answers[0], (1, true, b"css".to_vec()));
            assert_eq!(answers[1], (2, false, Vec::new()));
            assert_eq!(answers[2], (3, false, Vec::new()), "another render's token");
            peer.draw(id, 20, 20);
        }
        _ => drop(bytes),
    });
    r.client.want(vec![job(1)]);
    assert!(matches!(results(&r, 1)[0].1, Outcome::Drawn(_)));
}

#[test]
fn a_file_flood_gets_no_more_than_its_budget() {
    let served = Arc::new(AtomicUsize::new(0));
    let count = served.clone();
    let script = move |peer: &mut Peer, m: ToHelper, _: Vec<u8>| {
        if let ToHelper::Render(q) = m {
            // the client must answer each: it never stalls
            for token in 0..1500 {
                peer.say(
                    FromHelper::NeedFile {
                        token,
                        id: q.id,
                        area: Area::Package,
                        path: "a.css".to_string(),
                    },
                    &[],
                );
                if let Ok((ToHelper::File { found: true, .. }, _)) = proto::read_to_helper(&mut peer.from) {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }
            peer.draw(q.id, 20, 20);
        }
    };
    let mut slow = limits();
    slow.render = Duration::from_secs(10);
    let r = rig_with(Arc::new(script), hello(), slow, false);
    r.client.want(vec![job(1)]);
    results(&r, 1);
    assert_eq!(served.load(Ordering::SeqCst), 1024);
}

#[test]
fn a_block_that_cannot_be_prepared_fails_without_a_helper() {
    let r = rig(drawing);
    let mut j = job(1);
    j.source = Arc::new(Files(HashMap::from([("refuse".to_string(), Vec::new())])));
    r.client.want(vec![j]);
    assert_eq!(results(&r, 1)[0].1, Outcome::Failed("no such thing".to_string()));
}

struct NoteSource;
impl JobSource for NoteSource {
    fn prepare(&self) -> Result<Prepared, String> {
        Ok(Prepared::default())
    }
    fn file(&self, _: Area, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn images_note(&self) -> Option<String> {
        Some("File not found: x.png".to_string())
    }
}

#[test]
fn a_picture_that_did_not_come_is_named_by_the_job_and_nothing_else_is() {
    let r = rig(|peer, m, _| {
        if let ToHelper::Render(q) = m {
            let kind = if q.width == 21 {
                FailKind::Images
            } else {
                FailKind::Script
            };
            let reason = "the helper's words".to_string();
            peer.say(FromHelper::Failed { id: q.id, kind, reason }, &[]);
        }
    });
    let mut images = job(1);
    images.width = 21;
    images.source = Arc::new(NoteSource);
    let mut script = job(2);
    script.source = Arc::new(NoteSource);
    r.client.want(vec![images, script]);
    let got = results(&r, 2);
    assert_eq!(got[0], (1, Outcome::Failed("File not found: x.png".to_string())));
    assert_eq!(got[1], (2, Outcome::Failed("the helper's words".to_string())));
}

#[test]
fn a_failure_is_for_that_block_and_the_helper_goes_on() {
    let r = rig(|peer, m, _| {
        if let ToHelper::Render(q) = m {
            if q.width == 21 {
                peer.say(
                    FromHelper::Failed {
                        id: q.id,
                        kind: FailKind::Script,
                        reason: "threw".to_string(),
                    },
                    &[],
                );
            } else {
                peer.draw(q.id, q.width, 20);
            }
        }
    });
    let mut bad = job(1);
    bad.width = 21;
    r.client.want(vec![bad, job(2)]);
    let got = results(&r, 2);
    assert_eq!(got[0], (1, Outcome::Failed("threw".to_string())));
    assert!(matches!(got[1].1, Outcome::Drawn(_)));
    assert_eq!(r.spawned.load(Ordering::SeqCst), 1);
    assert_eq!(r.killed.load(Ordering::SeqCst), 0);
}

#[test]
fn a_hung_helper_is_killed_and_the_block_not_tried_again() {
    // draws everything but the block of width 11, for which it says nothing at all
    let r = rig(|peer, m, _| {
        if let ToHelper::Render(q) = m
            && q.width != 21
        {
            peer.draw(q.id, q.width, 20);
        }
    });
    let mut bad = job(1);
    bad.width = 21;
    let t = Instant::now();
    r.client.want(vec![bad, job(2)]);
    let got = results(&r, 2);
    assert!(t.elapsed() >= Duration::from_millis(350), "{:?}", t.elapsed());
    assert_eq!(got[0].0, 1);
    assert!(matches!(&got[0].1, Outcome::Failed(why) if why.contains("too long")));
    assert!(matches!(got[1].1, Outcome::Drawn(_)), "the next on a new helper");
    assert_eq!(r.spawned.load(Ordering::SeqCst), 2);
    assert_eq!(r.killed.load(Ordering::SeqCst), 1);
}

#[test]
fn the_helper_saying_it_hung_is_not_waited_for() {
    let r = rig(|peer, m, _| {
        if let ToHelper::Render(q) = m {
            peer.say(
                FromHelper::Failed {
                    id: q.id,
                    kind: FailKind::Hung,
                    reason: "hung".to_string(),
                },
                &[],
            );
        }
    });
    let t = Instant::now();
    r.client.want(vec![job(1)]);
    assert!(matches!(results(&r, 1)[0].1, Outcome::Failed(_)));
    assert!(t.elapsed() < Duration::from_millis(300), "{:?}", t.elapsed());
    assert_eq!(r.killed.load(Ordering::SeqCst), 1);
}

#[test]
fn garbage_from_the_helper_ends_it() {
    let lengths: Vec<Vec<u8>> = vec![
        vec![0xff; 4],
        {
            let mut v = 5u32.to_le_bytes().to_vec();
            v.extend_from_slice(b"12345");
            v.extend_from_slice(&0u32.to_le_bytes());
            v
        },
        {
            // a little message with an attachment
            let j = br#"{"t":"failed","id":1,"kind":"hung","reason":""}"#;
            let mut v = (j.len() as u32).to_le_bytes().to_vec();
            v.extend_from_slice(j);
            v.extend_from_slice(&3u32.to_le_bytes());
            v.extend_from_slice(b"abc");
            v
        },
    ];
    for bytes in lengths {
        let r = rig(move |peer, m, _| {
            if matches!(m, ToHelper::Render(_)) {
                let _ = peer.to.write_all(&bytes);
                let _ = peer.to.flush();
            }
        });
        r.client.want(vec![job(1)]);
        assert!(matches!(results(&r, 1)[0].1, Outcome::Failed(_)));
        assert_eq!(r.killed.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn an_image_not_what_was_asked_for_ends_the_helper() {
    // a false height would go straight into the layout
    let bad: [(u32, u32, f64); 2] = [(20, 20, 16000.0), (99, 20, 20.0)];
    for (w, h, css) in bad {
        let r = rig(move |peer, m, _| {
            if let ToHelper::Render(q) = m {
                let rendered = Rendered {
                    id: q.id,
                    css_height: css,
                    px_width: w,
                    px_height: h,
                    ms: 0.0,
                };
                peer.say(FromHelper::Rendered(rendered), &vec![0u8; (w * h * 4) as usize]);
            }
        });
        r.client.want(vec![job(1)]);
        assert!(matches!(&results(&r, 1)[0].1, Outcome::Failed(why) if why.contains("wrong size")));
    }
}

#[test]
fn a_helper_that_never_says_hello_is_given_up_on() {
    let r = rig_with(Arc::new(drawing), None, limits(), false);
    r.client.want(vec![job(1)]);
    until(|| r.killed.load(Ordering::SeqCst) >= 1);
}

#[test]
fn no_engine_here_means_no_helper_and_no_error() {
    let hello = Some(FromHelper::Hello {
        protocol: PROTOCOL,
        available: false,
    });
    let r = rig_with(Arc::new(drawing), hello, limits(), false);
    r.client.want(vec![job(1)]);
    until(|| r.client.availability() == Availability::Unavailable);
    std::thread::sleep(Duration::from_millis(30));
    assert!(r.client.take_results().is_empty());
    assert_eq!(r.spawned.load(Ordering::SeqCst), 1, "it does not start again");
    r.client.want(vec![job(2)]);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(r.spawned.load(Ordering::SeqCst), 1);
}

#[test]
fn another_protocol_is_no_helper_either() {
    let hello = Some(FromHelper::Hello {
        protocol: PROTOCOL + 1,
        available: true,
    });
    let r = rig_with(Arc::new(drawing), hello, limits(), false);
    r.client.want(vec![job(1)]);
    until(|| r.client.availability() == Availability::Unavailable);
}

#[test]
fn no_helper_to_start_means_no_helper() {
    let r = rig_with(Arc::new(drawing), hello(), limits(), true);
    r.client.want(vec![job(1)]);
    until(|| r.client.availability() == Availability::Unavailable);
}

#[test]
fn losing_helpers_again_and_again_ends_in_none() {
    let r = rig(|peer, m, _| {
        if matches!(m, ToHelper::Render(_)) {
            let _ = peer.to.write_all(&[0xff; 4]);
        }
    });
    r.client.want((0..10).map(job).collect());
    until(|| r.client.availability() == Availability::Unavailable);
    assert_eq!(r.spawned.load(Ordering::SeqCst), 3);
    assert_eq!(
        r.client.take_results().len(),
        3,
        "each one's block failed, the rest are not tried"
    );
}

#[test]
fn a_drawing_helper_starts_the_count_again() {
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let r = rig(move |peer, m, _| {
        if let ToHelper::Render(q) = m {
            if count.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
                let _ = peer.to.write_all(&[0xff; 4]);
            } else {
                peer.draw(q.id, q.width, 20);
            }
        }
    });
    r.client.want((0..8).map(job).collect());
    assert_eq!(results(&r, 8).len(), 8);
    assert_eq!(r.client.availability(), Availability::Available);
}

#[test]
fn a_new_helper_waits_out_its_pause() {
    let mut l = limits();
    l.backoff = Duration::from_millis(150);
    let r = rig_with(
        Arc::new(|peer: &mut Peer, m: ToHelper, _: Vec<u8>| {
            if matches!(m, ToHelper::Render(_)) {
                let _ = peer.to.write_all(&[0xff; 4]);
            }
        }),
        hello(),
        l,
        false,
    );
    r.client.want(vec![job(1), job(2)]);
    results(&r, 1);
    let t = Instant::now();
    results(&r, 1);
    assert!(t.elapsed() >= Duration::from_millis(120), "{:?}", t.elapsed());
}

#[test]
fn an_idle_helper_goes_and_comes_back_when_needed() {
    let mut l = limits();
    l.idle = Duration::from_millis(50);
    let r = rig_with(Arc::new(drawing), hello(), l, false);
    r.client.want(vec![job(1)]);
    results(&r, 1);
    until(|| r.killed.load(Ordering::SeqCst) == 1);
    r.client.want(vec![job(2)]);
    results(&r, 1);
    assert_eq!(r.spawned.load(Ordering::SeqCst), 2);
}

#[test]
fn dropping_the_client_ends_the_helper() {
    let r = rig(drawing);
    r.client.want(vec![job(1)]);
    results(&r, 1);
    let killed = r.killed.clone();
    drop(r);
    assert_eq!(killed.load(Ordering::SeqCst), 1);
}

#[test]
fn the_ui_thread_never_waits_on_a_helper_that_does_not_read() {
    // a helper that holds still: stdin is not read, and the pipe fills up
    let (seen, _rx) = channel();
    let played = Stuck {
        seen,
        keep: Mutex::new(Vec::new()),
    };
    let client = Client::new(Box::new(played), limits(), Arc::new(|| {}));
    let t = Instant::now();
    for k in 0..50 {
        client.want(vec![job(k)]);
    }
    let _ = client.take_results();
    assert!(t.elapsed() < Duration::from_millis(100), "{:?}", t.elapsed());
}

struct Stuck {
    #[allow(dead_code)]
    seen: Sender<()>,
    keep: Mutex<Vec<Box<dyn Read + Send>>>,
}

impl Spawner for Stuck {
    fn spawn(&mut self) -> std::io::Result<Spawned> {
        let (keep, to) = std::io::pipe()?;
        let (from, mut to_client) = std::io::pipe()?;
        self.keep.lock().unwrap().push(Box::new(keep));
        let hello = FromHelper::Hello {
            protocol: PROTOCOL,
            available: true,
        };
        proto::write_frame(&mut to_client, &hello.encode(), &[])?;
        std::mem::forget(to_client);
        Ok(Spawned {
            to: Box::new(to),
            from: Box::new(from),
            kill: Box::new(|| {}),
        })
    }
}

/// Keeps the client's thread busy: preparing it takes a while.
struct Slow(Duration);

impl JobSource for Slow {
    fn prepare(&self) -> Result<Prepared, String> {
        std::thread::sleep(self.0);
        Ok(Prepared {
            html: "<section></section>".to_string(),
            styles: Vec::new(),
            module: None,
        })
    }
    fn file(&self, _area: Area, _path: &str) -> Option<Vec<u8>> {
        std::thread::sleep(self.0);
        None
    }
}

/// Valid frames with an id nobody asked for, each with a large attachment.
fn stale(peer: &mut Peer, n: usize) {
    for _ in 0..n {
        let r = Rendered {
            id: 999_999,
            css_height: 512.0,
            px_width: 512,
            px_height: 512,
            ms: 0.0,
        };
        if proto::write_frame(
            &mut peer.to,
            &FromHelper::Rendered(r).encode(),
            &vec![0u8; 512 * 512 * 4],
        )
        .is_err()
        {
            return;
        }
    }
}

#[test]
fn a_helper_streaming_while_the_client_is_busy_is_ended_not_buffered() {
    let script = move |peer: &mut Peer, m: ToHelper, _: Vec<u8>| {
        if let ToHelper::Render(q) = m {
            // the file holds the client's thread while these come in: about 100 MiB in all
            let need = FromHelper::NeedFile {
                token: 1,
                id: q.id,
                area: Area::Package,
                path: "slow".to_string(),
            };
            peer.say(need, &[]);
            stale(peer, 100);
            peer.draw(q.id, 20, 20);
        }
    };
    let mut l = limits();
    l.render = Duration::from_secs(10);
    l.queued_bytes = 8 << 20;
    let r = rig_with(Arc::new(script), hello(), l, false);
    let mut j = job(1);
    j.source = Arc::new(Slow(Duration::from_millis(1500)));
    r.client.want(vec![j]);
    until(|| r.killed.load(Ordering::SeqCst) >= 1);
    let got = results(&r, 1);
    assert!(matches!(got[0].1, Outcome::Failed(_)), "{got:?}");
}

#[test]
fn frames_about_nothing_do_not_hold_a_deadline_off() {
    let r = rig(|peer, m, _| {
        if matches!(m, ToHelper::Render(_)) {
            // for ever, as fast as it can, none about this render
            loop {
                let chatter = FromHelper::Failed {
                    id: 999_999,
                    kind: FailKind::Script,
                    reason: "chatter".to_string(),
                };
                if proto::write_frame(&mut peer.to, &chatter.encode(), &[]).is_err() {
                    return;
                }
            }
        }
    });
    let t = Instant::now();
    r.client.want(vec![job(1)]);
    let got = results(&r, 1);
    assert!(
        matches!(&got[0].1, Outcome::Failed(why) if why.contains("too long")),
        "{got:?}"
    );
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(r.killed.load(Ordering::SeqCst), 1);
}

#[test]
fn a_helper_that_stops_taking_input_loses_no_job_behind_it() {
    // the first frame goes through, the next write finds the pipe gone
    struct Spawn2(Arc<AtomicUsize>);
    struct OneFrame(usize);
    impl Write for OneFrame {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0 = self.0.saturating_sub(1);
            Ok(())
        }
    }
    impl Spawner for Spawn2 {
        fn spawn(&mut self) -> std::io::Result<Spawned> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let (from, mut to) = std::io::pipe()?;
            let hello = FromHelper::Hello {
                protocol: PROTOCOL,
                available: true,
            };
            proto::write_frame(&mut to, &hello.encode(), &[])?;
            // after the first render: its picture, and a file nobody can be answered about
            let r = Rendered {
                id: 1,
                css_height: 20.0,
                px_width: 20,
                px_height: 20,
                ms: 0.0,
            };
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                let need = FromHelper::NeedFile {
                    token: 1,
                    id: 1,
                    area: Area::Package,
                    path: "a.css".to_string(),
                };
                let _ = proto::write_frame(&mut to, &need.encode(), &[]);
                std::thread::sleep(Duration::from_millis(30));
                let _ = proto::write_frame(&mut to, &FromHelper::Rendered(r).encode(), &[7u8; 1600]);
                std::thread::sleep(Duration::from_secs(5));
            });
            Ok(Spawned {
                to: Box::new(OneFrame(1)),
                from: Box::new(from),
                kill: Box::new(|| {}),
            })
        }
    }
    let spawned = Arc::new(AtomicUsize::new(0));
    let mut l = limits();
    l.losses = 100;
    let client = Client::new(Box::new(Spawn2(spawned)), l, Arc::new(|| {}));
    client.want(vec![job(1)]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while got.is_empty() {
        got.extend(client.take_results());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    // the writer has failed by now: the next jobs have to come to an end all the same
    std::thread::sleep(Duration::from_millis(50));
    client.want(vec![job(2), job(3)]);
    let mut keys: Vec<u64> = got.iter().map(|g| g.0).collect();
    while !(keys.contains(&2) && keys.contains(&3)) {
        keys.extend(client.take_results().into_iter().map(|g| g.0));
        assert!(Instant::now() < deadline, "only {keys:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}
