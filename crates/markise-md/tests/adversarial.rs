//! Adversarial inputs must parse in (near) linear time (review P2 #6) and never panic.

use std::time::{Duration, Instant};

use markise_md::inline::{InlineContext, parse_inlines};
use markise_md::*;

fn run(text: &str) -> Duration {
    let t = Instant::now();
    let d = parse(text);
    assert_eq!(d.serialize(), text);
    let ctx = InlineContext {
        link_defs: d.link_defs(),
        footnotes: d.footnote_labels().into_iter().collect(),
    };
    for b in d.blocks() {
        b.walk(&mut |b, _| {
            if !b.is_container() && !b.lines.is_empty() {
                let c = b.content(d.text());
                parse_inlines(&c, d.options(), &ctx);
            }
        });
    }
    t.elapsed()
}

#[test]
fn adversarial_inputs_scale_linearly() {
    for (name, f) in markise_corpus::adversarial() {
        markise_corpus::assert_scales_linearly(name, f, &|t| {
            run(t);
        });
    }
}
