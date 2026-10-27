# Licences of the fonts bundled inside egui

The desktop app links egui with its `default_fonts` feature, which compiles the fonts of the
crate `epaint_default_fonts` **0.37.0** (crates.io; licence
`(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`) into the binary as fallback fonts. The
licence texts below are copied unchanged from that crate's `fonts/` folder:

| Font file (in the crate) | Licence | Text in this folder (crate file) |
|---|---|---|
| `Ubuntu-Light.ttf` | Ubuntu Font Licence 1.0 | `ubuntu-font-licence-1.0.txt` (`UFL.txt`) |
| `Hack-Regular.ttf` | MIT (Source Foundry Authors) + Bitstream Vera License | `hack-MIT-Bitstream.txt` (`Hack-Regular.txt`) |
| `NotoEmoji-Regular.ttf` | SIL Open Font License 1.1 | `noto-emoji-OFL.txt` (`OFL.txt`) |
| `emoji-icon-font.ttf` | MIT, Copyright (c) 2014 John Slegers | `emoji-icon-font-MIT.txt` (`emoji-icon-font-mit-license.txt`) |
| `egui-icons.ttf` (five-glyph subset of emoji-icon-font) | MIT, Copyright (c) 2014 John Slegers | `emoji-icon-font-MIT.txt`, `egui-icons.txt` (`egui-icons.txt`) |

When the egui version changes, compare these texts with the new crate's `fonts/` folder.
