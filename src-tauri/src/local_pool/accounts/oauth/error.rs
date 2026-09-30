use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthErrorCode {
    AuthorizationDenied,
    ExpiredCallback,
    InvalidCallback,
    InvalidCallbackPort,
    InvalidConfiguration,
    InvalidJwt,
    InvalidResponse,
    MissingAuthorizationCode,
    ResponseTooLarge,
    StateMismatch,
    TokenEndpointRejected,
    Transport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthError {
    pub code: OAuthErrorCode,
    pub provider_code: Option<String>,
    pub http_status: Option<u16>,
    pub retryable: bool,
}

impl OAuthError {
    pub(super) fn new(code: OAuthErrorCode, retryable: bool) -> Self {
        Self {
            code,
            provider_code: None,
            http_status: None,
            retryable,
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.code {
            OAuthErrorCode::AuthorizationDenied => "OAuth authorization was denied",
            OAuthErrorCode::ExpiredCallback => "OAuth callback expired",
            OAuthErrorCode::InvalidCallback => "OAuth callback is invalid",
            OAuthErrorCode::InvalidCallbackPort => "OAuth callback port is invalid",
            OAuthErrorCode::InvalidConfiguration => "OAuth configuration is invalid",
            OAuthErrorCode::InvalidJwt => "OAuth token claims are invalid",
            OAuthErrorCode::InvalidResponse => "OAuth token response is invalid",
            OAuthErrorCode::MissingAuthorizationCode => {
                "OAuth callback is missing an authorization code"
            }
            OAuthErrorCode::ResponseTooLarge => "OAuth response is too large",
            OAuthErrorCode::StateMismatch => "OAuth callback state does not match",
            OAuthErrorCode::TokenEndpointRejected => "OAuth token endpoint rejected the request",
            OAuthErrorCode::Transport => "OAuth request failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OAuthError {}
