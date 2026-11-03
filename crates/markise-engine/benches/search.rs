//! Issue #48: source snapshot transfer and off-thread search workload.
//! Run `cargo bench -p markise-engine --bench search`.
use markise_engine::search::{Query, SNAPSHOT_CHUNK, find};
use std::time::{Duration, Instant};
fn main() {
    for mb in [1, 10] {
        let text = markise_corpus::generate(mb << 20, 7);
        let mut cursor = 0;
        let mut chunks = Vec::new();
        let mut longest = Duration::ZERO;
        let mut frames = 0;
        while cursor < text.len() {
            let start = Instant::now();
            for _ in 0..16 {
                if cursor == text.len() {
                    break;
                }
                let mut end = (cursor + SNAPSHOT_CHUNK).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                chunks.push(text[cursor..end].to_owned());
                cursor = end;
            }
            longest = longest.max(start.elapsed());
            frames += 1;
        }
        let start = Instant::now();
        let snapshot = chunks.concat();
        let concat = start.elapsed();
        for query in [
            Query {
                text: "unmatched-corpus-token".into(),
                ..Default::default()
            },
            Query {
                text: "(?m)^#{1,3} ".into(),
                regex: true,
                ..Default::default()
            },
        ] {
            let start = Instant::now();
            let matches = find(&snapshot, &query).unwrap();
            println!(
                "search {mb}MiB regex={} matches={} scan={:.2}ms snapshot_frames={frames} max_transfer_frame={:.3}ms worker_concat={:.2}ms",
                query.regex,
                matches.len(),
                start.elapsed().as_secs_f64() * 1000.0,
                longest.as_secs_f64() * 1000.0,
                concat.as_secs_f64() * 1000.0
            );
        }
    }
}
