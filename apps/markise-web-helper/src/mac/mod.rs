//! macOS: a process nobody sees. One hidden web view at a time, on the main thread, with requests
//! from a reader thread and answers through a writer thread, so that neither a full pipe nor a
//! slow client can hold the page's engine up.

use std::sync::mpsc::{self, Receiver};

use dispatch2::DispatchQueue;
use markise_web_render::proto::{self, FromHelper};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

mod delegates;
mod network;
mod page;
mod raster;
mod render;
mod self_test;

pub fn main() {
    let self_test = std::env::args().nth(1).as_deref() == Some("--self-test");
    let mtm = MainThreadMarker::new().expect("the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // no Dock icon, no menu bar, never active
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);

    let (out, frames) = mpsc::channel::<(FromHelper, Vec<u8>)>();
    let available = render::system_is_usable() && network::confine(self_test);
    if !available {
        // the client hears so, and blocks stay cards; nothing here may load a page
        if !self_test {
            hello(false);
        }
        std::process::exit(if self_test { 1 } else { 0 });
    }
    render::install(out);
    if self_test {
        self_test::start(frames);
    } else {
        hello(true);
        serve(frames);
    }
    app.run();
}

fn hello(available: bool) {
    let hello = FromHelper::Hello {
        protocol: proto::PROTOCOL,
        available,
    };
    let _ = proto::write_frame(&mut std::io::stdout().lock(), &hello.encode(), &[]);
}

fn serve(frames: Receiver<(FromHelper, Vec<u8>)>) {
    let writer = std::thread::Builder::new().name("writer".into()).spawn(move || {
        let mut stdout = std::io::stdout().lock();
        for (message, bytes) in frames {
            if proto::write_frame(&mut stdout, &message.encode(), &bytes).is_err() {
                std::process::exit(0);
            }
        }
    });
    let reader = std::thread::Builder::new().name("reader".into()).spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            // the client gone or a frame out of order: there is nobody to draw for
            let Ok((message, bytes)) = proto::read_to_helper(&mut stdin) else {
                std::process::exit(0);
            };
            DispatchQueue::main().exec_async(move || render::received(message, bytes));
        }
    });
    if writer.is_err() || reader.is_err() {
        std::process::exit(1);
    }
}
