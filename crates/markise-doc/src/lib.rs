//! `markise-doc` — the format-neutral document seam (layer L1, below `markise-md`; ADR 0006 §1).
//!
//! Data types and traits only; no dependency on any workspace crate, no GUI:
//! - [`text`]: [`Span`], [`LineEnding`];
//! - [`patch`]: [`Patch`], [`PatchSet`] — canonical forward patches and verbatim inverses;
//! - [`pos`]: [`DocPos`] (offset + affinity + virtual slot), [`Selection`] (multi-range ready);
//! - [`hints`]: literal-line hints ([`LiteralHint`], [`HintSet`]);
//! - [`view`]: neutral views: leaves ([`LeafView`]), the flattened block structure
//!   ([`FlatEvent`]), inline trees with rendered values ([`RichInline`]), tables, headings and
//!   fragment source maps ([`SourceMap`]);
//! - [`source`]: source-mode token classes ([`SourceTok`]);
//! - [`chunked`]: [`chunked::RelChunks`], relative-offset storage (gate G1);
//! - [`resource`]: resource path resolution;
//! - [`review`]: review marks (comments and suggestions, ADR 0012) through the inline views;
//! - [`command`]: pure-data [`CommandDescriptor`]s with [`PlannerId`]s;
//! - [`plan`]: [`Plan`], [`PlanRequest`], [`TypingPlan`];
//! - [`owner`]: position → block ownership ([`BlockOwner`], [`Adjacent`], ADR 0011);
//! - [`format`]: the [`DocumentFormat`] / [`FormatDoc`] / [`EditBehavior`] traits.

pub mod chunked;
pub mod command;
pub mod format;
pub mod hints;
pub mod owner;
pub mod patch;
pub mod plan;
pub mod pos;
pub mod resource;
pub mod review;
pub mod source;
pub mod text;
pub mod view;

pub use command::{
    CommandDescriptor, DefaultKey, Effect, ParamKind, ParamSchema, ParamSpec, ParamValue, Params, PlannerId, Scope,
    UndoGroup, planners,
};
pub use format::{DepChange, DocumentFormat, EditBehavior, EditSummary, FormatDoc, FormatOptions, label_key};
pub use hints::{HintOrigin, HintSet, LiteralHint};
pub use owner::{Adjacent, BlockLeaf, BlockOwner, BlockPart, ContainerAt};
pub use patch::{Patch, PatchError, PatchSet};
pub use plan::{
    EditPrefs, Plan, PlanError, PlanRequest, PlanStep, RulePlan, SmartDashes, SmartPunctuation, TypingPlan,
    plain_typing,
};
pub use pos::{Affinity, DocPos, SelRange, Selection, VirtualSlot};
pub use source::{SourceTok, SourceToken};
pub use text::{LineEnding, Span};
pub use view::{
    CalloutKind, CodeFenceView, ColumnAlign, ContainerKind, CriticKind, FlatContainer, FlatEvent, FlatLeaf,
    HeadingInfo, InlineKind, InlineNode, InlineSource, LeafKind, LeafView, ListMarker, MapPiece, PieceKind, RichInline,
    SourceMap, TableView,
};
