//! Grapheme clusters in the rendered editor (review round 2 of PR #23): Backspace, Delete and
//! arrows never split a long combining sequence, a regional-indicator flag or a ZWJ sequence —
//! the rendered view and the source view share one boundary-safe grapheme implementation.

mod common;

use common::T;

fn clusters() -> Vec<String> {
    vec![
        format!("a{}", "\u{301}".repeat(40)),
        "🇦🇺".to_string(),
        "👩\u{200d}👩\u{200d}👧\u{200d}👦".to_string(),
    ]
}

#[test]
fn rendered_backspace_delete_and_arrows_keep_clusters_whole() {
    for c in clusters() {
        // a run of the cluster (17 flags: an odd count of pairs before the caret)
        let run = c.repeat(17);
        let src = format!("x{run}y\n");
        // Backspace at the end of the run removes exactly one cluster
        let mut t = T::new(&src);
        t.set(1 + run.len());
        t.key("edit.deleteBackward");
        assert_eq!(t.text(), format!("x{}y\n", c.repeat(16)), "Backspace: {c:?}");
        // Delete at the start removes one cluster
        let mut t = T::new(&src);
        t.set(1);
        t.key("edit.deleteForward");
        assert_eq!(t.text(), format!("x{}y\n", c.repeat(16)), "Delete: {c:?}");
        // arrows step one cluster at a time
        let mut t = T::new(&src);
        t.set(1 + run.len());
        t.key("caret.left");
        assert_eq!(t.caret().offset, 1 + run.len() - c.len(), "Left: {c:?}");
        t.set(1);
        t.key("caret.right");
        assert_eq!(t.caret().offset, 1 + c.len(), "Right: {c:?}");
    }
}
