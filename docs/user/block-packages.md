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
| `authoring` | optional. The dialect your site writes in, as a document's front matter would say it. `blocks-v3` turns on [sections](blocks.md#sections) |
| `htmlContract` | optional, a whole number. Which markup your code expects of a block: `2` has a `<div>` as a block's root, and `1`, or none, or a number Markise does not know, a `<section>`. See [The element](#how-your-code-draws-a-block) |

Other keys are ignored, and so are files Markise does not read, such as `templates/<name>/<name>.json`.

### The schema of a block

`label`, optional `variants` (a name to a text; `""` is the block with no variant), optional
`argument` (see below), and `fields`. A **text** is a string or an object of strings per language,
looked up as the document's `lang` tag, the part before its `-` (`ru-RU` finds `ru`), English, then
the first there is; a document with no `lang` shows English. See
[Language](blocks.md#language) for what follows it.

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

An item of a `list` or `choices` field is its first line (its first paragraph). Markdown indented under
it, such as a quote, another paragraph or an image, belongs to that item: it shows under it, moves
and goes with it and is kept as written. Markise gives it no meaning. You edit it in the block's
Markdown, Source or Side by Side; the form keeps it.

Each body block goes to the first field at or after the last one filled that takes it. What fits
nothing is listed for the author and never touched. A key with a value a form cannot honour, such
as a `level` of 7, leaves the block with no form and a reason in the panel, and the block stays a
card. A key Markise does not know is ignored.

### palette.json

- `format`: optional, `1`.
- `groups`: `id`, `label`, `blocks`. A block can be listed once.
- `blocks`: a name to `description`, `when` and `icon`. `when` says when to reach for this block rather than
  another. Only [the reference](#the-command-line) shows it.
- `presets`: `id`, `block`, `label`, `keywords`, `markdown`. A block with no preset cannot be
  inserted from the library.

A preset's `markdown` must parse as exactly one block of the name it says. Markise refuses it
otherwise, at the moment of inserting. The `{#id}`s of its answers stay as you wrote them, as they belong to their
field and not to the page, so `{#none}` stays `{#none}`. A heading's id is unique on a page and gets a number if the page has it already.

A preset can also say how answers are chosen, with buttons: `widget: "icon-choices"`, `variant`,
`field`, `answers` (each with `label`, `icon`, `exclusive`), `columns` and `field_label`. An `icon`
here is the path of a file in the package. An answer's value is its `label`, the text it has in the
Markdown.

### sections.json

`editor/sections.json` is optional, and says what your site makes of [sections](blocks.md#sections). Without it a document
with sections still offers a new one in the block library, called `Section`.

```json
{
  "format": 1,
  "label": { "en": "Part", "fr": "Partie" },
  "description": { "en": "A new part of the page" },
  "keywords": "chapter",
  "icon": "flag",
  "title_level": 2,
  "fields": [
    { "key": "style", "label": "Look",
      "values": [ { "value": "wide", "label": "Wide" }, { "value": "plain" } ] },
    { "key": "kind", "label": "Kind" }
  ]
}
```

| Key | Value |
|---|---|
| `format` | optional, `1` |
| `label` | text, what the library and the form call a section. Default `Section` |
| `description`, `keywords` | as for a block in the palette |
| `icon` | a name, as a block's |
| `view` | how sections look on screen, below |
| `fields` | the metadata the form for a new section asks for, in this order, at most 19. None: it asks for none |

`"view"` says how sections look: `{"frame": true, "accent": "blue", "numbered": true}`, each optional.

| Key | Value |
|---|---|
| `frame` | `true`: each section is a card, with a rounded border, a faint ground, room inside and a gap between sections, around whatever is in it. Default `false`: sections are divided by a strong line |
| `accent` | a colour's name, as in [Views](#views), for the line or border and the number. Default: a colour from the text's |
| `numbered` | `false`: a boundary says no number. Default `true` |

Anything else in these keys refuses the package. This says nothing about what a site draws, and changes no byte of a document.

`title_level` is the level of the heading a new section starts with, a whole number from 1 to 6, default 2, for both the library's item and **Insert After…**, and the level a section's name comes from first. With `1` sections go by `#`, and a document's first `#` is no title apart. Anything else refuses the package.

A field has a `key` (`[a-z][a-z0-9-]*`, not `id`, as the form has its own field for that), an optional `label`, and optional `values`,
each with a `value` of one line up to 200 characters and an optional `label`. With `values` the form offers those; without, any one line.
A field left empty is not written. If no field is filled, the section has no metadata block. This file says nothing about what a value
means, and Markise checks a section's metadata in a file against it nowhere: it is only what the form offers.

### Views

`editor/views.json` maps a block to a `kind`, with optional `accent`, `icon`, `fill` and `label`. A
block without an entry has no native view.

| `kind` | Looks |
|---|---|
| `callout` | a tint of the accent, with a bar on the left |
| `card` | framed, with an accent top edge and the title in the accent |
| `choices` | framed in the accent; answers with a radio button or check box |
| `media` | no frame; pictures with their alt text as a caption |

`accent` is one of slate, gray, red, orange, amber, yellow, green, teal, cyan, blue, indigo, purple,
violet or pink, each with a light and a dark value, or `none`. Any other name means the theme's
accent. The first row shows the block's `label`, with the variant's label after it. A block of over
64 KiB stays a card.

Three options make a view quieter. A page with many blocks reads better with its text first. Each
is off by default, and a value of the wrong type refuses the package.

| Key | Looks |
|---|---|
| `"fill": false` | no tint, no frame, a thin bar of the accent on the left. A marked answer's row has no tint, a title and the first row are in the text's colours. `media` has neither to begin with |
| `"accent": "none"` | neutral: thin lines in the theme's rule, labels and marks in its quiet text. With a `fill` a `callout` has a very light neutral ground |
| `"label": false` | no first row and no icon: the block's own heading is enough. The label still reaches a screen reader. A warning still gets its row |

```json
{ "note": { "kind": "callout", "accent": "none", "fill": false, "label": false } }
```

A block's buttons show when the pointer is on the block, when it holds the caret, or when one of them
has the keyboard, and they take up no room otherwise.

A `callout`, `card` or `choices` view can be practised in [Read](blocks.md#practice):

```json
{
  "question": { "kind": "choices", "practice": { "reveal": ["explanation"] } },
  "exercise": { "kind": "card", "practice": { "reveal": ["solution"] } }
}
```

`reveal` names the fields hidden until the reader asks for them. A block whose schema has a `choices`
field is checked against its marks, and counts in the score. One with none needs something in `reveal`, shows it behind Show
answer, and does not count. `reveal` cannot name the answers themselves, or a heading, as the outline
shows every heading. A package that breaks any of this is refused, with a message naming the block and the field. A block with no
`practice` has no Practice. Answers keep the rule of their schema, so ids stay as your `choices` field has them.

With `"default": true` beside `reveal`, a document holding a block of that view opens in [Read](blocks.md#read-and-edit), so that answers do not show until asked for. Authors reach editing through the switch in the status bar.

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

**The element** comes from the fields, by type alone. With `"htmlContract": 2` in the manifest its root is a
`<div>` rather than this `<section>`, and nothing else changes. Select a block by `[data-component]`, never by its tag:

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
schema declares an `argument`. A field with nothing in it has no element. A block is drawn alone: no section of the
page stands around it, so your styles for a block cannot ask for one. Text goes through the HTML
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

## The command line

Two commands of `markise-cli` let a person or a coding agent write correct blocks without opening the
app. Neither draws anything, runs a package's code, uses the network or a helper, or asks for consent.

### Checking a document

```sh
markise-cli blocks check lesson.md more/*.md
```

For each file this finds the binding as the app does, reads the archive it names, checks the hash, the
identity and the package as a whole, and checks each block in memory. It writes nothing, and the
store of accepted packages is not read or changed, so the archive has to be there even when the app
still has a copy. Links are resolved first, as the app does when it opens a file, so a link to a file counts where that file lives.

```text
lesson.md:13: tip: Not filled in yet: Text: Required
lesson.md:35: question: Outside its schema: Answers 1: Has no id
lesson.md:71: sidebar: Unknown block: The package has no block `sidebar`.
lesson.md:77: Marker: `::: tip` is not a marker, so it stays text. ...
files: 1, blocks: 9, problems: 4
```

A reason about one piece of a block has that piece's line; others have the opener's. The words of the
reasons are those of the editor's `⚠`, field labels in the document's language.

| Kind | Headline | Means |
|---|---|---|
| `not-filled` | Not filled in yet | a block with a view lacks something: a required field, enough items, a marked answer, an argument, or an argument of the form its `pattern` says |
| `structure` | Outside its schema | content in no field, arguments naming no variant, an answer with no id or a repeated one, too many items or marks, any reason of a block with no view, or a block of over 64 KiB, which stays a card |
| `unknown-block` | Unknown block | the package has no block of this name. In the editor this is a plain card with no `⚠` |
| `no-form` | No form | the package says something a form cannot honour, the panel's reason. The block stays a card |
| `marker` | Marker | an opener with no closer, a closer with nothing to close, a line of three or more colons that is not of a marker's form, or a marker inside a quote or a list. In the editor these are text |

| Exit code | |
|---|---|
| 0 | every file is clean, or no binding applies to it |
| 1 | at least one problem |
| 2 | a usage error, a file that cannot be read, or a binding or package that cannot be used, whatever else was found |

A file no binding applies to gets a note and passes: nothing says which blocks it should have. Other
files are still checked after an error. Missing blank lines around markers are not reported, since the
parser does not need them.

`--json` prints this on stdout instead, except for a usage error, which is plain text on stderr:

```json
{
  "format": "markise-blocks-check/1",
  "files": [
    {
      "file": "lesson.md",
      "status": "problems",
      "note": null,
      "error": null,
      "binding": { "file": "/site/blocks.config.json", "library": "notes", "version": "1.0.0", "sha256": "<64 hexadecimal digits>" },
      "language": "en",
      "blocks": 9,
      "problems": [
        { "line": 13, "block_line": 13, "block": "tip", "kind": "not-filled", "headline": "Not filled in yet", "reason": "Text: Required" }
      ]
    }
  ],
  "summary": { "files": 1, "blocks": 9, "problems": 1, "errors": 0, "unbound": 0 }
}
```

- `status` is `clean`, `problems`, `unbound` (with `note`) or `error` (with `error`).
- `binding` is `null` when none applies. With an `error` it may hold only `file`, or be `null`.
- `language` is `null` unless the file was checked.
- `block` is `null` for a line that names no block; `block_line` is the opener's line, or the line itself.
- Characters a terminal would act on or hide are escaped in the strings as `\uXXXX`, and in text output as `\u{1b}`.
- Keys and values mean this for as long as `format` says `markise-blocks-check/1`. The wording of `reason` can change.

### A reference for authors

```sh
markise-cli blocks docs --package blocks/notes-1.0.0.tar.gz --lang ru > BLOCKS.md
```

`--package` takes an archive or a package's folder. `--binding` takes a `blocks.config.json`, whose pin and
identity are checked. With neither, it reads the binding nearest the current folder. The text of the
package follows `--lang` as a document's `lang` would, English by default; the rest is English.

It prints Markdown, made only from the package's data:

- a first line saying it is generated, from which version and hash, and the command that makes it again, as you typed it;
- each block in the palette's order, then those in no group by name, with its name, label, description, `when`, arguments
  and their pattern or variants, fields with their type and rules, what Practice hides, and each preset as an example;
- the general rules: the `:::` syntax, blank lines, the fence length for nesting, ids, and what `check` reports.

A package's text is treated as data, since an agent may act on this page. Each piece stands on one line after a fixed lead-in, what
would be Markdown is escaped, characters a terminal would act on or hide are written out, and a preset is in a fence no line of it can
close, so it cannot pose as a heading, a fence or part of the reference. A first paragraph says that it is quoted. A package whose
manifest names the sections dialect gets one more rule on them; others get nothing about sections.

The same package gives the same bytes: no dates, nothing from the machine, so it can sit in a repository with small diffs. For a
folder, the hash is that of the archive Markise packs from it in memory, which a `tar` of the same files
would not match.

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
