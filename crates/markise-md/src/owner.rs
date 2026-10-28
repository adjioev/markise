//! The block-ownership query of the Markdown document (ADR 0011; the rule is documented in
//! `markise_doc::owner`): answered from the block tree — a binary search over the top-level
//! blocks, then over each level's children — never by scanning lines across blocks.

use markise_doc::{Adjacent, BlockLeaf, BlockOwner, ContainerAt, FormatDoc, LeafView};

use crate::Document;
use crate::block::Block;
use crate::format::{container_kind, leaf_kind_at};
use crate::line_start;
use crate::text::Span;

impl Document {
    /// The leaf at block path `path` (which must end at a leaf) without its lines, with the
    /// block (spans relative to the returned origin). O(depth); the block's lines are not
    /// copied (small allocations: the container kinds, a code block's info string).
    fn leaf_head_at_path(&self, path: &[usize]) -> Option<(BlockLeaf, &Block, usize)> {
        let mut containers = Vec::with_capacity(path.len().saturating_sub(1));
        for depth in 1..path.len() {
            let (b, _) = self.block_at_path_rel(&path[..depth])?;
            if !b.is_container() {
                return None;
            }
            containers.push(container_kind(b));
        }
        let (b, origin) = self.block_at_path_rel(path)?;
        if b.is_container() {
            return None;
        }
        let span = Span::new(b.span.start + origin, b.span.end + origin);
        let first = b.lines.first().map_or(span.start, |l| l.start + origin);
        Some((
            BlockLeaf {
                kind: leaf_kind_at(self.text(), b, origin),
                span,
                containers,
                hinted: self.hints().contains_line(line_start(self.text(), first)),
            },
            b,
            origin,
        ))
    }

    /// The leaf at block path `path` (which must end at a leaf), as a full neutral view (its
    /// lines copied: O(lines)).
    pub(crate) fn leaf_view_at_path(&self, path: &[usize]) -> Option<LeafView> {
        let (head, b, origin) = self.leaf_head_at_path(path)?;
        Some(LeafView {
            kind: head.kind,
            span: head.span,
            lines: b
                .lines
                .iter()
                .map(|l| Span::new(l.start + origin, l.end + origin))
                .collect(),
            containers: head.containers,
            hinted: head.hinted,
        })
    }

    /// The ownership answer for the leaf at `path` (no line copies).
    fn owner_at_path(&self, path: &[usize], at: usize) -> Option<BlockOwner> {
        let (head, b, origin) = self.leaf_head_at_path(path)?;
        BlockOwner::resolve_lines(self.text(), head, &b.lines, origin, at)
    }

    /// The path to the innermost leaf holding `at` (end inclusive), if `at` is inside one.
    fn leaf_path_at(&self, at: usize) -> Option<Vec<usize>> {
        let path = self.block_path_at(at);
        let (b, _) = self.block_at_path_rel(&path)?;
        if b.is_container() {
            return None;
        }
        Some(path)
    }

    /// Number of children of the container at `path` (the top level for an empty path).
    fn child_count(&self, path: &[usize]) -> usize {
        if path.is_empty() {
            return self.top.len();
        }
        self.block_at_path_rel(path).map_or(0, |(b, _)| b.children.len())
    }

    /// `markise_doc::FormatDoc::adjacent_block` from the tree.
    pub(crate) fn adjacent_from_tree(&self, at: usize, forward: bool) -> Option<Adjacent> {
        let (block, leaf) = self.adjacent_path(at, forward)?;
        Some(Adjacent {
            block,
            leaf: leaf.and_then(|p| self.leaf_head_at_path(&p)).map(|(h, _, _)| h),
        })
    }

    /// The neighbour of `at` as (its span, the path to the leaf at its near edge).
    fn adjacent_path(&self, at: usize, forward: bool) -> Option<(Span, Option<Vec<usize>>)> {
        let mut path = self.block_path_at(at);
        let in_leaf = self.block_at_path_rel(&path).is_some_and(|(b, _)| !b.is_container());
        // between the children of the innermost container holding `at` (or the top level)
        let mut sib = None;
        if !in_leaf {
            let idx = if path.is_empty() {
                self.top.partition_point(|t| t.end() < at)
            } else {
                let (c, o) = self.block_at_path_rel(&path)?;
                c.children.partition_point(|b| b.span.end + o < at)
            };
            let n = self.child_count(&path);
            let i = if forward {
                (idx < n).then_some(idx)
            } else {
                idx.checked_sub(1)
            };
            if let Some(i) = i {
                let mut p = path.clone();
                p.push(i);
                sib = Some(p);
            }
        }
        // else: the nearest level with a sibling in that direction
        let mut sib = match sib {
            Some(s) => s,
            None => loop {
                let last = *path.last()?;
                let parent = &path[..path.len() - 1];
                let i = if forward {
                    Some(last + 1).filter(|&i| i < self.child_count(parent))
                } else {
                    last.checked_sub(1)
                };
                match i {
                    Some(i) => {
                        *path.last_mut()? = i;
                        break path;
                    }
                    None => {
                        path.pop();
                    }
                }
            },
        };
        let (b, o) = self.block_at_path_rel(&sib)?;
        let block = Span::new(b.span.start + o, b.span.end + o);
        // down to the leaf at its near edge
        loop {
            let (b, _) = self.block_at_path_rel(&sib)?;
            if !b.is_container() {
                return Some((block, Some(sib)));
            }
            if b.children.is_empty() {
                return Some((block, None));
            }
            sib.push(if forward { 0 } else { b.children.len() - 1 });
        }
    }

    /// `markise_doc::FormatDoc::block_owner` from the tree: the leaf holding `at`, else the
    /// leaf before it (whitespace ending its line) or after it (its container prefix).
    pub(crate) fn owner_from_tree(&self, at: usize) -> Option<BlockOwner> {
        if at > self.text().len() {
            return None;
        }
        if let Some(path) = self.leaf_path_at(at) {
            return self.owner_at_path(&path, at);
        }
        let near = |forward: bool| {
            self.adjacent_path(at, forward)
                .and_then(|(_, p)| p)
                .and_then(|p| self.owner_at_path(&p, at))
        };
        near(false).or_else(|| near(true))
    }

    /// `markise_doc::FormatDoc::block_owner_biased` from the tree (no line copies).
    pub(crate) fn owner_biased_from_tree(&self, at: usize, bias: markise_doc::Affinity) -> Option<BlockOwner> {
        if let Some(o) = self.owner_from_tree(at) {
            return Some(o);
        }
        let path = self.adjacent_path(at, bias == markise_doc::Affinity::Downstream)?.1?;
        let (head, b, origin) = self.leaf_head_at_path(&path)?;
        let edge = match bias {
            markise_doc::Affinity::Upstream => head.span.end,
            markise_doc::Affinity::Downstream => head.span.start,
        };
        let mut o = BlockOwner::resolve_lines(self.text(), head, &b.lines, origin, edge)?;
        o.at = at;
        o.part = markise_doc::BlockPart::Gap;
        Some(o)
    }

    /// The full view (lines included, O(lines)) of the leaf owning `at`, or of the leaf on the
    /// `bias` side of a gap: for planners that work on the leaf's lines.
    pub(crate) fn owner_leaf_view(&self, at: usize, bias: markise_doc::Affinity) -> Option<LeafView> {
        let o = self.owner_biased_from_tree(at, bias)?;
        self.leaf(o.leaf.span.start)
    }

    /// `markise_doc::FormatDoc::containers_at` from the tree.
    pub(crate) fn containers_from_tree(&self, at: usize) -> Vec<ContainerAt> {
        let path = self.block_path_at(at);
        (1..=path.len())
            .filter_map(|d| self.block_at_path_rel(&path[..d]))
            .filter(|(b, _): &(&Block, usize)| b.is_container())
            .map(|(b, o)| ContainerAt {
                kind: container_kind(b),
                span: Span::new(b.span.start + o, b.span.end + o),
            })
            .collect()
    }
}
