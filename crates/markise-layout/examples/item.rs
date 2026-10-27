//! Debug helper: `cargo run -p markise-layout --example item -- FILE INDEX` prints one item.
fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("usage: item FILE INDEX");
    let idx: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let text = std::fs::read_to_string(path).expect("read");
    let doc = markise_md::parse(&text);
    let m = markise_layout::DocModel::build(&doc, None);
    let it = &m.items[idx];
    println!(
        "{} items; item {idx}: {:?} gap={:?} containers={:?} lines={} chars={}",
        m.items.len(),
        it.kind,
        it.gap,
        it.containers,
        it.est_lines,
        it.est_chars
    );
    let src = m.item_span(idx).slice(&text);
    println!("{}", &src[..src.len().min(600)]);
}
