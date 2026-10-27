# Research: MarkText (Muya) compared with Markise

Status: research note, WIP (2026-10-27; revised after Codex review the same day: save-path scope, per-rule soft-line scope, C11 renumbering condition, measurement framing). No production code was changed. Author: research agent
on branch `research-marktext`. Docs only.

**Sources read** (shallow clone, read-only, outside the repository):

| Repository | Revision | Commit date | License |
|---|---|---|---|
| `marktext/marktext` (monorepo: `packages/desktop` = Electron app, `packages/muya` = editor engine `@muyajs/core` 0.2.0, TypeScript rewrite) | `635c9218081a864492771ec487d065fbb35de104` | 2026-10-27 | MIT (© Luo Ran, MarkText Contributors) |
| Markise `main` (for the round-trip comparison) | `e04cf2d` | 2026-10-27 | — |

Paths below are relative to `packages/` of the MarkText clone (`muya/src/…`, `desktop/src/…`).

**Ground rules applied.** MarkText is an independent MIT-licensed editor, **not** the reference
product; the clean-room rules of AGENTS.md do not restrict studying it. Nothing in this note comes
from the reference product. Code reuse is possible with MIT attribution, but MarkText is
TypeScript on a DOM/`contenteditable` model, so this note recommends *behaviours and tests*, not
code. Test data that could be reused (MIT) is listed in §7 with the attribution it needs.

**Verification legend.** "verified" = read in the source at the revision above or measured;
"measured (headless)" = run in Node 22 with `tsx` on Muya's own parse/serialize modules, outside
Electron and without DOM rendering; "inferred" = reasoned from code paths, not executed;
"unverified" = not checked.

---

## Summary

| # | Area | MarkText (Muya) | Markise (main / ADR 0006) | Who is better / action |
|---|---|---|---|---|
| 1 | Document model | JSON block tree (`JSONState`), leaves hold raw inline Markdown; edits are `ot-json1` ops with `ot-text-unicode` sub-ops; DOM via snabbdom (verified) | Source text is the truth; spans + patches (ADR 0006 §2) | Markise (fidelity, perf); MarkText's OT ops are collaboration-ready |
| 2 | Parse / serialize | `marked` 18 lexer + own token walk (`state/markdownToState.ts`); **regenerating** serializer (`state/stateToMarkdown.ts`); HTML paste via `turndown` (verified) | Own lossless parser, span-based writer, byte-identical round trip | **Markise** |
| 3 | Round-trip fidelity (owner's notes) | An untouched pure-LF or pure-CRLF file is written back unchanged (mixed/CR-only endings are normalised on load); **the first visual (Muya) edit re-serializes the whole file** (source mode saves CodeMirror text as typed). Normalisation baseline under default settings: 51/55 notes of a feature-selected sample change, ~9 % of the sample's lines (measured (headless), §1) | 55/55 byte-identical (measured); edits are local patches by design | **Markise**; keep A8/C11/C20/C23 strict |
| 4 | Input rules | Re-evaluated on **every keystroke**; `#`, setext, HR, 4-space indented code and lists convert immediately; ATX headings, lists, quotes and HR also match on a soft-broken line (indented code and task rules only at the block start) (verified, `muya/src/block/base/format.ts` `_convertIfNeeded`) | Spec 02 §2.1/§2.2, C9, ADR §2.6 hints | **Markise spec**; MarkText shows why (§3.1) |
| 5 | Enter / Backspace / Delete / Tab | Mature, many edge cases fixed by issue-numbered regressions; several behaviours our spec does not cover (§3.3) | Spec 02 §5 | Adopt ~12 gap fills (§5) |
| 6 | Enter inside a styled span | Raw text split, markers not repaired (verified) | S-3.1 repairs markers | **Markise spec** |
| 7 | Undo grouping | Group = edits within 1 s of the group's **first** edit; split on whitespace insert, insert↔delete switch, selection change; explicit commands isolated; 100-entry stack (verified, `muya/src/history/index.ts`) | C13 pause-based, ADR §4.2, byte-bounded | Mixed: adopt **word-boundary split** (A3), keep pause rule and byte bound |
| 8 | Input-rule undo | Conversion is in the same frame batch as the keystroke; Undo cannot restore the literal `- ` paragraph (inferred) | ADR §2.6: Undo restores literal text | **Markise** |
| 9 | Auto-pair | Brackets/quotes/Markdown pairs, default **on** for Markdown syntax; apostrophe test is ASCII-only (`[a-z\d]`) (verified, `muya/src/block/base/content.ts`) | Spec 02 §4 (letters incl. Cyrillic; Markdown pairing off) | **Markise spec** |
| 10 | Tables | Toolbar, drag bars, rectangular cell selection, `<br/>` in cells; **always pretty-printed on save** with display-width padding (verified) | A8 (untouched tables keep bytes) | Markise for fidelity; adopt display-width padding (§5 R10) |
| 11 | Code blocks | Language autocomplete, auto-indent, Emmet-like tag expansion in HTML blocks, Shift+Enter exits; `trimUnnecessaryCodeBlockEmptyLines` (default **on**) drops leading/trailing blank lines inside fences (verified) | Spec 02 §7; lossless | Markise for fidelity; keep the exit and auto-indent ideas |
| 12 | Paste | HTML → Markdown (turndown + GFM plugin), URL-over-selection → link, image paste; **fetches page titles over the network** for pasted bare-URL links (verified, `muya/src/utils/paste.ts`) | Spec 02 §14 | Adopt URL→link; forbid implicit network fetches (R14) |
| 13 | IME | Zero-width-space composition anchors (block end, empty table cell), no re-render while composing, Playwright IME tests (verified) | ADR 0005 §4.4 / ADR 0006 §6.3 | Equivalent intent; reuse test scenarios |
| 14 | Performance | No virtualization; desktop serializes the **whole document, word count, TOC and a deep state clone on every change batch**; Muya parse is super-linear: 1 MB 0.21 s, 3 MB 1.9 s, 10 MB ~19 s (measured (headless)) | NFR-1…10; 10 MB parse 17.8 ms (measured) | **Markise** |
| 15 | Themes | 35 bundled palette themes as CSS variables (`--editorColor`, `--themeColor` …) plus `customCss`; no Typora-theme import (verified) | CSS-subset importer, `Style` + `ColorScheme` (ADR 0007) | Markise more flexible; MarkText palettes are MIT test/inspiration input |
| 16 | Export | Styled HTML, PDF/print via Electron `printToPDF`, Pandoc bridge (verified) | HTML now; PDF P1 | MarkText ahead in breadth |
| 17 | Rendering extras | KaTeX, Mermaid, Vega-Lite, PlantUML, flowchart.js, sequence diagrams, emoji picker, slash menu, block handle menu, heading folding, spellcheck (verified) | Math/diagrams postponed (AGENTS.md) | MarkText ahead; not M2 scope |
| 18 | Dialect conformance | CommonMark 0.31: 87.7 %, GFM 0.29: 86.3 % (their `muya/test/spec/conformance.md`) | Spec 01 dialect with own conformance suite | Different dialects; see C29/C31 notes (§4) |
| 19 | Tests | 137 Playwright e2e specs, many named after the bug they pin (e.g. `editing/delete-merge-blockquote-5423.spec.ts`) (verified count) | Unit + proptest + kittest | Mine their regression list for M2 edge cases (§6) |

Scope note: the round-trip figures are a headless normalisation baseline under MarkText's default
settings on a feature-selected 55-note sample (§1.3), not an executed edit/save in the app, and
they apply to its visual (Muya) editor, not to source-code mode.

Top recommendations (details in §8): keep Markise's lossless architecture and the strict spec
choices where MarkText regresses (input rules, S-3.1, C11, A8, C20/C23); **fill ~12 spec gaps**
MarkText handles (Shift+Enter on a blank soft line, Enter on an empty heading, empty middle list
item/quote paragraph splits, Delete-at-end merges, atomic image deletion, URL paste over a
selection, format-command whitespace trimming and cross-block formatting, trailing paragraph after
a final table/code block, list looseness on structural edits, word-boundary undo splits); and
turn MarkText's regression titles into Markise tests.

---

## 1. Round-trip fidelity on the owner's notes

### 1.1 What MarkText writes on save (verified in source)

- On open, `desktop/src/main/filesystem/markdown.ts` (`loadMarkdownFile`) decodes with
  iconv-lite (encoding guessed with `ced`) and converts **all** line endings to LF. The ending
  used on save is the file's own ending when it was pure LF or pure CRLF, otherwise the preferred
  ending (`endOfLine`, default: platform); `adjustLineEndingOnSave = !autoNormalizeLineEndings &&
  lineEnding !== 'lf'`. So a pure-CRLF file is converted back (unless `autoNormalizeLineEndings`
  is on), while mixed or CR-only files are written with one ending — i.e. normalised even if the
  document was never edited.
- The tab keeps the original text in `currentFile.markdown` until the first `json-change`
  event; then `desktop/src/renderer/src/components/editorWithTabs/editor.vue` replaces it with
  `muya.getMarkdown()` — the full serialization of the whole state tree — on every change batch.
  `FILE_SAVE` (`desktop/src/renderer/src/store/editor.ts`) writes `currentFile.markdown`.
  So, in the visual (Muya) editor: **never edited → the loaded text is written back** (byte-
  identical for pure-LF/pure-CRLF files with the default `autoNormalizeLineEndings: false`;
  mixed/CR-only endings normalised as above); **one edit anywhere → whole file regenerated**.
  Source-code mode is different: `sourceCode.vue` sends CodeMirror's `getValue()` to the store,
  so text edited there is saved as typed (no Muya re-serialization) (verified).
- Trailing newlines follow `trimTrailingNewline` (default 2 = "auto": remembers whether the file
  ended with none, one, or more newlines; `adjustTrailingNewlines` in the same store file).

### 1.2 What the serializer normalises (verified in `muya/src/state/stateToMarkdown.ts` and `markdownToState.ts`)

- Blank lines are not stored (`space` tokens dropped); one blank line is re-inserted between
  blocks where needed (`blockBoundary.ts`), so missing blank lines are added (heading→paragraph,
  paragraph→list …) and runs of blank lines collapse to one.
- Tables are regenerated: header/delimiter/body re-padded to the widest cell using display width
  (`utils/stringWidth.ts`), delimiter rows rewritten to `| --- |` / `|:--- |` / `| ---:|` form,
  outer pipes always added.
- List items: marker character and ordered delimiter are kept per list; ordered numbers are kept
  only while the item count and per-position marker strings still match the parsed list (`_shouldPreserveOrderMarkers`), else
  renumbered from `start`; continuation lines are re-indented to the item's content column; tabs
  become spaces; `[X]` becomes `[x]`.
- Paragraph text, ATX heading text (including `#` and closing hashes), setext underlines, thematic
  break text, link reference definitions (kept as paragraphs), fence character and length, and
  info strings are stored raw → emphasis characters, escapes and entities survive.
- Lazy blockquote continuation lines get an explicit `> `; trailing whitespace after a fence info
  string is trimmed; front matter loses leading whitespace.
- With the desktop default `trimUnnecessaryCodeBlockEmptyLines: true`, leading/trailing empty
  lines **inside** fenced code are deleted on load (`_buildCodeState`).

### 1.3 Measurement (measured (headless))

Method: 55 notes copied from the vault into the scratchpad (stratified by feature: tables 37,
front matter 39, nested lists 32, ordered lists 35, fences 23, quotes 20, wikilinks 30, task lists
5, `*`/`+` bullets 6 each, tabs 3; no CRLF file exists in the vault). Each copy was run through
Muya's `MarkdownToState` → `ExportMarkdown` with the desktop defaults (`listIndentation: 1`,
`frontMatter`, `trimUnnecessaryCodeBlockEmptyLines`, `footnote`/`githubAlerts` off; a second run
with both on gave the same counts), followed by the desktop's trailing-newline rule. This is a
**normalisation baseline under default settings**: it shows what the serializer does to the
untouched parts of a note once a visual edit triggers re-serialization; no edit/save was executed
in the app. The sample was selected for feature coverage, so the percentages describe this sample,
not the whole vault. Metric: lines of the original file covered by non-equal `difflib` opcodes
(an inserted blank line counts as one changed line). Markise: `markise-cli roundtrip` (release build of `main`).
Scripts and outputs stayed in the session scratchpad; the vault was only read.

| | Markise | MarkText |
|---|---|---|
| Files unchanged | **55 / 55** | 4 / 55 |
| Changed-line units (metric above) | 0 | ~1,382 against 15,001 original lines (9.2 % of this sample) |
| Second pass on MarkText output | — | stable (0 files change again) |

MarkText changes by category (a diff hunk counts once, in its first matching category):

| Category | Files | Lines |
|---|---|---|
| Tables re-padded / delimiter rewritten | 36 | 999 |
| Blank lines inserted (115 paragraph→list, 78 heading→paragraph, 46 heading→list, 22 paragraph→fence, 13 fence→paragraph, 13 list→list, 9 paragraph→HTML) | 40 | 308 |
| Continuation lines re-indented (lazy lines inside list items indented to the content column) | 10 | 53 |
| Tab-indented nested list → spaces | 1 | 5 |
| Blank lines removed (runs collapsed, blank line between lists with different markers dropped) | 3 | 3 |
| Whitespace-only line emptied | 1 | 1 |
| List markers, emphasis characters, escapes, ordered numbers, line endings | 0 | 0 |

A synthetic probe (`* [X]`, `1)` lists, three blank lines, lazy quote line, `|a|b|` table with
alignment, fence with trailing spaces) confirmed the extra rules of §1.2 (`[X]`→`[x]`,
`> ` added, blank runs collapsed, table re-padded, info string trimmed); Markise kept it
byte-identical.

**Semantic changes.** Rendering original and MarkText output with Markise's HTML exporter
differed for 1 file: a table directly after a paragraph line (shape: `Some text:\n| a | b |\n|---|---|…`, no blank line in between). Marked
(MarkText) parses it as a table and inserts a blank line, so the saved file now renders as a
table in Markise too. Under Markise's C31 the original is paragraph text. See §4 (C31).

**Interop note.** Because MarkText collapses blank-line runs, two adjacent same-type lists that
Markise keeps apart with two blank lines (C24) merge into one list after the file is edited in
MarkText (inferred from §1.2). Nothing to change in Markise; worth a line in user docs.

---

## 2. Architecture survey

- **Shell (verified).** Electron 42 + Vue 3 + Pinia; main process handles files
  (`desktop/src/main/filesystem/`: iconv-lite + `ced` encoding detection, chokidar watcher, durable
  atomic save via temp file + fsync + rename in `filesystem/index.ts`), menus, keybindings (`desktop/src/main/keyboard/`, user
  `keybindings.json` keyed by command id such as `format.strong`), PDF (`printToPDF`), Pandoc
  export. Source-code mode is CodeMirror 5 with cursor mapping (`sourceCode.vue`).
- **Engine (verified).** `Muya` replaces a host element with one `contenteditable` div. `Editor`
  merges DOM events with RxJS and routes them to the active leaf block's handlers
  (`muya/src/editor/index.ts`). Blocks are classes (`TreeNode → Parent → Content → Format`,
  `muya/src/block/base/`), each with `enterHandler`, `backspaceHandler`, `tabHandler`,
  `arrowHandler`, `inputHandler`.
- **State and edits (verified).** `JSONState` (`muya/src/state/index.ts`) holds the JSON tree;
  edits are `ot-json1` operations; `EditTransactions` (`muya/src/editor/transactions/README.md`)
  batches block mutations per animation frame, runs "editing rules" (list looseness) on the
  candidate document, supports nested `muya.transact()` scopes with rollback on exception.
- **Inline rendering (verified).** Own inline lexer (`muya/src/inlineRenderer/lexer.ts`, rules in
  `rules.ts`) → snabbdom vnodes. Markers stay in the DOM and are styled `mu-gray` (revealed) or
  `mu-hide`; a token is revealed when either selection end lies in `[start, end]` inclusive
  (`renderer/index.ts` `_checkConflicted`, `utils/index.ts` `conflict`). ATX `#` markers are
  revealed only when the caret touches the marker range (`renderer/header.ts`). Thematic breaks
  are editable `---` text.
- **Selection (verified).** Three kinds: text (block path + UTF-16 offset), image (content path +
  source range + raw text), table rectangle (`muya/src/selection/README.md`).
- **Performance shape (verified + measured (headless)).** No viewport virtualization or
  `content-visibility` (grep found none). Per change batch the desktop calls `getMarkdown()`
  (deep clone + full serialize), word count, `getTOC()` and `getState()` (another deep clone).
  Headless timings on a 10.1 MB concatenation of the notes: Muya parse 18–21 s, clone 0.1 s,
  serialize 0.12–0.2 s; 1 MB parse 0.21 s, 3 MB 1.9 s (≈ quadratic; hypothesis: the
  `tokens.shift()`/`unshift(...)` loop in `markdownToState.ts`). Markise `roundtrip` parse of
  the same 10.1 MB file: 17.8 ms. A per-keystroke full serialize at 10 MB (≥ 0.2 s) would miss
  NFR-4 by an order of magnitude (inferred; the Electron app itself was not run).

---

## 3. Editing behaviour: spec 02 vs MarkText

Legend for the last column: **=** same behaviour; **M+** MarkText better or covers a gap → adopt;
**S+** our spec/decision better → keep; **≠** different, neither clearly better.

### 3.1 Core model and input rules (spec 02 §1–§2)

| Spec | MarkText (source) | Verdict |
|---|---|---|
| §1.2 re-parse focused block per keystroke | Re-tokenizes the focused leaf and patches its vnodes (`format.ts` `inputHandler`); but the desktop serializes the whole document per batch (§2) | = engine; S+ app |
| §1.3 reveal when caret inside/adjacent | Inclusive range; **both** selection ends reveal (`renderer/index.ts`) | ≠ (ADR §2.4 assumes focus end only; MarkText's choice is a data point for that assumption) |
| §1.4–1.5 block markers hidden; "source on focus" pref | `#` shown when caret is at the marker; HR always editable text (`renderer/header.ts`, `thematicBreakContent`) | ≠ |
| §2.1 immediate rules: `> `, `- `, `1. `, `- [ ] ` | Same set, plus **ATX/setext headings, HR and 4-space indented code convert immediately** on every input. Scope differs per rule (`format.ts` `INLINE_UPDATE_FRAGMENTS`): bullet/ordered lists, ATX headings, quotes and thematic breaks are anchored `(?:^\|\n)` and also fire on a soft-broken line, splitting the paragraph (`_convertToList`, `_convertToThematicBreak`); task boxes `^\[[x ]\]` and indented code `^( {4,})` only at the start of the block's text; setext needs a preceding line by definition | **S+** (S-2.1, C9, A10/C38). MarkText's eager rules are the failure mode our hints avoid: typing `#` alone already makes a heading; 4 leading spaces make code |
| §2.1 auto-pair `* ` cleanup | Same (`content.ts` `collapsedInputAutoPair`) | = |
| §2.1 `[ ] ` in a bullet item | Same; also ordered items (ordered task lists) | ≠ (C29, §4) |
| §2.1 callouts `[!NOTE]` | On **Enter**, only in a top-level quote, feature default **off**; Backspace at the start of the first paragraph turns it back into a plain quote (`paragraphContent` `_matchBlockConversion`, `_handleBackspaceInGithubAlert`) | = (revert) / ≠ (trigger) |
| new list looseness | New lists are created **loose** (`preferLooseListItem` default true) → blank lines between items on save; a transaction rule keeps looseness consistent after structural edits (`editor/transactions/listLooseness.ts`, e2e `list-looseness-5297`) | **M+ gap**: spec says nothing about tight/loose for new lists or after Enter/join; proposal R11 |
| §2.2 Enter rules: fence, `$$`, `\| a \| b \|` | Same; HTML opener `<div>` + Enter → HTML block pre-filled with the closing tag; GitLab `>>>` fenced quote (`paragraphContent` `matchBlockConversion`) | = ; HTML pre-fill is a nice touch (P1) |
| §2.2 `[toc]`, footnote/link definitions, `---` front matter | No TOC block; definitions stay paragraphs; front matter via menu (TOML `+++` and JSON `;;;`/`{}` too) | S+ (coverage) |

### 3.2 Inline formatting and auto-pairing (spec 02 §3–§4)

| Spec | MarkText | Verdict |
|---|---|---|
| §3.3 toggle with selection; empty pair without | Same toggle; **selection is trimmed to its non-whitespace span before wrapping** (`format.ts` `format`, comment cites CommonMark flanking) | **M+ gap** (R8) |
| §3.3 multi-block selection | Formats each leaf block in the range (`muya.ts` `_formatAcrossBlocks`) | **M+ gap** (R8) |
| §3.3 highlight `==` | `<mark>…</mark>` (`config/index.ts` `FORMAT_TAG_MAP`) | S+ (shorter, our dialect) |
| §3.3 Hyperlink pre-fill from clipboard | Not found | S+ |
| §3.4 Enter inside a styled span (S-3.1) | Plain text split (`format.ts` `splitToParagraph`): `**very` / `bold**` | **S+** |
| paste URL over a selection | Wraps the selection as `[sel](url)`, escaping `[ ] \` in the label and `\ ( ) < >`/`\|` in the URL, only if the selection does not cut through tokens (`clipboard/pasteUrl.ts`) | **M+ gap** (R9) |
| §4.1 brackets pair only before whitespace/end | Same (`postIsNotTouching`) | = |
| §4.1 apostrophe after a letter not paired | ASCII-only test `[a-z\d]` → `м'` pairs | **S+** (incl. Cyrillic) |
| §4.2–4.4 step over closer, delete pair, wrap selection | Same (`deleteAutoPair`, `selectionPairForKey`) | = |
| §4.5 Markdown pairing default | Default **on** | S+ (off) |
| Tab at a closing marker | Tab jumps over the closing marker of the span (`paragraphContent` `_checkCursorAtEndFormat`) | M+ (P1 option, R13) |

### 3.3 Keys by block type (spec 02 §5)

| Spec | MarkText | Verdict |
|---|---|---|
| Enter: paragraph split / at end | Same | = |
| Enter in heading (C7) | End → paragraph; middle → heading + paragraph tail; start → empty paragraph above (`atxHeadingContent` `enterHandler`) | = C7 |
| Enter on an **empty heading** (`# `) | Becomes an empty paragraph | **M+ gap** (R2) |
| Shift+Enter in heading | Inserts `<br/>` because an ATX heading is one source line | **M+ gap**: spec §5.1 says "soft break (inline)", which Markdown cannot represent in an ATX heading (R3) |
| Shift+Enter on a line that is blank up to the caret | Treated as Enter (removes the soft break first) so a paragraph never contains a blank line (`format.ts` `dropSoftBreakBeforeCursor`) | **M+ gap** (R1); essential when text is the source of truth |
| Enter on empty list item, top level | Leaves the list; in a **middle** item the list is split into two lists around a new paragraph (`paragraphContent` `_enterInListItem`) | = / **M+ gap** for the middle case (R4) |
| Enter on empty **nested** item (S-5.2) | Becomes a plain paragraph inside the parent item (not a level-1 item) | **S+** |
| Enter on empty paragraph in a quote | Leaves the quote; in the middle the quote is split (`_enterInBlockQuote`) | = / **M+ gap** for the middle case (R4) |
| Code block Cmd+Enter at end | Shift+Enter from anywhere jumps to the next block or creates a paragraph (`codeBlockContent` `enterHandler`) | ≠ (consider both) |
| Code Enter auto-indent | Copies indent; between `{}`/`[]`/`()`/`><` opens an indented middle line | = (detail worth copying) |
| Table Enter | Moves to the **first cell** of the next row; last row → next block or new paragraph; Shift+Enter `<br/>`; Cmd/Ctrl+Enter inserts row (`tableCell` `enterHandler`) | S+ (same column) / = |
| Backspace rule 1 (table) | Same: empty table + first cell → table removed; else caret to previous cell | = |
| Backspace rule 2 (previous HR) | No HR rule; HR is a text leaf (inferred: Backspace merges into the `---` text) | S+ |
| Backspace rule 3 (list item with previous sibling) | Same: content appended to the previous item | = |
| Backspace at start of the **first** item | Item content moves out before the list (`_handleBackspaceInList`) | **M+ gap** (R5): spec rules 3–5 do not say what happens to the first item |
| Backspace at start of the first paragraph of a quote | Paragraph leaves the quote (unwrap) | **M+ gap/clarify** (R5): rule 5 "merge with previous block" is ambiguous for the first quote paragraph |
| Backspace rule 4 (heading, code → paragraph) | Same for headings; a **multi-line** code block also becomes one paragraph | S+ (spec: single-line only) |
| Backspace after an inline image | First Backspace selects the image, second deletes it (`format.ts` `_scanBackspaceTokens`, `_selectInlineImage`); a reference image is deleted as one token | **M+ gap** (R6) |
| Ctrl/Alt/Cmd+Backspace | Left to the platform (word/line delete) | = |
| **Delete at end of block** | Merges the next paragraph; next code/table → no-op; blocks after a merged list paragraph move up with it (`format.ts` `deleteHandler`, issues #1845, #5423) | **M+ gap** (R7): spec has no forward-delete rules |
| Tab in list (with selection) | Collapsed caret only | S+ |
| Tab in table at last cell | Leaves the table (no new row) (`tableCell` `tabHandler`) | S+ (S-6.1) |
| Tab in paragraph | Inserts `tabSize` spaces (default 4) | ≠ (our literal tab + C38 protection is safer) |
| Shift+Tab in paragraph | No-op | S+ |
| HTML code block Tab | Emmet-like `div.cls#id` → `<div class id></div>` | P2 idea |
| Arrow Down/Right at end of the last block | Creates a new empty paragraph at the end (`content.ts` `arrowHandler`, also from the last table row) | **M+ gap** (R12): pairs well with ADR's virtual empty paragraph |
| Up/Down goal column | Uses the caret x to pick the offset in the target block (`offsetInBlockAtPoint`) | = ADR `goal_x` |

### 3.4 Tables, code, other blocks (spec 02 §6–§9)

- **Tables (verified).** Floating toolbar, row/column drag bars, row/column menu, "chessboard"
  size picker, rectangular cell selection with clipboard (`muya/src/ui/table*`,
  `selection/TableRectSelection.ts`). Every save after an edit pretty-prints **all** tables
  (§1) — A8 is stricter and better. Padding uses display width (`stringWidth`, issue #1983) —
  adopt for our A8 pretty-print (R10); note it still misaligns some emoji (seen in one sampled note).
- **Code blocks (verified).** Prism highlighting, language input with icons, optional line
  numbers (default on), wrap option, fence char/length and full info string preserved.
- **Math/diagrams (verified).** KaTeX, Mermaid, Vega-Lite, PlantUML, flowchart.js,
  js-sequence; out of Markise scope until after M3.
- **Images (verified).** Toolbar, resize bar, edit tool, picker; images are atomic in selection.

### 3.5 Selection, undo, modes, paste (spec 02 §10–§14)

- **§10 selection commands:** none of Select Word/Line/Styled Scope/Block or Move Row; instead
  Duplicate, Create Paragraph, Delete Paragraph (`desktop/src/main/keyboard/keybindingsDarwin.ts`).
  S+.
- **§11 undo:** see summary row 7. `_lastRecorded` is set only when a group starts, so a group
  spans at most ~1 s from its first edit, independent of pauses — C13's pause rule is better.
  Whitespace insertion starts a new group (e2e "typed words split into separate undo steps
  (#3825)", "undo after a correction reverts only the correction") — **M+** (R15). Insert↔delete
  switch = ADR. History is kept per tab across tab switches. Replace-All undo not examined.
- **§12 modes:** source mode (CodeMirror 5, cursor/heading mapping), focus, typewriter, read-only
  exist. Smart punctuation (§13): not implemented.
- **§14 paste:** HTML → Markdown via turndown + GFM plugin (`state/htmlToMarkdown.ts`,
  `utils/turndownService/`), colspans expanded, Apple Numbers table HTML from `text/plain`
  recognised. For pasted links whose text is the bare URL, `getPageTitle` fetches the page to use
  its title (`utils/paste.ts`, `resolveLinkTitles` true for normal paste) — **a network request
  triggered by paste**; Markise should never do this implicitly (R14).
- **IME (verified):** while composing, `Format.inputHandler` returns early (no re-render); a
  zero-width-space anchor is seeded when composing at a block's trailing newline, and in empty
  table cells for Safari; an identity op after composition is not recorded (#4806). Same intent
  as ADR 0005 §4.4; reuse the e2e scenarios (pinyin in paragraph, list item, table cell; soft line
  break kept when the IME commits the second line).

---

## 4. Section-C decisions vs MarkText

| # | Our decision | MarkText | Verdict |
|---|---|---|---|
| C7 | Enter in heading | Same three cases (+ empty heading → paragraph) | = (+R2) |
| C8 | Repeated Cmd+L/D/E | Commands absent | n/a |
| C9 | Rules only at block start, not soft-broken lines | List, ATX heading, quote and thematic-break rules also match after `\n` and split the paragraph; task and indented-code rules are start-anchored | **S+** |
| C10 | No double-tap Cmd | Absent | = |
| C11 | Renumber only if sequential | Original markers are kept only while the item count and the per-position marker sequence still match the parsed list (`_shouldPreserveOrderMarkers` compares count and marker strings, not item identity); otherwise the whole list is renumbered from `start`. Inserting an item into `1. 1. 1.` gives `1. 2. 3. 4.`; swapping two items of `1. 1. 1.` keeps `1.` (reproduced in review) | **S+** for inserts/deletes |
| C12 | Quick Open ranking | Not examined | — |
| C13 | Pause > 1 s etc. | Fixed 1 s window from group start, + whitespace and kind switches | S+ pause; M+ whitespace (R15) |
| C14 | Indented code in list items per CommonMark | Not examined | — |
| C15 | `encoding_rs` + `chardetng` | iconv-lite + `ced`, keeps encoding and BOM | = |
| C16 | Setext per CommonMark | marked | = |
| C17 | Folder config P2 | Absent | = |
| C18 | Deleted file kept, unsaved | Watcher exists; behaviour not examined | — |
| C19 | No hard limit; notice > 50 MB | No limit, no virtualization, quadratic parse | **S+** (NFR-3) |
| C20 / C23 | Preserve line endings, mixed endings byte-identical | Pure LF/CRLF kept (CRLF converted back unless `autoNormalizeLineEndings`); mixed/CR-only normalised to the preferred ending on load, even without edits | **S+** |
| C21 | `language-xxx` classes, front matter hidden | `language-xxx` via marked's default code renderer (inferred; export code path not traced); front matter handling in export not examined | = (unverified) |
| C22 | GitHub gemoji | Own emoji table (`config/emojis.ts`) with picker | = |
| C24 | Two blank lines end lists | Blank-line runs collapsed → lists merge on MarkText save | ≠ (interop note §1.3) |
| C26 | Remap Unicode punctuation | Absent | — |
| C29 | Task boxes only on unordered items | Ordered task lists supported (`task-list` with `ordered: true`); pandoc's `gfm` reader renders `1. [ ] x` as text (verified with pandoc locally); GitHub and Obsidian behaviour unverified. The vault has **15** ordered task items | **Owner to revisit C29** (R16) |
| C31 | Table header must start its paragraph | marked/MarkText: a table may follow paragraph lines directly; pandoc `gfm`: paragraph text (verified locally, = Markise); cmark-gfm/GitHub and Obsidian unverified (believed to accept it). The vault has **4** such tables in 3 files | **Owner to revisit C31** (R16) |
| C35 | BOM stripped and written back | BOM detected and kept | = |
| C37 | Front matter `---` only | YAML `---`, TOML `+++`, JSON `;;;` or `{…}` | ≠ (TOML is common with Hugo; P2 candidate) |
| C38 | U+200B prefix protects 4-space lines | No protection; 4 leading spaces become indented code while typing | **S+** |

---

## 5. Spec gaps found (candidates for `99-open-questions.md`, section C)

**Accepted by the owner on 2026-10-27:** R1–R16 are recorded as C42–C57 in
`docs/spec/99-open-questions.md` (R*n* → C*(41+n)*), with behaviour statements and test scenarios;
R15 also amends ADR 0006 §4.2. R16's verification result: C31 kept; C29 superseded by owner decision (ordered task items
allowed, spec 01 R3.4.2 amended, code follow-up MT-1; see C57).

| R | Behaviour (from MarkText, own wording) | Proposed default for Markise |
|---|---|---|
| R1 | Shift+Enter on a line that is empty up to the caret | Act as Enter: drop the empty soft line and split the paragraph (a blank line cannot live inside a paragraph) |
| R2 | Enter on an empty heading | Convert to an empty paragraph (no new block) |
| R3 | Shift+Enter in an ATX heading | Insert `<br/>` (heading stays one source line); setext headings may take a real soft break. Fix spec 02 §5.1 table |
| R4 | Enter on an empty item in the middle of a list / empty paragraph in the middle of a quote | Split the container: items/paragraphs after it form a second list/quote; the empty block becomes a paragraph between them (lists separated per C24) |
| R5 | Backspace at the start of the first list item / first quote paragraph | Unwrap: the item's blocks (or the paragraph) move out before the container; clarify spec 02 §5.2 rules 3–5 |
| R6 | Backspace/Delete next to an atomic inline image | First press selects the image, second deletes the whole `![…](…)` source |
| R7 | Delete (forward) at the end of a block | Mirror of Backspace: merge the next paragraph into this one; next block code/table/math → no-op (or move caret); blocks that followed the merged paragraph inside a list item move with it |
| R8 | Format commands | Trim leading/trailing whitespace of the selection before wrapping; a selection spanning blocks formats each leaf block separately; no-op in code/math |
| R9 | Paste a single http(s) URL over a non-empty single-block selection | Produce `[selection](url)` (label `[`/`]`/`\` escaped, URL `( ) < > \` escaped), unless the selection cuts through a token |
| R10 | Pretty-printed tables (A8) | Pad by display width (East Asian Wide/Fullwidth = 2, combining marks = 0); document emoji handling |
| R11 | Looseness of new and edited lists | Lists created by typing or menu are tight (pref default); Enter/Backspace/indent keep the list's existing looseness; a structural edit never silently makes a tight list loose |
| R12 | Arrow Down / Right at the end of the last block | Move to a virtual empty paragraph after it (ADR 0006 §2.2 `VirtualBlock`), so users can escape a final table/code/math block without Cmd+Enter |
| R13 | Tab at a closing inline marker | Optional pref (default off): Tab steps over the closing marker(s) |
| R14 | Paste must not touch the network | No implicit fetch (page titles, remote images) on paste; any such feature is explicit and opt-in |
| R15 | Undo groups | Add "inserting whitespace after a non-whitespace run starts a new group" to C13/ADR §4.2 (A3 says word/run groups; current C13 is pause-only) |
| R16 | Dialect: C29 and C31 | Re-check against cmark-gfm (GitHub) and Obsidian, since the owner's vault uses both patterns (15 ordered task items, 4 tables after a paragraph line); if they accept them, consider flipping the decisions (they are parse-only; round trip stays byte-identical either way) |

---

## 6. MarkText regression tests worth turning into Markise tests

Each line is a scenario from a MarkText e2e title or code comment (issue numbers refer to
`marktext/marktext`); write our own tests from the scenario, do not copy test code.

- Split a list item at offset 0 must not leave an item without a paragraph (#4644).
- Delete at the end of a list paragraph followed by a nested list: the nested list moves up, no
  doubled bullet (#1845); Delete inside a quote keeps the rest quoted, also when the quote is
  inside a list item (#5423).
- Outdent inside `list item > quote > quote` must not move the quote into the outer list (#5342);
  blocks following a nested list stay the item's own content after outdent (#4899); a task item is
  never indented into a bullet list (#5349).
- Typing over a selection that covers the rest of a list/quote removes it cleanly (#5385); cross-
  block Enter replaces the selection with one paragraph break.
- Heading level commands on setext headings (#5362) and on multi-line paragraphs keep both lines
  (#5366); typing 7 `#` stays a paragraph.
- Undo: first undo after load restores the pre-input caret (#5258); words are separate steps and a
  correction undoes alone (#3825); select-all after undo (#5387); an IME composition that nets to
  no change records nothing (#4806).
- Arrow keys on soft-broken paragraphs move one visual line at a time, including onto a trailing
  blank line (#3203); arrow column kept across blocks (#3412).
- `&nbsp;` entity: round-trips, becomes editable source when the caret enters, copy/cut keep it.
- Backspace at the start of an ATX/setext heading converts once, then merges (#5388, #5035).
- Table: Delete in the last cell (#5386); paste/cut into a table cell (#5400); rectangle copy.
- IME: pinyin commit in paragraph, list item, non-empty table cell; soft line break kept when the
  IME commits the second line.

---

## 7. Reusable material and licensing

- **Code:** not recommended (TypeScript/DOM). If a small algorithm is ported (e.g. the display-
  width table padding or the URL/label escaping of R9), add a `NOTICE` entry: "Portions derived
  from MarkText (https://github.com/marktext/marktext), MIT License, Copyright (c) 2017-present
  Luo Ran, Copyright (c) 2018-present MarkText Contributors", and keep the MIT text.
- **Round-trip fixtures** `muya/test/spec/fixtures/marktext-round-trip/{common,gfm}/*.md`
  (11 files, MIT): Markise already round-trips all 11 byte-identically (measured). They are cheap
  extra corpus input; committing them needs a `SOURCE.md`/`NOTICE` entry (AGENTS.md asset rule).
  Fetching at test time is the lighter option.
- **Palette themes** (`desktop/src/renderer/src/assets/themes/*.theme.css`, 35 files) are part of
  the MIT repository but many reproduce third-party palettes (Nord, Dracula, Catppuccin …) with
  their own licenses; use only as importer test input, not as built-ins, unless each palette's
  license is checked.
- No GPL or unclear licenses were found in the files read; Muya's runtime dependencies are MIT/
  Apache/BSD-style (not audited in detail, unverified).

---

## 8. Prioritised recommendations

**M2 (editor, P0)**

1. Keep the architecture: source text + patches, literal-line hints, byte-identical writes. MarkText
   is the counter-example: in its visual editor one edit triggers whole-file re-serialization,
   which changed ~9 % of the lines of our 55-note sample (normalisation baseline). Add a test that
   edits one paragraph in each of a few real-world notes and asserts that only that paragraph's
   bytes change (the M2 counterpart of the M0 round-trip test).
2. Keep spec choices where MarkText regresses: Enter-only heading/HR/code rules (S-2.1), C9,
   C38/A10, S-3.1 marker repair, C11 renumbering, A8 tables, C20/C23 line endings, Cyrillic-aware
   apostrophes, Markdown pairing off.
3. Accept R1–R7 and R11–R12 into section C before M2c (they are cheap to specify and each is a
   bug class MarkText has had to fix); add R15 to ADR 0006 §4.2.
4. Accept R8 (format whitespace trim, cross-block formatting) and R9 (URL paste → link) for M2c/M2d.
5. Write the §6 scenarios as M2 tests (headless engine tests where possible, kittest for keys).
6. Budget guard: never serialize or deep-clone the whole document per keystroke for word count,
   TOC or dirty tracking (MarkText does all three); derive them incrementally or off-thread
   (consistent with ADR 0006 §7).

**M3 (P1)**

7. R10 display-width padding when A8 pretty-prints tables; R13 Tab-out option; HTML-block Enter
   pre-fill of the closing tag; code-block bracket auto-indent.
8. R14 as a rule for spec 04 paste/image features.
9. Table UX references: rectangular cell selection with clipboard, drag handles, size picker
   (spec 02 §6 already asks for these; MarkText shows they are worth it).
10. R16: owner decision on C29/C31 after checking GitHub and Obsidian.

**Later**

11. TOML/JSON front matter (C37), heading folding, slash insert menu and block handle menu,
    Emmet-like tag expansion — product ideas, not spec requirements.

---

## Reproduction

All work ran in the session scratchpad (not committed): shallow clone of MarkText; a harness
directory inside the clone with `npm install --ignore-scripts` of `marked@18.1.1`,
`html-tags@5.1.0`, `dompurify@3.4.16`, `katex@0.19.0`, `marked-highlight@2.2.4`,
`prismjs@1.30.0`, `tsx@4.23.15` (versions from MarkText's `pnpm-lock.yaml`), symlinked as
`packages/muya/node_modules`; a small `tsx` script importing `muya/src/state/markdownToState.ts`
and `stateToMarkdown.ts`, applying the desktop's line-ending and trailing-newline rules; a Python
diff classifier; `markise-cli roundtrip` and `markise-cli html` from a release build of Markise
`main` (`e04cf2d`). The 55 notes were copied read-only from the vault; nothing was written to it.
