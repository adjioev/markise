# ADR 0018: fenced directives (`:::name` containers)

Status: accepted for issue #178, phase 1.

## Context

Documents can hold generic block containers written with colon fences:

```text
:::kind some arguments

## A heading

Ordinary Markdown: paragraphs, task lists, quotes, images.

:::
```

Markise must parse these as containers with no knowledge of any particular name, keep files
byte-identical, change only the edited range on an edit, and stay fast on large files. A
container can span any amount of text, and whether a line opens one can depend on a line very
far below it. That is the hard part: a parser that looks ahead for the closer, or that stores
the container as one node holding its content, pays for that distance on keystrokes.

## Decision

### Syntax

- A **marker line** is at the document level (see below), indented at most three columns, and
  starts with at least three colons (the *fence*).
- **Closer:** nothing but whitespace after the colons.
- **Opener:** directly after the colons a **name**, letters and digits of any script plus `-`
  and `_`, starting with a letter or digit; then optionally whitespace and **arguments**
  (trimmed, otherwise uninterpreted). Anything else is not a marker: `::: note`, `:::note:`,
  `:::note{x}` are ordinary text.
- No blank lines are needed around marker lines: a marker line ends an open paragraph, table,
  list or quote, as a heading does.
- Every syntactically valid name is a container. Markise has no list of names, so an *unknown
  name* does not exist as a case here.

### Where markers count

Only at the document level and between other markers. Inside a block quote, a list item or a
footnote definition a marker line is ordinary text and closes nothing. Inside fenced or
indented code, math, raw HTML and front matter it is what those blocks hold. These contexts are
decided by the parser in its normal way, so `:::` in a code fence is code.

Allowing containers in quotes and list items would add line prefixes to their markers, and with
them pairing per container and restart state per level, for no document we know of. It stays
possible later: such lines are text today, so the change would be an extension.

### Pairing

Pairing is a pure function of the sequence of marker lines, `(fence, opener or closer)`, in
document order:

- A new opener ends every waiting opener of the same or a shorter fence (they stay text), then
  waits.
- A closer closes the **outermost** waiting opener whose fence is not longer than its own; the
  openers inside that one stay text. With no such opener it stays text.

The results:

- **Nesting:** a longer outer fence holds shorter ones (`::::a` … `:::b` … `:::` … `::::`).
  Waiting openers are bounded by `MAX_DIRECTIVE_DEPTH` = 32; an opener beyond it stays text.
- **Same fence, or a longer fence inside a shorter one, does not nest:** the earlier opener is
  taken as the forgotten one. In `:::a`, text, `:::b`, text, `:::`, block `b` is a container and
  `:::a` is text. The first closer does not close `a` around `b`, so a forgotten closer harms
  only its own block, not the next one. The throwaway prototype paired an opener with the next
  bare `:::` and showed the other result, swallowing the next block; there is no nesting of
  equal fences in either.
- **An unclosed opener** stays ordinary text and takes nothing with it.
- **A bare `:::` with no opener** stays ordinary text.

A marker line that pairs with nothing is a paragraph **of its own**: it does not join a
neighbouring paragraph (`foo`, `:::a`, `bar` are three paragraphs, a small change from plain
Markdown for prose that happens to start a line with three colons). This is deliberate. It
makes the rule below possible.

### A container never changes how its content parses

Lines are parsed the same whether a marker pairs or not, since a marker line is a block of its
own in both cases and a container adds no line prefix. Therefore:

- **The stored top-level list stays flat.** `Document::tops()`, `blocks()`, `block_path_at` and
  the other paths see the marker lines and the content as siblings. A paired marker is a
  `BlockKind::DirectiveOpen` or `DirectiveClose` leaf, an unpaired one a `Paragraph`.
- **`Document::tree()` is the container view.** It folds the flat list into
  `BlockKind::Directive` nodes holding their content as children, O(document). The queries
  `directives()` and `directives_at(pos)` answer from the index without folding.
- **Pairs live in a document-wide index** of marker lines (`directive.rs`): a chunked
  relative-offset list like the top-level blocks, where partners refer to each other by index.
- **There is no look-ahead for a closer, and no restart at an earlier failed opener.** An edit
  re-parses its own region as usual. If the marker sequence of that region is unchanged the
  pairs are unchanged, and the later markers only shift, O(chunks). If it changed, pairs are
  recomputed from the sequence, O(markers), and only marker blocks whose pair changed are
  relabelled. Nothing between an opener and a far-away closer is parsed again.
- **Pages may split a container.** Paged models (ADR 0016) work on top-level blocks, so a large
  container does not become one page, the same way separate `<details>` markers cross pages
  (`CrossBlockIndex`).

The `EditResult` of an edit that changed a pair reports every top-level block from the earliest
to the latest marker whose pair changed: the blocks are the same, but what surrounds them is
not.

### Alternatives

- **A stored `Directive` node with children.** The natural shape, and what `tree()` shows. As
  storage it makes typing an opener above a far closer re-parse or move everything in between;
  typing an opener with no closer needs a scan to the end of the text per keystroke, or a
  restart at every failed opener for every later edit (as math openers do, ADR 0006), which costs
  O(distance) on each edit after a stray `:::name` at the top of a large file. Edits inside need
  nested restart checkpoints through one more container kind, and one large container would be
  one page.
- **Pairing the next bare `:::` (the prototype).** Simple, but a forgotten closer takes the next
  block's opener in as text and closes on its closer, silently, and has no nesting.

### Editing

Paired marker lines are `LeafKind::ContainerMarker` leaves: structure, not text. Planners keep
working with `LeafView::containers`, which stays the containers that put a prefix on a line; a
directive puts none, so inside one everything edits as at the top level. The neutral query for
the directives around a position is `FormatDoc::named_containers_at`. Outside source mode:

- Backspace at the first line inside and Delete at the last line inside do nothing: joins only
  reach paragraphs and headings. The same holds from outside, next to a block.
- The keyboard does not edit a marker line. Enter on it goes to the virtual paragraph after the
  line; after an opener that is the first line inside, which is also how an empty block takes
  its content. Other commands are not applicable on a marker or over a selection touching one,
  except inline styles, which skip it.
- A selection replaced or deleted across a marker keeps the markers whose pair it does not hold
  whole, each on its own line, and deletes all the other selected text. A selection holding a
  pair whole removes the block. So half a pair is never left behind. When typing or Enter
  replaces such a selection, the deletion is an undo step of its own.
- `:::name` typed on a new line at the document level is literal until Enter (as code fences),
  which writes the closer with the same fence and puts the caret inside. If the caret leaves
  the line instead, the line is escaped (`\:::name`) as other block syntax is.

Source mode edits the text as text. The pairing rule then decides what a line is, and an edit
that breaks a pair shows immediately: the other marker reads as plain text.

### Layout and paging

`FormatDoc::flatten` emits each top-level block inside `FlatContainer::Card` for every directive
around it, so items carry `Container::Card` in their chain and their hash. Pages split a block
freely. An edit changing a pair reports the blocks between as changed: real items are rebuilt
and the placeholders between are replaced, which drops their measured heights. Cost per edit
that changes a pair far away: one re-paging of that range, without a parse (see the PR's
numbers). Typing elsewhere costs a shift of the later markers.

`CrossBlockIndex` pairs `<details>` as before, by the containers that have a line prefix: a
directive is no scope for disclosures. A `<details>` opened in a block and closed outside it
pairs across the boundary, and this has no effect on `:::` pairing.

### On screen and in export

The rendered view draws a simple card, the same for every name: edges around the content,
`name args` as a label, no per-name behaviour. Marker lines have no caret stops; the virtual
paragraph in an empty block takes the caret. Source mode shows the lines as they are. The label
is cut at 200 characters for display, because a marker is one item and is not segmented like a
long paragraph; the source keeps it whole.

- **HTML:** `<div class='md-directive md-directive-NAME' data-name='…' data-args='…'>`, a
  `<p class='md-directive-label'>` and the content. Name and arguments are HTML-escaped; in the
  class, every character but ASCII letters, digits, `-` and `_` becomes `-`.
- **PDF and PNG:** a bar in the rule color, the label in bold, the content indented. Page
  breaks before an h1 apply inside a block; a block starting with an h1 breaks before the block.
- **Pandoc (Word and the other Pandoc targets):** a `Div` with classes `directive` and the
  sanitized name, attributes `name` and `args`, and a `title` `Div` holding the label, as alerts
  have. Values are JSON strings. What a target does with a `Div` is the target's own choice.

Unpaired markers export as the text they are. Exporters read `Document::tree`.
HTML's size limit and cancellation checkpoints also run between the blocks inside a block.

## Consequences

- Round trip is byte-exact by construction: the text remains the source of truth, and markers
  are ordinary spans in it.
- Code that walks `blocks()` or `tops()` meets marker leaves.
- Prose lines that look like markers now split a paragraph. A file relying on a line such as
  `:::x` inside a paragraph renders as three paragraphs; its bytes do not change.
