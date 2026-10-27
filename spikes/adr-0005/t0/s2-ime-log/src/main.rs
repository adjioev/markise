//! Spike S2 (ADR 0005 §4.4, §6): IME on egui 0.37 with a parley-laid-out paragraph.
//!
//! `s2 window [log-file]` — eframe window with a one-paragraph editor drawn through markise-text
//!   + the glyph atlas. Every `egui::Event::Ime`, key press, `Text` and focus event is logged
//!   with frame number and time (stdout and the log file) — this is the recording tool for the
//!   real-IME sequences (macOS Kotoeri Japanese, Pinyin, Korean). The preedit is shown inline
//!   (underlined, its own style run), the selection it replaces is suppressed until commit, the
//!   candidate window is placed via `IMEOutput { rect, cursor_rect }` at the caret; commit is one
//!   undo step (Cmd+Z), cancel restores the display. State machine per ADR 0005 §4.4.
//! `s2 selftest` — headless: the §4.4 injected-event tests through a real `egui::Context`.

#[path = "../../s1-theme-font/src/paint.rs"]
mod paint;

use std::io::Write;
use std::ops::Range;
use std::time::Instant;

use egui::{Color32, Event, ImeEvent, Key, Pos2, Rect, Stroke, pos2, vec2};
use markise_text::{Family, FontStore, LocalPos, StyledText, TextEngine, TextLayout, TextStyle};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Composing,
    Ending,
}

#[derive(Clone, Debug)]
struct Composition {
    anchor: usize,
    replaced: Option<Range<usize>>,
    preedit: String,
    /// Active range inside the preedit, bytes (egui 0.37 `active_range_chars`).
    cursor: Option<Range<usize>>,
    phase: Phase,
}

struct Editor {
    text: String,
    caret: usize,
    anchor: Option<usize>,
    comp: Option<Composition>,
    undo: Vec<(String, usize)>,
    log: Vec<String>,
}

impl Editor {
    fn new(text: &str) -> Self {
        Editor {
            text: text.into(),
            caret: text.len(),
            anchor: None,
            comp: None,
            undo: Vec::new(),
            log: Vec::new(),
        }
    }

    fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor?;
        (a != self.caret).then(|| a.min(self.caret)..a.max(self.caret))
    }

    fn commit(&mut self, s: &str) {
        let (range, at) = match self.comp.take() {
            Some(c) => (c.replaced.clone(), c.anchor),
            None => (self.selection(), self.caret),
        };
        self.undo.push((self.text.clone(), self.caret));
        match range {
            Some(r) => {
                self.text.replace_range(r.clone(), s);
                self.caret = r.start + s.len();
            }
            None => {
                self.text.insert_str(at, s);
                self.caret = at + s.len();
            }
        }
        self.anchor = None;
    }

    fn cancel(&mut self, why: &str) {
        if self.comp.take().is_some() {
            self.log.push(format!(
                "  -> composition cancelled ({why}); selection display restored"
            ));
        }
    }

    /// Applies one frame's events (ADR 0005 §4.4). `layout` is the previous frame's layout.
    fn handle(&mut self, events: &[Event], layout: Option<&TextLayout>) {
        let mut ime_seen = false;
        for e in events {
            match e {
                Event::Ime(ImeEvent::Preedit {
                    text,
                    active_range_chars,
                }) => {
                    ime_seen = true;
                    if text.is_empty() {
                        if let Some(c) = &mut self.comp {
                            c.phase = Phase::Ending;
                            c.preedit.clear();
                            c.cursor = None;
                        }
                    } else {
                        let sel = self.selection();
                        let c = self.comp.get_or_insert_with(|| Composition {
                            anchor: sel.as_ref().map_or(self.caret, |r| r.start),
                            replaced: sel,
                            preedit: String::new(),
                            cursor: None,
                            phase: Phase::Composing,
                        });
                        c.phase = Phase::Composing;
                        c.preedit = text.clone();
                        c.cursor = active_range_chars.as_ref().map(|r| {
                            let b = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
                            b(r.start)..b(r.end)
                        });
                    }
                }
                Event::Ime(ImeEvent::Commit(s)) => {
                    ime_seen = true;
                    self.commit(s);
                }
                Event::WindowFocused(false) => self.cancel("focus lost"),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if self.comp.as_ref().is_some_and(|c| c.phase == Phase::Composing) {
                        continue; // consumed by the IME
                    }
                    if self.comp.as_ref().is_some_and(|c| c.phase == Phase::Ending) {
                        self.cancel("key after empty preedit");
                    }
                    let Some(l) = layout else { continue };
                    match key {
                        Key::ArrowLeft | Key::ArrowRight => {
                            if modifiers.shift {
                                self.anchor.get_or_insert(self.caret);
                            } else {
                                self.anchor = None;
                            }
                            let p = LocalPos::down(self.caret);
                            self.caret = if *key == Key::ArrowLeft {
                                l.move_left(p)
                            } else {
                                l.move_right(p)
                            }
                            .byte;
                        }
                        Key::Z if modifiers.command => {
                            if let Some((t, c)) = self.undo.pop() {
                                self.text = t;
                                self.caret = c;
                                self.anchor = None;
                            }
                        }
                        Key::Backspace if self.comp.is_none() => {
                            if let Some(r) = self.selection() {
                                self.undo.push((self.text.clone(), self.caret));
                                self.text.replace_range(r.clone(), "");
                                self.caret = r.start;
                                self.anchor = None;
                            } else if self.caret > 0 {
                                let p = l.prev_logical(LocalPos::down(self.caret)).byte;
                                self.undo.push((self.text.clone(), self.caret));
                                self.text.replace_range(p..self.caret, "");
                                self.caret = p;
                            }
                        }
                        _ => {}
                    }
                }
                Event::Text(t) if self.comp.is_none() => self.commit(t),
                Event::PointerButton { pressed: true, .. } => self.cancel("pointer"),
                _ => {}
            }
        }
        if !ime_seen && self.comp.as_ref().is_some_and(|c| c.phase == Phase::Ending) {
            self.cancel("frame without IME event after empty preedit");
        }
    }

    /// Display text: `replaced` suppressed, preedit inserted at the anchor. Returns the styled
    /// text, the preedit's display range and a display→caret mapping for the caret.
    fn display(&self, size: f32) -> (StyledText, Option<Range<usize>>, usize) {
        let fam = Family::parse_list("Hiragino Sans, sans-serif");
        let base = TextStyle::new(fam.clone(), size);
        match &self.comp {
            None => (StyledText::plain(self.text.clone(), base), None, self.caret),
            Some(c) => {
                let (pre, post) = match &c.replaced {
                    Some(r) => (&self.text[..r.start], &self.text[r.end..]),
                    None => (&self.text[..c.anchor], &self.text[c.anchor..]),
                };
                let s = format!("{pre}{}{post}", c.preedit);
                let pr = pre.len()..pre.len() + c.preedit.len();
                let mut t = StyledText::plain(s, base);
                let mut st = TextStyle::new(fam, size);
                st.underline = true;
                if !pr.is_empty() {
                    t.push_run(pr.clone(), st);
                }
                let caret = pr.start + c.cursor.as_ref().map_or(c.preedit.len(), |r| r.end);
                (t, Some(pr), caret)
            }
        }
    }
}

struct App {
    engine: TextEngine,
    painter: paint::TextPainter,
    ed: Editor,
    layout: Option<TextLayout>,
    frame: u64,
    start: Instant,
    file: Option<std::fs::File>,
}

impl App {
    fn log_events(&mut self, events: &[Event]) {
        for e in events {
            let keep = matches!(e, Event::Ime(_) | Event::Text(_) | Event::WindowFocused(_))
                || matches!(e, Event::Key { pressed: true, .. });
            if keep {
                let line = format!(
                    "frame {:5} t {:8.1} ms  {:?}",
                    self.frame,
                    self.start.elapsed().as_secs_f64() * 1e3,
                    e
                );
                println!("{line}");
                if let Some(f) = &mut self.file {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        for l in self.ed.log.drain(..) {
            println!("{l}");
            if let Some(f) = &mut self.file {
                let _ = writeln!(f, "{l}");
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.frame += 1;
        self.engine.fonts.poll();
        let events = ui.input(|i| i.events.clone());
        self.ed.handle(&events, self.layout.as_ref());
        self.log_events(&events);
        let ppp = ui.ctx().pixels_per_point();
        let (styled, pre, caret) = self.ed.display(22.0);
        let l = self.engine.layout(&styled, Some(560.0), ppp);
        ui.label("S2 — type with an IME (Kotoeri, Pinyin, Korean); events are logged. Cmd+Z undoes a commit.");
        let (w, h) = l.size();
        let (rect, _) = ui.allocate_exact_size(vec2(560.0, h / ppp + 8.0), egui::Sense::click());
        let origin = rect.min;
        let to_pt = |r: markise_text::Rect| {
            Rect::from_min_max(
                pos2(origin.x + r.x0 / ppp, origin.y + r.y0 / ppp),
                pos2(origin.x + r.x1 / ppp, origin.y + r.y1 / ppp),
            )
        };
        if self.ed.comp.is_none()
            && let Some(sel) = self.ed.selection()
        {
            for r in l.selection_rects(LocalPos::down(sel.start), LocalPos::down(sel.end)) {
                ui.painter()
                    .rect_filled(to_pt(r), 0.0, Color32::from_rgb(180, 210, 255));
            }
        }
        if let Some(pr) = &pre
            && let Some(c) = &self.ed.comp
            && let Some(cr) = &c.cursor
        {
            for r in l.selection_rects(LocalPos::down(pr.start + cr.start), LocalPos::down(pr.start + cr.end)) {
                ui.painter()
                    .rect_filled(to_pt(r), 0.0, Color32::from_rgb(255, 230, 160));
            }
        }
        self.painter
            .paint(ui.ctx(), ui.painter(), &l, origin, ppp, ui.visuals().text_color());
        let cr = to_pt(l.caret_rect(LocalPos::down(caret)));
        ui.painter()
            .line_segment([cr.min, cr.max], Stroke::new(1.5, ui.visuals().text_color()));
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: Default::default(),
                rect: Rect::from_min_size(origin, vec2(w / ppp, h / ppp)),
                cursor_rect: Rect::from_min_size(cr.min, vec2(1.0, cr.height())),
                should_interrupt_composition: false,
            })
        });
        ui.label(format!(
            "text: {:?}  caret {}  comp {:?}  undo {}",
            self.ed.text,
            self.ed.caret,
            self.ed.comp.as_ref().map(|c| (&c.preedit, c.phase)),
            self.ed.undo.len()
        ));
        self.layout = Some(l);
        let _ = Pos2::ZERO;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.ui(ui));
    }
}

fn new_app(log: Option<&str>) -> App {
    let mut store = FontStore::new();
    store.start_system_discovery();
    store.wait_for_system(std::time::Duration::from_secs(30));
    App {
        engine: TextEngine::new(store),
        painter: paint::TextPainter::default(),
        ed: Editor::new("日本語の文章を入力します。Select text, then type with the IME."),
        layout: None,
        frame: 0,
        start: Instant::now(),
        file: log.map(|p| std::fs::File::create(p).expect("log file")),
    }
}

fn selftest() {
    let ctx = egui::Context::default();
    let mut app = new_app(None);
    let run = |app: &mut App, events: Vec<Event>| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 300.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.ui(ui));
    };
    let pre = |t: &str| {
        Event::Ime(ImeEvent::Preedit {
            text: t.into(),
            active_range_chars: Some(0..t.chars().count()),
        })
    };
    let commit = |t: &str| Event::Ime(ImeEvent::Commit(t.into()));
    let mut results = Vec::new();
    let original = app.ed.text.clone();
    let sel = original.find("文章").unwrap();
    let select = |app: &mut App| {
        app.ed.text = original.clone();
        app.ed.anchor = Some(sel);
        app.ed.caret = sel + "文章".len();
        app.ed.comp = None;
        app.ed.undo.clear();
    };
    // 1. selection replaced by the commit in one undo step
    select(&mut app);
    run(&mut app, vec![]);
    run(&mut app, vec![pre("か")]);
    let suppressed = app.ed.display(22.0).0.text.contains("文章");
    run(&mut app, vec![pre(""), commit("家")]);
    let ok1 = app.ed.text == original.replace("文章", "家") && app.ed.undo.len() == 1 && !suppressed;
    run(
        &mut app,
        vec![Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
    );
    results.push((
        "Preedit(か) → [Preedit(\"\"), Commit(家)] with selection: replaced in one undo step; Cmd+Z restores",
        ok1 && app.ed.text == original,
    ));
    // 2. clearing then a frame without IME events → cancel, selection intact
    select(&mut app);
    run(&mut app, vec![pre("か")]);
    run(&mut app, vec![pre("")]);
    let ending = app.ed.comp.as_ref().map(|c| c.phase);
    run(&mut app, vec![]);
    results.push((
        "Preedit → Preedit(\"\") → frame without IME events: cancel, text and selection intact",
        ending == Some(Phase::Ending)
            && app.ed.comp.is_none()
            && app.ed.text == original
            && app.ed.selection() == Some(sel..sel + "文章".len()),
    ));
    // 3. Ending then Preedit(x) resumes
    select(&mut app);
    run(&mut app, vec![pre("か")]);
    run(&mut app, vec![pre(""), pre("かな")]);
    let resumed = app
        .ed
        .comp
        .as_ref()
        .is_some_and(|c| c.phase == Phase::Composing && c.preedit == "かな");
    run(&mut app, vec![commit("仮名")]);
    results.push((
        "Ending then Preedit(かな) resumes; Commit replaces the selection",
        resumed && app.ed.text == original.replace("文章", "仮名"),
    ));
    // 4. focus loss while composing → cancel
    select(&mut app);
    run(&mut app, vec![pre("か")]);
    run(&mut app, vec![Event::WindowFocused(false)]);
    results.push((
        "focus loss while composing: cancel, no edit",
        app.ed.comp.is_none() && app.ed.text == original,
    ));
    // 5. active range → caret inside the preedit
    select(&mut app);
    run(
        &mut app,
        vec![Event::Ime(ImeEvent::Preedit {
            text: "にほんご".into(),
            active_range_chars: Some(0..2),
        })],
    );
    let (_, pr, caret) = app.ed.display(22.0);
    results.push((
        "active_range_chars 0..2 → caret after 2 preedit chars",
        pr.is_some_and(|r| caret == r.start + "にほ".len()),
    ));
    // 6. IMEOutput is set every composing frame at the caret
    let o = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 300.0))),
            ..Default::default()
        },
        |ui| app.ui(ui),
    );
    results.push((
        "IMEOutput set with a cursor rect while composing",
        o.platform_output.ime.is_some_and(|i| i.cursor_rect.height() > 5.0),
    ));
    let mut pass = 0;
    for (name, ok) in &results {
        println!("{} {name}", if *ok { "PASS" } else { "FAIL" });
        pass += usize::from(*ok);
    }
    println!("S2 selftest: {pass}/{} passed", results.len());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("selftest") => selftest(),
        Some("window") => {
            let log = args.get(2).cloned();
            eframe::run_native(
                "markise S2 IME",
                eframe::NativeOptions {
                    viewport: egui::ViewportBuilder::default().with_inner_size([660.0, 320.0]),
                    ..Default::default()
                },
                Box::new(move |_cc| Ok(Box::new(new_app(log.as_deref())))),
            )
            .unwrap();
        }
        _ => eprintln!("usage: s2 window [log-file] | selftest"),
    }
}
