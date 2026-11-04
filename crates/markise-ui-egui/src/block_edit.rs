//! PROTOTYPE (branch `proto/block-forms`, not product code): the AEM-style edit dialog of a
//! `:::name` block shown as a snapshot.
//!
//! "✎ Edit" on a block image → Markise decodes the block with
//! [`markise_engine::block_form`] against the site's `components/blocks.json`, generates the
//! JSON Schema / UI Schema / data (plus the site's optional `blocks/<name>/<name>.form.json`,
//! `.form.js` custom renderers and `.form.css`) and shows them in the right side panel
//! ([`crate::form_panel`], an in-process WKWebView docked in the Markise window). On Save the
//! page sends the data back; Markise checks the document did not change
//! since the dialog opened, turns the data into patches of the changed spans only and applies
//! them as one undoable transaction ("Edit Block"); the snapshot then re-renders by itself
//! (the block's content key changed). Cancel / Esc / the close button change nothing.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use markise_doc::{Patch, PatchSet};
use markise_engine::block_form as bf;
use markise_engine::editor::EditorView;
use serde_json::{Value, json};

use crate::block_snap::ThemeVars;

struct Open {
    id: u64,
    dec: bf::Decoded,
    revision: u64,
    sent: Instant,
    lang: String,
    /// A new block not written yet (inserted on Save).
    draft: Option<Draft>,
}

/// Where a new block goes: `start..end` of the document is replaced (an empty range inserts;
/// a `/query` paragraph is replaced), checked again on Save.
#[derive(Clone, Debug)]
pub struct InsertTarget {
    pub start: usize,
    pub end: usize,
    pub revision: u64,
    /// Up to 40 bytes before `start` (finds the point again after other edits).
    pub before: String,
    pub replaced: String,
}

impl InsertTarget {
    pub fn new(text: &str, start: usize, end: usize, revision: u64) -> Self {
        let mut b = start.saturating_sub(40);
        while !text.is_char_boundary(b) {
            b += 1;
        }
        InsertTarget {
            start,
            end,
            revision,
            before: text[b..start].to_string(),
            replaced: text[start..end].to_string(),
        }
    }

    /// The range in the current `text`, or None when the point cannot be found again.
    fn locate(&self, text: &str, revision: u64) -> Option<(usize, usize)> {
        if revision == self.revision {
            return Some((self.start, self.end));
        }
        let needle = format!("{}{}", self.before, self.replaced);
        let mut hits = text.match_indices(&needle);
        let first = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        let s = first.0 + self.before.len();
        Some((s, s + self.replaced.len()))
    }
}

struct Draft {
    markdown: String,
    target: InsertTarget,
}

/// Live preview of the values in the form (nothing is written to the document).
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewAt {
    /// The existing block whose `:::` line starts here.
    Edit(usize),
    /// A new block at this insertion range.
    Insert(usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub at: PreviewAt,
    /// The block's Markdown as the form would write it.
    pub markdown: String,
}

/// Edit dialog state of one viewer.
pub struct FormState {
    events: Receiver<Value>,
    doc: Option<PathBuf>,
    /// The block library's root (package cache, dev folder or site).
    root: Option<PathBuf>,
    open: Option<Open>,
    seq: u64,
    log: bool,
    /// (cold, click → form painted ms, helper op → painted ms) of every open (measurements).
    pub opens: Vec<(bool, f64, f64)>,
    /// Results of saves: Ok(edit count) or Err(message) (measurement script).
    pub saves: Vec<Result<usize, String>>,
    /// Why the dialog could not open (shown over the pane for a few seconds).
    pub error: Option<(String, Instant)>,
    /// Live preview of the open form (latest values; replaced by every change).
    pub preview: Option<Preview>,
}

pub fn lang() -> String {
    if let Ok(l) = std::env::var("MARKISE_LANG") {
        return if l.starts_with("en") { "en".into() } else { "ru".into() };
    }
    // PROTOTYPE: the owner's authoring language (Russian) unless MARKISE_LANG=en
    "ru".into()
}

/// Folder of a block's site files: `<root>/blocks/<name>/` if it has `<name>.form.json`,
/// `.form.js` or `.form.css`, else the fallback site (`MARKISE_BLOCK_SITE`).
fn form_dir(root: Option<&Path>, name: &str) -> Option<PathBuf> {
    let fallback = std::env::var_os("MARKISE_BLOCK_SITE").map(PathBuf::from);
    [root.map(Path::to_path_buf), fallback]
        .into_iter()
        .flatten()
        .map(|r| r.join("blocks").join(name))
        .find(|d| {
            ["json", "js", "css"]
                .iter()
                .any(|e| d.join(format!("{name}.form.{e}")).is_file())
        })
}

impl FormState {
    pub fn new(doc: Option<PathBuf>, root: Option<PathBuf>, ctx: &egui::Context) -> Self {
        let (tx, events) = std::sync::mpsc::channel();
        crate::form_panel::set_sink(tx, ctx.clone());
        FormState {
            events,
            doc,
            root,
            open: None,
            seq: 0,
            log: std::env::var_os("MARKISE_BLOCK_LOG").is_some_and(|v| v != "0"),
            opens: Vec::new(),
            saves: Vec::new(),
            error: None,
            preview: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        crate::form_panel::handle(v.clone());
        Ok(())
    }

    /// The library root this viewer uses.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Opens the dialog for the block whose `:::` line starts at `offset`.
    pub fn open(&mut self, text: &str, offset: usize, revision: u64, theme: &ThemeVars) -> Result<(), String> {
        let line = text[..offset].matches('\n').count() + 1;
        self.open_src(text, offset, revision, theme, None, line, text)
    }

    /// Opens a draft of a new block (`markdown`: a preset) to be inserted at `target` on Save.
    pub fn open_draft(
        &mut self,
        doc_text: &str,
        markdown: &str,
        target: InsertTarget,
        theme: &ThemeVars,
    ) -> Result<(), String> {
        let line = doc_text[..target.start.min(doc_text.len())].matches('\n').count() + 1;
        let revision = target.revision;
        let md = markdown.trim_end().to_string() + "\n";
        self.open_src(
            &md.clone(),
            0,
            revision,
            theme,
            Some(Draft { markdown: md, target }),
            line,
            doc_text,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_src(
        &mut self,
        text: &str,
        offset: usize,
        revision: u64,
        theme: &ThemeVars,
        draft: Option<Draft>,
        line: usize,
        doc_text: &str,
    ) -> Result<(), String> {
        let t = Instant::now();
        let doc = self.doc.as_deref().ok_or("the document has no path (no site)")?;
        let root = self
            .root
            .clone()
            .ok_or("для этого документа не подключена библиотека блоков")?;
        let manifest = bf::load_manifest(&root)?;
        let block = bf::block_at(text, offset).ok_or("no :::block at this position")?;
        let def = manifest
            .get(&block.name)
            .ok_or_else(|| format!("`{}` is not in blocks.json", block.name))?;
        let dec = bf::decode(text, &block, def)?;
        let lang = lang();
        let ru = lang == "ru";
        let package = crate::block_lib::is_package(&root);
        let dir = form_dir(Some(&root), &block.name);
        let read = |ext: &str| {
            dir.as_ref()
                .and_then(|d| std::fs::read_to_string(d.join(format!("{}.form.{ext}", block.name))).ok())
        };
        let over: Option<Value> = read("json").and_then(|s| serde_json::from_str(&s).ok());
        let (title, schema, mut uischema) = bf::form(&dec, &lang, over.as_ref());
        // a variant with a site-declared built-in widget (`:::question directions` → the direction
        // picker): the answers are picked with that widget
        if block.name == "question"
            && let Some(w) = crate::block_lib::widget_for(
                &root,
                &lang,
                &block.name,
                block.args.split_whitespace().next().unwrap_or(""),
            )
            && let Some(els) = uischema["elements"].as_array_mut()
        {
            for e in els.iter_mut() {
                if e["scope"] == "#/properties/options" {
                    e["options"]["format"] = json!(w["widget"]);
                    e["options"]["answers"] = w["answers"].clone();
                    e["label"] = json!(if ru {
                        "Разрешённые направления"
                    } else {
                        "Allowed directions"
                    });
                }
            }
        }
        let file = doc
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        // a lesson contract nearby may reject changed answers at build time (not rewritten here)
        let warning = if block.name == "question" {
            contract_of(doc, doc_text).map(|c| {
                if ru {
                    format!("Контракт урока {c} может отклонить изменения ответов (состав, верность) при сборке сайта.")
                } else {
                    format!("The lesson contract {c} may reject changed answers when the site is built.")
                }
            })
        } else {
            None
        };
        self.seq += 1;
        let id = self.seq;
        let note = if draft.is_some() {
            format!(
                "{file}, {} {line} · {}",
                if ru {
                    "вставка перед строкой"
                } else {
                    "insert at line"
                },
                if ru {
                    "новый блок, ещё не записан"
                } else {
                    "new block, not written yet"
                }
            )
        } else {
            format!(
                "{file}, {} {line} · :::{} {}",
                if ru { "строка" } else { "line" },
                block.name,
                block.args
            )
        };
        let req = json!({
            "op": "form_open", "id": id,
            "title": if draft.is_some() { format!("{} {title}", if ru { "Новый блок:" } else { "New block:" }) } else { title },
            "block": block.name, "draft": draft.is_some(), "warning": warning,
            "schema": schema, "uischema": uischema, "data": dec.data, "lang": lang,
            "theme": if theme.dark { "dark" } else { "light" }, "vars": theme.vars,
            "note": note,
            // packages: declarative forms only (no site form JS)
            "site_js": if package { String::new() } else { read("js").unwrap_or_default() },
            "site_css": read("css").unwrap_or_default(),
            "base": doc.parent().map(file_url_dir).unwrap_or_default(),
        });
        let ms_build = t.elapsed().as_secs_f64() * 1e3;
        self.send(&req)?;
        if self.log {
            eprintln!(
                "[blockform] open #{id} :::{} at {offset}{}: form built in {ms_build:.2} ms (site files: {:?})",
                block.name,
                if draft.is_some() { " (draft)" } else { "" },
                dir
            );
        }
        // a new block is previewed at its point right away (the preset as it is)
        self.preview = draft.as_ref().map(|d| Preview {
            at: PreviewAt::Insert(d.target.start, d.target.end),
            markdown: d.markdown.clone(),
        });
        self.open = Some(Open {
            id,
            dec,
            revision,
            sent: t,
            lang,
            draft,
        });
        Ok(())
    }

    /// Hides the dialog without saving (measurement script).
    pub fn close(&mut self) {
        self.open = None;
        self.preview = None;
        let _ = self.send(&json!({"op": "form_close"}));
    }

    /// Runs JavaScript in the form page (measurement script).
    pub fn eval(&self, js: &str) {
        let _ = self.send(&json!({"op": "form_eval", "js": js}));
    }

    /// Handles the helper's events; a save edits the document through `ed`.
    pub fn poll(&mut self, ed: &mut EditorView) {
        while let Ok(ev) = self.events.try_recv() {
            let id = ev["id"].as_u64();
            let current = self.open.as_ref().map(|o| o.id);
            match ev["event"].as_str().unwrap_or("") {
                "shown" => {
                    if let Some(o) = self.open.as_ref().filter(|o| Some(o.id) == id) {
                        let ms = o.sent.elapsed().as_secs_f64() * 1e3;
                        let cold = ev["cold"] == true;
                        let helper = ev["ms_open"].as_f64().unwrap_or(f64::NAN);
                        self.opens.push((cold, ms, helper));
                        if self.log {
                            eprintln!(
                                "[blockform] #{} shown: click→painted {ms:.1} ms (helper {helper:.1} ms, page {} ms, {})",
                                o.id,
                                ev["ms_page"],
                                if cold { "cold" } else { "warm" }
                            );
                        }
                    }
                }
                "pick" if id == current && id.is_some() => {
                    let dir = self.doc.as_deref().and_then(Path::parent).map(Path::to_path_buf);
                    let mut dlg =
                        rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "svg"]);
                    if let Some(d) = &dir {
                        dlg = dlg.set_directory(d);
                    }
                    if let (Some(file), Some(d)) = (dlg.pick_file(), dir) {
                        let rel = relative_path(&d, &file);
                        self.eval(&format!(
                            "window.markiseForm.picked({}, {}, {})",
                            ev["path"],
                            ev["index"],
                            serde_json::to_string(&rel).unwrap_or_default()
                        ));
                    }
                }
                "cancel" => {
                    if id == current {
                        self.open = None;
                        self.preview = None;
                    }
                }
                "draft" if id == current && id.is_some() => {
                    // live preview: render the values without writing (latest wins)
                    if let Some(o) = self.open.as_ref() {
                        let text = ed.session.text();
                        self.preview = match &o.draft {
                            Some(d) => Some(Preview {
                                at: PreviewAt::Insert(d.target.start, d.target.end),
                                markdown: bf::preview(&d.markdown, &o.dec, &ev["data"], &o.lang),
                            }),
                            None if text.get(o.dec.block.range.clone()) == Some(o.dec.source.as_str()) => {
                                Some(Preview {
                                    at: PreviewAt::Edit(o.dec.block.range.start),
                                    markdown: bf::preview(text, &o.dec, &ev["data"], &o.lang),
                                })
                            }
                            None => None,
                        };
                    }
                }
                "save" if id == current && id.is_some() => {
                    let res = self.save(ed, &ev["data"]);
                    if self.log {
                        eprintln!("[blockform] save #{}: {res:?}", id.unwrap_or(0));
                    }
                    let reply = match &res {
                        Ok(_) => json!({"op": "form_result", "id": id, "ok": true}),
                        Err(e) => json!({"op": "form_result", "id": id, "ok": false, "error": e}),
                    };
                    if res.is_ok() {
                        self.open = None;
                        self.preview = None;
                    }
                    self.saves.push(res);
                    let _ = self.send(&reply);
                }
                _ => {}
            }
        }
    }

    fn save(&mut self, ed: &mut EditorView, data: &Value) -> Result<usize, String> {
        let o = self.open.as_ref().ok_or("no open dialog")?;
        let ru = o.lang == "ru";
        let text = ed.session.text();
        if let Some(d) = &o.draft {
            // a new block: build it from the preset, then insert it in one transaction
            let edits = bf::edits(&d.markdown, &o.dec, data, false, &o.lang)?;
            let block = bf::apply(&d.markdown, &edits).trim_end().to_string();
            let (s, e) = d.target.locate(text, ed.session.revision().counter).ok_or_else(|| {
                if ru {
                    "Место вставки изменилось и не найдено — блок не вставлен. Нажмите «Отмена» и вставьте заново."
                        .to_string()
                } else {
                    "The insertion point changed and was not found; nothing was inserted.".to_string()
                }
            })?;
            let before = &text[..s];
            let after = &text[e..];
            let pre = if before.is_empty() || before.ends_with("\n\n") {
                ""
            } else if before.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            let post = if after.starts_with("\n\n") {
                ""
            } else if after.starts_with('\n') || after.is_empty() {
                "\n"
            } else {
                "\n\n"
            };
            let ps = PatchSet::new(vec![Patch::verbatim(s..e, format!("{pre}{block}{post}"))])
                .map_err(|e| format!("patches: {e:?}"))?;
            ed.edit(|s| s.apply_external(&ps, "Insert Block", false))
                .map_err(|e| format!("edit: {e:?}"))?;
            return Ok(1);
        }
        if ed.session.revision().counter != o.revision
            || text.get(o.dec.block.range.clone()) != Some(o.dec.source.as_str())
        {
            return Err(if ru {
                "Документ изменился после открытия формы — изменения не сохранены. Нажмите «Отмена» и откройте форму заново."
                    .into()
            } else {
                "The document changed after the dialog opened; nothing was saved. Cancel and open the dialog again."
                    .into()
            });
        }
        let edits = bf::edits(text, &o.dec, data, false, &o.lang)?;
        if edits.is_empty() {
            return Ok(0);
        }
        let n = edits.len();
        let ps = PatchSet::new(edits.into_iter().map(|(r, s)| Patch::verbatim(r, s)).collect())
            .map_err(|e| format!("patches: {e:?}"))?;
        ed.edit(|s| s.apply_external(&ps, "Edit Block", false))
            .map_err(|e| format!("edit: {e:?}"))?;
        Ok(n)
    }
}

// ---------------------------------------------------------------- measurement script

/// PROTOTYPE measurement driver (`MARKISE_PROTO_FORMS=1`, Side by Side): once every block
/// image is shown it opens the dialog on the blocks in turn (cold, then warm opens), saves
/// every block unchanged (must stay byte-identical), saves one real edit and undoes it, tries
/// a save after the document changed (must be rejected) and a locked change (rejected),
/// prints the numbers; screenshots go to `MARKISE_PROTO_SHOTS`, `MARKISE_PROTO_EXIT=1` exits.
#[derive(Default)]
pub struct FormScript {
    step: usize,
    sub: usize,
    since: Option<Instant>,
    waiting: Option<(usize, usize)>,
    original: String,
    log: Vec<String>,
    caret_test: Option<(usize, usize)>,
}

const WARM_OPENS: usize = 12;

impl FormScript {
    pub fn enabled() -> bool {
        std::env::var_os("MARKISE_PROTO_FORMS").is_some_and(|v| v != "0")
    }

    fn shot_form(form: &FormState, name: &str) {
        if let Some(dir) = std::env::var_os("MARKISE_PROTO_SHOTS") {
            let path = PathBuf::from(dir).join(format!("{name}.png"));
            let _ = form.send(&json!({"op": "form_shot", "path": path}));
        }
    }

    fn say(&mut self, s: String) {
        eprintln!("[proto-forms] {s}");
        self.log.push(s);
    }

    pub fn frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        {
            thread_local!(static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
            if LAST.with(|l| l.get().is_none_or(|t| t.elapsed().as_secs_f32() > 2.0)) {
                LAST.with(|l| l.set(Some(Instant::now())));
                eprintln!(
                    "[proto-forms] tick: step {} sub {} waiting {:?}",
                    self.step, self.sub, self.waiting
                );
            }
        }
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        let after = |ms: u64| now - since >= std::time::Duration::from_millis(ms);
        let offsets: Vec<(String, usize)> = snaps.blocks.iter().map(|b| (b.name.clone(), b.offset)).collect();
        let (opens, saves) = (snaps.form.opens.len(), snaps.form.saves.len());
        let all = !snaps.current.is_empty() && snaps.current.iter().all(|&c| c);
        let text = viewer.ed.session.text().to_string();
        let first_of = |n: &str| offsets.iter().find(|(b, _)| b == n).map(|x| x.1);
        let next = |me: &mut Self| {
            me.step += 1;
            me.sub = 0;
            me.since = Some(Instant::now());
            me.waiting = None;
        };
        match self.step {
            // wait for every image, scroll to the first block
            0 => {
                if !all {
                    self.since = Some(now);
                    return;
                }
                if let Some((_, _)) = offsets.first() {
                    let open = snaps.blocks[0].open;
                    viewer.ed.view.scroll_to_item(open);
                    viewer.ed.view.scroll_by(-40.0);
                }
                if after(1500) {
                    self.original = text;
                    viewer.force_edit_button = Some(0);
                    next(self);
                }
            }
            1 => {
                if after(300) {
                    crate::block_snap::ProtoScript::default().shot(ctx, "forms-proto-hover");
                    // caret test (live mode): a caret put inside the first block from before it
                    // must end up after it
                    if let Some(b) = snaps.blocks.first() {
                        let off = b.offset + 4;
                        self.caret_test = Some((b.offset, b.end));
                        viewer.ed.set_selection(markise_doc::Selection::caret(off));
                    }
                    next(self);
                }
            }
            // opens: cold first, then warm, cycling through goals / question / scene
            2 => {
                let order: Vec<usize> = ["goals", "question", "scene", "note", "takeaway", "comparison"]
                    .iter()
                    .filter_map(|n| first_of(n))
                    .collect();
                if order.is_empty() {
                    self.step = 9;
                    return;
                }
                match self.waiting {
                    None if after(400) => {
                        viewer.force_edit_button = None;
                        if let Some((start, end)) = self.caret_test.take() {
                            let f = viewer.ed.session.selection().primary().focus.offset;
                            let out = f < start || f > end;
                            self.say(format!(
                                "caret put inside block {start}..{end} → now at {f}, outside the block = {out}"
                            ));
                        }
                        if self.sub > WARM_OPENS {
                            next(self);
                            return;
                        }
                        viewer.pending_edit = Some(order[self.sub % order.len()]);
                        self.waiting = Some((opens, 0));
                        self.since = Some(now);
                    }
                    Some((n, _)) if opens > n => {
                        if self.sub == 0 {
                            Self::shot_form(&snaps.form, "forms-proto-goals");
                        } else if self.sub == 1 {
                            Self::shot_form(&snaps.form, "forms-proto-question");
                            crate::block_snap::ProtoScript::default().shot(ctx, "forms-proto-window");
                        } else if self.sub < order.len() {
                            let name = offsets
                                .iter()
                                .find(|(_, o)| *o == order[self.sub])
                                .map_or("x", |x| x.0.as_str());
                            Self::shot_form(&snaps.form, &format!("forms-proto-{name}"));
                        }
                        self.waiting = Some((usize::MAX, 0));
                        self.since = Some(now);
                    }
                    Some((usize::MAX, _)) if after(if self.sub < 6 { 700 } else { 150 }) => {
                        viewer.snaps.as_mut().expect("snaps").form.close();
                        self.sub += 1;
                        self.waiting = None;
                        self.since = Some(now);
                    }
                    _ => {}
                }
            }
            // unchanged save of every block
            3 => match self.waiting {
                None if after(300) => {
                    if self.sub >= offsets.len() {
                        let same = text == self.original;
                        self.say(format!(
                            "unchanged save of all {} blocks: document byte-identical = {same}",
                            offsets.len()
                        ));
                        next(self);
                        return;
                    }
                    viewer.pending_edit = Some(offsets[self.sub].1);
                    self.waiting = Some((opens, saves));
                    self.since = Some(now);
                }
                Some((n, s)) if opens > n && saves == s && after(150) => {
                    snaps.form.eval("window.markiseForm.save()");
                    self.waiting = Some((usize::MAX, s));
                }
                Some((usize::MAX, s)) if saves > s => {
                    let r = snaps.form.saves.last().cloned();
                    let line = format!(
                        "unchanged save :::{} → {:?}, identical so far: {}",
                        offsets[self.sub].0,
                        r,
                        text == self.original
                    );
                    self.say(line);
                    self.sub += 1;
                    self.waiting = None;
                    self.since = Some(now);
                }
                _ => {}
            },
            // a real edit of the goals, then undo
            4 => match self.waiting {
                None if after(300) => {
                    let Some(off) = first_of("goals") else {
                        next(self);
                        return;
                    };
                    viewer.pending_edit = Some(off);
                    self.waiting = Some((opens, saves));
                }
                Some((n, s)) if opens > n && saves == s && after(200) => {
                    snaps.form.eval(
                        "{ const d = window.markiseForm.data(); window.markiseForm.set('items.0', d.items[0] + ' (**ред.**)'); setTimeout(() => window.markiseForm.save(), 80); }",
                    );
                    self.waiting = Some((usize::MAX, s));
                }
                Some((usize::MAX, s)) if saves > s => {
                    let r = snaps.form.saves.last().cloned();
                    let changed: Vec<String> = similar::TextDiff::from_lines(&self.original, &text)
                        .iter_all_changes()
                        .filter(|c| c.tag() != similar::ChangeTag::Equal)
                        .map(|c| {
                            format!(
                                "{}{}",
                                if c.tag() == similar::ChangeTag::Delete {
                                    "-"
                                } else {
                                    "+"
                                },
                                c.value().trim_end()
                            )
                        })
                        .collect();
                    self.say(format!("edit goals → {r:?}; diff: {changed:?}"));
                    self.waiting = Some((usize::MAX - 1, 0));
                    self.since = Some(now);
                }
                Some((m, _)) if m == usize::MAX - 1 && all && after(600) => {
                    crate::block_snap::ProtoScript::default().shot(ctx, "forms-proto-after-save");
                    self.waiting = Some((usize::MAX - 2, 0));
                    self.since = Some(now);
                }
                Some((m, _)) if m == usize::MAX - 2 && after(400) => {
                    let label = viewer
                        .ed
                        .session
                        .history()
                        .undo_stack()
                        .last()
                        .map(|t| t.label.to_string());
                    viewer.ed.edit(|s| s.undo());
                    let same = viewer.ed.session.text() == self.original;
                    self.say(format!("undo of {label:?} → document byte-identical = {same}"));
                    next(self);
                }
                _ => {}
            },
            // a save after the document changed is rejected
            5 => match self.waiting {
                None if after(500) => {
                    let Some(off) = first_of("question") else {
                        next(self);
                        return;
                    };
                    viewer.pending_edit = Some(off);
                    self.waiting = Some((opens, saves));
                }
                Some((n, s)) if opens > n && saves == s && after(200) => {
                    let end = viewer.ed.session.text().len();
                    let _ = viewer
                        .ed
                        .edit(|s| s.apply_external(&PatchSet::single_verbatim(end..end, "\nx"), "Typing", false));
                    snaps.form.eval(
                        "window.markiseForm.set('prompt', 'Изменённый вопрос?'); setTimeout(() => window.markiseForm.save(), 80);",
                    );
                    self.waiting = Some((usize::MAX, s));
                }
                Some((usize::MAX, s)) if saves > s => {
                    let r = snaps.form.saves.last().cloned();
                    self.say(format!("save after a document change → {r:?}"));
                    self.waiting = Some((usize::MAX - 1, 0));
                    self.since = Some(now);
                }
                Some((m, _)) if m == usize::MAX - 1 && after(500) => {
                    Self::shot_form(&snaps.form, "forms-proto-stale");
                    self.waiting = Some((usize::MAX - 2, 0));
                    self.since = Some(now);
                }
                Some((m, _)) if m == usize::MAX - 2 && after(500) => {
                    viewer.snaps.as_mut().expect("snaps").form.close();
                    viewer.ed.edit(|s| s.undo());
                    let same = viewer.ed.session.text() == self.original;
                    self.say(format!("typing undone → document byte-identical = {same}"));
                    next(self);
                }
                _ => {}
            },
            // answers: add one, move it first, mark it correct (one-answer question), save, undo
            6 => match self.waiting {
                None if after(400) => {
                    let Some(off) = first_of("question") else {
                        next(self);
                        return;
                    };
                    viewer.pending_edit = Some(off);
                    self.waiting = Some((opens, saves));
                }
                Some((n, s)) if opens > n && saves == s && after(200) => {
                    snaps.form.eval(
                        "{ const d = window.markiseForm.data(); const o = d.options.map(x => ({...x, correct: false})); o.unshift({id: 'a9', text: 'Новый **верный** ответ', correct: true}); window.markiseForm.set('options', o); setTimeout(() => window.markiseForm.save(), 80); }",
                    );
                    self.waiting = Some((usize::MAX, s));
                }
                Some((usize::MAX, s)) if saves > s => {
                    let r = snaps.form.saves.last().cloned();
                    let changed: Vec<String> = similar::TextDiff::from_lines(&self.original, &text)
                        .iter_all_changes()
                        .filter(|c| c.tag() != similar::ChangeTag::Equal)
                        .map(|c| {
                            format!(
                                "{}{}",
                                if c.tag() == similar::ChangeTag::Delete {
                                    "-"
                                } else {
                                    "+"
                                },
                                c.value().trim_end()
                            )
                        })
                        .collect();
                    self.say(format!(
                        "answers added/reordered/correctness → {r:?}; diff: {changed:?}"
                    ));
                    self.waiting = Some((usize::MAX - 1, 0));
                    self.since = Some(now);
                }
                Some((m, _)) if m == usize::MAX - 1 && all && after(600) => {
                    crate::block_snap::ProtoScript::default().shot(ctx, "forms-proto-answers-saved");
                    self.waiting = Some((usize::MAX - 2, 0));
                    self.since = Some(now);
                }
                Some((m, _)) if m == usize::MAX - 2 && after(400) => {
                    viewer.ed.edit(|s| s.undo());
                    let same = viewer.ed.session.text() == self.original;
                    self.say(format!("undo → document byte-identical = {same}"));
                    next(self);
                }
                _ => {}
            },
            7 => {
                if after(400) {
                    let opens = &snaps.form.opens;
                    let cold: Vec<String> = opens
                        .iter()
                        .filter(|o| o.0)
                        .map(|o| format!("{:.1} ms (helper {:.1})", o.1, o.2))
                        .collect();
                    let mut warm: Vec<f64> = opens.iter().filter(|o| !o.0).map(|o| o.1).collect();
                    warm.sort_by(|a, b| a.total_cmp(b));
                    let p = |q: f64| {
                        warm.get(((warm.len().max(1) - 1) as f64 * q).round() as usize)
                            .copied()
                            .unwrap_or(f64::NAN)
                    };
                    let line = format!(
                        "dialog open, click → form painted: cold {cold:?}; warm n={} p50={:.1} p90={:.1} max={:.1} ms",
                        warm.len(),
                        p(0.5),
                        p(0.9),
                        p(1.0)
                    );
                    self.say(line);
                    let final_same = viewer.ed.session.text() == self.original;
                    self.say(format!(
                        "document at the end byte-identical to the start = {final_same}"
                    ));
                    next(self);
                }
            }
            8 => {
                if after(800) && std::env::var_os("MARKISE_PROTO_EXIT").is_some_and(|v| v != "0") {
                    std::process::exit(0);
                }
            }
            _ => {}
        }
    }
}

thread_local! {
    static LIVE_SCRIPT: std::cell::RefCell<Option<FormScript>> = const { std::cell::RefCell::new(None) };
}

/// Runs the measurement script in the normal (live) editing mode.
pub fn live_script(ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
    if !FormScript::enabled() {
        return;
    }
    LIVE_SCRIPT.with(|s| s.borrow_mut().get_or_insert_with(Default::default).frame(ctx, viewer));
}

/// `file://` URL of a folder (with a trailing slash), percent-encoded.
fn file_url_dir(dir: &Path) -> String {
    let mut out = String::from("file://");
    for b in dir.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out.push('/');
    out
}

/// `to` relative to the folder `from` (`../../styles/a.png`), with `/` separators.
fn relative_path(from: &Path, to: &Path) -> String {
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), a.len() - common).collect();
    parts.extend(b[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

/// `contracts/<lesson>.json` above the document (lesson id from a `Lesson ID:` line or the
/// file name), shown relative to the document's folder.
fn contract_of(doc: &Path, text: &str) -> Option<String> {
    let id = text
        .lines()
        .take(40)
        .find_map(|l| l.strip_prefix("Lesson ID:").map(|v| v.trim().to_string()))
        .or_else(|| doc.file_stem().map(|s| s.to_string_lossy().to_string()))?;
    let mut dir = doc.parent();
    while let Some(d) = dir {
        let c = d.join("contracts").join(format!("{id}.json"));
        if c.is_file() {
            return Some(relative_path(doc.parent()?, &c));
        }
        dir = d.parent();
    }
    None
}

// ---------------------------------------------------------------- block packages script

/// PROTOTYPE scripted check of the block-package round (`MARKISE_PROTO_INSERT=1`, live mode):
/// palette, draft insertion with validation, undo, Backspace on a block, `/` on an empty line.
#[derive(Default)]
pub struct InsertScript {
    step: usize,
    since: Option<Instant>,
    original: String,
    saves: usize,
    mark: usize,
}

thread_local! {
    static INSERT_SCRIPT: std::cell::RefCell<Option<InsertScript>> = const { std::cell::RefCell::new(None) };
}

pub fn insert_script(ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
    if !std::env::var_os("MARKISE_PROTO_INSERT").is_some_and(|v| v != "0") {
        return;
    }
    INSERT_SCRIPT.with(|s| s.borrow_mut().get_or_insert_with(Default::default).frame(ctx, viewer));
}

impl InsertScript {
    fn say(s: String) {
        eprintln!("[proto-insert] {s}");
    }

    fn key(ctx: &egui::Context, key: egui::Key) {
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            })
        });
    }

    fn frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        let after = |ms: u64| now - since >= std::time::Duration::from_millis(ms);
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let all = !snaps.current.is_empty() && snaps.current.iter().all(|&c| c);
        let shot = |name: &str| crate::block_snap::ProtoScript::default().shot(ctx, name);
        let panel_shot = |name: &str| {
            if let Some(dir) = std::env::var_os("MARKISE_PROTO_SHOTS") {
                crate::form_panel::handle(
                    json!({"op": "form_shot", "path": PathBuf::from(dir).join(format!("{name}.png"))}),
                );
            }
        };
        let text = viewer.ed.session.text().to_string();
        let next = |me: &mut Self| {
            me.step += 1;
            me.since = Some(Instant::now());
        };
        match self.step {
            0 => {
                if all && after(1200) && !viewer.gaps.is_empty() {
                    self.original = text;
                    shot("pkg-01-lesson");
                    crate::block_palette::toggle();
                    next(self);
                } else if !all {
                    self.since = Some(now);
                }
            }
            1 if after(2500) => {
                shot("pkg-02-palette");
                next(self);
            }
            2 if after(400) => {
                // "drop" the three-answer question after the first block image
                let root = snaps.form.root().map(Path::to_path_buf);
                let presets = root.map(|r| crate::block_lib::presets(&r, &lang())).unwrap_or_default();
                let p = presets.iter().find(|p| p.block == "question").cloned();
                let gap = viewer.atomic.first().map(|a| a.1).unwrap_or(0);
                let rev = viewer.ed.session.revision().counter;
                if let Some(p) = p {
                    Self::say(format!("dropping preset «{}» at offset {gap}", p.label));
                    crate::block_palette::choose(p, Some(InsertTarget::new(&text, gap, gap, rev)));
                }
                self.saves = snaps.form.saves.len();
                next(self);
            }
            3 if after(900) && snaps.form.is_open() => {
                panel_shot("pkg-03-draft");
                snaps.form.eval("window.markiseForm.save()");
                next(self);
            }
            4 if snaps.form.saves.len() > self.saves => {
                Self::say(format!("save without a correct answer → {:?}", snaps.form.saves.last()));
                self.saves = snaps.form.saves.len();
                next(self);
            }
            5 if after(500) => {
                panel_shot("pkg-04-validation");
                snaps.form.eval("{ const d = window.markiseForm.data(); const o = d.options.map((x, i) => ({...x, correct: i === 1})); window.markiseForm.set('options', o); window.markiseForm.set('prompt', 'Новый вопрос из палитры?'); }");
                self.mark = 0;
                self.since = Some(now);
                self.step = 50;
            }
            // live preview of the draft at its insertion point (nothing written yet)
            50 if self.mark == 0 => {
                if let Some(a) = viewer.atomic.first() {
                    let open = viewer
                        .ed
                        .view
                        .model()
                        .items
                        .iter_with_start()
                        .position(|(st, _)| st == a.0)
                        .unwrap_or(0);
                    viewer.ed.view.scroll_to_item(open);
                    viewer.ed.view.scroll_by(200.0);
                }
                self.mark = 1;
            }
            50 if after(1200) => {
                let p = snaps.form.preview.clone();
                Self::say(format!(
                    "live preview of the draft: shown = {}, document unchanged = {}, preview has the new prompt = {}",
                    snaps.draft_image.is_some(),
                    text == self.original,
                    p.is_some_and(|p| p.markdown.contains("Новый вопрос из палитры?"))
                ));
                shot("pkg-08-live-preview");
                self.step = 51;
                self.since = Some(now);
            }
            51 if after(500) => {
                snaps.form.eval("window.markiseForm.save()");
                self.step = 6;
                self.since = Some(now);
            }
            6 if snaps.form.saves.len() > self.saves => {
                let added: Vec<String> = similar::TextDiff::from_lines(&self.original, &text)
                    .iter_all_changes()
                    .filter(|c| c.tag() == similar::ChangeTag::Insert)
                    .map(|c| c.value().trim_end().to_string())
                    .collect();
                Self::say(format!(
                    "insert → {:?}; inserted lines: {added:?}",
                    snaps.form.saves.last()
                ));
                next(self);
            }
            7 if all && after(900) => {
                shot("pkg-05-inserted");
                next(self);
            }
            8 if after(500) => {
                let label = viewer
                    .ed
                    .session
                    .history()
                    .undo_stack()
                    .last()
                    .map(|t| t.label.to_string());
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo {label:?} → document byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                crate::block_palette::close();
                next(self);
            }
            // Backspace right after a block image: first selects it, the second deletes it
            9 if after(500) => {
                let pick = viewer
                    .atomic
                    .iter()
                    .find(|a| !viewer.atomic.iter().any(|b| b.0 == a.3))
                    .copied();
                if let Some((start, _, _, after_off)) = pick {
                    viewer.ed.set_selection(markise_doc::Selection::caret(after_off));
                    viewer.request_refocus();
                    self.mark = start;
                }
                next(self);
            }
            10 if after(300) => {
                Self::key(ctx, egui::Key::Backspace);
                next(self);
            }
            11 if after(300) => {
                Self::say(format!(
                    "Backspace after the block → block selected = {}",
                    viewer.focused_block == Some(self.mark)
                ));
                shot("pkg-06-block-selected");
                next(self);
            }
            12 if after(400) => {
                Self::key(ctx, egui::Key::Backspace);
                next(self);
            }
            13 if after(400) => {
                let before = bf::all_blocks(&self.original).len();
                let now_blocks = bf::all_blocks(&text);
                Self::say(format!(
                    "second Backspace → one whole block deleted = {}; all remaining `:::` lines paired = {}",
                    now_blocks.len() + 1 == before,
                    text.lines().filter(|l| l.starts_with(":::")).count() == now_blocks.len() * 2
                ));
                let label = viewer
                    .ed
                    .session
                    .history()
                    .undo_stack()
                    .last()
                    .map(|t| t.label.to_string());
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo {label:?} → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            // `/` on an empty line opens the palette for that line
            14 if after(500) => {
                let end = self.original.find("\n\n## ").unwrap_or(0);
                viewer.ed.set_selection(markise_doc::Selection::caret(end));
                viewer.request_refocus();
                next(self);
            }
            15 if after(300) => {
                Self::key(ctx, egui::Key::Enter);
                next(self);
            }
            16 if after(300) => {
                ctx.input_mut(|i| i.events.push(egui::Event::Text("/".into())));
                next(self);
            }
            17 if after(500) => {
                let t = crate::block_palette::target();
                Self::say(format!(
                    "`/` on an empty line → palette open = {}, target replaces the `/` line = {}",
                    crate::block_palette::is_open(),
                    t.as_ref().is_some_and(|t| t.replaced == "/")
                ));
                shot("pkg-07-slash");
                next(self);
            }
            18 if after(500) => {
                crate::block_palette::close();
                while viewer.ed.session.text() != self.original && viewer.ed.edit(|s| s.undo()) {}
                Self::say(format!(
                    "typing undone → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            // live preview while editing an existing block; Cancel restores it
            19 if after(500) => {
                if let Some(b) = snaps.blocks.iter().find(|b| b.name == "goals") {
                    viewer.pending_edit = Some(b.offset);
                    self.mark = b.offset;
                }
                next(self);
            }
            20 if after(900) && snaps.form.is_open() => {
                snaps.form.eval("{ const d = window.markiseForm.data(); window.markiseForm.set('items.0', 'Живой предпросмотр: ' + d.items[0]); }");
                next(self);
            }
            21 if after(1200) => {
                let shown = snaps
                    .blocks
                    .iter()
                    .find(|b| b.offset == self.mark)
                    .is_some_and(|b| b.inner.contains("Живой предпросмотр"));
                Self::say(format!(
                    "edit preview: block image shows the form values = {shown} (current = {:?}), document unchanged = {}, undo stack unchanged = {}",
                    snaps.current.iter().all(|c| *c),
                    text == self.original,
                    viewer
                        .ed
                        .session
                        .history()
                        .undo_stack()
                        .last()
                        .map(|t| t.label.to_string())
                        != Some("Edit Block".into())
                ));
                shot("pkg-09-edit-preview");
                next(self);
            }
            22 if after(500) => {
                crate::form_panel::handle(
                    json!({"op": "form_eval", "js": "window.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'}))"}),
                );
                next(self);
            }
            23 if after(800) => {
                let back = snaps
                    .blocks
                    .iter()
                    .find(|b| b.offset == self.mark)
                    .is_some_and(|b| !b.inner.contains("Живой предпросмотр"));
                Self::say(format!(
                    "cancel → original block restored = {back}, form closed = {}, document unchanged = {}",
                    !snaps.form.is_open(),
                    text == self.original
                ));
                next(self);
            }
            // the palette (list, then a keyboard-highlighted row with its preview)
            24 if after(500) => {
                crate::block_palette::open_for(None);
                next(self);
            }
            25 if after(2500) => {
                shot("pal-list");
                crate::block_palette::script_highlight(Some(0));
                next(self);
            }
            26 if after(1500) => {
                shot("pal-preview");
                next(self);
            }
            27 if after(400) => {
                crate::block_palette::close();
                // ⌥⌘↓ on a selected block moves it below the next item
                let pick = viewer
                    .atomic
                    .iter()
                    .find(|a| !viewer.atomic.iter().any(|b| b.0 == a.3))
                    .copied();
                if let Some((start, _, _, after_off)) = pick {
                    viewer.focused_block = Some(start);
                    viewer.ed.set_selection(markise_doc::Selection::caret(after_off));
                    viewer.request_refocus();
                    self.mark = start;
                }
                next(self);
            }
            28 if after(300) => {
                ctx.input_mut(|i| {
                    i.events.push(egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers {
                            alt: true,
                            command: true,
                            mac_cmd: true,
                            ..Default::default()
                        },
                    })
                });
                next(self);
            }
            29 if after(400) => {
                let moved = text != self.original;
                let label = viewer
                    .ed
                    .session
                    .history()
                    .undo_stack()
                    .last()
                    .map(|t| t.label.to_string());
                let blocks_ok = bf::all_blocks(&text).len() == bf::all_blocks(&self.original).len();
                Self::say(format!(
                    "⌥⌘↓ on a selected block → moved = {moved}, undo label {label:?}, blocks intact = {blocks_ok}"
                ));
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            // drag a block by its handle: the line at a valid point, then the drop
            30 if after(500) => {
                self.saves = 0;
                let pair = viewer.atomic.iter().copied().find_map(|a| {
                    viewer
                        .gaps
                        .iter()
                        .copied()
                        .find(|g| g.0 != a.1 && g.0 != a.2 && g.1 > 120.0)
                        .map(|g| (a, g))
                });
                if let Some((a, g)) = pair {
                    egui::DragAndDrop::set_payload(ctx, crate::viewer::MoveBlock(a.0));
                    viewer.debug_drag_y = Some(g.1 - 40.0 + 2.0);
                    self.mark = a.0;
                    self.saves = g.0;
                    Self::say(format!("dragging block at {} towards the point {}", a.0, g.0));
                }
                next(self);
            }
            31 if after(600) => {
                shot("move-drag-line");
                next(self);
            }
            32 if after(400) => {
                egui::DragAndDrop::clear_payload(ctx);
                viewer.debug_drag_y = None;
                let r = if self.saves > 10 {
                    viewer.move_block(self.mark, self.saves)
                } else {
                    None
                };
                let t2 = viewer.ed.session.text().to_string();
                Self::say(format!(
                    "drop → moved to {r:?}; blocks intact = {}; undo label {:?}",
                    bf::all_blocks(&t2).len() == bf::all_blocks(&self.original).len(),
                    viewer
                        .ed
                        .session
                        .history()
                        .undo_stack()
                        .last()
                        .map(|t| t.label.to_string())
                ));
                next(self);
            }
            33 if all && after(900) => {
                shot("move-dropped");
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo the move → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            34 if after(800) => {
                if std::env::var_os("MARKISE_PROTO_EXIT").is_some_and(|v| v != "0") {
                    std::process::exit(0);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- directions script

/// PROTOTYPE scripted check of the «О направлениях» preset (`MARKISE_PROTO_DIRECTIONS=1`).
#[derive(Default)]
pub struct DirScript {
    step: usize,
    since: Option<Instant>,
    original: String,
    saves: usize,
}

thread_local! {
    static DIR_SCRIPT: std::cell::RefCell<Option<DirScript>> = const { std::cell::RefCell::new(None) };
}

pub fn dir_script(ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
    if !std::env::var_os("MARKISE_PROTO_DIRECTIONS").is_some_and(|v| v != "0") {
        return;
    }
    DIR_SCRIPT.with(|s| s.borrow_mut().get_or_insert_with(Default::default).frame(ctx, viewer));
}

impl DirScript {
    fn frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        let after = |ms: u64| now - since >= std::time::Duration::from_millis(ms);
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let all = !snaps.current.is_empty() && snaps.current.iter().all(|&c| c);
        let shot = |name: &str| crate::block_snap::ProtoScript::default().shot(ctx, name);
        let panel_shot = |name: &str| {
            if let Some(dir) = std::env::var_os("MARKISE_PROTO_SHOTS") {
                crate::form_panel::handle(
                    json!({"op": "form_shot", "path": PathBuf::from(dir).join(format!("{name}.png"))}),
                );
            }
        };
        let say = |s: String| eprintln!("[proto-dir] {s}");
        let text = viewer.ed.session.text().to_string();
        let next = |me: &mut Self| {
            me.step += 1;
            me.since = Some(Instant::now());
        };
        match self.step {
            0 => {
                if all && after(1200) && !viewer.gaps.is_empty() {
                    self.original = text;
                    crate::block_palette::open_for(None);
                    next(self);
                } else if !all {
                    self.since = Some(now);
                }
            }
            1 if after(2500) => {
                shot("dir-palette");
                self.step = 100;
                self.since = Some(now);
            }
            100 if after(600) => {
                self.step = 1;
                let root = snaps.form.root().map(Path::to_path_buf);
                let p = root
                    .map(|r| crate::block_lib::presets(&r, &lang()))
                    .unwrap_or_default()
                    .into_iter()
                    .find(|p| p.widget.is_some());
                let gap = viewer.gaps.first().map(|g| g.0).unwrap_or(0);
                let rev = viewer.ed.session.revision().counter;
                if let Some(p) = p {
                    say(format!("inserting «{}» at {gap}", p.label));
                    crate::block_palette::choose(p, Some(InsertTarget::new(&text, gap, gap, rev)));
                }
                self.saves = snaps.form.saves.len();
                next(self);
            }
            2 if after(1200) && snaps.form.is_open() => {
                panel_shot("dir-form-draft");
                snaps.form.eval("window.markiseForm.save()");
                next(self);
            }
            3 if snaps.form.saves.len() > self.saves => {
                say(format!("save with no direction → {:?}", snaps.form.saves.last()));
                self.saves = snaps.form.saves.len();
                // pick «Прямо» and «Направо» like the widget does (and «Ехать нельзя» stays off)
                snaps.form.eval("{ const d = window.markiseForm.data(); window.markiseForm.set('options', d.options.map((x, i) => ({...x, correct: i === 1 || i === 2}))); }");
                next(self);
            }
            4 if after(1200) => {
                panel_shot("dir-form-selected");
                shot("dir-preview");
                snaps.form.eval("window.markiseForm.save()");
                next(self);
            }
            5 if snaps.form.saves.len() > self.saves => {
                let added: Vec<String> = similar::TextDiff::from_lines(&self.original, &text)
                    .iter_all_changes()
                    .filter(|c| c.tag() == similar::ChangeTag::Insert)
                    .map(|c| c.value().trim_end().to_string())
                    .collect();
                say(format!("insert → {:?}; lines {added:?}", snaps.form.saves.last()));
                viewer.ed.edit(|s| s.undo());
                say(format!(
                    "undo → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            // an existing `:::question directions` opens the direction form
            6 if after(800) => {
                if let Some(b) = snaps
                    .blocks
                    .iter()
                    .find(|b| b.name == "question" && b.variant == "directions")
                {
                    viewer.pending_edit = Some(b.offset);
                    say(format!("editing the directions question at {}", b.offset));
                }
                next(self);
            }
            7 if after(1500) && snaps.form.is_open() => {
                panel_shot("dir-form-edit");
                snaps
                    .form
                    .eval("window.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'}))");
                next(self);
            }
            8 if after(800) => {
                say(format!(
                    "cancel → document unchanged = {}",
                    viewer.ed.session.text() == self.original
                ));
                if std::env::var_os("MARKISE_PROTO_EXIT").is_some_and(|v| v != "0") {
                    std::process::exit(0);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- paged model check

/// PROTOTYPE measurement driver (`MARKISE_PROTO_PAGED=1`, on a copy of a lesson): blocks over
/// the paged view model (ADR 0016). Scrolls through the document and back, jumps, edits, moves
/// and walks the caret across pages, and reports what happened. Pair with `MARKISE_PAGE_TOPS=n`.
#[derive(Default)]
struct PagedScript {
    step: u32,
    since: Option<Instant>,
    original: String,
    last: std::collections::HashMap<(usize, usize), f32>,
    jump: f32,
    scrolled: bool,
    total: f64,
    total_first: f64,
    max_blocks: usize,
    moves: u32,
    target: usize,
    saves: usize,
    prev_off: usize,
    stuck: u32,
    max_stuck: u32,
    inside: u32,
    virt: u32,
    furthest: usize,
    partial_seen: bool,
}

thread_local! {
    static PAGED_SCRIPT: std::cell::RefCell<Option<PagedScript>> = const { std::cell::RefCell::new(None) };
}

pub fn paged_script(ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
    if !std::env::var_os("MARKISE_PROTO_PAGED").is_some_and(|v| v != "0") {
        return;
    }
    PAGED_SCRIPT.with(|s| s.borrow_mut().get_or_insert_with(Default::default).frame(ctx, viewer));
}

impl PagedScript {
    fn say(s: String) {
        eprintln!("[proto-paged] {s}");
    }

    fn key(ctx: &egui::Context, key: egui::Key) {
        ctx.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            })
        });
    }

    fn stats(viewer: &mut crate::viewer::DocViewer) -> String {
        let st = viewer.ed.view.paging_stats().clone();
        let n = viewer.ed.view.len();
        let pages = (0..n).filter(|&i| viewer.ed.view.model().is_page(i)).count();
        format!(
            "items {n} (placeholders {pages}), materialized {}, evicted {}, total height {:.0}, paged {}",
            st.materialized,
            st.evicted,
            viewer.ed.view.total_height(),
            viewer.ed.view.model().is_paged()
        )
    }

    /// On-screen movement of the items that stayed visible since the last frame, when nothing
    /// scrolled.
    fn track(&mut self, viewer: &crate::viewer::DocViewer) {
        let now: std::collections::HashMap<(usize, usize), f32> = viewer.painted().iter().copied().collect();
        if !self.scrolled {
            for (k, y) in &now {
                if let Some(o) = self.last.get(k) {
                    self.jump = self.jump.max((y - o).abs());
                }
            }
        }
        self.scrolled = false;
        self.last = now;
    }

    fn frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
        use markise_engine::block_form as bf;
        ctx.request_repaint();
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        let after = |ms: u64| now - since >= std::time::Duration::from_millis(ms);
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let pending = snaps.pending();
        let scanned = snaps.blocks.len();
        self.max_blocks = self.max_blocks.max(scanned);
        self.partial_seen |= viewer.partial;
        let form_open = snaps.form.is_open();
        let n_saves = snaps.form.saves.len();
        let vh = viewer.surface_rect().map_or(800.0, |r| r.height());
        let next = |me: &mut Self| {
            me.step += 1;
            me.since = Some(Instant::now());
            me.jump = 0.0;
            me.last.clear();
        };
        let at_end = viewer.ed.view.scroll_y() + vh as f64 >= viewer.ed.view.total_height() - 2.0;
        let at_top = viewer.ed.view.scroll_y() <= 1.0;
        match self.step {
            0 => {
                let ready = !viewer.partial && scanned > 0 && pending == 0;
                if ready && after(1500) {
                    self.original = viewer.ed.session.text().to_string();
                    let all = bf::all_blocks(&self.original).len();
                    Self::say(format!(
                        "start: blocks in the text {all}, scanned (materialized) {scanned}, progressive part seen {}; {}",
                        self.partial_seen,
                        Self::stats(viewer)
                    ));
                    next(self);
                } else if !ready {
                    self.since = Some(now);
                }
            }
            // down: images arrive as pages come in
            1 => {
                self.track(viewer);
                if after(5000) {
                    Self::say(format!(
                        "pass down waiting: pending {pending}, moves {}, scroll {:.0} of {:.0}, vh {vh}, {}",
                        self.moves,
                        viewer.ed.view.scroll_y(),
                        viewer.ed.view.total_height(),
                        Self::stats(viewer)
                    ));
                    self.since = Some(now);
                }
                if pending == 0 && after(60) {
                    if at_end {
                        self.total_first = viewer.ed.view.total_height();
                        Self::say(format!(
                            "pass down done ({} moves): at most {} blocks scanned at once; on-screen movement (images arriving) {:.1}; {}",
                            self.moves,
                            self.max_blocks,
                            self.jump,
                            Self::stats(viewer)
                        ));
                        self.moves = 0;
                        next(self);
                    } else {
                        viewer.ed.view.scroll_by(vh * 0.8);
                        self.scrolled = true;
                        self.moves += 1;
                        self.since = Some(now);
                    }
                }
            }
            // up: heights known, pages come back
            2 => {
                self.track(viewer);
                if pending == 0 && after(60) {
                    if at_top {
                        self.total = viewer.ed.view.total_height();
                        Self::say(format!(
                            "pass up done ({} moves): on-screen movement without scrolling {:.1}; total height {:.0} → {:.0}; {}",
                            self.moves,
                            self.jump,
                            self.total_first,
                            self.total,
                            Self::stats(viewer)
                        ));
                        self.moves = 0;
                        next(self);
                    } else {
                        viewer.ed.view.scroll_by(-vh * 0.8);
                        self.scrolled = true;
                        self.moves += 1;
                        self.since = Some(now);
                    }
                }
            }
            // a jump to a block far away
            3 => {
                let text = viewer.ed.session.text();
                let blocks = bf::all_blocks(text);
                let q: Vec<usize> = blocks
                    .iter()
                    .filter(|b| text[b.range.clone()].starts_with(":::question"))
                    .map(|b| b.range.start)
                    .collect();
                let off = q[q.len() * 2 / 3];
                self.target = off;
                viewer.ed.view.scroll_to_source(off, 0, 0.0);
                Self::say(format!("jump to the block at {off}"));
                next(self);
            }
            4 if pending == 0 && after(500) => {
                let t = self.target;
                let found = snaps.blocks.iter().any(|b| b.offset == t);
                let placed = viewer.atomic.iter().any(|a| a.0 == t);
                let y = viewer.painted().iter().find(|k| k.0.0 == t).map(|k| k.1);
                Self::say(format!(
                    "after the jump: scanned {found}, placed {placed}, top of the block on screen at {y:?}; {}",
                    Self::stats(viewer)
                ));
                viewer.pending_edit = Some(t);
                next(self);
            }
            5 if form_open && after(900) => {
                let dir = std::env::var_os("MARKISE_PROTO_SHOTS");
                crate::form_panel::handle(json!({
                    "op": "form_debug",
                    "path": dir.as_ref().map(|d| PathBuf::from(d).join("paged-window.png")),
                }));
                if let Some(d) = dir {
                    crate::form_panel::handle(
                        json!({"op": "form_shot", "path": PathBuf::from(d).join("paged-form.png")}),
                    );
                }
                crate::block_snap::ProtoScript::default().shot(ctx, "paged-form-egui");
                next(self);
            }
            // the block's page goes away while its form is open
            6 if after(800) => {
                viewer.ed.view.scroll_to_source(0, 0, 0.0);
                next(self);
            }
            7 if after(800) => {
                let t = self.target;
                let there = snaps.blocks.iter().any(|b| b.offset == t);
                Self::say(format!(
                    "scrolled away with the form open: block still materialized {there}; panel open {form_open}"
                ));
                self.saves = n_saves;
                snaps
                    .form
                    .eval("window.markiseForm.set('prompt', 'Правка через форму?')");
                next(self);
            }
            8 if after(400) => {
                snaps.form.eval("window.markiseForm.save()");
                next(self);
            }
            9 if n_saves > self.saves => {
                let text = viewer.ed.session.text().to_string();
                let changed: Vec<String> = similar::TextDiff::from_lines(&self.original, &text)
                    .iter_all_changes()
                    .filter(|c| c.tag() != similar::ChangeTag::Equal)
                    .map(|c| c.value().trim_end().to_string())
                    .collect();
                Self::say(format!(
                    "save → {:?}; changed lines {changed:?}; undo label {:?}",
                    snaps.form.saves.last(),
                    viewer
                        .ed
                        .session
                        .history()
                        .undo_stack()
                        .last()
                        .map(|t| t.label.to_string())
                ));
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            // the caret across images, pages and gaps
            10 if after(300) => {
                let text = viewer.ed.session.text();
                let first = bf::all_blocks(text).first().map_or(0, |b| b.range.start);
                let start = first.saturating_sub(1);
                viewer.ed.set_selection(markise_doc::Selection::caret(start));
                viewer.request_refocus();
                self.prev_off = start;
                self.furthest = start;
                self.moves = 0;
                next(self);
            }
            11 | 12 if after(300) => {
                let down = self.step == 11;
                let f = viewer.ed.session.selection().primary().focus;
                let inside = viewer
                    .atomic
                    .iter()
                    .any(|a| f.offset > a.0 && f.offset < a.1 && viewer.focused_block != Some(a.0));
                self.inside += inside as u32;
                self.virt += (f.virt > 0) as u32;
                let progress = if down {
                    f.offset > self.prev_off
                } else {
                    f.offset < self.prev_off
                };
                self.stuck = if progress { 0 } else { self.stuck + 1 };
                self.max_stuck = self.max_stuck.max(self.stuck);
                if self.stuck == 20 {
                    let i = viewer.ed.view.model().item_at_offset(f.offset);
                    for j in i.saturating_sub(6)..(i + 5).min(viewer.ed.view.len()) {
                        let m = viewer.ed.view.model();
                        Self::say(format!(
                            "  stall at {} virt {}: item {j} start {} len {} page {} h {:.1}{}",
                            f.offset,
                            f.virt,
                            m.item_start(j),
                            m.items[j].len,
                            m.is_page(j),
                            viewer.ed.view.height(j),
                            if j == i { " <" } else { "" }
                        ));
                    }
                    Self::say(format!(
                        "  atomic near: {:?}",
                        viewer
                            .atomic
                            .iter()
                            .filter(|a| a.1 + 400 > f.offset && a.0 < f.offset + 400)
                            .collect::<Vec<_>>()
                    ));
                }
                self.prev_off = f.offset;
                self.furthest = if down {
                    self.furthest.max(f.offset)
                } else {
                    self.furthest.min(f.offset)
                };
                self.moves += 1;
                if self.moves >= 400 || self.stuck > 30 {
                    Self::say(format!(
                        "caret {}: {} key presses, reached offset {} (text {}), inside an image {} times, on a gap paragraph {} frames, longest stall {} frames; text unchanged {}",
                        if down { "down" } else { "up" },
                        self.moves,
                        self.furthest,
                        self.original.len(),
                        self.inside,
                        self.virt,
                        self.max_stuck,
                        viewer.ed.session.text() == self.original
                    ));
                    self.moves = 0;
                    self.stuck = 0;
                    self.max_stuck = 0;
                    next(self);
                } else {
                    viewer.request_refocus();
                    Self::key(ctx, if down { egui::Key::ArrowDown } else { egui::Key::ArrowUp });
                }
            }
            // moving a block, by keys' code path
            13 if after(300) => {
                let t = self.target;
                viewer.ed.view.scroll_to_source(t, 0, 0.0);
                viewer.ed.view.ensure_offset(t);
                let r = viewer.move_block_step(t, false);
                let text = viewer.ed.session.text().to_string();
                Self::say(format!(
                    "move a block down → {r:?}; blocks intact {}; undo label {:?}",
                    bf::all_blocks(&text).len() == bf::all_blocks(&self.original).len(),
                    viewer
                        .ed
                        .session
                        .history()
                        .undo_stack()
                        .last()
                        .map(|t| t.label.to_string())
                ));
                next(self);
            }
            14 if after(500) => {
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            // read-only session
            15 if pending == 0 && after(500) => {
                viewer.ed.session.set_read_only(Some("read-only (script)"));
                viewer.pending_edit = Some(self.target);
                next(self);
            }
            16 if after(400) => {
                let err = snaps.form.error.as_ref().map(|e| e.0.clone());
                viewer.ed.session.set_read_only(Some("read-only (script)"));
                let r = viewer.move_block_step(self.target, false);
                Self::say(format!(
                    "read-only: form opened {form_open}, error {err:?}, move → {r:?}, text unchanged {}",
                    viewer.ed.session.text() == self.original
                ));
                viewer.ed.session.set_read_only(None);
                next(self);
            }
            // a block dragged to an insertion point (the line and the drop's edit; the real
            // pointer press and release are the owner's to try)
            17 if after(120) => {
                let pair = viewer.atomic.iter().copied().find_map(|a| {
                    viewer
                        .gaps
                        .iter()
                        .copied()
                        .find(|g| g.0 != a.1 && g.0 != a.2 && g.0 > a.3)
                        .map(|g| (a, g))
                });
                match pair {
                    Some((a, g)) if !at_end => {
                        egui::DragAndDrop::set_payload(ctx, crate::viewer::MoveBlock(a.0));
                        let top = viewer.surface_rect().map_or(0.0, |r| r.top());
                        viewer.debug_drag_y = Some(g.1 - top + 2.0);
                        self.target = a.0;
                        self.saves = g.0;
                        next(self);
                    }
                    _ if at_end => {
                        Self::say("no block with an insertion point on one screen".to_string());
                        self.step = 21;
                    }
                    _ => {
                        viewer.ed.view.scroll_by(vh * 0.5);
                        self.since = Some(now);
                    }
                }
            }
            18 if after(500) => {
                crate::block_snap::ProtoScript::default().shot(ctx, "paged-drag-line");
                next(self);
            }
            19 if after(300) => {
                egui::DragAndDrop::clear_payload(ctx);
                viewer.debug_drag_y = None;
                let r = viewer.move_block(self.target, self.saves);
                let text = viewer.ed.session.text().to_string();
                Self::say(format!(
                    "drop at {} → {r:?}; blocks intact {}; undo label {:?}",
                    self.saves,
                    bf::all_blocks(&text).len() == bf::all_blocks(&self.original).len(),
                    viewer
                        .ed
                        .session
                        .history()
                        .undo_stack()
                        .last()
                        .map(|t| t.label.to_string())
                ));
                next(self);
            }
            20 if after(500) => {
                viewer.ed.edit(|s| s.undo());
                Self::say(format!(
                    "undo → byte-identical = {}",
                    viewer.ed.session.text() == self.original
                ));
                next(self);
            }
            21 if after(300) => {
                Self::say(format!("done; {}", Self::stats(viewer)));
                if std::env::var_os("MARKISE_PROTO_EXIT").is_some_and(|v| v != "0") {
                    std::process::exit(0);
                }
                self.step = 99;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- native blocks check

thread_local! {
    static RAW_QUEUE: std::cell::RefCell<std::collections::VecDeque<Vec<egui::Event>>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
    static NATIVE_SCRIPT: std::cell::RefCell<Option<NativeScript>> = const { std::cell::RefCell::new(None) };
}

/// PROTOTYPE: scripted input for the next frames (`eframe::App::raw_input_hook`).
pub fn proto_raw_input(raw: &mut egui::RawInput) {
    if let Some(batch) = RAW_QUEUE.with(|q| q.borrow_mut().pop_front()) {
        raw.events.extend(batch);
    }
}

fn q_click(p: egui::Pos2) {
    use egui::{Event, Modifiers, PointerButton};
    let btn = |pressed| Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    RAW_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.push_back(vec![Event::PointerMoved(p)]);
        q.push_back(vec![]);
        q.push_back(vec![btn(true)]);
        q.push_back(vec![btn(false)]);
        q.push_back(vec![]);
    });
}

fn q_events(ev: Vec<egui::Event>) {
    RAW_QUEUE.with(|q| q.borrow_mut().push_back(ev));
}

fn q_key(key: egui::Key, modifiers: egui::Modifiers) {
    q_events(vec![
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        },
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        },
    ]);
}

/// The package's block names the native script works on (see below).
fn proto_name(i: usize) -> String {
    std::env::var("MARKISE_PROTO_NAMES")
        .ok()
        .and_then(|v| v.split(',').nth(i).map(str::to_string))
        .unwrap_or_default()
}

/// PROTOTYPE measurement driver (`MARKISE_PROTO_NATIVE=1`, on a copy of a lesson): native blocks
/// through real pointer and key events; every step says what it saw. Screenshots go to
/// `MARKISE_PROTO_SHOTS`, the process exits at the end. The package's names come from
/// `MARKISE_PROTO_NAMES=<callout with text>,<choices>,<block without a view>,<callout with a list>,<its form field>`.
#[derive(Default)]
struct NativeScript {
    step: u32,
    since: Option<Instant>,
    original: String,
    walked: u32,
    in_marker: u32,
    furthest: usize,
    saves: usize,
    natives: usize,
    fails: u32,
    probe: usize,
    revision: u64,
    find_range: std::ops::Range<usize>,
    walk_from: Option<Instant>,
}

pub fn native_script(ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
    if !std::env::var_os("MARKISE_PROTO_NATIVE").is_some_and(|v| v != "0") {
        return;
    }
    NATIVE_SCRIPT.with(|s| s.borrow_mut().get_or_insert_with(Default::default).frame(ctx, viewer));
}

impl NativeScript {
    fn check(&mut self, what: &str, ok: bool) {
        eprintln!("[proto-native] {} {what}", if ok { "OK  " } else { "FAIL" });
        if !ok {
            self.fails += 1;
        }
    }

    /// Scrolls so that the block at `off` is in the upper part of the window.
    fn scroll_to(viewer: &mut crate::viewer::DocViewer, off: usize) {
        viewer.ed.view.ensure_offset(off);
        let i = viewer.ed.view.model().item_at_offset(off);
        viewer.ed.view.scroll_to_item(i);
    }

    fn block_off(viewer: &crate::viewer::DocViewer, name: &str) -> usize {
        markise_engine::block_form::all_blocks(viewer.ed.session.text())
            .iter()
            .find(|b| b.name == name)
            .map_or(0, |b| b.range.start)
    }

    fn frame(&mut self, ctx: &egui::Context, viewer: &mut crate::viewer::DocViewer) {
        use crate::block_snap::BlockView as V;
        use egui::{Key, Modifiers};
        use markise_doc::Selection;
        ctx.request_repaint();
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        let after = |ms: u64| now - since >= std::time::Duration::from_millis(ms);
        if RAW_QUEUE.with(|q| !q.borrow().is_empty()) {
            self.since = Some(now);
            return;
        }
        let Some(snaps) = viewer.snaps.as_ref() else { return };
        let pending = snaps.pending();
        let form_open = snaps.form.is_open();
        let n_saves = snaps.form.saves.len();
        let shot = |name: &str| crate::block_snap::ProtoScript::default().shot(ctx, name);
        let text = viewer.ed.session.text().to_string();
        let same = text == self.original;
        let ctl_view = |viewer: &crate::viewer::DocViewer, off: usize| {
            viewer.block_ctl.iter().find(|c| c.offset == off).map(|c| c.view)
        };
        let next = |me: &mut Self| {
            me.step += 1;
            me.since = Some(Instant::now());
        };
        match self.step {
            0 => {
                if !viewer.partial && !snaps.blocks.is_empty() && pending == 0 && after(1500) {
                    self.original = text.clone();
                    let path = viewer.ed.path().map(|p| p.to_path_buf());
                    let disk = path.and_then(|p| std::fs::read_to_string(p).ok());
                    self.check("document = file on open", disk.as_deref() == Some(text.as_str()));
                    eprintln!(
                        "[proto-native] blocks on the pages in: native {:?}; site images {}",
                        snaps.native.iter().map(|n| n.spec.title.clone()).collect::<Vec<_>>(),
                        viewer.block_ctl.iter().filter(|c| c.view == V::Site).count()
                    );
                    shot("n-01-top");
                    self.walked = 0;
                    self.in_marker = 0;
                    self.step = 120;
                    self.since = Some(now);
                } else if pending != 0 || snaps.blocks.is_empty() {
                    self.since = Some(now);
                }
            }
            // the whole document in small scrolls: a block in the painted items has its view
            120 | 121 => {
                let down = self.step == 120;
                let painted: Vec<usize> = viewer.painted().iter().map(|((s, _), _)| *s).collect();
                let vh = viewer.surface_rect().map_or(800.0, |r| r.height());
                let bare = markise_engine::block_form::all_blocks(&text)
                    .iter()
                    .filter(|b| painted.contains(&b.range.start))
                    .filter(|b| !viewer.block_ctl.iter().any(|c| c.offset == b.range.start))
                    .count();
                if bare > 0 && self.walked > 0 {
                    self.in_marker += 1;
                }
                self.walked += 1;
                let (y, total) = (viewer.ed.view.scroll_y(), viewer.ed.view.total_height());
                let done = if down { y + vh as f64 >= total - 2.0 } else { y <= 1.0 };
                if done {
                    eprintln!(
                        "[proto-native] pass {}: {} frames, total height {total:.0}, frames with a block without a view {}, evicted {}",
                        if down { "down" } else { "up" },
                        self.walked,
                        self.in_marker,
                        viewer.ed.view.paging_stats().evicted
                    );
                    self.check("scroll pass: every painted block has its view", self.in_marker <= 2);
                    self.walked = 0;
                    self.in_marker = 0;
                    self.step = if down { 121 } else { 1 };
                    self.since = Some(now);
                } else {
                    viewer.ed.view.scroll_by(if down { vh * 0.3 } else { -vh * 0.3 });
                }
            }
            1 if after(500) => {
                Self::scroll_to(viewer, Self::block_off(viewer, &proto_name(0)));
                next(self);
            }
            2 if after(500) => {
                shot("n-02-note");
                let off = Self::block_off(viewer, &proto_name(0));
                self.check("callout shown natively", ctl_view(viewer, off) == Some(V::Native));
                let vt = viewer.visible_text();
                eprintln!("[proto-native] note unit: {:?}", viewer.native_units.iter().find(|u| u.0 == off));
                let _ = vt;
                let b = markise_engine::block_form::block_at(&text, off).expect("note");
                // the last paragraph of the note: click into its text
                let last = text[b.inner.clone()].trim_end().rfind("\n\n").map_or(b.inner.start, |i| b.inner.start + i + 2);
                self.probe = last;
                if let Some(r) = viewer.item_screen_rect(last) {
                    q_click(egui::pos2(r.left() + 60.0, r.bottom() - 10.0));
                }
                next(self);
            }
            3 if after(150) => {
                let c = viewer.ed.session.selection().primary().focus.offset;
                self.check(&format!("click put the caret in the note text ({c})"), c >= self.probe);
                q_events(vec![egui::Event::Text("ЙЖ".into())]);
                next(self);
            }
            4 if after(200) => {
                self.check("typed in place", text.len() == self.original.len() + 4 && text.contains("ЙЖ"));
                let off = Self::block_off(viewer, &proto_name(0));
                self.check("still native after typing", ctl_view(viewer, off) == Some(V::Native));
                shot("n-03-typed");
                q_events(vec![egui::Event::Text("Q".into())]);
                next(self);
            }
            5 if after(200) => {
                viewer.ed.edit(|s| s.undo());
                next(self);
            }
            6 if after(200) => {
                self.check("typing undone: byte-identical", same);
                // Backspace at the start of the first inner item, Delete at the end of the last
                let off = Self::block_off(viewer, &proto_name(0));
                let b = markise_engine::block_form::block_at(&text, off).expect("block");
                let first = b.inner.start + text[b.inner.clone()].len() - text[b.inner.clone()].trim_start().len();
                viewer.ed.set_selection(Selection::caret(first));
                q_key(Key::Backspace, Modifiers::NONE);
                next(self);
            }
            7 if after(150) => {
                self.check("Backspace at the card's start changes nothing", same);
                let off = Self::block_off(viewer, &proto_name(0));
                let b = markise_engine::block_form::block_at(&text, off).expect("block");
                let last = b.inner.start + text[b.inner.clone()].trim_end().len();
                viewer.ed.set_selection(Selection::caret(last));
                q_key(Key::Delete, Modifiers::NONE);
                next(self);
            }
            8 if after(150) => {
                self.check("Delete at the card's end changes nothing", same);
                // find highlights inside the card
                let off = Self::block_off(viewer, &proto_name(0));
                let (at, len) = {
                    let b = markise_engine::block_form::block_at(&text, off).expect("block");
                    let inner = text[b.inner.clone()].trim_end();
                    let para = inner.rfind("\n\n").map_or(0, |i| i + 2);
                    let word = inner[para..].split_whitespace().next().unwrap_or("");
                    (b.inner.start + para, word.len())
                };
                self.find_range = at..at + len;
                self.step = 130;
                self.since = Some(now);
            }
            // (the app sets the find matches every frame: hold them here)
            130 if !after(200) => {
                let rev = viewer.ed.session.revision().counter;
                viewer.find_matches = Some((rev, std::sync::Arc::new(vec![self.find_range.clone()]), Some(0)));
            }
            130 if after(200) => {
                let off = Self::block_off(viewer, &proto_name(0));
                let card = viewer.block_ctl.iter().find(|c| c.offset == off).map(|c| c.rect);
                let rects = viewer.find_rects.clone();
                self.check(
                    &format!("find highlight inside the card ({} rects)", rects.len()),
                    !rects.is_empty() && card.is_some_and(|c| rects.iter().all(|r| c.contains_rect(*r))),
                );
                shot("n-20-find");
                viewer.find_matches = None;
                // a comment typed at the end of the note's text
                let b = markise_engine::block_form::block_at(&text, off).expect("note");
                let last = b.inner.start + text[b.inner.clone()].trim_end().len();
                viewer.ed.set_selection(Selection::caret(last));
                q_events(vec![egui::Event::Text("{>>комментарий<<}".into())]);
                self.step = 131;
                self.since = Some(now);
            }
            131 if after(400) => {
                self.check("comment typed in a card is listed", !viewer.shown_comments().is_empty());
                shot("n-21-comment");
                while viewer.ed.session.text() != self.original && viewer.ed.session.can_undo() {
                    viewer.ed.edit(|s| s.undo());
                }
                self.step = 132;
                self.since = Some(now);
            }
            132 if after(200) => {
                self.check("comment undone: byte-identical", same);
                // Enter at the end of the card's text stays in the card
                let off = Self::block_off(viewer, &proto_name(0));
                let b = markise_engine::block_form::block_at(&text, off).expect("block");
                let last = b.inner.start + text[b.inner.clone()].trim_end().len();
                viewer.ed.set_selection(Selection::caret(last));
                q_key(Key::Enter, Modifiers::NONE);
                self.step = 133;
                self.since = Some(now);
            }
            133 if after(300) => {
                shot("n-22-enter");
                q_events(vec![egui::Event::Text("Ж".into())]);
                self.step = 134;
                self.since = Some(now);
            }
            134 if after(300) => {
                let off = Self::block_off(viewer, &proto_name(0));
                let b = markise_engine::block_form::block_at(&text, off).expect("block");
                self.check(
                    "Enter + typing at the end of a card stays inside it",
                    text[b.inner.clone()].contains('Ж') && ctl_view(viewer, off) == Some(V::Native),
                );
                shot("n-23-enter-typed");
                while viewer.ed.session.text() != self.original && viewer.ed.session.can_undo() {
                    viewer.ed.edit(|s| s.undo());
                }
                self.step = 135;
                self.since = Some(now);
            }
            135 if after(200) => {
                self.check("Enter undone: byte-identical", same);
                self.walk_from = Some(now);
                viewer.ed.set_selection(Selection::caret(0));
                viewer.ed.view.scroll_to_item(0);
                self.walked = 0;
                self.in_marker = 0;
                self.furthest = 0;
                self.step = 9;
                self.since = Some(now);
            }
            // the caret walks down the document
            9 => {
                let c = viewer.ed.session.selection().primary().focus.offset;
                let bad = viewer
                    .native_markers
                    .iter()
                    .any(|a| c >= a.0 && c <= a.1);
                self.in_marker += bad as u32;
                if bad {
                    eprintln!(
                        "[proto-native] caret {c} on a marker: {:?}, next text {:?}",
                        viewer.native_markers.iter().find(|a| c >= a.0 && c <= a.1),
                        text.get(c..).map(|t| t.chars().take(12).collect::<String>())
                    );
                }
                self.furthest = self.furthest.max(c);
                if self.walked < 200 {
                    self.walked += 1;
                    q_key(Key::ArrowDown, Modifiers::NONE);
                } else {
                    self.check(
                        &format!(
                            "caret walk: 200 ↓, furthest offset {} of {}, frames on a marker line {}",
                            self.furthest,
                            text.len(),
                            self.in_marker
                        ),
                        self.in_marker == 0 && self.furthest > text.len() / 2,
                    );
                    eprintln!(
                        "[proto-native] caret walk took {:.1} s",
                        self.walk_from.map_or(0.0, |t| t.elapsed().as_secs_f32())
                    );
                    self.check("caret walk: byte-identical", same);
                    self.walked = 0;
                    self.in_marker = 0;
                    next(self);
                }
            }
            10 => {
                let c = viewer.ed.session.selection().primary().focus.offset;
                self.in_marker += viewer.native_markers.iter().any(|a| c >= a.0 && c <= a.1) as u32;
                if self.walked < 200 {
                    self.walked += 1;
                    q_key(Key::ArrowUp, Modifiers::NONE);
                } else {
                    self.check(
                        &format!("caret walk back: ends at {c}, frames on a marker line {}", self.in_marker),
                        self.in_marker == 0,
                    );
                    self.step = 100;
                    self.since = Some(now);
                }
            }
            100 if after(500) => {
                Self::scroll_to(viewer, Self::block_off(viewer, &proto_name(1)));
                self.step = 11;
                self.since = Some(now);
            }
            // a question: answer rows, hidden labels, toggling
            11 if after(500) => {
                shot("n-04-question");
                let off = Self::block_off(viewer, &proto_name(1));
                self.check("choices shown natively", ctl_view(viewer, off) == Some(V::Native));
                let vt = viewer.visible_text();
                self.check("no {#id} label on screen", !vt.contains("{#a"));
                self.check("the labels are in the file", text.contains("{#a1}"));
                let n = snaps.native.iter().find(|n| n.offset == off).cloned();
                let Some(n) = n else {
                    next(self);
                    return;
                };
                let (a, _) = n.answers[0];
                self.probe = a;
                let i = viewer.ed.view.model().item_at_offset(a);
                let l = viewer.ed.view.cache_peek(i).cloned();
                let r = viewer.item_screen_rect(a);
                if let (Some(l), Some(r)) = (l, r)
                    && let Some((b, _)) = l.tasks.first()
                {
                    q_click(r.min + b.center().to_vec2());
                } else {
                    self.check("answer's checkbox found", false);
                }
                next(self);
            }
            12 if after(200) => {
                self.check(
                    "click on the box toggled the answer, nothing else",
                    text.len() == self.original.len() && !same && text.contains("{#a1}"),
                );
                shot("n-05-toggled");
                viewer.ed.edit(|s| s.undo());
                next(self);
            }
            13 if after(200) => {
                self.check("toggle undone: byte-identical", same);
                // Enter at an answer's end: its label stays with it
                let off = Self::block_off(viewer, &proto_name(1));
                let hid = snaps.native.iter().find(|n| n.offset == off).and_then(|n| n.hidden.first().cloned());
                if let Some(h) = hid {
                    self.probe = h.end;
                    viewer.ed.set_selection(Selection::caret(h.start));
                    q_key(Key::Enter, Modifiers::NONE);
                }
                self.step = 140;
                self.since = Some(now);
            }
            140 if after(300) => {
                let label = text.get(..self.probe).unwrap_or("");
                self.check("Enter at an answer's end leaves the label on it", label.ends_with("{#a1}") && !same);
                shot("n-24-answer-enter");
                while viewer.ed.session.text() != self.original && viewer.ed.session.can_undo() {
                    viewer.ed.edit(|s| s.undo());
                }
                self.step = 141;
                self.since = Some(now);
            }
            141 if after(200) => {
                self.check("answer's Enter undone: byte-identical", same);
                let off = Self::block_off(viewer, &proto_name(1));
                let vb = viewer.block_ctl.iter().find(|c| c.offset == off).map(|c| c.rect);
                if let Some(r) = vb {
                    // the view switch at the block's top right
                    q_click(egui::pos2(r.right() - 45.0, r.top() + 17.0));
                }
                self.revision = viewer.ed.session.revision().counter;
                next(self);
            }
            // rotation by real clicks: native → site → markdown → native
            14 if after(200) && pending == 0 => {
                let off = Self::block_off(viewer, &proto_name(1));
                self.check("rotate 1: question on Site", ctl_view(viewer, off) == Some(V::Site));
                shot("n-06-question-site");
                if let Some(c) = viewer.block_ctl.iter().find(|c| c.offset == off) {
                    q_click(egui::pos2(c.rect.right() - 45.0, c.rect.top() + 17.0));
                }
                next(self);
            }
            15 if after(300) => {
                let off = Self::block_off(viewer, &proto_name(1));
                self.check("rotate 2: question as Markdown", ctl_view(viewer, off) == Some(V::Markdown));
                let vt = viewer.visible_text();
                self.check("its raw source is on screen", vt.contains(&format!(":::{}", proto_name(1))) && vt.contains("{#a1}"));
                shot("n-07-question-markdown");
                if let Some(c) = viewer.block_ctl.iter().find(|c| c.offset == off) {
                    q_click(egui::pos2(c.rect.right() - 45.0, c.rect.top() + 17.0));
                }
                next(self);
            }
            16 if after(300) => {
                let off = Self::block_off(viewer, &proto_name(1));
                self.check("rotate 3: question back to Native", ctl_view(viewer, off) == Some(V::Native));
                self.check(
                    "rotating never touched the document",
                    same && viewer.ed.session.revision().counter == self.revision,
                );
                let scene = Self::block_off(viewer, &proto_name(2));
                Self::scroll_to(viewer, scene);
                next(self);
            }
            17 if after(500) && pending == 0 => {
                let off = Self::block_off(viewer, &proto_name(2));
                self.check("block without a view on Site", ctl_view(viewer, off) == Some(V::Site));
                if let Some(c) = viewer.block_ctl.iter().find(|c| c.offset == off) {
                    q_click(egui::pos2(c.rect.right() - 45.0, c.rect.top() + 17.0));
                }
                next(self);
            }
            18 if after(300) => {
                let off = Self::block_off(viewer, &proto_name(2));
                self.check("it rotated to Markdown", ctl_view(viewer, off) == Some(V::Markdown));
                // the state survives going to the end of the document (pages evicted) and back
                let n = viewer.ed.view.len();
                viewer.ed.view.scroll_to_item(n - 1);
                next(self);
            }
            19 if after(800) => {
                eprintln!(
                    "[proto-native] at the end: {}",
                    viewer.ed.view.paging_stats().evicted
                );
                Self::scroll_to(viewer, Self::block_off(viewer, &proto_name(2)));
                next(self);
            }
            20 if after(600) => {
                let off = Self::block_off(viewer, &proto_name(2));
                self.check("still Markdown after the trip", ctl_view(viewer, off) == Some(V::Markdown));
                // an edit above moves the block; the state follows
                viewer.ed.set_selection(Selection::caret(2));
                q_events(vec![egui::Event::Text("ЙЖ".into())]);
                next(self);
            }
            21 if after(300) => {
                let off = Self::block_off(viewer, &proto_name(2));
                self.check("… and after an edit above it", ctl_view(viewer, off) == Some(V::Markdown));
                viewer.ed.edit(|s| s.undo());
                next(self);
            }
            22 if after(300) => {
                let off = Self::block_off(viewer, &proto_name(2));
                self.check("… and after its undo", ctl_view(viewer, off) == Some(V::Markdown));
                self.check("byte-identical", same);
                viewer.snaps.as_mut().expect("snaps").set_all_views(V::Site);
                next(self);
            }
            // the global switch
            23 if after(400) && pending == 0 => {
                self.check("all on Site: nothing native", snaps.native.is_empty());
                shot("n-08-all-site");
                viewer.snaps.as_mut().expect("snaps").set_all_views(V::Markdown);
                next(self);
            }
            24 if after(400) => {
                self.check("all Markdown: nothing native, no images", snaps.native.is_empty() && viewer.atomic.is_empty());
                shot("n-09-all-markdown");
                viewer.snaps.as_mut().expect("snaps").set_all_views(V::Native);
                next(self);
            }
            25 if after(400) && pending == 0 => {
                let scene = Self::block_off(viewer, &proto_name(2));
                self.check(
                    "all Native: blocks without a view stay on Site",
                    !snaps.native.is_empty() && ctl_view(viewer, scene).is_none_or(|v| v == V::Site),
                );
                self.check("byte-identical", same);
                // the form from a native block, through its button
                let off = Self::block_off(viewer, &proto_name(3));
                Self::scroll_to(viewer, off);
                next(self);
            }
            26 if after(500) => {
                let off = Self::block_off(viewer, &proto_name(3));
                if let Some(c) = viewer.block_ctl.iter().find(|c| c.offset == off) {
                    q_click(egui::pos2(c.rect.right() - 125.0, c.rect.top() + 17.0));
                }
                self.saves = n_saves;
                next(self);
            }
            27 if form_open && after(1500) => {
                snaps.form.eval("window.markiseForm.save()");
                next(self);
            }
            28 if n_saves > self.saves && after(300) => {
                self.check(&format!("form from a native block, unchanged save: {:?}", snaps.form.saves.last()), same);
                self.saves = n_saves;
                let off = Self::block_off(viewer, &proto_name(3));
                if form_open {
                    viewer.snaps.as_mut().expect("snaps").form.close();
                }
                if let Some(c) = viewer.block_ctl.iter().find(|c| c.offset == off) {
                    q_click(egui::pos2(c.rect.right() - 125.0, c.rect.top() + 17.0));
                }
                self.step = 110;
                self.since = Some(now);
            }
            110 if form_open && after(1500) => {
                snaps.form.eval(
                    &format!(
                        "{{ const d = window.markiseForm.data(); window.markiseForm.set('{f}.0', d.{f}[0] + ' (ред.)'); setTimeout(() => window.markiseForm.save(), 80); }}",
                        f = proto_name(4)
                    ),
                );
                self.step = 29;
                self.since = Some(now);
            }
            29 if n_saves > self.saves && after(600) => {
                self.check("form edit applied", text.contains("(ред.)"));
                viewer.snaps.as_mut().expect("snaps").form.close();
                next(self);
            }
            30 if after(400) => {
                let off = Self::block_off(viewer, &proto_name(3));
                self.check("native again after the form", ctl_view(viewer, off) == Some(V::Native));
                shot("n-10-goals-after-form");
                viewer.ed.edit(|s| s.undo());
                next(self);
            }
            31 if after(300) => {
                self.check("form edit undone: byte-identical", same);
                // palette: a new note after the goals gives a native card
                let root = snaps.form.root().map(Path::to_path_buf);
                let presets = root.map(|r| crate::block_lib::presets(&r, &lang())).unwrap_or_default();
                let p = presets.iter().find(|p| p.block == proto_name(0)).cloned();
                let gap = viewer.native_units.first().map_or(0, |a| a.1);
                let rev = viewer.ed.session.revision().counter;
                self.natives = markise_engine::block_form::all_blocks(&text).len();
                if let Some(p) = p {
                    crate::block_palette::choose(p, Some(InsertTarget::new(&text, gap, gap, rev)));
                } else {
                    self.check("note preset found", false);
                }
                self.saves = n_saves;
                next(self);
            }
            32 if form_open && after(1500) => {
                shot("n-11-palette-draft");
                snaps.form.eval("window.markiseForm.save()");
                next(self);
            }
            33 if n_saves > self.saves && after(800) => {
                let blocks = markise_engine::block_form::all_blocks(&text).len();
                self.check(&format!("palette: block inserted ({:?})", snaps.form.saves.last()), blocks == self.natives + 1);
                viewer.snaps.as_mut().expect("snaps").form.close();
                next(self);
            }
            34 if after(600) => {
                let gap = viewer.native_units.get(1).copied();
                eprintln!("[proto-native] after insert: {gap:?}");
                let off = viewer.native_units.first().map_or(0, |a| a.3);
                self.check("the inserted note is native", snaps.native.iter().any(|n| n.offset == off));
                shot("n-12-palette-inserted");
                viewer.ed.edit(|s| s.undo());
                next(self);
            }
            35 if after(400) => {
                self.check("insertion undone: byte-identical", same);
                // save: must write back exactly the opened bytes
                viewer.ed.set_selection(Selection::caret(0));
                q_key(Key::S, Modifiers::COMMAND);
                next(self);
            }
            36 if after(1000) => {
                let path = viewer.ed.path().map(|p| p.to_path_buf());
                let disk = path.and_then(|p| std::fs::read_to_string(p).ok());
                self.check("file on disk = what was opened", disk.as_deref() == Some(self.original.as_str()));
                eprintln!("[proto-native] done, failures: {}", self.fails);
                std::process::exit(if self.fails == 0 { 0 } else { 1 });
            }
            _ => {
                if after(30000) {
                    self.check(&format!("step {} timed out (form open {form_open})", self.step), false);
                    self.step += 1;
                    self.since = Some(now);
                }
            }
        }
    }
}
