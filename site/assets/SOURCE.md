# Website assets (attribution record)

All files here are Markise project material, released under the project license (MIT OR
Apache-2.0, see `LICENSE-MIT` / `LICENSE-APACHE`). Checked by `cargo xtask assets`.

## Brand artwork (copies and renders of `assets/brand/`, see `assets/brand/README.md` and NOTICE)

| File | Origin |
|---|---|
| `favicon.svg` | copy of `assets/brand/markise-favicon-dark.svg` |
| `favicon-32.png` | copy of `assets/brand/png/markise-32.png` |
| `apple-touch-icon.png` | copy of `assets/brand/png/markise-256.png` |
| `markise-lockup-dark.svg`, `markise-lockup-light.svg` | copies of the same files in `assets/brand/` (wordmark glyph outlines of Playfair Display, OFL-1.1; see NOTICE) |
| `markise-mark-dark.svg`, `markise-mark-light.svg` | copies of the same files in `assets/brand/` |
| `hero.jpg`, `hero.webp`, `hero-2x.jpg`, `hero-2x.webp` | resized from the owner's AI-generated hero illustration `assets/brand/source/markise-hero.png` |
| `social-preview.jpg` | JPEG of the owner's social card `assets/brand/social-preview.png` |

## Screenshots of Markise (`shots/`)

### Theme gallery

Rendered headlessly from the Markise app with the `theme_shots` example (branch
`themes-builtin`, 2026-10-27). They show Markise's own UI and sample text, drawn with egui's
bundled fonts or installed system fonts (no font files here). Themes shown: the vendored
built-in themes Drake (`drake-dark`) and Lapis (`lapis-light`), MIT, see
`assets/themes/*/SOURCE.md` and NOTICE; Markise's original palette themes using hex values from
Catppuccin, Dracula, Gruvbox, Nord, Rosé Pine and Tokyo Night (see NOTICE and
`assets/themes/palettes/SOURCES.md`); and the default Markise themes.

| File | Theme |
|---|---|
| `shots/markise-light.jpg`, `shots/markise-light.webp`, `shots/markise-light-2x.webp` | Markise light (default) |
| `shots/palette-markise-night.jpg`, `shots/palette-markise-night.webp` | Markise night |
| `shots/drake-dark.jpg`, `shots/drake-dark.webp` | Drake dark |
| `shots/lapis-light.jpg`, `shots/lapis-light.webp` | Lapis light |
| `shots/tokyo-night.jpg`, `shots/tokyo-night.webp`, `shots/tokyo-night-2x.webp` | Tokyo Night palette |
| `shots/palette-catppuccin-latte.jpg`, `shots/palette-catppuccin-latte.webp` | Catppuccin Latte palette |
| `shots/palette-dracula.jpg`, `shots/palette-dracula.webp` | Dracula palette |
| `shots/palette-gruvbox-light.jpg`, `shots/palette-gruvbox-light.webp` | Gruvbox light palette |
| `shots/palette-nord.jpg`, `shots/palette-nord.webp` | Nord palette |
| `shots/palette-rose-pine-moon.jpg`, `shots/palette-rose-pine-moon.webp` | Rosé Pine Moon palette |

### Feature screenshots (issue #62)

Rendered headlessly from the real app with the `site-*` scenes of
`crates/markise-ui-egui/examples/dialog_shots.rs` (branch `task/62-site-docs`, 2026-11-03,
macOS 26 on Apple silicon, installed system fonts and egui's bundled fonts, default Markise
theme, isolated empty configuration). All text is synthetic and defined in that example
(`VAULT_NOTES`, `SIDE_DOC`, `MATH_DOC`, `REVIEW_DOC`); no personal notes. The in-window menu bar
is the one Markise draws on Linux and in headless runs. Command (960 × 680 pt at 2×; the
review scene at 960 × 480 pt):

```sh
MARKISE_SHOTS=site-vault,site-side-by-side,site-math-diagrams MARKISE_SHOT_WIDTH=960 \
  MARKISE_SHOT_HEIGHT=680 MARKISE_SHOT_SCALE=2 \
  cargo run -p markise-ui-egui --release --example dialog_shots -- .screenshots/site site
MARKISE_SHOTS=site-review MARKISE_SHOT_WIDTH=960 MARKISE_SHOT_HEIGHT=480 MARKISE_SHOT_SCALE=2 \
  cargo run -p markise-ui-egui --release --example dialog_shots -- .screenshots/site site
```

The 1920 px PNGs were converted with ImageMagick and cwebp: `-2x.webp` at full size (cwebp
`-q 78`), `.webp` and `.jpg` resized to 1080 px wide (cwebp `-q 80`, JPEG quality 82).

| File | Scene and appearance |
|---|---|
| `shots/side-by-side.jpg`, `shots/side-by-side.webp`, `shots/side-by-side-2x.webp` | `site-side-by-side`, light |
| `shots/vault.jpg`, `shots/vault.webp`, `shots/vault-2x.webp` | `site-vault`, light |
| `shots/math-diagrams.jpg`, `shots/math-diagrams.webp`, `shots/math-diagrams-2x.webp` | `site-math-diagrams`, dark |
| `shots/review.jpg`, `shots/review.webp`, `shots/review-2x.webp` | `site-review`, light |
