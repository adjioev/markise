//! Practice on a large document, headless through the editing surface (ADR 0024): heights and paging.

use std::collections::BTreeMap;
use std::sync::Arc;

use markise_doc::DocumentFormat;
use markise_engine::block_view::PackageViews;
use markise_engine::editor::EditorView;
use markise_layout::block::NoImages;
use markise_layout::{BlockViews, testing};
use markise_md::MarkdownFormat;
use markise_package::PackageIndex;
use markise_theme::Theme;

const VH: f32 = 900.0;

pub fn views() -> Arc<PackageViews> {
    let mut files = BTreeMap::new();
    let mut put = |path: &str, text: &str| files.insert(path.to_string(), text.as_bytes().to_vec());
    put(
        "manifest.json",
        r#"{"format":"markise-block-package/2","library":"sample","version":"1","blocks":["question"]}"#,
    );
    put(
        "components/question/question.json",
        r#"{"label": "Question", "fields": [
            {"name": "prompt", "type": "text"},
            {"name": "answers", "type": "choices", "minItems": 2, "correct": "exactly-one"},
            {"name": "explanation", "type": "quote", "min": 0}]}"#,
    );
    put("editor/palette.json", r#"{"format": 1}"#);
    put(
        "editor/views.json",
        r#"{"question": {"kind": "choices", "practice": {"reveal": ["explanation"]}}}"#,
    );
    let index = PackageIndex::from_files(&files).expect("a valid package");
    Arc::new(PackageViews::new(Arc::new(index), "en"))
}

pub fn questions(n: usize) -> String {
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&format!(
            "A paragraph between the blocks, number {i}.\n\n:::question\n\nThe prompt of question {i}?\n\n- [ ] One answer {{#a1}}\n- [x] Another answer {{#a2}}\n\n> The explanation of question {i}, long enough to take a line or two of the column here when it shows at last.\n\n:::\n\n"
        ));
    }
    out
}

pub fn editor(text: String) -> EditorView {
    let fields = views();
    let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
    let mut ed = EditorView::open(format, text, Theme::light(), 700.0);
    ed.view.set_block_views(Some(fields.clone() as Arc<dyn BlockViews>));
    ed.set_block_fields(Some(fields));
    ed
}

/// The document's height once every part of it has been laid out.
fn measured_total(ed: &mut EditorView) -> f64 {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(2.0);
    ed.view.scroll_to_y(0.0);
    loop {
        ed.view.frame(VH, 0.0, &mut f, &NoImages);
        let before = ed.view.scroll_y();
        ed.view.scroll_by(VH);
        if ed.view.scroll_y() + (VH as f64) >= ed.view.total_height() || ed.view.scroll_y() <= before {
            break;
        }
    }
    ed.view.frame(VH, 0.0, &mut f, &NoImages);
    ed.view.total_height()
}

#[test]
fn a_page_in_a_document_in_practice_is_estimated_as_what_it_shows() {
    let mut ed = editor(questions(1500));
    assert!(ed.view.model().is_paged(), "this has to be about pages");
    ed.toggle_practice_all();
    let estimated = ed.view.total_height();
    let measured = measured_total(&mut ed);
    let off = (estimated - measured).abs() / measured;
    assert!(off < 0.15, "estimated {estimated}, measured {measured}");
}

fn frame(ed: &mut EditorView) {
    let mut fonts = testing::fonts();
    let mut f = fonts.with_pixels_per_point(2.0);
    ed.view.frame(VH, VH * 0.5, &mut f, &NoImages);
}

#[test]
fn what_was_done_lasts_through_scrolling_paging_and_eviction_and_heights_follow() {
    let text = questions(1500);
    let mut ed = editor(text.clone());
    ed.toggle_practice_all();
    frame(&mut ed);
    let answer = text.find("Another answer").unwrap();
    let opener = text.find(":::question").unwrap();
    let explanation = text.find("The explanation").unwrap();
    assert!(ed.practice_press(answer));
    frame(&mut ed);
    let closed = ed.view.total_height();
    let at_top = ed.view.scroll_y();
    assert!(ed.practice_check(opener));
    frame(&mut ed);
    let open = ed.view.total_height();
    let grown = open - closed;
    assert!(grown > 20.0, "{closed} to {open}");
    assert_eq!(ed.view.scroll_y(), at_top, "what is on screen stays where it is");
    assert!(!ed.view.practice_conceals(explanation));

    // far enough for the page to be evicted, and back
    let evicted_before = ed.view.paging_stats().evicted;
    ed.view.scroll_to_y(ed.view.total_height() * 0.8);
    for _ in 0..5 {
        frame(&mut ed);
        ed.view.scroll_by(VH);
    }
    assert!(ed.view.paging_stats().evicted > evicted_before, "its page went");
    assert!(ed.practice_show(opener).is_some_and(|s| s.checked));
    assert!(
        (ed.view.total_height() - open).abs() < open * 0.05,
        "a measured page keeps what it showed"
    );
    ed.view.scroll_to_y(0.0);
    frame(&mut ed);
    assert!(ed.practice_show(opener).is_some_and(|s| s.checked && s.selected == [1]));
    assert!(!ed.view.practice_conceals(explanation), "still revealed");
    let top = ed.view.total_height();

    // Reset takes off what Check added, on a block on screen, and moves nothing above it
    ed.practice_reset(opener);
    frame(&mut ed);
    assert!(ed.view.practice_conceals(explanation));
    assert!((top - ed.view.total_height() - grown).abs() < 1.0);
    assert_eq!(ed.session.text(), text);
    assert!(!ed.session.is_dirty() && !ed.session.history().can_undo());
}

#[test]
fn a_hidden_field_at_the_top_of_the_viewport_leaves_the_viewport_on_what_comes_next() {
    let text = questions(40);
    let mut ed = editor(text.clone());
    ed.toggle_practice_all();
    frame(&mut ed);
    let opener = text.find(":::question").unwrap();
    assert!(ed.practice_press(text.find("Another answer").unwrap()));
    assert!(ed.practice_check(opener));
    frame(&mut ed);
    // the viewport starts inside the revealed text
    let i = ed.view.model().item_at_offset(text.find("The explanation").unwrap());
    ed.view.scroll_to_item(i);
    ed.view.scroll_by(5.0);
    frame(&mut ed);
    ed.practice_reset(opener);
    frame(&mut ed);
    let total = ed.view.total_height();
    let y = ed.view.scroll_y();
    assert!(y >= 0.0 && y <= total, "{y} of {total}");
    let (anchor, off) = ed.view.anchor();
    assert!(ed.view.height(anchor) >= off, "{off} into {}", ed.view.height(anchor));
}
