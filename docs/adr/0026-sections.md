# ADR 0026: sections

Status: prototype for issue #178, phase 9, of a draft syntax that may still move. Showing sections, starting one, and moving and deleting one, also from the outline.

## Context

Between ordinary content and blocks some sites have a third level: a page is a list of sections,
each a run of content and blocks with a little metadata of its own. An editor should show where
sections start and what they say, with no knowledge of what any site makes of them. The syntax is a
draft, implemented nowhere yet, so this has to be small and easy to change. The file never changes
on opening or viewing.

## Decision

### The syntax

- **A dialect switches it on** (below). Without it `---` is a line and nothing changes.
- **A boundary** is a root-level thematic break whose line is exactly `---`, as the parser reads the
  tree, never by a line search. `***`, `___`, `----`, `- - -`, ` ---` and `--- ` stay lines. `---`
  directly under text is a setext heading, as Markdown says. Inside a list, a quote or a paired
  `:::` block it is no boundary, and front matter delimiters are none.
- **Metadata** is a paired `:::section-metadata` block that is the first block of its section. Any
  other one is an ordinary block, flagged when the rendered view shows it. Between the markers the parsed
  tree holds exactly one paragraph, with a blank line on either side of it, and no other node. Its
  lines have a grammar of their own, not Markdown and not the block field grammar: `key: value` split at
  the first colon, both trimmed, values literal. Keys are `[a-z][a-z0-9-]*`, values one line up to 200
  characters, at most 20 keys. Only `id` and `style` mean anything to the engine; all keys are data here.

### The dialect

Sections are on when the document's dialect is `blocks-v3`. The document's front matter `authoring`
says, and if it does not, the manifest of its accepted package, a flat `"authoring": "blocks-v3"`.
Front matter wins, so another value there turns them off whatever the package says. An empty value, or
one that is no string in YAML, is an error shown quietly: sections are off, and it never falls back to the
package. A document needs no package: its own front matter is enough. A package without the key
reads as before, and this key is read, not validated (ADR 0019: a known format ignores what it does
not need). `sections::section_dialect` is the whole rule, and it says where the answer came from,
with the package's version when it came from a package.

### Tolerating what does not fit

The reader is a small line parser into ordered pairs, by the lines of the source between the markers,
so that a line Markdown would read otherwise (`---` under a line, `# x`) is seen for what the author
wrote. A line that does not fit is flagged with a reason and takes nothing else with it. A long
value is kept and flagged; of a repeated key the first counts. Refusing is for whoever publishes.
Anything but that one paragraph between the markers is malformed, and so is a marker with a line right
next to it: both flagged, with the lines still read for what they say. A fence inside takes the closer, so
the opener pairs with nothing, and that is flagged as well. A block over 64 KiB, markers included, is not read,
since this is asked on the UI thread, and counts as no metadata; its extent comes from the index, so
that no line of it is read to find that out.

### The index

`section.rs` in `markise-md` is built like the directive index (ADR 0018) and kept in the same places:
two chunked relative-offset lists of rules, those at the root and those inside a directive. It
comes from the edit's re-parsed top-level blocks, so it depends on no item or page of any view model
(ADR 0016), and it always stands, whatever the dialect: the cost in a document without sections is an
empty list and a few comparisons per re-parse.

- **An edit that touches no rule** shifts the later ones, O(chunks). The fast path and a re-parse
  inside one container do only that.
- **A re-parsed region** takes the rules of its new blocks.
- **A change of pairs** can bring a far-away rule in or out of a block. Only rules between the markers
  whose pair changed and the region can do that, and only those are read again.
- **A section's number** is the position of its boundary in its list, so `section_at(pos)` is
  O(log rules), with no count over the document.
- **Nothing is cached about metadata**: it is read from the text when asked, O(log) plus its size. The
  view asks for what is on screen.

`FormatDoc` has `section_count`, `section`, `section_at`, `section_metadata` and `authoring`, so layout
stays on the seam. A format with no boundaries has one section.

### Progressive open

A first part (ADR 0017) is a real prefix, so every section but the last ends where it will in the whole
document, and the last one is cut short: its extent is provisional, which a reader of a partial
document knows from `partial`. One thing can be wrong before the whole is in: a rule inside a
block whose closer lies after the cut stands at the root in the part, since that opener pairs with
nothing yet. It is corrected with everything else, since the whole document has its own index.

### On screen

Nothing here is new layout state in the model: as with a picture (ADR 0023), the same items show otherwise.
A document without sections pays one comparison per item.

- **A boundary** reads as a division, stronger than a rule and in every document with the dialect on, with no option needed: a generous gap above and below, the section's number from 1 in a small badge at its start, then the line to the right edge, 2 pt in a colour between the text's and the page's. Between them stands its name only when that says more than what stands below: its `id` unless the first heading says the same. A heading says itself, and `Section N` would say the number twice. The section's own heading keeps its style. No caret stands in it, as in
  any rule. It is taller than the line it stands for, so an item's estimate and a page's (`section_page_estimate`: boundaries in its range, O(log rules)) count that, and they come from the one function that lays it out (`divider_metrics`). The name for the outline and the status bar is as it was: its `id`, else its first heading among the section's own top-level blocks (a bounded look, as this is asked on the UI thread), else `Section N`. Only `id` and `style` are engine keys, and no other key is ranked.
- **The first section** has no boundary, and gets no number or line. It has no item to hold them, as nothing may be written for it, and looked at on a real page it needs none: the page's start says it. A first section with nothing in it does not count in the numbers.
- **A rule nearly a boundary** (`----`, `- - -`, ` ---`, `--- `) at the root says so on its own row, quietly.
- **A metadata block** is a compact plaque: `key: value` as written, and `⚠` with the first reason for what
  does not fit, all of them on hover. Its opener item holds it, the content items are hidden as a closed
  disclosure's are, the closer has no height and the gap after it stays. Pages, marks and source positions
  go on as they do.
- **Its text stays editable.** The block's state (`BlockViewMode`, ADR 0021) is the toggle: `Markdown` is
  the generic card, anything else the plaque, with one button on it, kept per block for the session
  as any block's state is. A selection's end inside it shows the card too, so that what is found or undone
  can be seen; asking for the plaque with the caret inside puts the caret after it.
- **The caret** goes past a plaque as past a picture: keys skip it and a click lands before or after it.
- **Source mode** shows the text; Side by Side's rendered pane is the rendered view.
- Everything is in points from the theme, so theme and zoom apply and the layout cache keys on them, the
  name and the plaque's contents included.

### A package's say in how sections look

`"view"` in `editor/sections.json`, optional and validated with the file: `frame` and `numbered`, true or false, and `accent`, a colour's name as a view's (ADR 0021). The defaults are as above; `numbered: false` leaves the badge out, and `accent` colours the line and the badge.

**With `frame`** each section is a card: a rounded border, a faint ground, room inside, and a clear gap between sections, in place of the line. It is one even shape across items and pages since it is what a native block's ground is (ADR 0021): every item of the section has a band, and `merged_bands` makes one rectangle of them, with `stroke` and `radius` now part of a band. A boundary's item holds two: the end of the frame above, which `closes` so that nothing goes on from it, and the start of the one below. A host is given only the items on screen, so a run that does not begin at a band that `opens`, or does not end at one that `closes`, goes straight off the view by more than a corner and its edge: edges and corners are for a section's true ends, also across pages. What stands in it draws above it: a block's frame and ground stay as they are, and read well on it. So heights are only the boundary's, and an item's layout is the same but for its bands. The column stands in by the frame's padding (`section_frame_inset`), so the frame takes no room from the page's margins and text wraps for the width it has. Selection, the caret and the hover controls go by the same items. Front matter stays outside: the first frame starts under it, and with none just above the first item.

### Finding one's way

The outline panel has every section as a level above headings, and a section's row reads as one: its number (or `§` where a package says no numbers, and for a first section with nothing in it) in a slot of its own, its name in the bold face on a ground of its own, with the headings indented under it. A heading's row has no control of any kind.

What a row is called: its `id`, or the first heading when that is what it goes by, and that heading then has no row of its own under it, as it would say the same; other headings stay. With a heading in a block before it the outline lists that first, so the naming heading keeps its row. **The first section's `#`** is the document's title and no name of its section, since looked at on a page it reads as the page's heading and a row called that reads as the heading too, which made a grip beside it seem to belong to the title. So that row says what sections are called (`Section`, or the package's word), with its number, and `#` stands under it as a heading row. A section with no heading at all reads the same way. The status bar goes on saying `section_name`, as it has room for only one word about where one is.

Its rows are not built: the row of section
`k` is `k` plus the headings before it less the headings left out before it, so the rows on screen are found by bisection over sections, and a
list of any size costs what is shown. The count of those left out is the one thing that goes over all sections, reading only what each starts with, and only while the outline is shown: about 7 ms at 4,961 sections with metadata. Typing in an item that is no heading and in no block keeps it, as nothing else about it changes, so typing does not pay for it; a structural edit does, once. The model's outline names a heading in a placeholder by the
placeholder, so a boundary in one reads that page's items, without materializing it, and remembers the count
until the document changes. A click materializes the target's page and goes there as a heading's does.

The status bar names the current section: the caret's while the caret is on screen, else the one at the top
of the screen. On hover it says which of how many, and where the dialect came from, with the package's
version. While a document is partial the last section's name ends in `…`, the count in `+`, and the outline
says that more is to come. A front matter `authoring` that is empty or no string shows as `§ off`, its reason on
hover.

Measured with `cargo bench -p markise-layout --bench layout -- sections/`: 10 MiB, about 4,700 sections, each
with metadata, a paged view, macOS, release, 2026-11-06, with other builds running on the machine, so these
are upper bounds and the small differences are noise:

| | sections | no boundary |
|---|---|---|
| Parse | 54 ms | 42 ms |
| A keystroke in a paragraph (`Document::edit`) | 0.31 ms | 0.26 ms |
| `section_at` | 66 ns | |
| A warm frame, sections off / on | 8 µs / 15 µs | |
| 40 outline rows with nothing remembered, a new model included | 1.1 ms | |

### Read mode

In Read (ADR 0024) dividers, frames and plaques read as they do in Edit, with their warnings. No control shows, and a
block opened as a card in Edit is a plaque there and a card again after. In Edit a plaque's switch follows the
rule for a block's controls: it shows under the pointer or the caret. While a block is in Practice the outline keeps
every section and leaves out the headings that ADR 0024 does, and a name never comes from a heading in a block.

### Exports

With sections on, HTML, PDF, PNG and Pandoc leave a root-level `:::section-metadata` out: they all read
`Document::export_tree`. The dialect comes to an export as a request's flag, for what a package says, and from
the document's own front matter, which wins; the command line knows only the front matter. `---` in exports stays
as it is for now: `<hr />`, a line in PDF and PNG, `HorizontalRule` for Pandoc.

### Writing

On a page with sections Horizontal Line writes `***`, as it would start a section otherwise (`EditPrefs::
rule_stars`, set while sections are on). A typed `---` and Enter does make a boundary, which is correct, and
Backspace at the start of what follows takes it, as for any rule, with an undo step.

### Starting one

`---` is no block, so ADR 0019's rule that a preset opens the block it names stays, and a new section is no preset: the library has one item of another kind
(`LibraryItem::Section`), first and in no group, while the document's dialect has sections. It needs no package, as sections do not. What it says comes from
`editor/sections.json`, optional and validated like the package's other files: `label`, default `Section`, `description`, `keywords`, `icon`, `title_level`, the level of the heading a new section starts with, 1 to 6, default 2, as a site's outline is the site's, and `fields`, the keys the form asks for, with
values to choose from if any. A site may call a section what it likes, and Markise does not know what.

Choosing the item opens a modal form. Nothing is written before Save. An edit, another document or Read closes it with nothing written. `EditorView::insert_section` writes, at root level by the rule that places a
block (`line_for_block`), with a blank line each side by `separated`, one undo step named Insert Section, the caret at the end of the title, before its hidden id:

- **A boundary above it**, if the section it lands in has anything before that place but its metadata. A boundary with nothing above it would be an empty section, and with only metadata above it a section a site refuses all the same, and on the first line of a
  file it can be front matter. Then the heading goes with no boundary, and if something other than a boundary follows, a boundary after it keeps that a section of its own.
- **Refused, for what it would leave**: new metadata in a section that has its metadata and nothing else, where a second block has no place and a boundary would leave that section as it is; and a boundary with only metadata below it. A message says so.
- **Refused**, nothing changed: the heading has to stand as a heading with its id, the number of sections has to be what was meant and every block has to pair as before. An unclosed fence that takes what is put after it is the case, and
  a lone first-line `---` that a second one would turn into front matter.
- A title is one line. It is Markdown as any heading's text is. A trailing `{#x}` typed in it stays text, since the id written after it is the last.

**Its id** follows the title until someone edits it (`markise_engine::section_id`). A letter, then up to 63 letters, digits, `_` and `-`, as sites read an id. From a title: lower case, Latin only, Cyrillic by a fixed table, so that the same title gives the
same id everywhere, whatever a machine's language says; anything else that is no ASCII letter or digit separates words, so a letter of another script does not carry over, and an apostrophe joins. It starts at the first letter. A title with no usable letters is `section-1`, the first free number. On a page where the id is taken it gets the
first free number after a hyphen, `-2` on, within the length. Taken is any `{#id}` written anywhere on the page, also in code, as for a block's ids (ADR 0022), and any section's metadata `id`: all of them share one namespace on a site. The form reads that once, when it opens, as an edit closes it. An edited id that is no id or is taken cannot be saved, and the form says why. The table:

| | | | | | | | | | |
|---|---|---|---|---|---|---|---|---|---|
| а a | б b | в v | г g | ґ g | д d | е e | ё e | є ye | ж zh |
| з z | и i | і i | ї yi | й y | к k | л l | м m | н n | о o |
| п p | р r | с s | т t | у u | ў u | ф f | х kh | ц ts | ч ch |
| ш sh | щ shch | ъ – | ы y | ь – | э e | ю yu | я ya | | |

### Moving and deleting one

`EditorView::move_section` and `delete_section`, in `editor/section_move.rs`, by what a block's move does (ADR 0022): `lift` and `separated` around the bytes, a transaction of one deletion and one insertion, named Move Section or Delete Section, and the check that every block
pairs as before. The commands Move Section Up, Move Section Down and Delete Section act on the section holding the caret, whichever pane it is in, also in Source mode, and are in the Paragraph menu while the dialect has sections, with or without a package. The chords (⇧⌥⌘↑, ⇧⌥⌘↓, ⇧⌥⌘⌫; Ctrl+Shift+Alt+↑, ↓, Backspace) are checked with the blocks' for clashes.

**A section's bytes** are from its first line with anything on it to the end of its last, blanks at the end of that line included. Blank lines around it are not its own, and only those are written. A file's end, with or without a line ending, stays as it is.

**A boundary is between sections** and belongs to neither: a section moves past one neighbour, and the two change places around it. So of two sections the upper one's bytes are left alone and the lower one goes above it, whichever the command was for: with the boundary it opens when the upper one has one, or, when the upper one is first and has none, with that boundary after it. The first section therefore never gets a boundary, and front matter stays first. What is in a section stays in it: metadata stays first in it, as bytes.

**What it cannot do.** A first section with nothing in it has nothing to change places with, as a section in its place would need a boundary above it; so the commands are off for its neighbour going up. A document's only section cannot be deleted. **Refused**, nothing changed: every section's bytes have to stand at their new places, the number of sections has to stay and blocks have to pair as before, with a message saying so. A stray `:::` that comes to pair, or an unclosed fence that comes to hold what is put after it, are the cases.

**A section with nothing or only metadata**, which a site refuses, is not moved, and nothing moves past it: the move is refused with a message that says to fill it in or delete it. That is the simple rule; a move cannot make such a section, as all bytes stay in their sections, and refusing keeps one from drifting around unnoticed. One elsewhere in the document stops nothing. A delete cannot make one either, for the same reason, and is how one goes away, so it is not refused.

**Delete** takes a section's bytes and its boundary, with the first section's next boundary, so that the next section comes to be first. One undo step, with no question; a changed form's draft of a block in it is asked about first, as a block's delete does, and a move waits while one is open, also as a block's does.

**On screen.** A divider has three quiet buttons at the right end of its row for the section it opens, under the pointer on that row or for the section with the caret, as a block's controls. A button is there only for what can be done, and only while it shows: what does not show is not there to press. The row stands whether or not it says a number or a name, as the buttons stand on it. In Read no button or command does anything, and a part of a large file still loading has them off, as its last section is cut short.

### From the outline

A section's row in the outline panel has a menu, by a right click on it and from a quiet `⋯` at its start, in the place of its number, that shows under the pointer with the whole row lit: Move Up, Move Down, Duplicate, Insert After, Delete. It says what is there to do by the same checks as the rest (`can_move_section`, `can_delete_section` and so on) and goes by the moves and refusals above, with their messages; the words in those come from the package's section label. By the keyboard: Tab comes to a row and on to its `⋯`, Return or Shift+F10 opens the menu on its first item, the arrows go through it, and Esc goes back to where it was opened from. Read, a read-only file, a part of a large file still loading and an open card have no menu and no `⋯`.

**Delete** by any way there is, these two as well, is followed by a notice in the banner above the document, with Undo: one line, for eight seconds and for as long as nothing else has changed the document. It is not put over a question about a file changed on disk. Undo is the ordinary undo of that step.

**Duplicate** (`duplicate_section`) puts a copy right after the section, its own boundary above it, as one undo step. Only the ids in the copy differ from its bytes, and only those unique on a page: a heading's outside a block, by what a reader takes for an id (`hidden_note`, so not what stands in code), and the value of the metadata's accepted `id`, by the position the reader found it at and with the blanks around it staying. Each gets one that is free on the page by the rule of a section's id above. Everything inside a `:::` block stays as written, as its ids belong to its fields and can mean something (ADR 0022): `{#none}` stays `{#none}`. A first heading with no `{#id}`, when it is written with `#`, gets one, as two headings saying the same would give a site one anchor; it is past what the original's text makes, so `## Two` becomes `## Two {#two-2}`. A setext heading or one with a closing `#` has no place for one and stays as it is. A section with nothing or only metadata has nothing to copy, and what would pair or divide otherwise is refused as for a move.

**Insert After** opens the section form, whose Save goes as Starting one says, at the end of that section wherever the caret is (`insert_section_after`). A first section with nothing in it has no end to go after, so it is off there.

**The title.** The first section may hold the document's `#` heading and intro, and that is no reason to refuse anything: a site's rules for what a section holds are the site's. When a document has exactly one `#` heading outside a block (`only_title_section`), a move that takes it along, or that brings another section above the first one holding it, says so in that notice, in one line.

**A drag.** A section's row has a grip beside it under the pointer, in a gutter the list keeps so that no row moves when it shows, with the Grab cursor, and Grabbing from the press. From the press a line stands between rows at the place nearest the pointer: strong where the section would land, quiet and dashed on its own place or where it cannot go. While it lasts the list has only its sections, so that it is short, with the row picked up under the pointer; the headings come back after it. The list scrolls at its edges once the pointer has left where it was pressed, as a block's drag does (ADR 0022). Esc ends it. Read and a part still loading have no grip, and neither has a section with nowhere to go. While a section or a block is being dragged the keys edit nothing (Esc is the one the drag reads), whoever holds the keyboard, and Esc comes before the release, in one frame too.

A drop is `move_section_to`, what a step is too: the section and the run it goes past change places by `plan_past`, as one undo step, with the same checks of every byte, of the number of sections and of what pairs, and the same refusals with their messages. A section with nothing or only metadata anywhere in that run stops it, and one elsewhere stops nothing. On its own place, or where a first section with nothing in it holds the place, it does nothing and makes no undo step. An edit, a reload or another document during the drag ends it, and what was dropped after that does nothing; the app checks again that the drop is for the document and the revision it was made for.

No frame of it walks the document. The rows on screen each ask for a name, O(log sections) and a bounded look, and the place to land is by the pointer and two ends of runs. `only_title_section` is a walk over the top-level blocks, and goes once, at a drop that concerns a section holding a `#`. Measured with `cargo bench -p markise-ui-egui --bench section_panel`, through the real app with no GPU: 10 MiB, 4,961 sections, each with metadata, macOS, release, 2026-11-07, with other builds running on the machine, so upper bounds:

| | p50 | p99 |
|---|---|---|
| A frame, the panel at rest | 0.34 ms | 0.78 ms |
| A frame of a drag, the pointer moving | 0.36 ms | 1.2 ms |
| A frame of a drag, the list scrolling | 0.33 ms | 0.61 ms |
| A drop past 36 sections | 6.4 ms | |
| A move past all of them, once / its undo | 123 ms / 99 ms | |
| The title's walk, once | 52 ms | |

## Consequences

- The bytes of a file do not depend on any of this: the index only reads.
- What moves blocks does not know sections: a block dropped above a plaque is allowed, and the metadata
  block then shows as a card with the warning above. No move is refused.
- A metadata block not first, or a second one, is a generic card with a warning.
- A metadata opener with no closer stays text, with a warning under it.
- A first line `---` with a later `---` is front matter, as ever, so a document that starts with a
  boundary and has no front matter loses its first two boundaries to it. The draft refuses a leading
  boundary anyway.
