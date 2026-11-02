# Update check

Issue #70. Code: `crates/markise-engine/src/update.rs` (decisions, parsing, background
check), `crates/markise-ui-egui/src/updates.rs` and `src/app/updates.rs` (card, menu, glue),
`apps/markise/src/update_client.rs` (HTTP client).

## Behaviour

- **The app's only network request.** One `GET
  https://api.github.com/repos/adjioev/markise/releases?per_page=20` with
  `User-Agent: Markise/<version>` and `Accept: application/vnd.github+json`. No cookies,
  tokens, identifiers or telemetry. It runs on a background thread, with a 5 s connect and
  10 s total timeout; the body is capped at 1 MiB.
- **First launch.** A small, non-modal card in the bottom right corner asks "Check for updates
  automatically?" (Check Automatically / Not Now). It does not block typing and does not
  cover the welcome tutorial, which opens as a document. Until it is answered, automatic
  checks are off; if the app is closed without an answer it asks again at the next launch.
- **Preferences → General → Updates → Check for updates automatically** changes the answer.
  Off means the app never contacts the network on its own.
- **Automatic check.** At launch, at most once a day (24 h after the last check that got an
  answer from GitHub). An offline/timeout failure does not count, so the next launch tries
  again; an HTTP error such as rate limiting (403/429) counts. Automatic checks are silent
  unless a newer version is found.
- **Notice.** "Markise X.Y.Z is available — What's new · Download"; both links open the
  release page in the browser (only pages under `https://github.com/adjioev/markise/releases/`
  are opened). × dismisses the notice for that version (remembered); a newer version is shown
  again.
- **Check for Updates…** (macOS: application menu; Windows/Linux: Help menu) works even with
  automatic checks off and always reports a result: up to date, the notice (even for a
  dismissed version), or the error (offline, rate limited, HTTP status, unreadable answer).
- **Channels.** A prerelease build (e.g. `0.1.0-beta.1`) is offered newer prereleases and
  newer stable releases; a stable build only stable releases (neither GitHub pre-releases nor
  tags with a SemVer prerelease part). Drafts are skipped. Versions compare by SemVer
  precedence; build metadata is ignored.

## Builds

| Build | Default | Override |
|---|---|---|
| release (`--release`, `dist`) | checks allowed (after the user agreed) | `MARKISE_UPDATE_CHECK=0` turns them off |
| debug (`cargo run`) | off: no network | `MARKISE_UPDATE_CHECK=1` turns them on |

With checks off for the build, the first-launch question is not shown, Check for Updates…
explains why, and Preferences shows the reason under the switch. Tests and tools never get an
HTTP client (`AppOptions::updates` defaults to off).

## Settings

`updates.autoCheck` (omitted until answered = off), `updates.lastCheck` (Unix seconds),
`updates.dismissed` (version string) in `settings.json`; each is patched on its own, like the
other settings.

## Dependencies

`ureq` 3 (no default features: no gzip, no bundled root store) over `rustls` 0.23 with the
`ring` provider; certificates are verified by the operating system
(`rustls-platform-verifier`: Security.framework on macOS, the Windows certificate store,
`rustls-native-certs` on Linux). Licences: MIT/Apache-2.0, ISC (`rustls-webpki`, `untrusted`),
`Apache-2.0 AND ISC` (`ring`), BSD-3-Clause (`subtle`). `ring` compiles a small amount of C and
assembly (BoringSSL-derived) through `cc`. `semver` (MIT/Apache-2.0) in `markise-engine`.
