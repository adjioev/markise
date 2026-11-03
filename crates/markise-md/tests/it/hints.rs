//! Literal-line hints (ADR 0006 §2.6) and patch-set application (§2.1): incremental re-parse
//! with hints equals a full parse with the same hints; hints never change bytes.

use markise_md::{Document, HintOrigin, HintSet, LiteralHint, ParseOptions, Patch, PatchSet, debug_tree};
use proptest::prelude::*;

const LINES: &[&str] = &[
    "- ",
    "-",
    "# T",
    "```",
    "~~~ js",
    "para",
    "> q",
    "1. x",
    "",
    "",
    "---",
    "* a",
    "| a |",
    "text **b**",
    "$$",
    "> - x",
    "- [ ] - y",
    "> > q",
    "  - n",
    "1. ",
];

fn full_parse(text: &str, hints: &HintSet) -> Document {
    let mut d = Document::parse(text, ParseOptions::default());
    d.set_hints(hints);
    // set_options re-parses everything with the current hints (no incremental path)
    d.set_options(ParseOptions::default());
    d
}

fn arb_doc() -> impl Strategy<Value = (String, Vec<usize>)> {
    (
        prop::collection::vec(prop::sample::select(LINES), 1..12),
        prop::bool::ANY,
        prop::collection::vec(any::<prop::sample::Index>(), 0..3),
    )
        .prop_map(|(lines, crlf, picks)| {
            let eol = if crlf { "\r\n" } else { "\n" };
            let text = lines.join(eol) + eol;
            let starts: Vec<usize> = {
                let mut v = vec![0];
                v.extend(text.match_indices('\n').map(|(i, _)| i + 1).filter(|&i| i < text.len()));
                v
            };
            let hinted = picks.iter().map(|ix| starts[ix.index(starts.len())]).collect();
            (text, hinted)
        })
}

/// Case count: `PROPTEST_CASES` when set (local stress runs), else 256.
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn incremental_with_hints_equals_full(
        (text, hinted) in arb_doc(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), 0usize..4, prop::sample::select(&["", "x", "\n", "- ", "```\n", "# "][..])), 1..4),
    ) {
        // hints at a column (the containers before it keep their meaning) and ForceBlock
        // hints (M2c) next to whole-line literal hints
        let hints = HintSet::from_hints(hinted.iter().enumerate().map(|(k, &l)| {
            let line_len = text[l..].find(['\r', '\n']).unwrap_or(text.len() - l);
            match k % 3 {
                0 => LiteralHint::new(l, HintOrigin::RuleTrigger),
                1 => LiteralHint::at(l, (2.min(line_len)) as u32, HintOrigin::EnterPending),
                _ => LiteralHint::new(l, HintOrigin::ForceBlock),
            }
        }));
        let mut d = Document::parse(text.as_str(), ParseOptions::default());
        d.set_hints(&hints);
        prop_assert_eq!(d.text(), text.as_str(), "hints never change bytes");
        let full = full_parse(&text, d.hints());
        prop_assert_eq!(d.blocks(), full.blocks(), "after set_hints\n{}---\n{}", debug_tree(&d), debug_tree(&full));

        // one random patch set (non-overlapping, char boundaries: the text is ASCII)
        let len = d.text().len();
        let mut patches = Vec::new();
        let mut used_end = 0usize;
        let mut picks: Vec<_> = edits.iter().map(|(ix, del, ins)| (ix.index(len + 1), *del, *ins)).collect();
        picks.sort();
        for (at, del, ins) in picks {
            if at < used_end || (at == used_end && used_end > 0) {
                continue;
            }
            let end = (at + del).min(len);
            patches.push(Patch::verbatim(at..end, ins));
            used_end = end.max(at + 1);
        }
        let Ok(ps) = PatchSet::new(patches) else { return Ok(()); };
        let before = d.text().to_string();
        // the atomic path with an arbitrary final hint set agrees with a full parse too
        let mut atomic = d.clone();
        let after_text = ps.apply_to(&before);
        let finals = HintSet::from_hints(
            hinted.iter().map(|&l| l.min(after_text.len())).map(|l| LiteralHint::new(l, HintOrigin::EnterPending)),
        );
        atomic.apply_patches_with_hints(&ps, Some(&finals));
        let full_a = full_parse(atomic.text(), atomic.hints());
        prop_assert_eq!(atomic.blocks(), full_a.blocks(), "atomic {:?}\n{}---\n{}", ps, debug_tree(&atomic), debug_tree(&full_a));
        d.apply_patches(&ps);
        prop_assert_eq!(d.text(), ps.apply_to(&before));
        let full = full_parse(d.text(), d.hints());
        prop_assert_eq!(d.blocks(), full.blocks(), "after patches {:?}\n{}---\n{}", ps, debug_tree(&d), debug_tree(&full));

        // clearing the hints restores the unhinted projection
        d.set_hints(&HintSet::new());
        let plain = Document::parse(d.text(), ParseOptions::default());
        prop_assert_eq!(d.blocks(), plain.blocks());
    }
}

#[test]
fn batch_renumber_is_one_patch_set() {
    // insert_list_item is now one PatchSet (new item + renumbered markers): C11
    let mut d = Document::parse("1. a\n2. b\n3. c\n", ParseOptions::default());
    markise_md::ops::insert_list_item(&mut d, &[0], 1, "new", None).unwrap();
    assert_eq!(d.text(), "1. a\n2. new\n3. b\n4. c\n");
    let ps = markise_md::ops::plan_insert_list_item(&d, &[0], 0, "z", None).unwrap();
    assert_eq!(ps.len(), 5, "one insertion + four renumbered markers");
}

#[test]
fn atomic_hints_keep_the_reparse_local() {
    // completing ``` on a line whose hint arrives with the same keystroke
    let mut text = String::from("a\n\n``\n\n");
    for i in 0..5000 {
        text.push_str(&format!("para {i}\n\n"));
    }
    let hint = HintSet::from_hints([LiteralHint::new(3, HintOrigin::EnterPending)]);
    let ps = PatchSet::single_verbatim(5..5, "`");

    let mut atomic = Document::parse(text.as_str(), ParseOptions::default());
    let r = atomic.apply_patches_with_hints(&ps, Some(&hint));
    assert!(r.reparsed.len() < 64, "local re-parse only: {:?}", r.reparsed);

    // the two-phase path (text first, hint after) parses the whole suffix as code first
    let mut two_phase = Document::parse(text.as_str(), ParseOptions::default());
    let r1 = two_phase.apply_patches(&ps);
    assert!(r1.reparsed.len() > 10_000, "{:?}", r1.reparsed);
    two_phase.set_hints(&hint);
    assert_eq!(atomic.blocks(), two_phase.blocks());
    assert_eq!(atomic.hints(), two_phase.hints());
}

#[test]
fn final_hints_outside_the_edit_are_projected_too() {
    let mut d = Document::parse("- a\n\nb\n\n- c\n", ParseOptions::default());
    // edit line "b", but the final hint set (also) hints the far line "- c"
    let hints = HintSet::from_hints([LiteralHint::new(9, HintOrigin::RuleTrigger)]); // post-edit coordinates
    d.apply_patches_with_hints(&PatchSet::single_verbatim(6..6, "x"), Some(&hints));
    assert_eq!(d.text(), "- a\n\nbx\n\n- c\n");
    let mut full = Document::parse(d.text(), ParseOptions::default());
    full.set_hints(d.hints());
    full.set_options(ParseOptions::default());
    assert_eq!(d.blocks(), full.blocks());
    assert!(debug_tree(&d).ends_with("para \"- c\"\n"), "{}", debug_tree(&d));
}
