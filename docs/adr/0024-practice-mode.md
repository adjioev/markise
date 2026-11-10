# ADR 0024: Read mode and Practice

Status: accepted for issue #178.

## Context

A block can hold what a reader is meant to work out: answers with correct marks, a worked solution.
In the editor those show at once, which is what an author wants and not what someone preparing from
the document wants. A document can be read as well as edited, and in Read blocks that hold such things are practised: the telling parts hide, the
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

### Read and Edit

The main control is one switch per document, Read or Edit: the command Read Mode (View menu, Shift+Opt+Cmd+Y; Ctrl+Shift+Alt+Y) and a button in the status bar that says which it is. It works in any document, with a package or with none.

- **Edit** is as it was: everything visible and editable, answers shown, and the view switch of a block goes through its native view, the site's picture and its Markdown.
- **Read**: the session is read-only (`EditorSession::set_read_only`, the one point every change goes through), so no typing, paste, drop, undo, Replace, block command or form. No block controls show but Check, Reset and Show answer, and the toolbar's formatting is off. Blocks with a `practice` are in Practice whatever their state in Edit, and have that state again after. Other blocks and text simply read. A link works with a plain click. Selection and Find work.

**There is no switch per block, and no Practice All**: the first steps of this stack had them, and with Read as the main control they were a second way to the same place, so they went. `BlockViewMode::Practice` remains, as what a block is in Read, and nothing switches to it.

**An open form closes on entering Read**, as it shows the answers. A changed draft is asked about first, Save, Discard or Cancel, and Read waits for the answer.

**An answer reads like a list line.** Before a check nothing marks a row but its control. After one, a tick on a correct answer and a cross on a chosen wrong one sit right after the text; a wrong answer not chosen has nothing. Check, Reset and Show answer are quiet text a paragraph's gap below. A thin ring within the column shows where the keyboard is, only while it is the keyboard that went there.

**Copying** in Read leaves out what is hidden (`EditorView::practice_copy`): a hidden field goes, a mark reads `[ ]`, and of a block that cannot be practised only its marker lines stay. What is shown after a check is copied as it stands. Export is a choice about the file, and copies all of it.

**The source pane is not available in Read**: entering Read closes Side by Side or Source mode, and they cannot be opened until Edit. This is the simpler of the two choices: an editable source pane beside a read-only document would be a second state to explain, and it would show the answers.

**The file never changes.** Nothing can make the document dirty in Read. Unsaved edits made before entering Read stay unsaved, and saving them still works: Read does not discard them. A change on disk reloads under the existing rules, as replacing the document goes past read-only, and what was answered goes with it.

### What a document opens in

`"practice": {"reveal": [...], "default": true}` on a view makes a document that holds a block of that view open in Read (`PackageViews::read_by_default`, a search of the text checked against the parsed blocks, once when the package is known and once when a large file has come whole). Any other document opens in Edit. Other packages are not affected.

The package has to be known before the first frame that could show an answer, and it is looked up off the UI thread. So a document opens
**veiled** (`View::set_blocks_veiled`) where packages can be in use: blocks show only their first rows until the lookup has answered, and Side by Side draws only the rendered pane. A document
with no package loses those few frames of what its blocks hold, and nothing else. Until a package has been accepted Markise does not use it, so at the
question, or after Not Now, the document is in Edit and blocks are cards: there is nothing yet to say what hides.

The choice is per document for the session, kept by path with the rest of its state (ADR 0021), and **what someone chose wins over the package's word**. The next session starts from the package's word, as nothing of this is on disk. A block inserted from a preset is inserted in Edit, so it shows as ever.

### What lists or finds text

- **Outline and `[toc]`.** A heading can stand in a quote or in content no field takes, and a list of headings cannot read every block to tell. So while any block is in Practice, these leave
  out every heading directly in a block of a name that can be practised (`OutlineEntry::block`, `View::practice_hides_heading`). `reveal` cannot name a heading, for the same reason.
- **Find.** It leaves out matches in what hides, so there is no count, highlight or jump for them. They come back when revealed.
- **Word count.** A number for the file, the same in every state. It holds no text. It does not count less in Practice, which would tell how long an answer is by the difference.
- **The Comments panel** leaves out comments and suggestions on what hides. Accept All and Reject All are the document's, and a block they change loses what was done in it.

### The state

`BlockViewMode::Practice` is what a block with a `practice` is in Read (`View::set_read_mode`). A block with none, and a document with none, are not touched by it.

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

The word count counts the file.

### Nothing reaches the file

What a reader does lives in `EditorView` only: never in the session's text, hints, history or marks, so there
is nothing to be dirty about and no undo step. Every other change is refused at the session while in Read.

### Identity

**A block's practice state is its opener's offset and its length, moved by every edit, and any edit that
touches those bytes ends it.** Answers are by their place among the block's answers, which cannot change
while the block does not. So it lasts through scrolling, paging and eviction, none of which change offsets, and
through edits elsewhere, made in Edit. It goes with a change to the block from anywhere, and with the document being replaced, as a
reload is. Nothing is written to disk. A content hash was the alternative, and it needs a tie-break for two equal
blocks and reads each block again.

Read or Edit is kept by path for the session (above). The answers are not: they end with the viewer.

### Cost

A document with no block in Practice pays one comparison per item. In Practice an item in a card pays a map lookup by its start, and a miss
reads the block around it once. Page statistics gain, per block name, closers and quote paragraphs, gathered when a page is cut.
So an estimate for a page in a document in Practice takes off the quotes of blocks that reveal quotes and adds a row of buttons per block, with no
block read. Hidden fields of other kinds are corrected when they are laid out, like any estimate. A measured page height holds for what its blocks showed. Check and Reset happen on a
block whose page is held by the caret or the screen, and its heights are read again there; Reset All can reach evicted pages, so it ends every measured
height.

## A block with nothing to practise

A block whose view has `practice`, with no answers and with nothing to reveal, as when what would be revealed is an optional field left empty, has nothing for anyone to work out. It is not practised: it fits, and in Read it shows as it does anywhere, with
no `Not ready to practice`. `markise-cli blocks check` and the view agree on what a block is: one with answers and none marked is what the view says is not ready, and the check says the same of it, as not filled in. The exemption is only for answers that are really absent. A list only some of whose lines have `[x]` or `[ ]` is no answers by the field model, so a text field takes it in. Its marks say what they say as text, and nothing hides them, so a line reading so that no answer holds makes a block not ready, whatever else it has, and the check reports it. It is by the lines of the block, except where the mark itself stands in a fenced or indented code block (a box with a fence after it on its line is still a box): a mark there is an example written as text, and no answer anywhere, in Practice, in the check and in what is not ready (issue #178, from the audit of primitives; it was by every line, code too, before).

Everything else about what cannot be practised stays as above.
