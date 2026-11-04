//! Block package consent and status (ADR 0019, issue #178): the question whether to use a
//! folder's package, as a non-modal card like the update question, and the status bar's item.
//! Documents without a binding show neither. The decisions live in [`markise_package`]; the
//! app glue is `app/package.rs`.

use egui::{Context, RichText, Ui};
use markise_package::{InstalledPackage, Pending, Problem, Sha256Hex};

/// Error text from a package can be long, and the package chooses it.
const MAX_SHOWN_CHARS: usize = 1200;

/// What the current document's binding came to.
#[derive(Clone, Debug, Default)]
pub enum PackageStatus {
    /// No binding, no document, or still looking: nothing is shown.
    #[default]
    None,
    /// The card asks.
    Asking(Pending),
    /// The user said Not Now: the status bar offers it again.
    NotInUse(Pending),
    /// Yes was answered; verifying and unpacking.
    Installing(Pending),
    Ready(InstalledPackage),
    Failed(Problem),
}

/// What the user did in the card or the status bar item this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageAction {
    Accept,
    NotNow,
    /// Show the question again.
    Review,
    /// Withdraw the consent.
    Forget,
}

fn short_hash(sha256: &Sha256Hex) -> String {
    let sha256 = sha256.as_str();
    format!(
        "{}…{}",
        &sha256[..8.min(sha256.len())],
        &sha256[sha256.len().saturating_sub(8)..]
    )
}

fn name_of(library: &str, version: &str) -> String {
    format!("{library} {version}")
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(MAX_SHOWN_CHARS).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

fn small(ui: &mut Ui, text: String) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).weak()).wrap());
}

/// Draws the question, `above` points higher than the update card's place.
pub fn show_card(ctx: &Context, pending: &Pending, above: f32, out: &mut Vec<PackageAction>) {
    egui::Area::new(egui::Id::unique("markise-package-card"))
        .order(egui::Order::Middle)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -36.0 - above))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(360.0);
                let name = name_of(&pending.id.library, &pending.id.version);
                ui.label(RichText::new(format!("Use the block package {name}?")).strong());
                small(
                    ui,
                    "This folder asks for it. A package belongs to the site that made it: it \
                     describes the site's blocks and holds code that later versions of Markise \
                     may run in isolation. Nothing runs now. Use it if you trust where this \
                     folder came from."
                        .to_string(),
                );
                if let Some(previous) = &pending.previous {
                    small(
                        ui,
                        format!(
                            "It differs from the package you accepted before, {}.",
                            name_of(&previous.library, &previous.version)
                        ),
                    );
                }
                small(ui, details(pending));
                ui.horizontal(|ui| {
                    if ui.button("Use Package").clicked() {
                        out.push(PackageAction::Accept);
                    }
                    if ui.button("Not Now").clicked() {
                        out.push(PackageAction::NotNow);
                    }
                });
            });
        });
}

fn details(pending: &Pending) -> String {
    format!(
        "{} blocks · {} · sha256 {}\n{}",
        pending.block_count,
        pending.binding.source,
        short_hash(&pending.sha256),
        pending.binding.file.display()
    )
}

/// The status bar's item: which package the document uses, or why not.
pub fn status_item(ui: &mut Ui, status: &PackageStatus, out: &mut Vec<PackageAction>) {
    match status {
        PackageStatus::None => {}
        PackageStatus::Asking(p) | PackageStatus::NotInUse(p) => {
            let name = name_of(&p.id.library, &p.id.version);
            let label = RichText::new(format!("Blocks: {name} · not in use")).weak();
            ui.menu_button(label, |ui| {
                ui.set_max_width(360.0);
                small(ui, details(p));
                if ui.button("Use This Package…").clicked() {
                    out.push(PackageAction::Review);
                    ui.close();
                }
            });
        }
        PackageStatus::Installing(p) => {
            ui.spinner();
            ui.weak(format!("Installing {}…", name_of(&p.id.library, &p.id.version)));
        }
        PackageStatus::Ready(p) => {
            let id = p.index.id();
            let label = RichText::new(format!("Blocks: {}", name_of(&id.library, &id.version))).weak();
            ui.menu_button(label, |ui| {
                ui.set_max_width(360.0);
                small(
                    ui,
                    format!(
                        "{} blocks · sha256 {}\n{}",
                        p.index.block_count(),
                        short_hash(&p.sha256),
                        p.binding.file.display()
                    ),
                );
                if ui.button("Stop Using This Package").clicked() {
                    out.push(PackageAction::Forget);
                    ui.close();
                }
            });
        }
        PackageStatus::Failed(problem) => {
            let color = ui.visuals().warn_fg_color;
            ui.menu_button(RichText::new("Block package problem").color(color), |ui| {
                ui.set_max_width(420.0);
                ui.add(egui::Label::new(clip(&problem.to_string())).wrap());
                small(
                    ui,
                    "Blocks show as simple cards. The document is not affected.".to_string(),
                );
            });
        }
    }
}
