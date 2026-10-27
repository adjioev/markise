//! Source-code mode (spec 02 §12.1, M1 subset): the raw Markdown in a plain monospace editor.
//! Edits are applied to the `markise-md` document as one minimal text patch (common prefix /
//! suffix), so untouched text — line endings, markers, spacing — round-trips byte-identically.

use egui::{FontId, TextEdit, Ui};
use markise_md::{Document, LineEnding};

/// Documents above this size open read-only in source mode (a full-text `TextEdit` lays out
/// the whole buffer; the virtualised source editor arrives with M2).
pub const EDIT_LIMIT: usize = 2 * 1024 * 1024;

/// A `TextEdit` layouter (syntax colouring of the source buffer).
pub type Layouter<'a> = &'a mut dyn FnMut(&Ui, &dyn egui::TextBuffer, f32) -> std::sync::Arc<egui::Galley>;

pub struct SourceState {
    pub buffer: String,
    /// Stable widget id of this buffer's editor: its `TextEdit` state (caret, undo history) is
    /// the same in Source and Side by Side mode, and a new buffer (another document, a reload)
    /// starts with a fresh history (99 C58: one undo stack per buffer).
    pub id: egui::Id,
    /// The buffer shows `\n` for a uniformly CRLF document.
    crlf: bool,
    pub dirty: bool,
    pub readonly: bool,
    /// Line starts of a read-only buffer.
    line_starts: Vec<usize>,
    /// Requested scroll: source line to centre (applied on the next frame).
    scroll_to: Option<f32>,
    /// Requested scroll: absolute vertical offset in points (Side by Side sync).
    scroll_px: Option<f32>,
    /// Caret (character index into `buffer`) after the last frame, if the editor has one.
    pub caret: Option<usize>,
    /// Vertical distance between line tops (row height, plus item spacing for the
    /// read-only row list).
    pub pitch: f32,
    /// Top of each source line in the editor's content (soft-wrapped lines span several rows;
    /// empty for the read-only row list, where every line is `pitch` tall), plus the bottom.
    line_tops: Vec<f32>,
    /// Identity of the galley `line_tops` was computed from.
    galley_id: usize,
    /// Current vertical scroll offset.
    pub offset: f32,
    pub viewport_h: f32,
    pub row_h: f32,
}

impl SourceState {
    /// Enters source mode, centring source line `line` (spec 02 §12.1 position mapping).
    pub fn new(doc: &Document, line: usize) -> Self {
        let text = doc.text();
        let crlf = doc.line_ending() == LineEnding::CrLf && !has_lone_lf(text);
        let readonly = text.len() > EDIT_LIMIT;
        let buffer = if crlf {
            text.replace("\r\n", "\n")
        } else {
            text.to_string()
        };
        let line_starts = if readonly {
            std::iter::once(0)
                .chain(buffer.match_indices('\n').map(|(i, _)| i + 1))
                .collect()
        } else {
            Vec::new()
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SourceState {
            id: egui::Id::unique(("markise-source-editor", n)),
            buffer,
            crlf,
            dirty: false,
            readonly,
            line_starts,
            scroll_to: Some(line as f32),
            scroll_px: None,
            caret: None,
            pitch: 16.0,
            line_tops: Vec::new(),
            galley_id: 0,
            offset: 0.0,
            viewport_h: 0.0,
            row_h: 16.0,
        }
    }

    /// The buffer in document form (CRLF restored for a uniformly CRLF document). Apply it
    /// with `markise_engine::edit::apply_text`, which patches only the changed regions.
    /// Replaces the editor text with the document text `text` (a command changed the
    /// document while this buffer shows it, e.g. in Side by Side). The editor's own undo
    /// history is kept (egui records the new text as the next state); the buffer is not marked
    /// as edited.
    pub fn set_doc_text(&mut self, text: &str) {
        if self.crlf && has_lone_lf(text) {
            self.crlf = false;
        }
        self.buffer = if self.crlf {
            text.replace("\r\n", "\n")
        } else {
            text.to_string()
        };
        if self.readonly {
            self.line_starts = std::iter::once(0)
                .chain(self.buffer.match_indices('\n').map(|(i, _)| i + 1))
                .collect();
        }
    }

    pub fn doc_text(&self) -> String {
        to_doc_text(&self.buffer, self.crlf)
    }

    /// Source line at the vertical middle of the viewport.
    pub fn middle_line(&self) -> usize {
        self.line_at_y(self.offset + self.viewport_h * 0.5) as usize
    }

    /// Source line (fractional: `2.5` = halfway down line 2) at content position `y`.
    pub fn line_at_y(&self, y: f32) -> f32 {
        let t = &self.line_tops;
        if t.len() < 2 {
            return (y / self.pitch.max(1.0)).max(0.0);
        }
        let i = t.partition_point(|&top| top <= y).saturating_sub(1).min(t.len() - 2);
        let h = (t[i + 1] - t[i]).max(1.0);
        i as f32 + ((y - t[i]) / h).clamp(0.0, 1.0)
    }

    /// Content position of the (fractional) source line `line`.
    pub fn y_of_line(&self, line: f32) -> f32 {
        let t = &self.line_tops;
        let line = line.max(0.0);
        if t.len() < 2 {
            return line * self.pitch;
        }
        let i = (line.floor() as usize).min(t.len() - 2);
        t[i] + (line - i as f32).min(1.0) * (t[i + 1] - t[i])
    }

    /// The buffer shows `\n` for a uniformly CRLF document (buffer offsets + line index =
    /// document offsets).
    pub fn is_crlf(&self) -> bool {
        self.crlf
    }

    /// Scrolls (on the next frame) so that the viewport top is at `y` points.
    pub fn scroll_to_px(&mut self, y: f32) {
        self.scroll_px = Some(y.max(0.0));
        self.scroll_to = None;
    }

    /// A programmatic scroll is waiting for the next frame.
    pub fn scroll_pending(&self) -> bool {
        self.scroll_px.is_some() || self.scroll_to.is_some()
    }

    /// Byte offset (in document coordinates) of the start of buffer line `line`.
    pub fn line_to_doc_offset(&self, line: usize) -> usize {
        let mut pos = 0;
        let mut l = 0;
        for (i, b) in self.buffer.bytes().enumerate() {
            if l == line {
                break;
            }
            if b == b'\n' {
                l += 1;
                pos = i + 1;
            }
        }
        if self.crlf { pos + l } else { pos }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        self.show_with(ui, None);
    }

    /// Shows the editor; `layouter` (if any) lays out the editable buffer (syntax colours).
    pub fn show_with(&mut self, ui: &mut Ui, layouter: Option<Layouter<'_>>) {
        let font = FontId::monospace(FONT_SIZE);
        self.row_h = ui.fonts_mut(|f| f.row_height(&font));
        self.pitch = if self.readonly {
            self.row_h + ui.spacing().item_spacing.y
        } else {
            self.row_h
        };
        self.viewport_h = ui.available_height();
        let mut area = egui::ScrollArea::both()
            .id_salt("markise-source")
            .auto_shrink([false, false]);
        if let Some(line) = self.scroll_to.take() {
            area = area.vertical_scroll_offset((self.y_of_line(line) - self.viewport_h * 0.5).max(0.0));
        }
        if let Some(y) = self.scroll_px.take() {
            area = area.vertical_scroll_offset(y);
        }
        let out = if self.readonly {
            let n = self.line_starts.len();
            let buf = &self.buffer;
            let starts = &self.line_starts;
            area.show_rows(ui, self.row_h, n, |ui, range| {
                for i in range {
                    let s = starts[i];
                    let e = starts.get(i + 1).map_or(buf.len(), |&e| e.saturating_sub(1));
                    ui.label(egui::RichText::new(&buf[s..e.max(s)]).font(font.clone()));
                }
            })
        } else {
            area.show(ui, |ui| {
                let mut edit = TextEdit::multiline(&mut self.buffer)
                    .id(self.id)
                    .font(font.clone())
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE)
                    .lock_focus(true);
                if let Some(l) = layouter {
                    edit = edit.layouter(l);
                }
                let out = edit.show(ui);
                if out.response.changed() {
                    self.dirty = true;
                }
                self.caret = out.cursor_range.map(|r| r.primary.index.0);
                let id = std::sync::Arc::as_ptr(&out.galley) as usize;
                if id != self.galley_id {
                    self.galley_id = id;
                    let base = out.galley_pos.y - out.response.rect.top();
                    self.line_tops.clear();
                    let mut new_line = true;
                    for r in &out.galley.rows {
                        if new_line {
                            self.line_tops.push(base + r.pos.y);
                        }
                        new_line = r.ends_with_newline;
                    }
                    let bottom = out.galley.rows.last().map_or(0.0, |r| r.pos.y + r.height());
                    self.line_tops.push(base + bottom.max(self.row_h));
                }
            })
        };
        self.offset = out.state.clamped_offset().y;
    }
}

/// Editor text → document text (`crlf`: restore `\r\n` of a uniformly CRLF document).
pub fn to_doc_text(buffer: &str, crlf: bool) -> String {
    if crlf {
        buffer.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        buffer.to_string()
    }
}

/// Font size of the source editor.
pub const FONT_SIZE: f32 = 13.0;

fn has_lone_lf(s: &str) -> bool {
    let b = s.as_bytes();
    memchr_iter(b'\n', b).any(|i| i == 0 || b[i - 1] != b'\r')
}

fn memchr_iter(c: u8, b: &[u8]) -> impl Iterator<Item = usize> + '_ {
    b.iter().enumerate().filter(move |(_, x)| **x == c).map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use markise_engine::edit::apply_text;

    #[test]
    fn source_edits_keep_untouched_bytes() {
        let src = "# Title\r\n\r\n* item\r\n* other\r\n\r\nend\r\n";
        let mut d = markise_md::parse(src);
        let mut s = SourceState::new(&d, 0);
        assert!(!s.buffer.contains('\r'));
        s.buffer = s.buffer.replace("other", "öther two");
        assert!(apply_text(&mut d, &s.doc_text()));
        assert_eq!(d.serialize(), "# Title\r\n\r\n* item\r\n* öther two\r\n\r\nend\r\n");
        let mut s = SourceState::new(&d, 0);
        assert!(!apply_text(&mut d, &s.doc_text()));
        // typing a newline in a CRLF document produces CRLF (R2.2)
        s.buffer.push_str("more\n");
        apply_text(&mut d, &s.doc_text());
        assert!(d.serialize().ends_with("end\r\nmore\r\n"));
    }

    #[test]
    fn mixed_endings_are_shown_raw_and_preserved() {
        // review item 5 through the source-mode path
        let mut d = markise_md::parse("a\r\nmiddle\nz\r\n");
        let mut s = SourceState::new(&d, 0);
        assert_eq!(s.buffer, "a\r\nmiddle\nz\r\n", "mixed document: raw buffer");
        s.buffer = s.buffer.replacen('a', "A", 1).replacen('z', "Z", 1);
        assert!(apply_text(&mut d, &s.doc_text()));
        assert_eq!(d.serialize(), "A\r\nmiddle\nZ\r\n");
    }

    #[test]
    fn soft_wrapped_lines_map_to_their_rows() {
        let d = markise_md::parse("a\nlong\nb\n");
        let mut s = SourceState::new(&d, 0);
        s.pitch = 10.0;
        assert_eq!(s.line_at_y(25.0), 2.5, "no galley yet: pitch");
        // line 1 wraps onto three rows
        s.line_tops = vec![0.0, 10.0, 40.0, 50.0, 60.0];
        assert_eq!(s.line_at_y(25.0), 1.5);
        assert_eq!(s.y_of_line(1.5), 25.0);
        assert_eq!(s.y_of_line(2.0), 40.0);
        assert_eq!(s.line_at_y(1000.0), 4.0);
    }

    #[test]
    fn line_offsets_map_back_to_document() {
        let d = markise_md::parse("a\r\nbb\r\nccc\r\n");
        let s = SourceState::new(&d, 0);
        assert_eq!(s.line_to_doc_offset(0), 0);
        assert_eq!(s.line_to_doc_offset(1), 3);
        assert_eq!(s.line_to_doc_offset(2), 7);
    }
}
