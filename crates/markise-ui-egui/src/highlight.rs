//! Adapter from the background highlighter (`markise_blocks::highlight::HighlightService`) to
//! layout's [`Highlights`] trait. The service (and its worker thread, and the syntax set) is
//! created lazily on the first code block that needs it, so documents without code — and
//! startup — never pay for it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use markise_blocks::highlight::{HighlightService, Lookup, MAX_BLOCK_SOURCE_BYTES};
use markise_layout::{CodeQuery, Highlights};
use markise_theme::TokenRun;

pub struct CodeHighlighter {
    service: OnceLock<Option<HighlightService>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Document generation (cancels unfinished work of closed / replaced documents).
    doc_generation: AtomicU64,
}

impl CodeHighlighter {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        CodeHighlighter {
            service: OnceLock::new(),
            wake,
            doc_generation: AtomicU64::new(0),
        }
    }

    fn service(&self) -> Option<&HighlightService> {
        self.service
            .get_or_init(|| {
                let s = HighlightService::new(self.wake.clone()).ok()?;
                s.set_generation(self.doc_generation.load(Ordering::Acquire));
                Some(s)
            })
            .as_ref()
    }

    /// The document changed (open, reload): unfinished jobs of the previous one are cancelled;
    /// blocks the new document still shows adopt their running jobs when requested again.
    pub fn document_changed(&self) {
        let g = self.doc_generation.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(Some(s)) = self.service.get() {
            s.set_generation(g);
        }
    }

    /// Whether the worker was started (tests: lazy start).
    pub fn started(&self) -> bool {
        self.service.get().is_some()
    }

    /// Whether the worker has unfinished requests.
    pub fn busy(&self) -> bool {
        matches!(self.service.get(), Some(Some(s)) if s.busy())
    }
}

impl Highlights for CodeHighlighter {
    fn generation(&self) -> u64 {
        1
    }

    fn ready(&self, q: &CodeQuery) -> bool {
        match self.service.get() {
            Some(Some(s)) => s.ready(q.code.key, q.range()),
            _ => false,
        }
    }

    fn runs(&self, q: &CodeQuery) -> Option<Vec<TokenRun>> {
        match self.service.get() {
            Some(Some(s)) => match s.lookup(q.code.key, q.range()) {
                Lookup::Ready(v) => Some(v),
                _ => None,
            },
            _ => None,
        }
    }

    fn want(&self, q: &CodeQuery) {
        if let Some(s) = self.service()
            && s.wants(q.code.key)
        {
            // the job gets its own copy of the block (built here, once per block version): a
            // queued job never holds the shared document, so the next keystroke never finds it
            // shared and copies it (issue #29). A full queue is retried at the next layout.
            let text = if q.code.len > MAX_BLOCK_SOURCE_BYTES {
                String::new() // never built: the service answers "plain" for such blocks
            } else {
                q.content()
            };
            s.request(q.code.key, q.lang, q.code.len, Box::new(move || text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::DiagramRenderer;
    use markise_doc::FormatDoc;
    use markise_layout::{DiagramQuery, Diagrams, DocModel, ItemKind};
    use std::time::{Duration, Instant};

    /// Issue #29: a queued highlight or diagram job must not hold the shared document; while
    /// one did, the next keystroke found the document shared and copied all of it.
    #[test]
    fn queued_jobs_do_not_hold_the_document() {
        let mut src = String::new();
        for i in 0..40 {
            src.push_str(&format!("```rust\nfn f{i}() {{ let x = {i}; }}\n```\n\n"));
            src.push_str(&format!("```mermaid\ngraph TD\nA{i}-->B{i}\n```\n\n"));
        }
        let doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse(&src));
        let model = DocModel::build_with(&*doc, None, markise_layout::Paging::Full);
        let hl = CodeHighlighter::new(Arc::new(|| {}));
        let dg = DiagramRenderer::new(Arc::new(|| {}), 14.0);
        let mut queries = Vec::new();
        for (i, (start, it)) in model.items.iter_with_start().enumerate() {
            let code = *it.code.as_deref().expect("code item");
            match &it.kind {
                ItemKind::Code { lang } => {
                    let q = CodeQuery {
                        code,
                        lang,
                        range: (0, code.len.saturating_sub(1)),
                        doc: &*doc,
                        start,
                        item: i,
                    };
                    hl.want(&q);
                    queries.push(q);
                }
                ItemKind::Diagram { lang } => {
                    let q = DiagramQuery {
                        lang: markise_blocks::diagram::DiagramLang::from_info(lang).unwrap(),
                        code,
                        doc: &*doc,
                        start,
                        item: i,
                    };
                    dg.want(&q);
                }
                other => panic!("unexpected item {other:?}"),
            }
            // right after the request, queued or not: nobody else holds the document
            assert_eq!(Arc::strong_count(&doc), 1, "item {i}");
        }
        assert!(hl.busy() || dg.busy() || queries.iter().all(|q| hl.ready(q)));
        // the jobs got the block text: every block gets highlighted (blocks cancelled beyond
        // the worker's active limit are asked again, as a layout would)
        let t = Instant::now();
        while !queries.iter().all(|q| hl.ready(q)) {
            assert!(t.elapsed() < Duration::from_secs(30), "highlights did not arrive");
            for q in queries.iter().filter(|q| !hl.ready(q)) {
                hl.want(q);
            }
            assert_eq!(Arc::strong_count(&doc), 1);
            std::thread::sleep(Duration::from_millis(2));
        }
        let runs = hl.runs(&queries[0]).unwrap();
        assert!(!runs.is_empty(), "the worker highlighted the real block text");
    }
}
