//! PROTOTYPE (branch `proto/block-forms`, not product code): lossless codec between a
//! `:::name args` … `:::` block of a Markdown document and the data of its edit form.
//!
//! The site describes its blocks in `<site>/components/blocks.json` (schema
//! `foliwake-blocks/1`: per block a label, optional variants/argument and an ordered list of
//! fields typed `heading`, `text`, `list`, `choices`, `quote`, `image`). This module
//!
//! - finds the block at a source offset and splits its interior into chunks (runs of non-blank
//!   lines), matching them to the manifest's fields in order; chunks no field claims stay
//!   verbatim and are never touched;
//! - exposes every editable piece as a Markdown string with its source span (heading text
//!   without `#` and without a trailing `{#id}`, list item text without its marker, answer text
//!   without `- [x]` and `{#id}`, quote text without `>`, image alt and path);
//! - generates a JSON Schema + JSON Forms UI Schema + data for the form (RU/EN labels), with an
//!   optional site override (`<name>.form.json`);
//! - turns the saved data back into patches of **only the changed spans**: an unchanged save
//!   yields no patch (byte-identical), answer `{#id}` labels, the set of answers, correctness
//!   and the variant are locked (a change is rejected).

use std::ops::Range;

use serde_json::{Map, Value, json};

/// A `:::name args` … `:::` block in the document text.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSrc {
    /// Opener line start .. end of the closing `:::` line (newline excluded).
    pub range: Range<usize>,
    pub name: String,
    /// Words after the name (variant or argument), trimmed.
    pub args: String,
    /// End of the opener line (newline excluded).
    pub opener_end: usize,
    /// Interior: after the opener's newline .. start of the closing line.
    pub inner: Range<usize>,
}

/// Lines of `text[from..to]`: (start, end) with the line break excluded.
fn lines(text: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = from;
    while s < to {
        let e = text[s..to].find('\n').map_or(to, |i| s + i);
        let ce = if e > s && text.as_bytes()[e - 1] == b'\r' {
            e - 1
        } else {
            e
        };
        out.push((s, ce));
        s = e + 1;
    }
    out
}

/// The block whose opener line starts at `offset`.
pub fn block_at(text: &str, offset: usize) -> Option<BlockSrc> {
    let line_end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let opener = text[offset..line_end].trim_end();
    let rest = opener.strip_prefix(":::")?;
    let mut words = rest.splitn(2, char::is_whitespace);
    let name = words.next()?.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let args = words.next().unwrap_or("").trim().to_string();
    let inner_start = (line_end + 1).min(text.len());
    for (s, e) in lines(text, inner_start, text.len()) {
        if text[s..e].trim_end() == ":::" {
            return Some(BlockSrc {
                range: offset..e,
                name,
                args,
                opener_end: offset + opener.len(),
                inner: inner_start..s,
            });
        }
    }
    None
}

/// Every block of the document (opener lines at column 0), in order.
pub fn all_blocks(text: &str) -> Vec<BlockSrc> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        if text[pos..].starts_with(":::")
            && let Some(b) = block_at(text, pos)
        {
            pos = b.range.end;
            out.push(b);
            continue;
        }
        pos = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Heading(usize),
    Quote,
    Choices,
    List,
    Images,
    Para,
}

#[derive(Clone, Debug)]
struct Chunk {
    start: usize,
    end: usize,
    lines: Vec<(usize, usize)>,
    kind: Kind,
}

fn list_marker_len(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return Some(2);
    }
    let d = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if d > 0 && b.len() > d + 1 && matches!(b[d], b'.' | b')') && b[d + 1] == b' ' {
        return Some(d + 2);
    }
    None
}

fn task_marker_len(line: &str) -> Option<(usize, bool)> {
    let m = list_marker_len(line)?;
    let rest = &line[m..];
    for (pat, checked) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
        if rest.starts_with(pat) {
            return Some((m + pat.len(), checked));
        }
    }
    None
}

fn heading_level(line: &str) -> Option<usize> {
    let n = line.bytes().take_while(|&c| c == b'#').count();
    ((1..=6).contains(&n) && line.as_bytes().get(n) == Some(&b' ')).then_some(n)
}

fn is_image_line(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("![") && t.ends_with(')') && t.contains("](")
}

fn chunks(text: &str, inner: &Range<usize>) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = Vec::new();
    let mut cur: Vec<(usize, usize)> = Vec::new();
    let mut flush = |cur: &mut Vec<(usize, usize)>| {
        if cur.is_empty() {
            return;
        }
        let first = &text[cur[0].0..cur[0].1];
        let kind = if let Some(l) = heading_level(first).filter(|_| cur.len() == 1) {
            Kind::Heading(l)
        } else if first.starts_with('>') {
            Kind::Quote
        } else if task_marker_len(first).is_some() {
            Kind::Choices
        } else if list_marker_len(first).is_some() {
            Kind::List
        } else if cur.iter().all(|&(s, e)| is_image_line(&text[s..e])) {
            Kind::Images
        } else {
            Kind::Para
        };
        out.push(Chunk {
            start: cur[0].0,
            end: cur[cur.len() - 1].1,
            lines: std::mem::take(cur),
            kind,
        });
    };
    for (s, e) in lines(text, inner.start, inner.end) {
        if text[s..e].trim().is_empty() {
            flush(&mut cur);
        } else {
            cur.push((s, e));
        }
    }
    flush(&mut cur);
    out
}

/// Text span of a line without a trailing ` {#id}` label (and trailing spaces).
fn strip_label(text: &str, s: usize, e: usize) -> (Range<usize>, Option<String>) {
    let line = text[s..e].trim_end();
    let mut end = s + line.len();
    let mut id = None;
    if line.ends_with('}')
        && let Some(i) = line.rfind(" {#")
    {
        let label = &line[i + 3..line.len() - 1];
        if !label.is_empty() && !label.contains(char::is_whitespace) {
            id = Some(label.to_string());
            end = s + text[s..s + i].trim_end().len();
        }
    }
    (s..end, id)
}

#[derive(Clone, Debug)]
pub struct Item {
    pub line_start: usize,
    pub text: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub id: String,
    pub checked: bool,
    pub text: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Img {
    pub alt: Range<usize>,
    pub src: Range<usize>,
}

#[derive(Clone, Debug)]
pub enum Parts {
    Absent,
    Heading { level: usize, text: Range<usize> },
    Text,
    List { items: Vec<Item> },
    Choices { items: Vec<Choice> },
    Quote,
    Images { items: Vec<Img> },
}

/// One manifest field matched against the block.
#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub def: Value,
    /// Source region of the field's chunks (None: absent).
    pub region: Option<Range<usize>>,
    /// Where an absent field's content is inserted (end of the previous claimed chunk).
    pub anchor: usize,
    pub parts: Parts,
}

/// A decoded block: fields with spans plus the form data built from them.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub block: BlockSrc,
    /// The block's source text when decoded (`text[block.range]`).
    pub source: String,
    pub fields: Vec<Field>,
    pub data: Value,
    /// The block's manifest entry.
    pub def: Value,
}

fn min_of(def: &Value) -> u64 {
    def.get("min")
        .or(def.get("minItems"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
}

fn quote_value(text: &str, region: &Range<usize>) -> String {
    lines(text, region.start, region.end)
        .into_iter()
        .map(|(s, e)| {
            let l = &text[s..e];
            let l = l.strip_prefix('>').unwrap_or(l);
            l.strip_prefix(' ').unwrap_or(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_quote(v: &str) -> String {
    v.lines()
        .map(|l| {
            if l.trim().is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_image(text: &str, s: usize, e: usize) -> Option<Img> {
    let line = &text[s..e];
    let lead = line.len() - line.trim_start().len();
    let t = line.trim();
    let mid = t.find("](")?;
    let alt = s + lead + 2..s + lead + mid;
    let src_start = s + lead + mid + 2;
    let body = &t[mid + 2..t.len() - 1];
    let src_len = match body.find(" \"") {
        Some(i) => i,
        None => body.len(),
    };
    Some(Img {
        alt,
        src: src_start..src_start + src_len,
    })
}

/// Matches the block's chunks to the manifest fields and builds the form data.
pub fn decode(text: &str, block: &BlockSrc, def: &Value) -> Result<Decoded, String> {
    let fdefs = def
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("block `{}` has no fields in blocks.json", block.name))?;
    let chunks = chunks(text, &block.inner);
    let mut ci = 0;
    let mut anchor = block.opener_end;
    let mut fields = Vec::new();
    let mut data = Map::new();
    for fd in fdefs {
        let name = fd.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let ty = fd.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        let want = |k: Kind| match ty.as_str() {
            "heading" => {
                matches!(k, Kind::Heading(l) if l as u64 == fd.get("level").and_then(Value::as_u64).unwrap_or(l as u64))
            }
            "text" => k == Kind::Para,
            "list" => k == Kind::List,
            "choices" => k == Kind::Choices,
            "quote" => k == Kind::Quote,
            "image" => k == Kind::Images,
            _ => false,
        };
        let single = matches!(ty.as_str(), "heading" | "quote");
        let mut taken: Vec<&Chunk> = Vec::new();
        while ci < chunks.len() && want(chunks[ci].kind) && !(single && !taken.is_empty()) {
            taken.push(&chunks[ci]);
            ci += 1;
        }
        let region = (!taken.is_empty()).then(|| taken[0].start..taken[taken.len() - 1].end);
        let (parts, value) = match (&region, ty.as_str()) {
            (None, "list" | "choices" | "image") => (Parts::Absent, json!([])),
            (None, _) => (Parts::Absent, json!("")),
            (Some(r), "heading") => {
                let (s, e) = taken[0].lines[0];
                let level = heading_level(&text[s..e]).unwrap_or(1);
                let (span, _id) = strip_label(text, s + level + 1, e);
                let _ = r;
                let v = json!(&text[span.clone()]);
                (Parts::Heading { level, text: span }, v)
            }
            (Some(r), "text") => (Parts::Text, json!(&text[r.clone()])),
            (Some(r), "quote") => (Parts::Quote, json!(quote_value(text, r))),
            (Some(_), "list") => {
                let mut items: Vec<Item> = Vec::new();
                for c in &taken {
                    for &(s, e) in &c.lines {
                        let l = &text[s..e];
                        let indent = l.len() - l.trim_start().len();
                        if indent == 0
                            && let Some(m) = list_marker_len(l)
                        {
                            let span = s + m..s + l.trim_end().len();
                            items.push(Item {
                                line_start: s,
                                text: span,
                            });
                        } else if let Some(last) = items.last_mut() {
                            last.text.end = s + l.trim_end().len(); // continuation line
                        }
                    }
                }
                let v: Vec<Value> = items.iter().map(|i| json!(&text[i.text.clone()])).collect();
                (Parts::List { items }, json!(v))
            }
            (Some(_), "choices") => {
                let mut items = Vec::new();
                let mut obj = Map::new();
                for c in &taken {
                    for &(s, e) in &c.lines {
                        let Some((m, checked)) = task_marker_len(&text[s..e]) else {
                            continue;
                        };
                        let (span, id) = strip_label(text, s + m, e);
                        let id =
                            id.ok_or_else(|| format!("answer without a {{#id}} label: `{}`", text[s..e].trim()))?;
                        obj.insert(id.clone(), json!({"text": &text[span.clone()], "correct": checked}));
                        items.push(Choice {
                            id,
                            checked,
                            text: span,
                        });
                    }
                }
                (Parts::Choices { items }, Value::Object(obj))
            }
            (Some(_), "image") => {
                let mut items = Vec::new();
                let mut obj = Map::new();
                for c in &taken {
                    for &(s, e) in &c.lines {
                        if let Some(img) = parse_image(text, s, e) {
                            obj.insert(
                                (items.len() + 1).to_string(),
                                json!({"alt": &text[img.alt.clone()], "src": &text[img.src.clone()]}),
                            );
                            items.push(img);
                        }
                    }
                }
                (Parts::Images { items }, Value::Object(obj))
            }
            (Some(_), other) => return Err(format!("unsupported field type `{other}`")),
        };
        data.insert(name.clone(), value);
        fields.push(Field {
            name,
            ty,
            def: fd.clone(),
            region: region.clone(),
            anchor,
            parts,
        });
        if let Some(r) = region {
            anchor = r.end;
        }
    }
    if def.get("variants").is_some() {
        data.insert("_variant".into(), json!(block.args));
    } else if def.get("argument").is_some() {
        data.insert("_argument".into(), json!(block.args));
    }
    Ok(Decoded {
        block: block.clone(),
        source: text[block.range.clone()].to_string(),
        fields,
        data: Value::Object(data),
        def: def.clone(),
    })
}

// ---------------------------------------------------------------- form generation

fn tr<'a>(v: Option<&'a Value>, lang: &str) -> Option<&'a str> {
    let v = v?;
    v.as_str()
        .or_else(|| v.get(lang).or(v.get("en")).and_then(Value::as_str))
}

fn type_label(ty: &str, lang: &str) -> &'static str {
    let ru = lang == "ru";
    match ty {
        "heading" => {
            if ru {
                "Заголовок"
            } else {
                "Heading"
            }
        }
        "text" => {
            if ru {
                "Текст"
            } else {
                "Text"
            }
        }
        "list" => {
            if ru {
                "Список"
            } else {
                "List"
            }
        }
        "choices" => {
            if ru {
                "Ответы"
            } else {
                "Answers"
            }
        }
        "quote" => {
            if ru {
                "Цитата"
            } else {
                "Quote"
            }
        }
        "image" => {
            if ru {
                "Картинки"
            } else {
                "Pictures"
            }
        }
        _ => "?",
    }
}

fn deep_merge(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                deep_merge(a.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (a, b) => *a = b.clone(),
    }
}

/// The form of a decoded block: (title, JSON Schema, UI Schema). `over`: the site's
/// `<name>.form.json` (`labels` {field: {ru,en}}, `options` {field: {...}} merged into the
/// field's control options, `schema` deep-merged, `uischema` replacing the generated one).
pub fn form(dec: &Decoded, lang: &str, over: Option<&Value>) -> (String, Value, Value) {
    let ru = lang == "ru";
    let title = tr(dec.def.get("label"), lang).unwrap_or(&dec.block.name).to_string();
    let mut props = Map::new();
    let mut els: Vec<Value> = Vec::new();
    let label_of = |f: &Field| -> String {
        tr(f.def.get("label"), lang)
            .or_else(|| tr(over.and_then(|o| o.get("labels")).and_then(|l| l.get(&f.name)), lang))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} ({})", type_label(&f.ty, lang), f.name))
    };
    let opts_of = |f: &Field, base: Value| -> Value {
        let mut o = base;
        if let Some(x) = over.and_then(|o| o.get("options")).and_then(|l| l.get(&f.name)) {
            deep_merge(&mut o, x);
        }
        o
    };
    // variant / argument: shown, locked (kept in the data so a change can be detected)
    if let Some(vs) = dec.def.get("variants") {
        let v = tr(vs.get(&dec.block.args), lang).unwrap_or(&dec.block.args);
        props.insert("_variant".into(), json!({"type": "string"}));
        els.push(json!({"type": "Label",
            "text": format!("{}: {v} 🔒", if ru { "Вариант" } else { "Variant" })}));
    } else if let Some(a) = dec.def.get("argument") {
        let n = a.get("name").and_then(Value::as_str).unwrap_or("argument");
        props.insert("_argument".into(), json!({"type": "string"}));
        els.push(json!({"type": "Label", "text": format!("{n}: {} 🔒", dec.block.args)}));
    }
    for f in &dec.fields {
        let scope = format!("#/properties/{}", f.name);
        let label = label_of(f);
        let min = min_of(&f.def);
        match f.ty.as_str() {
            "heading" => {
                props.insert(
                    f.name.clone(),
                    json!({"type": "string", "minLength": if min == 0 { 0 } else { 1 }}),
                );
                els.push(json!({"type": "Control", "scope": scope, "label": label, "options": opts_of(f, json!({}))}));
            }
            "text" | "quote" => {
                let mut s = json!({"type": "string"});
                if f.ty == "text" && min > 0 {
                    s["minLength"] = json!(1);
                }
                props.insert(f.name.clone(), s);
                els.push(json!({"type": "Control", "scope": scope, "label": label, "options": opts_of(f, json!({"multi": true}))}));
            }
            "list" => {
                let mut s = json!({"type": "array", "items": {"type": "string", "minLength": 1}});
                if let Some(n) = f.def.get("minItems").and_then(Value::as_u64) {
                    s["minItems"] = json!(n);
                }
                if let Some(n) = f.def.get("maxItems").and_then(Value::as_u64) {
                    s["maxItems"] = json!(n);
                }
                props.insert(f.name.clone(), s);
                els.push(json!({"type": "Control", "scope": scope, "label": label, "options": opts_of(f, json!({}))}));
            }
            "choices" => {
                let mut cp = Map::new();
                let mut group = vec![json!({"type": "Label", "text": if ru {
                    "Метки {#…}, состав ответов и правильность закреплены контрактом урока — здесь меняется только текст."
                } else {
                    "Labels {#…}, the set of answers and correctness are fixed by the lesson contract; only the text can change."
                }})];
                if let Parts::Choices { items } = &f.parts {
                    for c in items {
                        cp.insert(
                            c.id.clone(),
                            json!({"type": "object", "properties": {
                            "text": {"type": "string", "minLength": 1}, "correct": {"type": "boolean"}}}),
                        );
                        let mark = if c.checked {
                            if ru { "✓ верный" } else { "✓ correct" }
                        } else if ru {
                            "неверный"
                        } else {
                            "wrong"
                        };
                        group.push(json!({"type": "Control",
                            "scope": format!("{scope}/properties/{}/properties/text", c.id),
                            "label": format!("{{#{}}} — {mark}", c.id), "options": opts_of(f, json!({}))}));
                    }
                }
                props.insert(f.name.clone(), json!({"type": "object", "properties": cp}));
                els.push(json!({"type": "Group", "label": label, "elements": group}));
            }
            "image" => {
                let mut ip = Map::new();
                let mut group = Vec::new();
                if let Parts::Images { items } = &f.parts {
                    for i in 1..=items.len() {
                        ip.insert(
                            i.to_string(),
                            json!({"type": "object", "properties": {
                            "alt": {"type": "string"}, "src": {"type": "string", "minLength": 1}}}),
                        );
                        group.push(json!({"type": "Control", "scope": format!("{scope}/properties/{i}/properties/alt"),
                            "label": format!("{} {i}: {}", if ru { "Картинка" } else { "Picture" }, if ru { "описание (alt)" } else { "description (alt)" }),
                            "options": {"multi": true}}));
                        group.push(json!({"type": "Control", "scope": format!("{scope}/properties/{i}/properties/src"),
                            "label": format!("{} {i}: {}", if ru { "Картинка" } else { "Picture" }, if ru { "путь к файлу" } else { "file path" })}));
                    }
                }
                if group.is_empty() {
                    group.push(json!({"type": "Label", "text": if ru { "Картинок нет" } else { "No pictures" }}));
                }
                props.insert(f.name.clone(), json!({"type": "object", "properties": ip}));
                els.push(json!({"type": "Group", "label": label, "elements": group}));
            }
            _ => {}
        }
    }
    let mut schema = json!({"type": "object", "properties": props});
    let mut ui = json!({"type": "VerticalLayout", "elements": els});
    if let Some(o) = over {
        if let Some(s) = o.get("schema") {
            deep_merge(&mut schema, s);
        }
        if let Some(u) = o.get("uischema") {
            ui = u.clone();
        }
    }
    (title, schema, ui)
}

// ---------------------------------------------------------------- save

/// A text replacement in the document (absolute offsets).
pub type Edit = (Range<usize>, String);

fn norm_multi(s: &str) -> String {
    s.replace("\r\n", "\n").trim_matches('\n').trim_end().to_string()
}

fn norm_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ").trim().to_string()
}

/// Start of the blank run before `pos` (after the previous non-whitespace byte, not before
/// `floor`): removing a field also removes the blank lines that separated it.
fn blank_before(text: &str, pos: usize, floor: usize) -> usize {
    let t = &text[floor..pos];
    floor + t.trim_end().len()
}

/// The edits that turn the block into `data`. Only changed fields produce edits; with
/// `force` every field is re-rendered from its value (fidelity check of the renderers).
/// Locked parts (answer ids, set, correctness, variant/argument) and limits are validated.
pub fn edits(text: &str, dec: &Decoded, data: &Value, force: bool, lang: &str) -> Result<Vec<Edit>, String> {
    let ru = lang == "ru";
    let mut out: Vec<Edit> = Vec::new();
    for k in ["_variant", "_argument"] {
        if data.get(k).is_some() && data.get(k) != dec.data.get(k) {
            return Err(if ru {
                "Вариант блока менять нельзя.".into()
            } else {
                "The variant is locked.".into()
            });
        }
    }
    let floor = dec.block.inner.start.min(dec.block.opener_end);
    for f in &dec.fields {
        let old = &dec.data[&f.name];
        let new = data.get(&f.name).unwrap_or(old);
        let label = f.name.as_str();
        let min = min_of(&f.def);
        match (&f.parts, f.ty.as_str()) {
            (_, "heading" | "text" | "quote") => {
                let raw = new.as_str().unwrap_or("");
                let v = if f.ty == "heading" {
                    norm_line(raw)
                } else {
                    norm_multi(raw)
                };
                let o = old.as_str().unwrap_or("");
                if v == o && !force {
                    continue;
                }
                if v.is_empty() && min > 0 {
                    return Err(if ru {
                        format!("Поле «{label}» не может быть пустым.")
                    } else {
                        format!("Field `{label}` must not be empty.")
                    });
                }
                let level = f.def.get("level").and_then(Value::as_u64).unwrap_or(2) as usize;
                let rendered = match f.ty.as_str() {
                    "quote" => render_quote(&v),
                    _ => v.clone(),
                };
                match (&f.parts, &f.region) {
                    (Parts::Heading { text: span, .. }, Some(r)) => {
                        if v.is_empty() {
                            out.push((blank_before(text, r.start, floor)..r.end, String::new()));
                        } else {
                            out.push((span.clone(), rendered));
                        }
                    }
                    (_, Some(r)) => {
                        if v.is_empty() {
                            out.push((blank_before(text, r.start, floor)..r.end, String::new()));
                        } else {
                            out.push((r.clone(), rendered));
                        }
                    }
                    (_, None) => {
                        if !v.is_empty() {
                            let body = if f.ty == "heading" {
                                format!("{} {rendered}", "#".repeat(level))
                            } else {
                                rendered
                            };
                            out.push((f.anchor..f.anchor, format!("\n\n{body}")));
                        }
                    }
                }
            }
            (Parts::List { .. } | Parts::Absent, "list") => {
                let empty = Vec::new();
                let items = if let Parts::List { items } = &f.parts {
                    items
                } else {
                    &empty
                };
                let vals: Vec<String> = new
                    .as_array()
                    .map(|a| a.iter().map(|x| norm_line(x.as_str().unwrap_or(""))).collect())
                    .unwrap_or_default();
                let olds: Vec<String> = items.iter().map(|i| text[i.text.clone()].to_string()).collect();
                if vals == olds && !force {
                    continue;
                }
                let lo = f.def.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
                let hi = f
                    .def
                    .get("maxItems")
                    .and_then(Value::as_u64)
                    .map_or(usize::MAX, |n| n as usize);
                if vals.len() < lo || vals.len() > hi {
                    return Err(if ru {
                        format!("В списке «{label}» должно быть от {lo} до {hi} пунктов.")
                    } else {
                        format!("List `{label}` needs {lo}..{hi} items.")
                    });
                }
                if vals.iter().any(String::is_empty) {
                    return Err(if ru {
                        format!("Пустой пункт в списке «{label}».")
                    } else {
                        format!("Empty item in `{label}`.")
                    });
                }
                if vals.is_empty() {
                    if let Some(r) = &f.region {
                        out.push((blank_before(text, r.start, floor)..r.end, String::new()));
                    }
                    continue;
                }
                if items.is_empty() {
                    let body: Vec<String> = vals.iter().map(|v| format!("- {v}")).collect();
                    out.push((f.anchor..f.anchor, format!("\n\n{}", body.join("\n"))));
                    continue;
                }
                for (i, v) in vals.iter().enumerate().take(items.len()) {
                    if force || *v != olds[i] {
                        out.push((items[i].text.clone(), v.clone()));
                    }
                }
                let last = &items[items.len() - 1];
                if vals.len() > items.len() {
                    let marker = &text[last.line_start..last.text.start];
                    let add: String = vals[items.len()..].iter().map(|v| format!("\n{marker}{v}")).collect();
                    out.push((last.text.end..last.text.end, add));
                } else if vals.len() < items.len() {
                    let keep = &items[vals.len() - 1];
                    out.push((keep.text.end..last.text.end, String::new()));
                }
            }
            (Parts::Choices { items }, "choices") => {
                let obj = new.as_object().cloned().unwrap_or_default();
                if obj.len() != items.len() || items.iter().any(|c| !obj.contains_key(&c.id)) {
                    return Err(if ru {
                        "Состав ответов и их метки менять нельзя (контракт урока).".into()
                    } else {
                        "The set of answers and their labels are locked (lesson contract).".into()
                    });
                }
                for c in items {
                    let e = &obj[&c.id];
                    if e.get("correct").and_then(Value::as_bool).unwrap_or(c.checked) != c.checked {
                        return Err(if ru {
                            format!("Правильность ответа {{#{}}} менять нельзя (контракт урока).", c.id)
                        } else {
                            format!("Correctness of {{#{}}} is locked (lesson contract).", c.id)
                        });
                    }
                    let v = norm_line(e.get("text").and_then(Value::as_str).unwrap_or(""));
                    if v.is_empty() {
                        return Err(if ru {
                            format!("Пустой текст ответа {{#{}}}.", c.id)
                        } else {
                            format!("Empty answer {{#{}}}.", c.id)
                        });
                    }
                    if force || v != text[c.text.clone()] {
                        out.push((c.text.clone(), v));
                    }
                }
            }
            (Parts::Images { items }, "image") => {
                let obj = new.as_object().cloned().unwrap_or_default();
                for (i, img) in items.iter().enumerate() {
                    let Some(e) = obj.get(&(i + 1).to_string()) else {
                        continue;
                    };
                    let alt = norm_line(e.get("alt").and_then(Value::as_str).unwrap_or(""));
                    let src = norm_line(e.get("src").and_then(Value::as_str).unwrap_or(""));
                    if src.is_empty() {
                        return Err(if ru {
                            "Путь к картинке не может быть пустым.".into()
                        } else {
                            "Empty picture path.".into()
                        });
                    }
                    if force || alt != text[img.alt.clone()] {
                        out.push((img.alt.clone(), alt));
                    }
                    if force || src != text[img.src.clone()] {
                        let src = if src.contains(char::is_whitespace) && !src.starts_with('<') {
                            format!("<{src}>")
                        } else {
                            src
                        };
                        out.push((img.src.clone(), src));
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|(r, _)| (r.start, r.end));
    Ok(out)
}

/// Applies non-overlapping edits (sorted) to `text`.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (r, s) in edits {
        out.push_str(&text[pos..r.start]);
        out.push_str(s);
        pos = r.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// The site root of a document: nearest ancestor folder with `components/blocks.json`.
pub fn site_root(doc: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = doc.parent();
    while let Some(d) = dir {
        if d.join("components").join("blocks.json").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Reads `<root>/components/blocks.json` (schema `foliwake-blocks/1`) → its `blocks` object.
pub fn load_manifest(root: &std::path::Path) -> Result<Value, String> {
    let p = root.join("components").join("blocks.json");
    let s = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let v: Value = serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
    if v.get("schema").and_then(Value::as_str) != Some("foliwake-blocks/1") {
        return Err(format!("{}: unknown schema {:?}", p.display(), v.get("schema")));
    }
    Ok(v.get("blocks").cloned().unwrap_or(Value::Null))
}
