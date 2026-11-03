# Themes

A Markise theme has two independent parts:

- a **style** — typography and spacing: fonts, sizes, line height, margins;
- **colours** — a colour scheme, often with a light and a dark variant.

You can use a complete theme, or combine the style of one with the colours of another.

## Choose a theme

- **View → Theme** switches quickly between Markise, Markise Night and the bundled themes
  Drake, Lapis and Phycat, plus your own themes under **My Themes**.
- **Preferences → Appearance** (also **View → Theme → Theme Settings…**) shows all themes as
  preview cards and lets you pick the style and the colours separately. When they come from
  different themes, it says so ("Custom").

Built in:

| | |
|---|---|
| Styles | Markise Default, Classic, Modern, Technical, Compact |
| Colour schemes | Markise (light and dark), Markise Night, Catppuccin (Latte, Mocha, Frappé, Macchiato), Dracula, Gruvbox (light and dark), Nord, Rosé Pine (Dawn, Main, Moon), Tokyo Night (Day, Night, Storm) |
| Bundled community themes | Drake, Lapis, Phycat — each with light and dark variants, used under their MIT licences |

The colour palettes are Markise's own themes built from the published colour values of those
projects.

## Light, dark or system

**Preferences → Appearance → Light or dark**: **System** (the default) follows your operating
system's appearance and switches with it; **Light** and **Dark** keep one variant. The sidebar,
outline and dialogs take their colours from the theme too.

## Fonts and size

**Preferences → Appearance → Font & size** overrides the style's body font (with separate
choices for Cyrillic, Greek and CJK text), base size (9–36), line height (1.0–3.0) and content
width. Each override has a reset button that returns to the style's value. **Zoom** (**View →
Zoom In / Zoom Out / Actual Size**) scales the whole window between 50 % and 300 %.

## Import a CSS theme

Markise can import community CSS themes written for Typora:

1. **Preferences → Appearance → Import Theme…** and choose the theme's `.css` file.
2. Markise copies the CSS file and the files next to it into its themes folder. If a theme with
   that name exists, it asks before replacing it.
3. The theme appears in **View → Theme → My Themes** and in Preferences.

**Open Theme Folder** shows the themes folder:

| System | Themes folder |
|---|---|
| macOS | `~/Library/Application Support/markise/themes/` |
| Windows | `%APPDATA%\markise\themes\` |
| Linux | `~/.config/markise/themes/` (or `$XDG_CONFIG_HOME/markise/themes/`) |

Edit a theme's CSS in that folder and Markise reloads it as soon as the file changes. A file
named `base.user.css` or `<theme>.user.css` next to the themes adds your own rules on top.

Only import themes whose licence allows it, and never copy the themes that ship with other
commercial editors.

## What "compatible" means

Markise does not contain a web browser. It reads a theme's CSS with its own parser and applies
the subset of CSS it supports to its own rendering — properties such as colours, fonts, sizes,
spacing and backgrounds, CSS variables (`var()`), light and dark variants written
with `@media (prefers-color-scheme)`, and `@import` of files inside the theme's folder. Other
rules are skipped, so an imported theme can look different from the same theme in other
editors. Some limits:

- Fonts the theme loads with `@font-face` are not loaded; an installed font with the same name
  is used instead.
- Generated content (`::before` / `::after` text and icons) is not drawn.
- `:has()` selectors and layout tied to the window width are not supported.
- Theme files may be up to 2 MiB each and 8 MiB together.

### Theme Report

**Preferences → Appearance → Theme Report…** explains how a theme was applied: how many
declarations were used and how many were ignored, unsupported properties, values, selectors and
at-rules, fonts that are not installed, and the files copied by an import. Use it when a theme
does not look as expected.

Typora is a trademark of its respective owner. Markise is an independent project and is not
affiliated with or endorsed by Typora; importing a theme does not imply any endorsement by its
author.
