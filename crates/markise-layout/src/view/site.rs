//! Blocks shown as their site draws them (ADR 0023). The opener item takes the picture's height and
//! holds it, the content items are hidden as a closed disclosure's are, and the closer stays with no
//! height, so that the gap after it is still where a caret lands. So nothing about a block's items
//! changes: pages, marks and source positions go on as they do, and a picture coming or going is a
//! change of heights. The block the selection is in shows what it holds instead of a picture.
//!
//! A document whose blocks nothing can draw pays one comparison per item.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use markise_doc::NamedContainer;

use super::View;
use super::native::{BlockViewMode, Looked};
use crate::model::{Container, Item, ItemKind};
use crate::native::{MAX_NATIVE_BLOCK_BYTES, SitePicture, SiteQuery, SiteShot};

/// What an item is to a picture.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Role {
    /// Nothing: an item as ever.
    Plain,
    /// A block's opener showing no picture: not yet, or not ever, with the reason.
    Card { key: u64, note: Option<Arc<str>> },
    /// The opener of a block showing this.
    Opener { key: u64, picture: SitePicture },
    /// Between an opener and its closer: not there, as far as anyone looking can tell.
    Inside,
    /// The closer of a block shown as a picture: no height, and the gap after it stays.
    Closer,
}

/// What item `i`'s layout gets of a picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SiteLook {
    Picture(SitePicture),
    End,
}

#[derive(Default)]
pub(super) struct SiteState {
    /// By item start, for the document and pictures as they stand.
    roles: RefCell<HashMap<usize, Role>>,
    /// The openers of the blocks around the selection's ends: those show what they hold.
    held: Vec<usize>,
}

impl SiteState {
    pub(super) fn forget(&self) {
        self.roles.borrow_mut().clear();
    }
}

impl View {
    /// What this item is to a picture. Cheap: a comparison outside cards, a lookup inside.
    pub(super) fn site_role(&self, i: usize, it: &Item) -> Role {
        if !self.native.source.as_ref().is_some_and(|s| s.has_site())
            || !it.containers.iter().any(|c| matches!(c, Container::Card))
        {
            return Role::Plain;
        }
        let start = self.model.item_start(i);
        if let Some(found) = self.site.roles.borrow().get(&start) {
            return found.clone();
        }
        let role = self.read_role(start, it);
        self.site.roles.borrow_mut().insert(start, role.clone());
        role
    }

    fn read_role(&self, start: usize, it: &Item) -> Role {
        let containers = self.doc.named_containers_at(start);
        let last = containers.len().saturating_sub(1);
        for (depth, c) in containers.iter().enumerate() {
            let own = depth == last;
            let opener = own && it.kind == (ItemKind::ContainerMarker { close: false });
            let closer = own && it.kind == (ItemKind::ContainerMarker { close: true });
            match self.shot_of(c, depth) {
                Some((key, SiteShot::Ready(picture))) | Some((key, SiteShot::Pending(Some(picture)))) => {
                    return match (opener, closer) {
                        (true, _) => Role::Opener { key, picture },
                        (_, true) => Role::Closer,
                        _ => Role::Inside,
                    };
                }
                Some((key, SiteShot::Pending(None))) if opener => return Role::Card { key, note: None },
                Some((key, SiteShot::Failed(note))) if opener => return Role::Card { key, note: Some(note) },
                _ => {}
            }
        }
        Role::Plain
    }

    /// What the host has for this block, if it is one a picture would show.
    fn shot_of(&self, c: &NamedContainer, depth: usize) -> Option<(u64, SiteShot)> {
        let source = self.native.source.as_ref()?;
        if !source.has_schema(&c.name) || self.site.held.contains(&c.opener.start) {
            return None;
        }
        let wanted = match self.native.mode_of(c.opener.start) {
            BlockViewMode::Native => !source.has_view(&c.name),
            BlockViewMode::Markdown => false,
        };
        // a card with its warning is what a block that does not fit its schema shows, and only
        // a block that fits can be expected to draw
        if !wanted || !matches!(self.look_of(c), Looked::Generic | Looked::Native(_)) {
            return None;
        }
        let bytes =
            (self.doc.text().get(c.opener.start..c.closer.end)).filter(|b| b.len() <= MAX_NATIVE_BLOCK_BYTES)?;
        let width = (self.width - 2.0 * depth as f32 * crate::block::card_padding(&self.theme)).max(40.0);
        let mut h = crate::model::hasher();
        (&c.name, &c.args, bytes).hash(&mut h);
        (width.to_bits(), (self.ppp * 100.0) as u32, self.theme_fp).hash(&mut h);
        let answer = source.site_shot(&SiteQuery {
            hash: h.finish(),
            name: &c.name,
            args: &c.args,
            source: bytes,
            opener: c.opener.start,
            width,
            scale: self.ppp,
            theme: &self.theme,
        });
        Some(answer)
    }

    /// Why the block item `i` opens shows no picture, when it failed to draw.
    pub fn block_note(&self, i: usize) -> Option<Arc<str>> {
        let it = self.model.items.get(i)?;
        match self.site_role(i, it) {
            Role::Card { note, .. } => note,
            _ => None,
        }
    }

    /// Items a picture hides.
    pub(super) fn site_hidden(&self, i: usize) -> bool {
        (self.model.items.get(i)).is_some_and(|it| self.site_role(i, it) == Role::Inside)
    }

    /// What the host is told about the block item `i` opens: it is on screen or about to be.
    pub(super) fn site_seen(&self, i: usize) {
        let Some(it) = self.model.items.get(i) else { return };
        if it.kind != (ItemKind::ContainerMarker { close: false }) {
            return;
        }
        if let (Role::Card { key, .. } | Role::Opener { key, .. }, Some(source)) =
            (self.site_role(i, it), self.native.source.as_ref())
        {
            source.site_seen(key);
        }
    }

    /// What of a picture enters the cache key of `it`.
    pub(super) fn site_item_key(&self, i: usize, it: &Item) -> u64 {
        let mut h = crate::model::hasher();
        match self.site_role(i, it) {
            Role::Plain | Role::Inside => return 0,
            Role::Card { note: None, .. } => return 0,
            Role::Card { note: Some(n), .. } => n.hash(&mut h),
            Role::Opener { picture, .. } => {
                (picture.id, picture.width.to_bits(), picture.height.to_bits()).hash(&mut h);
            }
            Role::Closer => 1u8.hash(&mut h),
        }
        h.finish().rotate_left(37)
    }

    /// What item `i`'s layout gets: a picture or its end, and the quiet reason on a card.
    pub(super) fn site_params(&self, i: usize, it: &Item) -> (Option<SiteLook>, Option<Arc<str>>) {
        match self.site_role(i, it) {
            Role::Opener { picture, .. } => (Some(SiteLook::Picture(picture)), None),
            Role::Closer => (Some(SiteLook::End), None),
            Role::Card { note, .. } => (None, note),
            Role::Plain | Role::Inside => (None, None),
        }
    }

    /// The height of a picture as shown in this item, before its gap: an old one at another width is
    /// scaled until the new one comes.
    fn shown_height(&self, picture: &SitePicture, it: &Item) -> f32 {
        let cards = it.containers.iter().filter(|c| matches!(c, Container::Card)).count();
        let inset = 2.0 * cards.saturating_sub(1) as f32 * crate::block::card_padding(&self.theme);
        let width = (self.width - inset).max(40.0);
        picture.height * width / picture.width.max(1.0)
    }

    /// The height estimate of an item a picture has to do with.
    pub(super) fn site_estimate(&self, i: usize, it: &Item) -> Option<f32> {
        match self.site_role(i, it) {
            Role::Opener { picture, .. } => Some(self.theme.block_spacing + self.shown_height(&picture, it)),
            Role::Closer => Some(0.0),
            _ => None,
        }
    }

    /// Where the block around item `i` opens, when a picture hides `i`.
    pub(super) fn site_opener_of(&self, i: usize) -> Option<usize> {
        if !self.site_hidden(i) {
            return None;
        }
        let at = self.model.item_start(i);
        let c = self.doc.named_containers_at(at).into_iter().find(|c| {
            let j = self.model.items.partition_point_start(c.opener.start);
            self.model
                .items
                .get(j)
                .is_some_and(|it| matches!(self.site_role(j, it), Role::Opener { .. }))
        })?;
        Some(self.model.items.partition_point_start(c.opener.start))
    }

    /// The host's pictures changed: one came, one went, a block failed. Only the heights around the
    /// viewport are read again; the others are corrected when they are laid out, and above the
    /// viewport that moves nothing on screen.
    pub fn site_changed(&mut self) {
        if !self.native.source.as_ref().is_some_and(|s| s.has_site()) || self.is_empty() {
            return;
        }
        let before = self.scroll_y();
        self.site.forget();
        // a viewport that starts inside a block keeps its place in it as the block becomes a picture
        let opener = self.site_opener_of(self.anchor);
        let kept = opener.map(|j| {
            let closer = (j + 1..self.len())
                .find(|&k| {
                    self.model
                        .items
                        .get(k)
                        .is_some_and(|it| self.site_role(k, it) == Role::Closer)
                })
                .unwrap_or(j);
            let top = self.heights.prefix(j);
            let all = (self.heights.prefix(closer + 1) - top).max(1.0);
            (j, closer, ((before - top) / all).clamp(0.0, 1.0))
        });
        let vh = self.paging.last_vh.max(400.0) as f64;
        let mut lo = self.item_at_y(before - 2.0 * vh);
        let mut hi = self.item_at_y(before + 4.0 * vh);
        if let Some((j, closer, _)) = kept {
            lo = lo.min(j);
            hi = hi.max(closer);
        }
        for i in lo..=hi.min(self.len() - 1) {
            let card =
                (self.model.items.get(i)).is_some_and(|it| it.containers.iter().any(|c| matches!(c, Container::Card)));
            if card && i < self.exact.len() {
                let (v, e) = self.initial_height(i);
                self.heights.set(i, v);
                self.exact[i] = e;
            }
        }
        if let Some((j, _, frac)) = kept {
            self.anchor = j;
            self.anchor_off = (frac * self.heights.get(j)) as f32;
        }
    }

    /// Where the selection's ends are: the blocks around them show what they hold, so that what is
    /// selected, found or being typed in can be seen. A selection holding a block whole does not
    /// count. Called on every change of the selection.
    pub fn set_selection_for_site(&mut self, ends: &[usize]) {
        if !self.native.source.as_ref().is_some_and(|s| s.has_site()) {
            return;
        }
        let mut held = Vec::new();
        for &at in ends {
            for c in self.doc.named_containers_at(at) {
                if c.opener.start < at && at < c.closer.end && !held.contains(&c.opener.start) {
                    held.push(c.opener.start);
                }
            }
        }
        if held != self.site.held {
            self.site.held = held;
            self.site_changed();
        }
    }
}
