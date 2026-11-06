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
    /// A foldable callout (ADR 0025): its first leaf stays as its header and shows no
    /// `<details>` header; nothing is a control, nothing is indented.
    pub fold: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Disclosures {
    /// Groups by the item of their (materialized) start marker.
    pub groups: BTreeMap<usize, Group>,
    hidden: Vec<std::ops::Range<usize>>,
    controls: BTreeSet<usize>,
    depths: Vec<(usize, usize)>,
    /// Full derivations (tests: ordinary typing derives nothing).
    pub derived: u64,
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
                        fold: false,
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
        for f in model.cross.folds() {
            let (s, s_real) = locate(model, f.start);
            // the first item past the quote; a placeholder holding its end also holds what
            // follows, so it is not hidden
            let mut e = model.items.partition_point_start(f.end().max(f.start + 1));
            if e > s + 1
                && model.is_page(e - 1)
                && model.item_start(e - 1) + model.items.get(e - 1).map_or(0, |it| it.len) > f.end()
            {
                e -= 1;
            }
            if s_real {
                self.groups.insert(
                    s,
                    Group {
                        end: e.saturating_sub(1).max(s),
                        summary: None,
                        title: String::new(),
                        open: f.open(),
                        fold: true,
                    },
                );
            }
            if !f.open() && s + 1 < e {
                self.hidden.push(s + 1..e);
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

    /// Derives again after the disclosures changed or items holding markers came or went;
    /// `true` when what is hidden, controlled, titled or indented changed (heights must be
    /// re-estimated).
    pub fn update(&mut self, model: &DocModel) -> bool {
        let derived = self.derived + 1;
        let old = std::mem::take(self);
        self.derive(model);
        self.derived = derived;
        self.groups != old.groups
            || self.hidden != old.hidden
            || self.controls != old.controls
            || self.depths != old.depths
    }

    /// An edit that touched no marker (ordinary typing): the state follows the splices of
    /// `change` without a derivation; nothing a disclosure hides or indents changes. O(1)
    /// when no item count changed, else O(groups) index shifts.
    pub fn follow(&mut self, splices: &[(std::ops::Range<usize>, std::ops::Range<usize>)]) {
        if splices.iter().all(|(o, i)| o.len() == i.len()) || self.is_empty() {
            return;
        }
        // a boundary (an index right after a marker, or a marker) maps to its item's new index,
        // or to the new start of the splice that replaced its item (never the marker itself)
        let map = |b: usize| -> usize {
            crate::model::ModelChange::map_index(splices, b).unwrap_or_else(|| {
                let k = splices.partition_point(|(o, _)| o.end <= b);
                splices.get(k).map_or(b, |(_, n)| n.start)
            })
        };
        self.groups = std::mem::take(&mut self.groups)
            .into_iter()
            .map(|(i, mut g)| {
                g.end = map(g.end);
                g.summary = g.summary.map(map);
                (map(i), g)
            })
            .collect();
        self.controls = self.controls.iter().map(|&i| map(i)).collect();
        for r in &mut self.hidden {
            *r = map(r.start)..map(r.end);
        }
        for d in &mut self.depths {
            d.0 = map(d.0);
        }
    }

    fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.hidden.is_empty() && self.controls.is_empty() && self.depths.is_empty()
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
