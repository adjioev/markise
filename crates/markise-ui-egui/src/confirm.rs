//! Yes/no questions (issue #35): unsaved changes, Move to Trash, opening a link with another
//! application, replacing a theme, export overwrite, resetting a Preferences section.
//!
//! The application describes a question as a [`Question`] and hands it to a [`Confirm`]
//! implementation every frame until that returns the user's [`Choice`]. This is the seam:
//!
//! - [`Native`]: the platform's alert ([`rfd::MessageDialog`]: NSAlert on macOS, a message box
//!   on Windows, zenity on Linux). The desktop app uses it (see [`native`]). Where the
//!   platform's alert cannot make the question's default button its default (Cancel on
//!   Windows and Linux), that question uses the in-app dialog instead ([`plan`]): Return never
//!   confirms a destructive action.
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
    Create,
    Reset,
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
    CreateNote,
    ResetSettings,
}

/// A question with its buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// Which question this is: the app numbers every question it opens, so an answer can
    /// only ever apply to the question (and continuation) it was given for — two questions
    /// with the same text are still different questions.
    pub id: u64,
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
            id: 0,
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
            id: 0,
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
            id: 0,
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
            id: 0,
            kind: Kind::ReplaceTheme,
            message: format!("A theme named “{name}” is already installed. Replace it?"),
            detail,
            buttons: vec![button(Choice::Replace, "Replace", Role::Destructive), cancel()],
            default: Choice::Cancel,
        }
    }

    /// "Export and Overwrite with Previous" the first time for a document and destination.
    pub fn export_overwrite(target: &std::path::Path) -> Question {
        let name = target
            .file_name()
            .map_or_else(|| target.display().to_string(), |n| n.to_string_lossy().into_owned());
        Question {
            id: 0,
            kind: Kind::ExportOverwrite,
            message: format!("Overwrite “{name}” without asking for a file name?"),
            detail: format!(
                "{}\n\nYou will not be asked again for this document and file.",
                target.display()
            ),
            buttons: vec![button(Choice::Overwrite, "Overwrite", Role::Destructive), cancel()],
            default: Choice::Cancel,
        }
    }

    /// Cmd/Ctrl+click on a link to a note that does not exist (spec 06 V2.8): create it, and
    /// where (`folder`: the vault folder it goes to; empty = the top level)? Creating loses
    /// nothing, so Create is the default.
    pub fn create_note(name: &str, folder: &str) -> Question {
        let place = if folder.is_empty() {
            "at the top level of the folder".to_string()
        } else {
            format!("in {folder}")
        };
        Question {
            id: 0,
            kind: Kind::CreateNote,
            message: format!("Create “{name}” {place}?"),
            detail: "No note of that name exists yet. It is created empty and opened.".into(),
            buttons: vec![button(Choice::Create, "Create", Role::Primary), cancel()],
            default: Choice::Create,
        }
    }

    /// Preferences → "Reset to Defaults" for one section (issue #50).
    pub fn reset_settings(section: &str) -> Question {
        Question {
            id: 0,
            kind: Kind::ResetSettings,
            message: format!("Reset the {section} settings to their defaults?"),
            detail: "Every setting in this section goes back to its default value.".into(),
            buttons: vec![button(Choice::Reset, "Reset", Role::Destructive), cancel()],
            default: Choice::Cancel,
        }
    }

    /// The same question numbered `id`.
    pub fn with_id(mut self, id: u64) -> Question {
        self.id = id;
        self
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

    /// The implementation draws `q` in the egui window itself ([`InApp`]); otherwise the app
    /// blocks its window with a backdrop while waiting for an answer.
    fn draws_in_app(&self, _q: &Question) -> bool {
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

    fn draws_in_app(&self, _q: &Question) -> bool {
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

/// The platform's alert ([`rfd::MessageDialog`]), or the in-app dialog for a question whose
/// default button the platform's alert cannot make its default (see [`plan`]).
#[derive(Default)]
pub struct Native {
    /// Linux: the alert running on a helper thread and the question it shows.
    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    running: Mutex<Option<(Question, std::sync::mpsc::Receiver<Choice>)>>,
}

impl Confirm for Native {
    fn ask(&self, ctx: &Context, q: &Question) -> Option<Choice> {
        match plan(q, Os::current()) {
            Plan::InApp => InApp.ask(ctx, q),
            Plan::Alert(buttons) => self.alert(ctx, q, buttons),
        }
    }

    fn draws_in_app(&self, q: &Question) -> bool {
        plan(q, Os::current()) == Plan::InApp
    }
}

impl Native {
    #[cfg(any(target_os = "macos", windows))]
    fn alert(&self, _: &Context, q: &Question, buttons: AlertButtons) -> Option<Choice> {
        Some(rfd_choice(q, rfd_dialog(q, buttons).show()))
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    fn alert(&self, ctx: &Context, q: &Question, buttons: AlertButtons) -> Option<Choice> {
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
        // a new question (an earlier alert's answer, if any, is dropped with its channel: it
        // can never answer this one)
        let (tx, rx) = std::sync::mpsc::channel();
        let (q2, ctx) = (q.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("markise-alert".into()).spawn(move || {
            let c = rfd_choice(&q2, rfd_dialog(&q2, buttons).show());
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

/// The platforms the native alerts differ on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    /// Linux and the other Unix desktops (rfd: zenity).
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// The alert's buttons, in the order the backend adds them (a mirror of
/// [`rfd::MessageButtons`] that can be compared in tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlertButtons {
    OkCancel,
    YesNoCancel,
    OkCancelCustom(String, String),
    YesNoCancelCustom(String, String, String),
}

impl AlertButtons {
    fn to_rfd(&self) -> rfd::MessageButtons {
        use rfd::MessageButtons as M;
        match self.clone() {
            AlertButtons::OkCancel => M::OkCancel,
            AlertButtons::YesNoCancel => M::YesNoCancel,
            AlertButtons::OkCancelCustom(a, b) => M::OkCancelCustom(a, b),
            AlertButtons::YesNoCancelCustom(a, b, c) => M::YesNoCancelCustom(a, b, c),
        }
    }
}

/// How the native backend shows a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// A platform alert with these buttons; its default button (Return) is the question's.
    Alert(AlertButtons),
    /// The platform's alert would make another button the default: the in-app dialog.
    InApp,
}

/// Chooses how to show `q` natively so that Return picks [`Question::default`] and Esc picks
/// Cancel.
///
/// - macOS (NSAlert): the first button is the default and a button titled "Cancel" answers
///   Esc, so any default can be honoured: destructive questions list Cancel first; unsaved
///   changes read Save · Cancel · Don't Save from the right.
/// - Windows (stock message box; custom labels and default buttons need common controls 6,
///   issue #40) and Linux (rfd's zenity call has no `--default-cancel`): the first button is
///   always the default, so only questions whose default is their first action are native
///   (unsaved changes → Save); Cancel-default questions (Move to Trash, Replace, Overwrite,
///   Open Link) use the in-app dialog.
pub fn plan(q: &Question, os: Os) -> Plan {
    let actions: Vec<&Button> = q.buttons.iter().filter(|b| b.choice != Choice::Cancel).collect();
    let Some(first) = actions.first() else {
        return Plan::InApp;
    };
    let cancel = q
        .button(Choice::Cancel)
        .map_or("Cancel".to_string(), |b| b.label.clone());
    let first_is_default = q.default == first.choice;
    match os {
        Os::Mac if actions.len() >= 2 && first_is_default => Plan::Alert(AlertButtons::YesNoCancelCustom(
            first.label.clone(),
            cancel,
            actions[1].label.clone(),
        )),
        Os::Mac if actions.len() == 1 && q.default == Choice::Cancel => {
            Plan::Alert(AlertButtons::OkCancelCustom(cancel, first.label.clone()))
        }
        Os::Mac if actions.len() == 1 && first_is_default => {
            Plan::Alert(AlertButtons::OkCancelCustom(first.label.clone(), cancel))
        }
        Os::Windows if first_is_default => Plan::Alert(if actions.len() >= 2 {
            AlertButtons::YesNoCancel
        } else {
            AlertButtons::OkCancel
        }),
        Os::Linux if first_is_default => Plan::Alert(if actions.len() >= 2 {
            AlertButtons::YesNoCancelCustom(first.label.clone(), actions[1].label.clone(), cancel)
        } else {
            AlertButtons::OkCancelCustom(first.label.clone(), cancel)
        }),
        _ => Plan::InApp,
    }
}

/// The choice an alert's default button (Return) picks: its first button.
pub fn alert_default(q: &Question, buttons: &AlertButtons) -> Choice {
    let first = match buttons {
        AlertButtons::OkCancel => return rfd_choice(q, rfd::MessageDialogResult::Ok),
        AlertButtons::YesNoCancel => return rfd_choice(q, rfd::MessageDialogResult::Yes),
        AlertButtons::OkCancelCustom(a, _) | AlertButtons::YesNoCancelCustom(a, _, _) => a.clone(),
    };
    rfd_choice(q, rfd::MessageDialogResult::Custom(first))
}

fn rfd_dialog(q: &Question, buttons: AlertButtons) -> rfd::MessageDialog {
    use rfd::MessageLevel;
    let destructive = q.buttons.iter().any(|b| b.role == Role::Destructive);
    let d = rfd::MessageDialog::new()
        .set_level(if destructive {
            MessageLevel::Warning
        } else {
            MessageLevel::Info
        })
        .set_buttons(buttons.to_rfd());
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
            Question::export_overwrite(std::path::Path::new("a.html")),
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
    fn native_plans_honour_the_default_button() {
        let all = [
            Question::unsaved("a.md"),
            Question::trash("a.md", false),
            Question::replace_theme("t", &["t.css".into()]),
            Question::export_overwrite(std::path::Path::new("a.html")),
            Question::open_link("vscode://x"),
            Question::create_note("New idea.md", "Inbox"),
            Question::reset_settings("Editor"),
        ];
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            for q in &all {
                match plan(q, os) {
                    // whatever is shown natively, Return picks the question's default
                    Plan::Alert(b) => assert_eq!(alert_default(q, &b), q.default, "{os:?} {:?}", q.kind),
                    Plan::InApp => {
                        assert_ne!(os, Os::Mac, "macOS can honour every default");
                        assert_eq!(q.default, Choice::Cancel, "{os:?} {:?}", q.kind);
                    }
                }
            }
        }
        // the concrete button sets
        let trash = Question::trash("a.md", false);
        let label = trash.button(Choice::MoveToTrash).unwrap().label.clone();
        assert_eq!(
            plan(&trash, Os::Mac),
            Plan::Alert(AlertButtons::OkCancelCustom("Cancel".into(), label))
        );
        assert_eq!(plan(&trash, Os::Windows), Plan::InApp);
        assert_eq!(plan(&trash, Os::Linux), Plan::InApp);
        let unsaved = Question::unsaved("a.md");
        assert_eq!(
            plan(&unsaved, Os::Mac),
            Plan::Alert(AlertButtons::YesNoCancelCustom(
                "Save".into(),
                "Cancel".into(),
                "Don’t Save".into()
            ))
        );
        assert_eq!(plan(&unsaved, Os::Windows), Plan::Alert(AlertButtons::YesNoCancel));
        assert_eq!(
            plan(&unsaved, Os::Linux),
            Plan::Alert(AlertButtons::YesNoCancelCustom(
                "Save".into(),
                "Don’t Save".into(),
                "Cancel".into()
            ))
        );
        // Esc: every alert has a Cancel answer
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            if let Plan::Alert(_) = plan(&unsaved, os) {
                assert_eq!(rfd_choice(&unsaved, rfd::MessageDialogResult::Cancel), Choice::Cancel);
            }
        }
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
