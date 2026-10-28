//! The Markdown typing pipeline (ADR 0006 §5.1; spec 02 §2.1, §4, §13): auto-pairing,
//! smart punctuation, literal-line hints for block syntax typed at a block start, and the
//! immediate input rules (quote, lists, tasks, callouts) as separate `Auto` conversions.
//!
//! Hints and rules apply where a block's text starts: the first line of a paragraph (also
//! inside quotes and list items — the hint's column keeps the containers, ADR 0006 §2.6), an
//! empty list item, or an empty line at the top level. Never on a soft-broken line (C9).

use markise_doc::{
    DocPos, EditPrefs, FormatDoc, HintOrigin, LeafKind, LiteralHint, PatchSet, Plan, PlanRequest, PlanStep, RulePlan,
    Selection, SmartDashes, SmartPunctuation, TypingPlan, plain_typing,
};

use super::Edits;
use crate::text::line_end;
use crate::{Document, line_start, scan};

/// Block syntax a block-start line would open without a hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opener {
    /// Enter-only syntax (spec 02 §2.2).
    EnterOnly,
    /// An immediate-rule construct (list item, quote) whose trigger may still be incomplete.
    Immediate,
}

/// Classifies `line` (the text from a block's content start to the end of its physical line)
/// as a would-be opener.
pub(crate) fn opener_of(line: &str, opts: &crate::ParseOptions) -> Option<Opener> {
    let b = line.as_bytes();
    let end = b.len();
    let p = scan::skip_ws(b, 0, end);
    if p > 3 || p >= end {
        return None;
    }
    if scan::atx_heading(b, p, end, opts.strict).is_some()
        || scan::fence_open(b, p, end).is_some()
        || scan::thematic_break(b, p, end, opts.remap_unicode_punctuation)
        || scan::is_toc_marker(&line[p..], opts.remap_unicode_punctuation)
        || scan::footnote_def_start(b, p, end).is_some()
        || (opts.block_html && scan::html_block_start(b, p, end).is_some())
        || link_def_like(&line[p..])
    {
        return Some(Opener::EnterOnly);
    }
    if scan::list_marker(b, p, end).is_some() || b[p] == b'>' {
        return Some(Opener::Immediate);
    }
    if opts.remap_unicode_punctuation && (line[p..].starts_with('〉') || line[p..].starts_with('》')) {
        return Some(Opener::Immediate);
    }
    None
}

/// `[label]:` at the start (a link reference definition once the paragraph closes).
pub(crate) fn link_def_like(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('[') else { return false };
    let mut esc = false;
    for (i, c) in rest.char_indices() {
        match c {
            '\\' if !esc => esc = true,
            ']' if !esc => return i > 0 && rest[i + 1..].starts_with(':'),
            '[' if !esc => return false,
            _ => esc = false,
        }
    }
    false
}

/// The immediate rule completed by `before` (the block text up to and including the typed
/// character), if any.
fn immediate_rule(
    before: &str,
    item_first: Option<bool>,
    quote_first: bool,
    opts: &crate::ParseOptions,
) -> Option<&'static str> {
    let t = before.trim_start_matches(' ');
    if before.len() - t.len() > 3 {
        return None;
    }
    match t {
        "> " => return Some("Quote"),
        "- " | "* " | "+ " => return Some("Bullet List"),
        _ => {}
    }
    if opts.remap_unicode_punctuation && (t == "〉 " || t == "》 ") {
        return Some("Quote");
    }
    // ordered: 1–9 digits, `.` or `)`, space
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && matches!(&t[digits..], ". " | ") ") {
        return Some("Ordered List");
    }
    // `[ ] ` / `[x] ` at the start of a bullet item's first paragraph (ordered items: MT-1)
    if item_first == Some(false) && matches!(t, "[ ] " | "[x] " | "[X] ") {
        return Some("Task List");
    }
    // `[!NOTE]` … as the first line of a quote (spec 02 §2.1)
    if quote_first && opts.alerts {
        let u = t.to_ascii_uppercase();
        if matches!(
            u.as_str(),
            "[!NOTE]" | "[!TIP]" | "[!IMPORTANT]" | "[!WARNING]" | "[!CAUTION]"
        ) {
            return Some("Callout");
        }
    }
    None
}

/// `true` when `line` (a block start line after typing; `typed_end` bytes of it are before the
/// caret) already reads as a task box or a callout tag although the caret is still before its
/// closing `]` (the auto-paired closer). Parsed as is, the line would convert under the caret
/// and hide the bytes it is in.
fn pending_rule(
    line: &str,
    typed_end: usize,
    item_first: Option<bool>,
    quote_first: bool,
    opts: &crate::ParseOptions,
) -> bool {
    let t = line.trim_start_matches(' ');
    let lead = line.len() - t.len();
    if lead > 3 {
        return false;
    }
    let b = t.as_bytes();
    if item_first == Some(false) && scan::task_box(b, 0, b.len()).is_some() {
        return typed_end < lead + 3;
    }
    if quote_first && opts.alerts {
        let tag = t.trim_end_matches([' ', '\t']);
        if immediate_rule(tag, None, true, opts) == Some("Callout") {
            return typed_end < lead + tag.len();
        }
    }
    false
}

fn closer_of(c: char) -> Option<char> {
    Some(match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        _ => return None,
    })
}

fn md_pair(c: char, opts: &crate::ParseOptions) -> bool {
    match c {
        '*' | '`' | '_' | '~' => true,
        '$' => opts.inline_math,
        '^' => opts.superscript,
        '=' => opts.highlight,
        _ => false,
    }
}

fn is_closing_like(c: char) -> bool {
    matches!(
        c,
        ')' | ']' | '}' | '"' | '\'' | '\u{201D}' | '\u{2019}' | '>' | '*' | '_' | '`' | '~' | '$'
    )
}

impl Document {
    /// Where the text of the block holding `at` starts on `at`'s line, when `at` is on a block
    /// start line that input rules apply to: the first line of a paragraph (in any
    /// containers), an empty list item, a blank top-level line. Returns (content start,
    /// item first-paragraph flag `Some(ordered)`, quote-first-block flag).
    fn rule_context(&self, at: usize) -> Option<(usize, Option<bool>, bool)> {
        let text = self.text();
        let ls = line_start(text, at);
        if let Some(leaf) = self.leaf(at) {
            let first = *leaf.lines.first()?;
            if leaf.kind != LeafKind::Paragraph || line_start(text, first.start) != ls || at < first.start {
                return None;
            }
            // in a list item's first paragraph? in a quote's first block?
            let path = self.block_path_at(first.start);
            let mut item = None;
            let mut quote_first = false;
            for d in 1..path.len() {
                if let Some((b, o)) = self.block_at_path_rel(&path[..d]) {
                    let child_first = b.children.first().is_some_and(|c| c.span.start + o == first.start);
                    match &b.kind {
                        crate::block::BlockKind::ListItem { number, .. } if child_first => {
                            item = Some(number.is_some());
                        }
                        crate::block::BlockKind::BlockQuote { .. } => quote_first = child_first,
                        _ => {}
                    }
                }
            }
            return Some((first.start, item, quote_first));
        }
        // an empty list item: the text goes after the marker's spacing
        if let Some(it) = self.item_at(at) {
            if it.empty && it.ls == ls && at >= it.syntax_end {
                return Some((at.min(it.content), Some(it.number.is_some()), false));
            }
            return None;
        }
        let (le, _) = line_end(text, ls);
        // an empty quote (`> ` with nothing after it): text goes after the marker
        let path = self.block_path_at(at.min(le));
        if let Some((b, _)) = self.block_at_path_rel(&path)
            && matches!(b.kind, crate::block::BlockKind::BlockQuote { .. })
            && text[at..le].trim().is_empty()
        {
            let empty = b.children.is_empty();
            return Some((at, None, empty));
        }
        // a blank top-level line
        if self.block_path_at(at).is_empty() && text[ls..le].trim().is_empty() {
            // not right after a paragraph line: that would be its continuation (C9)
            let continues = ls > 0 && {
                let pe = crate::text::prev_line_content_end(text, ls);
                let pls = line_start(text, pe);
                !text[pls..pe].trim().is_empty()
                    && self
                        .leaf(pe.saturating_sub(1).max(pls))
                        .is_some_and(|l| l.kind == LeafKind::Paragraph)
            };
            if continues {
                return None;
            }
            return Some((ls, None, false));
        }
        None
    }

    /// The typing pipeline step (ADR 0006 §5.1).
    pub(crate) fn md_on_text(&self, req: &PlanRequest, typed: &str) -> TypingPlan {
        let mut tp = plain_typing(req, typed);
        if typed.is_empty() || crate::text::count_terminators(typed) > 0 || req.source_mode {
            return tp;
        }
        let text = self.text();
        let sel = req.selection.primary();
        let r = sel.range();
        let ctx = self.rule_context(r.start);
        let in_code = self.in_code_text(r.start);
        // ---- auto-pair and smart punctuation (spec 02 §4, §13)
        let mut typed_s = typed.to_string();
        let mut caret_after: Option<usize> = None;
        let mut ch = typed.chars();
        if let (Some(c), None) = (ch.next(), ch.next())
            && !in_code.is_block()
            && let Some(p) = self.pair_plan(req, c, in_code)
        {
            match p {
                PairPlan::StepOver(n) => {
                    tp.step = PlanStep::patches(PatchSet::empty());
                    tp.selection_after = Some(Selection::caret(r.start + n));
                    // a callout tag completed by stepping over its `]`
                    if let Some((cs, _, true)) = ctx {
                        let ls = line_start(text, r.start);
                        let (ce, _) = line_end(text, r.start);
                        let end = r.start + n;
                        if cs <= r.start
                            && line_start(text, cs) == ls
                            && text[end..ce].trim().is_empty()
                            && immediate_rule(&text[cs..end], None, true, self.options()) == Some("Callout")
                        {
                            let mut hints = self.hints().clone();
                            hints.insert(LiteralHint::at(ls, (cs - ls) as u32, HintOrigin::RuleTrigger));
                            let mut converted = hints.clone();
                            converted.remove_line(ls);
                            tp.step.hints_after = Some(hints);
                            tp.rule = Some(RulePlan {
                                label: "Callout",
                                plan: Plan {
                                    steps: vec![PlanStep::hints(converted)],
                                    selection_after: Some(Selection::caret_at(DocPos::with_virt(ce, 1))),
                                    label: None,
                                },
                            });
                        }
                    }
                    return tp;
                }
                PairPlan::Wrap(open, close) => {
                    let mut ed = Edits::new();
                    ed.insert(r.start, open.clone());
                    ed.insert(r.end, close);
                    if let Some(ps) = ed.build() {
                        tp.step = PlanStep::patches(ps);
                        let a = r.start + open.len();
                        let f = r.end + open.len();
                        tp.selection_after = Some(if sel.anchor.offset <= sel.focus.offset {
                            Selection::range(a, f)
                        } else {
                            Selection::range(f, a)
                        });
                    }
                    return tp;
                }
                PairPlan::Insert(s, caret) => {
                    typed_s = s;
                    caret_after = Some(caret);
                }
            }
        }
        // an empty item written without the space after its marker (`-`): the text needs it
        if sel.is_caret()
            && !typed.starts_with([' ', '\t'])
            && let Some(it) = self.item_at(r.start)
            && it.empty
            && it.ls == line_start(text, r.start)
            && r.start == it.syntax_end
            && text[r.start..].chars().next().is_none_or(|c| c == '\n' || c == '\r')
        {
            typed_s = format!(" {typed_s}");
            caret_after = Some(caret_after.map_or(typed_s.len(), |k| k + 1));
        }
        if typed_s != typed {
            tp.step = PlanStep::patches(PatchSet::single(r.clone(), typed_s.clone()));
            tp.selection_after = caret_after.map(|k| Selection::caret(r.start + k));
        }
        // smart dashes: a conversion of the previous characters, its own `Auto` step (Q9)
        if req.prefs.smart_punctuation == SmartPunctuation::ConvertOnInput
            && typed == "-"
            && sel.is_caret()
            && !in_code.any()
            && let Some(rule) = self.dash_rule(r.start, req.prefs)
        {
            tp.rule = Some(rule);
            return tp;
        }
        let Some((cs, item_first, quote_first)) = ctx else {
            self.soft_line_hint(&mut tp, r.clone(), &typed_s);
            return tp;
        };
        let ls = line_start(text, r.start);
        let (ce, _) = line_end(text, r.end);
        if line_start(text, r.end) != ls || r.start < cs {
            return tp;
        }
        let rest = &text[r.end..ce];
        let before = format!("{}{typed_s}", &text[cs..r.start]);
        let line_after = format!("{before}{rest}");
        let col = (cs - ls) as u32;
        let hinted = self.hints().get(ls).copied().filter(|h| h.origin.is_literal());
        let opener = opener_of(&line_after, self.options()).or_else(|| {
            // a task box / callout tag already closed by its auto-paired `]` while the caret is
            // still inside it: the line stays literal until the trigger is typed (§2.1, §4.2)
            pending_rule(&line_after, before.len(), item_first, quote_first, self.options())
                .then_some(Opener::Immediate)
        });
        let mut hints = self.hints().clone();
        match (opener, hinted) {
            (Some(o), None) => {
                let origin = match o {
                    Opener::EnterOnly => HintOrigin::EnterPending,
                    Opener::Immediate => HintOrigin::RuleTrigger,
                };
                hints.insert(LiteralHint::at(ls, col, origin));
            }
            (Some(Opener::EnterOnly), Some(h)) if h.origin == HintOrigin::RuleTrigger => {
                hints.insert(LiteralHint::at(ls, col, HintOrigin::EnterPending));
            }
            (None, Some(_)) => {
                hints.remove_line(ls);
            }
            _ => {}
        }
        // immediate rules (spec 02 §2.1): completed by this keystroke at the block start
        let quote_depth = text[ls..cs].matches(['>', '〉', '》']).count();
        let rule = immediate_rule(&before, item_first, quote_first, self.options())
            .filter(|l| *l != "Quote" || quote_depth < 10)
            .filter(|_| !in_code.any());
        if let Some(label) = rule {
            // T1 shows the literal trigger (hinted), T2 converts (ADR 0006 §2.6)
            hints.insert(LiteralHint::at(ls, col, HintOrigin::RuleTrigger));
            let mut converted = hints.clone();
            converted.remove_line(ls);
            let mut step = PlanStep::hints(converted);
            let mut sel_after = None;
            if label == "Bullet List"
                && before.trim_start() == "* "
                && rest.starts_with('*')
                && req.prefs.auto_pair_markdown
            {
                // the auto-paired `*` of `*` + Space is removed (spec 02 §2.1 note)
                let at = r.start + typed_s.len();
                step.patches = PatchSet::single_verbatim(at..at + 1, "");
            }
            if label == "Callout" {
                // a fresh body paragraph inside the callout (virtual until typed into)
                let end = r.start + typed_s.len() + rest.len();
                sel_after = Some(Selection::caret_at(DocPos::with_virt(end, 1)));
            }
            tp.step.hints_after = Some(hints);
            tp.rule = Some(RulePlan {
                label,
                plan: Plan {
                    steps: vec![step],
                    selection_after: sel_after,
                    label: None,
                },
            });
            return tp;
        }
        if hints != *self.hints() {
            tp.step.hints_after = Some(hints);
        }
        tp
    }

    /// C9 + ADR 0006 §2.6 on a paragraph's soft-broken line: no input rules, but text that
    /// would open a block there (a list item interrupting the paragraph, a setext underline
    /// turning the paragraph into a heading, a heading…) gets an `EnterPending` hint so it
    /// stays paragraph text as typed (escaped when the caret leaves).
    fn soft_line_hint(&self, tp: &mut TypingPlan, r: std::ops::Range<usize>, typed: &str) {
        let text = self.text();
        let Some(leaf) = self.leaf(r.start).or_else(|| self.leaf_before(r.start)) else {
            return;
        };
        if leaf.kind != LeafKind::Paragraph {
            return;
        }
        let line = match leaf.lines.iter().position(|l| l.start <= r.start && r.end <= l.end) {
            Some(0) | None if leaf.lines.iter().any(|l| l.start <= r.start && r.end <= l.end) => return,
            Some(i) => leaf.lines[i],
            None => {
                // an empty line right after the paragraph: typing there continues it
                let last = *leaf.lines.last().unwrap_or(&leaf.span);
                let ls = line_start(text, r.start);
                let (le, _) = line_end(text, ls);
                let next = line_end(text, last.end).1;
                if ls != next || !text[ls..le].trim_start_matches(['>', ' ', '\t']).is_empty() {
                    return;
                }
                let cs = ls + (text[ls..le].len() - text[ls..le].trim_start_matches(['>', ' ', '\t']).len());
                crate::text::Span::new(cs.min(r.start), le)
            }
        };
        if r.start < line.start || r.end > line.end {
            return;
        }
        let ls = line_start(text, line.start);
        let after = format!("{}{typed}{}", &text[line.start..r.start], &text[r.end..line.end]);
        let t = after.trim();
        let setext = !t.is_empty() && (t.bytes().all(|c| c == b'=') || t.bytes().all(|c| c == b'-'));
        let opens = setext || opener_of(&after, self.options()).is_some();
        let mut hints = self.hints().clone();
        let had = hints.get(ls).is_some_and(|h| h.origin.is_literal());
        if opens && !had {
            hints.insert(LiteralHint::at(ls, (line.start - ls) as u32, HintOrigin::EnterPending));
        } else if !opens && had {
            hints.remove_line(ls);
        } else {
            return;
        }
        tp.step.hints_after = Some(hints);
    }

    /// Whether `at` is inside code: a code block (no Markdown rules) or an inline code span
    /// on its line (backtick parity, a cheap bounded approximation).
    fn in_code_text(&self, at: usize) -> CodeCtx {
        let text = self.text();
        match self.leaf(at).map(|l| l.kind) {
            Some(LeafKind::Paragraph | LeafKind::Heading { .. }) | None => {}
            Some(LeafKind::Table { .. }) => {}
            Some(_) => return CodeCtx::Block,
        }
        let ls = line_start(text, at);
        let before = &text[ls..at];
        if before.len() > 4096 {
            return CodeCtx::None;
        }
        let ticks = before.matches('`').count();
        if ticks % 2 == 1 {
            return CodeCtx::Inline;
        }
        let dollars = before.matches('$').count() - before.matches("\\$").count();
        if self.options().inline_math && dollars % 2 == 1 {
            return CodeCtx::Math;
        }
        CodeCtx::None
    }

    fn pair_plan(&self, req: &PlanRequest, c: char, code: CodeCtx) -> Option<PairPlan> {
        let prefs = req.prefs;
        let text = self.text();
        let sel = req.selection.primary();
        let r = sel.range();
        let next = text[r.end..].chars().next();
        let prev = text[..r.start].chars().next_back();
        let smart = prefs.smart_punctuation == SmartPunctuation::ConvertOnInput && code == CodeCtx::None;
        let md = prefs.auto_pair_markdown && md_pair(c, self.options());
        let code_ok = |c: char| match code {
            CodeCtx::None => true,
            CodeCtx::Inline => c == '`',
            CodeCtx::Math => c == '$',
            CodeCtx::Block => false,
        };
        if !code_ok(c) {
            return None;
        }
        // smart quote characters for an ASCII quote
        let quote_chars = |c: char, opening: bool| -> char {
            match (c, opening) {
                ('"', true) => prefs.quotes[0],
                ('"', false) => prefs.quotes[1],
                ('\'', true) => prefs.quotes[2],
                (_, _) => prefs.quotes[3],
            }
        };
        let opening_ctx =
            prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '[' | '{' | '-' | '\u{2013}' | '\u{2014}'));
        // wrap a selection (spec 02 §4.4)
        if !sel.is_caret() {
            if prefs.auto_pair_brackets
                && let Some(cl) = closer_of(c)
            {
                let (o, cl) = if smart && matches!(c, '"' | '\'') {
                    (quote_chars(c, true), quote_chars(c, false))
                } else {
                    (c, cl)
                };
                return Some(PairPlan::Wrap(o.to_string(), cl.to_string()));
            }
            if md {
                return Some(PairPlan::Wrap(c.to_string(), c.to_string()));
            }
            return None;
        }
        // step over a closer already to the right (§4.2)
        let typed_as = if smart && matches!(c, '"' | '\'') {
            if c == '\'' && prev.is_some_and(char::is_alphanumeric) {
                quote_chars(c, false)
            } else {
                quote_chars(c, opening_ctx)
            }
        } else {
            c
        };
        let is_closer = matches!(c, ')' | ']' | '}' | '"' | '\'') || (md && !matches!(c, '(' | '[' | '{'));
        if (prefs.auto_pair_brackets || md)
            && is_closer
            && next == Some(typed_as)
            && !(c == '\'' && typed_as == prefs.quotes[2])
            && (!matches!(c, '"' | '\'') || !opening_ctx || smart)
        {
            return Some(PairPlan::StepOver(typed_as.len_utf8()));
        }
        if prev == Some('\\') {
            return None;
        }
        let pairable_next = next.is_none_or(|n| n.is_whitespace() || is_closing_like(n));
        if prefs.auto_pair_brackets
            && let Some(cl) = closer_of(c)
        {
            let quote = matches!(c, '"' | '\'');
            let ok = if quote {
                // not an apostrophe (`don't`), not closing an open quote on the line
                let ls = line_start(text, r.start);
                let open_on_line = text[ls..r.start].matches(c).count() % 2 == 1;
                !prev.is_some_and(char::is_alphanumeric) && !open_on_line && pairable_next
            } else {
                pairable_next
            };
            if ok {
                let (o, cl) = if smart && quote {
                    (quote_chars(c, true), quote_chars(c, false))
                } else {
                    (c, cl)
                };
                let s = format!("{o}{cl}");
                return Some(PairPlan::Insert(s, o.len_utf8()));
            }
        }
        if smart && matches!(c, '"' | '\'') {
            return Some(PairPlan::Insert(typed_as.to_string(), typed_as.len_utf8()));
        }
        if md && pairable_next && !(c == '_' && prev.is_some_and(char::is_alphanumeric)) {
            return Some(PairPlan::Insert(format!("{c}{c}"), 1));
        }
        None
    }

    /// Smart dashes (spec 02 §13): `--` → en dash (or em dash), `–-` → em dash.
    fn dash_rule(&self, at: usize, prefs: EditPrefs) -> Option<RulePlan> {
        let text = self.text();
        let ls = line_start(text, at);
        let before = &text[ls..at];
        // never where dashes are block syntax (list markers, rules, front matter)
        if before
            .trim_start_matches(['>', ' ', '\t'])
            .trim_start_matches(['-', '\u{2013}'])
            .is_empty()
        {
            return None;
        }
        let prev = before.chars().next_back()?;
        let (from, with) = match (prev, prefs.smart_dashes) {
            ('-', SmartDashes::EnEm) => (at - 1, "\u{2013}"),
            ('-', SmartDashes::Em) => (at - 1, "\u{2014}"),
            ('\u{2013}', SmartDashes::EnEm) => (at - '\u{2013}'.len_utf8(), "\u{2014}"),
            _ => return None,
        };
        if before[..before.len() - prev.len_utf8()].ends_with('-') {
            return None;
        }
        // after the typing step the text is `…prev-`; replace prev + the typed `-`
        let ps = PatchSet::single_verbatim(from..at + 1, with);
        Some(RulePlan {
            label: "Smart Dash",
            plan: Plan::single(PlanStep::patches(ps), Some(Selection::caret(from + with.len()))),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodeCtx {
    None,
    Inline,
    Math,
    Block,
}

impl CodeCtx {
    fn is_block(self) -> bool {
        self == CodeCtx::Block
    }
    fn any(self) -> bool {
        self != CodeCtx::None
    }
}

enum PairPlan {
    StepOver(usize),
    Wrap(String, String),
    /// Text to insert and the caret offset inside it.
    Insert(String, usize),
}
