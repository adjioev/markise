//! Cross-block risks fixed by the block-ownership query (ADR 0011): every join, deletion and
//! virtual-paragraph placement takes its neighbour from the parsed structure and never reaches
//! past a block without text (an empty quote `>`, an empty list item). Headless surface;
//! notation: `‸` caret, `(v)` virtual paragraph.

mod common;
use common::T;

fn script(src: &str, steps: &[&str], want: &str) {
    let mut t = T::at(src);
    for k in steps {
        if let Some(s) = k.strip_prefix("type:") {
            t.typ(s);
        } else {
            t.run(k);
        }
        t.check_model();
        assert!(
            t.selection_on_stops(),
            "selection off the stops after {k}: {}",
            t.shown()
        );
    }
    assert_eq!(t.shown(), want, "{src:?} after {steps:?}");
    assert!(t.selection_on_stops(), "selection off the stops: {}", t.shown());
}

#[test]
fn hinted_empty_heading_enter_keeps_an_empty_quote_before_it() {
    // `## ` typed as paragraph text (a literal hint), emptied, Enter (C43): the empty quote
    // before it used to be deleted together with the line
    script(
        "Before\n\n>\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline", "type:x"],
        "Before\n\n>\n\nx‸",
    );
    // C43 in place: the markers go, the empty paragraph is the virtual one after the quote
    script(
        "Before\n\n>\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline"],
        "Before\n\n>(v)‸",
    );
}

#[test]
fn hinted_empty_heading_enter_after_an_empty_nested_container_stays_in_its_own() {
    // C43 in place inside the enclosing container (Codex review round 2 of #37): the empty
    // nested block before the line stays, the paragraph stays in the line's own container
    for (src, want) in [
        ("> >\n>\n> ‸y", "> >\n>\n> x‸"),
        // an empty nested list item inside a quote
        ("> - a\n>   -\n>\n> ‸y", "> - a\n>   -\n>\n> x‸"),
        // an empty quote inside a list item
        ("- a\n\n  >\n\n  ‸y", "- a\n\n  >\n\n  x‸"),
        // the empty block outside the line's own container (Codex review round 3)
        (">\n\n- ‸y", ">\n\n- x‸"),
        ("- >\n- ‸y", "- >\n- x‸"),
    ] {
        script(src, &["type:## ", "edit.deleteForward", "edit.newline", "type:x"], want);
    }
}

#[test]
fn hinted_empty_heading_enter_after_a_quote_leaves_the_quote() {
    // the virtual paragraph after the quote's last paragraph used to open inside the quote
    script(
        "> q\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline", "type:x"],
        "> q\n\nx‸",
    );
    script(
        "- q\n\n‸y",
        &["type:## ", "edit.deleteForward", "edit.newline", "type:x"],
        "- q\n\nx‸",
    );
}

#[test]
fn list_item_paragraph_join_stops_at_an_empty_block() {
    // Backspace at the start of an item's later paragraph used to merge with the paragraph
    // before an empty quote / empty nested item, deleting it
    script("- a\n\n  >\n\n  ‸b\n", &["edit.deleteBackward"], "- a\n\n  >\n\n  ‸b\n");
    script(
        "- a\n\n  - \n\n  ‸b\n",
        &["edit.deleteBackward"],
        "- a\n\n  - \n\n  ‸b\n",
    );
    // a plain join still works
    script("- a\n\n  ‸b\n", &["edit.deleteBackward"], "- a‸b\n");
}

#[test]
fn deleting_the_final_block_keeps_the_block_before_it() {
    // Delete Block on the last block takes the separator before it — up to the previous
    // block's end, never into an empty quote
    script("x\n\n>\n\nab‸c\n", &["delete.block"], "x\n\n>‸\n");
    script("x\n\nab‸c\n", &["delete.block"], "x‸\n");
}

#[test]
fn virtual_paragraph_never_skips_a_block() {
    // Enter at the start of a heading after an empty quote: the virtual paragraph "above"
    // used to open after the paragraph before the quote
    script(
        "Before\n\n>\n\n## ‸Title\n",
        &["edit.newline"],
        "Before\n\n>\n\n## ‸Title\n",
    );
    script("Before\n\n## ‸Title\n", &["edit.newline"], "Before(v)‸\n\n## Title\n");
    // Enter on an empty first list item after a quote: the paragraph goes before the list,
    // outside the quote (it used to open inside it)
    script("> q\n\n- ‸\n- b\n", &["edit.newline", "type:x"], "> q\n\nx‸\n\n- b\n");
}

/// A generated document tree: containers (quote, bullet item, ordered item), the paragraph with
/// the caret, and the empty blocks (an empty quote, an empty bullet item).
#[derive(Clone, Debug)]
enum N {
    Q(Vec<N>),
    B(Vec<N>),
    O(Vec<N>),
    /// the paragraph holding the caret (`‸` before its text)
    P(&'static str),
    EQ,
    EB,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum K {
    Q,
    B,
    O,
}

fn wrap(k: K, ch: Vec<N>) -> N {
    match k {
        K::Q => N::Q(ch),
        K::B => N::B(ch),
        K::O => N::O(ch),
    }
}

/// `ch` nested in the containers `ks` (outermost first).
fn nest(ks: &[K], ch: Vec<N>) -> Vec<N> {
    match ks.split_first() {
        None => ch,
        Some((k, rest)) => vec![wrap(*k, nest(rest, ch))],
    }
}

fn render(ch: &[N], first: &str, cont: &str, out: &mut Vec<String>) {
    for (i, n) in ch.iter().enumerate() {
        let pfx = if i == 0 { first } else { cont };
        if i > 0 {
            out.push(cont.trim_end().to_string());
        }
        match n {
            N::P(t) => out.push(format!("{pfx}{t}")),
            N::EQ => out.push(format!("{pfx}>")),
            // `*`: a `-` here could complete a thematic break (`- - -`)
            N::EB => out.push(format!("{pfx}*")),
            N::Q(c) => render(c, &format!("{pfx}> "), &format!("{cont}> "), out),
            // bullets alternate with depth: three `-` items emptied in place would read as a
            // thematic break (`- - - `), a different block (see the PR's open questions)
            N::B(c) => {
                let m = ["-", "+"][pfx.matches(['-', '+']).count() % 2];
                render(c, &format!("{pfx}{m} "), &format!("{cont}  "), out)
            }
            N::O(c) => render(c, &format!("{pfx}1. "), &format!("{cont}   "), out),
        }
    }
}

fn doc(tree: &[N]) -> String {
    let mut lines = Vec::new();
    render(tree, "", "", &mut lines);
    lines.join("\n") + "\n"
}

fn with_text(tree: &[N], t: &'static str) -> Vec<N> {
    tree.iter()
        .map(|n| match n {
            N::P(_) => N::P(t),
            N::Q(c) => N::Q(with_text(c, t)),
            N::B(c) => N::B(with_text(c, t)),
            N::O(c) => N::O(with_text(c, t)),
            other => other.clone(),
        })
        .collect()
}

/// C43 on an emptied literal `## ` line (Codex review round 3 of #37): for every container
/// stack of the line (depth 0–3 of quote / bullet item / ordered item) and every place of an
/// empty block before it — in the same container, in an ancestor, in a sibling container, in an
/// unrelated top-level container — the paragraph typed afterwards stays in the line's own
/// containers and nothing else changes.
#[test]
fn c43_in_place_for_every_container_stack() {
    let kinds = [K::Q, K::B, K::O];
    let mut stacks: Vec<Vec<K>> = vec![vec![]];
    for d in 1..=3 {
        let mut next = Vec::new();
        for s in stacks.iter().filter(|s| s.len() == d - 1) {
            for k in kinds {
                let mut t = s.clone();
                t.push(k);
                next.push(t);
            }
        }
        stacks.extend(next);
    }
    let mut fails = Vec::new();
    let mut n = 0;
    for c in &stacks {
        for e in [N::EQ, N::EB] {
            let mut trees: Vec<(String, Vec<N>)> = Vec::new();
            // same container
            trees.push(("same".into(), nest(c, vec![e.clone(), N::P("‸y")])));
            for k in 0..c.len() {
                // in an ancestor: before the subtree of `c[k]`, in `c[..k]`
                let mut at_k = vec![e.clone()];
                at_k.extend(nest(&c[k..], vec![N::P("‸y")]));
                trees.push((format!("ancestor{k}"), nest(&c[..k], at_k)));
                // in a sibling container of the same kind as `c[k]`
                let mut sib = vec![wrap(c[k], vec![e.clone()])];
                sib.extend(nest(&c[k..], vec![N::P("‸y")]));
                trees.push((format!("sibling{k}"), nest(&c[..k], sib)));
            }
            // in an unrelated top-level container
            let other = kinds.into_iter().find(|k| c.first() != Some(k)).unwrap();
            let mut un = vec![wrap(other, vec![e.clone()])];
            un.extend(nest(c, vec![N::P("‸y")]));
            trees.push(("unrelated".into(), un));
            for (rel, tree) in trees {
                n += 1;
                let src = doc(&tree);
                let want = doc(&with_text(&tree, "x‸"));
                let mut t = T::at(&src);
                let mut off_stops = None;
                for k in ["type:## ", "edit.deleteForward", "edit.newline", "type:x"] {
                    if let Some(s) = k.strip_prefix("type:") {
                        t.typ(s);
                    } else {
                        t.run(k);
                    }
                    // the app moves a caret that is off the caret stops each frame
                    if off_stops.is_none() && !t.selection_on_stops() {
                        off_stops = Some(format!("off the stops after {k}: {:?}", t.shown()));
                    }
                }
                let got = t.shown();
                if let Some(why) = off_stops {
                    fails.push(format!("{c:?} {e:?} {rel}: {src:?}: {why}"));
                } else if got != want {
                    fails.push(format!("{c:?} {e:?} {rel}: {src:?} -> {got:?}, want {want:?}"));
                }
            }
        }
    }
    assert!(fails.is_empty(), "{} of {n} failed:\n{}", fails.len(), fails.join("\n"));
}
