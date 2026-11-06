//! Native views of named containers (ADR 0021): how a block shows when something outside this
//! crate knows what it holds. Layout knows the generic kinds below and nothing of packages,
//! schemas or any name: [`BlockViews`] is the whole interface, and a document with none is drawn
//! as it always was.

use std::ops::Range;
use std::sync::Arc;

use epaint::Color32;
use markise_theme::Theme;

use crate::block::color;

/// Blocks larger than this stay generic: a view reads the whole block, on the UI thread.
pub const MAX_NATIVE_BLOCK_BYTES: usize = 64 * 1024;

/// The generic view kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeKind {
    /// A tinted note with a bar in its accent.
    Callout,
    /// A framed block with a title.
    Card,
    /// A prompt and answers to mark.
    Choices,
    /// Pictures with their captions.
    Media,
}

/// How a block with a view looks, whatever it holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NativeView {
    pub kind: NativeKind,
    /// A colour name (see [`accent_color`]).
    pub accent: Option<Box<str>>,
    /// An icon's name; whether there is such a picture is [`BlockViews::icon_ready`]'s to say.
    pub icon: Option<Box<str>>,
    /// The label on the block's first row.
    pub label: Box<str>,
    /// One answer can be marked: answers draw as radio buttons, not check boxes.
    pub single_answer: bool,
}

/// What a part of a block is to its view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeRole {
    Title,
    Text,
    Quote,
    ListItem,
    Answer,
    Image,
}

/// A block shown natively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeBlock {
    pub view: Arc<NativeView>,
    /// The parts by source range relative to the block's start, ascending and disjoint.
    pub parts: Vec<(Range<u32>, NativeRole)>,
    /// What Practice needs of it, when its view can be practised (ADR 0024).
    pub practice: Option<PracticeParts>,
}

/// One answer of a block that can be practised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PracticeAnswer {
    /// Its text's source range, relative to the block's start.
    pub range: Range<u32>,
    pub correct: bool,
    /// Its text, for a screen reader. Not its mark.
    pub text: Box<str>,
}

/// What Practice shows and hides of a block. Only ever read by what draws Practice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PracticeParts {
    /// The block has answers to check, by its schema. If not, Practice only hides and reveals.
    pub has_choices: bool,
    pub answers: Vec<PracticeAnswer>,
    /// What stays hidden until the answers are checked, relative to the block's start.
    pub reveal: Vec<Range<u32>>,
}

impl PracticeParts {
    /// There is something to practise: answers with a mark to check them against, or with no answers
    /// to give, something to reveal.
    pub fn is_ready(&self) -> bool {
        if self.has_choices {
            self.answers.iter().any(|a| a.correct)
        } else {
            !self.reveal.is_empty()
        }
    }

    pub fn answer_at(&self, rel: u32) -> Option<usize> {
        (self.answers.iter()).position(|a| a.range.contains(&rel) || a.range.end == rel)
    }

    pub fn reveals(&self, rel: u32) -> bool {
        self.reveal.iter().any(|r| r.contains(&rel))
    }
}

/// What the layout of an item of a practised block needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PracticeItem {
    /// An answer's mark shows this and not what the file says.
    Answer {
        selected: bool,
    },
    Other,
}

impl NativeBlock {
    pub fn role_at(&self, rel: u32) -> Option<NativeRole> {
        let i = self.parts.partition_point(|(r, _)| r.end <= rel);
        self.parts.get(i).filter(|(r, _)| r.start <= rel).map(|(_, role)| *role)
    }
}

/// What a block is to its view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockLook {
    /// The generic card.
    Generic,
    /// Native with a warning: all that is wrong is that the author has yet to fill it in (an empty
    /// required field, no marked answer). The reasons are in words, the most useful first.
    Unfilled {
        reasons: Vec<String>,
        block: NativeBlock,
    },
    /// The generic card with a warning: the block's structure is outside what it declares (content
    /// in no field, an unknown variant, a shape its view cannot read). `native` is how it would show
    /// if it were not, for a block that stays native while it is being edited.
    Outside {
        reasons: Vec<String>,
        native: Option<NativeBlock>,
    },
    Native(NativeBlock),
}

/// A picture of a block, drawn by its site's code and held by a host (ADR 0023).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SitePicture {
    /// What the host draws it by.
    pub id: u64,
    /// In points, as it was drawn.
    pub width: f32,
    pub height: f32,
}

/// What is asked about a block that could show as a picture. `hash` stands for all of it but the host's own
/// inputs: the block's name, arguments and source, width, scale and theme.
pub struct SiteQuery<'a> {
    pub hash: u64,
    pub name: &'a str,
    pub args: &'a str,
    pub source: &'a str,
    /// Which block this is, through edits: its opener can move and its content change, and it stays
    /// this. An older picture of the same block can stand in for a new one; another block's cannot.
    pub identity: u64,
    pub width: f32,
    pub scale: f32,
    pub theme: &'a Theme,
}

/// What a host has for a block.
#[derive(Clone, Debug, PartialEq)]
pub enum SiteShot {
    Ready(SitePicture),
    /// Not yet; an older picture of it, if there is one, stands in.
    Pending(Option<SitePicture>),
    /// Not ever, for this version: why, in words. The block stays a card.
    Failed(Arc<str>),
}

/// What knows about blocks. Answers must be cheap: [`BlockViews::look`] is asked once per version
/// of a block on screen, with at most [`MAX_NATIVE_BLOCK_BYTES`] of source.
pub trait BlockViews: Send + Sync {
    /// Changes when the answers may: a cache key.
    fn id(&self) -> u64;
    /// Whether blocks of this name have a view at all (height estimates: no source is read).
    fn has_view(&self, name: &str) -> bool;
    /// Whether blocks of this name can be practised (ADR 0024), from the name alone. Implies a view.
    fn has_practice(&self, _name: &str) -> bool {
        false
    }
    /// Whether Practice hides the quotes of blocks of this name (page estimates: no source is read).
    fn practice_hides_quotes(&self, _name: &str) -> bool {
        false
    }
    /// Whether blocks of this name can be judged at all; the others are not read.
    fn has_schema(&self, name: &str) -> bool {
        self.has_view(name)
    }
    /// Whether the picture of this icon can be drawn. Must not block: a host that loads it off
    /// the UI thread answers `false` until then, and asks for a frame when it comes. Called for
    /// every opener on screen every frame. No such picture stays `false`, with no error.
    fn icon_ready(&self, _icon: &str) -> bool {
        false
    }
    /// Whether something can draw blocks here at all. `false` costs a comparison per item.
    fn has_site(&self) -> bool {
        false
    }
    /// What there is for this block, under a key the host names it by. Must not block: a host starts what
    /// it has to, and asks for a frame when it comes. Asked once per version of a block on screen.
    fn site_shot(&self, query: &SiteQuery<'_>) -> (u64, SiteShot) {
        let _ = query;
        (0, SiteShot::Pending(None))
    }
    /// The block with this key is on screen, or about to be. Called every frame for each, in the order
    /// they matter.
    fn site_seen(&self, _key: u64) {}
    /// How the block `name args` with this source looks. `source` runs from the first colon of
    /// its opener to the end of its closer.
    fn look(&self, name: &str, args: &str, source: &str) -> BlockLook;
}

/// What one item needs of its block's view.
#[derive(Clone, Copy, Debug)]
pub struct NativeParams<'a> {
    pub view: &'a NativeView,
    pub role: Option<NativeRole>,
    /// The view's icon can be drawn (the block's opener only).
    pub icon_ready: bool,
    /// The block is being practised.
    pub practice: Option<PracticeItem>,
}

/// The generic colour names a package can use, light then dark: a name means the same hue in every
/// theme. A name that is not here means the theme's own accent.
const ACCENTS: &[(&str, [u8; 3], [u8; 3])] = &[
    ("slate", [0x47, 0x55, 0x69], [0x94, 0xa3, 0xb8]),
    ("gray", [0x52, 0x52, 0x5b], [0xa1, 0xa1, 0xaa]),
    ("grey", [0x52, 0x52, 0x5b], [0xa1, 0xa1, 0xaa]),
    ("red", [0xc8, 0x28, 0x28], [0xf8, 0x71, 0x71]),
    ("orange", [0xc2, 0x55, 0x0c], [0xfb, 0x92, 0x3c]),
    ("amber", [0xb4, 0x6f, 0x09], [0xfb, 0xbf, 0x24]),
    ("yellow", [0xa1, 0x7c, 0x07], [0xfa, 0xcc, 0x15]),
    ("green", [0x15, 0x80, 0x3d], [0x4a, 0xde, 0x80]),
    ("teal", [0x0f, 0x76, 0x6e], [0x2d, 0xd4, 0xbf]),
    ("cyan", [0x0e, 0x74, 0x90], [0x22, 0xd3, 0xee]),
    ("blue", [0x1d, 0x4e, 0xd8], [0x60, 0xa5, 0xfa]),
    ("indigo", [0x43, 0x38, 0xca], [0x81, 0x8c, 0xf8]),
    ("purple", [0x7e, 0x22, 0xce], [0xc0, 0x84, 0xfc]),
    ("violet", [0x6d, 0x28, 0xd9], [0xa7, 0x8b, 0xfa]),
    ("pink", [0xbe, 0x18, 0x5d], [0xf4, 0x72, 0xb6]),
];

/// The colour of an accent name in this theme.
pub fn accent_color(name: Option<&str>, t: &Theme) -> Color32 {
    let found = name.and_then(|n| ACCENTS.iter().find(|(k, ..)| k.eq_ignore_ascii_case(n)));
    match found {
        Some((_, light, dark)) => {
            let [r, g, b] = if t.dark { *dark } else { *light };
            Color32::from_rgb(r, g, b)
        }
        None => color(t.chrome.accent),
    }
}

/// `c` as a tint over the page.
pub(crate) fn tint(c: Color32, t: &Theme) -> Color32 {
    let a = if t.dark { 38 } else { 26 };
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// A title's size in a native view: a heading of any level, since a level is how the file says
/// what the block's title is, not how large it is here.
pub(crate) fn title_size(t: &Theme) -> f32 {
    (t.font_size * 1.15).round()
}

/// The height of a title's line without the gap before it.
pub(crate) fn title_line(t: &Theme) -> f32 {
    title_size(t) * 1.3
}

/// How a title draws in a native view: bold body text in the view's accent for a card, the text
/// colour otherwise. Theme decoration of headings belongs to the document's own headings.
pub(crate) fn title_style(v: &NativeView, t: &Theme) -> (markise_theme::HeadingStyle, markise_theme::HeadingDecor) {
    let ink = if v.kind == NativeKind::Card {
        let [r, g, b, a] = accent_color(v.accent.as_deref(), t).to_srgba_unmultiplied();
        markise_theme::Rgba([r, g, b, a])
    } else {
        t.text
    };
    let style = markise_theme::HeadingStyle {
        size: title_size(t),
        color: ink,
        bold: true,
        margin_top: 0.0,
        margin_bottom: 0.0,
        rule: None,
    };
    (style, markise_theme::HeadingDecor::default())
}
