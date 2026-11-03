//! Block ownership (ADR 0011): the owner of every offset of edge-case documents, as a map
//! with one character per offset, and the neighbours from the structure.

use markise_doc::{BlockPart, FormatDoc};
use markise_md::{Document, ParseOptions};

fn parse(s: &str) -> Document {
    Document::parse(s, ParseOptions::default())
}

/// One character per offset `0..=len`: the part (`P`refix, `M`arker, `T`ext, `L`ine break,
/// `W` trailing whitespace, `C`losing, `.` between blocks) …
fn parts(d: &Document) -> String {
    (0..=d.text().len())
        .map(|at| match d.block_owner(at).map(|o| o.part) {
            None => '.',
            Some(BlockPart::Prefix) => 'P',
            Some(BlockPart::Marker) => 'M',
            Some(BlockPart::Text) => 'T',
            Some(BlockPart::LineBreak) => 'L',
            Some(BlockPart::TrailingWs) => 'W',
            Some(BlockPart::Closing) => 'C',
            Some(BlockPart::Gap) => 'G',
        })
        .collect()
}

/// … and the index (in document order) of the owning leaf (`.` between blocks).
fn owners(d: &Document) -> String {
    let mut starts: Vec<usize> = (0..=d.text().len())
        .filter_map(|at| d.block_owner(at).map(|o| o.leaf.span.start))
        .collect();
    starts.dedup();
    starts.sort_unstable();
    starts.dedup();
    (0..=d.text().len())
        .map(|at| match d.block_owner(at) {
            None => '.',
            Some(o) => char::from(b'0' + starts.iter().position(|&s| s == o.leaf.span.start).unwrap() as u8),
        })
        .collect()
}

fn check(src: &str, want_parts: &str, want_owners: &str) {
    let d = parse(src);
    assert_eq!(parts(&d), want_parts, "parts of {src:?}");
    assert_eq!(owners(&d), want_owners, "owners of {src:?}");
}

#[test]
fn heading_without_trailing_spaces() {
    check("## Title\n\np", "MMMTTTTTT.TT", "000000000.11");
}

#[test]
fn heading_with_trailing_spaces_be1() {
    check("## Title  \n\np", "MMMTTTTTTWW.TT", "00000000000.11");
}

#[test]
fn closed_atx_heading() {
    check("## T ##  \nx", "MMMTTWCCCCTT", "000000000011");
}

#[test]
fn setext_heading_with_trailing_spaces() {
    check("T  \n===  \n\nb", "TTWWCCCCCC.TT", "0000000000.11");
}

#[test]
fn empty_atx_heading() {
    check("## \n\nb", "MMMT.TT", "0000.11");
}

#[test]
fn empty_atx_heading_without_space() {
    check("##\n\nb", "MMT.TT", "000.11");
}

#[test]
fn lone_hash_final_block() {
    check("#", "MT", "00");
}

#[test]
fn list_items_last_paragraph() {
    check("- a\n- b  \n\nc", "PPTTPPTTWW.TT", "0000111111.22");
}

#[test]
fn list_item_second_paragraph() {
    check("- a\n\n  b  \n- c", "PPTT.PPTTWWPPTT", "0000.1111112222");
}

#[test]
fn quote_with_empty_line() {
    check("> a\n>\n> b", "PPTT..PPTT", "0000..1111");
}

#[test]
fn nested_quote_in_list_in_quote() {
    check("> - a\n>   > q  \n\nz", "PPPPTTPPPPPPTTWW.TT", "0000001111111111.22");
}

#[test]
fn front_matter() {
    check("---\nk: v\n---\n\nz", "MMMMTTTTTCCCC.TT", "0000000000000.11");
}

#[test]
fn html_block() {
    check("<div>\nx\n</div>\n\nz", "TTTTTTTTTTTTTTT.TT", "000000000000000.11");
}

#[test]
fn code_fence_with_trailing_spaces() {
    check(
        "```rs\ncode\n```  \n\nz",
        "MMMMMMTTTTTCCCCCC.TT",
        "00000000000000000.11",
    );
}

#[test]
fn thematic_break_between_paragraphs() {
    check("a\n\n---\n\nb", "TT.TCCC.TT", "00.1111.22");
}

#[test]
fn thematic_break_final_without_newline() {
    check("a\n\n***", "TT.TCCC", "00.1111");
}

#[test]
fn final_paragraph_without_newline() {
    check("para", "TTTTT", "00000");
}

#[test]
fn final_paragraph_with_trailing_spaces() {
    check("para  ", "TTTTTWW", "0000000");
}

#[test]
fn crlf_heading_with_trailing_spaces() {
    check("a\r\n\r\n## b  \r\nc", "TT...MMMTTWW.TT", "00...1111111.22");
}

#[test]
fn table_with_trailing_spaces() {
    check(
        "| a | b |\n|---|---|\n| 1 | 2 |  \n\nz",
        "TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTCC.TT",
        "00000000000000000000000000000000.11",
    );
}

#[test]
fn empty_quote_between_paragraphs() {
    check("a\n\n>\n\nb", "TT....TT", "00....11");
}

#[test]
fn empty_list_item_between_paragraphs() {
    check("a\n\n-\n\nb", "TT....TT", "00....11");
}

#[test]
fn empty_list_item_between_items() {
    check("- a\n-\n- b", "PPTT..PPTT", "0000..1111");
}

/// `(block start, leaf start)` of the neighbour of the leaf starting at `at`.
fn adj(d: &Document, at: usize, forward: bool) -> Option<(usize, Option<usize>)> {
    d.adjacent_block(at, forward)
        .map(|a| (a.block.start, a.leaf.map(|l| l.span.start)))
}

#[test]
fn neighbours_never_skip_a_block() {
    // an empty quote / an empty list item between two paragraphs is the neighbour, without leaf
    for src in ["a\n\n>\n\nb", "a\n\n-\n\nb"] {
        let d = parse(src);
        assert_eq!(adj(&d, 0, true), Some((3, None)), "{src:?}");
        assert_eq!(adj(&d, 6, false), Some((3, None)), "{src:?}");
        assert_eq!(d.adjacent_leaf(&d.leaf(0).unwrap(), true), Some(None));
        // from the blank lines between them too
        assert_eq!(adj(&d, 2, true), Some((3, None)), "{src:?}");
        assert_eq!(adj(&d, 5, false), Some((3, None)), "{src:?}");
    }
    // an empty item between two items
    let d = parse("- a\n-\n- b");
    assert_eq!(adj(&d, 2, true), Some((4, None)));
    assert_eq!(adj(&d, 8, false), Some((4, None)));
}

#[test]
fn neighbours_climb_out_of_and_into_containers() {
    let d = parse("- a\n- b  \n\nc");
    assert_eq!(adj(&d, 2, true), Some((4, Some(6))), "next item");
    assert_eq!(adj(&d, 6, true), Some((11, Some(11))), "out of the list");
    assert_eq!(adj(&d, 11, false), Some((0, Some(6))), "the list, its last leaf");
    assert_eq!(adj(&d, 2, false), None, "first block");
    assert_eq!(adj(&d, 11, true), None, "last block");
    let d = parse("> - a\n>   > q  \n\nz");
    assert_eq!(adj(&d, 4, true), Some((10, Some(12))), "into the nested quote");
    assert_eq!(
        adj(&d, 17, false),
        Some((0, Some(12))),
        "the outer quote, its deepest last leaf"
    );
    // between blocks: the nearest block of the innermost container holding the offset
    let d = parse("a\n\n---\n\nb");
    assert_eq!(adj(&d, 2, true), Some((3, Some(3))));
    assert_eq!(adj(&d, 2, false), Some((0, Some(0))));
}

#[test]
fn biased_owner_picks_a_side_between_blocks() {
    use markise_doc::Affinity::{Downstream, Upstream};
    let d = parse("a\n\n>\n\nb");
    let o = d.block_owner_biased(1, Upstream).unwrap();
    assert_eq!(
        (o.leaf.span.start, o.part),
        (0, BlockPart::Text),
        "owned offsets ignore the bias"
    );
    let o = d.block_owner_biased(2, Upstream).unwrap();
    assert_eq!((o.leaf.span.start, o.part, o.at), (0, BlockPart::Gap, 2));
    // the empty quote stops both sides
    assert!(d.block_owner_biased(2, Downstream).is_none());
    assert!(d.block_owner_biased(5, Upstream).is_none());
    assert_eq!(d.block_owner_biased(5, Downstream).map(|o| o.leaf.span.start), Some(6));
}

#[test]
fn owner_ranges() {
    let d = parse("- ## Title  \n\nx");
    let o = d.block_owner(11).unwrap();
    assert_eq!(o.part, BlockPart::TrailingWs);
    assert_eq!((o.prefix.start, o.prefix.end), (0, 2));
    assert_eq!((o.marker.start, o.marker.end), (2, 5));
    assert_eq!((o.content.start, o.content.end), (5, 10));
    assert_eq!((o.trailing_ws.start, o.trailing_ws.end), (10, 12));
    assert_eq!(o.text_end(), 10);
    assert!(o.outside_span());
    assert_eq!(o.clamp_to_content(), 10);
    assert_eq!(o.leaf.containers.len(), 2);
    let c = d.containers_at(11);
    assert!(c.is_empty(), "the trailing spaces lie outside the list's span: {c:?}");
    let c = d.containers_at(6);
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].span.start, c[1].span.start), (0, 0));
}

#[test]
fn containers_at_an_empty_item() {
    let d = parse("> - a\n> -\n");
    let c = d.containers_at(8);
    let kinds: Vec<_> = c.iter().map(|c| format!("{:?}", c.kind)).collect();
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    assert!(d.block_owner(8).is_none());
}

/// Every offset of a larger document: an offset inside a leaf's span is owned by that leaf, an
/// owned offset's leaf contains it or touches it on its line, and the neighbours of each leaf
/// are mutual.
#[test]
fn consistent_with_leaf_on_a_mixed_document() {
    let src = "---\nt: x\n---\n\n# H  \n\npara\nline  \n\n> q\n>\n> - i\n>   - j  \n\n```\nc\n```\n\n| a |\n|---|\n| b |\n\n***\n\n[r]: /u\n\nend";
    let d = parse(src);
    let t = d.text().as_bytes();
    for at in 0..=src.len() {
        if let Some(l) = d.leaf(at) {
            let o = d.block_owner(at).expect("owned");
            assert_eq!(o.leaf, markise_doc::BlockLeaf::from(&l), "at {at}");
            let line = l
                .lines
                .iter()
                .rev()
                .find(|x| x.start <= at)
                .or(l.lines.first())
                .copied();
            assert_eq!(
                Some(o.line),
                line.or(Some(markise_doc::Span::new(l.span.start, l.span.start))),
                "line at {at}"
            );
        }
        if let Some(o) = d.block_owner(at) {
            let (lo, hi) = (o.leaf.span.start.min(at), o.leaf.span.end.max(at));
            let between = &t[lo..hi];
            assert!(
                (o.leaf.span.start..=o.leaf.span.end.max(o.content.end)).contains(&at)
                    || !between.iter().any(|c| *c == b'\n' || *c == b'\r'),
                "at {at}: {:?}",
                o.part
            );
        }
    }
    let starts: Vec<usize> = (0..=src.len())
        .filter_map(|at| d.leaf(at).map(|l| l.span.start))
        .collect();
    for &s in &starts {
        if let Some(n) = d.adjacent_block(s, true).and_then(|a| a.leaf) {
            let back = d
                .adjacent_block(n.span.start, false)
                .and_then(|a| a.leaf)
                .map(|l| l.span.start);
            assert_eq!(back, Some(s), "neighbours of {s} / {}", n.span.start);
        }
    }
}
