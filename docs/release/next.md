# Next release (unreleased)

Changes on `main` since 0.1.0-beta.2, collected for the next release notes. When cutting the
release, copy this into `docs/release/<version>.md` (keep the structure of
`docs/release/0.1.0-beta.2.md`) and empty this file.

## What's new

### Tags

- `#tags` in the text and `tags:` (or `tag:`) in the front matter — lists, one item per line, or
  names separated by commas — are found in every note of the opened folder, in the background.
  Nested tags (`#area/health`) count for their parents; letter case does not matter.
- A tag is shown as a pill in the rendered view; the file keeps exactly what you typed.
  Cmd/Ctrl-click a tag to list the notes with it.
- A new **Tags** view in the sidebar lists every tag with its number of notes. Click a tag for
  its notes; type `tag:name` or `#name` in its Find field to search by tag.
- Exports keep tags as text (HTML marks them for styling).
- **Changed:** a line starting with `#` directly followed by a word (`#idea`) is now a paragraph
  with a tag, not a heading. **Preferences → Markdown → Tags #tag** turns tags off and restores
  the old reading.

See [Folders and vaults → Tags](https://github.com/adjioev/markise/blob/main/docs/user/folders-and-vaults.md#tags).
