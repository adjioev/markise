//! Display-only disclosure pairing. Only marker items are revisited after edits.
use crate::model::{DocModel, ModelChange};
use markise_doc::{FormatDoc, RawPresentation};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub end: usize,
    pub summary: Option<usize>,
    pub title: String,
    pub open: bool,
}
#[derive(Default)]
pub(crate) struct Disclosures {
    markers: BTreeMap<usize, RawPresentation>,
    pub groups: BTreeMap<usize, Group>,
    hidden: Vec<std::ops::Range<usize>>,
    controls: BTreeSet<usize>,
    depths: Vec<(usize, usize)>,
}
impl Disclosures {
    pub fn new(model: &DocModel, doc: &dyn FormatDoc) -> Self {
        let mut this = Self::default();
        for (i, item) in model.items.iter().enumerate() {
            if let Some(raw) = &item.raw {
                this.markers.insert(i, raw.clone());
            }
        }
        this.pair(model, doc);
        this
    }
    fn pair(&mut self, model: &DocModel, doc: &dyn FormatDoc) {
        let previous = std::mem::take(&mut self.groups);
        let mut stacks = BTreeMap::<Vec<usize>, Vec<usize>>::new();
        for (&i, raw) in &self.markers {
            let scope = doc
                .containers_at(model.item_start(i))
                .iter()
                .map(|c| c.span.start)
                .collect();
            let stack = stacks.entry(scope).or_default();
            match raw {
                RawPresentation::DisclosureStart { .. } => stack.push(i),
                RawPresentation::DisclosureEnd { count } if *count <= stack.len() => {
                    for _ in 0..*count {
                        let start = stack.pop().unwrap();
                        let RawPresentation::DisclosureStart { title, open } = &self.markers[&start] else {
                            unreachable!()
                        };
                        let (title, summary) = match self.markers.get(&(start + 1)) {
                            Some(RawPresentation::DisclosureSummary { title: s }) if title.is_empty() => {
                                (s.clone(), Some(start + 1))
                            }
                            _ => (title.clone(), None),
                        };
                        self.groups.insert(
                            start,
                            Group {
                                end: i,
                                summary,
                                title: if title.is_empty() { "Details".into() } else { title },
                                open: previous.get(&start).map_or(*open, |g| g.open),
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        self.refresh();
    }
    /// Rebuild the small marker index, never the document's item list.
    pub fn refresh(&mut self) {
        self.hidden.clear();
        self.controls.clear();
        let mut events = BTreeMap::<usize, isize>::new();
        for (&start, g) in &self.groups {
            self.controls.insert(g.end);
            if let Some(i) = g.summary {
                self.controls.insert(i);
            }
            if g.open {
                *events.entry(start + 1).or_default() += 1;
                *events.entry(g.end).or_default() -= 1;
            } else {
                let range = start + 1..g.end + 1;
                if let Some(last) = self.hidden.last_mut()
                    && last.end >= range.start
                {
                    last.end = last.end.max(range.end);
                } else {
                    self.hidden.push(range);
                }
            }
        }
        let mut depth = 0isize;
        self.depths = events
            .into_iter()
            .map(|(i, delta)| {
                depth += delta;
                (i, depth.max(0) as usize)
            })
            .collect();
    }
    /// Returns whether structural visibility changed (ordinary prose edits do no pairing).
    pub fn update(&mut self, model: &DocModel, doc: &dyn FormatDoc, change: &ModelChange) -> bool {
        let shift = change.inserted.len() as isize - change.replaced.len() as isize;
        let affected = self.markers.range(change.replaced.clone()).next().is_some()
            || change.inserted.clone().any(|i| model.items[i].raw.is_some());
        if shift == 0 && !affected {
            return false;
        }
        let mut next = BTreeMap::new();
        for (&i, raw) in &self.markers {
            if i < change.replaced.start {
                next.insert(i, raw.clone());
            } else if i >= change.replaced.end {
                next.insert((i as isize + shift) as usize, raw.clone());
            }
        }
        for i in change.inserted.clone() {
            if let Some(raw) = &model.items[i].raw {
                next.insert(i, raw.clone());
            }
        }
        if next == self.markers && !affected {
            return false;
        }
        self.groups = std::mem::take(&mut self.groups)
            .into_iter()
            .filter_map(|(i, mut group)| {
                if i >= change.replaced.start && i < change.replaced.end {
                    return None;
                }
                let i = if i >= change.replaced.end {
                    (i as isize + shift) as usize
                } else {
                    i
                };
                if group.end >= change.replaced.end {
                    group.end = (group.end as isize + shift) as usize;
                }
                Some((i, group))
            })
            .collect();
        self.markers = next;
        self.pair(model, doc);
        true
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
    pub fn reveal(&mut self, i: usize) -> bool {
        let mut changed = false;
        for (&start, g) in &mut self.groups {
            if start < i && i <= g.end && !g.open {
                g.open = true;
                changed = true;
            }
        }
        if changed {
            self.refresh();
        }
        changed
    }
}
