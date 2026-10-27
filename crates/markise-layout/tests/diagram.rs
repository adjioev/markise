//! Diagram items in layout (spec 01 R3.5.6 / §4.6, ADR 0009): source first, the scene when it
//! arrives, notices for errors, keys covering the diagram state, scaling to the column.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use markise_blocks::diagram::{ApproxMeasure, DiagramResult, RenderCtx, render};
use markise_layout::block::{NoImages, Prim};
use markise_layout::{DiagramQuery, DiagramState, Diagrams, ItemKind, View, testing};
use markise_theme::Theme;

fn model(src: &str) -> testing::Shown {
    testing::Shown::new(Arc::new(markise_md::parse(src)))
}

/// Renders synchronously when `deliver` runs.
#[derive(Default)]
struct Fake {
    queued: Mutex<HashMap<u64, (markise_blocks::diagram::DiagramLang, String)>>,
    done: Mutex<HashMap<u64, DiagramResult>>,
    wants: AtomicU64,
    metrics: Mutex<Option<(u64, markise_blocks::diagram::TableMeasure)>>,
}

impl Fake {
    fn deliver(&self) {
        for (k, (lang, src)) in self.queued.lock().unwrap().drain() {
            let r = render(lang, &src, &RenderCtx::new(&ApproxMeasure, 14.0));
            self.done.lock().unwrap().insert(k, Arc::new(r));
        }
    }
}

impl Diagrams for Fake {
    fn generation(&self) -> u64 {
        1
    }

    fn state(&self, q: &DiagramQuery) -> DiagramState {
        match self.done.lock().unwrap().get(&q.code.key) {
            Some(r) => DiagramState::Done(r.clone()),
            None => DiagramState::Pending,
        }
    }

    fn want(&self, q: &DiagramQuery) {
        self.wants.fetch_add(1, Ordering::Relaxed);
        let src = q.source()();
        self.queued.lock().unwrap().entry(q.code.key).or_insert((q.lang, src));
    }

    fn needs_metrics(&self, fonts: u64) -> bool {
        !matches!(&*self.metrics.lock().unwrap(), Some((f, _)) if *f == fonts)
    }

    fn set_metrics(&self, fonts: u64, m: markise_blocks::diagram::TableMeasure) {
        *self.metrics.lock().unwrap() = Some((fonts, m));
    }
}

fn layout(v: &mut View, i: usize) -> Arc<markise_layout::BlockLayout> {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(1.0);
    v.layout(i, &mut f, &NoImages)
}

fn texts(l: &markise_layout::BlockLayout) -> Vec<String> {
    l.prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => text.galley().map(|g| g.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn diagram_item(m: &testing::Shown) -> usize {
    m.items
        .iter()
        .position(|i| matches!(i.kind, ItemKind::Diagram { .. }))
        .unwrap()
}

#[test]
fn r4_6_1_source_first_then_scene() {
    let m = model("intro\n\n```mermaid\ngraph TD\nA[Start] --> B[End]\n```\n");
    let i = diagram_item(&m);
    assert!(
        m.items[i].code.is_some(),
        "unsegmented diagram items carry a content key"
    );
    let fake = Arc::new(Fake::default());
    let mut v = m.view(Theme::light(), 600.0, 1.0);
    // diagrams off: source box, nothing requested
    let l = layout(&mut v, i);
    assert!(texts(&l).iter().any(|t| t.contains("A[Start]")));
    v.set_diagrams(fake.clone());
    let l = layout(&mut v, i);
    assert!(
        texts(&l).iter().any(|t| t.contains("A[Start]")),
        "pending: source shown"
    );
    assert_eq!(fake.wants.load(Ordering::Relaxed), 1);
    v.laid_out.clear();
    layout(&mut v, i);
    assert!(v.laid_out.is_empty(), "pending layout is cached");

    fake.deliver();
    let l = layout(&mut v, i);
    assert_eq!(v.laid_out, vec![i], "arrival of the scene changes the key");
    let t = texts(&l);
    assert!(
        t.contains(&"Start".to_string()) && t.contains(&"End".to_string()),
        "{t:?}"
    );
    assert!(!t.iter().any(|s| s.contains("-->")), "source not shown: {t:?}");
    assert!(
        l.prims.iter().any(|p| matches!(p, Prim::Path { .. })),
        "edge drawn as a path"
    );
    // switching off returns to the source
    v.set_diagrams(Arc::new(markise_layout::NoDiagrams));
    let l = layout(&mut v, i);
    assert!(texts(&l).iter().any(|t| t.contains("A[Start]")));
}

#[test]
fn errors_show_a_notice_above_the_source() {
    let m = model("```mermaid\nclassDiagram\nA <|-- B\n```\n");
    let i = diagram_item(&m);
    let fake = Arc::new(Fake::default());
    let mut v = m.view(Theme::light(), 600.0, 1.0);
    v.set_diagrams(fake.clone());
    layout(&mut v, i);
    fake.deliver();
    let t = texts(&layout(&mut v, i));
    assert!(t.iter().any(|s| s.contains("not supported")), "{t:?}");
    assert!(t.iter().any(|s| s.contains("classDiagram")), "{t:?}");
}

#[test]
fn wide_diagrams_scale_to_the_column() {
    let chain: String = (0..30).map(|k| format!("N{k}[Node number {k}] --> ")).collect();
    let m = model(&format!("```mermaid\ngraph LR\n{chain}End\n```\n"));
    let i = diagram_item(&m);
    let fake = Arc::new(Fake::default());
    let mut v = m.view(Theme::light(), 400.0, 1.0);
    v.set_diagrams(fake.clone());
    layout(&mut v, i);
    fake.deliver();
    let l = layout(&mut v, i);
    for p in &l.prims {
        if let Prim::Path { points, .. } = p {
            assert!(points.iter().all(|q| q.x >= -1.0 && q.x <= 401.0), "inside the column");
        }
    }
}

#[test]
fn sequence_and_flow_render() {
    for (lang, src) in [
        ("sequence", "A->B: hi\n"),
        ("flow", "st=>start: S\ne=>end: E\nst->e\n"),
        ("mermaid", "sequenceDiagram\nA->>B: hi\n"),
    ] {
        let m = model(&format!("```{lang}\n{src}```\n"));
        let i = diagram_item(&m);
        let fake = Arc::new(Fake::default());
        let mut v = m.view(Theme::dark(), 600.0, 1.0);
        v.set_diagrams(fake.clone());
        layout(&mut v, i);
        fake.deliver();
        let l = layout(&mut v, i);
        assert!(l.prims.iter().any(|p| matches!(p, Prim::Path { .. })), "{lang}");
    }
}

/// DG-6: layout measures label metrics with its own fonts once per theme/font change.
#[test]
fn label_metrics_come_from_the_layout_fonts() {
    use markise_blocks::diagram::TextMeasure;
    let m = model("```mermaid\ngraph TD\nA-->B\n```\n\n```mermaid\ngraph TD\nC-->D\n```\n");
    let fake = Arc::new(Fake::default());
    let mut v = m.view(Theme::light(), 600.0, 1.0);
    v.set_diagrams(fake.clone());
    let items: Vec<usize> = m
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i.kind, ItemKind::Diagram { .. }))
        .map(|(k, _)| k)
        .collect();
    layout(&mut v, items[0]);
    let first = fake
        .metrics
        .lock()
        .unwrap()
        .as_ref()
        .map(|(f, t)| (*f, t.id()))
        .expect("measured");
    layout(&mut v, items[1]);
    let again = fake
        .metrics
        .lock()
        .unwrap()
        .as_ref()
        .map(|(f, t)| (*f, t.id()))
        .unwrap();
    assert_eq!(first, again, "measured once per font set");
    let guard = fake.metrics.lock().unwrap();
    let t = &guard.as_ref().unwrap().1;
    assert!(
        t.width("WWW", 14.0, false) > t.width("iii", 14.0, false),
        "proportional body font"
    );
    assert!(t.width("abc", 14.0, true) >= t.width("abc", 14.0, false) * 0.95);
}

/// Counts shaping calls and the bytes shaped (wraps the test epaint shaper).
struct Counting<'a> {
    inner: &'a mut dyn markise_layout::TextShaper,
    calls: usize,
    bytes: usize,
}

impl markise_layout::TextShaper for Counting<'_> {
    fn shape(&mut self, text: &markise_layout::StyledText, width: Option<f32>) -> Arc<dyn markise_layout::TextBlock> {
        self.calls += 1;
        self.bytes += text.text.len();
        self.inner.shape(text, width)
    }
}

/// Codex round 2: a large diagram fence shown as source (off / pending / error) stays
/// segmented like code — laying out the visible item shapes one segment, not the whole fence —
/// while a rendered scene is drawn by the first segment alone.
#[test]
fn large_fences_keep_segmented_source_fallback() {
    let body: String = (0..2000)
        .map(|k| format!("%% unique comment line number {k}\n"))
        .collect();
    for (name, head) in [
        ("off", "graph TD\nA-->B\n"),
        ("pending", "graph TD\nA-->B\n"),
        ("error", "classDiagram\nA <|-- B\n"),
        ("ready", "graph TD\nA-->B\n"),
    ] {
        let m = model(&format!("```mermaid\n{head}{body}```\n"));
        let items: Vec<usize> = m
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i.kind, ItemKind::Diagram { .. }))
            .map(|(k, _)| k)
            .collect();
        assert!(items.len() > 10, "{name}: segmented ({} items)", items.len());
        assert!(
            items
                .iter()
                .all(|&i| m.items[i].code.map(|c| c.key) == m.items[items[0]].code.map(|c| c.key)),
            "one content key"
        );
        let fake = Arc::new(Fake::default());
        let mut v = m.view(Theme::light(), 600.0, 1.0);
        if name != "off" {
            v.set_diagrams(fake.clone());
            // first layout requests the scene; deliver for error / ready
            layout(&mut v, items[0]);
            if name != "pending" {
                fake.deliver();
            }
        }
        let mut fonts = testing::fonts();
        let mut f = fonts.with_pixels_per_point(1.0);
        let mut c = Counting {
            inner: &mut f,
            calls: 0,
            bytes: 0,
        };
        let l = v.layout(items[1], &mut c, &markise_layout::block::NoImages);
        match name {
            "ready" => {
                assert_eq!(l.height, 0.0, "later segments are empty when the scene is shown");
                let first = v.layout(items[0], &mut c, &markise_layout::block::NoImages);
                assert!(
                    first.prims.iter().any(|p| matches!(p, Prim::Path { .. })),
                    "first segment draws the scene"
                );
            }
            _ => {
                assert!(c.calls <= 4, "{name}: {} shaping calls for one segment", c.calls);
                assert!(
                    c.bytes < body.len() / 8,
                    "{name}: shaped {} of {} bytes",
                    c.bytes,
                    body.len()
                );
                assert!(l.height > 0.0);
            }
        }
    }
}

/// Codex round 3: editing a label inside a later segment of a segmented fence (the fenced-code
/// fast path) gives every segment the new whole-block key, requests a new scene, and the
/// unchanged first segment shows it once ready (its layout key includes the block key).
#[test]
fn editing_a_later_segment_rerenders_the_whole_diagram() {
    use markise_doc::{FormatDoc, PatchSet};
    let nodes: String = (0..200).map(|k| format!("N{k}[Label {k}]\n")).collect();
    let mut doc = markise_md::parse(&format!("```mermaid\ngraph TD\n{nodes}```\n"));
    let mut m = markise_layout::DocModel::build_owned(&doc, None);
    let items = |m: &markise_layout::DocModel| -> Vec<usize> {
        m.items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i.kind, ItemKind::Diagram { .. }))
            .map(|(k, _)| k)
            .collect()
    };
    let ds = items(&m);
    assert!(ds.len() >= 3, "segmented: {}", ds.len());
    let old_key = m.items[ds[0]].code.unwrap().key;
    let fake = Arc::new(Fake::default());
    let mut v = View::new(Arc::new(doc.clone()), Arc::new(m.clone()), Theme::light(), 600.0, 1.0);
    v.set_diagrams(fake.clone());
    layout(&mut v, ds[0]);
    fake.deliver();
    let t = texts(&layout(&mut v, ds[0]));
    assert!(t.contains(&"Label 150".to_string()));

    // rename a node whose line lives in a later segment
    let at = doc.text().find("Label 150").unwrap();
    let e = doc.apply(&PatchSet::single_verbatim(at..at + "Label".len(), "Renamed"));
    let ch = m.apply(&doc, &e);
    m.diff(&markise_layout::DocModel::build_owned(&doc, None)).unwrap();
    let ds2 = items(&m);
    let new_key = m.items[ds2[0]].code.unwrap().key;
    assert_ne!(new_key, old_key, "new whole-block key");
    assert!(
        ds2.iter().all(|&i| m.items[i].code.unwrap().key == new_key),
        "on every segment"
    );
    v.apply(Arc::new(doc.clone()), Arc::new(m.clone()), &ch);

    let wants = fake.wants.load(Ordering::Relaxed);
    let t = texts(&layout(&mut v, ds2[0]));
    assert!(fake.wants.load(Ordering::Relaxed) > wants, "new scene requested");
    assert!(
        t.iter().any(|s| s.contains("N0[Label 0]")),
        "pending: the segment's source shown"
    );
    fake.deliver();
    let t = texts(&layout(&mut v, ds2[0]));
    assert!(
        t.contains(&"Renamed 150".to_string()),
        "first segment shows the new scene: {:?}",
        &t[..t.len().min(5)]
    );
    assert!(!t.contains(&"Label 150".to_string()));
}
