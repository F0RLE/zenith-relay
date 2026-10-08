use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, Instant};
use url::Url;
use zenith_relay_core::automations::WakeExecutionRequest;
#[cfg(test)]
use zenith_relay_core::automations::{WakeCompletionOutcome, WakeVerificationOutcome};
use zenith_relay_core::{providers::chatgpt::CodexIdentityEnvelope, ProxyConfig};

use super::{collect_limited, LimitedBodyError};

mod error;
mod runtime_exec;
mod validate;
pub use error::{
    completion_from_execution, WakeExecutionErrorCode, WakeExecutionFailure, WakeExecutionMetrics,
};
pub use runtime_exec::execute_with_runtime;
use validate::{elapsed_ms, status_failure, validate_endpoint, validate_request, validate_secret};

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
        let wake_request = WakePayload {
            model: request.model_id.trim(),
            input: FIXED_WAKE_INPUT,
            stream: false,
            store: false,
            max_output_tokens: request.output_token_cap,
        };
        let request_body = serde_json::to_vec(&wake_request)
            .map_err(|_| WakeExecutionFailure::invalid(WakeExecutionErrorCode::InvalidRequest))?;
        if request_body.len() > MAX_REQUEST_BYTES {
            return Err(WakeExecutionFailure::invalid(
                WakeExecutionErrorCode::RequestTooLarge,
            ));
        }

        let started = Instant::now();
        let identity = self
            .identity
            .with_configured_client_version()
            .map_err(|_| WakeExecutionFailure::configuration())?;
        let (response, permit) =
            zenith_relay_core::scheduler::refresh::http::management_http_gate()
                .send(
                    &self.http,
                    identity.apply(
                        self.http
                            .post(self.responses_endpoint.clone())
                            .header(AUTHORIZATION, self.authorization.clone())
                            .header(CONTENT_TYPE, "application/json")
                            .body(request_body),
                    ),
                    zenith_relay_core::scheduler::refresh::http::HttpClass::Ordinary,
                )
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
        drop(permit);
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
#[cfg(test)]
mod tests;
