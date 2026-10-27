# Research: reusing WordCraft and craft-libs components in Markise

Status: research note, WIP (2026-10-27; revised after review the same day). No production code was changed. Author: research agent
on branch `research-wordcraft`. Docs only.

**Sources read** (shallow clones, read-only, outside the repository):

| Repository | Revision | Commit date | License |
|---|---|---|---|
| `storytold/wordcraft` | `6a2b520fa49aa0d0595cea36589aaf16ca371cd3` | 2026-10-26 | MIT OR Apache-2.0 (+ bundled data, see §L) |
| `storytold/craft-libs` | `9ff719499d8f4730b93071abed993e5f12fe1be8` | 2026-10-25 | MIT OR Apache-2.0 (+ bundled data, see §L) |
| `storytold/egui` fork, `PATCHES.md` only | `6d518fc799681571963842f95b55a6b68431b15a` | — | upstream egui MIT OR Apache-2.0 |

WordCraft itself pins craft-libs at `b926b5c03e256c86ca10934f58d03f628810b5b7` (not the head
read here); any pin we take must be re-checked against the exact revision.

**Ground rules applied.** Clean-room rules (AGENTS.md) are not affected: WordCraft is an
independent open-source Word-like processor, not the reference product, and AGENTS.md names it
as the reference architecture. Text shaping is not re-opened here: ADR 0005 (on `main` as a
**proposal**, status "proposed", with feasibility gates still open; it supersedes ADR 0002's
"revisit before M2" item only once accepted) proposes parley + fontique, not `wordcraft-fonts`.
Recommendations below that depend on parley (caret/hit testing §4, IME §5, accessibility §12,
PDF glyph input §13) are **assumptions based on that proposal** and must be revisited if ADR 0005
is changed or rejected. Rust MSRV is
**not** a constraint (owner, 2026-10-27: the toolchain moves to 1.99.x; WordCraft's 1.95 is
acceptable). The egui *version* is still a constraint: Markise was on egui 0.33.3 when this note
was written and moved to stock egui 0.37.0 on 2026-10-27 (toolchain bump; the fork is **not**
adopted); WordCraft and every egui-facing craft crate are on egui **0.37 plus the
`storytold/egui` fork**.

**Verification legend.** "verified" = read in the source at the revision above; "unverified" =
taken from docs/README or inferred, not checked in code; nothing was compiled or run (resolving
WordCraft's workspace needs the egui/winit/krilla git forks; see next steps).

---

## Summary

| # | Area | Source | Verdict | Effort | Milestone |
|---|---|---|---|---|---|
| 1 | Layering check (`cargo xtask layers`, asset attribution check) | `wordcraft/xtask/src/layers.rs`, `assets.rs` | **REUSE — vendor with NOTICE** (adapt table to `markise-*`) | 0.5–1 d | now (before M2) |
| 2 | Command architecture (`CommandSpec` registry, ids, `enabled`, `mutates`, JSON params; one `Session::run` for menus/shortcuts/CLI/MCP) | `wordcraft/crates/engine/src/lib.rs` | **ADAPT** (port design, own code) | 3–5 d | M2 |
| 3 | Undo/redo semantics (typing group, `join_next_undo`, `merge_undo_since`, labels, selection restore, undo list) | `engine/src/session.rs` | **ADAPT design only**; storage must be patch-based, not snapshot | 3–5 d | M2 |
| 4 | Caret/selection/hit testing/keyboard navigation | `layout/src/hit.rs`, `engine/src/cmd/caret.rs`, `doc/src/para.rs`, `doc/src/cluster.rs` | **ADAPT** (reference for goal-x, visual step, select word/sentence/para); **REUSE `cluster.rs`** only if parley clusters prove insufficient | 1–2 w (with parley) | M2 |
| 5 | IME (surrounding text, delete-surrounding, preedit range, composition = undo boundary) | `doc/src/ime_text.rs`, `ui-egui/src/keys.rs`, egui fork E6/E21/P7b | **ADAPT** app side; egui side: track upstream / cherry-pick fork patches, **do not** depend on the fork wholesale | 1–2 w | M2 |
| 6 | System clipboard (HTML+text write, HTML/RTF/image read, Wayland data-control, partial size limits) | `apps/wordcraft/src/clipboard.rs` + its `platform` modules, `ui-egui/src/paste_picture.rs`, `geom::picture` probes | **REUSE — vendor (adapt) with NOTICE**; add bounded reads where it only rejects after reading | 2–3 d | M2 (P0 copy/paste), M3 (HTML→MD) |
| 7 | HTML import for smart paste (tag-soup tokenizer, implicit closes, caps) | `crates/formats/src/html.rs` | **ADAPT** (tokenizer as reference; output is Word model) | 3–5 d | M3 (P1) |
| 8 | Find & replace | `engine/src/cmd/edit.rs`, `find_pattern.rs` | **ADAPT** small parts (regex-only engine, last-first replace-all as one undo step); Word wildcards **SKIP** | 2–3 d | M2 (P0) |
| 9 | Spelling (affix/Hunspell reader, compact word list, suggestions) | `crates/proof` | **REUSE candidate — vendor `affix.rs`, `hunspell.rs`, `dict.rs`** after a quality spike; grammar rules **SKIP** | 2–4 d spike + 3 d integration | M3 (P1, non-macOS) |
| 10 | Hyphenation | `proof/hyphen.rs`, `craft-text` | **SKIP for now** (no spec requirement; revisit if themes need `hyphens: auto`) | — | later |
| 11 | Font picker, style galleries | `ui-egui/src/previews.rs`, `craft-font-manager` | **SKIP** (Markise has theme fonts, no font menu); keep "render preview off-thread, cache texture" idea | — | — |
| 12 | Accessibility tree for custom-painted text | `ui-egui/src/a11y.rs` | **ADAPT** (reference design for ADR 0005 R17) | 1 w | M2/M3 |
| 13 | PDF export | `crates/pdf`, `craft-pdf`, patched Krilla 0.8.2 | **ADAPT**: use upstream `krilla` from crates.io directly; port the export structure (text-as-text, links, outline, tagging, bad-font fallback) | 1–2 w | M3 (PDF is P1) |
| 14 | MCP server + CLI scripting | `crates/mcp`, `apps/wordcraft-cli`, `control-key` | **ADAPT** (small, std-only JSON-RPC; tools map onto command registry) | 3–5 d | M3 |
| 15 | Autosave / recovery | `engine/src/recovery*`, `docs/recovery.md` | **ADAPT design** (single worker, revision-gated ≤10 s snapshots, event table) | 3–4 d | M2 |
| 16 | Atomic save | `craft-atomic` | **ADAPT 2 details** into our `save.rs` (macOS `F_FULLFSYNC`, Windows rename retry); keep ours | 0.5 d | M2 |
| 17 | Linked-path policy filter (refuses remote hosts/UNC to other servers/device paths/non-file URLs, lexically) | `craft-docpath` | **REUSE — git dep pinned rev or vendor** (no deps) as an *extra* filter; not a confinement boundary | 0.5–1 d | M2/M3 (images) |
| 18 | Non-blocking file dialogs, window geometry, Linux dark-mode portal, scroll/momentum | `ui-egui/src/{file_dialogs,window_geometry,desktop_theme,scroll}.rs` | **ADAPT** (patterns; our UI is different) | 1–3 d each | M2/M3 |
| 19a | Keyboard bindings (chords, canonical text, overrides, conflicts, reserved OS chords, shortcut-set import) | `craft-keymap` | **REUSE candidate**: core (serde/serde_json only) as git dep or vendor; optional `egui` integration works on crates.io egui 0.37 with fallbacks; skip the `ui` dialog feature | 1–2 d spike | M3 (spec 05 §4.7) |
| 19b | Window frame decision / Window menu, menus, settings, i18n | `craft-window`, `craft-menu`, `craft-settings`, `craft-i18n` | **SKIP as deps** (egui 0.37, suite coupling); **ADAPT** `craft-settings` ideas (toml_edit merge, backup, migration) | — | release / M3 |
| 20 | Updater | `craft-update` | **SKIP** (Craft-suite coupled; integrity = SHA256SUMS from the same release, no signature found) | — | release |
| 21 | egui fork itself | `storytold/egui` + `storytold/winit` | **SKIP as dependency**; cherry-pick/upstream specific patches (E6, E7/E19, E21, P7b) when we move to egui 0.37 | 2–5 d per patch | M2 (IME), release |
| 22 | UI test harness | `egui_kittest` usage, `tests_perf.rs` | **ADAPT** (we already use kittest; WordCraft has **no** committed screenshot snapshots) | — | M2 |

Top licensing red flags are collected in §L; the prioritised next steps are at the end.

---

## Overall assessment

- **Scale and age.** WordCraft is ~330 kLOC of Rust (engine 72 k, ui-egui 80 k, docx 37 k,
  layout 34 k) in a repository created on **2026-10-07** (verified via GitHub API), now at
  0.11.0 with ~1,850 open issues. craft-libs is ~190 kLOC including vendored hayro/krilla. The
  code is carefully commented and heavily tested (engine 1,116 `#[test]`, ui-egui 1,003,
  layout 550, proof 284; counted with grep), uses `unsafe_code = "forbid"` workspace-wide and
  denies `unwrap/expect/panic` in library crates. But the velocity means **API churn is
  certain**: depending on any crate by git means frequent re-pinning, and its internals are
  shaped by Word semantics. AGENTS.md's guidance ("prefer vendoring selected crates over
  depending on its workspace") is right; this note goes further — most value is in *designs*,
  and only small, dependency-free modules are worth copying.
- **Coupling to Word's model.** Everything in `engine`, `layout`, `render`, `pdf`, `formats`
  and `ui-egui` addresses text as `Pos { story: StoryRef, path: Path, off }` over a
  `Document { body: Vec<Arc<Block>>, styles, numbering, revisions, … }` and lays out *pages*.
  Markise addresses text as byte offsets into one Markdown string (ADR 0001) and lays out a
  continuous, virtualised block list (ADR 0002). None of those crates can be used as-is.
- **egui version.** Every egui-facing crate (WordCraft `ui-egui`; craft `ui`, `window`,
  `keymap`, `menu`, `font-manager`, `update[egui]`, `platform`) targets egui 0.37 and the
  workspace patches all egui crates to the `storytold/egui` fork plus a `winit` fork. Not every
  craft crate needs the fork, though: `craft-keymap`'s core has only serde/serde_json as
  dependencies, its egui integration is an optional feature, the fork-only `RawInput` fields are
  used by its **dev-dependency** tests, and `keymap/src/egui.rs` (lines ~413–436, verified)
  supplies trait fallbacks so it compiles on crates.io egui (fork-only layout data then reads as
  `None`). Each crate has to be checked individually. Taking the egui-facing ones still means
  moving Markise to egui 0.37. With MSRV no longer a constraint, egui 0.37 itself is
  reachable; the fork is the problem (unreleased "local follow-ups not yet committed" in its
  `PATCHES.md`, a second fork of winit, ~25 patches).
- **MSRV.** WordCraft/craft-libs 1.95, vendored Krilla 1.92. Not a constraint (owner decision).

---

## Details

### 1. Layering and attribution checks (`xtask`) — REUSE (vendor)

*What (verified).* `xtask/src/layers.rs` (338 lines, 11 tests, std + `serde_json`) reads
`cargo metadata`, assigns each workspace crate a class (`Layer(n)`, `Standalone`, `Testkit`,
`Exempt`), allows explicit intra-layer orderings, and forbids UI crates (`egui`, `eframe`,
`winit`, `egui_kittest`, `rfd`) below layer 6. The rule engine is separated from metadata
parsing so it is unit-testable. `xtask/src/assets.rs` (87 lines) fails CI when an asset file
is not listed in `ATTRIBUTION.md`. `cargo xtask ci` chains fmt, clippy, tests, i18n audit,
doctests, assets, layers and a wasm check.

*Fit.* Markise has a layered architecture in AGENTS.md (L0–L6) but no automated check and no
`xtask` (verified: no `xtask/` in the repo). This is a direct fit.

*Recommendation.* Vendor `layers.rs` and `assets.rs` into a new `xtask` crate; replace the table
with `markise-*` crates (`markise-geom` L0, `markise-md`/`markise-doc` L1, `markise-layout`/
`markise-theme`/`markise-blocks`/`markise-text` L2, `markise-render`/`markise-export` L3,
`markise-engine` L4, `markise-mcp` L5, `markise-ui-egui` L6; `markise-corpus` as `Testkit`;
apps exempt). Point `assets` at our `NOTICE`. Add NOTICE entry "derived from WordCraft xtask,
rev 6a2b520f, MIT OR Apache-2.0". Effort 0.5–1 day. No license or perf concerns.

### 2. Command architecture — ADAPT

*What (verified).* `engine/src/lib.rs`: a `CommandSpec { id, label, location, shortcut,
params, mutates, enabled: fn(&Session) -> Option<&str>, run: fn(&mut Session, &Value) ->
CmdResult }` built with `const fn` builders (`.key("Mod+B")`, `.pure()`, `.when(..)`), a
`Registry` (id → spec, `describe()` as JSON for agents, `by_shortcut`), and parameter helpers.
Menus, ribbon, shortcuts, palette, CLI scripts, the JSON control channel and MCP all go
through `Session::run`, which also classifies effects for IME (`ImeEditOutcome`). The README
says "422 commands"; `grep 'CommandSpec::new('` over `engine/src/cmd` finds **648**
constructor sites (some may be variants), so the README figure is stale (verified count,
semantics not audited).

*Quality.* Simple, explicit, testable; `mutates`/`pure` lets the session decide undo and dirty
state centrally; JSON params give agents and scripts one surface. Weakness: params are
stringly-typed `serde_json::Value` with hand-written `"params"` descriptions rather than typed
schemas.

*Fit.* Spec 04 §2 is a menu inventory with shortcuts; spec 02 §15 the shortcut tables; M3 needs
an MCP server and the CLI already exists. One registry feeding menus, key handling, the command
palette (if any), CLI and MCP is exactly what Markise needs.

*Recommendation.* Port the design into `markise-engine` for M2: stable dotted ids
(`format.strong`, `block.heading2`, `edit.selectWord`), `enabled` predicates, a `mutates` flag
that drives undo/dirty, a portable shortcut string resolved per platform, `describe()` for
MCP. Consider typed params (a small enum or `serde` structs) with a generated JSON schema
instead of free-form strings. Effort 3–5 days for the core plus incremental command porting.

### 3. Undo/redo — ADAPT design, different storage

*What (verified, `engine/src/session.rs`).* Each step is
`Undo { label, doc: Document, sel, pending, column }` held as `Arc<Undo>`; `checkpoint(label)`
pushes a clone of the whole document before a mutating command, except that consecutive
`"Typing"` checkpoints coalesce while `typing_open` is true. `MAX_UNDO = 500`,
`UNDO_LIST_MAX = 100` for the Undo dropdown (`edit.undoTo`). Extras: `join_next_undo()` (each
frame of a drag joins the previous step), `merge_undo_since(mark)` (a slider drag becomes one
step), `push_undo` after automatic rewrites (AutoCorrect/AutoFormat get their own step so Undo
reverts only the rewrite), `edit_snapshot()/restore()` to roll back a failed multi-command
transaction exactly, and IME-specific joins.

*Memory/perf.* Snapshot cloning is cheap for them only because `Document.body` is
`Vec<Arc<Block>>` (verified): a clone copies a pointer per block, O(blocks) per step; changed
blocks are new `Arc`s. For Markise, where the source of truth is one Markdown `String` (ADR
0001), a snapshot per step would copy up to 10 MB per keystroke group: unacceptable for NFR-4
and NFR-7 (500 × 10 MB).

*Fit.* The *semantics* map almost 1:1 onto spec 02 §11: typing within one block coalesces; Enter,
input-rule conversions, paste, formatting commands, caret jumps and IME commit end a group;
each step restores the selection; Replace-All is one step; source mode gets its own history.
WordCraft's "automatic rewrite is its own step" is exactly the spec's input-rule rule ("typing
`- ` creates a list; Undo restores the literal `- `").

*Recommendation.* Implement undo as a list of transactions of inverse text patches (we already
compute minimal `Patch { start, end, insert }`, `markise-engine/src/edit.rs`) plus selection
before/after and a label. Port: label + typing-group coalescing, `join_next`, `merge_since`
marks, "auto-rewrite = separate step", transactional `snapshot/restore` (cheap with patches),
bounded depth. Effort 3–5 days, M2.

### 4. Caret, selection, hit testing, keyboard navigation — ADAPT

*What (verified).* `layout/src/hit.rs` (924 lines): `hit(page, x, y, story) -> Pos`,
`caret(pos) -> Caret`, `vertical(pos, goal_x, dir)` (nearest line across goal x, with table-row
skipping rules), `line_bounds`, `selection_rects`, `visual_step` (bidi-aware: `Moved(pos)` or
`Edge { start, stop, last_line, rtl }` so the caller decides to cross into the next line or
paragraph), `selection_bar_at` (margin click selects a line). Everything is page-based and
addressed by `StoryRef`/`Path`. `engine/src/cmd/caret.rs`: `caret.left/right/up/down/wordLeft/
wordRight/home/end/paraUp/paraDown/docStart/docEnd/pageUp/pageDown`, `select.word/sentence/
paragraph/line/all/dragUnit`, with `{extend}` params and RTL-aware word direction. Word
boundaries (`doc/src/para.rs::word_start/word_end`) are a character-class scan
(`bidi::is_word_char`), Word-like (Ctrl+Right includes trailing spaces), **not** UAX #29 word
segmentation and no dictionary segmentation for Thai/CJK. `doc/src/cluster.rs` (236 lines,
depends only on `unicode-segmentation`) extends grapheme clusters for Myanmar, Khmer and Tai
Tham stackers and dependent signs.

*Fit (assumes proposed ADR 0005).* ADR 0005 would put caret/hit testing on parley's cluster model, which provides
point→cursor, cursor→rect, visual left/right with affinity, and word/line movement inside one
layout. What Markise still has to build, and where WordCraft is a useful reference:
document-level movement across blocks/segments with a persistent goal-x (`vertical`), the
`Edge` result pattern for crossing block boundaries, selection-by-unit drag (`dragUnit`),
select word/sentence/paragraph (spec 02 §10 Cmd+D, Cmd+L, Opt+Cmd+P; Cmd+E "styled scope" is
ours), and margin/selection-bar behaviour.

*Recommendation.* ADAPT; no code reuse. Use UAX #29 (via parley/`icu_segmenter`) for words, not
WordCraft's char-class scan. Keep `cluster.rs` in mind: if parley's caret stops inside Myanmar
or Khmer stacks (test with ADR 0005's corpus), vendor it (tiny, permissive). Effort: part of the
M2 caret work (1–2 weeks with parley integration).

### 5. IME — ADAPT app side; egui side needs patches

*What (verified).* `doc/src/ime_text.rs` (712 lines) builds a bounded (`MAX_IME_TEXT_BYTES =
4000`) window of the *visible* text around the selection (hiding tracked deletions, objects as
their plain text), maps visible↔stored offsets in both directions, and turns an IME
delete-surrounding request (chars or bytes) into stored ranges. `ui-egui/src/keys.rs` handles
`ImeEvent::Preedit { text, active_range_chars }`, `Commit`, `DeleteSurrounding{before_chars,
after_chars}` and `DeleteSurroundingBytes` and publishes surrounding text with owner/generation
stamps. **These event shapes are fork APIs**: the fork's `PATCHES.md` lists E6 (`request_focus`
no longer interrupts composition), E21 (`ImeEvent::DeleteSurroundingBytes`, for macOS
press-and-hold, dictation and Wayland IMs; needs the winit fork) and local P7b (outgoing
surrounding text with owner/generation/purpose; changes `egui_winit::process_viewport_commands`'
signature). ADR 0005 R4 already notes that egui 0.33's `ImeEvent::Preedit(String)` has no
cursor range. *Re-checked against stock egui 0.37.0 (2026-10-27):* `Preedit { text,
active_range_chars }` and `DeleteSurrounding { before_chars, after_chars }` are upstream now,
but only eframe's web text agent emits `DeleteSurrounding` (egui-winit/winit 0.30 never do);
`DeleteSurroundingBytes` (E21), outgoing surrounding text (P7b) and E6 remain fork-only.

*Fit.* Spec 02 §11 requires "IME composition end" as an undo boundary; ADR 0005 R4 requires
inline preedit, cancel, candidate window at the caret. macOS press-and-hold accents and
dictation, and Wayland input methods, need delete-surrounding and surrounding text, which stock
egui (0.33, and 0.37 as verified on 2026-10-27 on the desktop/winit path) does not provide.

*Recommendation.* (a) App side: port the *idea* of a bounded surrounding-text window over the
focused block's source/rendered text and a visible↔source offset map (Markise's rendered vs
source mapping is a different problem; WordCraft's is a useful test checklist). (b) egui side:
when moving to egui 0.37, carry a minimal patch set (E6, E21, P7b subset) as our own small
fork or upstream PRs, rather than depending on `storytold/egui` + `aiongg/storytold winit`
wholesale. Verify which of these landed upstream first (unverified). Effort 1–2 weeks incl.
platform testing; M2.

### 6. System clipboard bridge — REUSE (vendor, adapt)

*What (verified).* `apps/wordcraft/src/clipboard.rs` (333 lines) on top of `arboard` 3.7
(`wayland-data-control` feature): `set_html(html, text)` keeps one writer open (X11 serves
contents from it) and refuses to fall back to X11 on Wayland sessions without data-control;
`text()`, `html()`, `rtf()` (Windows/Wayland, outside arboard), `picture()` (file list → image
pixels → encoded picture) and `formats()`. The engine side (`cmd/paste.rs`) accepts `text`,
`html`, `rtf`, `picture` as JSON params so headless/agent paste uses the same path.

*Size limits — what is bounded and what is only rejected after reading (verified):*
- `text()`: **no cap** (`get_text()` result returned as is).
- `html()`: arboard reads the whole HTML first; the 16 MiB `MAX_PASTE_BYTES` check runs **after
  allocation** (post-read rejection).
- Wayland `rtf()` / `encoded_picture()`: **bounded reads** — the platform module reads a MIME
  pipe with `take(cap + 1)` and refuses anything larger. RTF on other platforms is read by the
  per-OS `platform` modules (not audited in detail).
- Non-Windows images: `arboard::get_image()` **decodes before any validation**; size checks
  happen downstream in the UI.
- Windows images: read past arboard and header-checked before decode, but that check lives in
  `wordcraft_ui_egui::paste_picture` (`encoded_picture`/`dib_picture`), which in turn uses
  `wordcraft_geom::picture::probe`/`probe_dib`, `MAX_DECODED_BYTES` and the `image` crate.

So the module is not self-contained: it imports `wordcraft_engine::cmd::paste::MAX_PASTE_BYTES`
and `wordcraft_ui_egui::paste_picture::ClipboardPicture`, and has four per-OS `platform`
sub-modules (Wayland via `wl-clipboard-rs`, Windows, macOS, fallback).

*Fit.* Spec 02 §14 and 04 §9: Copy puts HTML + plain text (P0); smart paste reads HTML (P1);
image paste (04 §7.1, P0). egui's own clipboard is text-only.

*Recommendation.* Vendor this module into `apps/markise` (or a small `markise-clipboard`
platform crate) with a NOTICE entry, together with the pieces it needs: the `platform`
sub-modules we keep, the picture probe/limit logic from `paste_picture.rs` and
`geom::picture` (or our own image probe), and a local `MAX_PASTE_BYTES`. Drop MathML and RTF
initially; keep the Wayland rules. Improve on it where it only rejects after reading: cap
`text()`, and prefer bounded reads (or a size query) for HTML and images where the platform
allows; where arboard cannot bound a read, document the post-read rejection. Effort 2–3 days.
License fine (arboard and wl-clipboard-rs are MIT/Apache).

### 7. HTML → structure for smart paste — ADAPT

*What (verified).* `crates/formats/src/html.rs` (2,443 lines): tag-soup reader (tokenizer +
open-element stack with HTML's implicit closes for `p`, `li`, `tr`, `td/th`, headings; nesting
cap 256; scripts/styles skipped; entity decoding; limited CSS for colour/weight/style). Output is
WordCraft's intermediate `model::{FBlock, Para, Inline, …}` then the Word document.

*Fit.* Spec 04 §9 P1: convert clipboard HTML to Markdown (headings, lists, links, emphasis,
tables, code, images). Markise's target is Markdown text generated by our `write` generators
(ADR 0001).

*Recommendation.* Write our own HTML→Markdown converter; use WordCraft's tokenizer/implicit
close handling as a reference, or evaluate `html5ever` (MIT/Apache, heavier) for correctness.
Effort 3–5 days. M3.

### 8. Find & replace — ADAPT small parts

*What (verified).* `FindState { query, replace, match_case, whole_word, regex, wildcards,
literal, match_prefix, … }`; `find` collects all results for a story, selects the first after
the caret; `replace_all` replaces last-to-first so positions stay valid and records one undo
step. `find_pattern.rs` translates Word wildcards and `^p`/`^t` codes to `regex` (linear-time,
no catastrophic backtracking), pattern capped at 1,024 chars, `{n,m}` capped at 1,000. Search
runs synchronously inside the command (no background thread).

*Fit.* Spec 04 §2.3 (P0): case / whole word / regex, replace one / all; Replace-All one undo
step with an Undo button (spec 02 §11). NFR-4 forbids long UI-thread work: a regex over 10 MB
should run on a worker with streamed results.

*Recommendation.* Use the `regex` crate (linear time) and caps as WordCraft does; replace-all
last-first as one transaction; skip Word wildcard syntax. Add background search + incremental
re-search per edited block (our own). Effort 2–3 days. M2.

### 9–10. Proofing (`crates/proof`) — REUSE candidate for spelling; SKIP grammar/hyphenation

*What (verified).* `wordcraft-proof` (16.3 kLOC, 284 tests) depends only on `log`,
`miniz_oxide`, `unicode-normalization`: **no coupling to the Word model** (it checks plain text
with optional language spans). English: the public-domain Moby word list (~160k words) stored
as a front-coded, deflated sorted list (`dict.rs`, ~1 MB inflated; binary-searched, about 3 MB
resident by my estimate — unverified), plus `assets/spelling/en-extra.txt`, inflection rules,
user dictionary and ignore list, edit-distance suggestions. Other languages (pl, pt, it, ru,
cs, id, es variants): a Hunspell `.aff`/`.dic` reader (`affix.rs` 1,393 lines, `hunspell.rs`)
that loads **system-installed** dictionaries (Homebrew, `/usr/share/hunspell`, LibreOffice
folders); none are bundled. Per-language grammar rules with English/native messages ("Word's
defaults"). Hyphenation: dictionary + Liang patterns (en-us CC0, pl/pt/it/de from hyph-utf8).
State is process-global (`OnceLock<RwLock<HashSet<String>>>` for user words).

*Quality (partly verified).* Defensive caps (dictionary ≤ 64 MiB inflated, ≤ 4 M entries),
no panics in library code. Suggestion quality and Hunspell-compatibility coverage were **not**
measured; the affix engine is a from-scratch implementation, not a port of Hunspell.

*Fit.* Spec 04 §13 (P1): macOS uses the OS speller (WordCraft does not use NSSpellChecker —
we need our own small macOS bridge); other platforms "downloadable dictionaries per language",
learn/unlearn words, skip code and links by default.

*Recommendation.* Spike (2–4 days): vendor `affix.rs` + `hunspell.rs` + `dict.rs` into a
`markise-spell` crate behind a `Speller` trait with an NSSpellChecker backend on macOS, and
compare against `spellbook` (Hunspell-compatible pure-Rust crate from the Helix project,
MPL-2.0, allowed file-level by AGENTS.md; license and maturity unverified). Choose on accuracy
and memory. Replace the global statics with an owned `Speller`. Do not take the grammar modules
(Word-specific, messages not in our i18n). Hyphenation: SKIP until a theme requirement exists.
M3.

### 11. Font picker, style galleries — SKIP

*What (verified).* `ui-egui/src/previews.rs`: the Styles gallery and font-menu entries are tiny
documents laid out and rasterised by the real engine, cached as textures, rendered deferred
(tests assert no deadlock and that installed families stay unloaded until shown).
`craft-font-manager` is a whole Google Fonts/Noto download manager with egui panel.

*Fit.* Markise fonts come from themes (spec 03) and preferences; spec 04 has no font menu or
style gallery. The only transferable idea is "preview = real renderer, off-thread, cached
texture", useful for a theme picker with live thumbnails. SKIP.

### 12. Accessibility of custom-painted text — ADAPT

*What (verified).* `ui-egui/src/a11y.rs` (798 lines) publishes an AccessKit tree through
`ctx.accesskit_node_builder`: a `Document` node with caret/selection as an AccessKit
`TextSelection`, one paragraph/heading/list-item node per block, one text-run node per laid-out
line with per-character positions, tables as table/row/cell, images by alt text, links.

*Fit (assumes proposed ADR 0005).* ADR 0005 R17: if text is painted by parley glyph meshes, egui's built-in text a11y no
longer covers the document. This is the closest working example in egui.

*Recommendation.* ADAPT for M2/M3 with viewport-bounded node publication (WordCraft's page model
bounds it by pages; we must bound by visible blocks for 10 MB documents — check that their tree
is not built for the whole document every frame; unverified). Effort ~1 week.

### 13. PDF export — ADAPT, use upstream Krilla

*What (verified).* `crates/pdf/src/lib.rs` (1,959 lines, 63 tests) walks the page display list
and writes: real text with subsetted embedded fonts and a Unicode mapping (selectable,
searchable), synthetic bold/italic, fills/rules/underlines, images (JPEG passthrough), link
annotations (URL and internal), a document **outline from headings**, metadata, best-effort
**tagged PDF** (`H1`–`H6`, `P`, `Figure` with alt text, artifacts for headers/footers), and a
retry that draws an unsubsettable font as outlines (≤ 32 fonts). It goes through `craft-pdf`
(`write.rs`: `Writer`, `Metadata`, `configuration(version, archival)`, `font(data, index,
variations)`, `uri_target` URL vetting), which re-exports a **patched Krilla 0.8.2**:
`craft-libs/vendor/krilla` with 16 patches (unwind-safe drop, DeviceN, overprint, knockout,
PDF/X output intents, optional content, mesh gradients, PNG decode cap…), applied through
`[patch.crates-io] krilla = { git = craft-libs }` in the consuming workspace (verified in
WordCraft's root `Cargo.toml`).

*Fit.* Spec 04 §8.1 PDF (P1): theme styling, paper sizes, margins, page break per top-level
heading, header/footer placeholders, bookmarks from headings, no dark themes. Almost all
prepress patches (DeviceN, overprint, PDF/X) are irrelevant to Markise. Krilla upstream already
offers text with fonts, links, outlines and tagging (unverified for 0.8.x specifics). With
parley + fontique (if proposed ADR 0005 is accepted) we hold font bytes and glyph runs — the natural Krilla input.

*Recommendation.* Depend on **upstream `krilla`** from crates.io; port WordCraft's export
*structure* (text-as-text with ToUnicode from source text, outline from headings, tagging,
bad-font→outline fallback, URL vetting) into `markise-export`. Do not take `craft-pdf` as a git
dependency (it forces a `[patch.crates-io]` of Krilla on our workspace and drags hayro reader
code behind features). Effort 1–2 weeks after pagination exists. M3.

### 14. MCP server and CLI — ADAPT

*What (verified).* `crates/mcp` (1,586 lines incl. 357 test lines, 30 tests): hand-written
JSON-RPC 2.0 over stdio, protocol `2025-06-18` with fallbacks to `2025-03-26` and
`2024-11-05`, capabilities `tools` + `resources`, tools mapping to `list_commands`, `execute`,
`batch`, documents, `render_page`, `save`. Two backends: `Headless` (in-process `Session`) and
`Remote` (drives a running app through a loopback control channel authenticated with a key
from `crates/control-key`, 87 lines). File access can be confined with
`--automation-read-root/--automation-write-root`. The CLI (`apps/wordcraft-cli`) runs `--cmd`
or `run --script job.json` command lists, then saves. Dependencies: only workspace crates and
`serde_json`.

*Fit.* AGENTS.md L5 `markise-mcp` (M3). The design — MCP tools as a thin layer over the
command registry (§2), headless vs remote backends, path confinement — fits directly.

*Recommendation.* ADAPT: write `markise-mcp` the same way (or evaluate the official `rmcp`
SDK; unverified status). Path confinement is mandatory for us too. Effort 3–5 days once the
command registry exists. M3.

### 15–16. Autosave/recovery and atomic save — ADAPT

*Recovery (verified from `docs/recovery.md` and `engine/src/recovery*.rs`).* One worker thread
and one store for all open documents; each frame the host checks each document's revision and
timer and queues at most one job; a document is snapshotted at most every 10 s and only when its
revision changed; only the due document is cloned; encoding, I/O and validation happen on the
worker; no repaint when nothing changes. A table defines what happens to a copy on Save, Close
without saving, Don't Save on quit, clean exit and crash. Restore opens the copy as a new
untitled document and never writes to the original. Password-protected documents are excluded.

*Fit.* Spec 04 §10 (P1): periodic backup of unsaved content to an app data folder as
`{date}-{filename}.md`, untitled documents named from first heading/sentence, "Recover Unsaved
Drafts" opens that folder, can be disabled. Markise's snapshot is just the text (cheap to clone
an `Arc<str>`/rope; a 10 MB write on the worker is fine under NFR-8's "never blocks typing").

*Recommendation.* Port the worker/revision/timer design and the event table; our file format is
the spec's plain `.md` naming. Effort 3–4 days. M2 (spec says autosave/recovery is in M2).

*Atomic save (verified).* `craft-atomic` writes a same-directory temp file, `F_FULLFSYNC` on
macOS, renames, best-effort directory fsync on Unix, retries rename with backoff on Windows
(AV/indexer sharing violations), keeps Unix mode. Markise's `save.rs` already does
temp+fsync+rename+dir fsync with tempfile and a disk-version recheck. ADAPT the two missing
details (macOS full fsync, Windows rename retry). Effort 0.5 day.

### 17. `craft-docpath` — REUSE

*What (verified; 1,026 lines including a 403-line test file, 12 tests, no dependencies).* A
**lexical path-policy filter**: decides whether a path or `file:` URL stored in a document may
be opened — local paths, or paths on the same network share the document came from — and refuses
other hosts, device paths and non-file URLs. It is **not a confinement boundary**:
- its own caller contract states that a lexical check cannot see mapped drives, `subst` drives,
  symlinks or junctions, so a local-looking path can still reach another computer;
- it accepts absolute local paths and `..` that leaves the document's folder, as long as
  traversal stays within the filesystem root;
- it does no I/O.

*Fit.* Markise renders linked images (spec 04 §7) and HTML blocks; a tested lexical policy for
UNC/`\\?\`/device paths on Windows is useful as an **additional** filter there. It does not
replace theme-root confinement (TI-9 `@import`/`url()` must stay inside the theme folder) or
filesystem-aware checks (canonicalisation, symlink/junction and mapped-drive handling); those
remain separate work.

*Recommendation.* REUSE as an extra filter: pin as a git dependency (`rev = …`) or vendor
(small, dependency-free; check it has no craft-specific assumptions). Effort 0.5–1 day. M2/M3.

### 18. UI patterns from `ui-egui` — ADAPT

All verified at header level only:
- `file_dialogs.rs`: native dialogs that do not block the UI thread (host returns the answer on
  a later frame, follow-up actions carried with the request), fallback to an in-app browser
  when Linux has neither portal nor zenity. Fits spec 04 open/save; Markise uses `rfd`
  (unverified whether async mode is used today).
- `window_geometry.rs`: persist size/position/maximised in egui points. Spec 04 §12 restore.
- `desktop_theme.rs`: Linux light/dark via XDG Desktop Portal (winit reports none on Linux);
  uses `zbus` 5. Fits spec 05 §4.2 "follow system".
- `scroll.rs`: touchpad deltas 1:1, own momentum on platforms without system momentum, eased
  wheel notches; relevant to NFR-5 feel.
- `caret_glide.rs`: animated caret modelled on Word for Windows — not a Markise requirement;
  SKIP (and it would fight NFR-10 idle CPU unless strictly bounded).

### 19. craft-libs desktop crates — SKIP as dependencies

- `craft-window` (1,684 lines): one decision for client- vs server-side decorations per desktop,
  app-drawn title strip, window list/cascade/Window menu. Useful reference for Linux CSD; needs
  egui 0.37 (+ objc2 on macOS).
- `craft-keymap` (16.6 k, 191 tests) — **reuse candidate, not SKIP** (verified in
  `Cargo.toml`): the core (chords with canonical text and per-platform display, commands,
  scopes, keymap catalog with defaults, user overrides, conflict detection, profiles, per-OS
  reserved chords, import of other apps' shortcut text) depends only on `serde`/`serde_json`.
  Feature `egui` adds dispatch from egui input and compiles on crates.io egui (trait fallbacks in
  `src/egui.rs` ~413–436 for the fork-only `keyboard_layout_is_latin`/`latin_layout_char`, which
  then return `None`, i.e. no Latin-layout shortcut mapping on non-Latin layouts). Feature `ui`
  (the Keyboard Shortcuts dialog) pulls `craft-ui` + `craft-i18n`; skip it. The fork is needed
  only by dev-dependency tests. Fit: spec 05 §4.7 custom shortcuts and the `conf.user.json`
  key-binding interop; the command-id model also matches §2. Recommendation: 1–2 day spike —
  use the core (git dep pinned rev or vendor with NOTICE) and, after the egui 0.37 move, the
  `egui` feature; map `conf.user.json` bindings onto its overrides.
- `craft-menu` (6 k): native macOS menu through `muda`/objc2, platform menu layouts, shortcuts
  from craft-keymap. Requires egui 0.37 and `craft-quit`; reference for the macOS menu bar.
- `craft-settings` (2 k, 51 tests): `config.toml` via `toml_edit` so hand edits and comments
  survive, atomic saves, backup, damaged files moved aside, migrations. Markise's interop file is
  `conf.user.json` (spec 05 §5), so ADAPT the robustness ideas (never fail to load, backup,
  move-aside) only.
- `craft-i18n` (4.2 k): ICU4X plurals/numbers/collation with embedded postcard data (Unicode-3.0)
  and TSV catalogs from `benandez/craft-i18n` (Apache-2.0, verified via GitHub API). Markise UI
  localisation is P2; revisit then.

### 20. `craft-update` — SKIP

*Verified.* 12 kLOC, 229 tests; GitHub-releases based, `SHA256SUMS.txt` and GitHub's asset
`digest` for integrity, installers per platform, suite manifest. The app list is hard-coded to
the Craft suite (`suite.rs`), and integrity relies on checksums from the same release origin; a
grep for signature/minisign/ed25519 found **no** detached-signature verification (unverified
beyond grep). For Markise release packaging prefer OS code signing + notarisation and, if we
self-update, a signed manifest (e.g. minisign). SKIP.

### 21. The `storytold/egui` fork — SKIP as dependency, cherry-pick

*Verified from `PATCHES.md`.* Superset of egui 0.37.0 with ~21 numbered patches plus
uncommitted "local follow-ups", built on a winit fork. Relevant to us: E6 (focus does not break
composition), E7/E19/E10 (letter shortcuts on non-Latin layouts: Russian/Arabic/Hebrew users
pressing Cmd+B), E21 + P7b (IME delete-surrounding and surrounding text), E4 (Wayland clipboard
through winit). Bidi/shaping patches (E5, E12, E13, E16) matter only for egui widgets (find bar,
dialogs), not the parley-rendered document. Recommendation: when upgrading to egui 0.37, carry a
minimal, documented patch queue of our own (or upstream PRs) for E6/E7-E19/E21; do not pin the
whole fork. Effort 2–5 days per patch including platform tests.

### 22. Testing harness — ADAPT

*Verified.* WordCraft uses `egui_kittest` for interaction tests in `ui-egui` (ribbon, dialogs,
table mouse, perf probes) but has **no committed screenshot snapshots** (no `snapshots/*.png`
found). `xtask visual-diff` compares PDFs page by page with poppler (SSIM, line breaks) —
relevant later for PDF export regression. Markise already depends on `egui_kittest` 0.33.
Nothing to reuse beyond the pattern of kittest-driven interaction tests plus the layering check
in CI.

---

## L. Licensing review and red flags

Code in both repositories is MIT OR Apache-2.0 (verified in `Cargo.toml`, `NOTICE`). Items that
need care:

1. **Bundled fonts are OFL-1.1** (WordCraft `assets/fonts`: Inter, Source Sans 3, Source Serif 4,
   JetBrains Mono; optional craft-fonts: BIZ UDPGothic/UDMincho, Shippori Mincho). OFL is not in
   AGENTS.md's permissive list and is the same open question as M1-5. Do not copy these assets.
2. **Hunspell dictionaries.** WordCraft bundles none and reads system ones. Many language
   dictionaries are GPL/LGPL/MPL or mixed (WordCraft's own doc says the Italian one is GPL). If
   Markise ships or downloads dictionaries (spec 04 §13 "downloadable dictionaries"), each needs
   its own license review; bundling GPL data is not allowed. Runtime download of user-chosen
   dictionaries keeps them out of our distribution, but must be presented as third-party data.
3. **Hyphenation patterns.** `hypher`'s **default features embed all languages including Slovak
   (GPL-2.0-or-later)**; craft-text avoids it with `default-features = false` and an explicit
   `slovak-gpl` feature, and warns that Cargo feature unification can pull it in from another
   dependency. If we ever use `hypher`, use `default-features = false` and add a `cargo deny`
   rule. Italian patterns are LPPL-or-MIT (used under MIT); keep the notice text.
4. **CC0 data.** en-us hyphenation patterns (CC0) and Krilla's ICC profiles (CC0) — CC0 is not
   in AGENTS.md's list (same as `notify`, open question D3).
5. **Moby word list** (public domain) — fine; record provenance in NOTICE if vendored.
6. **Patched Krilla.** Taking `craft-pdf` forces a `[patch.crates-io]` on our workspace and a
   16-patch fork (still MIT/Apache). Use upstream Krilla instead.
7. **egui/winit forks.** License is fine (upstream MIT/Apache; GitHub reports Apache-2.0), but
   they are unreleased forks with in-flight local patches; supply-chain and maintenance risk, not
   a license problem.
8. **Trademarks.** `docs/brand/` (ArtCraft name, logos) is explicitly not open source; WordCraft
   icons are MIT/Apache but carry the product look. Do not copy brand assets or names.
9. **Fogra characterisation data** (craft-cms tests): redistribution only unmodified with
   attribution; irrelevant unless we vendor `craft-cms` (we should not).
10. **ICU4X data** (craft-i18n, craft-text segmenters): Unicode-3.0 — allowed by AGENTS.md;
    ship the Unicode license notice if adopted.

Every vendored file must get a `NOTICE` entry: source repo, revision, path, "MIT OR Apache-2.0",
and the copyright line "Copyright (c) 2026 ArtCraft Team and the WordCraft contributors" (or
craft-libs contributors).

---

## Prioritised next steps

1. **Now (pre-M2, ~1 day):** vendor `xtask/src/layers.rs` + `assets.rs` into a Markise `xtask`
   crate with our layer table; add `cargo xtask layers` to the pre-commit checks in AGENTS.md;
   NOTICE entry.
2. **M2 design (2–3 days, ADR):** write ADR "Command registry and undo" — command ids/registry
   modelled on WordCraft `CommandSpec`, undo as patch transactions with WordCraft's grouping
   rules (typing group, join, merge-since, auto-rewrite step, snapshot/restore), mapped to spec
   02 §11 cases. Include the IME boundary rules.
3. **M2 (2–3 days):** vendor and trim `apps/wordcraft/src/clipboard.rs` (plus its platform
   modules and picture-probe dependencies; add caps where it only rejects after reading) for HTML+text copy and
   image paste; read its `platform` sub-modules first.
4. **M2 (1–2 days):** add macOS `F_FULLFSYNC` and Windows rename retry to `markise-engine/save.rs`
   (from `craft-atomic`); port the recovery worker design for spec 04 §10.
5. **M2 spike (2–3 days):** egui 0.37 upgrade feasibility now that MSRV is unconstrained: build
   Markise on stock 0.37, list which of fork patches E6/E7/E19/E21/P7b are still needed, decide
   "own patch queue vs upstream PRs". Feeds ADR 0005 R4 (IME).
6. **M2/M3 (1 day):** vendor `craft-docpath` as an additional lexical filter for linked images;
   keep theme-root confinement and filesystem-aware checks as separate work.
6a. **M3 (1–2 days):** spike `craft-keymap` core for spec 05 §4.7 shortcut customisation.
7. **M3 spike (2–4 days):** spelling — `wordcraft-proof` affix/dict modules vs `spellbook` vs
   OS spellers; measure accuracy on a word list and resident memory against NFR-6/7.
8. **M3 (1–2 weeks):** PDF export on upstream Krilla, porting WordCraft's export structure
   (outline from headings, tagging, links, bad-font fallback); later reuse `xtask visual-diff`
   idea for regression tests.
9. **M3 (3–5 days):** `markise-mcp` over the command registry, headless + remote backends, path
   confinement, following `crates/mcp`.
10. **Before any vendoring:** re-read the exact files at the pinned revision, run their unit tests
    in our tree, and record license/attribution; do not take OFL fonts, dictionaries or brand
    assets.

Open items not verified in this pass: suggestion quality of `wordcraft-proof`; whether
`a11y.rs` builds nodes only for visible pages; `clipboard.rs` macOS/Windows platform sub-modules in detail; whether the
fork's IME patches exist upstream in egui ≥ 0.37; Krilla 0.8 upstream feature coverage for
tagging/outline; egui 0.37's own MSRV.
