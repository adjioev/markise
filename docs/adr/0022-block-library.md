# ADR 0022: the block library, inserting and moving blocks

Status: accepted for issue #178, phase 5.

## Context

With a bound, accepted package (ADR 0019) an author can edit a block in the form (ADR 0020) and in place
(ADR 0021), but not yet start one without typing its Markdown or move one without cutting it. This phase
completes authoring in the native view. Markise keeps no vocabulary here either: groups, labels, keywords,
icons and the Markdown of a block all come from the package, and the package is untrusted data.

## Decision

### The block library

`PackageIndex::library(language)` is plain data: one row per preset, by the package's groups, the blocks of
each group, the presets of each block. A preset of a block in no group comes last, with no heading, so that
a package's oversight cannot put a preset out of reach. A block with no preset has nothing to insert and has
no row. Markise makes no preset of its own from a block's fields: what a new block starts with is the
package's to say.

- **A row** shows the preset's label, else the block's. Below it the block's description; when the block has
  several presets, the block's label first, as such rows differ only by their titles. `:::name` does not show.
- **Icons** are the block's icon name, through the form's worker with the same refusal rules (ADR 0020). A
  name with no file, or a file refused, is no icon and no error; a list in which no row has a picture has no
  place for one. No icon is chosen by a block's name.
- **A query** takes every word of it, each anywhere in the preset's label, the block's label or description in
  any language the package has, its keywords and the block's name, ignoring case, and with `ё` as `е`. The
  rows stay in the package's order: a query narrows the list and does not rearrange it. Text shows through
  the lookup of ADR 0019, as English, the app's only language so far.

### `/`

The app has no `/` menu of its own, so this is new only where a package offers something. A document with no
accepted package, or a package with no presets, gets a typed `/` as ever.

`/` opens the library when the caret stands **on a free line**: no selection, on a line with no text, at the
document's level, in no block and no list or quote (`EditorView::on_free_line`). Anywhere else `/` is a
character, since a slash in a sentence, a path or a field of a block is text. Source mode edits text as text,
so there `/` is a character too; Side by Side's rendered pane behaves as the rendered view.

**Nothing is written until a choice.** The `/` goes into the library's field rather than the document, so
that undo after an insertion goes back to the exact source, in one step, with no `/query` in between. That
would make a line starting with a slash hard to type, so:

- `/` again with nothing else typed closes the library and types one `/`.
- Return with no row matching closes it and types what was typed, `/` first. So `/usr/bin` and Return is that
  text.
- Esc, or Backspace with nothing typed, closes it, and the keyboard is back where it was.
- A press outside closes it and goes on to what is there.

Characters typed right behind the `/`, before the library has shown, are its query rather than the document's.

### On screen

A small list by the caret, below it, or above it where there is more room; centred at the top for a command in
Source mode, where the rendered caret is not on screen. It stays no longer than it has to: a choice, Esc, a press
outside or any change of the document ends it. Up and Down go round the rows, the pointer takes the selection
only when it moves, and the pointer is an arrow over rows. Colours are the form's `Style`, sizes follow the
window's zoom.

### Inserting

`EditorView::insert_block(block, markdown, in_source)`. The library and the command Insert Block… (⌥⌘B, in the
Paragraph menu, only with a package) both end there. Paragraph is where the app keeps what inserts structure;
it has no Insert menu.

**Where.** A block goes at the top level:

- on a blank line, at that line, the blank lines there counting as separation;
- with the caret in a paragraph, list, quote, table or code: after all of it, so nothing is split;
- with the caret in a block: after that block, or after the outermost around it. Nothing is nested by this,
  also for a preset meant to sit inside another: a package has no way to say so yet;
- in Source mode and Side by Side's source pane, by the pane's caret, by the same rules. Undo there is
  the session's Source history.

A selection counts as a caret at its end, and is not replaced.

**Bytes.** A blank line on each side, with the least that does that: what is already blank is used, and no line
is rewritten. New lines take the document's line ending, whatever the package wrote. A file ending without a
line ending stays so.

**Ids.** Every trailing `{#id}` of the preset, by the one check readers use (ADR 0018), so that text only looking
like an id stays text. An id free in the document stays as the package wrote it; another gets `a<N>`
above every number in use, the rule of ADR 0020, now over the whole document. What counts as in use is any `{#id}`
written anywhere in it, in code too: a false positive costs a new number, and the alternative is a repeat. An
answer the preset gives no id is not given one here: the block then reports it (ADR 0020).

**One undo step.** The insertion is one transaction named Insert Block; undo brings back the source, byte for byte,
and the caret, virtual place included.

**Package data is not trusted.** The Markdown must parse as exactly one paired block of the name the preset says,
from its first byte to its last blank-free end; anything else is refused with a message and nothing changes. A
format-level check (ADR 0019) only says it opens the block. After the edit, inside the transaction, the block has to
stand where it was put and every other block has to pair as it did, or the transaction is rolled back and
refused: an unclosed code fence holds whatever is put after it, for example.

**The caret and the view.** It starts native, whatever state the document's view is in, not as a card and not as a
picture. The caret goes to the end of its first place to type (ADR 0021), before a hidden id; with no view, to the
end of its first content, or the place inside an empty block. A block just inserted may not fit its schema: a
preset of a question has no answer marked yet. The rule of ADR 0021 applies: it stays native while the caret is
in it. Once left in that state it is a card with its warning, as any block.

**A large file** still opening has a part of its text, where an id could not be seen, so until the rest is in, `/` is
a character and the command is disabled.

## Consequences

- A package's texts in other languages help find a block before they are shown.
- A line starting with `/` costs one more key in a document with a package, and only on a free line.
