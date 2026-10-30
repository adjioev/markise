//! Test hook (issue #42, "background settled"): counts results that were published to the UI
//! thread but whose wake-up call has not returned yet. A worker takes a [`Published`] guard
//! (with [`publishing`]) just before a result becomes visible and drops it after the wake
//! that announces it, so a test that sees no outstanding job *and* a zero count knows no
//! wake-up of this crate's workers is still on its way. The guard also counts down when the
//! wake (or anything else) unwinds, so a panic cannot leave the count stuck.
//!
//! Compiled into release builds on purpose: it costs one atomic increment and decrement per
//! finished job (next to the job's own work), changes no behaviour, and keeping it
//! unconditional avoids feature plumbing through every crate whose integration tests read it.

use std::sync::atomic::{AtomicUsize, Ordering};

static UNWOKEN: AtomicUsize = AtomicUsize::new(0);

/// Counts one published result until dropped (after its wake-up).
#[must_use = "drop it after the wake-up that announces the result"]
pub struct Published<'a>(&'a AtomicUsize);

impl Drop for Published<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn publishing_on(counter: &AtomicUsize) -> Published<'_> {
    counter.fetch_add(1, Ordering::SeqCst);
    Published(counter)
}

/// A result is about to become visible to the UI thread; drop the guard after its wake-up.
pub fn publishing() -> Published<'static> {
    publishing_on(&UNWOKEN)
}

/// Results published whose wake-up has not returned yet.
pub fn unwoken() -> usize {
    UNWOKEN.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_drops_after_the_wake_and_when_it_unwinds() {
        let c = AtomicUsize::new(0);
        {
            let _p = publishing_on(&c);
            assert_eq!(c.load(Ordering::SeqCst), 1);
        }
        assert_eq!(c.load(Ordering::SeqCst), 0);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _p = publishing_on(&c);
            assert_eq!(c.load(Ordering::SeqCst), 1);
            panic!("the wake panicked");
        }));
        assert!(r.is_err());
        assert_eq!(c.load(Ordering::SeqCst), 0, "counted down during the unwind");
    }
}
