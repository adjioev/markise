//! `markise-web-helper --self-test`: the helper as it is, with its client played in-process. A
//! fixture block has to draw, and a page that attempts everything forbidden has to reach nothing: a
//! listener here waits for whatever gets through. The page has no way to report (there is no such
//! thing), so it paints one cell per attempt and this reads the image. Exit 0 means it all held.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use dispatch2::DispatchQueue;
use markise_web_render::proto::{Area, FromHelper, RenderRequest, Rendered, ToHelper};

use super::render;

/// The attempts, in the order the page paints them.
const ATTEMPTS: &str = include_str!("self_test.names");
const PAGE: &str = include_str!("self_test.js");
const CELL: u32 = 8;

const FIXTURE_HTML: &str =
    r#"<section data-component="fixture"><h2 data-field="title">A title</h2><p>Text</p></section>"#;
const FIXTURE_CSS: &str = "h2,p{margin:0}[data-component]{background:rgb(10,200,30);height:60px;overflow:hidden}";
const FIXTURE_JS: &str = "export default function init(root){const d=document.createElement('div');d.style.cssText='height:40px;background:rgb(200,10,30)';root.after(d);}";

fn local_address() -> IpAddr {
    // no packet goes anywhere: this asks which interface would carry one
    let found = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).and_then(|s| {
        s.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
        s.local_addr()
    });
    match found {
        Ok(a) if !a.ip().is_unspecified() => a.ip(),
        _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}

fn request(id: u64, html: &str, module: &str, styles: &[&str], limit_ms: u32) -> RenderRequest {
    RenderRequest {
        id,
        session: "self-test".to_string(),
        width: 400,
        scale: 1.0,
        dark: false,
        lang: "en".to_string(),
        vars: Vec::new(),
        html: html.to_string(),
        styles: styles.iter().map(|s| s.to_string()).collect(),
        module: Some(module.to_string()),
        limit_ms,
    }
}

fn to_main(message: ToHelper, bytes: Vec<u8>) {
    DispatchQueue::main().exec_async(move || render::received(message, bytes));
}

/// Plays the client until this render's answer.
fn answer(
    frames: &Receiver<(FromHelper, Vec<u8>)>,
    files: &HashMap<(Area, String), Vec<u8>>,
    id: u64,
) -> Result<(Rendered, Vec<u8>), String> {
    loop {
        let (message, bytes) = frames
            .recv_timeout(Duration::from_secs(40))
            .map_err(|e| e.to_string())?;
        match message {
            FromHelper::NeedFile { token, area, path, .. } => {
                let file = files.get(&(area, path));
                let reply = ToHelper::File {
                    token,
                    found: file.is_some(),
                };
                to_main(reply, file.cloned().unwrap_or_default());
            }
            FromHelper::Rendered(r) if r.id == id => return Ok((r, bytes)),
            FromHelper::Failed { id: i, kind, reason } if i == id => return Err(format!("{kind:?}: {reason}")),
            _ => {}
        }
    }
}

fn pixel(r: &Rendered, bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * r.px_width + x) * 4) as usize;
    bytes.get(at..at + 4).map_or([0; 4], |p| [p[0], p[1], p[2], p[3]])
}

/// Hears anything that comes on this port, on any interface, until told to stop.
fn listen() -> std::io::Result<(u16, Arc<AtomicBool>, std::thread::JoinHandle<Vec<String>>)> {
    let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let port = tcp.local_addr()?.port();
    let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
    tcp.set_nonblocking(true)?;
    udp.set_read_timeout(Some(Duration::from_millis(50)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let heard = std::thread::spawn(move || {
        let mut heard = Vec::new();
        let mut buf = [0u8; 64];
        while !stopping.load(Ordering::SeqCst) {
            if let Ok((_, from)) = tcp.accept() {
                heard.push(format!("tcp {from}"));
            }
            if let Ok((_, from)) = udp.recv_from(&mut buf) {
                heard.push(format!("udp {from}"));
            }
        }
        heard
    });
    Ok((port, stop, heard))
}

/// The forbidden page, aimed at a listener on this address.
fn forbidden(
    frames: &Receiver<(FromHelper, Vec<u8>)>,
    files: &mut HashMap<(Area, String), Vec<u8>>,
    id: u64,
    address: IpAddr,
) -> Result<(), String> {
    let (port, stop, heard) = listen().map_err(|e| e.to_string())?;
    let target = format!("{address}:{port}");
    let page = PAGE.replace("TARGET", &target).replace("CELLpx", &format!("{CELL}px"));
    // a URL of its own: the engine keeps a module by its URL
    let module = format!("forbidden-{id}.js");
    files.insert((Area::Package, module.clone()), page.into());
    let request = request(id, "<section></section>", &module, &[], 25_000);
    to_main(ToHelper::Render(request), Vec::new());
    let (m, b) = answer(frames, files, id).map_err(|e| format!("the forbidden page: {e}"))?;
    // time for anything delayed to arrive
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::SeqCst);
    let heard = heard.join().map_err(|_| "the listener stopped")?;

    let mut wrong = Vec::new();
    let per_row = m.px_width / CELL;
    for (k, name) in ATTEMPTS.lines().enumerate() {
        let k = k as u32;
        let (x, y) = ((k % per_row) * CELL + CELL / 2, (k / per_row) * CELL + CELL / 2);
        if pixel(&m, &b, x, y) == [0, 255, 0, 255] {
            println!("stopped: {name}");
        } else {
            println!("MANAGED: {name}");
            wrong.push(name);
        }
    }
    for h in &heard {
        println!("HEARD: {h}");
    }
    println!("the listener on {target} heard {} connections", heard.len());
    if !wrong.is_empty() || !heard.is_empty() {
        return Err(format!("managed {wrong:?}, heard {heard:?}"));
    }
    Ok(())
}

fn run(frames: Receiver<(FromHelper, Vec<u8>)>) -> Result<(), String> {
    let mut files: HashMap<(Area, String), Vec<u8>> = HashMap::new();
    files.insert((Area::Package, "fixture.css".to_string()), FIXTURE_CSS.into());
    files.insert((Area::Package, "fixture.js".to_string()), FIXTURE_JS.into());

    to_main(
        ToHelper::Render(request(1, FIXTURE_HTML, "fixture.js", &["fixture.css"], 10_000)),
        Vec::new(),
    );
    let (m, b) = answer(&frames, &files, 1).map_err(|e| format!("the fixture: {e}"))?;
    if m.css_height != 100.0
        || pixel(&m, &b, 200, 30) != [10, 200, 30, 255]
        || pixel(&m, &b, 200, 80) != [200, 10, 30, 255]
    {
        return Err(format!("the fixture drew wrong: {m:?}"));
    }
    println!("fixture: drawn in {:.0} ms", m.ms);

    // at this machine's address, and at loopback, which a proxy might treat otherwise
    let mut id = 2;
    for address in [local_address(), IpAddr::V4(Ipv4Addr::LOCALHOST)] {
        forbidden(&frames, &mut files, id, address)?;
        id += 1;
    }
    if std::fs::read_dir(std::env::var("HOME").unwrap_or_default()).is_ok() {
        return Err("this process can read the home folder".to_string());
    }
    println!("the home folder: closed to this process");
    Ok(())
}

pub fn start(frames: Receiver<(FromHelper, Vec<u8>)>) {
    let spawned = std::thread::Builder::new()
        .name("self-test".into())
        .spawn(move || match run(frames) {
            Ok(()) => {
                println!("ok");
                std::process::exit(0);
            }
            Err(e) => {
                println!("FAILED: {e}");
                std::process::exit(1);
            }
        });
    if spawned.is_err() {
        std::process::exit(1);
    }
}
