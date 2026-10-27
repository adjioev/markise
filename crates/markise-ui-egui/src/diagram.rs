//! Adapter from the background diagram renderer (`markise_blocks::diagram::DiagramService`) to
//! layout's [`Diagrams`] trait (ADR 0009). Like the highlighter, the service and its worker are
//! created lazily on the first diagram shown, so documents without diagrams — and startup —
//! never pay for them.
//!
//! Text metrics (ADR 0009 §4, DG-6): layout measures the body font's advances with the document
//! text stack once per theme/font change (`Diagrams::set_metrics`); the worker lays labels out
//! with that table, so boxes fit the faces painted. Until the first measurement, and for
//! export, `ApproxMeasure` is used.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use markise_blocks::diagram::{ApproxMeasure, DiagramLookup, DiagramService, TableMeasure, TextMeasure};
use markise_layout::{DiagramQuery, DiagramState, Diagrams};

pub struct DiagramRenderer {
    service: OnceLock<Option<DiagramService>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Document generation (cancels unfinished work of closed / replaced documents).
    doc_generation: AtomicU64,
    /// Label font size (f32 bits), from the theme's body size.
    font_size: AtomicU32,
    /// Label metrics measured by layout: (fonts fingerprint, table).
    metrics: Mutex<Option<(u64, Arc<TableMeasure>)>>,
}

impl DiagramRenderer {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>, font_size: f32) -> Self {
        DiagramRenderer {
            service: OnceLock::new(),
            wake,
            doc_generation: AtomicU64::new(0),
            font_size: AtomicU32::new(font_size.to_bits()),
            metrics: Mutex::new(None),
        }
    }

    fn service(&self) -> Option<&DiagramService> {
        self.service
            .get_or_init(|| {
                let s = DiagramService::new(Arc::new(ApproxMeasure), self.wake.clone()).ok()?;
                s.set_generation(self.doc_generation.load(Ordering::Acquire));
                Some(s)
            })
            .as_ref()
    }

    /// The document changed (open, reload): unfinished jobs of the previous one are cancelled.
    pub fn document_changed(&self) {
        let g = self.doc_generation.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(Some(s)) = self.service.get() {
            s.set_generation(g);
        }
    }

    /// Label size in points (from the active style); changes the generation (new layouts).
    pub fn set_font_size(&self, size: f32) {
        self.font_size.store(size.to_bits(), Ordering::Release);
    }

    fn font_size(&self) -> f32 {
        f32::from_bits(self.font_size.load(Ordering::Acquire))
    }

    /// The measurer jobs use now.
    fn measure(&self) -> Arc<dyn TextMeasure> {
        match &*self.metrics.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((_, m)) => m.clone(),
            None => Arc::new(ApproxMeasure),
        }
    }

    fn measure_id(&self) -> u64 {
        match &*self.metrics.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((_, m)) => m.id(),
            None => ApproxMeasure.id(),
        }
    }

    fn key(&self, q: &DiagramQuery) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (q.code.key, q.lang, self.font_size().to_bits(), self.measure_id()).hash(&mut h);
        h.finish()
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

impl Diagrams for DiagramRenderer {
    fn generation(&self) -> u64 {
        // on (non-zero), label size, metrics
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (1u8, self.font_size().to_bits(), self.measure_id()).hash(&mut h);
        h.finish() | 1
    }

    fn state(&self, q: &DiagramQuery) -> DiagramState {
        match self.service.get() {
            Some(Some(s)) => match s.lookup(self.key(q)) {
                DiagramLookup::Ready(r) => DiagramState::Done(r),
                _ => DiagramState::Pending,
            },
            // not started yet: the first `want` starts it
            Some(None) => DiagramState::Off,
            None => DiagramState::Pending,
        }
    }

    fn want(&self, q: &DiagramQuery) {
        if let Some(s) = self.service() {
            let _ = s.request_with(
                self.key(q),
                q.lang,
                q.code.len,
                Box::new(q.source()),
                self.font_size(),
                Some(self.measure()),
            );
        }
    }

    fn needs_metrics(&self, fonts: u64) -> bool {
        !matches!(&*self.metrics.lock().unwrap_or_else(|e| e.into_inner()), Some((f, _)) if *f == fonts)
    }

    fn set_metrics(&self, fonts: u64, metrics: TableMeasure) {
        *self.metrics.lock().unwrap_or_else(|e| e.into_inner()) = Some((fonts, Arc::new(metrics)));
    }
}
