fn main() {
    let s = std::env::args().nth(1).unwrap_or_default();
    println!("{}", s.len());
}
