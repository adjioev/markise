# ADR 0024: Practice mode

Status: accepted for issue #178.

## Context

A block can hold what a reader is meant to work out: answers with correct marks, a worked solution.
In the editor those show at once, which is what an author wants and not what someone preparing from
the document wants. Blocks that hold such things can be practised in the editor: the telling parts hide, the
reader answers, and then sees how it went. This needs no helper and no site code, and Markise learns
no vocabulary: it knows a view with fields, a `choices` field with a correctness mark, and field names
a package gives it. Nothing about it may touch the file.

## Decision

### What a package says

`editor/views.json` gives a view `practice`:

```json
{ "question": { "kind": "choices", "practice": { "reveal": ["explanation"] } },
  "exercise": { "kind": "card", "practice": { "reveal": ["solution"] } } }
```

`reveal` names fields hidden until the reader asks for them. `markise-package` validates it and refuses
the package with a message naming the block: only `callout`, `card` and `choices` can have it (`media`
shows only pictures); `reveal` lists known fields, each once, not the answers themselves and not a
heading, as the outline shows every heading whatever the view hides. A block with no `choices` field has to
reveal something. A block with no `practice` has no Practice, and shows in a document in Practice as it does
outside it.

There are two kinds of practice, told apart by the schema and not by the view's kind:

- **With answers.** The block's schema has a `choices` field. The reader selects, Check says right or
  wrong per answer, shows the correct ones and reveals the fields. It counts in the score.
- **With no answers.** Show answer reveals the fields, and Reset hides them. It does not count, as there is
  nothing to be right about.

### The state

`BlockViewMode::Practice` comes after `Native` in the order of the view switch, per block and for the
document, and `BlockCan` says what a block can show. A separate command, Practice All Blocks, goes to it and
back for the document, since going through the switch's other states would run site code on the way.

### Hiding

Hiding belongs to the layout, and uses what picture blocks already use: an item that hides has no height and no
caret stop (`View::practice_hidden`, part of `disclosure_hidden`). `NativeBlock::practice` holds the answers, their marks and the
ranges to reveal, read once per version of a block with the field model, like the rest of its look. Only what
draws Practice reads them.

No part of a practised block may tell an answer, so:

- An answer's mark draws what the reader selected, and the file's mark reaches nothing until Check.
- A block that cannot be practised as it stands is not shown as anything else in Practice. Unfilled, outside
  its schema, marked nowhere, or too large to read (`MAX_NATIVE_BLOCK_BYTES`), it shows its first row with
  one reason, "Not ready to practice": the reasons it gives elsewhere can quote what it holds or count its marks.
  Switching it to another state shows what is wrong.
- The caret reveals nothing: `practice_hidden` comes before the rule that a focused item shows. A caret
  placed on what hides stands after the block.
- Find in the rendered view leaves out matches in what hides, as it does for hidden ids.

What this does not hide: Source mode and the source pane of Side by Side show the file, as they are for.
Copying a selection copies the file's text. The word count counts the file.

### Nothing reaches the file

What a reader does lives in `EditorView` only: never in the session's text, hints, history or marks, so there
is nothing to be dirty about and no undo step. `practice_refuses` turns down every mutating command when the selection touches a
practised block, and the routes around commands are closed where they start: the block's buttons, the form,
moving, deleting, link completion, code and image tools, Replace. A long selection is judged by what it holds:
any `:::` in it, while something is in Practice, does nothing. That can refuse more than it has to, and never less.

Undo and redo still work, as they are about what was done before, and if they touch a block its answers go.

### Identity

**A block's practice state is its opener's offset and its length, moved by every edit, and any edit that
touches those bytes ends it.** Answers are by their place among the block's answers, which cannot change
while the block does not. So it lasts through scrolling, paging and eviction, none of which change offsets, and
through edits elsewhere. It goes with a change to the block from anywhere, and with the document being replaced, as a
reload is. Nothing is written to disk. A content hash was the alternative, and it needs a tie-break for two equal
blocks and reads each block again.

The state of a block in the view switch (Practice or not) is kept by path for the session like any other (ADR
0021). The answers are not: they end with the viewer.

### Cost

A document with no block in Practice pays one comparison per item. In Practice an item in a card pays a map lookup by its start, and a miss
reads the block around it once. Page statistics gain, per block name, closers and quote paragraphs, gathered when a page is cut.
So an estimate for a page in a document in Practice takes off the quotes of blocks that reveal quotes and adds a row of buttons per block, with no
block read. Hidden fields of other kinds are corrected when they are laid out, like any estimate. A measured page height holds for what its blocks showed. Check and Reset happen on a
block whose page is held by the caret or the screen, and its heights are read again there; Reset All can reach evicted pages, so it ends every measured
height.
