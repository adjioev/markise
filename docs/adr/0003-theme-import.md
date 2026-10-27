# ADR 0003 — CSS theme import: cssparser + own selector engine over a synthetic editor DOM

Status: accepted for the M3 theme importer (2026-10-26). Scope: `crates/markise-theme` (L2,
GUI-free), `apps/markise-cli theme-import`.

## Context

Spec 03 §2 asks the native (egui, no web view) editor to import themes written for the
reference DOM contract into a native theme model: a frequency-ranked subset of CSS properties
and selectors, CSS custom properties with `var()`, `@media (prefers-color-scheme)` light/dark
variants, `@import` of theme-local files, `@font-face`, user layers (`base.user.css`,
`{name}.user.css`), dark detection by the luma rule, and an import report. Real themes are
written for a browser engine: they rely on cascade order, specificity, inheritance and on
invalid selectors dropping whole rules. Input is untrusted (any file in a theme folder), so the
importer must never panic and must bound its work.

## Options considered

1. **lightningcss** (MPL-2.0): full CSS parser, typed property values, minifier, selector
   parsing via `parcel_selectors`.
2. **cssparser** (MPL-2.0, Servo/Firefox tokenizer and rule-list parser, CSS Syntax Level 3)
   plus our own selector parsing/matching, value parsing and cascade.
3. **A hand-written tokenizer.**

## Decision

Option 2: **`cssparser` 0.38** for tokenizing and rule/declaration structure; everything above
the syntax layer is ours (`crates/markise-theme/src/import/`).

Why not lightningcss:
- It types every value it knows and rejects or keeps opaque what it does not; the importer
  needs the *raw* declaration text anyway, because `var()` must be substituted per element
  before a value can be parsed (spec 03 §2.3), and because it reports unparsed values.
- It brings a large dependency tree (parcel_selectors, browserslist data, bitflags, …) for
  features we do not use (minification, transpiling, source maps).
- Its selector matching needs a DOM implementation of `selectors::Element`; a ~200-line matcher
  over our synthetic tree is simpler and lets us decide what "unsupported" means.

Why cssparser:
- Error recovery exactly per CSS Syntax §5 (an invalid rule or declaration is skipped, parsing
  continues), comments/whitespace/escapes/strings/`url()` handled, nesting depth limited
  internally (75 blocks), source locations for the report.
- Small (deps: `phf`, `dtoa-short`, `itoa`, `smallvec`), MPL-2.0 is allowed by AGENTS.md
  (file-level; used unmodified as a crates.io dependency, listed in `NOTICE`).
- Includes the CSS named-color table and hex parsing; `rgb()/hsl()` are parsed by us.

## Design

- **Sheet loading** (`sheet.rs`): built-in base layer (heading size/weight defaults, like a UA
  stylesheet, original) → `{name}.css` with `@import`s inlined at their position → `base.user.css`
  → `{name}.user.css`. `@media` lists are evaluated statically against a nominal viewport
  (1100×780 CSS px, DPR 2); `prefers-color-scheme` (and the legacy `prefers-dark-interface`)
  tags rules Light/Dark; `@media print` blocks are kept verbatim for export; `@supports` is
  assumed true; CSS nesting is flattened (`&` → `:is(parent)`, otherwise descendant).
- **Selectors** (`selector.rs`): full Selectors-4 subset used by themes (type, universal, id,
  class, all attribute operators, 4 combinators, structural pseudo-classes, `:not/:is/:where`,
  `:checked`, state pseudo-classes that never match, `::before/::after/::selection/::marker`).
  Invalid selectors drop the whole rule (browser behaviour themes are tested against);
  valid-but-unsupported ones (`:has()`, `:nth-child(… of S)`) drop only themselves.
- **Synthetic DOM** (`dom.rs`): ~130 nodes mirroring the live-editor DOM of spec 03 §3–§4
  (`html > body > content > #write > h1…h6, p > a/code/…, blockquote > p, ul ul ul, task list
  inputs, pre.md-fences > .CodeMirror … span.cm-*, table, hr, front matter, math, alerts, toc`,
  `#typora-sidebar`), with placeholder first/last children so `:first-child` rules do not hit
  the generic blocks. Selectors are matched with real semantics, so `#write h1`, `h1`,
  `.md-heading`, `content h1` and `.h1` (C5 alias class) all reach the heading.
- **Cascade** (`cascade.rs`): per element, declarations sorted by (`!important`, specificity,
  source order); custom properties inherit and are substituted (`var()` with fallbacks, cycle
  detection, depth 24, 16 KiB value limit, 4096 expansions per value); invalid-at-computed-time
  declarations behave as `unset`; computed values with inheritance and `em/rem/%/pt/vw/vh`.
- **Mapping** (`map.rs`): each native slot reads the computed style of one node; unset slots
  keep the built-in theme (light or dark, chosen by the imported page background) so partial
  themes stay readable. Chrome reads colors only (spec 03 §2.2) from `#typora-sidebar`,
  `::selection` and the §2.3 variables.
- **Report** (`report.rs`): unsupported selectors / properties / values / at-rules with counts
  and `file:line`, deliberately ignored properties, warnings, coverage =
  applied / (applied + unsupported properties + unsupported values) over declarations of rules
  that match a modelled element (custom-property definitions excluded).
- **Import confinement** (TI-9): `@import` targets must resolve (after `canonicalize`, so
  symlinks count) inside the folder of the root theme CSS; `..` escapes, absolute paths and
  `file:` URLs are rejected and reported.
- **Limits** (`ImportOptions`): 2 MiB per file, 8 MiB total, import depth 10, 64 files,
  20 000 rules, 100 000 declarations, 50 000 selectors, selector nesting 8, CSS nesting 8.

## Consequences

- One import of a typical community theme takes ~0.5–2 ms (release; corpus test), a synthetic
  300-rule theme ~0.9 ms and a 6 000-rule one ~17 ms (`cargo bench -p markise-theme`). Import
  runs off the UI thread; no per-frame cost (the result is a plain `Theme`).
- `markise-theme` gains one runtime dependency (cssparser) and stays GUI-free.
- Font loading of `@font-face` records (incl. `unicode-range` fallback chains, C3) and the
  layout use of the new model fields (heading decorations, quote background, token styles,
  list styles, font stacks) are left to the UI/layout layers.
- Results depend on a nominal viewport for width media queries (TI-2) and on the synthetic DOM
  covering what themes target; contextual rules (focus mode, source mode, hover, export-only
  `.typora-export`) are intentionally not applied to the live theme.

## Corpus (C6)

`scripts/fetch-theme-corpus.sh` fetches 22 community themes (18 repositories linked from the
public theme gallery, pinned by commit in `crates/markise-theme/tests/theme-corpus.tsv`) into
the gitignored `.theme-cache/`; they are test input only. On 2026-10-26 all 22 import without
panic or warnings, mean coverage 99.5% (min 96.7%), with 4–25% of matched declarations in the
deliberately ignored set (most frequent: `height`, `position`, `box-shadow`, `vertical-align`,
`overflow`, `word-break`, `box-sizing`). Remaining unsupported items: `transform` (6×),
`calc()` values, `font-weight: medium`, `var()` to an undefined variable, `::-moz-selection`
rules (invalid in Chromium too, so dropped).
