//! PROTOTYPE check of the native block form (`MARKISE_PROTO_FORM_SCRIPT=1`, on a copy of a
//! lesson, live mode): real pointer and key events into the real panel; every step says what it
//! saw. Screenshots go to `MARKISE_PROTO_SHOTS`; the process exits at the end.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};
use markise_engine::block_form as bf;
use serde_json::Value;

use crate::block_edit::{q_click, q_events, q_idle, q_key};
use crate::viewer::DocViewer;

type Pred = Box<dyn Fn(&mut W<'_>) -> bool>;

enum S {
    /// The n-th block with this name and arguments; waits for the form.
    Open(String, String, usize),
    Click(String),
    ClickDoc,
    Type(String),
    Ime(String),
    Key(Key, Modifiers),
    Wait(u64),
    Check(String, Pred),
    Do(Box<dyn Fn(&mut W<'_>)>),
    Shot(String),
    Undo,
}

struct W<'a> {
    v: &'a mut DocViewer,
    ctx: &'a egui::Context,
    original: &'a str,
    saved: &'a mut Option<Option<egui::Id>>,
}

impl W<'_> {
    fn same(&self) -> bool {
        self.v.ed.session.text() == self.original
    }
    fn native(&mut self) -> Option<&mut crate::form_native::NativeForm> {
        self.v.snaps.as_mut()?.form.native.as_mut()
    }
    fn val(&mut self, key: &str) -> Value {
        self.native().map_or(Value::Null, |n| n.values[key].clone())
    }
    fn is_open(&self) -> bool {
        self.v.snaps.as_ref().is_some_and(|s| s.form.is_open())
    }
    fn focus(&self) -> Option<egui::Id> {
        self.ctx.memory(|m| m.focused())
    }
    fn last_save(&self) -> String {
        format!("{:?}", self.v.snaps.as_ref().and_then(|s| s.form.saves.last()))
    }
}

#[derive(Default)]
pub struct FormScript {
    plan: Option<VecDeque<S>>,
    original: String,
    until: Option<Instant>,
    open_since: Option<Instant>,
    saved_focus: Option<Option<egui::Id>>,
    fails: u32,
    started: Option<Instant>,
}

thread_local! {
    static SCRIPT: std::cell::RefCell<Option<FormScript>> = const { std::cell::RefCell::new(None) };
}

pub fn run(ctx: &egui::Context, v: &mut DocViewer) {
    if !std::env::var_os("MARKISE_PROTO_FORM_SCRIPT").is_some_and(|x| x != "0") {
        return;
    }
    SCRIPT.with(|s| s.borrow_mut().get_or_insert_with(Default::default).frame(ctx, v));
}

fn check(s: &str, f: impl Fn(&mut W<'_>) -> bool + 'static) -> S {
    S::Check(s.to_string(), Box::new(f))
}

fn cmd_enter() -> S {
    S::Key(Key::Enter, Modifiers::COMMAND)
}

fn esc() -> S {
    S::Key(Key::Escape, Modifiers::NONE)
}

fn click(s: &str) -> S {
    S::Click(s.to_string())
}

fn typ(s: &str) -> S {
    S::Type(s.to_string())
}

fn open(name: &str, args: &str, n: usize) -> S {
    S::Open(name.to_string(), args.to_string(), n)
}

fn shot(s: &str) -> S {
    S::Shot(s.to_string())
}

fn closed_same(what: &str) -> S {
    check(what, |w| !w.is_open() && w.same())
}

fn plan(text: &str) -> VecDeque<S> {
    let blocks = bf::all_blocks(text);
    let mut p: Vec<S> = Vec::new();
    let has = |n: &str, a: &str| blocks.iter().any(|b| b.name == n && b.args == a);
    // every block opens; an unchanged save writes nothing
    let mut counts = std::collections::HashMap::<(String, String), usize>::new();
    let mut seen = std::collections::HashSet::new();
    for b in &blocks {
        let k = (b.name.clone(), b.args.clone());
        let n = counts.entry(k).or_default();
        p.push(open(&b.name, &b.args, *n));
        *n += 1;
        if seen.insert(format!("{}-{}", b.name, b.args.split('/').next().unwrap_or(""))) {
            p.push(S::Wait(500));
            p.push(shot(&format!(
                "form-{}{}",
                b.name,
                if b.args.is_empty() || b.name == "scene" {
                    String::new()
                } else {
                    format!("-{}", b.args)
                }
            )));
        }
        p.push(cmd_enter());
        let what = format!(":::{} {} #{n}: unchanged save is byte-identical", b.name, b.args);
        p.push(S::Wait(120));
        p.push(check(&what, |w| {
            eprintln!("    save: {}", w.last_save());
            !w.is_open() && w.same() && w.last_save().contains("Ok(0)")
        }));
    }
    // string list: edit, live preview without a write, add, reorder, remove
    if has("goals", "") {
        p.extend([
            open("goals", "", 0),
            S::Wait(200),
            check("the first field has the keyboard", |w| {
                w.native().is_some_and(|n| n.state.had_focus())
            }),
            click("items/0/text"),
            typ(" ПРАВКА"),
            check("no preview before the debounce", |w| {
                w.v.snaps.as_ref().is_some_and(|s| s.form.preview.is_none())
            }),
            S::Wait(350),
            check("preview shows the value, document not written", |w| {
                let p = w.v.snaps.as_ref().and_then(|s| s.form.preview.clone());
                w.same() && p.is_some_and(|p| p.markdown.contains("ПРАВКА"))
            }),
            shot("form-preview"),
            click("items/add"),
            S::Wait(200),
            typ("новый пункт"),
            S::Wait(60),
            check("add: the new row has the keyboard", |w| {
                w.val("items").as_array().and_then(|a| a.last().cloned()) == Some(Value::from("новый пункт"))
            }),
            click("items/0/down"),
            S::Wait(100),
            check("down: first and second swapped", |w| {
                w.val("items")[1].as_str().is_some_and(|x| x.contains("ПРАВКА"))
            }),
            shot("form-list"),
            click("items/2/remove"),
            S::Wait(100),
            check("a row removed; document still not written", |w| w.same()),
            cmd_enter(),
            S::Wait(200),
            check("saved as one step", |w| {
                !w.is_open()
                    && w.v.ed.session.text().contains("ПРАВКА")
                    && w.v.ed.session.text().contains("новый пункт")
            }),
            S::Undo,
            check("undo: byte-identical", |w| w.same()),
        ]);
    }
    // answers
    if has("question", "") {
        p.extend([
            open("question", "", 0),
            S::Wait(200),
            click("options/1/correct"),
            check("radio: exactly one correct", |w| {
                w.val("options")
                    .as_array()
                    .is_some_and(|a| a.iter().filter(|x| x["correct"] == true).count() == 1)
                    && w.val("options")[1]["correct"] == true
            }),
            shot("form-question"),
            click("options/0/text"),
            S::Do(Box::new(|w| *w.saved = Some(w.focus()))),
            S::Key(Key::ArrowDown, Modifiers::ALT),
            S::Wait(100),
            check("⌥↓: the row moved and kept the keyboard", |w| {
                Some(w.focus()) == *w.saved
            }),
            typ("Ж"),
            S::Wait(60),
            check("typing goes to the moved row; ids kept", |w| {
                let o = w.val("options");
                o[1]["text"].as_str().is_some_and(|t| t.contains('Ж')) && o[1]["id"] != o[0]["id"]
            }),
            cmd_enter(),
            S::Wait(200),
            check("saved", |w| !w.is_open() && w.v.ed.session.text().contains('Ж')),
            S::Undo,
            check("undo: byte-identical", |w| w.same()),
            // shortcuts and focus
            open("question", "", 0),
            S::Wait(200),
            S::Key(Key::B, Modifiers::COMMAND),
            S::Key(Key::A, Modifiers::COMMAND),
            S::Wait(100),
            check("⌘B, ⌘A in a field do not reach the document", |w| w.same()),
            S::Do(Box::new(|w| *w.saved = Some(w.focus()))),
            S::Key(Key::Tab, Modifiers::NONE),
            S::Wait(100),
            check("Tab moves to another control of the form", |w| {
                Some(w.focus()) != *w.saved && w.native().is_some_and(|n| n.state.had_focus())
            }),
        ]);
        for _ in 0..30 {
            p.push(S::Key(Key::Tab, Modifiers::NONE));
        }
        p.extend([
            S::Wait(150),
            check("many Tabs stay in the form", |w| {
                w.native().is_some_and(|n| n.state.had_focus())
            }),
            S::Key(Key::Tab, Modifiers::SHIFT),
            S::Wait(100),
            check("Shift+Tab stays in the form", |w| {
                w.native().is_some_and(|n| n.state.had_focus())
            }),
            click("prompt/text"),
            S::Ime("ИМЕ".to_string()),
            S::Wait(60),
            check("IME commit reaches the field", |w| {
                w.val("prompt").as_str().is_some_and(|t| t.contains("ИМЕ"))
            }),
            S::ClickDoc,
            S::Wait(150),
            check("click outside keeps the draft", |w| {
                w.is_open() && w.val("prompt").as_str().is_some_and(|t| t.contains("ИМЕ"))
            }),
            S::Do(Box::new(|w| {
                let ps =
                    markise_doc::PatchSet::new(vec![markise_doc::Patch::verbatim(0..0, "\n".to_string())]).expect("p");
                let _ = w.v.ed.edit(|s| s.apply_external(&ps, "t", false));
            })),
            S::Wait(100),
            click("$save"),
            S::Wait(200),
            check("stale revision: rejected, banner, form open", |w| {
                let banner = w.native().and_then(|n| n.banner.clone());
                eprintln!("    banner: {banner:?}");
                w.is_open() && banner.is_some_and(|b| !b.is_empty())
            }),
            shot("form-stale"),
            click("$cancel"),
            S::Wait(150),
            S::Undo,
            closed_same("cancel and undo: byte-identical"),
        ]);
    }
    // pictures
    if blocks.iter().any(|b| b.name == "scene") {
        p.extend([
            open(
                "scene",
                blocks.iter().find(|b| b.name == "scene").map_or("", |b| &b.args),
                0,
            ),
            S::Wait(800),
            shot("form-scene"),
            click("pictures/0/alt"),
            typ(" АЛЬТ"),
            click("pictures/0/src"),
            typ("x"),
            S::Wait(300),
            shot("form-scene-nopreview"),
            cmd_enter(),
            S::Wait(200),
            check("picture alt and path saved", |w| {
                w.v.ed.session.text().contains("АЛЬТ") && !w.is_open()
            }),
            S::Undo,
            check("undo: byte-identical", |w| w.same()),
            open(
                "scene",
                blocks.iter().find(|b| b.name == "scene").map_or("", |b| &b.args),
                0,
            ),
            S::Wait(200),
            click("pictures/add"),
            S::Wait(150),
            check("an added picture without a path is an error", |w| {
                w.native()
                    .is_some_and(|n| n.errors.iter().any(|e| e.path.ends_with("/src")))
            }),
            cmd_enter(),
            S::Wait(150),
            check("save refused with errors: banner, nothing written", |w| {
                w.same() && w.native().is_some_and(|n| n.banner.is_some())
            }),
            shot("form-scene-error"),
            esc(),
            S::Wait(200),
            closed_same("Esc: nothing written"),
        ]);
    }
    // direction buttons
    if has("question", "directions") {
        p.extend([
            open("question", "directions", 0),
            S::Wait(300),
            shot("form-directions"),
            click("options/dir/stop"),
            check("exclusive «cannot go» clears the arrows", |w| {
                w.val("options")
                    .as_array()
                    .is_some_and(|a| a.iter().filter(|x| x["correct"] == true).count() == 1)
            }),
            shot("form-directions-stop"),
            click("options/dir/left"),
            click("options/dir/right"),
            check("arrows clear the exclusive answer", |w| {
                w.val("options")
                    .as_array()
                    .is_some_and(|a| a.iter().filter(|x| x["correct"] == true).count() == 2)
            }),
            cmd_enter(),
            S::Wait(200),
            check(":::question directions saved", |w| !w.is_open() && !w.same()),
            S::Undo,
            check("undo: byte-identical", |w| w.same()),
        ]);
    }
    // a draft from the palette
    p.extend([
        S::Do(Box::new(|w| choose(w, "question"))),
        S::Wait(600),
        check("palette: a draft opens, document not written", |w| {
            w.is_open() && w.same()
        }),
        shot("form-draft"),
        esc(),
        S::Wait(200),
        closed_same("Cancel on a draft writes nothing"),
        S::Do(Box::new(|w| choose(w, "question"))),
        S::Wait(600),
        cmd_enter(),
        S::Wait(200),
        check("draft without a correct answer: refused", |w| {
            w.is_open() && w.same() && w.native().is_some_and(|n| n.banner.is_some())
        }),
        click("options/1/correct"),
        cmd_enter(),
        S::Wait(250),
        check("draft saved: one block more", |w| {
            !w.is_open() && bf::all_blocks(w.v.ed.session.text()).len() == bf::all_blocks(w.original).len() + 1
        }),
        S::Undo,
        check("undo: byte-identical", |w| w.same()),
    ]);
    p.into()
}

fn choose(w: &mut W<'_>, block: &str) {
    let root = w.v.snaps.as_ref().and_then(|s| s.form.root().map(|p| p.to_path_buf()));
    let presets = root
        .map(|r| crate::block_lib::presets(&r, &crate::block_edit::lang()))
        .unwrap_or_default();
    if let Some(p) = presets.into_iter().find(|p| p.block == block) {
        crate::block_palette::choose(p, None);
    }
}

impl FormScript {
    fn say(&mut self, what: &str, ok: bool) {
        eprintln!("[form-script] {} {what}", if ok { "OK  " } else { "FAIL" });
        if !ok {
            self.fails += 1;
        }
    }

    fn frame(&mut self, ctx: &egui::Context, v: &mut DocViewer) {
        ctx.request_repaint();
        let now = Instant::now();
        let started = *self.started.get_or_insert(now);
        if v.snaps.is_none() || now - started < Duration::from_millis(1500) || !q_idle() {
            return;
        }
        if self.plan.is_none() {
            self.original = v.ed.session.text().to_string();
            self.plan = Some(plan(&self.original));
        }
        if self.until.is_some_and(|t| now < t) {
            return;
        }
        let Some(step) = self.plan.as_mut().and_then(|p| p.pop_front()) else {
            eprintln!("[form-script] done, failures: {}", self.fails);
            std::process::exit(if self.fails == 0 { 0 } else { 1 });
        };
        let mut wait = 80;
        let rect_of = |v: &DocViewer, n: &str| {
            v.snaps
                .as_ref()?
                .form
                .native
                .as_ref()?
                .state
                .rects
                .as_ref()?
                .get(n)
                .copied()
        };
        match step {
            S::Open(name, args, n) => {
                if !v.snaps.as_ref().is_some_and(|s| s.form.is_open()) && self.open_since.is_none() {
                    let text = v.ed.session.text();
                    let off = bf::all_blocks(text)
                        .into_iter()
                        .filter(|b| b.name == name && b.args == args)
                        .nth(n)
                        .map(|b| b.range.start);
                    if off.is_none() {
                        self.say(&format!(":::{name} {args} #{n} not found"), false);
                        return;
                    }
                    v.pending_edit = off;
                    self.open_since = Some(now);
                }
                if v.snaps.as_ref().is_some_and(|s| s.form.native.is_some()) {
                    self.open_since = None;
                } else {
                    let err = v
                        .snaps
                        .as_ref()
                        .and_then(|s| s.form.error.as_ref().map(|e| e.0.clone()));
                    if self.open_since.is_some_and(|t| now - t > Duration::from_secs(3)) {
                        self.say(&format!(":::{name} {args} #{n} did not open: {err:?}"), false);
                        self.open_since = None;
                    } else {
                        self.plan.as_mut().expect("plan").push_front(S::Open(name, args, n));
                    }
                    return;
                }
            }
            S::Click(n) => match rect_of(v, &n) {
                Some(r) => {
                    // scroll it into the panel first
                    let h = ctx.content_rect().bottom() - 90.0;
                    if r.center().y > h || r.center().y < 40.0 {
                        let delta = egui::vec2(0.0, if r.center().y > h { -200.0 } else { 200.0 });
                        q_events(vec![
                            Event::PointerMoved(egui::pos2(r.center().x, 200.0)),
                            Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta,
                                phase: egui::TouchPhase::Move,
                                source: egui::MouseWheelSource::Unknown,
                                modifiers: Modifiers::NONE,
                            },
                        ]);
                        self.plan.as_mut().expect("plan").push_front(S::Click(n));
                    } else {
                        q_click(r.center());
                    }
                }
                None => self.say(&format!("no control `{n}`"), false),
            },
            S::ClickDoc => {
                let c = ctx.content_rect();
                q_click(egui::pos2(c.left() + 300.0, c.center().y));
            }
            S::Type(t) => {
                q_events(vec![Event::Text(t)]);
                wait = 0;
            }
            S::Ime(t) => {
                q_events(vec![Event::Ime(egui::ImeEvent::Preedit {
                    text: t.clone(),
                    active_range_chars: None,
                })]);
                q_events(vec![Event::Ime(egui::ImeEvent::Commit(t))]);
            }
            S::Key(k, m) => q_key(k, m),
            S::Wait(ms) => wait = ms,
            S::Check(what, f) => {
                let original = std::mem::take(&mut self.original);
                let ok = f(&mut W {
                    v,
                    ctx,
                    original: &original,
                    saved: &mut self.saved_focus,
                });
                self.original = original;
                self.say(&what, ok);
                wait = 0;
            }
            S::Do(f) => {
                let original = std::mem::take(&mut self.original);
                f(&mut W {
                    v,
                    ctx,
                    original: &original,
                    saved: &mut self.saved_focus,
                });
                self.original = original;
            }
            S::Shot(name) => {
                crate::block_snap::ProtoScript::default().shot(ctx, &name);
                wait = 150;
            }
            S::Undo => {
                v.ed.edit(|s| s.undo());
                wait = 120;
            }
        }
        self.until = Some(now + Duration::from_millis(wait));
    }
}
