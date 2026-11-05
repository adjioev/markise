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

fn results(client: &Client, n: usize) -> Vec<(u64, Outcome)> {
    let deadline = Instant::now() + Duration::from_secs(40);
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
    let client = Client::new(Box::new(spawner), Limits::default(), wake);
    let mut j = job(1, "");
    j.source = Arc::new(MissingPicture);
    client.want(vec![j]);
    assert_eq!(
        results(&client, 1),
        vec![(1, Outcome::Failed("File not found: images/x.png".to_string()))]
    );
}

#[test]
fn a_block_that_hangs_costs_a_helper_and_nothing_else() {
    let spawner = ProcessSpawner {
        program: env!("CARGO_BIN_EXE_markise-web-helper").into(),
    };
    let limits = Limits {
        render: Duration::from_secs(4),
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
        panic!("{:?}", got[0])
    };
    assert_eq!((shot.px_width, shot.px_height, shot.css_height), (200, 60, 30.0));
    assert_eq!(&shot.rgba[..4], &[0, 0, 255, 255]);
    assert!(matches!(&got[1].1, Outcome::Failed(why) if why.contains("too long")));
    assert!(matches!(got[2].1, Outcome::Drawn(_)), "on the next helper");
}
