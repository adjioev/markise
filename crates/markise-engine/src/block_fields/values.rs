//! The JSON face of the model, what a form reads and writes: text fields are strings, a list is
//! an array of strings, choices are `{id, text, correct}`, images `{alt, path}`. Arrays hold no
//! [`super::ItemId`]s; [`super::identity`] tells which entry is which item.

use serde_json::{Map, Value, json};

use super::identity::matched;
use super::{BlockFields, FieldEdit, FieldKind, FieldValue, FieldsError, Item, ItemData};

fn item_json(d: &ItemData) -> Value {
    match d {
        ItemData::Text(t) => json!(t),
        ItemData::Choice { anchor, text, correct } => {
            json!({"id": anchor.as_deref().unwrap_or(""), "text": text, "correct": correct})
        }
        ItemData::Image { alt, path } => json!({"alt": alt, "path": path}),
    }
}

fn parse_entry(kind: FieldKind, v: &Value) -> Option<ItemData> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match kind {
        FieldKind::Choices => v.is_object().then(|| ItemData::Choice {
            anchor: Some(s("id")).filter(|a| !a.is_empty()),
            text: s("text"),
            correct: v.get("correct").and_then(Value::as_bool).unwrap_or(false),
        }),
        FieldKind::Image => v.is_object().then(|| ItemData::Image {
            alt: s("alt"),
            path: s("path"),
        }),
        _ => v.as_str().map(|t| ItemData::Text(t.to_string())),
    }
}

impl BlockFields {
    /// The values as JSON, every field present.
    pub fn values(&self) -> Value {
        let value = |v: &FieldValue| match v {
            FieldValue::Text(t) => json!(t),
            FieldValue::Items(items) => items.iter().map(|i| item_json(&i.data)).collect(),
        };
        Value::Object(
            self.fields
                .iter()
                .map(|f| (f.name.clone(), value(&f.value)))
                .collect::<Map<_, _>>(),
        )
    }

    /// The edits that make the model hold `values`. Unknown keys are ignored, a missing key means
    /// no change, equal values give no edits.
    pub fn edits_from_values(&self, values: &Value) -> Result<Vec<FieldEdit>, FieldsError> {
        let mut out = Vec::new();
        for f in &self.fields {
            let Some(v) = values.get(&f.name) else { continue };
            let wrong = || FieldsError::WrongKind { field: f.name.clone() };
            match &f.value {
                FieldValue::Text(old) => {
                    let text = v.as_str().ok_or_else(wrong)?.to_string();
                    if text != *old {
                        out.push(FieldEdit::SetText {
                            field: f.name.clone(),
                            text,
                        });
                    }
                }
                FieldValue::Items(old) => {
                    let arr = v.as_array().ok_or_else(wrong)?;
                    let new: Option<Vec<ItemData>> = arr.iter().map(|e| parse_entry(f.kind, e)).collect();
                    items_edits(&f.name, old, &new.ok_or_else(wrong)?, &mut out);
                }
            }
        }
        Ok(out)
    }
}

fn items_edits(name: &str, old: &[Item], new: &[ItemData], out: &mut Vec<FieldEdit>) {
    let to = matched(old, new);
    let field = || name.to_string();
    for (i, o) in old.iter().enumerate() {
        if !to.contains(&Some(i)) {
            out.push(FieldEdit::RemoveItem {
                field: field(),
                item: o.id,
            });
        }
    }
    let mut order: Vec<usize> = (0..old.len()).filter(|i| to.contains(&Some(*i))).collect();
    for (pos, &i) in to.iter().flatten().enumerate() {
        if order[pos] != i {
            let from = order.iter().position(|&x| x == i).expect("a kept item");
            order.remove(from);
            order.insert(pos, i);
            out.push(FieldEdit::MoveItem {
                field: field(),
                item: old[i].id,
                to: pos,
            });
        }
    }
    for (n, e) in new.iter().enumerate() {
        let Some(i) = to[n] else {
            out.push(FieldEdit::InsertItem {
                field: field(),
                at: n,
                item: e.clone(),
            });
            continue;
        };
        let item = old[i].id;
        match (&old[i].data, e) {
            (ItemData::Text(a), ItemData::Text(b)) if a != b => {
                out.push(FieldEdit::SetItemText {
                    field: field(),
                    item,
                    text: b.clone(),
                });
            }
            (
                ItemData::Choice {
                    text: a, correct: c, ..
                },
                ItemData::Choice {
                    text: b, correct: d, ..
                },
            ) => {
                if a != b {
                    out.push(FieldEdit::SetItemText {
                        field: field(),
                        item,
                        text: b.clone(),
                    });
                }
                if c != d {
                    out.push(FieldEdit::SetCorrect {
                        field: field(),
                        item,
                        correct: *d,
                    });
                }
            }
            (ItemData::Image { alt: a, path: p }, ItemData::Image { alt: b, path: q }) if a != b || p != q => {
                out.push(FieldEdit::SetImage {
                    field: field(),
                    item,
                    alt: b.clone(),
                    path: q.clone(),
                });
            }
            _ => {}
        }
    }
}
