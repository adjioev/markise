//! Best-of-20 timing of inline parsing every block of a file (perf check helper).
use markise_md::inline::{InlineContext, parse_inlines};
use std::time::Instant;
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).expect("FILE")).unwrap();
    let doc = markise_md::parse(&text);
    let ctx = InlineContext {
        link_defs: doc.link_defs(),
        footnotes: doc.footnote_labels().into_iter().collect(),
    };
    let mut leaves = Vec::new();
    for b in doc.blocks() {
        b.walk(&mut |b, _| {
            if !b.is_container() && !b.lines.is_empty() {
                leaves.push(b.content(doc.text()))
            }
        });
    }
    let best = (0..20)
        .map(|_| {
            let t = Instant::now();
            for l in &leaves {
                parse_inlines(l, doc.options(), &ctx);
            }
            t.elapsed()
        })
        .min()
        .unwrap();
    println!("inline-all best of 20: {best:?}");
}
