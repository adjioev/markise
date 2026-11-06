# ADR 0026: sections

Status: prototype for issue #178, phase 9, of a draft syntax that may still move. Display only.

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

- **A boundary** is a quiet divider on the line's own row, so it has the line's height and estimates
  stand: the section's name, then the line to the right edge. The name is its `id`, else its first heading
  among the section's own top-level blocks (a bounded look, as this is asked on the UI thread), else
  `Section N`. Only `id` and `style` are engine keys, and no other key is ranked. No caret stands in it, as in
  any rule.
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

### Finding one's way

The outline panel has every section as a level above headings. Its rows are not built: the row of section
`k` is `k` plus the headings before it, so the rows on screen are found by bisection over sections, and a
list of any size costs what is shown. The model's outline names a heading in a placeholder by the
placeholder, so a boundary in one reads that page's items, without materializing it, and remembers the count
until the document changes. A click materializes the target's page and goes there as a heading's does.

The status bar names the current section: the caret's while the caret is on screen, else the one at the top
of the screen. On hover it says which of how many, and where the dialect came from, with the package's
version. While a document is partial the last section's name ends in `…`, the count in `+`, and the outline
says that more is to come.

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

### Exports

With sections on, HTML, PDF, PNG and Pandoc leave a root-level `:::section-metadata` out: they all read
`Document::export_tree`. The dialect comes to an export as a request's flag, for what a package says, and from
the document's own front matter, which wins; the command line knows only the front matter. `---` in exports stays
as it is for now: `<hr />`, a line in PDF and PNG, `HorizontalRule` for Pandoc.

### Writing

On a page with sections Horizontal Line writes `***`, as it would start a section otherwise (`EditPrefs::
rule_stars`, set while sections are on). A typed `---` and Enter does make a boundary, which is correct, and
Backspace at the start of what follows takes it, as for any rule, with an undo step.

## Consequences

- The bytes of a file do not depend on any of this: the index only reads.
- What moves blocks does not know sections: a block dropped above a plaque is allowed, and the metadata
  block then shows as a card with the warning above. No move is refused.
- A metadata block not first, or a second one, is a generic card with a warning.
- A metadata opener with no closer stays text, with a warning under it.
- A first line `---` with a later `---` is front matter, as ever, so a document that starts with a
  boundary and has no front matter loses its first two boundaries to it. The draft refuses a leading
  boundary anyway.
