//! Block package glue (ADR 0019): looks up the open document's binding on a worker thread
//! whenever the document's file changes, and applies the user's answers.
//! Without a config folder (tests, measurements) nothing is looked up at all.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use egui::Context;
use markise_package::{InstalledPackage, Problem, Resolution, Sha256Hex, Store};

use super::MarkiseApp;
use crate::package_ui::{self, PackageAction, PackageStatus};
use crate::themes::ConfigDir;

enum Done {
    Resolved(Resolution),
    Accepted(Result<InstalledPackage, Problem>),
    Forgot(Result<(), String>),
}

/// The package state of the session.
#[derive(Default)]
pub(super) struct PackageState {
    store: Option<Store>,
    /// The file the state was looked up for.
    file: Option<PathBuf>,
    running: Option<Receiver<Done>>,
    /// The newest job's thread: the next job waits for it.
    last_job: Option<std::thread::JoinHandle<()>>,
    pub(super) status: PackageStatus,
    /// Hashes answered with Not Now in this session: no second card for them.
    not_now: std::collections::HashSet<Sha256Hex>,
}

impl PackageState {
    /// No job outstanding (tests: [`MarkiseApp::background_settled`]).
    pub(super) fn is_idle(&self) -> bool {
        self.running.is_none()
    }

    pub(super) fn new(config: &ConfigDir) -> PackageState {
        PackageState {
            store: match config {
                ConfigDir::None => None,
                ConfigDir::Dir(dir) => Some(Store::in_config(dir)),
            },
            ..Default::default()
        }
    }
}

impl MarkiseApp {
    /// The current document's package, when it has one in use.
    pub fn block_package(&self) -> Option<&InstalledPackage> {
        match &self.package.status {
            PackageStatus::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// What the current document's binding came to.
    pub fn block_package_status(&self) -> &PackageStatus {
        &self.package.status
    }

    /// Starts a job for the current document. A job of an earlier document is dropped with its
    /// receiver.
    fn package_job(&mut self, job: impl FnOnce(&Store) -> Option<Done> + Send + 'static) {
        let Some(store) = self.package.store.clone() else {
            return;
        };
        let (tx, rx) = channel();
        let wake = self.wake_handle();
        // Jobs run in the order they were started, also across documents: a lookup must see the
        // store as an earlier Stop Using or Use Package leaves it, whatever became of that job's
        // receiver.
        let earlier = self.package.last_job.take();
        let spawned = std::thread::Builder::new()
            .name("markise-block-package".into())
            .spawn(move || {
                if let Some(earlier) = earlier {
                    let _ = earlier.join();
                }
                // `None`: nothing to report, so no frame is spent on the common unbound case
                match job(&store) {
                    Some(done) => {
                        let _ = tx.send(done);
                        wake();
                    }
                    None => drop(tx),
                }
            });
        match spawned {
            Ok(handle) => {
                self.package.running = Some(rx);
                self.package.last_job = Some(handle);
            }
            Err(_) => {
                self.package.running = None;
                self.package_job_lost();
            }
        }
    }

    /// A job ended without an answer. A lookup has nothing to show; an install must not stay
    /// on screen as running.
    fn package_job_lost(&mut self) {
        if matches!(self.package.status, PackageStatus::Installing(_)) {
            let problem = Problem::Store("the install stopped unexpectedly".to_string());
            self.package.status = PackageStatus::Failed(problem);
        }
    }

    fn start_package_lookup(&mut self, file: PathBuf) {
        self.package_job(move |store| {
            let home = std::env::home_dir();
            match markise_package::resolve(&file, store, home.as_deref()) {
                Resolution::Unbound => None,
                other => Some(Done::Resolved(other)),
            }
        });
    }

    /// Per frame: follows the document's file and picks up finished jobs. Costs a path
    /// comparison unless something changed.
    pub(super) fn poll_block_package(&mut self) {
        if self.package.store.is_none() {
            return;
        }
        let current = self.document_path();
        if current != self.package.file.as_deref() {
            let current = current.map(PathBuf::from);
            self.package.file = current.clone();
            self.package.status = PackageStatus::None;
            self.package.running = None;
            if let Some(file) = current {
                self.start_package_lookup(file);
            }
        }
        let Some(rx) = &self.package.running else {
            return;
        };
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            // a finished lookup with nothing to report, or a worker that died
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.package.running = None;
                self.package_job_lost();
                return;
            }
        };
        self.package.running = None;
        match done {
            Done::Resolved(Resolution::Unbound) => self.package.status = PackageStatus::None,
            Done::Resolved(Resolution::Ask(p)) => {
                self.package.status = if self.package.not_now.contains(&p.sha256) {
                    PackageStatus::NotInUse(p)
                } else {
                    PackageStatus::Asking(p)
                };
            }
            Done::Resolved(Resolution::Ready(p)) | Done::Accepted(Ok(p)) => {
                self.package.status = PackageStatus::Ready(p);
            }
            Done::Resolved(Resolution::Failed { problem, .. }) | Done::Accepted(Err(problem)) => {
                self.package.status = PackageStatus::Failed(problem);
            }
            Done::Forgot(Ok(())) => {
                // looked up again from the disk; Not Now keeps the card away
                if let Some(file) = self.package.file.clone() {
                    self.start_package_lookup(file);
                }
            }
            Done::Forgot(Err(e)) => self.package.status = PackageStatus::Failed(Problem::Store(e)),
        }
    }

    /// Applies an answer (tests drive it directly too).
    pub fn block_package_action(&mut self, action: PackageAction) {
        if action == PackageAction::Forget && self.block_draft_changed() {
            self.set_ask(super::Ask::BlockDraft(super::block_form::DraftNext::StopPackage));
            return;
        }
        match (action, std::mem::take(&mut self.package.status)) {
            (PackageAction::Accept, PackageStatus::Asking(p) | PackageStatus::NotInUse(p)) => {
                self.package.not_now.remove(&p.sha256);
                let pending = p.clone();
                self.package.status = PackageStatus::Installing(p);
                self.package_job(move |store| Some(Done::Accepted(markise_package::accept(&pending, store))));
            }
            (PackageAction::NotNow, PackageStatus::Asking(p)) => {
                self.package.not_now.insert(p.sha256.clone());
                self.package.status = PackageStatus::NotInUse(p);
            }
            (PackageAction::Review, PackageStatus::NotInUse(p)) => {
                self.package.status = PackageStatus::Asking(p);
            }
            (PackageAction::Forget, PackageStatus::Ready(p)) => {
                self.package.not_now.insert(p.sha256.clone());
                let (library, sha256) = (p.index.id().library.clone(), p.sha256.clone());
                let hooks = self.hooks.clone();
                self.package_job(move |store| {
                    if let Some(hook) = &hooks.before_package_forget {
                        hook();
                    }
                    Some(Done::Forgot(store.forget(&library, &sha256)))
                });
            }
            (_, status) => self.package.status = status,
        }
    }

    /// Draws the question.
    pub(super) fn package_card(&mut self, ctx: &Context) {
        let PackageStatus::Asking(pending) = &self.package.status else {
            return;
        };
        if self.is_asking() {
            return; // a question is open: nothing competes with it
        }
        let mut actions = Vec::new();
        package_ui::show_card(
            ctx,
            pending,
            self.web_helpers.is_some(),
            self.updates.card_height(ctx),
            &mut actions,
        );
        for a in actions {
            self.block_package_action(a);
        }
    }

    /// The status bar's item.
    pub(super) fn package_status_item(&mut self, ui: &mut egui::Ui) {
        if matches!(self.package.status, PackageStatus::None) {
            return;
        }
        let mut actions = Vec::new();
        package_ui::status_item(ui, &self.package.status, &mut actions);
        for a in actions {
            self.block_package_action(a);
        }
    }
}
