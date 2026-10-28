//! Background jobs for the editing session: rebuilding the view model from a source-text
//! snapshot, and saving a snapshot. Each job carries the text revision it was made for, so
//! the UI can drop stale completions. Thread-spawn failures and worker panics surface as job
//! errors (a panicking worker drops its sender; the receiver sees a disconnect).
//! [`JobHooks`] let tests inject delays and failures deterministically.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use markise_doc::FormatDoc;
use markise_layout::DocModel;

use crate::Wake;
use crate::save::{Expect, SaveError, SaveHooks, SaveOutcome, save_checked_with};

/// A rebuild hook: `Err` fails the rebuild of that revision.
pub type RebuildHook = Box<dyn Fn(u64) -> Result<(), String> + Send + Sync>;

/// Test hooks run on the worker threads.
#[derive(Default)]
pub struct JobHooks {
    /// Before a rebuild; `Err` makes the rebuild fail (a panic here simulates a crash).
    pub before_rebuild: Option<RebuildHook>,
    /// Before a save job starts writing (tests block here to edit while saving).
    pub before_save: Option<Box<dyn Fn(u64) + Send + Sync>>,
    /// Hooks inside the save protocol.
    pub save: SaveHooks,
    /// On the file watcher's worker before it applies changed watch targets (tests stall it to
    /// prove that registering watches never holds up the caller).
    pub before_watch: Option<Box<dyn Fn() + Send + Sync>>,
}

pub type Hooks = Arc<JobHooks>;

/// Result of [`spawn_rebuild`].
pub struct Rebuilt {
    pub rev: u64,
    pub doc: Arc<dyn FormatDoc>,
    pub model: Arc<DocModel>,
}

/// Applies `text` to a copy of the document `base` (minimal verbatim patches,
/// [`crate::edit::apply_text_doc`]: the result's text equals `text` byte for byte) and
/// rebuilds the view model, off the UI thread. `base_model` is returned unchanged when the
/// text did not change.
pub fn spawn_rebuild(
    base: Arc<dyn FormatDoc>,
    base_model: Arc<DocModel>,
    text: String,
    path: Option<PathBuf>,
    rev: u64,
    hooks: Hooks,
    wake: Wake,
) -> Receiver<Result<Rebuilt, String>> {
    let (tx, rx) = channel();
    let tx2 = tx.clone();
    let r = std::thread::Builder::new()
        .name("markise-rebuild".into())
        .spawn(move || {
            let res = (|| {
                if let Some(h) = &hooks.before_rebuild {
                    h(rev)?;
                }
                let mut doc = base.clone_box();
                let (doc, model) = if crate::edit::apply_text_doc(&mut *doc, &text) {
                    let model = DocModel::build(&*doc, path.as_deref());
                    (Arc::<dyn FormatDoc>::from(doc), model)
                } else {
                    (base, base_model)
                };
                Ok(Rebuilt { rev, doc, model })
            })();
            let _ = tx.send(res);
            wake();
        });
    if let Err(e) = r {
        let _ = tx2.send(Err(format!("cannot start worker: {e}")));
    }
    rx
}

/// Result of [`spawn_save`].
pub struct Saved {
    pub rev: u64,
    pub result: Result<SaveOutcome, SaveError>,
    pub bytes: usize,
}

/// What a save writes.
pub enum SaveText {
    /// Exactly these bytes (an editor session's hint-free snapshot).
    Exact(Arc<str>),
    /// A source-mode buffer (the document text, 99 C58): applied to `base` as minimal verbatim
    /// patches (untouched bytes round-trip; the file gets exactly the buffer text).
    Buffer { base: Arc<dyn FormatDoc>, text: String },
}

/// Saves `text` (revision `rev`), written with the conflict protocol of [`crate::save`] —
/// off the UI thread.
pub fn spawn_save(
    text: SaveText,
    path: PathBuf,
    expect: Expect,
    rev: u64,
    hooks: Hooks,
    wake: Wake,
) -> Receiver<Saved> {
    let (tx, rx) = channel();
    let tx2 = tx.clone();
    let r = std::thread::Builder::new().name("markise-save".into()).spawn(move || {
        if let Some(h) = &hooks.before_save {
            h(rev);
        }
        let out: Arc<str> = match text {
            SaveText::Exact(t) => t,
            SaveText::Buffer { base, text } => {
                let mut doc = base.clone_box();
                crate::edit::apply_text_doc(&mut *doc, &text);
                doc.snapshot()
            }
        };
        let result = save_checked_with(&path, &out, expect, &hooks.save);
        let _ = tx.send(Saved {
            rev,
            result,
            bytes: out.len(),
        });
        wake();
    });
    if let Err(e) = r {
        let _ = tx2.send(Saved {
            rev,
            result: Err(SaveError::Io(format!("cannot start save worker: {e}"))),
            bytes: 0,
        });
    }
    rx
}
