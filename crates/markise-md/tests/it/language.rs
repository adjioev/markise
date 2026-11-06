//! The document's language (ADR 0019): read from the front matter once, and read again only when
//! an edit reaches the first block.

use markise_md::parse;

const DOC: &str = "---\ntitle: T\nlang: ru\n---\n\nFirst paragraph.\n\nSecond paragraph.\n";

#[test]
fn an_edit_in_the_body_does_not_read_the_front_matter_again() {
    let mut d = parse(DOC);
    assert!(!d.language_is_kept(), "nothing read yet");
    assert_eq!(d.language(), Some("ru"));
    assert!(d.language_is_kept());
    // through the fast path, and through a re-parse
    let at = d.text().find("Second").unwrap();
    d.edit(at..at, "x");
    assert!(d.language_is_kept(), "a letter in a paragraph");
    d.edit(at..at, "\n\n# New\n\n");
    assert!(d.language_is_kept(), "new blocks in the body");
    assert_eq!(d.language(), Some("ru"));
}

#[test]
fn an_edit_of_the_first_block_is_read_again() {
    let mut d = parse(DOC);
    assert_eq!(d.language(), Some("ru"));
    let at = d.text().find("ru").unwrap();
    d.edit(at..at + 2, "de-AT");
    assert!(!d.language_is_kept());
    assert_eq!(d.language(), Some("de-AT"));
    let (start, end) = (d.text().find("lang").unwrap(), d.text().find("\n---").unwrap());
    d.edit(start..end, "");
    assert_eq!(d.language(), None, "the key went");
}

#[test]
fn front_matter_that_comes_or_goes_by_an_edit_elsewhere_is_seen() {
    // not closed: no front matter, until an edit further down closes it
    let mut d = parse("---\nlang: ru\n\nBody.\n");
    assert_eq!(d.language(), None);
    let end = d.text().len();
    d.edit(end..end, "---\n");
    assert_eq!(d.language(), Some("ru"));
    // and the closer going away from below
    let closer = d.text().rfind("---").unwrap();
    d.edit(closer..closer + 3, "");
    assert_eq!(d.language(), None);
    // the opener written on top of a document
    let mut d = parse("lang: ru\n---\n\nBody.\n");
    assert_eq!(d.language(), None);
    d.edit(0..0, "---\n");
    assert_eq!(d.language(), Some("ru"));
    // a whole replacement, as a reload or the byte-order mark does
    d.edit(0..0, "\u{feff}");
    assert_eq!(d.language(), Some("ru"));
}
