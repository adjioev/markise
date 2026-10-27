# Markise brand assets

The mark is a **gold marquise crown over a heavy, rounded "M"** in a squircle: five crown points
with three diamond cut-outs, the letter below. Crown + M = *marquise* + *Markdown*.

**Origin.** The concept (icon, wordmark lockup, website hero illustration and social card) was
made by the project owner with an AI image generator; the originals are in `source/`. The icon
and lockups here are a **clean vector redraw** of that concept: the geometry was measured from
`source/markise-icon-dark.png` and rebuilt as filleted polygons in `tools/make_svgs.py`, so all
icons are generated from SVG, not from the (noisy, raster) originals. The hero illustration and
the social card are used as raster images.

All artwork is released under the project license (MIT OR Apache-2.0).

## Files

| File | Use |
|---|---|
| `markise-icon-dark.svg` | **App icon** (navy squircle, lavender M, gold crown): PNGs, `.ico`, window icon |
| `markise-icon-light.svg` | Light variant (cream squircle, navy M) |
| `markise-icon-{dark,light}-macos.svg` | Same artwork on the macOS Big Sur icon grid (824/1024 body, soft shadow): `.icns` |
| `markise-favicon-{dark,light}.svg` | Simplified artwork for 16–32 px: solid three-point crown, no cut-outs, bigger M |
| `markise-mark-light.svg` / `-dark.svg` | Crown + M without background, for light / dark backgrounds |
| `markise-lockup-light.svg` / `-dark.svg` | Horizontal lockup: mark + "Markise" wordmark |
| `markise-lockup-tagline-light.svg` / `-dark.svg` | Lockup with the letter-spaced tagline "THE MARQUISE OF MARKDOWN EDITORS" |
| `png/markise-{16,32,48,64,128,256,512,1024}.png` | App icon rasters (16/32 from the favicon artwork) |
| `png/markise-light-{16,32,256,512}.png` | Light variant rasters |
| `markise.icns`, `markise.ico` | macOS / Windows icon containers (dark variant) |
| `social-preview.png` | 1280×640 social card (Open Graph image; upload as the GitHub repo social preview) |
| `source/` | Owner's AI-generated originals (icon dark/light, wordmark dark/light, hero, social card) |
| `explorations/` | Earlier hand-drawn directions, not used (see below) |
| `tools/` | `make_svgs.py` (all SVGs from one geometry), `outline_text.py` (text to outlines), `build.sh` (rasters, `.ico`, `.icns`) |

Palette: navy `#1a1b26` (dark background, M on light), lavender-white `#bbc8f2` (M on dark;
wordmark on dark `#c0caf5`), gold `#e0af68` (crown on dark) / `#daab55` (crown on light),
cream `#faf6ee` (light background).

Type (lockups only, converted to outlines — no font is needed at run time and no font file is
committed): wordmark **Playfair Display** SemiBold, tagline **Nunito** Medium, both under the
SIL Open Font License 1.1 (github.com/google/fonts: `ofl/playfairdisplay`, `ofl/nunito`).
OFL-licensed glyph outlines may be used in a logo; see `NOTICE`.

## Rebuilding

```sh
assets/brand/tools/build.sh                     # PNGs, .ico, .icns from the committed SVGs
FONT_DIR=~/fonts PYTHON=venv/bin/python assets/brand/tools/build.sh --svg   # also regenerate the SVGs
```

Tools: `rsvg-convert` (librsvg) and ImageMagick `magick` (Homebrew; build tools only, nothing is
linked into Markise), `iconutil` (ships with macOS). Regenerating the SVGs needs Python with
`fonttools` and `uharfbuzz` and the variable fonts `PlayfairDisplay.ttf` and `Nunito.ttf`
(renamed from `PlayfairDisplay[wght].ttf` / `Nunito[wght].ttf`).

## Small sizes

At 16 and 32 px the five crown points and three diamond cut-outs blur into a smudge, so
`markise-favicon-*.svg` uses a solid three-point crown and a bigger M at full bleed. From 48 px
up the full artwork is used.

## Explorations (not chosen)

Hand-drawn directions made before the owner's concept arrived, kept for reference:

- `a-crown-m-plum/` — a crown whose body is the M, pearls on spiked stems, a navette gem, plum
  and gold. Legible at all sizes but more abstract.
- `b-crown-over-hash.svg` — a crown over a Markdown `#`; two stacked symbols, busy at 16 px.
- `c-navette-m-arrow.svg` — a marquise-cut gem outline holding "M↓"; thin outlines vanish at
  small sizes and it echoes an existing Markdown logo.

**Chosen: the owner's crown-over-M concept**, redrawn as vector. It is the most immediately
readable (a letter you know under a crown you know), the heavy rounded M survives down to 16 px,
and the navy / lavender / gold palette matches the website and the dark editor themes.
