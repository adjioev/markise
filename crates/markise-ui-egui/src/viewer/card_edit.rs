//! Editing a comment's text in its card (issue #87, Google Docs style): a click on a comment's
//! bubble or card — and Add Comment in the rendered view — opens a multi-line text field in
//! the margin card (or the pinned bubble popover) instead of revealing the raw `{>>…<<}`.
//!
//! - **Keys** (only while the field has the keyboard focus, taken out of the frame's input
//!   before the field sees them): Esc cancels, ⌘Enter / Ctrl+Enter saves; Enter is a line
//!   break in a paragraph's comment and saves in a heading's or table cell's (their comments
//!   are one line). Clicking anywhere else (the field loses the focus) saves.
//! - **One undo step per edit**: the new text is written by the `review.editComment` planner
//!   as one transaction ("Edit Comment"); an unchanged text writes nothing. A comment that
//!   Add Comment has just created is folded into Add Comment's own undo step when nothing
//!   happened in between (the empty insertion is undone and redone with the text), so one
//!   undo removes the comment with its text.
//! - **Empty text** removes the comment (and its highlight): saving an emptied comment, and
//!   cancelling or saving a new comment that was never written.
//! - The author prefix `@name:` is shown above the field and written back unchanged.
//! - The comment is followed by source position through edits made elsewhere (Side by Side,
//!   menus); if it disappears, or its card is no longer shown (scrolled away, its page
//!   evicted), the edit is saved — never lost silently — or, for a comment gone from the
//!   document, dropped.

use egui::{Event, Key, Pos2, Rect, Shape, Ui, pos2, vec2};
use markise_doc::{CriticKind, ParamValue, Params, Selection, planners, review};
use markise_engine::editor::{StateId, TxKind};

use super::{CARD_ICON, CARD_PAD, CardLook, DocViewer};

/// A comment being edited in its card.
#[derive(Clone, Debug)]
pub(super) struct CardEdit {
    /// Source start of the comment (`{>>`), kept current through edits.
    pub key: usize,
    /// The author prefix (`@name:`), shown above the field and written back.
    pub author: Option<String>,
    /// The text being edited (without the author prefix) and the text at the start.
    pub text: String,
    pub original: String,
    /// Created by Add Comment just now: the session state right after it (folded into its
    /// undo step while the document is still in that state).
    pub fresh: Option<StateId>,
    /// Line breaks are kept (a paragraph's comment); otherwise Enter saves.
    pub multiline: bool,
    /// The field takes the keyboard focus when it is next drawn.
    pub want_focus: bool,
    /// The field held the keyboard focus at some frame (a later loss of it saves).
    pub had_focus: bool,
    /// The card was drawn at some frame; frames it was not drawn since.
    pub seen: bool,
    pub unseen: u32,
    /// Why the last save was refused (shown in the card).
    pub error: Option<String>,
    /// The editor's document change count when `key` was last brought up to date.
    pub changes: u64,
}

/// How an edit ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Finish {
    Save,
    Cancel,
}

/// Frames a new comment's card may stay undrawn (its item is laid out the next frame) before
/// the edit gives up.
const UNSEEN_LIMIT: u32 = 30;

/// The field's id: one for every comment (only one is edited at a time), so the field keeps
/// its state while its comment moves through edits elsewhere.
fn field_id() -> egui::Id {
    egui::Id::unique("markise-comment-card-edit")
}

impl DocViewer {
    /// The comment being edited in its card: its source start and the field's text (tests).
    pub fn card_edit(&self) -> Option<(usize, &str)> {
        self.card_edit.as_ref().map(|e| (e.key, e.text.as_str()))
    }

    /// Opens the card editor for the comment whose mark starts at `key`, with `text` (as the
    /// card shows it: lines separated by `\n`; `None`: the text as parsed); the caret goes
    /// after the comment so its source is not revealed. `fresh`: created by Add Comment just
    /// now.
    pub(super) fn start_card_edit(&mut self, key: usize, text: Option<&str>, fresh: bool) {
        if let Some(e) = &self.card_edit
            && e.key == key
        {
            return;
        }
        // another comment's edit is saved first
        if self.card_edit.is_some() {
            self.finish_card_edit(Finish::Save, false);
        }
        let doc = self.ed.session.doc();
        let Some(m) = review::mark_at(doc, key + 1, |k| k == CriticKind::Comment).filter(|m| m.range.start == key)
        else {
            return;
        };
        let multiline = doc
            .leaf(m.content.start)
            .is_some_and(|l| matches!(l.kind, markise_doc::LeafKind::Paragraph));
        let (author, body) = review::split_author(text.unwrap_or(&m.text));
        let end = m.range.end;
        self.card_edit = Some(CardEdit {
            key,
            author: author.map(str::to_string),
            text: body.to_string(),
            original: body.to_string(),
            fresh: fresh.then(|| self.ed.session.state_id()),
            multiline,
            want_focus: true,
            had_focus: false,
            seen: false,
            unseen: 0,
            error: None,
            changes: self.ed.changes(),
        });
        self.ed.set_selection(Selection::caret(end));
        self.ed.refresh_focus();
    }

    /// After Add Comment ran in the rendered view: opens the new comment (at the caret) for
    /// editing in its card.
    pub(super) fn after_add_comment(&mut self) {
        let at = self.ed.session.selection().primary().focus.offset;
        if let Some(m) = review::mark_at(self.ed.session.doc(), at, |k| k == CriticKind::Comment) {
            self.start_card_edit(m.range.start, Some(""), true);
        }
    }

    /// Follows the edited comment through the edits since the last frame (called before the
    /// recent edits are cleared); drops the edit when the comment is gone.
    pub(super) fn track_card_edit(&mut self) {
        let Some(e) = self.card_edit.as_mut() else { return };
        if e.changes == self.ed.changes() {
            return; // nothing changed since the key was taken
        }
        // (the recent edits may include ones from before the edit started in the same frame:
        // the check below drops an edit whose key no longer names its comment)
        let Some(key) = self.ed.map_recent(e.key) else {
            self.card_edit = None;
            return;
        };
        e.key = key;
        e.changes = self.ed.changes();
        let doc = self.ed.session.doc();
        let ok = key < doc.text().len()
            && review::mark_at(doc, key + 1, |k| k == CriticKind::Comment).is_some_and(|m| m.range.start == key);
        if !ok {
            self.card_edit = None;
        }
    }

    /// Takes the card editor's keys out of this frame's input (only while its field has the
    /// focus): Esc cancels, ⌘Enter saves, and Enter saves a one-line comment.
    pub(super) fn card_edit_keys(&mut self, ctx: &egui::Context) -> Option<Finish> {
        let e = self.card_edit.as_ref()?;
        let id = field_id();
        if !ctx.memory(|m| m.has_focus(id)) {
            return None;
        }
        let multiline = e.multiline;
        let mut finish = None;
        ctx.input_mut(|i| {
            i.events.retain(|ev| match ev {
                Event::Key {
                    key: Key::Escape,
                    pressed,
                    modifiers,
                    ..
                } if modifiers.is_none() => {
                    if *pressed && finish.is_none() {
                        finish = Some(Finish::Cancel);
                    }
                    false
                }
                Event::Key {
                    key: Key::Enter,
                    pressed,
                    modifiers,
                    ..
                } if modifiers.command_only() || (!multiline && modifiers.is_none()) => {
                    if *pressed && finish.is_none() {
                        finish = Some(Finish::Save);
                    }
                    false
                }
                _ => true,
            })
        });
        finish
    }

    /// Draws the card editor at `min` (top left, screen) `w` wide; returns the card's rectangle
    /// and how the edit ended this frame (the field lost the focus: saved).
    pub(super) fn card_editor(
        &mut self,
        ui: &mut Ui,
        look: &CardLook,
        min: Pos2,
        w: f32,
        clip: Rect,
    ) -> (Rect, Option<Finish>) {
        let ctx = ui.ctx().clone();
        let Some(e) = self.card_edit.as_mut() else {
            return (Rect::from_min_size(min, vec2(w, 0.0)), None);
        };
        e.seen = true;
        e.unseen = 0;
        let id = field_id();
        let bg = ui.painter().add(Shape::Noop);
        let inner_w = (w - 2.0 * CARD_PAD - CARD_ICON).max(40.0);
        let inner = Rect::from_min_size(min + vec2(CARD_PAD + CARD_ICON, CARD_PAD), vec2(inner_w, 4000.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(clip);
        child.spacing_mut().item_spacing.y = 2.0;
        let font = egui::FontId::proportional(look.size);
        if let Some(a) = &e.author {
            child.label(
                egui::RichText::new(format!("@{a}"))
                    .font(font.clone())
                    .strong()
                    .color(look.text),
            );
        }
        let hint = if e.multiline {
            "Comment (⌘Enter to save, Esc to cancel)"
        } else {
            "Comment (Enter to save, Esc to cancel)"
        };
        let hint = if cfg!(target_os = "macos") {
            hint.to_string()
        } else {
            hint.replace('⌘', "Ctrl+")
        };
        let te = egui::TextEdit::multiline(&mut e.text)
            .id(id)
            .font(font.clone())
            .text_color(look.text)
            .hint_text(egui::RichText::new(hint).color(look.text.gamma_multiply(0.5)))
            .frame(egui::Frame::NONE)
            .desired_rows(1)
            .desired_width(inner_w)
            // Esc is the editor's (cancel), not egui's "give up the focus"
            .event_filter(egui::EventFilter {
                tab: false,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            });
        let resp = child.add(te);
        if let Some(err) = &e.error {
            child.label(
                egui::RichText::new(err)
                    .font(egui::FontId::proportional(look.size * 0.9))
                    .color(egui::Color32::from_rgb(200, 60, 50)),
            );
        }
        let used = child.min_rect();
        let card = Rect::from_min_max(
            min,
            pos2(
                min.x + w,
                (used.bottom() + CARD_PAD).max(min.y + CARD_ICON + 2.0 * CARD_PAD),
            ),
        );
        ui.painter().set(bg, look.background(card, true));
        look.paint_icon(ui.painter(), card);
        let mut finish = None;
        // the focus is taken once the click that opened the card is over (a release outside
        // the field would make it give the focus up again)
        let clicking = ui.input(|i| i.pointer.any_down() || i.pointer.any_released());
        if e.want_focus && !clicking {
            e.want_focus = false;
            resp.request_focus();
            // the text cursor at the end of the text
            let mut st = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
            let n = e.text.chars().count();
            st.cursor
                .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(n))));
            st.store(&ctx, id);
        } else if resp.has_focus() {
            e.had_focus = true;
        } else if e.had_focus || resp.lost_focus() {
            // focus went elsewhere (a click outside, another widget): saved
            finish = Some(Finish::Save);
        }
        if resp.has_focus() {
            e.had_focus = true;
        }
        if resp.changed() {
            e.error = None;
        }
        (card, finish)
    }

    /// The edited comment's card was not drawn this frame: after it had been shown (scrolled
    /// away, its page evicted), or when a new one never appears, the edit is saved.
    pub(super) fn card_edit_unseen(&mut self) {
        let Some(e) = self.card_edit.as_mut() else { return };
        e.unseen += 1;
        if e.seen || e.unseen > UNSEEN_LIMIT {
            self.finish_card_edit(Finish::Save, false);
        }
    }

    /// Ends the edit: writes the text (one undo step) or cancels. `refocus`: the document
    /// surface takes the keyboard focus back (keys; not after a click elsewhere, which
    /// placed its own caret and focus).
    pub(super) fn finish_card_edit(&mut self, how: Finish, refocus: bool) {
        let Some(e) = self.card_edit.take() else { return };
        let ok = match how {
            Finish::Cancel => {
                if e.fresh.is_some() {
                    // a new comment that was never written goes again
                    self.write_comment(&e, "")
                } else {
                    Ok(())
                }
            }
            Finish::Save => {
                let body = e.text.trim();
                if e.fresh.is_none() && body == e.original.trim() {
                    Ok(())
                } else if body.is_empty() {
                    self.write_comment(&e, "")
                } else {
                    let full = match &e.author {
                        Some(a) => format!("@{a}: {body}"),
                        None => body.to_string(),
                    };
                    self.write_comment(&e, &full)
                }
            }
        };
        if let Err(msg) = ok {
            // refused (the text cannot be written as a comment here): stays open
            let mut e = e;
            e.error = Some(msg);
            e.want_focus = true;
            e.had_focus = false;
            self.card_edit = Some(e);
            return;
        }
        // a bubble's popover closes with its editor
        self.popover = None;
        if refocus {
            self.refocus = true;
        }
    }

    /// Writes `text` as the comment's text (empty: removes the comment) as one transaction;
    /// a fresh comment's text joins Add Comment's undo step when the document is still in the
    /// state Add Comment left.
    fn write_comment(&mut self, e: &CardEdit, text: &str) -> Result<(), String> {
        let key = e.key;
        let fold = e
            .fresh
            .is_some_and(|s| s == self.ed.session.state_id() && self.ed.session.can_undo());
        let r = self.ed.edit(|s| {
            if fold && s.undo() {
                if text.is_empty() {
                    s.clear_redo();
                    return Ok(None);
                }
                // Add Comment again, at the selection it ran on, with the text
                let r = s.transact("Add Comment", TxKind::Format, |tx| {
                    let p = tx.plan(planners::REVIEW_ADD_COMMENT, Params::new())?;
                    tx.apply_plan(p)?;
                    let at = tx.selection().primary().focus.offset;
                    let params = Params::new()
                        .with("at", ParamValue::Int(at as i64))
                        .with("text", ParamValue::Text(text.into()));
                    let p = tx.plan(planners::REVIEW_EDIT_COMMENT, params)?;
                    tx.apply_plan(p)?;
                    Ok(at)
                });
                return match r {
                    Ok(at) => Ok(Some(at)),
                    Err(err) => {
                        s.redo();
                        Err(err.to_string())
                    }
                };
            }
            let at = key + 3;
            let params = Params::new()
                .with("at", ParamValue::Int(at as i64))
                .with("text", ParamValue::Text(text.into()));
            let label = if text.is_empty() {
                "Delete Comment"
            } else {
                "Edit Comment"
            };
            s.transact(label, TxKind::Format, |tx| {
                let p = tx.plan(planners::REVIEW_EDIT_COMMENT, params)?;
                tx.apply_plan(p)
            })
            .map(|_| None)
            .map_err(|err| err.to_string())
        });
        match r {
            Ok(Some(at)) => {
                // folded: the caret after the comment, as for an edited one
                let end = review::mark_at(self.ed.session.doc(), at, |k| k == CriticKind::Comment)
                    .map_or(at, |m| m.range.end);
                self.ed.set_selection(Selection::caret(end));
                self.ed.refresh_focus();
                Ok(())
            }
            Ok(None) => {
                self.ed.refresh_focus();
                Ok(())
            }
            Err(msg) => Err(format!("Not saved: {msg}")),
        }
    }

    /// Whether `p` (screen) is on the edited comment's card at the last frame.
    pub(super) fn on_edited_card(&self, p: Pos2) -> bool {
        let Some(e) = &self.card_edit else { return false };
        self.comments
            .iter()
            .any(|c| c.source.start == e.key && c.card.is_some_and(|r| r.contains(p)))
    }

    /// The comment being edited has the given source start.
    pub(super) fn editing(&self, key: usize) -> bool {
        self.card_edit.as_ref().is_some_and(|e| e.key == key)
    }

    /// Saves an open card edit (before the document is saved, closed or replaced).
    pub fn commit_card_edit(&mut self) {
        if self.card_edit.is_some() {
            self.finish_card_edit(Finish::Save, false);
            if let Some(e) = &self.card_edit
                && e.error.is_some()
            {
                // refused: keep the document as it is
                self.card_edit = None;
            }
        }
    }
}
