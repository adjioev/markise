//! PROTOTYPE (branch `proto/block-forms`, not product code): the AEM-style edit dialog of a
//! `:::name` block shown as a snapshot.
//!
//! "✎ Edit" on a block image → Markise decodes the block with
//! [`markise_engine::block_form`] against the site's `components/blocks.json`, generates the
//! JSON Schema / UI Schema / data (plus the site's optional `blocks/<name>/<name>.form.json`,
//! `.form.js` custom renderers and `.form.css`) and asks the snapshot helper to show its editor
//! window. On Save the helper sends the data back; Markise checks the document did not change
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

use crate::block_snap::{LiveStdin, ThemeVars, write_frame};

struct Open {
    id: u64,
    dec: bf::Decoded,
    revision: u64,
    sent: Instant,
    lang: String,
}

/// Edit dialog state of one viewer.
pub struct FormState {
    live: LiveStdin,
    events: Receiver<Value>,
    doc: Option<PathBuf>,
    open: Option<Open>,
    seq: u64,
    log: bool,
    /// (cold, click → form painted ms, helper op → painted ms) of every open (measurements).
    pub opens: Vec<(bool, f64, f64)>,
    /// Results of saves: Ok(edit count) or Err(message) (measurement script).
    pub saves: Vec<Result<usize, String>>,
    /// Why the dialog could not open (shown over the pane for a few seconds).
    pub error: Option<(String, Instant)>,
}

pub fn lang() -> String {
    if let Ok(l) = std::env::var("MARKISE_LANG") {
        return if l.starts_with("en") { "en".into() } else { "ru".into() };
    }
    let sys = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if sys.starts_with("en") {
        "en".into()
    } else {
        "ru".into()
    }
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
    pub fn new(live: LiveStdin, events: Receiver<Value>, doc: Option<PathBuf>) -> Self {
        FormState {
            live,
            events,
            doc,
            open: None,
            seq: 0,
            log: std::env::var_os("MARKISE_BLOCK_LOG").is_some_and(|v| v != "0"),
            opens: Vec::new(),
            saves: Vec::new(),
            error: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        let slot = self.live.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let stdin = slot.ok_or("the snapshot helper is not running")?;
        let mut w = stdin.lock().unwrap_or_else(|e| e.into_inner());
        write_frame(&mut *w, v).map_err(|e| format!("helper: {e}"))
    }

    /// Opens the dialog for the block whose `:::` line starts at `offset`.
    pub fn open(&mut self, text: &str, offset: usize, revision: u64, theme: &ThemeVars) -> Result<(), String> {
        let t = Instant::now();
        let doc = self.doc.as_deref().ok_or("the document has no path (no site)")?;
        let root = bf::site_root(doc).ok_or("no components/blocks.json above the document")?;
        let manifest = bf::load_manifest(&root)?;
        let block = bf::block_at(text, offset).ok_or("no :::block at this position")?;
        let def = manifest
            .get(&block.name)
            .ok_or_else(|| format!("`{}` is not in blocks.json", block.name))?;
        let dec = bf::decode(text, &block, def)?;
        let lang = lang();
        let dir = form_dir(Some(&root), &block.name);
        let read = |ext: &str| {
            dir.as_ref()
                .and_then(|d| std::fs::read_to_string(d.join(format!("{}.form.{ext}", block.name))).ok())
        };
        let over: Option<Value> = read("json").and_then(|s| serde_json::from_str(&s).ok());
        let (title, schema, uischema) = bf::form(&dec, &lang, over.as_ref());
        let line = text[..offset].matches('\n').count() + 1;
        let file = doc
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        self.seq += 1;
        let id = self.seq;
        let req = json!({
            "op": "form_open", "id": id, "title": title, "block": block.name,
            "schema": schema, "uischema": uischema, "data": dec.data, "lang": lang,
            "theme": if theme.dark { "dark" } else { "light" }, "vars": theme.vars,
            "note": format!("{file}, {} {line} · :::{} {}", if lang == "ru" { "строка" } else { "line" }, block.name, block.args),
            "site_js": read("js").unwrap_or_default(), "site_css": read("css").unwrap_or_default(),
        });
        let ms_build = t.elapsed().as_secs_f64() * 1e3;
        self.send(&req)?;
        if self.log {
            eprintln!(
                "[blockform] open #{id} :::{} at {offset}: form built in {ms_build:.2} ms (site files: {:?})",
                block.name, dir
            );
        }
        self.open = Some(Open {
            id,
            dec,
            revision,
            sent: t,
            lang,
        });
        Ok(())
    }

    /// Hides the dialog without saving (measurement script).
    pub fn close(&mut self) {
        self.open = None;
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
                "cancel" => {
                    if id == current {
                        self.open = None;
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
                    next(self);
                }
            }
            // opens: cold first, then warm, cycling through goals / question / scene
            2 => {
                let order: Vec<usize> = ["goals", "question", "scene"]
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
                        } else if self.sub == 2 {
                            Self::shot_form(&snaps.form, "forms-proto-scene");
                        }
                        self.waiting = Some((usize::MAX, 0));
                        self.since = Some(now);
                    }
                    Some((usize::MAX, _)) if after(if self.sub < 3 { 700 } else { 150 }) => {
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
                    viewer.ed.source_set_selection(markise_doc::Selection::caret(end));
                    let _ = viewer.ed.source_insert("\nx", false);
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
            // a locked change (correctness) is rejected
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
                        "{ const d = window.markiseForm.data(); const k = Object.keys(d.options)[0]; window.markiseForm.set('options.' + k + '.correct', !d.options[k].correct); setTimeout(() => window.markiseForm.save(), 80); }",
                    );
                    self.waiting = Some((usize::MAX, s));
                }
                Some((usize::MAX, s)) if saves > s => {
                    let r = snaps.form.saves.last().cloned();
                    self.say(format!("locked change (correctness) → {r:?}"));
                    viewer.snaps.as_mut().expect("snaps").form.close();
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
