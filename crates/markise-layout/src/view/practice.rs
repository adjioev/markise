//! Blocks in Practice (ADR 0024): what hides, by what a block's parts say. Nothing is known here of
//! who answered what but what the host hands over ([`View::set_block_practice`]); the file is never
//! read for it but through a block's parts. A document with no block in Practice pays one comparison.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use markise_doc::NamedContainer;

use super::View;
use super::native::{BlockViewMode, Looked, NativeAt};
use crate::model::{Container, Item, ItemKind, PageStats};
use crate::native::{NativeBlock, PracticeItem};

/// The one thing a block in Practice says about itself when it cannot be practised. The reasons it
/// would give anywhere else can tell an answer, so none of them shows.
const NOT_READY: &str = "Not ready to practice";

static NOT_READY_REASONS: LazyLock<Arc<[String]>> = LazyLock::new(|| vec![NOT_READY.to_string()].into());

/// What someone has done in a block in Practice, as the view shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PracticeShow {
    /// The answers selected, by their place among the block's answers, ascending.
    pub selected: Vec<u32>,
    pub checked: bool,
}

#[derive(Default)]
pub(super) struct PracticeState {
    /// Some block is in Practice: all the rest of this is only asked about then.
    on: bool,
    /// The document as a whole is.
    whole: bool,
    /// What was done, by a block's opener, ascending. A block with nothing done is not here.
    shows: Vec<(usize, PracticeShow)>,
    /// Whether an item hides, by its start, for the document and states as they stand.
    hidden: RefCell<HashMap<usize, bool>>,
    /// Goes up when measured page heights stop holding for what they hid.
    epoch: u64,
}

impl PracticeState {
    pub(super) fn forget(&self) {
        if self.on {
            self.hidden.borrow_mut().clear();
        }
    }

    pub(super) fn modes_changed(&mut self, mode: BlockViewMode, own: &[(usize, BlockViewMode)]) {
        self.whole = mode == BlockViewMode::Practice;
        self.on = self.whole || own.iter().any(|m| m.1 == BlockViewMode::Practice);
        self.hidden.borrow_mut().clear();
    }

    pub(super) fn key(&self) -> u64 {
        if self.on {
            (u64::from(self.whole) + 1) ^ self.epoch.rotate_left(7)
        } else {
            self.epoch.rotate_left(7)
        }
    }

    fn show_of(&self, opener: usize) -> Option<&PracticeShow> {
        (self.shows.binary_search_by_key(&opener, |s| s.0).ok()).map(|i| &self.shows[i].1)
    }
}

/// What a block in Practice is.
pub(super) enum Practiced {
    Ready(Arc<NativeBlock>),
    /// It cannot be practised as it stands: its first row and nothing of what it holds.
    NotReady,
}

impl View {
    /// Whether any block is in Practice.
    pub fn practice_on(&self) -> bool {
        self.native.practice.on
    }

    /// What `c` is, when it is in Practice.
    pub(super) fn practice_of(&self, c: &NamedContainer) -> Option<Practiced> {
        if !self.native.practice.on || self.mode_shown(c) != BlockViewMode::Practice {
            return None;
        }
        Some(match self.look_of(c) {
            Looked::Native(b) if b.practice.as_ref().is_some_and(|p| p.is_ready()) => Practiced::Ready(b),
            _ => Practiced::NotReady,
        })
    }

    /// The innermost block in Practice around `at`, with its opener: whatever else it is, its
    /// text does not change and it has no commands.
    pub fn practised_around(&self, at: usize) -> Option<NamedContainer> {
        if !self.native.practice.on {
            return None;
        }
        (self.doc.named_containers_at(at).into_iter()).rfind(|c| self.mode_shown(c) == BlockViewMode::Practice)
    }

    /// Whether the document's source range `r` hides from the reader now: Find and the caret do not go there.
    pub fn practice_conceals(&self, at: usize) -> bool {
        if !self.native.practice.on {
            return false;
        }
        self.doc
            .named_containers_at(at)
            .iter()
            .any(|c| match self.practice_of(c) {
                None => false,
                // its first row is no place for anything
                Some(Practiced::NotReady) => true,
                Some(Practiced::Ready(b)) => {
                    let rel = (at - c.opener.start) as u32;
                    let checked = self.native.practice.show_of(c.opener.start).is_some_and(|s| s.checked);
                    !checked && b.practice.as_ref().is_some_and(|p| p.reveals(rel))
                }
            })
    }

    /// The warning of a block in Practice that cannot be practised. No reason in it can tell an answer.
    pub(super) fn practice_warning(&self, c: &NamedContainer) -> Option<Arc<[String]>> {
        match self.practice_of(c)? {
            Practiced::NotReady => Some(NOT_READY_REASONS.clone()),
            Practiced::Ready(_) => None,
        }
    }

    /// What an item of a block in Practice lays out by.
    pub(super) fn practice_item(&self, at: &NativeAt, start: usize) -> Option<PracticeItem> {
        if !self.native.practice.on || self.native.mode_of(at.range.start) != BlockViewMode::Practice {
            return None;
        }
        let parts = at.block.practice.as_ref()?;
        let rel = start.checked_sub(at.range.start)? as u32;
        let show = self.native.practice.show_of(at.range.start);
        Some(match parts.answer_at(rel) {
            Some(n) => PracticeItem::Answer {
                selected: show.is_some_and(|s| s.selected.contains(&(n as u32))),
            },
            None => PracticeItem::Other,
        })
    }

    /// Whether Practice hides item `i`. Cheap: a comparison with no block in Practice, a lookup after that.
    pub(super) fn practice_hidden(&self, i: usize) -> bool {
        if !self.native.practice.on {
            return false;
        }
        let Some(it) = self.model.items.get(i) else {
            return false;
        };
        if !it.containers.iter().any(|c| matches!(c, Container::Card)) {
            return false;
        }
        let start = self.model.item_start(i);
        if let Some(&found) = self.native.practice.hidden.borrow().get(&start) {
            return found;
        }
        let found = self.read_practice_hidden(start, it);
        self.native.practice.hidden.borrow_mut().insert(start, found);
        found
    }

    fn read_practice_hidden(&self, start: usize, it: &Item) -> bool {
        self.doc
            .named_containers_at(start)
            .iter()
            .any(|c| match self.practice_of(c) {
                None => false,
                Some(Practiced::NotReady) => match it.kind {
                    ItemKind::ContainerMarker { close: false } => start != c.opener.start,
                    ItemKind::ContainerMarker { close: true } => start != c.closer.start,
                    _ => true,
                },
                Some(Practiced::Ready(b)) => {
                    let checked = self.native.practice.show_of(c.opener.start).is_some_and(|s| s.checked);
                    !checked && (b.practice.as_ref()).is_some_and(|p| p.reveals((start - c.opener.start) as u32))
                }
            })
    }

    /// How much of a page's estimate Practice hides: the quotes of blocks that reveal them, while the
    /// document is in Practice. Other fields are corrected when they are laid out, like any estimate.
    pub(super) fn practice_quotes_estimate(&self, s: &PageStats, per_line: f32) -> f32 {
        let Some(source) = self.native.source.as_ref().filter(|_| self.native.practice.whole) else {
            return 0.0;
        };
        let t = &self.theme;
        (s.card_quotes
            .iter()
            .filter(|(name, _)| source.practice_hides_quotes(name)))
        .map(|(_, q)| q.paras as f32 * t.block_spacing + q.lines_at(per_line) * t.line_px())
        .sum()
    }

    /// What was done in the blocks in Practice, by opener. Only the items of blocks that changed are
    /// laid out again; a block checked or reset has its height read again here, and the viewport stays
    /// on what it shows.
    pub fn set_block_practice(&mut self, mut shows: Vec<(usize, PracticeShow)>) {
        shows.sort_unstable_by_key(|s| s.0);
        if shows == self.native.practice.shows {
            return;
        }
        let old = std::mem::take(&mut self.native.practice.shows);
        let revealed: Vec<usize> = (old.iter().filter(|o| o.1.checked).map(|o| o.0))
            .filter(|o| !shows.iter().any(|s| s.0 == *o && s.1.checked))
            .chain(
                (shows.iter().filter(|s| s.1.checked).map(|s| s.0))
                    .filter(|s| !old.iter().any(|o| o.0 == *s && o.1.checked)),
            )
            .collect();
        // (the same blocks moved by an edit are the same blocks: their heights did not change)
        let moved = old.len() == shows.len() && old.iter().zip(&shows).all(|(a, b)| a.1 == b.1);
        self.native.practice.shows = shows;
        self.native.items.borrow_mut().clear();
        self.native.practice.hidden.borrow_mut().clear();
        if !moved {
            for opener in revealed {
                self.heights_of_block(opener);
            }
        }
    }

    /// Everything that was done is gone, on blocks anywhere in the document: what a page measured
    /// with some of them revealed no longer holds.
    pub fn practice_forgotten(&mut self) {
        self.native.practice.epoch += 1;
        self.reset_heights();
    }

    /// Reads again the heights of what the block with this opener has here, its pages excepted.
    fn heights_of_block(&mut self, opener: usize) {
        let Some(c) = (self.doc.named_containers_at(opener).into_iter()).rfind(|c| c.opener.start == opener) else {
            return;
        };
        let (a, b) = (
            self.model.items.partition_point_start(c.opener.start),
            self.model.items.partition_point_start(c.closer.end),
        );
        for i in a..b.min(self.exact.len()) {
            if !self.model.is_page(i) {
                let (v, e) = self.initial_height(i);
                self.heights.set(i, v);
                self.exact[i] = e;
            }
        }
        // a viewport that started in what has just hidden goes on from what comes after it
        self.scroll_by(0.0);
    }
}
