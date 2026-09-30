use serde::Serialize;
use std::fmt;
use zenith_relay_core::automations::{
    WakeCompletion, WakeCompletionOutcome, WakeVerificationOutcome,
};
use zenith_relay_core::error_codes;

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
    pub(super) fn configuration() -> Self {
        Self::runtime(WakeExecutionErrorCode::InvalidConfiguration, false, None, 0)
    }

    pub(super) fn invalid(code: WakeExecutionErrorCode) -> Self {
        Self::runtime(code, false, None, 0)
    }

    pub(super) fn runtime(
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
