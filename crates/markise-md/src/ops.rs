//! Structural edit operations. Each is a pure **planner** (`plan_*`: document in, [`PatchSet`]
//! out, ADR 0006 §2.1) computing a minimal patch from the block's recorded pattern (R1.3), plus
//! a convenience wrapper that applies the plan with [`Document::apply_patches`] (newly typed line
//! breaks converted to the document's line ending), so untouched text stays byte-identical.

use crate::block::{Block, BlockKind};
use crate::options::WriteOptions;
use crate::table::{self, PrettyStyle, TableModel};
use crate::text::Span;
use crate::write;
use crate::{Document, EditResult, line_start};
use markise_doc::{Patch, PatchSet};

/// Errors from structural operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    NotFound,
    WrongKind,
    OutOfRange,
}

fn prefix_of(text: &str, pos: usize) -> &str {
    &text[line_start(text, pos)..pos]
}

/// Converts `\n` in generated Markdown to the document line ending and prefixes continuation
/// lines with `cont_prefix` (container prefixes such as `> `).
fn localize(doc: &Document, md: &str, cont_prefix: &str) -> String {
    let eol = doc.line_ending().as_str();
    let mut out = String::with_capacity(md.len() + 8);
    for (i, l) in md.split('\n').enumerate() {
        if i > 0 {
            out.push_str(eol);
            if l.is_empty() {
                out.push_str(cont_prefix.trim_end());
            } else {
                out.push_str(cont_prefix);
            }
        }
        out.push_str(l);
    }
    out
}

/// Toggles the task checkbox of the deepest task item containing `pos` (R3.4.2):
/// checked → `[x]`, unchecked → `[ ]`, rewriting only the box.
pub fn plan_toggle_task(doc: &Document, pos: usize) -> Result<PatchSet, OpError> {
    let path = doc.block_path_at(pos);
    let mut found = None;
    for depth in (1..=path.len()).rev() {
        if let Some((b, origin)) = doc.block_at_path_rel(&path[..depth])
            && let BlockKind::ListItem {
                task: Some((checked, span)),
                ..
            } = &b.kind
        {
            found = Some((*checked, Span::new(span.start + origin, span.end + origin)));
            break;
        }
    }
    let (checked, span) = found.ok_or(OpError::NotFound)?;
    let new = if checked { "[ ]" } else { "[x]" };
    Ok(PatchSet::single(span.start..span.end, new))
}

fn list_info(b: &Block) -> Option<(bool, u8, u64, bool)> {
    match b.kind {
        BlockKind::List {
            ordered,
            marker,
            start,
            tight,
        } => Some((ordered, marker, start, tight)),
        _ => None,
    }
}

/// Inserts a new item with `content` at `index` of the list at `list_path` (R3.4.3, R3.4.4,
/// decision C11 for renumbering).
pub fn plan_insert_list_item(
    doc: &Document,
    list_path: &[usize],
    index: usize,
    content: &str,
    task: Option<bool>,
) -> Result<PatchSet, OpError> {
    let text = doc.text().to_string();
    let list = doc.block_at_path(list_path).ok_or(OpError::NotFound)?;
    let (ordered, delim, start, tight) = list_info(&list).ok_or(OpError::WrongKind)?;
    let items = &list.children;
    if index > items.len() || items.is_empty() {
        return Err(OpError::OutOfRange);
    }
    let reference = if index == 0 { &items[0] } else { &items[index - 1] };
    let BlockKind::ListItem {
        marker, content_offset, ..
    } = reference.kind
    else {
        return Err(OpError::WrongKind);
    };
    let marker_prefix = prefix_of(&text, marker.start).to_string();
    // gap between marker and content as written
    let gap_len = content_offset.saturating_sub(marker.len()).clamp(1, 4);
    let gap = " ".repeat(gap_len);
    let numbers: Vec<u64> = items
        .iter()
        .map(|it| match it.kind {
            BlockKind::ListItem { number, .. } => number.unwrap_or(0),
            _ => 0,
        })
        .collect();
    let fixed = ordered && numbers.len() > 1 && numbers.iter().all(|&n| n == numbers[0]);
    let sequential = ordered && numbers.iter().enumerate().all(|(i, &n)| n == start + i as u64);
    let new_marker = if !ordered {
        (delim as char).to_string()
    } else {
        let n = if fixed {
            numbers[0]
        } else if sequential {
            start + index as u64
        } else if index == 0 {
            numbers[0]
        } else {
            numbers[index - 1] + 1
        };
        format!("{n}{}", delim as char)
    };
    let task_txt = match task {
        Some(true) => "[x] ",
        Some(false) => "[ ] ",
        None => "",
    };
    let cont_prefix = format!("{}{}", marker_prefix, " ".repeat(new_marker.len() + gap_len));
    let body = localize(doc, &format!("{task_txt}{content}"), &cont_prefix);
    let eol = doc.line_ending().as_str();
    let blank_line = if tight {
        String::new()
    } else {
        format!("{}{eol}", marker_prefix.trim_end())
    };

    // Renumber following items (only for sequential, non-fixed lists).
    let mut renumber: Vec<(Span, String)> = Vec::new();
    if ordered && sequential && !fixed {
        for (i, it) in items.iter().enumerate().skip(index) {
            if let BlockKind::ListItem { marker, .. } = it.kind {
                let n = start + i as u64 + 1;
                renumber.push((marker, format!("{n}{}", delim as char)));
            }
        }
    }
    let (at, insert) = if index == items.len() {
        let end = items[index - 1].span.end;
        (end, format!("{eol}{blank_line}{marker_prefix}{new_marker}{gap}{body}"))
    } else {
        let ls = line_start(&text, items[index].span.start);
        (ls, format!("{marker_prefix}{new_marker}{gap}{body}{eol}{blank_line}"))
    };
    // One patch set: the new item plus the renumbered markers (one re-parse, ADR 0006 §2.1).
    let mut patches: Vec<Patch> = renumber
        .into_iter()
        .map(|(span, num)| Patch::new(span.range(), num))
        .collect();
    patches.push(Patch::new(at..at, insert));
    PatchSet::new(patches).map_err(|_| OpError::OutOfRange)
}

/// Number of blank lines to write between two adjacent top-level blocks (R6.2, R3.4.7).
pub fn separator_blank_lines(prev: &BlockKind, next: &BlockKind) -> usize {
    match (prev, next) {
        (BlockKind::LinkRefDef { .. }, BlockKind::LinkRefDef { .. }) => 0,
        (BlockKind::FootnoteDef { .. }, BlockKind::FootnoteDef { .. }) => 0,
        (
            BlockKind::List {
                ordered: o1,
                marker: m1,
                ..
            },
            BlockKind::List {
                ordered: o2,
                marker: m2,
                ..
            },
        ) if o1 == o2 && m1 == m2 => 2,
        _ => 1,
    }
}

/// Inserts a freshly generated top-level block (`md` uses `\n`) after top-level block
/// `after` (`None` = at the very start). Blank-line separation follows R6.2/R3.4.7; the final
/// newline state of the file is preserved (R2.3), or set from `prefer_final_newline` when the
/// document was empty.
pub fn plan_insert_block_after(
    doc: &Document,
    after: Option<usize>,
    md: &str,
    wopts: &WriteOptions,
) -> Result<PatchSet, OpError> {
    let probe = Document::parse(md, doc.options().clone());
    let new_first = probe
        .blocks()
        .first()
        .map(|b| b.kind.clone())
        .ok_or(OpError::WrongKind)?;
    let new_last = probe
        .blocks()
        .last()
        .map(|b| b.kind.clone())
        .unwrap_or(new_first.clone());
    let body = localize(doc, md, "");
    let eol = doc.line_ending().as_str().to_string();
    let text = doc.text();
    let bom = crate::bom_len(text);
    if text.len() == bom {
        let mut s = body;
        if wopts.prefer_final_newline {
            s.push_str(&eol);
        }
        return Ok(PatchSet::single(bom..bom, s));
    }
    // Blank lines already present between the insertion point and the following block.
    let existing_blanks = |from: usize, next: &crate::Block| {
        crate::text::count_terminators(&text[from..next.span.start]).saturating_sub(1)
    };
    let (at, s) = match after {
        None => match doc.top_block(0) {
            Some(next) => {
                let need = separator_blank_lines(&new_last, &next.kind);
                let have = crate::text::count_terminators(&text[bom..next.span.start]);
                let add = (need + 1).saturating_sub(have);
                (bom, format!("{body}{}", eol.repeat(add)))
            }
            None => (bom, format!("{body}{eol}")),
        },
        Some(i) => {
            let prev = doc.top_block(i).ok_or(OpError::OutOfRange)?;
            let before = separator_blank_lines(&prev.kind, &new_first);
            let at = prev.span.end;
            // The original gap (terminator + blank lines) now follows the new block; add blank
            // lines when the following block needs more separation (review P2 #8).
            let after_blanks = match doc.top_block(i + 1) {
                Some(next) => separator_blank_lines(&new_last, &next.kind).saturating_sub(existing_blanks(at, &next)),
                None => 0,
            };
            (
                at,
                format!("{}{body}{}", eol.repeat(before + 1), eol.repeat(after_blanks)),
            )
        }
    };
    Ok(PatchSet::single(at..at, s))
}

/// Replaces the content of the code block at `path`, keeping its fence pattern and
/// lengthening the fence when the content requires it (R3.5.3, T13).
pub fn plan_set_code_content(doc: &Document, path: &[usize], content: &str) -> Result<PatchSet, OpError> {
    let text = doc.text().to_string();
    let b = doc.block_at_path(path).ok_or(OpError::NotFound)?;
    let BlockKind::FencedCode {
        fence_char,
        fence_len,
        opener,
        closer,
        ..
    } = &b.kind
    else {
        return Err(OpError::WrongKind);
    };
    let (ch, len) = write::fence_for_pattern(content, *fence_char as char, *fence_len);
    let prefix = prefix_of(&text, opener.start).to_string();
    let open_line = opener.slice(&text);
    let new_open = format!("{}{}", (ch.to_string()).repeat(len), &open_line[*fence_len..]);
    let mut md = new_open;
    if !content.is_empty() {
        md.push('\n');
        md.push_str(content);
    }
    let end = match closer {
        Some(c) => {
            let cl = c.slice(&text);
            let new_close = if len != *fence_len {
                ch.to_string().repeat(len)
            } else {
                cl.to_string()
            };
            md.push('\n');
            md.push_str(&new_close);
            c.end
        }
        None => b.span.end,
    };
    let s = localize(doc, &md, &prefix);
    Ok(PatchSet::single(opener.start..end, s))
}

/// Sets the front matter content, creating it when missing (R3.10.2/3, T23).
pub fn plan_set_front_matter(doc: &Document, content: &str) -> Result<PatchSet, OpError> {
    let text = doc.text().to_string();
    match doc.top_block(0) {
        Some(b) if matches!(b.kind, BlockKind::FrontMatter { .. }) => {
            let BlockKind::FrontMatter { closer, .. } = &b.kind else {
                unreachable!()
            };
            let md = write::front_matter(content, closer.slice(&text));
            let s = localize(doc, &md, "");
            Ok(PatchSet::single(b.span.start..b.span.end, s))
        }
        _ => {
            let eol = doc.line_ending().as_str().to_string();
            let md = write::front_matter(content, "---");
            let s = localize(doc, &md, "");
            let sep = if text.len() == crate::bom_len(&text) {
                String::new()
            } else {
                eol
            };
            let at = crate::bom_len(&text);
            Ok(PatchSet::single(at..at, format!("{s}{sep}")))
        }
    }
}

/// Sets one cell of the table at `path` (`row` 0 = header). A pretty-printed table is
/// regenerated aligned; otherwise only the edited row is rewritten in the table's style (A8).
pub fn plan_set_table_cell(
    doc: &Document,
    path: &[usize],
    row: usize,
    col: usize,
    value: &str,
) -> Result<PatchSet, OpError> {
    let text = doc.text().to_string();
    let b = doc.block_at_path(path).ok_or(OpError::NotFound)?;
    if !matches!(b.kind, BlockKind::Table { .. }) {
        return Err(OpError::WrongKind);
    }
    let mut model = TableModel::from_block(&text, &b);
    if row >= model.rows.len() || col >= model.columns() {
        return Err(OpError::OutOfRange);
    }
    let style = PrettyStyle {
        no_outer_pipes: !table::has_outer_pipes(&text, &b),
    };
    let pretty = table::is_pretty(&text, &b);
    let n = model.columns();
    for r in &mut model.rows {
        r.resize(n, String::new());
    }
    // Cells read from the source are already escaped; the new value is raw text.
    let unescape_pipes = |s: &str| s.replace("\\|", "|");
    for r in &mut model.rows {
        for c in r.iter_mut() {
            *c = unescape_pipes(c);
        }
    }
    model.rows[row][col] = value.to_string();
    if pretty {
        let lines = table::pretty_print(&model, &style);
        let mut md = String::new();
        let last_prefix = prefix_of(&text, b.lines.last().unwrap().start).to_string();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                md.push_str(doc.line_ending().as_str());
                let p = b
                    .lines
                    .get(i)
                    .map(|s| prefix_of(&text, s.start).to_string())
                    .unwrap_or_else(|| last_prefix.clone());
                md.push_str(&p);
            }
            md.push_str(l);
        }
        let (s, e) = (b.lines[0].start, b.lines.last().unwrap().end);
        Ok(PatchSet::single(s..e, md))
    } else {
        let line_idx = if row == 0 { 0 } else { row + 1 };
        let span = b.lines[line_idx];
        let new_row = table::plain_row(&model.rows[row], &style);
        Ok(PatchSet::single(span.start..span.end, new_row))
    }
}

/// Replaces a whole block (by path) with freshly generated Markdown.
pub fn plan_replace_block(doc: &Document, path: &[usize], md: &str) -> Result<PatchSet, OpError> {
    let text = doc.text().to_string();
    let b = doc.block_at_path(path).ok_or(OpError::NotFound)?;
    let prefix = prefix_of(&text, b.span.start).to_string();
    let s = localize(doc, md, &prefix);
    let span = b.span;
    Ok(PatchSet::single(span.start..span.end, s))
}

// ---- Applying wrappers (the M0 API; each = planner + `Document::apply_patches`) ----

fn run(doc: &mut Document, ps: PatchSet) -> EditResult {
    let ps = doc.canonicalize_patches(&ps);
    doc.apply_patches(&ps)
}

/// Applies [`plan_toggle_task`].
pub fn toggle_task(doc: &mut Document, pos: usize) -> Result<EditResult, OpError> {
    let ps = plan_toggle_task(doc, pos)?;
    Ok(run(doc, ps))
}

/// Applies [`plan_insert_list_item`].
pub fn insert_list_item(
    doc: &mut Document,
    list_path: &[usize],
    index: usize,
    content: &str,
    task: Option<bool>,
) -> Result<EditResult, OpError> {
    let ps = plan_insert_list_item(doc, list_path, index, content, task)?;
    Ok(run(doc, ps))
}

/// Applies [`plan_insert_block_after`].
pub fn insert_block_after(
    doc: &mut Document,
    after: Option<usize>,
    md: &str,
    wopts: &WriteOptions,
) -> Result<EditResult, OpError> {
    let ps = plan_insert_block_after(doc, after, md, wopts)?;
    Ok(run(doc, ps))
}

/// Applies [`plan_set_code_content`].
pub fn set_code_content(doc: &mut Document, path: &[usize], content: &str) -> Result<EditResult, OpError> {
    let ps = plan_set_code_content(doc, path, content)?;
    Ok(run(doc, ps))
}

/// Applies [`plan_set_front_matter`].
pub fn set_front_matter(doc: &mut Document, content: &str) -> Result<EditResult, OpError> {
    let ps = plan_set_front_matter(doc, content)?;
    Ok(run(doc, ps))
}

/// Applies [`plan_set_table_cell`].
pub fn set_table_cell(
    doc: &mut Document,
    path: &[usize],
    row: usize,
    col: usize,
    value: &str,
) -> Result<EditResult, OpError> {
    let ps = plan_set_table_cell(doc, path, row, col, value)?;
    Ok(run(doc, ps))
}

/// Applies [`plan_replace_block`].
pub fn replace_block(doc: &mut Document, path: &[usize], md: &str) -> Result<EditResult, OpError> {
    let ps = plan_replace_block(doc, path, md)?;
    Ok(run(doc, ps))
}
