# Markise Keys (keyboard symbol fallback font)

- File: `MarkiseKeys-Regular.ttf` (4 064 bytes), SHA-256
  `3aaf88295fec94a5e705fb242d67a35691949327975dc80933b22c6dd76cfc61`.
- What: a 24-glyph subset of **Inter Regular 3.019** (git `0a5106e0b`) by Rasmus Andersson / The
  Inter Project Authors, https://github.com/rsms/inter. Source file: `fonts/Inter-Regular.ttf`
  of the crates.io crate `cosmic-text` 0.19.0 (SHA-256
  `3127f0b873387ee37e2040135a06e9e9c05030f509eb63689529becf28b50384`), unchanged upstream Inter.
- License: SIL Open Font License 1.1 — see [`OFL.txt`](OFL.txt) (copied unchanged from the same
  crate, `fonts/Inter-LICENSE`; embedded in the About window's licence viewer). Copyright 2020
  The Inter Project Authors. Inter declares **no Reserved Font Name**, so a modified (subset)
  version may be distributed; it is renamed anyway ("Markise Keys") so it cannot be mistaken for
  Inter. OFL-1.1 is allowed for bundled font files only (owner decision 2026-10-27, AGENTS.md).
  Listed in the repository `NOTICE`.
- Why: the macOS keyboard symbols the tutorial and menus use (⌘ ⇧ ⌥ ⌃ ⎋ ↩ ⌫ ⌦ ⇥ ← → ↑ ↓ ⇞ ⇟ …,
  issue #131) are missing from egui's bundled fonts and from most body fonts. This font is the
  last-resort fallback on every platform, embedded by `crates/markise-text/src/keys.rs`.
- Code points (24): U+2190–U+2193 (← ↑ → ↓), U+2196 ↖, U+2198 ↘, U+21A9 ↩, U+21DE ⇞, U+21DF ⇟,
  U+21E4 ⇤, U+21E5 ⇥, U+21E7 ⇧, U+21EA ⇪, U+2303 ⌃, U+2305 ⌅, U+2318 ⌘, U+2325 ⌥, U+2326 ⌦,
  U+2327 ⌧, U+232B ⌫, U+238B ⎋, U+23CE ⏎, U+23CF ⏏, U+2423 ␣.

## How it was made (fontTools 4.68.0)

```sh
pyftsubset Inter-Regular.ttf \
  --unicodes="U+2318,U+21E7,U+2325,U+2303,U+238B,U+21A9,U+232B,U+2326,U+21E5,U+21E4,U+2190-2193,U+21DE,U+21DF,U+23CE,U+21EA,U+23CF,U+2423,U+2196,U+2198,U+2305,U+2327" \
  --layout-features='' --no-hinting --name-IDs='*' --output-file=keys-raw.ttf
```

then the `name` table was rewritten with fontTools (family "Markise Keys", style "Regular",
PostScript name `MarkiseKeys-Regular`, version "Version 1.000; subset of Inter 3.019
(git-0a5106e0b)"; names 16, 17, 18, 21, 22, 25 and the axis names 271, 272, 279 removed). The
copyright (0), trademark (7), designer/vendor (8, 9, 11, 12) and license (13, 14) records are kept
unchanged.
