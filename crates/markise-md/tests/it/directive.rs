//! Fenced directives (ADR 0018): the rule of what is a directive, byte-exact round trips, and
//! after every edit exactly the tree of a fresh parse.

use markise_md::*;
use proptest::prelude::*;

fn tree(s: &str) -> String {
    debug_tree(&parse(s))
}

/// `(name, args)` of each directive, in order of their openers.
fn names(s: &str) -> Vec<(String, String)> {
    let d = parse(s);
    d.directives()
        .iter()
        .map(|v| (v.name.slice(d.text()).to_string(), v.args.slice(d.text()).to_string()))
        .collect()
}

fn pair(name: &str, args: &str) -> (String, String) {
    (name.to_string(), args.to_string())
}

#[test]
fn a_block_with_ordinary_markdown_inside() {
    let s = ":::kind multiple\n\n## Title\n\nSome text\n\n- [ ] one {#a}\n- [x] two {#b}\n\n> quoted\n\n![pic](a.png)\n\n:::\n";
    let t = tree(s);
    assert_eq!(
        t,
        "directive[kind|multiple]\n  h2 \"Title\"\n  para \"Some text\"\n  ul\n    li[ ]\n      para \"one {#a}\"\n    li[x]\n      para \"two {#b}\"\n  quote\n    para \"quoted\"\n  para \"![pic](a.png)\"\n"
    );
    assert_eq!(parse(s).serialize(), s);
}

#[test]
fn no_blank_lines_needed_around_markers() {
    assert_eq!(
        tree("before\n:::note\ninside\n:::\nafter\n"),
        "para \"before\"\ndirective[note|]\n  para \"inside\"\npara \"after\"\n"
    );
    // a table and a lazy paragraph end at a marker, a list and a quote too
    assert_eq!(
        tree(":::note\n| a |\n|---|\n| 1 |\n:::\n"),
        "directive[note|]\n  table1 \"| a |\\n|---|\\n| 1 |\"\n"
    );
    assert_eq!(
        tree(":::note\n- item\n:::\n"),
        "directive[note|]\n  ul\n    li\n      para \"item\"\n"
    );
    assert_eq!(
        tree(":::note\n> quoted\n:::\n"),
        "directive[note|]\n  quote\n    para \"quoted\"\n"
    );
}

#[test]
fn what_is_a_marker() {
    assert_eq!(names(":::a b c  \nx\n:::  \n"), [pair("a", "b c")]);
    assert_eq!(names(":::a some/path\nx\n:::\n"), [pair("a", "some/path")]);
    assert_eq!(names(":::заметка\nx\n:::\n"), [pair("заметка", "")]);
    assert_eq!(names(":::a-b_c1\nx\n:::\n"), [pair("a-b_c1", "")]);
    // indented up to three columns, as a code fence
    assert_eq!(names("   :::a\nx\n  :::\n"), [pair("a", "")]);
    // CRLF
    assert_eq!(names(":::a\r\nx\r\n:::\r\n"), [pair("a", "")]);
    // not markers
    for s in [
        "::: a\nx\n:::\n",
        ":::a:\nx\n:::\n",
        ":::a{b}\nx\n:::\n",
        ":::-a\nx\n:::\n",
        "::a\nx\n::\n",
        "    :::a\n\n    :::\n",
        "text :::a\nx :::\n",
    ] {
        assert_eq!(names(s), [], "{s:?}");
        assert!(!parse(s).has_directives());
    }
}

#[test]
fn an_empty_block() {
    assert_eq!(tree(":::a\n:::\n"), "directive[a|]\n");
    assert_eq!(tree(":::a\n\n\n:::"), "directive[a|]\n");
}

#[test]
fn code_holds_markers_as_code() {
    let fenced = ":::a\n```\n:::\n:::b\n```\n:::\n";
    assert_eq!(names(fenced), [pair("a", "")]);
    assert_eq!(tree(fenced), "directive[a|]\n  fence[] \":::\\n:::b\"\n");
    assert_eq!(names("```\n:::a\n:::\n```\n"), []);
    // an unclosed fence runs to the end of the text as everywhere: its opener stays text
    assert_eq!(tree(":::a\n```\n:::\n"), "para \":::a\"\nfence[] \":::\"\n");
    assert_eq!(names("~~~\n:::a\n~~~\n:::\n"), []);
    // an indented code block and raw HTML hold them too
    assert_eq!(names(":::a\n\n    :::\n\n:::\n"), [pair("a", "")]);
    assert_eq!(tree("<div>\n:::a\n:::\n</div>\n").lines().count(), 1);
    assert_eq!(names("$$\n:::a\n:::\n$$\n"), []);
}

#[test]
fn markers_belong_to_the_document_level() {
    // inside a quote, a list item or a footnote they are text and close nothing
    assert_eq!(names("> :::a\n> x\n> :::\n"), []);
    assert_eq!(names("- :::a\n  x\n  :::\n"), []);
    assert_eq!(names("[^f]: :::a\n    :::\n"), []);
    assert_eq!(
        tree(":::a\n> :::\n- :::\n:::\n"),
        "directive[a|]\n  quote\n    para \":::\"\n  ul\n    li\n      para \":::\"\n"
    );
}

#[test]
fn a_longer_fence_holds_a_shorter_one() {
    let s = "::::outer x\n\nfirst\n\n:::inner\n\nsecond\n\n:::\n\nthird\n\n::::\n";
    assert_eq!(
        tree(s),
        "directive[outer|x]\n  para \"first\"\n  directive[inner|]\n    para \"second\"\n  para \"third\"\n"
    );
    assert_eq!(parse(s).serialize(), s);
    assert_eq!(names(s), [pair("outer", "x"), pair("inner", "")]);
    // three levels
    assert_eq!(
        tree(":::::a\n::::b\n:::c\nx\n:::\n::::\n:::::\n"),
        "directive[a|]\n  directive[b|]\n    directive[c|]\n      para \"x\"\n"
    );
}

#[test]
fn an_unclosed_opener_stays_text_and_swallows_nothing() {
    assert_eq!(
        tree(":::a\n\nrest\n\n# heading\n"),
        "para \":::a\"\npara \"rest\"\nh1 \"heading\"\n"
    );
    // a forgotten closer harms only its own block: the next block still is one
    let s = ":::a\n\none\n\n:::b\n\ntwo\n\n:::\n\nafter\n";
    assert_eq!(
        tree(s),
        "para \":::a\"\npara \"one\"\ndirective[b|]\n  para \"two\"\npara \"after\"\n"
    );
    // a marker line that pairs with nothing is a paragraph of its own
    assert_eq!(tree("foo\n:::a\nbar\n"), "para \"foo\"\npara \":::a\"\npara \"bar\"\n");
}

#[test]
fn a_bare_closer_with_no_opener_stays_text() {
    assert_eq!(tree(":::\n"), "para \":::\"\n");
    assert_eq!(tree("foo\n:::\nbar\n"), "para \"foo\"\npara \":::\"\npara \"bar\"\n");
    assert_eq!(names(":::a\nx\n:::\n:::\n"), [pair("a", "")]);
    assert_eq!(names(":::\n:::a\nx\n:::\n"), [pair("a", "")]);
    // too short for the opener
    assert_eq!(names("::::a\nx\n:::\n"), []);
    // long enough
    assert_eq!(names(":::a\nx\n:::::\n"), [pair("a", "")]);
}

#[test]
fn the_same_fence_does_not_nest() {
    // the first opener is the forgotten one
    assert_eq!(names(":::a\n:::b\n:::\n:::\n"), [pair("b", "")]);
    assert_eq!(
        tree(":::a\n:::b\nx\n:::\n:::\n"),
        "para \":::a\"\ndirective[b|]\n  para \"x\"\npara \":::\"\n"
    );
    // so does a longer one inside a shorter one
    assert_eq!(names(":::a\n::::b\nx\n::::\n:::\n"), [pair("b", "")]);
}

#[test]
fn queries_by_position() {
    let s = "x\n\n::::a 1\n\none\n\n:::b\n\ntwo\n\n:::\n\nthree\n\n::::\n\ny\n";
    let d = parse(s);
    let at = |needle: &str| -> Vec<String> {
        let p = s.find(needle).unwrap();
        d.directives_at(p).iter().map(|v| v.name.slice(s).to_string()).collect()
    };
    assert!(at("x\n").is_empty());
    assert!(at("y\n").is_empty());
    assert_eq!(at("::::a"), ["a"]);
    assert_eq!(at("one"), ["a"]);
    assert_eq!(at(":::b"), ["a", "b"]);
    assert_eq!(at("two"), ["a", "b"]);
    assert_eq!(at(":::\n\nthree"), ["a", "b"]);
    assert_eq!(at("three"), ["a"]);
    assert_eq!(at("::::\n\ny"), ["a"]);
    let all = d.directives();
    assert_eq!(all[0].args.slice(s), "1");
    assert_eq!(
        all[0].span().slice(s),
        &s[s.find("::::a").unwrap()..s.rfind("::::").unwrap() + 4]
    );
    assert_eq!(all[1].opener.slice(s), ":::b");
    assert_eq!(all[1].closer.slice(s), ":::");
}

#[test]
fn trailing_whitespace_on_a_closer_belongs_to_its_line() {
    let mut d = parse(":::a\nx\n:::   \n\nafter\n");
    let in_ws = ":::a\nx\n:::  ".len();
    assert_eq!(d.directives_at(in_ws).len(), 1);
    assert_eq!(d.directives_at(d.text().find("after").unwrap()).len(), 0);
    // typing more of it moves the end of the line
    let end = in_ws + 1;
    d.edit(end..end, "      ");
    assert_eq!(d.directives_at(end + 6).len(), 1);
    assert_eq!(d.directives_at(d.text().find("after").unwrap()).len(), 0);
    // an edit above shifts it
    d.edit(0..0, "new\n\n");
    assert_eq!(d.directives_at(end + 11).len(), 1);
}

#[test]
fn stored_blocks_stay_flat() {
    let d = parse("a\n\n:::n x\n\nb\n\n:::\n\nc\n");
    let kinds: Vec<&BlockKind> = d.blocks().iter().map(|b| &b.kind).collect();
    assert_eq!(kinds.len(), 5);
    assert!(matches!(kinds[1], BlockKind::DirectiveOpen { .. }));
    assert!(matches!(kinds[2], BlockKind::Paragraph));
    assert!(matches!(kinds[3], BlockKind::DirectiveClose { .. }));
    assert_eq!(d.tree().len(), 3);
    assert!(d.tree()[1].is_container());
}

#[test]
fn an_edit_far_away_pairs_without_a_parse_of_what_lies_between() {
    let mut s = String::from(":::a\n\n");
    for i in 0..2000 {
        s.push_str(&format!("paragraph {i}\n\n"));
    }
    let mut d = parse(&s);
    assert!(!d.has_directives());
    let end = d.text().len();
    let r = d.edit(end..end, ":::\n");
    assert!(d.has_directives());
    // only the new line was parsed; the result reports the whole range as changed
    assert_eq!(r.new_blocks, 0..d.top_len());
    assert_eq!(debug_tree(&d), debug_tree(&parse(d.text())));
    // the other way round: removing the closer
    let end = d.text().len();
    let r = d.edit(end - 4..end, "");
    assert!(!d.has_directives());
    assert_eq!(r.new_blocks, 0..d.top_len());
    assert_eq!(debug_tree(&d), debug_tree(&parse(d.text())));
}

#[test]
fn every_keystroke_of_a_new_block() {
    let base = "before\n\nafter one\n\nafter two\n";
    let at = "before\n\n".len();
    let mut d = parse(base);
    let mut pos = at;
    for ch in ":::note x\n\nbody\n\n:::\n\n".chars() {
        let mut buf = [0u8; 4];
        d.edit(pos..pos, ch.encode_utf8(&mut buf));
        pos += ch.len_utf8();
        assert_eq!(debug_tree(&d), debug_tree(&parse(d.text())), "{:?}", d.text());
    }
    assert_eq!(names(d.text()), [pair("note", "x")]);
    // and back
    while pos > at {
        d.edit(pos - 1..pos, "");
        pos -= 1;
        assert_eq!(debug_tree(&d), debug_tree(&parse(d.text())), "{:?}", d.text());
    }
    assert_eq!(d.text(), base);
}

#[test]
fn a_fence_edit_hides_and_shows_markers() {
    let mut d = parse(":::a\n\n```\n:::\n```\n\ntext\n");
    assert!(!d.has_directives());
    // the opening fence goes: the marker in it closes the block, the closing fence opens code
    let p = d.text().find("```").unwrap();
    d.edit(p..p + 4, "");
    assert!(d.has_directives());
    assert_eq!(debug_tree(&d), debug_tree(&parse(d.text())));
    d.edit(p..p, "```\n");
    assert!(!d.has_directives());
    assert_eq!(debug_tree(&d), debug_tree(&parse(d.text())));
}

#[test]
fn deep_and_adversarial_inputs_stay_bounded() {
    // openers of ever shorter fences, all waiting
    let n = 200;
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&":".repeat(3 + n - i));
        s.push_str("a\n");
    }
    for i in 0..n {
        s.push_str(&":".repeat(3 + i));
        s.push('\n');
    }
    let d = parse(&s);
    assert_eq!(d.serialize(), s);
    let mut deepest = 0;
    for b in d.tree() {
        b.walk(&mut |_, depth| deepest = deepest.max(depth));
    }
    assert!(deepest < MAX_DIRECTIVE_DEPTH + MAX_NESTING);
    // many unclosed openers and many stray closers
    let s = ":::a\n".repeat(20_000) + &":::\n".repeat(20_000);
    let d = parse(&s);
    assert_eq!(d.directives().len(), 1);
    assert_eq!(d.serialize(), s);
}

// ---- properties ----------------------------------------------------------------------------

/// Lines that matter to directives: markers of several lengths, what hides them (code, HTML,
/// math), what they interrupt (paragraphs, tables, lists, quotes), blanks.
fn line() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        ":::a",
        ":::b x y",
        "::::c",
        ":::::d e",
        ":::",
        "::::",
        ":::::",
        " ",
        "::: not",
        "  :::i",
        "   :::",
        "    :::",
        "> :::",
        "> :::q",
        "- :::",
        "  :::",
        "text",
        "more text",
        "",
        "",
        "```",
        "~~~",
        "$$",
        "<div>",
        "</div>",
        "<!-- c -->",
        "# heading",
        "| a |",
        "|---|",
        "- item",
        "> quote",
        "---",
        "[r]: /u",
        "[^f]: note",
    ])
}

fn doc_text() -> impl Strategy<Value = String> {
    prop::collection::vec(line(), 0..30).prop_map(|l| {
        let mut s = l.join("\n");
        s.push('\n');
        s
    })
}

fn snap(d: &Document) -> (String, Vec<DirectiveView>) {
    (debug_tree(d), d.directives())
}

/// `true` when both block lists agree on spans and kinds at `i` / `j`.
fn same_top(a: &Document, i: usize, b: &Document, j: usize) -> bool {
    match (a.top(i), b.top(j)) {
        (Some(x), Some(y)) => {
            x.span().len() == y.span().len()
                && x.block.is_container() == y.block.is_container()
                && std::mem::discriminant(&x.block.kind) == std::mem::discriminant(&y.block.kind)
        }
        _ => false,
    }
}

proptest! {
    #[test]
    fn parse_round_trips_byte_for_byte(s in doc_text()) {
        let d = parse(&s);
        prop_assert_eq!(d.serialize(), s.clone());
        prop_assert_eq!(d.text(), s.as_str());
        d.validate().unwrap();
    }

    /// The tree is well formed: directive children lie between its markers, nothing overlaps,
    /// and flattening it gives the stored list back.
    #[test]
    fn the_tree_folds_the_flat_list(s in doc_text()) {
        let d = parse(&s);
        fn flat(b: &Block, out: &mut Vec<(usize, usize)>) {
            if let BlockKind::Directive { opener, closer, .. } = &b.kind {
                out.push((opener.start, b.children.first().map_or(opener.start, |_| opener.start)));
                for c in &b.children {
                    assert!(c.span.start >= opener.end && c.span.end <= closer.start);
                    flat(c, out);
                }
                out.push((closer.start, closer.start));
            } else {
                out.push((b.span.start, b.span.start));
            }
        }
        let mut got = Vec::new();
        for b in &d.tree() {
            flat(b, &mut got);
        }
        let starts: Vec<usize> = got.iter().map(|x| x.0).collect();
        let want: Vec<usize> = d.tops().map(|t| t.start()).collect();
        prop_assert_eq!(starts, want);
    }

    /// Whatever the edit: the same tree and directives as a fresh parse, only the edited range
    /// changed in the text, and blocks outside the reported range are the old ones.
    #[test]
    fn edits_match_a_fresh_parse(
        s in doc_text(),
        ops in prop::collection::vec((any::<prop::sample::Index>(), 0usize..12, line(), any::<bool>()), 1..8),
    ) {
        let mut d = parse(&s);
        for (at, del, ins, newline) in ops {
            let len = d.text().len();
            let mut a = at.index(len + 1);
            while !d.text().is_char_boundary(a) {
                a -= 1;
            }
            let mut z = (a + del).min(len);
            while !d.text().is_char_boundary(z) {
                z -= 1;
            }
            let insert = if newline { format!("{ins}\n") } else { ins.to_string() };
            let before = d.clone();
            let mut want_text = d.text().to_string();
            want_text.replace_range(a..z, &insert);
            let r = d.edit(a..z, &insert);
            prop_assert_eq!(d.text(), want_text.as_str());
            prop_assert_eq!(d.serialize(), want_text.clone());
            let fresh = parse(d.text());
            prop_assert_eq!(snap(&d), snap(&fresh), "after {:?} on {:?}", (a..z, &insert), before.text());
            prop_assert_eq!(d.blocks(), fresh.blocks());
            for p in (0..=d.text().len()).step_by(3) {
                prop_assert_eq!(d.directives_at(p), fresh.directives_at(p), "at {}", p);
            }
            // blocks before and after the reported range are the old ones
            for i in 0..r.new_blocks.start {
                prop_assert!(same_top(&before, i, &d, i), "before the range {}", i);
            }
            let tail_old = r.new_blocks.start + r.replaced;
            let tail = before.top_len().saturating_sub(tail_old);
            prop_assert_eq!(d.top_len() - r.new_blocks.end, tail);
            for k in 0..tail {
                prop_assert!(same_top(&before, tail_old + k, &d, r.new_blocks.end + k), "after the range {}", k);
            }
        }
    }

    /// Same as a fresh parse with undo-like reversal: an edit and its inverse restore the tree.
    #[test]
    fn an_edit_and_its_inverse_restore_the_tree(
        s in doc_text(),
        at in any::<prop::sample::Index>(),
        ins in line(),
    ) {
        let mut d = parse(&s);
        let a = at.index(s.len() + 1);
        let insert = format!("{ins}\n");
        d.edit(a..a, &insert);
        d.edit(a..a + insert.len(), "");
        prop_assert_eq!(d.text(), s.as_str());
        prop_assert_eq!(snap(&d), snap(&parse(&s)));
    }
}

// ---- a block's id (ADR 0018) -------------------------

fn inline_of(src: &str, needle: &str) -> markise_doc::RichInline {
    use markise_doc::{FormatDoc, InlineSource};
    let doc = Document::parse(src.to_string(), ParseOptions::default());
    let at = src.find(needle).expect("in the text");
    let end = src[at..].find('\n').map_or(src.len(), |e| at + e);
    let line = [markise_md::text::Span::new(at, end)];
    doc.inline(InlineSource::Lines(&line), at)
}

#[test]
fn an_id_is_a_hidden_note_on_a_heading_anywhere_and_a_list_item_in_a_block() {
    use markise_doc::InlineKind;
    for (src, needle, shown) in [
        ("## Title {#q}\n", "Title", "Title"),
        ("> ## Title {#q}\n", "Title", "Title"),
        ("# A b  {#T1-x_y}\n", "A b", "A b"),
        (":::a\n\n## Title {#q}\n\n:::\n", "Title", "Title"),
        (":::a\n\n- [ ] One {#a1}\n- Two  {#b}\n\n:::\n", "One", "One"),
        (":::a\n\n- [ ] One {#a1}\n- Two  {#b}\n\n:::\n", "Two", "Two"),
        (":::a\n\n1. **Bold** end {#c}\n\n:::\n", "**Bold", "Bold end"),
    ] {
        let r = inline_of(src, needle);
        assert_eq!(r.plain(&r.nodes), shown, "{src}");
        let last = r.nodes.last().expect("a node");
        assert_eq!(last.kind, InlineKind::Comment, "{src}");
        assert!(!last.markers.is_empty(), "it can be revealed");
        assert!(r.value(last).trim_start().starts_with("{#"), "{src}");
    }
}

#[test]
fn a_heading_of_only_an_id_shows_nothing_and_stays_a_heading() {
    let r = inline_of("## {#q}\n", "{#q}");
    assert_eq!(r.plain(&r.nodes), "");
    let doc = Document::parse("## {#q}\n".to_string(), ParseOptions::default());
    assert_eq!(
        doc.tree()[0].content(doc.text()),
        "{#q}",
        "the plain tree keeps it, for the field model"
    );
}

#[test]
fn an_id_elsewhere_stays_text() {
    for (src, needle, shown) in [
        // a list item or a paragraph outside a block
        ("- One {#a1}\n", "One", "One {#a1}"),
        ("Some text {#x}\n", "Some", "Some text {#x}"),
        // a plain paragraph in a block
        (":::a\n\nSome text {#x}\n\n:::\n", "Some", "Some text {#x}"),
        // a list item with an unpaired opener: no block
        (":::a\n\n- One {#a1}\n", "One", "One {#a1}"),
        // not the exact form
        ("## Title{#q}\n", "Title", "Title{#q}"),
        ("## Title \\{#q}\n", "Title", "Title {#q}"),
        ("## Title {# q}\n", "Title", "Title {# q}"),
        ("## Title {#}\n", "Title", "Title {#}"),
        ("## Title {#q} more\n", "Title", "Title {#q} more"),
        ("## Title {#a}b}\n", "Title", "Title {#a}b}"),
        ("## Title {#q} ##\n", "Title", "Title {#q} ##"),
        ("Title {#q}\n=====\n", "Title", "Title {#q}"),
        // not plain text
        ("## Title `{#q}`\n", "Title", "Title {#q}"),
        (":::a\n\n- Item **{#q}**\n\n:::\n", "Item", "Item {#q}"),
    ] {
        let r = inline_of(src, needle);
        assert_eq!(r.plain(&r.nodes), shown, "{src}");
    }
}

#[test]
fn an_id_that_is_not_plain_text_is_kept_by_every_reader() {
    use markise_doc::FormatDoc;
    for src in [
        "## `literal {#id`}\n",
        "## **bold {#id**}\n",
        ":::a\n\n- `x {#id`}\n\n:::\n",
    ] {
        let doc = Document::parse(src.to_string(), ParseOptions::default());
        let mut lines = Vec::new();
        for b in &doc.tree_for_export() {
            b.walk(&mut |b, _| {
                if !b.lines.is_empty() {
                    lines.push(b.content(doc.text()));
                }
            });
        }
        assert!(
            lines.iter().any(|l| l.ends_with("{#id`}") || l.ends_with("{#id**}")),
            "{src}: {lines:?}"
        );
    }
    let doc = Document::parse("## `literal {#id`}\n".to_string(), ParseOptions::default());
    let at = doc.text().find('`').expect("in the text");
    let info = doc.heading(at).expect("a heading");
    assert!(info.text.contains("{#id"), "{}", info.text);
}

#[test]
fn the_export_tree_leaves_the_id_out_and_the_document_alone() {
    let src = "## Out {#o}\n\n- Plain {#l}\n\n:::a\n\n## Title {#q}\n\nText {#t}\n\n- One {#a1}\n\n:::\n";
    let doc = Document::parse(src.to_string(), ParseOptions::default());
    let mut seen = Vec::new();
    for b in &doc.tree_for_export() {
        b.walk(&mut |b, _| {
            if !b.lines.is_empty() {
                seen.push(b.content(doc.text()));
            }
        });
    }
    assert_eq!(seen, ["Out", "Plain {#l}", "Title", "Text {#t}", "One"]);
    assert_eq!(doc.text(), src);
    let mut original = Vec::new();
    for b in &doc.tree() {
        b.walk(&mut |b, _| {
            if !b.lines.is_empty() {
                original.push(b.content(doc.text()));
            }
        });
    }
    assert!(original.contains(&"Title {#q}".to_string()), "the plain tree keeps it");
    assert!(
        Document::parse("# Plain\n".to_string(), ParseOptions::default())
            .export_tree()
            .is_none()
    );
}
