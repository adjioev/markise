//! A block as its site's code gets it (ADR 0023): markup by field type, files by convention, and
//! nothing of the disk but those.

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use markise_engine::block_page::{BlockPage, document_language, unreachable_image};
use markise_engine::block_view::PackageViews;
use markise_package::{InstalledPackage, PackageIndex};
use markise_web_render::JobSource;
use markise_web_render::proto::Area;
use serde_json::json;

struct Rig {
    _dir: tempfile::TempDir,
    package: InstalledPackage,
    views: Arc<PackageViews>,
    folder: std::path::PathBuf,
    outside: std::path::PathBuf,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let root = base.join("package");
    let folder = base.join("site");
    let outside = base.join("outside");
    for d in [&root, &folder, &outside] {
        fs::create_dir_all(d).unwrap();
    }
    let poll = json!({"label": "Poll", "variants": {"": "one", "many": "several"}, "fields": [
        {"name": "question", "type": "heading", "level": 3},
        {"name": "setup", "type": "text", "min": 0},
        {"name": "answers", "type": "choices", "ids": "required", "minItems": 2,
         "correct": {"": "exactly-one", "many": "at-least-one"}},
        {"name": "why", "type": "quote", "min": 0},
    ]});
    let shot = json!({"label": "Shot", "argument": {"name": "place"}, "fields": [
        {"name": "title", "type": "heading", "level": 2, "min": 0},
        {"name": "items", "type": "list", "minItems": 0},
        {"name": "pictures", "type": "image", "min": 0, "max": null},
    ]});
    let mut files = BTreeMap::new();
    let manifest = r#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["poll","shot"]}"#;
    files.insert("manifest.json".to_string(), manifest.as_bytes().to_vec());
    files.insert("components/poll/poll.json".to_string(), poll.to_string().into_bytes());
    files.insert("components/shot/shot.json".to_string(), shot.to_string().into_bytes());
    files.insert("editor/palette.json".to_string(), br#"{"format": 1}"#.to_vec());
    let index = Arc::new(PackageIndex::from_files(&files).unwrap());
    let hash = markise_package::Sha256Hex::parse(&"0".repeat(64)).unwrap();
    let package = InstalledPackage {
        binding: markise_package::Binding {
            file: folder.join("blocks.config.json"),
            library: "sample".to_string(),
            version: "1".to_string(),
            source: "sample.tar.gz".to_string(),
            sha256: hash.clone(),
        },
        sha256: hash,
        root,
        index: index.clone(),
    };
    Rig {
        _dir: dir,
        views: Arc::new(PackageViews::new(index, "en")),
        package,
        folder,
        outside,
    }
}

impl Rig {
    fn page(&self, name: &str, args: &str, source: &str) -> BlockPage {
        BlockPage::new(
            self.views.clone(),
            self.package.clone(),
            Some(self.folder.clone()),
            name,
            args,
            source,
        )
    }

    fn file(&self, relative: &str, content: &str) {
        let path = self.package.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

const POLL: &str = ":::poll many\n\n### Which **one**?\n\nFirst some text.\n\n- [x] One {#a1}\n- [ ] Two <b onclick=\"x\">y</b> {#a2}\n\n> Because.\n\n:::";

#[test]
fn the_markup_comes_from_the_field_types_alone() {
    let r = rig();
    let p = r.page("poll", "many", POLL).prepare().unwrap();
    let h = &p.html;
    assert!(
        h.starts_with(r#"<section data-component="poll" class="poll" data-variant="many">"#),
        "{h}"
    );
    assert!(!h.contains("data-argument"));
    assert!(
        h.contains(r#"<h3 data-field="question">Which <strong>one</strong>?</h3>"#),
        "{h}"
    );
    assert!(
        h.contains(r#"<div data-field="setup">"#) && h.contains("First some text."),
        "{h}"
    );
    assert!(h.contains(r#"<ul data-field="answers" data-select="multiple">"#), "{h}");
    assert!(
        h.contains(r#"<li data-item data-option-id="a1" data-correct="true">One</li>"#),
        "{h}"
    );
    assert!(h.contains(r#"data-option-id="a2" data-correct="false""#), "{h}");
    assert!(
        h.contains(r#"<blockquote data-field="why">"#) && h.contains("Because."),
        "{h}"
    );
    assert!(!h.contains("{#"), "ids are no text");
    assert!(!h.contains("onclick"), "the sanitizer of the export: {h}");
    assert!(h.ends_with("</section>"));
}

#[test]
fn a_single_answer_says_so_and_an_argument_is_not_a_variant() {
    let r = rig();
    let one = POLL.replace(":::poll many", ":::poll");
    let h = r.page("poll", "", &one).prepare().unwrap().html;
    assert!(h.contains(r#"data-select="single""#), "{h}");
    assert!(!h.contains("data-variant"), "{h}");
    let shot = ":::shot road/one\n\n## T\n\n- A\n- B\n\n![Alt \"q\"](images/a.png)\n\n:::";
    let h = r.page("shot", "road/one", shot).prepare().unwrap().html;
    assert!(
        h.contains(r#"data-argument="road/one""#) && !h.contains("data-variant"),
        "{h}"
    );
    assert!(
        h.contains(r#"<ul data-field="items"><li data-item>A</li><li data-item>B</li></ul>"#),
        "{h}"
    );
    assert!(
        h.contains(r#"<div data-field="pictures"><img src="images/a.png" alt="Alt &quot;q&quot;"></div>"#),
        "{h}"
    );
}

#[test]
fn a_field_with_nothing_in_it_has_no_element() {
    let r = rig();
    let h = r.page("shot", "x", ":::shot x\n\n- A\n\n:::").prepare().unwrap().html;
    assert!(
        !h.contains(r#"data-field="title""#) && !h.contains(r#"data-field="pictures""#),
        "{h}"
    );
}

#[test]
fn document_text_cannot_break_out_of_an_attribute() {
    let r = rig();
    let shot = ":::shot a\"><script>x</script>\n\n- A\n\n:::";
    let h = r.page("shot", "a\"><script>x</script>", shot).prepare().unwrap().html;
    assert!(!h.contains("<script>"), "{h}");
}

#[test]
fn styles_and_script_go_by_their_conventional_names_when_they_are_there() {
    let r = rig();
    let p = r.page("poll", "many", POLL).prepare().unwrap();
    assert!(p.styles.is_empty() && p.module.is_none());
    r.file("styles/styles.css", "a{}");
    r.file("components/poll/poll.css", "b{}");
    r.file("components/poll/poll.js", "export default function(){}");
    let p = r.page("poll", "many", POLL).prepare().unwrap();
    assert_eq!(p.styles, ["styles/styles.css", "components/poll/poll.css"]);
    assert_eq!(p.module.as_deref(), Some("components/poll/poll.js"));
}

#[test]
fn a_package_gives_its_own_files_and_nothing_above_them() {
    let r = rig();
    r.file("assets/a.svg", "<svg/>");
    fs::write(r.package.root.parent().unwrap().join("secret.txt"), "no").unwrap();
    let p = r.page("poll", "many", POLL);
    assert_eq!(p.file(Area::Package, "assets/a.svg").as_deref(), Some(&b"<svg/>"[..]));
    for bad in [
        "../secret.txt",
        "assets/../../secret.txt",
        "/etc/hosts",
        "",
        "assets",
        "missing",
    ] {
        assert_eq!(p.file(Area::Package, bad), None, "{bad}");
    }
}

#[test]
fn a_document_gives_this_blocks_images_and_nothing_else() {
    let r = rig();
    let block = ":::shot x\n\n![a](images/a%20b.png)\n\n![b](./images/c.png)\n\n![d](notes.txt)\n\n:::";
    fs::create_dir_all(r.folder.join("images")).unwrap();
    for f in [
        "images/a b.png",
        "images/c.png",
        "images/other.png",
        "notes.txt",
        "doc.md",
    ] {
        fs::write(r.folder.join(f), f).unwrap();
    }
    fs::write(r.outside.join("d.png"), "out").unwrap();
    let p = r.page("shot", "x", block);
    assert_eq!(
        p.file(Area::Document, "images/c.png"),
        None,
        "nothing until it is prepared"
    );
    p.prepare().unwrap();
    assert_eq!(
        p.file(Area::Document, "images/a b.png").as_deref(),
        Some(&b"images/a b.png"[..])
    );
    assert_eq!(
        p.file(Area::Document, "images/c.png").as_deref(),
        Some(&b"images/c.png"[..])
    );
    for bad in [
        "images/other.png",
        "doc.md",
        "notes.txt",
        "../outside/d.png",
        "/etc/hosts",
    ] {
        assert_eq!(p.file(Area::Document, bad), None, "{bad}");
    }
    // named by the block, and still not given
    let p = r.page("shot", "x", ":::shot x\n\n![d](../outside/d.png)\n\n:::");
    assert!(p.prepare().is_err());
    assert_eq!(p.file(Area::Document, "../outside/d.png"), None);
    assert_eq!(p.file(Area::Document, "outside/d.png"), None);
}

#[test]
fn an_image_with_a_query_or_a_fragment_is_the_same_file() {
    // a page's URL arrives without them, so the list has to hold the same
    let r = rig();
    let block = ":::shot x\n\n![a](images/a.svg#icon)\n\n![b](images/b%20c.png?v=1)\n\n:::";
    fs::create_dir_all(r.folder.join("images")).unwrap();
    fs::write(r.folder.join("images/a.svg"), "a").unwrap();
    fs::write(r.folder.join("images/b c.png"), "b").unwrap();
    let p = r.page("shot", "x", block);
    p.prepare().unwrap();
    assert_eq!(p.file(Area::Document, "images/a.svg").as_deref(), Some(&b"a"[..]));
    assert_eq!(p.file(Area::Document, "images/b c.png").as_deref(), Some(&b"b"[..]));
}

/// No such file name on Windows.
#[cfg(unix)]
#[test]
fn a_percent_encoded_question_mark_is_part_of_the_name() {
    let r = rig();
    fs::create_dir_all(r.folder.join("images")).unwrap();
    fs::write(r.folder.join("images/q?.png"), "q").unwrap();
    let p = r.page("shot", "x", ":::shot x\n\n![a](images/q%3F.png)\n\n:::");
    p.prepare().unwrap();
    assert_eq!(p.file(Area::Document, "images/q?.png").as_deref(), Some(&b"q"[..]));
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_folder_opens_nothing() {
    let r = rig();
    let block = ":::shot x\n\n![a](images/link.png)\n\n![b](dir/in.png)\n\n:::";
    fs::write(r.outside.join("secret.png"), "secret").unwrap();
    fs::create_dir_all(r.folder.join("images")).unwrap();
    std::os::unix::fs::symlink(r.outside.join("secret.png"), r.folder.join("images/link.png")).unwrap();
    std::os::unix::fs::symlink(&r.outside, r.folder.join("dir")).unwrap();
    fs::write(r.outside.join("in.png"), "secret").unwrap();
    let p = r.page("shot", "x", block);
    p.prepare().unwrap();
    assert_eq!(p.file(Area::Document, "images/link.png"), None);
    assert_eq!(p.file(Area::Document, "dir/in.png"), None);
}

#[test]
fn a_note_names_the_file_and_the_reason_in_plain_words() {
    let r = rig();
    fs::create_dir_all(r.folder.join("images")).unwrap();
    fs::write(r.folder.join("images/here.png"), "here").unwrap();
    let p = r.page(
        "shot",
        "x",
        ":::shot x\n\n![a](images/x.png)\n\n![b](images/here.png)\n\n:::",
    );
    p.prepare().unwrap();
    assert_eq!(p.images_note().as_deref(), Some("File not found: images/x.png"));

    let p = r.page("shot", "x", ":::shot x\n\n![b](images/here.png)\n\n:::");
    p.prepare().unwrap();
    assert_eq!(p.images_note(), None, "nothing to say about what is there");
}

#[test]
fn several_are_named_up_to_a_length_to_read() {
    let r = rig();
    let images: String = (0..5).map(|i| format!("![a](i{i}.png)\n\n")).collect();
    let p = r.page("shot", "x", &format!(":::shot x\n\n{images}:::"));
    p.prepare().unwrap();
    assert_eq!(
        p.images_note().as_deref(),
        Some("File not found: i0.png; File not found: i1.png; File not found: i2.png; and 2 more")
    );
}

#[test]
fn a_picture_refuses_a_path_outside_the_folder_before_any_page_is_asked() {
    let r = rig();
    fs::write(r.folder.join("x.png"), "the wrong file").unwrap();
    for (src, note) in [
        ("../x.png", "Outside the document's folder: ../x.png"),
        ("a/../../x.png", "Outside the document's folder: a/../../x.png"),
        ("/etc/x.png", "Outside the document's folder: /etc/x.png"),
        (
            "https://example.com/x.png",
            "Not a file in the document's folder: https://example.com/x.png",
        ),
    ] {
        let block = format!(":::shot x\n\n![a]({src})\n\n:::");
        assert_eq!(
            r.page("shot", "x", &block).prepare().err().as_deref(),
            Some(note),
            "{src}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_folder_is_outside_it_too() {
    let r = rig();
    fs::write(r.outside.join("secret.png"), "secret").unwrap();
    std::os::unix::fs::symlink(r.outside.join("secret.png"), r.folder.join("link.png")).unwrap();
    let p = r.page("shot", "x", ":::shot x\n\n![a](link.png)\n\n:::");
    p.prepare().unwrap();
    assert_eq!(
        p.images_note().as_deref(),
        Some("Outside the document's folder: link.png")
    );
}

#[test]
fn what_a_picture_can_never_have_is_known_without_the_disk() {
    for fine in ["a.png", "./a/b.png", "data:image/png;base64,AA", "BLOB:x", "a%20b.png"] {
        assert_eq!(unreachable_image(fine), None, "{fine}");
    }
    assert!(unreachable_image("..%2Fx.png").is_some(), "encoded");
    assert!(unreachable_image("a/../b.png").is_some_and(|n| n.contains("plain path")));
}

#[test]
fn a_block_that_cannot_be_read_says_so() {
    let r = rig();
    assert!(r.page("nobody", "", ":::nobody\n\n:::").prepare().is_err());
}

#[test]
fn the_language_comes_from_the_top_of_the_text() {
    assert_eq!(document_language("---\ntitle: T\nlang: de\n---\n# H\n"), "de");
    assert_eq!(document_language("---\nlang: \"pt-BR\"\n---\n"), "pt-BR");
    assert_eq!(document_language("# H\nlang: de\n"), "");
    assert_eq!(document_language("---\ntitle: T\n---\nlang: de\n"), "");
    assert_eq!(document_language("---\nlang: a\"><b\n---\n"), "");
    assert_eq!(document_language(""), "");
}
