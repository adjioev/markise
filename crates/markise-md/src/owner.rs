//! The block-ownership query of the Markdown document (ADR 0011; the rule is documented in
//! `markise_doc::owner`): answered from the block tree — a binary search over the top-level
//! blocks, then over each level's children — never by scanning lines across blocks.

use markise_doc::{Adjacent, BlockOwner, ContainerAt, LeafView};

use crate::Document;
use crate::block::Block;
use crate::format::{container_kind, leaf_kind_at};
use crate::line_start;
use crate::text::Span;

impl Document {
    /// The leaf at block path `path` (which must end at a leaf), as a neutral view.
    pub(crate) fn leaf_view_at_path(&self, path: &[usize]) -> Option<LeafView> {
        let mut containers = Vec::new();
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
        let shift = |s: Span| Span::new(s.start + origin, s.end + origin);
        let span = shift(b.span);
        let lines: Vec<Span> = b.lines.iter().map(|l| shift(*l)).collect();
        let first = lines.first().map_or(span.start, |l| l.start);
        Some(LeafView {
            kind: leaf_kind_at(self.text(), b, origin),
            span,
            lines,
            containers,
            hinted: self.hints().contains_line(line_start(self.text(), first)),
        })
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
                return Some(Adjacent {
                    block,
                    leaf: self.leaf_view_at_path(&sib),
                });
            }
            if b.children.is_empty() {
                return Some(Adjacent { block, leaf: None });
            }
            sib.push(if forward { 0 } else { b.children.len() - 1 });
        }
    }

    /// `markise_doc::FormatDoc::block_owner` from the tree: the leaf holding `at`, else the
    /// leaf before it (whitespace ending its line) or after it (its container prefix).
    pub(crate) fn owner_from_tree(&self, at: usize) -> Option<BlockOwner> {
        let text = self.text();
        if at > text.len() {
            return None;
        }
        if let Some(path) = self.leaf_path_at(at) {
            return BlockOwner::resolve(text, self.leaf_view_at_path(&path)?, at);
        }
        let near = |forward: bool| {
            self.adjacent_from_tree(at, forward)
                .and_then(|a| a.leaf)
                .and_then(|l| BlockOwner::resolve(text, l, at))
        };
        near(false).or_else(|| near(true))
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
