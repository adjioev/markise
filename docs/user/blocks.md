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
package offers something. Anywhere else, and in Source Code Mode, it is a slash. To type a line
that starts with a slash, press <kbd>/</kbd> twice, or type the text and press <kbd>Return</kbd>
when nothing matches.

A new block goes on the empty line, or after what the caret is in, and never splits or nests
anything. Inserting is one undo step.

## How a block shows

A block can show several ways. The button on its top edge says which comes next, and so do the
commands.

| Button | What you see |
|---|---|
| **Show Native View** | Markise's own view of the block, as the package describes it. You edit it where it stands |
| **Practice** | answers to give, with what tells them hidden, for a block whose package offers it: see [Practice](#practice) |
| **Show as on the Site** | a picture drawn by the site's own code (macOS; see [Limits](#limits)) |
| **Show Markdown** | the simple card: the block's content as ordinary text you edit like the rest of the page |

A block the package gives no native view starts as its picture, and has only the picture and the
card. **Switch This Block's View** (<kbd>Shift+Cmd+M</kbd>) changes the block at the caret, **Switch
All Blocks' Views** (<kbd>Shift+Alt+Cmd+M</kbd>) the whole document. The choice is not saved in the
file. It lasts until you quit, and a document opens in the native view.

Source Code Mode always shows the Markdown as it is.

## Practice

Some blocks hold what you are meant to work out: a question with its marked answers, an exercise with
its solution. If the package says so, such a block can be practised. Choose **Practice** on its top
edge, or **View → Practice All Blocks** (<kbd>Shift+Alt+Cmd+Y</kbd>; <kbd>Ctrl+Shift+Alt+Y</kbd> on
Windows and Linux) for the whole document. Press the same command to leave.

**A block with answers.** The marks and the explanation are hidden. Click an answer, or move to it
with the arrow keys and press <kbd>Space</kbd>: one answer where one is right, several where several
can be. **Check** (or <kbd>Return</kbd>) says right or wrong per answer with a tick or a cross, shows
the correct ones and reveals the explanation. **Reset** (or <kbd>Return</kbd> again) starts that block over.

**A block with no answers.** Its solution is hidden behind **Show answer**, and **Reset** hides it
again. It does not count in the score.

The status bar shows how it went: `3 of 5` means 3 right of the 5 blocks you have checked. **Reset All** there
starts the document over.

**Your file does not change.** What you select is kept in memory, for as long as the document stays
open: the document does not become unsaved, and there is nothing to undo. It lasts while you scroll, and goes
if the block changes, from here or on disk, or when the document is closed or reloaded. Nothing is written to disk.

While a block is being practised you cannot type in it, and it has no Edit, Delete or Move buttons;
switch its view to edit it. A block that cannot be practised as it stands, such as one with no marked answer, shows only
its first row and `⚠ Not ready to practice`. Switch its view to see what is missing.

Source Code Mode and the source side of Side by Side show the file as it is, answers included, and a
selection you copy is the file's text. Find in the rendered view leaves out what is hidden. The word count counts the whole file.

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
