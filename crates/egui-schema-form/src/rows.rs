//! The keys of array rows. They live beside the values, which stay plain JSON.

use std::collections::HashMap;

use egui::Id;
use schema_form_core::{Control, FieldSpec};
use serde_json::Value;

type Fit = dyn Fn(&Value, &Value) -> bool;

#[derive(Default)]
pub(crate) struct Rows {
    /// By the array's id, which is built from row keys and field names and no index.
    keys: HashMap<Id, Vec<u64>>,
    next: u64,
}

impl Rows {
    pub fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// The keys of an array of `len` rows; a changed length from nowhere gets repaired, so a key
    /// list and its array cannot disagree.
    pub fn keys(&mut self, array: Id, len: usize) -> Vec<u64> {
        let mut v = self.keys.remove(&array).unwrap_or_default();
        v.truncate(len);
        while v.len() < len {
            v.push(self.fresh());
        }
        self.keys.insert(array, v.clone());
        v
    }

    pub fn peek(&self, array: Id) -> Option<&[u64]> {
        self.keys.get(&array).map(Vec::as_slice)
    }

    pub fn store(&mut self, array: Id, keys: Vec<u64>) {
        self.keys.insert(array, keys);
    }

    /// Moves keys to the rows of `new`: answers by `id`, then equal content, then in order.
    pub fn reconcile(&mut self, fields: &[FieldSpec], old: &Value, new: &Value, scope: Id) {
        for f in fields {
            let (o, n) = (
                old.get(&f.key).unwrap_or(&Value::Null),
                new.get(&f.key).unwrap_or(&Value::Null),
            );
            let id = scope.with(&f.key);
            match &f.control {
                Control::Group { fields } => self.reconcile(fields, o, n, id),
                Control::StringList { .. } | Control::Array { .. } | Control::Choices { .. } | Control::Images => {
                    let (Some(oa), Some(na)) = (o.as_array(), n.as_array()) else {
                        continue;
                    };
                    let old_keys = self.keys(id, oa.len());
                    let mut used = vec![false; oa.len()];
                    let mut to: Vec<Option<usize>> = vec![None; na.len()];
                    let by_id = |x: &Value, y: &Value| {
                        x.get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|a| !a.is_empty() && y.get("id") == x.get("id"))
                    };
                    let equal = |x: &Value, y: &Value| x == y;
                    let any = |_: &Value, _: &Value| true;
                    let passes: [&Fit; 3] = [&by_id, &equal, &any];
                    for fits in passes {
                        for (k, e) in na.iter().enumerate() {
                            if to[k].is_none() {
                                to[k] = (0..oa.len()).find(|&i| !used[i] && fits(&oa[i], e));
                                if let Some(i) = to[k] {
                                    used[i] = true;
                                }
                            }
                        }
                    }
                    let keys: Vec<u64> = to
                        .iter()
                        .map(|t| t.map_or_else(|| self.fresh(), |i| old_keys[i]))
                        .collect();
                    if let Control::Array { fields } = &f.control {
                        for (k, t) in to.iter().enumerate() {
                            if let Some(i) = t {
                                self.reconcile(fields, &oa[*i], &na[k], id.with(keys[k]));
                            }
                        }
                    }
                    self.store(id, keys);
                }
                _ => {}
            }
        }
    }
}
