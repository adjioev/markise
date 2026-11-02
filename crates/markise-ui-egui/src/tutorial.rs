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
            ("⌘B", "format.strong"),
            ("⌘I", "format.emph"),
            ("⌘U", "format.underline"),
            ("⌃\\`", "format.code"),
            ("⌃⇧\\`", "format.strike"),
            ("⌘K", "format.link"),
            ("⌘\\\\", "format.clear"),
            ("⌘1", "block.heading1"),
            ("⌘6", "block.heading6"),
            ("⌘0", "block.paragraph"),
            ("**Tab**", "edit.indentOrNextCell"),
            ("**⇧Tab**", "edit.outdentOrPrevCell"),
            ("⌥⌘Q", "block.quote"),
            ("⌥↑", "edit.moveBlockUp"),
            ("⌥↓", "edit.moveBlockDown"),
            ("**⌘/**", "view.sourceMode"),
            ("**⌃⌘/**", "view.sideBySide"),
            ("**⌘F**", "find.open"),
            ("**⌘G**", "find.next"),
            ("**⇧⌘G**", "find.prev"),
            ("**⌥⌘F**", "find.replace"),
            ("**⇧⌘O**", "file.openFolder"),
            ("**⌘P**", "file.quickOpen"),
            ("**⌥⌘←**", "go.back"),
            ("**⌥⌘→**", "go.forward"),
            ("⇧⌘=", "view.zoomIn"),
            ("⇧⌘-", "view.zoomOut"),
            ("⇧⌘0", "view.actualSize"),
            ("⌃⌘F", "view.fullScreen"),
            ("⌘,", "app.preferences"),
            ("**⌘?**", "help.shortcuts"),
        ];
        for (text, id) in named {
            assert!(TEXT.contains(text), "the text names {text}");
            // the text's escapes and emphasis are Markdown, not part of the chord
            let plain = text.replace("**", "").replace("\\\\", "\\").replace("\\`", "`");
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
