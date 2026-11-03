//! Shared headless editing surface for integration tests (session + layout, CPU fonts).
#![allow(dead_code)]

use std::sync::Arc;

use epaint::text::Fonts;
use markise_doc::{DocPos, DocumentFormat, Params};
use markise_engine::editor::{EditorSession, EditorView, HistoryLimits, ManualClock};
use markise_layout::block::NoImages;
use markise_layout::{DocModel, Paging, View, testing};
use markise_md::MarkdownFormat;
use markise_theme::Theme;

pub struct T {
    pub ed: EditorView,
    pub fonts: Fonts,
    pub clock: ManualClock,
    /// Frames run the viewer's selection fix too ([`T::frame`]).
    pub fix_selection: bool,
}

impl T {
    pub fn new(text: &str) -> T {
        Self::with_paging(text, Paging::from_env())
    }

    /// With a paged model of `tops`-block pages (issue #125).
    pub fn paged(text: &str, tops: usize) -> T {
        Self::with_paging(text, Paging::Paged { tops })
    }

    pub fn with_paging(text: &str, paging: Paging) -> T {
        let clock = ManualClock::default();
        let format: Arc<dyn DocumentFormat> = Arc::new(MarkdownFormat::default());
        let session = EditorSession::with(format, text, HistoryLimits::default(), Box::new(clock.clone()));
        let model = Arc::new(DocModel::build_with(session.doc(), None, paging));
        let view = View::new(session.doc_arc().clone(), model, Theme::light(), 600.0, 1.0);
        let mut t = T {
            ed: EditorView::new(session, view, None),
            fonts: testing::fonts(),
            clock,
            fix_selection: false,
        };
        t.frame();
        t
    }

    /// One frame: focus and layout (a paged model materializes and evicts pages here); with
    /// [`T::fix_selection`], then the selection fixed onto caret stops, as the viewer does.
    pub fn frame(&mut self) {
        self.ed.refresh_focus();
        let mut f = self.fonts.with_pixels_per_point(1.0);
        self.ed.view.frame(800.0, 400.0, &mut f, &NoImages);
        if self.fix_selection && self.ed.fix_selection(&mut f, &NoImages) {
            self.ed.refresh_focus();
        }
    }

    /// Scrolls the caret into view (as the UI does after a command) and draws a frame.
    pub fn reveal(&mut self) {
        {
            let mut f = self.fonts.with_pixels_per_point(1.0);
            self.ed.scroll_to_caret(400.0, &mut f, &NoImages);
        }
        self.frame();
    }

    pub fn run(&mut self, id: &str) {
        self.run_p(id, Params::new());
    }

    pub fn run_p(&mut self, id: &str, p: Params) {
        {
            let mut f = self.fonts.with_pixels_per_point(1.0);
            let r = self.ed.execute(id, &p, &mut f, &NoImages, 800.0);
            if let Err(e) = r {
                assert!(
                    matches!(e, markise_engine::editor::command::CommandError::Disabled(_)),
                    "{id}: {e}"
                );
            }
        }
        self.frame();
    }

    pub fn typ(&mut self, s: &str) {
        for c in s.chars() {
            self.run_p("edit.insertText", Params::text("text", c.to_string()));
        }
    }

    pub fn text(&self) -> &str {
        self.ed.session.text()
    }

    pub fn caret(&self) -> DocPos {
        self.ed.session.selection().primary().focus
    }

    pub fn set(&mut self, off: usize) {
        self.ed.set_caret(DocPos::new(off), false);
        self.frame();
    }

    pub fn revealed(&self) -> bool {
        self.ed.view.focus().is_some_and(|(_, f)| !f.reveal.is_empty())
    }

    /// The incremental model equals a full rebuild (a paged one where it holds real items).
    pub fn check_model(&self) {
        let doc = self.ed.session.doc();
        let m = self.ed.view.model();
        if m.is_paged() {
            m.check_against_full(doc)
                .expect("incremental paged model == full build");
        } else {
            m.diff(&DocModel::build_with(doc, None, Paging::Full))
                .expect("incremental model == full build");
        }
    }
}

impl T {
    /// Text with `‸` at the caret (or `⟦…⟧` around the selection).
    pub fn shown(&self) -> String {
        let r = self.ed.session.selection().primary();
        let (a, f) = (r.anchor.offset, r.focus.offset);
        let mut s = self.text().to_string();
        if a == f {
            s.insert(a, '‸');
            if r.focus.virt > 0 {
                s.insert_str(a, "(v)");
            }
        } else {
            let (x, y) = (a.min(f), a.max(f));
            s.insert(y, '⟧');
            s.insert(x, '⟦');
        }
        s
    }
    /// Opens `src` with `‸` marking the caret.
    pub fn at(src: &str) -> T {
        let off = src.find('‸').expect("caret");
        let text = src.replace('‸', "");
        let mut t = T::new(&text);
        t.set(off);
        t
    }
    /// The caret can be drawn (it lies on a caret stop of the layout).
    /// `true` when the selection is on caret stops (the UI's per-frame fix leaves it).
    pub fn selection_on_stops(&mut self) -> bool {
        let mut f = self.fonts.with_pixels_per_point(1.0);
        !self.ed.fix_selection(&mut f, &NoImages)
    }
    pub fn caret_visible(&mut self) -> bool {
        let mut f = self.fonts.with_pixels_per_point(1.0);
        self.ed.caret_rect(&mut f, &NoImages).is_some()
    }
    pub fn key(&mut self, id: &str) {
        self.run(id);
        self.check_model();
    }
}
