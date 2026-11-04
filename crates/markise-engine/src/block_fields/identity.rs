//! Which item is which when items come without their [`ItemId`]s: answers by `{#id}`, then
//! anything by equal content, the rest in order. Equal items are interchangeable.

use super::{BlockFields, FieldValue, Item, ItemData, ItemId};

type Fit = dyn Fn(&ItemData, &ItemData) -> bool;

fn anchor(d: &ItemData) -> Option<&str> {
    match d {
        ItemData::Choice { anchor, .. } => anchor.as_deref(),
        _ => None,
    }
}

fn same_content(old: &ItemData, new: &ItemData) -> bool {
    match (old, new) {
        (
            ItemData::Choice {
                text: x, correct: c, ..
            },
            ItemData::Choice {
                anchor,
                text: y,
                correct: d,
            },
        ) => x == y && c == d && (anchor.is_none() || anchor.as_deref() == self::anchor(old)),
        _ => old == new,
    }
}

/// An unnamed answer takes over only an answer without an id; anything else takes its own kind.
fn may_replace(old: &ItemData, new: &ItemData) -> bool {
    anchor(new).is_none()
        && match (old, new) {
            (ItemData::Choice { anchor, .. }, ItemData::Choice { .. }) => anchor.is_none(),
            _ => std::mem::discriminant(old) == std::mem::discriminant(new),
        }
}

/// For each of `new`, the index of its item in `old`.
pub(super) fn matched(old: &[Item], new: &[ItemData]) -> Vec<Option<usize>> {
    let mut to: Vec<Option<usize>> = vec![None; new.len()];
    let mut used = vec![false; old.len()];
    let by_anchor = |o: &ItemData, n: &ItemData| anchor(o).is_some() && anchor(o) == anchor(n);
    let passes: [&Fit; 3] = [&by_anchor, &same_content, &may_replace];
    for fits in passes {
        for (n, e) in new.iter().enumerate() {
            if to[n].is_none() {
                to[n] = (0..old.len()).find(|&i| !used[i] && fits(&old[i].data, e));
                if let Some(i) = to[n] {
                    used[i] = true;
                }
            }
        }
    }
    to
}

/// Gives the items of `next` the identities of their counterparts in `prev`; new ones get ids
/// past every id `prev` knew.
pub(super) fn carry_ids(prev: &BlockFields, next: &mut BlockFields) {
    let mut fresh = prev.next_item.max(next.next_item);
    for f in &mut next.fields {
        let FieldValue::Items(items) = &mut f.value else {
            continue;
        };
        let old = match prev.field(&f.name).map(|p| &p.value) {
            Some(FieldValue::Items(old)) => old.as_slice(),
            _ => &[],
        };
        let datas: Vec<ItemData> = items.iter().map(|i| i.data.clone()).collect();
        for (item, hit) in items.iter_mut().zip(matched(old, &datas)) {
            item.id = hit.map_or_else(
                || {
                    fresh += 1;
                    ItemId(fresh - 1)
                },
                |i| old[i].id,
            );
        }
    }
    next.next_item = fresh;
}
