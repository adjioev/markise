//! The client with the real helper: the two ends of the protocol, and a real process to lose.

use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_web_render::proto::Area;
use markise_web_render::{Client, Job, JobSource, Limits, Outcome, Prepared, ProcessSpawner};

struct Block(&'static str, String);

impl JobSource for Block {
    fn prepare(&self) -> Result<Prepared, String> {
        Ok(Prepared {
            html: "<section style=\"height:30px;background:rgb(0,0,255)\"></section>".to_string(),
            styles: Vec::new(),
            module: Some(self.1.clone()),
        })
    }
    fn file(&self, _area: Area, path: &str) -> Option<Vec<u8>> {
        (path == self.1).then(|| self.0.as_bytes().to_vec())
    }
}

fn job(key: u64, js: &'static str) -> Job {
    Job {
        key,
        session: "s".to_string(),
        width: 100,
        scale: 2.0,
        dark: false,
        lang: "en".to_string(),
        vars: Vec::new(),
        // the engine keeps a loaded module by its URL: within a package, a file does not change
        source: Arc::new(Block(js, format!("m{key}.js"))),
    }
}

/// Limits for a page that is expected to draw. A helper's first page includes starting the web
/// engine's processes, which is counted against the page: on a loaded CI runner even a trivial
/// block took more than 4 s (issue #226, macOS run 42851048074). A test that does not exercise
/// the limit gives it the most the protocol allows; it is a bound, not an expectation.
fn generous() -> Limits {
    Limits {
        render: Duration::from_millis(u64::from(*markise_web_render::proto::LIMITS_MS.end())),
        ..Limits::default()
    }
}

/// Upper bound for any wait here: a hang guard, not a latency expectation.
const HANG_GUARD: Duration = Duration::from_secs(240);

fn results(client: &Client, n: usize) -> Vec<(u64, Outcome)> {
    let deadline = Instant::now() + HANG_GUARD;
    let mut out = Vec::new();
    while out.len() < n && Instant::now() < deadline {
        out.extend(client.take_results());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(out.len(), n, "{out:?}");
    out
}

struct MissingPicture;

impl JobSource for MissingPicture {
    fn prepare(&self) -> Result<Prepared, String> {
        Ok(Prepared {
            html: "<section><img src=\"images/x.png\"></section>".to_string(),
            ..Prepared::default()
        })
    }
    fn file(&self, _area: Area, _path: &str) -> Option<Vec<u8>> {
        None
    }
    fn images_note(&self) -> Option<String> {
        Some("File not found: images/x.png".to_string())
    }
}

#[test]
fn a_picture_that_did_not_come_reaches_the_block_as_its_reason() {
    let wake = Arc::new(|| {});
    let spawner = ProcessSpawner {
        program: env!("CARGO_BIN_EXE_markise-web-helper").into(),
    };
    let client = Client::new(Box::new(spawner), generous(), wake);
    let mut j = job(1, "");
    j.source = Arc::new(MissingPicture);
    client.want(vec![j]);
    assert_eq!(
        results(&client, 1),
        vec![(1, Outcome::MissingImage("File not found: images/x.png".to_string()))]
    );
}

#[test]
fn a_block_that_hangs_costs_a_helper_and_nothing_else() {
    let spawner = ProcessSpawner {
        program: env!("CARGO_BIN_EXE_markise-web-helper").into(),
    };
    // The hang is found by this limit, so it is what the test takes at least. It also covers the
    // other two pages, each the first of a helper (the engine's start counts against it): 4 s was
    // not enough for them on a loaded runner, 20 s is five times what that runner needed.
    let limits = Limits {
        render: Duration::from_secs(20),
        ..Limits::default()
    };
    let client = Client::new(Box::new(spawner), limits, Arc::new(|| {}));
    client.want(vec![
        job(1, "export default function(){}"),
        job(2, "export default function(){for(;;){}}"),
        job(3, "export default function(){}"),
    ]);
    let got = results(&client, 3);
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), [1, 2, 3]);
    let Outcome::Drawn(shot) = &got[0].1 else {
        panic!("the first block, on the first helper: {:?}", got[0])
    };
    assert_eq!((shot.px_width, shot.px_height, shot.css_height), (200, 60, 30.0));
    assert_eq!(&shot.rgba[..4], &[0, 0, 255, 255]);
    assert!(
        matches!(&got[1].1, Outcome::Failed(why) if why.contains("too long")),
        "the block that hangs: {:?}",
        got[1]
    );
    assert!(
        matches!(got[2].1, Outcome::Drawn(_)),
        "the last block, on the next helper: {:?}",
        got[2]
    );
}
