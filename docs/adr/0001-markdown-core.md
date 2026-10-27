# ADR 0001 — Markdown core: own block CST over the source text, not comrak/pulldown-cmark

Status: accepted for M0 (2026-10-26). Scope: `crates/markise-md` (layer L1).

## Context

Spec 01 requires that an unchanged document is written back **byte-identically** (R1.2–R1.4,
R3.8.3): line endings, final newline, emphasis/bullet/fence/rule markers, blank-line counts,
table padding, link-definition spacing, front-matter closer. Edits must only touch what the
user changed and must reuse the block's original "pattern" (R1.3). The performance budgets
(NFR-3/4/8) require that a keystroke in a 10 MB document re-parses only the affected block(s).

The dialect also deviates from GFM in ways that change *block structure*, not just rendering:
non-strict `#Title` headings (R3.2.1), a 4-space indented line interrupts a paragraph (R6.1),
quotes deeper than 10 levels are text (R3.3.1), `$$`/`\[` math blocks that need a closing line
(R3.7.1–3), front matter only on line 1 with a `---`/`...` closer (R3.10), `[toc]` blocks
(R3.11), the block-HTML tag list of R3.14.1, GitHub alerts (R3.3.4), optional Unicode
punctuation remapping (R4.5.2), and list separation by two blank lines (R3.4.7, decision C24).

## Options considered

1. **comrak** (GFM AST with `sourcepos`) for everything.
2. **pulldown-cmark** (event stream with offsets) for everything.
3. **comrak/pulldown-cmark for HTML export + our own CST for round-trip.**
4. **Own block CST over the source text + own inline parser** (chosen).

## Why not comrak / pulldown-cmark (concrete limitations)

- **Lossless round-trip.** Both produce an AST/event stream that normalises source: comrak's
  CommonMark formatter re-emits its own canonical markers, spacing and table layout; neither
  keeps blank-line counts, `__` vs `**`, bullet char per list, fence length, setext underline
  length, table cell padding, link-definition spacing or front-matter closer. We would have to
  serialize from source slices anyway, so the library would only provide spans.
- **Source spans.** comrak's `sourcepos` is line/column based (no byte offsets), and is
  imprecise for some nodes (container prefixes, lazy lines, tables); pulldown-cmark offsets are
  per event and container prefixes inside quotes/lists are not attributed to any node. Our
  editor needs exact byte spans per block *and* per content line (for caret mapping later).
- **Incremental re-parse.** Neither library can re-parse a region and splice it into an
  existing tree; both parse the whole document (comrak builds a full arena AST). Restarting a
  parse in the middle of a document and detecting re-synchronisation needs control over the
  parser state, which only an own parser gives.
- **Dialect.** The block-structure deviations listed above cannot be expressed through either
  library's extension flags (e.g. comrak has no "4-space indent interrupts a paragraph", no
  quote-depth limit, no closing-line-required `$$` blocks, no `\[ \]` blocks, no `[toc]`, a
  different HTML-block tag list; math options differ from R4.2.1; no sub/sup/highlight with the
  R4.3.1 whitespace rule; no remapping). Forking them to change block rules means owning a large
  foreign parser.
- **Two parsers disagree.** Option 3 (comrak for HTML, own CST for editing) would render a
  structure different from the one the editor shows for exactly the dialect deviations above
  (`#Title`, deep quotes, math blocks, two-blank-line list split…). The export must use the
  same tree as the editor.
- **Performance.** Our block parser does not allocate per inline node and does not build an
  inline AST during document open: the 10 MB corpus parses in ~30 ms (release, M-series), while
  a full comrak AST including inlines costs several times that and much more memory. Inline
  parsing is lazy and per block (viewport / export only).

## Decision

- `Document` owns the Markdown text; it is the single source of truth. The block tree is an
  index of byte spans into it (`Block { kind, span, lines, children }`). `serialize()` is the
  concatenation of block sources and inter-block trivia, so an unedited document round-trips
  byte-identically by construction (verified by tests over an authored corpus, generated
  corpora incl. CRLF/CJK, and property tests).
- Edits are text patches (`Document::edit`). Structural operations (`ops`) compute minimal
  patches from the recorded pattern (toggle `[x]`, add list item with the list's bullet and
  numbering style, lengthen a fence, regenerate only an edited table, …). New blocks come from
  `write` generators that apply the spec 01 §5 preferences (R1.3). Inserted text is converted
  to the document's line ending (R2.2).
- Block parsing follows the container/leaf "parsing strategy" of the CommonMark spec, adapted to
  spec 01. Each top-level block records whether the parser had no open block at its first line
  (`clean_start`). After an edit, re-parsing starts at the last clean block before the edit and
  stops at the first clean line after the edit where an old clean block starts (then the old
  tail is shifted and reused). Failed look-aheads (unclosed `$$`, front matter without closer)
  are remembered so an edit that completes them re-parses from the opener. Property tests check
  incremental == full parse.
- Inline parsing (`inline`) implements the CommonMark delimiter-run algorithm plus the spec 01
  §4 extensions and runs on demand for one block's content.
- HTML export (`markise-export`) renders from this same tree.

## Consequences

- We own CommonMark/GFM conformance. `crates/markise-export/tests/conformance.rs` renders the
  CommonMark 0.31.2 examples and the GFM extension examples through the production export
  (`render_body`, `trusted` so raw HTML is kept as the spec expects) and compares after a
  normalisation that only strips the spec 03 presentation (classes, ids, `lang`, table
  `<figure>`, `text-align` style vs `align`). Spec files are fetched by
  `scripts/fetch-spec-tests.sh` (CC-BY-SA, not committed); the test prints a visible skip notice
  without them and fails when `CI=true`. Result: 620/652 CommonMark (95.1%) and 19/24 GFM
  extension examples; every remaining failure is a listed dialect decision (R6.1, C24, C27, C33,
  R3.10, spec 01 §4, C40, C41) or a named gap. With the untrusted (sanitized) default, 606/676
  examples render identically.
- Robustness: nesting is capped (C39) and every look-ahead/delimiter search that can fail is
  memoised, so adversarial inputs stay linear (`tests/adversarial.rs`, bench group
  `md-adversarial`).
- Shifting spans after an edit is O(blocks after the edit); `String::replace_range` is O(n).
  Both are ~1–2 ms on 10 MB; if NFR-4 measurements in M2 show pressure, move to relative spans
  and a rope in `markise-doc`.
- No third-party Markdown code is vendored; nothing to add to `NOTICE` for the parser.

## Measurements (M0, after the review follow-up)

`cargo bench -p markise-md` (release, Apple M-series dev machine, generated corpus):

| Benchmark | Time | Budget it feeds |
|---|---|---|
| parse 1 MB / 10 MB | 3.4 ms / 35.7 ms | NFR-2 (< 200 ms) / NFR-3 (< 1 s) to first screen |
| serialize 1 MB / 10 MB | 0.11 ms / 1.6 ms | NFR-8 save < 100 ms (excluding I/O) |
| keystroke (insert + delete) in 10 MB | 2.4 ms | NFR-4 16 ms per frame |
| inline parse of every block, 1 MB | 13.9 ms | lazy; per viewport in practice |
| adversarial inputs (16 kinds, 25k repetitions) | 0.2–11 ms each | linear-time guard |
