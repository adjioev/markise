//! Block-ownership query (ADR 0011) on 1 MB / 10 MB documents: `block_owner`,
//! `block_owner_biased` and `adjacent_block` at offsets spread over the whole document. The
//! query is a binary search over the top-level blocks and each level's children, so its cost
//! must not grow with the document (NFR-4: never scan the whole document per keystroke).

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use markise_corpus::{Profile, generate_profile};
use markise_doc::{Affinity, FormatDoc};
use markise_md::{Document, ParseOptions};

fn offsets(len: usize) -> Vec<usize> {
    // 1024 deterministic offsets (LCG), on char boundaries of an ASCII-heavy corpus
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..1024)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 11) as usize % len
        })
        .collect()
}

fn bench_owner(c: &mut Criterion) {
    let mut g = c.benchmark_group("owner");
    g.sample_size(30).measurement_time(Duration::from_secs(4));
    for (name, size) in [("1mb", 1usize << 20), ("10mb", 10 << 20)] {
        let text = generate_profile(size, 42, Profile::Mixed, false);
        let doc = Document::parse(text.as_str(), ParseOptions::default());
        let offs: Vec<usize> = offsets(text.len())
            .into_iter()
            .map(|mut o| {
                while !text.is_char_boundary(o) {
                    o -= 1;
                }
                o
            })
            .collect();
        // per call: 1024 queries per iteration
        g.bench_with_input(BenchmarkId::new("block_owner_x1024", name), &offs, |b, offs| {
            b.iter(|| {
                for &o in offs {
                    black_box(doc.block_owner(black_box(o)));
                }
            })
        });
        g.bench_with_input(BenchmarkId::new("block_owner_biased_x1024", name), &offs, |b, offs| {
            b.iter(|| {
                for &o in offs {
                    black_box(doc.block_owner_biased(black_box(o), Affinity::Upstream));
                }
            })
        });
        g.bench_with_input(BenchmarkId::new("adjacent_block_x1024", name), &offs, |b, offs| {
            b.iter(|| {
                for &o in offs {
                    black_box(doc.adjacent_block(black_box(o), true));
                }
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_owner);
criterion_main!(benches);
