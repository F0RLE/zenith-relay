use super::oauth::{collect_limited, LimitedBodyError};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::redirect::Policy;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;
use url::Url;
use zenith_relay_core::{accounts::CodexIdentityEnvelope, ProxyConfig};

pub const CODEX_MODELS_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/models";

#[cfg(test)]
const ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
#[cfg(test)]
const ORIGINATOR_HEADER: &str = "originator";
#[cfg(test)]
const CODEX_ORIGINATOR: &str = zenith_relay_core::accounts::CODEX_ORIGINATOR;
const MAX_ACCESS_TOKEN_BYTES: usize = 64 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_CLIENT_VERSION_BYTES: usize = 64;
const MAX_MODEL_SLUG_BYTES: usize = 256;
const MAX_MODELS: usize = 4_096;
const MAX_MODELS_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub struct CodexModelsClient {
    http: reqwest::Client,
    endpoint: Url,
}

impl CodexModelsClient {
    pub fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, ModelDiscoveryFailure> {
        let endpoint = Url::parse(CODEX_MODELS_ENDPOINT)
            .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidEndpoint))?;
        Self::with_endpoint_and_proxy(endpoint, proxy)
    }

    #[cfg(test)]
    pub fn with_endpoint(endpoint: Url) -> Result<Self, ModelDiscoveryFailure> {
        Self::with_endpoint_and_proxy(endpoint, None)
    }

    fn with_endpoint_and_proxy(
        endpoint: Url,
        proxy: Option<&ProxyConfig>,
    ) -> Result<Self, ModelDiscoveryFailure> {
        validate_endpoint(&endpoint)?;
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10))
            .user_agent("Zenith Relay");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidEndpoint))?;
        Ok(Self { http, endpoint })
    }

    pub async fn discover(
        &self,
        access_token: &str,
        chatgpt_account_id: &str,
        client_version: &str,
    ) -> Result<Vec<String>, ModelDiscoveryFailure> {
        validate_access_token(access_token)?;
        validate_account_id(chatgpt_account_id)?;
        validate_client_version(client_version)?;

        let authorization =
            HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
                ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidAccessToken)
            })?;
        let identity = CodexIdentityEnvelope::new(chatgpt_account_id, client_version)
            .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidAccountId))?;
        let mut request_url = self.endpoint.clone();
        request_url
            .query_pairs_mut()
            .append_pair("client_version", client_version);
        let response = identity
            .apply(
                self.http
                    .get(request_url)
                    .header(AUTHORIZATION, authorization),
            )
            .send()
            .await
            .map_err(|_| ModelDiscoveryFailure::retryable(ModelDiscoveryFailureCode::Transport))?;
        let status = response.status();
        let body = collect_limited(response, MAX_MODELS_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                LimitedBodyError::Transport => {
                    ModelDiscoveryFailure::retryable(ModelDiscoveryFailureCode::Transport)
                }
                LimitedBodyError::TooLarge => {
                    ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::ResponseTooLarge)
                }
            })?;
        if !status.is_success() {
            let (code, retryable) = match status.as_u16() {
                401 => (ModelDiscoveryFailureCode::Unauthorized, false),
                403 => (ModelDiscoveryFailureCode::Forbidden, false),
                429 => (ModelDiscoveryFailureCode::RateLimited, true),
                _ if status.is_server_error() => (ModelDiscoveryFailureCode::Upstream, true),
                _ => (ModelDiscoveryFailureCode::HttpStatus, false),
            };
            return Err(ModelDiscoveryFailure {
                code,
                retryable,
                http_status: Some(status.as_u16()),
            });
        }

        parse_models(&body)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelDiscoveryFailureCode {
    Forbidden,
    HttpStatus,
    InvalidAccessToken,
    InvalidAccountId,
    InvalidClientVersion,
    InvalidEndpoint,
    InvalidResponse,
    RateLimited,
    ResponseTooLarge,
    Transport,
    Unauthorized,
    Upstream,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelDiscoveryFailure {
    pub code: ModelDiscoveryFailureCode,
    pub retryable: bool,
    pub http_status: Option<u16>,
}

impl ModelDiscoveryFailure {
    fn new(code: ModelDiscoveryFailureCode) -> Self {
        Self {
            code,
            retryable: false,
            http_status: None,
        }
    }

    fn retryable(code: ModelDiscoveryFailureCode) -> Self {
        Self {
            code,
            retryable: true,
            http_status: None,
        }
    }
}

impl fmt::Display for ModelDiscoveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            ModelDiscoveryFailureCode::Forbidden => "model discovery is forbidden",
            ModelDiscoveryFailureCode::HttpStatus => "model discovery request was rejected",
            ModelDiscoveryFailureCode::InvalidAccessToken => "model access token is invalid",
            ModelDiscoveryFailureCode::InvalidAccountId => "model account id is invalid",
            ModelDiscoveryFailureCode::InvalidClientVersion => "model client version is invalid",
            ModelDiscoveryFailureCode::InvalidEndpoint => "model endpoint is invalid",
            ModelDiscoveryFailureCode::InvalidResponse => "model discovery response is invalid",
            ModelDiscoveryFailureCode::RateLimited => "model discovery is rate limited",
            ModelDiscoveryFailureCode::ResponseTooLarge => "model discovery response is too large",
            ModelDiscoveryFailureCode::Transport => "model discovery request failed",
            ModelDiscoveryFailureCode::Unauthorized => "model discovery requires authentication",
            ModelDiscoveryFailureCode::Upstream => "model discovery service failed",
        })
    }
}

impl std::error::Error for ModelDiscoveryFailure {}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    slug: String,
    #[serde(default)]
    supported_in_api: Option<bool>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    upgrade: Option<serde_json::Value>,
}

fn parse_models(body: &[u8]) -> Result<Vec<String>, ModelDiscoveryFailure> {
    let response: ModelsResponse = serde_json::from_slice(body)
        .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidResponse))?;
    if response.models.len() > MAX_MODELS {
        return Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidResponse,
        ));
    }
    let mut seen = HashSet::new();
    Ok(response
        .models
        .into_iter()
        .filter(|model| model.supported_in_api != Some(false))
        .filter(|model| {
            !model
                .visibility
                .as_deref()
                .is_some_and(|visibility| visibility.eq_ignore_ascii_case("hide"))
                || model.upgrade.is_some()
        })
        .filter_map(|model| {
            let slug = model.slug.trim();
            (!slug.is_empty()
                && slug.len() <= MAX_MODEL_SLUG_BYTES
                && !slug.chars().any(char::is_control)
                && seen.insert(slug.to_string()))
            .then(|| slug.to_string())
        })
        .collect())
}

fn validate_endpoint(endpoint: &Url) -> Result<(), ModelDiscoveryFailure> {
    let loopback_http = endpoint.scheme() == "http"
        && endpoint
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if (endpoint.scheme() != "https" && !loopback_http)
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidEndpoint,
        ))
    } else {
        Ok(())
    }
}

fn validate_access_token(access_token: &str) -> Result<(), ModelDiscoveryFailure> {
    if access_token.is_empty()
        || access_token.len() > MAX_ACCESS_TOKEN_BYTES
        || access_token.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidAccessToken,
        ))
    } else {
        Ok(())
    }
}

fn validate_account_id(account_id: &str) -> Result<(), ModelDiscoveryFailure> {
    if account_id.is_empty()
        || account_id.len() > MAX_ACCOUNT_ID_BYTES
        || account_id.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidAccountId,
        ))
    } else {
        Ok(())
    }
}

fn validate_client_version(client_version: &str) -> Result<(), ModelDiscoveryFailure> {
    if client_version.is_empty()
        || client_version.len() > MAX_CLIENT_VERSION_BYTES
        || !client_version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_'))
    {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidClientVersion,
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;

    #[tokio::test]
    async fn discovers_unique_supported_slugs_with_codex_request_contract() {
        let (endpoint, server) =
            spawn(Router::new().route("/backend-api/codex/models", get(successful_models))).await;
        let models = CodexModelsClient::with_endpoint(endpoint)
            .unwrap()
            .discover(
                "access-secret",
                "account-123",
                zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
            .unwrap();

        assert_eq!(models, vec!["gpt-5", "gpt-legacy", "gpt-5-mini"]);
        let rendered = format!("{models:?}");
        assert!(!rendered.contains("description-secret"));
        assert!(!rendered.contains("instructions-secret"));
        server.abort();
    }

    #[tokio::test]
    async fn malformed_oversized_and_http_errors_are_redacted() {
        for (handler, expected, retryable) in [
            (
                get(malformed_models),
                ModelDiscoveryFailureCode::InvalidResponse,
                false,
            ),
            (
                get(oversized_models),
                ModelDiscoveryFailureCode::ResponseTooLarge,
                false,
            ),
            (
                get(upstream_failure),
                ModelDiscoveryFailureCode::Upstream,
                true,
            ),
        ] {
            let (endpoint, server) =
                spawn(Router::new().route("/backend-api/codex/models", handler)).await;
            let error = CodexModelsClient::with_endpoint(endpoint)
                .unwrap()
                .discover(
                    "access-secret",
                    "account-123",
                    zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION,
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, expected);
            assert_eq!(error.retryable, retryable);
            let rendered = format!("{error:?} {error}");
            for secret in [
                "access-secret",
                "account-123",
                "provider-body-secret",
                "description-secret",
                "instructions-secret",
            ] {
                assert!(!rendered.contains(secret));
            }
            server.abort();
        }
    }

    async fn successful_models(headers: HeaderMap, uri: Uri) -> impl IntoResponse {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-secret")
        );
        assert_eq!(
            headers
                .get(ACCOUNT_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("account-123")
        );
        assert_eq!(
            headers
                .get(ORIGINATOR_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(CODEX_ORIGINATOR)
        );
        let expected_query = format!(
            "client_version={}",
            zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION
        );
        assert_eq!(uri.query(), Some(expected_query.as_str()));
        assert_eq!(
            headers.get("version").and_then(|value| value.to_str().ok()),
            Some(zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION)
        );
        Json(json!({
            "models": [
                {
                    "slug": "gpt-5",
                    "supported_in_api": true,
                    "description": "description-secret",
                    "base_instructions": "instructions-secret"
                },
                { "slug": " gpt-5 " },
                { "slug": "gpt-hidden", "supported_in_api": false },
                { "slug": "gpt-internal", "visibility": "hide" },
                { "slug": "gpt-legacy", "visibility": "hide", "upgrade": { "model": "gpt-5" } },
                { "slug": "" },
                { "slug": "gpt-5-mini" }
            ]
        }))
    }

    async fn malformed_models() -> impl IntoResponse {
        (StatusCode::OK, "provider-body-secret")
    }

    async fn oversized_models() -> impl IntoResponse {
        (
            StatusCode::OK,
            format!(
                "provider-body-secret{}",
                "x".repeat(MAX_MODELS_RESPONSE_BYTES)
            ),
        )
    }

    async fn upstream_failure() -> impl IntoResponse {
        (StatusCode::BAD_GATEWAY, "provider-body-secret")
    }

    async fn spawn(router: Router) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (
            Url::parse(&format!("http://{address}/backend-api/codex/models")).unwrap(),
            server,
        )
    }
}
