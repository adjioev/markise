//! Cancellation and progress shared between an export worker and the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Cancellation flag plus a coarse progress value (0–1000 ‰), cheap to clone and poll.
#[derive(Clone, Debug, Default)]
pub struct JobControl {
    cancel: Arc<AtomicBool>,
    permille: Arc<AtomicU32>,
}

/// Returned by long-running steps when [`JobControl::cancel`] was requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("export cancelled")
    }
}

impl std::error::Error for Cancelled {}

impl JobControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation (the worker stops at its next check).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once cancellation was requested.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() { Err(Cancelled) } else { Ok(()) }
    }

    /// Sets the progress fraction (clamped to 0–1).
    pub fn set_progress(&self, fraction: f32) {
        let v = (fraction.clamp(0.0, 1.0) * 1000.0) as u32;
        self.permille.store(v, Ordering::Relaxed);
    }

    /// Progress fraction 0–1.
    pub fn progress(&self) -> f32 {
        self.permille.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_shared() {
        let a = JobControl::new();
        let b = a.clone();
        assert!(a.check().is_ok());
        b.cancel();
        assert_eq!(a.check(), Err(Cancelled));
        a.set_progress(2.0);
        assert_eq!(b.progress(), 1.0);
    }
}
