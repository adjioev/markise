//! The example package in `examples/block-package` and the small one printed in the guide, through the
//! code a document goes through: packed, bound, asked about, accepted and read, and on macOS drawn by the
//! real helper. So what the guide tells a package's maker is what happens.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use markise_engine::block_page::{BlockPage, document_language};
use markise_engine::block_view::PackageViews;
use markise_layout::native::{BlockLook, BlockViews};
use markise_md::{Document, ParseOptions};
use markise_package::{
    BINDING_FILE, InstalledPackage, PackageIndex, Resolution, Store, accept, pack_folder, read_archive, resolve,
};
use markise_web_render::JobSource;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example() -> PathBuf {
    repo().join("examples/block-package")
}

/// A folder as `cargo xtask block-package` leaves it, and the package accepted.
struct Site {
    _dir: tempfile::TempDir,
    folder: PathBuf,
    package: InstalledPackage,
    views: Arc<PackageViews>,
}

impl Site {
    fn of(package: &Path, document: &Path) -> Site {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let folder = base.join("site");
        copy_tree(document, &folder);
        let packed = pack_folder(package).unwrap();
        fs::create_dir_all(folder.join("blocks")).unwrap();
        fs::write(folder.join("blocks/package.tar.gz"), &packed.bytes).unwrap();
        fs::write(
            folder.join(BINDING_FILE),
            format!(
                r#"{{"library":"{}","version":"{}","source":"blocks/package.tar.gz","sha256":"{}"}}"#,
                packed.id.library, packed.id.version, packed.sha256
            ),
        )
        .unwrap();
        let store = Store::in_config(&base.join("config"));
        let doc = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "md"))
            .expect("a document");
        let Resolution::Ask(pending) = resolve(&doc, &store, Some(&base)) else {
            panic!("a question comes first");
        };
        let package = accept(&pending, &store).expect("accepted");
        let views = Arc::new(PackageViews::new(package.index.clone(), "en"));
        Site {
            _dir: dir,
            folder,
            package,
            views,
        }
    }

    fn text(&self) -> String {
        let doc = fs::read_dir(&self.folder)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "md"))
            .unwrap();
        fs::read_to_string(doc).unwrap()
    }

    /// Every block of the document: name, arguments, source.
    fn blocks(&self) -> Vec<(String, String, String)> {
        let text = self.text();
        let doc = Document::parse(&text, ParseOptions::default());
        let at = |s: markise_doc::Span| text[s.start..s.end].to_string();
        doc.directives()
            .into_iter()
            .map(|d| (at(d.name), at(d.args), text[d.opener.start..d.closer.end].to_string()))
            .collect()
    }

    fn page(&self, block: &(String, String, String)) -> BlockPage {
        BlockPage::new(
            self.views.clone(),
            self.package.clone(),
            Some(self.folder.clone()),
            &block.0,
            &block.1,
            &block.2,
        )
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn recipes() -> Site {
    Site::of(&example().join("package"), &example().join("document"))
}

/// The files the guide prints: a line of one `path`, then a fence.
fn printed_in_the_guide() -> BTreeMap<String, String> {
    let guide = fs::read_to_string(repo().join("docs/user/block-packages.md")).unwrap();
    let mut files = BTreeMap::new();
    let mut lines = guide.lines();
    while let Some(line) = lines.next() {
        let Some(path) = line.strip_prefix("File `").and_then(|l| l.strip_suffix("`:")) else {
            continue;
        };
        assert_eq!(lines.next(), Some(""), "{path}");
        assert!(lines.next().is_some_and(|l| l.starts_with("```")), "{path}");
        let body: Vec<&str> = lines.by_ref().take_while(|l| !l.starts_with("```")).collect();
        files.insert(path.to_string(), body.join("\n") + "\n");
    }
    files
}

/// The guide's small package and document, as folders.
fn smallest() -> (tempfile::TempDir, Site) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in printed_in_the_guide() {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    let site = Site::of(&dir.path().join("package"), &dir.path().join("document"));
    (dir, site)
}

#[test]
fn the_archive_holds_the_folder_and_the_same_files_give_the_same_archive() {
    let a = pack_folder(&example().join("package")).unwrap();
    let b = pack_folder(&example().join("package")).unwrap();
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(a.id.library, "recipe-site");
    let files = read_archive(&a.bytes).unwrap();
    assert!(files.contains_key("components/timing/timing.js"));
    assert_eq!(PackageIndex::from_files(&files).unwrap().block_count(), 3);
}

#[test]
fn every_block_of_the_sample_document_fits_and_shows_as_the_readme_says() {
    let site = recipes();
    let blocks = site.blocks();
    let names: Vec<&str> = blocks.iter().map(|b| b.0.as_str()).collect();
    assert_eq!(names, ["ingredients", "timing", "tip", "tip"]);
    for block in &blocks {
        let look = site.views.look(&block.0, &block.1, &block.2);
        match (block.0.as_str(), &look) {
            ("tip" | "ingredients", BlockLook::Native(_)) | ("timing", BlockLook::Generic) => {}
            _ => panic!("{}: {look:?}", block.0),
        }
    }
    assert_eq!(document_language(&site.text()), "en");
}

#[test]
fn every_preset_of_the_example_is_one_block_of_its_name_that_fits() {
    let site = recipes();
    let presets = site.package.index.presets();
    assert_eq!(presets.len(), 4);
    for preset in presets {
        let doc = Document::parse(&preset.markdown, ParseOptions::default());
        let found = doc.directives();
        assert_eq!(found.len(), 1, "{}", preset.id);
        let name = &preset.markdown[found[0].name.start..found[0].name.end];
        let args = &preset.markdown[found[0].args.start..found[0].args.end];
        assert_eq!(name, preset.block);
        let look = site.views.look(name, args, preset.markdown.trim_end());
        assert!(
            matches!(look, BlockLook::Native(_) | BlockLook::Generic),
            "{}: {look:?}",
            preset.id
        );
    }
}

#[test]
fn a_block_of_the_example_gets_the_markup_the_guide_prints() {
    let site = recipes();
    let blocks = site.blocks();
    let warning = site.page(&blocks[3]).prepare().unwrap();
    assert!(
        warning
            .html
            .starts_with(r#"<section data-component="tip" class="tip" data-variant="warning">"#)
    );
    assert_eq!(warning.styles, ["styles/styles.css", "components/tip/tip.css"]);
    assert_eq!(warning.module, None);
    let ingredients = site.page(&blocks[0]).prepare().unwrap();
    assert!(
        ingredients.html.contains(r#"data-argument="8""#),
        "{}",
        ingredients.html
    );
    assert!(
        ingredients.html.contains(r#"src="pictures/loaf.png""#),
        "{}",
        ingredients.html
    );
    let timing = site.page(&blocks[1]).prepare().unwrap();
    assert_eq!(timing.module.as_deref(), Some("components/timing/timing.js"));
}

#[test]
fn the_package_printed_in_the_guide_is_valid_and_its_block_fits() {
    let (_dir, site) = smallest();
    let blocks = site.blocks();
    assert_eq!(blocks.len(), 1);
    let look = site.views.look(&blocks[0].0, &blocks[0].1, &blocks[0].2);
    assert!(matches!(look, BlockLook::Native(_)), "{look:?}");
    assert_eq!(site.package.index.presets().len(), 1);
}

/// The command the guide gives makers, with the system's `tar`.
#[cfg(unix)]
#[test]
fn an_archive_made_as_the_guide_says_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("package.tar.gz");
    let done = std::process::Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(example().join("package"))
        .arg(".")
        .status()
        .expect("tar");
    assert!(done.success());
    let files = read_archive(&fs::read(&archive).unwrap()).unwrap();
    assert_eq!(PackageIndex::from_files(&files).unwrap().block_count(), 3);
}

#[cfg(target_os = "macos")]
mod drawn {
    use std::time::{Duration, Instant};

    use markise_web_render::{Client, Job, Limits, Outcome, ProcessSpawner, Shot};

    use super::*;

    /// One picture per job, by key.
    fn draw(jobs: Vec<Job>) -> BTreeMap<u64, Shot> {
        let spawner = ProcessSpawner {
            program: env!("CARGO_BIN_EXE_markise-web-helper").into(),
        };
        let client = Client::new(Box::new(spawner), Limits::default(), Arc::new(|| {}));
        let wanted = jobs.len();
        client.want(jobs);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut shots = BTreeMap::new();
        while shots.len() < wanted && Instant::now() < deadline {
            for (key, outcome) in client.take_results() {
                match outcome {
                    Outcome::Drawn(shot) => shots.insert(key, shot),
                    Outcome::Failed(why) => panic!("job {key}: {why}"),
                };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(shots.len(), wanted);
        shots
    }

    fn job(site: &Site, key: u64, block: &(String, String, String), dark: bool) -> Job {
        Job {
            key,
            session: "example".to_string(),
            width: 480,
            scale: 2.0,
            dark,
            lang: "en".to_string(),
            vars: vec![("--markise-font-size".to_string(), "16px".to_string())],
            source: Arc::new(site.page(block)),
        }
    }

    /// Whether the picture holds this colour somewhere, opaque.
    fn holds(shot: &Shot, rgb: [u8; 3]) -> bool {
        let (pixels, _) = shot.rgba.as_chunks::<4>();
        pixels.iter().any(|p| p[..3] == rgb && p[3] == 255)
    }

    #[test]
    fn every_block_of_the_sample_document_draws_with_its_styles_script_and_picture() {
        let site = recipes();
        let blocks = site.blocks();
        let jobs = blocks.iter().enumerate().map(|(i, b)| job(&site, i as u64, b, false));
        let shots = draw(jobs.collect());
        for shot in shots.values() {
            assert_eq!(shot.px_width, 960, "the column at this scale");
            assert!(shot.css_height > 20.0, "{}", shot.css_height);
        }
        assert!(holds(&shots[&0], [46, 125, 50]), "ingredients.css");
        assert!(holds(&shots[&0], [181, 101, 29]), "the document's picture");
        assert!(holds(&shots[&1], [192, 86, 33]), "the bar timing.js makes, one step");
        assert!(holds(&shots[&1], [120, 53, 15]), "the bar timing.js makes, the next");
        assert!(holds(&shots[&2], [217, 119, 6]), "tip.css");
        assert!(holds(&shots[&3], [185, 28, 28]), "tip.css, the variant");
    }

    #[test]
    fn a_block_follows_the_theme_it_is_told() {
        let site = recipes();
        let tip = &site.blocks()[2];
        let shots = draw(vec![job(&site, 1, tip, false), job(&site, 2, tip, true)]);
        assert!(holds(&shots[&1], [255, 243, 205]) && !holds(&shots[&1], [66, 45, 8]));
        assert!(holds(&shots[&2], [66, 45, 8]), "the dark look");
    }

    #[test]
    fn the_package_printed_in_the_guide_draws_as_the_guide_says() {
        let (_dir, site) = smallest();
        let block = &site.blocks()[0];
        let shots = draw(vec![job(&site, 1, block, false)]);
        assert!(holds(&shots[&1], [37, 99, 235]), "note.css");
        assert!(holds(&shots[&1], [220, 38, 38]), "the stripe note.js adds");
    }
}
