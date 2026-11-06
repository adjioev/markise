# Making block packages

A block package tells Markise about the blocks of your site: their fields, how they look, what an
author can insert, and, on macOS, how your site's own code draws them. Markise knows six field
types and four view kinds and nothing about your site: the rest is in the package.

This page is for people who make packages. Authors want [Blocks](blocks.md). A complete package
for a made-up recipe site, with a sample document, is in the repository under
[`examples/block-package`](https://github.com/adjioev/markise/tree/main/examples/block-package).

## A small complete package

One block with a native view, styles, a script and an icon. These files are checked by a test, and
it draws this block on macOS.

File `package/manifest.json`:

```json
{ "format": "markise-block-package/2", "library": "notes", "version": "1.0.0", "blocks": ["note"] }
```

File `package/components/note/note.json`:

```json
{
  "label": "Note",
  "fields": [
    { "name": "title", "type": "heading", "level": 3, "min": 0 },
    { "name": "body", "type": "text" }
  ]
}
```

File `package/components/note/note.css`:

```css
.note { padding: 8px 12px; background: rgb(37, 99, 235); color: white; }
```

File `package/components/note/note.js`:

```js
export default function draw(block) {
  const stripe = document.createElement("div");
  stripe.style.cssText = "height: 6px; background: rgb(220, 38, 38)";
  block.append(stripe);
}
```

File `package/editor/palette.json`:

```json
{
  "groups": [{ "id": "text", "label": "Text", "blocks": ["note"] }],
  "blocks": { "note": { "description": "A short note", "icon": "pin" } },
  "presets": [
    { "id": "note", "block": "note", "label": "Note", "keywords": "remark aside",
      "markdown": ":::note\n\nWrite the note here.\n\n:::\n" }
  ]
}
```

File `package/editor/views.json`:

```json
{ "note": { "kind": "callout", "accent": "blue" } }
```

File `package/assets/pin.svg`:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="currentColor"/></svg>
```

File `document/example.md`:

```markdown
# Example

:::note

### Remember

This is a block.

:::
```

Pack the `package` folder and put the archive and a binding next to the document:

```sh
COPYFILE_DISABLE=1 tar -czf document/notes-1.0.0.tar.gz -C package .
shasum -a 256 document/notes-1.0.0.tar.gz
```

`document/blocks.config.json`, with the hash that printed:

```json
{ "library": "notes", "version": "1.0.0", "source": "notes-1.0.0.tar.gz", "sha256": "<64 hexadecimal digits>" }
```

`COPYFILE_DISABLE=1` keeps macOS from adding its own extra files to the archive.

## The block in Markdown

An opener of at least three colons directly followed by a name, optionally arguments, and a closer
with the same number of colons or more.

- A name is letters and digits of any script, `-` and `_`, starting with a letter or digit.
- Markers count at the top level of a document only, not inside a quote or a list.
- A block nests in another with a longer fence (`::::a`, then `:::b`).
- A heading, or an item in a list or a task list inside a block, can end in ` {#id}`, a stable name
  that readers do not see. Markise keeps ids and gives a new answer one.

## The files

```text
manifest.json                  format, identity, the list of blocks
components/<name>/<name>.json  the schema of one block
components/<name>/<name>.css   optional: the block's styles
components/<name>/<name>.js    optional: the block's script
styles/styles.css              optional: styles for all blocks
editor/palette.json            required: groups, descriptions and icons, presets
editor/views.json              optional: native views
assets/<icon>.svg              optional: icons
anything else                  yours: fonts, images, more scripts
```

A package is valid as a whole or refused as a whole, with up to 32 problems listed by file. The
author then sees `Block package problem` and blocks stay cards.

### manifest.json

| Key | Value |
|---|---|
| `format` | exactly `markise-block-package/2`. Anything else under that stem is refused as not supported, before any other key is read |
| `library` | ASCII letters, digits, `-`, `_`, `.`, starting with a letter or digit |
| `version` | the same, with `+` allowed. Only compared for equality |
| `blocks` | every block, once. A `components/<x>/<x>.json` for a name not listed is an error |

Other keys are ignored.

### The schema of a block

`label`, optional `variants` (a name to a text; `""` is the block with no variant), optional
`argument` (see below), and `fields`. A **text** is a string or an object of strings per language,
looked up as the language, English, then the first there is; the app shows English so far.

If the block has `variants` and its arguments name none of them, it is outside its schema.

`argument` says the block takes something in its opener, as in `:::figure lesson/three`. Its form then has
a field for it, first, called by the argument's `label` (a text), or its `name` made readable, or
`Argument`, with its `help` (a text) under it. Saving the form changes only those bytes of the opener.

A block whose argument is missing, or does not match the `pattern` the schema gives, gets a `⚠` and counts as
not filled in, so it keeps its native view and can be saved. `pattern` is a regular expression searched for in
the argument, so anchor it with `^` and `$`. A pattern of more than 1 KiB, one that would compile too large, or
one using look-around or backreferences refuses the package when it is validated, saying which. An argument of
over 4 KiB does not match. If the schema also has `variants`, the arguments name a variant and this check does
not apply. A preset's sample argument is checked like any other: a sample that matches shows no `⚠`, so an
author may not notice it was left in.

Every field has a `name` and a `type`, and may have `label`, `help` and, for a repeating field,
`add_label`. A package adds no types of its own.

| `type` | In the Markdown | Limits it can state |
|---|---|---|
| `heading` | one ATX heading; `level` 1 to 6 | `min` |
| `text` | paragraphs, and what no later field takes | `min` |
| `quote` | one block quote | `min` |
| `list` | one list | `minItems`, `maxItems` |
| `choices` | one task list | `minItems`, `maxItems`, `correct`, `ids` |
| `image` | paragraphs holding only images, one per line | `min`, `max` |

`list`, `choices` and `image` are optional unless they state a minimum above 0; the others are
required unless `min` is 0. The `correct` of `choices` is `exactly-one` or `at-least-one`, or an object with a rule per variant.
`exactly-one` gives radio buttons, others check boxes. `ids` can only be `required`.

Each body block goes to the first field at or after the last one filled that takes it. What fits
nothing is listed for the author and never touched. A key with a value a form cannot honour, such
as a `level` of 7, leaves the block with no form and a reason in the panel, and the block stays a
card. A key Markise does not know is ignored.

### palette.json

- `format`: optional, `1`.
- `groups`: `id`, `label`, `blocks`. A block can be listed once.
- `blocks`: a name to `description` and `icon`.
- `presets`: `id`, `block`, `label`, `keywords`, `markdown`. A block with no preset cannot be
  inserted from the library.

A preset's `markdown` must parse as exactly one block of the name it says. Markise refuses it
otherwise, at the moment of inserting. Its `{#id}`s stay if they are free in the document and are
renumbered if not.

A preset can also say how answers are chosen, with buttons: `widget: "icon-choices"`, `variant`,
`field`, `answers` (each with `label`, `icon`, `exclusive`), `columns` and `field_label`. An `icon`
here is the path of a file in the package. An answer's value is its `label`, the text it has in the
Markdown.

### Views

`editor/views.json` maps a block to a `kind`, with optional `accent` and `icon`. A block without an
entry has no native view.

| `kind` | Looks |
|---|---|
| `callout` | a tint of the accent, with a bar on the left |
| `card` | framed, with an accent top edge and the title in the accent |
| `choices` | framed in the accent; answers with a radio button or check box |
| `media` | no frame; pictures with their alt text as a caption |

`accent` is one of slate, gray, red, orange, amber, yellow, green, teal, cyan, blue, indigo, purple,
violet or pink, each with a light and a dark value. Any other name means the theme's accent. The
first row shows the block's `label`, with the variant's label after it. A block of over 64 KiB
stays a card.

### Icons

An icon is a name such as `pin`, which means the file `assets/pin.svg`. Without that file nothing
is drawn and nothing is wrong. An icon the rules below refuse is treated the same way.

`fill="currentColor"` makes an icon a mask that follows the theme; other colours stay. It is drawn
at 128 by 128 whatever it declares. Refused:

- a file over 256 KiB;
- a DTD, `use` and `marker`;
- filters: `filter` and `fe*` elements, the `filter` attribute and CSS property;
- `pattern`, masks and images, and dashed strokes;
- more than 4096 parts or 16 levels of nesting.

## How your code draws a block

On macOS 14 and later, a block that fits its schema can be drawn as a picture by your own CSS and
JavaScript, on the system's web engine. A block with a view gets this when the author switches to
it. A block with no view shows it from the start. Where nothing can draw, the block is a card.

**Styles:** `styles/styles.css`, then `components/<name>/<name>.css`, whichever exist.

**Script:** `components/<name>/<name>.js`, an ES module. Its default export gets the block's element
and may return a promise. The picture is taken when that resolves. Without a script the block draws
from its styles.

**The element** comes from the fields, by type alone:

```html
<section data-component="name" class="name" data-variant="word" data-argument="word">
  <h2 data-field="title">…</h2>                       <!-- heading, of its level -->
  <div data-field="body"><p>…</p></div>               <!-- text -->
  <blockquote data-field="note"><p>…</p></blockquote> <!-- quote -->
  <ul data-field="steps"><li data-item>…</li></ul>    <!-- list -->
  <ul data-field="answers" data-select="single">      <!-- choices: single or multiple -->
    <li data-item data-option-id="a1" data-correct="true">…</li>
  </ul>
  <div data-field="pictures"><img src="…" alt="…"></div> <!-- image -->
</section>
```

`data-variant` is there when the arguments name a declared variant, and `data-argument` when the
schema declares an `argument`. A field with nothing in it has no element. Text goes through the HTML
export's sanitizer. Ids are not text.

**The page around it:** `<html lang data-theme="light|dark">`, with the block as the first element of
a container as wide as the document's column. Outer margins are zero, and animations and transitions
are off. The background is transparent, so what you do not paint is the document.

| On `<html>` | |
|---|---|
| `data-theme` | `light` or `dark`; also `color-scheme` |
| `lang` | the `lang` key of the document's front matter, if any |
| `--markise-background`, `--markise-text`, `--markise-link`, `--markise-accent`, `--markise-selection` | the theme's colours |
| `--markise-font-size` | the theme's body size in `px` |

**What a page can reach.** Your styles and scripts load the package's files by relative URL, and the
block's `<img>` elements load the images it lists. Everything else gets nothing: no network, no other
file, no inline scripts in the markup or `eval`, no workers, forms, new windows or dialogs, no way to
call the editor. Clicks do not arrive. Every render is a fresh document, and web storage is not kept on
disk or shared with another package or document folder.

**Bounds.**

| | |
|---|---|
| One render | 5 seconds, then it fails |
| A picture | 8192 pixels a side, 24 megapixels |
| Files one render can ask for | 512 |
| Waiting for styles, pictures and fonts | 1.5 s, 1.5 s, 1 s, then the picture is taken as it stands |

A block that fails is a card with the reason on its first row, and is not tried again until its
source, width, theme or scale changes. A picture is made again when any of those change, the package
or the document's language included. It does not depend on the contents of an image file.

## The binding

`blocks.config.json`, found in the document's folder or the nearest folder above it, up to the home
folder.

| Key | Value |
|---|---|
| `library`, `version` | must equal the manifest's |
| `source` | a `.tar.gz` by a relative path below the binding's folder. No absolute paths, `~`, `..`, backslashes, colons or URLs. With links resolved it must stay inside the folder |
| `sha256` | the hash of the archive, 64 hexadecimal digits |

An archive holds regular files and folders only. A link or device refuses it whole, and so do two
names that differ only by case or Unicode form. A changed hash asks the author again, whatever the
version says.

## Limits

| | |
|---|---|
| One JSON file | 1 MiB |
| Blocks in a package | 512 |
| Fields in a block | 256 |
| Palette groups | 256 |
| Palette presets | 2048 |
| A name (`library`, block, field, icon) | 128 bytes |
| A label, description or keyword text | 4 KiB |
| The Markdown of a preset | 64 KiB |
| The archive, compressed | 32 MiB |
| All files, unpacked | 64 MiB |
| One file, unpacked | 32 MiB |
| Entries in the archive | 4096 |
| A path | 1024 bytes, 16 levels, 255 bytes per name |
| The binding file | 64 KiB |
