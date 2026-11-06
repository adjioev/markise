# Export

**File → Export** saves the current document in another format. Choose a format, pick a file
name, and the export runs in the background: the status bar shows the progress with a
**Cancel** button, then the result.

| Format | Menu item | Notes |
|---|---|---|
| HTML | **HTML** | One self-contained file styled with the current theme. Local PNG, JPEG, GIF and WebP images are embedded (up to 10 MiB each, 64 MiB per export); math and diagrams are SVG. |
| HTML without styles | **HTML (without styles)** | Plain semantic HTML with no stylesheet; image paths are rewritten relative to the exported file. |
| PDF | **PDF** | Paginated, selectable text, working links, a document outline (bookmarks) and tagged PDF. |
| PNG | **Image (PNG)** | A picture of the whole document; very tall documents are split into several PNG files. |
| Word, OpenDocument, RTF, EPUB, LaTeX, MediaWiki, reStructuredText, Textile, OPML | **Other Formats via Pandoc** | Needs Pandoc, see below. |

**Export with Previous** repeats the last export of the session and asks for the file name with
the previous one filled in; **Export and Overwrite with Previous** writes to the same file again
without asking (it confirms once per document and destination). **Include Comments** decides
whether review comments are kept (see
[Comments and suggestions](comments-and-suggestions.md#in-exports)).

From the command line, `markise-cli html FILE.md -o FILE.html` writes the same kind of HTML.

## Callouts

[Callouts](editing.md#callouts) export with their title and colour. In HTML a foldable one is a
`<details>` element, closed or open as written. PDF and PNG show every callout open, without its
icon. Pandoc formats get a `Div` with the classes `callout` and the type, and what becomes of it
depends on the format.

## PDF page setup

The PDF page is A4 with 18 mm top and bottom and 16 mm side margins unless the document's front
matter says otherwise:

```yaml
---
title: Passage plan
papersize: letter        # a3, a4, a5, b5, letter, legal, tabloid, or "210mm 297mm"
margin: 20mm 15mm        # 1 to 4 values, in CSS order; or margin-top, margin-left, …
header: ${title}
footer: Page ${pageNo} of ${totalPages}
---
```

Lengths accept `mm`, `cm`, `in`, `pt` and `px`. Header and footer templates can use
`${pageNo}`, `${totalPages}`, `${title}`, `${author}`, `${today}`, `${currentFileName}` and any
other front-matter key.

## Pandoc formats

The formats under **Other Formats via Pandoc** are produced by [Pandoc](https://pandoc.org),
which you install separately (version **2.10 or later**). Markise finds it on your `PATH` and in
the usual install folders (for example `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`,
and on Windows `%LOCALAPPDATA%\Pandoc` or `C:\Program Files\Pandoc`). If Pandoc is not found,
the export fails with a message and a link to the Pandoc installation page. A Pandoc export is
stopped after 120 seconds.

Local images are handed to Pandoc as file paths. Remote images (`https://…`) are passed on as
written, and Pandoc may download them while it builds the file (for example for EPUB or Word);
that request comes from Pandoc, not from Markise.

## Limitations in the beta

- **No export settings.** Each format uses its built-in options; **Preferences → Export** has no
  settings yet, and the Pandoc path cannot be set by hand — install Pandoc where Markise looks
  for it.
- **PDF and PNG show math and diagrams as source text.** Use HTML export to get them drawn.
- **PDF and PNG use local images only.** Remote images (`https://…`) are not downloaded and
  `data:` images are not supported; a placeholder with the image's alt text is drawn instead.
- Fonts bundled with an imported CSS theme are not used in PDF and PNG export (installed system
  fonts are).
- Wikilinks become relative links in HTML; in PDF, PNG and Pandoc formats only their text is
  kept.
- Export is unavailable while a large file is still loading (see [Large files](large-files.md)).
- Very large HTML exports (over 200 MB, usually from many `[TOC]` blocks) ask before writing.
