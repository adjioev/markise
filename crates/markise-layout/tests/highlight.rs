//! Code highlighting in layout: plain first, recolored when runs arrive, cache keys include the
//! highlight state, segmented blocks map runs to the right lines.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use markise_layout::block::{NoImages, Prim};
use markise_layout::{CodeQuery, Highlights, ItemKind, View, testing};
use markise_theme::{Theme, TokenKind, TokenRun};

fn model(src: &str) -> markise_layout::testing::Shown {
    markise_layout::testing::Shown::new(Arc::new(markise_md::parse(src)))
}

/// Marks every `fn` in a block's content as a keyword once `deliver` runs.
#[derive(Default)]
struct Fake {
    queued: Mutex<HashMap<u64, String>>,
    done: Mutex<HashMap<u64, Vec<TokenRun>>>,
    wants: AtomicU64,
    generation: AtomicU64,
}

impl Fake {
    fn deliver(&self) {
        for (k, content) in self.queued.lock().unwrap().drain() {
            let runs = content
                .match_indices("fn")
                .map(|(i, _)| TokenRun {
                    start: i as u32,
                    end: i as u32 + 2,
                    kind: TokenKind::Keyword,
                })
                .collect();
            self.done.lock().unwrap().insert(k, runs);
        }
    }
}

impl Highlights for Fake {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    fn ready(&self, q: &CodeQuery) -> bool {
        self.done.lock().unwrap().contains_key(&q.code.key)
    }

    fn runs(&self, q: &CodeQuery) -> Option<Vec<TokenRun>> {
        let r = q.range();
        self.done.lock().unwrap().get(&q.code.key).map(|v| {
            v.iter()
                .filter(|t| (t.end as usize) > r.start && (t.start as usize) < r.end)
                .copied()
                .collect()
        })
    }

    fn want(&self, q: &CodeQuery) {
        self.wants.fetch_add(1, Ordering::Relaxed);
        self.queued
            .lock()
            .unwrap()
            .entry(q.code.key)
            .or_insert_with(|| q.content());
    }
}

/// `(text, color)` of every text section of item `i`'s layout.
fn sections(v: &mut View, i: usize) -> Vec<(String, epaint::Color32)> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let l = v.layout(i, &mut f, &NoImages);
    let mut out = Vec::new();
    for p in &l.prims {
        if let Prim::Text { text, .. } = p {
            let galley = text.galley().unwrap();
            for s in &galley.job.sections {
                let t = &galley.job.text[s.byte_range.start.0..s.byte_range.end.0];
                out.push((t.to_string(), s.format.color));
            }
        }
    }
    out
}

fn keyword_color(t: &Theme) -> epaint::Color32 {
    markise_layout::block::color(t.syntax.keyword)
}

#[test]
fn plain_first_then_recolored() {
    let theme = Theme::light();
    let m = model("text\n\n```rust\nfn a() {}\nlet b = fn_x;\n```\n");
    let fake = Arc::new(Fake::default());
    let mut v = m.view(theme.clone(), 600.0, 1.0);
    v.set_highlights(fake.clone());
    let code = m
        .items
        .iter()
        .position(|i| matches!(i.kind, ItemKind::Code { .. }))
        .unwrap();
    let kw = keyword_color(&theme);

    let s = sections(&mut v, code);
    assert!(s.iter().all(|(_, c)| *c != kw), "plain before highlighting: {s:?}");
    assert_eq!(fake.wants.load(Ordering::Relaxed), 1, "requested");
    // cache hit while pending: no relayout, request repeated (cheap no-op on the service side)
    v.laid_out.clear();
    sections(&mut v, code);
    assert!(v.laid_out.is_empty(), "pending: cached plain layout reused");

    fake.deliver();
    let s = sections(&mut v, code);
    assert_eq!(v.laid_out, vec![code], "highlight arrival changes the key");
    let colored: Vec<&str> = s.iter().filter(|(_, c)| *c == kw).map(|(t, _)| t.as_str()).collect();
    assert_eq!(colored, ["fn", "fn"], "{s:?}");
    // the text is unchanged
    let text: String = s.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().concat();
    assert!(text.starts_with("fn a() {}\nlet b = fn_x;"), "{text:?}");

    // generation change (e.g. highlighting switched off and on): new key
    v.laid_out.clear();
    fake.generation.store(7, Ordering::Relaxed);
    sections(&mut v, code);
    assert_eq!(v.laid_out, vec![code]);
    // theme change recolors without new highlight requests
    let wants = fake.wants.load(Ordering::Relaxed);
    let dark = Theme::dark();
    v.set_params(600.0, &dark, 1.0);
    let s = sections(&mut v, code);
    assert!(s.iter().any(|(t, c)| t == "fn" && *c == keyword_color(&dark)));
    assert_eq!(fake.wants.load(Ordering::Relaxed), wants);
}

#[test]
fn no_language_and_indented_code_are_not_queried() {
    let m = model("```\nfn a\n```\n\n    fn b\n");
    assert!(m.items.iter().all(|i| i.code.is_none()));
    let fake = Arc::new(Fake::default());
    let mut v = m.view(Theme::light(), 600.0, 1.0);
    v.set_highlights(fake.clone());
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.frame(800.0, 0.0, &mut f, &NoImages);
    assert_eq!(fake.wants.load(Ordering::Relaxed), 0);
}

#[test]
fn heights_unchanged_by_highlighting() {
    let theme = Theme::light();
    let m = model("```rust\nfn a() {}\nfn b() {}\n```\n");
    let fake = Arc::new(Fake::default());
    let mut v = m.view(theme, 600.0, 1.0);
    v.set_highlights(fake.clone());
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    let h0 = v.layout(0, &mut f, &NoImages).height;
    fake.deliver();
    let h1 = v.layout(0, &mut f, &NoImages).height;
    assert_eq!(h0, h1);
}

/// A segmented block (in a quote, so lines are not contiguous in the document): every
/// segment's runs land on its own `fn`s, and the segment ranges tile the block content.
#[test]
fn segmented_block_maps_runs_per_segment() {
    let theme = Theme::light();
    let mut src = String::from("> ```rust\n");
    for i in 0..400 {
        src.push_str(&format!("> fn f{i}() {{}}\n"));
    }
    src.push_str("> ```\n");
    let m = model(&src);
    let segs: Vec<usize> = (0..m.items.len()).filter(|&i| m.items[i].code.is_some()).collect();
    assert!(segs.len() > 2, "segmented: {}", segs.len());
    let key = m.items[segs[0]].code.unwrap().key;
    assert!(
        segs.iter().all(|&i| m.items[i].code.unwrap().key == key),
        "one block key"
    );
    // offsets: each segment starts where the previous one's content ended (+ newline)
    let line = "fn f0() {}\n".len();
    assert_eq!(m.items[segs[0]].code.unwrap().offset, 0);
    assert!(m.items[segs[1]].code.unwrap().offset >= 64 * line - 64);

    let fake = Arc::new(Fake::default());
    let mut v = m.view(theme.clone(), 600.0, 1.0);
    v.set_highlights(fake.clone());
    sections(&mut v, segs[0]);
    fake.deliver();
    let kw = keyword_color(&theme);
    let mut total = 0;
    for &i in &segs {
        let s = sections(&mut v, i);
        let colored: Vec<&str> = s.iter().filter(|(_, c)| *c == kw).map(|(t, _)| t.as_str()).collect();
        let lines: usize = s.iter().map(|(t, _)| t.matches("() {}").count()).sum();
        assert!(colored.iter().all(|t| *t == "fn"), "segment {i}: {colored:?}");
        assert_eq!(colored.len(), lines, "one keyword per line in segment {i}");
        total += colored.len();
    }
    assert_eq!(total, 400);
}

#[test]
fn over_long_line_pieces_keep_offsets() {
    // a 10 KB line is split into pieces (MAX_UNIT_BYTES); runs crossing piece boundaries and
    // later lines must still line up
    let long = "x".repeat(10_000);
    let mut src = format!("```rust\n{long}fn\n");
    for _ in 0..300 {
        src.push_str("fn\n");
    }
    src.push_str("```\n");
    let m = model(&src);
    let theme = Theme::light();
    let fake = Arc::new(Fake::default());
    let mut v = m.view(theme.clone(), 600.0, 1.0);
    v.set_highlights(fake.clone());
    let segs: Vec<usize> = (0..m.items.len()).filter(|&i| m.items[i].code.is_some()).collect();
    sections(&mut v, segs[0]);
    fake.deliver();
    let kw = keyword_color(&theme);
    let mut total = 0;
    for &i in &segs {
        let s = sections(&mut v, i);
        for (t, c) in &s {
            if *c == kw {
                assert_eq!(t, "fn");
                total += 1;
            }
        }
    }
    assert_eq!(total, 301);
}

/// Comments everything when the block content starts with `/*`, else marks every `fn`: its
/// result for a segment depends on the block's first line (like syntect's parser state).
#[derive(Default)]
struct Stateful {
    done: Mutex<HashMap<u64, (bool, Vec<usize>)>>,
}

impl Highlights for Stateful {
    fn generation(&self) -> u64 {
        1
    }

    fn ready(&self, q: &CodeQuery) -> bool {
        self.done.lock().unwrap().contains_key(&q.code.key)
    }

    fn runs(&self, q: &CodeQuery) -> Option<Vec<TokenRun>> {
        let r = q.range();
        let d = self.done.lock().unwrap();
        let (comment, fns) = d.get(&q.code.key)?;
        Some(if *comment {
            vec![TokenRun {
                start: r.start as u32,
                end: r.end as u32,
                kind: TokenKind::Comment,
            }]
        } else {
            fns.iter()
                .filter(|&&i| i >= r.start && i + 2 <= r.end)
                .map(|&i| TokenRun {
                    start: i as u32,
                    end: i as u32 + 2,
                    kind: TokenKind::Keyword,
                })
                .collect()
        })
    }

    fn want(&self, q: &CodeQuery) {
        let c = q.content();
        let fns = c.match_indices("fn").map(|(i, _)| i).collect();
        self.done.lock().unwrap().insert(q.code.key, (c.starts_with("/*"), fns));
    }
}

fn long_fence(first: &str, quote: &str) -> String {
    let mut src = format!("{quote}```rust\n{quote}{first}\n");
    for i in 0..300 {
        src.push_str(&format!("{quote}fn f{i}() {{}}\n"));
    }
    src.push_str(&format!("{quote}```\n"));
    src
}

/// Review P2: a same-length edit of an early line changes the colors of later segments whose
/// own text is unchanged; their highlighted layouts must not be reused.
#[test]
fn edit_changing_parser_state_recolors_later_segments() {
    for quote in ["", "> "] {
        let theme = Theme::light();
        let a = model(&long_fence("// x", quote));
        let b = model(&long_fence("/* x", quote));
        let segs: Vec<usize> = (0..a.items.len()).filter(|&i| a.items[i].code.is_some()).collect();
        let last = *segs.last().unwrap();
        assert!(segs.len() >= 3);
        assert_eq!(
            a.items[last].hash, b.items[last].hash,
            "the late segment's own text is identical"
        );
        let hl = Arc::new(Stateful::default());
        let mut v = a.view(theme.clone(), 600.0, 1.0);
        v.set_highlights(hl.clone());
        sections(&mut v, last); // requests
        let s = sections(&mut v, last);
        assert!(
            s.iter().any(|(_, c)| *c == keyword_color(&theme)),
            "{quote:?}: keywords"
        );
        v.set_model(b.doc.clone(), b.model.clone());
        sections(&mut v, last); // requests the new block
        let s = sections(&mut v, last);
        let comment = markise_layout::block::color(theme.syntax.comment);
        assert!(
            s.iter().all(|(t, c)| *c == comment || t.trim().is_empty()),
            "{quote:?}: stale colors reused: {s:?}"
        );
    }
}

/// Malformed runs from a highlighter (out of range, overlapping, unsorted, splitting
/// multibyte characters) never panic and never change the text.
#[test]
fn malformed_runs_are_harmless() {
    struct Bad(Vec<TokenRun>);
    impl Highlights for Bad {
        fn generation(&self) -> u64 {
            1
        }
        fn ready(&self, _q: &CodeQuery) -> bool {
            true
        }
        fn runs(&self, _q: &CodeQuery) -> Option<Vec<TokenRun>> {
            Some(self.0.clone())
        }
        fn want(&self, _q: &CodeQuery) {}
    }
    let src = "```rust\nfn é界😀 = \"ü\";\nlet 界 = 1;\n```\n";
    let m = model(src);
    let content = "fn é界😀 = \"ü\";\nlet 界 = 1;";
    let mut seed = 0x1234_5678u64;
    let mut rnd = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for _ in 0..300 {
        let runs: Vec<TokenRun> = (0..rnd(8))
            .map(|_| {
                let a = rnd(60) as u32;
                TokenRun {
                    start: a,
                    end: a + rnd(20) as u32,
                    kind: TokenKind::ALL[rnd(20) as usize],
                }
            })
            .collect();
        let mut v = m.view(Theme::light(), 600.0, 1.0);
        v.set_highlights(Arc::new(Bad(runs)));
        let text: String = sections(&mut v, 0).iter().map(|(t, _)| t.as_str()).collect();
        assert!(text.starts_with(content), "{text:?}");
    }
}
