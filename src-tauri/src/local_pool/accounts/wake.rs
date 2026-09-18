use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;
use zenith_relay_core::automations::{
    WakeCompletion, WakeCompletionOutcome, WakeExecutionRequest, WakeVerificationOutcome,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::{is_loopback_url, providers::chatgpt::CodexIdentityEnvelope, ProxyConfig};

use super::{collect_limited, LimitedBodyError};
use zenith_relay_core::gateway::{execute_account_wake, AccountWakeRequest};
use zenith_relay_core::GatewayRuntime;

pub const DEFAULT_CODEX_WAKE_RESPONSES_ENDPOINT: &str = super::records::CODEX_RESPONSES_URL;

#[cfg(test)]
const ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
#[cfg(test)]
const ORIGINATOR_HEADER: &str = "originator";
#[cfg(test)]
const ORIGINATOR: &str = zenith_relay_core::providers::chatgpt::CODEX_ORIGINATOR;
const FIXED_WAKE_INPUT: &str = "Reply briefly.";
const MAX_ACCESS_TOKEN_BYTES: usize = 64 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_LOCAL_ACCOUNT_ID_BYTES: usize = 128;
const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_OUTPUT_TOKEN_CAP: u16 = 256;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct CodexWakeClient {
    http: reqwest::Client,
    responses_endpoint: Url,
    authorization: HeaderValue,
    identity: CodexIdentityEnvelope,
}

impl CodexWakeClient {
    pub fn new_with_proxy(
        access_token: &str,
        chatgpt_account_id: &str,
        proxy: Option<&ProxyConfig>,
    ) -> Result<Self, WakeExecutionFailure> {
        let endpoint = Url::parse(DEFAULT_CODEX_WAKE_RESPONSES_ENDPOINT)
            .map_err(|_| WakeExecutionFailure::configuration())?;
        Self::with_endpoint_and_proxy(endpoint, access_token, chatgpt_account_id, proxy)
    }

    #[cfg(test)]
    pub fn with_endpoint(
        responses_endpoint: Url,
        access_token: &str,
        chatgpt_account_id: &str,
    ) -> Result<Self, WakeExecutionFailure> {
        Self::with_endpoint_and_proxy(responses_endpoint, access_token, chatgpt_account_id, None)
    }

    fn with_endpoint_and_proxy(
        responses_endpoint: Url,
        access_token: &str,
        chatgpt_account_id: &str,
        proxy: Option<&ProxyConfig>,
    ) -> Result<Self, WakeExecutionFailure> {
        validate_endpoint(&responses_endpoint)?;
        validate_secret(
            access_token,
            MAX_ACCESS_TOKEN_BYTES,
            WakeExecutionErrorCode::InvalidAccessToken,
        )?;
        validate_secret(
            chatgpt_account_id,
            MAX_ACCOUNT_ID_BYTES,
            WakeExecutionErrorCode::InvalidProviderAccountId,
        )?;

        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
                WakeExecutionFailure::invalid(WakeExecutionErrorCode::InvalidAccessToken)
            })?;
        authorization.set_sensitive(true);
        let identity = CodexIdentityEnvelope::standard(chatgpt_account_id).map_err(|_| {
            WakeExecutionFailure::invalid(WakeExecutionErrorCode::InvalidProviderAccountId)
        })?;
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent("Zenith Relay");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|_| WakeExecutionFailure::configuration())?;
        Ok(Self {
            http,
            responses_endpoint,
            authorization,
            identity,
        })
    }

    pub async fn execute(
        &self,
        request: &WakeExecutionRequest,
    ) -> Result<WakeExecutionMetrics, WakeExecutionFailure> {
        validate_request(request)?;
        let payload = WakePayload {
            model: request.model_id.trim(),
            input: FIXED_WAKE_INPUT,
            stream: false,
            store: false,
            max_output_tokens: request.output_token_cap,
        };
        let body = serde_json::to_vec(&payload)
            .map_err(|_| WakeExecutionFailure::invalid(WakeExecutionErrorCode::InvalidRequest))?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(WakeExecutionFailure::invalid(
                WakeExecutionErrorCode::RequestTooLarge,
            ));
        }

        let started = Instant::now();
        let identity = self
            .identity
            .with_configured_client_version()
            .map_err(|_| WakeExecutionFailure::configuration())?;
        let response = identity
            .apply(
                self.http
                    .post(self.responses_endpoint.clone())
                    .header(AUTHORIZATION, self.authorization.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(body),
            )
            .send()
            .await
            .map_err(|error| {
                let code = if error.is_timeout() {
                    WakeExecutionErrorCode::Timeout
                } else {
                    WakeExecutionErrorCode::Transport
                };
                WakeExecutionFailure::runtime(code, true, None, elapsed_ms(started))
            })?;
        let status = response.status();
        let response_body = collect_limited(response, MAX_RESPONSE_BYTES).await;
        if !status.is_success() {
            return Err(status_failure(status.as_u16(), elapsed_ms(started)));
        }
        let response_body = response_body.map_err(|error| match error {
            LimitedBodyError::Transport => WakeExecutionFailure::runtime(
                WakeExecutionErrorCode::Transport,
                true,
                Some(status.as_u16()),
                elapsed_ms(started),
            ),
            LimitedBodyError::TooLarge => WakeExecutionFailure::runtime(
                WakeExecutionErrorCode::ResponseTooLarge,
                false,
                Some(status.as_u16()),
                elapsed_ms(started),
            ),
        })?;

        let envelope: WakeResponseEnvelope =
            serde_json::from_slice(&response_body).map_err(|_| {
                WakeExecutionFailure::runtime(
                    WakeExecutionErrorCode::InvalidResponse,
                    false,
                    Some(status.as_u16()),
                    elapsed_ms(started),
                )
            })?;
        Ok(WakeExecutionMetrics {
            http_status: status.as_u16(),
            latency_ms: elapsed_ms(started),
            input_tokens: envelope.usage.as_ref().and_then(|usage| usage.input_tokens),
            output_tokens: envelope
                .usage
                .as_ref()
                .and_then(|usage| usage.output_tokens),
            total_tokens: envelope.usage.and_then(|usage| usage.total_tokens),
        })
    }
}

/// Executes a wake through the already-running shared GatewayRuntime.  The
/// runtime owns account selection, token refresh, scheduler leases, cooldowns,
/// and usage emission; this adapter only converts its sanitized HTTP response
/// into the desktop wake metrics contract.
pub async fn execute_with_runtime(
    runtime: Arc<GatewayRuntime>,
    local_key_id: &str,
    request: &WakeExecutionRequest,
) -> Result<WakeExecutionMetrics, WakeExecutionFailure> {
    let started = Instant::now();
    let response = execute_account_wake(
        runtime,
        AccountWakeRequest {
            local_key_id: local_key_id.to_string(),
            account_id: request.account_id.clone(),
            model_id: request.model_id.clone(),
            output_token_cap: request.output_token_cap,
        },
    )
    .await;
    let status = response.status();
    let category = response
        .headers()
        .get("x-zenith-relay-error-category")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let response_body = axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
        .await
        .map_err(|_| {
            WakeExecutionFailure::runtime(
                WakeExecutionErrorCode::ResponseTooLarge,
                false,
                Some(status.as_u16()),
                elapsed_ms(started),
            )
        })?;
    if !status.is_success() {
        return Err(runtime_status_failure(
            status,
            category.as_deref(),
            elapsed_ms(started),
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&response_body).map_err(|_| {
        WakeExecutionFailure::runtime(
            WakeExecutionErrorCode::InvalidResponse,
            false,
            Some(status.as_u16()),
            elapsed_ms(started),
        )
    })?;
    let usage = wake_usage(&value);
    Ok(WakeExecutionMetrics {
        http_status: status.as_u16(),
        latency_ms: elapsed_ms(started),
        input_tokens: usage
            .and_then(|usage| usage_token(usage, &["input_tokens", "prompt_tokens"])),
        output_tokens: usage
            .and_then(|usage| usage_token(usage, &["output_tokens", "completion_tokens"])),
        total_tokens: usage.and_then(|usage| usage_token(usage, &["total_tokens"])),
    })
}

fn runtime_status_failure(
    status: reqwest::StatusCode,
    category: Option<&str>,
    latency_ms: u64,
) -> WakeExecutionFailure {
    let (code, retryable) = match category {
        Some(
            error_codes::ALL_SOURCES_COOLING_DOWN
            | error_codes::ALL_SOURCES_TEMPORARILY_UNAVAILABLE
            | error_codes::NO_ELIGIBLE_SOURCE,
        ) => (WakeExecutionErrorCode::Upstream, true),
        Some(error_codes::MODEL_NOT_FOUND | error_codes::INVALID_REQUEST) => {
            (WakeExecutionErrorCode::InvalidRequest, false)
        }
        Some(error_codes::UPSTREAM_BODY_TOO_LARGE) => {
            (WakeExecutionErrorCode::ResponseTooLarge, false)
        }
        _ => match status {
            reqwest::StatusCode::UNAUTHORIZED => (WakeExecutionErrorCode::Unauthorized, false),
            reqwest::StatusCode::FORBIDDEN => (WakeExecutionErrorCode::Forbidden, false),
            reqwest::StatusCode::TOO_MANY_REQUESTS => (WakeExecutionErrorCode::RateLimited, true),
            status if status.is_server_error() => (WakeExecutionErrorCode::Upstream, true),
            reqwest::StatusCode::BAD_REQUEST => (WakeExecutionErrorCode::InvalidRequest, false),
            _ => (WakeExecutionErrorCode::HttpStatus, false),
        },
    };
    WakeExecutionFailure::runtime(code, retryable, Some(status.as_u16()), latency_ms)
}

fn wake_usage(value: &serde_json::Value) -> Option<&serde_json::Value> {
    value
        .get("usage")
        .or_else(|| value.pointer("/response/usage"))
        .or_else(|| value.pointer("/response/response/usage"))
        .or_else(|| value.get("usageMetadata"))
}

fn usage_token(usage: &serde_json::Value, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| usage.get(*name).and_then(serde_json::Value::as_u64))
}

impl fmt::Debug for CodexWakeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexWakeClient")
            .field("responses_endpoint", &"[configured]")
            .field("authorization", &"[redacted]")
            .field("identity", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeExecutionMetrics {
    pub http_status: u16,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeExecutionErrorCode {
    InvalidConfiguration,
    InvalidEndpoint,
    InvalidAccessToken,
    InvalidProviderAccountId,
    InvalidRequest,
    RequestTooLarge,
    Transport,
    Timeout,
    Unauthorized,
    Forbidden,
    RateLimited,
    Upstream,
    HttpStatus,
    ResponseTooLarge,
    InvalidResponse,
}

impl WakeExecutionErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => error_codes::WAKE_INVALID_CONFIGURATION,
            Self::InvalidEndpoint => error_codes::WAKE_INVALID_ENDPOINT,
            Self::InvalidAccessToken => error_codes::WAKE_INVALID_ACCESS_TOKEN,
            Self::InvalidProviderAccountId => error_codes::WAKE_INVALID_PROVIDER_ACCOUNT_ID,
            Self::InvalidRequest => error_codes::WAKE_INVALID_REQUEST,
            Self::RequestTooLarge => error_codes::WAKE_REQUEST_TOO_LARGE,
            Self::Transport => error_codes::WAKE_TRANSPORT,
            Self::Timeout => error_codes::WAKE_TIMEOUT,
            Self::Unauthorized => error_codes::WAKE_UNAUTHORIZED,
            Self::Forbidden => error_codes::WAKE_FORBIDDEN,
            Self::RateLimited => error_codes::WAKE_RATE_LIMITED,
            Self::Upstream => error_codes::WAKE_UPSTREAM,
            Self::HttpStatus => error_codes::WAKE_HTTP_STATUS,
            Self::ResponseTooLarge => error_codes::WAKE_RESPONSE_TOO_LARGE,
            Self::InvalidResponse => error_codes::WAKE_INVALID_RESPONSE,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeExecutionFailure {
    pub code: WakeExecutionErrorCode,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub latency_ms: u64,
}

impl WakeExecutionFailure {
    fn configuration() -> Self {
        Self::runtime(WakeExecutionErrorCode::InvalidConfiguration, false, None, 0)
    }

    fn invalid(code: WakeExecutionErrorCode) -> Self {
        Self::runtime(code, false, None, 0)
    }

    fn runtime(
        code: WakeExecutionErrorCode,
        retryable: bool,
        http_status: Option<u16>,
        latency_ms: u64,
    ) -> Self {
        Self {
            code,
            retryable,
            http_status,
            latency_ms,
        }
    }
}

impl fmt::Display for WakeExecutionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.as_str())
    }
}

impl std::error::Error for WakeExecutionFailure {}

pub fn completion_from_execution(
    execution: &Result<WakeExecutionMetrics, WakeExecutionFailure>,
    verification: WakeVerificationOutcome,
    completed_at_ms: u64,
) -> WakeCompletion {
    match execution {
        Ok(metrics) => WakeCompletion {
            outcome: match verification {
                WakeVerificationOutcome::ConfirmedQuotaConsumed
                | WakeVerificationOutcome::ConfirmedCountdownAdvanced => {
                    WakeCompletionOutcome::Confirmed
                }
                WakeVerificationOutcome::Unconfirmed => WakeCompletionOutcome::Unconfirmed,
            },
            completed_at_ms,
            latency_ms: Some(metrics.latency_ms),
            input_tokens: metrics.input_tokens,
            output_tokens: metrics.output_tokens,
            error_code: None,
        },
        Err(failure) => WakeCompletion {
            outcome: WakeCompletionOutcome::Failed,
            completed_at_ms,
            latency_ms: Some(failure.latency_ms),
            input_tokens: None,
            output_tokens: None,
            error_code: Some(failure.code.as_str().to_string()),
        },
    }
}

#[derive(Serialize)]
struct WakePayload<'a> {
    model: &'a str,
    input: &'static str,
    stream: bool,
    store: bool,
    max_output_tokens: u16,
}

#[derive(Deserialize)]
struct WakeResponseEnvelope {
    #[serde(default)]
    usage: Option<WakeUsage>,
}

#[derive(Deserialize)]
struct WakeUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

fn validate_endpoint(endpoint: &Url) -> Result<(), WakeExecutionFailure> {
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidEndpoint,
        ));
    }
    if endpoint.scheme() == "http" && !is_loopback_url(endpoint) {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidEndpoint,
        ));
    }
    Ok(())
}

fn validate_secret(
    value: &str,
    max_bytes: usize,
    code: WakeExecutionErrorCode,
) -> Result<(), WakeExecutionFailure> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(WakeExecutionFailure::invalid(code));
    }
    Ok(())
}

fn validate_request(request: &WakeExecutionRequest) -> Result<(), WakeExecutionFailure> {
    let account_id = request.account_id.trim();
    if account_id.is_empty()
        || account_id.len() > MAX_LOCAL_ACCOUNT_ID_BYTES
        || !account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidRequest,
        ));
    }
    let model = request.model_id.trim();
    if model.is_empty()
        || model.len() > MAX_MODEL_ID_BYTES
        || model.bytes().any(|byte| byte.is_ascii_control())
        || !(1..=MAX_OUTPUT_TOKEN_CAP).contains(&request.output_token_cap)
    {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

fn status_failure(status: u16, latency_ms: u64) -> WakeExecutionFailure {
    let (code, retryable) = match status {
        401 => (WakeExecutionErrorCode::Unauthorized, false),
        403 => (WakeExecutionErrorCode::Forbidden, false),
        429 => (WakeExecutionErrorCode::RateLimited, true),
        500..=599 => (WakeExecutionErrorCode::Upstream, true),
        _ => (WakeExecutionErrorCode::HttpStatus, false),
    };
    WakeExecutionFailure::runtime(code, retryable, Some(status), latency_ms)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, Response, StatusCode};
    use axum::routing::post;
    use axum::Router;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use zenith_relay_core::quota::QuotaWindowKind;

    const ACCESS_TOKEN: &str = "access-private-secret";
    const PROVIDER_ACCOUNT_ID: &str = "provider-private-account";

    #[derive(Clone)]
    struct TestState {
        response: Arc<Mutex<TestResponse>>,
        requests: Arc<Mutex<Vec<ObservedRequest>>>,
    }

    #[derive(Clone)]
    struct TestResponse {
        status: StatusCode,
        body: Vec<u8>,
        content_length: Option<usize>,
    }

    #[derive(Clone, Debug)]
    struct ObservedRequest {
        headers: HeaderMap,
        body: Value,
    }

    struct TestServer {
        endpoint: Url,
        task: JoinHandle<()>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    #[tokio::test]
    async fn sends_only_fixed_bounded_payload_and_private_headers() {
        let (server, state) = spawn_server(TestResponse {
            status: StatusCode::OK,
            body: json!({
                "output": [{"content": [{"text": "generated private text"}]}],
                "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
            })
            .to_string()
            .into_bytes(),
            content_length: None,
        })
        .await;
        let client = CodexWakeClient::with_endpoint(
            server.endpoint.clone(),
            ACCESS_TOKEN,
            PROVIDER_ACCOUNT_ID,
        )
        .unwrap();
        let result = client.execute(&request()).await.unwrap();
        assert_eq!(result.input_tokens, Some(3));
        assert_eq!(result.output_tokens, Some(2));
        assert_eq!(result.total_tokens, Some(5));

        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let observed = &requests[0];
        assert_eq!(
            header(&observed.headers, AUTHORIZATION.as_str()).as_deref(),
            Some("Bearer access-private-secret")
        );
        assert_eq!(
            header(&observed.headers, ACCOUNT_ID_HEADER).as_deref(),
            Some(PROVIDER_ACCOUNT_ID)
        );
        assert_eq!(
            header(&observed.headers, ORIGINATOR_HEADER).as_deref(),
            Some(ORIGINATOR)
        );
        assert_eq!(observed.body["model"], "gpt-wake");
        assert_eq!(observed.body["input"], FIXED_WAKE_INPUT);
        assert_eq!(observed.body["stream"], false);
        assert_eq!(observed.body["store"], false);
        assert_eq!(observed.body["max_output_tokens"], 8);
        assert_eq!(observed.body.as_object().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn response_content_and_credentials_never_escape_metrics_or_debug() {
        let private_response = "generated-private-response";
        let (server, _) = spawn_server(TestResponse {
            status: StatusCode::OK,
            body: json!({
                "output_text": private_response,
                "provider_account_id": PROVIDER_ACCOUNT_ID,
                "token": ACCESS_TOKEN,
                "usage": {"input_tokens": 1}
            })
            .to_string()
            .into_bytes(),
            content_length: None,
        })
        .await;
        let client = CodexWakeClient::with_endpoint(
            server.endpoint.clone(),
            ACCESS_TOKEN,
            PROVIDER_ACCOUNT_ID,
        )
        .unwrap();
        let metrics = client.execute(&request()).await.unwrap();
        let serialized = serde_json::to_string(&metrics).unwrap();
        let debug = format!("{client:?}");
        for secret in [
            private_response,
            PROVIDER_ACCOUNT_ID,
            ACCESS_TOKEN,
            FIXED_WAKE_INPUT,
        ] {
            assert!(!serialized.contains(secret));
            assert!(!debug.contains(secret));
        }
    }

    #[tokio::test]
    async fn oversized_and_malformed_success_responses_fail_closed() {
        let (server, state) = spawn_server(TestResponse {
            status: StatusCode::OK,
            body: vec![b'x'; MAX_RESPONSE_BYTES + 1],
            content_length: None,
        })
        .await;
        let client = CodexWakeClient::with_endpoint(
            server.endpoint.clone(),
            ACCESS_TOKEN,
            PROVIDER_ACCOUNT_ID,
        )
        .unwrap();
        let failure = client.execute(&request()).await.unwrap_err();
        assert_eq!(failure.code, WakeExecutionErrorCode::ResponseTooLarge);
        assert!(!failure.retryable);

        *state.response.lock().unwrap() = TestResponse {
            status: StatusCode::OK,
            body: b"not-json-private-body".to_vec(),
            content_length: None,
        };
        let failure = client.execute(&request()).await.unwrap_err();
        assert_eq!(failure.code, WakeExecutionErrorCode::InvalidResponse);
        assert!(!serde_json::to_string(&failure)
            .unwrap()
            .contains("not-json-private-body"));
    }

    #[tokio::test]
    async fn http_failures_have_typed_retryability_without_body_leaks() {
        let (server, state) = spawn_server(TestResponse {
            status: StatusCode::UNAUTHORIZED,
            body: b"provider-secret-body".to_vec(),
            content_length: None,
        })
        .await;
        let client = CodexWakeClient::with_endpoint(
            server.endpoint.clone(),
            ACCESS_TOKEN,
            PROVIDER_ACCOUNT_ID,
        )
        .unwrap();
        for (status, code, retryable) in [
            (
                StatusCode::UNAUTHORIZED,
                WakeExecutionErrorCode::Unauthorized,
                false,
            ),
            (
                StatusCode::FORBIDDEN,
                WakeExecutionErrorCode::Forbidden,
                false,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                WakeExecutionErrorCode::RateLimited,
                true,
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                WakeExecutionErrorCode::Upstream,
                true,
            ),
        ] {
            state.response.lock().unwrap().status = status;
            let failure = client.execute(&request()).await.unwrap_err();
            assert_eq!(failure.code, code);
            assert_eq!(failure.retryable, retryable);
            assert_eq!(failure.http_status, Some(status.as_u16()));
            assert!(!serde_json::to_string(&failure)
                .unwrap()
                .contains("provider-secret-body"));
        }
    }

    #[test]
    fn completion_helper_uses_only_metrics_error_and_verification() {
        let success = Ok(WakeExecutionMetrics {
            http_status: 200,
            latency_ms: 10,
            input_tokens: Some(1),
            output_tokens: Some(1),
            total_tokens: Some(2),
        });
        let confirmed = completion_from_execution(
            &success,
            WakeVerificationOutcome::ConfirmedCountdownAdvanced,
            100,
        );
        assert_eq!(confirmed.outcome, WakeCompletionOutcome::Confirmed);
        assert_eq!(confirmed.input_tokens, Some(1));

        let unconfirmed =
            completion_from_execution(&success, WakeVerificationOutcome::Unconfirmed, 100);
        assert_eq!(unconfirmed.outcome, WakeCompletionOutcome::Unconfirmed);

        let failure = Err(WakeExecutionFailure::runtime(
            WakeExecutionErrorCode::RateLimited,
            true,
            Some(429),
            12,
        ));
        let failed = completion_from_execution(&failure, WakeVerificationOutcome::Unconfirmed, 100);
        assert_eq!(failed.outcome, WakeCompletionOutcome::Failed);
        assert_eq!(failed.error_code.as_deref(), Some("wake_rate_limited"));
        let serialized = format!("{failed:?}");
        assert!(!serialized.contains(ACCESS_TOKEN));
        assert!(!serialized.contains(PROVIDER_ACCOUNT_ID));
    }

    fn request() -> WakeExecutionRequest {
        WakeExecutionRequest {
            account_id: "relay-account".into(),
            model_id: "gpt-wake".into(),
            window_kind: QuotaWindowKind::Primary,
            output_token_cap: 8,
        }
    }

    async fn spawn_server(response: TestResponse) -> (TestServer, TestState) {
        let state = TestState {
            response: Arc::new(Mutex::new(response)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/responses", post(handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            TestServer {
                endpoint: Url::parse(&format!("http://{address}/responses")).unwrap(),
                task,
            },
            state,
        )
    }

    async fn handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response<Body> {
        state.requests.lock().unwrap().push(ObservedRequest {
            headers,
            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
        });
        let response = state.response.lock().unwrap().clone();
        let mut builder = Response::builder()
            .status(response.status)
            .header(CONTENT_TYPE, "application/json");
        if let Some(content_length) = response.content_length {
            builder = builder.header("content-length", content_length);
        }
        builder.body(Body::from(response.body)).unwrap()
    }

    fn header(headers: &HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}
