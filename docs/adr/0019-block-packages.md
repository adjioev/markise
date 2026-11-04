# ADR 0019: block packages

Status: accepted for issue #178, phase 2.

## Context

ADR 0018 made `:::name` a container with no knowledge of any name. A site that uses such blocks
knows what they mean: their fields, how they look, what an author may insert. That knowledge
belongs to the site and must reach Markise as data, so that Markise gains no vocabulary of any
site. It also has to arrive safely: a document can come from anyone, and a package holds code a
later phase will run in an isolated helper.

This phase validates, indexes and binds packages and asks for consent. It changes nothing in how a document renders
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

| The archive, compressed | 32 MiB |
| All files, unpacked | 64 MiB |
| One file, unpacked | 32 MiB |
| Entries in the archive | 4096 |
| A path | 1024 bytes, 16 levels, 255 bytes per name |
| The binding file | 64 KiB |

JSON nesting is bounded by the parser's recursion limit, which is an error, not a crash. The size
limits are enforced while decompressing, so sizes stated in an archive are not trusted.

### Binding

A document finds its package through `blocks.config.json`, in its folder or in a folder above it:

```json
{ "library": "sample", "version": "1.2.0",
  "source": "blocks/sample-1.2.0.tar.gz",
  "sha256": "<64 hexadecimal digits>" }
```

- **Discovery** looks at one file name per folder, from the document's folder up to and including
  the home folder (for a document outside it, up to the root; 64 levels at most). It lists no
  folder. The nearest binding wins, also when it is broken: a broken binding is an error to show,
  not a reason to use one farther up.
- **`source`** is a `.tar.gz` archive given by a relative path below the binding's folder. Refused:
  absolute paths, `~`, `..`, backslashes, colons, control characters and names Windows reserves.
  With links resolved, the file must still be inside the folder. A link file as the binding is
  refused too.
- **No network.** A `source` with a scheme is refused with a message. A later phase can add one;
  the pin below is what would make that safe.
- **`sha256` pins the archive.** It is hashed and validated from one read of its bytes, so nothing
  can change in between. `library` and `version` must equal the manifest's, so that a question
  never names something the package is not.
- **When it is looked up:** on a worker thread, when the document's file changes (open, Save As).
  An edit of the binding takes effect the next time the document is opened. No file is watched.
- The throwaway prototype also read a `blocks:` key in front matter and accepted folders as a
  source. Neither is adopted: a document should not choose its own package, and a folder has no
  hash to pin.

### Archive

An archive may hold regular files and folders only. Any link, device or FIFO entry, unsafe path, or
two paths that differ only by case refuses it whole. It is read into memory, and nothing is written,
until the user has accepted it. The `tar` crate is used to read entries only, never to unpack.

### Store

Per user, in the config folder, `<config>/markise/block-packages/`:

```text
trust.json                         what the user accepted
packages/<sha256>/files/...        the unpacked files, by archive hash
```

A package appears whole: it is unpacked in a staging folder and renamed into place. Files are
created new, without executable bits from the archive. After that the package works without its
archive (offline, or a removed file). A staging folder over an hour old was left by a crash and is
removed, best effort, at the next install.

Hashes reach the store as a validated type (64 hexadecimal digits, lowercase), so no store operation
can name another folder. A record in `trust.json` with any other value is dropped.

Everything next to a package can be replaced along with it, so nothing there is trusted. On every
load the unpacked files are hashed into one digest of their paths and contents, and compared with
the digest held in the consent record. A difference, an added or removed file, or anything on the
way that is not a real folder or regular file (the `files` root included) counts as damage: the
package is removed and unpacked again from a verified source, or the binding reports an error. A
self-consistent replacement of the cache therefore does not inherit consent, as long as `trust.json`
itself is intact. Someone who can rewrite that file can also accept any package; the store does not
defend against code already running as the user.

The config folder rather than the OS cache folder holds this, since an OS cache can be purged under
an author who works offline. Tests and measurements without a config folder look nothing up at all.

### Trust

Consent is recorded against the package's identity and hash: `library`, `version`, `sha256`, and
`files_digest`, the digest of the files that archive unpacks to.

- **Nothing is written before yes.** A document can only cause reads of its binding and archive,
  off the UI thread. The archive is validated before the question, so a broken package shows its
  error rather than a question.
- **A changed hash asks again,** whatever the version says. The question then says it differs from
  the version accepted before. An older accepted hash stays accepted, for folders still bound to it.
- **Consent follows the hash, not the folder.** A second folder binding the same bytes does not ask
  again: the user decided on those bytes. Its documents are its own, and a later phase hands a
  renderer only the block at hand, without access to files or the network.
- **Not Now** holds for the session and is not stored. The status bar offers the question again.
- **Stop Using This Package** removes the record and the unpacked files.
- **Store changes and lookups run in the order they were started,** also across a document switch,
  so a document opened right after Stop Using cannot be handed files that are about to go.
- **A hash is not a publisher.** It says these are the bytes the binding names, not who made them.
  Signatures are left out for now; consent is about trusting where a folder came from.
- A missing or damaged `trust.json` grants nothing and leads to a question. A record from a newer
  Markise is neither used nor overwritten. Two running copies writing at once can lose a record,
  which at worst asks again.
- **Accepted limit: the source path can race.** `source` is checked with links resolved and then
  read by that path, so a link swapped in between could redirect the read outside the folder. This
  stays: the bytes read must still match the pinned hash, so the swap can only cause a read of a
  file that is identical to the pinned archive or fail. Traversal through directory handles would
  close it at a cost across platforms that this does not justify.
- An archive changed under an unchanged pin is an error when it is read. While the cache holds that
  hash the archive is not read at all: the pin is what counts, and the cache already matches it.

### On screen

The question is a non-modal card above the status bar, like the first-launch question about
updates, and has no default button: the document is usable meanwhile, and blocks stay simple cards
until the answer. Return cannot accept a package by accident. It names the package, its source, its
hash and the binding file, and says when it differs from what was accepted before. The status bar
shows `Blocks: <library> <version>`, or `not in use`, or `Block package problem` in the warning
color; its menu holds the details and the actions. An unbound document shows none of this. Nothing
in how a document renders changes in this phase.

### Cost for other documents

A document with no binding costs one short-lived thread and a few `lstat` calls when it is opened,
and nothing per frame beyond comparing a path. Neither wakes the UI. All file access is off the UI
thread.

### Crate

`markise-package` (L2) holds all of this. It depends on no layer above and no UI toolkit. `markise-blocks`
stays what it is, the renderers of code, math, diagrams and images: a package is not a renderer.
The index, `PackageIndex`, answers by block name with its schema, optional view and palette entry.
`resolve` goes from a document path to a question, a ready package or a problem; `accept` installs.

## Consequences

- Markise's only vocabulary is the field types and the view kinds above. Everything else a site
  wants to say travels as package data.
- A new field type or view kind is a Markise change. That is deliberate.
- A package made for a later format shows a clear message and leaves documents as they are.
- New dependency: `tar` (with `filetime`), both MIT or Apache-2.0 and pure Rust. `sha2` and `flate2`
  were in the tree already.
