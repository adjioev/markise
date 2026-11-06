# ADR 0025: Extended callouts

Status: accepted for implementation.

## Context

A blockquote can start with a GitHub alert, `> [!NOTE]`: five fixed names, no title, nothing to
fold. People write more than that in their notes: any type name, a title of their own, and a body
that can be folded away.

```markdown
> [!warning] Optional title
> Body.

> [!success]- Starts closed, with a title
> Hidden until opened.

> [!info]+ Starts open
> Body.
```

The owner asked for this as a module of its own, not spread through the parser.

## Decision

**A module of `markise-md`.** `crates/markise-md/src/callout.rs` holds the grammar of the first
line, the table of names, default titles and the neutral view. The parser has one hook,
`callout::recognise`, called from the two places that finish a quote (`finish` and `refinish`,
which already held the alert check), and the tree stores one small `Copy` value on the quote
(`BlockKind::BlockQuote { callout }`): type, fold, alert kind. It holds no text or spans, so a shift
never touches it and an edit that changes it is seen by the existing computed-state check of
incremental parses. It is not a crate of its own: the hook needs `ParseOptions` and `Block`, so such
a crate would sit below `markise-md` with nothing else above it, for one module's worth of code.
Layout reads callouts through the format seam and exports through `markise-md`, as they read
everything else.

**The option.** `ParseOptions::callouts`, default on. Off, every extended form stays a plain quote,
as it was. `alerts` keeps governing the bare forms of the five GitHub names.

**The grammar.** The first line of a quote's first paragraph: `[!name]`, then `-` or `+` at once,
then, after white space, the title (inline Markdown). A name is letters, digits, `-` and `_`,
starting with a letter or digit, at most 64 bytes. Text right after the tag is not a callout, so
`[!x](url)` stays a link. A literal-line hint keeps a line plain, as for any input rule.

**Alerts are the narrow case.** The bare tag of a GitHub name recognises as `Callout { alert:
Some(kind) }`: same bytes, same HTML, same Pandoc, same PDF and same drawing as before, including no
icon. `[!NOTE] Title` and `[!NOTE]-` are extended forms.

**Types.** Names and aliases map to a type, the type to a colour and an icon. Colours are the theme's
five alert colours and its quote bar, so every style, palette and imported theme has them without a
new theme field. An unknown name has the note style and shows its name. The default title is the
name, lower case, capitalised.

**The seam.** `FlatContainer::Quote` carries a shared `CalloutView`: accent, icon, written title and
tag end as offsets into the first line, and the default title. Offsets into the line keep it
independent of the block's position, and nothing in it depends on the body: typing there does not
invalidate the header's cached layout. Layout keeps the accent on every leaf of the quote and the
view on the first leaf only (`Container::Quote::head`, spent like a list marker).

**Editing.** The tag is syntax and never shows outside source mode; a written title is ordinary text
with caret stops. Backspace at the start of a title removes the tag; on the first body line of a
callout without a title it removes the tag line, as for an alert. A valid `[!name]` typed as a
quote's first line converts when its `]` is typed, as for an alert, with the caret in a fresh body. A
title or fold mark is added in source mode or by hand-editing the file: the rule fires before they
can be typed.

**Icons** are drawn from primitives in `markise-layout/src/callout.rs` (original, no assets).

## Exports

An extended callout is `ExBlock::Callout` in the export model, with its title resolved to inlines.
Bare alerts keep `ExBlock::Alert` and every byte of their output.

| Target | Foldable | Not foldable |
|---|---|---|
| HTML | `<details class="…"><summary>title</summary>…</details>`, `open` for `+` | `<div class="…"><p>title</p>…</div>` |
| PDF, PNG | open, bar in the accent colour, bold title, no icon | the same |
| Pandoc | `Div` classes `callout` and the type, attributes `callout` (the name) and `fold`, a `title` `Div` first | the same without `fold` |

HTML classes are `md-alert md-alert-<accent> md-callout md-callout-<type> <type>`, and the title has
`md-alert-text-<accent>`. They reuse the theme's alert rules, so no CSS is added and a page without
callouts is byte-identical to before. A `quote` type has no alert colour there and uses the note
colour; an unknown name has `data-callout` with its name, escaped.

## Folding

The state is view state, kept by source position beside the disclosures' in `CrossBlockIndex`: in
memory, for the session, never in the file, with no dirty state and no undo entry. It survives edits
elsewhere, eviction of the page and trips to source mode; reopening the file starts as written.

A fold reuses what `<details>` has: `Disclosures` holds its hidden range and its group, so zero
heights, caret traversal that jumps over the body, the frame's skipping and `toggle` in a
`BlockLayout` all work as they do. It differs in three ways, so it is a `FoldMarker` list of its own
rather than a `RawPresentation`:

- **No pairing.** A fold knows its extent from the format's containers (`containers_at`, innermost).
  An edit inside it does not re-pair anything; its extent is read again, for the folds starting
  before the edit in the re-parsed top-level blocks. That reaches a quote the edit ended above itself,
  a lazy line turned into a block of its own, and the view derives what is hidden again when an end
  moved other than with the text. An edit outside touches nothing per fold.
- **Its first leaf stays.** The header is the quote's first paragraph, also holding body lines, which
  a closed callout does not draw. Later segments of that paragraph are hidden like the rest.
- **Nothing is a control and nothing is indented.** The quote is its own frame.

Clicking the header toggles, chevron or not; the whole header is the control, so a click there sets no
caret. The caret reaches the title with the arrow keys. A caret entering a closed callout opens it,
the title included; so does a search hit, or an edit such as an undo landing there. A document opening
with the caret at offset 0, on a closed callout, has not gone in: it stays closed. Closing a
callout around the caret moves the caret to the next thing shown, so nothing is typed out of sight.
A closed callout without a title offers no caret stop at all, since a stop in its body could be typed
into: the arrow keys pass it, and the click or a search opens it.

The reader's choices follow each patch of a composed edit, such as Replace All, not their covering
range, up to the session's limit of 256 position steps. Beyond that, a choice inside the covering
range goes back to as written.

**Keyboard and accessibility** are those of `<details>`: the caret opens it, and there is no key to
close it and no accessibility node for the control. Both kinds would gain these together.

**Heights.** A closed header's estimate is its line. A placeholder page takes off the bodies of the
closed folds starting in it, each counted by `PageStats` as its items are when shown, less the line the
header keeps. A body's statistics are taken from its items when a marker is made and again when its
page is evicted, the two times a placeholder is made with the items in hand. A measured page height
depends on `DepGens::disclosures` as for `<details>`, which a toggle bumps.

## Measurement (2026-11-06)

`heavy_suite_many_closed_callouts_do_not_regress`, local release build, Apple Silicon, other builds
running at times. Paged model of 128 blocks a page, viewport 800 points. Baseline: the same bytes with
the tag spoiled, so they are plain quotes. Two runs gave the same picture; numbers from the second.

| | 1,000 callouts | 10,000 callouts |
|---|---|---|
| Model build, baseline → callouts | 0.9 → 1.3 ms | 7.2 → 12.7 ms |
| Keystroke outside (model, view, frame), p50 | 0.027 → 0.027 ms | 0.311 → 0.264 ms |
| Scroll frame, p50 | 0.069 → 0.048 ms | 0.069 → 0.052 ms |
| Toggle and frame, p50 | 0.072 ms | 0.524 ms |
| Keystroke in the first leaf of an open callout, p50 | 0.082 ms | 0.807 ms |
| Total height, baseline → callouts | 139,571 → 78,285 pt | 1,385,171 → 755,037 pt |

The keystroke rows include the test's copy of the document per edit, which rebuilds the reference
context because the text holds `[`; the editor edits in place. A keystroke in a first leaf derives the
fold list again, O(folds), still far inside the 16 ms budget. The model build costs about 0.6 µs a
fold. Criterion cases `closed-callouts-frame-*` and `callout-toggle-*` are in the layout bench.

Rerun the same day after page estimates moved to counting hidden blocks, with more load on the machine:
10,000 callouts gave keystroke outside 0.412 → 0.298 ms, scroll frame 0.067 → 0.052 ms, toggle and frame
0.686 ms, keystroke in a first leaf 0.983 ms, the same picture. The model build, 59.5 → 46.8 ms in that
run, was dominated by the load and says nothing of the difference.

## Limits

- A `[!name]` that is also a defined reference label reads as a callout, not a link.
- Pictures in a title are not drawn.
- PDF and PNG exports do not draw the icon.
- In a list, the bullet of an item that starts with a callout sits beside the first body line, not the
  header. A bare alert does the same.
