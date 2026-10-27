//! Quick timing: `cargo run --release -p markise-md --example time_parse -- FILE`
use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).expect("usage: time_parse FILE");
    let text = std::fs::read_to_string(path).expect("read");
    let t = Instant::now();
    let doc = markise_md::parse(&text);
    let parse = t.elapsed();
    let t = Instant::now();
    let out = doc.serialize();
    let ser = t.elapsed();
    assert_eq!(out, text, "round-trip mismatch");
    let mut n = 0;
    for b in doc.blocks() {
        b.walk(&mut |_, _| n += 1);
    }
    println!(
        "{} bytes, {} top-level blocks, {n} blocks: parse {:?}, serialize {:?}",
        text.len(),
        doc.blocks().len(),
        parse,
        ser
    );
}
