# Comments and suggestions

Markise shows review marks written in **CriticMarkup**, a plain-text syntax. Because the marks
are part of the text, they survive any editor, version control and e-mail.

## Syntax

| Mark | Write |
|---|---|
| Comment | `{>>Check this date<<}` |
| Comment with an author | `{>>@dana: Check this date<<}` |
| Suggested insertion | `{++new text++}` |
| Suggested deletion | `{--old text--}` |
| Suggested replacement | `{~~old~>new~~}` |
| Highlight (usually followed by a comment) | `{==this sentence==}{>>why?<<}` |

Marks stay within one paragraph and cannot be nested. Comments are plain text (no formatting
inside). Marks inside code, math and HTML blocks are left alone, and `\{` writes a literal brace.

## Add and review

- Select text and choose **Edit → Comments & Suggestions → Add Comment**
  (<kbd>Alt+Cmd+M</kbd>, <kbd>Ctrl+Alt+M</kbd>), use the **Comment** button in the toolbar, or
  type the marks yourself.
- In the rendered view, insertions are underlined, deletions struck through, and comments
  appear as cards in the right margin — or, if you prefer, as bubbles that show on hover
  (**Preferences → Markdown → Show comments**).
- **Accept Suggestion** applies a suggestion and removes its marks; **Reject Suggestion**
  restores the original. **Accept All Suggestions** and **Reject All Suggestions** do this for
  the whole document; **Delete Comment** removes a comment. These commands are in **Edit →
  Comments & Suggestions** and in the context menu.
- The **Comments** panel in the sidebar lists every mark with **Accept** / **Reject** (or
  **Delete**) buttons, and **Accept All** / **Reject All** at the top.

Accepting keeps inserted text, drops deleted text and keeps the new side of a replacement;
rejecting does the opposite. Each action is an ordinary edit, so undo works.

**Preferences → Markdown → Comments & suggestions (CriticMarkup)** turns the feature off; the
marks then show as the plain text they are.

## Save a clean copy

**File → Save Copy Without Comments…** writes a copy with all comments and highlights removed
(the highlighted text is kept). If suggestions are still pending, Markise first asks whether to
**Accept All** or **Reject All** of them in the copy. The open document does not change, and the
copy cannot replace the document's own file. The suggested name is `<name> (clean).md`.

## In exports

Exports never accept suggestions on their own: insertions appear underlined and deletions
struck through. Comments are kept by default — in HTML as marked-up notes, in PDF, PNG and
Pandoc formats as italic text in brackets. Turn them off with **File → Export → Include
Comments** or **Preferences → Markdown → Keep comments in exports**; for a document without any
marks, save a clean copy first and export that.
