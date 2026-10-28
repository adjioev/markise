//! Undo history (ADR 0006 §4.1, §4.5): transactions of ordered steps, byte-bounded.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use markise_doc::{HintSet, PatchSet, Selection, UndoGroup};

/// Identity of a (text, hint set) state (ADR 0006 §4.3). Fresh per new transaction; undo/redo
/// restore recorded ids, so "dirty" is `state != saved_state` even after undoing to the saved
/// content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateId(pub u64);

/// One applied step: a canonical forward patch set (verbatim bytes, coordinates = the state
/// before this step), the verbatim bytes each patch removed, and the exact hint sets around it
/// (hints are few, so both sets are stored; undo restores `hints_before` exactly).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub forward: PatchSet,
    pub removed: Vec<Arc<str>>,
    pub hints_before: HintSet,
    pub hints_after: HintSet,
}

impl Step {
    /// The inverse patch set, in the coordinates after this step.
    pub fn inverse(&self) -> PatchSet {
        self.forward
            .inverse(&self.removed)
            .expect("a recorded step's removed bytes match its patches")
    }
    pub fn is_noop(&self) -> bool {
        self.forward.is_empty() && self.hints_before == self.hints_after
    }
    /// Heap bytes retained by this step (budget accounting): insert capacities, removed
    /// strings (+ `Arc` header), hint sets and the step itself. Allocator slack is not
    /// counted (bounded approximation, see [`Transaction::bytes`]).
    pub fn bytes(&self) -> usize {
        let hint = std::mem::size_of::<markise_doc::LiteralHint>();
        self.forward.byte_cost()
            + self
                .removed
                .iter()
                .map(|r| r.len() + 2 * std::mem::size_of::<usize>())
                .sum::<usize>()
            + self.removed.capacity() * std::mem::size_of::<Arc<str>>()
            + (self.hints_before.len() + self.hints_after.len()) * hint
            + std::mem::size_of::<Step>()
    }
}

/// Direction of a typing group (switching direction opens a new group, ADR 0006 §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypingDir {
    Insert,
    DeleteBackward,
    DeleteForward,
}

/// Kind of a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TxKind {
    Typing(TypingDir),
    /// Input-rule conversions, hint materialisation.
    Auto,
    Structural,
    Format,
    Paste,
    Bulk,
}

impl From<UndoGroup> for TxKind {
    fn from(g: UndoGroup) -> Self {
        match g {
            UndoGroup::Typing | UndoGroup::Structural => TxKind::Structural,
            UndoGroup::Format => TxKind::Format,
            UndoGroup::Paste => TxKind::Paste,
            UndoGroup::Auto => TxKind::Auto,
            UndoGroup::Bulk => TxKind::Bulk,
        }
    }
}

/// One undoable unit. `sel_before` / `sel_after` are immutable: valid in the before/after
/// states, restored verbatim by undo/redo (never transformed).
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub label: Cow<'static, str>,
    pub kind: TxKind,
    pub steps: Vec<Step>,
    pub sel_before: Selection,
    pub sel_after: Selection,
    pub state_before: StateId,
    pub state_after: StateId,
}

impl Transaction {
    /// Heap bytes retained by this transaction: its steps, an owned label, the selection range
    /// vectors and the transaction itself. This is the budget figure of ADR 0006 §4.5; it
    /// ignores allocator rounding and the `Vec` growth slack of `steps` beyond one step's size,
    /// so it may undercount real usage by a small constant per transaction.
    pub fn bytes(&self) -> usize {
        let label = match &self.label {
            Cow::Owned(s) => s.capacity(),
            Cow::Borrowed(_) => 0,
        };
        let sel = (self.sel_before.ranges().len() + self.sel_after.ranges().len())
            * std::mem::size_of::<markise_doc::SelRange>();
        self.steps.iter().map(Step::bytes).sum::<usize>()
            + (self.steps.capacity() - self.steps.len()) * std::mem::size_of::<Step>()
            + label
            + sel
            + std::mem::size_of::<Transaction>()
    }
}

/// History limits (ADR 0006 §4.5, Q8; C13 pause).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    /// Maximum number of undoable transactions (default 500).
    pub max_steps: usize,
    /// Maximum bytes held by undo + redo (default 32 MB).
    pub max_bytes: usize,
    /// A pause longer than this between keystrokes opens a new typing group (C13: 1 s).
    pub typing_pause: Duration,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        HistoryLimits {
            max_steps: 500,
            max_bytes: 32 << 20,
            typing_pause: Duration::from_secs(1),
        }
    }
}

/// Undo and redo stacks with byte accounting; the oldest transactions are evicted first, the
/// newest one is always kept.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: VecDeque<Transaction>,
    redo: Vec<Transaction>,
    bytes: usize,
    limits: HistoryLimits,
    evicted: u64,
}

impl History {
    pub fn new(limits: HistoryLimits) -> Self {
        History {
            limits,
            ..Default::default()
        }
    }
    pub fn limits(&self) -> &HistoryLimits {
        &self.limits
    }
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// Bytes currently held by both stacks.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    /// Transactions dropped by the budget so far.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }
    pub fn last(&self) -> Option<&Transaction> {
        self.undo.back()
    }
    pub fn undo_stack(&self) -> impl Iterator<Item = &Transaction> {
        self.undo.iter()
    }

    /// Records a new transaction: clears redo, then enforces the budget.
    pub fn push(&mut self, tx: Transaction) {
        for r in self.redo.drain(..) {
            self.bytes -= r.bytes();
        }
        self.bytes += tx.bytes();
        self.undo.push_back(tx);
        self.evict();
    }

    /// Mutates the newest transaction (typing coalescing, joins) with byte re-accounting.
    pub fn amend_last<R>(&mut self, f: impl FnOnce(&mut Transaction) -> R) -> Option<R> {
        let tx = self.undo.back_mut()?;
        let old = tx.bytes();
        let r = f(tx);
        let new = tx.bytes();
        self.bytes = self.bytes + new - old;
        self.evict();
        Some(r)
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        let tx = self.undo.pop_back()?;
        self.bytes -= tx.bytes();
        Some(tx)
    }
    pub fn push_redo(&mut self, tx: Transaction) {
        self.bytes += tx.bytes();
        self.redo.push(tx);
    }
    pub fn pop_redo(&mut self) -> Option<Transaction> {
        let tx = self.redo.pop()?;
        self.bytes -= tx.bytes();
        Some(tx)
    }
    /// Re-records a redone transaction without clearing the redo stack.
    pub fn push_undo_keep_redo(&mut self, tx: Transaction) {
        self.bytes += tx.bytes();
        self.undo.push_back(tx);
    }

    /// Removes and returns the redo stack (oldest last, as stored).
    pub fn take_redo(&mut self) -> Vec<Transaction> {
        let r = std::mem::take(&mut self.redo);
        for t in &r {
            self.bytes -= t.bytes();
        }
        r
    }

    /// Puts a redo stack taken with [`History::take_redo`] back (replacing the current one).
    pub fn restore_redo(&mut self, redo: Vec<Transaction>) {
        self.clear_redo();
        self.bytes += redo.iter().map(Transaction::bytes).sum::<usize>();
        self.redo = redo;
    }

    /// Drops the redo stack.
    pub fn clear_redo(&mut self) {
        for r in self.redo.drain(..) {
            self.bytes -= r.bytes();
        }
    }

    fn evict(&mut self) {
        while self.undo.len() > 1 && (self.undo.len() > self.limits.max_steps || self.bytes > self.limits.max_bytes) {
            let tx = self.undo.pop_front().unwrap();
            self.bytes -= tx.bytes();
            self.evicted += 1;
        }
    }
}
