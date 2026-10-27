# Lapis (built-in theme, vendored)

- Upstream: https://github.com/YiNNx/typora-theme-lapis
- Commit: `9124f8659699691edccdc52ba710c79aa801c621` (2025-04-01)
- License: MIT, Copyright (c) 2024 YiNN — see [`LICENSE`](LICENSE) (copied unchanged from the
  repository root). Also listed in the repository `NOTICE`.
- License check and origin check (no derivation from the reference product's built-in CSS):
  done by the project lead before vendoring (2026-10-26).

## Files

| File | Upstream path | Changes |
|---|---|---|
| `lapis.css` | `lapis.css` | none |
| `lapis-dark.css` | `lapis-dark.css` | none (upstream file mode 755 → 644) |
| `LICENSE` | `LICENSE` | none |

Not vendored: the fonts in `lapis/` (Cantarell, JetBrains Mono, Source Han Serif CN —
OFL-1.1 fonts are out of scope for this milestone). The `@font-face` rules still reference
them; the importer records them as missing files and the viewer falls back to installed system
fonts. Screenshots (`imgs/`) were not vendored.
