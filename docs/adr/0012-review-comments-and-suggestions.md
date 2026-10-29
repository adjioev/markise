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
  Markdown (so `{++**bold**++}` is bold); trailing spaces inside are kept as text. A comment's
  text is not parsed.
- **Linear time.** Closer searches are memoised per mark kind, the code-span crossing check per
  closer and the `~>` search per closer; a test counts the bytes scanned (not time) for 1× and
  16× adversarial inputs.
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
  blocks, inside code or math, or touching an existing mark are refused. ⌥⌘M was in the
  reserved-chord list (macOS Minimize All, only reachable by Option-clicking the Window menu);
  it is taken off that list.
- **Accept / Reject** at the caret or at an offset (`at` parameter: the panel and the hover bar):
  insertion accepted → its text, rejected → nothing; deletion the other way round; substitution
  → new / old text. A comment written directly after the suggestion goes with it. The caret
  ends after the resulting text.
- **Accept All / Reject All**, **Delete Comment** (a highlight and its attached comment go
  together; the highlighted text stays).
- **Whitespace.** When a mark is removed without replacement, one adjacent space goes too: the
  following one when there are spaces on both sides or the mark starts its line, otherwise the
  preceding one when the mark is followed by punctuation or the line end.
- Menu `enabled` checks for the whole-document commands answer from the setting alone (menus
  are built every frame); one-mark commands parse only the caret's leaf.

### 4. Rendering (`markise-layout`, `markise-theme`)

Insertions green and underlined, deletions red and struck through, a substitution as both,
highlights on the highlight colour, a comment as a small bubble (speech mark + text, at most 60
characters, one caret stop). Colours come from the theme (`Theme::review_colors`): the tip and
caution alert accents and the `mark` background when they have the expected hue, otherwise
fixed light/dark fallbacks. The caret entering a mark shows its raw source (spec 02 inline
reveal), styled the same. The source editor colours marks with the existing token classes
(inserted text as link text, deleted text struck, highlights marked, comments as metadata).

### 5. Export

HTML: suggestions as `<ins class="critic">` / `<del class="critic">` (a substitution as both);
with comments kept (default, setting `markdown.exportComments`, File → Export → Include
Comments) a highlight is `<mark class="critic" title="its comment">` and a comment
`<span class="critic comment">`; stripped, highlights are plain text and comments are dropped.
PDF/PNG/Pandoc (export model): insertions underlined, deletions struck, comments as
`[comment]` in italics or dropped.

### 6. UI

- Comments panel: an activity-bar item (speech bubble) listing comments and suggestions in
  document order with counts, Accept All / Reject All, per-item Accept / Reject / Delete; a click
  on an item's text jumps to it. The list is computed only while the panel is shown, when the
  text changed and typing has paused for 250 ms.
- Hover bar: Accept / Reject above a suggestion under the pointer (rendered view, editable).
- **Save Copy Without Comments…** (File): asks through the confirm seam whether to accept or
  reject pending suggestions (skipped when there are none), asks for a new file (default
  `<name> (clean).md`), refuses the document's own file, and writes through the checked save
  path (`save_checked`, never overwriting a file changed since the dialog). The open document is
  not changed.
- The switch lives in View → Comments & Suggestions until the Preferences window (PR #68) lands;
  then it moves to Preferences → Markdown.

### 7. CommonMark conformance

The conformance suite runs with the extension off (pure CommonMark/GFM dialect). A test parses
every example with CriticMarkup on and checks the export is unchanged; none of the 0.31.2 or
GFM examples contains an opener, so none differs (the test pins that).

## Later (not v1)

Marks across blocks (paragraph insertions/deletions); a track-changes typing mode (edits become
suggestions); sending comments to an AI through the MCP server; Word comments through Pandoc;
comment threads and resolved state.
