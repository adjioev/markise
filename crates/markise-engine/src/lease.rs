//! Lending the session's shared document to a background reader (the word count's worker,
//! issue #132; the comment presence scanner, issue #138) without making the next edit copy
//! it. Each reader has its own lease; the session revokes all of them before an edit.
//!
//! The session edits its document in place only while no one else holds it
//! ([`crate::editor::EditorSession`] copies a shared document first — 100 MiB for a huge
//! file). A worker that keeps an `Arc` of the document would force that copy on every
//! keystroke. Instead the owner *lends* the document for one revision; the worker reads it in
//! short steps ([`DocLease::with`]), and the session *revokes* the loan before every edit
//! ([`DocLease::revoke`]), waiting at most for the worker's current step. After the edit the
//! owner lends the new revision again.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use markise_doc::FormatDoc;

/// A document lent for one revision (see the module docs).
#[derive(Default)]
pub struct DocLease {
    slot: Mutex<Option<(u64, Arc<dyn FormatDoc>)>>,
    /// Revokes waiting for the slot: a borrower lets them go first.
    revoking: AtomicUsize,
}

impl std::fmt::Debug for DocLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLease").field("lent", &self.lent()).finish()
    }
}

impl DocLease {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Option<(u64, Arc<dyn FormatDoc>)>> {
        // a borrower that panicked mid-step leaves nothing half-done here
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Lends `doc` as revision `rev` (replacing an earlier loan).
    pub fn lend(&self, rev: u64, doc: Arc<dyn FormatDoc>) {
        let old = self.lock().replace((rev, doc));
        drop(old);
    }

    /// Takes the document back: when this returns, the lease holds no reference to it. Waits
    /// for a borrower's current step ([`DocLease::with`]) to finish.
    pub fn revoke(&self) {
        self.revoking.fetch_add(1, Ordering::SeqCst);
        let old = self.lock().take();
        self.revoking.fetch_sub(1, Ordering::SeqCst);
        drop(old);
    }

    /// The revision lent, if any.
    pub fn lent(&self) -> Option<u64> {
        self.lock().as_ref().map(|(r, _)| *r)
    }

    /// Runs `f` on the document lent as revision `rev`; `None` when nothing or another revision
    /// is lent. Keep `f` short: an edit on the owner's thread waits for it.
    pub fn with<R>(&self, rev: u64, f: impl FnOnce(&dyn FormatDoc) -> R) -> Option<R> {
        // an owner about to revoke goes first (the mutex is not fair)
        while self.revoking.load(Ordering::SeqCst) > 0 {
            std::thread::yield_now();
        }
        let g = self.lock();
        match &*g {
            Some((r, d)) if *r == rev => Some(f(d.as_ref())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoke_releases_the_document() {
        let doc: Arc<dyn FormatDoc> = Arc::new(markise_md::parse("a b c\n"));
        let l = DocLease::new();
        l.lend(3, doc.clone());
        assert_eq!(Arc::strong_count(&doc), 2);
        assert_eq!(l.with(3, |d| d.text().len()), Some(6));
        assert_eq!(l.with(2, |d| d.text().len()), None, "another revision");
        l.revoke();
        assert_eq!(Arc::strong_count(&doc), 1, "the owner holds the only reference again");
        assert_eq!(l.with(3, |_| ()), None);
        assert_eq!(l.lent(), None);
    }
}
