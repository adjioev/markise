# ADR 0011 — One block-ownership query for every layer

Status: **proposed** (2026-10-28). Scope: `markise-doc` (L1, the query and its rule),
`markise-md` (L1, the tree-based implementation), `markise-engine` (L4), `markise-layout` (L2,
format seam), the toolbar's active block (via the format's `plan_checked`). Builds on ADR 0006
(format seam).

## Context

Several editing bugs (Space / Enter / Backspace / Delete at a heading end or in its trailing
spaces, the caret jumping to the next block, the toolbar showing the wrong block type) had one
cause: each layer decided for itself which block a source offset belongs to. An ATX heading's
span stops at its text, so the spaces typed after it lie *between* blocks for `FormatDoc::leaf`,
and each consumer patched that differently — some by scanning lines backwards across blank
lines and `>` prefixes, which can skip a whole block (an empty quote, an empty list item).

### Survey: ownership decisions before this ADR

| # | Where | Decision | Method |
|---|---|---|---|
| S1 | `markise-md` `format.rs` `leaf_before` | "the leaf before `at`" for 8 callers | line scan backwards over blanks, spaces and `>` |
| S2 | `markise-md` `format.rs` `heading_end_before_ws` | caret in a heading's trailing whitespace = its end (Enter, empty heading, block commands, C48, inline commands) | line scan + `leaf` |
| S3 | `markise-md` `format.rs` `adjacent_leaf_tree` | neighbour for joins (C48, BE-3, rule 5) | block tree (kept, generalised) |
| S4 | `markise-md` hinted-heading Enter (C43 on a literal `## ` line), link-definition Enter, Enter at a block start, `typing_virtual` prefix | where the virtual paragraph goes | S1 |
| S5 | `markise-md` `edit/blocks.rs` `item_paragraph_join`, first-empty-item Enter, `plan_soft_break` | join target / virtual paragraph / the leaf of an empty soft line | S1 |
| S6 | `markise-md` `edit/select.rs` Delete Block (final block) | where the separator before the last block starts | S1 |
| S7 | `markise-md` `edit/rules.rs` `soft_line_hint` | the paragraph an empty line continues | S1 |
| S8 | `markise-md` `edit/blocks.rs` `block_checked` (toolbar / menu check marks) + `heading_block_request` | the caret's block | `leaf` + S2 |
| S9 | `markise-doc` `FormatDoc::adjacent_leaf` default | neighbour | line scan (kept as the default for formats without a tree) |
| S10 | `markise-engine` `in_heading_trailing_ws`, `block_end_forward` | trailing whitespace of a heading; Delete at its end | byte scan + `leaf` |
| S11 | `markise-engine` `block_syntax`, `merge_blocks` | a leaf's syntax before / after its content | own arithmetic on `LeafView` |
| S12 | `markise-engine` `leaf_info` (typing groups), `expire_hints_left_behind` | the caret's block | `leaf` |
| S13 | `markise-engine` `past_trailing_ws` (End) | a paragraph's trailing whitespace | byte scan + `leaf` |
| S14 | `markise-layout` `caret::item_pos` | the item of an offset in the gap after a heading | byte scan over the gap |
| S15 | `markise-layout` `caret::focus_for` (`ItemFocus.trailing_ws` / `trailing_len`) | where the shown trailing whitespace starts | rendered-content end + byte scan |

Not ownership decisions (unchanged, listed because they were suspects): `clamp_selection` /
`snap_selection` (char-boundary clamping of untrusted selections), `widen_whole_document_selection`
and `widen_from_heading_start` (they ask whether a selection lies *inside* the first / last leaf:
containment, `leaf`), `content_bounds` / `deletion_allowed` (the content lines of the caret's
leaf), table-cell lookups (`table_row_cells`), list structure (`list_ctx`, `item_at`: Markdown list
syntax, below the seam).

## Decision

1. **One query, in the seam.** `markise-doc::owner` defines the rule and the types;
   `FormatDoc` gains
   - `block_owner(at) -> Option<BlockOwner>` — the owning leaf (a `BlockLeaf`: kind, span,
     container chain, hint flag — **without its lines**, see Performance) and the part of it `at` is in
     (`Prefix`, `Marker`, `Text`, `LineBreak`, `TrailingWs`, `Closing`) with the ranges
     `line` (the content line holding `at`), `prefix` (container prefix on the first line), `marker` (the leaf's own leading syntax),
     `content` (first content byte … end of the last content line), `trailing_ws` (the spaces and
     tabs ending a paragraph's / heading's text, wherever they are) and `closing`;
   - `block_owner_biased(at, Affinity)` — the same, with offsets between blocks resolved to the
     leaf before / after them (part `Gap`), never across a block without text;
   - `adjacent_block(at, forward) -> Option<Adjacent>` — the sibling block in document order
     (`block`: its span at the level where it was found, possibly a whole container; `leaf`: the
     text leaf at its near edge as a `BlockLeaf`, `None` for an empty quote, an empty item…);
   - `containers_at(at)` — the container chain (kind + span), also between blocks;
   - `adjacent_leaf` is now a provided method on top of `adjacent_block`.
   `LeafView` gains `content()`, `leading_syntax()`, `trailing_syntax()`.
2. **The rule** (documented once, in `markise_doc::owner`): an offset is owned by a leaf when
   it lies (1) inside the leaf — from its first byte to the end of its span or of its last
   content line, ends inclusive; (2) in the spaces and tabs that end the leaf's last line after
   that, up to and including the line end; (3) in the container prefix on the leaf's first line.
   Every other offset is between blocks: a line break after a block, blank lines, a container
   line without a leaf (`>`). A position at a block's end before its line break is that block's.
   This matches the owner's block-edge decisions: BE-1 (trailing spaces belong to the heading
   / paragraph and the caret is drawn after them), BE-2 (`trailing_ws` is exactly "only
   whitespace after the caret"), BE-3 / C48 (neighbours from the structure: a join or a removal
   never reaches past the next block), BE-4 (marker / content ranges for the heading-start
   widening).
3. **Format-specific implementation in the format crate.** `markise-md` answers from the block
   tree: `block_path_at` (binary search over the top-level blocks, then over each level's
   children), the sibling at the innermost level that has one, then down to the edge leaf.
   O(log n + depth); no line scanning across blocks. The trait defaults (for formats without a
   tree, e.g. `markise-testfmt`) use `leaf` plus a scan of the offset's own line only.
4. **Layout reads it through the seam only** (`markise-layout` is `[format_seam]`): `item_pos`
   and `focus_for` call `FormatDoc::block_owner`.
5. **`leaf(at)` stays** as the containment primitive the query is built on ("the leaf whose
   span contains `at`"); code that asks about containment keeps using it, code that asks "whose
   is this offset" uses `block_owner`.

## Migrated call sites

S1 removed (`leaf_before` deleted); S2 now a thin helper on `block_owner`; S3 generalised into
`adjacent_block`; S4–S7 use `adjacent_block` / `block_owner_biased`; S8 `block_checked` uses
`block_owner` (the redundant `heading_block_request` step in `plan_checked` removed; it stays for
planning, where the caret must be moved into the span); S10 `in_heading_trailing_ws` and
`block_end_forward` use `BlockOwner::trailing_ws`; S11 `block_syntax` uses the new `LeafView`
methods; S12 uses `block_owner`; S13 uses `BlockOwner::trailing_ws`; S14 / S15 use
`block_owner`.

## Behaviour changes

None intended except four cross-block fixes (Codex review of #18), each with a regression test
(`markise-engine/tests/block_ownership.rs`, kittest `cross_block_risks_block_ownership`):

1. **Hinted-heading Enter** (C43 on `## ` typed as paragraph text): an empty quote before the
   line is no longer deleted — the markers go and the empty paragraph is the virtual one right
   after the quote, in place of the line and at the line's own container level (C43; inside an
   enclosing quote / list item it stays there); after a quote / list the virtual paragraph
   leaves that container instead of opening inside it.
2. **List-item paragraph join** (Backspace at an item's later paragraph): stops at an empty quote
   or empty nested item between the paragraphs (it deleted them).
3. **Final-block deletion** (Delete Paragraph / Block on the last block): the separator is taken
   up to the previous block's end, never into an empty quote before it.
4. **Virtual-paragraph placement**: Enter at the start of a block after an empty quote no longer
   opens the paragraph before the quote; Enter on an empty first list item after a quote opens it
   outside the quote.

## Performance

The query is O(log n + depth) per call (no per-keystroke document scan, NFR-4), also when the
owning block is huge: `block_owner` / `block_owner_biased` / `adjacent_block` make no
per-query copy of block content (only small allocations for the block path, the container
chain and a code block's info string), in particular never of a block's content lines (one paragraph or code fence can span the whole document) — the Markdown
implementation binary-searches the lines in place (`BlockOwner::resolve_lines` borrows them,
relative to the block's origin). Planners that need every line ask `FormatDoc::leaf` for the
full `LeafView` explicitly (Shift+Enter, the soft-line hint, typing on the virtual paragraph,
joins). Microbenchmark `markise-md/benches/owner.rs`: 1024 queries over 1 MB / 10 MB mixed
documents, a 10 MB single paragraph with soft line breaks and a 100k-line code fence (numbers in
the PR).

## Open questions

- A caret on the virtual paragraph (`virt > 0`) is owned by the leaf before it, so the toolbar
  shows that leaf's type (e.g. "Heading 1" right after Enter at a heading end) — main's
  behaviour, kept. Should the virtual paragraph count as a paragraph for the toolbar?
- `containers_at` reports the containers whose span holds the offset; an owned offset after a
  container's last leaf (trailing spaces of `- # T··`) is outside the list's span, so the chain
  is empty there while `BlockOwner::leaf.containers` lists the list. Callers needing the leaf's
  chain use the latter.
