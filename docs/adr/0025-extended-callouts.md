# ADR 0025: Extended callouts

Status: accepted for implementation.

## Context

A blockquote can start with a GitHub alert, `> [!NOTE]`: five fixed names, no title, nothing to
fold. People write more than that in their notes: any type name, a title of their own, and a body
that can be folded away.

```markdown
> [!warning] Optional title
> Body.

> [!success]- Starts closed, with a title
> Hidden until opened.

> [!info]+ Starts open
> Body.
```

The owner asked for this as a module of its own, not spread through the parser.

## Decision

**A module of `markise-md`.** `crates/markise-md/src/callout.rs` holds the grammar of the first
line, the table of names, default titles and the neutral view. The parser has one hook,
`callout::recognise`, called from the two places that finish a quote (`finish` and `refinish`,
which already held the alert check), and the tree stores one small `Copy` value on the quote
(`BlockKind::BlockQuote { callout }`): type, fold, alert kind. It holds no text or spans, so a shift
never touches it and an edit that changes it is seen by the existing computed-state check of
incremental parses. It is not a crate of its own: the hook needs `ParseOptions` and `Block`, so such
a crate would sit below `markise-md` with nothing else above it, for one module's worth of code.
Layout reads callouts through the format seam and exports through `markise-md`, as they read
everything else.

**The option.** `ParseOptions::callouts`, default on. Off, every extended form stays a plain quote,
as it was. `alerts` keeps governing the bare forms of the five GitHub names.

**The grammar.** The first line of a quote's first paragraph: `[!name]`, then `-` or `+` at once,
then, after white space, the title (inline Markdown). A name is letters, digits, `-` and `_`,
starting with a letter or digit, at most 64 bytes. Text right after the tag is not a callout, so
`[!x](url)` stays a link. A literal-line hint keeps a line plain, as for any input rule.

**Alerts are the narrow case.** The bare tag of a GitHub name recognises as `Callout { alert:
Some(kind) }`: same bytes, same HTML, same Pandoc, same PDF and same drawing as before, including no
icon. `[!NOTE] Title` and `[!NOTE]-` are extended forms.

**Types.** Names and aliases map to a type, the type to a colour and an icon. Colours are the theme's
five alert colours and its quote bar, so every style, palette and imported theme has them without a
new theme field. An unknown name has the note style and shows its name. The default title is the
name, lower case, capitalised.

**The seam.** `FlatContainer::Quote` carries a shared `CalloutView`: accent, icon, written title and
tag end as offsets into the first line, and the default title. Offsets into the line keep it
independent of the block's position, and nothing in it depends on the body: typing there does not
invalidate the header's cached layout. Layout keeps the accent on every leaf of the quote and the
view on the first leaf only (`Container::Quote::head`, spent like a list marker).

**Editing.** The tag is syntax and never shows outside source mode; a written title is ordinary text
with caret stops. Backspace at the start of a title removes the tag; on the first body line of a
callout without a title it removes the tag line, as for an alert. A valid `[!name]` typed as a
quote's first line converts when its `]` is typed, as for an alert, with the caret in a fresh body. A
title or fold mark is added in source mode or by hand-editing the file: the rule fires before they
can be typed.

**Icons** are drawn from primitives in `markise-layout/src/callout.rs` (original, no assets).

## Limits

- A `[!name]` that is also a defined reference label reads as a callout, not a link.
- Pictures in a title are not drawn.
- PDF and PNG exports do not draw the icon.

## Exports

An extended callout is `ExBlock::Callout` in the export model, with its title resolved to inlines.
Bare alerts keep `ExBlock::Alert` and every byte of their output.

| Target | Foldable | Not foldable |
|---|---|---|
| HTML | `<details class="…"><summary>title</summary>…</details>`, `open` for `+` | `<div class="…"><p>title</p>…</div>` |
| PDF, PNG | open, bar in the accent colour, bold title, no icon | the same |
| Pandoc | `Div` classes `callout` and the type, attributes `callout` (the name) and `fold`, a `title` `Div` first | the same without `fold` |

HTML classes are `md-alert md-alert-<accent> md-callout md-callout-<type> <type>`, and the title has
`md-alert-text-<accent>`. They reuse the theme's alert rules, so no CSS is added and a page without
callouts is byte-identical to before. A `quote` type has no alert colour there and uses the note
colour; an unknown name has `data-callout` with its name, escaped.
