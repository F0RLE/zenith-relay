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
    Transient,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenRefreshFailure {
    pub kind: TokenRefreshFailureKind,
    pub code: String,
}

impl TokenRefreshFailure {
    pub fn new(kind: TokenRefreshFailureKind, code: &str) -> Self {
        Self {
            kind,
            code: safe_error_code(code),
        }
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
            TokenRefreshFailureKind::Transient => None,
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
