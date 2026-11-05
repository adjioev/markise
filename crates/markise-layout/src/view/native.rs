//! Which blocks on screen show natively (ADR 0021). A document with no [`BlockViews`] pays one
//! comparison per item key; with one, items in cards pay a map lookup, and a block's source is
//! read once per version of it on screen.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use markise_doc::NamedContainer;

use super::View;
use crate::model::{Container, Item, ItemKind};
use crate::native::{BlockLook, BlockViews, MAX_NATIVE_BLOCK_BYTES, NativeBlock, NativeParams, NativeRole};

/// More distinct block versions than this are not remembered.
const LOOKS_MAX: usize = 4096;

/// How a document shows its blocks that have a view. Ordered for [`BlockViewMode::next`]; another
/// state goes in the list below and into the two `match`es on it, and nowhere else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockViewMode {
    #[default]
    Native,
    Markdown,
}

impl BlockViewMode {
    /// The state after this one, in the order a switch goes through them.
    pub fn next(self) -> BlockViewMode {
        match self {
            BlockViewMode::Native => BlockViewMode::Markdown,
            BlockViewMode::Markdown => BlockViewMode::Native,
        }
    }
}

/// A block shown natively, around a position.
#[derive(Clone, Debug)]
pub struct NativeAt {
    pub block: Arc<NativeBlock>,
    /// From the first colon of its opener to the end of its closer.
    pub range: Range<usize>,
}

impl NativeAt {
    pub fn role_at(&self, at: usize) -> Option<NativeRole> {
        at.checked_sub(self.range.start)
            .and_then(|rel| self.block.role_at(rel as u32))
    }
}

/// What a block looks like, once read.
#[derive(Clone)]
enum Looked {
    Generic,
    Outside(Arc<[String]>, Option<Arc<NativeBlock>>),
    Native(Arc<NativeBlock>),
}

#[derive(Clone)]
struct OfItem {
    at: NativeAt,
    role: Option<NativeRole>,
    /// What of this reaches the item's layout, for its cache key.
    key: u64,
}

#[derive(Default)]
pub(super) struct NativeState {
    source: Option<Arc<dyn BlockViews>>,
    mode: BlockViewMode,
    /// By item start, for the document as it stands (see [`NativeState::forget`]).
    items: RefCell<HashMap<usize, Option<OfItem>>>,
    /// By name, arguments and source of a block: the same text looks the same anywhere.
    looks: RefCell<HashMap<u64, Looked>>,
    /// The opener of a block that stays native while it is edited, whatever it looks like now. Two
    /// blocks with the same text can look alike and only this one is it: not in [`Self::looks`].
    sticky: Option<usize>,
}

impl NativeState {
    /// The document changed, or its stand-in came during an edit.
    pub(super) fn forget(&self) {
        if self.source.is_some() {
            self.items.borrow_mut().clear();
        }
    }

    /// What shows blocks natively: not in the Markdown state.
    fn active(&self) -> Option<&Arc<dyn BlockViews>> {
        self.source.as_ref().filter(|_| self.mode == BlockViewMode::Native)
    }
}

impl View {
    /// What knows about blocks (`None`: every block is a generic card). A different one is another
    /// look for every block.
    pub fn set_block_views(&mut self, views: Option<Arc<dyn BlockViews>>) {
        let same = match (&views, &self.native.source) {
            (None, None) => true,
            (Some(a), Some(b)) => a.id() == b.id(),
            _ => false,
        };
        if same {
            return;
        }
        self.native.source = views;
        self.native.items.borrow_mut().clear();
        self.native.looks.borrow_mut().clear();
        self.reset_heights();
    }

    /// Whether blocks with a view show it. Height estimates and layouts follow.
    pub fn set_block_view_mode(&mut self, mode: BlockViewMode) {
        if mode != self.native.mode {
            self.native.mode = mode;
            self.native.items.borrow_mut().clear();
            self.reset_heights();
        }
    }

    pub fn block_view_mode(&self) -> BlockViewMode {
        self.native.mode
    }

    /// The block with this opener stays native while it is being edited, also when it does not fit
    /// for now (`None`: no such block). Only that block's items get new keys.
    pub fn set_sticky_block(&mut self, opener: Option<usize>) {
        if self.native.sticky != opener {
            self.native.sticky = opener;
            self.native.items.borrow_mut().clear();
        }
    }

    /// Part of what a measured page height holds for.
    pub(super) fn native_key(&self) -> u64 {
        self.native.active().map_or(0, |s| s.id() | 1)
    }

    /// The block shown natively around `at`, the innermost one if any.
    pub fn native_block(&self, at: usize) -> Option<NativeAt> {
        self.native.active()?;
        let c = self.doc.named_containers_at(at).pop()?;
        let block = match self.look_of(&c) {
            Looked::Native(block) => block,
            Looked::Outside(_, Some(block)) if self.native.sticky == Some(c.opener.start) => block,
            _ => return None,
        };
        Some(NativeAt {
            block,
            range: c.opener.start..c.closer.end,
        })
    }

    fn look_of(&self, c: &NamedContainer) -> Looked {
        let Some(source) = &self.native.source else {
            return Looked::Generic;
        };
        if !source.has_schema(&c.name) {
            return Looked::Generic;
        }
        let Some(bytes) =
            (self.doc.text().get(c.opener.start..c.closer.end)).filter(|b| b.len() <= MAX_NATIVE_BLOCK_BYTES)
        else {
            return Looked::Generic;
        };
        let mut h = crate::model::hasher();
        (&c.name, &c.args, bytes).hash(&mut h);
        let key = h.finish();
        if let Some(found) = self.native.looks.borrow().get(&key) {
            return found.clone();
        }
        let look = match source.look(&c.name, &c.args, bytes) {
            BlockLook::Generic => Looked::Generic,
            BlockLook::Outside { reasons, native } => Looked::Outside(reasons.into(), native.map(Arc::new)),
            BlockLook::Native(b) => Looked::Native(Arc::new(b)),
        };
        let mut looks = self.native.looks.borrow_mut();
        if looks.len() >= LOOKS_MAX {
            looks.clear();
        }
        looks.insert(key, look.clone());
        look
    }

    /// Why the block item `i` opens is outside what it declares; `None` for any other item.
    pub fn block_warnings(&self, i: usize) -> Option<Arc<[String]>> {
        self.native.source.as_ref()?;
        let it = self.model.items.get(i)?;
        if it.kind != (ItemKind::ContainerMarker { close: false }) {
            return None;
        }
        let c = self.doc.named_containers_at(self.model.item_start(i)).pop()?;
        match self.look_of(&c) {
            Looked::Outside(why, _) => Some(why),
            _ => None,
        }
    }

    fn of_item(&self, start: usize, it: &Item) -> Option<OfItem> {
        self.native.active()?;
        if !it.containers.iter().any(|c| matches!(c, Container::Card)) {
            return None;
        }
        if let Some(found) = self.native.items.borrow().get(&start) {
            return found.clone();
        }
        let found = self.native_block(start).map(|at| {
            let role = at.role_at(start);
            let mut h = crate::model::hasher();
            (&*at.block.view, role).hash(&mut h);
            OfItem {
                role,
                key: h.finish().rotate_left(23),
                at,
            }
        });
        self.native.items.borrow_mut().insert(start, found.clone());
        found
    }

    /// The block of item `i` when it shows natively.
    pub fn native_item(&self, i: usize) -> Option<NativeAt> {
        let it = self.model.items.get(i)?;
        self.of_item(self.model.item_start(i), it).map(|o| o.at)
    }

    /// What of the view enters the cache key of `it`.
    pub(super) fn native_item_key(&self, i: usize, it: &Item) -> u64 {
        if self.native.source.is_none() || !it.containers.iter().any(|c| matches!(c, Container::Card)) {
            return 0;
        }
        let of = self.of_item(self.model.item_start(i), it);
        let mut key = of.as_ref().map_or(0, |o| o.key);
        if it.kind == (ItemKind::ContainerMarker { close: false }) {
            let opener = (of.as_ref()).is_some_and(|o| self.icon_ready(o));
            key ^= u64::from(opener).rotate_left(47);
            if let Some(why) = self.block_warnings(i) {
                let mut h = crate::model::hasher();
                why.hash(&mut h);
                key ^= h.finish().rotate_left(31);
            }
        }
        key
    }

    fn icon_ready(&self, of: &OfItem) -> bool {
        let source = self.native.source.as_ref();
        (of.at.block.view.icon.as_deref()).is_some_and(|name| source.is_some_and(|s| s.icon_ready(name)))
    }

    /// Runs `f` with what item `i`'s layout gets of its view: the view and the warnings.
    pub(super) fn with_native_params<R>(
        &self,
        i: usize,
        f: impl FnOnce(Option<NativeParams<'_>>, Option<&[String]>) -> R,
    ) -> R {
        let of = (self.model.items.get(i)).and_then(|it| self.of_item(self.model.item_start(i), it));
        let why = self.block_warnings(i);
        let params = of.as_ref().map(|o| NativeParams {
            view: &o.at.block.view,
            role: o.role,
            icon_ready: self.icon_ready(o),
        });
        f(params, why.as_deref())
    }

    /// A heading in this item draws at a native title's size, as far as its block's name can tell:
    /// a block that turns out generic corrects this when it is laid out.
    pub(super) fn native_title_estimate(&self, i: usize, it: &Item) -> bool {
        let Some(source) = self.native.active() else {
            return false;
        };
        it.containers.iter().any(|c| matches!(c, Container::Card))
            && (self.doc.named_containers_at(self.model.item_start(i)).pop()).is_some_and(|c| source.has_view(&c.name))
    }

    /// Whether blocks of this name show natively, as far as the name can tell.
    pub(super) fn native_name(&self, name: &str) -> bool {
        self.native.active().is_some_and(|s| s.has_view(name))
    }
}
