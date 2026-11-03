//! The welcome tutorial (issue #63): a hands-on Markdown page bundled with the app
//! (`assets/tutorial/welcome.md`), opened at the first launch and from Help → Tutorial.
//!
//! It opens as an **untitled scratch copy**: nothing is read from or written to disk, so it
//! can never change a user file. Closing it without edits asks nothing; after an edit the
//! usual unsaved-changes question appears and Save proposes [`FILE_NAME`]. Images pasted into
//! it go where every untitled document's go (the app-data image folder).
//!
//! First launch: the desktop app passes [`crate::AppOptions::welcome`]; once the settings file
//! has been read and its `tutorialShown` flag is unset, the tutorial opens (only when the
//! launch opened nothing else) and the flag is set, so it is not shown again.

/// The tutorial text.
pub const TEXT: &str = include_str!("../../../assets/tutorial/welcome.md");

/// The file name Save As proposes for the tutorial.
pub const FILE_NAME: &str = "Welcome to Markise.md";

#[cfg(test)]
mod tests {
    use markise_md::{Document, ParseOptions};

    use super::*;

    /// Apple's order of modifier glyphs: ⌃⌥⇧⌘, then the key.
    fn mac_glyphs(c: &crate::menu::Chord) -> String {
        let mut s = String::new();
        for (on, g) in [(c.ctrl, '⌃'), (c.alt, '⌥'), (c.shift, '⇧'), (c.meta, '⌘')] {
            if on {
                s.push(g);
            }
        }
        s.push_str(match c.key.as_str() {
            "Left" => "←",
            "Right" => "→",
            "Up" => "↑",
            "Down" => "↓",
            k => k,
        });
        s
    }

    #[test]
    fn tutorial_parses_with_its_constructs_and_round_trips() {
        let opts = ParseOptions {
            wikilinks: true,
            critic_markup: true,
            ..ParseOptions::default()
        };
        let doc = Document::parse(TEXT.to_string(), opts);
        assert_eq!(doc.text(), TEXT, "lossless");
        assert!(TEXT.starts_with("# Welcome to Markise\n"));
        for needle in [
            "```mermaid",
            "$$\n",
            "- [ ] ",
            "{++",
            "{>>",
            "[[My first note]]",
            "|:---|:---:|",
        ] {
            assert!(TEXT.contains(needle), "{needle}");
        }
    }

    /// No broken internal references: every `#anchor` link names a heading of the page (the
    /// only wikilink is the deliberately unresolved example of section 8).
    #[test]
    fn tutorial_has_no_broken_internal_links() {
        let headings: Vec<String> = TEXT
            .lines()
            .filter_map(|l| l.strip_prefix('#'))
            .map(|h| {
                h.trim_start_matches('#')
                    .trim()
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                    .collect::<String>()
                    .replace(' ', "-")
            })
            .collect();
        for (i, _) in TEXT.match_indices("](#") {
            let rest = &TEXT[i + 3..];
            let anchor = &rest[..rest.find(')').unwrap()];
            assert!(headings.iter().any(|h| h == anchor), "#{anchor}");
        }
        let wikilinks: Vec<&str> = TEXT.match_indices("[[").map(|(i, _)| &TEXT[i..]).collect();
        assert!(
            wikilinks
                .iter()
                .all(|w| w.starts_with("[[My first note]]") || w.starts_with("[[wikilinks]]")),
            "{wikilinks:?}"
        );
    }

    /// Shortcuts are written as `<kbd>` key caps (issue #131: breathing room between chords
    /// and the surrounding text, readable in other Markdown tools): no modifier glyph outside a
    /// key cap, every key cap closed on its line, and the live view pairs them into keyboard
    /// nodes (also inside table cells).
    #[test]
    fn tutorial_shortcuts_are_key_caps() {
        let mut caps = 0;
        for line in TEXT.lines() {
            let mut rest = line;
            let mut outside = String::new();
            while let Some(a) = rest.find("<kbd>") {
                outside.push_str(&rest[..a]);
                let b = rest[a..]
                    .find("</kbd>")
                    .unwrap_or_else(|| panic!("unclosed <kbd>: {line}"));
                let key = &rest[a + 5..a + b];
                assert!(!key.is_empty() && !key.contains(' ') && !key.contains('<'), "{line}");
                caps += 1;
                rest = &rest[a + b + 6..];
            }
            outside.push_str(rest);
            assert!(
                !outside.contains(['⌘', '⌥', '⌃', '⇧']),
                "chord outside a key cap: {line}"
            );
        }
        assert!(caps >= 40, "{caps}");
        // the live view pairs the tags into key caps: the chords show, the tags do not
        let shown = markise_layout::testing::Shown::new(std::sync::Arc::new(markise_md::parse(TEXT)));
        let mut view = shown.view(markise_theme::Theme::light(), 900.0, 1.0);
        let mut fonts = egui::epaint::text::Fonts::new(
            markise_layout::testing::text_options(),
            crate::fonts::definitions(false),
        );
        let mut sh = fonts.with_pixels_per_point(1.0);
        let text: String = view
            .frame(100_000.0, 0.0, &mut sh, &markise_layout::block::NoImages)
            .iter()
            .flat_map(|v| &v.layout.prims)
            .filter_map(|p| match p {
                markise_layout::block::Prim::Text { text, .. } => Some(text.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            text.contains("⇧⌘G") && text.contains("⌥⌘F") && text.contains("⌃⇧`"),
            "{text}"
        );
        assert!(!text.contains("kbd>"), "{text}");
    }

    /// The shortcuts the text names are the real default chords on macOS (the text's
    /// platform), and the menu items it names exist.
    #[test]
    fn tutorial_shortcuts_and_menu_items_match_the_app() {
        use markise_engine::editor::command::Registry;
        use markise_engine::editor::keymap::{Keymap, Platform};
        let r = Registry::for_format(&markise_md::MarkdownFormat::default());
        let k = Keymap::new(&r, Platform::Mac);
        let chord = |id: &str| -> String {
            match crate::menu::app_command(id) {
                Some(d) if r.get(id).is_none() => mac_glyphs(&crate::menu::descriptor_chord(d, Platform::Mac).unwrap()),
                _ => mac_glyphs(k.shown_chord_of(id).unwrap_or_else(|| panic!("{id}"))),
            }
        };
        // (as written in the text, command)
        let named = [
            ("<kbd>⌘B</kbd>", "format.strong"),
            ("<kbd>⌘I</kbd>", "format.emph"),
            ("<kbd>⌘U</kbd>", "format.underline"),
            ("<kbd>⌃\\`</kbd>", "format.code"),
            ("<kbd>⌃⇧\\`</kbd>", "format.strike"),
            ("<kbd>⌘K</kbd>", "format.link"),
            ("<kbd>⌘\\\\</kbd>", "format.clear"),
            ("<kbd>⌘1</kbd>", "block.heading1"),
            ("<kbd>⌘6</kbd>", "block.heading6"),
            ("<kbd>⌘0</kbd>", "block.paragraph"),
            ("<kbd>Tab</kbd>", "edit.indentOrNextCell"),
            ("<kbd>⇧Tab</kbd>", "edit.outdentOrPrevCell"),
            ("<kbd>⌥⌘Q</kbd>", "block.quote"),
            ("<kbd>⌥↑</kbd>", "edit.moveBlockUp"),
            ("<kbd>⌥↓</kbd>", "edit.moveBlockDown"),
            ("<kbd>⌘/</kbd>", "view.sourceMode"),
            ("<kbd>⌃⌘/</kbd>", "view.sideBySide"),
            ("<kbd>⌘F</kbd>", "find.open"),
            ("<kbd>⌘G</kbd>", "find.next"),
            ("<kbd>⇧⌘G</kbd>", "find.prev"),
            ("<kbd>⌥⌘F</kbd>", "find.replace"),
            ("<kbd>⇧⌘O</kbd>", "file.openFolder"),
            ("<kbd>⌘P</kbd>", "file.quickOpen"),
            ("<kbd>⌥⌘←</kbd>", "go.back"),
            ("<kbd>⌥⌘→</kbd>", "go.forward"),
            ("<kbd>⇧⌘=</kbd>", "view.zoomIn"),
            ("<kbd>⇧⌘-</kbd>", "view.zoomOut"),
            ("<kbd>⇧⌘0</kbd>", "view.actualSize"),
            ("<kbd>⌃⌘F</kbd>", "view.fullScreen"),
            ("<kbd>⌘,</kbd>", "app.preferences"),
            ("<kbd>⌘?</kbd>", "help.shortcuts"),
        ];
        for (text, id) in named {
            assert!(TEXT.contains(text), "the text names {text}");
            // the key caps and escapes are Markdown, not part of the chord
            let plain = text
                .replace("<kbd>", "")
                .replace("</kbd>", "")
                .replace("\\\\", "\\")
                .replace("\\`", "`");
            let real = chord(id).replace("⇧⌘/", "⌘?");
            assert_eq!(plain, real, "{id}");
        }
        // menu items named by the text
        let labels: Vec<&str> = crate::menu::APP_COMMANDS.iter().map(|d| d.label).collect();
        for l in [
            "Recover Unsaved Drafts…",
            "Save Copy Without Comments…",
            "Theme Settings…",
            "Import Theme…",
            "Tutorial",
        ] {
            assert!(labels.contains(&l), "{l}");
            assert!(TEXT.contains(l.trim_end_matches('…')), "{l}");
        }
        assert!(labels.contains(&"Keyboard Shortcuts"));
    }
}
