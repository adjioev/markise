//! HTML and export-model output of CriticMarkup review marks (ADR 0012 §5).

use markise_export::model::{ExBlock, ExDoc, ExInline, ModelOptions};
use markise_export::{CommentExport, HtmlOptions, render_body};
use markise_md::{Document, ParseOptions};

const SRC: &str = "A {++new++} {--old--} {~~x~>y~~} {==this==}{>>@ai: \"why\" <b>?<<} end{>>note<<}.\n";

fn html(comments: CommentExport, critic: bool) -> String {
    let doc = Document::parse(
        SRC,
        ParseOptions {
            critic_markup: critic,
            ..Default::default()
        },
    );
    render_body(
        &doc,
        &HtmlOptions {
            comments,
            ..Default::default()
        },
    )
}

#[test]
fn suggestions_are_ins_and_del_and_comments_kept_by_default() {
    let h = html(CommentExport::default(), true);
    assert!(h.contains("<ins class=\"critic\">new</ins>"), "{h}");
    assert!(h.contains("<del class=\"critic\">old</del>"), "{h}");
    assert!(
        h.contains("<del class=\"critic\">x</del><ins class=\"critic\">y</ins>"),
        "{h}"
    );
    // the attached comment is the highlight's title (escaped) and shown after it
    assert!(
        h.contains("<mark class=\"critic\" title=\"@ai: &quot;why&quot; &lt;b&gt;?\">this</mark>"),
        "{h}"
    );
    assert!(h.contains("<span class=\"critic comment\">note</span>"), "{h}");
    assert!(!h.contains("{++") && !h.contains("<b>"), "{h}");
}

#[test]
fn comments_can_be_stripped() {
    let h = html(CommentExport::Strip, true);
    assert!(h.contains("<ins class=\"critic\">new</ins>"), "{h}");
    assert!(
        !h.contains("critic comment") && !h.contains("<mark") && !h.contains("note"),
        "{h}"
    );
    assert!(h.contains(" this end."), "{h}");
}

#[test]
fn off_the_marks_are_text() {
    let h = html(CommentExport::Keep, false);
    assert!(h.contains("{++new++}"), "{h}");
    assert!(!h.contains("critic"), "{h}");
}

#[test]
fn export_model_underlines_strikes_and_keeps_or_drops_comments() {
    let doc = Document::parse(SRC, ParseOptions::default());
    let para = |o: &ModelOptions| -> Vec<ExInline> {
        match ExDoc::build(&doc, o).blocks.into_iter().next() {
            Some(ExBlock::Paragraph(v)) => v,
            other => panic!("{other:?}"),
        }
    };
    let keep = para(&ModelOptions::default());
    assert!(keep.contains(&ExInline::Underline(vec![ExInline::Text("new".into())])));
    assert!(keep.contains(&ExInline::Strike(vec![ExInline::Text("old".into())])));
    assert!(keep.contains(&ExInline::Highlight(vec![ExInline::Text("this".into())])));
    assert!(keep.contains(&ExInline::Emph(vec![ExInline::Text(" [note]".into())])));
    let strip = para(&ModelOptions {
        comments: CommentExport::Strip,
        ..Default::default()
    });
    assert!(
        !strip
            .iter()
            .any(|n| matches!(n, ExInline::Highlight(_) | ExInline::Emph(_)))
    );
}
