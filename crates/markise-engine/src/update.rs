//! Update check (issue #70): is a newer Markise release out on GitHub?
//!
//! This module is GUI-free and network-free: it decides *whether* to check ([`due`]), parses
//! the GitHub releases list ([`parse_releases`]), picks the newest release the running
//! version's channel may see ([`newest`]) and runs one check on a background thread
//! ([`spawn_check`]). The HTTP request itself goes through a [`ReleaseClient`] the caller
//! injects: the desktop app passes its TLS client (`apps/markise/src/update_client.rs`), tests
//! pass a mock, and a session without a client can never contact the network.
//!
//! **What is sent.** One `GET` of [`RELEASES_URL`] with a `User-Agent: Markise/<version>`
//! header ([`user_agent`]) — no identifiers, cookies, tokens or telemetry. It is the app's only
//! network request.
//!
//! **Channels** ([`Channel`]). A prerelease build (`0.1.0-beta.1`) is on the beta channel and
//! is offered newer prereleases and newer stable releases; a stable build is offered stable
//! releases only (a release GitHub marks as a pre-release, or whose tag has a SemVer
//! prerelease part, is skipped). Drafts are always skipped (GitHub does not list them for
//! anonymous requests anyway). Versions compare by SemVer 2.0.0 precedence; build metadata
//! (`+…`) is ignored.
//!
//! **Throttling** ([`due`]): an automatic check runs at launch at most once per
//! [`CHECK_INTERVAL`] (a day) after the last completed contact with GitHub. A failed
//! connection (offline, timeout) does not count — the next launch tries again; an answer from
//! GitHub does, including an error status such as rate limiting (HTTP 403/429).
//!
//! **Release page.** The page offered to the user ("What's new · Download") is the release's
//! `html_url`, accepted only when it is a page of this repository's releases
//! ([`RELEASE_PAGE_PREFIX`]); anything else falls back to the repository's releases page.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use semver::BuildMetadata;
pub use semver::Version;

use crate::Wake;

/// The GitHub repository whose releases are checked.
pub const REPOSITORY: &str = "adjioev/markise";

/// The releases list (newest first). `/releases/latest` is not used: it never returns a
/// pre-release, which the beta channel needs.
pub const RELEASES_URL: &str = "https://api.github.com/repos/adjioev/markise/releases?per_page=20";

/// Release pages the app opens in the browser start with this.
pub const RELEASE_PAGE_PREFIX: &str = "https://github.com/adjioev/markise/releases/";

/// Fallback page when a release has no usable `html_url`.
pub const RELEASES_PAGE: &str = "https://github.com/adjioev/markise/releases";

/// Minimum time between two automatic checks (seconds).
pub const CHECK_INTERVAL: u64 = 24 * 60 * 60;

/// Largest response body accepted (bytes); the client stops reading after it.
pub const MAX_BODY: u64 = 1 << 20;

/// The `User-Agent` header: the app name and version, nothing else.
pub fn user_agent(version: &str) -> String {
    format!("Markise/{version}")
}

/// An HTTP answer: status code and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// The request did not get an HTTP answer (offline, DNS, TLS, timeout, oversized body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError(pub String);

/// Performs the one request of an update check: `GET` [`RELEASES_URL`]. Implementations must
/// use a short timeout and send no identifiers beyond [`user_agent`]. Called on a background
/// thread, never on the UI thread.
pub trait ReleaseClient: Send + Sync {
    fn get_releases(&self) -> Result<Response, TransportError>;
}

/// Which releases a build is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Stable releases only.
    Stable,
    /// Prereleases and stable releases.
    Beta,
}

impl Channel {
    /// The channel of the running version: a prerelease build is on the beta channel.
    pub fn of(current: &Version) -> Channel {
        if current.pre.is_empty() {
            Channel::Stable
        } else {
            Channel::Beta
        }
    }
}

/// One entry of the releases list, as parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// The tag's version (`v` prefix stripped; build metadata dropped); `None` when the tag is
    /// not SemVer.
    pub version: Option<Version>,
    pub prerelease: bool,
    pub draft: bool,
    /// The release page (validated, see [`RELEASE_PAGE_PREFIX`]).
    pub page: String,
}

/// A newer release the user may install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page ("What's new · Download").
    pub page: String,
}

/// Why a check found nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    /// No HTTP answer (offline, timeout, TLS failure).
    Offline(String),
    /// GitHub's rate limit for anonymous requests (HTTP 403 or 429).
    RateLimited,
    /// Another HTTP error status.
    Http(u16),
    /// The answer is not a releases list.
    Malformed(String),
}

impl CheckError {
    /// GitHub answered (the attempt counts for throttling).
    pub fn contacted(&self) -> bool {
        !matches!(self, CheckError::Offline(_))
    }
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Offline(e) => write!(f, "GitHub could not be reached ({e})."),
            CheckError::RateLimited => f.write_str("GitHub is limiting requests right now. Try again later."),
            CheckError::Http(s) => write!(f, "GitHub answered with HTTP status {s}."),
            CheckError::Malformed(e) => write!(f, "GitHub's answer could not be read ({e})."),
        }
    }
}

/// The result of one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No newer release for this channel.
    UpToDate,
    Available(Release),
    Failed(CheckError),
}

impl Outcome {
    /// GitHub answered (the attempt counts for throttling).
    pub fn contacted(&self) -> bool {
        match self {
            Outcome::Failed(e) => e.contacted(),
            _ => true,
        }
    }
}

/// Parses a version as written in a tag or `Cargo.toml` (`v` prefix allowed; build metadata
/// dropped, so it never affects comparisons).
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let mut v = Version::parse(s).ok()?;
    v.build = BuildMetadata::EMPTY;
    Some(v)
}

fn valid_page(url: Option<&str>) -> String {
    match url {
        Some(u) if u.starts_with(RELEASE_PAGE_PREFIX) && u.len() <= 300 && u.bytes().all(|b| b.is_ascii_graphic()) => {
            u.to_string()
        }
        _ => RELEASES_PAGE.to_string(),
    }
}

/// Parses the GitHub releases list (a JSON array of release objects).
pub fn parse_releases(body: &str) -> Result<Vec<Listed>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let serde_json::Value::Array(items) = v else {
        return Err("not a list of releases".into());
    };
    Ok(items
        .iter()
        .filter_map(|r| {
            let r = r.as_object()?;
            let tag = r.get("tag_name").and_then(|t| t.as_str())?;
            Some(Listed {
                version: parse_version(tag),
                // missing flags: assume the cautious value (not offered to stable users / skipped)
                prerelease: r.get("prerelease").and_then(|b| b.as_bool()).unwrap_or(true),
                draft: r.get("draft").and_then(|b| b.as_bool()).unwrap_or(true),
                page: valid_page(r.get("html_url").and_then(|u| u.as_str())),
            })
        })
        .collect())
}

/// The newest listed release that is newer than `current` and visible on its channel.
pub fn newest(current: &Version, listed: &[Listed]) -> Option<Release> {
    let channel = Channel::of(current);
    let mut current = current.clone();
    current.build = BuildMetadata::EMPTY;
    listed
        .iter()
        .filter(|l| !l.draft)
        .filter_map(|l| {
            let v = l.version.as_ref()?;
            let stable = !l.prerelease && v.pre.is_empty();
            (channel == Channel::Beta || stable).then_some((v, l))
        })
        .filter(|(v, _)| **v > current)
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(v, l)| Release {
            version: v.clone(),
            page: l.page.clone(),
        })
}

/// Interprets the answer to the releases request.
pub fn evaluate(current: &Version, answer: Result<Response, TransportError>) -> Outcome {
    let r = match answer {
        Ok(r) => r,
        Err(TransportError(e)) => return Outcome::Failed(CheckError::Offline(e)),
    };
    match r.status {
        200 => match parse_releases(&r.body) {
            Ok(listed) => newest(current, &listed).map_or(Outcome::UpToDate, Outcome::Available),
            Err(e) => Outcome::Failed(CheckError::Malformed(e)),
        },
        403 | 429 => Outcome::Failed(CheckError::RateLimited),
        s => Outcome::Failed(CheckError::Http(s)),
    }
}

/// Runs one check synchronously (the caller's thread does the request).
pub fn check(client: &dyn ReleaseClient, current: &Version) -> Outcome {
    evaluate(current, client.get_releases())
}

/// Runs one check on a new background thread; the outcome arrives on the returned channel,
/// followed by a call of `wake`.
pub fn spawn_check(client: Arc<dyn ReleaseClient>, current: Version, wake: Wake) -> Receiver<Outcome> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("markise-update-check".into())
        .spawn(move || {
            let out = check(client.as_ref(), &current);
            let published = crate::wake_track::publishing();
            let _ = tx.send(out);
            wake();
            drop(published);
        });
    if let Err(e) = spawned {
        // no thread: report it as a failed check (the sender went with the closure)
        let (tx, rx2) = channel();
        let _ = tx.send(Outcome::Failed(CheckError::Offline(format!("no worker thread: {e}"))));
        return rx2;
    }
    rx
}

/// Whether the launch should check automatically: only when the user turned automatic checks
/// on (`auto`: `None` = not asked yet = off) and the last completed check is at least
/// [`CHECK_INTERVAL`] old. A last check in the future (clock moved back) is treated as stale.
pub fn due(auto: Option<bool>, last_check: Option<u64>, now: u64) -> bool {
    if auto != Some(true) {
        return false;
    }
    match last_check {
        None => true,
        Some(t) if t > now => true,
        Some(t) => now - t >= CHECK_INTERVAL,
    }
}

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn v(s: &str) -> Version {
        parse_version(s).unwrap()
    }

    fn release_json(tag: &str, prerelease: bool, draft: bool) -> String {
        format!(
            r#"{{"tag_name":"{tag}","prerelease":{prerelease},"draft":{draft},"html_url":"https://github.com/adjioev/markise/releases/tag/{tag}","name":"Markise {tag}","body":"notes"}}"#
        )
    }

    fn list(items: &[(&str, bool, bool)]) -> String {
        let parts: Vec<String> = items.iter().map(|(t, p, d)| release_json(t, *p, *d)).collect();
        format!("[{}]", parts.join(","))
    }

    struct Mock {
        answer: Result<Response, TransportError>,
        calls: AtomicUsize,
    }

    impl Mock {
        fn ok(body: String) -> Mock {
            Mock {
                answer: Ok(Response { status: 200, body }),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl ReleaseClient for Mock {
        fn get_releases(&self) -> Result<Response, TransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    #[test]
    fn versions_compare_by_semver_precedence_including_prereleases() {
        let order = [
            "0.1.0-alpha.1",
            "0.1.0-beta.1",
            "0.1.0-beta.2",
            "0.1.0-beta.10",
            "0.1.0-rc.1",
            "0.1.0",
            "0.1.1-beta.1",
            "0.1.1",
            "0.2.0",
            "1.0.0",
        ];
        for w in order.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        // `v` prefix and build metadata do not matter
        assert_eq!(v("v0.2.0+build.7"), v("0.2.0"));
        assert_eq!(parse_version("not-a-version"), None);
        assert_eq!(parse_version("1.2"), None);
    }

    #[test]
    fn channel_follows_the_running_version() {
        assert_eq!(Channel::of(&v("0.1.0-beta.1")), Channel::Beta);
        assert_eq!(Channel::of(&v("0.1.0")), Channel::Stable);
        // the workspace version parses (the app passes it)
        assert!(parse_version(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn beta_users_see_newer_betas_and_stable_releases() {
        let l = parse_releases(&list(&[("v0.1.0-beta.2", true, false), ("v0.1.0-beta.1", true, false)])).unwrap();
        assert_eq!(newest(&v("0.1.0-beta.1"), &l).unwrap().version, v("0.1.0-beta.2"));
        let l = parse_releases(&list(&[
            ("v0.1.0", false, false),
            ("v0.1.0-beta.3", true, false),
            ("v0.1.0-beta.2", true, false),
        ]))
        .unwrap();
        let r = newest(&v("0.1.0-beta.2"), &l).unwrap();
        assert_eq!(r.version, v("0.1.0"));
        assert_eq!(r.page, "https://github.com/adjioev/markise/releases/tag/v0.1.0");
    }

    #[test]
    fn stable_users_see_stable_releases_only() {
        let l = parse_releases(&list(&[
            ("v0.3.0-beta.1", true, false),
            // a SemVer prerelease tag published without GitHub's pre-release flag
            ("v0.2.1-rc.1", false, false),
            ("v0.2.0", false, false),
        ]))
        .unwrap();
        assert_eq!(newest(&v("0.1.0"), &l).unwrap().version, v("0.2.0"));
        assert_eq!(newest(&v("0.2.0"), &l), None);
        // a stable-looking tag GitHub marks as a pre-release is not offered either
        let l = parse_releases(&list(&[("v0.4.0", true, false)])).unwrap();
        assert_eq!(newest(&v("0.2.0"), &l), None);
        assert_eq!(newest(&v("0.2.0-beta.1"), &l).unwrap().version, v("0.4.0"));
    }

    #[test]
    fn drafts_older_and_equal_releases_are_not_offered() {
        let l = parse_releases(&list(&[
            ("v9.0.0", false, true),
            ("v0.1.0-beta.1", true, false),
            ("v0.0.9", false, false),
            ("nightly", false, false),
        ]))
        .unwrap();
        assert_eq!(newest(&v("0.1.0-beta.1"), &l), None);
    }

    #[test]
    fn foreign_release_pages_fall_back_to_the_releases_page() {
        let body = r#"[{"tag_name":"v2.0.0","prerelease":false,"draft":false,"html_url":"https://evil.example/x"},
                       {"tag_name":"v1.5.0","prerelease":false,"draft":false}]"#;
        let l = parse_releases(body).unwrap();
        assert!(l.iter().all(|r| r.page == RELEASES_PAGE));
    }

    #[test]
    fn mocked_responses_map_to_outcomes() {
        let cur = v("0.1.0-beta.1");
        let newer = Mock::ok(list(&[("v0.1.0-beta.2", true, false)]));
        assert!(matches!(check(&newer, &cur), Outcome::Available(r) if r.version == v("0.1.0-beta.2")));
        assert_eq!(newer.calls.load(Ordering::SeqCst), 1);
        let same = Mock::ok(list(&[("v0.1.0-beta.1", true, false)]));
        assert_eq!(check(&same, &cur), Outcome::UpToDate);
        let older = Mock::ok(list(&[("v0.0.5", false, false)]));
        assert_eq!(check(&older, &cur), Outcome::UpToDate);
        let empty = Mock::ok("[]".into());
        assert_eq!(check(&empty, &cur), Outcome::UpToDate);
        let prerelease_for_stable = Mock::ok(list(&[("v0.2.0-beta.1", true, false)]));
        assert_eq!(check(&prerelease_for_stable, &v("0.1.0")), Outcome::UpToDate);
        for bad in ["<html>", "{\"message\":\"x\"}", ""] {
            let m = Mock::ok(bad.into());
            assert!(
                matches!(check(&m, &cur), Outcome::Failed(CheckError::Malformed(_))),
                "{bad}"
            );
        }
        // entries without a tag are skipped, not fatal
        let partial = Mock::ok(r#"[{"name":"x"}, 5, null]"#.into());
        assert_eq!(check(&partial, &cur), Outcome::UpToDate);
    }

    #[test]
    fn errors_and_rate_limits_are_reported() {
        let cur = v("0.1.0");
        let status = |s: u16| Mock {
            answer: Ok(Response {
                status: s,
                body: "{\"message\":\"API rate limit exceeded\"}".into(),
            }),
            calls: AtomicUsize::new(0),
        };
        assert_eq!(check(&status(403), &cur), Outcome::Failed(CheckError::RateLimited));
        assert_eq!(check(&status(429), &cur), Outcome::Failed(CheckError::RateLimited));
        assert_eq!(check(&status(500), &cur), Outcome::Failed(CheckError::Http(500)));
        let offline = Mock {
            answer: Err(TransportError("dns error".into())),
            calls: AtomicUsize::new(0),
        };
        let out = check(&offline, &cur);
        assert!(matches!(&out, Outcome::Failed(CheckError::Offline(e)) if e == "dns error"));
        // offline attempts do not count for throttling; answers from GitHub do
        assert!(!out.contacted());
        assert!(Outcome::Failed(CheckError::RateLimited).contacted());
        assert!(Outcome::UpToDate.contacted());
    }

    #[test]
    fn automatic_checks_run_at_most_once_a_day_and_only_when_on() {
        let now = 1_800_000_000;
        assert!(!due(None, None, now), "not asked yet = off");
        assert!(!due(Some(false), None, now));
        assert!(due(Some(true), None, now));
        assert!(!due(Some(true), Some(now - 60), now));
        assert!(!due(Some(true), Some(now - CHECK_INTERVAL + 1), now));
        assert!(due(Some(true), Some(now - CHECK_INTERVAL), now));
        // clock moved back past the last check
        assert!(due(Some(true), Some(now + 3600), now));
    }

    #[test]
    fn background_check_delivers_the_outcome_and_wakes() {
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        let client: Arc<dyn ReleaseClient> = Arc::new(Mock::ok(list(&[("v0.2.0", false, false)])));
        let rx = spawn_check(
            client,
            v("0.1.0"),
            Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let out = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(matches!(out, Outcome::Available(r) if r.version == v("0.2.0")));
        // the wake follows the send
        let t = std::time::Instant::now();
        while woke.load(Ordering::SeqCst) == 0 && t.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert_eq!(woke.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn user_agent_carries_only_the_version() {
        assert_eq!(user_agent("0.1.0-beta.1"), "Markise/0.1.0-beta.1");
    }
}
