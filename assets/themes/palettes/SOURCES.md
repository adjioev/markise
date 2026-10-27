# Palette sources (native palette themes)

The palette themes in `crates/markise-theme/src/palettes.rs` are original Markise themes. They
use only the **hex values** of the palettes below and the palettes' published role guidance
(which color is background, surface, text, accent); the mapping to Markise theme slots
(headings, links, code boxes, quotes, tables, markers, selection, sidebar, alerts, code tokens)
is Markise's own design. Shades between two palette colors are computed mixes of those two
palette colors (`mx(a, b, pct)` in the source), never new hues. No theme files of the projects
below were copied. Markise Night is an original palette.

| Palette | Variants used | Source | Commit (fetched 2026-10-26) | License |
|---|---|---|---|---|
| Tokyo Night | Night, Storm | https://github.com/enkia/tokyo-night-vscode-theme (README palette table, `themes/*.json` editor/sidebar backgrounds) | `7c0f11eaef322f293621ca7befe462214b7ea468` | MIT, Copyright (c) 2018-present Enkia |
| Tokyo Night | Day (and `bg_highlight`/`bg_visual` shades of Night/Storm) | https://github.com/folke/tokyonight.nvim (`extras/lua/tokyonight_day.lua`, `lua/tokyonight/colors/*.lua`) | `cdc07ac78467a233fd62c493de29a17e0cf2b2b6` | Apache-2.0 (folke) |
| Catppuccin | Latte, Frappé, Macchiato, Mocha | https://github.com/catppuccin/palette (`palette.json`) | `07d02aa110ef9eb7e7427afca5c73ba9cf7f8ebd` | MIT, Copyright (c) 2021 Catppuccin |
| Nord | Nord (nord0–nord15) | https://github.com/nordtheme/nord (`src/nord.styl`) | `1cef71605416a222e57225b544540ce0fcec18d4` | MIT, Copyright (c) 2016-present Sven Greb |
| Gruvbox | dark, light | https://github.com/morhetz/gruvbox (`colors/gruvbox.vim` palette block) | `ef8864bb42bf244f0295d1c5a403b27e3d139695` | MIT/X11 (per README and package.json), author morhetz |
| Rosé Pine | Main, Moon, Dawn | https://github.com/rose-pine/palette (`palette.json`) | `92af52b465ab6e47437aca223c9b8d3009a2023b` | MIT, Copyright (c) mvllow |
| Dracula | Dracula | https://github.com/dracula/dracula-theme (README color palette specification) | `e22ebcc048b8aff586f9bc94db30ff5347070b42` | MIT, Copyright (c) 2023 Dracula Theme |

Copyright holders are as stated in each repository's LICENSE file at the commit above (the
gruvbox repository has no LICENSE file; its README and package.json declare MIT).

## Pairing in the Theme menu ("Palettes")

Tokyo Night Day ↔ Night, Catppuccin Latte ↔ Mocha, Gruvbox light ↔ dark, Rosé Pine Dawn ↔ Main;
Tokyo Night Storm, Catppuccin Frappé / Macchiato, Rosé Pine Moon, Nord and Dracula are dark-only
entries. Markise Night is listed with the Markise themes.

## Accessibility

`palettes::tests::contrast_meets_wcag_aa` checks WCAG 2 contrast ≥ 4.5:1 for body text, links,
inline code, code blocks, quotes and `mark` text in every palette theme. Where a palette's
documented accent fell below 4.5:1 for a role, another color of the same palette was chosen
(Catppuccin Latte links use mauve; Tokyo Night Day links use `blue5`, quotes use `fg`).
