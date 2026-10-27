//! Random patch-set sequences (ADR 0006 §4.1): apply all, undo all through the verbatim
//! inverses → identical bytes; redo through the canonical forwards → identical bytes; mixed
//! CRLF/LF/CR preserved; live marks stay on char boundaries.

use std::sync::Arc;

use markise_doc::{Affinity, LineEnding, Patch, PatchSet};
use proptest::prelude::*;

const PIECES: &[&str] = &["a", "é", "日本", "\r\n", "\n", "\r", " ", "- ", "#"];

fn arb_text() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..40).prop_map(|v| v.concat())
}

fn boundary(t: &str, ix: prop::sample::Index) -> usize {
    let mut p = ix.index(t.len() + 1);
    while !t.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Builds a valid patch set for `t` from random picks (overlaps skipped).
fn patch_set(t: &str, picks: &[(prop::sample::Index, prop::sample::Index, &'static str, bool)]) -> PatchSet {
    let mut ranges: Vec<(usize, usize, &str, bool)> = picks
        .iter()
        .map(|(a, b, s, typed)| {
            let (x, y) = (boundary(t, *a), boundary(t, *b));
            (x.min(y), x.max(y), *s, *typed)
        })
        .collect();
    ranges.sort();
    let mut out = Vec::new();
    let mut end: Option<usize> = None;
    for (s, e, ins, typed) in ranges {
        if end.is_some_and(|x| s < x || (s == x && s == e)) {
            continue;
        }
        out.push(if typed {
            Patch::new(s..e, ins)
        } else {
            Patch::verbatim(s..e, ins)
        });
        end = Some(e);
    }
    PatchSet::new(out).unwrap()
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn apply_undo_redo_is_byte_exact(
        start in arb_text(),
        rounds in prop::collection::vec(
            prop::collection::vec((any::<prop::sample::Index>(), any::<prop::sample::Index>(), prop::sample::select(PIECES), any::<bool>()), 1..4),
            1..12),
        eol in prop::sample::select(&[LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr][..]),
        mark in any::<prop::sample::Index>(),
    ) {
        let mut text = start.clone();
        let mut history: Vec<(PatchSet, Vec<Arc<str>>)> = Vec::new();
        let mut states = vec![text.clone()];
        let mut m = boundary(&text, mark);
        for picks in &rounds {
            let ps = patch_set(&text, picks).canonical_eol(eol);
            prop_assert!(ps.patches().iter().all(|p| p.verbatim));
            ps.validate(&text).unwrap();
            let before = text.clone();
            let removed = ps.apply(&mut text);
            // every boundary maps onto a boundary; Upstream never passes Downstream
            for q in (0..=before.len()).filter(|&q| before.is_char_boundary(q)) {
                let up = ps.map_offset(q, Affinity::Upstream);
                let down = ps.map_offset(q, Affinity::Downstream);
                prop_assert!(up <= down && down <= text.len());
                prop_assert!(text.is_char_boundary(up) && text.is_char_boundary(down));
            }
            let inv = ps.inverse(&removed).unwrap();
            prop_assert!(PatchSet::new(inv.patches().to_vec()).is_ok(), "inverse keeps the invariants: {:?}", inv);
            for q in (0..=text.len()).filter(|&q| text.is_char_boundary(q)) {
                let up = inv.map_offset(q, Affinity::Upstream);
                let down = inv.map_offset(q, Affinity::Downstream);
                prop_assert!(up <= down && down <= before.len());
                prop_assert!(before.is_char_boundary(up) && before.is_char_boundary(down));
            }
            m = ps.map_offset(m, Affinity::Downstream);
            prop_assert!(text.is_char_boundary(m), "live mark on a char boundary");
            history.push((ps, removed));
            states.push(text.clone());
        }
        // undo all
        for (i, (ps, removed)) in history.iter().enumerate().rev() {
            let inv = ps.inverse(removed).unwrap();
            inv.validate(&text).unwrap();
            inv.apply(&mut text);
            prop_assert_eq!(&text, &states[i]);
        }
        prop_assert_eq!(&text, &start, "mixed endings of the original survive");
        // redo all
        for (i, (ps, _)) in history.iter().enumerate() {
            ps.apply(&mut text);
            prop_assert_eq!(&text, &states[i + 1]);
        }
    }
}
