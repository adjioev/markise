//! Review comments and suggestions (ADR 0012, issue #69): the CriticMarkup switch (View →
//! Comments & Suggestions, setting `markdown.criticMarkup`), review commands from menus, the
//! context menu, the hover bar and the comments panel, and File → Save Copy Without
//! Comments.
//!
//! The comments panel lists the document's marks ([`markise_doc::review::all_marks`]) only
//! while it is shown. The list is built on a worker from a text snapshot when the text
//! changed and the typing has paused ([`PANEL_DEBOUNCE`]), with every row's display data and
//! the counts precomputed; only the visible rows are drawn. A row's actions carry the
//! revision and the mark's identity (kind and source bytes): on a newer revision the mark is
//! found again by identity before anything is applied (review P1-1). Nothing here runs per
//! keystroke while the panel is hidden.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Context, RichText, Ui};
use markise_doc::review::{self, ReviewMark};
use markise_doc::{CriticKind, Params, Selection, planners};
use markise_engine::save::{Expect, SaveError, SaveOutcome};
use markise_engine::settings::CommentDisplay;
use markise_layout::block::color;

use super::{Ask, Banner, MarkiseApp};
use crate::sidebar::SideLook;

/// The panel's list is refreshed this long after the last edit.
pub const PANEL_DEBOUNCE: Duration = Duration::from_millis(250);

/// A comment's preview: its author (`@name:`) and at most [`PREVIEW`] characters of text on
/// one line.
type CommentPreview = (Option<String>, String);

/// Characters of text a panel row shows.
const PREVIEW: usize = 120;

/// One row of the panel: a mark and the comment attached to it, as shown — bounded previews
/// computed on the worker — and a cheap identity of the mark (kind, length and hash of its
/// source bytes) for actions on a newer revision.
struct PanelRow {
    kind: CriticKind,
    start: usize,
    content_start: usize,
    len: usize,
    hash: u64,
    /// The text (substitution: the new text), the old text of a substitution, and the comment
    /// of a comment row.
    text: String,
    old_text: String,
    comment: Option<CommentPreview>,
    /// The comment attached to the mark.
    attached: Option<CommentPreview>,
}

/// Hash of a mark's source bytes (its identity with kind and length).
fn ident_hash(bytes: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn comment_preview(m: &ReviewMark) -> CommentPreview {
    let (author, text) = m.author();
    (author.map(str::to_string), short(text))
}

/// The listed marks of one document revision, with the counts.
struct PanelList {
    doc_id: u64,
    counter: u64,
    rows: Vec<PanelRow>,
    comments: usize,
    suggestions: usize,
}

impl PanelList {
    /// Builds the rows and counts of `marks` (the text they were found in: `text`).
    fn build(doc_id: u64, counter: u64, text: &str, marks: Vec<ReviewMark>) -> PanelList {
        let suggestions = marks.iter().filter(|m| m.is_suggestion()).count();
        let mut rows = Vec::new();
        let mut comments = 0;
        let mut it = marks.into_iter().peekable();
        while let Some(m) = it.next() {
            let attached = it.next_if(|c| c.kind == CriticKind::Comment && c.attached);
            // a comment, a highlight, or a suggestion with a comment: one comment thread
            if matches!(m.kind, CriticKind::Comment | CriticKind::Highlight) || attached.is_some() {
                comments += 1;
            }
            let src = text.get(m.range.clone()).unwrap_or_default();
            rows.push(PanelRow {
                kind: m.kind,
                start: m.range.start,
                content_start: m.content.start,
                len: src.len(),
                hash: ident_hash(src),
                text: short(&m.text),
                old_text: short(&m.old_text),
                comment: (m.kind == CriticKind::Comment).then(|| comment_preview(&m)),
                attached: attached.as_ref().map(comment_preview),
            });
        }
        PanelList {
            doc_id,
            counter,
            rows,
            comments,
            suggestions,
        }
    }
}

/// A list being built on the worker.
struct PendingList {
    rx: Receiver<PanelList>,
}

/// The mark a panel action is for: the revision it was listed at and its identity.
#[derive(Clone, Debug)]
struct Target {
    counter: u64,
    kind: CriticKind,
    start: usize,
    content_start: usize,
    len: usize,
    hash: u64,
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
    pending: Option<PendingList>,
    /// How long typing must pause before the list is rebuilt ([`PANEL_DEBOUNCE`]; tests may
    /// lengthen it).
    debounce: Option<Duration>,
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
    Jump(Target),
    Run(&'static str, Option<Target>),
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
        let display = self.themes.comment_display();
        if let Some(d) = self.doc.as_mut() {
            d.viewer.set_comment_display(display);
        }
    }

    /// How comments are shown in the document (setting `markdown.commentDisplay`).
    pub fn comment_display(&self) -> CommentDisplay {
        self.themes.comment_display()
    }

    /// Preferences → Markdown → Show comments; the comments panel's switch.
    pub fn set_comment_display(&mut self, d: CommentDisplay) {
        self.themes.set_comment_display(d);
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

    /// Sets how long typing must pause before the comments panel's list is rebuilt (tests).
    pub fn set_comments_debounce(&mut self, d: Duration) {
        self.review.debounce = Some(d);
    }

    /// The comments panel shows the list of the current revision (tests).
    pub fn comments_panel_ready(&self) -> bool {
        let Some(d) = self.doc.as_ref() else { return false };
        self.review
            .list
            .as_ref()
            .is_some_and(|l| l.doc_id == d.id && l.counter == d.viewer.ed.session.revision().counter)
    }

    /// The source offset an action on `t` applies at now: its listed position when the text
    /// has not changed since, else the mark found again by kind and source bytes (the nearest
    /// such mark), `None` when it is gone.
    fn resolve_target(&self, t: &Target) -> Option<usize> {
        let d = self.doc.as_ref()?;
        let s = &d.viewer.ed.session;
        if s.revision().counter == t.counter {
            return Some(t.content_start);
        }
        let doc = s.doc();
        let text = doc.text();
        review::all_marks(doc)
            .into_iter()
            .filter(|m| {
                m.kind == t.kind
                    && m.range.len() == t.len
                    && text.get(m.range.clone()).is_some_and(|b| ident_hash(b) == t.hash)
            })
            .min_by_key(|m| m.range.start.abs_diff(t.start))
            .map(|m| m.content.start)
    }

    /// Keeps the panel's list current: a worker lists the marks of a text snapshot once the
    /// typing paused; the stale list stays shown meanwhile.
    fn refresh_panel_list(&mut self, ctx: &Context) {
        if let Some(p) = &self.review.pending
            && let Ok(list) = p.rx.try_recv()
        {
            self.review.pending = None;
            self.review.list = Some(list);
        }
        let Some(d) = self.doc.as_ref() else { return };
        let (doc_id, counter) = (d.id, d.viewer.ed.session.revision().counter);
        let now = Instant::now();
        match self.review.changed {
            Some((id, c, _)) if id == doc_id && c == counter => {}
            _ => self.review.changed = Some((doc_id, counter, now)),
        }
        let current = |l: &PanelList| l.doc_id == doc_id && l.counter == counter;
        if self.review.list.as_ref().is_some_and(current) {
            return;
        }
        // one worker at a time: when it is done, the latest revision is listed next
        if self.review.pending.is_some() {
            return;
        }
        let since = self.review.changed.map_or(Duration::MAX, |(_, _, t)| now - t);
        let fresh_doc = self.review.list.as_ref().is_none_or(|l| l.doc_id != doc_id);
        let debounce = self.review.debounce.unwrap_or(PANEL_DEBOUNCE);
        if !fresh_doc && since < debounce {
            ctx.request_repaint_after(debounce - since);
            return;
        }
        // a snapshot of the text (one copy) parsed and listed off the UI thread
        let text = d.viewer.ed.session.doc().snapshot();
        let format = d.viewer.ed.session.format().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.wake_handle();
        let spawned = std::thread::Builder::new()
            .name("markise-review-list".into())
            .spawn(move || {
                let doc = format.parse(text.to_string(), &markise_doc::FormatOptions::default());
                let marks = review::all_marks(doc.as_ref());
                let _ = tx.send(PanelList::build(doc_id, counter, doc.text(), marks));
                wake();
            });
        if spawned.is_ok() {
            self.review.pending = Some(PendingList { rx });
        }
    }

    /// The comments panel (activity bar → Comments): comments and suggestions in document
    /// order; a click jumps to the mark; Accept / Reject on suggestions, Delete on comments;
    /// Accept All / Reject All on top.
    pub(super) fn comments_panel(&mut self, ui: &mut Ui, look: &SideLook) {
        ui.label(RichText::new("Comments").font(look.bold_font.clone()).color(look.text));
        if !self.parse.critic_markup {
            ui.add_space(4.0);
            ui.weak("Comments and suggestions are off.");
            if ui.button("Turn On").clicked() {
                self.set_critic_markup(true);
            }
            return;
        }
        if self.doc.is_none() {
            ui.weak("No document");
            return;
        }
        self.refresh_panel_list(&ui.ctx().clone());
        let rc = self.theme.review_colors();
        let mut act = None;
        let Some(list) = self
            .review
            .list
            .as_ref()
            .filter(|l| Some(l.doc_id) == self.doc.as_ref().map(|d| d.id))
        else {
            ui.weak("Listing comments…");
            return;
        };
        let (comments, suggestions) = (list.comments, list.suggestions);
        // how comments are shown in the document: the same setting as Preferences
        let display = self.themes.comment_display();
        let picked = display_toggle(ui, look, display);
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
        if list.rows.is_empty() {
            ui.weak("No comments or suggestions. Select text and press Add Comment.");
        }
        // fixed-height rows: only the visible ones are built
        let line = ui.text_style_height(&egui::TextStyle::Body);
        let row_h = (line + 4.0) * 3.0 + 18.0;
        let counter = list.counter;
        egui::ScrollArea::vertical()
            .id_salt("markise-comments-list")
            .auto_shrink([false, true])
            .show_rows(ui, row_h, list.rows.len(), |ui, range| {
                for r in &list.rows[range] {
                    ui.push_id(("markise-review-row", r.start), |ui| {
                        if let Some(a) = row(ui, look, &rc, r, counter, row_h) {
                            act = Some(a);
                        }
                    });
                }
            });
        if let Some(d) = picked {
            self.themes.set_comment_display(d);
        }
        let ctx = ui.ctx().clone();
        match act {
            Some(PanelAction::Jump(t)) => {
                if let Some(off) = self.resolve_target(&t) {
                    self.jump_to(off.saturating_sub(t.content_start - t.start));
                }
            }
            Some(PanelAction::Run(id, t)) => match t {
                None => {
                    self.run_review(&ctx, id, None);
                }
                Some(t) => match self.resolve_target(&t) {
                    Some(off) => {
                        self.run_review(&ctx, id, Some(off));
                    }
                    // gone since it was listed: show the current list
                    None => self.review.list = None,
                },
            },
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

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The comments panel's display switch (margin cards / bubbles), right-aligned in the header
/// row: two small icon buttons, the current one highlighted.
fn display_toggle(ui: &mut Ui, look: &SideLook, current: CommentDisplay) -> Option<CommentDisplay> {
    let mut picked = None;
    let size = egui::vec2(22.0, 20.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (d, label) in [
            (CommentDisplay::Margin, "Show comments in the margin"),
            (CommentDisplay::Bubbles, "Show comments as bubbles"),
        ] {
            let (r, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let on = d == current;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, label));
            let p = ui.painter_at(r);
            if on {
                p.rect_filled(r, look.radius, look.selected_bg);
            } else if resp.hovered() {
                p.rect_filled(r, look.radius, look.hover_bg);
            }
            let c = if on { look.accent } else { look.icon };
            let stroke = egui::Stroke::new(1.3, c);
            let o = r.center();
            match d {
                CommentDisplay::Margin => {
                    // a page with lines and a card on its right
                    let page = egui::Rect::from_center_size(o - egui::vec2(3.0, 0.0), egui::vec2(9.0, 12.0));
                    p.rect_stroke(page, 1.0, stroke, egui::StrokeKind::Middle);
                    let card = egui::Rect::from_min_size(page.right_top() + egui::vec2(2.5, 2.0), egui::vec2(5.0, 4.0));
                    p.rect_filled(card, 1.0, c);
                }
                CommentDisplay::Bubbles => {
                    let b = egui::Rect::from_center_size(o - egui::vec2(0.0, 1.0), egui::vec2(12.0, 9.0));
                    p.rect_stroke(b, 2.0, stroke, egui::StrokeKind::Middle);
                    p.line_segment(
                        [
                            b.left_bottom() + egui::vec2(3.0, 0.0),
                            b.left_bottom() + egui::vec2(2.5, 3.5),
                        ],
                        stroke,
                    );
                    p.line_segment(
                        [
                            b.left_bottom() + egui::vec2(2.5, 3.5),
                            b.left_bottom() + egui::vec2(6.0, 0.0),
                        ],
                        stroke,
                    );
                }
            }
            if resp.on_hover_text(label).clicked() && !on {
                picked = Some(d);
            }
        }
    });
    picked
}

/// A clickable, one-line label (a click on a row's text jumps to the mark).
fn link_label(ui: &mut Ui, text: RichText, jump: &mut bool) {
    let r = ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()));
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    *jump |= r.clicked();
}

/// One panel row of fixed height `row_h`: badge and buttons, the text, the comment; returns
/// the chosen action, bound to the listed revision and the mark's identity.
fn row(
    ui: &mut Ui,
    look: &SideLook,
    rc: &markise_theme::ReviewColors,
    r: &PanelRow,
    counter: u64,
    row_h: f32,
) -> Option<PanelAction> {
    let target = || Target {
        counter,
        kind: r.kind,
        start: r.start,
        content_start: r.content_start,
        len: r.len,
        hash: r.hash,
    };
    let suggestion = r.kind.is_suggestion();
    let mut act = None;
    let mut jump = false;
    let (badge, badge_color) = match r.kind {
        CriticKind::Insert => ("Insert", color(rc.insert)),
        CriticKind::Delete => ("Delete", color(rc.delete)),
        CriticKind::Substitute => ("Replace", color(rc.insert)),
        _ => ("Comment", look.accent),
    };
    let w = ui.available_width();
    ui.allocate_ui(egui::vec2(w, row_h), |ui| {
        ui.set_height(row_h - 4.0);
        egui::Frame::new()
            .fill(look.hover_bg.gamma_multiply(0.5))
            .corner_radius(look.radius)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(row_h - 16.0);
                ui.horizontal(|ui| {
                    link_label(
                        ui,
                        RichText::new(badge).font(look.small_font.clone()).color(badge_color),
                        &mut jump,
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if suggestion {
                            if ui.small_button("Reject").clicked() {
                                act = Some(PanelAction::Run("review.reject", Some(target())));
                            }
                            if ui.small_button("Accept").clicked() {
                                act = Some(PanelAction::Run("review.accept", Some(target())));
                            }
                        } else if ui.small_button("Delete").clicked() {
                            act = Some(PanelAction::Run("review.deleteComment", Some(target())));
                        }
                    });
                });
                match r.kind {
                    CriticKind::Insert => {
                        link_label(
                            ui,
                            RichText::new(format!("+ {}", r.text)).color(color(rc.insert)),
                            &mut jump,
                        );
                    }
                    CriticKind::Delete => {
                        link_label(
                            ui,
                            RichText::new(r.text.as_str()).strikethrough().color(color(rc.delete)),
                            &mut jump,
                        );
                    }
                    CriticKind::Substitute => {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(r.old_text.as_str())
                                        .strikethrough()
                                        .color(color(rc.delete)),
                                )
                                .truncate(),
                            );
                            link_label(ui, RichText::new(r.text.as_str()).color(color(rc.insert)), &mut jump);
                        });
                    }
                    CriticKind::Highlight => {
                        link_label(
                            ui,
                            RichText::new(format!("“{}”", r.text)).background_color(color(rc.highlight_bg)),
                            &mut jump,
                        );
                    }
                    _ => {
                        if let Some(c) = &r.comment {
                            comment_text(ui, look, c, &mut jump);
                        }
                    }
                }
                if let Some(c) = &r.attached {
                    comment_text(ui, look, c, &mut jump);
                }
            });
    });
    if act.is_none() && jump {
        act = Some(PanelAction::Jump(target()));
    }
    act
}

/// `s` on one line, at most 120 characters.
fn short(s: &str) -> String {
    // reads no more of `s` than the preview needs
    let mut out = String::new();
    let mut n = 0;
    for w in s.split_whitespace() {
        if n > 0 {
            out.push(' ');
            n += 1;
        }
        for c in w.chars() {
            if n >= PREVIEW - 1 {
                out.push('…');
                return out;
            }
            out.push(c);
            n += 1;
        }
    }
    out
}

fn comment_text(ui: &mut Ui, look: &SideLook, c: &CommentPreview, jump: &mut bool) {
    let (author, text) = c;
    ui.horizontal(|ui| {
        if let Some(a) = author {
            link_label(ui, RichText::new(format!("@{a}")).strong().color(look.text), jump);
        }
        let t = if text.is_empty() {
            "(empty comment)"
        } else {
            text.as_str()
        };
        link_label(ui, RichText::new(t).color(look.text), jump);
    });
}
