//! Markdown behind the format seam (ADR 0006 §1.2): [`MarkdownFormat`] implements
//! [`DocumentFormat`], [`Document`] implements [`FormatDoc`] and [`EditBehavior`].
//!
//! Input rules in M2a are a representative subset proving the seam (full set: M2c):
//! - immediate rule `- ` / `* ` / `+ ` → bullet list (spec 02 §2.1): while the marker is being
//!   typed the line carries a `RuleTrigger` hint; the completing space ends the typing step and
//!   the conversion is a separate `Auto` plan removing the hint (ADR 0006 §2.6, `S0→S1→S2`);
//! - Enter-only syntaxes (spec 02 §2.2) — ATX heading, code fence, thematic break: typing adds an
//!   `EnterPending` hint so the line stays a paragraph; Enter at the line end removes the hint
//!   (fence: also writes the closing fence, caret on code line 1, S-2.5).
//!
//! Rules apply only to top-level lines that start a paragraph (previous line blank or start of
//! document); container prefixes (`> - `) are M2c.

use std::sync::Arc;

use std::any::Any;
use std::ops::Range;

use markise_doc::{
    CalloutKind, ColumnAlign, CommandDescriptor, ContainerKind, DefaultKey, DocumentFormat, EditBehavior, EditSummary,
    Effect, FlatContainer, FlatEvent, FlatLeaf, FormatDoc, FormatOptions, HeadingInfo, HintOrigin, HintSet,
    InlineKind as NK, InlineNode, InlineSource, LeafKind, LeafView, LineEnding, ListMarker, LiteralHint, ParamSchema,
    PatchSet, Plan, PlanError, PlanRequest, PlanStep, PlannerId, RichInline, Scope, Selection, TableView, TypingPlan,
    UndoGroup, planners,
};

use crate::block::{AlertKind, Align, Block, BlockKind};
use crate::inline::{Inline, InlineKind, decode_entity, parse_inlines, plain_text};
use crate::options::ParseOptions;
use crate::text::{Span, line_end};
use crate::{Document, EditResult, line_start, ops, scan};

/// The Markdown file type. Holds the Markdown-specific parse options.
#[derive(Clone, Debug, Default)]
pub struct MarkdownFormat {
    pub opts: ParseOptions,
}

const fn editor_cmd(
    id: &'static str,
    label: &'static str,
    keys: &'static [DefaultKey],
    group: UndoGroup,
    planner: PlannerId,
) -> CommandDescriptor {
    CommandDescriptor {
        id,
        label,
        menu: None,
        keys,
        scope: Scope::Editor,
        effect: Effect::Mutates { group },
        params: ParamSchema::NONE,
        planner: Some(planner),
    }
}

const fn key(mac: &'static str, other: &'static str) -> DefaultKey {
    DefaultKey {
        mac: Some(mac),
        other: Some(other),
    }
}

const fn cmd(
    id: &'static str,
    label: &'static str,
    keys: &'static [DefaultKey],
    group: UndoGroup,
) -> CommandDescriptor {
    editor_cmd(id, label, keys, group, PlannerId(id))
}

/// A command whose menu is Edit (delete / move commands of spec 02 §10).
const fn edit_menu_cmd(id: &'static str, label: &'static str, keys: &'static [DefaultKey]) -> CommandDescriptor {
    let mut d = cmd(id, label, keys, S);
    d.menu = Some(&["Edit"]);
    d
}

const fn mac_only(mac: &'static str) -> DefaultKey {
    DefaultKey {
        mac: Some(mac),
        other: None,
    }
}

const S: UndoGroup = UndoGroup::Structural;
const F: UndoGroup = UndoGroup::Format;

const fn view_cmd(id: &'static str, label: &'static str, keys: &'static [DefaultKey]) -> CommandDescriptor {
    CommandDescriptor {
        id,
        label,
        menu: None,
        keys,
        scope: Scope::Editor,
        effect: Effect::View,
        params: ParamSchema::NONE,
        planner: Some(PlannerId(id)),
    }
}

const URL_PARAM: ParamSchema = ParamSchema(&[markise_doc::ParamSpec {
    name: "url",
    kind: markise_doc::ParamKind::Text,
    required: false,
}]);

const fn link_cmd(id: &'static str, label: &'static str, keys: &'static [DefaultKey]) -> CommandDescriptor {
    CommandDescriptor {
        id,
        label,
        menu: None,
        keys,
        scope: Scope::Editor,
        effect: Effect::Mutates {
            group: UndoGroup::Format,
        },
        params: URL_PARAM,
        planner: Some(PlannerId(id)),
    }
}

/// Commands implemented by the Markdown planners (spec 02 §10, §15; ADR 0006 §3.2).
pub static MARKDOWN_COMMANDS: [CommandDescriptor; 39] = [
    cmd("block.toggleTask", "Toggle Task Status", &[mac_only("Ctrl+X")], S),
    cmd("block.paragraph", "Paragraph", &[key("Cmd+0", "Ctrl+0")], S),
    cmd("block.heading1", "Heading 1", &[key("Cmd+1", "Ctrl+1")], S),
    cmd("block.heading2", "Heading 2", &[key("Cmd+2", "Ctrl+2")], S),
    cmd("block.heading3", "Heading 3", &[key("Cmd+3", "Ctrl+3")], S),
    cmd("block.heading4", "Heading 4", &[key("Cmd+4", "Ctrl+4")], S),
    cmd("block.heading5", "Heading 5", &[key("Cmd+5", "Ctrl+5")], S),
    cmd("block.heading6", "Heading 6", &[key("Cmd+6", "Ctrl+6")], S),
    cmd(
        "block.headingUp",
        "Increase Heading Level",
        &[key("Cmd+=", "Ctrl+=")],
        S,
    ),
    cmd(
        "block.headingDown",
        "Decrease Heading Level",
        &[key("Cmd+-", "Ctrl+-")],
        S,
    ),
    cmd("block.quote", "Quote", &[key("Opt+Cmd+Q", "Ctrl+Shift+Q")], S),
    cmd(
        "block.orderedList",
        "Ordered List",
        &[key("Opt+Cmd+O", "Ctrl+Shift+[")],
        S,
    ),
    cmd(
        "block.bulletList",
        "Unordered List",
        &[key("Opt+Cmd+U", "Ctrl+Shift+]")],
        S,
    ),
    cmd("block.taskList", "Task List", &[key("Opt+Cmd+X", "Ctrl+Alt+X")], S),
    cmd("block.indent", "Indent", &[key("Cmd+]", "Ctrl+]")], S),
    cmd("block.outdent", "Outdent", &[key("Cmd+[", "Ctrl+[")], S),
    edit_menu_cmd("edit.moveBlockUp", "Move Row Up", &[key("Alt+Up", "Alt+Up")]),
    edit_menu_cmd("edit.moveBlockDown", "Move Row Down", &[key("Alt+Down", "Alt+Down")]),
    edit_menu_cmd("edit.openLine", "Open Line", &[mac_only("Ctrl+O")]),
    cmd("format.strong", "Strong", &[key("Cmd+B", "Ctrl+B")], F),
    cmd("format.emph", "Emphasis", &[key("Cmd+I", "Ctrl+I")], F),
    cmd("format.underline", "Underline", &[key("Cmd+U", "Ctrl+U")], F),
    cmd("format.code", "Code", &[key("Ctrl+`", "Ctrl+Shift+`")], F),
    cmd("format.strike", "Strike", &[key("Ctrl+Shift+`", "Alt+Shift+5")], F),
    cmd(
        "format.highlight",
        "Highlight",
        &[key("Shift+Cmd+H", "Ctrl+Shift+H")],
        F,
    ),
    cmd("format.inlineMath", "Inline Math", &[mac_only("Ctrl+M")], F),
    cmd("format.comment", "Comment", &[mac_only("Ctrl+-")], F),
    link_cmd("format.link", "Hyperlink", &[key("Cmd+K", "Ctrl+K")]),
    link_cmd("format.image", "Image", &[key("Ctrl+Cmd+I", "Ctrl+Shift+I")]),
    cmd("format.clear", "Clear Format", &[key("Cmd+\\", "Ctrl+\\")], F),
    view_cmd("select.word", "Select Word", &[key("Cmd+D", "Ctrl+D")]),
    view_cmd("select.line", "Select Line / Sentence", &[key("Cmd+L", "Ctrl+L")]),
    view_cmd("select.scope", "Select Styled Scope", &[key("Cmd+E", "Ctrl+E")]),
    view_cmd(
        "select.block",
        "Select Paragraph / Block",
        &[key("Opt+Cmd+P", "Ctrl+Alt+P")],
    ),
    edit_menu_cmd("delete.word", "Delete Word", &[key("Shift+Cmd+D", "Ctrl+Shift+D")]),
    edit_menu_cmd(
        "delete.line",
        "Delete Line / Sentence",
        &[key("Shift+Opt+Cmd+L", "Ctrl+Alt+Shift+L")],
    ),
    edit_menu_cmd(
        "delete.scope",
        "Delete Styled Scope",
        &[key("Shift+Opt+Cmd+E", "Ctrl+Alt+Shift+E")],
    ),
    edit_menu_cmd(
        "delete.block",
        "Delete Paragraph / Block",
        &[key("Shift+Opt+Cmd+P", "Ctrl+Alt+Shift+P")],
    ),
    cmd(
        "block.insertRule",
        "Horizontal Line",
        &[key("Opt+Cmd+-", "Ctrl+Alt+-")],
        S,
    ),
];

impl DocumentFormat for MarkdownFormat {
    fn id(&self) -> &'static str {
        "markdown"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["md", "markdown", "mdown", "mkd", "mkdn"]
    }
    fn parse(&self, text: String, opts: &FormatOptions) -> Box<dyn FormatDoc> {
        Box::new(Document::parse_with_default_eol(
            text,
            self.opts.clone(),
            opts.default_eol,
        ))
    }
    fn commands(&self) -> &'static [CommandDescriptor] {
        &MARKDOWN_COMMANDS
    }
}

fn summary(r: EditResult, changed: std::ops::Range<usize>, delta: isize) -> EditSummary {
    let s = r.new_blocks.start;
    EditSummary {
        new_len: (changed.len() as isize + delta) as usize,
        changed,
        reparsed: r.reparsed,
        delta,
        replaced_top: s..s + r.replaced,
        new_top: r.new_blocks,
        deps: r.deps,
        inline_leaf: r.inline_leaf,
        nested_from: r.nested_from,
    }
}

impl FormatDoc for Document {
    fn format_id(&self) -> &'static str {
        "markdown"
    }
    fn text(&self) -> &str {
        Document::text(self)
    }
    fn line_ending(&self) -> LineEnding {
        Document::line_ending(self)
    }
    fn apply(&mut self, patches: &PatchSet) -> EditSummary {
        let changed = patches.covering_range().unwrap_or(0..0);
        let delta = patches.delta();
        let r = self.apply_patches(patches);
        summary(r, changed, delta)
    }
    fn apply_with_hints(&mut self, patches: &PatchSet, hints: Option<&HintSet>) -> EditSummary {
        let changed = patches.covering_range().unwrap_or(0..0);
        let delta = patches.delta();
        let r = self.apply_patches_with_hints(patches, hints);
        summary(r, changed, delta)
    }
    fn hints(&self) -> &HintSet {
        Document::hints(self)
    }
    fn set_hints(&mut self, hints: &HintSet) -> EditSummary {
        let r = Document::set_hints(self, hints);
        let changed = r.reparsed.clone();
        summary(r, changed, 0)
    }
    fn leaf(&self, at: usize) -> Option<LeafView> {
        let path = self.block_path_at(at);
        let mut containers = Vec::new();
        let mut leaf = None;
        for depth in 1..=path.len() {
            let (b, origin) = self.block_at_path_rel(&path[..depth])?;
            if b.is_container() {
                containers.push(container_kind(b));
            } else {
                let mut b = b.clone();
                b.shift(origin as isize);
                leaf = Some(b);
                break;
            }
        }
        let b = &leaf?;
        let first = b.lines.first().map_or(b.span.start, |l| l.start);
        Some(LeafView {
            kind: leaf_kind(self.text(), b),
            span: b.span,
            lines: b.lines.clone(),
            containers,
            hinted: self.hints().contains_line(line_start(self.text(), first)),
        })
    }
    fn inlines(&self, leaf: &LeafView) -> RichInline {
        if !matches!(leaf.kind, LeafKind::Paragraph | LeafKind::Heading { .. }) || leaf.lines.is_empty() {
            return RichInline::default();
        }
        self.inline(InlineSource::Lines(&leaf.lines), leaf.lines[0].start)
    }
    fn inline(&self, src: InlineSource<'_>, base: usize) -> RichInline {
        let text = self.text();
        let ctx = &self.ref_context().inline;
        match src {
            InlineSource::Lines(lines) => {
                if lines.is_empty() {
                    return RichInline::default();
                }
                let mut content = String::new();
                let mut starts = Vec::with_capacity(lines.len());
                for (i, l) in lines.iter().enumerate() {
                    if i > 0 {
                        content.push('\n');
                    }
                    starts.push(content.len());
                    content.push_str(l.slice(text));
                }
                let nodes = parse_inlines(&content, self.options(), ctx);
                let map = |k: usize| -> u32 {
                    let i = starts.partition_point(|&s| s <= k).saturating_sub(1);
                    let src = lines[i].start + (k - starts[i]).min(lines[i].len());
                    (src - base) as u32
                };
                neutral_tree(&content, &nodes, &map)
            }
            InlineSource::Cell { span, cap } => {
                let raw = span.slice(text);
                let (capped, truncated) = cap_str(raw, cap);
                // GFM: `\|` in a cell is a literal pipe before inline parsing; keep a content →
                // source offset table (one entry per removed backslash)
                let mut content = String::with_capacity(capped.len() + 4);
                let mut removed: Vec<usize> = Vec::new(); // content offsets after which 1 byte was dropped
                let mut rest = capped;
                while let Some(i) = rest.find("\\|") {
                    content.push_str(&rest[..i]);
                    removed.push(content.len());
                    content.push('|');
                    rest = &rest[i + 2..];
                }
                content.push_str(rest);
                let capped_len = content.len();
                if truncated {
                    content.push_str(" …");
                }
                let nodes = parse_inlines(&content, self.options(), ctx);
                let start = span.start - base;
                let map = |k: usize| -> u32 {
                    let k = k.min(capped_len);
                    let dropped = removed.partition_point(|&r| r <= k);
                    (start + k + dropped) as u32
                };
                neutral_tree(&content, &nodes, &map)
            }
        }
    }
    fn top_count(&self) -> usize {
        self.top_len()
    }
    fn flatten(&self, tops: Range<usize>, sink: &mut dyn FnMut(FlatEvent<'_>)) {
        let text = self.text();
        for (k, t) in self.tops_range(tops.clone()).enumerate() {
            flatten_block(text, t.block, t.origin, tops.start + k, sink);
        }
    }
    fn flatten_region(&self, tops: Range<usize>, from: usize, to: usize, sink: &mut dyn FnMut(FlatEvent<'_>)) {
        let text = self.text();
        let Some(t) = self.top(tops.start) else { return };
        let o = t.origin;
        if from <= t.start() || from > t.end() {
            return self.flatten(tops, sink);
        }
        let (rel_from, rel_to) = (from - o, to.saturating_sub(o));
        // the containers down to the one whose child starts at `from`, with the path indices
        let mut chain: Vec<&Block> = vec![t.block];
        let mut idx: Vec<usize> = Vec::new();
        let first = loop {
            let x = *chain.last().unwrap();
            let c = x.children.partition_point(|ch| ch.span.start < rel_from);
            if x.children.get(c).is_some_and(|ch| ch.span.start == rel_from) {
                break c;
            }
            match c.checked_sub(1).and_then(|k| x.children.get(k).map(|ch| (k, ch))) {
                Some((k, ch)) if ch.is_container() => {
                    idx.push(k);
                    chain.push(ch);
                }
                _ => return self.flatten(tops, sink),
            }
        };
        // `Resume` means "decorations spent": the innermost container must have emitted an
        // item before the region (that spent the markers / quote starts of every container on
        // the chain and the list-start gap), and no list may have opened after it without an
        // item (it would leave a pending list-start gap). Otherwise flatten the whole block.
        let x = *chain.last().unwrap();
        if !matches!(scan_tail(&x.children[..first]), Tail::Emits { list_after: false }) {
            return self.flatten(tops, sink);
        }
        for x in &chain {
            sink(FlatEvent::Resume(flat_container(text, x, o)));
        }
        for lvl in (0..chain.len()).rev() {
            let x = chain[lvl];
            let from_idx = if lvl + 1 == chain.len() { first } else { idx[lvl] + 1 };
            for ch in &x.children[from_idx.min(x.children.len())..] {
                if ch.span.start >= rel_to {
                    break;
                }
                flatten_child(text, x, ch, o, tops.start, sink);
            }
            sink(FlatEvent::Close);
        }
        for (k, b) in self.top.range(tops.start + 1..tops.end).enumerate() {
            if b.start() >= to {
                break;
            }
            flatten_block(text, b.block, b.origin, tops.start + 1 + k, sink);
        }
    }
    fn table(&self, at: usize) -> Option<TableView> {
        let (b, origin) = self.leaf_rel(at)?;
        let BlockKind::Table { aligns } = &b.kind else {
            return None;
        };
        Some(TableView {
            aligns: aligns
                .iter()
                .map(|a| match a {
                    Align::None => ColumnAlign::None,
                    Align::Left => ColumnAlign::Left,
                    Align::Center => ColumnAlign::Center,
                    Align::Right => ColumnAlign::Right,
                })
                .collect(),
            rows: b
                .lines
                .iter()
                .map(|l| crate::Span::new(l.start + origin, l.end + origin))
                .collect(),
        })
    }
    fn table_row_cells(&self, row: crate::Span) -> Vec<crate::Span> {
        crate::table::row_cells(self.text(), row)
    }
    fn heading(&self, at: usize) -> Option<HeadingInfo> {
        let (b, origin) = self.leaf_rel(at)?;
        if !matches!(b.kind, BlockKind::Heading { .. }) {
            return None;
        }
        let text = self.text();
        let mut content = String::new();
        for (i, l) in b.lines.iter().enumerate() {
            if i > 0 {
                content.push('\n');
            }
            content.push_str(&text[l.start + origin..l.end + origin]);
        }
        let nodes = parse_inlines(&content, self.options(), &self.ref_context().inline);
        let plain = plain_text(&content, &nodes);
        let key = crate::slug::slug(&plain);
        Some(HeadingInfo { text: plain, key })
    }
    fn anchor_id(&self, key: &str, n_before: usize) -> String {
        // R7.2 numbering, R7.3 `write` reserved (as `slug::SlugTracker`)
        let id = if n_before > 0 {
            format!("{key}-{}", n_before + 1)
        } else {
            key.to_string()
        };
        if id == "write" { "write-1".to_string() } else { id }
    }
    fn source_tokens(&self, range: std::ops::Range<usize>, out: &mut Vec<markise_doc::SourceToken>) {
        crate::source_tokens::source_tokens(self, range, out);
    }

    fn reference_candidates(&self, lines: &[crate::Span], out: &mut Vec<u64>) {
        reference_labels(self.text(), lines, out)
    }
    fn reference_fingerprint(&self, key: u64) -> Option<u64> {
        self.ref_context().fingerprints.get(&key).copied()
    }
    fn reference_fingerprints(&self, keys: &[u64]) -> u64 {
        let fp = &self.ref_context().fingerprints;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for k in keys {
            h = (h ^ k ^ fp.get(k).copied().unwrap_or(0x9e37_79b9)).wrapping_mul(0x0100_0000_01b3);
        }
        h
    }
    fn resource_root(&self) -> Option<String> {
        crate::frontmatter::resource_root(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn debug_tree(&self) -> String {
        crate::debug_tree(self)
    }
    fn snapshot(&self) -> Arc<str> {
        Arc::from(Document::text(self))
    }
    fn clone_box(&self) -> Box<dyn FormatDoc> {
        Box::new(self.clone())
    }
    fn canonicalize(&self, patches: &PatchSet) -> PatchSet {
        self.canonicalize_patches(patches)
    }
    fn is_line_start(&self, offset: usize) -> bool {
        offset <= self.text().len() && line_start(self.text(), offset) == offset
    }
}

fn container_kind(b: &Block) -> ContainerKind {
    match &b.kind {
        BlockKind::BlockQuote { alert: None } => ContainerKind::Quote,
        BlockKind::BlockQuote { alert: Some(k) } => ContainerKind::Callout(callout(*k)),
        BlockKind::List { ordered, .. } => ContainerKind::List { ordered: *ordered },
        BlockKind::ListItem { number, task, .. } => ContainerKind::ListItem {
            ordered: number.is_some(),
            task: task.map(|(c, _)| c),
        },
        _ => ContainerKind::Footnote,
    }
}

fn leaf_kind(text: &str, b: &Block) -> LeafKind {
    match &b.kind {
        BlockKind::Paragraph => LeafKind::Paragraph,
        BlockKind::Heading { level, .. } => LeafKind::Heading { level: *level },
        BlockKind::FencedCode { info, .. } => LeafKind::CodeBlock {
            info: info.slice(text).into(),
        },
        BlockKind::IndentedCode => LeafKind::CodeBlock { info: "".into() },
        BlockKind::MathBlock { .. } => LeafKind::Math,
        BlockKind::HtmlBlock => LeafKind::RawBlock,
        BlockKind::ThematicBreak => LeafKind::ThematicBreak,
        BlockKind::Table { aligns } => LeafKind::Table {
            columns: aligns.len() as u16,
        },
        BlockKind::FrontMatter { .. } => LeafKind::Metadata,
        BlockKind::Toc => LeafKind::Toc,
        _ => LeafKind::Definition,
    }
}

/// `s` capped at `cap` bytes on a char boundary, and whether it was truncated.
fn cap_str(s: &str, cap: usize) -> (&str, bool) {
    if s.len() <= cap {
        return (s, false);
    }
    let mut cut = cap;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    (&s[..cut], true)
}

/// Neutral inline tree of `nodes` parsed from `content`; `map` turns content offsets into
/// base-relative source offsets.
fn neutral_tree(content: &str, nodes: &[Inline], map: &dyn Fn(usize) -> u32) -> RichInline {
    let mut text = String::new();
    let nodes = nodes
        .iter()
        .map(|n| neutral_inline(content, n, map, &mut text))
        .collect();
    RichInline { nodes, text }
}

fn push_value(pool: &mut String, v: &str) -> Range<u32> {
    let a = pool.len() as u32;
    pool.push_str(v);
    a..pool.len() as u32
}

/// Converts one Markdown inline node: kind, source/content/marker ranges and the rendered
/// value of leaf nodes (what the viewer shows; ADR 0006 §1.3 "neutral rendered values").
fn neutral_inline(s: &str, n: &Inline, map: &dyn Fn(usize) -> u32, pool: &mut String) -> InlineNode {
    let (kind, value): (NK, Option<String>) = match &n.kind {
        InlineKind::Text => (NK::Text, Some(n.span.slice(s).to_string())),
        InlineKind::Escape => (NK::Atom, Some(s[n.span.start + 1..n.span.end].to_string())),
        InlineKind::Entity => (NK::Atom, Some(decode_entity(n.span.slice(s)))),
        InlineKind::Emoji { emoji } => (NK::Atom, Some((*emoji).to_string())),
        InlineKind::Code { content } => {
            // CommonMark code-span normalisation: line endings → spaces, one surrounding space
            // stripped when both ends have one and the content is not all spaces
            let c = content.slice(s).replace('\n', " ");
            let c = if c.len() >= 2 && c.starts_with(' ') && c.ends_with(' ') && !c.trim().is_empty() {
                c[1..c.len() - 1].to_string()
            } else {
                c
            };
            (NK::Code, Some(c))
        }
        InlineKind::SoftBreak => (NK::Break { hard: false }, None),
        InlineKind::HardBreak => (NK::Break { hard: true }, None),
        InlineKind::Emph => (NK::Emph, None),
        InlineKind::Strong => (NK::Strong, None),
        InlineKind::Strike => (NK::Strike, None),
        InlineKind::Highlight => (NK::Highlight, None),
        InlineKind::Sub => (NK::Sub, None),
        InlineKind::Sup => (NK::Sup, None),
        InlineKind::Link { dest, title, .. } => (
            NK::Link {
                target: dest.as_str().into(),
                title: title.as_deref().map(Into::into),
            },
            None,
        ),
        InlineKind::Image { dest, title, .. } => (
            NK::Image {
                src: dest.as_str().into(),
                title: title.as_deref().map(Into::into),
            },
            Some(plain_text(s, &n.children)),
        ),
        InlineKind::Autolink { url, email } => {
            let target = if *email && !url.starts_with("mailto:") {
                format!("mailto:{url}")
            } else {
                url.clone()
            };
            // the URL between `<` and `>` is the (synthetic) text child
            let inner = Span::new(n.span.start + 1, n.span.end.saturating_sub(1).max(n.span.start + 1));
            let child = InlineNode::leaf(NK::Text, map(inner.start)..map(inner.end), push_value(pool, url));
            return InlineNode {
                kind: NK::Link {
                    target: target.into(),
                    title: None,
                },
                source: map(n.span.start)..map(n.span.end),
                content: map(inner.start)..map(inner.end),
                markers: vec![map(n.span.start)..map(inner.start), map(inner.end)..map(n.span.end)],
                children: vec![child],
                text: 0..0,
            };
        }
        InlineKind::BareUrl { url } => {
            let src = map(n.span.start)..map(n.span.end);
            let child = InlineNode::leaf(NK::Text, src.clone(), push_value(pool, n.span.slice(s)));
            return InlineNode {
                kind: NK::Link {
                    target: url.as_str().into(),
                    title: None,
                },
                content: src.clone(),
                source: src,
                markers: Vec::new(),
                children: vec![child],
                text: 0..0,
            };
        }
        InlineKind::Html => {
            let raw = n.span.slice(s);
            if matches!(raw.to_ascii_lowercase().as_str(), "<br>" | "<br/>" | "<br />") {
                (NK::Break { hard: true }, None)
            } else {
                (NK::RawInline, Some(raw.to_string()))
            }
        }
        InlineKind::Math { content, delim } => (
            NK::Math {
                display: *delim == crate::inline::MathInline::DoubleDollar,
            },
            Some(content.slice(s).to_string()),
        ),
        InlineKind::FootnoteRef { label } => (
            NK::FootnoteRef {
                label: label.as_str().into(),
            },
            Some(label.clone()),
        ),
    };
    let inner: Span = match &n.kind {
        InlineKind::Code { content } | InlineKind::Math { content, .. } => *content,
        _ => match (n.children.first(), n.children.last()) {
            (Some(a), Some(z)) => Span::new(a.span.start, z.span.end),
            // a link with empty text (`[]()`, spec 02 §3.3 at a caret): empty content after
            // `[`, so its markers can be revealed and the caret can stand between them
            _ if matches!(n.kind, InlineKind::Link { .. }) && n.span.slice(s).starts_with('[') => {
                Span::new(n.span.start + 1, n.span.start + 1)
            }
            _ => n.span,
        },
    };
    let mut markers = Vec::new();
    if inner.start > n.span.start {
        markers.push(map(n.span.start)..map(inner.start));
    }
    if n.span.end > inner.end {
        markers.push(map(inner.end)..map(n.span.end));
    }
    let text = match value {
        Some(v) => push_value(pool, &v),
        None => 0..0,
    };
    InlineNode {
        kind,
        source: map(n.span.start)..map(n.span.end),
        content: map(inner.start)..map(inner.end),
        markers,
        children: n.children.iter().map(|c| neutral_inline(s, c, map, pool)).collect(),
        text,
    }
}

/// Labels (as [`label_key`]s) that the inline parser of the content `lines` (joined with line
/// breaks) may look up: the text of every innermost bracket pair (`[…]`, `【…】`, escapes
/// respected), as a link label and — for `[^x]` — as a footnote label. A superset of the
/// attempted lookups, resolved or not. Streams over the lines; allocates only for a label
/// spanning lines.
fn reference_labels(text: &str, lines: &[Span], out: &mut Vec<u64>) {
    let base = out.len();
    // one pass over the whole leaf (container prefixes included: a superset) first
    let (Some(a), Some(z)) = (lines.first(), lines.last()) else {
        return;
    };
    let whole = &text.as_bytes()[a.start..z.end.max(a.start)];
    let any_wide = memchr::memchr(0xE3, whole).is_some() && has_wide_bracket(whole);
    if !any_wide && memchr::memchr(b'[', whole).is_none() {
        return;
    }
    // the open bracket's text from earlier lines (joined with `\n`)
    let mut pending: Option<String> = None;
    let mut first = true;
    for l in lines {
        let s = l.slice(text);
        if !first && let Some(p) = pending.as_mut() {
            p.push('\n');
        }
        first = false;
        let wide = any_wide && has_wide_bracket(s.as_bytes());
        if pending.is_none() && !wide && memchr::memchr(b'[', s.as_bytes()).is_none() {
            continue;
        }
        let mut open: Option<usize> = None;
        let mut on_close = |o: Option<usize>, i: usize, pending: &mut Option<String>| {
            if let Some(o) = o {
                emit_label(&s[o..i], out);
            } else if let Some(mut p) = pending.take() {
                p.push_str(&s[..i]);
                emit_label(&p, out);
            }
        };
        if wide {
            let mut it = s.char_indices();
            while let Some((i, c)) = it.next() {
                match c {
                    '\\' => {
                        it.next();
                    }
                    '[' | '【' => {
                        pending = None;
                        open = Some(i + c.len_utf8());
                    }
                    ']' | '】' => on_close(open.take(), i, &mut pending),
                    _ => {}
                }
            }
        } else {
            // ASCII brackets only: jump between `[`, `]` and `\`
            let b = s.as_bytes();
            let mut i = 0;
            while let Some(k) = memchr::memchr3(b'[', b']', b'\\', &b[i..]) {
                let at = i + k;
                match b[at] {
                    b'\\' => {
                        i = at + 2;
                        if i > b.len() {
                            break;
                        }
                        continue;
                    }
                    b'[' => {
                        pending = None;
                        open = Some(at + 1);
                    }
                    _ => {
                        // a certain inline link `[text](dest)` looks nothing up
                        if let Some(o) = open
                            && simple_inline_link(b, o - 1, at)
                        {
                            open = None;
                        } else {
                            on_close(open.take(), at, &mut pending);
                        }
                    }
                }
                i = at + 1;
            }
        }
        if let Some(o) = open {
            pending = Some(s[o..].to_string());
        } else if let Some(p) = pending.as_mut() {
            p.push_str(s);
        }
        if pending.as_ref().is_some_and(|p| p.len() > 999 * 4) {
            pending = None;
        }
    }
    if out.len() - base > 1 {
        out[base..].sort_unstable();
        let mut w = base + 1;
        for r in base + 1..out.len() {
            if out[r] != out[w - 1] {
                out[w] = out[r];
                w += 1;
            }
        }
        out.truncate(w);
    }
}

/// `true` when `b` contains `【` or `】` (prebuilt SIMD finders: CJK text has an E3 lead
/// byte every few bytes, so a lead-byte scan would crawl).
fn has_wide_bracket(b: &[u8]) -> bool {
    use std::sync::LazyLock;
    static OPEN: LazyLock<memchr::memmem::Finder<'static>> =
        LazyLock::new(|| memchr::memmem::Finder::new("【".as_bytes()));
    static CLOSE: LazyLock<memchr::memmem::Finder<'static>> =
        LazyLock::new(|| memchr::memmem::Finder::new("】".as_bytes()));
    OPEN.find(b).is_some() || CLOSE.find(b).is_some()
}

/// `true` when the bracket pair `b[open] == '['` … `b[close] == ']'` is certainly an inline
/// link or image the parser forms without a reference lookup: `](` follows, the destination up
/// to the next `)` on the line has no blank, control, `<`, `(`, quote or backslash (a valid
/// CommonMark destination with no title), the pair is not the label of a full reference
/// (`[text][label](…)`: no `]` right before `[`), and it is no footnote reference candidate
/// (`[^x]` is looked up as a footnote before any link syntax is tried).
fn simple_inline_link(b: &[u8], open: usize, close: usize) -> bool {
    if b.get(close + 1) != Some(&b'(') || (open > 0 && b[open - 1] == b']') || b.get(open + 1) == Some(&b'^') {
        return false;
    }
    for &c in &b[close + 2..] {
        match c {
            b')' => return true,
            b' ' | b'\t' | b'<' | b'(' | b'"' | b'\'' | b'\\' | 0..=0x1f | 0x7f => return false,
            _ => {}
        }
    }
    false
}

fn emit_label(lab: &str, out: &mut Vec<u64>) {
    if lab.len() > 999 * 4 {
        return;
    }
    // a table cell is parsed with `\|` replaced by `|` (GFM): its label may be looked up in
    // that form
    if lab.contains("\\|") {
        emit_label(&lab.replace("\\|", "|"), out);
    }
    // blank labels hash to nothing (normalized_key reports them)
    let Some(k) = normalized_key("", lab) else { return };
    out.push(k);
    if let Some(f) = lab.strip_prefix('^')
        && let Some(k) = normalized_key("^", f)
    {
        out.push(k);
    }
}

/// `label_key(prefix + normalize_label(s))` without allocating: FNV-1a over the bytes
/// `normalize_label` would produce; `None` for a blank `s`.
fn normalized_key(prefix: &str, s: &str) -> Option<u64> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut put = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for b in prefix.bytes() {
        put(b);
    }
    let mut ws = false;
    let mut any = false;
    if s.is_ascii() {
        for &b in s.trim().as_bytes() {
            if b.is_ascii_whitespace() || b == 0x0b {
                ws = true;
                continue;
            }
            if ws && any {
                put(b' ');
            }
            ws = false;
            any = true;
            put(b.to_ascii_uppercase());
        }
        return any.then_some(h);
    }
    let mut buf = [0u8; 4];
    for c in s.trim().chars() {
        if c.is_whitespace() {
            ws = true;
            continue;
        }
        if ws && any {
            put(b' ');
        }
        ws = false;
        any = true;
        if c.is_ascii() {
            put(c.to_ascii_uppercase() as u8);
        } else {
            for l in c.to_lowercase() {
                for u in l.to_uppercase() {
                    for b in u.encode_utf8(&mut buf).bytes() {
                        put(b);
                    }
                }
            }
        }
    }
    any.then_some(h)
}

/// What the flatten events of a block end with (see [`flatten_block`]): whether it emits a leaf
/// or an empty-container item at all, and whether a list opens after its last item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tail {
    Emits { list_after: bool },
    Nothing { has_list: bool },
}

fn tail_of(b: &Block) -> Tail {
    match &b.kind {
        BlockKind::BlockQuote { .. } => scan_tail(&b.children),
        // a list opens before its items: it counts only when none of them emits
        BlockKind::List { .. } => match scan_tail(&b.children) {
            Tail::Nothing { .. } => Tail::Nothing { has_list: true },
            t => t,
        },
        // an item / footnote without children emits an empty-container item
        BlockKind::ListItem { .. } | BlockKind::FootnoteDef { .. } if b.children.is_empty() => {
            Tail::Emits { list_after: false }
        }
        BlockKind::ListItem { .. } | BlockKind::FootnoteDef { .. } => scan_tail(&b.children),
        _ => Tail::Emits { list_after: false },
    }
}

/// [`tail_of`] for a sequence of sibling blocks.
fn scan_tail(children: &[Block]) -> Tail {
    let mut list_after = false;
    for c in children.iter().rev() {
        match tail_of(c) {
            Tail::Emits { list_after: l } => {
                return Tail::Emits {
                    list_after: l || list_after,
                };
            }
            Tail::Nothing { has_list } => list_after |= has_list,
        }
    }
    Tail::Nothing { has_list: list_after }
}

/// The neutral container of a container block (list item: its marker).
fn flat_container(text: &str, b: &Block, origin: usize) -> FlatContainer {
    match &b.kind {
        BlockKind::BlockQuote { alert } => FlatContainer::Quote {
            callout: alert.map(callout),
        },
        BlockKind::List { ordered, tight, .. } => FlatContainer::List {
            ordered: *ordered,
            tight: *tight,
        },
        BlockKind::ListItem { number, task, .. } => FlatContainer::ListItem {
            marker: match (task, number) {
                (Some((checked, _)), _) => ListMarker::Task { checked: *checked },
                (None, Some(n)) => ListMarker::Number { n: *n, delim: '.' },
                (None, None) => ListMarker::Bullet,
            },
        },
        BlockKind::FootnoteDef { label } => FlatContainer::Footnote {
            label: text[label.start + origin..label.end + origin].into(),
        },
        _ => FlatContainer::Quote { callout: None },
    }
}

/// Emits one child of container `parent` (a list's items with their item wrappers).
fn flatten_child(
    text: &str,
    parent: &Block,
    ch: &Block,
    origin: usize,
    top: usize,
    sink: &mut dyn FnMut(FlatEvent<'_>),
) {
    if let (BlockKind::List { marker, .. }, BlockKind::ListItem { number, task, .. }) = (&parent.kind, &ch.kind) {
        let m = match (task, number) {
            (Some((checked, _)), _) => ListMarker::Task { checked: *checked },
            (None, Some(n)) => ListMarker::Number {
                n: *n,
                delim: *marker as char,
            },
            (None, None) => ListMarker::Bullet,
        };
        sink(FlatEvent::Open(FlatContainer::ListItem { marker: m }));
        if ch.children.is_empty() {
            sink(FlatEvent::Empty {
                span: empty_item_span(text, ch, origin),
                top,
            });
        } else {
            for c in &ch.children {
                flatten_block(text, c, origin, top, sink);
            }
        }
        sink(FlatEvent::Close);
    } else {
        flatten_block(text, ch, origin, top, sink);
    }
}

/// Emits the flatten events of one block (spans relative to `origin`).
fn flatten_block(text: &str, b: &Block, origin: usize, top: usize, sink: &mut dyn FnMut(FlatEvent<'_>)) {
    let abs = |s: Span| Span::new(s.start + origin, s.end + origin);
    match &b.kind {
        BlockKind::BlockQuote { alert } => {
            sink(FlatEvent::Open(FlatContainer::Quote {
                callout: alert.map(callout),
            }));
            if b.children.is_empty() {
                // an empty quote (`> `): a caret anchor after the marker
                sink(FlatEvent::Empty {
                    span: empty_item_span(text, b, origin),
                    top,
                });
            }
            for c in &b.children {
                flatten_block(text, c, origin, top, sink);
            }
            sink(FlatEvent::Close);
        }
        BlockKind::List {
            ordered, marker, tight, ..
        } => {
            sink(FlatEvent::Open(FlatContainer::List {
                ordered: *ordered,
                tight: *tight,
            }));
            for item in &b.children {
                let BlockKind::ListItem { number, task, .. } = &item.kind else {
                    flatten_block(text, item, origin, top, sink);
                    continue;
                };
                let m = match (task, number) {
                    (Some((checked, _)), _) => ListMarker::Task { checked: *checked },
                    (None, Some(n)) => ListMarker::Number {
                        n: *n,
                        delim: *marker as char,
                    },
                    (None, None) => ListMarker::Bullet,
                };
                sink(FlatEvent::Open(FlatContainer::ListItem { marker: m }));
                if item.children.is_empty() {
                    sink(FlatEvent::Empty {
                        span: empty_item_span(text, item, origin),
                        top,
                    });
                } else {
                    for c in &item.children {
                        flatten_block(text, c, origin, top, sink);
                    }
                }
                sink(FlatEvent::Close);
            }
            sink(FlatEvent::Close);
        }
        BlockKind::ListItem { .. } => {
            for c in &b.children {
                flatten_block(text, c, origin, top, sink);
            }
        }
        BlockKind::FootnoteDef { label } => {
            sink(FlatEvent::Open(FlatContainer::Footnote {
                label: abs(*label).slice(text).into(),
            }));
            if b.children.is_empty() {
                sink(FlatEvent::Empty { span: abs(b.span), top });
            } else {
                for c in &b.children {
                    flatten_block(text, c, origin, top, sink);
                }
            }
            sink(FlatEvent::Close);
        }
        _ => sink(FlatEvent::Leaf(FlatLeaf {
            kind: leaf_kind_at(text, b, origin),
            origin,
            span: b.span,
            lines: &b.lines,
            top,
        })),
    }
}

/// The span of an empty list item for the layout: its syntax up to the end of the marker line,
/// so the caret anchor (the span end) sits after the marker's spacing, where typed text goes.
fn empty_item_span(text: &str, item: &Block, origin: usize) -> Span {
    let s = item.span.start + origin;
    let e = item.span.end + origin;
    let le = line_end(text, s).0;
    Span::new(s, le.max(e))
}

fn callout(k: AlertKind) -> CalloutKind {
    match k {
        AlertKind::Note => CalloutKind::Note,
        AlertKind::Tip => CalloutKind::Tip,
        AlertKind::Important => CalloutKind::Important,
        AlertKind::Warning => CalloutKind::Warning,
        AlertKind::Caution => CalloutKind::Caution,
    }
}

/// [`leaf_kind`] of a block with spans relative to `origin`.
fn leaf_kind_at(text: &str, b: &Block, origin: usize) -> LeafKind {
    match &b.kind {
        BlockKind::FencedCode { info, .. } => LeafKind::CodeBlock {
            info: text[info.start + origin..info.end + origin].into(),
        },
        BlockKind::LinkRefDef { .. } => LeafKind::Definition,
        _ => leaf_kind("", b),
    }
}

impl Document {
    /// Top-level blocks `r`.
    fn tops_range(&self, r: Range<usize>) -> impl Iterator<Item = crate::Top<'_>> + '_ {
        self.top.range(r)
    }

    /// The leaf block starting at (or containing) absolute offset `at`, relative to its origin.
    fn leaf_rel(&self, at: usize) -> Option<(&Block, usize)> {
        let path = self.block_path_at(at);
        for depth in 1..=path.len() {
            let (b, origin) = self.block_at_path_rel(&path[..depth])?;
            if !b.is_container() {
                return Some((b, origin));
            }
        }
        None
    }
}

use crate::edit::rules::opener_of;

impl Document {
    fn plan_newline(&self, req: &PlanRequest) -> Result<Plan, PlanError> {
        let sel = req.selection.primary();
        if !sel.is_caret() {
            return Err(PlanError::NotApplicable("selection"));
        }
        let at = sel.focus.offset;
        if sel.focus.virt > 0 {
            return self.virtual_enter(at, sel.focus.virt);
        }
        let text = self.text();
        let ls = line_start(text, at);
        let (ce, _) = line_end(text, ls);
        let Some(hint) = self.hints().get(ls).copied() else {
            return self.plan_split(at);
        };
        if hint.origin != HintOrigin::EnterPending || at != ce {
            return self.plan_split(at);
        }
        // a hinted soft-broken line (C9: no rules there) splits like any paragraph line
        if self
            .leaf(at)
            .and_then(|l| l.lines.first().copied())
            .is_some_and(|f| line_start(text, f.start) != ls)
        {
            return self.plan_split(at);
        }
        let from = hint.from().min(ce);
        let line = &text[from..ce];
        let mut hints = self.hints().clone();
        hints.remove_line(ls);
        let eol = self.line_ending().as_str();
        let prefix = crate::edit::blocks::cont_prefix(text, ls, from);
        let b = line.as_bytes();
        let p = scan::skip_ws(b, 0, b.len());
        if let Some((ch, len, _)) = scan::fence_open(b, p, b.len()) {
            // S-2.5: code block, caret on its first (empty) line
            let blank = prefix.trim_end();
            let insert = format!("{eol}{blank}{eol}{prefix}{}", (ch as char).to_string().repeat(len));
            return Ok(Plan::single(
                PlanStep {
                    patches: PatchSet::single_verbatim(at..at, insert),
                    hints_after: Some(hints),
                },
                Some(Selection::caret(at + eol.len() + blank.len())),
            ));
        }
        if ls == 0 && from == 0 && scan::fm_open(line, self.options().remap_unicode_punctuation) {
            // S-2.4: the delimiter typed on the document's first line opens front matter —
            // its closing delimiter is written, the caret on the empty line between them
            let insert = format!("{eol}{eol}{line}");
            return Ok(Plan::single(
                PlanStep {
                    patches: PatchSet::single_verbatim(ce..ce, insert),
                    hints_after: Some(hints),
                },
                Some(Selection::caret(ce + eol.len())),
            ));
        }
        if let Some((_, content, _)) = scan::atx_heading(b, p, b.len(), self.options().strict)
            && content.is_empty()
        {
            // C43: an empty heading becomes an empty paragraph (its `##` removed)
            if let Some(prev) = self.leaf_before(ls)
                && line_start(text, prev.span.end) < ls
            {
                let ps = PatchSet::single_verbatim(prev.span.end..ce, "");
                return Ok(Plan::single(
                    PlanStep {
                        patches: ps,
                        hints_after: Some(hints),
                    },
                    Some(Selection::caret_at(markise_doc::DocPos::with_virt(prev.span.end, 1))),
                ));
            }
            return Ok(Plan::single(
                PlanStep {
                    patches: PatchSet::single_verbatim(from..ce, ""),
                    hints_after: Some(hints),
                },
                Some(Selection::caret(from)),
            ));
        }
        // heading, rule, `[toc]`, definitions, HTML: the block is created by dropping the hint;
        // the caret goes to the (virtual) paragraph after it — leaving a footnote definition
        let footnote = scan::footnote_def_start(b, p, b.len()).is_some();
        let html = self.options().block_html && scan::html_block_start(b, p, b.len()).is_some();
        if html {
            return Ok(Plan::single(PlanStep::hints(hints), Some(Selection::caret(at))));
        }
        let virt = if footnote { 2 } else { 1 };
        // a link reference definition is not shown: the virtual paragraph goes after the
        // visible block before it (definitions may stand anywhere)
        let def = !footnote && crate::edit::rules::link_def_like(&line[p..]);
        let gap = match self.leaf_before(ls) {
            Some(prev) if def => prev.span.end,
            _ => ce,
        };
        Ok(Plan::single(
            PlanStep::hints(hints),
            Some(Selection::caret_at(markise_doc::DocPos::with_virt(gap, virt))),
        ))
    }

    /// The container prefix to repeat on a new line of the leaf line holding `at`: the
    /// prefix before the line's content with list markers turned into spaces (quote `>` kept).
    fn line_prefix(&self, leaf: &LeafView, at: usize) -> String {
        let text = self.text();
        let line = leaf
            .lines
            .iter()
            .rev()
            .find(|l| l.start <= at)
            .copied()
            .or_else(|| leaf.lines.first().copied());
        let Some(line) = line else { return String::new() };
        let ls = line_start(text, line.start);
        // on the block's first line only the container prefix counts: block markers of the
        // leaf itself (`### `) are not repeated (they would become indentation, e.g. code)
        let end = if line_start(text, leaf.span.start) == ls {
            leaf.span.start.clamp(ls, line.start)
        } else {
            line.start
        };
        text[ls..end]
            .chars()
            .map(|c| match c {
                '>' | '\t' => c,
                _ => ' ',
            })
            .collect()
    }

    /// Enter without an input rule (ADR 0006 §5.2, the M2b subset of spec 02 §5.1):
    /// paragraph split with style preservation (§3.4, S-3.1), the editor-only empty paragraph
    /// at the end of a paragraph or heading (§2.2, Q1), heading split (C7), a new list item
    /// from an item's first paragraph, a plain line break in code/math/HTML/front matter, and
    /// nothing in tables (M2d) and blocks without text.
    fn plan_split(&self, at: usize) -> Result<Plan, PlanError> {
        if let Some(r) = self.list_enter(at) {
            return r;
        }
        let text = self.text();
        let eol = self.line_ending().as_str();
        let nothing = Ok(Plan::default());
        let Some(leaf) = self.leaf(at) else { return nothing };
        let (Some(first), Some(last)) = (leaf.lines.first().copied(), leaf.lines.last().copied()) else {
            return nothing;
        };
        match leaf.kind {
            LeafKind::Paragraph | LeafKind::Heading { .. } => {}
            LeafKind::CodeBlock { .. } | LeafKind::Math | LeafKind::RawBlock | LeafKind::Metadata => {
                if at < first.start || at > last.end {
                    return nothing;
                }
                let ins = format!("{eol}{}", self.line_prefix(&leaf, at));
                let caret = at + ins.len();
                return Ok(Plan::single(
                    PlanStep::patches(PatchSet::single_verbatim(at..at, ins)),
                    Some(Selection::caret(caret)),
                ));
            }
            _ => return nothing,
        }
        // a caret inside the block's own syntax (ATX marker / closing sequence, setext
        // underline) counts as the start / end of its text
        if at < leaf.span.start || at > leaf.span.end.max(last.end) {
            return nothing;
        }
        let at = at.clamp(first.start, last.end);
        // whitespace around the split point is dropped (S-3.1: "very" | "bold")
        let line = leaf
            .lines
            .iter()
            .rev()
            .find(|l| l.start <= at)
            .copied()
            .unwrap_or(first);
        let b = text.as_bytes();
        let mut a = at;
        while a > line.start && matches!(b[a - 1], b' ' | b'\t') {
            a -= 1;
        }
        let mut z = at;
        while z < line.end && matches!(b[z], b' ' | b'\t') {
            z += 1;
        }
        // splitting at a soft line break replaces it (no blank line of its own)
        if z == line.end
            && let Some(next) = leaf.lines.iter().find(|l| l.start > line.end)
        {
            z = next.start;
            while z < next.end && matches!(b[z], b' ' | b'\t') {
                z += 1;
            }
        } else if a == line.start
            && let Some(prev) = leaf.lines.iter().rev().find(|l| l.end < line.start)
        {
            a = prev.end;
            while a > prev.start && matches!(b[a - 1], b' ' | b'\t') {
                a -= 1;
            }
        }
        let rich = self.inline(InlineSource::Lines(&leaf.lines), 0);
        // inline spans the split crosses, outermost first
        let mut spans: Vec<&InlineNode> = Vec::new();
        fn crossing<'r>(nodes: &'r [InlineNode], a: u32, z: u32, out: &mut Vec<&'r InlineNode>) {
            for n in nodes {
                if !n.markers.is_empty() && n.content.start < a && z < n.content.end {
                    out.push(n);
                    crossing(&n.children, a, z, out);
                }
            }
        }
        crossing(&rich.nodes, a as u32, z as u32, &mut spans);
        let marker_text = |n: &InlineNode, open: bool| -> String {
            n.markers
                .iter()
                .filter(|m| {
                    if open {
                        m.end <= n.content.start
                    } else {
                        m.start >= n.content.end
                    }
                })
                .map(|m| &text[m.start as usize..m.end as usize])
                .collect()
        };
        // only closing markers (or nothing) after the caret: the end of the block
        let closers_after: usize = spans.iter().map(|n| marker_text(n, false).len()).sum();
        let rest_trimmed = text[z..last.end].trim_end_matches([' ', '\t']);
        let at_end = z >= last.end
            || (rest_trimmed.len() <= closers_after && {
                let mut covered = vec![false; last.end - z];
                for n in &spans {
                    for m in &n.markers {
                        if m.start as usize >= z && (m.end as usize) <= last.end {
                            for c in &mut covered[m.start as usize - z..m.end as usize - z] {
                                *c = true;
                            }
                        }
                    }
                }
                covered.iter().take(rest_trimmed.len()).all(|&c| c)
            });
        let at_start = a <= first.start;
        // (a heading inside an item splits as a heading, C7 — not into a sibling item)
        let list_item_first = leaf.kind == LeafKind::Paragraph
            && matches!(leaf.containers.last(), Some(ContainerKind::ListItem { .. }))
            && line_start(text, first.start) != first.start
            && {
                let ls = line_start(text, first.start);
                scan::list_marker(
                    b,
                    ls + text[ls..first.start].len() - text[ls..first.start].trim_start_matches(['>', ' ', '\t']).len(),
                    first.start,
                )
                .is_some()
            };
        if list_item_first {
            // a new sibling item: same indentation and bullet, `n + 1` for ordered items
            let ls = line_start(text, first.start);
            let prefix = &text[ls..first.start];
            let new_prefix = next_item_prefix(prefix);
            let close: String = spans.iter().rev().map(|n| marker_text(n, false)).collect();
            let open: String = spans.iter().map(|n| marker_text(n, true)).collect();
            let (close, open) = if at_end {
                (String::new(), String::new())
            } else {
                (close, open)
            };
            let z = if at_end { last.end } else { z };
            let a = if at_end { at.min(last.end) } else { a };
            if let Some(r) = self.item_split_plan(first.start, a, z, &close, &open) {
                return r;
            }
            let ins = format!("{close}{eol}{new_prefix}{open}");
            let caret = a + close.len() + eol.len() + new_prefix.len() + open.len();
            return Ok(Plan::single(
                PlanStep::patches(PatchSet::single_verbatim(a..z, ins)),
                Some(Selection::caret(caret)),
            ));
        }
        if at_end {
            // the editor-only empty paragraph after the block (no source change)
            return Ok(Plan::single(
                PlanStep::patches(PatchSet::default()),
                Some(Selection::caret_at(markise_doc::DocPos::with_virt(leaf.span.end, 1))),
            ));
        }
        if at_start {
            // an empty paragraph above: the virtual paragraph after the previous block of the
            // same container, if there is one
            if let Some(prev) = self.leaf_before(leaf.span.start)
                && prev.span.start < leaf.span.start
                && prev.containers == leaf.containers
                && matches!(
                    prev.kind,
                    LeafKind::Paragraph
                        | LeafKind::Heading { .. }
                        | LeafKind::CodeBlock { .. }
                        | LeafKind::ThematicBreak
                )
            {
                return Ok(Plan::single(
                    PlanStep::patches(PatchSet::default()),
                    Some(Selection::caret_at(markise_doc::DocPos::with_virt(prev.span.end, 1))),
                ));
            }
            return nothing;
        }
        let prefix = self.line_prefix(&leaf, at);
        let blank = prefix.trim_end();
        let close: String = spans.iter().rev().map(|n| marker_text(n, false)).collect();
        let open: String = spans.iter().map(|n| marker_text(n, true)).collect();
        if matches!(leaf.kind, LeafKind::Heading { .. })
            && !text[leaf.span.start..].starts_with('#')
            && leaf.span.end > last.end
        {
            // setext heading (C7): the first part keeps the underline, the rest moves into a
            // paragraph after it
            let rest = &text[z..last.end];
            let tail = format!("{eol}{blank}{eol}{prefix}{open}{rest}");
            let removed = last.end - a;
            let caret = leaf.span.end - removed + close.len() + eol.len() * 2 + blank.len() + prefix.len();
            let ps = PatchSet::new(vec![
                markise_doc::Patch::verbatim(a..last.end, close),
                markise_doc::Patch::verbatim(leaf.span.end..leaf.span.end, tail),
            ])
            .map_err(|_| PlanError::NotApplicable("setext split"))?;
            return Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))));
        }
        if matches!(leaf.kind, LeafKind::Heading { .. }) && leaf.span.end > last.end {
            // ATX heading with a closing sequence (`## ab cd ##`): it stays on the heading
            // part; the new paragraph gets none
            let trail = &text[last.end..leaf.span.end];
            let ins = format!("{close}{trail}{eol}{blank}{eol}{prefix}{open}");
            let caret = a + ins.len() - open.len();
            let ps = PatchSet::new(vec![
                markise_doc::Patch::verbatim(a..z, ins),
                markise_doc::Patch::verbatim(last.end..leaf.span.end, ""),
            ])
            .map_err(|_| PlanError::NotApplicable("heading split"))?;
            return Ok(Plan::single(PlanStep::patches(ps), Some(Selection::caret(caret))));
        }
        let ins = format!("{close}{eol}{blank}{eol}{prefix}{open}");
        let caret = a + ins.len() - open.len();
        Ok(Plan::single(
            PlanStep::patches(PatchSet::single_verbatim(a..z, ins)),
            Some(Selection::caret(caret)),
        ))
    }

    /// The leaf ending before offset `at` (skipping blank lines and container prefixes).
    pub(crate) fn leaf_before(&self, at: usize) -> Option<LeafView> {
        let b = self.text().as_bytes();
        let mut q = at.min(b.len());
        while q > 0 && matches!(b[q - 1], b' ' | b'\t' | b'\n' | b'\r' | b'>') {
            q -= 1;
        }
        if q == 0 {
            return None;
        }
        self.leaf(q - 1).filter(|l| l.span.start < at)
    }

    /// The typing step on the virtual empty paragraph (ADR 0006 §2.2): materialises the
    /// paragraph after the block at `gap` with the separator and the container prefix.
    fn typing_virtual(&self, gap: usize, virt: u16, typed: &str) -> TypingPlan {
        let eol = self.line_ending().as_str();
        let text = self.text();
        let levels = virt.saturating_sub(1) as usize;
        let prefix = if levels > 0 {
            // the paragraph goes after the `levels` innermost containers around the leaf
            // (e.g. after a list the user left with Enter on an empty item)
            let probe = if gap > 0 { gap - 1 } else { gap };
            let path = self.block_path_at(probe);
            let containers: Vec<usize> = (1..=path.len())
                .filter(|&d| {
                    self.block_at_path_rel(&path[..d])
                        .is_some_and(|(b, _)| b.is_container())
                })
                .collect();
            let keep = containers.len().saturating_sub(levels);
            match containers.get(keep).and_then(|&d| self.block_at_path_rel(&path[..d])) {
                Some((x, o)) => {
                    let xs = x.span.start + o;
                    crate::edit::blocks::cont_prefix(text, line_start(text, xs), xs)
                }
                None => String::new(),
            }
        } else {
            match self.leaf(gap).or_else(|| self.leaf_before(gap)) {
                Some(l) => self.line_prefix(&l, gap),
                None => String::new(),
            }
        };
        let blank = prefix.trim_end();
        let ins = format!("{eol}{blank}{eol}{prefix}{typed}");
        // a separator of two blank lines after the gap (C45 list split) gives one of them
        // to the new paragraph: `- a⏎⏎⏎- c` → `- a⏎⏎x⏎⏎- c`
        let triple = format!("{eol}{blank}{eol}{blank}{eol}");
        let end = if levels > 0 && text[gap..].starts_with(&triple) {
            gap + eol.len()
        } else {
            gap
        };
        let caret = gap + ins.len();
        TypingPlan {
            step: PlanStep::patches(PatchSet::single_verbatim(gap..end, ins)),
            selection_after: Some(Selection::caret(caret)),
            rule: None,
        }
    }

    fn plan_heading(&self, level: Option<u8>, req: &PlanRequest) -> Result<Plan, PlanError> {
        let at = req.selection.primary().focus.offset;
        let leaf = self.leaf(at).ok_or(PlanError::NotApplicable("no block"))?;
        if leaf.hinted {
            return Err(PlanError::NotApplicable("literal line"));
        }
        let text = self.text();
        let first = *leaf.lines.first().ok_or(PlanError::NotApplicable("empty block"))?;
        let marker = |l: u8| format!("{} ", "#".repeat(l as usize));
        let patches = match (&leaf.kind, level) {
            (LeafKind::Paragraph, Some(l)) => PatchSet::single_verbatim(first.start..first.start, marker(l)),
            (LeafKind::Paragraph, None) => return Err(PlanError::NotApplicable("already a paragraph")),
            (LeafKind::Heading { level: cur }, want) if !text[leaf.span.start..].starts_with('#') => {
                // setext (spec 02 §15.1): H1/H2 keep the underline style; other levels and
                // Paragraph remove the underline (and add `#` markers)
                let last = *leaf.lines.last().ok_or(PlanError::NotApplicable("empty block"))?;
                let ul_start = line_end(text, last.end).1;
                let ul = ul_start..leaf.span.end;
                let ul_text = &text[ul.clone()];
                let n = ul_text.trim().len().max(3);
                let mut ed = crate::edit::Edits::new();
                match want {
                    Some(l) if l == *cur => {
                        ed.delete(last.end..leaf.span.end);
                    }
                    Some(l @ (1 | 2)) => {
                        let ch = if l == 1 { "=" } else { "-" };
                        let lead = ul_text.len() - ul_text.trim_start().len();
                        ed.push(ul.start + lead..ul.end, ch.repeat(n));
                    }
                    Some(l) => {
                        ed.insert(first.start, marker(l));
                        ed.delete(last.end..leaf.span.end);
                    }
                    None => ed.delete(last.end..leaf.span.end),
                }
                if leaf.lines.len() > 1 && matches!(want, Some(3..=6)) {
                    return Err(PlanError::NotApplicable("multi-line setext heading"));
                }
                ed.build().ok_or(PlanError::NotApplicable("overlap"))?
            }
            (LeafKind::Heading { level: cur }, want) => {
                let new = match want {
                    Some(l) if l != *cur => marker(l),
                    _ => String::new(), // same level or Paragraph → paragraph
                };
                PatchSet::single_verbatim(leaf.span.start..first.start.max(leaf.span.start), new)
            }
            _ => return Err(PlanError::NotApplicable("not a paragraph or heading")),
        };
        Ok(Plan::single(PlanStep::patches(patches), None))
    }
}

/// The inline style a `format.*` planner toggles.
fn style_of(id: PlannerId) -> Option<crate::edit::inline_fmt::Style> {
    use crate::edit::inline_fmt::Style;
    Some(match id {
        planners::STRONG => Style::Strong,
        planners::EMPH => Style::Emph,
        planners::UNDERLINE => Style::Underline,
        planners::CODE => Style::Code,
        planners::STRIKE => Style::Strike,
        planners::HIGHLIGHT => Style::Highlight,
        planners::INLINE_MATH => Style::Math,
        planners::COMMENT => Style::Comment,
        _ => return None,
    })
}

impl EditBehavior for Document {
    fn plan_checked(&self, id: PlannerId, req: &PlanRequest) -> bool {
        if let Some(st) = style_of(id) {
            return self.format_active(st, req);
        }
        self.block_checked(id, req)
    }

    fn plan(&self, id: PlannerId, req: &PlanRequest) -> Result<Plan, PlanError> {
        if let Some(st) = style_of(id) {
            return self.plan_format(st, req);
        }
        match id {
            planners::LINK => return self.plan_link(false, req),
            planners::IMAGE => return self.plan_link(true, req),
            planners::CLEAR_FORMAT => return self.plan_clear_format(req),
            _ => {}
        }
        if let Some(r) = self.plan_m2c(id, req) {
            return r;
        }
        if id == planners::NEWLINE {
            return self.plan_newline(req);
        }
        if id == planners::TOGGLE_TASK {
            let ps = ops::plan_toggle_task(self, req.selection.primary().focus.offset)
                .map_err(|_| PlanError::NotApplicable("no task item"))?;
            return Ok(Plan::single(PlanStep::patches(ps), None));
        }
        if id == planners::PARAGRAPH {
            return self.plan_heading(None, req);
        }
        if let Some(l) = planners::heading_level(id) {
            return self.plan_heading(Some(l), req);
        }
        if id == planners::HEADING_UP || id == planners::HEADING_DOWN {
            let at = req.selection.primary().focus.offset;
            let cur = match self.leaf(at).map(|l| l.kind) {
                Some(LeafKind::Heading { level }) => Some(level),
                Some(LeafKind::Paragraph) => None,
                _ => return Err(PlanError::NotApplicable("not a paragraph or heading")),
            };
            // spec 02 §15.1: Increase: paragraph → H6, Hn → Hn−1, H1 stays; Decrease: Hn →
            // Hn+1, H6 → paragraph, paragraph stays
            let target = match (id == planners::HEADING_UP, cur) {
                (true, None) => Some(6),
                (true, Some(1)) => return Err(PlanError::NotApplicable("already H1")),
                (true, Some(l)) => Some(l - 1),
                (false, None) => return Err(PlanError::NotApplicable("already a paragraph")),
                (false, Some(6)) => None,
                (false, Some(l)) => Some(l + 1),
            };
            if target.is_none() {
                return self.plan_heading(None, req);
            }
            return self.plan_heading(target, req);
        }
        Err(PlanError::Unsupported)
    }

    fn on_text(&self, req: &PlanRequest, typed: &str) -> TypingPlan {
        let sel = req.selection.primary();
        if sel.is_caret() && sel.focus.virt > 0 && !req.source_mode {
            let mut tp = self.typing_virtual(sel.focus.offset, sel.focus.virt, typed);
            // the new paragraph's first character may start block syntax (`#`, `-`, `>`):
            // it gets its literal hint like any block start (ADR 0006 §2.6)
            if let (Some(o), Some(p)) = (opener_of(typed, self.options()), tp.step.patches.patches().first()) {
                let ins = &p.insert;
                let line_off = ins.len() - typed.len();
                let ls = p.range.start + ins[..line_off].rfind(['\n', '\r']).map_or(0, |i| i + 1);
                let col = (p.range.start + line_off - ls) as u32;
                let origin = match o {
                    crate::edit::rules::Opener::EnterOnly => HintOrigin::EnterPending,
                    crate::edit::rules::Opener::Immediate => HintOrigin::RuleTrigger,
                };
                let mut hints = self.hints().transform(&tp.step.patches, |_| true);
                hints.insert(LiteralHint::at(ls, col, origin));
                tp.step.hints_after = Some(hints);
            }
            return tp;
        }
        self.md_on_text(req, typed)
    }

    fn materialize_hint(&self, hint: &LiteralHint) -> Option<PatchSet> {
        let text = self.text();
        if hint.line > text.len() || line_start(text, hint.line) != hint.line {
            return None;
        }
        let (ce, _) = line_end(text, hint.line);
        if hint.origin == HintOrigin::ForceBlock {
            // a nested empty item still empty when the caret leaves it cannot be written
            // (it would read as paragraph text): its line is removed
            let line = &text[hint.line..ce];
            let b = line.as_bytes();
            let p = scan::skip_ws(b, 0, b.len());
            let empty = scan::list_marker(b, p, b.len()).is_some_and(|m| scan::rest_blank(b, m.marker.end, b.len()));
            if !empty || hint.line == 0 {
                return Some(PatchSet::empty());
            }
            let prev_end = crate::text::prev_line_content_end(text, hint.line);
            return Some(PatchSet::delete(prev_end..ce));
        }
        let from = hint.from().min(ce);
        let line = &text[from..ce];
        let setext = {
            let t = line.trim();
            !t.is_empty() && (t.bytes().all(|c| c == b'=') || t.bytes().all(|c| c == b'-'))
        };
        if opener_of(line, self.options()).is_none() && !setext {
            return None;
        }
        let b = line.as_bytes();
        let p = scan::skip_ws(b, 0, b.len());
        // Ordered list `1.` / `1)`: escape the delimiter; otherwise the first marker byte.
        let digits = b[p..].iter().take_while(|c| c.is_ascii_digit()).count();
        let at = if digits > 0 && matches!(b.get(p + digits), Some(b'.' | b')')) {
            p + digits
        } else {
            p
        };
        Some(PatchSet::single_verbatim(from + at..from + at, "\\"))
    }

    fn continuation_prefix(&self, at: usize) -> String {
        let text = self.text();
        let ls = line_start(text, at);
        let line = &text[ls..at];
        let n = line.len() - line.trim_start_matches(['>', ' ']).len();
        line[..n].to_string()
    }

    fn pairs(&self) -> &'static [(char, char)] {
        &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')]
    }
}

/// The marker prefix of the next list item after an item whose first line starts with
/// `prefix` (container prefix + list marker): same indentation and bullet, `n + 1` for an
/// ordered item (no renumbering of later items: C11 is M2c), an unchecked box for a task item.
fn next_item_prefix(prefix: &str) -> String {
    let lead = prefix.len() - prefix.trim_start_matches(['>', ' ', '\t']).len();
    let (head, marker) = prefix.split_at(lead);
    let body = marker.trim_end_matches([' ', '\t']);
    let gap = &marker[body.len()..];
    let gap = if gap.is_empty() { " " } else { gap };
    let (mark, task) = match body.find('[') {
        Some(i) if body[i..].len() >= 3 && matches!(&body[i..i + 3], "[ ]" | "[x]" | "[X]") => {
            (body[..i].trim_end(), true)
        }
        _ => (body, false),
    };
    let digits = mark.chars().take_while(char::is_ascii_digit).count();
    let mark = if digits > 0 {
        let n: u64 = mark[..digits].parse().unwrap_or(0);
        format!("{}{}", n + 1, &mark[digits..])
    } else {
        mark.to_string()
    };
    if task {
        format!("{head}{mark} [ ]{gap}")
    } else {
        format!("{head}{mark}{gap}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline::normalize_label;
    use markise_doc::{FormatDoc, label_key};

    fn md(s: &str) -> Document {
        crate::parse(s)
    }

    #[test]
    fn hint_makes_a_list_line_a_paragraph() {
        let mut d = md("- \n");
        assert_eq!(crate::debug_tree(&d), "ul\n  li\n");
        d.set_hints(&HintSet::from_hints([LiteralHint::new(0, HintOrigin::RuleTrigger)]));
        assert_eq!(crate::debug_tree(&d), "para \"- \"\n");
        assert_eq!(d.text(), "- \n", "hints never change bytes");
        d.set_hints(&HintSet::new());
        assert_eq!(crate::debug_tree(&d), "ul\n  li\n");
    }

    #[test]
    fn hint_unclosed_fence_does_not_swallow() {
        let mut d = md("a\n\n```\n\nb\n\nc\n");
        assert!(crate::debug_tree(&d).starts_with("para \"a\"\nfence"));
        d.set_hints(&HintSet::from_hints([LiteralHint::new(3, HintOrigin::EnterPending)]));
        assert_eq!(
            crate::debug_tree(&d),
            "para \"a\"\npara \"```\"\npara \"b\"\npara \"c\"\n"
        );
        assert_eq!(
            crate::debug_tree(&d),
            crate::debug_tree(&{
                let mut f = md("a\n\n```\n\nb\n\nc\n");
                f.set_hints(d.hints());
                f
            })
        );
    }

    #[test]
    fn hints_shift_with_edits_and_survive_reparse() {
        let mut d = md("x\n\n# T\n");
        d.set_hints(&HintSet::from_hints([LiteralHint::new(3, HintOrigin::EnterPending)]));
        assert_eq!(crate::debug_tree(&d), "para \"x\"\npara \"# T\"\n");
        d.apply_patches(&PatchSet::single_verbatim(0..0, "yy"));
        assert_eq!(d.hints().iter().next().unwrap().line, 5);
        assert_eq!(crate::debug_tree(&d), "para \"yyx\"\npara \"# T\"\n");
        // the hinted line typed into keeps its hint
        d.apply_patches(&PatchSet::single_verbatim(8..8, "itle"));
        assert_eq!(crate::debug_tree(&d), "para \"yyx\"\npara \"# Title\"\n");
    }

    #[test]
    fn leaf_and_inlines_are_neutral() {
        let d = md("> - a **b** [l](u)\n");
        let leaf = d.leaf(7).unwrap();
        assert_eq!(leaf.kind, LeafKind::Paragraph);
        assert_eq!(
            leaf.containers,
            vec![
                ContainerKind::Quote,
                ContainerKind::List { ordered: false },
                ContainerKind::ListItem {
                    ordered: false,
                    task: None
                }
            ]
        );
        let rich = d.inlines(&leaf);
        assert_eq!(
            rich.debug(),
            "text[0..2] strong[2..7 m:2..4,5..7](text[4..5]) text[7..8] link[8..14 m:8..9,10..14](text[9..10])"
        );
    }

    #[test]
    fn inline_offsets_skip_container_prefixes() {
        let d = md("> a\n> *b*\n");
        let leaf = d.leaf(3).unwrap();
        let rich = d.inlines(&leaf);
        // second line starts at source 6, leaf base 2 → `*b*` at 4..7
        assert!(rich.debug().contains("emph[4..7 m:4..5,6..7]"), "{}", rich.debug());
    }

    #[test]
    fn materialize_escapes_markers() {
        let d = md("- \n\n# T\n\n```\n\n1. x\n\nplain\n");
        let esc = |line| {
            d.materialize_hint(&LiteralHint::new(line, HintOrigin::EnterPending))
                .map(|p| p.apply_to(d.text()))
        };
        assert!(esc(0).unwrap().starts_with("\\- \n"));
        assert!(esc(4).unwrap().contains("\n\\# T\n"));
        assert!(esc(9).unwrap().contains("\n\\```\n"));
        assert!(esc(14).unwrap().contains("\n1\\. x\n"));
        assert_eq!(esc(20), None, "plain text needs no escape");
        for src in ["- ", "# T", "```", "1. x", "---", "> q"] {
            let d = md(src);
            let ps = d
                .materialize_hint(&LiteralHint::new(0, HintOrigin::EnterPending))
                .unwrap();
            let out = md(&ps.apply_to(src));
            assert!(
                crate::debug_tree(&out).starts_with("para"),
                "{src:?}: {}",
                crate::debug_tree(&out)
            );
        }
    }

    #[test]
    fn streamed_label_keys_match_normalized_keys() {
        for l in ["Foo", "  foo   BAR ", "ẞtraße", "a\nb", "^Note", "Ǆx"] {
            assert_eq!(normalized_key("", l), Some(label_key(&normalize_label(l))), "{l:?}");
            assert_eq!(
                normalized_key("^", l),
                Some(label_key(&format!("^{}", normalize_label(l))))
            );
        }
        let d = md("x [Foo\nbar] and [^n] and `[a]`\n");
        let leaf = d.leaf(0).unwrap();
        let mut keys = Vec::new();
        d.reference_candidates(&leaf.lines, &mut keys);
        assert!(keys.contains(&label_key(&normalize_label("foo bar"))));
        assert!(keys.contains(&label_key("^N")));
        assert!(keys.contains(&label_key("A")));
        // certain inline links look nothing up; anything doubtful stays a candidate
        let cands = |src: &str| {
            let d = md(src);
            let leaf = d.leaf(0).unwrap();
            let mut keys = Vec::new();
            d.reference_candidates(&leaf.lines, &mut keys);
            keys
        };
        assert!(cands("see [a](http://x/y?z=1) and ![b](c.png)\n").is_empty());
        assert_eq!(cands("bad [a](b c) dest\n"), vec![label_key("A")]);
        assert_eq!(cands("unclosed [a](b\n"), vec![label_key("A")]);
        assert!(
            cands("full [t][ref](x)\n").contains(&label_key("REF")),
            "full reference label"
        );
        // the parser agrees: those simple links are links without definitions
        let d = md("see [a](http://x/y?z=1) and ![b](c.png)\n");
        let rich = d.inlines(&d.leaf(0).unwrap());
        assert!(
            rich.debug().contains("link") && rich.debug().contains("image"),
            "{}",
            rich.debug()
        );
    }

    #[test]
    fn heading_planner() {
        let d = md("para\n");
        let p = d
            .plan(planners::HEADING_2, &PlanRequest::at(Selection::caret(1)))
            .unwrap();
        assert_eq!(p.steps[0].patches.apply_to(d.text()), "## para\n");
        let d = md("### H\n");
        let p = d
            .plan(planners::HEADING_1, &PlanRequest::at(Selection::caret(5)))
            .unwrap();
        assert_eq!(p.steps[0].patches.apply_to(d.text()), "# H\n");
        let p = d
            .plan(planners::HEADING_3, &PlanRequest::at(Selection::caret(5)))
            .unwrap();
        assert_eq!(p.steps[0].patches.apply_to(d.text()), "H\n", "same level → paragraph");
        assert_eq!(
            d.plan(PlannerId("markdown.nothing"), &PlanRequest::default()),
            Err(PlanError::Unsupported)
        );
    }
}
