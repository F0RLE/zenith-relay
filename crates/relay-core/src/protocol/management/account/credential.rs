use crate::accounts::{AccountAuthState, ReauthReason};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Pending,
    NotExpired,
    Expired,
    Revoked,
    AuthFailed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRecovery {
    None,
    RefreshNextRequest,
    SignIn,
    RetryLater,
}

/// Safe local evidence only. NotExpired never means upstream authentication
/// succeeded, and recovery never changes enabled/draining/pool membership.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialDiagnostics {
    pub status: CredentialStatus,
    pub expires_at_ms: Option<u64>,
    pub has_refresh_token: bool,
    pub recovery: CredentialRecovery,
}

impl CredentialDiagnostics {
    pub fn from_local(
        auth: AccountAuthState,
        credential_available: bool,
        has_refresh_token: bool,
        expires_at_ms: Option<u64>,
        now_ms: u64,
    ) -> Self {
        let (status, recovery) = match auth {
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken) => {
                (CredentialStatus::Revoked, CredentialRecovery::SignIn)
            }
            state if state.requires_fresh_login() => {
                (CredentialStatus::AuthFailed, CredentialRecovery::SignIn)
            }
            AccountAuthState::Error => {
                (CredentialStatus::AuthFailed, CredentialRecovery::RetryLater)
            }
            AccountAuthState::Refreshing => (CredentialStatus::Pending, CredentialRecovery::None),
            _ if !credential_available => (CredentialStatus::Unknown, CredentialRecovery::SignIn),
            _ if expires_at_ms.is_some_and(|at| at <= now_ms) => (
                CredentialStatus::Expired,
                if has_refresh_token {
                    CredentialRecovery::RefreshNextRequest
                } else {
                    CredentialRecovery::SignIn
                },
            ),
            _ if expires_at_ms.is_some() => {
                (CredentialStatus::NotExpired, CredentialRecovery::None)
            }
            _ => (CredentialStatus::Unknown, CredentialRecovery::None),
        };
        Self {
            status,
            recovery,
            expires_at_ms,
            has_refresh_token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_and_auth_refusal_are_independent_and_no_secrets_are_returned() {
        let expired =
            CredentialDiagnostics::from_local(AccountAuthState::Active, true, true, Some(10), 20);
        assert_eq!(expired.status, CredentialStatus::Expired);
        assert_eq!(expired.recovery, CredentialRecovery::RefreshNextRequest);
        let invalid = CredentialDiagnostics::from_local(
            AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
            true,
            true,
            Some(60_000),
            20,
        );
        assert_eq!(invalid.status, CredentialStatus::AuthFailed);
        assert_eq!(invalid.recovery, CredentialRecovery::SignIn);
        let valid = CredentialDiagnostics::from_local(
            AccountAuthState::Active,
            true,
            false,
            Some(60_000),
            20,
        );
        assert_eq!(valid.status, CredentialStatus::NotExpired);
        assert_eq!(
            CredentialDiagnostics::from_local(AccountAuthState::Active, false, false, None, 20)
                .status,
            CredentialStatus::Unknown
        );
    }
}
