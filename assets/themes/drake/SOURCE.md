# Drake (built-in theme, vendored)

- Upstream: https://github.com/liangjingkanji/DrakeTyporaTheme
- Commit: `771df28080610cf6b0bd5f6f0c1864efa2d9a41e` (2024-12-15)
- License: MIT, Copyright (c) 2023 劉強東 — see [`LICENSE`](LICENSE) (copied unchanged from
  the repository root). Also listed in the repository `NOTICE`.
- License check and origin check (no derivation from the reference product's built-in CSS):
  done by the project lead before vendoring (2026-10-26).

## Files

| File | Upstream path | Changes |
|---|---|---|
| `drake-light.css` | `drake-light.css` | none |
| `drake-dark.css` | `drake-dark.css` | none |
| `drake/font.css` | `drake/font.css` | none |
| `LICENSE` | `LICENSE` | none |

Not vendored: the JetBrains Mono `.woff2` files in `drake/` (OFL-1.1 fonts are out of scope for
this milestone). `drake/font.css` still references them; the importer records those
`@font-face` sources as missing files and the viewer falls back to installed system fonts
(`local("JetBrains Mono")` is honored when that font is installed).
