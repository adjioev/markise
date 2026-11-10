//! Source to fields: the parser finds the blocks of the `:::` body, each goes to the first
//! declared field that takes it, and every value keeps the ranges it was read from.

use std::collections::BTreeSet;
use std::ops::Range;

use markise_doc::text::LineEnding;
use markise_md::inline::{InlineContext, InlineKind, LinkKind, parse_inlines};
use markise_md::{Block, BlockKind, Document, ParseOptions};

use super::escape::{anchor_start, unescape_anchor_tail, unescape_heading_tail, unescape_line};
use super::{BlockFields, Field, FieldDecl, FieldKind, FieldValue, FieldsError, Item, ItemData, ItemId};

type R = Range<usize>;

/// One line of a value: `prefix` (quote marker, indent) + `content` + terminator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub prefix: R,
    pub content: R,
    /// The line terminator, empty on the last line of the region.
    pub term: R,
    /// The content as a value line.
    pub value: String,
}

/// Where an item was read from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ItemSrc {
    pub whole: R,
    /// Line start to content start: indent, marker, task box.
    pub head: R,
    pub lines: Vec<Line>,
    /// ` {#id}` at the end of the last line.
    pub tail: R,
    pub task_box: Option<R>,
    pub number: Option<R>,
    /// Alt and destination of an image.
    pub image: Option<(R, R)>,
    /// What in its own text is not plain text: a block that starts the item.
    pub protected: Vec<R>,
    /// The code, math and HTML blocks among `protected`: kept byte for byte, never rewritten.
    pub code: Vec<R>,
    /// Markdown nested under the item after its own text: from the end of that text (its line
    /// terminator included) to the end of the item. Part of the item, kept byte for byte, never
    /// read as a value.
    pub under: R,
}

/// How ordered list markers go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Numbering {
    pub start: u64,
    pub step: u64,
}

/// Where a field was read from; all empty when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FieldSrc {
    /// A heading's `## `.
    pub head: R,
    pub lines: Vec<Line>,
    /// What follows a heading's text: ` {#id}`, closing hashes.
    pub tail: R,
    /// The bytes between neighbouring items.
    pub gaps: Vec<R>,
    pub numbering: Option<Numbering>,
    /// What in a text field or quote is not plain text: a code block or list it took as it stands.
    pub protected: Vec<R>,
    /// The code, math and HTML blocks in it, at any depth: their lines are values as written, and
    /// no edit may change their bytes.
    pub code: Vec<R>,
}

/// A region of the body. Field regions and extras never overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Chunk {
    pub range: R,
    pub field: Option<usize>,
}

pub(super) fn line_start(s: &str, at: usize) -> usize {
    s[..at].rfind(['\n', '\r']).map_or(0, |i| i + 1)
}

pub(super) fn line_end(s: &str, at: usize) -> usize {
    s[at..].find(['\n', '\r']).map_or(s.len(), |i| at + i)
}

/// `(line, terminator)` of each line of `r`, which starts at a line start and ends at a line end.
fn split_lines(s: &str, r: R) -> Vec<(R, R)> {
    let mut out = Vec::new();
    let mut at = r.start;
    loop {
        let e = line_end(s, at).min(r.end);
        if e >= r.end {
            out.push((at..e, e..e));
            return out;
        }
        let t = if s[e..].starts_with("\r\n") { e + 2 } else { e + 1 };
        out.push((at..e, e..t));
        at = t;
    }
}

fn extended(s: &str, b: &Block) -> R {
    line_start(s, b.span.start)..line_end(s, b.span.end)
}

/// A body block as fields see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    Heading(u8),
    Paragraph,
    Images,
    Quote,
    PlainList,
    TaskList,
    Other,
    /// A nested `:::` block: always an extra.
    Directive,
}

fn shape(s: &str, b: &Block, opts: &ParseOptions) -> Shape {
    match &b.kind {
        BlockKind::Heading {
            level, setext: false, ..
        } => Shape::Heading(*level),
        BlockKind::Paragraph if image_lines(s, b, opts).is_some() => Shape::Images,
        BlockKind::Paragraph => Shape::Paragraph,
        BlockKind::BlockQuote { .. } => Shape::Quote,
        BlockKind::List { .. } if b.children.iter().all(is_task) => Shape::TaskList,
        BlockKind::List { .. } => Shape::PlainList,
        BlockKind::Directive { .. } => Shape::Directive,
        _ => Shape::Other,
    }
}

fn is_task(b: &Block) -> bool {
    matches!(b.kind, BlockKind::ListItem { task: Some(_), .. })
}

fn takes(kind: FieldKind, sh: Shape) -> bool {
    match kind {
        FieldKind::Heading { level } => matches!(sh, Shape::Heading(l) if level.is_none_or(|w| w == l)),
        FieldKind::Text => sh == Shape::Paragraph,
        FieldKind::Quote => sh == Shape::Quote,
        FieldKind::List => sh == Shape::PlainList,
        FieldKind::Choices => sh == Shape::TaskList,
        FieldKind::Image => sh == Shape::Images,
        FieldKind::Argument => false,
    }
}

/// Which field takes each body block. A field fills once, except that text and images go on
/// over neighbouring blocks. A task list reads as a plain list when no field wants answers, and
/// text takes what no field after the cursor takes.
fn assign(shapes: &[Shape], decls: &[FieldDecl]) -> Vec<Option<usize>> {
    let mut filled = vec![false; decls.len()];
    let (mut cursor, mut last) = (0, None);
    let mut out = Vec::with_capacity(shapes.len());
    for &sh in shapes {
        let first = |want: &dyn Fn(FieldKind) -> bool| {
            (cursor..decls.len()).find(|&k| {
                let goes_on = last == Some(k) && matches!(decls[k].kind, FieldKind::Text | FieldKind::Image);
                (!filled[k] || goes_on) && want(decls[k].kind)
            })
        };
        let k = first(&|kind| takes(kind, sh))
            .or_else(|| first(&|kind| sh == Shape::TaskList && kind == FieldKind::List))
            .or_else(|| first(&|kind| sh != Shape::Directive && kind == FieldKind::Text));
        if let Some(k) = k {
            filled[k] = true;
            cursor = k;
        }
        last = k;
        out.push(k);
    }
    out
}

/// A block whose lines are literal text: escaping, unescaping or trimming them changes what it says.
fn is_verbatim(b: &Block) -> bool {
    matches!(
        b.kind,
        BlockKind::FencedCode { .. } | BlockKind::IndentedCode | BlockKind::MathBlock { .. } | BlockKind::HtmlBlock
    )
}

/// The verbatim blocks among `blocks` and below them, as whole lines.
fn verbatim_ranges<'a>(s: &str, blocks: impl IntoIterator<Item = &'a Block>, out: &mut Vec<R>) {
    for b in blocks {
        if is_verbatim(b) {
            out.push(extended(s, b));
        } else {
            verbatim_ranges(s, &b.children, out);
        }
    }
}

/// The lines of a text region, or of a quote with its markers as prefixes. A line of a code block
/// in it is its value as written: no escape is taken from it.
fn plain_lines(s: &str, r: R, quote: bool, code: &[R], opts: &ParseOptions) -> Vec<Line> {
    let line = |(l, term): (R, R)| {
        let text = &s[l.clone()];
        let indent = text.len() - text.trim_start_matches(' ').len();
        let mut p = l.start;
        if quote && indent <= 3 && text[indent..].starts_with('>') {
            p += indent + 1 + usize::from(text[indent + 1..].starts_with(' '));
        }
        let verbatim = code.iter().any(|c| c.start <= l.start && l.end <= c.end);
        let value = if verbatim {
            s[p..l.end].to_string()
        } else {
            unescape_line(&s[p..l.end], opts)
        };
        Line {
            prefix: l.start..p,
            content: p..l.end,
            term,
            value,
        }
    };
    split_lines(s, r).into_iter().map(line).collect()
}

fn numbering(items: &[Block]) -> Option<Numbering> {
    let nums: Vec<u64> = (items.iter())
        .filter_map(|b| match b.kind {
            BlockKind::ListItem { number, .. } => number,
            _ => None,
        })
        .collect();
    let (&start, rest) = nums.split_first()?;
    let step = rest.first().map_or(1, |&n| n.wrapping_sub(start));
    let regular = step <= 1 && nums.iter().enumerate().all(|(i, &n)| n == start + i as u64 * step);
    regular.then_some(Numbering { start, step })
}

/// Cuts a real ` {#id}` off the end of `s[r]`: the id and the range of the cut part.
fn cut_anchor(s: &str, r: &R) -> Option<(String, R)> {
    let text = &s[r.clone()];
    // the form a reader of the rendered view has hidden too: at the start or after a blank
    let at = anchor_start(text).filter(|&at| at == 0 || text[..at].ends_with([' ', '\t']))?;
    let cut = at - usize::from(text[..at].ends_with(' '));
    Some((text[at + 2..text.len() - 1].to_string(), r.start + cut..r.end))
}

/// The columns `text` takes, with tab stops every four.
pub(super) fn columns(text: &str) -> usize {
    text.chars().fold(0, next_column)
}

fn next_column(col: usize, c: char) -> usize {
    if c == '\t' { (col / 4 + 1) * 4 } else { col + 1 }
}

/// How many bytes of `text` reach column `target`, or as far as it goes; with `blank_only` only
/// over spaces and tabs.
fn bytes_to_column(text: &str, target: usize, blank_only: bool) -> usize {
    let mut col = 0;
    for (at, c) in text.char_indices() {
        if col >= target || (blank_only && !matches!(c, ' ' | '\t')) {
            return at;
        }
        col = next_column(col, c);
    }
    text.len()
}

fn list_item(s: &str, b: &Block, opts: &ParseOptions, choice: bool) -> Option<(ItemSrc, ItemData)> {
    let BlockKind::ListItem {
        marker,
        number,
        task,
        content_offset,
    } = &b.kind
    else {
        return None;
    };
    let whole = line_start(s, marker.start)..line_end(s, b.span.end);
    let task = task.as_ref().filter(|_| choice);
    // the parser's offset is in columns, and a tab is several of them
    let indent = columns(&s[whole.start..marker.start]) + content_offset;
    let first_end = line_end(s, marker.start);
    // the item's own text is its first paragraph, or its first line; what follows is nested under it
    let own_end = match b.children.first() {
        Some(c) if matches!(c.kind, BlockKind::Paragraph) => line_end(s, c.span.end).max(first_end),
        _ => first_end,
    }
    .min(whole.end);
    let content_start = match task {
        Some((_, bx)) => bx.end + usize::from(s[bx.end..first_end].starts_with(' ')),
        None => whole.start + bytes_to_column(&s[whole.start..first_end], indent, false),
    };
    let mut lines = Vec::new();
    for (i, (l, term)) in split_lines(s, content_start..own_end).into_iter().enumerate() {
        let text = &s[l.clone()];
        let p = if i == 0 { 0 } else { bytes_to_column(text, indent, true) };
        let value = unescape_line(&text[p..], opts);
        lines.push(Line {
            prefix: l.start..l.start + p,
            content: l.start + p..l.end,
            term,
            value,
        });
    }
    let mut src = ItemSrc {
        whole: whole.clone(),
        head: whole.start..content_start,
        ..ItemSrc::default()
    };
    src.tail = whole.end..whole.end;
    let mut anchor = None;
    if choice && let Some(last) = lines.last_mut() {
        if let Some((id, tail)) = cut_anchor(s, &last.content) {
            anchor = Some(id);
            last.content.end = tail.start;
            last.value = unescape_line(&s[last.content.clone()], opts);
            src.tail = tail;
        }
        last.value = unescape_anchor_tail(&last.value);
    }
    let text = join_values(&lines);
    let data = match task {
        Some((correct, _)) => ItemData::Choice {
            anchor,
            text,
            correct: *correct,
        },
        None => ItemData::Text(text),
    };
    src.number = number.map(|_| {
        let digits = s[marker.start..marker.end]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        marker.start..marker.start + digits
    });
    src.task_box = task.map(|(_, bx)| bx.start..bx.end);
    src.lines = lines;
    let own: Vec<&Block> = (b.children.iter())
        .filter(|c| !matches!(c.kind, BlockKind::Paragraph) && c.span.start < own_end)
        .collect();
    src.protected = own.iter().map(|c| extended(s, c)).collect();
    verbatim_ranges(s, own, &mut src.code);
    src.under = own_end..whole.end;
    Some((src, data))
}

pub(super) fn join_values(lines: &[Line]) -> String {
    lines.iter().map(|l| l.value.as_str()).collect::<Vec<_>>().join("\n")
}

/// `![alt](dest…)` exactly filling `s[r]`: the ranges of alt and dest.
fn image_parts(s: &str, r: R, opts: &ParseOptions) -> Option<(R, R)> {
    let t = s[r.clone()].trim();
    let base = r.start + (s[r.clone()].len() - s[r].trim_start().len());
    let inl = parse_inlines(t, opts, &InlineContext::default());
    let [one] = &inl[..] else { return None };
    let InlineKind::Image {
        dest,
        kind: LinkKind::Inline,
        ..
    } = &one.kind
    else {
        return None;
    };
    if one.span.start != 0 || one.span.end != t.len() || !t.starts_with("![") {
        return None;
    }
    // a destination written other than literally is not edited here: the paragraph stays text
    let (mid, written) = [format!("](<{dest}>"), format!("]({dest}")]
        .into_iter()
        .find_map(|w| t.rfind(&w).map(|at| (at, w.len() - 2)))?;
    let dest_at = base + mid + 2;
    Some((base + 2..base + mid, dest_at..dest_at + written))
}

fn image_lines(s: &str, b: &Block, opts: &ParseOptions) -> Option<Vec<(R, R, R)>> {
    let lines = split_lines(s, extended(s, b));
    (lines.into_iter())
        .map(|(l, _)| image_parts(s, l.clone(), opts).map(|(a, d)| (l, a, d)))
        .collect()
}

pub(super) fn image_value(s: &str, alt: &R, dest: &R) -> (String, String) {
    let d = &s[dest.clone()];
    let path = d.strip_prefix('<').and_then(|x| x.strip_suffix('>')).unwrap_or(d);
    (s[alt.clone()].replace("\\[", "[").replace("\\]", "]"), path.to_string())
}

fn heading_src(s: &str, b: &Block, whole: R) -> FieldSrc {
    let text = &s[whole.clone()];
    let hashes = text.len() - text.trim_start_matches(' ').len();
    let run = text[hashes..].bytes().take_while(|&c| c == b'#').count();
    let spaces = text[hashes + run..].len() - text[hashes + run..].trim_start_matches([' ', '\t']).len();
    let h = whole.start + hashes + run + spaces;
    let closed = matches!(b.kind, BlockKind::Heading { closed: true, .. });
    let end = b.lines.first().filter(|_| closed).map_or(whole.end, |l| l.end.max(h));
    let mut content = h..end;
    let mut tail = end..whole.end;
    if let Some((_, cut)) = cut_anchor(s, &content) {
        tail = cut.start..whole.end;
        content.end = cut.start;
    }
    let value = unescape_anchor_tail(&unescape_heading_tail(&s[content.clone()]));
    let term = content.end..content.end;
    let line = Line {
        prefix: h..h,
        content,
        term,
        value,
    };
    FieldSrc {
        head: whole.start..h,
        lines: vec![line],
        tail,
        ..FieldSrc::default()
    }
}

/// Reads the value and source of a field from the body blocks it took.
fn read_field(s: &str, f: &mut Field, group: &[Block], whole: R, next_item: &mut u32, opts: &ParseOptions) {
    let mut new_item = |data, src: ItemSrc| {
        *next_item += 1;
        Item {
            id: ItemId(*next_item - 1),
            data,
            src: Some(src),
        }
    };
    match f.kind {
        FieldKind::Argument => {}
        FieldKind::Heading { .. } => {
            f.src = heading_src(s, &group[0], whole);
            f.value = FieldValue::Text(f.src.lines[0].value.clone());
        }
        FieldKind::Text | FieldKind::Quote => {
            // a quote holding more than paragraphs, a text field with what it took as it stands
            let held: Vec<&Block> = if f.kind == FieldKind::Quote {
                group.iter().flat_map(|b| &b.children).collect()
            } else {
                group.iter().collect()
            };
            verbatim_ranges(s, held.iter().copied(), &mut f.src.code);
            f.src.lines = plain_lines(s, whole.clone(), f.kind == FieldKind::Quote, &f.src.code, opts);
            f.value = FieldValue::Text(join_values(&f.src.lines));
            f.src.protected = (held.into_iter())
                .filter(|b| !matches!(b.kind, BlockKind::Paragraph))
                .map(|b| extended(s, b))
                .collect();
        }
        FieldKind::List | FieldKind::Choices | FieldKind::Image => {
            let mut items = Vec::new();
            if f.kind == FieldKind::Image {
                for (l, alt, dest) in group.iter().flat_map(|b| image_lines(s, b, opts).unwrap_or_default()) {
                    let (a, p) = image_value(s, &alt, &dest);
                    let src = ItemSrc {
                        whole: l,
                        image: Some((alt, dest)),
                        ..ItemSrc::default()
                    };
                    items.push(new_item(ItemData::Image { alt: a, path: p }, src));
                }
            } else {
                for c in &group[0].children {
                    if let Some((src, data)) = list_item(s, c, opts, f.kind == FieldKind::Choices) {
                        items.push(new_item(data, src));
                    }
                }
                f.src.numbering = numbering(&group[0].children);
            }
            let wholes: Vec<&R> = items.iter().filter_map(|i| i.src.as_ref()).map(|x| &x.whole).collect();
            f.src.gaps = wholes.windows(2).map(|w| w[0].end..w[1].start).collect();
            f.value = FieldValue::Items(items);
        }
    }
}

/// The kinds of what stands below a block, in order. A paragraph and a list item come and go as
/// text is edited; anything else there means the text became structure.
fn nested_kinds(b: &Block, out: &mut Vec<std::mem::Discriminant<BlockKind>>) {
    for c in &b.children {
        if !matches!(c.kind, BlockKind::Paragraph | BlockKind::ListItem { .. }) {
            out.push(std::mem::discriminant(&c.kind));
        }
        nested_kinds(c, out);
    }
}

pub(super) fn decode(
    s: &str,
    base: usize,
    decls: &[FieldDecl],
    opts: &ParseOptions,
) -> Result<BlockFields, FieldsError> {
    let doc = Document::parse(s, opts.clone());
    let tree = doc.tree();
    let [block] = &tree[..] else {
        return Err(FieldsError::NotABlock);
    };
    let BlockKind::Directive {
        name,
        args,
        opener,
        closer,
        ..
    } = &block.kind
    else {
        return Err(FieldsError::NotABlock);
    };
    if opener.start != 0 || closer.end != s.len() {
        return Err(FieldsError::NotABlock);
    }
    let kids = &block.children;
    let shapes: Vec<Shape> = kids.iter().map(|b| shape(s, b, opts)).collect();
    let owner = assign(&shapes, decls);
    let structure = shapes.iter().copied().zip(owner.iter().copied()).collect();
    let new_field = |d: &FieldDecl| {
        let value = match d.kind {
            FieldKind::List | FieldKind::Choices | FieldKind::Image => FieldValue::Items(Vec::new()),
            _ => FieldValue::Text(String::new()),
        };
        Field {
            name: d.name.clone(),
            kind: d.kind,
            value,
            src: FieldSrc::default(),
        }
    };
    let mut fields: Vec<Field> = decls.iter().map(new_field).collect();
    for f in fields.iter_mut().filter(|f| f.kind == FieldKind::Argument) {
        f.value = FieldValue::Text(s[args.start..args.end].to_string());
    }
    let (mut chunks, mut next_item, mut i) = (Vec::new(), 0, 0);
    while i < kids.len() {
        let mut j = i + 1;
        while owner[i].is_some() && j < kids.len() && owner[j] == owner[i] {
            j += 1;
        }
        let range = extended(s, &kids[i]).start..extended(s, &kids[j - 1]).end;
        if let Some(k) = owner[i] {
            read_field(s, &mut fields[k], &kids[i..j], range.clone(), &mut next_item, opts);
        }
        chunks.push(Chunk { range, field: owner[i] });
        i = j;
    }
    Ok(BlockFields {
        source: s.to_string(),
        base,
        name: name.start..name.end,
        args: args.start..args.end,
        opener_end: opener.end,
        closer_start: line_start(s, closer.start),
        eol: if doc.line_ending() == LineEnding::CrLf {
            "\r\n"
        } else {
            "\n"
        },
        opts: opts.clone(),
        decls: decls.to_vec(),
        fields,
        chunks,
        shapes: structure,
        nested: kids.iter().fold(Vec::new(), |mut out, b| {
            nested_kinds(b, &mut out);
            out
        }),
        next_item,
        retired: BTreeSet::new(),
    })
}
