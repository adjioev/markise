# ADR 0021: native views of blocks

Status: accepted for issue #178, phase 4.

## Context

With a bound, accepted package, a block whose package entry has a `view` should read as what it is
rather than as Markdown in a frame, and its text should be edited where it stands. Markise keeps no
site vocabulary: only the generic kinds `callout`, `card`, `choices` and `media`, with accents and icons
from package data (ADR 0019). A block without a view, or with no package, stays the generic card of ADR
0018. Files stay hand-editable, and large documents stay fast, so the view cannot cost more than the
paged model allows (ADR 0016).

## Decision

### A view styles the items a block already has

A block's marker lines and content are flat items of the view model, and pages split them freely (ADR
0018). A native view does not change that. It adds no item, no model state and no parsing to the model:
the same items lay out with a little more input, namely the block's view and what the item is to it. So:

- The paged model, its invariants and its differential tests hold as they did. A large block still
  splits into pages.
- Caret geometry, source maps, selection and search read the same layouts as anywhere else. Editing in
  place builds on that rather than on a second text surface.
- The alternative, one laid-out item per block, needs a block to be materialized whole and gives it a
  text surface of its own, with a second implementation of hit testing, IME and undo.

### The interface

`markise_layout::native` holds what layout knows, and depends on no package:

- `NativeKind`, `NativeView { kind, accent, icon, label, single_answer, fill, label_row }`, `NativeRole` (title, text, quote,
  list item, answer, image).
- `BlockViews`, a trait the view holds as `Option<Arc<dyn BlockViews>>`: `has_view(name)` and
  `has_schema(name)` from the name alone, `look(name, args, source)`, which is `Generic`, `Outside` or
  `Unfilled` with its reasons, or `Native` with each part's role by source range, `icon_ready(icon)`, and
  `label_row(name)`, from the name alone.

`markise_engine::block_view::PackageViews` implements it from a `PackageIndex` and the field model
(ADR 0020): the form spec of the block's variant, `BlockFields::decode`, and `.check`. A package's view
and the form therefore agree on which fields a block has and whether it fits. `markise-layout` does not
depend on `markise-package`, and `cargo xtask layers` holds.

Nothing about a document's bytes depends on any of this.

### A block outside its schema

A block with a schema in the package, with or without a view, can fail to fit it (`.check`) in two ways (owner's
decision, phase 6):

- **Not filled in yet.** All that is wrong is an empty required field, too few characters or items, no marked
  answer, or an argument missing or not of its form (`BlockFit::only_unfilled`, ADR 0020). A block with a view stays native (`Unfilled`), also after the caret leaves,
  with `⚠` and the first reason on its first row, all the reasons on hover and for a screen reader. It edits in
  place and switches as any native block. A block just inserted from a preset is usually this.
- **A structural problem.** Content in no field, arguments naming no variant, or any other reason (an answer with no
  id or a repeated one, two marks where one can be, too many items) is a shape the view cannot read: `Outside`, the
  generic card with the same `⚠`. A block with no view that is only unfilled is this too, as it has nothing else to show.

The row keeps its height, so a long reason is cut there. A card edits and saves as any card: strict checks belong
to whoever publishes (ADR 0020). A block with no schema, or one the package says something a form cannot honour
about, is plain `Generic`, and the panel explains the second.

### Icons

An icon is the file `assets/<name>.svg` of the package (ADR 0019), through the same worker and the same
refusal rules as the form's icons (`FormAssets`, ADR 0020). Nothing is read in layout: `BlockViews::
icon_ready` answers without blocking, a host that does not have the picture yet remembers it was asked,
loads it off the UI thread and asks for a frame when it comes. Only the opener's key depends on the
answer, so only that item is laid out again, at the same height. A name with no such file, or a file the
rules refuse, is no icon and no error. The layout emits `Prim::Icon` and the viewer draws it, as it has
the textures.

### Cost

A document with no package pays one comparison per item key. With a package, per frame:

- An item outside a card pays one comparison more.
- An item in a card pays a map lookup by its start. A miss, after any change to the document, asks the
  parser for the block around it, O(log markers + depth), and hashes the block's source.
- `look` runs once per distinct block text. Blocks over 64 KiB (`MAX_NATIVE_BLOCK_BYTES`) stay generic
  without being read: the decode is on the UI thread, so its size has to be bounded by something other
  than the file.
- Only what reaches an item's layout enters its cache key: the view and the item's role. So typing in
  one field lays out one item, as it does outside a block.

**Height estimates.** A native opener has the height of a generic one, so nothing changes for it. A
heading in a native view draws at one size whatever its level (it is the block's title; the level says
what it is in the file), so estimates ask for the block's name: `View::native_title_estimate` for a real
item, and for a placeholder `PageStats::card_heads`, headings per level per block name, gathered when a
page is cut. A block that turns out generic after all is corrected when it is laid out, like any
estimate. A measured page height holds for the views it was measured under (`paging_key`).

An opener with no label row (below) has only what makes the space above the first field the block's
padding, so its estimate is the same as its layout. `BlockViews::label_row` answers by name, for a real item,
and `PageStats::card_names`, openers per block name gathered when a page is cut, for a placeholder. A
block with a warning has the row after all and corrects the estimate when it is laid out: nothing is read to
estimate a height.

### Look

Each kind is the generic card's geometry with its own edges, so a block lays out to the same width and
padding whichever way it shows. The first row is the package's label, in the accent, on the row where the
generic card has `name args`.

- **callout:** a tint of the accent over the block and a bar in the accent on its left.
- **card:** framed in the theme's rule, with an accent top edge and the title in the accent.
- **choices:** framed in the accent. Answers show a radio button when one can be marked and a check box
  otherwise, from the same `selection` as the form; a marked answer's row has a tint of the accent.
- **media:** no frame; pictures as usual, each with its alt text below as a caption.

**Quiet views.** On a real page, a tint, a frame, a coloured title and a coloured label per block compete
with the text. A package can say less, per view, in generic terms; the default stays as above, so a package
that says nothing does not change.

- **`fill: false`:** no tint and no frame on any kind, only a 2 pt bar of the accent on the left. A marked
  answer's row has no tint, a card's title and the first row are in the text's colours. A frame is left out
  because it still draws a box per block, in the accent for `choices`.
- **`accent: "none"`:** lines in the theme's rule, 2 pt or 1 pt, a card's top edge no heavier than its sides,
  labels and marks in the theme's quiet text. A `callout` that keeps its fill has a veil of the text's colour,
  light enough for a code block to stay distinct on it. `none` is a name like the others: an older build reads it
  as the theme's accent, which is what an unknown name means, and no format change is needed.
- **`label: false`:** no first row and no icon. The block's own first field comes first, with the block's padding
  above it. The label goes to the accessibility tree as a node at the block's top edge. A warning or a note
  still needs the row, so a block with one has a row holding only that. It is the package's call that the
  block has a heading: nothing checks that a heading is filled in.

A wrong type for `fill` or `label` refuses the package, like any problem in a view.

**A block is one ground.** A tint and a bar are not prims of the item: an item's layout lists them as `bands`,
and `block::merged_bands` makes one rectangle per block from the items on screen, painted before any item.
Painted per item they left faint seams (issue #231), because the viewer moves every item's top to a whole pixel
of its own (issue #86) while heights stay fractional: an item's bottom and the next one's top differ by up to a
pixel, and rounded apart they leave a line with no tint or with twice the tint, at any zoom. Moving the
tops would bring back what that rule fixed, and an opaque fill would hide what the page has below, so the
one rectangle goes from where the first item stands to where the last one ends. An item with a band of another
block, or none, ends a band: a nested block keeps its own look, and an outer ground does not run under it.

**A block's controls** (the pencil, the switch, delete) show when the pointer is on the block, when it is the
innermost block around the caret, or when one of them has the keyboard, like the drag grip. They are registered for
every block on screen in any case and only their painting depends on this, so nothing moves and the keyboard and a
screen reader have them. The surface keeps Tab for its fields, so from the text the way to them is Esc and then
Shift+Tab; the commands (Edit Block, Switch This Block's View, Delete Block) do the same for the caret's block. On a
block with no label row they lie over the end of its first row, on a ground of their own.

**Accents** are generic colour names with a light and a dark value (`native::accent_color`): slate, gray,
red, orange, amber, yellow, green, teal, cyan, blue, indigo, purple, violet, pink. A name that is not one
of these or `none` means the theme's own accent, with no error: a package may say more than a build knows.

Everything is in points from the theme's font size, so zoom and themes apply as they do to the rest of
the document, and the layout cache already keys on them.

### Hidden ids

A trailing `{#id}` is no text of any field, so a native view never shows it, also with the caret on the
line. The generic card keeps the rule of ADR 0018.

### Editing in place

The text of a native block is edited where it stands. There is no second text surface: the caret,
selection, hit testing, IME and undo are the rendered surface's, since a view is the same items. What is
different is what a keystroke means there. `markise_engine::editor::inplace` takes the commands that
would change the text and turns them into an edit of one field's value. Everything else that changes text
(the form, Find, undo, Source mode, the file on disk) works as it does anywhere, and the next keystroke
reads what is there.

**Slots.** `BlockFields::slots` lists the places to type: a heading, a text field (its paragraphs are one
value), a quote, a list item, an answer. Pictures are not slots: the form has them. A slot maps source
offsets to value offsets line by line. A value line lacks only the escapes of its source line (`\# x`
reads `# x`), so the mapping is a walk over both, and a stop never falls between an escape and what it
escapes.

**An edit** (`BlockFields::edit_slot`) replaces a value range through `FieldEdit`, so markup typed at the
start of a line becomes the escaped form the form would write, and the patches are the codec's minimal
diff. Then, before anything is applied, the result has to pass all of this or the keystroke does nothing:

- the block has the same name and arguments, and what no field took has the same bytes;
- every other field and every other item reads the same, and the item keeps its id and mark;
- the value reads back as asked, leading blanks of a line aside (a typed leading blank is not kept, ADR
  0020, and no text then means no change and no caret move);
- the body parses to the same sequence of kinds. A chunk count does not say this: `[r]x:` over `/url` is
  a paragraph until the `x` goes, and then it is a reference definition that changes links elsewhere,
  while each line alone reads as a paragraph;
- every patch lies inside the lines of the edit, or inside the field's region when it goes away.

The codec needed changes for that last rule, which also hold for the form: a line of blanks stays as
written while it stays, a line rewritten in place keeps its prefix and its terminator (a tab-indented
continuation, CRLF in a mixed file), and `02.` stays as written when it still says 2.

**Read-only parts.** A text field can take a code block or a list as it stands, and an edited line of it
would become escaped text. So what is not a paragraph below a field, a quote or an item is protected. The
caret can stand on it and a keystroke there does nothing, and a selection touching it does nothing either.
The Markdown view has it. Similarly a trailing id is only cut where the reader's view hides it too, at the
line's start or after a blank, so the codec and the screen agree on what is an id.

**The caret.** Markers have no stops, and hidden ids are not revealed here. Inline marks (`**`) still show
with the caret on them, since a field's value is a Markdown string. Plain arrows cross fields freely and
leave the block at its ends. A gap the layout offers between two items is passed over. A selection that
starts in a slot stays in that slot, however far it is dragged. One from outside into a block holds the
whole block: the block is then deleted or replaced as a whole, by the rule of ADR 0018.

**Undo.** A keystroke goes through the session's own keystroke path, so typing groups (their pause, the
break at a blank, direction), undo steps and the selection restored are the document's. It is typing.
Inline marks run their usual planners with a selection inside one slot, and are taken back, with no redo, if
the same block no longer stands from the same opener to its own closer and reads the same. Whatever else
inserts text in a field (link completion) goes through `EditorView::field_replace` and the same checks. Every other mutating command is refused in a block.

**Keys.** `editor::inplace` decides them; the field model does the work, with the same checks.

| Key | Where | What |
|---|---|---|
| Enter | a list item | what follows the caret goes to a new item after it |
| Enter | an answer | the same, a new answer with a new `{#id}` and no mark |
| Enter | an empty last item | it goes, and the caret stays at the end of the one before |
| Enter | an empty item in the middle | nothing |
| Enter | a heading | to the next field; the text stays |
| Enter | text, a quote | a new paragraph; at the end of the text a place for one, nothing in the file until it has text |
| Shift+Enter | text, a quote | a line break in the paragraph |
| Backspace, Delete | an empty item, at its only edge | it goes, unless it is the only one; its id does not come back while the model lives |
| Backspace, Delete | an item or field with text, at the edge | nothing: items are not joined, fields do not run together |
| Backspace | between two paragraphs of one text field | they join: they are one value |
| Tab, Shift+Tab | any field | the next or the previous field, staying at the ends |
| A paste | text, a quote | its lines; markup at their start stays text |
| A paste | a heading, an item, an answer | one line: a run of line breaks is one blank |
| A press on an answer's mark | | marks it by the same `selection` as the form: one mark moves to it and a press on the marked one does nothing; several toggle. Its own undo step, and the caret stays |

**A place with no text.** An emptied heading stays in the source as `## ` with its id (`FieldEdit::
ClearHeading`). An emptied item stays an empty item. An emptied text or quote has nothing to stand on in the
source, so the caret goes on a virtual paragraph where it was, and the session's in-place state remembers
which field that position stands for, by the state of the document: `DocPos::virt` cannot say (two absent
fields can share a gap), and undo brings back a state and its caret, so it brings back the meaning too. Only
a caret an edit put there has that meaning; one with none writes nowhere. Typing on it fills that field in its
declared place. Enter and Shift+Enter at the end of text put the caret on such a place as well, with what goes
between the text and what comes next.

**A block being edited stays native.** Emptying a required field only makes a block unfilled, which is native
anyway. What the caret leaves behind can still be worse: a preset with a broken structure, or a change from outside
while the caret is in the block. So the block the caret is editing stays native until the selection leaves it,
marked by a live mark at its opener. Then the rule applies: a block left with a structural problem is a card with a
warning. This lives in the view by opener, not in the look cache, which is by content: another block with the same
text is another block.

**The sticky state, the cached model and limits.** One model is kept between keystrokes, through `refreshed`,
so that retired ids live on while it does. Blocks over 64 KiB are generic, which also bounds the cost per
keystroke: a decode, a diff and a decode of one block. An optional field with no text in the file has no place
in the view: the form adds it. A picture dropped or pasted into a field asks for the form instead.

### The switch

`markise_layout::BlockViewMode` is what a block can show as: `Native`, then `Site` (ADR 0023), then `Markdown`, which is the generic
card, where anything a card allows is allowed. A block with a structural problem is a card in any state, with its
warning. `BlockViewMode::next` is the order a switch goes through, and nothing else asks for a state by position.

- **A block.** A button beside the block's Edit button says what comes next, or the command Switch This
  Block's View (⇧⌘M) for the caret's block. A block with no view has neither, unless something can draw it (ADR 0023).
- **The document.** Switch All Blocks' Views (⌥⇧⌘M) moves the document to the next state, and the blocks that
  differed follow it.
- **Where it is kept.** The session keeps a block's own state at a live mark on its opener, so it survives
  edits above it, and hands the view the positions after every change: moving them costs what any edit does
  and lays out nothing else. A state goes with its block: when the opener's first byte is deleted or the name
  changes, it is dropped rather than collapsing onto what follows. The state is not in the file, and not in settings. The app keeps it per document
  path for the session and brings it back when that document opens again, for openers that still stand where
  they did. A large file opens in parts, so a state for an opener not there yet waits for the rest. A document opens native by default.
- **The pointer.** Text gets the text caret. A block's first row, its buttons and an answer's mark get the arrow.
