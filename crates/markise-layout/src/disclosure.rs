//! Display state of the disclosures (`<details>`) in item terms, derived from the model's
//! document-wide disclosure index ([`crate::model::CrossBlockIndex`], by source position):
//! which items are hidden by a closed disclosure, which are its controls (summary, end marker),
//! the title and state of each start marker and the nesting depth of open ones. Derived again
//! whenever the items change (edits, materialization, eviction): pairing never depends on which
//! pages of a paged model are materialized (issue #125, phase 3).
//!
//! A placeholder holding a disclosure's start or end is treated as visible (it also holds
//! content outside the disclosure); placeholders wholly inside a closed one are hidden.
use crate::model::DocModel;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    /// Item of the end marker (or the placeholder holding it).
    pub end: usize,
    /// Item of the separate summary block, if materialized.
    pub summary: Option<usize>,
    pub title: String,
    pub open: bool,
    /// Source position of the start marker (the group's identity).
    pub start_at: usize,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Disclosures {
    /// Groups by the item of their (materialized) start marker.
    pub groups: BTreeMap<usize, Group>,
    hidden: Vec<std::ops::Range<usize>>,
    controls: BTreeSet<usize>,
    depths: Vec<(usize, usize)>,
}

/// The item holding source position `at`: `(index, true)` for a real item starting there,
/// else the placeholder (or item) containing it with `false`.
fn locate(model: &DocModel, at: usize) -> (usize, bool) {
    let n = model.items.len();
    let i = model.items.partition_point_start(at);
    if i < n && model.item_start(i) == at {
        return (i, !model.is_page(i));
    }
    (i.saturating_sub(1).min(n.saturating_sub(1)), false)
}

impl Disclosures {
    pub fn new(model: &DocModel) -> Self {
        let mut d = Self::default();
        d.derive(model);
        d
    }

    /// Derives the item-index view of the model's disclosures.
    fn derive(&mut self, model: &DocModel) {
        self.groups.clear();
        self.hidden.clear();
        self.controls.clear();
        self.depths.clear();
        if model.items.is_empty() {
            return;
        }
        let mut events = BTreeMap::<usize, isize>::new();
        for g in model.cross.disclosures() {
            let (s, s_real) = locate(model, g.start);
            let (e, e_real) = locate(model, g.end);
            let summary = g.summary.map(|at| locate(model, at)).filter(|x| x.1).map(|x| x.0);
            if s_real {
                self.groups.insert(
                    s,
                    Group {
                        end: e,
                        summary,
                        title: g.title.clone(),
                        open: g.open,
                        start_at: g.start,
                    },
                );
            }
            if e_real {
                self.controls.insert(e);
            }
            if let Some(i) = summary {
                self.controls.insert(i);
            }
            if g.open {
                *events.entry(s + 1).or_default() += 1;
                *events.entry(e).or_default() -= 1;
            } else {
                // a placeholder holding the end also holds what follows it: not hidden
                let range = s + 1..if e_real { e + 1 } else { e };
                if !range.is_empty() {
                    self.hidden.push(range);
                }
            }
        }
        self.hidden.sort_by_key(|r| r.start);
        let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(self.hidden.len());
        for r in self.hidden.drain(..) {
            match merged.last_mut() {
                Some(last) if last.end >= r.start => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        self.hidden = merged;
        let mut depth = 0isize;
        self.depths = events
            .into_iter()
            .map(|(i, delta)| {
                depth += delta;
                (i, depth.max(0) as usize)
            })
            .collect();
    }

    /// Derives again after the items or the disclosures changed; `true` when what is hidden,
    /// controlled, titled or indented changed (heights must be re-estimated).
    pub fn update(&mut self, model: &DocModel) -> bool {
        let old = std::mem::take(self);
        self.derive(model);
        *self != old
    }

    pub fn hidden(&self, i: usize) -> bool {
        self.controls.contains(&i) || self.closed_end(i).is_some()
    }
    pub fn depth(&self, i: usize) -> usize {
        let at = self.depths.partition_point(|(start, _)| *start <= i);
        at.checked_sub(1).map_or(0, |j| self.depths[j].1)
    }
    pub fn closed_range(&self, i: usize) -> Option<std::ops::Range<usize>> {
        let at = self.hidden.partition_point(|r| r.start <= i);
        at.checked_sub(1)
            .and_then(|j| (i < self.hidden[j].end).then(|| self.hidden[j].clone()))
    }
    pub fn closed_end(&self, i: usize) -> Option<usize> {
        let at = self.hidden.partition_point(|r| r.start <= i);
        at.checked_sub(1)
            .and_then(|j| (i < self.hidden[j].end).then_some(self.hidden[j].end - 1))
    }
}
