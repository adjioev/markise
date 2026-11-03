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

Marks stay within one paragraph, heading or table cell and cannot be nested. Comments are plain
text (no formatting inside). Marks inside code, math and HTML blocks are left alone, and `\{`
writes a literal brace.

## Add a comment

- Select text and choose **Edit → Comments & Suggestions → Add Comment**
  (<kbd>Alt+Cmd+M</kbd>, <kbd>Ctrl+Alt+M</kbd>), use the **Comment** button in the toolbar or the
  context menu, or type the marks yourself. Without a selection, the comment is added at the caret.
- The new comment opens in its card. Write the text and press <kbd>Cmd+Enter</kbd>
  (<kbd>Ctrl+Enter</kbd>) or click anywhere else to save; <kbd>Esc</kbd> cancels and removes the
  empty comment. Adding a comment and its text is a single undo step.
- Comments work in paragraphs, headings, list items, quotes and **table cells**. In a table, the
  selection must stay inside one cell. Empty cells can take a comment too; a comment in a cell is
  one line, and a `|` you type in it is saved as `\|` (it still shows as `|`).
- If a comment can't go where you are (inside code or math, across table cells or paragraphs, or
  right after a backslash), Markise tells you why instead of doing nothing.

## Edit and remove comments

- Click a comment's card, or its bubble, to edit the text right there. <kbd>Enter</kbd> starts a
  new line; in headings and table cells comments are one line, so <kbd>Enter</kbd> saves. Blank
  lines are dropped.
- An author prefix such as `@dana:` is shown above the text and kept when you edit.
- Clear the text and save to remove the comment together with its highlight. Each saved edit is
  one undo step.
- While a comment is being edited, its text counts as an unsaved change: saving the document
  includes it, and closing or opening another file asks first.
- In Source mode, and in the source pane of Side by Side, comments are edited as plain
  `{>>…<<}` text.

## Review suggestions

- In the rendered view, insertions are underlined and deletions struck through. Comments appear
  as cards in the right margin, or as bubbles that show on hover if you prefer
  (**Preferences → Markdown → Show comments**).
- **Accept Suggestion** applies a suggestion and removes its marks; **Reject Suggestion**
  restores the original. **Accept All Suggestions** and **Reject All Suggestions** do this for the
  whole document, and **Delete Comment** removes a comment. These commands are in **Edit →
  Comments & Suggestions** and in the context menu, and they work inside table cells too.
- The **Comments** panel in the sidebar lists every mark with **Accept** / **Reject** (or
  **Delete**) buttons, and **Accept All** / **Reject All** at the top.

Accepting keeps inserted text, drops deleted text and keeps the new side of a replacement;
rejecting does the opposite. Each action is an ordinary edit, so undo works.

## Choose how comments show

**View → Comments**, or the switch in the status bar, picks how comments and suggestions are
shown. The setting is remembered:

- **All Markup:** highlights, comment cards and coloured suggestions.
- **Simple Markup:** comment icons and coloured suggestions only, with no cards and no highlight
  colour. Click an icon to read or edit the comment.
- **No Markup (Final):** the text as if you had accepted every suggestion; comments are hidden.
- **Original:** the text as if you had rejected every suggestion; comments are hidden.

These views only change the display: the file never changes, and exports keep their own setting.
In **No Markup** and **Original** the document is read-only and the status bar says so. Copying
takes the text as shown. Switch back to edit, or use Source mode.

**Preferences → Markdown → Comments & suggestions (CriticMarkup)** turns the feature off. The
marks then show as the plain text they are, and View → Comments is unavailable.

## Save a clean copy

**File → Save Copy Without Comments…** writes a copy with all comments and highlights removed
(the highlighted text is kept). If suggestions are still pending, Markise first asks whether to
**Accept All** or **Reject All** of them in the copy. The open document does not change, and the
copy cannot replace the document's own file. The suggested name is `<name> (clean).md`. This
command is only available while **Comments & suggestions** is turned on: when it is off,
comments are ordinary text and there is nothing to remove.

## In exports

Exports never accept suggestions on their own: insertions appear underlined and deletions
struck through. Comments are kept by default — in HTML as marked-up notes, in PDF, PNG and
Pandoc formats as italic text in brackets. Turn them off with **File → Export → Include
Comments** or **Preferences → Markdown → Keep comments in exports**; for a document without any
marks, save a clean copy first and export that.
