//! Yes/no questions (issue #35): unsaved changes, Move to Trash, opening a link with another
//! application, replacing a theme, export overwrite.
//!
//! The application describes a question as a [`Question`] and hands it to a [`Confirm`]
//! implementation every frame until that returns the user's [`Choice`]. This is the seam:
//!
//! - [`Native`]: the platform's alert ([`rfd::MessageDialog`]: NSAlert on macOS, a message box
//!   on Windows, zenity on Linux). The desktop app uses it (see [`native`]).
//! - [`InApp`]: the same question as an egui modal in the shared dialog style
//!   ([`crate::dialog`]); the default of [`crate::AppOptions`] (no OS window in tests and
//!   headless runs) and the Linux fallback when zenity is not installed.
//! - [`Scripted`]: answers queued by a test, recording every question asked.
//!
//! Blocking: on macOS and Windows the native alert is application-modal and runs its own
//! event loop on the UI thread (NSAlert `runModal`, `MessageBoxW`), like the native file
//! dialogs the app already opens there; eframe simply does not get a frame until it closes.
//! On Linux the alert is a separate process (zenity), so it runs on a helper thread and the
//! window keeps repainting (a blocking call would make the compositor report the window as
//! not responding); meanwhile the app shows a dimmed backdrop that blocks input.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use egui::Context;

/// The answer to a question: the button chosen (Esc / closing the alert: [`Choice::Cancel`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Choice {
    Save,
    DontSave,
    Cancel,
    MoveToTrash,
    Open,
    Replace,
    Overwrite,
}

/// How a button is drawn: the accented primary action, a red destructive one, or plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Primary,
    Destructive,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub choice: Choice,
    pub label: String,
    pub role: Role,
}

/// Which question this is (tests, platform wording).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    UnsavedChanges,
    MoveToTrash,
    OpenLink,
    ReplaceTheme,
    ExportOverwrite,
}

/// A question with its buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub kind: Kind,
    /// The question itself (the alert's bold message).
    pub message: String,
    /// Explanation under it.
    pub detail: String,
    /// Action buttons first (primary or destructive), [`Choice::Cancel`] last.
    pub buttons: Vec<Button>,
    /// The button Return chooses. Cancel for every question whose action destroys or
    /// replaces something or leaves the app; Esc always chooses Cancel.
    pub default: Choice,
}

fn button(choice: Choice, label: &str, role: Role) -> Button {
    Button {
        choice,
        label: label.to_string(),
        role,
    }
}

fn cancel() -> Button {
    button(Choice::Cancel, "Cancel", Role::Cancel)
}

/// The platform's name for the trash.
pub fn trash_name() -> &'static str {
    if cfg!(windows) { "Recycle Bin" } else { "Trash" }
}

impl Question {
    /// The document has unsaved edits and the action would drop them (close, open another
    /// file, File → New, reload). Save is the default: it loses nothing.
    pub fn unsaved(name: &str) -> Question {
        Question {
            kind: Kind::UnsavedChanges,
            message: format!("Do you want to save the changes made to “{name}”?"),
            detail: "Your changes will be lost if you don’t save them.".into(),
            buttons: vec![
                button(Choice::Save, "Save", Role::Primary),
                button(Choice::DontSave, "Don’t Save", Role::Destructive),
                cancel(),
            ],
            default: Choice::Save,
        }
    }

    /// Move to Trash from the file tree.
    pub fn trash(name: &str, folder: bool) -> Question {
        let trash = trash_name();
        Question {
            kind: Kind::MoveToTrash,
            message: format!("Move “{name}” to the {trash}?"),
            detail: if folder {
                format!("The folder and everything in it go to the {trash}.")
            } else {
                format!("You can restore it from the {trash}.")
            },
            buttons: vec![
                button(Choice::MoveToTrash, &format!("Move to {trash}"), Role::Destructive),
                cancel(),
            ],
            default: Choice::Cancel,
        }
    }

    /// A link with a scheme other than http, https or mailto.
    pub fn open_link(url: &str) -> Question {
        Question {
            kind: Kind::OpenLink,
            message: "Open this link with another application?".into(),
            detail: url.to_string(),
            buttons: vec![button(Choice::Open, "Open Link", Role::Primary), cancel()],
            default: Choice::Cancel,
        }
    }

    /// "Import theme…" found an installed theme of that name.
    pub fn replace_theme(name: &str, existing: &[String]) -> Question {
        let mut detail = "Replacing it overwrites these files:".to_string();
        for f in existing.iter().take(8) {
            detail.push_str("\n• ");
            detail.push_str(f);
        }
        if existing.len() > 8 {
            detail.push_str(&format!("\n… and {} more", existing.len() - 8));
        }
        Question {
            kind: Kind::ReplaceTheme,
            message: format!("A theme named “{name}” is already installed. Replace it?"),
            detail,
            buttons: vec![button(Choice::Replace, "Replace", Role::Destructive), cancel()],
            default: Choice::Cancel,
        }
    }

    /// "Export and Overwrite with Previous" the first time for a document and destination.
    pub fn export_overwrite(target: &str) -> Question {
        Question {
            kind: Kind::ExportOverwrite,
            message: format!("Overwrite “{target}” without asking for a file name?"),
            detail: "You will not be asked again for this document and file.".into(),
            buttons: vec![button(Choice::Overwrite, "Overwrite", Role::Destructive), cancel()],
            default: Choice::Cancel,
        }
    }

    /// The button for `choice`, if the question has one.
    pub fn button(&self, choice: Choice) -> Option<&Button> {
        self.buttons.iter().find(|b| b.choice == choice)
    }
}

/// Shows questions. Called once per frame while a question is open, with the same question,
/// until it returns the answer; `None`: not answered yet (the question stays open).
pub trait Confirm: Send + Sync {
    fn ask(&self, ctx: &Context, q: &Question) -> Option<Choice>;

    /// The implementation draws the question in the egui window itself ([`InApp`]); otherwise
    /// the app blocks its window with a backdrop while waiting for an answer.
    fn draws_in_app(&self) -> bool {
        false
    }
}

/// The question as an egui modal (shared dialog style).
#[derive(Default)]
pub struct InApp;

impl Confirm for InApp {
    fn ask(&self, ctx: &Context, q: &Question) -> Option<Choice> {
        crate::dialog::confirm_modal(ctx, q)
    }

    fn draws_in_app(&self) -> bool {
        true
    }
}

/// Test answers: [`Scripted::answer`] queues choices, handed out one per question; without a
/// queued answer the question stays open. Every question asked is recorded once.
#[derive(Default)]
pub struct Scripted {
    state: Mutex<ScriptState>,
}

#[derive(Default)]
struct ScriptState {
    answers: VecDeque<Choice>,
    asked: Vec<Question>,
    /// The last recorded question is still open (repeated frames are not recorded again).
    open: bool,
}

impl Scripted {
    pub fn new() -> Arc<Scripted> {
        Arc::new(Scripted::default())
    }

    /// Queues the answer to the next question (or the open one).
    pub fn answer(&self, c: Choice) {
        self.lock().answers.push_back(c);
    }

    /// Every question asked so far, in order.
    pub fn asked(&self) -> Vec<Question> {
        self.lock().asked.clone()
    }

    /// The question asked last.
    pub fn last(&self) -> Option<Question> {
        self.lock().asked.last().cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ScriptState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Confirm for Scripted {
    fn ask(&self, _: &Context, q: &Question) -> Option<Choice> {
        let mut s = self.lock();
        if !(s.open && s.asked.last() == Some(q)) {
            s.asked.push(q.clone());
            s.open = true;
        }
        let a = s.answers.pop_front();
        if a.is_some() {
            s.open = false;
        }
        a
    }
}

/// The platform's alert ([`rfd::MessageDialog`]).
#[derive(Default)]
pub struct Native {
    /// Linux: the alert running on a helper thread and the question it shows.
    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    running: Mutex<Option<(Question, std::sync::mpsc::Receiver<Choice>)>>,
}

impl Confirm for Native {
    #[cfg(any(target_os = "macos", windows))]
    fn ask(&self, _: &Context, q: &Question) -> Option<Choice> {
        Some(rfd_choice(q, rfd_dialog(q).show()))
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    fn ask(&self, ctx: &Context, q: &Question) -> Option<Choice> {
        use std::sync::mpsc::TryRecvError;
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((shown, rx)) = running.as_ref()
            && shown == q
        {
            return match rx.try_recv() {
                Ok(c) => {
                    *running = None;
                    Some(c)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    *running = None;
                    Some(Choice::Cancel)
                }
            };
        }
        // a new question (an earlier alert's answer, if any, is dropped with its channel)
        let (tx, rx) = std::sync::mpsc::channel();
        let (q2, ctx) = (q.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("markise-alert".into()).spawn(move || {
            let c = rfd_choice(&q2, rfd_dialog(&q2).show());
            let _ = tx.send(c);
            ctx.request_repaint();
        });
        if spawned.is_err() {
            return Some(Choice::Cancel);
        }
        *running = Some((q.clone(), rx));
        None
    }
}

/// The alert for `q` and its button set. macOS: the default button is the first one (Return)
/// and a button titled "Cancel" answers Esc; three buttons read Save · Cancel · Don't Save
/// from the right. Windows: the stock message box (Yes/No/Cancel or OK/Cancel; custom
/// labels need common controls 6), so the message is phrased as a yes/no question. Linux
/// (zenity): custom labels, Esc cancels.
fn rfd_dialog(q: &Question) -> rfd::MessageDialog {
    use rfd::{MessageButtons, MessageLevel};
    let actions: Vec<&Button> = q.buttons.iter().filter(|b| b.choice != Choice::Cancel).collect();
    let cancel_label = q
        .button(Choice::Cancel)
        .map_or("Cancel".to_string(), |b| b.label.clone());
    let destructive = q.buttons.iter().any(|b| b.role == Role::Destructive);
    let buttons = if cfg!(windows) {
        if actions.len() >= 2 {
            MessageButtons::YesNoCancel
        } else {
            MessageButtons::OkCancel
        }
    } else if actions.len() >= 2 {
        let (a, b) = (actions[0].label.clone(), actions[1].label.clone());
        if cfg!(target_os = "macos") {
            MessageButtons::YesNoCancelCustom(a, cancel_label, b)
        } else {
            MessageButtons::YesNoCancelCustom(a, b, cancel_label)
        }
    } else {
        let a = actions.first().map_or("OK".to_string(), |b| b.label.clone());
        if cfg!(target_os = "macos") && q.default == Choice::Cancel {
            // Cancel first: the default button (Return), still answered by Esc
            MessageButtons::OkCancelCustom(cancel_label, a)
        } else {
            MessageButtons::OkCancelCustom(a, cancel_label)
        }
    };
    let d = rfd::MessageDialog::new()
        .set_level(if destructive {
            MessageLevel::Warning
        } else {
            MessageLevel::Info
        })
        .set_buttons(buttons);
    if cfg!(target_os = "macos") {
        d.set_title(&q.message).set_description(&q.detail)
    } else {
        // the title is the window caption here
        d.set_title("Markise")
            .set_description(format!("{}\n\n{}", q.message, q.detail))
    }
}

/// Maps the alert's result back to a choice; anything unexpected is Cancel.
fn rfd_choice(q: &Question, r: rfd::MessageDialogResult) -> Choice {
    use rfd::MessageDialogResult as R;
    let actions: Vec<&Button> = q.buttons.iter().filter(|b| b.choice != Choice::Cancel).collect();
    match r {
        R::Custom(label) => q
            .buttons
            .iter()
            .find(|b| b.label == label)
            .map_or(Choice::Cancel, |b| b.choice),
        R::Yes | R::Ok => actions.first().map_or(Choice::Cancel, |b| b.choice),
        R::No => actions.get(1).map_or(Choice::Cancel, |b| b.choice),
        R::Cancel => Choice::Cancel,
    }
}

/// The desktop app's questions: native alerts, except on Linux without `zenity` (rfd's
/// alert backend there), where the in-app dialog is used.
pub fn native() -> Arc<dyn Confirm> {
    if cfg!(any(target_os = "macos", windows)) || on_path("zenity") {
        Arc::new(Native::default())
    } else {
        Arc::new(InApp)
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_questions_default_to_cancel() {
        for q in [
            Question::trash("a.md", false),
            Question::replace_theme("t", &["t.css".into()]),
            Question::export_overwrite("a.html"),
            Question::open_link("vscode://x"),
        ] {
            assert_eq!(q.default, Choice::Cancel, "{:?}", q.kind);
            assert_eq!(q.buttons.last().map(|b| b.choice), Some(Choice::Cancel));
        }
        let q = Question::unsaved("a.md");
        assert_eq!(q.default, Choice::Save);
        assert_eq!(q.button(Choice::DontSave).map(|b| b.role), Some(Role::Destructive));
    }

    #[test]
    fn alert_results_map_back_to_choices() {
        let q = Question::unsaved("a.md");
        assert_eq!(
            rfd_choice(&q, rfd::MessageDialogResult::Custom("Don’t Save".into())),
            Choice::DontSave
        );
        assert_eq!(
            rfd_choice(&q, rfd::MessageDialogResult::Custom("Save".into())),
            Choice::Save
        );
        assert_eq!(rfd_choice(&q, rfd::MessageDialogResult::Yes), Choice::Save);
        assert_eq!(rfd_choice(&q, rfd::MessageDialogResult::No), Choice::DontSave);
        assert_eq!(rfd_choice(&q, rfd::MessageDialogResult::Cancel), Choice::Cancel);
        let q = Question::trash("a.md", false);
        let label = q.button(Choice::MoveToTrash).unwrap().label.clone();
        assert_eq!(
            rfd_choice(&q, rfd::MessageDialogResult::Custom(label)),
            Choice::MoveToTrash
        );
        assert_eq!(rfd_choice(&q, rfd::MessageDialogResult::Ok), Choice::MoveToTrash);
        assert_eq!(
            rfd_choice(&q, rfd::MessageDialogResult::Custom("x".into())),
            Choice::Cancel
        );
    }

    #[test]
    fn scripted_records_each_question_once() {
        let s = Scripted::new();
        let ctx = Context::default();
        let q = Question::trash("a.md", false);
        assert_eq!(s.ask(&ctx, &q), None);
        assert_eq!(s.ask(&ctx, &q), None);
        s.answer(Choice::Cancel);
        assert_eq!(s.ask(&ctx, &q), Some(Choice::Cancel));
        assert_eq!(s.asked().len(), 1);
        s.answer(Choice::MoveToTrash);
        assert_eq!(s.ask(&ctx, &q), Some(Choice::MoveToTrash));
        assert_eq!(s.asked().len(), 2, "asked again after the answer");
    }
}
