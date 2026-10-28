//! epaint galleys behind the seam ([`EpaintShaper`] = egui's `FontsView`).
//!
//! [`to_layout_job`] reproduces exactly the `LayoutJob`s the M1 layout code built, so galleys,
//! egui's galley cache and painting are unchanged (ADR 0005 T2: no visible change).

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use epaint::text::cursor::CCursor;
use epaint::text::{FontFamily, FontId, FontsView, Galley, LayoutJob, TextFormat, TextWrapping};
use epaint::{Pos2, Rect, Stroke, Vec2, pos2};

use super::{Affinity, Cluster, InkSpan, LineInfo, LocalPos, StyledText, TextBlock, TextShaper, TextStyle};

/// The epaint backend: egui's font system shapes [`StyledText`] as galleys.
pub type EpaintShaper<'a> = FontsView<'a>;

/// The epaint font of a style.
pub fn font_id(style: &TextStyle) -> FontId {
    FontId::new(style.size, FontFamily::Name(style.role.family_name().into()))
}

/// epaint text format of a style (identical to M1's `Style::format`).
pub fn text_format(s: &TextStyle) -> TextFormat {
    TextFormat {
        font_id: font_id(s),
        extra_letter_spacing: s.letter_spacing,
        line_height: Some(s.line_height),
        color: s.color,
        background: s.background,
        expand_bg: s.expand_bg,
        italics: s.italic,
        underline: if s.underline {
            Stroke::new(1.0, s.color)
        } else {
            Stroke::NONE
        },
        strikethrough: if s.strike {
            Stroke::new(1.0, s.color)
        } else {
            Stroke::NONE
        },
        valign: s.valign,
        coords: Default::default(),
    }
}

/// The epaint job for `text` wrapped at `width` (`None` = no wrapping).
pub fn to_layout_job(text: &StyledText, width: Option<f32>) -> LayoutJob {
    let mut job = LayoutJob {
        text: text.text.clone(),
        sections: Vec::with_capacity(text.runs.len()),
        halign: text.halign,
        break_on_newline: text.break_on_newline,
        // centred / right-aligned rows count their trailing whitespace: the spaces typed at
        // the end of a centred heading (shown while the caret is in them, BE-1) must move the
        // caret — epaint otherwise aligns on the text without them and clamps the caret at its
        // end
        keep_trailing_whitespace: true,
        ..LayoutJob::default()
    };
    job.wrap = TextWrapping {
        max_width: width.unwrap_or(f32::INFINITY),
        ..TextWrapping::default()
    };
    for r in &text.runs {
        job.sections.push(epaint::text::LayoutSection {
            leading_space: r.lead,
            byte_range: epaint::text::ByteIndex(r.range.start)..epaint::text::ByteIndex(r.range.end),
            format: text_format(&r.style),
        });
    }
    job
}

impl TextShaper for FontsView<'_> {
    fn shape(&mut self, text: &StyledText, width: Option<f32>) -> Arc<dyn TextBlock> {
        Arc::new(EpaintBlock {
            galley: self.layout_job(to_layout_job(text, width)),
            map: text.source_map.clone(),
        })
    }

    fn has_glyphs(&mut self, style: &TextStyle, s: &str) -> bool {
        FontsView::has_glyphs(self, &font_id(style), s)
    }

    /// Each section's own font chain (body, heading, mono and sidebar chains differ).
    fn lacks_glyphs(&mut self, block: &dyn TextBlock) -> bool {
        let Some(galley) = block.galley() else {
            return false;
        };
        let mut buf = [0u8; 4];
        let text = &galley.job.text;
        for sec in &galley.job.sections {
            let Some(run) = text.get(sec.byte_range.start.0..sec.byte_range.end.0) else {
                continue;
            };
            for c in run.chars() {
                if !c.is_ascii()
                    && !c.is_whitespace()
                    && !FontsView::has_glyphs(self, &sec.format.font_id, c.encode_utf8(&mut buf))
                {
                    return true;
                }
            }
        }
        false
    }
}

/// A galley as a [`TextBlock`].
#[derive(Debug)]
pub struct EpaintBlock {
    galley: Arc<Galley>,
    map: Option<Arc<super::SourceMap>>,
}

impl EpaintBlock {
    pub fn new(galley: Arc<Galley>) -> Self {
        EpaintBlock { galley, map: None }
    }

    pub fn galley(&self) -> &Arc<Galley> {
        &self.galley
    }

    /// Byte offset of every character, plus `text.len()`.
    fn char_starts(&self) -> Vec<usize> {
        let text = &self.galley.job.text;
        text.char_indices()
            .map(|(b, _)| b)
            .chain(std::iter::once(text.len()))
            .collect()
    }

    fn byte_to_char(&self, byte: usize) -> usize {
        let text = &self.galley.job.text;
        let b = byte.min(text.len());
        text[..floor_boundary(text, b)].chars().count()
    }

    /// epaint cursor of a position: downstream affinity prefers the next row at a soft wrap.
    fn cursor(&self, p: LocalPos) -> CCursor {
        CCursor {
            index: epaint::text::CharIndex(self.byte_to_char(p.byte)),
            prefer_next_row: p.affinity == Affinity::Downstream,
        }
    }

    fn char_to_byte(&self, ci: usize) -> usize {
        let text = &self.galley.job.text;
        text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
    }
}

fn floor_boundary(s: &str, mut b: usize) -> usize {
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

impl TextBlock for EpaintBlock {
    fn text(&self) -> &str {
        &self.galley.job.text
    }

    fn size(&self) -> Vec2 {
        self.galley.size()
    }

    fn rect(&self) -> Rect {
        self.galley.rect
    }

    fn line_count(&self) -> usize {
        self.galley.rows.len()
    }

    fn lines(&self) -> Vec<LineInfo> {
        let starts = self.char_starts();
        let last = starts.len() - 1;
        let mut ci = 0usize;
        let mut out = Vec::with_capacity(self.galley.rows.len());
        for prow in &self.galley.rows {
            let n = prow.row.glyphs.len();
            let b0 = starts[ci.min(last)];
            let b1 = starts[(ci + n).min(last)];
            let baseline = prow
                .row
                .glyphs
                .iter()
                .map(|g| prow.pos.y + g.pos.y)
                .fold(f32::NEG_INFINITY, f32::max);
            out.push(LineInfo {
                range: b0..b1,
                origin: prow.pos,
                size: prow.row.size,
                baseline: if baseline.is_finite() {
                    baseline
                } else {
                    prow.pos.y + prow.row.size.y
                },
                ends_with_newline: prow.ends_with_newline,
            });
            ci += n + usize::from(prow.ends_with_newline);
        }
        out
    }

    fn first_line_height(&self) -> Option<f32> {
        self.galley.rows.first().map(|r| r.row.size.y)
    }

    fn half_leading(&self) -> f32 {
        self.galley
            .rows
            .first()
            .and_then(|r| r.row.glyphs.first())
            .map_or(0.0, |gl| 0.5 * (gl.line_height - gl.font_height))
    }

    fn clusters(&self) -> Vec<Cluster> {
        let starts = self.char_starts();
        let last = starts.len() - 1;
        let mut ci = 0usize;
        let mut out = Vec::new();
        for (li, prow) in self.galley.rows.iter().enumerate() {
            for g in &prow.row.glyphs {
                out.push(Cluster {
                    line: li,
                    byte: starts[ci.min(last)],
                    ch: g.chr,
                    x: prow.pos.x + g.pos.x,
                    advance: g.advance_width,
                });
                ci += 1;
            }
            ci += usize::from(prow.ends_with_newline);
        }
        out
    }

    fn ink_spans(&self, range: &Range<usize>) -> Vec<InkSpan> {
        let text = &self.galley.job.text;
        let starts: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        let mut out = Vec::new();
        let mut ci = 0usize;
        for prow in &self.galley.rows {
            let row = &prow.row;
            let mut cur: Option<InkSpan> = None;
            for g in &row.glyphs {
                let hit = starts.get(ci).is_some_and(|b| range.contains(b)) && !g.chr.is_control();
                ci += 1;
                if hit {
                    let x0 = prow.pos.x + g.pos.x;
                    let x1 = x0 + g.advance_width;
                    let baseline = prow.pos.y + g.pos.y;
                    let ascent = g.font_ascent;
                    let descent = (g.font_height - g.font_ascent).max(0.0) * 0.8;
                    let stroke = (g.font_height * 0.06).clamp(1.0, 2.5);
                    cur = Some(match cur {
                        Some(c) => InkSpan {
                            x1,
                            baseline: c.baseline.max(baseline),
                            ascent: c.ascent.max(ascent),
                            descent: c.descent.max(descent),
                            stroke: c.stroke.max(stroke),
                            ..c
                        },
                        None => InkSpan {
                            x0,
                            x1,
                            baseline,
                            ascent,
                            descent,
                            stroke,
                        },
                    });
                } else if let Some(c) = cur.take() {
                    out.push(c);
                }
            }
            if let Some(c) = cur.take() {
                out.push(c);
            }
            if prow.ends_with_newline {
                ci += 1;
            }
        }
        out
    }

    fn hit(&self, pos: Pos2) -> LocalPos {
        let cc = self.galley.cursor_from_pos(pos.to_vec2());
        // at a soft wrap epaint says which row the cursor belongs to: the end of the upper row
        // (upstream) or the start of the lower one (downstream)
        let affinity = if cc.prefer_next_row {
            Affinity::Downstream
        } else {
            Affinity::Upstream
        };
        LocalPos::new(self.char_to_byte(cc.index.0), affinity)
    }

    fn char_at(&self, pos: Pos2) -> Option<usize> {
        let galley = &self.galley;
        if !galley.rect.expand(2.0).contains(pos) {
            return None;
        }
        let cc = galley.cursor_from_pos(pos.to_vec2());
        // the cursor is *between* characters: test the character under the pointer
        let cursor_rect = galley.pos_from_cursor(cc);
        let ci = if cursor_rect.min.x > pos.x && cc.index.0 > 0 {
            cc.index.0 - 1
        } else {
            cc.index.0
        };
        galley.job.text.char_indices().nth(ci).map(|(b, _)| b)
    }

    fn caret_rect(&self, p: LocalPos) -> Rect {
        let r = self.galley.pos_from_cursor(self.cursor(p));
        Rect::from_min_max(r.min, pos2(r.min.x, r.max.y))
    }

    fn selection_rects(&self, a: LocalPos, b: LocalPos) -> Vec<Rect> {
        let (s, e) = if a.byte <= b.byte { (a, b) } else { (b, a) };
        if s.byte == e.byte {
            return Vec::new();
        }
        let ca = self.galley.pos_from_cursor(self.cursor(s));
        let cb = self.galley.pos_from_cursor(self.cursor(e));
        let mut out = Vec::new();
        for prow in &self.galley.rows {
            let r = prow.rect();
            if r.max.y <= ca.min.y + 0.5 || r.min.y >= cb.max.y - 0.5 {
                continue;
            }
            let x0 = if (r.min.y - ca.min.y).abs() < 0.5 {
                ca.min.x
            } else {
                r.min.x
            };
            let x1 = if (r.min.y - cb.min.y).abs() < 0.5 {
                cb.min.x
            } else {
                r.max.x
            };
            if x1 > x0 {
                out.push(Rect::from_min_max(pos2(x0, r.min.y), pos2(x1, r.max.y)));
            }
        }
        out
    }

    fn cost(&self) -> usize {
        let g = &self.galley;
        g.num_vertices * 20
            + g.num_indices * 4
            + g.job.text.len() * 2
            + g.rows.iter().map(|r| r.row.glyphs.len()).sum::<usize>() * 48
            + self
                .map
                .as_ref()
                .map_or(0, |m| m.pieces.len() * std::mem::size_of::<super::MapPiece>())
    }

    fn source_map(&self) -> Option<&super::SourceMap> {
        self.map.as_deref()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{FontRole, LocalPos, StyledText, TextStyle};
    use epaint::Color32;
    use epaint::emath::Align;
    use markise_text::Affinity;

    #[test]
    fn trailing_spaces_of_a_centred_row_move_the_caret() {
        // a centred heading with spaces typed after its text (Classic H1): each space moves the
        // caret right (epaint used to align without them and clamp the caret at the text end)
        let mut fonts = crate::testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut last = None;
        for n in 0..4 {
            let s = format!("Title{}", " ".repeat(n));
            let mut t = StyledText::single(s.clone(), TextStyle::new(32.0, 40.0, FontRole::Heading, Color32::BLACK));
            t.halign = Align::Center;
            let b = f.shape(&t, Some(600.0));
            let x = b.caret_rect(LocalPos::new(s.len(), Affinity::Upstream)).min.x;
            if let Some(prev) = last {
                assert!(x > prev, "space {n}: {prev} -> {x}");
            }
            last = Some(x);
        }
    }
}
