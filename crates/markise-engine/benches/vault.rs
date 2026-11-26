//! Vault link resolution and autocomplete on 10 000 synthetic files (spec 06 §6 / V3.5:
//! autocomplete refresh ≤ 30 ms per keystroke on a 10k-note vault; resolution runs on the UI
//! thread once per wikilink laid out or followed, so it must stay in the microseconds).
//!
//! Reported: index build from a file list, a full background scan of a real folder of 10k
//! files (file list, then every note read for its tags, ADR 0027), wikilink resolution (unique
//! name, ambiguous name, path, suffix, unresolved), Markdown link resolution, autocomplete
//! queries of growing length, and the tag index build and a tag search.
//!
//! Run: `cargo bench -p markise-engine --bench vault`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use markise_engine::vault::{VaultIndex, VaultScanner};

const N: usize = 10_000;

/// 10k vault paths: 100 folders × 100 notes, a tenth of the names repeated in every folder
/// (ambiguous basenames), plus some attachments.
fn rels() -> Vec<String> {
    let mut v = Vec::with_capacity(N + 200);
    for f in 0..100 {
        for n in 0..100 {
            let name = if n < 10 {
                format!("Daily {n}")
            } else {
                format!("Note {f}-{n} about topic {}", (f * 7 + n) % 53)
            };
            v.push(format!("Area {}/Folder {f}/{name}.md", f % 10));
        }
    }
    for i in 0..200 {
        v.push(format!("Attachments/image {i}.png"));
    }
    v
}

fn time<T>(label: &str, iters: usize, mut f: impl FnMut() -> T) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..iters {
            std::hint::black_box(f());
        }
        best = best.min(t.elapsed() / iters as u32);
    }
    println!("{label:<44} {:>10.3} µs", best.as_secs_f64() * 1e6);
    best
}

fn main() {
    let rels = rels();
    let root = PathBuf::from("/vault");
    let t = Instant::now();
    let ix = VaultIndex::from_rels(root.clone(), rels.clone(), 1);
    println!(
        "index build ({} files)                       {:>10.3} ms",
        ix.len(),
        t.elapsed().as_secs_f64() * 1e3
    );

    let from = Some("Area 3/Folder 33/Note 33-50 about topic 0.md");
    time("resolve [[unique name]]", 10_000, || {
        ix.resolve_wikilink("Note 57-42 about topic 0", from)
    });
    time("resolve [[ambiguous name]] (100 candidates)", 10_000, || {
        ix.resolve_wikilink("Daily 3", from)
    });
    time("resolve [[folder/path]]", 10_000, || {
        ix.resolve_wikilink("Area 7/Folder 57/Note 57-42 about topic 0", from)
    });
    time("resolve [[suffix/path]]", 10_000, || {
        ix.resolve_wikilink("Folder 57/Daily 4", from)
    });
    time("resolve [[unresolved]]", 10_000, || {
        ix.resolve_wikilink("No such note", from)
    });
    time("resolve [md](../Folder%2012/Daily%201.md)", 10_000, || {
        ix.resolve_markdown("../Folder%2012/Daily%201.md", from)
    });
    let mut worst = Duration::ZERO;
    for q in ["n", "no", "not", "note 5", "note 57-4", "dly", "topic 1"] {
        worst = worst.max(time(&format!("suggest {q:?} (50 rows)"), 20, || {
            ix.suggest(q, 50).len()
        }));
    }
    println!(
        "autocomplete worst query: {:.2} ms (budget 30 ms, V3.5)",
        worst.as_secs_f64() * 1e3
    );

    // a real folder of 10k files, scanned by the background scanner
    let dir = tempfile::tempdir().expect("temp dir");
    let base = std::fs::canonicalize(dir.path()).unwrap();
    for r in &rels {
        let p = base.join(r);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // a short note with front-matter and inline tags (attachments stay empty)
        let text = if r.ends_with(".md") {
            format!(
                "---\ntags: [area/{}, daily]\n---\n# Note\n\nSome text about #topic{} and more.\n",
                r.len() % 10,
                r.len() % 53
            )
        } else {
            String::new()
        };
        std::fs::write(&p, text).unwrap();
    }
    let sc = VaultScanner::new(Arc::new(|| {})).unwrap();
    let t = Instant::now();
    sc.mount(base.clone());
    let mut got = sc.recv_timeout(Duration::from_secs(60)).expect("scan");
    println!(
        "background scan of {} files (worker thread) {:>10.3} ms",
        got.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    while !got.tags().is_complete() {
        got = sc.recv_timeout(Duration::from_secs(120)).expect("tags");
    }
    println!(
        "… and every note read for tags ({} tags)     {:>10.3} ms",
        got.tags().len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    let tags = got.tags().clone();
    time("tag search tag:#area (all its notes)", 1_000, || {
        tags.search("tag:#area").len()
    });
    let notes: std::collections::BTreeMap<String, Arc<[String]>> = got
        .files()
        .iter()
        .filter(|f| f.is_note())
        .map(|f| (f.rel.clone(), tags.tags_of(&f.rel).to_vec().into()))
        .collect();
    time("tag index build (10k notes)", 5, || {
        markise_engine::vault::tags::TagIndex::build(notes.clone(), true).len()
    });
    // a real indexed change (a no-op notification publishes no snapshot)
    std::fs::write(base.join("Area 3/Folder 33/Added note.md"), "").unwrap();
    let t = Instant::now();
    sc.changed(base.join("Area 3/Folder 33"));
    let got = sc.recv_timeout(Duration::from_secs(60)).expect("rescan");
    assert!(got.file("Area 3/Folder 33/Added note.md").is_some());
    println!(
        "one folder changed → new snapshot           {:>10.3} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
}
