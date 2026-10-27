//! Writes the benchmark corpus: `cargo run --release -p markise-corpus -- [OUT_DIR]`
//! (default `benches/corpus`, gitignored).
use markise_corpus::{Profile, generate_profile};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "benches/corpus".to_string());
    std::fs::create_dir_all(&dir).expect("create output dir");
    let files: [(&str, usize, Profile, bool); 6] = [
        ("small.md", 16 * 1024, Profile::Mixed, false),
        ("1mb.md", 1 << 20, Profile::Mixed, false),
        ("10mb.md", 10 << 20, Profile::Mixed, false),
        ("tables.md", 1 << 20, Profile::Tables, false),
        ("math.md", 1 << 20, Profile::Math, false),
        ("cjk-crlf.md", 1 << 20, Profile::Cjk, true),
    ];
    for (name, size, profile, crlf) in files {
        let text = generate_profile(size, 42, profile, crlf);
        let path = format!("{dir}/{name}");
        std::fs::write(&path, &text).expect("write corpus file");
        println!("{path}: {} bytes, {} lines", text.len(), text.lines().count());
    }
}
