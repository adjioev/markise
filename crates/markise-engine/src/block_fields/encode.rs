//! Edits to source: the new block text is put together from the old bytes of everything that
//! did not change, and a diff of old and new source gives the patches. So a patch can only hold
//! what differs, whatever the edits were.

use std::collections::BTreeSet;
use std::ops::Range;
use std::time::{Duration, Instant};

use markise_doc::{Patch, PatchSet};
use similar::{Algorithm, DiffOp, TextDiff, capture_diff_slices};

use super::decode::{Line, columns};
use super::escape::{escape_anchor_tail, escape_heading_tail, escape_line, valid_anchor};
use super::{Applied, BlockFields, Field, FieldEdit, FieldKind, FieldValue, FieldsError, Item, ItemData, ItemId};

pub(super) fn apply(old: &BlockFields, edits: &[FieldEdit]) -> Result<Applied, FieldsError> {
    let mut draft = Draft {
        fields: old.fields.clone(),
        kept: BTreeSet::new(),
        next_item: old.next_item,
        retired: old.retired.clone(),
        inserted: Vec::new(),
    };
    for e in edits {
        draft.edit(old, e)?;
    }
    let source = render(old, &draft.fields, &draft.kept);
    let patches = PatchSet::new(line_patches(&old.source, &source)).map_err(FieldsError::Patch)?;
    let mut next = super::decode::decode(&source, old.base, &old.decls, &old.opts)?;
    argument_reads_back(&draft.fields, &next)?;
    next.retired = draft.retired;
    next.next_item = draft.next_item;
    carry(&draft.fields, &mut next, old);
    Ok(Applied {
        patches,
        fields: next,
        inserted: draft.inserted,
    })
}

/// The blanks the parser does not count as part of the arguments.
pub(super) fn trim_argument(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

fn argument_reads_back(edited: &[Field], next: &BlockFields) -> Result<(), FieldsError> {
    for f in edited.iter().filter(|f| f.kind == FieldKind::Argument) {
        if f.value != FieldValue::Text(next.args().to_string()) {
            return Err(FieldsError::WrongKind { field: f.name.clone() });
        }
    }
    Ok(())
}

/// The opener line, with only the bytes of its arguments changed: its fence, its name, what stands
/// between and after them stay. An argument coming or going takes the one blank before it with it.
fn render_opener(old: &BlockFields, fields: &[Field]) -> String {
    let s = old.source.as_str();
    let wanted = fields.iter().find(|f| f.kind == FieldKind::Argument);
    let Some(FieldValue::Text(new)) = wanted.map(|f| &f.value) else {
        return s[..old.opener_end].to_string();
    };
    let (args, end) = (&old.args, old.opener_end);
    match (args.is_empty(), new.is_empty()) {
        (true, true) => s[..end].to_string(),
        (true, false) => format!("{} {new}{}", &s[..old.name.end], &s[old.name.end..end]),
        (false, true) => format!("{}{}", &s[..old.name.end], &s[args.end..end]),
        (false, false) => format!("{}{new}{}", &s[..args.start], &s[args.end..end]),
    }
}

/// Whole lines that moved stay whole; a changed line narrows to the characters that differ. A
/// character diff over the whole text would be smaller and would scatter a move over its lines.
fn line_patches(old: &str, new: &str) -> Vec<Patch> {
    let d = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + Duration::from_millis(200))
        .diff_lines(old, new);
    let mut out = Vec::new();
    let (mut at, mut bt) = (0, 0);
    for op in d.ops() {
        let a: Vec<&str> = op.old_range().filter_map(|i| d.old_slice(i)).collect();
        let b: Vec<&str> = op.new_range().filter_map(|i| d.new_slice(i)).collect();
        let (alen, blen) = (a.concat().len(), b.concat().len());
        if !matches!(op, DiffOp::Equal { .. }) {
            if a.len() == b.len() {
                let mut x = at;
                for (l, m) in a.iter().zip(&b) {
                    narrowed(l, m, x, &mut out);
                    x += l.len();
                }
            } else {
                narrowed(&old[at..at + alen], &new[bt..bt + blen], at, &mut out);
            }
        }
        at += alen;
        bt += blen;
    }
    out
}

/// The patch from `a` at `base` to `b` without their common start and end.
fn narrowed(a: &str, b: &str, base: usize, out: &mut Vec<Patch>) {
    let mut p = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(p) || !b.is_char_boundary(p) {
        p -= 1;
    }
    let (ra, rb) = (&a[p..], &b[p..]);
    let mut e = ra
        .bytes()
        .rev()
        .zip(rb.bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    while !ra.is_char_boundary(ra.len() - e) || !rb.is_char_boundary(rb.len() - e) {
        e -= 1;
    }
    out.push(Patch::verbatim(base + p..base + a.len() - e, &rb[..rb.len() - e]));
}

/// The decoded items are the edited items in the same order, so identities go by position; if
/// a value did not read back as its field (ADR 0020, limits) the general matching takes over.
fn carry(edited: &[Field], next: &mut BlockFields, old: &BlockFields) {
    let same_count = edited
        .iter()
        .zip(&next.fields)
        .all(|(a, b)| match (&a.value, &b.value) {
            (FieldValue::Items(x), FieldValue::Items(y)) => x.len() == y.len(),
            _ => true,
        });
    if !same_count {
        super::identity::carry_ids(old, next);
        return;
    }
    for (a, b) in edited.iter().zip(&mut next.fields) {
        if let (FieldValue::Items(x), FieldValue::Items(y)) = (&a.value, &mut b.value) {
            x.iter().zip(y).for_each(|(xi, yi)| yi.id = xi.id);
        }
    }
}

struct Draft {
    fields: Vec<Field>,
    /// Headings emptied and kept ([`FieldEdit::ClearHeading`]).
    kept: BTreeSet<usize>,
    next_item: u32,
    retired: BTreeSet<String>,
    inserted: Vec<ItemId>,
}

impl FieldEdit {
    fn field(&self) -> &str {
        match self {
            FieldEdit::SetText { field, .. }
            | FieldEdit::ClearHeading { field }
            | FieldEdit::SetItemText { field, .. }
            | FieldEdit::SetCorrect { field, .. }
            | FieldEdit::SetImage { field, .. }
            | FieldEdit::InsertItem { field, .. }
            | FieldEdit::RemoveItem { field, .. }
            | FieldEdit::MoveItem { field, .. } => field,
        }
    }
}

impl Draft {
    fn edit(&mut self, old: &BlockFields, e: &FieldEdit) -> Result<(), FieldsError> {
        let name = e.field();
        let taken = block_ids(old, &self.fields);
        let f = &mut self.fields[old.field_index(name)?];
        let wrong = || FieldsError::WrongKind {
            field: name.to_string(),
        };
        let unknown = || FieldsError::UnknownItem {
            field: name.to_string(),
        };
        let items = match (&mut f.value, e) {
            (FieldValue::Text(_), FieldEdit::SetText { text, .. })
                if f.kind == FieldKind::Argument && text.contains(['\n', '\r']) =>
            {
                return Err(wrong());
            }
            (FieldValue::Text(t), FieldEdit::SetText { text, .. }) if f.kind == FieldKind::Argument => {
                *t = trim_argument(text).to_string();
                return Ok(());
            }
            (FieldValue::Text(t), FieldEdit::SetText { text, .. }) => {
                t.clone_from(text);
                return Ok(());
            }
            (FieldValue::Text(t), FieldEdit::ClearHeading { .. }) if matches!(f.kind, FieldKind::Heading { .. }) => {
                t.clear();
                self.kept.insert(old.field_index(name)?);
                return Ok(());
            }
            (FieldValue::Items(items), e)
                if !matches!(e, FieldEdit::SetText { .. } | FieldEdit::ClearHeading { .. }) =>
            {
                items
            }
            _ => return Err(wrong()),
        };
        let find = |items: &[Item], id: &ItemId| items.iter().position(|x| x.id == *id).ok_or_else(unknown);
        match e {
            FieldEdit::SetText { .. } | FieldEdit::ClearHeading { .. } => {}
            FieldEdit::SetItemText { item, text, .. } => {
                let i = find(items, item)?;
                match &mut items[i].data {
                    ItemData::Text(t) | ItemData::Choice { text: t, .. } => t.clone_from(text),
                    ItemData::Image { .. } => return Err(wrong()),
                }
            }
            FieldEdit::SetCorrect { item, correct, .. } => {
                let i = find(items, item)?;
                match &mut items[i].data {
                    ItemData::Choice { correct: c, .. } => *c = *correct,
                    _ => return Err(wrong()),
                }
            }
            FieldEdit::SetImage { item, alt, path, .. } => {
                let i = find(items, item)?;
                match &mut items[i].data {
                    ItemData::Image { alt: a, path: p } => {
                        a.clone_from(alt);
                        p.clone_from(path);
                    }
                    _ => return Err(wrong()),
                }
            }
            FieldEdit::InsertItem { at, item, .. } => {
                if *at > items.len() {
                    return Err(unknown());
                }
                let fits = match item {
                    ItemData::Text(_) => f.kind == FieldKind::List,
                    ItemData::Choice { .. } => f.kind == FieldKind::Choices,
                    ItemData::Image { .. } => f.kind == FieldKind::Image,
                };
                if !fits {
                    return Err(wrong());
                }
                let mut data = item.clone();
                if let ItemData::Choice { anchor, .. } = &mut data {
                    let fresh = new_anchor(&taken, anchor.as_deref(), &self.retired);
                    *anchor = Some(fresh);
                }
                let id = ItemId(self.next_item);
                self.next_item += 1;
                items.insert(*at, Item { id, data, src: None });
                self.inserted.push(id);
            }
            FieldEdit::RemoveItem { item, .. } => {
                let i = find(items, item)?;
                if let ItemData::Choice { anchor: Some(a), .. } = items.remove(i).data {
                    self.retired.insert(a);
                }
            }
            FieldEdit::MoveItem { item, to, .. } => {
                let i = find(items, item)?;
                if *to >= items.len() {
                    return Err(unknown());
                }
                let x = items.remove(i);
                items.insert(*to, x);
            }
        }
        Ok(())
    }
}

/// Every `{#id}` written anywhere in `text`, whether or not a reader would call it an id there: what
/// must not be given out again.
pub(crate) fn ids_in(text: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for part in text.split("{#").skip(1) {
        if let Some(id) = part.split('}').next().filter(|id| valid_anchor(id)) {
            ids.insert(id.to_string());
        }
    }
    ids
}

/// Every id in the block: in the source, headings and what no field took included, and on the
/// answers as edited so far.
fn block_ids(old: &BlockFields, fields: &[Field]) -> BTreeSet<String> {
    let mut ids = ids_in(&old.source);
    for f in fields {
        if let FieldValue::Items(items) = &f.value {
            ids.extend(items.iter().filter_map(|i| match &i.data {
                ItemData::Choice { anchor, .. } => anchor.clone(),
                _ => None,
            }));
        }
    }
    ids
}

/// The wanted id if valid and free, or `a<N>` above every number in use or retired, so a
/// removed answer's id does not come back.
pub(crate) fn new_anchor(taken: &BTreeSet<String>, wanted: Option<&str>, retired: &BTreeSet<String>) -> String {
    if let Some(w) = wanted.filter(|w| valid_anchor(w) && !taken.contains(*w) && !retired.contains(*w)) {
        return w.to_string();
    }
    let number = |a: &String| a.strip_prefix('a').and_then(|n| n.parse::<u64>().ok());
    let top = taken.iter().chain(retired).filter_map(number).max().unwrap_or(0);
    // wide enough that a u64 id in the file cannot overflow it; what u64 could not read is checked
    (u128::from(top) + 1..)
        .map(|n| format!("a{n}"))
        .find(|a| !taken.contains(a) && !retired.contains(a))
        .expect("finitely many ids are taken")
}

/// The new block text: the regions in order, with the old gaps where neighbours stay neighbours
/// and a blank line elsewhere. Keeping every neighbour apart is what keeps blocks from merging.
fn render(old: &BlockFields, fields: &[Field], kept: &BTreeSet<usize>) -> String {
    let s = old.source.as_str();
    let is_absent = |f: &Field| {
        !old.chunks
            .iter()
            .any(|c| c.field.is_some_and(|k| fields[k].name == f.name))
    };
    // (index in the old chunk list, text); a new field comes right before the next field read
    let mut pieces: Vec<(Option<usize>, String)> = Vec::new();
    let mut pending = 0;
    let mut absent_up_to = |k: usize, pieces: &mut Vec<(Option<usize>, String)>| {
        for f in &fields[pending..k] {
            if is_absent(f) {
                pieces.push((None, render_field(old, f)));
            }
        }
        pending = pending.max(k);
    };
    for (ci, c) in old.chunks.iter().enumerate() {
        match c.field {
            None => pieces.push((Some(ci), s[c.range.clone()].to_string())),
            Some(k) => {
                absent_up_to(k, &mut pieces);
                // a value nobody changed is its bytes, even when it reads as empty
                let same = fields[k].value == old.fields[k].value;
                let text = if same {
                    s[c.range.clone()].to_string()
                } else if kept.contains(&k) {
                    // its marks, its tail with no blank before it, and the blank that keeps it a heading
                    let f = &fields[k].src;
                    let head = &s[f.head.clone()];
                    let tail = s[f.tail.clone()].trim_start();
                    let blank = if head.ends_with([' ', '\t']) { "" } else { " " };
                    format!("{head}{blank}{tail}")
                } else {
                    render_field(old, &fields[k])
                };
                pieces.push((Some(ci), text));
            }
        }
    }
    absent_up_to(fields.len(), &mut pieces);
    pieces.retain(|(_, t)| !t.is_empty());

    let chunk = |i: usize| &old.chunks[i].range;
    let (first_gap, last_gap) = match (old.chunks.first(), old.chunks.last()) {
        (Some(a), Some(z)) => (old.opener_end..a.range.start, z.range.end..old.closer_start),
        _ => (old.opener_end..old.closer_start, old.opener_end..old.closer_start),
    };
    let mut out = String::with_capacity(s.len() + 64);
    out.push_str(&render_opener(old, fields));
    out.push_str(&s[first_gap]);
    for (n, (at, text)) in pieces.iter().enumerate() {
        if n > 0 {
            let gap_after = pieces[n - 1]
                .0
                .filter(|&a| a + 1 < old.chunks.len())
                .map(|a| chunk(a).end..chunk(a + 1).start);
            match gap_after {
                Some(g) if *at == pieces[n - 1].0.map(|a| a + 1) || has_blank(&s[g.clone()]) => out.push_str(&s[g]),
                _ => {
                    out.push_str(old.eol);
                    out.push_str(old.eol);
                }
            }
        }
        out.push_str(text);
    }
    if !pieces.is_empty() {
        out.push_str(&s[last_gap]);
    }
    out.push_str(&s[old.closer_start..]);
    out
}

fn has_blank(gap: &str) -> bool {
    gap.matches('\n').count() >= 2
}

fn value_lines(v: &str) -> Vec<String> {
    let v = v.replace("\r\n", "\n").replace('\r', "\n");
    let v = v.trim_matches('\n');
    if v.trim().is_empty() {
        return Vec::new();
    }
    (v.split('\n'))
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                l.to_string()
            }
        })
        .collect()
}

/// How new lines of a value are written.
struct Style<'a> {
    /// The prefix of the first line and of the others.
    first: &'a str,
    rest: &'a str,
    /// The prefix of a blank line.
    blank: &'a str,
    esc: fn(&BlockFields, &str, bool) -> String,
}

fn esc_block(old: &BlockFields, line: &str, _last: bool) -> String {
    escape_line(line, &old.opts)
}

fn esc_answer(old: &BlockFields, line: &str, last: bool) -> String {
    let l = escape_line(line, &old.opts);
    if last { escape_anchor_tail(&l) } else { l }
}

fn esc_heading(_: &BlockFields, line: &str, _last: bool) -> String {
    escape_anchor_tail(&escape_heading_tail(line.trim_start()))
}

/// Writes `new` over the old `lines`: a line that stays keeps its bytes.
fn render_lines(old: &BlockFields, lines: &[Line], new: &[String], st: &Style, out: &mut String) {
    let s = old.source.as_str();
    // a line of blanks reads as an empty one here too, and keeps its bytes while it stays
    let olds: Vec<&str> = (lines.iter())
        .map(|l| {
            if l.value.trim().is_empty() {
                ""
            } else {
                l.value.as_str()
            }
        })
        .collect();
    let news: Vec<&str> = new.iter().map(String::as_str).collect();
    let mut from: Vec<Option<usize>> = vec![None; new.len()];
    // a line rewritten where it stands keeps what is not its text: its prefix and its terminator
    let mut shell: Vec<Option<usize>> = vec![None; new.len()];
    for op in capture_diff_slices(Algorithm::Myers, &olds, &news) {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => (0..len).for_each(|i| from[new_index + i] = Some(old_index + i)),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (0..old_len.min(new_len)).for_each(|i| shell[new_index + i] = Some(old_index + i)),
            _ => {}
        }
    }
    for (k, value) in new.iter().enumerate() {
        let last = k + 1 == new.len();
        let moved_prefix = if k == 0 { st.first } else { st.rest };
        if let (None, Some(l), false) = (from[k], shell[k].map(|j| &lines[j]), value.is_empty()) {
            out.push_str(&s[l.prefix.clone()]);
            out.push_str(&(st.esc)(old, value, last));
            if !last {
                out.push_str(if l.term.is_empty() { old.eol } else { &s[l.term.clone()] });
            }
            continue;
        }
        match from[k].map(|j| (j, &lines[j])) {
            Some((j, l)) => {
                out.push_str(if (k == 0) == (j == 0) {
                    &s[l.prefix.clone()]
                } else {
                    moved_prefix
                });
                out.push_str(&s[l.content.clone()]);
                if !last {
                    out.push_str(if l.term.is_empty() { old.eol } else { &s[l.term.clone()] });
                }
            }
            None => {
                if value.is_empty() {
                    out.push_str(st.blank);
                } else {
                    out.push_str(moved_prefix);
                    out.push_str(&(st.esc)(old, value, last));
                }
                if !last {
                    out.push_str(old.eol);
                }
            }
        }
    }
}

fn render_field(old: &BlockFields, f: &Field) -> String {
    let s = old.source.as_str();
    let mut out = String::new();
    if f.kind == FieldKind::Argument {
        // it is in the opener
        return out;
    }
    let v = match &f.value {
        FieldValue::Items(items) => {
            render_items(old, f, items, &mut out);
            return out;
        }
        FieldValue::Text(v) => v,
    };
    let mut new = value_lines(v);
    if new.is_empty() {
        return out;
    }
    let mut st = Style {
        first: "",
        rest: "",
        blank: "",
        esc: esc_block,
    };
    match f.kind {
        FieldKind::Heading { level } => {
            new = vec![new.join(" ")];
            st.esc = esc_heading;
            if f.src.lines.is_empty() {
                out.push_str(&"#".repeat(usize::from(level.unwrap_or(2))));
                out.push(' ');
            }
        }
        FieldKind::Quote => {
            let first = f.src.lines.first().map_or("", |l| &s[l.prefix.clone()]);
            st.first = if first.is_empty() { "> " } else { first };
            st.rest = st.first;
            st.blank = ">";
        }
        _ => {}
    }
    out.push_str(&s[f.src.head.clone()]);
    render_lines(old, &f.src.lines, &new, &st, &mut out);
    // the id of a heading that had no text has no space before it yet
    if s[f.src.tail.clone()].starts_with('{') {
        out.push(' ');
    }
    out.push_str(&s[f.src.tail.clone()]);
    out
}

fn render_items(old: &BlockFields, f: &Field, items: &[Item], out: &mut String) {
    let s = old.source.as_str();
    let two = format!("{0}{0}", old.eol);
    let default_gap = if f.kind == FieldKind::Image {
        two.as_str()
    } else {
        old.eol
    };
    // a new item copies the marker of a neighbour
    let model = items.iter().find_map(|i| i.src.as_ref().filter(|x| x.image.is_none()));
    for (k, item) in items.iter().enumerate() {
        if k > 0 {
            match f.src.gaps.get(k - 1).or(f.src.gaps.last()) {
                Some(g) => out.push_str(&s[g.clone()]),
                None => out.push_str(default_gap),
            }
        }
        match (
            &item.data,
            item.src.as_ref().and_then(|x| x.image.clone().map(|i| (x, i))),
        ) {
            (ItemData::Image { alt, path }, Some((x, (a, d)))) => {
                let (oa, op) = super::decode::image_value(s, &a, &d);
                out.push_str(&s[x.whole.start..a.start]);
                out.push_str(&if *alt == oa {
                    s[a.clone()].to_string()
                } else {
                    alt_source(alt)
                });
                out.push_str(&s[a.end..d.start]);
                out.push_str(&if *path == op {
                    s[d.clone()].to_string()
                } else {
                    dest_source(path)
                });
                out.push_str(&s[d.end..x.whole.end]);
            }
            (ItemData::Image { alt, path }, None) => {
                out.push_str(&format!("![{}]({})", alt_source(alt), dest_source(path)));
            }
            _ => render_list_item(old, f, item, k, model, out),
        }
    }
}

/// The marker of an item at `pos`: the old one, with its number and box brought up to date.
fn marker(old: &BlockFields, f: &Field, x: &super::decode::ItemSrc, pos: usize, correct: Option<bool>) -> String {
    let s = old.source.as_str();
    let number = f.src.numbering.map(|n| (n.start + pos as u64 * n.step).to_string());
    let task = correct.map(|c| if c { "[x]" } else { "[ ]" }.to_string());
    let mut m = String::new();
    let mut at = x.head.start;
    for (r, new) in [(&x.number, number), (&x.task_box, task)] {
        let (Some(r), Some(new)): (&Option<Range<usize>>, Option<String>) = (r, new) else {
            continue;
        };
        m.push_str(&s[at..r.start]);
        // `[X]` stays as written while it says the same, and so does `02`
        let same_number = s[r.clone()].parse::<u64>().is_ok_and(|a| new.parse::<u64>() == Ok(a));
        m.push_str(if same_number || s[r.clone()].eq_ignore_ascii_case(&new) {
            &s[r.clone()]
        } else {
            &new
        });
        at = r.end;
    }
    m.push_str(&s[at..x.head.end]);
    m
}

fn render_list_item(
    old: &BlockFields,
    f: &Field,
    item: &Item,
    pos: usize,
    model: Option<&super::decode::ItemSrc>,
    out: &mut String,
) {
    let s = old.source.as_str();
    let (text, correct, anchor) = match &item.data {
        ItemData::Text(t) => (t.as_str(), None, None),
        ItemData::Choice { text, correct, anchor } => (text.as_str(), Some(*correct), anchor.as_deref()),
        ItemData::Image { .. } => return,
    };
    let tail = item.src.as_ref().map(|x| &s[x.tail.clone()]);
    let written = match (tail, anchor) {
        (Some(t), _) => t.to_string(),
        (None, Some(a)) => format!("{{#{a}}}"),
        _ => String::new(),
    };
    let mut head = match item.src.as_ref().or(model) {
        Some(x) => marker(old, f, x, pos, correct),
        None => match correct {
            Some(true) => "- [x] ".to_string(),
            Some(false) => "- [ ] ".to_string(),
            None => "- ".to_string(),
        },
    };
    // a bare `-` needs its space once it has something to hold
    if (!text.trim().is_empty() || !written.is_empty()) && !head.ends_with([' ', '\t']) {
        head.push(' ');
    }
    out.push_str(&head);
    // the continuation lines hang under the text; under the box's `[` for an answer
    let hang = head.find('[').filter(|_| correct.is_some()).unwrap_or(head.len());
    let width = columns(&head[..hang]);
    let indent = " ".repeat(width);
    let mut new = value_lines(text);
    if new.is_empty() {
        new.push(String::new());
    }
    let lines = item.src.as_ref().map_or(&[][..], |x| &x.lines[..]);
    let esc = if correct.is_some() { esc_answer } else { esc_block };
    let st = Style {
        first: "",
        rest: &indent,
        blank: "",
        esc,
    };
    render_lines(old, lines, &new, &st, out);
    if text.trim().is_empty() {
        // the marker's blank is the one before the id
        out.push_str(written.trim_start());
    } else {
        if !written.is_empty() && !written.starts_with(' ') {
            out.push(' ');
        }
        out.push_str(&written);
    }
}

fn alt_source(alt: &str) -> String {
    let one = value_lines(alt).join(" ");
    one.replace('[', "\\[").replace(']', "\\]")
}

fn dest_source(path: &str) -> String {
    let one = value_lines(path).join(" ");
    let one = one.trim();
    if one.contains([' ', '\t', '(', ')', '<', '>']) {
        format!("<{}>", one.replace('<', "%3C").replace('>', "%3E"))
    } else {
        one.to_string()
    }
}
