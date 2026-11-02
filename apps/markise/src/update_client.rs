//! The update check's HTTP client (issue #70): one `GET` of the GitHub releases list over
//! HTTPS (`ureq` + `rustls` with the `ring` provider; certificates verified by the operating
//! system through `rustls-platform-verifier`). What is checked and how often:
//! `markise_engine::update`.
//!
//! **Builds.** Release builds check (once the user agreed, see the first-launch question).
//! Debug builds do not contact the network unless `MARKISE_UPDATE_CHECK=1` is set, so
//! development runs and test scripts never reach GitHub. `MARKISE_UPDATE_CHECK=0` turns checks
//! off in any build (packagers, CI, offline machines); Check for Updates… then says so.

use std::sync::Arc;
use std::time::Duration;

use markise_engine::update::{self, ReleaseClient, Response, TransportError};
use markise_ui_egui::updates::UpdateSource;

/// Connecting (DNS + TCP + TLS) may take this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The whole request may take this long.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

struct GithubClient {
    agent: ureq::Agent,
}

impl GithubClient {
    fn new() -> GithubClient {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(TOTAL_TIMEOUT))
            // 4xx/5xx are answers (rate limiting is reported as such), not transport errors
            .http_status_as_error(false)
            .user_agent(update::user_agent(env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        GithubClient { agent }
    }
}

impl ReleaseClient for GithubClient {
    fn get_releases(&self) -> Result<Response, TransportError> {
        let mut r = self
            .agent
            .get(update::RELEASES_URL)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| TransportError(e.to_string()))?;
        let status = r.status().as_u16();
        let body = r
            .body_mut()
            .with_config()
            .limit(update::MAX_BODY)
            .read_to_string()
            .map_err(|e| TransportError(e.to_string()))?;
        Ok(Response { status, body })
    }
}

/// The update source of this process (see the module docs for the build rules).
pub fn source() -> UpdateSource {
    let env = std::env::var("MARKISE_UPDATE_CHECK").ok();
    match env.as_deref() {
        Some("0") => UpdateSource::Off("Update checks are turned off (MARKISE_UPDATE_CHECK=0).".into()),
        Some("1") => UpdateSource::Client(Arc::new(GithubClient::new())),
        _ if cfg!(debug_assertions) => UpdateSource::Off(
            "Development build: update checks are off (set MARKISE_UPDATE_CHECK=1 to enable them).".into(),
        ),
        _ => UpdateSource::Client(Arc::new(GithubClient::new())),
    }
}
