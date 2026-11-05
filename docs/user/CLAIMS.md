# Claims and sources (maintainer note, not published)

Every number or claim on the website (`site/index.html`) and in the user guide (`docs/user/`)
must be checkable in this repository. This note lists where each one comes from. Update it
when a claim changes; `cargo xtask site` skips this file.

## Front page

| Claim | Source |
|---|---|
| Rendered, Source Code Mode, Side by Side editing; formatting shortcuts; Markdown typed by hand becomes formatting | `assets/tutorial/welcome.md` §1–6; `crates/markise-md/src/format.rs` (`MARKDOWN_COMMANDS`); `crates/markise-md/src/edit/rules.rs` (input rules) |
| macOS, Windows, Linux builds | `docs/release/README.md`, `xtask/src/dist.rs` |
| Linux AppImage and `.deb` (from the release after 0.1.0-beta.1), `.deb` dependencies, AppImage FUSE fallback | `scripts/package.sh` (`linux-*`), `.github/workflows/release.yml` (`linux-smoke`), `packaging/linux/` |
| "written in Rust" (not "pure Rust": TLS uses `ring`, which compiles C/assembly) | `docs/features/update-check.md` ("Dependencies") |
| "no browser engine inside" | egui/eframe UI (`crates/markise-ui-egui`); no engine is bundled (no `wry`/`webview` in `Cargo.lock`). Since ADR 0023, on macOS only, the separate helper `apps/markise-web-helper` uses the system's WebKit (`objc2-web-kit`) for accepted packages; the app's window has none. To review with the owner: the phrase still holds of what ships inside the app, but not of everything it can start |
| Unchanged files are written back byte for byte | `docs/adr/0001-markdown-core.md` (byte-identical round-trip, tested over a corpus); `markise-cli roundtrip` |
| Files of 4 MiB or more open in two steps | `docs/adr/0017-progressive-open.md` ("Two steps for large files"); `crates/markise-engine/src/load.rs` |
| 41–52 ms first painted frame, 311–813 ms fully loaded, 1.2–2.4 ms slowest frame (100 MiB) | `docs/adr/0017-progressive-open.md` "Measurements": headless, Apple M-series, release build, 2026-11-03, `markise-corpus --huge` mixed/prose/log; excludes window/GPU/compositor latency |
| Vault: no import, `[[wikilinks]]` with Cmd/Ctrl-click, Back/Forward, `[[` autocomplete, create missing note | `crates/markise-engine/src/vault.rs`; `crates/markise-ui-egui/src/app/vault.rs`; `crates/markise-ui-egui/src/viewer.rs` (modifier click) |
| `.obsidian/` read (only `app.json`, a few keys), never written | `crates/markise-engine/src/vault.rs` (doc comment "read only, never written"; hidden path components refused for created notes); `crates/markise-engine/src/attach.rs` |
| Background indexing that follows folder changes | `crates/markise-engine/src/vault.rs` module docs (scanner + watcher events, memory only) |
| TeX math; Mermaid flowcharts and sequence diagrams only | `docs/adr/0008-math-rendering.md`, `docs/adr/0009-diagrams.md`; `crates/markise-blocks/src/diagram/mod.rs` |
| Comments and suggestions (CriticMarkup), accept/reject, Save Copy Without Comments | `docs/adr/0012-review-comments-and-suggestions.md`; `crates/markise-ui-egui/src/app/review.rs` |
| Export HTML, PDF, PNG; Pandoc formats need Pandoc ≥ 2.10 | `crates/markise-engine/src/export.rs`; `crates/markise-export/src/pandoc.rs` |
| PDF/PNG show math and diagrams as source | `crates/markise-pdf/src/lib.rs` (module docs), `crates/markise-pdf/src/layout.rs` |
| Styles × colour schemes, follow system appearance, CSS import of a supported subset, Theme Report | `docs/adr/0003-theme-import.md`, `docs/adr/0007-style-and-colors.md`; `crates/markise-ui-egui/src/themes.rs`; `crates/markise-theme/src/style.rs` (five styles), `palettes.rs` |
| No telemetry; automatic update checks opt-in; manual check available; GitHub sees IP and app version; beta checks two endpoints; no automatic installation | `docs/features/update-check.md`; `crates/markise-engine/src/update.rs` (`urls`, `user_agent`, `RELEASES_URL`, `LATEST_URL`); `apps/markise/src/update_client.rs` (the only HTTP client; `ureq` is a dependency of the app crate only) |
| Markise's only own network requests are update checks; links open in the browser and Pandoc exports may fetch remote images (user-initiated, other programs) | `apps/markise/src/update_client.rs` (only HTTP client); `crates/markise-export/src/pandoc.rs` (`ExInline::Image`: remote URLs stay as written, Pandoc may fetch them) |
| Remote images are not downloaded by Markise (display, HTML/PDF/PNG export) | `crates/markise-blocks/src/lib.rs` (`decode` reads local files; test with an `https://` path); `crates/markise-pdf/src/image.rs`; `crates/markise-export/src/lib.rs` (`ImageLinks`: remote URLs never fetched) |
| System requirements (macOS 11, Windows 10/11, glibc 2.35) | `scripts/package.sh` (`LSMinimumSystemVersion` 11.0); `.github/workflows/release.yml` (Linux built on ubuntu-22.04); `docs/release/0.1.0-beta.1.md`. Re-check against the published artifacts. |
| Not code-signed / not notarized | `scripts/package.sh` (ad-hoc `codesign --sign -`); `docs/release/README.md` "Not done yet" |
| Download placeholder (no release published yet) | `gh release list` was empty on 2026-11-03 |

Removed from the old page because they were not measured or not countable: "huge documents
that open instantly", "starts in a blink and sits at zero CPU", "more than 25 themes",
"pure Rust from parser to pixels", "no network calls" (update checks exist), "about a tenth of
a second" for 10 MB.

## User guide

| Page | Main sources |
|---|---|
| `getting-started.md` | `docs/release/0.1.0-beta.1.md`, `docs/release/README.md`, `scripts/package.sh`; Apple's "Open Anyway" procedure: https://support.apple.com/en-au/102445; first-launch card: `docs/features/update-check.md` |
| `editing.md` | `assets/tutorial/welcome.md`; `crates/markise-md/src/format.rs`; `crates/markise-engine/src/editor/command.rs`; `crates/markise-ui-egui/src/app/find.rs`; `crates/markise-engine/src/attach.rs`; `crates/markise-ui-egui/src/viewer.rs` (copy = Markdown source, paste = plain text) |
| `folders-and-vaults.md` | `crates/markise-engine/src/vault.rs`, `tree.rs`, `quick_open.rs`, `fsops.rs`; `crates/markise-ui-egui/src/app/vault.rs`, `sidebar.rs`, `confirm.rs`; `crates/markise-md/src/wikilink.rs` |
| `shortcuts.md` | `crates/markise-engine/src/editor/command.rs` (`ENGINE_COMMANDS`), `crates/markise-md/src/format.rs` (`MARKDOWN_COMMANDS`), `crates/markise-ui-egui/src/menu.rs` (`APP_COMMANDS`), `keymap.rs` (overrides, reserved chords), `shortcuts.rs`; inside a block: ADR 0021 "Keys", `crates/markise-engine/src/editor/inplace.rs`; library: `crates/markise-ui-egui/src/block_library.rs`; form: `crates/markise-ui-egui/src/block_form.rs` (`SAVE_KEY`), ADR 0020 "The panel" and "Keyboard"; picker: `crates/markise-ui-egui/src/image_picker.rs` |
| `themes.md` | `crates/markise-theme/src/style.rs`, `catalog.rs`, `palettes.rs`, `discover.rs`; `crates/markise-ui-egui/src/themes.rs`, `prefs.rs`; ADR 0003, 0007 |
| `math-and-diagrams.md` | ADR 0008, 0009; `crates/markise-md/src/options.rs`; `crates/markise-blocks/src/diagram/mod.rs`; `crates/markise-export/src/lib.rs`, `pandoc.rs` |
| `comments-and-suggestions.md` | ADR 0012; `crates/markise-md/src/format.rs`; `crates/markise-ui-egui/src/app/review.rs`; `crates/markise-export/src/lib.rs`, `model.rs` |
| `export.md` | `crates/markise-engine/src/export.rs`; `crates/markise-export/src/pandoc.rs`, `images.rs`, `vars.rs`; `crates/markise-pdf/src/lib.rs`, `image.rs`; `crates/markise-ui-egui/src/export_ui.rs`, `prefs.rs` (empty Export section) |
| `large-files.md` | ADR 0017 (threshold, measurements, behaviour while loading, peak heap); `crates/markise-ui-egui/src/app/progressive.rs`, `find.rs`, `sidebar.rs`, `export_ui.rs` |
| `saving-and-recovery.md` | `crates/markise-engine/src/settings.rs` (defaults), `recovery.rs`, `save.rs`, `watch.rs`; `crates/markise-ui-egui/src/app/workflow.rs` (30 s backup interval, autosave conditions), `app.rs` (conflict banners) |
| `settings.md` | `crates/markise-ui-egui/src/prefs.rs`; `crates/markise-engine/src/settings.rs`; `crates/markise-theme/src/discover.rs` (config folders) |
| `updates-and-privacy.md` | `docs/features/update-check.md`; `crates/markise-engine/src/update.rs`; `apps/markise/src/diag.rs` (log); a package's code: ADR 0023 "Isolation", `apps/markise-web-helper/src/mac/self_test.rs` |
| `blocks.md` | see [Blocks](#blocks) below |
| `block-packages.md` | see [Block packages](#block-packages) below |
| `faq.md` | the pages above; `apps/markise/src/diag.rs`; `docs/release/0.1.0-beta.1.md` |

## Blocks

Claims of `blocks.md`. Behaviour in a real window is on the acceptance issue "Acceptance: site blocks": the tests
below run headless.

| Claim | Source |
|---|---|
| A block is `:::name` lines in plain Markdown; Enter writes the closer; a simple card with no package | ADR 0018 ("Syntax", "Editing", "On screen"); `crates/markise-md/src/directive.rs` |
| `blocks.config.json`, nearest wins, up to the home folder, no network | ADR 0019 "Binding"; `crates/markise-package/src/binding.rs`, `tests/it/binding.rs` |
| The card's wording: Use Package, Not Now, what it names; the status bar's texts and menus | `crates/markise-ui-egui/src/package_ui.rs` |
| Unpacked copy, works without the archive, one question per hash, a changed file asks again, Stop Using removes copy and record | ADR 0019 "Store", "Trust"; `crates/markise-package/src/store.rs`, `resolve.rs`, `tests/it/resolve.rs` |
| A hash is not a publisher | ADR 0019 "Trust" |
| A changed binding counts from the next opening | ADR 0019 ("When it is looked up") |
| `/` and Insert Block…: where they open, a slash otherwise, `//` and Return with no match | ADR 0022 "`/`"; `crates/markise-ui-egui/src/block_library.rs`; `crates/markise-ui-egui/tests/it/block_library.rs` |
| Where a new block goes, one undo step | ADR 0022 "Inserting"; `crates/markise-engine/tests/it/block_insert.rs` |
| The three views and the button's words | ADR 0023 "The switch"; `crates/markise-layout/src/native.rs` (`BlockViewMode`); `crates/markise-ui-egui/src/viewer/block_tools.rs` (`block_view_button`) |
| The state is not in the file, lasts the session, a document opens native | ADR 0021 "The switch" |
| Editing in place: fields, arrow keys, Tab, Return, marks, typed markup stays text, read-only parts | ADR 0021 "Editing in place"; `crates/markise-engine/src/editor/inplace.rs`; `crates/markise-engine/tests/it/block_inplace.rs` |
| The form: Save, Cancel, keys, not written to the file, the question before losing a draft | ADR 0020 "The panel"; `crates/markise-ui-egui/src/block_form.rs`; `crates/markise-ui-egui/tests/it/block_form.rs` |
| Moving: six dots, line, Esc, scroll at edges, stays in its parent; commands | ADR 0022 "Moving"; `crates/markise-ui-egui/src/viewer/block_move.rs`; `crates/markise-engine/tests/it/block_move.rs` |
| Delete: bin, command, no question, undo | ADR 0022 "Deleting"; `crates/markise-ui-egui/src/menu.rs` (`block.delete`) |
| Pictures in native views belong to the form; Replace…, Edit… elsewhere; Choose file opens the picker | `crates/markise-ui-egui/src/viewer/code_tools.rs` (`native_item`, buttons); `crates/markise-ui-egui/src/app/image_edit.rs`; ADR 0022 "The image picker" |
| 400 points, zoom scales it, proportions kept, a site's picture not capped, double click shows it whole | `crates/markise-layout/src/block.rs` (`IMAGE_MAX_HEIGHT`, `layout_image`); `crates/markise-layout/tests/it/view.rs`, `crates/markise-ui-egui/tests/it/full_view.rs` |
| ⚠, Not filled in yet, Outside its schema, stays as it was under the caret, editable and savable | ADR 0021 "A block outside its schema", "A block being edited stays native"; `crates/markise-ui-egui/src/viewer/block_tools.rs` |
| A failed picture: a card, a quiet reason, no ⚠ | ADR 0023 "Showing a picture"; `crates/markise-ui-egui/src/viewer/block_tools.rs` ("Not drawn by its site's code") |
| Site pictures on macOS only; none on Windows and Linux | ADR 0023 "Distribution"; `apps/markise-web-helper/src/main.rs` (`available: false` elsewhere); `crates/markise-ui-egui/src/viewer/site_board.rs` (`system_web_helpers`) |
| macOS 14 or later | `apps/markise-web-helper/src/mac/network.rs` (`usable`: the proxy functions came with macOS 14) |
| Not interactive | ADR 0023 ("A click and a submit stop at the window"); `apps/markise-web-helper/tests/it/hostile.rs` |
| A replaced file does not redraw a picture until the block changes | `crates/markise-ui-egui/src/viewer/site_board.rs` (the key: source, width, scale, theme, session, language; no file's contents); ADR 0023 "The cache" |
| Exports show a plain frame with the name | ADR 0018 "On screen and in export"; `crates/markise-export/src/lib.rs`, `crates/markise-pdf/src/layout.rs` |
| Code kept apart: accepted packages only, no network, only the package's files and the block's images | ADR 0023 "Isolation"; `crates/markise-engine/src/block_page.rs` (`file`); `apps/markise-web-helper/tests/it/hostile.rs`, `src/mac/self_test.rs` |

## Block packages

Claims of `block-packages.md`.

| Claim | Source |
|---|---|
| The small package and document printed on the page, and its `tar` command | `apps/markise-web-helper/tests/it/example_package.rs` reads them from the page, validates them, and draws the block on macOS |
| Layout of the files, manifest, schema, palette, views, validation, all limits | ADR 0019 "Format", "Validation"; `crates/markise-package/src/limits.rs`, `index.rs`, `schema.rs`, `palette.rs`, `views.rs` |
| Fields: Markdown, limits, `correct`, `ids`, refused keys | ADR 0020 "Fields", "From a package to a form"; `crates/markise-package/src/form.rs` |
| Variants: arguments naming none are outside the schema | `crates/markise-engine/src/block_view.rs` (`variant_of`) |
| Presets: parse as one block, ids kept or renumbered; icon choices | ADR 0022 "Inserting"; ADR 0020 "Icon choices" |
| View kinds, accent names, label, 64 KiB | ADR 0021 "Look", "Cost"; `crates/markise-layout/src/native.rs` |
| Icons and refused SVG | ADR 0019, ADR 0020 "Icons"; `crates/markise-ui-egui/src/block_form.rs` (`refuse_in_source`, `rasterize_svg`) |
| The render contract: styles, script, markup, page, variables | ADR 0023 "How a package draws a block", "What the page sees of the theme"; `crates/markise-engine/src/block_page.rs`; `crates/markise-engine/tests/it/block_page.rs` |
| What a page can reach, bounds | ADR 0023 "Isolation"; `apps/markise-web-helper/src/mac/render.rs` (`FILES_LIMIT`), `network.rs`; `apps/markise-web-helper/tests/it/hostile.rs` |
| What a picture is keyed on | ADR 0023 "The cache"; `crates/markise-ui-egui/src/viewer/site_board.rs` |
| Binding and archive rules | ADR 0019 "Binding", "Archive"; `crates/markise-package/src/binding.rs`, `archive.rs` |
| The example package passes validation and draws | `apps/markise-web-helper/tests/it/example_package.rs`; `xtask/src/block_package.rs` |

## Screenshots

Every image is listed with its scene, command and conversion in `site/assets/SOURCE.md`.
