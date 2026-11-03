# ADR 0013: in-document search against source

Status: provisional local design decision for issue #48; specification clarification pending.

Spec 04 requires Find with case, word and regex switches, navigation and replacement;
spec 02 §11 requires a single undo step and panel Undo for Replace All. Neither resolves
whether the searched representation includes source syntax (open question for the spec
team: should rendered mode instead search projected prose?). Until that is resolved,
all three modes search the same source bytes. The bar explicitly says “Find in source”.
This preserves byte-accurate replacement, including mixed line endings, and avoids
inventing offsets for generated/rendered content. No reference implementation was used.

Matching is case insensitive by default, Unicode aware, with non-overlapping UTF-8 byte
ranges. Whole word means the regex crate's Unicode `\b` boundaries; it is not linguistic
word segmentation. Literal mode escapes regex syntax. Regex mode uses Rust regex syntax
(no backreferences/look-around); malformed expressions show an error. Empty queries find
nothing. Zero-width expressions make finite UTF-8-aware progress through regex's iterator.
Navigation wraps. Replacement is literal in literal mode; regex mode expands `$0`, `$1`,
`$name`, `${name}` and `$$`, with an unknown capture expanding to empty. Bytes outside
matches are untouched. Search navigation changes selection and reveal state only; it
never materialises literal hints. Rendered highlights use existing source projection
geometry: hidden syntax has no fabricated rectangle. The bar explains Source mode is
the inspection fallback for hidden syntax. Zero-width matches navigate but have no area
highlight.

One shared application state serves Rendered, Source and Side by Side. A sleeping worker
compiles regexes, scans, and builds replacement patches. Requests carry document identity,
monotonic revision and generation; completions are accepted only when all three match.
The latest queued job replaces the previous one. Cancellation is checked between matches;
a regex call with no match must finish before cancellation takes effect. Rust regex avoids
backtracking but a large input/pattern can still delay the worker. There is never a UI wait
or join for this worker. Search results cannot be used after an edit, undo or document swap.

Snapshots never retain `Arc<dyn FormatDoc>`: doing so would add a full document clone on
the next keystroke. The UI transfers at most 1 MiB per active frame as separately allocated
64 KiB UTF-8-safe chunks, and the worker concatenates them. A revision change discards
partial snapshots. Thus 10 MiB requires ten active frames before scanning, and typing can
continually postpone completion. A worker-owned patch mirror is a possible future
improvement. Repaints occur only while copying chunks or when a worker completes.

Results are bounded at 100,000 matches; exceeding that fails visibly and never silently
truncates Replace All. Replacement removed+inserted bytes are capped at 64 MiB with bounded
capture expansion. One validated PatchSet is applied through the existing session transaction
path, producing one undo entry. The panel Undo only runs at the exact document/revision
immediately after Replace All, so subsequent edits or undo/redo disable it. Patch application
and existing incremental reparse still run on the UI thread; large Replace All transactions
may therefore exceed a frame budget, despite off-thread scanning and patch preparation.

## Local validation (2026-11-01)

The source-versus-projected-prose question remains open in issue #48; A24 is also drafted
in the specification's `99-open-questions.md`.

`cargo bench -p markise-engine --bench search` (release, local macOS; build 3m14s separately):
generated ~10 MiB corpus, maximum UI transfer batch 0.115 ms over 11 batches, worker
concatenation 1.05 ms, literal no-match scan 1.54 ms, regex heading scan 0.43 ms (1,993 matches).
The ~1 MiB corpus took two batches, maximum 0.113 ms, concat 0.09 ms, literal 0.91 ms,
regex 0.12 ms. UTF-8 boundary trimming and the corpus generator's final block can add a
batch beyond the nominal MiB count. These are local measurements, not CI guarantees;
transfer work is well below the 16 ms frame budget in this workload. At 60 fps a ~10 MiB
initial transfer still takes approximately 0.18 seconds of frames before scanning starts.
