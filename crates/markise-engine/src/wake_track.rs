//! Test hook (issue #42, "background settled"): counts results that were published to the UI
//! thread but whose wake-up call has not returned yet. Workers call [`publishing`] before a
//! result becomes visible and [`woken`] after the wake that announces it, so a test that sees
//! no outstanding job *and* a zero count knows no wake-up of this crate's workers is still on
//! its way. Process-wide (cheap; tests only read it). No effect on behaviour.

use std::sync::atomic::{AtomicUsize, Ordering};

static UNWOKEN: AtomicUsize = AtomicUsize::new(0);

/// A result is about to become visible to the UI thread.
pub fn publishing() {
    UNWOKEN.fetch_add(1, Ordering::SeqCst);
}

/// The wake-up announcing a result returned (or the result was never published).
pub fn woken() {
    UNWOKEN.fetch_sub(1, Ordering::SeqCst);
}

/// Results published whose wake-up is still running.
pub fn unwoken() -> usize {
    UNWOKEN.load(Ordering::SeqCst)
}
