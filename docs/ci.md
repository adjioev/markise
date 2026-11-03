# CI policy

GitHub Actions minutes on this private repository are paid (Linux $0.008/min, Windows
$0.016/min, macOS $0.08/min), so CI (`.github/workflows/ci.yml`) runs the cheap Linux checks on
every PR update and the macOS + Windows matrix only on demand and after merge.

## What runs when

| Trigger | Linux fast checks | macOS + Windows, release binaries | Heavy (Linux) |
|---|---|---|---|
| PR opened / pushed / reopened / marked ready for review | yes | no | no |
| Any PR event above with the `full-ci` label | yes | yes | no |
| Any PR event above with the `heavy` label | yes | only with `full-ci` too | yes |
| Push to `main` (after a merge) | yes | yes | no |
| `workflow_dispatch` (manual; not shown in PR checks) | yes | yes | yes |
| Nightly `main` (schedule, 16:23 UTC) | yes | no | yes |
| Docs-only change (Markdown under `docs/`, `README.md`, `CLAUDE.md`, `site/*.html`, `site/*.css`) | not triggered | not triggered | not triggered |

The Linux jobs are: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo xtask layers` and `cargo xtask assets` (job `fmt + clippy (Linux)`), and the workspace
fast correctness tests (job `test + build (linux-x86_64)`). The full matrix also runs the tests on macOS (Apple Silicon) and
Windows and builds and packages release binaries on all three OSes; each OS also starts the
release app once on a document (`scripts/smoke-start.sh`: it must start a renderer — wgpu, or
glow after a fallback — and quit by itself; Linux under Xvfb). The packaged binaries are
uploaded as artifacts (`markise-<os>`, kept 3 days) **only from pushes to `main`**; `full-ci` PR
runs and manual runs build and package but upload nothing.

The docs-only filter is deliberately narrow: only prose and the static site page are ignored.
Everything else triggers CI, including `AGENTS.md` (its crate table is checked by
`cargo xtask layers`), images or media under `docs/` and `site/assets/` (`cargo xtask assets`),
Markdown test fixtures under `crates/`, and any `SOURCE.md`/`SOURCES.md`, `tests/fixtures/` or
`testdata/` file even under `docs/`.

## Merge gate

Check **`CI result`** in the PR checks. It is green only when every job of that run succeeded;
any failed or cancelled job (including a single matrix entry) makes it red. On a PR run without
the `full-ci` label the matrix is Linux only, so a green `CI result` then **certifies Linux
only**; with the label it certifies Linux, macOS and Windows. (Branch protection is not available
for this private repository on the Free plan, so nothing enforces it; look at it before
merging.) Run the full matrix (label `full-ci`) before merging changes that touch
platform-specific code, packaging, the app shell, fonts/rendering, file I/O or paths; `main`
runs it after every merge in any case.

Label events do not start CI at all (no `labeled` trigger), so adding an unrelated label cannot
produce a green `CI result` without tests.

The `heavy` label is independent of `full-ci`: set it (at the pilot's discretion) on PRs that
change the heavy tests themselves or large-document code paths. It is requested the same way
as `full-ci` below (add the label, then push or close and reopen); with it, `CI result` also
requires the heavy job to succeed.

## Triggering the full matrix

On a PR (results appear in the PR checks):

1. Add the label: `gh pr edit <number> --add-label full-ci` (or in the PR sidebar). Adding it
   does not start a run by itself.
2. Start a run: push a commit (an empty one is fine:
   `git commit --allow-empty -m "Run full CI" && git push`), or close and reopen the PR
   (`gh pr close <number> && gh pr reopen <number>`).
3. Every later PR run includes macOS + Windows while the label is set; remove it
   (`gh pr edit <number> --remove-label full-ci`) to go back to Linux-only runs.

Manually on main or any branch: `gh workflow run ci.yml --ref <branch>`. This run is not linked
to a PR and does not report into PR checks; look at it under Actions.

## Cost controls

- **Superseded runs are cancelled**: PR runs are keyed by PR number; a new push cancels the old
  run, pending or in progress. Manual runs are keyed by branch. Each push to `main` gets its own concurrency group, so `main` runs are
  never cancelled or replaced.
- **Cargo cache** (`Swatinem/rust-cache`, in `.github/actions/setup-rust`): keyed per job, runner
  OS/arch, toolchain and `Cargo.lock`; saved only from `main` and restored by PRs, so the
  repository cache does not fill up with per-PR entries. Because the key carries the
  toolchain/env and `Cargo.lock`/`Cargo.toml` hashes, each dependency or toolchain change on
  `main` saves a new entry; the `prune superseded Cargo caches` job (pushes to `main` only, not
  part of `CI result`) then reads every page of the cache list and deletes the older
  `v0-rust-lint-*`/`v0-rust-test-*` entries on `main`, keeping the newest one per job/OS (other
  workflows' caches, e.g. `release.yml`'s, are left alone).
- **Artifacts**: only pushes to `main` upload the CI binaries (3-day retention); release
  workflow artifacts are kept 7 days.
- Test binaries use line-tables-only debug info (keeps the Linux runner's disk from filling up).


## Integration-test binaries (#116, #151)

Every file in `crates/*/tests/` used to build and link its own executable, each one linking
the full UI stack; most of the test time was compiling and linking them. Each crate now has a
single integration-test target: `tests/it/main.rs` declares one module per topic
(`tests/it/<topic>.rs`), shared helpers live in `tests/it/common.rs`, and fixtures and data
files stay where they were (`tests/fixtures/`, `tests/corpus/`, `tests/snapshots/`, …).
Saved proptest failures live in `tests/proptest-regressions/<topic>.txt`: with a `main.rs` in
`tests/it/`, that is where proptest's default `SourceParallel` persistence reads and writes
them (a `tests/it/<topic>.proptest-regressions` file would be silently ignored). Crates
with a single integration-test file keep it (`tests/<name>.rs`) until they get a second one.
New integration tests are modules, never a new `tests/*.rs` file. Test names carry the module
(`qa_sweep::l12_…`); select one module with `cargo test -p <crate> --test it <topic>::`.
`cargo xtask heavy` filters by name as before (`heavy_suite_`), and its 3000-case property
step selects the `nested::`, `fastpath::`, `hints::` (markise-md) and `incremental::`
(markise-layout) modules of the `it` targets.

All tests of a crate now share one process. No test sets environment variables or the current
directory, and the allocator setting of the heavy run (`GLIBC_TUNABLES`, #109) is per process
as before. The one shared resource that mattered is the macOS file watcher: every watcher is
an FSEvents registration, serialized system-wide, so the UI suite opening one per test spent
minutes waiting for registrations once all its tests ran together. `AppOptions::watch` turns
the watchers off; UI tests that do not exercise watching use `common::no_watch()`, and only
tests of reloads, conflicts with external writes and live updates (`watch_gaps`, `idle`,
`open_latency`, `safety`, plus single tests in `smoke`, `source_editor`, `review`, `themes`,
`qa_sweep`, `vault` and `sidebar`) keep it on. Settings-file assertions settle the background
and read the file once (`common::assert_settings`) instead of polling it.

## Fast and heavy coverage (#98)

`cargo test --workspace` and `cargo xtask ci` run the fast suite. The target is a few minutes
of warm test execution on CI, separately from compilation; this is a target, not a claim based
on local timings. Normal property case counts, spec conformance, theme importer inputs and
small regressions remain in this suite. Keep both text backends and representative styling,
width and zoom boundaries in small regressions; expand the combinations in heavy coverage.

`cargo xtask heavy` runs release-profile ignored tests **filtered by `heavy_suite_`**, then the
incremental parse/layout properties with `PROPTEST_CASES=3000`. Heavy tests carry both the
`heavy_suite_` prefix and `#[ignore = "heavy: reason"]`. Do not use `cargo test -- --ignored`:
`qa_sweep` has intentionally ignored known bugs, which this command would also execute.
Heavy tests retain the original large inputs, scaling checks and full matrices. Smaller tests
use the same assertion helpers where possible. Preserve generous asynchronous hang guards;
wall-clock speed belongs in benchmarks, not correctness assertions.

The heavy job is separate from the OS matrix flag. It runs on nightly `main` (schedule,
16:23 UTC, at the latest `main` commit), manual CI (`workflow_dispatch`) and PR runs labeled
`heavy`; **not** on ordinary main pushes and **not** for the `full-ci` label, which only adds
macOS and Windows. Nightly uses Linux only; ordinary main pushes retain the three-OS matrix.
`CI result` requires heavy success when requested (the job condition and its `EXPECT_HEAVY`
are the same expression) and requires it to be skipped otherwise; a heavy failure or
cancellation fails the gate.

A failed nightly run is reported twice: GitHub e-mails the user who last edited the `schedule`
cron in `ci.yml`, and the `report nightly failure` job opens one issue labelled
`nightly-failure` (or comments on the already open one). Close that issue once `main` is
fixed. The job runs only on scheduled runs, needs only `issues: write`, and is not part of
`CI result`.
The release workflow calls the same reusable heavy workflow at `prepare.outputs.sha` and
requires success before any release build/artifact upload or draft creation. This prevents a
nightly success for a different commit from being used as a release gate. Spec/theme fetching,
pinned actions, existing cache keys and artifact retention policies are preserved.

For local per-test observations (stdlib Python, no new dependencies):

```sh
python3 scripts/test-times.py --match adversarial --output /tmp/test-times.json
python3 scripts/test-times.py --release --heavy --output /tmp/heavy-times.json
```

The script first measures `cargo test --no-run`, then runs matching tests individually from
already built executables. Its JSON separates build time and per-test process wall time
(including startup), records failures and exits nonzero on a test failure. These isolated local
measurements are not parallel suite totals or CI timings. Omit `--match` to inventory all fast
tests. Reports stay outside the repository; compare before/after on the same machine/profile.
See `benches/README.md` for performance measurements and budgets.

### Local observations (2026-11-01)

On the implementation machine (macOS, debug profile), an initial workspace `--no-run` build
finished in **2m 27s**. Repeating the build before the isolated baseline measurements took
**3.15s** wall time; the rebuild after test edits took **5.90s** according to Cargo. These are
compilation observations, separate from the already-built test processes below:

| Coverage | Original heavy input/loop | Fast regression |
| --- | ---: | ---: |
| Parser adversarial families | 14.394s | 0.016s |
| Sanitizer/export adversarial families | 30.819s | 0.267s |
| Heading caret, styles/themes/backends/levels | 19.250s | 1.858s |
| CLI 220 MB export warning / small size-limit checks | 2.010s | 1.518s |

These tests intentionally perform different amounts of work: the original coverage remains
in heavy, while fast keeps small inputs and representative combinations. They are process
wall times from `scripts/test-times.py`, including startup, not a before/after CI suite total.
The remaining large-input baseline was stopped rather than repeatedly running huge tests.
The small source-editor, incremental word-count and comment-scan regressions took 0.466s,
1.265s and 1.385s respectively. Inexpensive standalone layout width sweeps remain complete in
the fast suite: a coarse sample did not meet their justified-row coverage guard, so the full
sweeps were restored rather than weakening the guard. No CI runtime claim is made until the
new workflow actually runs; compilation and execution have separate CI steps for comparison.


The local default workspace run passed **1,565 tests**, with **22 ignored** (11 heavy,
11 pre-existing known bugs). Cargo reported **13.79s** compilation, while the sum of libtest's
per-binary execution durations was **144.93s**. This sum excludes process startup and is not
end-to-end wall time; release compilation was running concurrently. The macOS watcher tests
required running outside the sandbox (FSEvents could not start inside it). Local downloaded
spec/theme caches were absent, so CI must still validate those inputs after its fetch steps.


The final timing-assertion audit removed the wikilink test's 2s speed assertion (link-count
assertions and all adversarial inputs remain) and the diagram fuzz helper's 8s process-time
assertion. The diagram renderer retains its 5s production budget and outcome/limit checks;
PDF and event-wait termination guards remain unchanged. Wikilink speed now has explicit
Criterion cases in `markise-md/benches/parse.rs`; diagram benchmarks already exist.


`cargo xtask heavy` also passed locally: all **11 retained heavy tests**, then **42 tests**
in the incremental targets with `PROPTEST_CASES=3000`. Initial release compilation took
**3m 38s**; the narrower property-target build took **40.48s** (different Cargo feature
unification). Final targeted wikilink/diagram tests passed **18 tests**, and all-target Clippy,
formatting, layering, assets, workflow lint and the merge-gate status truth table passed.
No paid workflow was dispatched; the owner still needs to review the change before committing.
