# ADR 0019: block packages

Status: accepted for issue #178, phase 2.

## Context

ADR 0018 made `:::name` a container with no knowledge of any name. A site that uses such blocks
knows what they mean: their fields, how they look, what an author may insert. That knowledge
belongs to the site and must reach Markise as data, so that Markise gains no vocabulary of any
site. It also has to arrive safely: a document can come from anyone, and a package holds code a
later phase will run in an isolated helper.

This phase validates, indexes and binds packages. It changes nothing in how a document renders
and runs nothing from a package. Script and style files are opaque here.

## Decision

### Format

A package is a set of files with paths relative to its root:

```text
manifest.json                  format, identity, the list of blocks
components/<name>/<name>.json  the schema of one block, forms data included
components/<name>/...          scripts, styles and other files of the block (opaque)
editor/palette.json            groups, per-block descriptions and icons, presets
editor/views.json              optional: the generic view of a block
anything else                  opaque (shared styles, fonts, images)
```

`manifest.json`:

```json
{ "format": "markise-block-package/2", "library": "sample", "version": "1.2.0",
  "blocks": ["tip", "poll"] }
```

**The format name is the only version check, and it is exact.** Anything else under
`markise-block-package/` refuses cleanly with "not supported by this version", before any other
key is read, so a later format may change everything else. Version `/2` is the first supported
one: `/1` was a pre-release layout that never shipped. Unknown keys inside a known format are
ignored or kept (below), so a package can add data for later Markise versions without a new
format. A change that an older Markise would misread needs a new format name.

**`library`** is an ASCII name (letters, digits, `-`, `_`, `.`, starting with a letter or digit).
**`version`** is the same with SemVer's `+` allowed; Markise compares versions only for equality.
**`blocks`** lists every block exactly once. A block name follows the `:::name` rule of ADR 0018.
A `components/<x>/<x>.json` for a name not listed is an error, so a block cannot be half present.

**Schema** (`components/<name>/<name>.json`): `label` (text), optional `variants` (variant name to
text), optional `argument` (object), and `fields`, an array of objects with a `name` and a `type`.
The types are a closed list of generic primitives: `heading`, `text`, `list`, `choices`, `quote`,
`image`. A site adds no types of its own; an unknown type is an error. Other keys of a field (`min`,
`level`, ...) belong to the field model and are kept as JSON in the index.

**Text** is a string or an object of strings per language. Lookup: the language, English, the
first language there is.

**Palette** (`editor/palette.json`, required): optional `format` (`1`), `groups` (`id`, `label`,
`blocks`), `blocks` (name to `description` and `icon`), `presets` (`id`, `block`, `label`,
`keywords`, `markdown`). A preset's Markdown must open the block it names. Any other key of a preset
is kept untouched for later phases.

**Views** (`editor/views.json`, optional): block name to `kind`, one of `callout`, `card`,
`choices`, `media`, with optional `accent` and `icon`. Other keys are kept. A block without a view
stays what it is today, a simple card. An unknown kind is an error.

**Icons and accents are names**, ASCII tokens like `library`. Only the package gives them meaning;
the token form means later code can use one as a key or a file stem without another check.

Every reference is checked: a palette group, palette block, preset or view naming a block outside
`blocks` is an error.

### Validation

All or nothing: a package with any problem is refused as a whole, with up to 32 problems listed by
file. A package is a built artifact, so a broken one is a bug to report to its site, and a half
working one would hide that from the author. The document keeps working in any case: its blocks
stay simple cards.

| Limit | Value |
|---|---|
| One JSON file | 1 MiB |
| Blocks in a package | 512 |
| Fields in a block | 256 |
| Palette groups | 256 |
| Palette presets | 2048 |
| A name (`library`, block, field, icon, ...) | 128 bytes |
| A label, description or keyword text | 4 KiB |
| The Markdown of a preset | 64 KiB |

JSON nesting is bounded by the parser's recursion limit, which is an error, not a crash.

### Crate

`markise-package` (L2) holds this. It depends on no layer above and no UI toolkit. `markise-blocks`
stays what it is, the renderers of code, math, diagrams and images: a package is not a renderer.
The index, `PackageIndex`, answers by block name with its schema, optional view and palette entry.

## Consequences

- Markise's only vocabulary is the field types and the view kinds above. Everything else a site
  wants to say travels as package data.
- A new field type or view kind is a Markise change. That is deliberate.
- A package made for a later format shows a clear message and leaves documents as they are.
