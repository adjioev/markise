# Phycat (built-in theme, vendored)

- Upstream: https://github.com/sumruler/typora-theme-phycat
- Commit: `b49f1fc6b193e333a3ebb186bbf1ae7f8fc21778` (2026-03-01)
- License: MIT, Copyright (c) 2024 徐继龙 — see [`LICENSE`](LICENSE) (copied unchanged from
  the repository root). Also listed in the repository `NOTICE`.
- License check and origin check (no derivation from the reference product's built-in CSS):
  done by the project lead before vendoring (2026-10-26).

## Files

| File | Upstream path | Changes |
|---|---|---|
| `phycat-sky.css` | `phycat-sky.css` | heading icons removed (see below) |
| `phycat-abyss.css` | `phycat-abyss.css` | heading icons removed (see below) |
| `phycat/phycat.light.css` | `phycat/phycat.light.css` | heading icon rules removed (see below) |
| `phycat/phycat.dark.css` | `phycat/phycat.dark.css` | heading icon rules removed (see below) |
| `LICENSE` | `LICENSE` | none |

## Edits (Markise, 2026-10-26)

The upstream author notes that the small heading icons are borrowed from the SiYuan theme
"Savor"; their licence is unclear, so they were removed:

- `phycat-sky.css`, `phycat-abyss.css`: the comment naming "Savor" and the six
  `--h1-icon-shape` … `--h6-icon-shape` custom properties (SVG `data:` URIs) at the top of
  `:root` were replaced by a one-line comment.
- `phycat/phycat.light.css`, `phycat/phycat.dark.css`: the rule block that draws the icons
  (`#write h3.md-heading:after, h4.md-heading:after, h5…, h6…` with its mask settings, and the
  four `#write hN.md-heading:after { mask-image: var(--hN-icon-shape) }` rules; upstream lines
  59–96) was replaced by a one-line comment. Without the variables these rules would paint
  empty coloured boxes in a browser export.

Nothing else was changed. Not vendored: `phycat/Cascadia-Code-Regular.ttf` and
`phycat/LXGWWenKai-Regular.ttf` (OFL-1.1 fonts are out of scope for this milestone); the
`@font-face` rules still reference them, the importer records them as missing files and the
viewer falls back to installed system fonts. Other colour variants (mint, forest, …), demo
files, screenshots and the "Theme Studio" pages were not vendored.
