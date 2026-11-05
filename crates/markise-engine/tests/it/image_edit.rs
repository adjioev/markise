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

mod listing {
    use std::sync::atomic::AtomicBool;

    use markise_engine::image_list::{destinations_in, list_images, list_images_within};

    fn touch(root: &std::path::Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    fn rels(root: &std::path::Path) -> Vec<String> {
        let no = AtomicBool::new(false);
        list_images(root, &no).images.into_iter().map(|i| i.rel).collect()
    }

    #[test]
    fn the_nearest_first_and_only_images_and_nothing_hidden() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "b.png",
            "a.JPG",
            "n.md",
            "x/z.gif",
            "x/y/w.webp",
            ".git/h.png",
            ".hidden.png",
        ] {
            touch(d.path(), f);
        }
        assert_eq!(rels(d.path()), ["a.JPG", "b.png", "x/z.gif", "x/y/w.webp"]);
    }

    #[test]
    fn it_stops_at_its_bounds_and_says_so_and_when_told() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..20 {
            touch(d.path(), &format!("{i:02}.png"));
        }
        let no = AtomicBool::new(false);
        let l = list_images_within(d.path(), &no, 5, 1000);
        assert_eq!(l.images.len(), 5);
        assert!(l.truncated);
        let l = list_images_within(d.path(), &no, 1000, 3);
        assert!(l.truncated && l.images.len() == 3);
        let yes = AtomicBool::new(true);
        assert!(list_images(d.path(), &yes).images.is_empty());
    }

    #[test]
    fn too_deep_is_said_too() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "1/2/3/4/5/6/7/8/far.png");
        let no = AtomicBool::new(false);
        let l = list_images(d.path(), &no);
        assert!(l.images.is_empty() && l.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_does_not_lead_outside_and_a_folder_link_is_not_followed() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        touch(outside.path(), "secret.png");
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "real.png");
        std::fs::create_dir(d.path().join("loop")).unwrap();
        symlink(outside.path().join("secret.png"), d.path().join("out.png")).unwrap();
        symlink(d.path().join("real.png"), d.path().join("in.png")).unwrap();
        symlink(outside.path(), d.path().join("outdir")).unwrap();
        symlink(d.path(), d.path().join("loop/up")).unwrap();
        assert_eq!(rels(d.path()), ["in.png", "real.png"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_read_is_counted_and_the_rest_listed() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "ok.png");
        touch(d.path(), "shut/no.png");
        let shut = d.path().join("shut");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let no = AtomicBool::new(false);
        let l = list_images(d.path(), &no);
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(l.images.len(), 1);
        assert_eq!(l.unreadable, 1);
    }

    #[test]
    fn destinations_are_read_in_their_forms() {
        let d = destinations_in("![a](x.png) ![b](<y z.png> \"t\") [c]( w.md )");
        assert!(d.contains("x.png") && d.contains("y z.png") && d.contains("w.md"));
    }
}
