//! Adapter from the background highlighter (`markise_blocks::highlight::HighlightService`) to
//! layout's [`Highlights`] trait. The service (and its worker thread, and the syntax set) is
//! created lazily on the first code block that needs it, so documents without code — and
//! startup — never pay for it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use markise_blocks::highlight::{HighlightService, Lookup};
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
        if let Some(s) = self.service() {
            // a full queue is retried at the next layout of the item
            // content is built on the worker from the shared document (O(1) here)
            s.request(q.code.key, q.lang, q.code.len, Box::new(q.source()));
        }
    }
}
