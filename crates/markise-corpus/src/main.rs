//! Writes the benchmark corpus: `cargo run --release -p markise-corpus -- [OUT_DIR]`
//! (default `benches/corpus`, gitignored).
//!
//! `--huge[=MIB]` writes only the scaling corpus instead (default 100 MiB each, seed 42):
//! `huge-mixed.md`, `huge-prose.md` (mostly paragraphs) and `huge-log.md` (many short lines),
//! used by the opt-in `huge` bench (`MARKISE_BENCH_HUGE=1`). They need ~3× the size on disk;
//! point OUT_DIR at a scratch folder and delete them afterwards.
use markise_corpus::{Profile, generate_profile};

fn write(dir: &str, name: &str, size: usize, profile: Profile, crlf: bool) {
    let text = generate_profile(size, 42, profile, crlf);
    let path = format!("{dir}/{name}");
    std::fs::write(&path, &text).expect("write corpus file");
    println!("{path}: {} bytes, {} lines", text.len(), text.lines().count());
}

fn main() {
    let mut dir = "benches/corpus".to_string();
    let mut huge: Option<usize> = None;
    for a in std::env::args().skip(1) {
        if a == "--huge" {
            huge = Some(100);
        } else if let Some(mib) = a.strip_prefix("--huge=") {
            huge = Some(mib.parse().expect("--huge=MIB: a whole number of MiB"));
        } else {
            dir = a;
        }
    }
    std::fs::create_dir_all(&dir).expect("create output dir");
    if let Some(mib) = huge {
        for (name, profile) in [
            ("huge-mixed.md", Profile::Mixed),
            ("huge-prose.md", Profile::Prose),
            ("huge-log.md", Profile::Log),
        ] {
            write(&dir, name, mib << 20, profile, false);
        }
        return;
    }
    let files: [(&str, usize, Profile, bool); 6] = [
        ("small.md", 16 * 1024, Profile::Mixed, false),
        ("1mb.md", 1 << 20, Profile::Mixed, false),
        ("10mb.md", 10 << 20, Profile::Mixed, false),
        ("tables.md", 1 << 20, Profile::Tables, false),
        ("math.md", 1 << 20, Profile::Math, false),
        ("cjk-crlf.md", 1 << 20, Profile::Cjk, true),
    ];
    for (name, size, profile, crlf) in files {
        write(&dir, name, size, profile, crlf);
    }
}
