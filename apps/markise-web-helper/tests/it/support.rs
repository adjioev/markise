//! Speaks the protocol with a helper, with a package held in memory.

use std::collections::HashMap;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use markise_web_render::proto::{self, Area, FailKind, FromHelper, RenderRequest, Rendered, ToHelper};

pub struct Helper {
    child: Child,
    to: Option<ChildStdin>,
    from: Receiver<(FromHelper, Vec<u8>)>,
    /// The files of the package and of the document.
    pub files: HashMap<(Area, String), Vec<u8>>,
    /// What the page asked for.
    pub asked: Vec<(Area, String)>,
    /// What is never answered, by path.
    pub silent: Vec<String>,
    next_id: u64,
}

pub enum Answer {
    Image(Rendered, Vec<u8>),
    Failed(FailKind, String),
}

impl Helper {
    pub fn start() -> Helper {
        let mut child = Command::new(env!("CARGO_BIN_EXE_markise-web-helper"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the helper starts");
        let to = child.stdin.take().unwrap();
        let mut out = child.stdout.take().unwrap();
        let (tx, from) = channel();
        std::thread::spawn(move || {
            while let Ok(frame) = proto::read_from_helper(&mut out) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let (hello, _) = from.recv_timeout(Duration::from_secs(30)).expect("its first frame");
        assert_eq!(
            hello,
            FromHelper::Hello {
                protocol: proto::PROTOCOL,
                available: true
            }
        );
        Helper {
            child,
            to: Some(to),
            from,
            files: HashMap::new(),
            asked: Vec::new(),
            silent: Vec::new(),
            next_id: 1,
        }
    }

    pub fn file(&mut self, area: Area, path: &str, bytes: impl Into<Vec<u8>>) {
        self.files.insert((area, path.to_string()), bytes.into());
    }

    pub fn request(&mut self, html: &str, module: Option<&str>, styles: &[&str]) -> RenderRequest {
        let id = self.next_id;
        self.next_id += 1;
        RenderRequest {
            id,
            session: "one".to_string(),
            width: 400,
            scale: 1.0,
            dark: false,
            lang: "en".to_string(),
            vars: vec![("--markise-text".to_string(), "#112233".to_string())],
            html: html.to_string(),
            styles: styles.iter().map(|s| s.to_string()).collect(),
            module: module.map(str::to_string),
            limit_ms: 20_000,
        }
    }

    pub fn send(&mut self, m: ToHelper) {
        proto::write_frame(self.to.as_mut().unwrap(), &m.encode(), &[]).expect("the helper listens");
    }

    /// Serves files until this render's answer comes.
    pub fn answer(&mut self, id: u64) -> Answer {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (message, bytes) = self.from.recv_timeout(left).expect("an answer in time");
            match message {
                FromHelper::NeedFile { token, area, path, .. } => {
                    self.asked.push((area, path.clone()));
                    if self.silent.contains(&path) {
                        continue;
                    }
                    let file = self.files.get(&(area, path));
                    let m = ToHelper::File {
                        token,
                        found: file.is_some(),
                    };
                    proto::write_frame(
                        self.to.as_mut().unwrap(),
                        &m.encode(),
                        file.map_or(&[][..], Vec::as_slice),
                    )
                    .unwrap();
                }
                FromHelper::Rendered(r) if r.id == id => return Answer::Image(r, bytes),
                FromHelper::Failed { id: i, kind, reason } if i == id => return Answer::Failed(kind, reason),
                other => panic!("unexpected: {other:?}"),
            }
        }
    }

    pub fn render(&mut self, r: RenderRequest) -> Answer {
        let id = r.id;
        self.send(ToHelper::Render(r));
        self.answer(id)
    }

    /// The helper's end, by itself, within the time.
    pub fn exits_within(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    pub fn close_input(&mut self) {
        self.to = None;
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The colour at a point of an image.
pub fn pixel(r: &Rendered, bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * r.px_width + x) * 4) as usize;
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}
