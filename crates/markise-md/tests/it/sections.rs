//! Sections (ADR 0026): what is a boundary, what a metadata block reads as, and after every edit
//! exactly the index of a fresh parse, with the file's bytes untouched.

use markise_doc::FormatDoc;
use markise_md::*;
use proptest::prelude::*;

/// The text of each boundary's position, by line number from 1.
fn boundary_lines(s: &str) -> Vec<usize> {
    let d = parse(s);
    (1..d.section_count())
        .map(|i| {
            let at = d.section(i).unwrap().boundary.unwrap().start;
            s[..at].matches('\n').count() + 1
        })
        .collect()
}

#[test]
fn only_a_root_level_exact_rule_is_a_boundary() {
    assert_eq!(boundary_lines("a\n\n---\n\nb\n"), [3]);
    assert_eq!(
        boundary_lines("---\n\nb\n"),
        [1],
        "a first line that opens no front matter"
    );
    assert_eq!(boundary_lines("a\n\n---"), [3], "with no line ending");
    assert_eq!(boundary_lines("a\r\n\r\n---\r\n\r\nb\r\n"), [3], "CRLF");
    for line in ["***", "___", "----", "- - -", "-- -", " ---", "  ---", "--- ", "---\t"] {
        assert_eq!(
            boundary_lines(&format!("a\n\n{line}\n\nb\n")),
            [] as [usize; 0],
            "{line:?}"
        );
    }
}

#[test]
fn what_markdown_reads_as_something_else_is_no_boundary() {
    let none = [
        "Title\n---\n\nb\n",
        "---\ntitle: x\n---\n\nb\n",
        "> quote\n>\n> ---\n",
        "- item\n\n  ---\n",
        "```\n---\n```\n",
        "    ---\n",
        "<div>\n---\n</div>\n",
        "| a |\n|---|\n",
        "$$\n---\n$$\n",
        "[^f]: note\n\n    ---\n",
        ":::card\n\n---\n\n:::\n",
        "::::outer\n\n:::inner\n\n---\n\n:::\n\n---\n\n::::\n",
    ];
    for s in none {
        assert_eq!(boundary_lines(s), [] as [usize; 0], "{s:?}");
    }
    // an unclosed opener and a stray closer hold nothing
    assert_eq!(boundary_lines(":::card\n\n---\n"), [3]);
    assert_eq!(boundary_lines(":::\n\n---\n"), [3]);
    // a list ends at the blank line and a dedent
    assert_eq!(boundary_lines("- item\n\n---\n"), [3]);
}

#[test]
fn sections_cover_the_document_after_its_front_matter() {
    let s = "---\ntitle: x\n---\n\none\n\n---\n\ntwo\n\n---\n\nthree\n";
    let d = parse(s);
    assert_eq!(d.section_count(), 3);
    let first = d.section(0).unwrap();
    assert_eq!(first.boundary, None);
    assert_eq!(first.start, s.find("\n\none").unwrap());
    assert_eq!(first.end, s.find("---\n\ntwo").unwrap());
    let second = d.section(1).unwrap();
    assert_eq!(second.boundary.unwrap().slice(s), "---");
    assert_eq!(second.end, s.find("---\n\nthree").unwrap());
    assert_eq!(d.section(2).unwrap().end, s.len());
    assert_eq!(d.section(3), None);
    // a position finds its section, and a boundary belongs to the one it opens
    assert_eq!(d.section_at(s.find("one").unwrap()).index, 0);
    assert_eq!(d.section_at(second.boundary.unwrap().start).index, 1);
    assert_eq!(d.section_at(s.find("two").unwrap()).index, 1);
    assert_eq!(d.section_at(s.len()).index, 2);
    // nothing written anywhere
    assert_eq!(d.serialize(), s);
    // an empty document has its one section
    let e = parse("");
    assert_eq!((e.section_count(), e.section_at(0).index), (1, 0));
}

fn pairs(d: &Document, i: usize) -> Vec<(String, String)> {
    let m = d.section_metadata(i).expect("metadata");
    m.pairs.into_iter().map(|p| (p.key, p.value)).collect()
}

fn reasons(s: &str) -> Vec<&'static str> {
    let d = parse(s);
    d.section_metadata(0)
        .expect("metadata")
        .flagged
        .iter()
        .map(|f| f.reason)
        .collect()
}

fn own(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

#[test]
fn metadata_first_in_a_section_reads_as_ordered_pairs() {
    let s = "---\na: b\n---\n\n:::section-metadata\n\nstyle: hero dark\nid: top\n\nnote: 12:30 **literal**\n\n:::\n\n# T\n\n---\n\n:::section-metadata\nkind: other\n:::\n\ntext\n\n---\n\nnone\n";
    let d = parse(s);
    assert_eq!(
        pairs(&d, 0),
        [
            own("style", "hero dark"),
            own("id", "top"),
            own("note", "12:30 **literal**")
        ]
    );
    let m = d.section_metadata(0).unwrap();
    assert_eq!(m.get("id"), Some("top"));
    assert_eq!(m.opener.slice(s), ":::section-metadata");
    assert_eq!(m.closer.slice(s), ":::");
    assert!(m.flagged.is_empty());
    assert_eq!(
        pairs(&d, 1),
        [own("kind", "other")],
        "no blank lines are needed around markers here"
    );
    assert_eq!(d.section_metadata(2), None);
    assert_eq!(d.section_metadata(3), None);
}

#[test]
fn metadata_anywhere_else_is_no_metadata() {
    for s in [
        "text\n\n:::section-metadata\n\na: b\n\n:::\n",
        "# T\n\n:::section-metadata\n\na: b\n\n:::\n",
        "::::outer\n\n:::section-metadata\n\na: b\n\n:::\n\n::::\n",
        ":::section-metadata\n\na: b\n",
        ":::section-metadatas\n\na: b\n\n:::\n",
        "> :::section-metadata\n>\n> a: b\n>\n> :::\n",
    ] {
        assert_eq!(parse(s).section_metadata(0), None, "{s:?}");
    }
    // a second one is an ordinary block; the first one counts
    let d = parse(":::section-metadata\n\na: 1\n\n:::\n\n:::section-metadata\n\nb: 2\n\n:::\n");
    assert_eq!(pairs(&d, 0), [own("a", "1")]);
}

#[test]
fn lines_that_do_not_fit_are_flagged_and_take_nothing_else_with_them() {
    let s = ":::section-metadata\n\nfine: yes\nno colon here\nBad-Key: x\n1key: x\nempty:\nfine: again\n---\n# heading\n\nalso: fine\n\n:::\n";
    let d = parse(s);
    let m = d.section_metadata(0).unwrap();
    assert_eq!(
        m.pairs.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
        ["fine", "also"]
    );
    assert_eq!(m.flagged.len(), 7);
    assert_eq!(m.flagged[0].text, "no colon here");
    assert_eq!(m.flagged[0].at, s.find("no colon").unwrap());
    assert_eq!(m.flagged[4].text, "fine: again", "the first of a repeated key counts");
    assert_eq!(
        m.flagged[5].text, "---",
        "a line under text is what it is in Markdown, here it fits nothing"
    );
    assert_eq!(d.serialize(), s);
    // a long value is kept and flagged, and arguments are flagged on the opener
    let long = "x".repeat(201);
    assert_eq!(reasons(&format!(":::section-metadata\n\na: {long}\n\n:::\n")).len(), 1);
    let exact = "x".repeat(200);
    assert!(reasons(&format!(":::section-metadata\n\na: {exact}\n\n:::\n")).is_empty());
    let d = parse(&format!(":::section-metadata\n\na: {long}\n\n:::\n"));
    assert_eq!(d.section_metadata(0).unwrap().get("a"), Some(long.as_str()));
    assert_eq!(reasons(":::section-metadata wide\n\na: b\n\n:::\n").len(), 1);
    // keys over the limit
    let many: String = (0..22).map(|i| format!("k{i}: v\n")).collect();
    let d = parse(&format!(":::section-metadata\n\n{many}\n:::\n"));
    let m = d.section_metadata(0).unwrap();
    assert_eq!((m.pairs.len(), m.flagged.len()), (20, 2));
    // CRLF reads the same
    let d = parse(":::section-metadata\r\n\r\na: b\r\nc: d\r\n\r\n:::\r\n");
    assert_eq!(pairs(&d, 0), [own("a", "b"), own("c", "d")]);
}

#[test]
fn a_very_large_block_is_not_read() {
    let body = "k: v\n".repeat(MAX_METADATA_BYTES / 5 + 1);
    let d = parse(&format!(":::section-metadata\n\n{body}\n:::\n"));
    assert_eq!(d.section_metadata(0), None);
    assert!(!d.directives_at(0).is_empty());
}

#[test]
fn the_front_matter_names_the_dialect_until_an_edit_reaches_it() {
    let mut d = parse("---\nauthoring: blocks-v3\n---\n\nText\n");
    assert_eq!(Document::authoring(&d), Some("blocks-v3"));
    assert_eq!(FormatDoc::authoring(&d).as_deref(), Some("blocks-v3"));
    let at = d.text().find("blocks-v3").unwrap();
    d.edit(at..at + 9, "blocks-v2");
    assert_eq!(Document::authoring(&d), Some("blocks-v2"));
    assert_eq!(Document::authoring(&parse("Text\n")), None);
    assert_eq!(Document::authoring(&parse("---\nauthoring:\n---\n\nText\n")), None);
    assert_eq!(
        Document::authoring(&parse("---\nauthoring: \"blocks-v3\"\n---\n")),
        Some("blocks-v3")
    );
}

fn line() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "",
        "",
        "text",
        "---",
        "---",
        "---",
        "***",
        "----",
        " ---",
        ":::a",
        ":::section-metadata",
        "::::b",
        ":::",
        ":::",
        "::::",
        "key: value",
        "```",
        "# heading",
        "- item",
        "  ---",
        "> quote",
        "> ---",
        "| a |",
        "|---|",
        "<details>",
    ])
}

fn doc_text() -> impl Strategy<Value = String> {
    prop::collection::vec(line(), 0..30).prop_map(|l| {
        let mut s = l.join("\n");
        s.push('\n');
        s
    })
}

/// Everything the index answers.
fn snap(d: &Document) -> Vec<(markise_doc::SectionAt, Option<markise_doc::SectionMetadata>)> {
    (0..d.section_count())
        .map(|i| (d.section(i).unwrap(), d.section_metadata(i)))
        .collect()
}

proptest! {
    /// Whatever the edit: the index of a fresh parse, in a file that holds what was asked.
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
            let before = d.text().to_string();
            let mut want = before.clone();
            want.replace_range(a..z, &insert);
            d.edit(a..z, &insert);
            prop_assert_eq!(d.text(), want.as_str());
            prop_assert_eq!(d.serialize(), want.clone());
            let fresh = parse(d.text());
            prop_assert_eq!(snap(&d), snap(&fresh), "after {:?} on {:?}", (a..z, &insert), before);
            for p in (0..=d.text().len()).step_by(5) {
                prop_assert_eq!(d.section_at(p), fresh.section_at(p), "at {}", p);
            }
        }
    }

    /// Reading the index changes no byte, and a section's parts lie in order in the text.
    #[test]
    fn sections_are_in_order_and_the_file_is_untouched(s in doc_text()) {
        let d = parse(&s);
        let _ = snap(&d);
        prop_assert_eq!(d.serialize(), s.clone());
        let mut end = 0;
        for i in 0..d.section_count() {
            let x = d.section(i).unwrap();
            prop_assert_eq!(x.index, i);
            if let Some(b) = x.boundary {
                prop_assert_eq!(b.slice(&s), "---");
                prop_assert_eq!(b.start, end, "each section ends where the next boundary starts");
                prop_assert_eq!(x.start, b.end);
            }
            prop_assert!(x.start <= x.end);
            end = x.end;
        }
        prop_assert_eq!(end, s.len());
    }
}

#[test]
fn a_first_part_agrees_with_the_whole_document_up_to_its_cut() {
    let opts = ParseOptions::default();
    let mut text = String::from("---\nauthoring: blocks-v3\n---\n\n");
    for i in 0..400 {
        text.push_str(&format!(
            ":::section-metadata\n\nid: s{i}\n\n:::\n\n## Part {i}\n\nSome text here.\n\n---\n\n"
        ));
    }
    let (cut, part) = progressive_prefix(&text, 4000, &opts).expect("a cut");
    let whole = parse(&text);
    assert!(part.section_count() < whole.section_count());
    let n = part.section_count();
    for i in 0..n - 1 {
        assert_eq!(part.section(i), whole.section(i), "section {i} is complete");
        assert_eq!(part.section_metadata(i), whole.section_metadata(i));
    }
    // the last one is cut short in the part and goes on in the whole
    let (a, b) = (part.section(n - 1).unwrap(), whole.section(n - 1).unwrap());
    assert_eq!((a.boundary, a.start), (b.boundary, b.start));
    assert_eq!(a.end, cut);
    assert!(a.end <= b.end);
    assert_eq!(part.authoring(), whole.authoring());
}

#[test]
fn a_rule_in_a_block_closing_after_the_cut_is_corrected_with_the_whole_document() {
    let opts = ParseOptions::default();
    let filler = "A paragraph that fills the first part of the file.\n\n".repeat(60);
    let text = format!("one\n\n:::card\n\n---\n\n{filler}:::\n\nafter\n\n---\n\nlast\n");
    let (_, part) = progressive_prefix(&text, 1500, &opts).expect("a cut");
    // the opener has no closer in the part, so the rule stands at the root for now …
    assert_eq!(part.section_count(), 2);
    assert_eq!(
        part.section(1).unwrap().boundary.unwrap().start,
        text.find("---").unwrap()
    );
    // … and the whole document has it inside its block
    let whole = parse(&text);
    assert_eq!(whole.section_count(), 2);
    assert_eq!(
        whole.section(1).unwrap().boundary.unwrap().start,
        text.rfind("---").unwrap()
    );
}

#[test]
fn the_example_document_reads_as_it_says() {
    let s = include_str!("../../../../examples/sections/sample.md");
    let d = parse(s);
    assert_eq!(d.authoring(), Some("blocks-v3"));
    assert_eq!(d.serialize(), s);
    assert_eq!(d.section_count(), 5);
    assert_eq!(d.section_metadata(0).unwrap().get("id"), Some("welcome"));
    assert_eq!(
        d.section_metadata(1).unwrap().get("note"),
        Some("time: 12:30, values are literal **as written**")
    );
    assert_eq!(d.section_metadata(2), None);
    let flagged = d.section_metadata(3).unwrap();
    assert_eq!(flagged.get("kind"), Some("practice"));
    assert_eq!(flagged.flagged.len(), 3);
    assert_eq!(d.section_metadata(4).unwrap().get("style"), Some("closing"));
}
