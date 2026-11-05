//! The web helper (ADR 0023): gets a block's markup, draws it with the system's web engine in a
//! window nobody sees, and answers with an image. It reads its requests from stdin and answers on
//! stdout. It opens no file: everything a page loads comes through the client, which validates it.

/// No web engine is wired up here: the client hears so, and blocks stay cards.
fn main() {
    use markise_web_render::proto::{FromHelper, PROTOCOL, write_frame};
    let hello = FromHelper::Hello {
        protocol: PROTOCOL,
        available: false,
    };
    let _ = write_frame(&mut std::io::stdout().lock(), &hello.encode(), &[]);
}
