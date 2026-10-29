//! Review comments and suggestions (ADR 0012, issue #69): the CriticMarkup switch (View →
//! Comments & Suggestions, setting `markdown.criticMarkup`), review commands from menus, the
//! context menu, the hover bar and the comments panel, and File → Save Copy Without
//! Comments.
//!
//! The comments panel lists the document's marks ([`markise_doc::review::all_marks`]) only
//! while it is shown, recomputed when the text changed and the typing has paused
//! ([`PANEL_DEBOUNCE`]); a document without marks costs one byte scan. Nothing here runs per
//! keystroke while the panel is hidden.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Context, RichText, Ui};
use markise_doc::review::{self, ReviewMark};
use markise_doc::{CriticKind, Params, Selection, planners};
use markise_engine::save::{Expect, SaveError, SaveOutcome};
use markise_layout::block::color;

use super::{Ask, Banner, MarkiseApp};
use crate::sidebar::SideLook;

/// The panel's list is refreshed this long after the last edit.
pub const PANEL_DEBOUNCE: Duration = Duration::from_millis(250);

/// The listed marks of one document revision.
struct PanelList {
    doc_id: u64,
    counter: u64,
    marks: Vec<ReviewMark>,
}

/// A clean copy being written.
struct CopyJob {
    rx: Receiver<Result<SaveOutcome, SaveError>>,
    path: PathBuf,
}

/// Review state of the application.
#[derive(Default)]
pub(crate) struct ReviewState {
    /// CriticMarkup may be on at all (the startup parse options allow it).
    pub(crate) allowed: bool,
    list: Option<PanelList>,
    /// The revision counter seen changing last, and when.
    changed: Option<(u64, u64, Instant)>,
    copy: Option<CopyJob>,
}

impl ReviewState {
    pub(crate) fn new(allowed: bool) -> Self {
        ReviewState {
            allowed,
            ..Default::default()
        }
    }
}

/// What the user chose in the comments panel.
enum PanelAction {
    Jump(usize),
    Run(&'static str, Option<usize>),
}

impl MarkiseApp {
    // ------------------------------------------------------------------ the switch

    /// Whether CriticMarkup comments and suggestions are parsed (View → Comments &
    /// Suggestions, setting `markdown.criticMarkup`).
    pub fn critic_markup(&self) -> bool {
        self.parse.critic_markup
    }

    /// View → Comments & Suggestions: switches the dialect extension (persisted) and
    /// re-parses the open document in the background; the text is not changed.
    pub fn set_critic_markup(&mut self, on: bool) {
        self.themes.set_critic_markup(on);
        self.review_poll_setting();
    }

    /// Per frame: applies a changed setting.
    pub(super) fn review_poll_setting(&mut self) {
        let want = self.review.allowed && self.themes.critic_markup();
        if want != self.parse.critic_markup {
            self.parse.critic_markup = want;
            self.review.list = None;
            self.reparse_document("Comments & Suggestions");
        }
        self.poll_clean_copy();
    }

    /// File → Export → Include Comments (setting `markdown.exportComments`).
    pub fn export_comments(&self) -> bool {
        self.themes.export_comments()
    }

    pub fn set_export_comments(&mut self, on: bool) {
        self.themes.set_export_comments(on);
    }

    // ------------------------------------------------------------------ commands

    /// Runs review command `id` on the rendered view (`at`: the offset it acts at, else the
    /// selection). Returns whether it ran.
    pub fn run_review(&mut self, ctx: &Context, id: &str, at: Option<usize>) -> bool {
        let Some(d) = self.doc.as_mut() else { return false };
        let params = match at {
            Some(a) => Params::new().with("at", markise_doc::ParamValue::Int(a as i64)),
            None => Params::new(),
        };
        let ok = d.viewer.run(ctx, self.parley.as_mut(), id, &params).is_ok();
        if ok {
            d.viewer.request_refocus();
            self.review.list = None;
        }
        ok
    }

    /// The document's review marks as the comments panel lists them (computed now; tests).
    pub fn review_marks(&self) -> Vec<ReviewMark> {
        self.doc
            .as_ref()
            .map(|d| review::all_marks(d.viewer.ed.session.doc()))
            .unwrap_or_default()
    }

    /// Puts the caret at `offset` and scrolls its block into view (comments panel).
    pub fn jump_to(&mut self, offset: usize) {
        let Some(d) = self.doc.as_mut() else { return };
        let len = d.viewer.ed.session.text().len();
        let off = offset.min(len);
        d.viewer.ed.set_selection(Selection::caret(off));
        d.viewer.ed.refresh_focus();
        let model = d.viewer.model().clone();
        let item = model.item_at_offset(off);
        d.viewer.ed.view.scroll_to_item(item);
        self.sbs.sync_source = true;
        d.viewer.request_refocus();
    }

    // ------------------------------------------------------------------ the panel

    /// The comments panel (activity bar → Comments): comments and suggestions in document
    /// order; a click jumps to the mark; Accept / Reject on suggestions, Delete on comments;
    /// Accept All / Reject All on top.
    pub(super) fn comments_panel(&mut self, ui: &mut Ui, look: &SideLook) {
        if !self.parse.critic_markup {
            ui.label(RichText::new("Comments").font(look.bold_font.clone()).color(look.text));
            ui.add_space(4.0);
            ui.weak("Comments and suggestions are off.");
            if ui.button("Turn On").clicked() {
                self.set_critic_markup(true);
            }
            return;
        }
        let Some(d) = self.doc.as_ref() else {
            ui.weak("No document");
            return;
        };
        let (doc_id, counter) = (d.id, d.viewer.ed.session.revision().counter);
        let now = Instant::now();
        match self.review.changed {
            Some((id, c, _)) if id == doc_id && c == counter => {}
            _ => self.review.changed = Some((doc_id, counter, now)),
        }
        let stale = self
            .review
            .list
            .as_ref()
            .is_none_or(|l| l.doc_id != doc_id || l.counter != counter);
        let since = self.review.changed.map_or(Duration::MAX, |(_, _, t)| now - t);
        let fresh_doc = self.review.list.as_ref().is_none_or(|l| l.doc_id != doc_id);
        if stale && (fresh_doc || since >= PANEL_DEBOUNCE) {
            self.review.list = Some(PanelList {
                doc_id,
                counter,
                marks: review::all_marks(d.viewer.ed.session.doc()),
            });
        } else if stale {
            ui.ctx().request_repaint_after(PANEL_DEBOUNCE - since);
        }
        let marks: &[ReviewMark] = self.review.list.as_ref().map_or(&[], |l| &l.marks);
        let suggestions = marks.iter().filter(|m| m.is_suggestion()).count();
        let comments = marks
            .iter()
            .filter(|m| m.kind == CriticKind::Comment || (m.kind == CriticKind::Highlight && !has_comment(marks, m)))
            .count();
        let rc = self.theme.review_colors();
        let mut act = None;
        ui.label(RichText::new("Comments").font(look.bold_font.clone()).color(look.text));
        ui.label(
            RichText::new(format!(
                "{comments} comment{} · {suggestions} suggestion{}",
                if comments == 1 { "" } else { "s" },
                if suggestions == 1 { "" } else { "s" }
            ))
            .font(look.small_font.clone())
            .color(look.muted),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(suggestions > 0, egui::Button::new("Accept All"))
                .clicked()
            {
                act = Some(PanelAction::Run("review.acceptAll", None));
            }
            if ui
                .add_enabled(suggestions > 0, egui::Button::new("Reject All"))
                .clicked()
            {
                act = Some(PanelAction::Run("review.rejectAll", None));
            }
        });
        ui.add_space(6.0);
        if marks.is_empty() {
            ui.weak("No comments or suggestions. Select text and press Add Comment.");
        }
        egui::ScrollArea::vertical()
            .id_salt("markise-comments-list")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let mut i = 0;
                while i < marks.len() {
                    let m = &marks[i];
                    // a comment attached to the previous mark is shown with it
                    let attached = marks.get(i + 1).filter(|c| c.kind == CriticKind::Comment && c.attached);
                    ui.push_id(("markise-review-row", m.range.start), |ui| {
                        if let Some(a) = row(ui, look, &rc, m, attached) {
                            act = Some(a);
                        }
                    });
                    i += if attached.is_some() { 2 } else { 1 };
                    ui.add_space(4.0);
                }
            });
        match act {
            Some(PanelAction::Jump(off)) => self.jump_to(off),
            Some(PanelAction::Run(id, at)) => {
                let ctx = ui.ctx().clone();
                self.run_review(&ctx, id, at);
            }
            None => {}
        }
    }

    // ------------------------------------------------------------------ clean copy

    /// File → Save Copy Without Comments…: asks (when the document has pending suggestions)
    /// whether the copy accepts or rejects them, then for the copy's file, and writes it
    /// through the checked save path. The open document and its file are not changed.
    pub fn save_copy_without_comments(&mut self) {
        if self.doc.is_none() || self.review.copy.is_some() {
            return;
        }
        let n = self
            .doc
            .as_ref()
            .map(|d| {
                review::all_marks(d.viewer.ed.session.doc())
                    .iter()
                    .filter(|m| m.is_suggestion())
                    .count()
            })
            .unwrap_or(0);
        if n > 0 && self.parse.critic_markup {
            self.set_ask(Ask::CleanCopy { suggestions: n });
        } else {
            self.write_clean_copy(false);
        }
    }

    /// The clean copy's text: comments and highlights removed, suggestions accepted (or
    /// rejected); the document text itself when there is nothing to remove.
    pub fn clean_copy_text(&self, reject: bool) -> Option<String> {
        let d = self.doc.as_ref()?;
        let doc = d.viewer.ed.session.doc();
        let text = doc.text().to_string();
        let req = markise_doc::PlanRequest {
            params: Params::new().with("reject", markise_doc::ParamValue::Bool(reject)),
            ..Default::default()
        };
        match doc.plan(planners::REVIEW_CLEAN_COPY, &req) {
            Ok(plan) => {
                let mut t = text;
                for step in plan.steps {
                    t = step.patches.apply_to(&t);
                }
                Some(t)
            }
            Err(_) => Some(text),
        }
    }

    /// Asks for the copy's file and starts writing it.
    pub(super) fn write_clean_copy(&mut self, reject: bool) {
        let Some(text) = self.clean_copy_text(reject) else {
            return;
        };
        let Some(d) = self.doc.as_ref() else { return };
        let stem = d
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let name = format!("{stem} (clean).md");
        let dir = d.path.parent().map(Path::to_path_buf);
        let own = d.file().map(Path::to_path_buf);
        let Some(p) = self.pick_save_path_with("Save Copy Without Comments", dir, &name) else {
            return;
        };
        let (p, _) = super::files::normalize_target(p);
        if own.as_deref().is_some_and(|o| same_path(o, &p)) {
            self.banner = Some(Banner::Error(
                "The copy cannot replace the document itself. Choose another file name.".into(),
            ));
            return;
        }
        let expect = match markise_engine::save::disk_version(&p) {
            // the dialog confirmed replacing that file
            Ok(Some(v)) => Expect::Version(v),
            Ok(None) => Expect::Missing,
            Err(e) => {
                self.banner = Some(Banner::Error(format!("Cannot save to {}: {e}", p.display())));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.wake_handle();
        let target = p.clone();
        let spawned = std::thread::Builder::new()
            .name("markise-clean-copy".into())
            .spawn(move || {
                let r = markise_engine::save::save_checked(&target, &text, expect);
                let _ = tx.send(r);
                wake();
            });
        match spawned {
            Ok(_) => self.review.copy = Some(CopyJob { rx, path: p }),
            Err(e) => self.banner = Some(Banner::Error(format!("Cannot save the copy: {e}"))),
        }
    }

    fn poll_clean_copy(&mut self) {
        let Some(job) = &self.review.copy else { return };
        let r = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Err(SaveError::Io("the save worker stopped unexpectedly".into())),
        };
        let path = job.path.clone();
        self.review.copy = None;
        self.banner = Some(match r {
            Ok(_) => Banner::Info(format!("Saved a copy without comments: {}", path.display())),
            Err(e) => Banner::Error(format!("The copy was not saved: {e}")),
        });
    }

    /// A clean copy is being written (tests).
    pub fn is_saving_copy(&self) -> bool {
        self.review.copy.is_some()
    }
}

/// Whether highlight `m` has an attached comment right after it.
fn has_comment(marks: &[ReviewMark], m: &ReviewMark) -> bool {
    marks
        .iter()
        .any(|c| c.kind == CriticKind::Comment && c.attached && c.range.start == m.range.end)
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A clickable label (a click on a row's text jumps to the mark).
fn link_label(ui: &mut Ui, text: RichText, jump: &mut bool) {
    let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    *jump |= r.clicked();
}

/// One panel row: the mark (and its attached comment); returns the chosen action.
fn row(
    ui: &mut Ui,
    look: &SideLook,
    rc: &markise_theme::ReviewColors,
    m: &ReviewMark,
    attached: Option<&ReviewMark>,
) -> Option<PanelAction> {
    let mut act = None;
    let mut jump = false;
    let (badge, badge_color) = match m.kind {
        CriticKind::Insert => ("Insert", color(rc.insert)),
        CriticKind::Delete => ("Delete", color(rc.delete)),
        CriticKind::Substitute => ("Replace", color(rc.insert)),
        _ => ("Comment", look.accent),
    };
    egui::Frame::new()
        .fill(look.hover_bg.gamma_multiply(0.5))
        .corner_radius(look.radius)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            link_label(
                ui,
                RichText::new(badge).font(look.small_font.clone()).color(badge_color),
                &mut jump,
            );
            match m.kind {
                CriticKind::Insert => {
                    link_label(
                        ui,
                        RichText::new(format!("+ {}", short(&m.text))).color(color(rc.insert)),
                        &mut jump,
                    );
                }
                CriticKind::Delete => {
                    link_label(
                        ui,
                        RichText::new(short(&m.text)).strikethrough().color(color(rc.delete)),
                        &mut jump,
                    );
                }
                CriticKind::Substitute => {
                    // old text struck, the new text under it (no arrow glyph: the UI font may
                    // lack it)
                    link_label(
                        ui,
                        RichText::new(short(&m.old_text))
                            .strikethrough()
                            .color(color(rc.delete)),
                        &mut jump,
                    );
                    link_label(ui, RichText::new(short(&m.text)).color(color(rc.insert)), &mut jump);
                }
                CriticKind::Highlight => {
                    link_label(
                        ui,
                        RichText::new(format!("“{}”", short(&m.text))).background_color(color(rc.highlight_bg)),
                        &mut jump,
                    );
                }
                _ => comment_text(ui, look, m, &mut jump),
            }
            if let Some(c) = attached {
                comment_text(ui, look, c, &mut jump);
            }
            ui.horizontal(|ui| {
                if m.is_suggestion() {
                    if ui.small_button("Accept").clicked() {
                        act = Some(PanelAction::Run("review.accept", Some(m.content.start)));
                    }
                    if ui.small_button("Reject").clicked() {
                        act = Some(PanelAction::Run("review.reject", Some(m.content.start)));
                    }
                } else if ui.small_button("Delete").clicked() {
                    act = Some(PanelAction::Run("review.deleteComment", Some(m.content.start)));
                }
            });
        });
    if act.is_none() && jump {
        act = Some(PanelAction::Jump(m.range.start));
    }
    act
}

/// `s` on one line, at most 120 characters.
fn short(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 120 {
        format!("{}…", one.chars().take(119).collect::<String>())
    } else {
        one
    }
}

fn comment_text(ui: &mut Ui, look: &SideLook, m: &ReviewMark, jump: &mut bool) {
    let (author, text) = m.author();
    ui.horizontal_wrapped(|ui| {
        if let Some(a) = author {
            link_label(ui, RichText::new(format!("@{a}")).strong().color(look.text), jump);
        }
        let t = if text.is_empty() {
            "(empty comment)".to_string()
        } else {
            short(text)
        };
        link_label(ui, RichText::new(t).color(look.text), jump);
    });
}
