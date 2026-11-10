# Blocks

A block is a named part of a document that a website knows how to show: a tip, a question, a
gallery. In the file it is plain Markdown between two lines:

```text
:::tip

Tap the bottom of the loaf. It should sound hollow.

:::
```

Any text editor can read and change that. What a block means comes from the **block package** of
the site you write for: which fields it has, how it looks, what you can insert. Without a package
a block is a simple card with its name on the top edge, and you can write one by hand: type
`:::name` on a new line and press <kbd>Return</kbd>, and Markise adds the closing line.

Block packages are new after 0.1.0-beta.1. If you make a site and want to write a package, see
[Making block packages](block-packages.md).

## Connect a package

A site gives you a folder with two things in it: the package, a `.tar.gz` file, and a file named
`blocks.config.json` that says which package this folder uses and pins its exact contents with a
hash. Markise looks for `blocks.config.json` next to your document and in the folders above it, up
to your home folder, and the nearest one counts. It reads nothing from the network.

The first time, a card in the corner names the package, its file and its hash, and asks whether to
use it. You can go on typing meanwhile, and blocks stay cards until you answer.

- **Use Package** unpacks a copy into your settings folder, so it goes on working without the
  `.tar.gz` file. Other documents bound to the same file do not ask again.
- **Not Now** holds until you quit. The status bar then shows `Blocks: name version · not in use`,
  and its menu has **Use This Package…**.

Use a package only if you trust where the folder came from. A hash proves that the files are the
ones the folder names, not who made them. If the file changes, Markise asks again.

Once it is in use, the status bar shows `Blocks: name version`. Its menu has the details and
**Stop Using This Package**, which deletes the unpacked copy and your yes; your documents do not
change, and their blocks are cards again. If something is wrong with the package, the status bar
says `Block package problem` and its menu says what. The blocks stay cards and the document works
as usual. A changed `blocks.config.json` counts from the next time you open the document.

## Insert a block

Press <kbd>/</kbd> on an empty line, or choose **Paragraph → Insert Block…**
(<kbd>Alt+Cmd+B</kbd>; <kbd>Ctrl+Alt+B</kbd> on Windows and Linux). The block library opens with
what the package offers. Type to narrow the list, use the arrow keys and <kbd>Return</kbd> to
choose, <kbd>Esc</kbd> to close.

`/` opens the library only on an empty line outside any block, list or quote, and only when the
package offers something or the document [has sections](#sections). Anywhere else, and in Source Code Mode, it is a slash. To type a line
that starts with a slash, press <kbd>/</kbd> twice, or type the text and press <kbd>Return</kbd>
when nothing matches.

A new block goes on the empty line, or after what the caret is in, and never splits or nests
anything. Inserting is one undo step.

## How a block shows

A block can show three ways. The button on its top edge says which comes next, and so do the
commands.

| Button | What you see |
|---|---|
| **Show Native View** | Markise's own view of the block, as the package describes it. You edit it where it stands |
| **Show as on the Site** | a picture drawn by the site's own code (macOS; see [Limits](#limits)) |
| **Show Markdown** | the simple card: the block's content as ordinary text you edit like the rest of the page |

A block the package gives no native view starts as its picture, and has only the picture and the
card. **Switch This Block's View** (<kbd>Shift+Cmd+M</kbd>) changes the block at the caret, **Switch
All Blocks' Views** (<kbd>Shift+Alt+Cmd+M</kbd>) the whole document. The choice is not saved in the
file. It lasts until you quit, and a document opens in the native view.

Source Code Mode always shows the Markdown as it is.

## Read and Edit

Every document is in one of two states, and the status bar says which, with a button to switch. **View → Read Mode** (<kbd>Shift+Alt+Cmd+Y</kbd>; <kbd>Ctrl+Shift+Alt+Y</kbd> on Windows and Linux) does the same.

**Edit** is what you know: everything shows and everything can be changed.

**Read** is for reading and practising, in any document, with or without a package. Nothing can be typed, pasted, dropped, undone or replaced, and blocks have no buttons but the ones below. **Your file does not change.** A link opens with a plain click. You can still select, copy and find. A change to the file on disk reloads as usual. Source Code Mode and Side by Side are not available in Read, and switching to Read closes them.

A package can make a document with its lessons open in Read, so that answers stay hidden from the first moment. Otherwise a document opens in Edit. Your choice for a document lasts until you quit. While you have not accepted a package, Markise does not know what it hides, so the document opens in Edit and its blocks are cards.

## Practice

Some blocks hold what you are meant to work out: a question with its marked answers, an exercise with its solution. If the package says so, such a block is practised in Read.

**A block with answers.** The marks and the explanation are hidden. Click an answer, or move to it with the arrow keys and press <kbd>Space</kbd>: one answer where one is right, several where several can be. **Check** (or <kbd>Return</kbd>) says right or wrong per answer with a tick or a cross, shows the correct ones and reveals the explanation. **Reset** (or <kbd>Return</kbd> again) starts that block over.

**A block with no answers.** Its solution is hidden behind **Show answer**, and **Reset** hides it again. It does not count in the score.

The status bar shows how it went: `3 of 5` means 3 right of the 5 blocks you have checked. **Reset All** there starts the document over.

What you select is kept in memory, for as long as the document stays open. It lasts while you scroll, and goes if the block changes or the document is closed or reloaded. Nothing is written to disk.

Nothing hidden gets out: what you copy leaves it out, Find does not find it, and while a document has blocks being practised the outline, a table of contents and the Comments panel leave out what is in them. The word count counts the whole file. A block that cannot be practised as it stands, such as one with no marked answer, shows only its first row and `⚠ Not ready to practice`. Switch to Edit to see what is missing. A block with nothing to work out, such as an exercise whose optional solution is empty, is not practised at all and shows as it is.

## Edit in place

In the native view, click and type. Each field is a place of its own: arrow keys cross between
fields, <kbd>Tab</kbd> and <kbd>Shift+Tab</kbd> jump, and a selection that starts in a field stays
in it. <kbd>Return</kbd> in a list adds an item, and on an empty last item removes it. Press on an
answer's mark to mark it. Undo works as in the rest of the document. All the keys are in
[Keyboard shortcuts](shortcuts.md#inside-a-block).

What you type stays text: `# x` at the start of a line is written so that it does not become a
heading. Parts a field cannot edit as text, such as a code block inside it, can be read but not
typed in; use the card or Source Code Mode for those. A picture has nothing to type in.

## The form

**Edit Block…** (<kbd>Shift+Cmd+E</kbd>), or the pencil on a block's top edge, opens a form beside
the document with every field of the block, also the optional ones that have no place in the view
yet. **Save** (<kbd>Cmd+Return</kbd> or <kbd>Cmd+S</kbd>) puts your changes into the document as
one undo step; it does not write the file. **Cancel** (<kbd>Esc</kbd> or <kbd>Cmd+W</kbd>) drops
them. If you are about to lose changed fields, for example by opening another document, Markise
asks first. If the block changes underneath the form, the form says so.

When the package says a block takes an argument in its opener, as in `:::figure lesson/three`, it is the
first field of the form. It is the way to change it outside Source mode: the opener line itself takes no
typing. Saving changes only the argument; the block's picture, its variant and its view follow. If the
argument is missing or not of the form the package asks for, the field says so, and you can still save.

## Move and delete

Hover over a block and drag the six dots to the left of its first row. A line shows where it will
land, <kbd>Esc</kbd> cancels, and the view scrolls near its edges. **Move Block Up** and **Down**
(<kbd>Alt+Cmd+↑</kbd>, <kbd>Alt+Cmd+↓</kbd>) do the same from the keyboard, and a block inside
another block stays inside it.

The bin on a block's top edge, or **Delete Block** (<kbd>Alt+Cmd+⌫</kbd>), removes the block with
no question. Undo brings it back exactly. Each move and each deletion is one undo step.

## Sections

Some sites divide a page into sections, each a run of text and blocks. A package whose manifest says
so, or a document with `authoring: blocks-v3` in its front matter, has them on: a line that is exactly
`---`, at the document's level, starts a section. Markise shows it as a quiet divider with the
section's number and, when it says more than the heading below it, its name, with a gap above and below and a strong line across: a division you can see when you scroll. The first section has no `---` and needs none, so it has no number either. The outline and status bar follow. A package can have each section drawn as a card instead. This is only how it looks: the file does not change. What a section means is the site's business.

### Start a section

The block library (<kbd>/</kbd> on an empty line, or **Insert Block…**) has an item for it, first in the list and shown only while the document has
sections. It says `Section`, or whatever the package calls one. A document with `authoring: blocks-v3` of its own needs no package for this.

Choosing it opens a small form: a title, and metadata only if the site declares some. **Save** (<kbd>Return</kbd>) writes it after what the caret is in, at the
document's level, never inside a block or a list, as one undo step, and puts the caret at the end of the title. **Cancel** (<kbd>Esc</kbd>) writes nothing.

```text
---

## Title {#id}
```

The form makes the `id` from the title as you type: lower case, in Latin letters, Cyrillic written out by a fixed table (`щ` is `shch`, `ю` is `yu`), with a hyphen between words and a number added if the page has it already. A title with no usable letters gets `section-1`. You can edit it: a letter, then up to 63 letters, digits, `_` and `-`, not used elsewhere on the page.

With nothing before it, at the top of the document or in a section that is still empty or has only its metadata, no `---` is written above it, as that would make a section with nothing in
it. If something follows, the `---` goes after the title instead, and what follows stays a section of its own. What would leave a section with only metadata is refused with a message, as a site refuses such a section.

### Move and delete a section

**Move Section Up** and **Down** (<kbd>Shift+Alt+Cmd+↑</kbd>, <kbd>Shift+Alt+Cmd+↓</kbd>; <kbd>Ctrl+Shift+Alt+↑</kbd>, <kbd>Ctrl+Shift+Alt+↓</kbd> on Windows and Linux) and **Delete Section** (<kbd>Shift+Alt+Cmd+⌫</kbd>; <kbd>Ctrl+Shift+Alt+Backspace</kbd>), in the
Paragraph menu, act on the section holding the caret. Under the pointer a divider has the same three as small buttons for the section it opens. Read has none of this.

A section moves past one neighbour, with everything in it, its metadata first as it was. Its bytes do not change, and blank lines around the dividers it touches come out as one on each side. The first section has no `---` above it, and whichever comes to be first still has none: the `---` stays between them. Front matter stays where it is.
A first section with nothing in it stays first, as a section cannot take its place without a divider above it. If something would pair differently afterwards, such as a stray `:::` in each of two sections, a message says so and nothing changes.

A section with nothing in it or only metadata moves like any other. A site may refuse such a section when it is published, so a short notice says so when a move or a new section involves one.

Delete takes the section's bytes and one divider, with no question: a short notice says so and has **Undo** for a few seconds. The only section of a document cannot be deleted this way. Each move and each deletion is one undo step that brings back the exact file.

### In the outline

In the outline a section's row reads differently from a heading's: it has its number, or `§`, on a ground of its own, with the headings indented under it. A section goes by its `id`, else its first heading at the package's `title_level` (normally `##`), else its first heading of any level, else what the package calls a section with its number, `Section 3`. The heading that names it has no row of its own.

The document's `#`, when it is the first thing in the first section, is its title and no section's name: it has a plain row above the sections, with nothing to press but itself, which goes there.

A section's row has a menu: right-click it, or use the small **⋯** at its start, in place of the number, when the pointer is on the row. Heading rows have none. It has **Move Up**, **Move Down**, **Duplicate**, **Insert After…** and **Delete**, and works on that section wherever the caret is. With the keyboard, Tab to the row and press <kbd>Shift+F10</kbd>, or go on to the **⋯** and press <kbd>Return</kbd>; the arrows go through the items and <kbd>Esc</kbd> goes back. Read has no menu.

**Duplicate** puts a copy right after the section, one undo step. The copy is the same text but for the ids that must be unique on a page: a heading's `{#id}` and the `id` in its metadata get a new number, `intro` becoming `intro-2`, and a first heading with no id gets one. What is inside a block stays as written, its answers' ids included.

**Insert After…** opens the form of [a new section](#start-a-section), for after this one.

To drag a section, take the grip that shows beside its row. The cursor says so from the press, and a line shows where it would land; the headings under sections fold away while you drag, and the list scrolls at its edges. Dropping it on its own place does nothing and <kbd>Esc</kbd> cancels. It is one undo step, and what a move refuses a drop refuses, with the same message. A change to the file meanwhile, or another document, ends the drag, and keys edit nothing while you hold it.

The first section may hold the document's `#` title and intro. If a move takes the only `#` heading along, or brings another section above the first one holding it, the notice says that the title moves too. Nothing is refused for it.

## Images

A picture in a block's native view belongs to the form: **Choose file** opens the image picker. A
picture in a card or in ordinary text has **Replace…** and **Edit…** when you hover over it. Both,
and the picker, are described under [Images](editing.md#images).

In the rendered view an image shows at most 400 points high, in a block or out of one, scaled with
the window's zoom; a double click shows it whole. The file stays as it is. A site's picture is the
site's drawing and has no such cap.

## The ⚠ badge

A `⚠` on a block's first row says that it does not match what the package expects. Hover over it to
read why. It stops nothing: you can edit and save the block, and whoever publishes decides how
strict to be.

- **Not filled in yet:** a required field is empty or has too little in it, or the block's argument
  is missing or not of the form the package asks for. The block stays in its native view; a block with no native view shows as a card. A block fresh from the library often
  starts like this.
- **Outside its schema:** content that fits no field, arguments the package does not know, or
  something else the view cannot read. It shows as a card. While the caret is in it, it stays as it
  was, so that it does not change under your typing.

A block with a `⚠` has no site's picture, since a site's code can only be expected to draw a block
that fits.

If a site's code cannot draw a block, the block stays a card with a quiet reason on its first row,
and no `⚠`.

## Language

A package's words follow the document: set `lang` in its front matter (`lang: ru`, `lang: ru-RU`)
and block labels, the block library and the form's labels show in that language, if the package has
it. Otherwise they show in the language before the `-`, then English, then whatever the package has.
With no `lang`, English. Changing `lang` changes them at once, and an open form keeps what you typed.
Search in the library finds a block by its words in any language.

Markise's own words stay English: Save, Cancel, the reasons it gives, and the sentence around a block's
name. A field the package gives no label to shows its name, and a block without a view shows its name
from the file.

## Limits

- **Site pictures are macOS only, on macOS 14 or later.** On Windows, on Linux and on older macOS,
  native views, the form and everything else work, but there is no **Show as on the Site**: a block
  with no native view shows as a card.
- **A picture is not interactive.** Clicks in it do not reach the site's code. A click puts the
  caret before or after the block.
- **A picture shows the block as it stands in the document.** It is drawn again when the block
  changes. If you replace an image file on disk and leave the block's text alone, an older picture
  can go on showing until the block changes.
- **Exports use neither views nor pictures.** HTML, PDF, PNG and Pandoc exports show a block's
  content with its name as a label, whichever view is on screen.
- **A package's code is kept apart.** It runs only for a package you accepted, in a separate
  process with no network, and gets only the package's files and the block's own images.
  See [Updates and privacy](updates-and-privacy.md).
