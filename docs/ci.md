# CI policy

GitHub Actions minutes on this private repository are paid (Linux $0.008/min, Windows
$0.016/min, macOS $0.08/min), so CI (`.github/workflows/ci.yml`) runs the cheap Linux checks on
every PR update and the macOS + Windows matrix only on demand and after merge.

## What runs when

| Trigger | Linux: `fmt + clippy (Linux)`, `test + build (linux-x86_64)` | macOS + Windows `test + build`, release binaries, artifacts |
|---|---|---|
| PR opened / pushed / reopened / marked ready for review | yes | no |
| Any of the PR events above while the PR carries the `full-ci` label | yes | yes |
| Push to `main` (after a merge) | yes | yes |
| `workflow_dispatch` (manual, main or any branch; not shown in PR checks) | yes | yes |
| Docs-only change (Markdown under `docs/`, `README.md`, `CLAUDE.md`, `site/*.html`, `site/*.css`) | not triggered | not triggered |

The Linux jobs are: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo xtask layers` and `cargo xtask assets` (job `fmt + clippy (Linux)`), and the workspace
tests plus the release-mode incremental property tests with 3000 cases (job
`test + build (linux-x86_64)`). The full matrix also runs the tests on macOS (Apple Silicon) and
Windows and builds and packages release binaries on all three OSes. The packaged binaries are
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
