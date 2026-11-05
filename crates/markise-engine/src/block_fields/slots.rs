//! The editable places of a block for in-place editing (ADR 0021): a heading, a text field, a
//! quote, a list item, an answer. A slot maps positions between the source and its value, and an
//! edit of a slot is a [`FieldEdit`] that is checked before anything is applied: it has to change
//! that value as asked and nothing else about the block.

use std::ops::Range;

use markise_doc::Patch;

use super::decode::{Line, line_end, line_start};
use super::{Applied, BlockFields, FieldEdit, FieldKind, FieldValue, FieldsError, ItemData, ItemId};

/// Which slot: lasts through an edit of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotAt {
    pub field: usize,
    pub item: Option<ItemId>,
}

/// A place to type in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub at: SlotAt,
    pub kind: FieldKind,
    lines: Vec<SlotLine>,
    /// Source ranges in it that are not plain text: what a text field swallowed.
    protected: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SlotLine {
    content: Range<usize>,
    value: String,
    /// Where the line starts in the slot's value.
    at: usize,
}

/// Why an edit does not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The position is not in plain text of a field.
    NotText,
    /// The edit would change more than the value (what it would have changed).
    Unsafe(&'static str),
    Fields(FieldsError),
}

impl From<FieldsError> for Refused {
    fn from(e: FieldsError) -> Self {
        Refused::Fields(e)
    }
}

/// Where the caret goes after an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotCaret {
    /// In the source of the new model.
    At(usize),
    /// The field has no text left, so no place in the source: a position after this offset stands
    /// for it (a virtual paragraph; it is the caller who knows which field it means).
    Absent { after: usize, field: usize },
}

/// A checked edit.
#[derive(Clone, Debug)]
pub struct SlotEdit {
    pub applied: Applied,
    pub caret: SlotCaret,
}

fn slot_lines(lines: &[Line]) -> Vec<SlotLine> {
    let mut at = 0;
    (lines.iter())
        .map(|l| {
            let line = SlotLine {
                content: l.content.clone(),
                value: l.value.clone(),
                at,
            };
            at += l.value.len() + 1;
            line
        })
        .collect()
}

/// A value line lacks only escapes of its source line: bytes of `content` up to `k` in the value.
fn to_value(content: &str, value: &str, k: usize) -> usize {
    let (c, v) = (content.as_bytes(), value.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < k.min(c.len()) {
        if j < v.len() && c[i] == v[j] {
            j += 1;
        } else if c[i] != b'\\' {
            break;
        }
        i += 1;
    }
    j
}

/// The other way round: the source position of value byte `k`. A position right after an escape's
/// backslash is not one: it goes before the backslash, or after what it escapes.
fn to_content(content: &str, value: &str, k: usize) -> usize {
    let (c, v) = (content.as_bytes(), value.as_bytes());
    let (mut i, mut j) = (0, 0);
    while j < k.min(v.len()) && i < c.len() {
        if c[i] == v[j] {
            j += 1;
        } else if c[i] != b'\\' {
            break;
        }
        i += 1;
    }
    i
}

impl Slot {
    /// From its first byte of text to its last.
    pub fn range(&self) -> Range<usize> {
        match (self.lines.first(), self.lines.last()) {
            (Some(a), Some(z)) => a.content.start..z.content.end,
            _ => 0..0,
        }
    }

    pub fn value(&self) -> String {
        (self.lines.iter())
            .map(|l| l.value.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Where the plain text of the slot starts or ends, what it holds besides aside.
    pub fn text_edge(&self, first: bool) -> Option<usize> {
        let plain = |l: &&SlotLine| !l.value.trim().is_empty() && !self.is_protected(&l.content);
        if first {
            self.lines.iter().find(plain).map(|l| l.content.start)
        } else {
            self.lines.iter().rev().find(plain).map(|l| l.content.end)
        }
    }

    /// One line of text and no more: Enter and a pasted line break cannot go in it.
    pub fn is_one_line(&self) -> bool {
        !matches!(self.kind, FieldKind::Text | FieldKind::Quote)
    }

    /// Whether `r` touches something in the slot that is not plain text.
    pub fn is_protected(&self, r: &Range<usize>) -> bool {
        (self.protected.iter()).any(|p| r.start <= p.end && p.start <= r.end)
    }

    fn line_of(&self, source: usize) -> Option<&SlotLine> {
        let i = self.lines.partition_point(|l| l.content.end < source);
        self.lines.get(i).or(self.lines.last())
    }

    /// The value offset of a source offset in the slot.
    pub fn value_offset(&self, s: &str, source: usize) -> usize {
        let Some(l) = self.line_of(source) else { return 0 };
        let k = source.saturating_sub(l.content.start).min(l.content.len());
        l.at + l
            .value
            .floor_char_boundary(to_value(&s[l.content.clone()], &l.value, k))
    }

    /// The source offset of a value offset.
    pub fn source_offset(&self, s: &str, value: usize) -> usize {
        let i = self.lines.partition_point(|l| l.at + l.value.len() < value);
        let Some(l) = self.lines.get(i).or(self.lines.last()) else {
            return 0;
        };
        let k = value.saturating_sub(l.at).min(l.value.len());
        let content = &s[l.content.clone()];
        l.content.start + content.floor_char_boundary(to_content(content, &l.value, k))
    }
}

/// What must read the same, as lines: a new line loses its leading blanks and a line of blanks is
/// empty (ADR 0020). The field's own ends do not count.
fn plain_value(v: &str) -> String {
    let v = v.replace("\r\n", "\n");
    let lines: Vec<&str> = v.split('\n').map(str::trim_start).collect();
    lines.join("\n").trim_matches('\n').to_string()
}

impl BlockFields {
    pub fn slots(&self) -> Vec<Slot> {
        let mut out = Vec::new();
        for (field, f) in self.fields.iter().enumerate() {
            match &f.value {
                FieldValue::Text(_) if !f.src.lines.is_empty() => out.push(Slot {
                    at: SlotAt { field, item: None },
                    kind: f.kind,
                    lines: slot_lines(&f.src.lines),
                    protected: f.src.protected.clone(),
                }),
                FieldValue::Items(items) if f.kind != FieldKind::Image => {
                    for i in items {
                        let Some(src) = &i.src else { continue };
                        out.push(Slot {
                            at: SlotAt {
                                field,
                                item: Some(i.id),
                            },
                            kind: f.kind,
                            lines: slot_lines(&src.lines),
                            protected: src.protected.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The slot holding source offset `at`; a slot's two ends count.
    pub fn slot_at(&self, at: usize) -> Option<Slot> {
        self.slots()
            .into_iter()
            .find(|s| s.range().contains(&at) || s.range().end == at)
    }

    pub fn slot(&self, at: SlotAt) -> Option<Slot> {
        self.slots().into_iter().find(|s| s.at == at)
    }

    /// Replaces source range `r` of slot `at` with `text`, as a change of its value. Nothing is
    /// applied here. `Ok(None)`: the value reads the same, so there is nothing to do.
    pub fn edit_slot(&self, at: SlotAt, r: Range<usize>, text: &str) -> Result<Option<SlotEdit>, Refused> {
        let slot = self.slot(at).ok_or(Refused::NotText)?;
        if slot.is_protected(&r) {
            return Err(Refused::NotText);
        }
        let s = self.source.as_str();
        let typed = if slot.is_one_line() {
            one_line(text)
        } else {
            text.replace("\r\n", "\n").replace('\r', "\n")
        };
        let value = slot.value();
        let (va, vb) = (slot.value_offset(s, r.start), slot.value_offset(s, r.end));
        let wanted = format!("{}{typed}{}", &value[..va], &value[vb..]);
        let name = self.fields[at.field].name.clone();
        let edit = match (at.item, self.fields[at.field].kind) {
            (Some(item), _) => FieldEdit::SetItemText {
                field: name,
                item,
                text: wanted.clone(),
            },
            (None, FieldKind::Heading { .. }) if plain_value(&wanted).is_empty() => {
                FieldEdit::ClearHeading { field: name }
            }
            (None, _) => FieldEdit::SetText {
                field: name,
                text: wanted.clone(),
            },
        };
        let applied = self.apply(&[edit])?;
        if applied.patches.is_empty() {
            return Ok(None);
        }
        self.check_edit(at, &r, &wanted, &applied).map_err(Refused::Unsafe)?;
        let caret = match applied.fields.slot(at) {
            Some(next) => {
                // what was typed ends here, also if the codec took blanks away around it
                let end = stored_offset(&wanted, &next.value(), va + typed.len());
                SlotCaret::At(next.source_offset(&applied.fields.source, end))
            }
            None => SlotCaret::Absent {
                after: self.end_before_field(at.field),
                field: at.field,
            },
        };
        Ok(Some(SlotEdit { applied, caret }))
    }

    /// Gives a field with no place in the source `text`, in its declared place: a field with no
    /// text at all, or what follows `sep` at the end of one that has some.
    pub fn fill_field(&self, field: usize, sep: &str, text: &str) -> Result<Option<SlotEdit>, Refused> {
        let f = self.fields.get(field).ok_or(Refused::NotText)?;
        let mut typed = text.replace("\r\n", "\n").replace('\r', "\n");
        // a new paragraph after the field's text, when it has some
        if let FieldValue::Text(old) = &f.value
            && !old.trim().is_empty()
        {
            typed = format!("{old}{sep}{typed}");
        }
        let edit = FieldEdit::SetText {
            field: f.name.clone(),
            text: typed.clone(),
        };
        let applied = self.apply(&[edit])?;
        if applied.patches.is_empty() {
            return Ok(None);
        }
        let at = SlotAt { field, item: None };
        let next = applied
            .fields
            .slot(at)
            .ok_or(Refused::Unsafe("the text did not land in its field"))?;
        self.check_rest(at, &applied).map_err(Refused::Unsafe)?;
        if plain_value(&next.value()) != plain_value(&typed) {
            return Err(Refused::Unsafe("the text does not read back"));
        }
        let end = next.value().len();
        let caret = SlotCaret::At(next.source_offset(&applied.fields.source, end));
        Ok(Some(SlotEdit { applied, caret }))
    }

    /// Enter in a list item or an answer: what follows `r` goes to a new one after it, a new answer
    /// with a new id and no mark.
    pub fn split_item(&self, at: SlotAt, r: Range<usize>) -> Result<Option<SlotEdit>, Refused> {
        let (slot, item) = self.item_slot(at)?;
        if slot.is_protected(&r) {
            return Err(Refused::NotText);
        }
        let s = self.source.as_str();
        let value = slot.value();
        let (va, vb) = (slot.value_offset(s, r.start), slot.value_offset(s, r.end));
        let (left, right) = (value[..va].trim_end().to_string(), value[vb..].trim_start().to_string());
        let f = &self.fields[at.field];
        let FieldValue::Items(items) = &f.value else {
            return Err(Refused::NotText);
        };
        let place = items.iter().position(|i| i.id == item).ok_or(Refused::NotText)?;
        let data = match items[place].data {
            ItemData::Text(_) => ItemData::Text(right.clone()),
            _ => ItemData::Choice {
                anchor: None,
                text: right.clone(),
                correct: false,
            },
        };
        let edits = [
            FieldEdit::SetItemText {
                field: f.name.clone(),
                item,
                text: left.clone(),
            },
            FieldEdit::InsertItem {
                field: f.name.clone(),
                at: place + 1,
                item: data,
            },
        ];
        let applied = self.apply(&edits)?;
        self.check_rest(at, &applied).map_err(Refused::Unsafe)?;
        let FieldValue::Items(next) = &applied.fields.fields[at.field].value else {
            return Err(Refused::Unsafe("the field changed kind"));
        };
        let mut wanted: Vec<ItemData> = items.iter().map(|i| i.data.clone()).collect();
        set_text(&mut wanted[place], &left);
        let same = next.len() == wanted.len() + 1
            && (next.iter().enumerate()).all(|(k, i)| match k.cmp(&(place + 1)) {
                std::cmp::Ordering::Less => i.data == wanted[k],
                std::cmp::Ordering::Equal => item_text(&i.data) == right && has_new_id(&i.data, &wanted),
                std::cmp::Ordering::Greater => i.data == wanted[k - 1],
            });
        if !same {
            return Err(Refused::Unsafe("the items do not read back"));
        }
        let new = SlotAt {
            field: at.field,
            item: applied.inserted.first().copied(),
        };
        let slot = applied
            .fields
            .slot(new)
            .ok_or(Refused::Unsafe("the new item is not there"))?;
        let caret = SlotCaret::At(slot.source_offset(&applied.fields.source, 0));
        Ok(Some(SlotEdit { applied, caret }))
    }

    /// Removes an empty list item or answer. Its id does not come back while this model lives.
    /// The caret goes to the end of the one before it, or with `forward` to the start of the one
    /// after it; to the other when there is none. `Ok(None)`: it has text, or it is the only one.
    pub fn remove_empty_item(&self, at: SlotAt, forward: bool) -> Result<Option<SlotEdit>, Refused> {
        let (slot, item) = self.item_slot(at)?;
        if !plain_value(&slot.value()).is_empty() {
            return Ok(None);
        }
        let f = &self.fields[at.field];
        let FieldValue::Items(items) = &f.value else {
            return Err(Refused::NotText);
        };
        let place = items.iter().position(|i| i.id == item).ok_or(Refused::NotText)?;
        if items.len() < 2 {
            return Ok(None);
        }
        let edit = FieldEdit::RemoveItem {
            field: f.name.clone(),
            item,
        };
        let applied = self.apply(&[edit])?;
        self.check_rest(at, &applied).map_err(Refused::Unsafe)?;
        let FieldValue::Items(next) = &applied.fields.fields[at.field].value else {
            return Err(Refused::Unsafe("the field changed kind"));
        };
        let same = next.len() + 1 == items.len()
            && (next.iter().enumerate()).all(|(k, i)| i.data == items[if k < place { k } else { k + 1 }].data);
        if !same {
            return Err(Refused::Unsafe("the items do not read back"));
        }
        let (forward, near) = if forward && place + 1 < items.len() || place == 0 {
            (true, place)
        } else {
            (false, place - 1)
        };
        let id = next.get(near).ok_or(Refused::NotText)?.id;
        let other = applied.fields.slot(SlotAt {
            field: at.field,
            item: Some(id),
        });
        let other = other.ok_or(Refused::Unsafe("the next item is not there"))?;
        let end = if forward { 0 } else { other.value().len() };
        let caret = SlotCaret::At(other.source_offset(&applied.fields.source, end));
        Ok(Some(SlotEdit { applied, caret }))
    }

    /// Marks or unmarks an answer. With `single` a press marks this one and clears the others, and
    /// a press on the one marked changes nothing.
    pub fn press_mark(&self, at: SlotAt, single: bool) -> Result<Option<Applied>, Refused> {
        let (_, item) = self.item_slot(at)?;
        let f = &self.fields[at.field];
        let FieldValue::Items(items) = &f.value else {
            return Err(Refused::NotText);
        };
        let marked = |i: &super::Item| matches!(i.data, ItemData::Choice { correct: true, .. });
        let mut edits = Vec::new();
        for i in items {
            let now = marked(i);
            let next = match (single, i.id == item) {
                (true, this) => this,
                (false, true) => !now,
                (false, false) => now,
            };
            if next != now {
                edits.push(FieldEdit::SetCorrect {
                    field: f.name.clone(),
                    item: i.id,
                    correct: next,
                });
            }
        }
        if edits.is_empty() {
            return Ok(None);
        }
        let applied = self.apply(&edits)?;
        self.check_rest(at, &applied).map_err(Refused::Unsafe)?;
        let FieldValue::Items(next) = &applied.fields.fields[at.field].value else {
            return Err(Refused::Unsafe("the field changed kind"));
        };
        let same = next.len() == items.len() && next.iter().zip(items).all(|(a, b)| same_but_mark(&a.data, &b.data));
        if !same {
            return Err(Refused::Unsafe("an answer's text changed"));
        }
        Ok(Some(applied))
    }

    /// Whether the slot's field marks answers.
    pub fn is_answer(&self, at: SlotAt) -> bool {
        self.fields.get(at.field).is_some_and(|f| f.kind == FieldKind::Choices)
    }

    fn item_slot(&self, at: SlotAt) -> Result<(Slot, ItemId), Refused> {
        let slot = self.slot(at).ok_or(Refused::NotText)?;
        Ok((slot, at.item.ok_or(Refused::NotText)?))
    }

    /// The end of what stands before field `k` in the block.
    fn end_before_field(&self, k: usize) -> usize {
        (self.chunks.iter())
            .take_while(|c| c.field != Some(k))
            .last()
            .map_or(self.opener_end, |c| c.range.end)
    }

    /// The whole check of a slot's edit: what it must hold before it is let through.
    fn check_edit(&self, at: SlotAt, r: &Range<usize>, wanted: &str, applied: &Applied) -> Result<(), &'static str> {
        self.check_rest(at, applied)?;
        let next = &applied.fields;
        let f = &self.fields[at.field];
        let read = match at.item {
            None => match &next.fields[at.field].value {
                FieldValue::Text(t) => t.clone(),
                _ => return Err("the field changed kind"),
            },
            Some(item) => {
                let (FieldValue::Items(a), FieldValue::Items(b)) = (&f.value, &next.fields[at.field].value) else {
                    return Err("the field changed kind");
                };
                if a.len() != b.len() {
                    return Err("items came or went");
                }
                let mut read = None;
                for (x, y) in a.iter().zip(b) {
                    if x.id == item {
                        read = Some(item_text(&y.data));
                        if !same_but_text(&x.data, &y.data) {
                            return Err("an item's id or mark changed");
                        }
                    } else if x.data != y.data {
                        return Err("another item changed");
                    }
                }
                read.ok_or("the item is gone")?
            }
        };
        if plain_value(&read) != plain_value(wanted) {
            return Err("the text does not read back as typed");
        }
        // bytes: only the lines of the edit, or a field's region when it goes away altogether
        let allowed = if plain_value(wanted).is_empty() && at.item.is_none() && next.slot(at).is_none() {
            self.end_before_field(at.field)..self.start_after_field(at.field)
        } else {
            let s = self.source.as_str();
            line_start(s, r.start)..line_end(s, r.end)
        };
        let outside = |p: &Patch| p.range.start < allowed.start || p.range.end > allowed.end;
        if applied.patches.patches().iter().any(outside) {
            return Err("bytes outside the edit would change");
        }
        Ok(())
    }

    /// What an edit of one slot may not change anywhere: the block's name and arguments, what no
    /// field took, every other field, and what its source parses as (a reference definition can
    /// form over two lines, and neither line says so alone).
    fn check_rest(&self, at: SlotAt, applied: &Applied) -> Result<(), &'static str> {
        let next = &applied.fields;
        if next.name() != self.name() || next.args() != self.args() {
            return Err("the block's opener changed");
        }
        let extras =
            |m: &BlockFields| -> Vec<String> { m.extras().into_iter().map(|r| m.source[r].to_string()).collect() };
        if extras(self) != extras(next) {
            return Err("what no field took changed");
        }
        for (k, (a, b)) in self.fields.iter().zip(&next.fields).enumerate() {
            if k != at.field && a.value != b.value && !same_items(&a.value, &b.value) {
                return Err("another field changed");
            }
        }
        // a field going away or coming back is the one change in structure that is meant
        let held = |m: &BlockFields| m.chunks.iter().any(|c| c.field == Some(at.field));
        let without = |m: &BlockFields| {
            let mut v = m.structure();
            if !(held(self) && held(next)) {
                v.retain(|x| x.1 != Some(at.field));
            }
            v
        };
        if without(self) != without(next) {
            return Err("the block would parse differently");
        }
        Ok(())
    }

    /// The start of what stands after field `k` in the block.
    fn start_after_field(&self, k: usize) -> usize {
        (self.chunks.iter())
            .skip_while(|c| c.field != Some(k))
            .skip(1)
            .find(|c| c.field != Some(k))
            .map_or(self.closer_start, |c| c.range.start)
    }
}

/// Offset `k` of `wanted` in `stored`, which reads the same but for blanks the codec took away.
/// Character by character: both sides may hold any text, and `k` is on a character of `wanted`.
fn stored_offset(wanted: &str, stored: &str, k: usize) -> usize {
    if wanted == stored {
        return k;
    }
    let mut w = wanted.char_indices().peekable();
    let mut t = stored.char_indices().peekable();
    while let Some(&(i, a)) = w.peek() {
        if i >= k {
            break;
        }
        match t.peek() {
            Some(&(_, b)) if a == b => {
                w.next();
                t.next();
            }
            // a blank the codec has and this side does not
            Some(&(_, b)) if b.is_whitespace() && !a.is_whitespace() => {
                t.next();
            }
            // what the codec took away
            _ => {
                w.next();
            }
        }
    }
    t.peek().map_or(stored.len(), |&(j, _)| j)
}

/// What goes in a field of one line: a run of line breaks is one blank.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = false;
    for c in text.chars() {
        if matches!(c, '\n' | '\r') {
            if !run {
                out.push(' ');
            }
            run = true;
        } else {
            out.push(c);
            run = false;
        }
    }
    // a line break at the end of a pasted line is no blank to keep
    if text.ends_with(['\n', '\r']) {
        out.pop();
    }
    out
}

fn set_text(d: &mut ItemData, new: &str) {
    match d {
        ItemData::Text(t) | ItemData::Choice { text: t, .. } => *t = new.to_string(),
        ItemData::Image { .. } => {}
    }
}

fn has_new_id(d: &ItemData, others: &[ItemData]) -> bool {
    match d {
        ItemData::Choice { anchor, .. } => {
            anchor.is_some() && !(others.iter()).any(|o| matches!(o, ItemData::Choice { anchor: a, .. } if a == anchor))
        }
        _ => true,
    }
}

fn same_but_mark(a: &ItemData, b: &ItemData) -> bool {
    match (a, b) {
        (ItemData::Choice { anchor: x, text: s, .. }, ItemData::Choice { anchor: y, text: t, .. }) => x == y && s == t,
        _ => false,
    }
}

fn item_text(d: &ItemData) -> String {
    match d {
        ItemData::Text(t) | ItemData::Choice { text: t, .. } => t.clone(),
        ItemData::Image { .. } => String::new(),
    }
}

fn same_but_text(a: &ItemData, b: &ItemData) -> bool {
    match (a, b) {
        (ItemData::Text(_), ItemData::Text(_)) => true,
        (
            ItemData::Choice {
                anchor: x, correct: c, ..
            },
            ItemData::Choice {
                anchor: y, correct: d, ..
            },
        ) => x == y && c == d,
        _ => false,
    }
}

/// The same items, whatever their identity: `Item`'s comparison includes where it was read from.
fn same_items(a: &FieldValue, b: &FieldValue) -> bool {
    match (a, b) {
        (FieldValue::Items(x), FieldValue::Items(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.data == q.data)
        }
        (FieldValue::Text(x), FieldValue::Text(y)) => x == y,
        _ => false,
    }
}
