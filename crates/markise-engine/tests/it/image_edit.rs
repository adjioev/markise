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
fn the_parser_says_where_the_description_ends() {
    // a code span may hold what looks like the end of an image
    let doc = "![`x](fake.png)`](old.png)\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.replace_image_dest(at(doc), "new.png"), Ok(true));
    assert_eq!(
        s.text(),
        "![`x](fake.png)`](new.png)\n",
        "not the one in the description"
    );
    // and such a one with no path of its own has none
    let doc = "![`x](fake.png)`][r]\n\n[r]: old.png\n";
    let mut s = surface(doc);
    let line = 0..doc.find('\n').unwrap();
    assert_eq!(s.ed.replace_image_dest(line, "new.png"), Err(ImageRefused::NoImage));
    assert_eq!(s.text(), doc);
    // what is no image at all, where a scan would see one
    let doc = "`![a](old.png)`\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.replace_image_dest(1..14, "new.png"), Err(ImageRefused::NoImage));
    // the forms a description has
    for doc in [
        "![](old.png)\n",
        "![a ](old.png)\n",
        "![a\\]b](old.png)\n",
        "![a *b*](old.png)\n",
        "![[b]](old.png)\n",
    ] {
        let mut s = surface(doc);
        assert_eq!(s.ed.replace_image_dest(at(doc), "new.png"), Ok(true), "{doc:?}");
        assert_eq!(s.text(), doc.replace("old.png", "new.png"), "{doc:?}");
    }
}

#[test]
fn in_a_quote_and_a_list_the_right_bytes_change() {
    for doc in [
        "> a\n> ![x](old.png)\n",
        "- ![x](old.png)\n",
        "text ![x](old.png) more\n",
    ] {
        let mut s = surface(doc);
        let start = doc.find("![").unwrap();
        assert_eq!(
            s.ed.replace_image_dest(start..start + 14, "new.png"),
            Ok(true),
            "{doc:?}"
        );
        assert_eq!(s.text(), doc.replace("old.png", "new.png"));
    }
}

#[test]
fn an_image_in_a_table_cell_changes_too() {
    let doc = "| a | b |\n|---|---|\n| one | ![x](old.png) |\n";
    let mut s = surface(doc);
    let start = doc.find("![").unwrap();
    assert_eq!(s.ed.replace_image_dest(start..start + 14, "new.png"), Ok(true));
    assert_eq!(s.text(), doc.replace("old.png", "new.png"));
    s.undo();
    assert_eq!(s.text(), doc);
    assert_eq!(s.ed.edit_image(start..start + 14, "y", "new.png"), Ok(true));
    assert_eq!(s.text(), "| a | b |\n|---|---|\n| one | ![y](new.png) |\n");
    s.undo();
    // a pipe written as a cell has it, before the description ends
    let doc = "| a |\n|---|\n| ![x\\|y](old.png) |\n";
    let mut s = surface(doc);
    let start = doc.find("![").unwrap();
    assert_eq!(s.ed.replace_image_dest(start..start + 16, "new.png"), Ok(true));
    assert_eq!(s.text(), doc.replace("old.png", "new.png"));
    // a path that would end the cell does not stand
    let doc = "| a |\n|---|\n| ![x](old.png) |\n";
    let mut s = surface(doc);
    let start = doc.find("![").unwrap();
    assert_eq!(
        s.ed.replace_image_dest(start..start + 13, "a|b.png"),
        Err(ImageRefused::WouldNotStand)
    );
    assert_eq!(s.text(), doc);
}

#[test]
fn changing_nothing_leaves_the_view_with_its_document() {
    let doc = "a\n\n![x](old.png)\n";
    let mut s = surface(doc);
    assert_eq!(s.ed.edit_image(at(doc), "x", "old.png"), Ok(false));
    assert_eq!(s.ed.view.doc().text(), doc, "as with any other edit");
    assert_eq!(s.ed.replace_image_dest(at(doc), "old.png"), Ok(false));
    assert_eq!(s.ed.view.doc().text(), doc);
    assert_eq!(s.ed.replace_image_dest(0..1, "new.png"), Err(ImageRefused::NoImage));
    assert_eq!(s.ed.view.doc().text(), doc);
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
