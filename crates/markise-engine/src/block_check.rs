//! A document's blocks against its package, without a window: what `markise-cli blocks check`
//! reports. The reasons are those of the block's `⚠` ([`PackageViews::verdict`]), and the same
//! parser says which lines are markers, so this cannot disagree with what the app shows. It adds what
//! the app has no `⚠` for, since there the line or the block simply stays text or a card: a name
//! the package does not have and a marker that does not work.

use std::collections::HashMap;
use std::sync::Arc;

use markise_layout::BlockViews;
use markise_layout::native::MAX_NATIVE_BLOCK_BYTES;
use markise_md::{BadMarkerKind, Document, ParseOptions};
use markise_package::PackageIndex;

use crate::block_page::package_language;
use crate::block_view::{BlockReason, BlockVerdict, PackageViews};

/// What kind of problem a [`Problem`] is. The names are part of the JSON of `blocks check`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// A block with a view that has yet to be filled in, its argument included.
    NotFilled,
    /// A block its view cannot read, or one with no view that does not fit.
    Structure,
    /// A block name the package does not have.
    UnknownBlock,
    /// The package says something about the block that a form cannot honour.
    NoForm,
    /// A marker that does not pair, or a line meant as a marker that is not one.
    Marker,
}

impl ProblemKind {
    /// The name in JSON.
    pub fn name(self) -> &'static str {
        match self {
            ProblemKind::NotFilled => "not-filled",
            ProblemKind::Structure => "structure",
            ProblemKind::UnknownBlock => "unknown-block",
            ProblemKind::NoForm => "no-form",
            ProblemKind::Marker => "marker",
        }
    }

    /// The words for it, the first two as the `⚠` says them.
    pub fn headline(self) -> &'static str {
        match self {
            ProblemKind::NotFilled => "Not filled in yet",
            ProblemKind::Structure => "Outside its schema",
            ProblemKind::UnknownBlock => "Unknown block",
            ProblemKind::NoForm => "No form",
            ProblemKind::Marker => "Marker",
        }
    }
}

/// One reason. A block with three reasons is three of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The line the reason is about, from 1: of the opener, unless it is about a piece of the block.
    pub line: usize,
    /// The line of the block's opener, or of the marker.
    pub block_line: usize,
    /// The block's name; `None` for a line that is no marker.
    pub block: Option<String>,
    pub kind: ProblemKind,
    pub reason: String,
}

/// What a document's check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentCheck {
    /// The language of the package's text in the reasons.
    pub language: String,
    /// The paired blocks in the document, nested ones too.
    pub blocks: usize,
    /// In document order.
    pub problems: Vec<Problem>,
}

/// Checks documents against one package, and keeps what it has read of it between them.
pub struct BlockChecker {
    index: Arc<PackageIndex>,
    views: HashMap<String, PackageViews>,
}

impl BlockChecker {
    pub fn new(index: Arc<PackageIndex>) -> BlockChecker {
        BlockChecker {
            index,
            views: HashMap::new(),
        }
    }

    /// One parse of the document, then one of each block, a block of at most
    /// [`MAX_NATIVE_BLOCK_BYTES`] as the view reads it.
    pub fn check(&mut self, text: &str) -> DocumentCheck {
        let doc = Document::parse(text, ParseOptions::default());
        let lines = Lines::new(text);
        let language = package_language(&doc);
        let index = self.index.clone();
        let views = (self.views.entry(language.clone())).or_insert_with(|| PackageViews::new(index, &language));
        let directives = doc.directives();
        let mut problems = Vec::new();
        for d in &directives {
            let name = d.name.slice(text);
            let block_line = lines.of(d.opener.start);
            let mut add = |line: usize, kind: ProblemKind, reason: String| {
                problems.push(Problem {
                    line,
                    block_line,
                    block: Some(name.to_string()),
                    kind,
                    reason,
                });
            };
            let source = &text[d.opener.start..d.closer.end];
            if views.has_schema(name) && source.len() > MAX_NATIVE_BLOCK_BYTES {
                let limit = MAX_NATIVE_BLOCK_BYTES >> 10;
                add(
                    block_line,
                    ProblemKind::Structure,
                    format!("Over {limit} KiB, so it stays a card."),
                );
                continue;
            }
            let (kind, reasons) = match views.verdict(name, d.args.slice(text), source) {
                BlockVerdict::Fits => continue,
                BlockVerdict::NoSchema => {
                    add(
                        block_line,
                        ProblemKind::UnknownBlock,
                        format!("The package has no block `{name}`."),
                    );
                    continue;
                }
                BlockVerdict::NoForm(why) => {
                    add(block_line, ProblemKind::NoForm, why);
                    continue;
                }
                BlockVerdict::Unfilled(reasons) => (ProblemKind::NotFilled, reasons),
                BlockVerdict::Outside(reasons) => (ProblemKind::Structure, reasons),
            };
            for BlockReason { text: reason, range } in reasons {
                let line = range.map_or(block_line, |r| lines.of(d.opener.start + r.start));
                add(line, kind, reason);
            }
        }
        for bad in doc.bad_directive_markers() {
            let line = lines.of(bad.line.start);
            let shown = clipped(bad.line.slice(text));
            let (block, reason) = match bad.kind {
                BadMarkerKind::NoCloser { name } => (
                    Some(name.slice(text).to_string()),
                    format!(
                        "`{shown}` has no closer it pairs with, so it stays text. Of two blocks one inside the other, the outer needs the longer fence."
                    ),
                ),
                BadMarkerKind::NoOpener => (None, format!("`{shown}` closes nothing, so it stays text.")),
                BadMarkerKind::NotAMarker => (
                    None,
                    format!(
                        "`{shown}` is not a marker, so it stays text. A marker is three or more colons, followed directly by a name of letters, digits, `-` and `_`, then any arguments; or by nothing."
                    ),
                ),
                BadMarkerKind::NotAtTopLevel => (
                    None,
                    format!("`{shown}` is inside a quote or a list, where markers do not count, so it stays text."),
                ),
            };
            problems.push(Problem {
                line,
                block_line: line,
                block,
                kind: ProblemKind::Marker,
                reason,
            });
        }
        problems.sort_by_key(|p| (p.line, p.block_line));
        DocumentCheck {
            language,
            blocks: directives.len(),
            problems,
        }
    }
}

fn clipped(line: &str) -> String {
    let mut shown: String = line.chars().take(40).collect();
    if shown.len() < line.len() {
        shown.push('…');
    }
    shown
}

/// From a byte to its line, as an editor counts lines: `\n`, `\r\n` and `\r` end one.
struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn new(text: &str) -> Lines {
        let b = text.as_bytes();
        let mut starts = vec![0];
        for (i, &c) in b.iter().enumerate() {
            if c == b'\n' || (c == b'\r' && b.get(i + 1) != Some(&b'\n')) {
                starts.push(i + 1);
            }
        }
        Lines { starts }
    }

    fn of(&self, at: usize) -> usize {
        self.starts.partition_point(|&s| s <= at)
    }
}
