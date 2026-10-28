//! Executable commands (ADR 0006 §3.1): the registry assembles [`CommandSpec`]s from the
//! engine's own descriptors and the format's planner descriptors (pure data from L1). No UI
//! binding yet (keymap/menus: M2b).

use std::collections::HashMap;
use std::fmt;

use markise_doc::{
    CommandDescriptor, DefaultKey, DocumentFormat, Effect, ParamKind, ParamSchema, ParamSpec, Params, PlanError,
    PlannerId, Scope, UndoGroup,
};

use markise_doc::Selection;

use super::{CaretGeometry, EditError, EditorSession, TxKind};

/// An engine-implemented command.
pub type EngineFn = fn(&mut EditorSession, &Params) -> Result<(), CommandError>;
/// An engine command that needs caret geometry.
pub type CaretFn = fn(&mut EditorSession, &mut dyn CaretGeometry, &Params) -> Result<(), CommandError>;

/// How a command runs.
#[derive(Clone, Copy)]
pub enum Run {
    /// `session.apply_plan(doc.plan(id, &req)?)` with the descriptor's undo group.
    Planner(PlannerId),
    /// Engine-only behaviour (undo, typing pipeline…).
    Engine(EngineFn),
    /// Needs the caret geometry of a laid-out view (caret moves, stop-aware deletion).
    Caret(CaretFn),
    /// Performed by the host application (file I/O, view modes); listed for keymaps and
    /// `describe()`.
    Host,
}

impl fmt::Debug for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Run::Planner(id) => write!(f, "Planner({})", id.0),
            Run::Engine(_) => write!(f, "Engine"),
            Run::Caret(_) => write!(f, "Caret"),
            Run::Host => write!(f, "Host"),
        }
    }
}

/// An executable command.
#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    pub desc: &'static CommandDescriptor,
    pub run: Run,
}

/// Why a command did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Unknown(String),
    Disabled(&'static str),
    Params(String),
    Plan(PlanError),
    Edit(EditError),
    /// The command needs caret geometry (run it through an editor surface).
    NeedsLayout,
    /// The command is performed by the host application.
    Host(&'static str),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Unknown(id) => write!(f, "unknown command `{id}`"),
            CommandError::Disabled(why) => write!(f, "disabled: {why}"),
            CommandError::Params(why) => write!(f, "{why}"),
            CommandError::Plan(e) => write!(f, "{e}"),
            CommandError::Edit(e) => write!(f, "{e}"),
            CommandError::NeedsLayout => write!(f, "needs a laid-out view"),
            CommandError::Host(id) => write!(f, "`{id}` is performed by the application"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<EditError> for CommandError {
    fn from(e: EditError) -> Self {
        CommandError::Edit(e)
    }
}

const fn key(mac: &'static str, other: &'static str) -> DefaultKey {
    DefaultKey {
        mac: Some(mac),
        other: Some(other),
    }
}

const fn engine(
    id: &'static str,
    label: &'static str,
    keys: &'static [DefaultKey],
    scope: Scope,
    effect: Effect,
    params: ParamSchema,
) -> CommandDescriptor {
    CommandDescriptor {
        id,
        label,
        menu: None,
        keys,
        scope,
        effect,
        params,
        planner: None,
    }
}

const TYPING: Effect = Effect::Mutates {
    group: UndoGroup::Typing,
};
const STRUCTURAL: Effect = Effect::Mutates {
    group: UndoGroup::Structural,
};

const VIEW: Effect = Effect::View;

const fn mac(k: &'static str) -> DefaultKey {
    DefaultKey {
        mac: Some(k),
        other: None,
    }
}
const fn other(k: &'static str) -> DefaultKey {
    DefaultKey {
        mac: None,
        other: Some(k),
    }
}

const fn nav(id: &'static str, label: &'static str, keys: &'static [DefaultKey]) -> CommandDescriptor {
    engine(id, label, keys, Scope::Editor, VIEW, ParamSchema::NONE)
}

/// The engine's own commands (spec 02 §5.4, §15; ADR 0006 §3.2 table) with how they run.
/// Caret and selection commands need the caret geometry of a laid-out view ([`Run::Caret`]);
/// `file.*` / `view.*` commands are performed by the host (I/O, UI state).
pub static ENGINE_COMMANDS: [(CommandDescriptor, Run); 43] = [
    (
        engine(
            "edit.undo",
            "Undo",
            &[key("Cmd+Z", "Ctrl+Z")],
            Scope::Global,
            STRUCTURAL,
            ParamSchema::NONE,
        ),
        Run::Engine(run_undo),
    ),
    (
        engine(
            "edit.redo",
            "Redo",
            &[key("Shift+Cmd+Z", "Ctrl+Shift+Z"), other("Ctrl+Y")],
            Scope::Global,
            STRUCTURAL,
            ParamSchema::NONE,
        ),
        Run::Engine(run_redo),
    ),
    (
        engine(
            "edit.insertText",
            "Insert Text",
            &[],
            Scope::Editor,
            TYPING,
            ParamSchema(&[ParamSpec {
                name: "text",
                kind: ParamKind::Text,
                required: true,
            }]),
        ),
        Run::Engine(run_insert),
    ),
    (
        engine(
            "edit.deleteBackward",
            "Delete Backward",
            &[
                key("Backspace", "Backspace"),
                mac("Shift+Backspace"),
                other("Shift+Backspace"),
            ],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Caret(caret_backspace),
    ),
    (
        engine(
            "edit.deleteForward",
            "Delete Forward",
            &[key("Delete", "Delete"), mac("Ctrl+D")],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Caret(caret_delete),
    ),
    (
        engine(
            "edit.deleteWordBackward",
            "Delete Word Backward",
            &[key("Alt+Backspace", "Ctrl+Backspace")],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Caret(caret_delete_word_back),
    ),
    (
        engine(
            "edit.deleteWordForward",
            "Delete Word Forward",
            &[key("Alt+Delete", "Ctrl+Delete")],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Caret(caret_delete_word_fwd),
    ),
    (
        engine(
            "delete.toLineStart",
            "Delete to Line Start",
            &[mac("Cmd+Backspace")],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Caret(caret_delete_line_start),
    ),
    (
        engine(
            "edit.newline",
            "New Line",
            &[key("Return", "Enter")],
            Scope::Editor,
            STRUCTURAL,
            ParamSchema::NONE,
        ),
        Run::Engine(run_newline),
    ),
    (
        engine(
            "edit.softBreak",
            "Line Break",
            &[key("Shift+Return", "Shift+Enter")],
            Scope::Editor,
            STRUCTURAL,
            ParamSchema::NONE,
        ),
        Run::Engine(run_soft_break),
    ),
    (
        engine(
            "edit.indentOrNextCell",
            "Indent or Tab",
            &[key("Tab", "Tab")],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Engine(run_tab),
    ),
    (
        engine(
            "edit.outdentOrPrevCell",
            "Outdent or Back Tab",
            &[key("Shift+Tab", "Shift+Tab")],
            Scope::Editor,
            TYPING,
            ParamSchema::NONE,
        ),
        Run::Engine(run_shift_tab),
    ),
    (
        nav("caret.left", "Move Left", &[key("Left", "Left")]),
        Run::Caret(caret_left),
    ),
    (
        nav("caret.right", "Move Right", &[key("Right", "Right")]),
        Run::Caret(caret_right),
    ),
    (nav("caret.up", "Move Up", &[key("Up", "Up")]), Run::Caret(caret_up)),
    (
        nav("caret.down", "Move Down", &[key("Down", "Down")]),
        Run::Caret(caret_down),
    ),
    (
        nav("caret.wordLeft", "Move Word Left", &[key("Alt+Left", "Ctrl+Left")]),
        Run::Caret(caret_word_left),
    ),
    (
        nav("caret.wordRight", "Move Word Right", &[key("Alt+Right", "Ctrl+Right")]),
        Run::Caret(caret_word_right),
    ),
    (
        nav(
            "caret.lineStart",
            "Jump to Line Start",
            &[key("Cmd+Left", "Home"), mac("Home"), mac("Ctrl+A")],
        ),
        Run::Caret(caret_line_start),
    ),
    (
        nav(
            "caret.lineEnd",
            "Jump to Line End",
            &[key("Cmd+Right", "End"), mac("End"), mac("Ctrl+E")],
        ),
        Run::Caret(caret_line_end),
    ),
    (
        nav("caret.pageUp", "Page Up", &[key("PageUp", "PageUp")]),
        Run::Caret(caret_page_up),
    ),
    (
        nav("caret.pageDown", "Page Down", &[key("PageDown", "PageDown")]),
        Run::Caret(caret_page_down),
    ),
    (
        nav("nav.top", "Jump to Top", &[key("Cmd+Up", "Ctrl+Home")]),
        Run::Caret(caret_top),
    ),
    (
        nav("nav.bottom", "Jump to Bottom", &[key("Cmd+Down", "Ctrl+End")]),
        Run::Caret(caret_bottom),
    ),
    (
        nav("select.left", "Select Left", &[key("Shift+Left", "Shift+Left")]),
        Run::Caret(sel_left),
    ),
    (
        nav("select.right", "Select Right", &[key("Shift+Right", "Shift+Right")]),
        Run::Caret(sel_right),
    ),
    (
        nav("select.up", "Select Up", &[key("Shift+Up", "Shift+Up")]),
        Run::Caret(sel_up),
    ),
    (
        nav("select.down", "Select Down", &[key("Shift+Down", "Shift+Down")]),
        Run::Caret(sel_down),
    ),
    (
        nav(
            "select.wordLeft",
            "Select Word Left",
            &[key("Shift+Alt+Left", "Ctrl+Shift+Left")],
        ),
        Run::Caret(sel_word_left),
    ),
    (
        nav(
            "select.wordRight",
            "Select Word Right",
            &[key("Shift+Alt+Right", "Ctrl+Shift+Right")],
        ),
        Run::Caret(sel_word_right),
    ),
    (
        nav(
            "select.lineStart",
            "Select to Line Start",
            &[key("Shift+Cmd+Left", "Shift+Home"), mac("Shift+Home")],
        ),
        Run::Caret(sel_line_start),
    ),
    (
        nav(
            "select.lineEnd",
            "Select to Line End",
            &[key("Shift+Cmd+Right", "Shift+End"), mac("Shift+End")],
        ),
        Run::Caret(sel_line_end),
    ),
    (
        nav(
            "select.pageUp",
            "Select Page Up",
            &[key("Shift+PageUp", "Shift+PageUp")],
        ),
        Run::Caret(sel_page_up),
    ),
    (
        nav(
            "select.pageDown",
            "Select Page Down",
            &[key("Shift+PageDown", "Shift+PageDown")],
        ),
        Run::Caret(sel_page_down),
    ),
    (
        nav("select.top", "Select to Top", &[key("Shift+Cmd+Up", "Ctrl+Shift+Home")]),
        Run::Caret(sel_top),
    ),
    (
        nav(
            "select.bottom",
            "Select to Bottom",
            &[key("Shift+Cmd+Down", "Ctrl+Shift+End")],
        ),
        Run::Caret(sel_bottom),
    ),
    (
        nav("select.all", "Select All", &[key("Cmd+A", "Ctrl+A")]),
        Run::Caret(sel_all),
    ),
    (
        engine(
            "file.save",
            "Save",
            &[key("Cmd+S", "Ctrl+S")],
            Scope::Global,
            Effect::Pure,
            ParamSchema::NONE,
        ),
        Run::Host,
    ),
    (
        engine(
            "file.saveAs",
            "Save As…",
            &[key("Shift+Cmd+S", "Ctrl+Shift+S")],
            Scope::Global,
            Effect::Pure,
            ParamSchema::NONE,
        ),
        Run::Host,
    ),
    (
        engine(
            "file.new",
            "New",
            &[key("Cmd+N", "Ctrl+N")],
            Scope::Global,
            Effect::Pure,
            ParamSchema::NONE,
        ),
        Run::Host,
    ),
    (
        engine(
            "file.open",
            "Open…",
            &[key("Cmd+O", "Ctrl+O")],
            Scope::Global,
            Effect::Pure,
            ParamSchema::NONE,
        ),
        Run::Host,
    ),
    (
        engine(
            "view.sourceMode",
            "Source Code Mode",
            &[key("Cmd+/", "Ctrl+/")],
            Scope::Global,
            VIEW,
            ParamSchema::NONE,
        ),
        Run::Host,
    ),
    (
        engine(
            "view.toggleSidebar",
            "Toggle Sidebar",
            &[key("Shift+Cmd+L", "Ctrl+Shift+L")],
            Scope::Global,
            VIEW,
            ParamSchema::NONE,
        ),
        Run::Host,
    ),
];

fn run_undo(s: &mut EditorSession, _: &Params) -> Result<(), CommandError> {
    s.undo().then_some(()).ok_or(CommandError::Disabled("nothing to undo"))
}
fn run_redo(s: &mut EditorSession, _: &Params) -> Result<(), CommandError> {
    s.redo().then_some(()).ok_or(CommandError::Disabled("nothing to redo"))
}
fn run_insert(s: &mut EditorSession, p: &Params) -> Result<(), CommandError> {
    Ok(s.insert_text(p.get_text("text").unwrap_or_default())?)
}
fn run_newline(s: &mut EditorSession, _: &Params) -> Result<(), CommandError> {
    Ok(s.newline()?)
}
fn run_soft_break(s: &mut EditorSession, _: &Params) -> Result<(), CommandError> {
    Ok(s.soft_break()?)
}
fn run_tab(s: &mut EditorSession, _: &Params) -> Result<(), CommandError> {
    Ok(s.tab(false)?)
}
fn run_shift_tab(s: &mut EditorSession, _: &Params) -> Result<(), CommandError> {
    Ok(s.tab(true)?)
}

// ---- caret commands (need layout) ---------------------------------------------------------

fn caret_backspace(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
    let p = s.selection().primary();
    let stop = (p.is_caret() && p.focus.virt == 0).then(|| g.horizontal(p.focus, false, false).offset);
    g.release_doc();
    Ok(s.delete_backward_to(stop)?)
}
fn caret_delete(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
    let p = s.selection().primary();
    let stop = (p.is_caret() && p.focus.virt == 0).then(|| g.horizontal(p.focus, true, false).offset);
    g.release_doc();
    Ok(s.delete_forward_to(stop)?)
}
fn caret_delete_word_back(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
    generated_delete(s, g, true, |g, p| g.horizontal(p, false, true))
}
fn caret_delete_word_fwd(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
    generated_delete(s, g, false, |g, p| g.horizontal(p, true, true))
}
fn caret_delete_line_start(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
    generated_delete(s, g, true, |g, p| g.row_edge(p, false))
}

/// A deletion whose range the command generates (word, line start): clamped to the caret's
/// block content by the session; a user selection is deleted as it is.
fn generated_delete(
    s: &mut EditorSession,
    g: &mut dyn CaretGeometry,
    backward: bool,
    target: impl FnOnce(&mut dyn CaretGeometry, markise_doc::DocPos) -> markise_doc::DocPos,
) -> Result<(), CommandError> {
    let p = s.selection().primary();
    if !p.is_caret() || p.focus.virt > 0 {
        g.release_doc();
        if backward {
            s.delete_backward_to(None)?;
        } else {
            s.delete_forward_to(None)?;
        }
        return Ok(());
    }
    let to = target(g, p.focus).offset;
    g.release_doc();
    Ok(s.delete_generated(to, backward)?)
}

#[derive(Clone, Copy)]
enum Motion {
    Char(bool),
    Word(bool),
    Line(bool),
    Vertical(bool),
    Page(bool),
    Doc(bool),
}

/// Moves the focus by `m`; `extend` keeps the anchor (selection), otherwise the selection
/// collapses (a plain Left/Right with a selection collapses to its start/end).
fn motion(s: &mut EditorSession, g: &mut dyn CaretGeometry, m: Motion, extend: bool) -> Result<(), CommandError> {
    let sel = s.selection().clone();
    let p = sel.primary();
    let mut goal = None;
    let focus = match m {
        Motion::Char(fwd) if !extend && !p.is_caret() => {
            if fwd {
                p.end()
            } else {
                p.start()
            }
        }
        Motion::Char(fwd) => g.horizontal(p.focus, fwd, false),
        Motion::Word(fwd) => g.horizontal(p.focus, fwd, true),
        Motion::Line(end) => g.row_edge(p.focus, end),
        Motion::Vertical(down) => {
            let (d, x) = g.vertical(p.focus, sel.goal_x, down);
            goal = Some(x);
            d
        }
        Motion::Page(down) => {
            let (d, x) = g.page(p.focus, sel.goal_x, down);
            goal = Some(x);
            d
        }
        Motion::Doc(end) => g.doc_edge(end),
    };
    let mut next = if extend {
        Selection::single(p.anchor, focus)
    } else {
        Selection::caret_at(focus)
    };
    next.goal_x = goal;
    s.set_selection(next);
    Ok(())
}

macro_rules! motions {
    ($($name:ident => $m:expr, $ext:expr;)*) => {
        $(fn $name(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
            motion(s, g, $m, $ext)
        })*
    };
}

motions! {
    caret_left => Motion::Char(false), false;
    caret_right => Motion::Char(true), false;
    caret_up => Motion::Vertical(false), false;
    caret_down => Motion::Vertical(true), false;
    caret_word_left => Motion::Word(false), false;
    caret_word_right => Motion::Word(true), false;
    caret_line_start => Motion::Line(false), false;
    caret_line_end => Motion::Line(true), false;
    caret_page_up => Motion::Page(false), false;
    caret_page_down => Motion::Page(true), false;
    caret_top => Motion::Doc(false), false;
    caret_bottom => Motion::Doc(true), false;
    sel_left => Motion::Char(false), true;
    sel_right => Motion::Char(true), true;
    sel_up => Motion::Vertical(false), true;
    sel_down => Motion::Vertical(true), true;
    sel_word_left => Motion::Word(false), true;
    sel_word_right => Motion::Word(true), true;
    sel_line_start => Motion::Line(false), true;
    sel_line_end => Motion::Line(true), true;
    sel_page_up => Motion::Page(false), true;
    sel_page_down => Motion::Page(true), true;
    sel_top => Motion::Doc(false), true;
    sel_bottom => Motion::Doc(true), true;
}

fn sel_all(s: &mut EditorSession, g: &mut dyn CaretGeometry, _: &Params) -> Result<(), CommandError> {
    let a = g.doc_edge(false);
    let b = g.doc_edge(true);
    s.set_selection(Selection::single(a, b));
    Ok(())
}

/// The command registry: one entry per id; menus, shortcuts, palette, CLI and MCP all go
/// through it.
#[derive(Clone, Debug)]
pub struct Registry {
    specs: Vec<CommandSpec>,
    index: HashMap<&'static str, usize>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    /// The engine commands only.
    pub fn new() -> Self {
        let mut r = Registry {
            specs: Vec::new(),
            index: HashMap::new(),
        };
        for (desc, run) in ENGINE_COMMANDS.iter() {
            r.insert(CommandSpec { desc, run: *run });
        }
        r
    }

    /// Engine commands plus the format's planner commands.
    pub fn for_format(format: &dyn DocumentFormat) -> Self {
        let mut r = Self::new();
        r.merge(format.commands());
        r
    }

    fn insert(&mut self, spec: CommandSpec) {
        self.index.insert(spec.desc.id, self.specs.len());
        self.specs.push(spec);
    }

    /// Merges format descriptors (at document open). Returns the ids that were skipped: no
    /// planner, or an id the registry already has (engine commands win).
    pub fn merge(&mut self, descs: &'static [CommandDescriptor]) -> Vec<&'static str> {
        let mut skipped = Vec::new();
        for desc in descs {
            match desc.planner {
                Some(id) if !self.index.contains_key(desc.id) => self.insert(CommandSpec {
                    desc,
                    run: Run::Planner(id),
                }),
                _ => skipped.push(desc.id),
            }
        }
        skipped
    }

    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.index.get(id).map(|&i| &self.specs[i])
    }

    /// Every command's descriptor (MCP `list_commands`, `markise-cli commands`).
    pub fn describe(&self) -> impl Iterator<Item = &'static CommandDescriptor> + '_ {
        self.specs.iter().map(|s| s.desc)
    }

    /// Whether `id` can run now (planner commands ask the document).
    pub fn enabled(&self, s: &EditorSession, id: &str) -> Result<(), CommandError> {
        let spec = self.get(id).ok_or_else(|| CommandError::Unknown(id.to_string()))?;
        match spec.run {
            Run::Planner(pid) => {
                if s.doc().can_plan(pid, &s.request(Params::new())) {
                    Ok(())
                } else {
                    Err(CommandError::Disabled("not applicable here"))
                }
            }
            Run::Engine(_) | Run::Caret(_) | Run::Host => match id {
                "edit.undo" if !s.history().can_undo() => Err(CommandError::Disabled("nothing to undo")),
                "edit.redo" if !s.history().can_redo() => Err(CommandError::Disabled("nothing to redo")),
                _ => Ok(()),
            },
        }
    }

    /// Whether toggle command `id` is active at the selection (menu check marks, toolbar).
    pub fn checked(&self, s: &EditorSession, id: &str) -> bool {
        match self.get(id).map(|c| c.run) {
            Some(Run::Planner(pid)) => s.doc().plan_checked(pid, &s.request(Params::new())),
            _ => false,
        }
    }

    /// Runs command `id` with caret geometry available (any kind of command).
    pub fn execute_with(
        &self,
        s: &mut EditorSession,
        g: &mut dyn CaretGeometry,
        id: &str,
        params: &Params,
    ) -> Result<(), CommandError> {
        let spec = *self.get(id).ok_or_else(|| CommandError::Unknown(id.to_string()))?;
        if let Run::Caret(f) = spec.run {
            params.validate(&spec.desc.params).map_err(CommandError::Params)?;
            return f(s, g, params);
        }
        if matches!(spec.desc.effect, Effect::Mutates { .. }) {
            g.release_doc();
        }
        self.execute(s, id, params)
    }

    /// Runs command `id` with `params` on the session.
    pub fn execute(&self, s: &mut EditorSession, id: &str, params: &Params) -> Result<(), CommandError> {
        let spec = *self.get(id).ok_or_else(|| CommandError::Unknown(id.to_string()))?;
        params.validate(&spec.desc.params).map_err(CommandError::Params)?;
        match spec.run {
            Run::Engine(f) => f(s, params),
            Run::Caret(_) => Err(CommandError::NeedsLayout),
            Run::Host => Err(CommandError::Host(spec.desc.id)),
            Run::Planner(pid) => {
                let plan = s
                    .doc()
                    .plan(pid, &s.request(params.clone()))
                    .map_err(CommandError::Plan)?;
                let kind = match spec.desc.effect {
                    Effect::Mutates { group } => TxKind::from(group),
                    Effect::Pure | Effect::View => TxKind::Structural,
                };
                Ok(s.apply_plan(spec.desc.label, kind, plan)?)
            }
        }
    }
}
