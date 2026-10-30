# ADR 0012 — Review comments and suggestions (CriticMarkup)

Status: **proposed** (2026-10-29, issue #69). Scope: `markise-doc` (L1: the neutral node kind,
planner ids, `review` helpers), `markise-md` (L1: parsing, planners, source tokens),
`markise-theme` (L2: review colours), `markise-layout` (L2: rendering, caret reveal),
`markise-export` (L3: HTML, export model), `markise-engine` (L4: settings, export request),
`markise-ui-egui` (L6: menus, context menu, hover bar, comments panel, clean copy).

## Context

An early user's workflow: an AI tool writes a Markdown document, the user comments on passages
in Markise, the file goes back to an AI tool, which revises it with suggestions, and the user
accepts or rejects them in Markise. Comments must travel with the file (owner decision: no
sidecar file), so they are stored inline. CriticMarkup is a small, public, plain-text syntax for
exactly this, readable without tools and already understood by AI models and several editors:

| Mark | Syntax |
|---|---|
| insertion | `{++text++}` |
| deletion | `{--text--}` |
| substitution | `{~~old~>new~~}` |
| highlight | `{==text==}` (normally followed by a comment) |
| comment | `{>>text<<}` (optional author convention `{>>@name: text<<}`) |

## Decisions

### 1. Parsing (`markise-md`, `ParseOptions::critic_markup`, default on)

- A dialect extension like wikilinks (PR #60): one parse option, the setting
  `markdown.criticMarkup` (default on), re-parse of the open document in the background when it
  is switched, byte-identical round-trip with it on and off (the text is never rewritten).
- **Inline only.** Marks are recognised in inline content (paragraphs, headings, table cells,
  list items, quotes); the content runs to the first matching closer *within the block*. A mark
  never spans blocks: CriticMarkup allows `{++` + blank line + `++}` to insert a paragraph
  break, but in v1 both halves stay plain text. A mark may cross a soft line break inside one
  paragraph.
- **Precedence.** Code blocks, math blocks and HTML blocks are never scanned. Inline elements
  are recognised left to right: a code span or inline math that starts before an opener
  contains it; a code span that starts inside a mark and ends after its closer wins over the
  mark (as for wikilinks). Inline math that starts inside a mark and would end after it does
  not cancel the mark (its `$` is then literal inside).
- **No nesting, no escapes.** Inside a mark CriticMarkup is off; the first closer of the same
  kind ends it. `\{` before an opener makes it text (Markdown's own escape). A substitution
  needs `~>` (otherwise text). Empty marks are marks (`{>><<}` is the comment Add Comment
  creates).
- **Content.** Insertions, deletions, highlights and both sides of a substitution are parsed as
  Markdown (so `{++**bold**++}` is bold); trailing spaces inside are kept as text.
- **Comments are plain text** (owner decision): no inline parsing inside `{>>…<<}` — `**x**`,
  links and code show literally — and a backtick in a comment opens no code span, so the
  code-span precedence rule does not apply to comments. A single line break inside a comment is
  allowed (a soft break of the paragraph); a comment containing a blank line is two paragraphs
  and stays plain text (v1 limitation).
- **Linear time.** Closer searches and the code-span crossing check are memoised per mark kind
  and closer (alternating kinds do not evict each other), the `~>` search per closer; a test
  counts the bytes scanned (not time) for 1× and 16× adversarial inputs, mixed kinds included.
  The crossing check honours backslash escapes (an escaped backtick opens nothing).
- **Plain text** (outline entries, image alt text) reads the original text: insertions, the new
  side of substitutions and comments are left out.

### 2. Neutral seam (`markise-doc`)

`InlineKind::Critic(CriticKind)` with kinds Insert, Delete, Substitute, Highlight, Comment and
the two marker-less parts Old / New of a substitution (markers: the delimiters; a substitution
also has `~>` as its middle marker). `markise_doc::review` finds marks through the inline views
(`all_marks`, `marks_near`, `mark_at`); a document without marks costs one byte scan for the
openers, and only leaves holding an opener are parsed. Planner ids `review.addComment`,
`review.accept`, `review.reject`, `review.acceptAll`, `review.rejectAll`,
`review.deleteComment`, `review.cleanCopy`.

### 3. Commands (`markise-md` planners; one patch set each = one undo step)

- **Add Comment** (⌥⌘M / Ctrl+Alt+M, Edit → Comments & Suggestions, the document's context
  menu): a selection becomes `{==selection==}{>><<}`, a caret gets `{>><<}`; the caret goes into
  the comment, which is revealed as source, so the user types the comment in place. Selection
  ends that cut through a styled span are widened to the whole span; selections over several
  blocks, inside code or math, or touching an existing mark are refused. The formatting
  toolbar has a Comment button (speech bubble) for the same command, shown only while
  CriticMarkup is on. **Shortcut:** Apple documents ⌥⌘M as Minimize All; Markise deliberately
  overrides it in-app for Add Comment, as Google Docs does, so it is taken off the
  reserved-chord list. It is the key equivalent of our own menu item (Edit → Comments &
  Suggestions), so AppKit should route it to the app — to be verified by hand on macOS.
- **Enter inside a comment** (paragraphs only) inserts one line break inside the comment,
  followed by the continuation prefix: a continuation line repeats its own prefix; the first
  line of a list or task item gets the quote markers plus the item's content indent (marker and
  its spaces, the task box not included); a quote line its markers. In single-line blocks
  (headings, setext headings, table rows) Enter inside a comment is ignored, and so it is when
  either part of the comment's line around the caret would be empty or whitespace only (no
  blank line inside a mark). Beyond these fast checks one general rule decides: the top-level
  block is parsed alone with the line break inserted, and the Enter happens only if the same
  comment still spans both lines (text after the caret that starts block syntax — `# `, `> `,
  `- `, `1. `, a fence, a setext underline — would end the paragraph, so Enter does nothing).
- **Accept / Reject** at the caret or at an offset (`at` parameter: the panel and the hover bar):
  insertion accepted → its text, rejected → nothing; deletion the other way round; substitution
  → new / old text. A comment written directly after the suggestion goes with it. The caret
  ends after the resulting text.
- **Accept All / Reject All**, **Delete Comment** (a highlight and its attached comment go
  together; the highlighted text stays).
- **Whitespace.** When a mark is removed without replacement, one adjacent space goes too: the
  following one when there are spaces on both sides or the mark starts its line, otherwise the
  preceding one when the mark is followed by punctuation or the line end.
- **Accept All, Reject All and clean copies** are literally one mark after another: each mark
  is resolved by the one-mark planner on a working copy of the document (incrementally
  re-parsed), the next mark is looked up after it, and the overall difference becomes one patch
  (one undo step). A property test compares the result with resolving one by one.
- Menu `enabled` checks for the whole-document commands answer from the setting alone (menus
  are built every frame); one-mark commands parse only the caret's leaf.

### 4. Rendering (`markise-layout`, `markise-theme`, `markise-ui-egui`)

Insertions green and underlined, deletions red and struck through, a substitution as both,
highlights on a soft highlight colour. Colours come from the theme (`Theme::review_colors`):
the tip and caution alert accents and the `mark` background when they have the expected hue,
otherwise fixed light/dark fallbacks. The caret entering a mark shows its raw source (spec 02
inline reveal) — for a **comment only when the caret is inside its text**, not when it merely
sits before or after the mark (owner feedback); until then a comment is one caret stop.

**Comments** (Google Docs style; setting `markdown.commentDisplay`, Preferences → Markdown →
Show comments, and a switch in the comments panel header):

- In the text a comment collapses to a speech-bubble glyph in the accent colour after its
  passage (the highlighted text stays in the flow).
- `margin` (default): its body is a rounded card in the right margin, aligned with its line
  (cards stack downwards when they would overlap): bubble icon, `@author` (from the `@name:`
  prefix) in bold, then the text in the normal-weight UI font, wrapped at about 240 pt, line
  breaks kept. A thin connector in the margin and the comment tint link it to its line.
  Cards do not depend on the theme's column width (owner decision, issue #91): while the
  document contains at least one comment, a **comment gutter** of 260 pt (points, so it scales
  with the zoom) is reserved at the right of the document surface, and the content column
  lays out and centres in the remaining width (as word processors do with their comments
  pane). Without comments the gutter is not reserved and the layout is unchanged. Whether the
  document has comments is answered by a worker from a text snapshot (never a whole-document
  scan on the UI thread), one lookup at a time and at most every 0.4 s while the document
  changes, so the layout does not jump per keystroke: the gutter appears shortly after the
  first comment and is released shortly after the last one goes. A gutter change is applied
  only on a frame without pointer presses, releases or a held button, so a click is never
  hit-tested against the geometry of two different column widths. When the window is too narrow (the surface
  without its scrollbar, i.e. window minus sidebar, would leave under 420 pt beside the
  gutter) no gutter is reserved and the bubbles behaviour applies. Cards are only placed in
  the part of the surface that is actually visible (window and clip edge, scrollbar
  excluded); if the visible room right of the column is under 190 pt, bubbles apply too.
- `bubbles`: only the glyph; resting the pointer on it for 150 ms opens a non-modal popover card
  (same look) that stays while the pointer is on the glyph or the card and closes when it
  leaves. The popover opens below the glyph and flips left or up near the visible area's
  edges, so it always stays on screen.
- A click on the glyph or the card puts the caret at the end of the comment's text (its source
  is revealed for editing); in bubbles mode the popover stays pinned until Esc.
- Only the visible items' comments are looked at and only shown cards are shaped (layout
  reports glyph positions per item, `BlockLayout::comments`).

The source editor colours marks with the existing token classes (inserted text as link text,
deleted text struck, highlights marked, comments as metadata).

### 5. Export

HTML: suggestions as `<ins class="critic">` / `<del class="critic">` (a substitution as both);
with comments kept (default, setting `markdown.exportComments`, Preferences → Markdown and
File → Export → Include Comments) a highlight is `<mark class="critic" title="its comment">` and a comment
`<span class="critic comment">`; stripped, highlights are plain text and comments are dropped.
PDF/PNG/Pandoc (export model): insertions underlined, deletions struck, comments as
`[comment]` in italics or dropped.

### 6. UI

- Comments panel: an activity-bar item (speech bubble) listing comments and suggestions in
  document order with counts, Accept All / Reject All, per-item Accept / Reject / Delete; a click
  on an item's text jumps to it. The list is built on a worker from a text snapshot once typing
  has paused for 250 ms (row data and counts precomputed) and only visible rows are drawn.
  Every row action carries the revision it was listed at: while the document is at another
  revision the rows are disabled (a click does nothing) until the next listing arrives, so a
  stale row never acts on another mark.
- Hover bar: Accept / Reject above a suggestion under the pointer (rendered view, editable).
  Any edit hides it; it comes back with fresh data on the next pointer move over a mark.
- **Save Copy Without Comments…** (File): asks through the confirm seam whether to accept or
  reject pending suggestions (skipped when there are none), asks for a new file (default
  `<name> (clean).md`), refuses the document's own file, and writes through the checked save
  path (`save_checked`, never overwriting a file changed since the dialog). The open document is
  not changed.
- The switches live in Preferences → Markdown ("Comments & suggestions (CriticMarkup)", "Show
  comments: In the margin / As bubbles", "Keep comments in exports"); the export one is also
  File → Export → Include Comments, the display one also the comments panel's switch.

### 7. CommonMark conformance

The conformance suite runs with the extension off (pure CommonMark/GFM dialect). A test parses
every example with CriticMarkup on and checks the export is unchanged; none of the 0.31.2 or
GFM examples contains an opener, so none differs (the test pins that).

## Later (not v1)

Marks across blocks (paragraph insertions/deletions); a track-changes typing mode (edits become
suggestions); sending comments to an AI through the MCP server; Word comments through Pandoc;
comment threads and resolved state.
