//! `markise-testfmt` — a tiny line-based toy format proving the `markise-doc` seam and layering
//! (ADR 0006 §1.2). Test/dev only (`testkit` role in `layers.toml`).
//!
//! Syntax, one block per physical line, blank lines separate:
//! - `= text` heading; `* text` list item; anything else is a paragraph line (consecutive
//!   paragraph lines form one paragraph);
//! - inline `*strong*` inside paragraphs and headings;
//! - a literal-line hint makes its line a paragraph line whatever it starts with.
//!
//! Editing behaviour: immediate input rule `* ` → item (typing step adds a `RuleTrigger` hint,
//! the `Auto` conversion removes it); Enter rule `= x` (typing `= ` adds an `EnterPending` hint,
//! Enter at the line end removes it); `block.heading1` toggles `= `. The serializer is the text
//! itself (lossless by construction).

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use markise_doc::{
    CommandDescriptor, DefaultKey, DepChange, DocumentFormat, EditBehavior, EditSummary, Effect, FlatContainer,
    FlatEvent, FlatLeaf, FormatDoc, FormatOptions, HeadingInfo, HintOrigin, HintSet, InlineKind, InlineNode,
    InlineSource, LeafKind, LeafView, LineEnding, ListMarker, LiteralHint, ParamSchema, PatchSet, Plan, PlanError,
    PlanRequest, PlanStep, PlannerId, RichInline, RulePlan, Scope, Selection, Span, TableView, TypingPlan, UndoGroup,
    plain_typing, planners, text::is_line_start,
};

/// The toy file type.
#[derive(Clone, Copy, Debug, Default)]
pub struct TestFormat;

pub static TESTFMT_COMMANDS: [CommandDescriptor; 1] = [CommandDescriptor {
    id: "block.heading1",
    label: "Heading 1",
    menu: None,
    keys: &[DefaultKey {
        mac: Some("Cmd+1"),
        other: Some("Ctrl+1"),
    }],
    scope: Scope::Editor,
    effect: Effect::Mutates {
        group: UndoGroup::Structural,
    },
    params: ParamSchema::NONE,
    planner: Some(planners::HEADING_1),
}];

impl DocumentFormat for TestFormat {
    fn id(&self) -> &'static str {
        "testfmt"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["tfmt"]
    }
    fn parse(&self, text: String, opts: &FormatOptions) -> Box<dyn FormatDoc> {
        Box::new(TestDoc::parse(text, opts.default_eol))
    }
    fn commands(&self) -> &'static [CommandDescriptor] {
        &TESTFMT_COMMANDS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Heading,
    Item,
    Para,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    kind: Kind,
    span: Span,
    /// content span per line (marker excluded)
    lines: Vec<Span>,
}

/// A parsed toy document.
#[derive(Clone, Debug)]
pub struct TestDoc {
    text: String,
    eol: LineEnding,
    hints: HintSet,
    blocks: Vec<Block>,
}

/// `(start, content_end, next)` of every physical line (CR, LF, CRLF).
fn lines(text: &str) -> Vec<(usize, usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut s = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                out.push((s, i, i + 1));
                i += 1;
                s = i;
            }
            b'\r' => {
                let n = if b.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                out.push((s, i, i + n));
                i += n;
                s = i;
            }
            _ => i += 1,
        }
    }
    if s < b.len() {
        out.push((s, b.len(), b.len()));
    }
    out
}

fn line_start_of(text: &str, pos: usize) -> usize {
    text[..pos].rfind(['\n', '\r']).map_or(0, |i| i + 1)
}

fn line_end_of(text: &str, pos: usize) -> usize {
    text[pos..].find(['\n', '\r']).map_or(text.len(), |i| pos + i)
}

impl TestDoc {
    pub fn parse(text: impl Into<String>, default_eol: LineEnding) -> Self {
        let text = text.into();
        let eol = LineEnding::detect(&text).unwrap_or(default_eol);
        let mut d = TestDoc {
            text,
            eol,
            hints: HintSet::new(),
            blocks: Vec::new(),
        };
        d.reparse();
        d
    }

    /// Full re-parse (the toy has no incremental parser).
    fn reparse(&mut self) {
        let mut blocks: Vec<Block> = Vec::new();
        let mut prev_para = false;
        for (s, e, _) in lines(&self.text) {
            let l = &self.text[s..e];
            if l.trim().is_empty() {
                prev_para = false;
                continue;
            }
            let hinted = self.hints.contains_line(s);
            let (kind, content) = if !hinted && l.starts_with("= ") {
                (Kind::Heading, Span::new(s + 2, e))
            } else if !hinted && l.starts_with("* ") {
                (Kind::Item, Span::new(s + 2, e))
            } else {
                (Kind::Para, Span::new(s, e))
            };
            if kind == Kind::Para && prev_para {
                let b = blocks.last_mut().unwrap();
                b.span.end = e;
                b.lines.push(content);
            } else {
                blocks.push(Block {
                    kind,
                    span: Span::new(s, e),
                    lines: vec![content],
                });
            }
            prev_para = kind == Kind::Para;
        }
        self.blocks = blocks;
    }

    fn block_at(&self, at: usize) -> Option<&Block> {
        self.blocks.iter().find(|b| b.span.start <= at && at <= b.span.end)
    }

    /// `true` when the line at `ls` starts (or would start) a paragraph: no existing heading or
    /// item there, and the previous physical line (whole terminator skipped) is blank.
    fn starts_block(&self, ls: usize) -> bool {
        if self
            .block_at(ls)
            .is_some_and(|b| b.kind != Kind::Para || b.span.start != ls)
        {
            return false;
        }
        if ls == 0 {
            return true;
        }
        let term_start = if self.text[..ls].ends_with("\r\n") {
            ls - 2
        } else {
            ls - 1
        };
        let prev = line_start_of(&self.text, term_start);
        self.text[prev..term_start].trim().is_empty()
    }
}

impl FormatDoc for TestDoc {
    fn format_id(&self) -> &'static str {
        "testfmt"
    }
    fn text(&self) -> &str {
        &self.text
    }
    fn line_ending(&self) -> LineEnding {
        self.eol
    }
    fn apply(&mut self, patches: &PatchSet) -> EditSummary {
        if let Err(e) = patches.validate(&self.text) {
            panic!("invalid patch set: {e}");
        }
        let changed = patches.covering_range().unwrap_or(0..0);
        let delta = patches.delta();
        let old_top = self.blocks.len();
        patches.apply(&mut self.text);
        let t = &self.text;
        self.hints = self.hints.transform(patches, |o| is_line_start(t, o));
        self.reparse();
        // the toy re-parses everything: every block is "replaced"
        EditSummary {
            new_len: (changed.len() as isize + delta) as usize,
            changed,
            reparsed: 0..self.text.len(),
            delta,
            replaced_top: 0..old_top,
            new_top: 0..self.blocks.len(),
            deps: DepChange::default(),
            inline_leaf: None,
            nested_from: None,
        }
    }
    fn hints(&self) -> &HintSet {
        &self.hints
    }
    fn set_hints(&mut self, hints: &HintSet) -> EditSummary {
        let t = &self.text;
        let old_top = self.blocks.len();
        self.hints = HintSet::from_hints(hints.iter().copied().filter(|h| is_line_start(t, h.line)));
        self.reparse();
        EditSummary {
            reparsed: 0..self.text.len(),
            replaced_top: 0..old_top,
            new_top: 0..self.blocks.len(),
            ..Default::default()
        }
    }
    fn leaf(&self, at: usize) -> Option<LeafView> {
        let b = self.block_at(at)?;
        Some(LeafView {
            kind: match b.kind {
                Kind::Heading => LeafKind::Heading { level: 1 },
                Kind::Item | Kind::Para => LeafKind::Paragraph,
            },
            span: b.span,
            lines: b.lines.clone(),
            containers: if b.kind == Kind::Item {
                vec![
                    markise_doc::ContainerKind::List { ordered: false },
                    markise_doc::ContainerKind::ListItem {
                        ordered: false,
                        task: None,
                    },
                ]
            } else {
                Vec::new()
            },
            hinted: self.hints.contains_line(b.span.start),
        })
    }
    fn inline(&self, src: InlineSource<'_>, base: usize) -> RichInline {
        // `*strong*` on single lines; the lines of a paragraph are joined by soft breaks
        let lines: Vec<Span> = match src {
            InlineSource::Lines(l) => l.to_vec(),
            InlineSource::Cell { span, .. } => vec![span],
        };
        let mut rich = RichInline::default();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                let at = (lines[i - 1].end - base) as u32;
                rich.nodes.push(InlineNode::leaf(
                    InlineKind::Break { hard: false },
                    at..(l.start - base) as u32,
                    0..0,
                ));
            }
            let one = self.inline_line(*l, base, &mut rich.text);
            rich.nodes.extend(one);
        }
        rich
    }
    fn top_count(&self) -> usize {
        self.blocks.len()
    }
    fn flatten(&self, tops: Range<usize>, sink: &mut dyn FnMut(FlatEvent<'_>)) {
        for (i, b) in self.blocks[tops.clone()].iter().enumerate() {
            let leaf = FlatLeaf {
                kind: match b.kind {
                    Kind::Heading => LeafKind::Heading { level: 1 },
                    Kind::Item | Kind::Para => LeafKind::Paragraph,
                },
                origin: 0,
                span: b.span,
                lines: &b.lines,
                top: tops.start + i,
            };
            if b.kind == Kind::Item {
                sink(FlatEvent::Open(FlatContainer::List {
                    ordered: false,
                    tight: true,
                }));
                sink(FlatEvent::Open(FlatContainer::ListItem {
                    marker: ListMarker::Bullet,
                }));
                sink(FlatEvent::Leaf(leaf));
                sink(FlatEvent::Close);
                sink(FlatEvent::Close);
            } else {
                sink(FlatEvent::Leaf(leaf));
            }
        }
    }
    fn table(&self, _at: usize) -> Option<TableView> {
        None
    }
    fn table_row_cells(&self, _row: Span) -> Vec<Span> {
        Vec::new()
    }
    fn heading(&self, at: usize) -> Option<HeadingInfo> {
        let b = self.block_at(at).filter(|b| b.kind == Kind::Heading)?;
        let text = b.lines[0].slice(&self.text).replace('*', "");
        let key = text.trim().to_lowercase().replace(' ', "-");
        Some(HeadingInfo { text, key })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn debug_tree(&self) -> String {
        let mut out = String::new();
        for b in &self.blocks {
            let name = match b.kind {
                Kind::Heading => "h1",
                Kind::Item => "item",
                Kind::Para => "para",
            };
            let content: Vec<&str> = b.lines.iter().map(|l| l.slice(&self.text)).collect();
            out.push_str(&format!("{name} {:?}\n", content.join("\n")));
        }
        out
    }
    fn snapshot(&self) -> Arc<str> {
        Arc::from(self.text.as_str())
    }
    fn clone_box(&self) -> Box<dyn FormatDoc> {
        Box::new(self.clone())
    }
}

impl TestDoc {
    /// Inline nodes of one content line (`*strong*`), offsets relative to `base`.
    fn inline_line(&self, line: Span, base: usize, pool: &mut String) -> Vec<InlineNode> {
        let s = line.slice(&self.text);
        let off = (line.start - base) as u32;
        let mut nodes = Vec::new();
        let mut pos = 0;
        let mut text_from = 0;
        let value = |pool: &mut String, a: usize, b: usize| {
            let start = pool.len() as u32;
            pool.push_str(&s[a..b]);
            start..pool.len() as u32
        };
        let push_text = |nodes: &mut Vec<InlineNode>, pool: &mut String, a: usize, b: usize| {
            if b > a {
                let v = value(pool, a, b);
                nodes.push(InlineNode::leaf(InlineKind::Text, off + a as u32..off + b as u32, v));
            }
        };
        while let Some(o) = s[pos..].find('*') {
            let open = pos + o;
            let Some(c) = s[open + 1..].find('*') else { break };
            let close = open + 1 + c;
            if close == open + 1 {
                pos = close + 1;
                continue;
            }
            push_text(&mut nodes, pool, text_from, open);
            let (a, z) = (off + open as u32, off + close as u32 + 1);
            let mut inner = Vec::new();
            push_text(&mut inner, pool, open + 1, close);
            nodes.push(InlineNode {
                kind: InlineKind::Strong,
                source: a..z,
                content: a + 1..z - 1,
                markers: vec![a..a + 1, z - 1..z],
                children: inner,
                text: 0..0,
            });
            pos = close + 1;
            text_from = pos;
        }
        push_text(&mut nodes, pool, text_from, s.len());
        nodes
    }
}

impl EditBehavior for TestDoc {
    fn plan(&self, id: PlannerId, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        let at = sel.focus.offset;
        let ls = line_start_of(&self.text, at);
        let le = line_end_of(&self.text, at);
        if id == planners::NEWLINE {
            let pending = self.hints.get(ls).is_some_and(|h| h.origin == HintOrigin::EnterPending);
            if !sel.is_caret() || !pending || at != le {
                return Err(PlanError::NotApplicable("no Enter rule here"));
            }
            let mut hints = self.hints.clone();
            hints.remove_line(ls);
            let eol = self.eol.as_str();
            return Ok(Plan::single(
                PlanStep {
                    patches: PatchSet::single_verbatim(at..at, eol),
                    hints_after: Some(hints),
                },
                Some(Selection::caret(at + eol.len())),
            ));
        }
        if id == planners::HEADING_1 {
            if self.text[ls..le].trim().is_empty() {
                return Err(PlanError::NotApplicable("empty line"));
            }
            let ps = if self.text[ls..].starts_with("= ") {
                PatchSet::single_verbatim(ls..ls + 2, "")
            } else {
                PatchSet::single_verbatim(ls..ls, "= ")
            };
            return Ok(Plan::single(PlanStep::patches(ps), None));
        }
        Err(PlanError::Unsupported)
    }

    fn on_text(&self, req: &PlanRequest, typed: &str) -> TypingPlan {
        let mut tp = plain_typing(req, typed);
        let r = req.selection.primary().range();
        let ls = line_start_of(&self.text, r.start);
        if typed != " " || line_start_of(&self.text, r.end) != ls || !self.starts_block(ls) {
            return tp;
        }
        let line_after = format!(
            "{}{typed}{}",
            &self.text[ls..r.start],
            &self.text[r.end..line_end_of(&self.text, r.end)]
        );
        let mut hints = self.hints.clone();
        if line_after == "* " {
            hints.insert(LiteralHint::new(ls, HintOrigin::RuleTrigger));
            let mut converted = hints.clone();
            converted.remove_line(ls);
            tp.step.hints_after = Some(hints);
            tp.rule = Some(RulePlan {
                label: "Make Item",
                plan: Plan::single(PlanStep::hints(converted), None),
            });
        } else if line_after.starts_with("= ") && !self.hints.contains_line(ls) {
            hints.insert(LiteralHint::new(ls, HintOrigin::EnterPending));
            tp.step.hints_after = Some(hints);
        }
        tp
    }

    fn materialize_hint(&self, hint: &LiteralHint) -> Option<PatchSet> {
        let l = &self.text[hint.line..line_end_of(&self.text, hint.line)];
        (l.starts_with("= ") || l.starts_with("* ")).then(|| PatchSet::single_verbatim(hint.line..hint.line, "\\"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_hints() {
        let mut d = TestDoc::parse("= T\n\n* a\npara *b*\n", LineEnding::Lf);
        assert_eq!(d.debug_tree(), "h1 \"T\"\nitem \"a\"\npara \"para *b*\"\n");
        d.set_hints(&HintSet::from_hints([LiteralHint::new(5, HintOrigin::RuleTrigger)]));
        assert_eq!(d.debug_tree(), "h1 \"T\"\npara \"* a\\npara *b*\"\n");
        let leaf = d.leaf(9).unwrap();
        assert_eq!(
            d.inlines(&leaf).debug(),
            "text[0..3] softbreak[3..4] text[4..9] strong[9..12 m:9..10,11..12](text[10..11])"
        );
        let d = TestDoc::parse("x *b* y", LineEnding::Lf);
        let leaf = d.leaf(0).unwrap();
        assert_eq!(
            d.inlines(&leaf).debug(),
            "text[0..2] strong[2..5 m:2..3,4..5](text[3..4]) text[5..7]"
        );
    }
}
