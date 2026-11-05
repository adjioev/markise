//! Changing a Markdown image's path and description through the editing surface: only those bytes
//! change, in one undo step.

use markise_engine::editor::{ImageRefused, image_dest_for};

use super::block_inplace::surface;

fn at(text: &str) -> std::ops::Range<usize> {
    let start = text.find("![").unwrap();
    // to the end of the line: the parts stand inside it
    start..text[start..].find(['\r', '\n']).map_or(text.len(), |i| start + i)
}

#[test]
fn only_the_destination_changes_and_one_undo_brings_it_back() {
    for (doc, dest, want) in [
        ("a\n\n![alt](old.png)\n\nb\n", "new.png", "a\n\n![alt](new.png)\n\nb\n"),
        // title and attributes stay
        (
            "![alt *x*](old.png \"The title\"){width=3}\n",
            "n%20w.png",
            "![alt *x*](n%20w.png \"The title\"){width=3}\n",
        ),
        // what it was written as does not matter
        ("![](<old one.png>)\n", "new.png", "![](new.png)\n"),
        ("![a](old(1).png)\n", "new.png", "![a](new.png)\n"),
        ("![a [b] c](  old.png  )\n", "new.png", "![a [b] c](  new.png  )\n"),
        ("![a](https://example.com/x.png)\n", "local.png", "![a](local.png)\n"),
        // line endings
        (
            "a\r\n\r\n![alt](old.png)\r\n\r\nb\r\n",
            "new.png",
            "a\r\n\r\n![alt](new.png)\r\n\r\nb\r\n",
        ),
        // in a block
        (
            ":::x\n\n![alt](old.png)\n\n:::\n",
            "new.png",
            ":::x\n\n![alt](new.png)\n\n:::\n",
        ),
    ] {
        let mut s = surface(doc);
        let undos = s.ed.session.history().undo_len();
        assert_eq!(s.ed.replace_image_dest(at(doc), dest), Ok(true), "{doc:?}");
        assert_eq!(s.text(), want);
        assert_eq!(s.ed.session.history().undo_len(), undos + 1);
        s.undo();
        assert_eq!(s.text(), doc, "exact");
    }
}

#[test]
fn a_reference_image_has_no_path_of_its_own() {
    let doc = "![alt][r]\n\n[r]: old.png\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.replace_image_dest(0..9, "new.png"), Err(ImageRefused::NoImage));
    assert_eq!(s.text(), doc);
}

#[test]
fn the_description_and_the_path_change_together_in_one_step() {
    let doc = "![old alt](old.png \"t\")\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.edit_image(at(doc), "new alt", "new.png"), Ok(true));
    assert_eq!(s.text(), "![new alt](new.png \"t\")\n");
    s.undo();
    assert_eq!(s.text(), doc);
    assert_eq!(
        s.ed.edit_image(at(doc), "old alt", "old.png"),
        Ok(false),
        "nothing to do"
    );
    // what would not stay one image
    assert_eq!(
        s.ed.edit_image(at(doc), "a]b", "x.png"),
        Err(ImageRefused::WouldNotStand)
    );
    assert_eq!(
        s.ed.edit_image(at(doc), "a\nb", "x.png"),
        Err(ImageRefused::WouldNotStand)
    );
    assert_eq!(s.text(), doc);
}

#[test]
fn what_a_person_types_is_written_so_that_it_reads_as_one_path() {
    for (typed, want) in [
        ("a.png", Some("a.png")),
        ("  a.png ", Some("a.png")),
        ("a b.png", Some("<a b.png>")),
        ("a(1).png", Some("a(1).png")),
        ("a).png", Some("<a).png>")),
        ("<a b.png>", Some("<a b.png>")),
        ("a b<c.png", None),
        ("a\nb", None),
    ] {
        assert_eq!(image_dest_for(typed).as_deref(), want, "{typed:?}");
    }
}
