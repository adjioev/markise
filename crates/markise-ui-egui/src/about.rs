//! About Markise window (macOS: application menu; Windows/Linux: Help menu): logo, name,
//! tagline, version, copyright, licence with the licence texts and third-party notices, and
//! links to the website and the repository (opened in the default browser).

use egui::{Context, RichText, TextureHandle, TextureOptions};

pub const NAME: &str = "Markise";
pub const TAGLINE: &str = "The marquise of Markdown editors";
pub const DESCRIPTION: &str = "A fast live-preview Markdown editor in pure Rust.";
/// Owner decision (2026-10-28): no personal name.
pub const COPYRIGHT: &str = "© The Markise contributors";
pub const LICENSE: &str = "MIT OR Apache-2.0";
pub const WEBSITE: &str = "https://adjioev.github.io/markise-site/";
pub const REPOSITORY: &str = "https://github.com/adjioev/markise";

/// A licence document: menu title, repository path, text.
pub struct Document {
    pub title: &'static str,
    pub path: &'static str,
    pub text: &'static str,
}

macro_rules! doc {
    ($title:expr, $path:literal) => {
        Document {
            title: $title,
            path: $path,
            text: include_str!(concat!("../../../", $path)),
        }
    };
}

/// Every licence text the repository ships (NOTICE, the project licences, THIRD_PARTY and the
/// licence files of vendored assets referenced from NOTICE); the test
/// `licence_viewer_embeds_every_shipped_licence_file` fails when one is missing.
pub const DOCUMENTS: [Document; 15] = [
    doc!("Third-party notices (NOTICE)", "NOTICE"),
    doc!("Markise: MIT licence", "LICENSE-MIT"),
    doc!("Markise: Apache licence 2.0", "LICENSE-APACHE"),
    doc!("Syntax grammars (THIRD_PARTY)", "THIRD_PARTY/syntax-licenses.md"),
    doc!("KaTeX fonts: SIL Open Font License 1.1", "assets/fonts/katex/OFL.txt"),
    doc!("Lucide icons: ISC licence", "assets/icons/lucide/LICENSE"),
    doc!("Drake theme: MIT licence", "assets/themes/drake/LICENSE"),
    doc!("Lapis theme: MIT licence", "assets/themes/lapis/LICENSE"),
    doc!("Phycat theme: MIT licence", "assets/themes/phycat/LICENSE"),
    doc!("Fonts bundled inside egui (overview)", "THIRD_PARTY/fonts/README.md"),
    doc!(
        "Ubuntu-Light: Ubuntu Font Licence 1.0",
        "THIRD_PARTY/fonts/ubuntu-font-licence-1.0.txt"
    ),
    doc!(
        "Hack: MIT and Bitstream Vera licences",
        "THIRD_PARTY/fonts/hack-MIT-Bitstream.txt"
    ),
    doc!(
        "Noto Emoji: SIL Open Font License 1.1",
        "THIRD_PARTY/fonts/noto-emoji-OFL.txt"
    ),
    doc!(
        "emoji-icon-font / egui-icons: MIT licence",
        "THIRD_PARTY/fonts/emoji-icon-font-MIT.txt"
    ),
    doc!("egui-icons: subset description", "THIRD_PARTY/fonts/egui-icons.txt"),
];

/// App icon on dark and light backgrounds (assets/brand/png, see assets/brand/README.md).
const ICON_DARK: &[u8] = include_bytes!("../../../assets/brand/png/markise-256.png");
const ICON_LIGHT: &[u8] = include_bytes!("../../../assets/brand/png/markise-light-256.png");

/// Version line: the workspace version plus the short commit the binary was built from, if
/// the build knew it.
pub fn version_text(commit: Option<&str>) -> String {
    let v = env!("CARGO_PKG_VERSION");
    match commit.filter(|c| !c.is_empty()) {
        Some(c) => format!("Version {v} ({c})"),
        None => format!("Version {v}"),
    }
}

#[derive(Default)]
pub struct About {
    open: bool,
    /// The licence viewer is open, showing [`DOCUMENTS`]`[doc]`.
    licences: bool,
    doc: usize,
    /// Decoded icon for (dark?) — decoded on first show.
    logo: Option<(bool, TextureHandle)>,
}

impl About {
    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn licences_open(&self) -> bool {
        self.licences
    }

    fn logo(&mut self, ctx: &Context, dark: bool) -> Option<TextureHandle> {
        if self.logo.as_ref().is_none_or(|(d, _)| *d != dark) {
            let bytes = if dark { ICON_DARK } else { ICON_LIGHT };
            let pix = vello_cpu::Pixmap::from_png(std::io::Cursor::new(bytes)).ok()?;
            let size = [pix.width() as usize, pix.height() as usize];
            let img = egui::ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice());
            let tex = ctx.load_texture("markise-about-logo", img, TextureOptions::LINEAR);
            self.logo = Some((dark, tex));
        }
        self.logo.as_ref().map(|(_, t)| t.clone())
    }

    /// Shows the window (and the licence viewer) while open.
    pub fn show(&mut self, ctx: &Context, commit: Option<&str>) {
        if !self.open {
            self.logo = None; // free the texture
            self.licences = false;
            return;
        }
        let dark = ctx.global_style().visuals.dark_mode;
        let logo = self.logo(ctx, dark);
        let mut open = true;
        let mut show_licences = false;
        egui::Window::new("About Markise")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    if let Some(t) = &logo {
                        ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(96.0, 96.0)));
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(NAME).size(24.0).strong());
                    ui.label(RichText::new(TAGLINE).italics());
                    ui.add_space(4.0);
                    ui.label(DESCRIPTION);
                    ui.add_space(6.0);
                    ui.weak(version_text(commit));
                    ui.add_space(6.0);
                    ui.label(COPYRIGHT);
                    ui.label(format!("Licensed under {LICENSE}"));
                    if ui.button("Licences & Third-Party Notices…").clicked() {
                        show_licences = true;
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        // centre the two links
                        let w = 150.0;
                        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                        ui.hyperlink_to("Website", WEBSITE);
                        ui.label("·");
                        ui.hyperlink_to("GitHub", REPOSITORY);
                    });
                    ui.add_space(8.0);
                });
            });
        if show_licences {
            self.licences = true;
        }
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.licences {
            self.open = false;
        }
        if self.licences {
            let mut lic_open = true;
            egui::Window::new("Licences & Third-Party Notices")
                .collapsible(false)
                .open(&mut lic_open)
                .default_size([620.0, 480.0])
                .show(ctx, |ui| {
                    egui::ComboBox::new("markise-about-doc", "Document")
                        .selected_text(DOCUMENTS[self.doc].title)
                        .show_ui(ui, |ui| {
                            for (i, d) in DOCUMENTS.iter().enumerate() {
                                ui.selectable_value(&mut self.doc, i, d.title);
                            }
                        });
                    ui.separator();
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        ui.label(RichText::new(DOCUMENTS[self.doc].text).monospace().size(11.0));
                    });
                });
            if !lic_open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.licences = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_has_the_commit_when_known() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(version_text(None), format!("Version {v}"));
        assert_eq!(version_text(Some("")), format!("Version {v}"));
        assert_eq!(version_text(Some("abc1234")), format!("Version {v} (abc1234)"));
    }

    #[test]
    fn logos_decode_and_documents_are_embedded() {
        for b in [ICON_DARK, ICON_LIGHT] {
            let p = vello_cpu::Pixmap::from_png(std::io::Cursor::new(b)).unwrap();
            assert_eq!((p.width(), p.height()), (256, 256));
        }
        assert!(DOCUMENTS.iter().all(|d| !d.text.trim().is_empty()));
        assert!(DOCUMENTS[1].text.contains("MIT"));
    }

    fn is_licence_name(name: &str) -> bool {
        let n = name.to_ascii_uppercase();
        ["LICENSE", "LICENCE", "COPYING", "OFL", "UNLICENSE"]
            .iter()
            .any(|p| n.starts_with(p))
    }

    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !name.starts_with('.') && !["target", "spikes", "node_modules"].contains(&name.as_str()) {
                    walk(&p, root, out);
                }
            } else if is_licence_name(&name) {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }

    /// Codex review of PR #34: the viewer must hold every licence text the repository ships.
    /// Expected = licence-named files under the shipped folders (root, assets/, THIRD_PARTY/)
    /// plus every repository path NOTICE and THIRD_PARTY name that ends in a licence file
    /// name, so a new vendored asset with a LICENSE (or a new NOTICE reference) fails here.
    #[test]
    fn licence_viewer_embeds_every_shipped_licence_file() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root = root.canonicalize().unwrap();
        let mut expected = vec!["NOTICE".to_string(), "THIRD_PARTY/syntax-licenses.md".to_string()];
        for name in std::fs::read_dir(&root).unwrap().flatten() {
            let n = name.file_name().to_string_lossy().into_owned();
            if name.path().is_file() && is_licence_name(&n) {
                expected.push(n);
            }
        }
        walk(&root.join("assets"), &root, &mut expected);
        // every file under THIRD_PARTY/ is a licence text or its index
        fn all(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    all(&p, root, out);
                } else {
                    out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        all(&root.join("THIRD_PARTY"), &root, &mut expected);
        // fonts from crates (not in this repository) listed in NOTICE need a licence text in
        // THIRD_PARTY/ named in their entry ("- Ubuntu-Light.ttf — … — THIRD_PARTY/fonts/…")
        let notice = DOCUMENTS[0].text;
        let mut entries: Vec<String> = Vec::new();
        for line in notice.lines() {
            if let Some(rest) = line.strip_prefix("- ") {
                entries.push(rest.to_string());
            } else if line.starts_with("  ")
                && let Some(last) = entries.last_mut()
            {
                last.push(' ');
                last.push_str(line.trim());
            } else if line.trim().is_empty() {
                entries.push(String::new());
            }
        }
        let mut crate_fonts = 0;
        for e in entries.iter().filter(|e| e.contains(".ttf") && !e.contains("assets/")) {
            crate_fonts += 1;
            let refs: Vec<&str> = e
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|w| w.starts_with("THIRD_PARTY/"))
                .collect();
            assert!(
                !refs.is_empty(),
                "NOTICE font entry without a THIRD_PARTY licence text: {e}"
            );
            for r in refs {
                assert!(root.join(r).is_file(), "{r} (named in NOTICE) does not exist");
                expected.push(r.to_string());
            }
        }
        assert!(crate_fonts >= 5, "the egui font entries are missing from NOTICE");
        // explicit references ("License text: assets/fonts/katex/OFL.txt")
        for text in [DOCUMENTS[0].text, DOCUMENTS[3].text] {
            for word in text.split(|c: char| c.is_whitespace() || "(),;:`\"'".contains(c)) {
                let word = word.trim_end_matches('.');
                let file = word.rsplit('/').next().unwrap_or("");
                if word.contains('/') && !word.contains("://") && is_licence_name(file) && root.join(word).is_file() {
                    expected.push(word.to_string());
                }
            }
        }
        expected.sort();
        expected.dedup();
        let embedded: Vec<&str> = DOCUMENTS.iter().map(|d| d.path).collect();
        for e in &expected {
            assert!(
                embedded.contains(&e.as_str()),
                "licence file {e} is not in the About licence viewer"
            );
        }
        assert!(expected.len() >= 15, "{expected:?}");
    }
}
