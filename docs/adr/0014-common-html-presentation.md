# ADR 0014: Common HTML presentation

Status: accepted for implementation (issue #103).

Common HTML remains raw Markdown source and exports as native HTML. The Markdown adapter
supplies format-neutral presentation hints through `FormatDoc::raw_presentation` and rich
inline nodes. Layout does not parse HTML or depend on the Markdown crate.

A display-only index pairs `<details>` markers with a stack inside each enclosing Markdown
container. It supports nested disclosures, a `<summary>` title (or “Details”), and the boolean
`open` attribute. Clicking changes view state only: no edit, dirty flag, undo entry or source
rewrite. Reloading restores the source defaults. Markdown children retain normal layout with
an indented border. Moving the caret into a hidden child opens its ancestors; focusing a tag
block reveals its source for editing. A virtual paragraph after a tag does not reveal it.

Paired inline `<kbd>` tags become padded monospace key caps. HTML comments are hidden until
the caret reveals their source. Unknown HTML and unmatched disclosure/keyboard tags retain
the existing raw fallback. This is intentionally not a browser or arbitrary HTML renderer:
headers containing other raw body content, malformed headers, and headers above 32 KiB stay
raw. Keyboard nesting is interpreted up to 32 levels; deeper tags stay raw. Summary labels
are plain text, with HTML entities decoded and inline tags stripped.

The marker index is updated after structural edits; ordinary same-item prose edits do not
re-pair the document. Collapsed ranges have zero height and viewport/caret traversal jumps
over their children without shaping them. Toggling rebuilds height estimates (linear in item
count), but never parses or shapes the full document. Criterion cases measure toggles and
cached frames with 1,000 and 10,000 children. Regression tests cover nested/container-scoped
pairing, source reveal, incremental edits, source/export preservation and UI clicks.

## Measurement (2026-11-01)

Local release Criterion run (`cargo bench -p markise-layout --bench layout --
'html-frame|disclosure-toggle'`, 20 samples): 1,000 children toggle 36 µs and cached frame
0.87 µs; 10,000 children toggle 345 µs and cached frame 0.30 µs. Other checks ran concurrently,
so these are indicative local measurements, not a cross-machine latency guarantee. Both toggle
measurements are well below the 16 ms interactive budget; collapsed-frame work does not grow
with child count. These measurements exclude parsing/model construction and OS/UI delivery.
