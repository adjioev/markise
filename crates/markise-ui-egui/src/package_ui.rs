//! Block package consent and status (ADR 0019, issue #178): the question whether to use a
//! folder's package, as a non-modal card like the update question, and the status bar's item.
//! Documents without a binding show neither. The decisions live in [`markise_package`]; the
//! app glue is `app/package.rs`.

use egui::{Context, RichText, Ui};
use markise_package::{InstalledPackage, Pending, Problem, Sha256Hex, printable};

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

/// What accepting a package means, on a system that can run its code or on one that cannot. All of
/// the wording of this question about what runs is here: it is a consent, and has to stay true.
fn consent_text(name: &str, runs_code: bool) -> String {
    if runs_code {
        format!(
            "This document uses the block package {name}. If you allow it, Markise runs the package's code in an \
             isolated background process to draw its blocks, with no network access and no access to your files \
             except the package and images in this document's folder. Use it if you trust where this folder came from."
        )
    } else {
        format!(
            "This document uses the block package {name}. If you allow it, its data is used for the views and forms, \
             and no code runs. Use it if you trust where this folder came from."
        )
    }
}

/// Draws the question, `above` points higher than the update card's place. `runs_code`: there is a
/// helper here to run the package's code with.
pub fn show_card(ctx: &Context, pending: &Pending, runs_code: bool, above: f32, out: &mut Vec<PackageAction>) {
    egui::Area::new(egui::Id::unique("markise-package-card"))
        .order(egui::Order::Middle)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -36.0 - above))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(360.0);
                let name = name_of(&pending.id.library, &pending.id.version);
                ui.label(RichText::new(format!("Use the block package {name}?")).strong());
                small(ui, consent_text(&name, runs_code));
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

/// Above a package's notes: what they are and that they change nothing.
pub const NOTES_HEAD: &str = "This version of Markise does not read these and ignores them:";

fn notes_count(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => " · 1 note".to_string(),
        n => format!(" · {n} notes"),
    }
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
            let notes = p.index.notes();
            let label = RichText::new(format!(
                "Blocks: {}{}",
                name_of(&id.library, &id.version),
                notes_count(notes.len())
            ))
            .weak();
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
                if !notes.is_empty() {
                    // a package wrote these: each stays one line, and nothing in it hides or turns the text around
                    let text: Vec<String> = notes.iter().map(|n| printable(&n.to_string())).collect();
                    small(ui, clip(&format!("{NOTES_HEAD}\n{}", text.join("\n"))));
                }
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

#[cfg(test)]
mod consent_tests {
    use super::consent_text;

    #[test]
    fn the_question_says_what_runs_where_something_does() {
        let text = consent_text("lib 1.2.3", true);
        assert!(text.contains("block package lib 1.2.3"), "{text}");
        assert!(text.contains("runs the package's code"), "{text}");
        assert!(text.contains("no network access"), "{text}");
        assert!(!text.contains("later"), "{text}");
        assert!(!text.contains("Nothing runs"), "{text}");
    }

    #[test]
    fn and_only_what_happens_where_nothing_can() {
        let text = consent_text("lib 1.2.3", false);
        assert!(text.contains("no code runs"), "{text}");
        assert!(!text.contains("background process"), "{text}");
        assert!(!text.contains("later"), "{text}");
    }
}
