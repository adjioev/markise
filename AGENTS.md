# Markise — instructions for implementers (humans and agents)

Markise is a cross-platform, live-preview (WYSIWYG) Markdown editor in pure Rust.
The functional specification is maintained in a separate private repository
(`adjioev/markise-spec`; maintainers and project agents keep a local checkout next to this one at
`../markise-spec/`, start with its `README.md`). Public contributors work from issues, ADRs in
`docs/adr/` and the public standards below.

## Clean-room rules (mandatory)

This project is a clean-room implementation. You are on the **clean** side.

- Your sources about required behavior are the Markise specification, public standards (CommonMark, GitHub
  Flavored Markdown spec, CSS specs, Unicode) and the vendor's **public documentation**:
  support.typora.io and the theme documentation at theme.typora.io (e.g. "Write Custom Theme",
  "Install Theme"). Reading these is allowed; do not copy their text into code or docs.
- **Never** download, install, run, decompile, or read the files of the reference product
  (Typora) — no binaries, bundles, JavaScript, the CSS themes shipped with it (github, night,
  newsprint, pixyll, gothic, whitey), or decompiled output. Do not search the web for its source
  or internals. Do not ask other agents for them.
- Community themes (e.g. listed on theme.typora.io) are used **only** as test input for the theme
  importer (spec 03 §2), never as a source of code. Fetch them at test
  time (gitignored cache) rather than committing them; commit a theme only if its own license is
  MIT/BSD/Apache-2.0/CC0, together with that license and a `NOTICE` entry. Never copy GPL
  theme CSS into the repository or into built-in themes.
- Built-in Markise themes are original work (or permissively licensed themes with attribution).
- If the spec is unclear or missing something, do not guess from memory of the reference
  product: add the question to the specification's open questions (`99-open-questions.md`) or a GitHub issue (section A/B for the spec team,
  C if it is a design decision you may make) and continue with a clearly marked placeholder.
- Third-party crates: permissive licenses only (MIT, Apache-2.0, BSD, Zlib, ISC, Unicode-3.0 (code and data; owner-approved 2026-10-27 for ICU4X via parley), MPL-2.0 file-level; for bundled syntax-highlighting grammars only, also the Sublime HQ license, public-domain dedications, Unlicense and WTFPL — owner-approved 2026-10-27; SIL Open Font License 1.1 (OFL-1.1) for bundled font files only, never code — owner-approved 2026-10-27; Apache-2.0 WITH LLVM-exception (owner-approved 2026-10-28; e.g. winx via cap-std)).
  Record every vendored/forked crate in `NOTICE`.

## Architecture

Layered workspace (`crates/*`, `apps/*`), lower layers never depend on higher ones:

| Layer | Crate | Job |
|---|---|---|
| L0 | `markise-geom` | units, measurements |
| L1 | `markise-doc` | format-neutral document seam (ADR 0006): spans, patches, positions, selections, hints, views, command descriptors, format traits |
| L1 | `markise-md` | Markdown parse/serialize, lossless round-trip (spec 01); implements the `markise-doc` traits |
| L2 | `markise-layout` | shaping, line breaking, block layout, tables, hit testing |
| L2 | `markise-theme` | native theme model + CSS-subset importer (spec 03 §2) |
| L2 | `markise-blocks` | code highlighting, math, diagrams, images |
| L2 | `markise-text` | GUI-free text shaping, font matching, paragraph layout (ADR 0005) |
| L3 | `markise-render`, `markise-export`, `markise-pdf` | rasterising; HTML (spec 03 DOM contract), export model, Pandoc bridge; paginated PDF/PNG export |
| L4 | `markise-engine` | session, undo, commands, files, autosave, search, file watching |
| L5 | `markise-mcp` | MCP server |
| L6 | `markise-ui-egui` | egui front end |
| apps | `markise`, `markise-cli`, `markise-web-render` | desktop app, command line; block-snapshot helper (prototype branch only) |
| tests | `markise-corpus` | deterministic corpus for tests and benchmarks (dev-dependency only) |
| tests | `markise-testfmt` | tiny line-based toy format proving the format seam (dev-dependency only) |

The table is enforced by `cargo xtask layers` (CI): its machine-readable copy is `layers.toml`,
which must list the same crates at the same layers. Within one layer a crate may only use an
earlier crate of an `[intra_layer]` chain there (today `markise-doc` → `markise-md` and
`markise-text` → `markise-theme` → `markise-blocks` → `markise-layout`, and `markise-export` →
`markise-pdf`); apps sit on top and nothing depends on them; UI toolkits (egui, eframe, winit, rfd, muda) are allowed from L6 only;
format-neutral crates (`[format_seam]`, today `markise-layout`) read documents only through
`markise-doc` and may use a format crate (`markise-md`) only as a dev-dependency.
Throwaway `spikes/` are not workspace members and are
not checked. Every committed asset (`assets/`, `site/assets/`, images, fonts, test fixtures)
needs an attribution record — a `SOURCE.md` naming the file in its folder or a parent folder,
or a `NOTICE` entry naming the file or its folder — enforced by `cargo xtask assets`.

Reference architecture: `storytold/wordcraft` (pure Rust, egui). Its crates are MIT/Apache-2.0
and may be reused with attribution; prefer vendoring selected crates over depending on its
workspace.

## Performance

Performance budgets are requirements, not nice-to-haves: see "Performance requirements" in
the specification's README (NFR-1…NFR-10). In short: start < 250 ms, 10 MB file opens < 1 s,
keystroke latency p99 < 16 ms, idle CPU ~0%.

- Design for incremental parse/layout and viewport virtualisation from M0; never parse,
  shape or lay out the whole document per keystroke or per frame.
- Keep the UI thread free of I/O, full parses, search, export and heavy rendering.
- Every milestone adds benchmarks to `benches/` for the features it introduces; report
  numbers against the budgets in the milestone's final commit.

## Milestones

- **M0 — core round-trip:** workspace skeleton, `markise-md` parsing and serializing every
  construct in spec 01; all spec 01 test cases as unit tests; byte-identical round-trip of
  unchanged documents; parse+serialize of the 10 MB corpus benchmarked. CLI: `markise-cli roundtrip <file>` and `markise-cli html <file>`.
- **M1 — viewer:** egui window rendering a document with the default theme, scroll, outline,
  file tree sidebar, open/save, source-code mode; virtualised rendering meeting NFR-1/2/3/5/6/7/10.
- **M2 — editor (P0):** live-preview editing per spec 02 for P0 block/inline types, undo/redo,
  autosave/recovery, HTML export; incremental layout meeting NFR-4/8.
  Before building the editor on the block model, audit it for "format as plugin": the layout
  model, caches, caret/selection model and editing commands must not depend on Markdown-specific
  types, so another source format (e.g. Org mode via a `markise-org` parser producing the same
  block model) can be added later. Markdown-specific behaviour (input rules, serialization) lives
  behind a format trait. Record the outcome in an ADR.
- **M3 — P1 features** from spec 04, theme importer, MCP server. Math (TeX) rendering (ADR 0008) and diagram rendering (ADR 0009) are merged; diagrams are frozen except for bug fixes.
- **Later (P2):** Org mode viewing (`.org` → same block model; evaluate the `orgize` crate's
  lossless tree), then optional Org editing as a normal editor. Emacs-specific features (agenda,
  babel/code execution, clocking, export pipelines) are out of scope.

## Working rules

- Checks before every commit: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo test --workspace`, `cargo xtask layers`, `cargo xtask assets` (or all of them:
  `cargo xtask ci`).
- Spec test cases become tests; reference the spec ID in the test name or comment (e.g. `T26`).
- Work on task branches; commit honest checkpoints (`WIP:` when unfinished, list checks run and
  known failures). Merges to `main`, releases and pushes to new remotes need owner approval.
- Never commit secrets, build artifacts, or files from the reference product.
