# ADR 0004 — Built-in themes: vendored CSS imported at run time; theme UI and settings

Status: accepted for the `themes-builtin` milestone (2026-10-26). Scope: `assets/themes/`,
`crates/markise-theme` (`builtin`, `catalog`, `install`), `crates/markise-engine`
(`settings`, `watch`), `crates/markise-layout` (theme model use), `crates/markise-ui-egui`
(`themes`, `fonts`, Theme menu).

## Context

The importer (ADR 0003) turns spec 03 CSS themes into the native `Theme`, but the viewer only
used a subset of the model and offered only the original Markise Light/Dark themes. This
milestone ships three permissively licensed community themes as built-ins (Drake, Phycat,
Lapis — MIT, license- and origin-checked by the lead), a Theme menu with user themes, theme
import, live reload, and a persisted selection.

## Threat model (lead/owner decision, 2026-10-27)

Markise is a local, single-user application. File handling defends against **accidents**:
two Markise instances at once, external editors and cloud sync (Dropbox, iCloud) touching the
config folder, and crashes in the middle of a write. It does **not** defend against a malicious
local process racing it (e.g. swapping symbolic links inside the user's own config folder at the
right moment). Fixes are chosen to be simple and robust under that model:

- writes go through exclusively created temporary files renamed into place;
- theme installs are serialized across Markise processes by an advisory lock file and re-check
  collisions right before committing; publication without approved replacement uses
  no-replace semantics (hard link), and rollback touches only what the transaction moved or
  created;
- built-in extraction and pruning create folders component by component and never follow a
  symbolic link (checked per component with `symlink_metadata`); generations are leased by every
  process using them (shared file lock) and pruned only when the exclusive lease can be taken;
- only fonts in the OS-protected system font folder are memory-mapped (a mapped file truncated
  by another program would fault the process); all other fonts are owned, validated snapshots.

## Decision 1 — built-ins are imported at run time by the regular importer

Options:

1. **Import at build time** (a `build.rs` running the importer and generating Rust code or a
   serialized `Theme`). Needs the importer as a build-dependency of another crate, a
   serialization or code generator for every `Theme` field, and a rebuild for importer fixes.
2. **Hand-port** the themes to native `Theme` values. Loses fidelity, has to be redone for every
   upstream update, and the result is no longer the vendored, attributed file.
3. **Embed the CSS, import at run time** (chosen).

The vendored files (`assets/themes/<name>/`, unchanged except the documented Phycat edit) are
embedded with `include_bytes!` (`markise_theme::builtin`). On first use they are written to a
content-addressed folder `<config>/markise/builtin-themes/<hash>/<name>/` (temp folder when the
app runs without a config folder; exclusive temp files renamed into place; each generation
records the files it owns in a manifest and is leased by every process using it; only
manifest-listed files of unleased generations older than the newest previous one are removed,
never through a symbolic link — unexpected files stay) and imported from there with
`import_theme_file`, exactly like user themes: same `@import` confinement (TI-9), same report,
same code path, and importer improvements apply without regenerating anything. Only the selected
variant is imported, on the theme worker thread; an import takes 1–5 ms (release). The first
frame waits at most 300 ms for the selected theme so the window does not flash the default
theme; the Markise Light/Dark themes stay native (`Theme::light/dark`) and remain the default.

The binary carries ~230 KB of CSS. Fonts bundled by the upstream themes (OFL) are **not**
vendored; their `@font-face` rules stay and are reported as missing files.

## Decision 2 — the catalog groups themes into light/dark families

The Theme menu lists *families* (`markise_theme::catalog`): Markise, the built-ins (each with a
fixed light and dark file), and user themes from `<config>/markise/themes/` (existing discovery).
A user theme with `@media (prefers-color-scheme)` is one family with both variants; otherwise
files whose stems differ only by `-dark`/`-light`/`-night`/`-day` (also `_`, `.`) pair up;
anything else is a single-variant family. The appearance setting — Follow System / Light /
Dark — picks the family's matching variant and falls back to the only variant it has. Window
chrome (egui visuals) follows the *rendered* theme's dark flag and chrome colors (spec 03 §1.4,
§2.2), so a dark-only theme in light mode still gets dark chrome.

## Decision 3 — settings file

> Amended by ADR 0007: version 2 stores `style` + `colors` (+ `overrides`) instead of `theme`;
> version-1 files migrate.

`<config>/markise/settings.json` (`markise_engine::settings`), created on the first change:

```json
{ "version": 1, "theme": "builtin:drake", "appearance": "system" }
```

`theme`: `markise` | `builtin:<drake|lapis|phycat>` | `user:<family key>` (lowercase stem
without the light/dark suffix); `appearance`: `system` | `light` | `dark`. Unknown keys are
preserved, malformed files mean defaults, writes go through temp file + rename on the worker.
An unknown theme id falls back to Markise. The file is read and written on the theme worker
(exclusive temp file + rename). Tests and measurements run with `ConfigDir::None`
(nothing read or written in the user's folder).

## Decision 4 — "Import theme…", open folder, live reload

- **Import** (`markise_theme::install`): the chosen `{name}.css`, its asset folder `{name}/`
  and the files the importer actually reads for it (`@import` targets, existing local font
  files) are copied into an **isolated package** `{name}.markise-theme/` (paths relative to the
  source folder, only sources that resolve canonically inside it; symlinks skipped; bounded by
  files, bytes, depth and entries/folders visited during enumeration). A one-line stub
  `{name}.css` `@import`s the package stylesheet, so discovery, the label and
  `{name}.user.css` work unchanged and no other user file can be overwritten. If the stub or
  package exists, nothing is written and the UI asks "Replace?" first. Installs hold the
  advisory lock `.markise-install.lock`; files are staged in a fresh temporary folder inside the
  user folder and moved in by rename (old package moved into a unique backup folder until the
  new one and the stub are in place; restored on failure); without approved replacement the stub
  is published with a hard link, so a stub that appeared meanwhile is reported, not replaced. User layers are not
  copied. The theme is then selected and the import report shown; "Theme Report…" shows it
  for the active theme at any time.
- **Open Theme Folder** creates the folder and opens it in the platform file manager.
- **Live reload**: the existing `FileWatcher` also watches the user theme folder recursively
  (`WatchEvent::ThemeChanged`). Each import records its dependencies (files read plus the
  folder's `base.user.css` / `{name}.user.css` and every local `@import` target tried, even
  when absent); a change to a file or to a folder above a dependency drops cached and failed
  imports depending on it, and imports in flight carry a per-path generation so a result
  read before the change is discarded and re-requested. The active variant is re-imported; the new theme has a new
  fingerprint, so `View::set_params` invalidates the layout cache (per-item keys include the
  fingerprint). Top-level additions/removals rescan the catalog.

## Decision 5 — fonts by family name, `@font-face` files later

`ui-egui::fonts` indexes installed fonts once (platform font folders, name tables via
`ttf-parser`, which epaint already depends on; on the worker; ≈0.8 s for 1 GB of user fonts
here) and resolves each stack (body, heading, mono, sidebar) to up to three installed families in
stack order, regular and bold faces, in front of egui's fallbacks. Fonts in the OS-protected
system font folder (macOS `/System/Library/Fonts`, `%WINDIR%\Fonts`, `/usr/share/fonts`), opened
by a path whose components below that folder are not symbolic links, are memory-mapped (read-only
to user programs under the threat model; pages become resident only when glyphs are used; epaint
borrows them without a copy). Every other face — user and admin-writable folders — is an owned
snapshot (≤ 64 MiB per file) read on the theme worker, validated with epaint's own parser
(skrifa since egui 0.37; ab_glyph before) before it is kept, shared with egui through `Arc`, and retained in a process-wide
store bounded to 128 MiB (owned faces count once: since egui 0.37 epaint parses the shared bytes
in place, where 0.33 kept its own parser copy and owned faces counted twice; faces
egui no longer uses are evicted least-recently-used). Faces that fail validation or do not fit
are skipped and listed in the theme report.

**Lazy fallback fonts.** The CJK fallback is not loaded at start: the layout reports when an item
it lays out contains a character the font chain has no glyph for (`View::missing_glyphs`), the
app then asks the worker to load the fallback, and the font-generation bump lays the affected
items out again (the first frame never waits for it). Headless RSS, 1 MB corpus document, system
fonts, release (`cargo run -p markise-ui-egui --release --example rss_probe -- <doc>`; 3 runs):

| Build | Latin-only first screen | CJK on the first screen |
|---|---|---|
| owned snapshots, CJK loaded on demand (intermediate) | 26–27 MB | 72–73 MB |
| protected-folder mapping + lazy CJK (current) | 24 MB | 25–26 MB |

(The desktop app adds the eframe/winit/GL window baseline, ~90 MB per ADR 0002, to these
figures; the difference between the two columns carries over.)

Generic names map to per-platform lists (`sans-serif`/`system-ui` = the default
body font). `local("…")` names of `@font-face` rules are tried; the font **files** of
`@font-face` are not loaded (C3, next milestone with custom shaping). Because epaint cannot
synthesize bold, families with a real bold face come first in the bold role (a variable font
without static bold instances keeps regular text, bold text uses the next family's bold).

## What the viewer honors from the model

Honored: page/text colors, body size, line height, column width and padding, block spacing;
font stacks by family name (body, heading, mono) and mono scale; per heading level size, color,
weight, margins, bottom rule, italic, alignment, `text-transform`, letter spacing, generated
`::before`/`::after` text (+ prefix color), background with padding/radius, full-width or
fit-to-text, left bar; link color and underline (drawn under the baseline); inline code
color/background box with padding/radius; strike (drawn through the glyphs) and `del` color;
`mark` background and text color; `strong`/`em` colors; paragraph `text-align` incl. justify;
text-align: justify by widening inter-word spaces only (never letters, not inside code; last
row, hard breaks and rows whose spaces would exceed 1.5× their normal width stay ragged);
blockquote bar color/width, indent, text color, background, italic, generated prefix; list
indent, marker color, bullet style per level (disc/circle/square/none), number style
(decimal, decimal-leading-zero, lower/upper-alpha/latin/roman, none); task checkbox colors; rule
color; table border, header and zebra fills, cell padding; code/math/meta box colors, border,
padding, radius; alert colors; chrome: sidebar, control text, active/hover fill, selection,
accent/link in egui visuals.

Not yet: syntax token colors (no highlighter until `markise-blocks` gets one), real italic
faces (faux italics), `@font-face` files (C3), heading/quote vertical box padding across items,
quote `::before` bars drawn as pseudo-elements, `box-shadow`, gradients beyond an average
color, focus-mode dimming (TI-4), markers (`.md-meta`, no visible Markdown markers in the
read-only viewer). See TH-1…TH-8 in `docs/spec/99-open-questions.md`.

## Importer changes made for the built-ins

Found while checking the screenshots; all generic: `color-mix()` (premultiplied sRGB for every
interpolation space), an opaque gradient background approximated by its average stop color
(transparent patterns stay ignored), `width: fit-content`/`display: inline(-block)` → heading
"fit", heading padding/radius/left bar, quote text = the computed (inherited) color, and no
leaking of Markise's own heading rules, table header/zebra fills or code-box border into themes
that style those elements themselves. Corpus (22 themes): still no panics or warnings, mean
coverage 99.5%.

## Consequences

- Built-in themes are real vendored files with LICENSE + SOURCE.md + NOTICE entry; updating one
  is a file copy plus a new pinned commit.
- First run writes ~230 KB to the config folder; switching themes costs one import (ms) plus,
  for a new font set, an egui font-atlas rebuild and relayout of the visible items.
- Headless screenshots: `cargo run -p markise-ui-egui --release --example theme_shots`
  (software rasterizer, output in the gitignored `.screenshots/`).

## Decision 6 — palette theme pack (native, original)

> Amended by ADR 0007: palette themes are color schemes usable with any style.

Owner request (2026-10-26): original Markise themes from published MIT/Apache palettes —
Tokyo Night (Night, Storm, Day), Catppuccin (Latte, Frappé, Macchiato, Mocha), Nord, Gruvbox
(dark, light), Rosé Pine (Main, Moon, Dawn), Dracula — plus **Markise Night**, an original dark
theme. They are written as native data, not CSS ports (`markise_theme::palettes`):

- a `Palette` holds the palette's colors by role (bg, bg_alt, surface, overlay, text, ink,
  subtext, muted, accent, accent2, code, code_bg, block_bg, hues, selection, code tokens);
  values are the palettes' hex values, intermediate shades are mixes of two palette colors;
- one Markise-designed mapping (`palettes::build`) turns every palette into a `Theme`
  (bright h1/h2 with a rule, accent2 h3/h4, accent links, code on a raised surface, accent2
  quote bar and markers, palette hues for alerts, sidebar on bg_alt, `mark` as a yellow tint
  or full yellow, whichever is legible);
- a test checks WCAG AA (≥ 4.5:1) for body text, links, inline code, code blocks, quotes and
  `mark` text in every palette theme; where a documented accent fell short, another color of the
  same palette is used (recorded in `assets/themes/palettes/SOURCES.md`);
- `SyntaxColors` gained `function` and `type_name`; each palette defines keyword / string /
  number / comment / function / type colors for the future highlighter (the importer fills the
  new slots from `cm-def` and `cm-type`/`cm-variable-3`);
- menu: a "Palettes" section with light/dark pairs (Tokyo Night Day↔Night, Catppuccin
  Latte↔Mocha, Gruvbox light↔dark, Rosé Pine Dawn↔Main), other variants dark-only; Markise Night
  sits with Markise. Settings value `palette:<family id>`. Catalog entries resolve through
  `Theme::native(id)`; no files, no import.

## Decision 7 — sidebar theming and icons

- Model: `Theme::sidebar: SidebarStyle` (font stack, size, weight, text, active text/background,
  hover text/background, current-outline-item text/background, icon tint, row height,
  horizontal padding, indent per level, corner radius); `None` = UI default derived from the
  chrome colors. Markise and palette themes define it in their native mapping.
- Importer: the synthetic DOM gained the spec 03 §3.2 sidebar nodes (tabs incl. active tab,
  `#file-library-tree` folder / file / `.active` file rows with `.file-node-content` /
  `.file-node-title`, `#outline-content` items incl. `.outline-item-active` and a nested
  `.outline-h3`). A value counts as sidebar styling only when it differs from what the node
  inherits from the document, so content fonts do not leak into the sidebar; colors fall back to
  the §2.3 variables (`--control-text-color`, `--active-file-*`, `--item-hover-*`,
  `--outline-active-color`). Indent = half the padding/margin difference between `.outline-h3`
  and `.outline-h1`. `:hover` rules never match (TI-4), so hover comes from the variables.
- UI: rows are painted by `sidebar::SideLook` (full-width active/hover backgrounds, chevrons,
  icons, ellipsis by clipping) with accessibility labels; the sidebar font is its own egui family
  (`markise-sidebar`), resolved like the document stacks.
- Icons: four Lucide icons (ISC; `assets/icons/lucide/` with LICENSE + SOURCE.md, NOTICE),
  embedded as SVG and drawn as strokes by a small path flattener (`ui-egui::icons`), tinted
  with the sidebar icon color; outline rows get subtle level ticks.
