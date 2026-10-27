# Releasing Markise

Release artifacts are built by `cargo xtask dist <platform>` (xtask/src/dist.rs) and packaged
by `scripts/package.sh`; `.github/workflows/release.yml` runs the same command for every
platform. Publishing is always a manual, owner-approved step: the workflow only ever creates a
**draft** release.

| Platform | Artifact | Contents |
|---|---|---|
| `macos-universal` | `markise-macos-universal.dmg` | `Markise.app` (aarch64 + x86_64, ad-hoc signed, `.md`/`.markdown` document types), `/Applications` link, `markise-cli`, licences |
| `windows-x86_64` | `markise-windows-x86_64.zip` | `markise.exe` (icon embedded, no console window), `markise-cli.exe`, licences |
| `linux-x86_64` | `markise-linux-x86_64.tar.gz` | `markise`, `markise-cli`, `share/` (desktop entry, hicolor icons), licences |
| | `SHA256SUMS` | SHA-256 of the three archives |

Tag runs use versionless asset names (for `/releases/latest/download/…` links); local and
dry-run artifacts carry the label, e.g. `markise-0.1.0-beta.1-macos-universal.dmg`.

## Local build

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin   # once (macOS)
cargo xtask dist macos-universal        # -> dist/markise-<version>-macos-universal.dmg
cargo xtask dist --help                 # all platforms and options
```

Each platform is built on its own OS (no cross-compilation). The `dist` profile is `release`
plus thin LTO and stripped symbols.

## Dry run (no release)

```sh
gh workflow run release.yml --ref <branch>      # or Actions > Release > Run workflow
gh run watch
gh run download <run-id> -D /tmp/markise-dry-run
```

Builds the selected commit as `<version>-dryrun.<sha7>` and uploads the archives and
`SHA256SUMS` to the workflow run only (7 days).

## Cutting a release

1. On a branch, set `version` in `[workspace.package]` of `Cargo.toml` (e.g. `0.1.0-beta.1`),
   run `cargo check` so `Cargo.lock` follows, and write `docs/release/<version>.md`; merge to
   `main` (owner approval).
2. Optional: dry-run the workflow on `main` and smoke-test the artifacts.
3. Tag the merge commit and push the tag (owner approval):

   ```sh
   git switch main && git pull --ff-only
   git tag -a v0.1.0-beta.1 -m "Markise 0.1.0-beta.1"
   git push origin v0.1.0-beta.1
   ```

   The workflow checks that the tag equals `v` + the workspace version and that the release
   notes file exists, builds all platforms, and creates a new **draft** release (a GitHub
   *pre-release* when the version has a `-beta`/`-rc` part) with the archives and
   `SHA256SUMS`. It refuses to run if the tag already has a published release.
4. Download the draft's assets, check them (`shasum -a 256 -c SHA256SUMS`), launch each app.
5. Edit the draft notes if needed (check the feature list against `main`), then **Publish release**.

A failed tag run can be re-run from the Actions page; each successful run creates a new draft,
so delete stale drafts by hand. To move a tag that has no published release:
`git push origin :refs/tags/vX` and tag again.

## Not done yet

- macOS Developer ID signing and notarization, Windows Authenticode signing (need owner
  accounts and secrets).
- Linux AppImage / .deb / Flatpak.
- The .dmg is not byte-reproducible (disk images carry creation dates and UUIDs); the archive
  contents follow the rules in `scripts/package.sh`.
