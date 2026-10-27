//! Plans: what a planner returns (ADR 0006 §1.2, §2.1). Planners never mutate; the engine
//! applies a plan's steps in order inside one transaction.

use std::fmt;

use crate::command::Params;
use crate::hints::HintSet;
use crate::patch::{PatchError, PatchSet};
use crate::pos::Selection;

/// Smart punctuation while typing (spec 02 §13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SmartPunctuation {
    /// Off (the default).
    #[default]
    Off,
    /// ASCII quotes and dashes are replaced while typing (saved as Unicode).
    ConvertOnInput,
}

/// Dash conversion of smart punctuation (spec 02 §13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SmartDashes {
    /// `--` → en dash, `---` → em dash (the default).
    #[default]
    EnEm,
    /// `--` → em dash.
    Em,
}

/// Editing preferences a planner may consult (spec 02 §4, §13, §16; format-neutral).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditPrefs {
    /// "Auto pair brackets and quotes" (default on).
    pub auto_pair_brackets: bool,
    /// "Auto pair common Markdown syntax" (default off).
    pub auto_pair_markdown: bool,
    pub smart_punctuation: SmartPunctuation,
    pub smart_dashes: SmartDashes,
    /// Quote characters used by smart quotes: opening/closing double, opening/closing single
    /// (OS preference on macOS; defaults “ ” ‘ ’).
    pub quotes: [char; 4],
    /// List indentation unit in columns for Tab in lists ("Indent Size", default 2); 0 = tab.
    pub indent: u8,
}

impl Default for EditPrefs {
    fn default() -> Self {
        EditPrefs {
            auto_pair_brackets: true,
            auto_pair_markdown: false,
            smart_punctuation: SmartPunctuation::Off,
            smart_dashes: SmartDashes::EnEm,
            quotes: ['\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}'],
            indent: 2,
        }
    }
}

/// The input of a planner.
#[derive(Clone, Debug, Default)]
pub struct PlanRequest {
    pub selection: Selection,
    pub params: Params,
    /// The current literal-line hints (read-only view).
    pub hints: HintSet,
    /// `true` in source-code mode (`PlanRequest.source` of ADR 0006 §5.5).
    pub source_mode: bool,
    pub prefs: EditPrefs,
}

impl PlanRequest {
    pub fn at(selection: Selection) -> Self {
        PlanRequest {
            selection,
            ..Default::default()
        }
    }
}

/// One step: a patch set in the coordinates produced by the previous step, and optionally the
/// complete hint set after it (`None` = the current hints transformed through the patches).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanStep {
    pub patches: PatchSet,
    pub hints_after: Option<HintSet>,
}

impl PlanStep {
    pub fn patches(patches: PatchSet) -> Self {
        PlanStep {
            patches,
            hints_after: None,
        }
    }
    pub fn hints(hints_after: HintSet) -> Self {
        PlanStep {
            patches: PatchSet::empty(),
            hints_after: Some(hints_after),
        }
    }
}

/// A planner's result: ordered steps plus the editing caret after the last one (`None` = the
/// live selection transformed through the steps).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
    pub selection_after: Option<Selection>,
    /// For key planners that may act either as a keystroke or as a structure change
    /// (Backspace, Delete, Tab): `Some(label)` = its own structural transaction with that
    /// label; `None` = a keystroke of the open typing group. Commands ignore it.
    pub label: Option<&'static str>,
}

impl Plan {
    pub fn single(step: PlanStep, selection_after: Option<Selection>) -> Self {
        Plan {
            steps: vec![step],
            selection_after,
            label: None,
        }
    }
    /// A plan that only moves the selection (no history entry).
    pub fn select(selection: Selection) -> Self {
        Plan {
            steps: Vec::new(),
            selection_after: Some(selection),
            label: None,
        }
    }
    /// Sets [`Plan::label`].
    pub fn labeled(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }
}

/// Why a planner produced no plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// The format does not implement this planner (the engine disables the command).
    Unsupported,
    /// Implemented, but not applicable at this selection.
    NotApplicable(&'static str),
    /// Bad parameters or inconsistent state.
    Invalid(String),
    Patch(PatchError),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Unsupported => write!(f, "not supported by this format"),
            PlanError::NotApplicable(why) => write!(f, "not applicable: {why}"),
            PlanError::Invalid(why) => write!(f, "invalid: {why}"),
            PlanError::Patch(e) => write!(f, "patch: {e}"),
        }
    }
}

impl std::error::Error for PlanError {}

impl From<PatchError> for PlanError {
    fn from(e: PatchError) -> Self {
        PlanError::Patch(e)
    }
}

/// An input-rule conversion pushed as its own `Auto` transaction right after the typing step
/// (ADR 0006 §2.6: `T1` then `T2`, both before the next frame).
#[derive(Clone, Debug, PartialEq)]
pub struct RulePlan {
    pub label: &'static str,
    pub plan: Plan,
}

/// The typing pipeline's result for one text event (ADR 0006 §5.1).
#[derive(Clone, Debug, PartialEq)]
pub struct TypingPlan {
    /// The keystroke step: the insertion replacing the selection, plus any hint it adds.
    pub step: PlanStep,
    /// Caret after the keystroke (`None` = after the inserted text).
    pub selection_after: Option<Selection>,
    /// An immediate input rule completed by this keystroke.
    pub rule: Option<RulePlan>,
}

/// The format-independent typing step: replace the primary selection with `typed`.
pub fn plain_typing(req: &PlanRequest, typed: &str) -> TypingPlan {
    let r = req.selection.primary().range();
    TypingPlan {
        step: PlanStep::patches(PatchSet::single(r, typed)),
        selection_after: None,
        rule: None,
    }
}
