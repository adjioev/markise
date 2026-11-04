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
use and every number removed through this model. A removed id does not come back while the model
lives; after a reload the file is all there is.

### Encoding: old bytes plus a diff

There are no per-edit patch rules. An edit changes the values, the new block text is put together
from the old bytes of everything still the same, and a line diff of old and new text gives the
patches, narrowed to the differing characters on a changed line. Therefore:

- No change gives no patch, and a patch can hold only what differs.
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
- The decode cost is one parse of the block and the encode cost one line diff of it, per apply.
  Neither depends on the document's size.
