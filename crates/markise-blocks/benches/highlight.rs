//! Highlight throughput (`cargo bench -p markise-blocks --bench highlight`): bytes per second
//! of the synchronous highlighter for typical code, per language, and first-use load cost.

use std::time::Instant;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use markise_blocks::highlight::{highlight_block, syntax_set};

const RUST: &str = "/// Doc comment.\n#[derive(Debug, Clone)]\npub struct Point { x: f64, y: f64 }\n\nimpl Point {\n    pub fn norm(&self) -> f64 {\n        (self.x * self.x + self.y * self.y).sqrt() // length\n    }\n}\n\nfn main() {\n    let p = Point { x: 1.0, y: 2.5 };\n    println!(\"{:?} {}\", p, p.norm());\n}\n";
const PY: &str = "import os\n\nclass A(object):\n    \"\"\"Doc.\"\"\"\n    def f(self, n: int = 3) -> int:\n        # comment\n        return [x * 2 for x in range(n) if x % 2 == 0][0]\n";
const TS: &str = "interface User { id: number; name?: string }\nexport async function load(id: number): Promise<User> {\n  const r = await fetch(`/u/${id}`);\n  return (await r.json()) as User; // cast\n}\n";
const JSON: &str =
    "{\"name\": \"markise\", \"version\": \"0.0.1\", \"list\": [1, 2.5, true, null], \"nested\": {\"k\": \"v\"}}\n";

fn bench(c: &mut Criterion) {
    let t = Instant::now();
    syntax_set();
    eprintln!("syntax set load: {:?}", t.elapsed());
    let t = Instant::now();
    highlight_block("rust", "fn a() {}\n");
    eprintln!("first rust highlight (lazy contexts): {:?}", t.elapsed());
    let mut g = c.benchmark_group("highlight");
    for (lang, src) in [("rust", RUST), ("python", PY), ("typescript", TS), ("json", JSON)] {
        let text = src.repeat(64 * 1024 / src.len() + 1);
        highlight_block(lang, &text);
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_function(lang, |b| b.iter(|| highlight_block(lang, &text)));
    }
    g.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench
}
criterion_main!(benches);
