# Updates and privacy

Markise works offline. Your documents, settings, drafts and logs stay on your computer. There
is no account, no telemetry and no analytics. The only network requests the app makes are the
update checks described here — and only when you agree to them or ask for one.

## Automatic update checks are opt-in

On first launch, a small card in the corner asks **"Check for updates automatically?"**:

- **Check Automatically** turns automatic checks on.
- **Not Now** leaves them off. Until you answer, they stay off; if you close Markise without
  answering, it asks again at the next launch.

Change your answer any time in **Preferences → General → Updates → Check for updates
automatically**. With it off, Markise never contacts the network on its own.

With it on, Markise checks at launch, at most once a day. If you are offline, it tries again at
the next launch. An automatic check stays silent unless a newer version exists.

## Manual checks

**Check for Updates…** (on macOS in the **Markise** menu, on Windows and Linux in the **Help**
menu) checks right away, even with automatic checks off, and always reports the result: up to
date, a newer version, or the error (for example offline or rate-limited).

## What a check sends

A check is an HTTPS `GET` request to GitHub's public API for this repository's releases:

- A **stable** build requests `https://api.github.com/repos/adjioev/markise/releases/latest`.
- A **beta** build, such as 0.1.0-beta.1, requests
  `https://api.github.com/repos/adjioev/markise/releases?per_page=20` **and** the address
  above, so it finds both newer betas and newer stable releases.

Each request carries the headers `User-Agent: Markise/<version>` and
`Accept: application/vnd.github+json` — no cookies, tokens, identifiers or information about
your documents. Like any web request, it lets **GitHub see your IP address and the Markise
version**; GitHub's own privacy statement applies to that. Markise sends nothing anywhere else.

## When an update is available

A notice says "Markise X.Y.Z is available" with **What's new** and **Download** links. Both open
the release page on github.com in your browser; Markise only opens release pages of this
project. You download and install the new version yourself: **Markise never downloads or
installs updates by itself.** The × button hides the notice for that version.

Betas are offered newer betas and newer stable releases; stable versions are offered stable
releases only.

## Turning checks off completely

Besides the preference, the environment variable `MARKISE_UPDATE_CHECK=0` turns update checks
off for a release build, including the first-launch question. Builds you compile yourself in
debug mode never check unless `MARKISE_UPDATE_CHECK=1` is set.

## What stays local

- **Log file** `markise.log`: start lines (version, operating system), renderer information,
  fatal errors and crash locations with a backtrace — never document contents. It is kept under
  1 MiB, with the previous part in `markise.log.1`.

  | System | Log folder |
  |---|---|
  | macOS | `~/Library/Logs/Markise/` |
  | Windows | `%LOCALAPPDATA%\Markise\logs\` |
  | Linux | `~/.local/state/markise/logs/` (or `$XDG_STATE_HOME/markise/logs/`) |

- **Settings, themes and draft backups** live in Markise's configuration folder (see
  [Settings](settings.md#the-settings-file) and [Saving and recovery](saving-and-recovery.md)).
- **Recent files** are remembered locally unless you turn that off in **Preferences → Files &
  Images**.

Remote images in your documents (`https://…`) are not downloaded, neither for display nor
for export. Links open in your browser only when you <kbd>Cmd</kbd>/<kbd>Ctrl</kbd>-click them.
