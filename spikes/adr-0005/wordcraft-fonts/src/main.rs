//! Throwaway spike (ADR 0005): does `wordcraft-fonts` resolve as a pinned git dependency and
//! build with the workspace toolchain, and what does shaping a paragraph cost?
use std::time::Instant;
use wordcraft_fonts::{FontDb, shape};

fn main() {
    let t = Instant::now();
    let db = FontDb::with_font_dirs(Vec::new());
    db.set_system_fallback(false);
    println!("FontDb (bundled only) ready in {:?}", t.elapsed());

    // A theme @font-face file: add raw bytes (what C3 needs). Use a system font as stand-in.
    let path = std::env::args()
        .nth(1)
        .unwrap_or("/System/Library/Fonts/Supplemental/Georgia.ttf".into());
    let bytes = std::fs::read(&path).expect("font file");
    let added = db.add_font(bytes);
    let fams = db.families();
    println!("added {added} face(s) from {path}; families: {fams:?}");

    let face = db.face("Georgia", "Regular");
    println!("resolved face: {face:?} upem={}", face.upem);
    // Same input as the parley and epaint harnesses (ADR 0005 §7): 1660 bytes, a counter
    // appended per iteration, shape only (no line breaking: wordcraft-fonts has none).
    let para = "The quick brown fox jumps over the lazy dog — office finance, “quotes”, 1/2. ".repeat(20);
    let _ = shape(&face, &para, &[], |c| c); // warm-up
    let mut rounds = Vec::new();
    for r in 0..10 {
        let t = Instant::now();
        for i in 0..100 {
            let text = format!("{para}{}", r * 100 + i);
            std::hint::black_box(shape(&face, &text, &[], |c| c));
        }
        rounds.push(t.elapsed().as_secs_f64() * 1e6 / 100.0 / (para.len() as f64 / 1024.0));
    }
    rounds.sort_by(f64::total_cmp);
    println!(
        "shape-only µs/KiB over 10 rounds x 100: min {:.1} median {:.1} max {:.1}",
        rounds[0], rounds[5], rounds[9]
    );
    let ar = "مرحبا بالعالم hello";
    let g = shape(&face, ar, &[], |c| c);
    println!(
        "mixed RTL in Georgia: {} glyphs, notdef={} (needs fallback)",
        g.len(),
        g.iter().filter(|g| g.gid == 0).count()
    );
    println!(
        "fallback for 'م' with system fallback off: {:?}",
        db.fallback_for('م', face.id()).map(|f| f.family.clone())
    );
    let lig = shape(&db.face("Source Serif 4", "Regular"), "office", &[], |c| c);
    println!(
        "'office' in Source Serif 4 (bundled): {} glyphs (ligature if < 6)",
        lig.len()
    );
    println!("loaded font bytes: {}", db.loaded_font_bytes());

    // System fallback on (the global db scans the platform font folders): memory cost of
    // falling back for Arabic, CJK and colour emoji. wordcraft-fonts forbids unsafe code, so
    // font files are read into memory (no mmap) and never unloaded.
    println!("RSS before system fallback: {} KiB", rss_kib());
    let g = FontDb::global();
    let t = Instant::now();
    let base = g.face("Source Serif 4", "Regular");
    for c in ['م', '日', '한', '👩', 'क'] {
        let f = g.fallback_for(c, base.id());
        println!("  fallback {c:?} -> {:?}", f.map(|f| f.family.clone()));
    }
    println!(
        "system fallbacks resolved in {:?}; loaded font bytes now {} MiB; RSS {} KiB",
        t.elapsed(),
        g.loaded_font_bytes() >> 20,
        rss_kib()
    );
}

fn rss_kib() -> String {
    let pid = std::process::id().to_string();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}
