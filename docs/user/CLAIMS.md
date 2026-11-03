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
| "no browser engine inside" | egui/eframe UI (`crates/markise-ui-egui`); no webview crate in `Cargo.lock` (no `wry`/`webview`) |
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
| `shortcuts.md` | `crates/markise-engine/src/editor/command.rs` (`ENGINE_COMMANDS`), `crates/markise-md/src/format.rs` (`MARKDOWN_COMMANDS`), `crates/markise-ui-egui/src/menu.rs` (`APP_COMMANDS`), `keymap.rs` (overrides, reserved chords), `shortcuts.rs` |
| `themes.md` | `crates/markise-theme/src/style.rs`, `catalog.rs`, `palettes.rs`, `discover.rs`; `crates/markise-ui-egui/src/themes.rs`, `prefs.rs`; ADR 0003, 0007 |
| `math-and-diagrams.md` | ADR 0008, 0009; `crates/markise-md/src/options.rs`; `crates/markise-blocks/src/diagram/mod.rs`; `crates/markise-export/src/lib.rs`, `pandoc.rs` |
| `comments-and-suggestions.md` | ADR 0012; `crates/markise-md/src/format.rs`; `crates/markise-ui-egui/src/app/review.rs`; `crates/markise-export/src/lib.rs`, `model.rs` |
| `export.md` | `crates/markise-engine/src/export.rs`; `crates/markise-export/src/pandoc.rs`, `images.rs`, `vars.rs`; `crates/markise-pdf/src/lib.rs`, `image.rs`; `crates/markise-ui-egui/src/export_ui.rs`, `prefs.rs` (empty Export section) |
| `large-files.md` | ADR 0017 (threshold, measurements, behaviour while loading, peak heap); `crates/markise-ui-egui/src/app/progressive.rs`, `find.rs`, `sidebar.rs`, `export_ui.rs` |
| `saving-and-recovery.md` | `crates/markise-engine/src/settings.rs` (defaults), `recovery.rs`, `save.rs`, `watch.rs`; `crates/markise-ui-egui/src/app/workflow.rs` (30 s backup interval, autosave conditions), `app.rs` (conflict banners) |
| `settings.md` | `crates/markise-ui-egui/src/prefs.rs`; `crates/markise-engine/src/settings.rs`; `crates/markise-theme/src/discover.rs` (config folders) |
| `updates-and-privacy.md` | `docs/features/update-check.md`; `crates/markise-engine/src/update.rs`; `apps/markise/src/diag.rs` (log) |
| `faq.md` | the pages above; `apps/markise/src/diag.rs`; `docs/release/0.1.0-beta.1.md` |

## Screenshots

Every image is listed with its scene, command and conversion in `site/assets/SOURCE.md`.
