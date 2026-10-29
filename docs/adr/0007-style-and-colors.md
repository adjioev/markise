# ADR 0007 — Style and colors as independent theme parts

Status: accepted for the `style-split` milestone (2026-10-27). Scope: `crates/markise-theme`
(`style`, `catalog`), `crates/markise-engine` (`settings`), `crates/markise-ui-egui` (`themes`,
Theme menu, "Font & size" window, `theme_shots`). Amends ADR 0004 Decisions 2, 3 and 6.

## Context

Owner decision (2026-10-27): users pick **typography/layout** and **colors** independently —
e.g. a serif book page in Nord colors, or the Drake theme's colors with a compact layout. Until
now a theme was one flat `Theme` value: palette themes (ADR 0004 §6) all shared Markise's
typography, and CSS themes could only be used as a whole.

## Decision 1 — split the model, keep `Theme` as the composed value

`markise_theme::style` adds two plain-data parts:

- **`Style`** — font stacks (body, heading, mono, `@font-face` records), base size, mono scale,
  line height, column width/padding, block spacing; per heading level size, weight, margins,
  rule on/off, italic, alignment, transform, letter spacing, generated prefix/suffix, background
  box on/off with padding/radius/fit, left bar width; link underline; inline-code / code / math /
  front-matter box padding, radius, border on/off; quote bar width, indent, italic, prefix, fill
  on/off; list indent and marker styles; table cell padding; paragraph alignment; sidebar
  fonts, size, weight, row height, padding, indent, radius.
- **`ColorScheme`** — every color: page, text, per heading color / rule / box / bar / prefix,
  link, box colors (background, text, border), quote, list, checkbox, rule, table, `mark`,
  meta, alerts, image placeholder, `SyntaxColors` and imported token styles, chrome, inline
  colors, selection text, sidebar colors; plus `dark` (the light/dark flag) and
  `scheme_aware`.

`Theme` stays the flat struct layout consumes, so `markise-layout` and the importer are
unchanged. `compose(style, colors)` builds it; `Theme::style()` / `Theme::colors()` decompose any
theme. Invariant (tested for Markise, every palette theme and every vendored CSS theme, both
variants, and for boundary values such as a transparent-but-non-zero quote background, which is
kept as is): `compose(&t.style(), &t.colors()) == t`.

When a style asks for a decoration the scheme has no color for, the color comes from the
scheme's roles: rule → `rule`, heading box → `table_header_bg` (raised surface), left bar →
`chrome.accent`, box border → `table_border`, quote fill → code-block background.

When the composition changes the background an element's text sits on, the text color is
re-checked against the effective background and, if needed, replaced by the first legible
scheme color (else the most contrasting one); unchanged parts — and so every complete theme —
keep their colors:

| Change | Effective background | Text candidates (minimum) |
|---|---|---|
| heading box removed (e.g. Lapis' white h2 on a blue pill, style without pills) | page | heading color, opaque box color, body text (3:1) |
| heading box added | derived surface over the page | heading color, body text, page color (3:1) |
| quote fill removed | page | quote text, body text (4.5:1) |
| quote fill added | code-block background over the page | quote text, code text, body text (4.5:1) |

Backgrounds are alpha-composited over the page before contrast is measured (a transparent
surface leaves the page color). Native schemes (`native_colors`) color generated heading prefixes with the
list-marker color.

`Overrides` (body font, base size, line height, content width) apply to the style before
composition: the font is put first in the body stack, heading sizes scale with the base size,
values are clamped (size 9–36, line height 1–3, width 360–2400) and non-finite ones ignored.

Every change of style, colors or overrides that changes the composed `Theme` changes its
fingerprint, which is already part of every layout-cache key (ADR 0004 Decision 4), so the
cache is invalidated by construction. (Changes without effect — e.g. an override clamped to the
value the style already has — need not change it.)

## Decision 2 — built-in styles (original)

| id | Name | Design |
|---|---|---|
| `markise` | Markise Default | the typography of Markise Light/Dark (default; reproduces them exactly) |
| `classic` | Classic | book page: old-style serif stack (Iowan Old Style, Palatino, Book Antiqua, Georgia, Cambria, Liberation/DejaVu Serif, serif), 17 pt, 1.65, 680 pt measure, justified, centred regular-weight h1, ruled h2, italic h3, small-caps-like uppercase h4/h6, italic quotes, underlined links |
| `modern` | Modern | humanist/grotesque sans stack (Avenir Next, Segoe UI, Helvetica Neue, Ubuntu, Cantarell, Noto Sans, Arial), 1.75 line height, tight bold headings, accent bar on h2, filled quotes, soft rounded borderless code boxes, roomier sidebar |
| `technical` | Technical | code notebook: monospace everywhere (JetBrains Mono, SF Mono, Menlo, Cascadia Mono, Consolas, DejaVu/Liberation Mono), `#`…`######` heading prefixes, square boxes with borders, square bullets, 860 pt column |
| `compact` | Compact | dense reference reading: 14 pt sans, 1.42 line height, small margins, 980 pt column, tight tables and sidebar |

Stacks name widely installed system fonts with a generic fallback; nothing is bundled. The
font resolver uses the first three installed families (ADR 0004 Decision 5).

## Decision 3 — selection: style, colors, complete themes

`catalog::Selection { style: StyleId, colors: ThemeId }`:

- `StyleId::Builtin(id)` or `StyleId::Theme(family)` — the style of a CSS theme family
  (built-in or user). Native families (Markise, palettes) have no style of their own; their
  "style" is the default.
- `colors` is any theme family of the catalog (Markise, Markise Night, palettes, CSS themes);
  the family's light/dark variant is picked by the appearance setting as before, for the style
  source as well.
- A **complete theme** is `Selection::complete(t) = (StyleId::of_theme(t), t)`. Its name is the
  theme's own (e.g. "Drake Light"); a mixed selection is named "colors · style" ("Nord · Drake"),
  or just the colors when the style is the default.

Imports are shared: Drake's style and Drake's colors come from the same cached import; a change
to its files invalidates both (ADR 0004 live reload).

## Decision 4 — settings version 2

```json
{ "version": 2, "style": "style:classic", "colors": "palette:nord", "appearance": "system",
  "overrides": { "font_family": "Georgia", "font_size": 17, "line_height": 1.7, "content_width": 860 } }
```

`style`: `style:<id>` | `theme:<theme id>`; `colors`: the ADR 0004 theme id; `overrides`
omitted when empty, invalid entries ignored. `markise-engine` stays independent of
`markise-theme` (strings and numbers only).

- **Version-aware migration.** In a version-1 file (or one without `version`) `theme` is
  authoritative and migrates to a complete theme (`builtin:`/`user:` → `style: "theme:<id>"`;
  `markise`/`palette:` → default style); `style`/`colors` found there are stale leftovers of a
  newer build and are ignored. From version 2 on `style`/`colors` are authoritative.
- **Downgrade policy.** Version 2 also writes `theme` = the `colors` family, so an older build
  shows those colors as a complete theme (falling back to Markise for families it does not
  know). When it changes the theme it writes `version: 1`; the next newer build migrates that.
- **Newer versions** are read best-effort and never written: saves fail and the UI reports it
  once, so a newer build's file is not rewritten as version 2.
- **Unknown keys** are preserved, top-level and inside `overrides`.
- **Writes** are a read–merge–write of only the changed fields (`SettingsPatch`; override
  members one by one, "unchanged" distinct from "cleared") under the advisory lock
  `.settings.lock`, so two instances do not overwrite each other's unrelated changes. The UI
  coalesces changes and writes 300 ms after the last one (the preview applies at once).
- **Shutdown** hands pending changes to the worker and waits (up to 3 s) until every queued
  save is answered. Changes not written then — worker busy or blocked, a retryable write
  error — are written directly or, if that is not possible, recorded in a per-instance file
  `settings.pending.<pid>-<random>.json` (published without replacing anything) and applied at
  the next start under the settings lock, oldest first, removing only the files applied
  (logged to stderr). Every settings write also consumes existing recovery files first (they
  are older than it), so a recovered value never overwrites a later save. The names of the
  consumed files are stored in `settings.json` (`consumed_pending`) in the same atomic write as
  the merged values; listed files are skipped and deleted later (failures logged, retried), so a
  crash between the write and the deletion cannot replay them. Names of files that are gone
  drop out of the list on the next write. A successful save drops the same fields from older failed ones, so a
  stale value is never re-applied. Saves refused because the file is from a newer version are
  not retried.
- **Startup.** Style, colors, appearance and overrides chosen before the file was read each
  replace only their own part of the file (a complete theme counts as choosing both).

## Decision 5 — UI

Theme menu: **Style ▸** (built-in styles, then "From themes": Drake, Lapis, Phycat, user
themes), **Colors ▸** (Markise, Markise Night; Palettes with their light/dark pairs; "From
themes"), **Complete themes** (Markise, built-in and user CSS themes; selecting one sets both
parts), the Appearance radios (Follow System / Light / Dark — they pick the variant of the
colors *and* of a theme-derived style), **Font & Size…** (a window with body font, base size,
line height, content width; each override has an on/off box; applied live and persisted;
"Use the style's values" clears them), then Import / Open Theme Folder / Theme Report as before.
The theme report shows the import of the colors' CSS theme, else of the style's.

**Amended (issue #50, Preferences window).** The top-level Theme menu is gone. Style, colors,
complete theme, the Appearance radios, the Font & size controls and Import / Open Theme Folder /
Theme Report live in Preferences → Appearance; View → Theme ▸ keeps a quick switcher between the
complete themes (radio items) and ends with "Theme Settings…", which opens that section. The
"Font & size" window and every former menu command still exist and run by id.

## Consequences

- Every built-in style × native scheme combination passes WCAG AA for body text, links, inline
  code, code blocks and quotes, and 3:1 for headings on their background (test); CSS styles ×
  native schemes and built-in styles × CSS schemes keep headings ≥ 3:1 (test).
- Palette themes now use Markise Default's code-box radius (5 instead of 6) — the palette pack
  is a color pack.
- Headless grid screenshots: `MARKISE_SHOTS=grid cargo run -p markise-ui-egui --release
  --example theme_shots` → `.screenshots/grid-<style>-<colors>.png` (4 styles × 3 schemes, plus
  Lapis' style in Tokyo Night and Modern in Lapis colors).
- Open points: TH-12…TH-15 in `docs/spec/99-open-questions.md`.
