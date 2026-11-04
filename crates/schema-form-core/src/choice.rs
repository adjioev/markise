//! A choice among known options, whatever draws it: what is selected in a value, what selecting
//! an option does, and how that goes back into the value. Three shapes of value hold a choice;
//! only the form's drawing is left outside this file.

use serde_json::{Value, json};

use crate::Selection;

/// One option. Its label and icon are data: `icon` is a name only the host can resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption {
    /// What the value holds for it.
    pub value: String,
    pub label: String,
    pub icon: Option<String>,
    /// Selecting it clears the others, and selecting another clears it.
    pub exclusive: bool,
}

impl ChoiceOption {
    pub(crate) fn plain(value: &str) -> ChoiceOption {
        ChoiceOption {
            value: value.to_string(),
            label: value.to_string(),
            icon: None,
            exclusive: false,
        }
    }
}

/// How a value holds a choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceShape {
    /// One string, or empty for none.
    String,
    /// An array of the strings selected.
    Strings,
    /// An array of answers, `{id, text, correct}`: an option's row is the one whose `text` is its
    /// value, and `correct` says whether it is selected.
    Marks,
}

/// What an array entry holds for an option.
fn entry_value(shape: ChoiceShape, entry: &Value) -> Option<&str> {
    match shape {
        ChoiceShape::Marks => entry.get("text").and_then(Value::as_str),
        _ => entry.as_str(),
    }
}

fn row_of(rows: &[Value], value: &str) -> Option<usize> {
    rows.iter()
        .position(|r| r.get("text").and_then(Value::as_str) == Some(value))
}

/// Per option, whether `value` selects it.
pub fn selected_options(shape: ChoiceShape, options: &[ChoiceOption], value: &Value) -> Vec<bool> {
    let rows = value.as_array().map_or(&[][..], Vec::as_slice);
    (options.iter())
        .map(|o| match shape {
            ChoiceShape::String => value.as_str() == Some(&o.value),
            ChoiceShape::Strings => rows.iter().any(|r| r.as_str() == Some(&o.value)),
            ChoiceShape::Marks => {
                row_of(rows, &o.value).is_some_and(|i| rows[i].get("correct") == Some(&Value::Bool(true)))
            }
        })
        .collect()
}

/// What pressing option `i` selects: with one selection it alone, however often it is pressed;
/// otherwise it toggles, and an exclusive option and the others clear each other.
pub fn toggle_option(options: &[ChoiceOption], selection: Selection, selected: &mut [bool], i: usize) {
    if selection == Selection::Single {
        selected.iter_mut().enumerate().for_each(|(k, s)| *s = k == i);
        return;
    }
    selected[i] = !selected[i];
    if selected[i] {
        for (k, s) in selected.iter_mut().enumerate() {
            if k != i && (options[i].exclusive || options[k].exclusive) {
                *s = false;
            }
        }
    }
}

/// The rows of an array that belong to no option: nothing a button can show, and not the
/// buttons' to change. A second row with an option's value is one too.
pub fn unknown_rows(shape: ChoiceShape, options: &[ChoiceOption], value: &Value) -> Vec<usize> {
    let Some(rows) = value.as_array().filter(|_| shape != ChoiceShape::String) else {
        return Vec::new();
    };
    let mut seen: Vec<&str> = Vec::new();
    (0..rows.len())
        .filter(|&i| match entry_value(shape, &rows[i]) {
            Some(t) if options.iter().any(|o| o.value == t) && !seen.contains(&t) => {
                seen.push(t);
                false
            }
            _ => true,
        })
        .collect()
}

/// Makes `value` select what `selected` says. Only what changes is written, so rows of no option
/// and a mark that cannot be read stay as they are. A selected option with no row gets one in the
/// options' order, without an `id` key: its host gives it one when the values are stored.
pub fn write_selected(shape: ChoiceShape, options: &[ChoiceOption], selected: &[bool], value: &mut Value) {
    if shape == ChoiceShape::String {
        let now = options.iter().zip(selected).find(|(_, s)| **s);
        *value = json!(now.map_or("", |(o, _)| o.value.as_str()));
        return;
    }
    let before = selected_options(shape, options, value);
    let Some(rows) = value.as_array_mut() else {
        return;
    };
    for (i, o) in options.iter().enumerate() {
        if before[i] == selected[i] {
            continue;
        }
        let later = |r: &Value| {
            options[i + 1..]
                .iter()
                .any(|x| Some(x.value.as_str()) == entry_value(shape, r))
        };
        let at = rows.iter().position(later).unwrap_or(rows.len());
        match shape {
            ChoiceShape::Strings if selected[i] => rows.insert(at, json!(o.value)),
            ChoiceShape::Strings => rows.retain(|r| r.as_str() != Some(&o.value)),
            _ => match row_of(rows, &o.value) {
                Some(row) => {
                    if let Some(r) = rows[row]
                        .as_object_mut()
                        .filter(|r| r.get("correct").is_none_or(Value::is_boolean))
                    {
                        r.insert("correct".into(), Value::Bool(selected[i]));
                    }
                }
                None => rows.insert(at, json!({"text": o.value, "correct": true})),
            },
        }
    }
}
