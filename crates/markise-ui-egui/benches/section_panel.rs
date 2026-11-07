//! Dragging a section's row in the outline on a large document (ADR 0026), headless, through the real app UI on a
//! plain `egui::Context` (layout, paint, tessellation; no GPU): a frame with the panel at rest, a frame of a drag
//! with the pointer moving and the list scrolling at its edge, and a far drop and its undo.
//!
//! Run: `cargo bench -p markise-ui-egui --bench section_panel` (`MARKISE_SECTION_MB`, default 10).

use std::time::{Duration, Instant};

use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use markise_ui_egui::{AppOptions, MarkiseApp};

const W: f32 = 1200.0;
const H: f32 = 900.0;

struct Run {
    ctx: Context,
    app: MarkiseApp,
    events: Vec<Event>,
    labels: Vec<(String, Rect)>,
}

impl Run {
    fn frame(&mut self) -> Duration {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(W, H))),
            focused: true,
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let app = &mut self.app;
        let t = Instant::now();
        let full = self.ctx.run_ui(raw, |ui| app.ui(ui));
        let _ = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        let spent = t.elapsed();
        self.labels.clear();
        if let Some(update) = &full.platform_output.accesskit_update {
            for (_, n) in &update.nodes {
                if let (Some(l), Some(b)) = (n.label(), n.bounds()) {
                    let min = Pos2::new(b.x0 as f32, b.y0 as f32);
                    let max = Pos2::new(b.x1 as f32, b.y1 as f32);
                    self.labels.push((l.to_string(), Rect::from_min_max(min, max)));
                }
            }
        }
        spent
    }

    fn at(&self, label: &str) -> Option<Rect> {
        (self.labels.iter().filter(|(l, _)| l == label).map(|(_, r)| *r))
            .min_by(|a, b| (a.left(), a.top()).partial_cmp(&(b.left(), b.top())).unwrap())
    }

    fn hover(&mut self, p: Pos2) {
        self.events.push(Event::PointerMoved(p));
        self.frame();
        self.frame();
    }

    fn button(&mut self, p: Pos2, pressed: bool) -> Duration {
        self.events.push(Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        self.frame()
    }
}

fn report(name: &str, mut v: Vec<Duration>) {
    v.sort();
    let ms = |x: f64| v[((v.len() as f64 - 1.0) * x).round() as usize].as_secs_f64() * 1e3;
    println!(
        "  {name:<28} n={:<4} p50 {:>8.3}  p99 {:>8.3}  max {:>8.3} ms",
        v.len(),
        ms(0.5),
        ms(0.99),
        ms(1.0)
    );
}

fn main() {
    if std::env::args().any(|a| a == "--list" || a == "--test") {
        return;
    }
    let mb: usize = (std::env::var("MARKISE_SECTION_MB").ok().and_then(|s| s.parse().ok())).unwrap_or(10);
    let block = "---\n\n:::section-metadata\n\nid: part-N\nkind: reading\n\n:::\n\n## A part\n\nSome text of a paragraph that goes on for a while, as text does.\n\n";
    let filler = "Some text of a paragraph that goes on for a while, as text does.\n\n".repeat(30);
    let n = (mb << 20) / (block.len() + filler.len());
    let body: String = (0..n)
        .map(|i| format!("{}{filler}", block.replace('N', &i.to_string())))
        .collect();
    let text = format!("---\nauthoring: blocks-v3\n---\n\n# Title\n\nIntro.\n\n{body}");
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("doc.md");
    std::fs::write(&doc, &text).unwrap();
    let opts = AppOptions {
        config: markise_ui_egui::ConfigDir::Dir(dir.path().join("config")),
        ..AppOptions::default()
    };
    let ctx = Context::default();
    ctx.enable_accesskit();
    let mut r = Run {
        ctx,
        app: MarkiseApp::new(opts),
        events: vec![],
        labels: vec![],
    };
    r.frame();
    r.app.open(doc);
    let t = Instant::now();
    while (r.app.is_loading() || r.app.is_partial()) && t.elapsed() < Duration::from_secs(120) {
        r.frame();
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..20 {
        r.frame();
    }
    let sections = r.app.viewer().unwrap().ed.session.doc().section_count();
    println!("{} KiB, {sections} sections", text.len() >> 10);

    // typing: the frame after each key, with the outline shown and without
    for (name, files) in [("a key, the outline hidden", true), ("a key, the outline shown", false)] {
        r.app.show_files_tab(files);
        r.frame();
        let mut typing = Vec::new();
        for _ in 0..40 {
            let v = r.app.viewer().unwrap();
            let at = v.ed.session.text().find("Intro").unwrap();
            v.ed.set_selection(markise_doc::Selection::caret_at(markise_doc::DocPos::new(at)));
            let t = Instant::now();
            let _ = v.ed.edit(|s| s.insert_text("x"));
            let spent = t.elapsed();
            typing.push(spent + r.frame());
        }
        report(name, typing);
        for _ in 0..40 {
            let _ = r.app.viewer().unwrap().ed.edit(|s| s.undo());
        }
        r.frame();
    }

    let row = r.at("part-0").expect("a row").center();
    r.hover(Pos2::new(row.x, H - 300.0));
    report("panel at rest", (0..60).map(|_| r.frame()).collect());
    r.hover(row);
    let grip = r.at("Drag Section to move").expect("a grip").center();
    r.hover(grip);
    let pressed = r.button(grip, true);
    report("the press", vec![pressed]);
    let mut moving = Vec::new();
    for i in 0..120 {
        r.events.push(Event::PointerMoved(grip + vec2(0.0, (i * 5) as f32)));
        moving.push(r.frame());
    }
    report("drag, the pointer moving", moving);
    let bottom = Pos2::new(grip.x, H - 60.0);
    let mut edge = Vec::new();
    for _ in 0..240 {
        r.events.push(Event::PointerMoved(bottom));
        edge.push(r.frame());
    }
    report("drag, the list scrolling", edge);
    let shown = r.app.section_drag_shown().cloned().expect("a drag");
    println!("  (went to {} of {}, {} rows)", shown.to, sections, shown.rows);
    let v = r.app.viewer().unwrap();
    let t = Instant::now();
    let title = v.ed.only_title_section();
    report("the title's walk", vec![t.elapsed()]);
    assert_eq!(title, Some(0));
    let dropped = r.button(bottom, false);
    report("the drop", vec![dropped, r.frame()]);
    let v = r.app.viewer().unwrap();
    assert_eq!(v.ed.session.doc().section_count(), sections);
    assert!(v.ed.session.text() != text, "it moved");
    let t = Instant::now();
    let _ = v.ed.edit(|s| s.undo());
    report("its undo", vec![t.elapsed()]);
    assert!(v.ed.session.text() == text, "exact");
    // the whole way, past every section
    let t = Instant::now();
    assert_eq!(v.ed.move_section_to(1, sections, false), Ok(true));
    report("a move past all of them", vec![t.elapsed()]);
    let t = Instant::now();
    let _ = v.ed.edit(|s| s.undo());
    report("its undo", vec![t.elapsed()]);
    assert!(v.ed.session.text() == text, "exact");
}
