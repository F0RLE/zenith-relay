use super::super::ReauthReason;
use crate::error::safe_error_code;
use std::fmt;

#[derive(Debug)]
pub struct TokenPersistenceFailure {
    pub code: String,
}

impl TokenPersistenceFailure {
    pub fn new(code: &str) -> Self {
        Self {
            code: safe_error_code(code),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenRefreshFailureKind {
    InvalidGrant,
    ReusedRefreshToken,
    ExpiredRefreshToken,
    InvalidatedRefreshToken,
    RotationRecoveryRequired,
    RotationOutcomeUnknown,
    Deferred,
    Transient,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenRefreshStage {
    Preflight,
    Transport,
    ResponseRead,
    ResponseStatus,
    ResponseDecode,
    IdentityVerification,
    Recovery,
}

impl TokenRefreshStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::Transport => "transport",
            Self::ResponseRead => "response_read",
            Self::ResponseStatus => "response_status",
            Self::ResponseDecode => "response_decode",
            Self::IdentityVerification => "identity_verification",
            Self::Recovery => "recovery",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenRefreshFailure {
    pub kind: TokenRefreshFailureKind,
    pub code: String,
    pub retry_at_ms: Option<u64>,
    pub stage: Option<TokenRefreshStage>,
    pub request_id: Option<String>,
}

impl TokenRefreshFailure {
    pub fn new(kind: TokenRefreshFailureKind, code: &str) -> Self {
        Self {
            kind,
            code: safe_error_code(code),
            retry_at_ms: None,
            stage: None,
            request_id: None,
        }
    }

    pub fn deferred(code: &str, retry_at_ms: u64) -> Self {
        let mut failure = Self::new(TokenRefreshFailureKind::Deferred, code);
        failure.retry_at_ms = Some(retry_at_ms);
        failure.stage = Some(TokenRefreshStage::Preflight);
        failure
    }

    pub fn with_stage(mut self, stage: TokenRefreshStage) -> Self {
        self.stage = Some(stage);
        self
    }

    pub fn with_request_id(mut self, request_id: Option<&str>) -> Self {
        self.request_id = request_id
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
            .map(str::to_owned);
        self
    }

    pub(in crate::accounts::token_authority) fn reauth_reason(&self) -> Option<ReauthReason> {
        match self.kind {
            TokenRefreshFailureKind::InvalidGrant => Some(ReauthReason::InvalidGrant),
            // Another concurrent refresh can rotate the token first. Preserve
            // the current state and retry normally instead of forcing login.
            TokenRefreshFailureKind::ReusedRefreshToken => None,
            TokenRefreshFailureKind::ExpiredRefreshToken => Some(ReauthReason::ExpiredRefreshToken),
            TokenRefreshFailureKind::InvalidatedRefreshToken => {
                Some(ReauthReason::InvalidatedRefreshToken)
            }
            TokenRefreshFailureKind::Transient
            | TokenRefreshFailureKind::Deferred
            | TokenRefreshFailureKind::RotationOutcomeUnknown => None,
            TokenRefreshFailureKind::RotationRecoveryRequired => {
                Some(ReauthReason::TokenRotationRecoveryRequired)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAuthorityError {
    InvalidCapacity,
    InvalidAccountId,
    CapacityReached,
    AccountNotFound,
    AccessTokenExpired,
    RequiresReauth(ReauthReason),
    RefreshFailed(String),
    RefreshDeferred { retry_at_ms: u64 },
    PersistenceRequired,
    PersistenceFailed(String),
}

impl fmt::Display for TokenAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapacity => {
                formatter.write_str("token authority capacity must be positive")
            }
            Self::InvalidAccountId => formatter.write_str("account id must not be empty"),
            Self::CapacityReached => formatter.write_str("token authority capacity reached"),
            Self::AccountNotFound => formatter.write_str("account token state not found"),
            Self::AccessTokenExpired => {
                formatter.write_str("access token expired and cannot refresh")
            }
            Self::RequiresReauth(_) => formatter.write_str("account requires reauthentication"),
            Self::RefreshFailed(code) => write!(formatter, "token refresh failed: {code}"),
            Self::RefreshDeferred { .. } => {
                formatter.write_str("token renewal is scheduled for later")
            }
            Self::PersistenceRequired => {
                formatter.write_str("refreshed account tokens require persistence")
            }
            Self::PersistenceFailed(code) => {
                write!(formatter, "token persistence failed: {code}")
            }
        }
    }
}

impl std::error::Error for TokenAuthorityError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renewal_diagnostics_keep_only_bounded_request_identifiers() {
        let failure = TokenRefreshFailure::deferred("siwc_refresh_too_early", 1234)
            .with_request_id(Some("req_synthetic-42"));
        assert_eq!(failure.retry_at_ms, Some(1234));
        assert_eq!(failure.stage, Some(TokenRefreshStage::Preflight));
        assert_eq!(failure.request_id.as_deref(), Some("req_synthetic-42"));
        for private in [
            "Bearer synthetic-secret",
            "https://private.test/token",
            "req_bad\nvalue",
            &"x".repeat(129),
        ] {
            assert!(
                failure
                    .clone()
                    .with_request_id(Some(private))
                    .request_id
                    .is_none()
            );
        }
    }
}
