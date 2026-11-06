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
use crate::native::{
    BlockLook, BlockViews, MAX_NATIVE_BLOCK_BYTES, NativeBlock, NativeParams, NativeRole, PracticeItem,
};

/// More distinct block versions than this are not remembered.
const LOOKS_MAX: usize = 4096;

/// How a document shows its blocks that have a view. Ordered for [`BlockViewMode::next`]; another
/// state goes in the list below and into the two `match`es on it, and nowhere else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockViewMode {
    /// A block's own view, or its site's picture if it has none.
    #[default]
    Native,
    /// Answers to give, with what tells them hidden: a block that can be practised, in a document in
    /// Read (ADR 0024). No state a block can be switched to.
    Practice,
    /// As its site draws it (ADR 0023).
    Site,
    Markdown,
}

impl BlockViewMode {
    /// The state after this one, in the order a switch goes through them.
    pub fn next(self) -> BlockViewMode {
        match self {
            BlockViewMode::Native => BlockViewMode::Site,
            // no switch goes through it: it comes with Read (ADR 0024)
            BlockViewMode::Practice => BlockViewMode::Native,
            BlockViewMode::Site => BlockViewMode::Markdown,
            BlockViewMode::Markdown => BlockViewMode::Native,
        }
    }

    /// What a block shows in this state, as far as what it can show goes.
    pub fn shown(self, can: BlockCan) -> BlockViewMode {
        match self {
            BlockViewMode::Native | BlockViewMode::Practice if can.view => BlockViewMode::Native,
            BlockViewMode::Site if can.site => BlockViewMode::Site,
            BlockViewMode::Native | BlockViewMode::Practice if can.site => BlockViewMode::Site,
            _ => BlockViewMode::Markdown,
        }
    }

    /// The next state that shows something else, for a block that can show what `can` says. A block
    /// that can show nothing else has only one state, and this is it.
    pub fn next_for(self, can: BlockCan) -> BlockViewMode {
        let now = self.shown(can);
        let mut next = now.next();
        while next != now && next.shown(can) != next {
            next = next.next();
        }
        next
    }
}

/// What a block can show, besides its Markdown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockCan {
    /// It has a view of its own.
    pub view: bool,
    /// Something can draw it.
    pub site: bool,
}

/// What an item's layout gets of its block.
pub(super) struct ItemLook<'a> {
    pub native: Option<NativeParams<'a>>,
    pub warnings: Option<&'a [String]>,
    pub site: Option<super::site::SiteLook>,
    pub note: Option<&'a str>,
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
pub(super) enum Looked {
    Generic,
    Unfilled(Arc<[String]>, Arc<NativeBlock>),
    Outside(Arc<[String]>, Option<Arc<NativeBlock>>),
    Native(Arc<NativeBlock>),
}

#[derive(Clone)]
pub(super) struct OfItem {
    at: NativeAt,
    role: Option<NativeRole>,
    practice: Option<PracticeItem>,
    /// What of this reaches the item's layout, for its cache key.
    key: u64,
}

#[derive(Default)]
pub(super) struct NativeState {
    pub(super) source: Option<Arc<dyn BlockViews>>,
    /// How the document shows its blocks …
    mode: BlockViewMode,
    /// … and the blocks that differ from it, by opener, ascending.
    own_modes: Vec<(usize, BlockViewMode)>,
    /// By item start, for the document as it stands (see [`NativeState::forget`]).
    pub(super) items: RefCell<HashMap<usize, Option<OfItem>>>,
    /// By name, arguments and source of a block: the same text looks the same anywhere.
    looks: RefCell<HashMap<u64, Looked>>,
    /// The opener of a block that stays native while it is edited, whatever it looks like now. Two
    /// blocks with the same text can look alike and only this one is it: not in [`Self::looks`].
    sticky: Option<usize>,
    /// Which blocks are in Practice and what they show ([`super::practice`]).
    pub(super) practice: super::practice::PracticeState,
}

impl NativeState {
    /// The document changed, or its stand-in came during an edit.
    pub(super) fn forget(&self) {
        if self.source.is_some() {
            self.items.borrow_mut().clear();
            self.practice.forget();
        }
    }

    /// What shows blocks natively, as far as the document's state can tell: for what is estimated
    /// by name, with no block read.
    fn active(&self) -> Option<&Arc<dyn BlockViews>> {
        self.source
            .as_ref()
            .filter(|_| self.mode == BlockViewMode::Native || self.practice.read())
    }

    /// The state of a block with this opener and name. In Read, one that can be practised is, whatever
    /// else it is in Edit (ADR 0024).
    pub(super) fn mode_of(&self, opener: usize, name: &str) -> BlockViewMode {
        if self.practice.read() && self.source.as_ref().is_some_and(|s| s.has_practice(name)) {
            return BlockViewMode::Practice;
        }
        match self.own_modes.binary_search_by_key(&opener, |m| m.0) {
            Ok(i) => self.own_modes[i].1,
            Err(_) => self.mode,
        }
    }

    fn practice_source_changed(&mut self) {
        let any = self.source.as_ref().is_some_and(|s| s.any_practice());
        self.practice.package_changed(any);
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
        self.native.practice_source_changed();
        self.site.forget();
        self.reset_heights();
    }

    /// How blocks with a view show, for the whole document. Height estimates and layouts follow.
    pub fn set_block_view_mode(&mut self, mode: BlockViewMode) {
        self.set_block_modes(mode, Vec::new());
    }

    /// How blocks with a view show: `mode` for the whole document, and the blocks that differ from
    /// it by opener. A block's own state lasts through edits above it by the caller moving it
    /// here: that alone costs what any edit does.
    pub fn set_block_modes(&mut self, mode: BlockViewMode, mut own: Vec<(usize, BlockViewMode)>) {
        own.sort_unstable_by_key(|m| m.0);
        if mode == self.native.mode && own == self.native.own_modes {
            return;
        }
        let same_states = mode == self.native.mode
            && own.len() == self.native.own_modes.len()
            && (own.iter().zip(&self.native.own_modes)).all(|(a, b)| a.1 == b.1);
        self.native.mode = mode;
        self.native.own_modes = own;
        self.native.items.borrow_mut().clear();
        self.site.forget();
        if !same_states {
            self.reset_heights();
        }
    }

    /// Whether something can draw a block here at all.
    pub fn can_draw_site(&self) -> bool {
        self.native.source.as_ref().is_some_and(|s| s.has_site())
    }

    /// The state the block with this opener is in.
    pub fn block_mode_of(&self, opener: usize) -> BlockViewMode {
        let around = self.doc.named_containers_at(opener);
        let name = around
            .iter()
            .rfind(|c| c.opener.start == opener)
            .map_or("", |c| &*c.name);
        self.native.mode_of(opener, name)
    }

    pub fn block_view_mode(&self) -> BlockViewMode {
        self.native.mode
    }

    /// The state the block `c` is in, as far as it can be: Practice is for blocks that can be practised,
    /// and any other block in a document in Practice is as it is outside it.
    pub(super) fn mode_shown(&self, c: &NamedContainer) -> BlockViewMode {
        match self.native.mode_of(c.opener.start, &c.name) {
            BlockViewMode::Practice if !self.native.source.as_ref().is_some_and(|s| s.has_practice(&c.name)) => {
                BlockViewMode::Native
            }
            mode => mode,
        }
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
            ^ (self.native.own_modes.len() as u64).rotate_left(53)
            ^ self.native.practice.key()
    }

    /// The block shown natively around `at`, the innermost one if any.
    pub fn native_block(&self, at: usize) -> Option<NativeAt> {
        self.native.source.as_ref()?;
        let c = self.doc.named_containers_at(at).pop()?;
        let mode = self.mode_shown(&c);
        let block = match (mode, self.look_of(&c)) {
            (BlockViewMode::Markdown, _) => return None,
            // in Practice a block that is not ready to be practised shows nothing of itself
            (BlockViewMode::Practice, Looked::Native(block))
                if block.practice.as_ref().is_some_and(|p| p.is_ready()) =>
            {
                block
            }
            (BlockViewMode::Practice, _) => return None,
            // not filled in yet has no look on any site (ADR 0021)
            (_, Looked::Unfilled(_, block)) => block,
            // being edited natively in the Site state, and it has come to fit: it stays what it was
            // until the caret leaves, as any block being edited does
            (BlockViewMode::Site, Looked::Native(block)) if self.native.sticky == Some(c.opener.start) => block,
            (BlockViewMode::Native, Looked::Native(block)) => block,
            (BlockViewMode::Native, Looked::Outside(_, Some(block))) if self.native.sticky == Some(c.opener.start) => {
                block
            }
            _ => return None,
        };
        Some(NativeAt {
            block,
            range: c.opener.start..c.closer.end,
        })
    }

    pub(super) fn look_of(&self, c: &NamedContainer) -> Looked {
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
            BlockLook::Unfilled { reasons, block } => Looked::Unfilled(reasons.into(), Arc::new(block)),
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

    /// What is wrong with the block item `i` opens, whether it is yet to be filled in or outside
    /// what it declares; `None` for any other item.
    pub fn block_warnings(&self, i: usize) -> Option<Arc<[String]>> {
        self.native.source.as_ref()?;
        let it = self.model.items.get(i)?;
        if it.kind != (ItemKind::ContainerMarker { close: false }) {
            return None;
        }
        let c = self.doc.named_containers_at(self.model.item_start(i)).pop()?;
        if self.practice_of(&c).is_some() {
            return self.practice_warning(&c);
        }
        match self.look_of(&c) {
            Looked::Outside(why, _) | Looked::Unfilled(why, _) => Some(why),
            _ => None,
        }
    }

    /// The block item `i` opens is a card whatever its state: its structure is outside what it
    /// declares. A block that is only yet to be filled in is not.
    pub fn block_outside(&self, i: usize) -> bool {
        (self.native.source.is_some() && self.model.items.get(i).is_some())
            && self.model.items[i].kind == (ItemKind::ContainerMarker { close: false })
            && (self.doc.named_containers_at(self.model.item_start(i)).pop())
                .is_some_and(|c| self.practice_of(&c).is_none() && matches!(self.look_of(&c), Looked::Outside(..)))
    }

    fn of_item(&self, start: usize, it: &Item) -> Option<OfItem> {
        self.native.source.as_ref()?;
        if !it.containers.iter().any(|c| matches!(c, Container::Card)) {
            return None;
        }
        if let Some(found) = self.native.items.borrow().get(&start) {
            return found.clone();
        }
        let found = self.native_block(start).map(|at| {
            let role = at.role_at(start);
            let practice = self.practice_item(&at, start, it);
            let mut h = crate::model::hasher();
            (&*at.block.view, role, practice).hash(&mut h);
            OfItem {
                role,
                practice,
                key: h.finish().rotate_left(23),
                at,
            }
        });
        self.native.items.borrow_mut().insert(start, found.clone());
        found
    }

    /// The block of item `i` when it shows natively. Not one a picture stands for.
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
        let mut key = of.as_ref().map_or(0, |o| o.key) ^ self.site_item_key(i, it);
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
    pub(super) fn with_native_params<R>(&self, i: usize, f: impl FnOnce(ItemLook<'_>) -> R) -> R {
        let it = self.model.items.get(i);
        let of = it.and_then(|it| self.of_item(self.model.item_start(i), it));
        let why = self.block_warnings(i);
        let params = of.as_ref().map(|o| NativeParams {
            view: &o.at.block.view,
            role: o.role,
            icon_ready: self.icon_ready(o),
            practice: o.practice,
        });
        let (site, note) = it.map_or((None, None), |it| self.site_params(i, it));
        f(ItemLook {
            native: params,
            warnings: why.as_deref(),
            site,
            note: note.as_deref(),
        })
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

    /// The opener just above the first item on screen, when it has no label row and its block's
    /// controls are still in view. Such an opener has next to no height, and the controls stand on it
    /// over the next row, so it has scrolled away long before they have. A host finds them by the opener.
    pub(super) fn quiet_opener_above(
        &mut self,
        first: &super::Visible,
        shaper: &mut dyn crate::text::TextShaper,
        images: &dyn crate::block::Images,
    ) -> Option<super::Visible> {
        let i = first.idx.checked_sub(1)?;
        let it = self.model.items.get(i)?;
        if it.kind != (ItemKind::ContainerMarker { close: false }) || self.disclosure_hidden(i) || self.site_hidden(i) {
            return None;
        }
        if self.native_item(i)?.block.view.label_row {
            return None;
        }
        let layout = self.layout(i, shaper, images);
        let (pad, _) = self.item_pad(i);
        let y = first.y - self.height(i) + pad;
        let (edge, _) = layout.card_top?;
        (layout.warning.is_none() && y + edge.y + crate::block::card_controls_reach(&self.theme) > 0.0)
            .then_some(super::Visible { idx: i, y, layout })
    }

    /// The opener of item `i` has no label row, as far as its block's name can tell: a block with a
    /// warning has one after all, and corrects this when it is laid out.
    pub(super) fn native_quiet_opener(&self, i: usize) -> bool {
        (self.doc.named_containers_at(self.model.item_start(i)).pop()).is_some_and(|c| self.native_quiet_name(&c.name))
    }

    /// Blocks of this name show natively with no label row, as far as the name can tell.
    pub(super) fn native_quiet_name(&self, name: &str) -> bool {
        self.native
            .active()
            .is_some_and(|s| s.has_view(name) && !s.label_row(name))
    }

    /// Whether blocks of this name show natively, as far as the name can tell.
    pub(super) fn native_name(&self, name: &str) -> bool {
        self.native.active().is_some_and(|s| s.has_view(name))
    }
}
