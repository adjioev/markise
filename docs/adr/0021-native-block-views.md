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

- `NativeKind`, `NativeView { kind, accent, icon, label, single_answer }`, `NativeRole` (title, text, quote,
  list item, answer, image).
- `BlockViews`, a trait the view holds as `Option<Arc<dyn BlockViews>>`: `has_view(name)` and
  `has_schema(name)` from the name alone, `look(name, args, source)`, which is `Generic`, `Outside` with
  its reasons, or `Native` with each part's role by source range, and `icon_ready(icon)`.

`markise_engine::block_view::PackageViews` implements it from a `PackageIndex` and the field model
(ADR 0020): the form spec of the block's variant, `BlockFields::decode`, and `.check`. A package's view
and the form therefore agree on which fields a block has and whether it fits. `markise-layout` does not
depend on `markise-package`, and `cargo xtask layers` holds.

Nothing about a document's bytes depends on any of this.

### A block outside its schema

A block with a schema in the package, with or without a view, that does not fit it (`.check`), whose
arguments name no variant, or that has content in no field, is `Outside`: the generic card, with `⚠` and
the first reason on its first row, and all the reasons on hover and for a screen reader. The row keeps its
height, so a long reason is cut there. It edits and saves as any card: strict checks belong to whoever
publishes (ADR 0020). A block with no schema, or one the package says something a form cannot honour
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

### Look

Each kind is the generic card's geometry with its own edges, so a block lays out to the same width and
padding whichever way it shows. The first row is the package's label, in the accent, on the row where the
generic card has `name args`.

- **callout:** a tint of the accent over the block and a bar in the accent on its left.
- **card:** framed in the theme's rule, with an accent top edge and the title in the accent.
- **choices:** framed in the accent. Answers show a radio button when one can be marked and a check box
  otherwise, from the same `selection` as the form; a marked answer's row has a tint of the accent.
- **media:** no frame; pictures as usual, each with its alt text below as a caption.

**Accents** are generic colour names with a light and a dark value (`native::accent_color`): slate, gray,
red, orange, amber, yellow, green, teal, cyan, blue, indigo, purple, violet, pink. A name that is not one
of these means the theme's own accent, with no error: a package may say more than a build knows.

Everything is in points from the theme's font size, so zoom and themes apply as they do to the rest of
the document, and the layout cache already keys on them.

### Hidden ids

A trailing `{#id}` is no text of any field, so a native view never shows it, also with the caret on the
line. The generic card keeps the rule of ADR 0018.

### Editing

A native block's text is not edited in place yet: commands and typing that would reach into one, with a
selection that does not hold it whole, do nothing. Its form, undo and redo work as anywhere else.
