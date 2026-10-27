//! Prints the block tree of a file (debug helper): `cargo run -p markise-md --example dump -- FILE`
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump FILE");
    let text = std::fs::read_to_string(path).expect("read");
    let doc = markise_md::parse(&text);
    print!("{}", markise_md::debug_tree(&doc));
}
