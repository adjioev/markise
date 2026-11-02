//! Update check (issue #70): is a newer Markise release out on GitHub?
//!
//! This module is GUI-free and network-free: it decides *whether* to check ([`due`]), parses
//! GitHub's release answers ([`parse_releases`], [`parse_latest`]), picks the newest release
//! the running version's channel may see ([`newest`]) and runs one check on a background
//! thread ([`spawn_check`]). The HTTP requests go through a [`ReleaseClient`] the caller
//! injects: the desktop app passes its TLS client (`apps/markise/src/update_client.rs`), tests
//! pass a mock, and a session without a client can never contact the network.
//!
//! **What is sent.** `GET` requests with a `User-Agent: Markise/<version>` header
//! ([`user_agent`]) and nothing else — no identifiers, cookies, tokens or telemetry; these are
//! the app's only network requests. A stable build asks for [`LATEST_URL`] (GitHub's latest
//! stable release) only; a beta build also asks for the newest releases ([`RELEASES_URL`],
//! one page), so a stable release is found however many prereleases came after it, and the
//! newest prerelease is found too.
//!
//! **Channels** ([`Channel`]). A prerelease build (`0.1.0-beta.1`) is on the beta channel and
//! is offered newer prereleases and newer stable releases; a stable build is offered stable
//! releases only (a release GitHub marks as a pre-release, or whose tag has a SemVer
//! prerelease part, is skipped). Drafts are always skipped (GitHub does not list them for
//! anonymous requests anyway). Versions compare by SemVer 2.0.0 precedence; build metadata
//! (`+…`) is ignored.
//!
//! **Throttling** ([`due`]): an automatic check runs at launch at most once per
//! [`CHECK_INTERVAL`] (a day) after the last contact with GitHub ([`Checked::contacted`]). A
//! failed connection (offline, timeout before an answer) does not count — the next launch tries
//! again; any HTTP answer does, including an error status such as rate limiting (HTTP 403/429)
//! and a body that could not be read.
//!
//! **Release page.** The page offered to the user ("What's new · Download") is the release's
//! `html_url`, accepted only when [`release_page`] validates it as a page of this
//! repository's releases; anything else falls back to [`RELEASES_PAGE`]. The UI opens only
//! URLs [`is_release_page`] accepts.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use semver::BuildMetadata;
pub use semver::Version;

use crate::Wake;

/// The GitHub repository whose releases are checked.
pub const REPOSITORY: &str = "adjioev/markise";

/// The newest releases, newest first (beta channel; includes prereleases).
pub const RELEASES_URL: &str = "https://api.github.com/repos/adjioev/markise/releases?per_page=20";

/// The latest stable release (never a prerelease or a draft; 404 when there is none).
pub const LATEST_URL: &str = "https://api.github.com/repos/adjioev/markise/releases/latest";

/// The repository's releases page (fallback when a release has no usable `html_url`).
pub const RELEASES_PAGE: &str = "https://github.com/adjioev/markise/releases";

/// Minimum time between two automatic checks (seconds).
pub const CHECK_INTERVAL: u64 = 24 * 60 * 60;

/// Largest response body accepted (bytes); the client stops reading after it.
pub const MAX_BODY: u64 = 1 << 20;

/// The `User-Agent` header: the app name and version, nothing else.
pub fn user_agent(version: &str) -> String {
    format!("Markise/{version}")
}

/// An HTTP answer: the status code and the body, or why the body could not be read (too
/// large, not UTF-8, the connection broke). Clients need not read the body of a non-2xx
/// answer (an empty body is fine). A received status always counts as contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Result<String, String>,
}

impl Response {
    pub fn ok(body: impl Into<String>) -> Response {
        Response {
            status: 200,
            body: Ok(body.into()),
        }
    }
}

/// The request got no HTTP answer (offline, DNS, TLS, timeout before the status line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError(pub String);

/// Performs one `GET` of an update check URL ([`RELEASES_URL`], [`LATEST_URL`]).
/// Implementations must use a short timeout and send no identifiers beyond [`user_agent`].
/// Called on a background thread, never on the UI thread.
pub trait ReleaseClient: Send + Sync {
    fn get(&self, url: &str) -> Result<Response, TransportError>;
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

/// One release from GitHub's answer, as parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// The tag's version (`v` prefix stripped; build metadata dropped); `None` when the tag is
    /// not SemVer.
    pub version: Option<Version>,
    pub prerelease: bool,
    pub draft: bool,
    /// The release page (validated by [`release_page`], else [`RELEASES_PAGE`]).
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
    /// The answer could not be read or is not a release list / release.
    Malformed(String),
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

/// A finished check: its outcome and whether GitHub answered at least one request (the
/// attempt then counts for throttling, whatever the outcome).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub outcome: Outcome,
    pub contacted: bool,
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

/// Decodes `%XX` escapes of one path segment; `None` for a malformed escape or non-UTF-8.
fn percent_decode(seg: &str) -> Option<String> {
    let b = seg.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `url` if it is a page of this repository's releases, else `None`. Strict on purpose:
/// scheme `https`, host exactly `github.com` (no userinfo, no port), no query, fragment or
/// backslash, and a path whose segments — percent-decoded — are non-empty printable ASCII
/// without `/`, `\` or dot segments (`.`, `..`, also as `%2e`), starting with
/// `adjioev/markise/releases/` and naming something below it. A browser therefore opens
/// exactly the page that was checked (no normalisation can lead to another repository).
pub fn release_page(url: &str) -> Option<String> {
    if url.len() > 300 {
        return None;
    }
    let path = url.strip_prefix("https://github.com/")?;
    if path
        .bytes()
        .any(|b| !b.is_ascii_graphic() || matches!(b, b'\\' | b'?' | b'#' | b'@'))
    {
        return None;
    }
    let mut segs = Vec::new();
    for raw in path.split('/') {
        let seg = percent_decode(raw)?;
        let bad = seg.is_empty()
            || seg == "."
            || seg == ".."
            || seg.bytes().any(|b| !b.is_ascii_graphic() || matches!(b, b'/' | b'\\'));
        if bad {
            return None;
        }
        segs.push(seg);
    }
    (segs.len() >= 4 && segs[..3] == ["adjioev", "markise", "releases"]).then(|| url.to_string())
}

/// The UI may open `url`: a validated release page or [`RELEASES_PAGE`].
pub fn is_release_page(url: &str) -> bool {
    url == RELEASES_PAGE || release_page(url).is_some()
}

fn listed(r: &serde_json::Map<String, serde_json::Value>) -> Option<Listed> {
    let tag = r.get("tag_name").and_then(|t| t.as_str())?;
    let page = r.get("html_url").and_then(|u| u.as_str()).and_then(release_page);
    Some(Listed {
        version: parse_version(tag),
        // missing flags: assume the cautious value (not offered to stable users / skipped)
        prerelease: r.get("prerelease").and_then(|b| b.as_bool()).unwrap_or(true),
        draft: r.get("draft").and_then(|b| b.as_bool()).unwrap_or(true),
        page: page.unwrap_or_else(|| RELEASES_PAGE.to_string()),
    })
}

/// Parses the releases list ([`RELEASES_URL`]: a JSON array of release objects).
pub fn parse_releases(body: &str) -> Result<Vec<Listed>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let serde_json::Value::Array(items) = v else {
        return Err("not a list of releases".into());
    };
    Ok(items.iter().filter_map(|r| r.as_object().and_then(listed)).collect())
}

/// Parses the latest release ([`LATEST_URL`]: one release object).
pub fn parse_latest(body: &str) -> Result<Listed, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    v.as_object().and_then(listed).ok_or_else(|| "not a release".into())
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

/// The releases one answer names, or why it names none.
fn read_answer(url: &str, answer: Result<Response, TransportError>) -> (bool, Result<Vec<Listed>, CheckError>) {
    let r = match answer {
        Ok(r) => r,
        Err(TransportError(e)) => return (false, Err(CheckError::Offline(e))),
    };
    let found = match r.status {
        200..=299 => match &r.body {
            Ok(b) if url == LATEST_URL => parse_latest(b).map(|l| vec![l]).map_err(CheckError::Malformed),
            Ok(b) => parse_releases(b).map_err(CheckError::Malformed),
            Err(e) => Err(CheckError::Malformed(e.clone())),
        },
        // no stable release published yet
        404 if url == LATEST_URL => Ok(Vec::new()),
        403 | 429 => Err(CheckError::RateLimited),
        s => Err(CheckError::Http(s)),
    };
    (true, found)
}

/// Interprets the answers to a check's requests (`(url, answer)` pairs): a newer release
/// found in any answer wins; otherwise the first failure is reported; otherwise up to date.
pub fn evaluate(current: &Version, answers: Vec<(&str, Result<Response, TransportError>)>) -> Checked {
    let mut contacted = false;
    let mut all = Vec::new();
    let mut error = None;
    for (url, a) in answers {
        let (c, found) = read_answer(url, a);
        contacted |= c;
        match found {
            Ok(l) => all.extend(l),
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    let outcome = match (newest(current, &all), error) {
        (Some(r), _) => Outcome::Available(r),
        (None, Some(e)) => Outcome::Failed(e),
        (None, None) => Outcome::UpToDate,
    };
    Checked { outcome, contacted }
}

/// The URLs a check of `current` requests (see the module docs).
pub fn urls(current: &Version) -> &'static [&'static str] {
    match Channel::of(current) {
        Channel::Stable => &[LATEST_URL],
        Channel::Beta => &[RELEASES_URL, LATEST_URL],
    }
}

/// Runs one check synchronously (the caller's thread does the requests).
pub fn check(client: &dyn ReleaseClient, current: &Version) -> Checked {
    let answers = urls(current).iter().map(|u| (*u, client.get(u))).collect();
    evaluate(current, answers)
}

/// Runs one check on a new background thread; the result arrives on the returned channel,
/// followed by a call of `wake`.
pub fn spawn_check(client: Arc<dyn ReleaseClient>, current: Version, wake: Wake) -> Receiver<Checked> {
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
        let _ = tx.send(Checked {
            outcome: Outcome::Failed(CheckError::Offline(format!("no worker thread: {e}"))),
            contacted: false,
        });
        return rx2;
    }
    rx
}

/// Whether the launch should check automatically: only when the user turned automatic checks
/// on (`auto`: `None` = not asked yet = off) and the last contact is at least
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
    use std::sync::Mutex;
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

    /// A mocked GitHub: answers per URL, records the URLs asked for.
    struct Mock {
        list: Result<Response, TransportError>,
        latest: Result<Response, TransportError>,
        asked: Mutex<Vec<String>>,
    }

    impl Mock {
        fn new(list: Result<Response, TransportError>, latest: Result<Response, TransportError>) -> Mock {
            Mock {
                list,
                latest,
                asked: Mutex::new(Vec::new()),
            }
        }

        /// GitHub with these releases (newest first): the list and the latest stable one.
        fn releases(items: &[(&str, bool, bool)]) -> Mock {
            let latest = items
                .iter()
                .find(|(_, p, d)| !p && !d)
                .map_or_else(|| status(404), |(t, p, d)| Response::ok(release_json(t, *p, *d)));
            Mock::new(Ok(Response::ok(list(items))), Ok(latest))
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    fn status(s: u16) -> Response {
        Response {
            status: s,
            body: Ok(String::new()),
        }
    }

    impl ReleaseClient for Mock {
        fn get(&self, url: &str) -> Result<Response, TransportError> {
            self.asked.lock().unwrap().push(url.to_string());
            if url == LATEST_URL {
                self.latest.clone()
            } else {
                self.list.clone()
            }
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
    fn release_pages_are_validated_strictly() {
        let ok = "https://github.com/adjioev/markise/releases/tag/v0.2.0";
        assert_eq!(release_page(ok).as_deref(), Some(ok));
        assert!(release_page("https://github.com/adjioev/markise/releases/tag/v0.2.0-beta.1%2Bx").is_some());
        for bad in [
            // dot segments, plain and percent-encoded (browsers normalise them away)
            "https://github.com/adjioev/markise/releases/../../../other/repo",
            "https://github.com/adjioev/markise/releases/./tag",
            "https://github.com/adjioev/markise/releases/%2e%2e/%2E%2E/%2e%2e/other/repo",
            "https://github.com/adjioev/markise/releases/.%2e/x",
            "https://github.com/adjioev/markise/releases/tag%2f..%2f..",
            "https://github.com/adjioev/markise/releases/tag\\..\\..",
            // userinfo, port, other hosts and schemes
            "https://evil@github.com/adjioev/markise/releases/tag/v1.0.0",
            "https://github.com@evil.example/adjioev/markise/releases/tag/v1.0.0",
            "https://github.com:8443/adjioev/markise/releases/tag/v1.0.0",
            "https://github.com.evil.example/adjioev/markise/releases/tag/v1.0.0",
            "https://evil.example/adjioev/markise/releases/tag/v1.0.0",
            "http://github.com/adjioev/markise/releases/tag/v1.0.0",
            "HTTPS://GITHUB.COM/adjioev/markise/releases/tag/v1.0.0",
            // other repositories, nothing below releases/, query, fragment, empty segments
            "https://github.com/adjioev/markise-archive/releases/tag/v1.0.0",
            "https://github.com/adjioev/markise/releases/",
            "https://github.com/adjioev/markise/releases",
            "https://github.com/adjioev/markise/releases/tag/v1?x=1",
            "https://github.com/adjioev/markise/releases/tag/v1#x",
            "https://github.com/adjioev/markise/releases//tag",
            "https://github.com/adjioev/markise/releases/tag/%zz",
            "https://github.com/adjioev/markise/releases/tag/%00",
            "https://github.com/adjioev/markise/releases/tag/a b",
        ] {
            assert_eq!(release_page(bad), None, "{bad}");
            // (the bare releases page is not a release page, but the UI may open it)
            assert_eq!(is_release_page(bad), bad == RELEASES_PAGE, "{bad}");
        }
        assert!(is_release_page(RELEASES_PAGE));
        // a foreign html_url falls back to the releases page
        let body = r#"[{"tag_name":"v2.0.0","prerelease":false,"draft":false,"html_url":"https://github.com/adjioev/markise/releases/../../x/y"},
                       {"tag_name":"v1.5.0","prerelease":false,"draft":false}]"#;
        assert!(parse_releases(body).unwrap().iter().all(|r| r.page == RELEASES_PAGE));
    }

    #[test]
    fn stable_builds_ask_for_the_latest_stable_release_only() {
        // twenty-five newer betas after the stable release: the first list page has no stable one
        let mut items: Vec<(String, bool, bool)> = (1..=25)
            .rev()
            .map(|n| (format!("v0.3.0-beta.{n}"), true, false))
            .collect();
        items.push(("v0.2.0".into(), false, false));
        let items: Vec<(&str, bool, bool)> = items.iter().map(|(t, p, d)| (t.as_str(), *p, *d)).collect();
        let first_page = list(&items[..20]);
        let latest = Response::ok(release_json("v0.2.0", false, false));
        let m = Mock::new(Ok(Response::ok(first_page)), Ok(latest));
        let out = check(&m, &v("0.1.0"));
        assert!(
            matches!(&out.outcome, Outcome::Available(r) if r.version == v("0.2.0")),
            "{out:?}"
        );
        assert_eq!(m.asked(), [LATEST_URL]);
        // a beta build asks for both and gets the newest beta
        let out = check(&m, &v("0.1.0-beta.1"));
        assert!(matches!(&out.outcome, Outcome::Available(r) if r.version == v("0.3.0-beta.25")));
        assert_eq!(m.asked(), [LATEST_URL, RELEASES_URL, LATEST_URL]);
        // a beta build behind a stable release that is no longer on the first page
        let m = Mock::new(
            Ok(Response::ok(list(&items[..20]))),
            Ok(Response::ok(release_json("v1.0.0", false, false))),
        );
        let out = check(&m, &v("0.9.0-beta.1"));
        assert!(matches!(&out.outcome, Outcome::Available(r) if r.version == v("1.0.0")));
        // no stable release yet: 404 is "none", not an error
        let m = Mock::releases(&[("v0.1.0-beta.1", true, false)]);
        assert_eq!(check(&m, &v("0.1.0-beta.1")).outcome, Outcome::UpToDate);
        assert_eq!(check(&m, &v("0.0.1")).outcome, Outcome::UpToDate);
    }

    #[test]
    fn mocked_responses_map_to_outcomes() {
        let cur = v("0.1.0-beta.1");
        let newer = Mock::releases(&[("v0.1.0-beta.2", true, false)]);
        let out = check(&newer, &cur);
        assert!(matches!(&out.outcome, Outcome::Available(r) if r.version == v("0.1.0-beta.2")));
        assert!(out.contacted);
        let same = Mock::releases(&[("v0.1.0-beta.1", true, false)]);
        assert_eq!(check(&same, &cur).outcome, Outcome::UpToDate);
        let older = Mock::releases(&[("v0.0.5", false, false)]);
        assert_eq!(check(&older, &cur).outcome, Outcome::UpToDate);
        let empty = Mock::new(Ok(Response::ok("[]")), Ok(status(404)));
        assert_eq!(check(&empty, &cur).outcome, Outcome::UpToDate);
        let prerelease_for_stable = Mock::releases(&[("v0.2.0-beta.1", true, false)]);
        assert_eq!(check(&prerelease_for_stable, &v("0.1.0")).outcome, Outcome::UpToDate);
        for bad in ["<html>", "{\"message\":\"x\"}", ""] {
            let m = Mock::new(Ok(Response::ok(bad)), Ok(Response::ok(bad)));
            let out = check(&m, &cur);
            assert!(
                matches!(out.outcome, Outcome::Failed(CheckError::Malformed(_))),
                "{bad}"
            );
            assert!(out.contacted);
            assert!(matches!(
                check(&m, &v("0.1.0")).outcome,
                Outcome::Failed(CheckError::Malformed(_))
            ));
        }
        // entries without a tag are skipped, not fatal
        let partial = Mock::new(Ok(Response::ok(r#"[{"name":"x"}, 5, null]"#)), Ok(status(404)));
        assert_eq!(check(&partial, &cur).outcome, Outcome::UpToDate);
        // a newer release in one answer wins over an error in the other
        let half = Mock::new(
            Ok(Response::ok(list(&[("v0.2.0-beta.1", true, false)]))),
            Ok(status(500)),
        );
        assert!(matches!(check(&half, &cur).outcome, Outcome::Available(_)));
    }

    #[test]
    fn errors_rate_limits_and_unreadable_bodies_are_reported_and_count_as_contact() {
        let cur = v("0.1.0");
        let both = |r: Result<Response, TransportError>| Mock::new(r.clone(), r);
        assert_eq!(
            check(&both(Ok(status(403))), &cur).outcome,
            Outcome::Failed(CheckError::RateLimited)
        );
        assert_eq!(
            check(&both(Ok(status(429))), &cur).outcome,
            Outcome::Failed(CheckError::RateLimited)
        );
        assert_eq!(
            check(&both(Ok(status(500))), &cur).outcome,
            Outcome::Failed(CheckError::Http(500))
        );
        assert!(check(&both(Ok(status(429))), &cur).contacted);
        // a status arrived but the body did not (oversized, broken off): contact, malformed
        let cut = both(Ok(Response {
            status: 200,
            body: Err("body exceeds the limit".into()),
        }));
        let out = check(&cut, &cur);
        assert_eq!(
            out.outcome,
            Outcome::Failed(CheckError::Malformed("body exceeds the limit".into()))
        );
        assert!(out.contacted, "an answer counts for throttling");
        // no answer at all: offline, not counted
        let offline = both(Err(TransportError("dns error".into())));
        let out = check(&offline, &cur);
        assert!(matches!(&out.outcome, Outcome::Failed(CheckError::Offline(e)) if e == "dns error"));
        assert!(!out.contacted);
        // beta: one request answered, the other offline: counted
        let mixed = Mock::new(Err(TransportError("reset".into())), Ok(status(404)));
        assert!(check(&mixed, &v("0.1.0-beta.1")).contacted);
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
        let client: Arc<dyn ReleaseClient> = Arc::new(Mock::releases(&[("v0.2.0", false, false)]));
        let rx = spawn_check(
            client,
            v("0.1.0"),
            Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let out = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(matches!(out.outcome, Outcome::Available(r) if r.version == v("0.2.0")));
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
