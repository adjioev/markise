# ADR 0020: block fields and their Markdown codec

Status: accepted for issue #178, phase 3.

## Context

A `:::name` block (ADR 0018) is plain Markdown. To edit it through a form now, and in place
later, its content has to appear as named fields with values, and a change of a value has to come
back as an edit of the Markdown. Files stay hand-editable, so two things must hold whatever the
edit: bytes nobody changed stay as they are, and an edit cannot break the structure of the block
by accident. Markise holds no vocabulary of any site: what fields a block has comes from outside.

## Decision

`markise_engine::block_fields` is the model. It knows Markdown and no GUI, no form and no
package format. Its input is a block's source and a list of declared fields.

### Fields

| Kind | Markdown | Value |
|---|---|---|
| heading | one ATX heading, of the given level if one is declared | its text without `#` |
| text | paragraphs, and whatever no later field takes | one Markdown string |
| quote | one block quote | its text without `>` |
| list | one list | one string per item, without the marker |
| choices | one task list | `{id, text, correct}` per answer, `id` from `{#id}` |
| image | paragraphs holding only images, one per line | `{alt, path}` per image |

A trailing `{#id}` of a heading or an answer is kept out of the value and is never lost. Each
body block goes to the first field at or after the cursor that takes it. An exact kind goes first,
then text takes the rest. A field fills once; text and images go on over neighbouring blocks. What
nothing takes, a nested `:::` block for instance, is reported as an **extra** and never touched.

### Identity

The identity of a field is its declared name. An item (list item, answer, image) has an
`ItemId`, which lasts through edits and reorders and through the successors of the model
(`Applied::fields`, `refreshed`). It is not written to the file. Across a change from outside, it
goes by anchor, then equal content, then position.

A persistent answer id is the `{#id}` in the file. A new answer gets `a<N>` above every number in
use anywhere in the block and every number removed through this model. A removed id does not come back while the model
lives; after a reload the file is all there is.

### Encoding: old bytes plus a diff

There are no per-edit patch rules. An edit changes the values, the new block text is put together
from the old bytes of everything still the same, and a line diff of old and new text gives the
patches, narrowed to the differing characters on a changed line. Therefore:

- No change gives no patch, and a patch can hold only what differs.
- A value nobody changed is written as its old bytes, even when it reads as empty: `## {#q}` keeps
  its id until its field is really edited.
- A line that stays in a value keeps its bytes: prefix, trailing spaces, line terminator, in
  mixed-ending files too (C23). New lines take the block's line ending.
- A move is a deleted and an inserted line. A character diff over the whole text would be smaller
  and would scatter it over the lines between, so it is not used.
- Patches are verbatim, in the coordinates of the block. `apply_to_session` shifts them, refuses
  when the document no longer holds the source (`Stale`), and applies them as one transaction, so
  a set of edits is one undo step.

Lists keep their style: marker, tightness (gaps go by position, not with the item) and ordered
numbering if it was regular (`3. 4. 5.`, `1. 1. 1.`). An item emptied of everything stays as an
empty item. A field emptied of everything leaves the source with the blank lines around it; an
absent field that gets a value is written at its declared place with a blank line around it.

### Markup typed into a field stays text

A value is what the author typed, so a new line that would start a block is escaped on the way to
the source: `# x` becomes `\# x`, `1. x` becomes `1\. x`, `:::x` becomes `\:::x`. Whether it would
start a block is decided by the parser, plus the few things visible only next to another line (an
underline, a table's delimiter row, `$$`). Decoding removes exactly what encoding adds, so `\# x`
reads as `# x`, and `\\# x` as `\# x`. The same holds for a typed `{#x}` at the end of an answer or
heading and for closing `#`s of a heading. Leading whitespace of a new line is dropped, because
four spaces would make code.

Lines that stay are not escaped. Text holding a list keeps that list's bytes while other lines
change; an edited line of it becomes text. Changing structure is what the Markdown view is for.

### Values for a form

`values()` is JSON: strings, arrays of strings, `{id, text, correct}`, `{alt, path}`.
`edits_from_values` diffs a new JSON value against the model. Arrays hold no `ItemId`, so entries
match answers by `id`, then anything by equal content, then the rest in order. Equal items are
interchangeable, and an item both moved and edited at once still ends up correct. In-place editing
uses `FieldEdit`s and `ItemId`s directly.

### Validation

Fitting a block to its schema is a report, never a gate: a block outside the schema reports a
reason per field, stays editable and can be saved, because strict checks belong to whoever
publishes. `BlockFields::check` gives the reasons plus the extras.

The rules live in `schema-form-core`: no GUI, no Markdown, used by the engine here and by a form
crate above. It reads a **declared, versioned subset** (`SPEC_VERSION` 2) and refuses the rest by
name, so an arbitrary schema cannot silently become a promise:

- JSON Schema: `type` (one of string, boolean, array, object), `properties`, `required`, `items`,
  `minLength`, `maxLength`, `minItems`, `maxItems`, `additionalProperties` as true or false, and
  `enum` of distinct strings on a string. Annotations are ignored. `pattern`, `format`, `$ref` and
  combinators are refused, and so is a keyword that would mean nothing on its type. This holds at any depth, in the entries of
  choices and images and below a custom widget too; a custom widget may only leave `type` out.
- UI hints, our own small format and not JSON Forms: `version`, `order`, `fields`, and per field
  `label`, `help`, `widget`, `multiline`, `add_label`, `selection`, `correct`, `options`, `columns`,
  nested `fields` and `order`, and `custom` for a host's widget. `variants` overlays the hints by a host's name; the overlay named `""` is for
  no variant, and a named variant does not inherit it.
  Layouts and rules are not offered. `x-` keys are ignored; the engine reads `x-markdown` (heading,
  quote) there, so the rest stays free of Markdown.
- Correctness (`exactly-one`, `at-least-one`), missing and duplicate answer ids, an option that
  excludes the others: these are not JSON Schema, so a validator would not have given them anyway.
- Version 1 hints keep their meaning: `enum` and choice buttons need version 2, which an older
  build refuses by version rather than by a keyword it has not heard of.

Empty is how missing content looks in a document, so an empty string or array as a field's value
fails `required` and passes the other limits. An entry of an array exists, so its strings answer
to their limits even when empty.

**Own validator, not the `jsonschema` crate.** The crate is MIT, and with default features off its
tree has no HTTP or TLS. Measured on this machine with 0.60.2 against a binary using only
`serde_json` (release, macOS arm64): 5.47 MB against 0.47 MB, 64 against 6 crates in the tree, 44 s
against 4 s from clean. For that it would check what about 170 lines check here, its error output
would still need mapping to fields and languages, and it would not refuse keywords outside the
subset. Should the subset grow to where this is no longer small, the choice can be revisited
behind `FormSpec::validate`.

### The native form

`crates/egui-schema-form` draws a `FormSpec` over JSON values: schema, hints and values in, changed
values and reasons out. It depends on `schema-form-core`, egui and `serde_json`, and knows nothing of
Markdown, Markise or any block. It draws generic primitives only: text, multiline text, string
list, array with add, remove and reorder, radio and checkbox choices, choice buttons, checkbox,
group. No widget of
any site's vocabulary lives here.

- **Identity.** Values stay plain JSON. Beside them every array row has a key that moves with it,
  and each row has an explicit egui scope built from the key, not its position. Text fields, their
  scroll areas and the buttons take their ids from it, so caret, selection, IME composition, focus
  and scroll follow a row through a reorder. `set_values` keeps keys by `id`, equal content, then
  order.
- **IME.** egui writes the unfinished composition into the text. So `changed` stays false during a
  composition and comes at its end, and values set from outside wait for it. A composition belongs
  to the form whose editor has the focus; other forms in the window hear nothing of it.
- **Keyboard.** No drag in this phase. Up, down and remove are buttons in the tab order, and Alt+Up
  and Alt+Down move a row from anywhere inside it. Focus does not get lost: a button going disabled
  at the edge passes it to the other, a removed row to the next, or to Add when none can go. A new
  row gives it to its first real control, whatever kind that is.
- **Screen readers.** Fields are labelled, a reason is an `Alert` live region and the field's
  description, a move is announced.
- **Theme and zoom.** Colours and sizes come from the `Ui`; icons are painted, so no font is needed.
- A value of an unexpected type is shown as not editable and kept. The form never coerces it.

### Choice buttons

A choice among known options can be drawn as one button per option (`"widget": "buttons"`, the
default for an `enum`). This is a generic primitive, the owner's decision: a choice can be made
"by icon buttons", and an option can be `exclusive`. No widget for any particular choice exists.

```json
{ "widget": "buttons", "selection": "multiple", "columns": 4, "options": [
    { "value": "Left", "label": "To the left", "icon": "left" },
    { "value": "None", "exclusive": true } ] }
```

The value may be held three ways, and the schema says which:

| Schema | Value | An option is selected when |
|---|---|---|
| string with `enum` | one string, empty for none | the string is its value |
| array of strings with `enum` | the strings selected | the array holds its value |
| array of `{id, text, correct}` | answers | the first row with its value as `text` is `correct` |

`enum` says what is allowed, `options` how it looks; an `enum` value with no option shows under its
own name, and an option outside the `enum` is refused. Answers have no place for an `enum` that
differs by variant, so for them `options` alone says what is allowed.

- **What a press means** lives in `schema-form-core` (`toggle_option`, `write_selected`), not in
  the drawing: one selection stays on what was pressed; otherwise a press toggles, an exclusive option
  clears the others and any other clears the exclusive ones.
- **Answers keep their rows.** A press writes only the marks that changed. A selected option with
  no row gets one in the options' order. A row that fits no option, or a second answer with an
  option's text, is not a button's to change: the form shows its text and a remove button, and the
  validator gives a reason (`NotAnOption`). Marks that break an exclusive option give a reason too,
  and any press mends them.
- **Repeated strings are one selected option.** In an array of strings, `["a", "a"]` selects `a` once.
  The second row still has a reason (`NotAnOption`), but no remove button of its own: pressing the
  option removes every row of that value, so a press always deselects and an exclusive press leaves
  no copy behind.
- **New answers have no `id` key.** In a form draft, a row with no `id` key is new and its host
  gives it an id when the values are stored. An empty `id` is a row read from somewhere that has
  none, and that is a reason (`MissingId`). So adding an answer no longer shows a false reason.
- **Icons come through the host.** `FormHost::icon(name)` answers like a thumbnail and must not block;
  the name is data the form does not read. `Thumbnail::Mask` is a one-colour picture the form tints
  with the text colour, so it follows the theme. No answer means the label alone.
- **Keyboard and screen readers** as for other choices: each button is a tab stop, Space and Enter
  press it, its id comes from the option, and it reads as a radio button or checkbox with its
  state and label. The row count follows the width, down to one button per row, so a narrow
  panel or a large zoom never cuts one off. In a choice with icons, an option without one takes
  a row of its own.

### From a package to a form

A package declares `fields: [{name, type, ...}]` (ADR 0019); the form reads JSON Schema and hints.
`BlockSchema::form_input` in `markise-package` is the one place that knows both. Its output is plain
JSON, so that crate needs no form, and the hint `x-markdown` tells `decls_from_spec` where each
field lives, so the form and the field declarations come from one source.

| Type | Schema | Widget | Required unless |
|---|---|---|---|
| `heading` | string; `level` goes to `x-markdown` | text | `min` is 0 |
| `text` | string | multiline | `min` is 0 |
| `quote` | string | multiline | `min` is 0 |
| `list` | array of non-empty strings | list | `minItems` is 0 or absent |
| `choices` | array of answers with non-empty `text` | choices | `minItems` is 0 or absent |
| `image` | array of `{alt, path}` with non-empty `path` | images | `min` is 0 or absent |

- **Labels.** A field's `label` (string or per language), else its name made readable, as nothing
  else knows better. `help` and `add_label` go through too.
- **Limits.** `minItems` and `maxItems`, or `min` and `max` on an image, become the array's limits.
- **Variants.** `correct` is one rule, or a rule per variant with `""` for the block without one. It
  becomes the base hints and `variants` overlays. A block's arguments pick the variant when
  they name a declared one. Selection follows the rule: `exactly-one` is one mark, others many.
- **Refused by name.** A known key with a value a form cannot honour stops the block's form with a
  reason rather than being dropped: a numeric `max` or a `min` above 1 on one piece of text, a
  `level` outside 1 to 6, `ids` other than `required`. A key not known here is data for
  later versions and is ignored (ADR 0019). The panel shows the reason; the block stays a card.
- **Icon choices.** A preset can say how a variant's answers are chosen: `widget: "icon-choices"`,
  `variant`, `field`, `answers: [{label, icon, exclusive}]`, `columns`, `field_label`. These become
  choice buttons in that variant's hints; `variant: ""` is the block with no variant and no key at all
  is every variant. A `field` or `variant` that is not a string is refused. An option's value is its `label`, the text the answer has
  in the Markdown. `icon` is a file of the package, which the host resolves. Other `widget`
  values stay data for later.
- **The block's `argument` is not checked in this phase**: it is not a field, and the subset has no
  `pattern`.

`tests/it/block_forms.rs` in the engine holds the agreement: for every variant, a full and an empty
block, the engine's values have the type the spec declares at every depth and no other key,
and storing them unchanged gives no edits.

### Images and host widgets

The form opens no dialog, reads no file and decodes no image. It asks `FormHost`:

- `request_file` gets a token and returns at once; the answer comes later through `deliver_file`.
  The token belongs to the row, not its place: a row may move while the dialog is open and takes
  the file there. A removed row drops the answer, and cancelling changes nothing.
- `thumbnail(path)` returns loading, ready, failed or none and must not block. Called each frame
  for each image with a path, so loading and caching stay with the host, which repaints when
  a picture arrives. A failed picture is a message, not an edit.

The only extension point is a **named host widget**: `register_widget(name, widget)`, referred to
by the hints as `"widget": "custom", "custom": name`. The form passes it the value, label, a stable
id and the reasons, and shows its reasons as well. An unregistered name shows a notice and keeps
the value. This repository holds no such widget except one in the tests, and no site's vocabulary:
a site's particular picker is a host's or a package's business.

Known limits of that contract: the form does not see inside a host widget. Inside an array row, Alt+Up
and Alt+Down do not work from the widget's own controls, and a composition in its own text editor
is not the form's: `is_composing` stays false, so a host widget owns that. No host widget exists in
the repository yet, so this stays as it is until one needs more.

### Limits

- A text value holding a line that is only an image reads back as the image field when the block
  declares one after it.
- A setext heading is not a heading field: it goes to text or stays an extra.
- Hard-break trailing spaces of an answer's last line belong to its text.
- The block's name and arguments are not fields.

## Alternatives

- **Spans per value and one patch per changed value, as the prototype did.** Reorders rewrite every
  text by position, and the insert and remove cases multiply with each neighbouring field. The
  prototype's Markdown scanning also ran on raw lines; this one uses the parser, so `-` in a code
  fence stays code.
- **Parsing values back through a Markdown serializer.** Would normalize what nobody edited.

## Consequences

- The form crate and the panel get JSON and need no Markdown. The panel is a later PR.
- `egui-schema-form` could leave the repository as it is: nothing in it is ours.
- The decode cost is one parse of the block and the encode cost one line diff of it, per apply.
  Neither depends on the document's size.
