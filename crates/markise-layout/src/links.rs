//! Link targets known to the application (spec 06 §2): whether a wikilink resolves decides its
//! style (resolved links use the theme's link color, unresolved ones a muted variant).
//!
//! Cache contract: items whose source may hold a wikilink ([`crate::Item::has_wikilink`]) carry
//! [`LinkTargets::generation`] in their cache key, so a new set of files (a note created or
//! deleted) restyles exactly those items. Only colors depend on it, never heights.

/// What layout may ask about link targets. Format-neutral: targets are passed as written.
pub trait LinkTargets: std::fmt::Debug {
    /// Changes whenever any answer may change (part of the keys of items with wikilinks).
    fn generation(&self) -> u64;
    /// Whether the wikilink target `target` (trimmed; empty = the document itself) resolves to
    /// an existing file.
    fn resolves(&self, target: &str) -> bool;
}

/// No knowledge of files: every link is shown resolved (plain link style).
#[derive(Debug)]
pub struct AllResolved;

impl LinkTargets for AllResolved {
    fn generation(&self) -> u64 {
        0
    }

    fn resolves(&self, _target: &str) -> bool {
        true
    }
}
