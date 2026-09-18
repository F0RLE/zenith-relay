use super::{collect_response_body, valid_codex_client_version, ResponseBodyError};
use reqwest::{
    header::{HeaderName, ACCEPT},
    redirect::Policy,
};
use semver::Version;
use serde::Deserialize;
use std::time::Duration;
use url::Url;

pub const CODEX_RELEASES_API_URL: &str =
    "https://api.github.com/repos/openai/codex/releases/latest";
pub const CODEX_RELEASE_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);

const MAX_RELEASE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const RELEASE_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const RELEASE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_API_VERSION_HEADER: HeaderName = HeaderName::from_static("x-github-api-version");
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const RELEASE_USER_AGENT: &str = "Zenith-Relay";

/// A verified stable Rust Codex release. It lives only in process memory;
/// Relay never writes a release-version JSON file to the user's storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRelease {
    version: String,
}

impl CodexRelease {
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexReleaseError {
    Client,
    Transport,
    HttpStatus(u16),
    ResponseTooLarge,
    InvalidResponse,
}

impl std::fmt::Display for CodexReleaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client => formatter.write_str("Codex release client is unavailable"),
            Self::Transport => formatter.write_str("Codex release request failed"),
            Self::HttpStatus(status) => {
                write!(formatter, "Codex release request returned HTTP {status}")
            }
            Self::ResponseTooLarge => formatter.write_str("Codex release response is too large"),
            Self::InvalidResponse => formatter.write_str("Codex release response is invalid"),
        }
    }
}

impl std::error::Error for CodexReleaseError {}

#[derive(Clone, Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Fetches the latest stable Rust Codex release from the official GitHub
/// release endpoint. GitHub's `latest` endpoint excludes prereleases; the
/// explicit flags and SemVer check below enforce that contract defensively as
/// well. The caller applies the result to its process-wide identity.
///
/// This deliberately has no disk cache: Relay starts immediately on its
/// compiled, validated stable fallback and performs this network operation in
/// a background worker after the host has loaded. A failed or rate-limited
/// lookup therefore cannot delay startup or replace the fallback.
pub async fn refresh_codex_client_release() -> Result<CodexRelease, CodexReleaseError> {
    let endpoint = Url::parse(CODEX_RELEASES_API_URL).map_err(|_| CodexReleaseError::Client)?;
    refresh_codex_client_release_from_endpoint(&endpoint).await
}

async fn refresh_codex_client_release_from_endpoint(
    endpoint: &Url,
) -> Result<CodexRelease, CodexReleaseError> {
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(RELEASE_CONNECT_TIMEOUT)
        .timeout(RELEASE_REQUEST_TIMEOUT)
        .user_agent(RELEASE_USER_AGENT)
        .build()
        .map_err(|_| CodexReleaseError::Client)?;
    let response = client
        .get(endpoint.clone())
        .header(ACCEPT, GITHUB_ACCEPT)
        .header(GITHUB_API_VERSION_HEADER, GITHUB_API_VERSION)
        .send()
        .await
        .map_err(|_| CodexReleaseError::Transport)?;
    if !response.status().is_success() {
        return Err(CodexReleaseError::HttpStatus(response.status().as_u16()));
    }
    let body = collect_response_body(response, MAX_RELEASE_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseBodyError::Transport => CodexReleaseError::Transport,
            ResponseBodyError::TooLarge => CodexReleaseError::ResponseTooLarge,
        })?;
    let version =
        parse_stable_rust_release_response(&body).ok_or(CodexReleaseError::InvalidResponse)?;
    Ok(CodexRelease { version })
}

fn parse_stable_rust_release_response(body: &[u8]) -> Option<String> {
    let release: GitHubRelease = serde_json::from_slice(body).ok()?;
    (!release.draft && !release.prerelease)
        .then(|| parse_stable_rust_release_tag(&release.tag_name))
        .flatten()
        .map(|version| version.to_string())
}

fn parse_stable_rust_release_tag(tag: &str) -> Option<Version> {
    let version = tag.strip_prefix("rust-v")?;
    if version.is_empty() || version.len() > 64 || !valid_codex_client_version(version) {
        return None;
    }
    let version = Version::parse(version).ok()?;
    version.pre.is_empty().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{response::IntoResponse, routing::get, Router};

    #[test]
    fn latest_stable_rust_release_excludes_drafts_and_prereleases() {
        let stable = br#"{"tag_name":"rust-v0.154.0","draft":false,"prerelease":false}"#;
        assert_eq!(
            parse_stable_rust_release_response(stable).as_deref(),
            Some("0.154.0")
        );

        for response in [
            br#"{"tag_name":"rust-v0.155.0-alpha.3.7","draft":false,"prerelease":true}"#.as_slice(),
            br#"{"tag_name":"rust-v0.156.0-beta.1","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"rust-v9.0.0","draft":true,"prerelease":false}"#.as_slice(),
        ] {
            assert_eq!(parse_stable_rust_release_response(response), None);
        }
    }

    #[test]
    fn malformed_or_non_stable_tags_cannot_choose_a_version() {
        for response in [
            br#"{"tag_name":"rust-vnot-semver","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"rust-v0.155","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"rust-v0.155.0\nheader","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"desktop-v9.0.0","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"rust-v1.0.0-rc.1","draft":false,"prerelease":false}"#.as_slice(),
            br#"[]"#.as_slice(),
        ] {
            assert_eq!(parse_stable_rust_release_response(response), None);
        }
        assert!(parse_stable_rust_release_tag("rust-v0.154.0").is_some());
        assert!(parse_stable_rust_release_tag("rust-v0.155.0-alpha.3").is_none());
        assert!(parse_stable_rust_release_tag("Rust-v0.154.0").is_none());
    }

    #[tokio::test]
    async fn refresh_uses_the_published_stable_release() {
        let app = Router::new().route("/releases", get(stable_release_handler));
        let (endpoint, server) = spawn(app).await;

        let release = refresh_codex_client_release_from_endpoint(&endpoint)
            .await
            .unwrap();
        assert_eq!(release.version(), "0.154.0");

        server.abort();
    }

    #[tokio::test]
    async fn refresh_rejects_oversized_or_invalid_responses() {
        let oversized = Router::new().route("/releases", get(oversized_handler));
        let (oversized_endpoint, oversized_server) = spawn(oversized).await;
        assert_eq!(
            refresh_codex_client_release_from_endpoint(&oversized_endpoint)
                .await
                .unwrap_err(),
            CodexReleaseError::ResponseTooLarge
        );
        oversized_server.abort();

        let invalid = Router::new().route("/releases", get(|| async { "not-json" }));
        let (invalid_endpoint, invalid_server) = spawn(invalid).await;
        assert_eq!(
            refresh_codex_client_release_from_endpoint(&invalid_endpoint)
                .await
                .unwrap_err(),
            CodexReleaseError::InvalidResponse
        );
        invalid_server.abort();
    }

    async fn stable_release_handler() -> impl IntoResponse {
        r#"{"tag_name":"rust-v0.154.0","draft":false,"prerelease":false}"#
    }

    async fn oversized_handler() -> impl IntoResponse {
        vec![b'x'; MAX_RELEASE_RESPONSE_BYTES + 1]
    }

    async fn spawn(app: Router) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            Url::parse(&format!("http://{address}/releases")).unwrap(),
            server,
        )
    }
}
