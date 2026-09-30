use crate::error_codes;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelDiscoveryFailureCode {
    AgentTaskInvalid,
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

impl ModelDiscoveryFailureCode {
    /// Stable management identifier shared by desktop and server account flows.
    pub fn management_code(self) -> &'static str {
        match self {
            Self::AgentTaskInvalid => error_codes::MODELS_AGENT_TASK_INVALID,
            Self::Forbidden => error_codes::MODELS_FORBIDDEN,
            Self::HttpStatus => error_codes::MODELS_HTTP_STATUS,
            Self::InvalidAccessToken => error_codes::MODELS_INVALID_ACCESS_TOKEN,
            Self::InvalidAccountId => error_codes::MODELS_INVALID_ACCOUNT_ID,
            Self::InvalidClientVersion => error_codes::MODELS_INVALID_CLIENT_VERSION,
            Self::InvalidEndpoint => error_codes::MODELS_INVALID_ENDPOINT,
            Self::InvalidResponse => error_codes::MODELS_INVALID_RESPONSE,
            Self::RateLimited => error_codes::MODELS_RATE_LIMITED,
            Self::ResponseTooLarge => error_codes::MODELS_RESPONSE_TOO_LARGE,
            Self::Transport => error_codes::MODELS_TRANSPORT,
            Self::Unauthorized => error_codes::MODELS_UNAUTHORIZED,
            Self::Upstream => error_codes::MODELS_UPSTREAM,
        }
    }

    pub fn is_authentication_failure(self) -> bool {
        matches!(
            self,
            Self::InvalidAccessToken | Self::InvalidAccountId | Self::Unauthorized
        )
    }

    pub fn blocks_account(self) -> bool {
        self == Self::Forbidden
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelDiscoveryFailure {
    pub code: ModelDiscoveryFailureCode,
    pub retryable: bool,
    pub http_status: Option<u16>,
    pub retry_after_ms: Option<u64>,
}

impl ModelDiscoveryFailure {
    pub(super) fn new(code: ModelDiscoveryFailureCode) -> Self {
        Self {
            code,
            retryable: false,
            http_status: None,
            retry_after_ms: None,
        }
    }

    pub(super) fn retryable(code: ModelDiscoveryFailureCode) -> Self {
        Self {
            code,
            retryable: true,
            http_status: None,
            retry_after_ms: None,
        }
    }
}

impl fmt::Display for ModelDiscoveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            ModelDiscoveryFailureCode::AgentTaskInvalid => {
                "Agent Identity task must be registered again"
            }
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
